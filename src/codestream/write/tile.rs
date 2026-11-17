// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One tile being encoded, after the write side of `ojph_tile.cpp`: it
//! turns image samples into the lines its components transform (level
//! shift, NLT, colour transform).

use super::tile_comp::TileComp;
use super::Ctx;
use crate::codestream::tile::nlt_lut;
use crate::codestream::Budget;
use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::params::marker::SOD;
use crate::params::tilepart_divisions::{
    OJPH_TILEPART_COMPONENTS, OJPH_TILEPART_NO_DIVISIONS, OJPH_TILEPART_RESOLUTIONS,
};
use crate::params::{NltRec, ParamSot, ParamTlm, ProgressionOrder};
use crate::transform::colour::{
    ict_forward, irv_convert_to_float, irv_convert_to_float_nlt, irv_convert_to_float_nlt_type3,
    rct_forward, rev_convert, rev_convert_nlt_type3, rev_encode_nlt,
};
use crate::types::{div_ceil, Point, Rect, Size};
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
    pub(crate) comps: Vec<TileComp>,
    /// The converted lines of the first three components, before the
    /// colour transform; empty when it is not used.
    lines: Vec<LineBuf>,
    employ_color_transform: bool,
    reversible: Vec<bool>,
    comp_rects: Vec<Rect>,
    /// Where this tile's samples start in a line of the whole image, per
    /// component.
    line_offsets: Vec<u32>,
    num_bits: Vec<u32>,
    is_signed: Vec<bool>,
    cur_line: Vec<u32>,
    nlt: Vec<Option<NltRec>>,
    pub(crate) prog_order: ProgressionOrder,
    pub(crate) tile_idx: u16,
    /// The bytes of all the packets of the tile, for its length.
    num_bytes: u64,
}

