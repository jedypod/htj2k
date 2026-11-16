// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Sample-level kernels: the wavelet lifting steps (`ojph_transform.cpp`),
//! the colour transforms and sample-format conversions (`ojph_colour.cpp`),
//! and the conversions between line samples and code-block buffers
//! (`ojph_codestream_gen.cpp`, dispatched from `ojph_codeblock_fun.cpp`).
//!
//! Only OpenJPH's generic (`gen_*`) kernels are ported; the SIMD variants
//! are meant to produce the same results. Integer kernels are bit-exact
//! ports that use wrapping arithmetic wherever the C++ would overflow (which
//! is undefined behaviour there; the results match what the C++ produces
//! when compiled with `-fwrapv`, which is also what it produces on common
//! targets). Float kernels perform the same operations in the same order,
//! without fused multiply-add.

pub mod codeblock_fun;
pub mod colour;
pub mod wavelet;

#[cfg(test)]
mod tests;

/// Converts a float to `i32` by truncation, the way an x86 `cvttss2si`
/// instruction does: values outside the range of `i32`, and NaN, become
/// `i32::MIN`. This is the `(si32)` cast in OpenJPH's C++, whose result is
/// undefined there when the value is out of range; Rust's `as i32` would
/// saturate instead.
#[inline(always)]
pub(crate) fn f32_to_i32_trunc(val: f32) -> i32 {
    if (-2147483648.0f32..2147483648.0f32).contains(&val) {
        val as i32
    } else {
        i32::MIN
    }
}

/// `ojph_round` from `ojph_arch.h`, as compiled with GCC or MSVC:
/// `(si32)(val + (val >= 0.0f ? 0.5f : -0.5f))`. The final conversion
/// follows [`f32_to_i32_trunc`].
#[inline(always)]
pub(crate) fn ojph_round(val: f32) -> i32 {
    f32_to_i32_trunc(val + if val >= 0.0f32 { 0.5f32 } else { -0.5f32 })
}

/// `ojph_trunc` from `ojph_arch.h`: `(si32)(val)`, see
/// [`f32_to_i32_trunc`].
#[inline(always)]
pub(crate) fn ojph_trunc(val: f32) -> i32 {
    f32_to_i32_trunc(val)
}
