// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Sample-level kernels written for the compiler to vectorise (the
//! `simd` feature).
//!
//! Each function here does what a generic kernel of the parent modules
//! does, with the same floating-point operations in the same order (Rust
//! never contracts or reassociates them), and is compiled by
//! `multiversion` for the x86_64 baseline and for AVX2, chosen once per
//! process from the CPU's features. The loops are plain safe Rust laid out
//! so that LLVM vectorises them: no branches in the body, and slices cut
//! to their length before the loop so that no index needs a check.
//!
//! Table lookups cannot be vectorised that way: a lookup that might be
//! out of bounds must panic in order, and the gather instructions take
//! raw pointers. Kernels with lookups therefore work on chunks of
//! [`CHUNK`] samples in three passes: the indices with vectors, the
//! lookups one by one (each with its bounds check, so a bad table panics
//! as in the scalar code), and the rest of the arithmetic with vectors.
//!
//! Nor does LLVM vectorise a conversion from float to integer on x86: it
//! compiles Rust's saturating `as i32` and `as u32` one lane at a time.
//! [`trunc_i32`] and [`trunc_u32`] give the same results from the float's
//! bits, with shifts and selects, which it does vectorise.
//!
//! Unlike the block coder's AVX2 kernels, these need no `unsafe` and no
//! `#[target_feature]` caller, so builds that enable AVX2 at compile time
//! (`-C target-cpu=native`) vectorise them too.

use super::colour::{binary_complement, nlt_bias, nlt_to_float_t, ojph_max, ojph_min, NltLut};
use multiversion::multiversion;

/// Samples per chunk of the kernels with table lookups.
const CHUNK: usize = 64;

/// The magnitude bits of a finite float `x` below 2^32 truncated toward
/// zero (`|x| as u32`), and its biased exponent: from the bits, with
/// shifts the compiler vectorises.
#[inline(always)]
fn trunc_bits(x: f32) -> (u32, u32) {
    let b = x.to_bits();
    let e = (b >> 23) & 0xFF;
    let m = (b & 0x7F_FFFF) | 0x80_0000;
    // m * 2^(e - 150): shifted left for e >= 150 (only used below 159,
    // so by at most 8), right otherwise (all bits gone from e <= 119)
    let l = m.wrapping_shl(e.wrapping_sub(150));
    let r = m >> 150u32.saturating_sub(e).min(31);
    (if e >= 150 { l } else { r }, e)
}

/// [`super::f32_to_i32_trunc`] (`cvttss2si`): truncation toward zero,
/// `i32::MIN` out of range and for NaN.
#[inline(always)]
pub(super) fn trunc_i32(x: f32) -> i32 {
    let (mag, e) = trunc_bits(x);
    let v = if x.is_sign_negative() {
        mag.wrapping_neg()
    } else {
        mag
    };
    // |x| >= 2^31, infinities and NaN
    if e >= 158 {
        i32::MIN
    } else {
        v as i32
    }
}

/// `x as u32` (the scalar kernels' `floor_u32`): truncation toward zero,
/// saturating, 0 for NaN.
#[inline(always)]
pub(super) fn trunc_u32(x: f32) -> u32 {
    let (mag, e) = trunc_bits(x);
    if x.is_nan() || x.is_sign_negative() {
        0
    } else if e >= 159 {
        u32::MAX
    } else {
        mag
    }
}

/// `lut[k[i]]` and `lut[k[i] + 1]` into `a[i]` and `b[i]`. When every
/// index is in bounds (checked once, on the largest), the loop has no
/// checks: clamping to the bound changes no index but lets the compiler
/// drop them. Otherwise each lookup is checked in order, so the same one
/// panics as in the scalar code.
#[inline(always)]
fn lookup_pairs(lut: &[f32], k: &[u32], a: &mut [f32], b: &mut [f32]) {
    let n = k.len();
    let (a, b) = (&mut a[..n], &mut b[..n]);
    let k_max = k.iter().fold(0u32, |m, &k| m.max(k)) as usize;
    if k_max + 1 < lut.len() {
        let last = lut.len() - 2;
        for i in 0..n {
            let j = (k[i] as usize).min(last);
            a[i] = lut[j];
            b[i] = lut[j + 1];
        }
    } else {
        for i in 0..n {
            a[i] = lut[k[i] as usize];
            b[i] = lut[k[i] as usize + 1];
        }
    }
}