impl Tile {
    /// Builds the tile covering `tile_rect`. `offset` is the sum of the
    /// widths of the tiles to the left of this one in its row, and is
    /// advanced by the width of this tile.
    pub(crate) fn new(
        ctx: &Ctx,
        budget: &mut Budget,
        tile_rect: Rect,
        tile_idx: u16,
        offset: &mut u32,
    ) -> Result<Self> {
        let szp = ctx.siz;
        let num_comps = szp.get_num_components();
        let n = num_comps as usize;
        let Some(prog_order) = ctx.cod.get_progression_order_enum() else {
            return ojph_error!(0x00080004, "unknown progression order");
        };
        let mut tile = Tile {
            comps: Vec::new(),
            lines: Vec::new(),
            employ_color_transform: ctx.cod.is_employing_color_transform(),
            reversible: Vec::with_capacity(n),
            comp_rects: Vec::with_capacity(n),
            line_offsets: Vec::with_capacity(n),
            num_bits: Vec::with_capacity(n),
            is_signed: Vec::with_capacity(n),
            cur_line: vec![0; n],
            nlt: Vec::with_capacity(n),
            prog_order,
            tile_idx,
            num_bytes: 0,
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
            if recon_downsamp.x == 0 || recon_downsamp.y == 0 {
                return ojph_error!(0x00070021, "a component has a subsampling factor of 0");
            }

            let tcx0 = div_ceil(tx0, downsamp.x);
            let tcy0 = div_ceil(ty0, downsamp.y);
            let tcx1 = div_ceil(tx1, downsamp.x);
            let tcy1 = div_ceil(ty1, downsamp.y);
            let recon_tcx0 = div_ceil(tx0, recon_downsamp.x);

            // tx0 - offset is where the image starts, so this cannot wrap
            // for a tile grid built by the encoder.
            tile.line_offsets.push(
                recon_tcx0.wrapping_sub(div_ceil(tx0.wrapping_sub(*offset), recon_downsamp.x)),
            );
            let comp_rect = Rect {
                org: Point::new(tcx0, tcy0),
                siz: Size::new(tcx1 - tcx0, tcy1 - tcy0),
            };
            if i < 3 {
                colour_comp_rect[i as usize] = comp_rect;
            }
            tile.comps
                .push(TileComp::new(ctx, budget, i, comp_rect, tile_rect.org)?);
            tile.comp_rects.push(comp_rect);
            width = width.max(comp_rect.siz.w);

            let num_bits = szp.get_bit_depth(i);
            let is_signed = szp.is_signed(i);
            let reversible = ctx.cod.get_coc(i).is_reversible();
            let mut nlt = ctx.nlt.get_nlt_rec(i).cloned();
            if let Some(rec) = &nlt {
                let bd = u32::from(rec.get_bit_depth());
                let is = rec.is_signed();
                let nlt_type = rec.get_type();
                if nlt_type == NLT_TYPE3 && (bd != num_bits || is != is_signed) {
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
                // type 0, and type 3 on unsigned reversible samples, change
                // nothing; push() converts both as no nonlinearity
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
            // cod.check_validity makes sure there are at least three
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
            // The forward RCT writes three lines of one width. OpenJPH
            // asserts this; components of the same depth always agree.
            let wide = |c: u32| {
                ctx.qcd
                    .get_qcc(c)
                    .propose_precision(ctx.cod.get_coc(c), ctx.cod, ctx.qcd)
                    > 32
            };
            if r[0] && (wide(0) != wide(1) || wide(1) != wide(2)) {
                return ojph_error!(
                    0x00070026,
                    "the colour components need different sample widths, so the colour transform cannot be applied"
                );
            }
            for _ in 0..3 {
                tile.lines.push(if tile.reversible[0] {
                    budget.line_i32(width as usize, 0)?
                } else {
                    budget.line_f32(width as usize, 0)?
                });
            }
        }
        Ok(tile)
    }

    /// Takes this tile's part of the next line of component `comp_num`
    /// from `line`, a line of the whole image. Returns false when the
    /// tile has all the lines of that component already.
    pub(crate) fn push(
        &mut self,
        line: &LineBuf,
        comp_num: u32,
        budget: &mut Budget,
    ) -> Result<bool> {
        let c = comp_num as usize;
        if c >= self.comps.len() {
            return ojph_error!(0x00080030, "internal error: component index out of range");
        }
        if self.cur_line[c] >= self.comp_rects[c].siz.h {
            return Ok(false);
        }
        self.cur_line[c] += 1;

        let comp_width = self.comp_rects[c].siz.w;
        let line_offset = self.line_offsets[c];
        if line_offset as u64 + comp_width as u64 > line.size as u64 || !line.is_i32() {
            return ojph_error!(
                0x00080031,
                "internal error: a tile does not fit the image line"
            );
        }

        // converts to signed representation
        // employs color transform if there is a need
        let use_lines = self.employ_color_transform && c < 3;
        let num_bits = self.num_bits[c];
        let is_signed = self.is_signed[c];
        let nlt = self.nlt[c].as_ref();
        {
            let tc: &mut LineBuf = if use_lines {
                &mut self.lines[c]
            } else {
                self.comps[c].get_line()
            };
            if self.reversible[c] {
                // num_bits is between 1 and 38
                let shift = 1i64 << (num_bits - 1);
                match nlt {
                    None => {
                        let shift = if is_signed { 0 } else { -shift };
                        rev_convert(line, line_offset, tc, 0, shift, comp_width);
                    }
                    Some(rec) if is_signed && rec.get_type() == NLT_TYPE3 => {
                        rev_convert_nlt_type3(line, line_offset, tc, 0, shift + 1, comp_width);
                    }
                    Some(rec) if rec.get_type() == NLT_TYPE2 || rec.get_type() == NLT_TYPE4 => {
                        rev_encode_nlt(
                            line,
                            line_offset,
                            tc,
                            num_bits,
                            is_signed,
                            comp_width,
                            &nlt_lut(rec),
                        );
                    }
                    Some(_) => {}
                }
            } else {
                match nlt {
                    None => {
                        irv_convert_to_float(line, line_offset, tc, num_bits, is_signed, comp_width)
                    }
                    Some(rec) if rec.get_type() == NLT_TYPE3 => irv_convert_to_float_nlt_type3(
                        line,
                        line_offset,
                        tc,
                        num_bits,
                        is_signed,
                        comp_width,
                    ),
                    Some(rec) if rec.get_type() == NLT_TYPE2 || rec.get_type() == NLT_TYPE4 => {
                        irv_convert_to_float_nlt(
                            line,
                            line_offset,
                            tc,
                            num_bits,
                            is_signed,
                            comp_width,
                            &nlt_lut(rec),
                        )
                    }
                    Some(_) => {}
                }
            }
        }

        if !use_lines {
            self.comps[c].push_line(budget)?;
        } else if c == 2 {
            let [c0, c1, c2, ..] = &mut self.comps[..] else {
                return ojph_error!(0x00080030, "internal error: component index out of range");
            };
            let [l0, l1, l2] = &self.lines[..] else {
                return ojph_error!(0x00080030, "internal error: component index out of range");
            };
            let (y, cb, cr) = (c0.get_line(), c1.get_line(), c2.get_line());
            if self.reversible[c] {
                rct_forward(l0, l1, l2, y, cb, cr, comp_width);
            } else {
                ict_forward(
                    l0.f32(),
                    l1.f32(),
                    l2.f32(),
                    y.f32_mut(),
                    cb.f32_mut(),
                    cr.f32_mut(),
                    comp_width,
                );
            }
            c0.push_line(budget)?;
            c1.push_line(budget)?;
            c2.push_line(budget)?;
        }
        Ok(true)
    }

    fn max_decompositions(&self) -> u32 {
        self.comps
            .iter()
            .map(|c| c.get_num_decompositions())
            .max()
            .unwrap_or(0)
    }

    /// Codes the packet headers of every component, and adds up the
    /// tile's length.
    pub(crate) fn prepare_for_flush(&mut self, scratch: &mut Vec<u8>) -> Result<()> {
        self.num_bytes = 0;
        for c in self.comps.iter_mut() {
            self.num_bytes += c.prepare_precincts(scratch)?;
        }
        Ok(())
    }

    /// Adds a TLM entry for every tile part of the tile.
    pub(crate) fn fill_tlm(&self, tlm: &mut ParamTlm, tilepart_div: u32) -> Result<()> {
        let mut pair = |bytes: u64| -> Result<()> {
            tlm.set_next_pair(self.tile_idx, tile_part_length(bytes)?);
            Ok(())
        };
        let max_decs = self.max_decompositions();
        let layer_first = matches!(
            self.prog_order,
            ProgressionOrder::Lrcp | ProgressionOrder::Rlcp
        );
        if tilepart_div == OJPH_TILEPART_NO_DIVISIONS {
            pair(self.num_bytes)?;
        } else if tilepart_div == OJPH_TILEPART_RESOLUTIONS {
            for r in 0..=max_decs {
                pair(self.comps.iter().map(|c| c.get_num_bytes_res(r)).sum())?;
            }
        } else if tilepart_div == OJPH_TILEPART_COMPONENTS && !layer_first {
            // CPRL; write_headers leaves no other progression with this
            for c in &self.comps {
                pair(c.get_num_bytes())?;
            }
        } else {
            // resolutions and components, LRCP or RLCP
            for r in 0..=max_decs {
                for c in &self.comps {
                    if r <= c.get_num_decompositions() {
                        pair(c.get_num_bytes_res(r))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Appends the tile's tile parts, the packets sequenced by the
    /// progression order.
    pub(crate) fn flush(&mut self, out: &mut Vec<u8>, tilepart_div: u32) -> Result<()> {
        let max_decompositions = self.max_decompositions();
        let num_comps = self.comps.len() as u32;
        let tile_idx = self.tile_idx;
        let tile_part = |out: &mut Vec<u8>, bytes: u64, tp: u32, tn: u32| -> Result<()> {
            // write tile header
            let mut sot = ParamSot::default();
            sot.init(0, tile_idx, 0, 1);
            sot.write_with_parts(out, tile_part_length(bytes)?, tp as u8, tn as u8);
            // write start of data
            out.extend_from_slice(&SOD.to_be_bytes());
            Ok(())
        };

        if tilepart_div == OJPH_TILEPART_NO_DIVISIONS {
            tile_part(out, self.num_bytes, 0, 1)?;
        }

        let comps = &mut self.comps;
        // This is how OpenJPH starts its search for the smallest point.
        let far = Point::new(i32::MAX as u32, i32::MAX as u32);
        // sequence the writing of precincts according to progression order
        match self.prog_order {
            ProgressionOrder::Lrcp | ProgressionOrder::Rlcp => {
                if tilepart_div == OJPH_TILEPART_NO_DIVISIONS {
                    for r in 0..=max_decompositions {
                        for c in comps.iter() {
                            c.write_precincts(r, out)?;
                        }
                    }
                } else if tilepart_div == OJPH_TILEPART_RESOLUTIONS {
                    for r in 0..=max_decompositions {
                        let bytes = comps.iter().map(|c| c.get_num_bytes_res(r)).sum();
                        tile_part(out, bytes, r, max_decompositions + 1)?;
                        for c in comps.iter() {
                            c.write_precincts(r, out)?;
                        }
                    }
                } else {
                    // OpenJPH counts num_comps * (max + 1) tile parts even
                    // when some components have fewer resolutions; the
                    // encoder refuses that case (0x0008000C), so every
                    // component has every resolution here
                    let num_tileparts = num_comps * (max_decompositions + 1);
                    for r in 0..=max_decompositions {
                        for (ci, c) in comps.iter().enumerate() {
                            if r <= c.get_num_decompositions() {
                                let tp = ci as u32 + r * num_comps;
                                tile_part(out, c.get_num_bytes_res(r), tp, num_tileparts)?;
                                c.write_precincts(r, out)?;
                            }
                        }
                    }
                }
            }
            ProgressionOrder::Rpcl => {
                for r in 0..=max_decompositions {
                    if tilepart_div == OJPH_TILEPART_RESOLUTIONS {
                        let bytes = comps.iter().map(|c| c.get_num_bytes_res(r)).sum();
                        tile_part(out, bytes, r, max_decompositions + 1)?;
                    }
                    loop {
                        let mut found = None;
                        let mut smallest = far;
                        for (ci, c) in comps.iter().enumerate() {
                            let Some(cur) = c.get_top_left_precinct(r) else {
                                continue;
                            };
                            if found.is_none() {
                                found = Some(0);
                            }
                            if cur.y < smallest.y || (cur.y == smallest.y && cur.x < smallest.x) {
                                smallest = cur;
                                found = Some(ci);
                            }
                        }
                        match found {
                            Some(ci) => comps[ci].write_one_precinct(r, out)?,
                            None => break,
                        }
                    }
                }
            }
            ProgressionOrder::Pcrl => loop {
                let mut found = None;
                let (mut comp_num, mut res_num) = (0usize, 0u32);
                let mut smallest = far;
                for (ci, c) in comps.iter().enumerate() {
                    for r in 0..=c.get_num_decompositions() {
                        let Some(cur) = c.get_top_left_precinct(r) else {
                            continue;
                        };
                        found = Some(());
                        let same = cur.y == smallest.y && cur.x == smallest.x;
                        if cur.y < smallest.y
                            || (cur.y == smallest.y && cur.x < smallest.x)
                            || (same && ci < comp_num)
                            || (same && ci == comp_num && r < res_num)
                        {
                            smallest = cur;
                            comp_num = ci;
                            res_num = r;
                        }
                    }
                }
                if found.is_none() {
                    break;
                }
                comps[comp_num].write_one_precinct(res_num, out)?;
            },
            ProgressionOrder::Cprl => {
                for (ci, c) in comps.iter_mut().enumerate() {
                    if tilepart_div == OJPH_TILEPART_COMPONENTS {
                        tile_part(out, c.get_num_bytes(), ci as u32, num_comps)?;
                    }
                    loop {
                        let mut found = false;
                        let mut res_num = 0;
                        let mut smallest = far;
                        for r in 0..=max_decompositions {
                            // does the resolution exist?
                            let Some(cur) = c.get_top_left_precinct(r) else {
                                continue;
                            };
                            found = true;
                            if cur.y < smallest.y || (cur.y == smallest.y && cur.x < smallest.x) {
                                smallest = cur;
                                res_num = r;
                            }
                        }
                        if !found {
                            break;
                        }
                        c.write_one_precinct(res_num, out)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A tile part's length as SOT and TLM carry it: 32 bits including the
/// 14 bytes of SOT and SOD. OpenJPH lets a longer one wrap.
fn tile_part_length(bytes: u64) -> Result<u32> {
    if bytes > (u32::MAX - 14) as u64 {
        return ojph_error!(
            0x00080060,
            "a tile part of {} bytes is longer than SOT can describe; use tile parts or smaller tiles",
            bytes
        );
    }
    Ok(bytes as u32)
}
