// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! AVX2 MagSgn step of the HT block decoder for 32-bit samples (port of
//! `decode_cb_step2_32bit`, `decode_cb_step2_16bit`,
//! `decode_two_quad32_avx2` and `decode_four_quad16` in OpenJPH's
//! `ojph_block_decoder_avx2.cpp`), for the `simd` feature.
//!
//! As in OpenJPH, the MagSgn segment is first unstuffed into a flat
//! buffer ([`destuff_frwd`]), so the bits of a quad can be fetched at an
//! absolute bit position ([`dfetch`]) instead of through a reader that
//! carries state from one sample to the next. Each step then decodes
//! several quads, one sample per lane: the bit offset of each sample
//! within its quad is a prefix sum of the sample lengths, and byte
//! shuffles and multiplies extract the sample's bits from that offset.
//!
//! Also as in OpenJPH, a block of at most 14 bit-planes (`mmsbp2 < 16`)
//! is decoded four quads a step in 16-bit lanes
//! ([`quads16`]), any other two quads a step in 32-bit lanes
//! ([`quads32`]). The 16-bit lanes give the same samples: every decoded
//! `U_q` is then at most `mmsbp2 <= 15`, so `v_n` has at most 16 bits,
//! and `p >= 17`, so the low 16 bits of a sample, `(v_n + 2) << (p - 1)`
//! with the sign at bit 31, are 0 and its high 16 bits are
//! `(v_n + 2) << (p - 17)`, wrapped to 16 bits.
//!
//! The result is that of [`super::decoder32`]'s scalar MagSgn step for
//! every input, including malformed ones, where OpenJPH's kernel and its
//! generic decoder can differ:
//!
//! * a sample is significant when its `rho` bit is set, as in the generic
//!   decoder (OpenJPH's kernel also looks at `e_k` and `e_1`, which the
//!   VLC tables never set without `rho`);
//! * the sign is the first bit at the sample's position even when the
//!   sample has no bits (`m_n == 0`, impossible in a valid stream);
//! * the column to the right of an odd-width block is not decoded, so it
//!   consumes no bits;
//! * when a `U_q` is too large, the quads before it are still written,
//!   and those after it are not;
//! * only the block's columns are written.

// where `avx2_dispatch!` can call the kernels (see `crate::simd`)
#![cfg(all(
    target_arch = "x86_64",
    not(all(
        target_feature = "avx2",
        target_feature = "bmi1",
        target_feature = "bmi2",
        target_feature = "lzcnt",
        target_feature = "popcnt"
    ))
))]

use super::encoder_avx2::lzcnt_epi32;
use super::{whole_rows, Clipped, RowPair, SCRATCH_LEN, V_N_SIZE};
use crate::simd::x86::*;

/// Bytes after the unstuffed data: the byte holding its last bits, and
/// the 0xFF bytes that the 24-byte window of [`dfetch`] may read.
const PAD: usize = 1 + 24;

/// The 64 bits at `s[i..i + 8]`, little-endian.
#[inline(always)]
fn le64(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().unwrap())
}

/// Whether any byte of `v` is 0xFF.
#[inline(always)]
fn has_ff_byte64(v: u64) -> bool {
    let x = !v;
    x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080 != 0
}

/// `destuff_frwd<0xFF>`: unstuffs the MagSgn segment `src` into `dst` as
/// the scalar reader (`Frwd<0xFF>`) would deliver its bits: a byte after
/// a 0xFF contributes 7 bits (its MSB is OR'ed into the next byte's
/// first bit), and past the end every bit is 1. At most `cap` bytes are
/// produced, then the partial byte with its high bits set, then 0xFF
/// bytes; `dst` must have room for `cap + PAD` bytes. Returns the offset
/// from which every byte is 0xFF.
#[inline(always)]
fn destuff_frwd(src: &[u8], dst: &mut [u8], cap: usize) -> usize {
    /// The output position and the bits not yet written.
    struct Out {
        o: usize,
        acc: u64, // the low `nb` bits are pending
        nb: u32,  // below 8 between bytes
        prev_ff: bool,
    }
    impl Out {
        #[inline(always)]
        fn byte(&mut self, b: u8, dst: &mut [u8]) {
            self.acc |= (b as u64) << self.nb;
            self.nb += if self.prev_ff { 7 } else { 8 };
            self.prev_ff = b == 0xFF;
            if self.nb >= 8 {
                dst[self.o] = self.acc as u8;
                self.o += 1;
                self.acc >>= 8;
                self.nb -= 8;
            }
        }
    }
    let mut out = Out {
        o: 0,
        acc: 0,
        nb: 0,
        prev_ff: false,
    };
    let mut s = 0usize;

    // 8 bytes at a time while none is 0xFF or follows one
    while s + 8 <= src.len() && out.o + 16 <= cap {
        let v = le64(src, s);
        if out.prev_ff || has_ff_byte64(v) {
            for &b in &src[s..s + 8] {
                out.byte(b, dst);
            }
        } else {
            let w = out.acc | (v << out.nb);
            dst[out.o..out.o + 8].copy_from_slice(&w.to_le_bytes());
            out.acc = if out.nb != 0 { v >> (64 - out.nb) } else { 0 };
            out.o += 8;
        }
        s += 8;
    }
    while s < src.len() && out.o < cap {
        out.byte(src[s], dst);
        s += 1;
    }
    let o = out.o;
    dst[o] = (out.acc as u8) | (0xFFu32 << out.nb) as u8;
    dst[o + 1..o + PAD].fill(0xFF);
    o + 1
}

/// `dfetch`: the 128 bits of `dbuf` from bit `pos`. Reads past `limit`
/// (all 1s) are clamped to `limit`.
#[target_feature(enable = "avx2")]
#[inline]
fn dfetch(dbuf: &[u8], limit: usize, pos: u32) -> __m128i {
    let off = ((pos >> 3) as usize).min(limit);
    let w: &[u8; 24] = dbuf[off..off + 24].try_into().unwrap();
    let (a, b, c) = (le64(w, 0), le64(w, 8), le64(w, 16));
    let k = pos & 7;
    // `(x << 1) << (63 - k)` is `x << (64 - k)`, and 0 for k == 0
    let lo = (a >> k) | ((b << 1) << (63 - k));
    let hi = (b >> k) | ((c << 1) << (63 - k));
    _mm_set_epi64x(hi as i64, lo as i64)
}

