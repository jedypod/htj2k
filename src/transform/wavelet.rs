// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Wavelet lifting kernels, after the generic (`gen_*`) functions of
//! `transform/ojph_transform.cpp`.
//!
//! The reversible kernels work on 32-bit or 64-bit integer lines (the C++
//! dispatches on the `line_buf` flags; here on [`LineData`]); the
//! irreversible kernels work on float lines.
//!
//! Lines are [`LineBuf`]s. The horizontal kernels write boundary-extension
//! samples at logical index `-1` and at the width of the line they extend,
//! so every line they lift needs `pre_size >= 1` and room for one sample
//! past its width.

use crate::line::LineBuf;
use crate::params::{LiftingStep, ParamAtk};
use std::mem::swap;

/// The integer sample types of the reversible path, with the wrapping
/// arithmetic the C++ performs (its signed overflow is undefined; these
/// wrap, as the C++ does in practice).
trait RevSample: Copy {
    fn from_i16(v: i16) -> Self;
    fn w_add(self, o: Self) -> Self;
    fn w_sub(self, o: Self) -> Self;
    fn w_mul(self, o: Self) -> Self;
    /// `self >> e` (arithmetic). Kernels require `e` below the type's
    /// width (a larger shift is undefined in the C++); this port masks the
    /// count to the width.
    fn w_shr(self, e: u8) -> Self;
}

macro_rules! impl_rev_sample {
    ($t:ty) => {
        impl RevSample for $t {
            #[inline(always)]
            fn from_i16(v: i16) -> Self {
                v as $t
            }
            #[inline(always)]
            fn w_add(self, o: Self) -> Self {
                self.wrapping_add(o)
            }
            #[inline(always)]
            fn w_sub(self, o: Self) -> Self {
                self.wrapping_sub(o)
            }
            #[inline(always)]
            fn w_mul(self, o: Self) -> Self {
                self.wrapping_mul(o)
            }
            #[inline(always)]
            fn w_shr(self, e: u8) -> Self {
                self.wrapping_shr(e as u32)
            }
        }
    };
}
impl_rev_sample!(i32);
impl_rev_sample!(i64);

// ---------------------------------------------------------------------------
// Reversible
// ---------------------------------------------------------------------------

/// The body of `gen_rev_vert_step32/64`, and of one horizontal lifting
/// step: `dst[i] (+|-)= f(src1[i], src2[i])` for the step `s`.
///
/// Analysis adds the update and synthesis subtracts it (for the 5/3
/// predict form the signs are the other way round, as in the C++). `src1`
/// and `src2` must be at least as long as `dst`.
#[inline(always)]
fn rev_lift<T: RevSample>(s: &LiftingStep, src1: &[T], src2: &[T], dst: &mut [T], synthesis: bool) {
    let a = s.rev_a;
    let e = s.rev_e;
    let b = T::from_i16(s.rev_b);
    let n = dst.len();
    let (src1, src2) = (&src1[..n], &src2[..n]);
    let it = dst.iter_mut().zip(src1.iter().zip(src2.iter()));
    // The general definition of the wavelet in Part 2 is slightly different
    // to Part 1, although they are mathematically equivalent; the simpler
    // forms of Part 1 are identified and used, as the C++ does.
    if a == 1 {
        // 5/3 update and any case with a == 1
        if synthesis {
            for (d, (&x, &y)) in it {
                *d = d.w_sub(b.w_add(x).w_add(y).w_shr(e));
            }
        } else {
            for (d, (&x, &y)) in it {
                *d = d.w_add(b.w_add(x).w_add(y).w_shr(e));
            }
        }
    } else if a == -1 && s.rev_b == 1 && e == 1 {
        // 5/3 predict
        if synthesis {
            for (d, (&x, &y)) in it {
                *d = d.w_add(x.w_add(y).w_shr(e));
            }
        } else {
            for (d, (&x, &y)) in it {
                *d = d.w_sub(x.w_add(y).w_shr(e));
            }
        }
    } else if a == -1 {
        // any case with a == -1, which is not 5/3 predict
        if synthesis {
            for (d, (&x, &y)) in it {
                *d = d.w_sub(b.w_sub(x.w_add(y)).w_shr(e));
            }
        } else {
            for (d, (&x, &y)) in it {
                *d = d.w_add(b.w_sub(x.w_add(y)).w_shr(e));
            }
        }
    } else {
        // general case
        let a = T::from_i16(a);
        if synthesis {
            for (d, (&x, &y)) in it {
                *d = d.w_sub(b.w_add(a.w_mul(x.w_add(y))).w_shr(e));
            }
        } else {
            for (d, (&x, &y)) in it {
                *d = d.w_add(b.w_add(a.w_mul(x.w_add(y))).w_shr(e));
            }
        }
    }
}

