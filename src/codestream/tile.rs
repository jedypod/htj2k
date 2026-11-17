// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One tile, after `ojph_tile.cpp`: it sequences the packets of its tile
//! parts to the right precincts, and turns the lines of its components
//! back into image samples (colour transform, level shift, NLT).

use crate::codestream::bitbuffer_read::ParseResult;
use crate::codestream::tile_comp::TileComp;
use crate::codestream::{Budget, Build};
use crate::error::ojph_error;
use crate::file::{MemInfile, SeekFrom};
use crate::line::LineBuf;
use crate::params::{NltRec, ParamSot, ProgressionOrder};
use crate::transform::colour::{
    ict_backward, irv_convert_to_integer, irv_convert_to_integer_nlt,
    irv_convert_to_integer_nlt_type3, rct_backward, rev_convert, rev_convert_nlt_type3, NltLut,
};
use crate::types::{div_ceil, Point, Rect};
use crate::Result;

/// `OJPH_NLT_NO_NLT`
const NLT_TYPE0: u8 = 0;
/// `OJPH_NLT_LUT_STYLE_NLT`
const NLT_TYPE2: u8 = 2;
/// `OJPH_NLT_BINARY_COMPLEMENT_NLT`
const NLT_TYPE3: u8 = 3;
/// `OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT`
const NLT_TYPE4: u8 = 4;

#[derive(Debug)]
pub(crate) struct Tile {
    comps: Vec<TileComp>,
    /// The colour-transformed lines of the first three components; empty
    /// when the colour transform is not used.
    lines: Vec<LineBuf>,
    employ_color_transform: bool,
    resilient: bool,
    reversible: Vec<bool>,
    recon_comp_rects: Vec<Rect>,
    /// Where this tile's samples start in a line of the whole image, per
    /// component.
    line_offsets: Vec<u32>,
    skipped_res_for_read: u32,
    num_bits: Vec<u32>,
    is_signed: Vec<bool>,
    cur_line: Vec<u32>,
    nlt: Vec<Option<NltRec>>,
    prog_order: ProgressionOrder,
    next_tile_part: u32,
}

