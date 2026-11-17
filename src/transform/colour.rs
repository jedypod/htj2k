// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Colour transforms and sample-format conversions, after the generic
//! (`gen_*`) functions of `transform/ojph_colour.cpp`.
//!
//! Line offsets (`src_line_offset`, `dst_line_offset`) are in samples from
//! logical index 0 of the line. Every line must hold at least
//! `offset + width` (or `repeat`) logical samples.

use super::ojph_round;
use crate::line::LineBuf;

/// The constants of the irreversible colour transform, `CT_CNST` in the
/// C++, computed the same way (in `f64` from the `f32` alphas, then
/// rounded to `f32`).
pub mod ct_cnst {
    pub const ALPHA_RF: f32 = 0.299f32;
    pub const ALPHA_GF: f32 = 0.587f32;
    pub const ALPHA_BF: f32 = 0.114f32;
    pub const BETA_CB_F: f32 = (0.5 / (1.0 - ALPHA_BF as f64)) as f32;
    pub const BETA_CR_F: f32 = (0.5 / (1.0 - ALPHA_RF as f64)) as f32;
    pub const GAMMA_CB2G: f32 =
        (2.0 * (ALPHA_BF as f64) * (1.0 - ALPHA_BF as f64) / (ALPHA_GF as f64)) as f32;
    pub const GAMMA_CR2G: f32 =
        (2.0 * (ALPHA_RF as f64) * (1.0 - ALPHA_RF as f64) / (ALPHA_GF as f64)) as f32;
    pub const GAMMA_CB2B: f32 = (2.0 * (1.0 - ALPHA_BF as f64)) as f32;
    pub const GAMMA_CR2R: f32 = (2.0 * (1.0 - ALPHA_RF as f64)) as f32;
}

/// `nlt_rec::nonlinearity::OJPH_NLT_LUT_STYLE_NLT`.
pub const NLT_LUT_STYLE: u8 = 2;
/// `nlt_rec::nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT`.
pub const NLT_BINARY_COMPLEMENT_PLUS_LUT: u8 = 4;

/// The members of OpenJPH's `nlt_rec` that the LUT-style nonlinearity
/// kernels ([`irv_convert_to_integer_nlt`], [`irv_convert_to_float_nlt`])
/// read. The caller builds one from its own `nlt_rec` equivalent.
///
/// All table slices start at the table's index 0 (the C++ pointer), not at
/// the padding the C++ allocates before some tables.
#[derive(Debug, Clone, Copy)]
pub struct NltLut<'a> {
    /// `get_type()` (`Tnlt`): [`NLT_LUT_STYLE`] or
    /// [`NLT_BINARY_COMPLEMENT_PLUS_LUT`].
    pub nlt_type: u8,
    /// `get_bit_depth()`: `(BDnlt & 0x7F) + 1`.
    pub bit_depth: u8,
    /// `is_signed()`: `(BDnlt & 0x80) != 0`.
    pub is_signed: bool,
    /// `multiplier`.
    pub multiplier: f32,
    /// `fd_min`.
    pub fd_min: f32,
    /// `fd_max` (to-integer only).
    pub fd_max: f32,
    /// `delta`.
    pub delta: f32,
    /// `inv_delta`.
    pub inv_delta: f32,
    /// `dec_points` (to-integer only). Indexed by `k` and `k + 1`, where
    /// `k = floor((t - fd_min) * inv_delta)` for `t` clamped to
    /// `[fd_min, fd_max]`.
    pub dec_points: &'a [f32],
    /// `ft_min` (to-float only).
    pub ft_min: f32,
    /// `ft_max` (to-float only).
    pub ft_max: f32,
    /// `approx_encoding_nlt` (to-float only): selects the approximate table
    /// over the precise ones.
    pub approx_encoding_nlt: bool,
    /// `approx_enc_points` (to-float, approximate). Indexed by `k` and
    /// `k + 1`, with `k = floor((t - ft_min) * inv_delta)`.
    pub approx_enc_points: &'a [f32],
    /// `precise_max_steps` (to-float, precise): 1 selects the one-step
    /// variant, anything else the search.
    pub precise_max_steps: u32,
    /// `precise_enc_dec_indices` (to-float, precise). Indexed by `k` as for
    /// `approx_enc_points`.
    pub precise_enc_dec_indices: &'a [u16],
    /// `precise_dec_lut` (to-float, precise). For an index `k` read from
    /// `precise_enc_dec_indices`, the one-step variant reads `k`, `k + 1`
    /// and `k + 2`; the search reads `k` to
    /// `min(k + precise_max_steps + 1, num_points)` inclusive.
    pub precise_dec_lut: &'a [f32],
    /// `num_points` (to-float, precise search only).
    pub num_points: u32,
}

