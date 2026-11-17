// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The codestream and its parts: tiles, tile components, resolutions,
//! subbands, precincts and code blocks, after `src/core/codestream` in
//! OpenJPH.
//!
//! OpenJPH builds this tree in two passes over one arena: `pre_alloc`
//! adds up what every object needs, and `finalize_alloc` hands the memory
//! out. Here each object owns its buffers and is built in one pass, and a
//! [`Budget`] takes the place of the arena's total so that a codestream
//! cannot make the decoder allocate without limit.

mod bitbuffer_read;
mod codeblock;
#[allow(clippy::module_inception)]
mod codestream;
mod precinct;
mod resolution;
mod subband;
mod tile;
mod tile_comp;
mod write;

pub use codestream::{Codestream, DecoderMemory};
pub use write::Encoder;

use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::params::{ParamCod, ParamDfs, ParamNlt, ParamQcd, ParamSiz};
use crate::Result;

/// The parameters every part of the tree is built from. This stands in
/// for the `codestream*` that OpenJPH passes to each `finalize_alloc`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Build<'a> {
    pub siz: &'a ParamSiz,
    pub cod: &'a ParamCod,
    pub qcd: &'a ParamQcd,
    /// `None` when the codestream has no DFS marker segment.
    pub dfs: Option<&'a ParamDfs>,
    pub nlt: &'a ParamNlt,
    pub skipped_res_for_read: u32,
    pub skipped_res_for_recon: u32,
    pub resilient: bool,
}

/// How much memory the decoder may still allocate, in bytes.
///
/// Every buffer whose size derives from the codestream is taken from the
/// budget before it is allocated.
#[derive(Debug)]
pub(crate) struct Budget {
    limit: u64,
    left: u64,
}

impl Budget {
    pub(crate) fn new(limit: u64) -> Self {
        Budget { limit, left: limit }
    }

    /// The bytes that may still be accounted for.
    pub(crate) fn left(&self) -> u64 {
        self.left
    }

    /// The bytes accounted for so far.
    pub(crate) fn taken(&self) -> u64 {
        self.limit - self.left
    }

    /// Accounts for `bytes` bytes, or fails if the limit would be passed.
    pub(crate) fn take(&mut self, bytes: u64) -> Result<()> {
        if bytes > self.left {
            return ojph_error!(
                0x00070001,
                "this codestream needs more memory than the limit allows"
            );
        }
        self.left -= bytes;
        Ok(())
    }

    fn take_elems<T>(&mut self, n: u64) -> Result<()> {
        match n.checked_mul(std::mem::size_of::<T>() as u64) {
            Some(bytes) => self.take(bytes),
            None => self.take(u64::MAX),
        }
    }

    /// A vector of `n` default values.
    pub(crate) fn alloc<T: Default + Clone>(&mut self, n: usize) -> Result<Vec<T>> {
        self.take_elems::<T>(n as u64)?;
        Ok(vec![T::default(); n])
    }

    pub(crate) fn line_i32(&mut self, size: usize, pre_size: usize) -> Result<LineBuf> {
        self.take_elems::<i32>(size as u64 + pre_size as u64)?;
        Ok(LineBuf::new_i32(size, pre_size))
    }

    pub(crate) fn line_i64(&mut self, size: usize, pre_size: usize) -> Result<LineBuf> {
        self.take_elems::<i64>(size as u64 + pre_size as u64)?;
        Ok(LineBuf::new_i64(size, pre_size))
    }

    pub(crate) fn line_f32(&mut self, size: usize, pre_size: usize) -> Result<LineBuf> {
        self.take_elems::<f32>(size as u64 + pre_size as u64)?;
        Ok(LineBuf::new_f32(size, pre_size))
    }
}
