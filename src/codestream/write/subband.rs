// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One subband being encoded, after the write side of
//! `ojph_subband.cpp`.

use super::codeblock::{Codeblock, CodedCbHeader};
use super::precinct::Precinct;
use super::Ctx;
use crate::codestream::subband::{cb_indices, ceil_shift};
use crate::codestream::Budget;
use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::types::{Rect, Size};
use crate::Result;

#[derive(Debug, Default)]
pub(crate) struct Subband {
    /// True if the subband has no samples, or is not used.
    pub(crate) empty: bool,
    band_num: u32,
    band_rect: Rect,
    /// The line the resolution fills before `push_line`.
    lines: LineBuf,
    /// One row of code blocks, reused for every row of the subband.
    blocks: Vec<Codeblock>,
    pub(crate) num_blocks: Size,
    log_pp: Size,
    xcb_prime: u32,
    ycb_prime: u32,
    cur_cb_row: u32,
    cur_line: u32,
    cur_cb_height: u32,
    pub(crate) coded_cbs: Vec<CodedCbHeader>,
    /// The coded bytes of every block, in the order they were coded;
    /// `CodedCbHeader::data` indexes into it.
    pub(crate) coded: Vec<u8>,
}

impl Subband {
    /// A subband that is not used: the default state in OpenJPH.
    pub(crate) fn absent() -> Self {
        Subband {
            empty: true,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ctx: &Ctx,
        budget: &mut Budget,
        band_rect: Rect,
        comp_num: u32,
        res_num: u32,
        band_num: u32,
        has_horz_transform: bool,
        has_vert_transform: bool,
    ) -> Result<Self> {
        let cdp = ctx.cod.get_coc(comp_num);
        let reversible = cdp.access_atk().is_reversible();
        let log_cb = cdp.get_log_block_dims();
        let log_pp = cdp.get_log_precinct_size(res_num);
        let x_off = u32::from(has_horz_transform);
        let y_off = u32::from(has_vert_transform);
        if log_pp.w < x_off || log_pp.h < y_off {
            return ojph_error!(
                0x00070003,
                "a precinct size exponent of 0 is not valid at a resolution with a wavelet transform"
            );
        }
        let xcb_prime = log_cb.w.min(log_pp.w - x_off);
        let ycb_prime = log_cb.h.min(log_pp.h - y_off);
        let nominal = Size::new(1 << xcb_prime, 1 << ycb_prime);

        let qcd = ctx.qcd.get_qcc(comp_num);
        let num_decomps = cdp.get_num_decompositions();
        let k_max = qcd.get_kmax(None, num_decomps, res_num, band_num);
        let mut delta_inv = 0.0f32;
        if !reversible {
            let d = qcd.get_irrev_delta(None, num_decomps, comp_num, res_num, band_num)?;
            if k_max > 31 {
                return ojph_error!(
                    0x00070004,
                    "K_max of {} is too large for irreversible coding",
                    k_max
                );
            }
            let delta = d / (1u32 << (31 - k_max)) as f32;
            delta_inv = 1.0 / delta;
        }
        let precision = qcd.propose_precision(cdp, ctx.cod, ctx.qcd);

        let mut band = Subband {
            empty: band_rect.siz.w == 0 || band_rect.siz.h == 0,
            band_num,
            band_rect,
            log_pp,
            xcb_prime,
            ycb_prime,
            ..Default::default()
        };
        // OpenJPH allocates the line of a non-empty subband only, and the
        // wavelet kernels never write to the missing one. A line of one
        // sample takes its place here, so the kernels get a reference.
        let width = band_rect.siz.w as usize + 1;
        band.lines = if !reversible {
            budget.line_f32(width, 1)?
        } else if precision <= 32 {
            budget.line_i32(width, 1)?
        } else {
            budget.line_i64(width, 1)?
        };
        if band.empty {
            return Ok(band);
        }

        let tbx0 = band_rect.org.x;
        let tby0 = band_rect.org.y;
        let tbx1 = band_rect.org.x + band_rect.siz.w;
        let tby1 = band_rect.org.y + band_rect.siz.h;
        band.num_blocks.w = ceil_shift(tbx1, xcb_prime) - (tbx0 >> xcb_prime);
        band.num_blocks.h = ceil_shift(tby1, ycb_prime) - (tby0 >> ycb_prime);

        band.coded_cbs = budget.alloc(band.num_blocks.area() as usize)?;
        let x_lower_bound = (tbx0 >> xcb_prime) << xcb_prime;
        let y_lower_bound = (tby0 >> ycb_prime) << ycb_prime;

        let mut cb_size = Size::new(0, band.first_row_height());
        // The tallest row of blocks: the first, the last, or any between,
        // which are of nominal height.
        let max_height = match band.num_blocks.h {
            0 | 1 => cb_size.h,
            2 => cb_size.h.max(tby1 - (y_lower_bound + nominal.h)),
            _ => nominal.h,
        };
        let mut line_offset = 0;
        budget.take(band.num_blocks.w as u64 * std::mem::size_of::<Codeblock>() as u64)?;
        band.blocks = Vec::with_capacity(band.num_blocks.w as usize);
        for i in 0..band.num_blocks.w as u64 {
            let cbx0 = (tbx0 as u64).max(x_lower_bound as u64 + i * nominal.w as u64);
            let cbx1 = (tbx1 as u64).min(x_lower_bound as u64 + (i + 1) * nominal.w as u64);
            cb_size.w = (cbx1 - cbx0) as u32;
            band.blocks.push(Codeblock::new(
                budget,
                cb_size,
                max_height,
                k_max,
                line_offset,
                precision,
                delta_inv,
                reversible,
            )?);
            line_offset += cb_size.w;
        }
        Ok(band)
    }

    /// The height of the first row of code blocks.
    fn first_row_height(&self) -> u32 {
        let tby0 = self.band_rect.org.y;
        let tby1 = tby0 + self.band_rect.siz.h;
        let y_lower_bound = (tby0 >> self.ycb_prime) << self.ycb_prime;
        tby1.min(y_lower_bound.saturating_add(1 << self.ycb_prime)) - tby0
    }

    /// Sets every sample of the subband's buffers to the bit pattern
    /// `bits`.
    #[cfg(test)]
    pub(crate) fn fill_buffers(&mut self, bits: u32) {
        self.lines.fill_bits(bits);
        for block in self.blocks.iter_mut() {
            block.fill_buffers(bits);
        }
    }

    /// Returns the subband to the state `new` leaves it in: nothing
    /// pushed or coded, and its code blocks on the first row. The coded
    /// bytes are dropped but their vector keeps its capacity.
    pub(crate) fn reset(&mut self) {
        self.cur_cb_row = 0;
        self.cur_line = 0;
        self.coded.clear();
        if self.empty {
            return;
        }
        let height = self.first_row_height();
        self.cur_cb_height = height;
        self.coded_cbs.fill(CodedCbHeader::default());
        for block in self.blocks.iter_mut() {
            block.reset(height);
        }
    }

    /// Works out which code blocks of this subband belong to each precinct
    /// of the resolution.
    pub(crate) fn get_cb_indices(
        &self,
        num_precincts: Size,
        precincts: &mut [Precinct],
        res_rect: Rect,
        has_horz_transform: bool,
        has_vert_transform: bool,
    ) {
        if self.empty {
            return;
        }
        let b = self.band_num as usize;
        cb_indices(
            self.log_pp,
            self.band_num,
            self.xcb_prime,
            self.ycb_prime,
            num_precincts,
            res_rect,
            has_horz_transform,
            has_vert_transform,
            |i, r| precincts[i].cb_idxs[b] = r,
        );
    }

    /// The line to fill before [`push_line`](Self::push_line).
    pub(crate) fn get_line(&mut self) -> &mut LineBuf {
        &mut self.lines
    }

    /// Swaps the subband's line with `l`, which must be of the same kind
    /// and size; used by resolution 0 to pass its line straight down.
    pub(crate) fn exchange_buf(&mut self, l: &mut LineBuf) {
        if self.empty {
            return;
        }
        std::mem::swap(&mut self.lines, l);
    }

    /// Passes the filled line to the code blocks, and encodes them when
    /// a row of blocks is complete.
    pub(crate) fn push_line(&mut self, budget: &mut Budget) -> Result<()> {
        if self.empty {
            return Ok(());
        }
        if self.cur_cb_row >= self.num_blocks.h {
            return ojph_error!(
                0x00080013,
                "internal error: pushed past the end of a subband"
            );
        }
        // push to codeblocks
        for block in self.blocks.iter_mut() {
            block.push(&self.lines)?;
        }
        self.cur_line += 1;
        if self.cur_line >= self.cur_cb_height {
            let row = self.cur_cb_row as usize * self.num_blocks.w as usize;
            let (coded_cbs, coded) = (&mut self.coded_cbs, &mut self.coded);
            for (i, block) in self.blocks.iter_mut().enumerate() {
                block.encode(&mut coded_cbs[row + i], coded, budget)?;
            }

            self.cur_cb_row += 1;
            if self.cur_cb_row < self.num_blocks.h {
                self.cur_line = 0;

                let tbx0 = self.band_rect.org.x as u64;
                let tby0 = self.band_rect.org.y as u64;
                let tbx1 = tbx0 + self.band_rect.siz.w as u64;
                let tby1 = tby0 + self.band_rect.siz.h as u64;
                let nominal_w = 1u64 << self.xcb_prime;
                let nominal_h = 1u64 << self.ycb_prime;
                let x_lower_bound = (tbx0 >> self.xcb_prime) << self.xcb_prime;
                let y_lower_bound = (tby0 >> self.ycb_prime) << self.ycb_prime;
                let cby0 = y_lower_bound + self.cur_cb_row as u64 * nominal_h;
                let cby1 = tby1.min(cby0 + nominal_h);
                let mut cb_size = Size::new(0, (cby1 - tby0.max(cby0)) as u32);
                self.cur_cb_height = cb_size.h;
                for (i, block) in self.blocks.iter_mut().enumerate() {
                    let i = i as u64;
                    let cbx0 = tbx0.max(x_lower_bound + i * nominal_w);
                    let cbx1 = tbx1.min(x_lower_bound + (i + 1) * nominal_w);
                    cb_size.w = (cbx1 - cbx0) as u32;
                    block.recreate(cb_size);
                }
            }
        }
        Ok(())
    }
}