/// `(a > b) ? a : b`, `ojph_max` in the C++ (not `f32::max`, which treats
/// NaN differently).
#[inline(always)]
fn ojph_max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

/// `(a < b) ? a : b`, `ojph_min` in the C++.
#[inline(always)]
fn ojph_min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

/// `(ui32)(1ULL << (bit_depth - 1)) + 1u`, the bias of the binary-complement
/// (type 3 and 4) nonlinearities.
#[inline(always)]
fn nlt_bias(bit_depth: u32) -> u32 {
    (1u64.wrapping_shl(bit_depth.wrapping_sub(1)) as u32).wrapping_add(1)
}

/// `(si32)(1ULL << (bit_depth - 1))`.
#[inline(always)]
fn half_of(bit_depth: u32) -> i32 {
    1u64.wrapping_shl(bit_depth.wrapping_sub(1)) as i32
}

/// `v >= 0 ? v : (si32)(-(ui32)v - bias)`.
#[inline(always)]
fn binary_complement(v: i32, bias: u32) -> i32 {
    if v >= 0 {
        v
    } else {
        (v as u32).wrapping_neg().wrapping_sub(bias) as i32
    }
}

/// `(ui32)floorf(x)`. For the values the kernels produce (non-negative,
/// below 2^32) this is exact; out of range, Rust saturates (and maps NaN
/// to 0) where the C++ is undefined.
///
/// The cast alone gives the same result for every `f32`: it truncates
/// toward zero, which is the floor for `x >= 0`, and for `x < 0` both the
/// truncation and the floor saturate to 0. Without the `floor`, the
/// x86-64 baseline (which has no rounding instruction) does not call
/// libm's `floorf` once per sample. `tests::floor_u32_is_floor` checks
/// all 2^32 bit patterns.
#[inline(always)]
fn floor_u32(x: f32) -> u32 {
    x as u32
}

// ---------------------------------------------------------------------------
// Reversible
// ---------------------------------------------------------------------------

/// `gen_rev_convert`: copies `width` integer samples from `src_line`
/// (starting at `src_line_offset`) to `dst_line` (starting at
/// `dst_line_offset`), adding `shift`. Supported combinations are 32-bit to
/// 32-bit (`shift` truncated to `i32`), 32-bit to 64-bit, and 64-bit to
/// 32-bit (the sum truncated to `i32`). Additions wrap.
pub fn rev_convert(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    shift: i64,
    width: u32,
) {
    let (so, doff, w) = (
        src_line_offset as usize,
        dst_line_offset as usize,
        width as usize,
    );
    if src_line.is_i32() {
        let sp = &src_line.i32()[so..so + w];
        if dst_line.is_i32() {
            let dp = &mut dst_line.i32_mut()[doff..doff + w];
            let s = shift as i32;
            for (d, &v) in dp.iter_mut().zip(sp) {
                *d = v.wrapping_add(s);
            }
        } else {
            let dp = &mut dst_line.i64_mut()[doff..doff + w];
            for (d, &v) in dp.iter_mut().zip(sp) {
                *d = (v as i64).wrapping_add(shift);
            }
        }
    } else {
        let sp = &src_line.i64()[so..so + w];
        let dp = &mut dst_line.i32_mut()[doff..doff + w];
        for (d, &v) in dp.iter_mut().zip(sp) {
            *d = v.wrapping_add(shift) as i32;
        }
    }
}

