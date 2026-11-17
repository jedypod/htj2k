// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One component of a tile being encoded, after the write side of
//! `ojph_tile_comp.cpp`.

use super::resolution::Resolution;
use super::Ctx;
use crate::codestream::Budget;
use crate::line::LineBuf;
use crate::types::{Point, Rect};
use crate::Result;

#[derive(Debug)]
pub(crate) struct TileComp {
    /// The highest resolution; each resolution owns the one below it.
    res: Resolution,
    num_decomps: u32,
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
        Ok(TileComp { res, num_decomps })
    }

    pub(crate) fn get_num_decompositions(&self) -> u32 {
        self.num_decomps
    }

    pub(crate) fn get_line(&mut self) -> &mut LineBuf {
        self.res.get_line()
    }

    pub(crate) fn push_line(&mut self, budget: &mut Budget) -> Result<()> {
        self.res.push_line(budget)
    }
}