/// `lut[k[i]]` into `k[i]`, as [`lookup_pairs`].
#[inline(always)]
fn lookup_indices(lut: &[u16], k: &mut [u32]) {
    let k_max = k.iter().fold(0u32, |m, &k| m.max(k)) as usize;
    if k_max < lut.len() {
        let last = lut.len() - 1;
        for k in k.iter_mut() {
            *k = u32::from(lut[(*k as usize).min(last)]);
        }
    } else {
        for k in k.iter_mut() {
            *k = u32::from(lut[*k as usize]);
        }
    }
}

/// `lut[k[i]]`, `lut[k[i] + 1]` and `lut[k[i] + 2]` into `a[i]`, `b[i]`
/// and `c[i]`, as [`lookup_pairs`].
#[inline(always)]
fn lookup_triples(lut: &[f32], k: &[u32], a: &mut [f32], b: &mut [f32], c: &mut [f32]) {
    let n = k.len();
    let (a, b, c) = (&mut a[..n], &mut b[..n], &mut c[..n]);
    let k_max = k.iter().fold(0u32, |m, &k| m.max(k)) as usize;
    if k_max + 2 < lut.len() {
        let last = lut.len() - 3;
        for i in 0..n {
            let j = (k[i] as usize).min(last);
            a[i] = lut[j];
            b[i] = lut[j + 1];
            c[i] = lut[j + 2];
        }
    } else {
        for i in 0..n {
            a[i] = lut[k[i] as usize];
            b[i] = lut[k[i] as usize + 1];
            c[i] = lut[k[i] as usize + 2];
        }
    }
}

/// [`super::ojph_round`] with [`trunc_i32`].
#[inline(always)]
pub(super) fn round_i32(val: f32) -> i32 {
    trunc_i32(val + if val >= 0.0f32 { 0.5f32 } else { -0.5f32 })
}

/// `local_gen_irv_convert_to_integer_nlt2or4` (see
/// `colour::irv_convert_to_integer_nlt2or4`), on `sp` and `dp` of the same
/// length.
#[multiversion(targets("x86_64+avx2+bmi1+bmi2+lzcnt+popcnt"))]
pub(super) fn irv_convert_to_integer_nlt2or4<const TYPE4: bool>(
    sp: &[f32],
    dp: &mut [i32],
    rec: &NltLut<'_>,
) {
    let mul = rec.multiplier;
    let d_min = rec.fd_min;
    let d_max = rec.fd_max;
    let delta = rec.delta;
    let inv_delta = rec.inv_delta;
    let lut = rec.dec_points;
    let signed = rec.is_signed;
    let bd = u32::from(rec.bit_depth);
    let bias = nlt_bias(bd);
    let half = 1u64.wrapping_shl(bd.wrapping_sub(1)) as f32;

    for (sp, dp) in sp.chunks(CHUNK).zip(dp.chunks_mut(CHUNK)) {
        let n = sp.len();
        let (mut t, mut k) = ([0f32; CHUNK], [0u32; CHUNK]);
        let (t, k) = (&mut t[..n], &mut k[..n]);
        for ((t, k), &s) in t.iter_mut().zip(k.iter_mut()).zip(sp) {
            let mut v = s + 0.5f32; // convert to [0, 1]
            v = ojph_max(v, d_min);
            v = ojph_min(v, d_max);
            *t = v;
            *k = trunc_u32((v - d_min) * inv_delta);
        }
        let (mut t_k, mut t_kp1) = ([0f32; CHUNK], [0f32; CHUNK]);
        let (t_k, t_kp1) = (&mut t_k[..n], &mut t_kp1[..n]);
        lookup_pairs(lut, k, t_k, t_kp1);
        let dp = &mut dp[..n];
        for i in 0..n {
            let d_k = d_min + k[i] as f32 * delta;
            let z = t_k[i] + (t[i] - d_k) * inv_delta * (t_kp1[i] - t_k[i]);
            dp[i] = if signed {
                let v = round_i32(z * mul - half);
                if TYPE4 {
                    binary_complement(v, bias)
                } else {
                    v
                }
            } else {
                round_i32(z * mul)
            };
        }
    }
}