/// `gen_rev_convert_nlt_type3`: as [`rev_convert`], but applies the
/// binary-complement nonlinearity: non-negative samples are copied, a
/// negative sample `v` becomes `-v - shift`. Arithmetic wraps.
pub fn rev_convert_nlt_type3(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    shift: i64,
    width: u32,
) {
    let (so, doff, w) = (
        src_line_offset as usize,
        dst_line_offset as usize,
        width as usize,
    );
    if src_line.is_i32() {
        let sp = &src_line.i32()[so..so + w];
        if dst_line.is_i32() {
            let dp = &mut dst_line.i32_mut()[doff..doff + w];
            let s = shift as i32;
            for (d, &v) in dp.iter_mut().zip(sp) {
                *d = if v >= 0 {
                    v
                } else {
                    v.wrapping_neg().wrapping_sub(s)
                };
            }
        } else {
            let dp = &mut dst_line.i64_mut()[doff..doff + w];
            for (d, &v) in dp.iter_mut().zip(sp) {
                let v = v as i64;
                *d = if v >= 0 {
                    v
                } else {
                    v.wrapping_neg().wrapping_sub(shift)
                };
            }
        }
    } else {
        let sp = &src_line.i64()[so..so + w];
        let dp = &mut dst_line.i32_mut()[doff..doff + w];
        for (d, &v) in dp.iter_mut().zip(sp) {
            *d = (if v >= 0 {
                v
            } else {
                v.wrapping_neg().wrapping_sub(shift)
            }) as i32;
        }
    }
}

/// `gen_rev_encode_nlt`. In OpenJPH 0.33.0 this is only `assert(0)`: the
/// LUT-style nonlinearities are implemented for the irreversible path
/// only, and `param_nlt::check_validity` refuses to write a codestream that
/// would reach this. It reads no `nlt_rec` members. This port does nothing
/// (it does not assert).
#[allow(clippy::too_many_arguments)]
pub fn rev_encode_nlt(
    _src_line: &LineBuf,
    _src_line_offset: u32,
    _dst_line: &mut LineBuf,
    _bit_depth: u32,
    _is_signed: bool,
    _width: u32,
    _rec: &NltLut<'_>,
) {
}

/// `gen_rev_decode_nlt`. In OpenJPH 0.33.0 this is only `assert(0)`, even
/// though its comment says the samples are passed through unchanged: it
/// writes nothing to `dst_line`. It is reachable from a codestream that
/// combines a LUT-style nonlinearity with the reversible wavelet. It reads
/// no `nlt_rec` members. This port does nothing (it does not assert).
#[allow(clippy::too_many_arguments)]
pub fn rev_decode_nlt(
    _src_line: &LineBuf,
    _src_line_offset: u32,
    _dst_line: &mut LineBuf,
    _dst_line_offset: u32,
    _bit_depth: u32,
    _is_signed: bool,
    _width: u32,
    _rec: &NltLut<'_>,
) {
}

// ---------------------------------------------------------------------------
// Irreversible, no nonlinearity or binary complement (types 0 and 3)
// ---------------------------------------------------------------------------