/// The 16 entries of `s` from `i` (8 quads: `inf` in the low and `u_q` in
/// the high half of each lane), zeros past its end.
#[target_feature(enable = "avx2")]
#[inline]
fn load_quads(s: &[u16], i: usize) -> __m256i {
    let mut a = [0u16; 16];
    match s.get(i..i + 16) {
        Some(src) => a.copy_from_slice(src),
        None => {
            let src = s.get(i..).unwrap_or(&[]);
            a[..src.len()].copy_from_slice(src);
        }
    }
    _mm256_setr_epi16(
        a[0] as i16,
        a[1] as i16,
        a[2] as i16,
        a[3] as i16,
        a[4] as i16,
        a[5] as i16,
        a[6] as i16,
        a[7] as i16,
        a[8] as i16,
        a[9] as i16,
        a[10] as i16,
        a[11] as i16,
        a[12] as i16,
        a[13] as i16,
        a[14] as i16,
        a[15] as i16,
    )
}

/// `decode_two_quad32_avx2`: the 8 samples of quads `inf0`/`u0` and
/// `inf1`/`u1` (lanes 0 to 3 and 4 to 7, in the generic decoder's sample
/// order), decoded from bit `pos`, which moves past them. Returns the
/// samples and their `v_n` (both 0 for an insignificant sample).
#[target_feature(enable = "avx2")]
#[inline]
#[allow(clippy::too_many_arguments)]
fn decode_two_quads(
    inf0: u32,
    u0: u32,
    inf1: u32,
    u1: u32,
    dbuf: &[u8],
    limit: usize,
    pos: &mut u32,
    p: u32,
) -> (__m256i, __m256i) {
    let inf = _mm256_set_m128i(_mm_set1_epi32(inf1 as i32), _mm_set1_epi32(inf0 as i32));
    // rho, the significance of each sample
    let rho = _mm256_and_si256(
        inf,
        _mm256_setr_epi32(0x10, 0x20, 0x40, 0x80, 0x10, 0x20, 0x40, 0x80),
    );
    let insig = _mm256_cmpeq_epi32(rho, _mm256_setzero_si256());
    if _mm256_movemask_epi8(insig) == -1 {
        return (_mm256_setzero_si256(), _mm256_setzero_si256());
    }
    let u_q = _mm256_set_m128i(_mm_set1_epi32(u1 as i32), _mm_set1_epi32(u0 as i32));

    // e_k at bit 15, e_1 at bit 11
    let flags = _mm256_and_si256(
        inf,
        _mm256_setr_epi32(
            0x1110, 0x2220, 0x4440, 0x8880, 0x1110, 0x2220, 0x4440, 0x8880,
        ),
    );
    let flags = _mm256_sllv_epi32(flags, _mm256_setr_epi32(3, 2, 1, 0, 3, 2, 1, 0));
    let e_k = _mm256_srli_epi32::<15>(flags);

    // the sample lengths, m_n, and their prefix sums within each quad
    let m_n = _mm256_andnot_si256(insig, _mm256_sub_epi32(u_q, e_k));
    let mut inc_sum = _mm256_add_epi32(m_n, _mm256_bslli_epi128::<4>(m_n));
    inc_sum = _mm256_add_epi32(inc_sum, _mm256_bslli_epi128::<8>(inc_sum));
    let total0 = _mm256_extract_epi32::<3>(inc_sum) as u32;
    let total1 = _mm256_extract_epi32::<7>(inc_sum) as u32;

    let ms0 = dfetch(dbuf, limit, *pos);
    let ms1 = dfetch(dbuf, limit, *pos + total0);
    *pos += total0 + total1;
    let ms_vec = _mm256_set_m128i(ms1, ms0);

    // the 32 bits from each sample's first bit
    let ex_sum = _mm256_bslli_epi128::<4>(inc_sum);
    let mut byte_idx = _mm256_srli_epi32::<3>(ex_sum);
    let mut bit_idx = _mm256_and_si256(ex_sum, _mm256_set1_epi32(7));
    byte_idx = _mm256_shuffle_epi8(
        byte_idx,
        _mm256_setr_epi32(
            0x00000000, 0x04040404, 0x08080808, 0x0C0C0C0C, 0x00000000, 0x04040404, 0x08080808,
            0x0C0C0C0C,
        ),
    );
    byte_idx = _mm256_add_epi32(byte_idx, _mm256_set1_epi32(0x03020100));
    let mut d0 = _mm256_shuffle_epi8(ms_vec, byte_idx);
    byte_idx = _mm256_add_epi32(byte_idx, _mm256_set1_epi32(0x01010101));
    let mut d1 = _mm256_shuffle_epi8(ms_vec, byte_idx);

    bit_idx = _mm256_or_si256(bit_idx, _mm256_slli_epi32::<16>(bit_idx));
    let shr = _mm256_setr_epi8(
        -1, 127, 63, 31, 15, 7, 3, 1, -1, 127, 63, 31, 15, 7, 3, 1, -1, 127, 63, 31, 15, 7, 3, 1,
        -1, 127, 63, 31, 15, 7, 3, 1,
    );
    // 1 << (8 - bit_idx) in each 16-bit half
    let bit_shift = _mm256_add_epi16(_mm256_shuffle_epi8(shr, bit_idx), _mm256_set1_epi16(0x0101));
    d0 = _mm256_srli_epi16::<8>(_mm256_mullo_epi16(d0, bit_shift));
    d1 = _mm256_and_si256(
        _mm256_mullo_epi16(d1, bit_shift),
        _mm256_set1_epi32(0xFF00FF00u32 as i32),
    );
    let bits = _mm256_or_si256(d0, d1);

    // 1 << m_n, where e_1 goes
    let ones = _mm256_set1_epi32(1);
    let twos = _mm256_set1_epi32(2);
    let u_q_m1 = _mm256_and_si256(_mm256_sub_epi32(u_q, ones), _mm256_set1_epi32(0x1F));
    let shift = _mm256_sllv_epi32(_mm256_sub_epi32(twos, e_k), u_q_m1);
    let mut ms = _mm256_and_si256(bits, _mm256_sub_epi32(shift, ones));
    let e_1 = _mm256_cmpeq_epi32(
        _mm256_and_si256(flags, _mm256_set1_epi32(0x800)),
        _mm256_setzero_si256(),
    );
    ms = _mm256_or_si256(ms, _mm256_andnot_si256(e_1, shift));
    ms = _mm256_or_si256(ms, ones); // bin center
    let v_n = _mm256_andnot_si256(insig, ms);
    let sign = _mm256_slli_epi32::<31>(bits);
    ms = _mm256_sll_epi32(_mm256_add_epi32(ms, twos), _mm_cvtsi32_si128(p as i32 - 1));
    ms = _mm256_or_si256(ms, sign);
    (_mm256_andnot_si256(insig, ms), v_n)
}