/// `local_gen_irv_convert_to_float_nlt2or4_precise_1step` (see
/// `colour::irv_convert_to_float_nlt2or4_precise_1step`).
#[multiversion(targets("x86_64+avx2+bmi1+bmi2+lzcnt+popcnt"))]
pub(super) fn irv_convert_to_float_nlt2or4_precise_1step<const TYPE4: bool>(
    sp: &[i32],
    dp: &mut [f32],
    rec: &NltLut<'_>,
) {
    let ft_min = rec.ft_min;
    let fd_min = rec.fd_min;
    let delta = rec.delta;
    let inv_delta = rec.inv_delta;
    let indices = rec.precise_enc_dec_indices;
    let lut = rec.precise_dec_lut;
    let signed = rec.is_signed;
    let bias = nlt_bias(u32::from(rec.bit_depth));

    for (sp, dp) in sp.chunks(CHUNK).zip(dp.chunks_mut(CHUNK)) {
        let n = sp.len();
        let (mut t, mut k) = ([0f32; CHUNK], [0u32; CHUNK]);
        let (t, k) = (&mut t[..n], &mut k[..n]);
        for ((t, k), &v) in t.iter_mut().zip(k.iter_mut()).zip(sp) {
            let v = nlt_to_float_t::<TYPE4>(v, signed, bias, rec);
            *t = v;
            *k = trunc_u32((v - ft_min) * inv_delta);
        }
        lookup_indices(indices, k);
        let (mut y0, mut y1, mut y2) = ([0f32; CHUNK], [0f32; CHUNK], [0f32; CHUNK]);
        let (y0, y1, y2) = (&mut y0[..n], &mut y1[..n], &mut y2[..n]);
        lookup_triples(lut, k, y0, y1, y2);
        let dp = &mut dp[..n];
        for i in 0..n {
            // one step of the search
            let step = t[i] > y1[i];
            let (a, b) = if step { (y1[i], y2[i]) } else { (y0[i], y1[i]) };
            let k = if step { k[i] + 1 } else { k[i] };
            let frac = if b > a {
                (t[i] - a) * delta / (b - a)
            } else {
                0.0f32
            };
            let y = fd_min + k as f32 * delta + frac;
            dp[i] = y - 0.5f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conversions against the scalar code's, for every `step`-th
    /// bit pattern and every one near the edges of the ranges.
    fn conversions_match(step: usize) {
        let edges = [
            0u64,
            0x3F00_0000,
            0x4B00_0000,
            0x4F00_0000,
            0x4F80_0000,
            0x7F80_0000,
        ];
        let near = edges.iter().flat_map(|&e| {
            let n = e | 1 << 31;
            (e.saturating_sub(300)..e + 300).chain(n - 300..n + 300)
        });
        for bits in (0..=u32::MAX as u64).step_by(step).chain(near) {
            let x = f32::from_bits(bits as u32);
            assert_eq!(
                trunc_i32(x),
                super::super::f32_to_i32_trunc(x),
                "{bits:#010x}"
            );
            assert_eq!(trunc_u32(x), x as u32, "{bits:#010x}");
            assert_eq!(round_i32(x), super::super::ojph_round(x), "{bits:#010x}");
        }
    }

    #[test]
    fn conversions_match_the_casts() {
        conversions_match(4099);
    }

    /// All 2^32 bit patterns. Ignored by default; run it with `cargo test
    /// --release --features simd --lib conversions -- --ignored`.
    #[test]
    #[ignore]
    fn conversions_match_the_casts_everywhere() {
        conversions_match(1);
    }
}
