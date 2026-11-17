// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One subband of a resolution, after `ojph_subband.cpp`.

use crate::codestream::codeblock::{Codeblock, CodedCbHeader};
use crate::codestream::precinct::Precinct;
use crate::codestream::{Budget, Build};
use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::types::{Rect, Size};
use crate::Result;

#[derive(Debug, Default)]
pub(crate) struct Subband {
    /// True if the subband has no samples, or does not exist at all.
    pub(crate) empty: bool,
    res_num: u32,
    band_num: u32,
    band_rect: Rect,
    lines: LineBuf,
    /// One row of code blocks, reused for every row of the subband.
    blocks: Vec<Codeblock>,
    pub(crate) num_blocks: Size,
    log_pp: Size,
    xcb_prime: u32,
    ycb_prime: u32,
    cur_cb_row: u32,
    cur_line: i32,
    cur_cb_height: i32,
    pub(crate) coded_cbs: Vec<CodedCbHeader>,
}

impl Subband {
    /// A subband that does not exist: the default state in OpenJPH.
    pub(crate) fn absent() -> Self {
        Subband {
            empty: true,
            ..Default::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        build: &Build,
        budget: &mut Budget,
        band_rect: Rect,
        comp_num: u32,
        res_num: u32,
        band_num: u32,
        has_horz_transform: bool,
        has_vert_transform: bool,
    ) -> Result<Self> {
        let cdp = build.cod.get_coc(comp_num);
        let reversible = cdp.access_atk().is_reversible();
        let log_cb = cdp.get_log_block_dims();
        let log_pp = cdp.get_log_precinct_size(res_num);
        let x_off = u32::from(has_horz_transform);
        let y_off = u32::from(has_vert_transform);
        if log_pp.w < x_off || log_pp.h < y_off {
            // OpenJPH lets these subtractions wrap, which is harmless here
            // but makes Resolution compute a nonsensical tag tree size.
            return ojph_error!(
                0x00070003,
                "a precinct size exponent of 0 is not valid at a resolution with a wavelet transform"
            );
        }
        let xcb_prime = log_cb.w.min(log_pp.w - x_off);
        let ycb_prime = log_cb.h.min(log_pp.h - y_off);
        let nominal = Size::new(1 << xcb_prime, 1 << ycb_prime);

        let dfs = if cdp.is_dfs_defined() {
            build.dfs.and_then(|d| d.get_dfs(cdp.get_dfs_index()))
        } else {
            None
        };
        let qcd = build.qcd.get_qcc(comp_num);
        let num_decomps = cdp.get_num_decompositions();
        let k_max = qcd.get_kmax(dfs, num_decomps, res_num, band_num);
        let mut delta = 0.0f32;
        if !reversible {
            let d = qcd.get_irrev_delta(dfs, num_decomps, comp_num, res_num, band_num)?;
            if k_max > 31 {
                return ojph_error!(
                    0x00070004,
                    "K_max of {} is too large for irreversible coding",
                    k_max
                );
            }
            delta = d / (1u32 << (31 - k_max)) as f32;
        }
        let precision = qcd.propose_precision(cdp, build.cod, build.qcd);

        let mut band = Subband {
            empty: band_rect.siz.w == 0 || band_rect.siz.h == 0,
            res_num,
            band_num,
            band_rect,
            log_pp,
            xcb_prime,
            ycb_prime,
            ..Default::default()
        };
        // OpenJPH leaves the line of an empty subband unallocated and hands
        // out a null pointer for it. A typed line of no samples takes its
        // place here, so the wavelet kernels can be given a reference.
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
        for cp in band.coded_cbs.iter_mut() {
            cp.k_max = k_max;
        }
        let x_lower_bound = (tbx0 >> xcb_prime) << xcb_prime;
        let y_lower_bound = (tby0 >> ycb_prime) << ycb_prime;

        let mut cb_size = Size::new(0, tby1.min(y_lower_bound.saturating_add(nominal.h)) - tby0);
        band.cur_cb_height = cb_size.h as i32;
        // The tallest row of blocks: the first, the last, or any between,
        // which are of nominal height. The blocks keep their buffers from
        // row to row, so this is the height they are allocated for.
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
                delta,
                reversible,
                build.resilient,
                cdp.get_block_vertical_causality(),
            )?);
            line_offset += cb_size.w;
        }

        Ok(band)
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
        let log_pp = self.log_pp;
        let band_num = self.band_num as u64;
        let trx0 = res_rect.org.x as u64;
        let try0 = res_rect.org.y as u64;
        let trx1 = trx0 + res_rect.siz.w as u64;
        let try1 = try0 + res_rect.siz.h as u64;
        let pc_lft = (trx0 >> log_pp.w) << log_pp.w;
        let pc_top = (try0 >> log_pp.h) << log_pp.h;
        let x_shift = u32::from(has_horz_transform);
        let y_shift = u32::from(has_vert_transform);
        let xcb = self.xcb_prime;
        let ycb = self.ycb_prime;
        let mut coly = 0u32;
        let mut colx = 0u32;
        for y in 0..num_precincts.h as u64 {
            let mut pcy0 = try0.max(pc_top + (y << log_pp.h));
            let mut pcy1 = try1.min(pc_top + ((y + 1) << log_pp.h));
            pcy0 = (pcy0 + (1 << y_shift) - 1 - (band_num >> 1)) >> y_shift;
            pcy1 = (pcy1 + (1 << y_shift) - 1 - (band_num >> 1)) >> y_shift;
            let yb = (((pcy1 + (1 << ycb) - 1) >> ycb) - (pcy0 >> ycb)) as u32;
            colx = 0;
            for x in 0..num_precincts.w as u64 {
                let p = &mut precincts[(y * num_precincts.w as u64 + x) as usize];
                let mut pcx0 = trx0.max(pc_lft + (x << log_pp.w));
                let mut pcx1 = trx1.min(pc_lft + ((x + 1) << log_pp.w));
                pcx0 = (pcx0 + (1 << x_shift) - 1 - (band_num & 1)) >> x_shift;
                pcx1 = (pcx1 + (1 << x_shift) - 1 - (band_num & 1)) >> x_shift;
                let xb = (((pcx1 + (1 << xcb) - 1) >> xcb) - (pcx0 >> xcb)) as u32;
                let bp = &mut p.cb_idxs[self.band_num as usize];
                bp.org.x = colx;
                bp.org.y = coly;
                bp.siz.w = xb;
                bp.siz.h = yb;
                colx += xb;
            }
            coly += yb;
        }
        debug_assert!(colx == self.num_blocks.w && coly == self.num_blocks.h);
    }

    /// Returns the next line of the subband, decoding a row of code blocks
    /// when the previous row is used up. `data` is the whole codestream.
    pub(crate) fn pull_line(&mut self, data: &[u8]) -> Result<&mut LineBuf> {
        if self.empty {
            return Ok(&mut self.lines);
        }
        self.cur_line -= 1;
        if self.cur_line <= 0 && self.cur_cb_row < self.num_blocks.h {
            let tbx0 = self.band_rect.org.x as u64;
            let tby0 = self.band_rect.org.y as u64;
            let tbx1 = tbx0 + self.band_rect.siz.w as u64;
            let tby1 = tby0 + self.band_rect.siz.h as u64;
            let nominal_w = 1u64 << self.xcb_prime;
            let nominal_h = 1u64 << self.ycb_prime;
            let x_lower_bound = (tbx0 >> self.xcb_prime) << self.xcb_prime;
            let y_lower_bound = (tby0 >> self.ycb_prime) << self.ycb_prime;
            let cby0 = tby0.max(y_lower_bound + self.cur_cb_row as u64 * nominal_h);
            let cby1 = tby1.min(y_lower_bound + (self.cur_cb_row as u64 + 1) * nominal_h);
            let mut cb_size = Size::new(0, (cby1 - cby0) as u32);
            self.cur_cb_height = cb_size.h as i32;
            self.cur_line = self.cur_cb_height;
            let row = self.cur_cb_row as usize * self.num_blocks.w as usize;
            for (i, block) in self.blocks.iter_mut().enumerate() {
                let i = i as u64;
                let cbx0 = tbx0.max(x_lower_bound + i * nominal_w);
                let cbx1 = tbx1.min(x_lower_bound + (i + 1) * nominal_w);
                cb_size.w = (cbx1 - cbx0) as u32;
                block.recreate(cb_size);
                block.decode(&self.coded_cbs[row + i as usize], data)?;
            }
            self.cur_cb_row += 1;
        }
        if self.cur_line <= 0 {
            // More lines were asked for than the subband has.
            return ojph_error!(
                0x000300A3,
                "internal error: pulled past the end of a subband"
            );
        }
        for block in self.blocks.iter_mut() {
            block.pull_line(&mut self.lines)?;
        }
        Ok(&mut self.lines)
    }

    #[allow(dead_code)]
    pub(crate) fn res_num(&self) -> u32 {
        self.res_num
    }
}

/// `ceil(v / 2^shift)`, without the overflow of OpenJPH's
/// `(v + (1 << shift) - 1) >> shift` when `v` is close to `u32::MAX`.
#[inline]
pub(crate) fn ceil_shift(v: u32, shift: u32) -> u32 {
    ((v as u64 + (1u64 << shift) - 1) >> shift) as u32
}