/// The 8 entries of `s` from `i` (4 quads, as in [`load_quads`]), zeros
/// past its end.
#[target_feature(enable = "avx2")]
#[inline]
fn load_quads4(s: &[u16], i: usize) -> __m128i {
    let mut a = [0u16; 8];
    match s.get(i..i + 8) {
        Some(src) => a.copy_from_slice(src),
        None => {
            let src = s.get(i..).unwrap_or(&[]);
            a[..src.len()].copy_from_slice(src);
        }
    }
    _mm_setr_epi16(
        a[0] as i16,
        a[1] as i16,
        a[2] as i16,
        a[3] as i16,
        a[4] as i16,
        a[5] as i16,
        a[6] as i16,
        a[7] as i16,
    )
}

/// The first 4 elements of `s` (one `vmovdqu`).
#[target_feature(enable = "avx2")]
#[inline]
fn load_u32x4(s: &[u32]) -> __m128i {
    let s: &[u32; 4] = s[..4].try_into().unwrap();
    _mm_setr_epi32(s[0] as i32, s[1] as i32, s[2] as i32, s[3] as i32)
}

/// Stores the 4 lanes of `v` at the start of `s`.
#[target_feature(enable = "avx2")]
#[inline]
fn store_u32x4(s: &mut [u32], v: __m128i) {
    let s: &mut [u32; 4] = (&mut s[..4]).try_into().unwrap();
    *s = [
        _mm_extract_epi32::<0>(v) as u32,
        _mm_extract_epi32::<1>(v) as u32,
        _mm_extract_epi32::<2>(v) as u32,
        _mm_extract_epi32::<3>(v) as u32,
    ];
}

/// `decode_four_quad16`: the 16 samples of the four quads whose `inf` and
/// `U_q` are the 32-bit lanes of `inf` and `u_q`, decoded from bit `pos`,
/// which moves past them, when no `U_q` of a quad with a significant
/// sample exceeds 15 and `p >= 17`. Lanes `4k` to `4k + 3` hold quad
/// `k`'s samples in the generic decoder's order, each the upper 16 bits
/// of its 32-bit value (the lower 16 are 0). Returns the samples and
/// their `v_n` (both 0 for an insignificant sample).
#[target_feature(enable = "avx2")]
#[inline]
fn decode_four_quads(
    inf: __m128i,
    u_q: __m128i,
    dbuf: &[u8],
    limit: usize,
    pos: &mut u32,
    p: u32,
) -> (__m256i, __m256i) {
    // each quad's 16-bit inf and U_q in its four lanes
    let spread = _mm256_setr_epi32(0, 0, 1, 1, 2, 2, 3, 3);
    let inf = _mm_or_si128(inf, _mm_slli_epi32::<16>(inf));
    let inf = _mm256_permutevar8x32_epi32(_mm256_castsi128_si256(inf), spread);
    // rho, the significance of each sample
    let rho = _mm256_and_si256(
        inf,
        _mm256_setr_epi16(
            0x10, 0x20, 0x40, 0x80, 0x10, 0x20, 0x40, 0x80, 0x10, 0x20, 0x40, 0x80, 0x10, 0x20,
            0x40, 0x80,
        ),
    );
    let insig = _mm256_cmpeq_epi16(rho, _mm256_setzero_si256());
    if _mm256_movemask_epi8(insig) == -1 {
        return (_mm256_setzero_si256(), _mm256_setzero_si256());
    }
    let u_q = _mm_or_si128(u_q, _mm_slli_epi32::<16>(u_q));
    let u_q = _mm256_permutevar8x32_epi32(_mm256_castsi128_si256(u_q), spread);

    // e_k at bit 15, e_1 at bit 11
    let flags = _mm256_and_si256(
        inf,
        _mm256_setr_epi16(
            0x1110,
            0x2220,
            0x4440,
            0x8880u16 as i16,
            0x1110,
            0x2220,
            0x4440,
            0x8880u16 as i16,
            0x1110,
            0x2220,
            0x4440,
            0x8880u16 as i16,
            0x1110,
            0x2220,
            0x4440,
            0x8880u16 as i16,
        ),
    );
    let flags = _mm256_mullo_epi16(
        flags,
        _mm256_setr_epi16(8, 4, 2, 1, 8, 4, 2, 1, 8, 4, 2, 1, 8, 4, 2, 1),
    );
    let e_k = _mm256_srli_epi16::<15>(flags);

    // the sample lengths, m_n, and their prefix sums within each pair of
    // quads (a 128-bit half)
    let m_n = _mm256_andnot_si256(insig, _mm256_sub_epi16(u_q, e_k));
    let mut inc_sum = _mm256_add_epi16(m_n, _mm256_bslli_epi128::<2>(m_n));
    inc_sum = _mm256_add_epi16(inc_sum, _mm256_bslli_epi128::<4>(inc_sum));
    inc_sum = _mm256_add_epi16(inc_sum, _mm256_bslli_epi128::<8>(inc_sum));
    let total0 = _mm256_extract_epi16::<7>(inc_sum) as u32;
    let total1 = _mm256_extract_epi16::<15>(inc_sum) as u32;

    let ms0 = dfetch(dbuf, limit, *pos);
    let ms1 = dfetch(dbuf, limit, *pos + total0);
    *pos += total0 + total1;
    let ms_vec = _mm256_set_m128i(ms1, ms0);

    // the 16 bits from each sample's first bit
    let ex_sum = _mm256_bslli_epi128::<2>(inc_sum);
    let mut byte_idx = _mm256_srli_epi16::<3>(ex_sum);
    let bit_idx = _mm256_and_si256(ex_sum, _mm256_set1_epi16(7));
    byte_idx = _mm256_shuffle_epi8(
        byte_idx,
        _mm256_setr_epi8(
            0, 0, 2, 2, 4, 4, 6, 6, 8, 8, 10, 10, 12, 12, 14, 14, 0, 0, 2, 2, 4, 4, 6, 6, 8, 8, 10,
            10, 12, 12, 14, 14,
        ),
    );
    byte_idx = _mm256_add_epi16(byte_idx, _mm256_set1_epi16(0x0100));
    let mut d0 = _mm256_shuffle_epi8(ms_vec, byte_idx);
    byte_idx = _mm256_add_epi16(byte_idx, _mm256_set1_epi16(0x0101));
    let mut d1 = _mm256_shuffle_epi8(ms_vec, byte_idx);

    let shr = _mm256_setr_epi8(
        -1, 127, 63, 31, 15, 7, 3, 1, -1, 127, 63, 31, 15, 7, 3, 1, -1, 127, 63, 31, 15, 7, 3, 1,
        -1, 127, 63, 31, 15, 7, 3, 1,
    );
    // 1 << (8 - bit_idx): the high byte of bit_idx is 0, so the shuffle
    // puts 0xFF there, and the carry of the low byte clears it
    let bit_shift = _mm256_add_epi16(_mm256_shuffle_epi8(shr, bit_idx), _mm256_set1_epi16(0x0101));
    d0 = _mm256_srli_epi16::<8>(_mm256_mullo_epi16(d0, bit_shift));
    d1 = _mm256_and_si256(
        _mm256_mullo_epi16(d1, bit_shift),
        _mm256_set1_epi16(0xFF00u16 as i16),
    );
    let bits = _mm256_or_si256(d0, d1);

    // 1 << m_n (m_n <= 15), where e_1 goes: the low byte looks up m_n
    // below 8, the high byte m_n - 8 (no lookup, so 0, when negative)
    let idx = _mm256_or_si256(
        m_n,
        _mm256_slli_epi16::<8>(_mm256_sub_epi16(m_n, _mm256_set1_epi16(8))),
    );
    let pow2 = _mm256_setr_epi8(
        1, 2, 4, 8, 16, 32, 64, -128, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 64, -128, 0, 0,
        0, 0, 0, 0, 0, 0,
    );
    let shift = _mm256_shuffle_epi8(pow2, idx);
    let ones = _mm256_set1_epi16(1);
    let mut ms = _mm256_and_si256(bits, _mm256_sub_epi16(shift, ones));
    let e_1 = _mm256_cmpeq_epi16(
        _mm256_and_si256(flags, _mm256_set1_epi16(0x800)),
        _mm256_setzero_si256(),
    );
    ms = _mm256_or_si256(ms, _mm256_andnot_si256(e_1, shift));
    ms = _mm256_or_si256(ms, ones); // bin center
    let v_n = _mm256_andnot_si256(insig, ms);
    let sign = _mm256_slli_epi16::<15>(bits);
    // the upper half of (v_n + 2) << (p - 1)
    ms = _mm256_sll_epi16(
        _mm256_add_epi16(ms, _mm256_set1_epi16(2)),
        _mm_cvtsi32_si128(p as i32 - 17),
    );
    ms = _mm256_or_si256(ms, sign);
    (_mm256_andnot_si256(insig, ms), v_n)
}

