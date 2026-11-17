// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Conversions between subband line samples and the sign-magnitude
//! code-block buffers, after the generic (`gen_*`) functions of
//! `codestream/ojph_codestream_gen.cpp` that `ojph_codeblock_fun.cpp`
//! selects.
//!
//! A code-block sample is a sign bit (the top bit) and a magnitude. For
//! the reversible path the magnitude is left-aligned so that its
//! `K_max` bits end just below the sign bit (it is shifted left by
//! `31 - K_max`, or `63 - K_max`); for the irreversible path it is the
//! quantised value, unshifted.
//!
//! The `delta_inv`/`delta` and `K_max` arguments that a kernel does not
//! use are kept so that all variants share one signature, as in the C++.
//! Each kernel processes `count` samples; slices must be at least that
//! long. OpenJPH has no generic irreversible 64-bit `tx_to_cb`.

use super::ojph_trunc;

/// `gen_mem_clear`: zeroes `addr`. (The C++ takes a byte count; here the
/// length of the slice is the count.)
pub fn mem_clear<T: Copy + Default>(addr: &mut [T]) {
    addr.fill(T::default());
}

/// `gen_find_max_val32`: the generic version returns `addr[0]` (the
/// maximum is kept in `addr[0]` by the callers).
pub fn find_max_val32(addr: &[u32]) -> u32 {
    addr[0]
}

/// `gen_find_max_val64`: returns `addr[0]`.
pub fn find_max_val64(addr: &[u64]) -> u64 {
    addr[0]
}

/// `gen_rev_tx_to_cb32`: converts `count` reversible samples to
/// sign-magnitude form shifted left by `31 - k_max`, and ORs the shifted
/// magnitudes into `max_val`. `delta_inv` is unused. Requires
/// `k_max <= 31` (otherwise the C++ shift is undefined; this port masks
/// the shift count to 5 bits). The same applies to the other reversible
/// conversions here, with `k_max <= 63` for the 64-bit ones.
pub fn rev_tx_to_cb32(
    sp: &[i32],
    dp: &mut [u32],
    k_max: u32,
    delta_inv: f32,
    count: u32,
    max_val: &mut u32,
) {
    #[cfg(feature = "simd")]
    super::vectorised::rev_tx_to_cb32(sp, dp, k_max, delta_inv, count, max_val);
    #[cfg(not(feature = "simd"))]
    rev_tx_to_cb32_scalar(sp, dp, k_max, delta_inv, count, max_val);
}

/// The scalar [`rev_tx_to_cb32`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn rev_tx_to_cb32_scalar(
    sp: &[i32],
    dp: &mut [u32],
    k_max: u32,
    _delta_inv: f32,
    count: u32,
    max_val: &mut u32,
) {
    let n = count as usize;
    let shift = 31u32.wrapping_sub(k_max);
    let mut tmax = *max_val;
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let sign = if v >= 0 { 0u32 } else { 0x8000_0000u32 };
        let val = v.unsigned_abs().wrapping_shl(shift);
        *d = sign | val;
        tmax |= val; // it is more efficient to use or than max
    }
    *max_val = tmax;
}

/// `gen_rev_tx_to_cb64`: as [`rev_tx_to_cb32`] for 64-bit samples, shifted
/// by `63 - k_max`.
pub fn rev_tx_to_cb64(
    sp: &[i64],
    dp: &mut [u64],
    k_max: u32,
    _delta_inv: f32,
    count: u32,
    max_val: &mut u64,
) {
    let n = count as usize;
    let shift = 63u32.wrapping_sub(k_max);
    let mut tmax = *max_val;
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let sign = if v >= 0 {
            0u64
        } else {
            0x8000_0000_0000_0000u64
        };
        let val = v.unsigned_abs().wrapping_shl(shift);
        *d = sign | val;
        tmax |= val;
    }
    *max_val = tmax;
}

/// `gen_irv_tx_to_cb32`: quantises `count` float samples by truncating
/// `v * delta_inv` toward zero (out-of-range values and NaN become
/// `i32::MIN`, as on x86), converts to sign-magnitude form and ORs the
/// magnitudes into `max_val`. `k_max` is unused.
pub fn irv_tx_to_cb32(
    sp: &[f32],
    dp: &mut [u32],
    k_max: u32,
    delta_inv: f32,
    count: u32,
    max_val: &mut u32,
) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_tx_to_cb32(sp, dp, k_max, delta_inv, count, max_val);
    #[cfg(not(feature = "simd"))]
    irv_tx_to_cb32_scalar(sp, dp, k_max, delta_inv, count, max_val);
}

