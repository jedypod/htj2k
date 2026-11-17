// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One resolution level being encoded, after the write side of
//! `ojph_resolution.cpp`.
//!
//! A resolution takes the lines of its rectangle one at a time, runs the
//! forward wavelet transform on them, and passes the results down to its
//! three high-pass subbands and to the next lower resolution.

use super::precinct::Precinct;
use super::subband::Subband;
use super::Ctx;
use crate::codestream::subband::ceil_shift;
use crate::codestream::Budget;
use crate::error::ojph_error;
use crate::line::{LineBuf, LineData};
use crate::params::ParamAtk;
use crate::transform::wavelet::{
    irv_horz_ana, irv_vert_step, irv_vert_times_k, rev_horz_ana, rev_vert_step,
};
use crate::types::{Point, Rect, Size};
use crate::Result;

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
    num_steps: u32,
    res_num: u32,
    res_rect: Rect,
    /// Used to store lines: one per lifting step, then sig and aug.
    lines: Vec<LineBuf>,
    /// Step state.
    ssp: Vec<LiftingBuf>,
    sig: LiftingBuf,
    aug: LiftingBuf,
    pub(crate) bands: [Subband; 4],
    child_res: Option<Box<Resolution>>,
    pub(crate) precincts: Vec<Precinct>,
    num_precincts: Size,
    /// Used for the spatial progression orders.
    cur_precinct_loc: Point,
    /// The bytes of this resolution's packets, for tile-part lengths.
    num_bytes: u64,
    atk: ParamAtk,
    cur_line: u32,
    rows_to_produce: u32,
    vert_even: bool,
    horz_even: bool,
}