/// `U_q` of each quad of the row pair whose records start at `sp`, and
/// the first quad whose `U_q` exceeds `mmsbp2`. `v_n[k]` holds the
/// previous row pair's `v_n` of the bottom samples of columns `2k - 1`
/// and `2k`, for `k <= nq`. `W16` only gives each version of [`rows`]
/// its own copy, which LLVM compiles better than a shared one (about
/// 0.25 instructions per sample).
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
fn row_u_q<const W16: bool>(
    scratch: &[u16; SCRATCH_LEN],
    sp: usize,
    nq: usize,
    initial: bool,
    v_n: &[u32; V_N_SIZE + 16],
    u_q: &mut [u32; V_N_SIZE + 16],
    mmsbp2: u32,
) -> Option<usize> {
    if initial {
        for k in 0..nq {
            u_q[k] = scratch[sp + 2 * k + 1] as u32;
        }
        return (0..nq).find(|&k| u_q[k] > mmsbp2);
    }
    let limit = _mm256_set1_epi32(mmsbp2 as i32);
    let mut k = 0;
    while k < nq {
        let v = _mm256_or_si256(load_u32(&v_n[k..]), load_u32(&v_n[k + 1..]));
        let v = _mm256_or_si256(v, _mm256_set1_epi32(2));
        let emax = _mm256_sub_epi32(_mm256_set1_epi32(31), lzcnt_epi32(v));

        let inf_u_q = load_quads(scratch, sp + 2 * k);
        let gamma = _mm256_and_si256(inf_u_q, _mm256_set1_epi32(0xF0));
        let gamma = _mm256_and_si256(gamma, _mm256_sub_epi32(gamma, _mm256_set1_epi32(0x10)));
        let gamma0 = _mm256_cmpeq_epi32(gamma, _mm256_setzero_si256());
        // kappa: emax, or 1 where gamma is 0
        let kappa = _mm256_or_si256(
            _mm256_andnot_si256(gamma0, emax),
            _mm256_and_si256(gamma0, _mm256_set1_epi32(1)),
        );
        let uq = _mm256_add_epi32(_mm256_srli_epi32::<16>(inf_u_q), kappa);
        u_q[k..k + 8].copy_from_slice(&lanes_u32(uq));

        let over = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpgt_epi32(uq, limit))) as u32;
        let valid = if nq - k >= 8 {
            0xFF
        } else {
            (1u32 << (nq - k)) - 1
        };
        if over & valid != 0 {
            return Some(k + (over & valid).trailing_zeros() as usize);
        }
        k += 8;
    }
    None
}

/// Decodes the row pair at row `y` into `out`: the scalar `row_pair`.
/// Returns `false` when a `U_q` exceeds `mmsbp2`.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
#[allow(clippy::too_many_arguments)]
fn row_pair<const W16: bool>(
    scratch: &[u16; SCRATCH_LEN],
    sp: usize,
    width: usize,
    initial: bool,
    v_n: &mut [u32; V_N_SIZE + 16],
    u_q: &mut [u32; V_N_SIZE + 16],
    dbuf: &[u8],
    limit: usize,
    pos: &mut u32,
    p: u32,
    mmsbp2: u32,
    out: &mut impl RowPair<u32>,
) -> bool {
    let nq = width.div_ceil(2);
    let fail = row_u_q::<W16>(scratch, sp, nq, initial, v_n, u_q, mmsbp2);
    // the quads to decode
    let end = fail.unwrap_or(nq);
    if W16 {
        quads16(
            scratch, sp, width, end, initial, v_n, u_q, dbuf, limit, pos, p, out,
        );
    } else {
        quads32(scratch, sp, width, end, v_n, u_q, dbuf, limit, pos, p, out);
    }
    fail.is_none()
}