/// The scalar [`irv_tx_to_cb32`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_tx_to_cb32_scalar(
    sp: &[f32],
    dp: &mut [u32],
    _k_max: u32,
    delta_inv: f32,
    count: u32,
    max_val: &mut u32,
) {
    let n = count as usize;
    let mut tmax = *max_val;
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let t = ojph_trunc(v * delta_inv);
        let sign = if t >= 0 { 0u32 } else { 0x8000_0000u32 };
        let val = t.unsigned_abs();
        *d = sign | val;
        tmax |= val;
    }
    *max_val = tmax;
}

/// `gen_rev_tx_from_cb32`: converts `count` sign-magnitude code-block
/// samples back to reversible samples, shifting the magnitude right by
/// `31 - k_max`. `delta` is unused.
pub fn rev_tx_from_cb32(sp: &[u32], dp: &mut [i32], k_max: u32, delta: f32, count: u32) {
    #[cfg(feature = "simd")]
    super::vectorised::rev_tx_from_cb32(sp, dp, k_max, delta, count);
    #[cfg(not(feature = "simd"))]
    rev_tx_from_cb32_scalar(sp, dp, k_max, delta, count);
}

/// The scalar [`rev_tx_from_cb32`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn rev_tx_from_cb32_scalar(
    sp: &[u32],
    dp: &mut [i32],
    k_max: u32,
    _delta: f32,
    count: u32,
) {
    let n = count as usize;
    let shift = 31u32.wrapping_sub(k_max);
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let val = ((v & 0x7FFF_FFFF).wrapping_shr(shift)) as i32;
        *d = if v & 0x8000_0000 != 0 {
            val.wrapping_neg()
        } else {
            val
        };
    }
}

/// `gen_rev_tx_from_cb64`: as [`rev_tx_from_cb32`] for 64-bit samples,
/// shifting by `63 - k_max`.
pub fn rev_tx_from_cb64(sp: &[u64], dp: &mut [i64], k_max: u32, _delta: f32, count: u32) {
    let n = count as usize;
    let shift = 63u32.wrapping_sub(k_max);
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let val = ((v & 0x7FFF_FFFF_FFFF_FFFF).wrapping_shr(shift)) as i64;
        *d = if v & 0x8000_0000_0000_0000 != 0 {
            val.wrapping_neg()
        } else {
            val
        };
    }
}

/// `gen_irv_tx_from_cb32`: dequantises `count` sign-magnitude code-block
/// samples, `±(magnitude as f32) * delta`. `k_max` is unused.
pub fn irv_tx_from_cb32(sp: &[u32], dp: &mut [f32], k_max: u32, delta: f32, count: u32) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_tx_from_cb32(sp, dp, k_max, delta, count);
    #[cfg(not(feature = "simd"))]
    irv_tx_from_cb32_scalar(sp, dp, k_max, delta, count);
}

/// The scalar [`irv_tx_from_cb32`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_tx_from_cb32_scalar(
    sp: &[u32],
    dp: &mut [f32],
    _k_max: u32,
    delta: f32,
    count: u32,
) {
    let n = count as usize;
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let val = (v & 0x7FFF_FFFF) as f32 * delta;
        *d = if v & 0x8000_0000 != 0 { -val } else { val };
    }
}

/// `gen_irv_tx_from_cb64`: as [`irv_tx_from_cb32`] for 64-bit code-block
/// samples (the magnitude is rounded to `f32`).
pub fn irv_tx_from_cb64(sp: &[u64], dp: &mut [f32], _k_max: u32, delta: f32, count: u32) {
    let n = count as usize;
    for (d, &v) in dp[..n].iter_mut().zip(&sp[..n]) {
        let val = (v & 0x7FFF_FFFF_FFFF_FFFF) as f32 * delta;
        *d = if v & 0x8000_0000_0000_0000 != 0 {
            -val
        } else {
            val
        };
    }
}