impl Tile {
    /// Builds the tile covering `tile_rect`. `offset` is the sum of the
    /// widths of the tiles to the left of this one in its row, and is
    /// advanced by the width of this tile.
    pub(crate) fn new(
        build: &Build,
        budget: &mut Budget,
        tile_rect: Rect,
        offset: &mut u32,
    ) -> Result<Self> {
        let szp = build.siz;
        let num_comps = szp.get_num_components();
        let n = num_comps as usize;

        let Some(prog_order) = build.cod.get_progression_order_enum() else {
            return ojph_error!(0x00070025, "unknown progression order");
        };
        let mut tile = Tile {
            comps: Vec::new(),
            lines: Vec::new(),
            employ_color_transform: build.cod.is_employing_color_transform(),
            resilient: build.resilient,
            reversible: Vec::with_capacity(n),
            recon_comp_rects: Vec::with_capacity(n),
            line_offsets: Vec::with_capacity(n),
            skipped_res_for_read: build.skipped_res_for_read,
            num_bits: Vec::with_capacity(n),
            is_signed: Vec::with_capacity(n),
            cur_line: vec![0; n],
            nlt: Vec::with_capacity(n),
            prog_order,
            next_tile_part: 0,
        };
        budget.take(n as u64 * std::mem::size_of::<TileComp>() as u64)?;
        tile.comps.reserve_exact(n);

        let tx0 = tile_rect.org.x;
        let ty0 = tile_rect.org.y;
        let tx1 = tile_rect.org.x + tile_rect.siz.w;
        let ty1 = tile_rect.org.y + tile_rect.siz.h;

        let mut width = 0;
        let mut colour_comp_rect = [Rect::default(); 3];
        for i in 0..num_comps {
            let downsamp = szp.get_downsampling(i);
            let recon_downsamp = szp.get_recon_downsampling(i);
            if downsamp.x == 0 || downsamp.y == 0 || recon_downsamp.x == 0 || recon_downsamp.y == 0
            {
                return ojph_error!(0x00070021, "a component has a subsampling factor of 0");
            }

            let tcx0 = div_ceil(tx0, downsamp.x);
            let tcy0 = div_ceil(ty0, downsamp.y);
            let tcx1 = div_ceil(tx1, downsamp.x);
            let tcy1 = div_ceil(ty1, downsamp.y);
            let recon_tcx0 = div_ceil(tx0, recon_downsamp.x);
            let recon_tcy0 = div_ceil(ty0, recon_downsamp.y);
            let recon_tcx1 = div_ceil(tx1, recon_downsamp.x);
            let recon_tcy1 = div_ceil(ty1, recon_downsamp.y);

            // tx0 - offset is where the image starts, so this cannot wrap
            // for a tile grid built by Codestream.
            tile.line_offsets.push(
                recon_tcx0.wrapping_sub(div_ceil(tx0.wrapping_sub(*offset), recon_downsamp.x)),
            );
            let comp_rect = Rect {
                org: Point::new(tcx0, tcy0),
                siz: crate::types::Size::new(tcx1 - tcx0, tcy1 - tcy0),
            };
            let recon_comp_rect = Rect {
                org: Point::new(recon_tcx0, recon_tcy0),
                siz: crate::types::Size::new(recon_tcx1 - recon_tcx0, recon_tcy1 - recon_tcy0),
            };
            if i < 3 {
                colour_comp_rect[i as usize] = comp_rect;
            }

            tile.comps.push(TileComp::new(
                build,
                budget,
                i,
                comp_rect,
                recon_comp_rect,
                tile_rect.org,
            )?);
            tile.recon_comp_rects.push(recon_comp_rect);
            width = width.max(recon_comp_rect.siz.w);

            let num_bits = szp.get_bit_depth(i);
            let is_signed = szp.is_signed(i);
            let reversible = build.cod.get_coc(i).is_reversible();
            let mut nlt = build.nlt.get_nlt_rec(i).cloned();
            if let Some(rec) = &nlt {
                let bd = u32::from(rec.get_bit_depth());
                let is = rec.is_signed();
                if rec.get_type() == NLT_TYPE3 && (bd != num_bits || is != is_signed) {
                    return ojph_error!(
                        0x000300A1,
                        "Mismatch between Ssiz (bit_depth = {}, is_signed = {}) from SIZ marker segment, and BDnlt (bit_depth = {}, is_signed = {}) from NLT marker segment, for component {}",
                        num_bits,
                        if is_signed { "True" } else { "False" },
                        bd,
                        if is { "True" } else { "False" },
                        i
                    );
                }
                // Type 0, and type 3 on unsigned reversible samples, change
                // nothing; `pull` converts both as no nonlinearity. OpenJPH
                // 0.33.0 leaves the line unwritten for them, which was
                // fixed after that release (OpenJPH #421).
                let nlt_type = rec.get_type();
                if nlt_type == NLT_TYPE0 || (nlt_type == NLT_TYPE3 && !is_signed && reversible) {
                    nlt = None;
                }
            }
            tile.num_bits.push(num_bits);
            tile.is_signed.push(is_signed);
            tile.nlt.push(nlt);
            tile.reversible.push(reversible);
        }

        *offset = offset.wrapping_add(tile_rect.siz.w);

        if tile.employ_color_transform {
            // The marker segments allow the colour transform with fewer
            // than three components; `pull` then ignores it for a single
            // component, and OpenJPH asserts for two.
            if num_comps == 2 {
                return ojph_error!(
                    0x00070022,
                    "the colour transform cannot be used with two components"
                );
            }
            if num_comps >= 3 {
                let r = &tile.reversible;
                if r[0] != r[1] || r[1] != r[2] {
                    let name = |b: bool| if b { "reversible" } else { "irreversible" };
                    return ojph_error!(
                        0x000300A2,
                        "When the colour transform is employed, all colour components must undergo either reversible or irreversible wavelet transform; if not, then it is not clear what colour transform should be used (reversible or irreversible).  Here we found that the first three colour components uses {}, {}, and {} transforms, respectively.",
                        name(r[0]),
                        name(r[1]),
                        name(r[2])
                    );
                }
                let c = &colour_comp_rect;
                if c[0] != c[1] || c[1] != c[2] {
                    return ojph_error!(
                        0x000300A3,
                        "When the colour transform is employed, the first three colour components must have the same rectangle; i.e., the same origin on the canvas and the same width and height. The first three components have the following origin-size (x,y)-(w,h) values. Component 0 ({},{})-({},{}), Component 1 ({},{})-({},{}), Component 2 ({},{})-({},{})",
                        c[0].org.x, c[0].org.y, c[0].siz.w, c[0].siz.h,
                        c[1].org.x, c[1].org.y, c[1].siz.w, c[1].siz.h,
                        c[2].org.x, c[2].org.y, c[2].siz.w, c[2].siz.h
                    );
                }
                // The inverse RCT takes three lines of one width. OpenJPH
                // asserts this; components of the same depth always agree.
                let wide = |c: u32| {
                    build.qcd.get_qcc(c).propose_precision(
                        build.cod.get_coc(c),
                        build.cod,
                        build.qcd,
                    ) > 32
                };
                if r[0] && (wide(0) != wide(1) || wide(1) != wide(2)) {
                    return ojph_error!(
                        0x00070026,
                        "the colour components need different sample widths, so the colour transform cannot be applied"
                    );
                }
                // The reversible lines are 32 bits wide even when the
                // components are 64: the inverse RCT narrows its results.
                for _ in 0..3 {
                    tile.lines.push(if tile.reversible[0] {
                        budget.line_i32(width as usize, 0)?
                    } else {
                        budget.line_f32(width as usize, 0)?
                    });
                }
            }
        }
        Ok(tile)
    }

