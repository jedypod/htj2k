// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Codestream marker segments and their parameters, after
//! `ojph_params.cpp` and `ojph_params_local.h`.

mod atk;

pub use atk::{LiftingStep, ParamAtk};