/// `gen_rev_vert_step`: one reversible vertical lifting step, applied to the
/// first `repeat` samples of `aug` using the matching samples of `sig` and
/// `other`. In analysis `aug` is updated by `(b + a(sig + other)) >> e`
/// (in the simplified forms the C++ uses for `a == ±1`); in synthesis the
/// update is undone.
///
/// `sig` and `other` may be the same line (pass the same reference twice).
/// The 32-bit path is taken if any line is 32-bit, otherwise the 64-bit
/// path; all three lines must then be of that type (the C++ asserts this;
/// here a mismatch panics). Each line needs at least `repeat` samples.
pub fn rev_vert_step(
    s: &LiftingStep,
    sig: &LineBuf,
    other: &LineBuf,
    aug: &mut LineBuf,
    repeat: u32,
    synthesis: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::rev_vert_step(s, sig, other, aug, repeat, synthesis);
    #[cfg(not(feature = "simd"))]
    rev_vert_step_body(s, sig, other, aug, repeat, synthesis);
}

/// The body of [`rev_vert_step`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn rev_vert_step_body(
    s: &LiftingStep,
    sig: &LineBuf,
    other: &LineBuf,
    aug: &mut LineBuf,
    repeat: u32,
    synthesis: bool,
) {
    let n = repeat as usize;
    if sig.is_i32() || other.is_i32() || aug.is_i32() {
        rev_lift(
            s,
            &sig.i32()[..n],
            &other.i32()[..n],
            &mut aug.i32_mut()[..n],
            synthesis,
        );
    } else {
        rev_lift(
            s,
            &sig.i64()[..n],
            &other.i64()[..n],
            &mut aug.i64_mut()[..n],
            synthesis,
        );
    }
}

/// Lengths of the low-pass and high-pass halves of a line of `width`
/// samples whose first sample is at an even (`even == true`) or odd
/// position.
#[inline(always)]
fn split_widths(width: usize, even: bool) -> (usize, usize) {
    let l_width = (width + usize::from(even)) >> 1;
    let h_width = (width + usize::from(!even)) >> 1;
    (l_width, h_width)
}

/// Deinterleaves `src` (logical samples) into the low- and high-pass
/// buffers `ld` and `hd` (logical samples), the first sample of `src` going
/// to `hd` when `!even`. The callers pass buffers of exactly the lengths
/// [`split_widths`] gives.
///
/// One pass over the pairs of `src` fills both buffers, and the last
/// sample of an odd-length `src` goes to the first buffer.
#[inline(always)]
fn deinterleave<T: Copy>(src: &[T], ld: &mut [T], hd: &mut [T], even: bool) {
    let (first, second) = if even { (ld, hd) } else { (hd, ld) };
    let n = (src.len() / 2).min(first.len()).min(second.len());
    let (pairs, rest) = src.split_at(2 * n);
    for ((c, a), b) in pairs
        .chunks_exact(2)
        .zip(&mut first[..n])
        .zip(&mut second[..n])
    {
        *a = c[0];
        *b = c[1];
    }
    if let (Some(d), Some(&v)) = (first.get_mut(n), rest.first()) {
        *d = v;
    }
}

/// Interleaves the low- and high-pass buffers into `dst`, the inverse of
/// [`deinterleave`].
#[inline(always)]
fn interleave<T: Copy>(dst: &mut [T], ls: &[T], hs: &[T], even: bool) {
    let (first, second) = if even { (ls, hs) } else { (hs, ls) };
    let n = (dst.len() / 2).min(first.len()).min(second.len());
    let (pairs, rest) = dst.split_at_mut(2 * n);
    for ((c, &a), &b) in pairs.chunks_exact_mut(2).zip(&first[..n]).zip(&second[..n]) {
        c[0] = a;
        c[1] = b;
    }
    if let (Some(d), Some(&v)) = (rest.first_mut(), first.get(n)) {
        *d = v;
    }
}