impl Resolution {
    pub(crate) fn new(
        ctx: &Ctx,
        budget: &mut Budget,
        res_rect: Rect,
        comp_num: u32,
        res_num: u32,
        res_downsamp: Point,
        tile_top_left: Point,
    ) -> Result<Self> {
        budget.take(std::mem::size_of::<Resolution>() as u64)?;
        let cdp = ctx.cod.get_coc(comp_num);
        if cdp.is_dfs_defined() {
            return ojph_error!(
                0x00080020,
                "the encoder does not support DFS (downsampling factor styles)"
            );
        }
        let atk = cdp.access_atk().clone();
        let has_transform = res_num > 0;

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
                    child_res = Some(Box::new(Resolution::new(
                        ctx,
                        budget,
                        re,
                        comp_num,
                        res_num - 1,
                        down,
                        tile_top_left,
                    )?));
                } else {
                    bands[i as usize] =
                        Subband::new(ctx, budget, re, comp_num, res_num, i as u32, true, true)?;
                }
            }
        } else {
            bands[0] = Subband::new(ctx, budget, res_rect, comp_num, res_num, 0, false, false)?;
        }

        // finalize precincts
        let log_pp = cdp.get_log_precinct_size(res_num);
        let mut num_precincts = Size::default();
        let mut precincts = Vec::new();
        if trx0 != trx1 && try0 != try1 {
            num_precincts.w = ceil_shift(trx1 as u32, log_pp.w) - (trx0 >> log_pp.w) as u32;
            num_precincts.h = ceil_shift(try1 as u32, log_pp.h) - (try0 >> log_pp.h) as u32;
            precincts = budget.alloc::<Precinct>(num_precincts.area() as usize)?;
        }
        let x_lower_bound = ((trx0 >> log_pp.w) << log_pp.w) as u32;
        let y_lower_bound = ((try0 >> log_pp.h) << log_pp.h) as u32;
        let mut pp = precincts.iter_mut();
        for y in 0..num_precincts.h {
            let ppy0 = y_lower_bound.wrapping_add(y << log_pp.h);
            for x in 0..num_precincts.w {
                let ppx0 = x_lower_bound.wrapping_add(x << log_pp.w);
                let Some(p) = pp.next() else {
                    return ojph_error!(0x00080021, "internal error: precinct count");
                };
                let mut t = Point::new(
                    res_downsamp.x.wrapping_mul(ppx0),
                    res_downsamp.y.wrapping_mul(ppy0),
                );
                t.x = t.x.max(tile_top_left.x);
                t.y = t.y.max(tile_top_left.y);
                p.img_point = t;
            }
        }
        for band in bands.iter() {
            band.get_cb_indices(
                num_precincts,
                &mut precincts,
                res_rect,
                has_transform,
                has_transform,
            );
        }

        // allocate lines
        let reversible = atk.is_reversible();
        let num_steps = atk.get_num_steps();
        let qp = ctx.qcd.get_qcc(comp_num);
        let precision = qp.propose_precision(cdp, ctx.cod, ctx.qcd);
        let width = res_rect.siz.w as usize + 1;
        let mut lines = Vec::with_capacity(num_steps as usize + 2);
        for _ in 0..num_steps + 2 {
            lines.push(if !reversible {
                budget.line_f32(width, 1)?
            } else if precision <= 32 {
                budget.line_i32(width, 1)?
            } else {
                budget.line_i64(width, 1)?
            });
        }
        let ssp = (0..num_steps as usize)
            .map(|i| LiftingBuf {
                line: i,
                active: false,
            })
            .collect();

        Ok(Resolution {
            reversible,
            num_steps,
            res_num,
            res_rect,
            lines,
            ssp,
            sig: LiftingBuf {
                line: num_steps as usize,
                active: false,
            },
            aug: LiftingBuf {
                line: num_steps as usize + 1,
                active: false,
            },
            bands,
            child_res,
            precincts,
            num_precincts,
            cur_precinct_loc: Point::new(0, 0),
            num_bytes: 0,
            atk,
            cur_line: 0,
            rows_to_produce: res_rect.siz.h,
            vert_even: (res_rect.org.y & 1) == 0,
            horz_even: (res_rect.org.x & 1) == 0,
        })
    }

    pub(crate) fn next_resolution(&self) -> Option<&Resolution> {
        self.child_res.as_deref()
    }

    pub(crate) fn next_resolution_mut(&mut self) -> Option<&mut Resolution> {
        self.child_res.as_deref_mut()
    }

    /// Codes the packet headers of this resolution and the ones below,
    /// and returns the bytes of all their packets.
    pub(crate) fn prepare_precinct(&mut self, scratch: &mut Vec<u8>) -> Result<u64> {
        let mut lower_resolutions_bytes = 0;
        if self.res_num != 0 {
            let Some(child) = self.child_res.as_deref_mut() else {
                return ojph_error!(0x00080022, "internal error: missing resolution");
            };
            lower_resolutions_bytes = child.prepare_precinct(scratch)?;
        }
        self.num_bytes = 0;
        for p in self.precincts.iter_mut() {
            self.num_bytes += p.prepare_precinct(&self.bands, scratch)?;
        }
        Ok(self.num_bytes + lower_resolutions_bytes)
    }

    /// Appends every packet of this resolution.
    pub(crate) fn write_precincts(&self, out: &mut Vec<u8>) -> Result<()> {
        for p in &self.precincts {
            p.write(&self.bands, out)?;
        }
        Ok(())
    }

    /// The top left corner of the next precinct to write, if any is left.
    pub(crate) fn get_top_left_precinct(&self) -> Option<Point> {
        let idx = self.cur_precinct_loc.x as u64
            + self.cur_precinct_loc.y as u64 * self.num_precincts.w as u64;
        if idx < self.num_precincts.area() {
            Some(self.precincts[idx as usize].img_point)
        } else {
            None
        }
    }

    /// Appends the next precinct's packet.
    pub(crate) fn write_one_precinct(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let idx = self.cur_precinct_loc.x as u64
            + self.cur_precinct_loc.y as u64 * self.num_precincts.w as u64;
        let Some(p) = self.precincts.get(idx as usize) else {
            return ojph_error!(0x00080024, "internal error: no precinct left to write");
        };
        p.write(&self.bands, out)?;
        self.cur_precinct_loc.x += 1;
        if self.cur_precinct_loc.x >= self.num_precincts.w {
            self.cur_precinct_loc.x = 0;
            self.cur_precinct_loc.y += 1;
        }
        Ok(())
    }

    /// The bytes of resolution `resolution_num`'s packets, if it is this
    /// one or below; 0 otherwise.
    pub(crate) fn get_num_bytes(&self, resolution_num: u32) -> u64 {
        if self.res_num == resolution_num {
            return self.num_bytes;
        }
        match self.child_res.as_deref() {
            Some(child) if resolution_num < self.res_num => child.get_num_bytes(resolution_num),
            _ => 0,
        }
    }

    /// The line to fill before [`push_line`](Self::push_line).
    pub(crate) fn get_line(&mut self) -> &mut LineBuf {
        self.cur_line += 1;
        if self.vert_even {
            self.sig.active = true;
            &mut self.lines[self.sig.line]
        } else {
            self.aug.active = true;
            &mut self.lines[self.aug.line]
        }
    }

    /// Transforms the line filled since [`get_line`](Self::get_line), as
    /// far as it can, and passes the results on.
    pub(crate) fn push_line(&mut self, budget: &mut Budget) -> Result<()> {
        if self.res_num == 0 {
            let idx = if self.vert_even {
                self.sig.line
            } else {
                self.aug.line
            };
            self.bands[0].exchange_buf(&mut self.lines[idx]);
            return self.bands[0].push_line(budget);
        }

        let width = self.res_rect.siz.w;
        if width == 0 {
            return Ok(());
        }
        let height = self.res_rect.siz.h;
        if height > 1 {
            if !self.vert_even && self.cur_line < height {
                self.vert_even = !self.vert_even;
                return Ok(());
            }

            let mut idle = 0;
            loop {
                let rows_before = self.rows_to_produce;
                // vertical transform
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
                        let s = self.atk.get_step(self.num_steps - i as u32 - 1);
                        // take the destination out so the sources can be
                        // borrowed from the same vector
                        let mut dp = std::mem::take(&mut self.lines[self.aug.line]);
                        if self.reversible {
                            rev_vert_step(
                                s,
                                &self.lines[sp1],
                                &self.lines[sp2],
                                &mut dp,
                                width,
                                false,
                            );
                        } else {
                            irv_vert_step(
                                s,
                                &self.lines[sp1],
                                &self.lines[sp2],
                                &mut dp,
                                width,
                                false,
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
                    if !self.reversible {
                        let k = self.atk.get_k();
                        irv_vert_times_k(k, &mut self.lines[self.aug.line], width);
                    }
                    self.horz_analysis(self.aug.line, false, budget)?;
                    self.aug.active = false;
                    self.rows_to_produce = self.rows_to_produce.saturating_sub(1);
                }
                if self.sig.active {
                    if !self.reversible {
                        let k_inv = 1.0 / self.atk.get_k();
                        irv_vert_times_k(k_inv, &mut self.lines[self.sig.line], width);
                    }
                    self.horz_analysis(self.sig.line, true, budget)?;
                    self.sig.active = false;
                    self.rows_to_produce = self.rows_to_produce.saturating_sub(1);
                }
                self.vert_even = !self.vert_even;
                if !(self.cur_line >= height && self.rows_to_produce > 0) {
                    break;
                }
                // Past the last line, the lifting state empties within a
                // few passes; OpenJPH would loop forever if it did not.
                idle = if self.rows_to_produce == rows_before {
                    idle + 1
                } else {
                    0
                };
                if idle > self.num_steps + 2 {
                    return ojph_error!(
                        0x00080023,
                        "internal error: the wavelet transform does not finish"
                    );
                }
            }
        } else if self.vert_even {
            self.horz_analysis(self.sig.line, true, budget)?;
        } else {
            // vertical transform
            let w = width as usize;
            let aug = &mut self.lines[self.aug.line];
            match &mut aug.data {
                LineData::I32(_) => {
                    for v in &mut aug.i32_mut()[..w] {
                        *v = v.wrapping_mul(2);
                    }
                }
                LineData::I64(_) => {
                    for v in &mut aug.i64_mut()[..w] {
                        *v = v.wrapping_mul(2);
                    }
                }
                LineData::F32(_) => {
                    for v in &mut aug.f32_mut()[..w] {
                        *v *= 2.0;
                    }
                }
                LineData::None => {}
            }
            self.horz_analysis(self.aug.line, false, budget)?;
        }
        Ok(())
    }

    /// The horizontal transform of `self.lines[src]`: into the next lower
    /// resolution and the HL band when `low` is true, into the LH and HH
    /// bands otherwise. The destinations are then pushed.
    fn horz_analysis(&mut self, src: usize, low: bool, budget: &mut Budget) -> Result<()> {
        let width = self.res_rect.siz.w;
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
        let src = &lines[src];
        if low {
            let Some(child) = child_res.as_deref_mut() else {
                return ojph_error!(0x00080022, "internal error: missing resolution");
            };
            let (l, h) = (child.get_line(), band_hl.get_line());
            if *reversible {
                rev_horz_ana(atk, l, h, src, width, *horz_even);
            } else {
                irv_horz_ana(atk, l, h, src, width, *horz_even);
            }
            band_hl.push_line(budget)?;
            child.push_line(budget)?;
        } else {
            let (l, h) = (band_lh.get_line(), band_hh.get_line());
            if *reversible {
                rev_horz_ana(atk, l, h, src, width, *horz_even);
            } else {
                irv_horz_ana(atk, l, h, src, width, *horz_even);
            }
            band_lh.push_line(budget)?;
            band_hh.push_line(budget)?;
        }
        Ok(())
    }
}
