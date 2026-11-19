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
use crate::params::{ParamAtk, ParamCod, ParamDfs, ParamNlt, ParamQcd, ParamSiz};
use crate::Result;

/// The parameter objects a decoder or encoder keeps from one codestream
/// to the next, for their storage only: the per-component COD, QCD and
/// NLT objects with the NLT tables, the components of SIZ and its copy
/// of COD, and the kernels. [`KeptParams::recycle`] gives each the state
/// a new one has before it is used again. `ParamCap` holds no storage
/// and is not kept.
#[derive(Debug, Default)]
pub(crate) struct KeptParams {
    pub siz: ParamSiz,
    pub cod: ParamCod,
    pub qcd: ParamQcd,
    pub nlt: ParamNlt,
    pub dfs: ParamDfs,
    pub atk: ParamAtk,
}

impl KeptParams {
    /// Gives every object the state `new` gives, keeping its storage.
    pub(crate) fn recycle(&mut self) {
        self.siz.init();
        self.cod.recycle();
        self.qcd.recycle();
        self.nlt.restart();
        self.dfs.restart();
        self.atk.restart();
    }
}

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

/// The error of an encoder whose codestream would be longer than
/// `Encoder::set_output_limit` allows.
pub(crate) const OUTPUT_LIMIT_ERROR: u32 = 0x00080070;

/// How much memory the decoder may still allocate, in bytes.
///
/// Every buffer whose size derives from the codestream is taken from the
/// budget before it is allocated.
#[derive(Debug)]
pub(crate) struct Budget {
    limit: u64,
    left: u64,
    /// The encoder's coded bytes that the output limit still allows.
    coded_left: u64,
    /// Bytes of the limit held by something that can be freed, which
    /// [`take`](Self::take) does not let the bytes taken reach.
    reserve: u64,
    /// Whether a `take` failed only because of the reserve.
    reserve_hit: bool,
}

impl Budget {
    pub(crate) fn new(limit: u64) -> Self {
        Budget {
            limit,
            left: limit,
            coded_left: u64::MAX,
            reserve: 0,
            reserve_hit: false,
        }
    }

    /// Keeps `bytes` of the limit for memory already held that could be
    /// freed: a `take` that the limit allows but that would reach into
    /// them fails, and [`reserve_hit`](Self::reserve_hit) says so. The
    /// caller can then free that memory and start again without the
    /// reserve, which decides as if there had never been one.
    pub(crate) fn reserve(&mut self, bytes: u64) {
        self.reserve = bytes;
    }

    /// Whether a `take` failed because of the reserve rather than the
    /// limit.
    pub(crate) fn reserve_hit(&self) -> bool {
        self.reserve_hit
    }

    /// Allows at most `bytes` more of coded data through
    /// [`take_coded`](Self::take_coded).
    pub(crate) fn limit_coded(&mut self, bytes: u64) {
        self.coded_left = bytes;
    }

    /// Accounts for `bytes` bytes of coded data, which both the memory
    /// limit and the output limit count, or fails if either would be
    /// passed.
    pub(crate) fn take_coded(&mut self, bytes: u64) -> Result<()> {
        self.take(bytes)?;
        if bytes > self.coded_left {
            return ojph_error!(
                OUTPUT_LIMIT_ERROR,
                "the coded data is longer than the output limit allows"
            );
        }
        self.coded_left -= bytes;
        Ok(())
    }

    /// The bytes that may still be accounted for.
    pub(crate) fn left(&self) -> u64 {
        self.left
    }

    /// The bytes accounted for so far.
    pub(crate) fn taken(&self) -> u64 {
        self.limit - self.left
    }

    /// Accounts for `bytes` bytes, or fails if the limit would be passed,
    /// or the reserve reached.
    pub(crate) fn take(&mut self, bytes: u64) -> Result<()> {
        if bytes > self.left {
            return ojph_error!(
                0x00070001,
                "this codestream needs more memory than the limit allows"
            );
        }
        if bytes > self.left.saturating_sub(self.reserve) {
            self.reserve_hit = true;
            return ojph_error!(
                0x00070001,
                "this codestream needs memory that kept memory holds"
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