/// `local_gen_irv_convert_to_integer_nlt0or3`.
fn irv_convert_to_integer_nlt0or3<const NLT_TYPE3: bool>(
    src_line: &LineBuf,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    let (doff, w) = (dst_line_offset as usize, width as usize);
    let sp = &src_line.f32()[..w];
    let dp = &mut dst_line.i32_mut()[doff..doff + w];
    // Converting to integer may exceed the range of a 32-bit integer, so
    // values outside the half-closed interval [-0.5, 0.5) are limited to
    // the extremes the bit depth supports.
    let sh = 32u32.wrapping_sub(bit_depth);
    let neg_limit = i32::MIN.wrapping_shr(sh);
    let mul = 1u64.wrapping_shl(bit_depth) as f32;
    let fl_up_lim = -(neg_limit as f32); // val < upper
    let fl_low_lim = neg_limit as f32; // val >= lower
    let s32_up_lim = i32::MAX.wrapping_shr(sh);
    let s32_low_lim = i32::MIN.wrapping_shr(sh);

    if is_signed {
        let bias = nlt_bias(bit_depth);
        for (d, &s) in dp.iter_mut().zip(sp) {
            let t = s * mul;
            let mut v = ojph_round(t);
            v = if t >= fl_low_lim { v } else { s32_low_lim };
            v = if t < fl_up_lim { v } else { s32_up_lim };
            if NLT_TYPE3 {
                v = binary_complement(v, bias);
            }
            *d = v;
        }
    } else {
        let half = half_of(bit_depth);
        for (d, &s) in dp.iter_mut().zip(sp) {
            let t = s * mul;
            let mut v = ojph_round(t);
            v = if t >= fl_low_lim { v } else { s32_low_lim };
            v = if t < fl_up_lim { v } else { s32_up_lim };
            *d = v.wrapping_add(half);
        }
    }
}

/// `gen_irv_convert_to_integer`: converts `width` float samples of
/// `src_line` (in `[-0.5, 0.5)`) to integers of `bit_depth` bits
/// (1..=32), written to `dst_line` (32-bit) from `dst_line_offset`, with
/// saturation; unsigned results are offset by `2^(bit_depth-1)`.
pub fn irv_convert_to_integer(
    src_line: &LineBuf,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    irv_convert_to_integer_nlt0or3::<false>(
        src_line,
        dst_line,
        dst_line_offset,
        bit_depth,
        is_signed,
        width,
    );
}

/// `gen_irv_convert_to_integer_nlt_type3`: as [`irv_convert_to_integer`],
/// then applies the binary-complement nonlinearity to signed samples.
pub fn irv_convert_to_integer_nlt_type3(
    src_line: &LineBuf,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    irv_convert_to_integer_nlt0or3::<true>(
        src_line,
        dst_line,
        dst_line_offset,
        bit_depth,
        is_signed,
        width,
    );
}

/// `local_gen_irv_convert_to_float_nlt0or3`.
fn irv_convert_to_float_nlt0or3<const NLT_TYPE3: bool>(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    let (so, w) = (src_line_offset as usize, width as usize);
    let sp = &src_line.i32()[so..so + w];
    let dp = &mut dst_line.f32_mut()[..w];
    let mul = (1.0f64 / (1u64.wrapping_shl(bit_depth) as f64)) as f32;

    if is_signed {
        let bias = nlt_bias(bit_depth);
        for (d, &v) in dp.iter_mut().zip(sp) {
            let v = if NLT_TYPE3 {
                binary_complement(v, bias)
            } else {
                v
            };
            *d = v as f32 * mul;
        }
    } else {
        let half = half_of(bit_depth);
        for (d, &v) in dp.iter_mut().zip(sp) {
            let v = v.wrapping_sub(half);
            *d = v as f32 * mul;
        }
    }
}

/// `gen_irv_convert_to_float`: converts `width` integer samples of
/// `bit_depth` bits (1..=32) from the 32-bit line `src_line` (starting at
/// `src_line_offset`) to floats in `[-0.5, 0.5)` in `dst_line`; unsigned
/// samples are first offset by `-2^(bit_depth-1)`.
pub fn irv_convert_to_float(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    irv_convert_to_float_nlt0or3::<false>(
        src_line,
        src_line_offset,
        dst_line,
        bit_depth,
        is_signed,
        width,
    );
}

/// `gen_irv_convert_to_float_nlt_type3`: as [`irv_convert_to_float`], but
/// first undoes the binary-complement nonlinearity on signed samples.
pub fn irv_convert_to_float_nlt_type3(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    bit_depth: u32,
    is_signed: bool,
    width: u32,
) {
    irv_convert_to_float_nlt0or3::<true>(
        src_line,
        src_line_offset,
        dst_line,
        bit_depth,
        is_signed,
        width,
    );
}