/// Writes the symmetric boundary extension of the `width` logical samples
/// that start at index `pre` of `buf`: `buf[pre - 1] = buf[pre]` and
/// `buf[pre + width] = buf[pre + width - 1]`.
#[inline(always)]
fn extend<T: Copy>(buf: &mut [T], pre: usize, width: usize) {
    buf[pre - 1] = buf[pre];
    buf[pre + width] = buf[pre + width - 1];
}

/// `gen_rev_horz_ana32/64`. `l` and `h` are the full backing vectors of
/// the destination lines, with their `pre_size`s; `src` holds the logical
/// samples of the source line.
#[allow(clippy::too_many_arguments)]
// inlined into the AVX2 version of its caller with the `simd` feature
#[cfg_attr(feature = "simd", inline(always))]
fn rev_horz_ana_impl<T: RevSample>(
    atk: &ParamAtk,
    l: &mut [T],
    lpre: usize,
    h: &mut [T],
    hpre: usize,
    src: &[T],
    width: u32,
    mut even: bool,
) {
    if width > 1 {
        let w = width as usize;
        let (mut l_width, mut h_width) = split_widths(w, even);
        deinterleave(
            &src[..w],
            &mut l[lpre..lpre + l_width],
            &mut h[hpre..hpre + h_width],
            even,
        );

        let (mut lp, mut lpre, mut hp, mut hpre) = (l, lpre, h, hpre);
        for j in (0..atk.get_num_steps()).rev() {
            let s = atk.get_step(j);
            // extension
            extend(lp, lpre, l_width);
            // lifting step
            let o = lpre + usize::from(even);
            let s0 = &lp[o - 1..o - 1 + h_width];
            let s1 = &lp[o..o + h_width];
            rev_lift(s, s0, s1, &mut hp[hpre..hpre + h_width], false);
            // swap buffers
            swap(&mut lp, &mut hp);
            swap(&mut lpre, &mut hpre);
            even = !even;
            swap(&mut l_width, &mut h_width);
        }
    } else if even {
        if let (Some(d), Some(&v)) = (l.get_mut(lpre), src.first()) {
            *d = v;
        }
    } else if let (Some(d), Some(&v)) = (h.get_mut(hpre), src.first()) {
        *d = v.w_add(v); // src * 2
    }
}

/// `gen_rev_horz_syn32/64`. `dst` holds the logical samples of the
/// destination line; `l` and `h` are the full backing vectors of the
/// source lines, with their `pre_size`s.
#[allow(clippy::too_many_arguments)]
// inlined into the AVX2 version of its caller with the `simd` feature
#[cfg_attr(feature = "simd", inline(always))]
fn rev_horz_syn_impl<T: RevSample>(
    atk: &ParamAtk,
    dst: &mut [T],
    l: &mut [T],
    lpre: usize,
    h: &mut [T],
    hpre: usize,
    width: u32,
    even: bool,
) {
    if width > 1 {
        let w = width as usize;
        let (l_width, h_width) = split_widths(w, even);
        {
            let mut ev = even;
            let (mut aug_width, mut oth_width) = (l_width, h_width);
            let (mut oth, mut opre, mut aug, mut apre) = (&mut *h, hpre, &mut *l, lpre);
            for j in 0..atk.get_num_steps() {
                let s = atk.get_step(j);
                // extension
                extend(oth, opre, oth_width);
                // lifting step
                let o = opre + usize::from(!ev);
                let s0 = &oth[o - 1..o - 1 + aug_width];
                let s1 = &oth[o..o + aug_width];
                rev_lift(s, s0, s1, &mut aug[apre..apre + aug_width], true);
                // swap buffers
                swap(&mut aug, &mut oth);
                swap(&mut apre, &mut opre);
                ev = !ev;
                swap(&mut aug_width, &mut oth_width);
            }
        }
        // combine both lsrc and hsrc into dst
        interleave(
            &mut dst[..w],
            &l[lpre..lpre + l_width],
            &h[hpre..hpre + h_width],
            even,
        );
    } else if even {
        if let (Some(d), Some(&v)) = (dst.first_mut(), l.get(lpre)) {
            *d = v;
        }
    } else if let (Some(d), Some(&v)) = (dst.first_mut(), h.get(hpre)) {
        *d = v.w_shr(1);
    }
}

