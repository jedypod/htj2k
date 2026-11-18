// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One resolution level of a tile component, after `ojph_resolution.cpp`.
//!
//! A resolution owns the subbands that, together with the next lower
//! resolution, make it up, and runs the inverse wavelet transform one line
//! at a time.

use crate::codestream::precinct::Precinct;
use crate::codestream::subband::{ceil_shift, Subband};
use crate::codestream::{Budget, Build};
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::line::{LineBuf, LineData};
use crate::params::{DfsDwtType, ParamAtk};
use crate::transform::wavelet::{
    irv_horz_syn, irv_vert_step, irv_vert_times_k, rev_horz_syn, rev_vert_step,
};
use crate::types::{Point, Rect, Size};
use crate::Result;

/// Horizontal transform.
const HORZ_TRX: u32 = 0x01;
/// Vertical transform.
const VERT_TRX: u32 = 0x02;

/// A line of the lifting state, by index into `Resolution::lines`, and
/// whether it currently holds data.
#[derive(Debug, Clone, Copy, Default)]
struct LiftingBuf {
    line: usize,
    active: bool,
}

#[derive(Debug)]
pub(crate) struct Resolution {
    reversible: bool,
    skipped_res_for_read: bool,
    skipped_res_for_recon: bool,
    num_steps: u32,
    res_num: u32,
    res_rect: Rect,
    /// Used to store lines: one per lifting step, then sig and aug.
    lines: Vec<LineBuf>,
    /// Step state.
    ssp: Vec<LiftingBuf>,
    sig: LiftingBuf,
    aug: LiftingBuf,
    bands: [Subband; 4],
    child_res: Option<Box<Resolution>>,
    precincts: Vec<Precinct>,
    num_precincts: Size,
    /// Used for the spatial progression orders.
    cur_precinct_loc: Point,
    atk: ParamAtk,
    transform_flags: u32,
    cur_line: u32,
    vert_even: bool,
    horz_even: bool,
}