// ---------------------------------------------------------------------------
// Irreversible, LUT-style nonlinearities (types 2 and 4)
// ---------------------------------------------------------------------------

/// `local_gen_irv_convert_to_integer_nlt2or4`. `bit_depth` and `is_signed`
/// are unused (the record's own are used), as in the C++.
fn irv_convert_to_integer_nlt2or4<const TYPE4: bool>(
    src_line: &LineBuf,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    width: u32,
    rec: &NltLut<'_>,
) {
    let (doff, w) = (dst_line_offset as usize, width as usize);
    let sp = &src_line.f32()[..w];
    let dp = &mut dst_line.i32_mut()[doff..doff + w];

    let mul = rec.multiplier;
    let d_min = rec.fd_min;
    let d_max = rec.fd_max;
    let delta = rec.delta;
    let inv_delta = rec.inv_delta;
    let lut = rec.dec_points;

    let interp = |s: f32, offset: f32| -> f32 {
        let mut t = s + offset; // convert to [0, 1]
        t = ojph_max(t, d_min);
        t = ojph_min(t, d_max);
        let k = floor_u32((t - d_min) * inv_delta);
        let d_k = d_min + k as f32 * delta;
        let t_k = lut[k as usize];
        let t_kp1 = lut[k as usize + 1];
        t_k + (t - d_k) * inv_delta * (t_kp1 - t_k)
    };

    if rec.is_signed {
        let bd = u32::from(rec.bit_depth);
        let bias = nlt_bias(bd);
        let half = 1u64.wrapping_shl(bd.wrapping_sub(1)) as f32;
        for (d, &s) in dp.iter_mut().zip(sp) {
            let z = interp(s, 0.5f32);
            let mut v = ojph_round(z * mul - half);
            if TYPE4 {
                v = binary_complement(v, bias);
            }
            *d = v;
        }
    } else {
        for (d, &s) in dp.iter_mut().zip(sp) {
            let z = interp(s, 0.5f32);
            *d = ojph_round(z * mul);
        }
    }
}

/// `gen_irv_convert_to_integer_nlt`: converts `width` float samples of
/// `src_line` to integers in `dst_line` (from `dst_line_offset`) through
/// the LUT-style nonlinearity `rec` (type 2 or 4). Reads `nlt_type`,
/// `bit_depth`, `is_signed`, `multiplier`, `fd_min`, `fd_max`, `delta`,
/// `inv_delta` and `dec_points` of `rec`; `bit_depth` and `is_signed`
/// arguments are unused, as in the C++. Other types do nothing (the C++
/// asserts).
#[allow(clippy::too_many_arguments)]
pub fn irv_convert_to_integer_nlt(
    src_line: &LineBuf,
    dst_line: &mut LineBuf,
    dst_line_offset: u32,
    _bit_depth: u32,
    _is_signed: bool,
    width: u32,
    rec: &NltLut<'_>,
) {
    if rec.nlt_type == NLT_LUT_STYLE {
        irv_convert_to_integer_nlt2or4::<false>(src_line, dst_line, dst_line_offset, width, rec);
    } else if rec.nlt_type == NLT_BINARY_COMPLEMENT_PLUS_LUT {
        irv_convert_to_integer_nlt2or4::<true>(src_line, dst_line, dst_line_offset, width, rec);
    } else {
        debug_assert!(
            false,
            "irv_convert_to_integer_nlt: not a LUT-style nonlinearity"
        );
    }
}

/// The integer-to-`t` front end shared by the to-float LUT kernels: the
/// optional binary complement, scaling, offset and clamping to
/// `[ft_min, ft_max]`.
#[inline(always)]
fn nlt_to_float_t<const TYPE4: bool>(v: i32, signed: bool, bias: u32, rec: &NltLut<'_>) -> f32 {
    let mut t = if signed {
        let v = if TYPE4 { binary_complement(v, bias) } else { v };
        v as f32 * rec.multiplier + 0.5f32 // convert to [0, 1]
    } else {
        v as f32 * rec.multiplier // it is in [0, 1]
    };
    t = ojph_max(t, rec.ft_min);
    t = ojph_min(t, rec.ft_max);
    t
}