/// `gen_rev_horz_ana`: reversible horizontal analysis of the first `width`
/// samples of `src` into the low-pass line `ldst` and the high-pass line
/// `hdst`. `even` is true when the first sample of `src` is at an even
/// position (it then goes to the low-pass band).
///
/// For `width > 1`, `ldst` and `hdst` need `pre_size >= 1` and room for
/// `(width + 1) / 2 + 1` samples (their own band width plus one extension
/// sample); the kernel writes boundary extensions into both. For
/// `width <= 1` only sample 0 of one destination is written (`width == 0`
/// behaves as `width == 1`, as in the C++). Lines are dispatched on `src`:
/// 32-bit or 64-bit, and the destinations must match.
pub fn rev_horz_ana(
    atk: &ParamAtk,
    ldst: &mut LineBuf,
    hdst: &mut LineBuf,
    src: &LineBuf,
    width: u32,
    even: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::rev_horz_ana(atk, ldst, hdst, src, width, even);
    #[cfg(not(feature = "simd"))]
    rev_horz_ana_body(atk, ldst, hdst, src, width, even);
}

/// The body of [`rev_horz_ana`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn rev_horz_ana_body(
    atk: &ParamAtk,
    ldst: &mut LineBuf,
    hdst: &mut LineBuf,
    src: &LineBuf,
    width: u32,
    even: bool,
) {
    let (lpre, hpre) = (ldst.pre_size, hdst.pre_size);
    if src.is_i32() {
        rev_horz_ana_impl(
            atk,
            ldst.i32_full_mut(),
            lpre,
            hdst.i32_full_mut(),
            hpre,
            src.i32(),
            width,
            even,
        );
    } else {
        rev_horz_ana_impl(
            atk,
            ldst.i64_full_mut(),
            lpre,
            hdst.i64_full_mut(),
            hpre,
            src.i64(),
            width,
            even,
        );
    }
}

/// `gen_rev_horz_syn`: reversible horizontal synthesis of the low-pass line
/// `lsrc` and the high-pass line `hsrc` into the first `width` samples of
/// `dst`; the inverse of [`rev_horz_ana`] with the same `even`.
///
/// The lifting is done in place in `lsrc` and `hsrc`, which are left
/// modified (including boundary-extension samples at logical index -1 and
/// at their band width); they need the same room as the destinations of
/// [`rev_horz_ana`]. Lines are dispatched on `dst`.
pub fn rev_horz_syn(
    atk: &ParamAtk,
    dst: &mut LineBuf,
    lsrc: &mut LineBuf,
    hsrc: &mut LineBuf,
    width: u32,
    even: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::rev_horz_syn(atk, dst, lsrc, hsrc, width, even);
    #[cfg(not(feature = "simd"))]
    rev_horz_syn_body(atk, dst, lsrc, hsrc, width, even);
}

/// The body of [`rev_horz_syn`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn rev_horz_syn_body(
    atk: &ParamAtk,
    dst: &mut LineBuf,
    lsrc: &mut LineBuf,
    hsrc: &mut LineBuf,
    width: u32,
    even: bool,
) {
    let (lpre, hpre) = (lsrc.pre_size, hsrc.pre_size);
    if dst.is_i32() {
        rev_horz_syn_impl(
            atk,
            dst.i32_mut(),
            lsrc.i32_full_mut(),
            lpre,
            hsrc.i32_full_mut(),
            hpre,
            width,
            even,
        );
    } else {
        rev_horz_syn_impl(
            atk,
            dst.i64_mut(),
            lsrc.i64_full_mut(),
            lpre,
            hsrc.i64_full_mut(),
            hpre,
            width,
            even,
        );
    }
}

// ---------------------------------------------------------------------------
// Irreversible
// ---------------------------------------------------------------------------