/// The first `end` quads of the row pair at `sp`, two at a time in 32-bit
/// lanes, into `out`, and their `v_n` into `v_n` (`v_n[nq]` too when
/// `end` is the row's quad count).
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
#[allow(clippy::too_many_arguments)]
fn quads32(
    scratch: &[u16; SCRATCH_LEN],
    sp: usize,
    width: usize,
    end: usize,
    v_n: &mut [u32; V_N_SIZE + 16],
    u_q: &[u32; V_N_SIZE + 16],
    dbuf: &[u8],
    limit: usize,
    pos: &mut u32,
    p: u32,
    out: &mut impl RowPair<u32>,
) {
    let nq = width.div_ceil(2);
    // in an odd-width block, the samples of the last column's right
    // neighbours are left alone
    let odd_mask = if width & 1 != 0 { !0xCCC0u32 } else { !0 };

    let mut carry = 0u32; // v_n of the previous quad's bottom-right sample
    let mut q = 0;
    while q < end {
        let inf0 = scratch[sp + 2 * q] as u32;
        let inf0 = if q + 1 == nq { inf0 & odd_mask } else { inf0 };
        let two = q + 1 < end;
        let inf1 = if two {
            let inf1 = scratch[sp + 2 * q + 2] as u32;
            if q + 2 == nq {
                inf1 & odd_mask
            } else {
                inf1
            }
        } else {
            0
        };
        let (row, vn) = decode_two_quads(inf0, u_q[q], inf1, u_q[q + 1], dbuf, limit, pos, p);
        let row = lanes_u32(_mm256_permutevar8x32_epi32(
            row,
            _mm256_setr_epi32(0, 2, 4, 6, 1, 3, 5, 7),
        ));
        let vn = lanes_u32(vn);

        let x = 2 * q;
        let n = (width - x).min(if two { 4 } else { 2 });
        if n == 4 {
            // a fixed length: no call to `memcpy`
            out.put_run(0, x, &row[..4]);
            out.put_run(1, x, &row[4..]);
        } else {
            out.put_run(0, x, &row[..n]);
            out.put_run(1, x, &row[4..4 + n]);
        }

        v_n[q] = carry | vn[1];
        carry = vn[3];
        if two {
            v_n[q + 1] = carry | vn[5];
            carry = vn[7];
        }
        q += 2;
    }
    if end == nq {
        v_n[nq] = carry;
    }
}

/// The first `end` quads of the row pair at `sp`, four at a time in
/// 16-bit lanes ([`decode_four_quads`]; every `U_q` of these is at most
/// `mmsbp2 <= 15`, and `p >= 17`), into `out`, and their `v_n` into
/// `v_n` (`v_n[nq]` too when `end` is the row's quad count).
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
#[allow(clippy::too_many_arguments)]
fn quads16(
    scratch: &[u16; SCRATCH_LEN],
    sp: usize,
    width: usize,
    end: usize,
    initial: bool,
    v_n: &mut [u32; V_N_SIZE + 16],
    u_q: &[u32; V_N_SIZE + 16],
    dbuf: &[u8],
    limit: usize,
    pos: &mut u32,
    p: u32,
    out: &mut impl RowPair<u32>,
) {
    let nq = width.div_ceil(2);
    // the columns to write
    let cols = width.min(2 * end);
    let lane = _mm_setr_epi32(0, 1, 2, 3);
    let low16 = _mm_set1_epi32(0xFFFF);
    let carry_lane = _mm256_setr_epi32(0, 2, 4, 6, 1, 3, 5, 7);

    let mut carry = 0u32; // v_n of the previous quad's bottom-right sample
    let mut q = 0;
    while q < end {
        let quads = load_quads4(scratch, sp + 2 * q);
        let mut inf = _mm_and_si128(quads, low16);
        let uq = if initial {
            _mm_srli_epi32::<16>(quads)
        } else {
            load_u32x4(&u_q[q..])
        };
        if q + 4 > end || (q + 4 >= nq && width & 1 != 0) {
            // quads from `end` on are not decoded, and in an odd-width
            // block the samples right of the last column are left alone
            let k = _mm_add_epi32(_mm_set1_epi32(q as i32), lane);
            let keep = _mm_cmpgt_epi32(_mm_set1_epi32(end as i32), k);
            let last = _mm_cmpeq_epi32(k, _mm_set1_epi32(nq as i32 - 1));
            let odd = if width & 1 != 0 { 0xCCC0 } else { 0 };
            let drop = _mm_and_si128(last, _mm_set1_epi32(odd));
            inf = _mm_andnot_si128(drop, _mm_and_si128(inf, keep));
        }
        let (row, vn) = decode_four_quads(inf, uq, dbuf, limit, pos, p);
        // samples 0 and 2 of a quad are on the top row, 1 and 3 below
        let top = lanes_u32(_mm256_slli_epi32::<16>(row));
        let bottom = lanes_u32(_mm256_and_si256(
            row,
            _mm256_set1_epi32(0xFFFF0000u32 as i32),
        ));
        let x = 2 * q;
        let n = (cols - x).min(8);
        if n == 8 {
            // a fixed length: no call to `memcpy`
            out.put_run(0, x, &top);
            out.put_run(1, x, &bottom);
        } else {
            out.put_run(0, x, &top[..n]);
            out.put_run(1, x, &bottom[..n]);
        }

        // v_n of samples 1 (low half) and 3 (high half) of each quad
        let vn = _mm256_permutevar8x32_epi32(_mm256_srli_epi32::<16>(vn), carry_lane);
        let s1 = _mm256_castsi256_si128(vn);
        let s3 = _mm256_extracti128_si256::<1>(vn);
        let before = _mm_or_si128(_mm_bslli_si128::<4>(s3), _mm_cvtsi32_si128(carry as i32));
        // past the last quad, s1 is 0: v_n[nq] gets its s3 here
        store_u32x4(&mut v_n[q..], _mm_or_si128(s1, before));
        carry = _mm_extract_epi32::<3>(s3) as u32;
        q += 4;
    }
    if end == nq && nq % 4 == 0 {
        v_n[nq] = carry;
    }
}

