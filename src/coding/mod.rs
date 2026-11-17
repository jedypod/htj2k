// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! HT (High-Throughput) block coding: the code-block decoders and the
//! encoder, ported from OpenJPH's `src/core/coding` (generic, non-SIMD
//! versions).
//!
//! * [`decode_codeblock32`]: samples of up to 31 magnitude bits
//!   (`ojph_decode_codeblock32`);
//! * [`decode_codeblock64`]: samples of up to 63 magnitude bits
//!   (`ojph_decode_codeblock64`), used when a component needs more than
//!   32-bit precision.
//! * [`encode_codeblock32`] and [`encode_codeblock64`]: the cleanup-pass
//!   encoders for 32- and 64-bit samples (`ojph_encode_codeblock32` and
//!   `ojph_encode_codeblock64`).
//!
//! Samples are in sign-magnitude form: the MSB is the sign. Decoded
//! magnitudes carry one extra fractional bit (the bin centre), as in
//! OpenJPH.

mod decoder32;
mod decoder64;
mod encoder;
mod readers;
mod tables;

pub use decoder32::decode_codeblock32;
pub use decoder64::decode_codeblock64;
pub use encoder::{encode_codeblock32, encode_codeblock64};

/// Length of the `ui16 scratch[8 * 513]` array of the C++ decoders.
pub(crate) const SCRATCH_LEN: usize = 8 * 513;
/// Length of the `v_n_scratch` array of the C++ decoders.
pub(crate) const V_N_SIZE: usize = 512 + 4;

/// Derived layout values, computed once argument checks have passed.
#[derive(Clone, Copy)]
pub(crate) struct ScratchLayout {
    /// Quad-row stride of `scratch`, in `u16` entries.
    pub(crate) sstr: usize,
}

/// Argument checks that the C++ does not make (it relies on its callers);
/// they reject only arguments for which the C++ would read or write out of
/// bounds, plus a `decoded_data` smaller than `stride * height`.
///
/// * `coded_len >= lengths1 + lengths2` (and the sum fits an `int`, which
///   the C++ converts it to);
/// * `width <= 1024`, and every index the C++ uses into its stack arrays
///   `scratch[8 * 513]`, `v_n_scratch[516]` and `prev_row_sig[264]` is in
///   range. That holds for every block JPEG 2000 allows (both nominal
///   dimensions powers of two from 4 to 1024 with a product of at most
///   4096, and the actual block no larger), but not for every
///   `width * height <= 4096` (for example 7 x 585 overflows `scratch` in
///   the C++ too);
/// * `dec_len >= stride * height`, and the indices of all rows the C++ may
///   touch (up to `height + 3`) fit a `usize`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_args(
    coded_len: usize,
    dec_len: usize,
    num_passes: u32,
    lengths1: u32,
    lengths2: u32,
    width: u32,
    height: u32,
    stride: u32,
) -> Option<ScratchLayout> {
    let total = lengths1 as u64 + lengths2 as u64;
    if total > coded_len as u64 || total > i32::MAX as u64 {
        return None;
    }
    if width > 1024 || height > 4096 {
        return None;
    }
    let (w, h, s) = (width as u64, height as u64, stride as u64);
    if (h + 4) * s + w + 8 > usize::MAX as u64 || h * s > dec_len as u64 {
        return None;
    }
    let sstr = (((width + 2) + 7) & !7) as u64;
    // quad pairs per quad row (one `scratch` entry pair per quad)
    let pairs = w.div_ceil(4);
    // step 1 (and step 2): last quad row, including its two trailing zeros
    let rows = if h > 0 { (h - 1) >> 1 } else { 0 };
    let mut max_idx = rows * sstr + 4 * pairs + 1;
    if num_passes > 1 {
        let mstr = ((((w + 3) >> 2) + 2) + 7) & !7;
        let ylast = if h > 0 { 4 * ((h - 1) / 4) } else { 0 };
        let yend = 4 * h.div_ceil(4);
        if h > 0 && w > 0 {
            // sigma_from_quads reads the quad row after each stripe
            max_idx = max_idx.max(((ylast >> 1) + 1) * sstr + 4 * (pairs - 1) + 2);
            // SPP reads one entry beyond the next sigma row's last column
            max_idx = max_idx.max(((ylast >> 2) + 1) * mstr + pairs);
        }
        // sigma_from_quads clears the row after the block
        max_idx = max_idx.max((yend >> 2) * mstr + pairs);
    }
    if max_idx >= SCRATCH_LEN as u64 {
        return None;
    }
    Some(ScratchLayout {
        sstr: sstr as usize,
    })
}

/// Stores a sample; writes outside `decoded_data` (only ever outside the
/// block, see the decoders' docs) are dropped.
#[inline(always)]
pub(crate) fn put<T: Copy>(dec: &mut [T], idx: usize, val: T) {
    if let Some(d) = dec.get_mut(idx) {
        *d = val;
    }
}

/// `dec[idx] ^= val`, dropped outside `decoded_data` like [`put`].
#[inline(always)]
pub(crate) fn xor<T: Copy + core::ops::BitXorAssign>(dec: &mut [T], idx: usize, val: T) {
    if let Some(d) = dec.get_mut(idx) {
        *d ^= val;
    }
}

/// Re-arranges quad significance (in the `rho` bits of the cleanup-pass
/// records in `scratch`) into column significance, in place, as the first
/// step of the SigProp pass. Returns `mstr`, the stride of the result.
pub(crate) fn sigma_from_quads(
    scratch: &mut [u16; SCRATCH_LEN],
    width: u32,
    height: u32,
    sstr: usize,
) -> usize {
    let mut mstr = ((width + 3) >> 2) as usize;
    mstr = ((mstr + 2) + 7) & !7;

    let mut y = 0u32;
    while y < height {
        let mut sp = (y >> 1) as usize * sstr;
        let mut dp = (y >> 2) as usize * mstr;
        let mut x = 0u32;
        while x < width {
            let s0 = scratch[sp] as u32;
            let s2 = scratch[sp + 2] as u32;
            let s0n = scratch[sp + sstr] as u32;
            let s2n = scratch[sp + 2 + sstr] as u32;
            let mut t0 = ((s0 & 0x30) >> 4) | ((s0 & 0xC0) >> 2);
            t0 |= ((s2 & 0x30) << 4) | ((s2 & 0xC0) << 6);
            let mut t1 = ((s0n & 0x30) >> 2) | (s0n & 0xC0);
            t1 |= ((s2n & 0x30) << 6) | ((s2n & 0xC0) << 8);
            scratch[dp] = (t0 | t1) as u16;
            x += 4;
            sp += 4;
            dp += 1;
        }
        scratch[dp] = 0;
        y += 4;
    }
    // reset one row after the code-block
    let mut dp = (y >> 2) as usize * mstr;
    let mut x = 0u32;
    while x < width {
        scratch[dp] = 0;
        x += 4;
        dp += 1;
    }
    scratch[dp] = 0;
    mstr
}
