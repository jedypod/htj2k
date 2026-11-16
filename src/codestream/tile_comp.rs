// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One component of a tile, after `ojph_tile_comp.cpp`.

use crate::codestream::bitbuffer_read::ParseResult;
use crate::codestream::resolution::Resolution;
use crate::codestream::{Budget, Build};
use crate::file::MemInfile;
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
        build: &Build,
        budget: &mut Budget,
        comp_num: u32,
        comp_rect: Rect,
        recon_comp_rect: Rect,
        tile_top_left: Point,
    ) -> Result<Self> {
        let num_decomps = build.cod.get_coc(comp_num).get_num_decompositions();
        let comp_downsamp = build.siz.get_downsampling(comp_num);
        let res = Resolution::new(
            build,
            budget,
            comp_rect,
            recon_comp_rect,
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

    /// Produces the next line of the component. `data` is the whole
    /// codestream.
    pub(crate) fn pull_line(&mut self, data: &[u8]) -> Result<&mut LineBuf> {
        self.res.pull_line(data)
    }

    /// The resolution numbered `res_num`, counting from the lowest, if the
    /// component has it.
    fn resolution(&mut self, res_num: u32) -> Option<&mut Resolution> {
        // how many levels to go down
        let mut down = self.num_decomps.checked_sub(res_num)?;
        let mut r = Some(&mut self.res);
        while down > 0 {
            r = r?.next_resolution();
            down -= 1;
        }
        r
    }

    pub(crate) fn get_top_left_precinct(&mut self, res_num: u32) -> Option<Point> {
        self.resolution(res_num)?.get_top_left_precinct()
    }

    pub(crate) fn parse_precincts(
        &mut self,
        res_num: u32,
        scratch: &mut Vec<u8>,
        data_left: &mut u32,
        file: &mut MemInfile,
    ) -> ParseResult<()> {
        match self.resolution(res_num) {
            Some(r) => r.parse_all_precincts(scratch, data_left, file),
            None => Ok(()),
        }
    }

    pub(crate) fn parse_one_precinct(
        &mut self,
        res_num: u32,
        scratch: &mut Vec<u8>,
        data_left: &mut u32,
        file: &mut MemInfile,
    ) -> ParseResult<()> {
        match self.resolution(res_num) {
            Some(r) => r.parse_one_precinct(scratch, data_left, file),
            None => Ok(()),
        }
    }
}