/// `local_gen_irv_convert_to_float_nlt2or4`, the approximate inverse.
fn irv_convert_to_float_nlt2or4_approx<const TYPE4: bool>(
    sp: &[i32],
    dp: &mut [f32],
    rec: &NltLut<'_>,
) {
    let ft_min = rec.ft_min;
    let delta = rec.delta;
    let inv_delta = rec.inv_delta;
    let lut = rec.approx_enc_points;
    let signed = rec.is_signed;
    let bias = nlt_bias(u32::from(rec.bit_depth));
    for (d, &v) in dp.iter_mut().zip(sp) {
        let t = nlt_to_float_t::<TYPE4>(v, signed, bias, rec);
        let k = floor_u32((t - ft_min) * inv_delta);
        let d_k = ft_min + k as f32 * delta;
        let t_k = lut[k as usize];
        let t_kp1 = lut[k as usize + 1];
        let y = t_k + (t - d_k) * inv_delta * (t_kp1 - t_k);
        *d = y - 0.5f32;
    }
}

/// `local_gen_irv_convert_to_float_nlt2or4_precise_1step`.
fn irv_convert_to_float_nlt2or4_precise_1step<const TYPE4: bool>(
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
    for (d, &v) in dp.iter_mut().zip(sp) {
        let t = nlt_to_float_t::<TYPE4>(v, signed, bias, rec);
        let mut k = floor_u32((t - ft_min) * inv_delta);
        k = u32::from(indices[k as usize]);
        let mut y0 = lut[k as usize]; // lookup 0
        let mut y1 = lut[k as usize + 1]; // lookup 1
        let y2 = lut[k as usize + 2]; // lookup 2: one-step search
        if t > y1 {
            y0 = y1;
            y1 = y2;
            k += 1;
        }
        let y = fd_min
            + k as f32 * delta
            + if y1 > y0 {
                (t - y0) * delta / (y1 - y0)
            } else {
                0.0f32
            };
        *d = y - 0.5f32;
    }
}