    /// Writes this tile's part of the next line of component `comp_num`
    /// into `tgt_line`, a line of the whole image. Returns false when the
    /// tile has no more lines for that component. `data` is the whole
    /// codestream.
    pub(crate) fn pull(
        &mut self,
        tgt_line: &mut LineBuf,
        comp_num: u32,
        data: &[u8],
    ) -> Result<bool> {
        let c = comp_num as usize;
        if c >= self.comps.len() {
            return ojph_error!(0x00070023, "internal error: component index out of range");
        }
        if self.cur_line[c] >= self.recon_comp_rects[c].siz.h {
            return Ok(false);
        }
        self.cur_line[c] += 1;

        let comp_width = self.recon_comp_rects[c].siz.w;
        if comp_width == 0 {
            return Ok(true); // nothing to pull, but not an error
        }
        let line_offset = self.line_offsets[c];
        if line_offset as u64 + comp_width as u64 > tgt_line.size as u64 {
            return ojph_error!(
                0x00070024,
                "internal error: a tile does not fit the image line"
            );
        }

        let num_comps = self.comps.len();
        let use_lines = self.employ_color_transform && num_comps != 1 && c < 3;
        if use_lines && c == 0 {
            let (comps, lines) = (&mut self.comps, &mut self.lines);
            let [c0, c1, c2, ..] = &mut comps[..] else {
                return ojph_error!(0x00070023, "internal error: component index out of range");
            };
            let [l0, l1, l2] = &mut lines[..] else {
                return ojph_error!(0x00070023, "internal error: component index out of range");
            };
            let y = c0.pull_line(data)?;
            let cb = c1.pull_line(data)?;
            let cr = c2.pull_line(data)?;
            if self.reversible[c] {
                rct_backward(y, cb, cr, l0, l1, l2, comp_width);
            } else {
                ict_backward(
                    y.f32(),
                    cb.f32(),
                    cr.f32(),
                    l0.f32_mut(),
                    l1.f32_mut(),
                    l2.f32_mut(),
                    comp_width,
                );
            }
        }

        let src_line: &LineBuf = if use_lines {
            &self.lines[c]
        } else {
            self.comps[c].pull_line(data)?
        };
        let num_bits = self.num_bits[c];
        let is_signed = self.is_signed[c];
        let nlt = self.nlt[c].as_ref();
        if self.reversible[c] {
            // num_bits is between 1 and 38
            let shift = 1i64 << (num_bits - 1);
            match nlt {
                None => {
                    let shift = if is_signed { 0 } else { shift };
                    rev_convert(src_line, 0, tgt_line, line_offset, shift, comp_width);
                }
                Some(rec) if is_signed && rec.get_type() == NLT_TYPE3 => {
                    rev_convert_nlt_type3(
                        src_line,
                        0,
                        tgt_line,
                        line_offset,
                        shift + 1,
                        comp_width,
                    );
                }
                // A LUT with the reversible transform is turned away when
                // the headers are read; OpenJPH leaves the line untouched
                // for anything else.
                Some(_) => {}
            }
        } else {
            match nlt {
                None => irv_convert_to_integer(
                    src_line,
                    tgt_line,
                    line_offset,
                    num_bits,
                    is_signed,
                    comp_width,
                ),
                Some(rec) if rec.get_type() == NLT_TYPE3 => irv_convert_to_integer_nlt_type3(
                    src_line,
                    tgt_line,
                    line_offset,
                    num_bits,
                    is_signed,
                    comp_width,
                ),
                Some(rec) if rec.get_type() == NLT_TYPE2 || rec.get_type() == NLT_TYPE4 => {
                    irv_convert_to_integer_nlt(
                        src_line,
                        tgt_line,
                        line_offset,
                        num_bits,
                        is_signed,
                        comp_width,
                        &nlt_lut(rec),
                    )
                }
                Some(_) => {}
            }
        }
        Ok(true)
    }

