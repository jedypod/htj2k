// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! High-Throughput JPEG 2000 (HTJ2K, ITU-T T.814 | ISO/IEC 15444-15)
//! codestream codec, ported from OpenJPH 0.33.0 to safe Rust.

#![forbid(unsafe_code)]

// Lets the unit tests share helpers with the integration tests, which
// name the crate `htj2k`.
#[cfg(test)]
extern crate self as htj2k;

mod error;
pub mod file;
pub mod line;
pub mod types;

pub mod codestream;
pub mod coding;
pub mod params;
pub mod transform;

#[cfg(feature = "simd")]
mod simd;

pub use error::{Error, Result};