impl Resolution {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        build: &Build,
        budget: &mut Budget,
        res_rect: Rect,
        recon_res_rect: Rect,
        comp_num: u32,
        res_num: u32,
        res_downsamp: Point,
        tile_top_left: Point,
    ) -> Result<Self> {
        budget.take(std::mem::size_of::<Resolution>() as u64)?;
        let cdp = build.cod.get_coc(comp_num);
        let num_decomps = cdp.get_num_decompositions();
        // res_num never exceeds num_decomps, and the number of skipped
        // resolutions is validated against it
        let skipped_res_for_recon = res_num + build.skipped_res_for_recon > num_decomps;
        let skipped_res_for_read = res_num + build.skipped_res_for_read > num_decomps;
        let atk = cdp.access_atk().clone();

        let mut ds = DfsDwtType::BidirDwt;
        if cdp.is_dfs_defined() {
            let Some(dfs) = build.dfs else {
                return ojph_error!(
                    0x00070011,
                    "There is a problem with codestream                      marker segments. COD/COC specifies the use of a DFS marker                      but there are no DFS markers within the main codestream                      headers"
                );
            };
            let dfs_idx = cdp.get_dfs_index();
            let Some(dfs) = dfs.get_dfs(dfs_idx) else {
                return ojph_error!(
                    0x00070012,
                    "There is a problem with codestream                      marker segments. COD/COC specifies the use of a DFS marker                      with index {}, but there are no such marker within the                      main codestream headers",
                    dfs_idx
                );
            };
            ds = dfs.get_dwt_type(num_decomps - res_num + 1);
        }

        let mut transform_flags = 0;
        if res_num > 0 {
            transform_flags = match ds {
                DfsDwtType::BidirDwt => HORZ_TRX | VERT_TRX,
                DfsDwtType::HorzDwt => HORZ_TRX,
                DfsDwtType::VertDwt => VERT_TRX,
                DfsDwtType::NoDwt => 0,
            };
        }
        let has_horz = transform_flags & HORZ_TRX != 0;
        let has_vert = transform_flags & VERT_TRX != 0;

        // Coordinates are widened so that the +1 below cannot overflow.
        let trx0 = res_rect.org.x as u64;
        let try0 = res_rect.org.y as u64;
        let trx1 = trx0 + res_rect.siz.w as u64;
        let try1 = try0 + res_rect.siz.h as u64;

        let mut bands = [
            Subband::absent(),
            Subband::absent(),
            Subband::absent(),
            Subband::absent(),
        ];
        let mut child_res = None;
        if res_num > 0 {
            let mut child = |budget: &mut Budget, re: Rect, down: Point| -> Result<()> {
                child_res = Some(Box::new(Resolution::new(
                    build,
                    budget,
                    re,
                    if skipped_res_for_recon {
                        recon_res_rect
                    } else {
                        re
                    },
                    comp_num,
                    res_num - 1,
                    down,
                    tile_top_left,
                )?));
                Ok(())
            };
            match ds {
                DfsDwtType::BidirDwt => {
                    for i in 0..4u64 {
                        let tbx0 = (trx0 + 1 - (i & 1)) >> 1;
                        let tbx1 = (trx1 + 1 - (i & 1)) >> 1;
                        let tby0 = (try0 + 1 - (i >> 1)) >> 1;
                        let tby1 = (try1 + 1 - (i >> 1)) >> 1;
                        let re = Rect {
                            org: Point::new(tbx0 as u32, tby0 as u32),
                            siz: Size::new((tbx1 - tbx0) as u32, (tby1 - tby0) as u32),
                        };
                        if i == 0 {
                            let down = Point::new(
                                res_downsamp.x.wrapping_mul(2),
                                res_downsamp.y.wrapping_mul(2),
                            );
                            child(budget, re, down)?;
                        } else {
                            bands[i as usize] = Subband::new(
                                build, budget, re, comp_num, res_num, i as u32, has_horz, has_vert,
                            )?;
                        }
                    }
                }
                DfsDwtType::VertDwt => {
                    let mut re = res_rect;
                    let tby0 = (try0 + 1) >> 1;
                    let tby1 = (try1 + 1) >> 1;
                    re.org.y = tby0 as u32;
                    re.siz.h = (tby1 - tby0) as u32;
                    let down = Point::new(res_downsamp.x, res_downsamp.y.wrapping_mul(2));
                    child(budget, re, down)?;
                    let tby0 = try0 >> 1;
                    let tby1 = try1 >> 1;
                    re.org.y = tby0 as u32;
                    re.siz.h = (tby1 - tby0) as u32;
                    bands[2] =
                        Subband::new(build, budget, re, comp_num, res_num, 2, has_horz, has_vert)?;
                }
                DfsDwtType::HorzDwt => {
                    let mut re = res_rect;
                    let tbx0 = (trx0 + 1) >> 1;
                    let tbx1 = (trx1 + 1) >> 1;
                    re.org.x = tbx0 as u32;
                    re.siz.w = (tbx1 - tbx0) as u32;
                    let down = Point::new(res_downsamp.x.wrapping_mul(2), res_downsamp.y);
                    child(budget, re, down)?;
                    let tbx0 = trx0 >> 1;
                    let tbx1 = trx1 >> 1;
                    re.org.x = tbx0 as u32;
                    re.siz.w = (tbx1 - tbx0) as u32;
                    bands[1] =
                        Subband::new(build, budget, re, comp_num, res_num, 1, has_horz, has_vert)?;
                }
                DfsDwtType::NoDwt => {
                    child(budget, res_rect, res_downsamp)?;
                }
            }
        } else {
            bands[0] = Subband::new(build, budget, res_rect, comp_num, res_num, 0, false, false)?;
        }

        let log_pp = cdp.get_log_precinct_size(res_num);
        let mut num_precincts = Size::default();
        let mut precincts = Vec::new();
        if trx0 != trx1 && try0 != try1 {
            num_precincts.w = ceil_shift(trx1 as u32, log_pp.w) - (trx0 >> log_pp.w) as u32;
            num_precincts.h = ceil_shift(try1 as u32, log_pp.h) - (try0 >> log_pp.h) as u32;
            precincts = budget.alloc::<Precinct>(num_precincts.area() as usize)?;
        }
        // precincts will be used in full or not at all
        let x_lower_bound = ((trx0 >> log_pp.w) << log_pp.w) as u32;
        let y_lower_bound = ((try0 >> log_pp.h) << log_pp.h) as u32;
        let mut pp = precincts.iter_mut();
        for y in 0..num_precincts.h {
            let ppy0 = y_lower_bound.wrapping_add(y << log_pp.h);
            for x in 0..num_precincts.w {
                let ppx0 = x_lower_bound.wrapping_add(x << log_pp.w);
                let p = pp
                    .next()
                    .expect("precincts has num_precincts.area() elements");
                let mut t = Point::new(
                    res_downsamp.x.wrapping_mul(ppx0),
                    res_downsamp.y.wrapping_mul(ppy0),
                );
                t.x = t.x.max(tile_top_left.x);
                t.y = t.y.max(tile_top_left.y);
                p.img_point = t;
                p.may_use_sop = cdp.packets_may_use_sop();
                p.uses_eph = cdp.packets_use_eph();
            }
        }
        for band in bands.iter() {
            if !band.empty {
                band.get_cb_indices(num_precincts, &mut precincts, res_rect, has_horz, has_vert);
            }
        }

        let mut res = Resolution {
            reversible: atk.is_reversible(),
            skipped_res_for_read,
            skipped_res_for_recon,
            num_steps: atk.get_num_steps(),
            res_num,
            res_rect,
            lines: Vec::new(),
            ssp: Vec::new(),
            sig: LiftingBuf::default(),
            aug: LiftingBuf::default(),
            bands,
            child_res,
            precincts,
            num_precincts,
            cur_precinct_loc: Point::new(0, 0),
            atk,
            transform_flags,
            cur_line: 0,
            vert_even: false,
            horz_even: false,
        };

        if !skipped_res_for_recon {
            let num_steps = res.num_steps as usize;
            let qp = build.qcd.get_qcc(comp_num);
            let precision = qp.propose_precision(cdp, build.cod, build.qcd);
            let width = res_rect.siz.w as usize + 1;
            for _ in 0..num_steps + 2 {
                res.lines.push(if res.reversible {
                    if precision <= 32 {
                        budget.line_i32(width, 1)?
                    } else {
                        budget.line_i64(width, 1)?
                    }
                } else {
                    budget.line_f32(width, 1)?
                });
            }
            res.ssp = vec![LiftingBuf::default(); num_steps];
            res.horz_even = (res_rect.org.x & 1) == 0;
        }
        Ok(res)
    }

    /// Sets every sample of the buffers of this resolution and the ones
    /// below it to the bit pattern `bits`.
    #[cfg(test)]
    pub(crate) fn fill_buffers(&mut self, bits: u32) {
        for line in self.lines.iter_mut() {
            line.fill_bits(bits);
        }
        for band in self.bands.iter_mut() {
            band.fill_buffers(bits);
        }
        if let Some(child) = self.child_res.as_deref_mut() {
            child.fill_buffers(bits);
        }
    }

    /// Returns this resolution, the ones below it and their subbands to
    /// the state `new` leaves them in, ready for another codestream of
    /// the same parameters. Everything that decoding a codestream
    /// changes is set here and only here; the line and code-block
    /// buffers are kept as they are, since every sample of them is
    /// written before it is read.
    pub(crate) fn reset(&mut self) {
        self.cur_line = 0;
        self.cur_precinct_loc = Point::new(0, 0);
        if self.skipped_res_for_recon {
            self.sig = LiftingBuf::default();
            self.aug = LiftingBuf::default();
            self.vert_even = false;
        } else {
            let num_steps = self.ssp.len();
            for (i, s) in self.ssp.iter_mut().enumerate() {
                *s = LiftingBuf {
                    line: i,
                    active: false,
                };
            }
            self.sig = LiftingBuf {
                line: num_steps,
                active: false,
            };
            self.aug = LiftingBuf {
                line: num_steps + 1,
                active: false,
            };
            self.vert_even = (self.res_rect.org.y & 1) == 0;
        }
        for band in self.bands.iter_mut() {
            band.reset();
        }
        if let Some(child) = self.child_res.as_deref_mut() {
            child.reset();
        }
    }

    pub(crate) fn next_resolution(&mut self) -> Option<&mut Resolution> {
        self.child_res.as_deref_mut()
    }

    /// Produces the next line of this resolution. `data` is the whole
    /// codestream, from which code blocks are decoded as they are needed.
    pub(crate) fn pull_line(&mut self, data: &[u8]) -> Result<&mut LineBuf> {
        if self.res_num == 0 {
            return self.bands[0].pull_line(data);
        }
        if self.skipped_res_for_recon {
            return match self.child_res.as_mut() {
                Some(child) => child.pull_line(data),
                None => ojph_error!(0x00070013, "internal error: missing resolution"),
            };
        }
        if self.res_rect.siz.w == 0 {
            // OpenJPH returns a null pointer here
            return Ok(&mut self.lines[self.aug.line]);
        }
        let idx = self.produce_line(data)?;
        Ok(&mut self.lines[idx])
    }

    /// Runs the synthesis until a line is ready, and returns its index in
    /// `self.lines`.
    // kept out of `pull_line`, whose fast path (a line already made) is
    // then small enough to inline; LLVM otherwise picks either layout
    #[inline(never)]
    fn produce_line(&mut self, data: &[u8]) -> Result<usize> {
        let width = self.res_rect.siz.w;
        let height = self.res_rect.siz.h;
        let reversible = self.reversible;
        if self.transform_flags & VERT_TRX == 0 {
            self.horz_synthesis(self.aug.line, true, data)?;
            return Ok(self.aug.line);
        }
        if height > 1 {
            if self.sig.active {
                self.sig.active = false;
                return Ok(self.sig.line);
            }
            let mut idle = 0;
            loop {
                if self.cur_line < height {
                    if self.vert_even {
                        self.horz_synthesis(self.aug.line, true, data)?;
                        self.aug.active = true;
                        self.vert_even = !self.vert_even;
                        self.cur_line += 1;
                        if !reversible {
                            let k = self.atk.get_k();
                            irv_vert_times_k(k, &mut self.lines[self.aug.line], width);
                        }
                        continue;
                    } else {
                        self.horz_synthesis(self.sig.line, false, data)?;
                        self.sig.active = true;
                        self.vert_even = !self.vert_even;
                        self.cur_line += 1;
                        if !reversible {
                            let k_inv = 1.0 / self.atk.get_k();
                            irv_vert_times_k(k_inv, &mut self.lines[self.sig.line], width);
                        }
                    }
                } else {
                    // Only what is still in the lifting state can come out.
                    // OpenJPH would spin here if asked for too many lines.
                    idle += 1;
                    if idle > self.num_steps + 2 {
                        return ojph_error!(
                            0x00070014,
                            "internal error: pulled past the end of a resolution"
                        );
                    }
                }

                for i in 0..self.num_steps as usize {
                    if self.aug.active && (self.sig.active || self.ssp[i].active) {
                        let sp1 = if self.sig.active {
                            self.sig.line
                        } else {
                            self.ssp[i].line
                        };
                        let sp2 = if self.ssp[i].active {
                            self.ssp[i].line
                        } else {
                            self.sig.line
                        };
                        let s = self.atk.get_step(i as u32);
                        // take the destination out so the sources can be
                        // borrowed from the same vector
                        let mut dp = std::mem::take(&mut self.lines[self.aug.line]);
                        if reversible {
                            rev_vert_step(
                                s,
                                &self.lines[sp1],
                                &self.lines[sp2],
                                &mut dp,
                                width,
                                true,
                            );
                        } else {
                            irv_vert_step(
                                s,
                                &self.lines[sp1],
                                &self.lines[sp2],
                                &mut dp,
                                width,
                                true,
                            );
                        }
                        self.lines[self.aug.line] = dp;
                    }
                    let t = self.aug;
                    self.aug = self.ssp[i];
                    self.ssp[i] = self.sig;
                    self.sig = t;
                }

                if self.aug.active {
                    self.aug.active = false;
                    return Ok(self.aug.line);
                }
                if self.sig.active {
                    self.sig.active = false;
                    return Ok(self.sig.line);
                }
            }
        } else {
            let aug = self.aug.line;
            if self.vert_even {
                self.horz_synthesis(aug, true, data)?;
            } else {
                self.horz_synthesis(aug, false, data)?;
                let w = width as usize;
                match &mut self.lines[aug].data {
                    LineData::I32(_) => {
                        for v in &mut self.lines[aug].i32_mut()[..w] {
                            *v >>= 1;
                        }
                    }
                    LineData::I64(_) => {
                        for v in &mut self.lines[aug].i64_mut()[..w] {
                            *v >>= 1;
                        }
                    }
                    LineData::F32(_) => {
                        for v in &mut self.lines[aug].f32_mut()[..w] {
                            *v *= 0.5;
                        }
                    }
                    LineData::None => {}
                }
            }
            Ok(aug)
        }
    }

    /// Fills `self.lines[dst]` from the next lines of the two sources that
    /// make up a row: the lower resolution and the HL band when `low` is
    /// true, the LH and HH bands otherwise.
    fn horz_synthesis(&mut self, dst: usize, low: bool, data: &[u8]) -> Result<()> {
        let width = self.res_rect.siz.w;
        let has_horz = self.transform_flags & HORZ_TRX != 0;
        let Resolution {
            lines,
            child_res,
            bands,
            atk,
            reversible,
            horz_even,
            ..
        } = self;
        let [_, band_hl, band_lh, band_hh] = bands;
        let lsrc = if low {
            match child_res.as_mut() {
                Some(child) => child.pull_line(data)?,
                None => return ojph_error!(0x00070013, "internal error: missing resolution"),
            }
        } else {
            band_lh.pull_line(data)?
        };
        let dst = &mut lines[dst];
        if has_horz {
            let hsrc = if low {
                band_hl.pull_line(data)?
            } else {
                band_hh.pull_line(data)?
            };
            if *reversible {
                rev_horz_syn(atk, dst, lsrc, hsrc, width, *horz_even);
            } else {
                irv_horz_syn(atk, dst, lsrc, hsrc, width, *horz_even);
            }
        } else {
            let w = width as usize;
            match (&mut dst.data, &lsrc.data) {
                (LineData::I32(_), LineData::I32(_)) => {
                    dst.i32_mut()[..w].copy_from_slice(&lsrc.i32()[..w])
                }
                (LineData::I64(_), LineData::I64(_)) => {
                    dst.i64_mut()[..w].copy_from_slice(&lsrc.i64()[..w])
                }
                (LineData::F32(_), LineData::F32(_)) => {
                    dst.f32_mut()[..w].copy_from_slice(&lsrc.f32()[..w])
                }
                _ => return ojph_error!(0x00070015, "internal error: line types do not match"),
            }
        }
        Ok(())
    }

    pub(crate) fn get_top_left_precinct(&self) -> Option<Point> {
        let idx = self.cur_precinct_loc.x as u64
            + self.cur_precinct_loc.y as u64 * self.num_precincts.w as u64;
        if idx < self.num_precincts.area() {
            Some(self.precincts[idx as usize].img_point)
        } else {
            None
        }
    }

    fn advance_precinct(&mut self) {
        self.cur_precinct_loc.x += 1;
        if self.cur_precinct_loc.x >= self.num_precincts.w {
            self.cur_precinct_loc.x = 0;
            self.cur_precinct_loc.y += 1;
        }
    }

    pub(crate) fn parse_all_precincts(
        &mut self,
        scratch: &mut Vec<u8>,
        data_left: &mut u32,
        file: &mut MemInfile,
    ) -> std::result::Result<(), &'static str> {
        let idx = self.cur_precinct_loc.x as u64
            + self.cur_precinct_loc.y as u64 * self.num_precincts.w as u64;
        for i in idx..self.num_precincts.area() {
            if *data_left == 0 {
                break;
            }
            self.precincts[i as usize].parse(
                &mut self.bands,
                scratch,
                data_left,
                file,
                self.skipped_res_for_read,
            )?;
            self.advance_precinct();
        }
        Ok(())
    }

    pub(crate) fn parse_one_precinct(
        &mut self,
        scratch: &mut Vec<u8>,
        data_left: &mut u32,
        file: &mut MemInfile,
    ) -> std::result::Result<(), &'static str> {
        let idx = self.cur_precinct_loc.x as u64
            + self.cur_precinct_loc.y as u64 * self.num_precincts.w as u64;
        if idx >= self.num_precincts.area() {
            return Err("internal error: no precinct left to parse");
        }
        if *data_left == 0 {
            return Ok(());
        }
        self.precincts[idx as usize].parse(
            &mut self.bands,
            scratch,
            data_left,
            file,
            self.skipped_res_for_read,
        )?;
        self.advance_precinct();
        Ok(())
    }
}
