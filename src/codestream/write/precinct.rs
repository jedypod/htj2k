// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Packets: one precinct being written, after the write side of
//! `ojph_precinct.cpp`.

use crate::types::{Point, Rect};

#[derive(Debug, Clone, Default)]
pub(crate) struct Precinct {
    /// The precinct's top left corner projected to full resolution.
    pub img_point: Point,
    /// Which code blocks of each subband are in this precinct.
    pub cb_idxs: [Rect; 4],
}