/// The scalar `decode_magsgn32` on an unstuffed copy of the segment held
/// in `dbuf`.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[allow(clippy::too_many_arguments)]
fn magsgn(
    dbuf: &mut [u8],
    cap: usize,
    scratch: &[u16; SCRATCH_LEN],
    src: &[u8],
    dec: &mut [u32],
    width: u32,
    height: u32,
    stride: u32,
    p: u32,
    mmsbp2: u32,
    sstr: usize,
) -> bool {
    // OpenJPH's choice: 16-bit lanes when every sample fits in them
    if mmsbp2 < 16 {
        rows::<true>(
            dbuf, cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    } else {
        rows::<false>(
            dbuf, cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    }
}

/// [`magsgn`] in 16-bit lanes ([`quads16`]) or 32-bit ones
/// ([`quads32`]). Not inlined, as in OpenJPH, so each version gets its
/// own register allocation. The segment is unstuffed here, where the
/// bounds of [`dfetch`]'s reads follow from it.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn rows<const W16: bool>(
    dbuf: &mut [u8],
    cap: usize,
    scratch: &[u16; SCRATCH_LEN],
    src: &[u8],
    dec: &mut [u32],
    width: u32,
    height: u32,
    stride: u32,
    p: u32,
    mmsbp2: u32,
    sstr: usize,
) -> bool {
    let limit = destuff_frwd(src, dbuf, cap);
    // exactly the bytes dfetch can read, so its reads need no bounds check
    let dbuf = &dbuf[..limit + PAD - 1];
    let mut pos = 0u32;
    let mut v_n = [0u32; V_N_SIZE + 16];
    let mut u_q = [0u32; V_N_SIZE + 16];
    let (w, stride) = (width as usize, stride as usize);

    let mut y = 0u32;
    while y == 0 || y < height {
        let sp = (y >> 1) as usize * sstr;
        let dp = y as usize * stride;
        let initial = y == 0;
        let ok = match whole_rows(dec, dp, stride, w) {
            Some(mut r) => row_pair::<W16>(
                scratch, sp, w, initial, &mut v_n, &mut u_q, dbuf, limit, &mut pos, p, mmsbp2,
                &mut r,
            ),
            None => row_pair::<W16>(
                scratch,
                sp,
                w,
                initial,
                &mut v_n,
                &mut u_q,
                dbuf,
                limit,
                &mut pos,
                p,
                mmsbp2,
                &mut Clipped { dec, dp, stride },
            ),
        };
        if !ok {
            return false;
        }
        y += 2;
    }
    true
}

/// Runs [`magsgn`] with an unstuffing buffer of `N` bytes on the stack.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn with_buf<const N: usize>(
    cap: usize,
    scratch: &[u16; SCRATCH_LEN],
    src: &[u8],
    dec: &mut [u32],
    width: u32,
    height: u32,
    stride: u32,
    p: u32,
    mmsbp2: u32,
    sstr: usize,
) -> bool {
    let mut dbuf = [0u8; N];
    magsgn(
        &mut dbuf, cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
    )
}