/// `local_gen_irv_convert_to_float_nlt2or4_precise_search`.
fn irv_convert_to_float_nlt2or4_precise_search<const TYPE4: bool>(
    sp: &[i32],
    dp: &mut [f32],
    rec: &NltLut<'_>,
) {
    debug_assert!(rec.precise_max_steps > 1);
    let ft_min = rec.ft_min;
    let fd_min = rec.fd_min;
    let delta = rec.delta;
    let inv_delta = rec.inv_delta;
    let indices = rec.precise_enc_dec_indices;
    let lut = rec.precise_dec_lut;
    let num_points = rec.num_points;
    let signed = rec.is_signed;
    let bias = nlt_bias(u32::from(rec.bit_depth));
    for (d, &v) in dp.iter_mut().zip(sp) {
        let t = nlt_to_float_t::<TYPE4>(v, signed, bias, rec);
        let mut k = floor_u32((t - ft_min) * inv_delta);
        k = u32::from(indices[k as usize]);

        let mut lo = k;
        let mut hi = k
            .wrapping_add(rec.precise_max_steps)
            .wrapping_add(1)
            .min(num_points);
        while hi.wrapping_sub(lo) > 1 {
            let mid = lo.wrapping_add(hi) >> 1;
            if t >= lut[mid as usize] {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        let y0 = lut[lo as usize];
        let y1 = lut[hi as usize];
        let y = fd_min
            + lo as f32 * delta
            + if y1 > y0 {
                (t - y0) * delta / (y1 - y0)
            } else {
                0.0f32
            };
        *d = y - 0.5f32;
    }
}

/// `gen_irv_convert_to_float_nlt`: converts `width` integer samples of the
/// 32-bit line `src_line` (from `src_line_offset`) to floats in
/// `dst_line` through the inverse of the LUT-style nonlinearity `rec`
/// (type 2 or 4), using the approximate table if
/// `rec.approx_encoding_nlt`, else the precise tables (one-step variant if
/// `rec.precise_max_steps == 1`, else the search). Reads `nlt_type`,
/// `bit_depth`, `is_signed`, `multiplier`, `ft_min`, `ft_max`, `delta`,
/// `inv_delta`, `approx_encoding_nlt`, and either `approx_enc_points` or
/// `fd_min`, `precise_max_steps`, `precise_enc_dec_indices`,
/// `precise_dec_lut` (and `num_points` for the search). The `bit_depth`
/// and `is_signed` arguments are unused. Other types do nothing (the C++
/// asserts).
#[allow(clippy::too_many_arguments)]
pub fn irv_convert_to_float_nlt(
    src_line: &LineBuf,
    src_line_offset: u32,
    dst_line: &mut LineBuf,
    _bit_depth: u32,
    _is_signed: bool,
    width: u32,
    rec: &NltLut<'_>,
) {
    let (so, w) = (src_line_offset as usize, width as usize);
    let sp = &src_line.i32()[so..so + w];
    let dp = &mut dst_line.f32_mut()[..w];
    if rec.nlt_type == NLT_LUT_STYLE {
        if rec.approx_encoding_nlt {
            irv_convert_to_float_nlt2or4_approx::<false>(sp, dp, rec);
        } else if rec.precise_max_steps == 1 {
            irv_convert_to_float_nlt2or4_precise_1step::<false>(sp, dp, rec);
        } else {
            irv_convert_to_float_nlt2or4_precise_search::<false>(sp, dp, rec);
        }
    } else if rec.nlt_type == NLT_BINARY_COMPLEMENT_PLUS_LUT {
        if rec.approx_encoding_nlt {
            irv_convert_to_float_nlt2or4_approx::<true>(sp, dp, rec);
        } else if rec.precise_max_steps == 1 {
            irv_convert_to_float_nlt2or4_precise_1step::<true>(sp, dp, rec);
        } else {
            irv_convert_to_float_nlt2or4_precise_search::<true>(sp, dp, rec);
        }
    } else {
        debug_assert!(
            false,
            "irv_convert_to_float_nlt: not a LUT-style nonlinearity"
        );
    }
}

// ---------------------------------------------------------------------------
// Colour transforms
// ---------------------------------------------------------------------------

/// `gen_rct_forward`: the reversible colour transform of the first
/// `repeat` samples. Either all six lines are 32-bit, or `r`, `g`, `b` are
/// 32-bit and `y`, `cb`, `cr` are 64-bit (selected by `y`). 32-bit
/// arithmetic wraps.
pub fn rct_forward(
    r: &LineBuf,
    g: &LineBuf,
    b: &LineBuf,
    y: &mut LineBuf,
    cb: &mut LineBuf,
    cr: &mut LineBuf,
    repeat: u32,
) {
    let n = repeat as usize;
    let (rp, gp, bp) = (&r.i32()[..n], &g.i32()[..n], &b.i32()[..n]);
    if y.is_i32() {
        let (yp, cbp, crp) = (
            &mut y.i32_mut()[..n],
            &mut cb.i32_mut()[..n],
            &mut cr.i32_mut()[..n],
        );
        for i in 0..n {
            let (rr, gg, bb) = (rp[i], gp[i], bp[i]);
            yp[i] = rr.wrapping_add(gg.wrapping_mul(2)).wrapping_add(bb) >> 2;
            cbp[i] = bb.wrapping_sub(gg);
            crp[i] = rr.wrapping_sub(gg);
        }
    } else {
        let (yp, cbp, crp) = (
            &mut y.i64_mut()[..n],
            &mut cb.i64_mut()[..n],
            &mut cr.i64_mut()[..n],
        );
        for i in 0..n {
            let (rr, gg, bb) = (rp[i] as i64, gp[i] as i64, bp[i] as i64);
            yp[i] = (rr + 2 * gg + bb) >> 2;
            cbp[i] = bb - gg;
            crp[i] = rr - gg;
        }
    }
}

/// `gen_rct_backward`: the inverse of [`rct_forward`]. Either all six lines
/// are 32-bit, or `y`, `cb`, `cr` are 64-bit and `r`, `g`, `b` 32-bit
/// (selected by `y`; results truncated to `i32`). Arithmetic wraps.
pub fn rct_backward(
    y: &LineBuf,
    cb: &LineBuf,
    cr: &LineBuf,
    r: &mut LineBuf,
    g: &mut LineBuf,
    b: &mut LineBuf,
    repeat: u32,
) {
    let n = repeat as usize;
    let (rp, gp, bp) = (
        &mut r.i32_mut()[..n],
        &mut g.i32_mut()[..n],
        &mut b.i32_mut()[..n],
    );
    if y.is_i32() {
        let (yp, cbp, crp) = (&y.i32()[..n], &cb.i32()[..n], &cr.i32()[..n]);
        for i in 0..n {
            let (yy, cbb, crr) = (yp[i], cbp[i], crp[i]);
            let gg = yy.wrapping_sub(cbb.wrapping_add(crr) >> 2);
            rp[i] = crr.wrapping_add(gg);
            gp[i] = gg;
            bp[i] = cbb.wrapping_add(gg);
        }
    } else {
        let (yp, cbp, crp) = (&y.i64()[..n], &cb.i64()[..n], &cr.i64()[..n]);
        for i in 0..n {
            let (yy, cbb, crr) = (yp[i], cbp[i], crp[i]);
            let gg = yy.wrapping_sub(cbb.wrapping_add(crr) >> 2);
            rp[i] = crr.wrapping_add(gg) as i32;
            gp[i] = gg as i32;
            bp[i] = cbb.wrapping_add(gg) as i32;
        }
    }
}

/// `gen_ict_forward`: the irreversible colour transform of the first
/// `repeat` samples (all slices at least that long).
pub fn ict_forward(
    r: &[f32],
    g: &[f32],
    b: &[f32],
    y: &mut [f32],
    cb: &mut [f32],
    cr: &mut [f32],
    repeat: u32,
) {
    use ct_cnst::*;
    let n = repeat as usize;
    let (r, g, b) = (&r[..n], &g[..n], &b[..n]);
    let (y, cb, cr) = (&mut y[..n], &mut cb[..n], &mut cr[..n]);
    for i in 0..n {
        let yy = ALPHA_RF * r[i] + ALPHA_GF * g[i] + ALPHA_BF * b[i];
        y[i] = yy;
        cb[i] = BETA_CB_F * (b[i] - yy);
        cr[i] = BETA_CR_F * (r[i] - yy);
    }
}

/// `gen_ict_backward`: the inverse of [`ict_forward`].
pub fn ict_backward(
    y: &[f32],
    cb: &[f32],
    cr: &[f32],
    r: &mut [f32],
    g: &mut [f32],
    b: &mut [f32],
    repeat: u32,
) {
    use ct_cnst::*;
    let n = repeat as usize;
    let (y, cb, cr) = (&y[..n], &cb[..n], &cr[..n]);
    let (r, g, b) = (&mut r[..n], &mut g[..n], &mut b[..n]);
    for i in 0..n {
        g[i] = y[i] - GAMMA_CR2G * cr[i] - GAMMA_CB2G * cb[i];
        r[i] = y[i] + GAMMA_CR2R * cr[i];
        b[i] = y[i] + GAMMA_CB2B * cb[i];
    }
}

#[cfg(test)]
mod tests {
    use super::floor_u32;

    /// `floor_u32` against `x.floor() as u32` for every `f32` bit
    /// pattern, NaNs and infinities included. Ignored by default; run it
    /// with `cargo test --release --lib floor_u32_is_floor -- --ignored`
    /// (about 20 seconds).
    #[test]
    #[ignore]
    fn floor_u32_is_floor() {
        for bits in 0..=u32::MAX {
            let x = f32::from_bits(bits);
            assert_eq!(floor_u32(x), x.floor() as u32, "{bits:#010x}");
        }
    }
}
