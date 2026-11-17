// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One component of a tile being encoded, after the write side of
//! `ojph_tile_comp.cpp`.

use super::resolution::Resolution;
use super::Ctx;
use crate::codestream::Budget;
use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::types::{Point, Rect};
use crate::Result;

#[derive(Debug)]
pub(crate) struct TileComp {
    /// The highest resolution; each resolution owns the one below it.
    res: Resolution,
    num_decomps: u32,
    /// The bytes of this tile component's packets, for tile-part lengths.
    num_bytes: u64,
}

impl TileComp {
    pub(crate) fn new(
        ctx: &Ctx,
        budget: &mut Budget,
        comp_num: u32,
        comp_rect: Rect,
        tile_top_left: Point,
    ) -> Result<Self> {
        let num_decomps = ctx.cod.get_coc(comp_num).get_num_decompositions();
        let comp_downsamp = ctx.siz.get_downsampling(comp_num);
        let res = Resolution::new(
            ctx,
            budget,
            comp_rect,
            comp_num,
            num_decomps,
            comp_downsamp,
            tile_top_left,
        )?;
        Ok(TileComp {
            res,
            num_decomps,
            num_bytes: 0,
        })
    }

    pub(crate) fn get_num_decompositions(&self) -> u32 {
        self.num_decomps
    }

    pub(crate) fn get_line(&mut self) -> &mut LineBuf {
        self.res.get_line()
    }

    #[cfg(test)]
    pub(crate) fn fill_buffers(&mut self, bits: u32) {
        self.res.fill_buffers(bits);
    }

    pub(crate) fn reset(&mut self) {
        self.num_bytes = 0;
        self.res.reset();
    }

    pub(crate) fn push_line(&mut self, budget: &mut Budget) -> Result<()> {
        self.res.push_line(budget)
    }

    /// The resolution numbered `res_num` from the lowest. As in OpenJPH, a
    /// number above the component's decompositions gives the highest
    /// resolution.
    fn resolution(&self, res_num: u32) -> Option<&Resolution> {
        let mut down = self.num_decomps.saturating_sub(res_num);
        let mut r = Some(&self.res);
        while down > 0 {
            r = r?.next_resolution();
            down -= 1;
        }
        r
    }

    fn resolution_mut(&mut self, res_num: u32) -> Option<&mut Resolution> {
        let mut down = self.num_decomps.saturating_sub(res_num);
        let mut r = Some(&mut self.res);
        while down > 0 {
            r = r?.next_resolution_mut();
            down -= 1;
        }
        r
    }

    /// Codes every packet header; returns the bytes of all the packets.
    pub(crate) fn prepare_precincts(&mut self, scratch: &mut Vec<u8>) -> Result<u64> {
        self.num_bytes = self.res.prepare_precinct(scratch)?;
        Ok(self.num_bytes)
    }

    /// Appends every packet of resolution `res_num`.
    pub(crate) fn write_precincts(&self, res_num: u32, out: &mut Vec<u8>) -> Result<()> {
        // OpenJPH asserts this; write_headers refuses the configurations
        // that would reach it
        if res_num > self.num_decomps {
            return ojph_error!(
                0x00080025,
                "internal error: resolution {} does not exist",
                res_num
            );
        }
        match self.resolution(res_num) {
            Some(r) => r.write_precincts(out),
            None => Ok(()),
        }
    }

    pub(crate) fn get_top_left_precinct(&self, res_num: u32) -> Option<Point> {
        self.resolution(res_num)?.get_top_left_precinct()
    }

    pub(crate) fn write_one_precinct(&mut self, res_num: u32, out: &mut Vec<u8>) -> Result<()> {
        match self.resolution_mut(res_num) {
            Some(r) => r.write_one_precinct(out),
            None => Ok(()),
        }
    }

    /// The bytes of all this component's packets.
    pub(crate) fn get_num_bytes(&self) -> u64 {
        self.num_bytes
    }

    /// The bytes of resolution `resolution_num`'s packets; OpenJPH's
    /// `get_num_bytes(ui32)`.
    pub(crate) fn get_num_bytes_res(&self, resolution_num: u32) -> u64 {
        self.res.get_num_bytes(resolution_num)
    }
}