/// The MagSgn step (`decode_magsgn32` in [`super::decoder32`]), with the
/// same result and the same writes to `dec`. `None` for a block too large
/// for the unstuffing buffers (never one that JPEG 2000 allows) and for
/// rows that overlap in `dec`; the caller then runs the scalar step.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[allow(clippy::too_many_arguments)]
pub(super) fn decode_magsgn32(
    scratch: &[u16; SCRATCH_LEN],
    data: &[u8],
    dec: &mut [u32],
    lcup: i32,
    scup: i32,
    width: u32,
    height: u32,
    stride: u32,
    p: u32,
    mmsbp2: u32,
    sstr: usize,
) -> Option<bool> {
    // overlapping rows (`stride < width`): the scalar step's order of
    // stores, sample by sample, decides which value is left
    if width as usize > 2 * (V_N_SIZE - 4) || stride < width {
        return None;
    }
    let src = &data[..(lcup - scup) as usize];
    // the most bytes the row pairs can consume: mmsbp2 <= 31 bits a sample
    let quads = width.div_ceil(2) as usize * height.div_ceil(2).max(1) as usize;
    let cap = (quads * 4 * 31).div_ceil(8) + 1;
    let need = src.len().min(cap) + PAD;
    Some(if need <= 1024 {
        with_buf::<1024>(
            cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    } else if need <= 4096 {
        with_buf::<4096>(
            cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    } else if need <= 16384 {
        with_buf::<16384>(
            cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    } else if need <= 40960 {
        with_buf::<40960>(
            cap, scratch, src, dec, width, height, stride, p, mmsbp2, sstr,
        )
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::super::decoder32::decode;
    use super::super::encoder::encode_codeblock32;
    use super::super::readers::Frwd;
    use super::*;

    /// splitmix64
    fn mix(z: &mut u64) -> u64 {
        *z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = *z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    /// Samples of one of several kinds (random, zero, largest, sparse,
    /// small near bit-plane `p`, runs, every size), sign-magnitude.
    fn samples(kind: u64, n: usize, p: u32, seed: &mut u64) -> Vec<u32> {
        (0..n)
            .map(|i| {
                let r = mix(seed);
                let sign = (r >> 63) as u32 * 0x8000_0000;
                match kind % 7 {
                    0 => r as u32,
                    1 => 0,
                    2 => sign | 0x7FFF_FFFF,
                    3 if r % 16 == 0 => (r >> 8) as u32,
                    3 => 0,
                    4 => sign | (((r >> 8) as u32 % 8) << p >> 1),
                    5 if (i / 7) % 3 == 0 => sign | ((r >> 8) as u32 & 0x7FFF_FFFF),
                    5 => 0,
                    _ => sign | (((r >> 8) as u32 & 0x7FFF_FFFF) >> (r % 31)),
                }
            })
            .collect()
    }

    /// One decode with the scalar and with the AVX2 MagSgn step, into
    /// buffers of `dec_len` samples (plus a canary tail); the results and
    /// every sample written must be the same.
    #[allow(clippy::too_many_arguments)]
    fn check(
        coded: &[u8],
        missing_msbs: u32,
        passes: u32,
        l1: u32,
        l2: u32,
        w: u32,
        h: u32,
        stride: u32,
        causal: bool,
        dec_len: usize,
        canary: u32,
        what: &str,
    ) -> bool {
        let mut want = vec![canary; dec_len + 8];
        let mut got = vec![canary; dec_len + 8];
        let a = decode::<false>(
            coded,
            &mut want[..dec_len],
            missing_msbs,
            passes,
            l1,
            l2,
            w,
            h,
            stride,
            causal,
        );
        let b = decode::<true>(
            coded,
            &mut got[..dec_len],
            missing_msbs,
            passes,
            l1,
            l2,
            w,
            h,
            stride,
            causal,
        );
        assert_eq!(a, b, "{what}");
        if want != got {
            let i = want.iter().zip(&got).position(|(x, y)| x != y).unwrap();
            panic!("{what}: sample {i}: {:#x} against {:#x}", got[i], want[i]);
        }
        a
    }

    /// Block sizes: the powers of two JPEG 2000 allows, small and odd
    /// widths and heights, and a few more.
    fn sizes() -> Vec<(u32, u32)> {
        let mut sizes = Vec::new();
        for lw in 2..=10 {
            for lh in 2..=10 {
                if lw + lh <= 12 {
                    sizes.push((1u32 << lw, 1u32 << lh));
                }
            }
        }
        for w in 1..=40 {
            for h in [1, 2, 3, 4, 5, 8] {
                sizes.push((w, h));
            }
        }
        sizes.extend([
            (1024, 1),
            (1023, 3),
            (1009, 4),
            (17, 240),
            (33, 124),
            (1, 4096),
            (3, 1365),
        ]);
        sizes
    }

    #[test]
    fn matches_scalar_on_valid_blocks() {
        let mut seed = 0x6465_6332u64;
        let (mut decoded, mut total) = (0, 0);
        for (n, (w, h)) in sizes().into_iter().enumerate() {
            let kind = n as u64 % 7;
            let missing_msbs = [0, 1, 5, 14, 15, 16, 17, 20, 28, 29][n % 10];
            let p = 30 - missing_msbs;
            let stride = w + (n as u32 % 3) * 3;
            let mut buf = samples(kind, (stride * h) as usize, p, &mut seed);
            // magnitudes the block's bit-planes can hold
            let keep = (1u32 << (31 - missing_msbs)) - 1;
            for v in &mut buf {
                *v &= 0x8000_0000 | keep;
            }
            let mut coded = Vec::new();
            let Ok(len) = encode_codeblock32(&buf, missing_msbs, 1, w, h, stride, &mut coded)
            else {
                continue;
            };
            // an odd height writes the row below the block when it is in
            // the buffer; try both
            for extra in [0, 1] {
                let dec_len = (stride * (h + extra)) as usize;
                let what = format!("{w}x{h} kind {kind} mmsb {missing_msbs} +{extra}");
                decoded += check(
                    &coded,
                    missing_msbs,
                    1,
                    len,
                    0,
                    w,
                    h,
                    stride,
                    false,
                    dec_len,
                    0x5A5A_5A5A,
                    &what,
                ) as usize;
                total += 1;
            }
        }
        // all but the blocks too tall for the decoders' scratch arrays
        assert!(decoded + 8 >= total, "{decoded} of {total} decoded");
    }

    /// The 16-bit path at its limits: every block size and kind of
    /// samples, with the most bit-planes it takes (`mmsbp2` 15 and 14, so
    /// `U_q` up to 15 and samples up to bit 30) and the fewest (2).
    #[test]
    fn matches_scalar_at_16_bit_limits() {
        let mut seed = 0x3136_6269u64;
        let (mut decoded, mut total) = (0, 0);
        for (n, (w, h)) in sizes().into_iter().enumerate() {
            for (k, missing_msbs) in [13u32, 12, 0].into_iter().enumerate() {
                let kind = (n + k) as u64 % 7;
                let stride = w + (n as u32 % 3);
                let mut buf = samples(kind, (stride * h) as usize, 30 - missing_msbs, &mut seed);
                let keep = (1u32 << (31 - missing_msbs)) - 1;
                for v in &mut buf {
                    *v &= 0x8000_0000 | keep;
                }
                let mut coded = Vec::new();
                let Ok(len) = encode_codeblock32(&buf, missing_msbs, 1, w, h, stride, &mut coded)
                else {
                    continue;
                };
                let dec_len = (stride * (h + (n as u32 & 1))) as usize;
                let what = format!("{w}x{h} kind {kind} mmsb {missing_msbs}");
                decoded += check(
                    &coded,
                    missing_msbs,
                    1,
                    len,
                    0,
                    w,
                    h,
                    stride,
                    false,
                    dec_len,
                    0xA5A5_A5A5,
                    &what,
                ) as usize;
                total += 1;
                // one plane fewer than coded: a U_q too large somewhere,
                // with the quads before it written
                if missing_msbs > 0 {
                    let what = format!("{what} - 1");
                    check(
                        &coded,
                        missing_msbs - 1,
                        1,
                        len,
                        0,
                        w,
                        h,
                        stride,
                        false,
                        dec_len,
                        0xA5A5_A5A5,
                        &what,
                    );
                }
            }
        }
        assert!(decoded + 24 >= total, "{decoded} of {total} decoded");
    }

    #[test]
    fn matches_scalar_on_corrupt_blocks() {
        let mut seed = 0x6261_6432u64;
        let (mut ok, mut failed) = (0, 0);
        let sizes = sizes();
        for round in 0..20000usize {
            let (w, h) = sizes[mix(&mut seed) as usize % sizes.len()];
            let enc_mmsb = (mix(&mut seed) % 30) as u32;
            let p = 30 - enc_mmsb;
            let stride = w + (mix(&mut seed) % 5) as u32;
            let mut buf = samples(mix(&mut seed) % 7, (stride * h) as usize, p, &mut seed);
            let keep = (1u32 << (31 - enc_mmsb)) - 1;
            for v in &mut buf {
                *v &= 0x8000_0000 | keep;
            }
            let mut coded = Vec::new();
            let Ok(len) = encode_codeblock32(&buf, enc_mmsb, 1, w, h, stride, &mut coded) else {
                continue;
            };
            let mut l1 = len;
            let r = mix(&mut seed);
            // decode with fewer missing MSBs than coded: U_q too large
            let mut missing_msbs = if r % 4 == 0 {
                enc_mmsb.saturating_sub(1 + (r >> 8) as u32 % 4)
            } else {
                enc_mmsb
            };
            match (r >> 16) % 6 {
                0 => {}
                1 => {
                    // flip bits, mostly in the MagSgn segment
                    for _ in 0..1 + (r >> 24) % 8 {
                        let i = mix(&mut seed) as usize % (l1 as usize).max(1);
                        coded[i] ^= 1 << (mix(&mut seed) % 8);
                    }
                }
                2 => {
                    // random bytes, many of them 0xFF
                    for b in coded.iter_mut().take(l1.saturating_sub(2) as usize) {
                        let x = mix(&mut seed);
                        if x % 3 == 0 {
                            *b = if x % 2 == 0 { 0xFF } else { x as u8 };
                        }
                    }
                }
                3 => {
                    // a truncated MagSgn segment
                    let scup = ((coded[l1 as usize - 1] as u32) << 4)
                        + (coded[l1 as usize - 2] as u32 & 0xF);
                    let cut = (mix(&mut seed) as u32) % (l1 - scup + 1);
                    let tail: Vec<u8> = coded[cut as usize..l1 as usize].to_vec();
                    coded.splice(0..l1 as usize, tail);
                    l1 -= cut;
                }
                4 => missing_msbs = (mix(&mut seed) % 30) as u32,
                _ => {
                    for b in coded.iter_mut().take(l1.saturating_sub(2) as usize) {
                        *b = mix(&mut seed) as u8;
                    }
                }
            }
            // SigProp and MagRef passes of random bytes
            let passes = 1 + (mix(&mut seed) % 3) as u32;
            let l2 = if passes > 1 {
                (mix(&mut seed) % 64) as u32
            } else {
                0
            };
            coded.truncate(l1 as usize);
            for _ in 0..l2 {
                coded.push(mix(&mut seed) as u8);
            }
            // now and then a stride below the width: overlapping rows
            let stride = if mix(&mut seed) % 8 == 0 {
                (mix(&mut seed) % w.min(4) as u64) as u32
            } else {
                stride
            };
            let extra = (mix(&mut seed) % 2) as u32;
            let mut dec_len = ((stride * (h + extra)) as usize).max(w as usize);
            if mix(&mut seed) % 8 == 0 {
                dec_len -= dec_len / 3; // too short: the row pairs are clipped
            }
            let causal = mix(&mut seed) % 2 == 0;
            let what =
                format!("round {round}: {w}x{h} mmsb {missing_msbs}/{enc_mmsb} passes {passes}");
            if check(
                &coded,
                missing_msbs,
                passes,
                l1,
                l2,
                w,
                h,
                stride,
                causal,
                dec_len,
                mix(&mut seed) as u32,
                &what,
            ) {
                ok += 1;
            } else {
                failed += 1;
            }
        }
        assert!(ok > 5000 && failed > 1000, "{ok} decoded, {failed} failed");
    }

    /// `dfetch` on the unstuffed bytes gives the bits the scalar reader
    /// gives, at every position, past the end of the data, and up to the
    /// `cap`.
    #[test]
    fn dfetch_matches_frwd() {
        let mut seed = 0x6466_6574u64;
        for round in 0..3000 {
            let n = (mix(&mut seed) % 200) as usize;
            let ff = mix(&mut seed) % 4;
            let data: Vec<u8> = (0..n)
                .map(|_| {
                    let r = mix(&mut seed);
                    if r % 4 < ff {
                        0xFF
                    } else {
                        r as u8
                    }
                })
                .collect();
            let cap = if round % 3 == 0 {
                (mix(&mut seed) % 64) as usize
            } else {
                4096
            };
            let mut dbuf = vec![0u8; n.min(cap) + PAD];
            let limit = destuff_frwd(&data, &mut dbuf, cap);
            assert!(limit + 24 <= dbuf.len());

            let mut frwd = Frwd::<0xFF>::new(&data, 0, n as i32);
            let mut pos = 0u32;
            while (pos as usize) < 8 * cap.min(n + 40) {
                let want = frwd.fetch();
                let Some(got) = fetch32(&dbuf, limit, pos) else {
                    return;
                };
                // within the cap, every bit; past it, only the ones the
                // decoder could still use
                let valid = (8 * cap).saturating_sub(pos as usize).min(32) as u32;
                let mask = if valid == 32 {
                    u32::MAX
                } else {
                    (1u32 << valid) - 1
                };
                assert_eq!(
                    got & mask,
                    want & mask,
                    "round {round}, {n} bytes, bit {pos}"
                );
                let k = (mix(&mut seed) % 33) as u32;
                frwd.advance(k);
                pos += k;
            }
        }
    }

    crate::simd::avx2_dispatch! {
        /// The low 32 bits of `dfetch`.
        fn fetch32(dbuf: &[u8], limit: usize, pos: u32) -> Option<u32> {
            avx2 => Some(_mm_cvtsi128_si32(dfetch(dbuf, limit, pos)) as u32),
            fallback => None,
        }
    }

    /// The upper 96 bits of `dfetch` too.
    #[test]
    fn dfetch_gives_128_bits() {
        let mut seed = 0x3132_3862u64;
        let data: Vec<u8> = (0..300).map(|_| mix(&mut seed) as u8).collect();
        let mut dbuf = vec![0u8; data.len() + PAD];
        let limit = destuff_frwd(&data, &mut dbuf, 4096);
        for pos in 0..8 * 280u32 {
            let Some(got) = fetch128(&dbuf, limit, pos) else {
                return;
            };
            for k in 0..4 {
                let want = fetch32(&dbuf, limit, pos + 32 * k).unwrap();
                assert_eq!(got[k as usize], want, "bit {pos} word {k}");
            }
        }
    }

    crate::simd::avx2_dispatch! {
        fn fetch128(dbuf: &[u8], limit: usize, pos: u32) -> Option<[u32; 4]> {
            avx2 => {
                let v = dfetch(dbuf, limit, pos);
                Some([
                    _mm_extract_epi32::<0>(v) as u32,
                    _mm_extract_epi32::<1>(v) as u32,
                    _mm_extract_epi32::<2>(v) as u32,
                    _mm_extract_epi32::<3>(v) as u32,
                ])
            },
            fallback => None,
        }
    }
}