/// `dst[i] += a * (src1[i] + src2[i])`, in that order of operations.
#[inline(always)]
fn irv_lift(a: f32, src1: &[f32], src2: &[f32], dst: &mut [f32]) {
    let n = dst.len();
    let (src1, src2) = (&src1[..n], &src2[..n]);
    for (d, (&x, &y)) in dst.iter_mut().zip(src1.iter().zip(src2.iter())) {
        *d += a * (x + y);
    }
}

/// `dst[i] *= k`.
#[inline(always)]
fn scale(dst: &mut [f32], k: f32) {
    for d in dst {
        *d *= k;
    }
}

/// `gen_irv_vert_step`: one irreversible vertical lifting step,
/// `aug[i] += a * (sig[i] + other[i])` for the first `repeat` samples, with
/// `a` negated in synthesis. `sig` and `other` may be the same line. All
/// lines must be float lines of at least `repeat` samples.
pub fn irv_vert_step(
    s: &LiftingStep,
    sig: &LineBuf,
    other: &LineBuf,
    aug: &mut LineBuf,
    repeat: u32,
    synthesis: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_vert_step(s, sig, other, aug, repeat, synthesis);
    #[cfg(not(feature = "simd"))]
    irv_vert_step_body(s, sig, other, aug, repeat, synthesis);
}

/// The body of [`irv_vert_step`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_vert_step_body(
    s: &LiftingStep,
    sig: &LineBuf,
    other: &LineBuf,
    aug: &mut LineBuf,
    repeat: u32,
    synthesis: bool,
) {
    let a = if synthesis { -s.irv_a } else { s.irv_a };
    let n = repeat as usize;
    irv_lift(
        a,
        &sig.f32()[..n],
        &other.f32()[..n],
        &mut aug.f32_mut()[..n],
    );
}

/// `gen_irv_vert_times_K`: multiplies the first `repeat` samples of the
/// float line `aug` by `k`.
pub fn irv_vert_times_k(k: f32, aug: &mut LineBuf, repeat: u32) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_vert_times_k(k, aug, repeat);
    #[cfg(not(feature = "simd"))]
    irv_vert_times_k_body(k, aug, repeat);
}

/// The body of [`irv_vert_times_k`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_vert_times_k_body(k: f32, aug: &mut LineBuf, repeat: u32) {
    scale(&mut aug.f32_mut()[..repeat as usize], k);
}

/// `gen_irv_horz_ana`: irreversible horizontal analysis of the first
/// `width` samples of the float line `src` into `ldst` (low pass) and
/// `hdst` (high pass), including the final scaling by `1/K` and `K`. Same
/// buffer requirements as [`rev_horz_ana`]; all lines must be float.
pub fn irv_horz_ana(
    atk: &ParamAtk,
    ldst: &mut LineBuf,
    hdst: &mut LineBuf,
    src: &LineBuf,
    width: u32,
    even: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_horz_ana(atk, ldst, hdst, src, width, even);
    #[cfg(not(feature = "simd"))]
    irv_horz_ana_body(atk, ldst, hdst, src, width, even);
}

/// The body of [`irv_horz_ana`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_horz_ana_body(
    atk: &ParamAtk,
    ldst: &mut LineBuf,
    hdst: &mut LineBuf,
    src: &LineBuf,
    width: u32,
    mut even: bool,
) {
    let (lpre, hpre) = (ldst.pre_size, hdst.pre_size);
    let l = ldst.f32_full_mut();
    let h = hdst.f32_full_mut();
    let src = src.f32();
    if width > 1 {
        let w = width as usize;
        let (mut l_width, mut h_width) = split_widths(w, even);
        // split src into ldst and hdst
        deinterleave(
            &src[..w],
            &mut l[lpre..lpre + l_width],
            &mut h[hpre..hpre + h_width],
            even,
        );

        let (mut lp, mut lpre, mut hp, mut hpre) = (l, lpre, h, hpre);
        for j in (0..atk.get_num_steps()).rev() {
            let a = atk.get_step(j).irv_a;
            // extension
            extend(lp, lpre, l_width);
            // lifting step
            let o = lpre + usize::from(even);
            let s0 = &lp[o - 1..o - 1 + h_width];
            let s1 = &lp[o..o + h_width];
            irv_lift(a, s0, s1, &mut hp[hpre..hpre + h_width]);
            // swap buffers
            swap(&mut lp, &mut hp);
            swap(&mut lpre, &mut hpre);
            even = !even;
            swap(&mut l_width, &mut h_width);
        }

        let k = atk.get_k();
        let k_inv = 1.0f32 / k;
        scale(&mut lp[lpre..lpre + l_width], k_inv);
        scale(&mut hp[hpre..hpre + h_width], k);
    } else if even {
        if let (Some(d), Some(&v)) = (l.get_mut(lpre), src.first()) {
            *d = v;
        }
    } else if let (Some(d), Some(&v)) = (h.get_mut(hpre), src.first()) {
        *d = v * 2.0f32;
    }
}

