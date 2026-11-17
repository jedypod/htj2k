// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoding side of the codestream, after the write paths of
//! `src/core/codestream` in OpenJPH.
//!
//! OpenJPH keeps encoding and decoding in the same classes (`tile`,
//! `tile_comp`, `resolution`, `subband`, `precinct`, `codeblock`). The
//! decoder of this crate has its own, leaner versions of them (one reused
//! row of code blocks, coded bytes left in the input), so the encoder has
//! its own tree here, with the same names and the same structure as the
//! C++ write paths. Neither side pays for the other's state.

#[allow(clippy::module_inception)]
mod codestream;

pub use codestream::Encoder;