    /// Parses the packets of one tile part. `file` is just past the SOD
    /// marker, and is left at the end of the tile part.
    pub(crate) fn parse_tile_header(
        &mut self,
        sot: &ParamSot,
        scratch: &mut Vec<u8>,
        file: &mut MemInfile,
        tile_start_location: u64,
    ) -> Result<()> {
        if u32::from(sot.get_tile_part_index()) != self.next_tile_part && !self.resilient {
            return ojph_error!(0x00030091, "wrong tile part index");
        }
        self.next_tile_part += 1;

        // tile_end_location is used on failure
        let payload_length = sot.get_payload_length();
        let tile_end_location = tile_start_location + payload_length as u64;

        // bytes left to parse; like OpenJPH, this wraps if the tile part
        // header is longer than the tile part claims to be, and the
        // packets then run to wherever they end
        let data_left =
            payload_length.wrapping_sub((file.tell() as u64 - tile_start_location) as u32);
        if data_left == 0 {
            return Ok(());
        }

        if let Err(error) = self.parse_packets(scratch, data_left, file) {
            if !self.resilient {
                return ojph_error!(0x00030092, "{}", error);
            }
        }
        file.seek(tile_end_location as i64, SeekFrom::Start);
        Ok(())
    }

    /// Sequences the reading of precincts according to progression order.
    fn parse_packets(
        &mut self,
        scratch: &mut Vec<u8>,
        mut data_left: u32,
        file: &mut MemInfile,
    ) -> ParseResult<()> {
        let comps = &mut self.comps;
        let mut max_decompositions = comps
            .iter()
            .map(|c| c.get_num_decompositions())
            .max()
            .unwrap_or(0);
        // This is how OpenJPH starts its search for the smallest point,
        // so a precinct at or beyond it is never the smallest.
        let far = Point::new(i32::MAX as u32, i32::MAX as u32);

        match self.prog_order {
            ProgressionOrder::Lrcp | ProgressionOrder::Rlcp => {
                max_decompositions = max_decompositions.saturating_sub(self.skipped_res_for_read);
                for r in 0..=max_decompositions {
                    for comp in comps.iter_mut() {
                        if data_left > 0 {
                            comp.parse_precincts(r, scratch, &mut data_left, file)?;
                        }
                    }
                }
            }
            ProgressionOrder::Rpcl => {
                max_decompositions = max_decompositions.saturating_sub(self.skipped_res_for_read);
                for r in 0..=max_decompositions {
                    loop {
                        let mut found = false;
                        let mut comp_num = 0;
                        let mut smallest = far;
                        for (c, comp) in comps.iter_mut().enumerate() {
                            let Some(cur) = comp.get_top_left_precinct(r) else {
                                continue;
                            };
                            found = true;
                            if cur.y < smallest.y || (cur.y == smallest.y && cur.x < smallest.x) {
                                smallest = cur;
                                comp_num = c;
                            }
                        }
                        if found && data_left > 0 {
                            comps[comp_num].parse_one_precinct(r, scratch, &mut data_left, file)?;
                        } else {
                            break;
                        }
                    }
                }
            }
            ProgressionOrder::Pcrl => loop {
                let mut found = false;
                let mut comp_num = 0;
                let mut res_num = 0;
                let mut smallest = far;
                for (c, comp) in comps.iter_mut().enumerate() {
                    for r in 0..=comp.get_num_decompositions() {
                        let Some(cur) = comp.get_top_left_precinct(r) else {
                            continue;
                        };
                        found = true;
                        let same = cur.y == smallest.y && cur.x == smallest.x;
                        if cur.y < smallest.y
                            || (cur.y == smallest.y && cur.x < smallest.x)
                            || (same && c < comp_num)
                            || (same && c == comp_num && r < res_num)
                        {
                            smallest = cur;
                            comp_num = c;
                            res_num = r;
                        }
                    }
                }
                if found && data_left > 0 {
                    comps[comp_num].parse_one_precinct(res_num, scratch, &mut data_left, file)?;
                } else {
                    break;
                }
            },
            ProgressionOrder::Cprl => {
                for comp in comps.iter_mut() {
                    loop {
                        let mut found = false;
                        let mut res_num = 0;
                        let mut smallest = far;
                        for r in 0..=max_decompositions {
                            // does the resolution exist?
                            let Some(cur) = comp.get_top_left_precinct(r) else {
                                continue;
                            };
                            found = true;
                            if cur.y < smallest.y || (cur.y == smallest.y && cur.x < smallest.x) {
                                smallest = cur;
                                res_num = r;
                            }
                        }
                        if found && data_left > 0 {
                            comp.parse_one_precinct(res_num, scratch, &mut data_left, file)?;
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// The view of an NLT record that the LUT kernels take.
pub(crate) fn nlt_lut(rec: &NltRec) -> NltLut<'_> {
    fn from<T>(table: &[T], offset: usize) -> &[T] {
        table.get(offset..).unwrap_or(&[])
    }
    NltLut {
        nlt_type: rec.get_type(),
        bit_depth: rec.get_bit_depth(),
        is_signed: rec.is_signed(),
        multiplier: rec.multiplier,
        fd_min: rec.fd_min,
        fd_max: rec.fd_max,
        delta: rec.delta,
        inv_delta: rec.inv_delta,
        dec_points: from(&rec.dec_points, NltRec::DEC_POINTS_OFFSET),
        ft_min: rec.ft_min,
        ft_max: rec.ft_max,
        approx_encoding_nlt: rec.approx_encoding_nlt,
        approx_enc_points: from(&rec.approx_enc_points, NltRec::ENC_POINTS_OFFSET),
        precise_max_steps: rec.precise_max_steps,
        precise_enc_dec_indices: from(&rec.precise_enc_dec_indices, NltRec::ENC_POINTS_OFFSET),
        precise_dec_lut: from(&rec.precise_dec_lut, NltRec::ENC_POINTS_OFFSET),
        num_points: rec.num_points,
    }
}