/// `gen_irv_horz_syn`: irreversible horizontal synthesis of the float
/// lines `lsrc` and `hsrc` into the first `width` samples of `dst`; the
/// inverse of [`irv_horz_ana`]. `lsrc` and `hsrc` are scaled and lifted in
/// place, as for [`rev_horz_syn`].
pub fn irv_horz_syn(
    atk: &ParamAtk,
    dst: &mut LineBuf,
    lsrc: &mut LineBuf,
    hsrc: &mut LineBuf,
    width: u32,
    even: bool,
) {
    #[cfg(feature = "simd")]
    super::vectorised::irv_horz_syn(atk, dst, lsrc, hsrc, width, even);
    #[cfg(not(feature = "simd"))]
    irv_horz_syn_body(atk, dst, lsrc, hsrc, width, even);
}

/// The body of [`irv_horz_syn`].
#[inline(always)]
// only the tests call it with the `simd` feature
#[cfg_attr(feature = "simd", allow(dead_code))]
pub(super) fn irv_horz_syn_body(
    atk: &ParamAtk,
    dst: &mut LineBuf,
    lsrc: &mut LineBuf,
    hsrc: &mut LineBuf,
    width: u32,
    even: bool,
) {
    let (lpre, hpre) = (lsrc.pre_size, hsrc.pre_size);
    let l = lsrc.f32_full_mut();
    let h = hsrc.f32_full_mut();
    let dst = dst.f32_mut();
    if width > 1 {
        let w = width as usize;
        let (l_width, h_width) = split_widths(w, even);
        {
            let mut ev = even;
            let (mut aug_width, mut oth_width) = (l_width, h_width);
            let (mut oth, mut opre, mut aug, mut apre) = (&mut *h, hpre, &mut *l, lpre);

            let k = atk.get_k();
            let k_inv = 1.0f32 / k;
            scale(&mut aug[apre..apre + aug_width], k);
            scale(&mut oth[opre..opre + oth_width], k_inv);

            for j in 0..atk.get_num_steps() {
                let a = atk.get_step(j).irv_a;
                // extension
                extend(oth, opre, oth_width);
                // lifting step: *dp -= a * (sp[-1] + sp[0])
                let o = opre + usize::from(!ev);
                let s0 = &oth[o - 1..o - 1 + aug_width];
                let s1 = &oth[o..o + aug_width];
                irv_lift_sub(a, s0, s1, &mut aug[apre..apre + aug_width]);
                // swap buffers
                swap(&mut aug, &mut oth);
                swap(&mut apre, &mut opre);
                ev = !ev;
                swap(&mut aug_width, &mut oth_width);
            }
        }
        // combine both lsrc and hsrc into dst
        interleave(
            &mut dst[..w],
            &l[lpre..lpre + l_width],
            &h[hpre..hpre + h_width],
            even,
        );
    } else if even {
        if let (Some(d), Some(&v)) = (dst.first_mut(), l.get(lpre)) {
            *d = v;
        }
    } else if let (Some(d), Some(&v)) = (dst.first_mut(), h.get(hpre)) {
        *d = v * 0.5f32;
    }
}

/// `dst[i] -= a * (src1[i] + src2[i])`, in that order of operations.
#[inline(always)]
fn irv_lift_sub(a: f32, src1: &[f32], src2: &[f32], dst: &mut [f32]) {
    let n = dst.len();
    let (src1, src2) = (&src1[..n], &src2[..n]);
    for (d, (&x, &y)) in dst.iter_mut().zip(src1.iter().zip(src2.iter())) {
        *d -= a * (x + y);
    }
}
