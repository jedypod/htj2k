// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! AVX2 HT block encoder for 32-bit samples (port of OpenJPH's
//! `ojph_block_encoder_avx2.cpp`), for the `simd` feature.
//!
//! The structure is OpenJPH's: each step takes 16 columns of a row pair,
//! that is 8 quads, and works out their exponents, significance,
//! contexts, `u_q`, `eps` and VLC table entries with 8-lane vectors, one
//! quad per lane. The bits then go out per quad in the generic encoder's
//! order: each of the three streams (MEL, VLC and MagSgn) gets the same
//! sequence of bits as from [`super::encoder`], only in larger pieces.
//! The writers gather up to 64 bits and write up to 8 bytes at a time,
//! with the generic encoder's bit stuffing.
//!
//! The C++ has no checks on its buffers. Here the MagSgn bytes go
//! straight to the output, the MEL and VLC writers have room for one more
//! step past the generic encoder's buffer sizes, the positions are
//! checked against those sizes after every step, and at the end
//! [`encode32`] returns `None` if the generic encoder would have failed,
//! or might have: its caller then runs the generic encoder, which
//! returns the error. This happens only for blocks far larger than
//! JPEG 2000 allows. Otherwise the bytes are the generic encoder's.

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

use super::encoder::{MEL_EXP, MEL_SIZE, MS_SIZE32, VLC_SIZE, VLC_TBL0, VLC_TBL1};
use crate::simd::x86::*;

/// The VLC buffer: the generic encoder's size and room for one step past
/// it (a step writes at most 4 x 60 bits, plus stuffing).
const VLC_ROOM: usize = VLC_SIZE + 64;
/// The same for the MEL buffer (a step codes at most 12 symbols of at
/// most 6 bits).
const MEL_ROOM: usize = MEL_SIZE + 64;

// ---------------------------------------------------------------------------
// UVLC tables
// ---------------------------------------------------------------------------

/// `ulvc_cwd_pre`, `_pre_len`, `_suf`, `_suf_len` of the C++ for `u_q`
/// from 0 to 32 (the 32-bit encoder needs no extension).
const fn uvlc_entry(u: usize) -> (u32, u32, u32, u32) {
    match u {
        0 => (0, 0, 0, 0),
        1 => (1, 1, 0, 0),
        2 => (2, 2, 0, 0),
        3 => (4, 3, 0, 1),
        4 => (4, 3, 1, 1),
        _ => (0, 3, (u - 5) as u32, 5),
    }
}

/// `uvlc_tbl_pair1` (`initial`, for the initial row of quads) and
/// `uvlc_tbl_pair2`: the UVLC codewords of a pair of quads, indexed by
/// `u_q0 * 33 + u_q1`, as `(cwd << 5) | len`.
const fn build_uvlc_pairs(initial: bool) -> [u32; 33 * 33] {
    let mut t = [0u32; 33 * 33];
    let mut uq0 = 0;
    while uq0 < 33 {
        let mut uq1 = 0;
        while uq1 < 33 {
            let mut cwd = 0u32;
            let mut len = 0u32;
            if initial && uq0 > 2 && uq1 > 2 {
                let a = uvlc_entry(uq0 - 2);
                let b = uvlc_entry(uq1 - 2);
                cwd |= a.0;
                len += a.1;
                cwd |= b.0 << len;
                len += b.1;
                cwd |= a.2 << len;
                len += a.3;
                cwd |= b.2 << len;
                len += b.3;
            } else if initial && uq0 > 2 && uq1 > 0 {
                let a = uvlc_entry(uq0);
                cwd |= a.0;
                len += a.1;
                cwd |= ((uq1 - 1) as u32) << len;
                len += 1;
                cwd |= a.2 << len;
                len += a.3;
            } else {
                let a = uvlc_entry(uq0);
                let b = uvlc_entry(uq1);
                cwd |= a.0;
                len += a.1;
                cwd |= b.0 << len;
                len += b.1;
                cwd |= a.2 << len;
                len += a.3;
                cwd |= b.2 << len;
                len += b.3;
            }
            t[uq0 * 33 + uq1] = (cwd << 5) | len;
            uq1 += 1;
        }
        uq0 += 1;
    }
    t
}

static UVLC_PAIR1: [u32; 33 * 33] = build_uvlc_pairs(true);
static UVLC_PAIR2: [u32; 33 * 33] = build_uvlc_pairs(false);

// ---------------------------------------------------------------------------
// writers
// ---------------------------------------------------------------------------

/// `mel_struct` of the AVX2 C++, which emits several bits at once.
struct Mel {
    buf: [u8; MEL_ROOM],
    pos: usize,
    remaining_bits: i32,
    tmp: i32,
    run: i32,
    k: i32,
    threshold: i32,
}

impl Mel {
    fn new() -> Self {
        Mel {
            buf: [0; MEL_ROOM],
            pos: 0,
            remaining_bits: 8,
            tmp: 0,
            run: 0,
            k: 0,
            threshold: 1,
        }
    }

    /// `mel_emit_bits`: at most 6 bits, so at most one byte completes.
    #[inline(always)]
    fn emit_bits(&mut self, bits: i32, num_bits: i32) {
        self.tmp = (self.tmp << num_bits) | bits;
        self.remaining_bits -= num_bits;
        if self.remaining_bits <= 0 {
            let excess = -self.remaining_bits;
            let byte = (self.tmp >> excess) as u8;
            self.buf[self.pos] = byte;
            self.pos += 1;
            self.tmp &= (1 << excess) - 1;
            self.remaining_bits += 8 - (byte == 0xFF) as i32;
        }
    }

    /// `mel_encode`.
    #[inline(always)]
    fn encode(&mut self, bit: bool) {
        if !bit {
            self.run += 1;
            if self.run >= self.threshold {
                self.emit_bits(1, 1);
                self.run = 0;
                self.k = (self.k + 1).min(12);
                self.threshold = 1 << MEL_EXP[self.k as usize];
            }
        } else {
            let t = MEL_EXP[self.k as usize];
            self.emit_bits(self.run & ((1 << t) - 1), t + 1);
            self.run = 0;
            self.k = (self.k - 1).max(0);
            self.threshold = 1 << MEL_EXP[self.k as usize];
        }
    }
}

/// `vlc_struct` of the AVX2 C++, writing forwards: the bytes are reversed
/// when copied out, which the C++ does by writing backwards.
struct Vlc {
    buf: [u8; VLC_ROOM],
    pos: usize,
    used_bits: u32,
    tmp: u64,
    last_greater_than_8f: bool,
}

impl Vlc {
    fn new() -> Self {
        let mut buf = [0; VLC_ROOM];
        buf[0] = 0xFF;
        Vlc {
            buf,
            pos: 1,
            used_bits: 4,
            tmp: 0xF,
            last_greater_than_8f: true,
        }
    }

    /// `vlc_drain`: writes the complete bytes; a byte after one above
    /// 0x8F holds 7 bits if they are all ones.
    #[inline(always)]
    fn drain(&mut self) {
        while self.used_bits >= 8 {
            let stuff = self.last_greater_than_8f && (self.tmp & 0x7F) == 0x7F;
            let bits = 8 - stuff as u32;
            let byte = (self.tmp & ((1 << bits) - 1)) as u8;
            self.buf[self.pos] = byte;
            self.pos += 1;
            self.tmp >>= bits;
            self.used_bits -= bits;
            self.last_greater_than_8f = byte > 0x8F;
        }
    }

    /// `vlc_encode`: `cwd` has no bits above `cwd_len`, at most 64.
    #[inline(always)]
    fn encode(&mut self, mut cwd: u64, mut cwd_len: u32) {
        loop {
            let avail = 64 - self.used_bits;
            if avail > 0 && cwd_len <= avail {
                self.tmp |= cwd << self.used_bits;
                self.used_bits += cwd_len;
                return;
            }
            if avail > 0 {
                self.tmp |= cwd << self.used_bits;
            }
            self.used_bits = 64;
            self.drain();
            cwd = cwd.checked_shr(avail).unwrap_or(0);
            cwd_len -= avail;
        }
    }
}

/// `ms_struct` of the AVX2 C++: a 64-bit accumulator, written up to 8
/// bytes at a time; a byte after 0xFF holds 7 bits.
///
/// The bytes go to the end of the output, where the MagSgn segment comes
/// first, through a small buffer that [`Self::flush`] empties after every
/// step, rather than to a buffer of the C++'s size, which would have to be
/// zeroed for every block.
struct Ms<'a> {
    out: &'a mut Vec<u8>,
    /// Where the segment starts in `out`.
    start: usize,
    /// The bytes not yet in `out`: a step writes fewer than 160, and the
    /// writer stores 8 at a time.
    stage: [u8; 256],
    staged: usize,
    used_bits: u32,
    tmp: u64,
    last_was_ff: bool,
}

impl<'a> Ms<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        let start = out.len();
        Ms {
            out,
            start,
            stage: [0; 256],
            staged: 0,
            used_bits: 0,
            tmp: 0,
            last_was_ff: false,
        }
    }

    /// The number of bytes written.
    #[inline(always)]
    fn pos(&self) -> usize {
        self.out.len() - self.start + self.staged
    }

    /// Writes the first `n` bytes of `word`.
    #[inline(always)]
    fn put8(&mut self, word: u64, n: usize) {
        self.stage[self.staged..self.staged + 8].copy_from_slice(&word.to_le_bytes());
        self.staged += n;
    }

    /// Writes a byte.
    #[inline(always)]
    fn push(&mut self, byte: u8) {
        self.stage[self.staged] = byte;
        self.staged += 1;
    }

    /// Moves the staged bytes to the output.
    #[inline(always)]
    fn flush(&mut self) {
        self.out.extend_from_slice(&self.stage[..self.staged]);
        self.staged = 0;
    }

    /// `ms_drain`.
    #[inline(always)]
    fn drain(&mut self) {
        if self.last_was_ff {
            if self.used_bits < 7 {
                return;
            }
            self.push((self.tmp & 0x7F) as u8);
            self.tmp >>= 7;
            self.used_bits -= 7;
            self.last_was_ff = false;
        }
        while self.used_bits >= 8 {
            let n_bytes = (self.used_bits >> 3).min(8);
            let word = self.tmp;
            let valid_mask = if n_bytes < 8 {
                (1u64 << (n_bytes * 8)) - 1
            } else {
                !0
            };
            // the bytes of `word` that are 0xFF (and possibly bytes above
            // the first of them)
            let w = !word;
            let ff = w.wrapping_sub(0x0101_0101_0101_0101) & !w & 0x8080_8080_8080_8080;
            let ff = ff & valid_mask;
            if ff == 0 {
                self.put8(word, n_bytes as usize);
                self.tmp = self.tmp.checked_shr(n_bytes * 8).unwrap_or(0);
                self.used_bits -= n_bytes * 8;
            } else {
                // up to and including the first 0xFF
                let safe = (ff.trailing_zeros() >> 3) + 1;
                self.put8(word, safe as usize);
                self.tmp = self.tmp.checked_shr(safe * 8).unwrap_or(0);
                self.used_bits -= safe * 8;
                if self.used_bits >= 7 {
                    self.push((self.tmp & 0x7F) as u8);
                    self.tmp >>= 7;
                    self.used_bits -= 7;
                    self.last_was_ff = false;
                } else {
                    self.last_was_ff = true;
                    return;
                }
            }
        }
    }

    /// `ms_encode_nodefer`: `cwd` has no bits above `cwd_len`, at most
    /// 64.
    #[inline(always)]
    fn encode(&mut self, mut cwd: u64, mut cwd_len: u32) {
        loop {
            let avail = 64 - self.used_bits;
            if avail > 0 && cwd_len <= avail {
                self.tmp |= cwd.checked_shl(self.used_bits).unwrap_or(0);
                self.used_bits += cwd_len;
                return;
            }
            if avail > 0 {
                self.tmp |= cwd << self.used_bits;
            }
            self.used_bits = 64;
            self.drain();
            cwd = cwd.checked_shr(avail).unwrap_or(0);
            cwd_len -= avail;
        }
    }

    /// `ms_terminate`, with the C++'s buffer check, then [`Self::flush`].
    fn terminate(&mut self) -> Option<()> {
        self.drain();
        if self.used_bits != 0 {
            let max_bits = if self.last_was_ff { 7 } else { 8 };
            let t = max_bits - self.used_bits;
            let mut byte = (self.tmp & ((1u64 << self.used_bits) - 1)) as u32;
            byte |= (0xFF & ((1u32 << t) - 1)) << self.used_bits;
            if byte != 0xFF {
                if self.pos() >= MS_SIZE32 {
                    return None;
                }
                self.push(byte as u8);
            }
        } else if self.last_was_ff {
            // drop the last byte, a 0xFF
            self.flush();
            self.out.pop();
        }
        self.flush();
        Some(())
    }
}

/// `terminate_mel_vlc` of the AVX2 C++, after `vlc_drain`; `None` where
/// the generic encoder would fail.
fn terminate_mel_vlc(mel: &mut Mel, vlc: &mut Vlc) -> Option<()> {
    if mel.run > 0 {
        mel.emit_bits(1, 1);
    }
    if vlc.last_greater_than_8f && (vlc.tmp & 0x7F) == 0x7F {
        vlc.buf[vlc.pos] = 0x7F;
        vlc.pos += 1;
        vlc.tmp >>= 7;
        vlc.used_bits -= 7;
    }
    mel.tmp <<= mel.remaining_bits;
    let mel_mask = (0xFF << mel.remaining_bits) & 0xFF;
    let vlc_mask = 0xFF >> (8 - vlc.used_bits);
    if (mel_mask | vlc_mask) == 0 {
        return Some(());
    }
    if mel.pos >= MEL_SIZE {
        return None;
    }
    let vlc_tmp = vlc.tmp as u8 as i32;
    let fuse = mel.tmp | vlc_tmp;
    if (((fuse ^ mel.tmp) & mel_mask) | ((fuse ^ vlc_tmp) & vlc_mask)) == 0
        && fuse != 0xFF
        && vlc.pos > 1
    {
        mel.buf[mel.pos] = fuse as u8;
        mel.pos += 1;
    } else {
        if vlc.pos >= VLC_SIZE {
            return None;
        }
        mel.buf[mel.pos] = mel.tmp as u8;
        mel.pos += 1;
        vlc.buf[vlc.pos] = vlc_tmp as u8;
        vlc.pos += 1;
    }
    Some(())
}

// ---------------------------------------------------------------------------
// vector steps
// ---------------------------------------------------------------------------

#[target_feature(enable = "avx2")]
#[inline]
fn one() -> __m256i {
    _mm256_set1_epi32(1)
}

#[target_feature(enable = "avx2")]
#[inline]
fn cmpneq_zero(v: __m256i) -> __m256i {
    _mm256_xor_si256(
        _mm256_cmpeq_epi32(v, _mm256_setzero_si256()),
        _mm256_set1_epi32(-1),
    )
}

/// `avx2_lzcnt_epi32`: the leading zeros of each lane, through the float
/// exponent; exact for every value (the bits below the top 8 that could
/// round the conversion up are cleared first).
#[target_feature(enable = "avx2")]
#[inline]
pub(super) fn lzcnt_epi32(v: __m256i) -> __m256i {
    let v = _mm256_andnot_si256(_mm256_srli_epi32::<8>(v), v);
    let v = _mm256_castps_si256(_mm256_cvtepi32_ps(v));
    let v = _mm256_srli_epi32::<23>(v);
    let v = _mm256_subs_epu16(_mm256_set1_epi32(158), v);
    _mm256_min_epi16(v, _mm256_set1_epi32(32))
}

/// The per-quad values of 8 quads (`proc_pixel`): `e_q` and `s` of each
/// of a quad's 4 samples (index 0 to 3 as in the generic encoder: top
/// left, bottom left, top right, bottom right), and the quad's `rho` and
/// `e_qmax`, one quad per lane.
struct Quads {
    e_q: [__m256i; 4],
    s: [__m256i; 4],
    rho: __m256i,
    e_qmax: __m256i,
}

/// `proc_pixel`: `src` is row 0 of columns 0 to 7, row 1 of columns 0 to
/// 7, then the same for columns 8 to 15.
#[target_feature(enable = "avx2")]
#[inline]
fn proc_pixel(src: &[__m256i; 4], shift: __m128i) -> Quads {
    let zero = _mm256_setzero_si256();
    let mut e_q = [zero; 4];
    let mut s = [zero; 4];
    let mut sig = [zero; 4];
    for i in 0..4 {
        // val = t + t; val >>= p; val &= ~1u;
        let mut val = _mm256_add_epi32(src[i], src[i]);
        val = _mm256_srl_epi32(val, shift);
        val = _mm256_and_si256(val, _mm256_set1_epi32(!1));
        let nonzero = cmpneq_zero(val);
        // e_q = 32 - count_leading_zeros(--val)
        val = _mm256_sub_epi32(val, one());
        let e = _mm256_sub_epi32(_mm256_set1_epi32(32), lzcnt_epi32(val));
        // s = --val + (t >> 31)
        val = _mm256_sub_epi32(val, one());
        let sv = _mm256_add_epi32(_mm256_srli_epi32::<31>(src[i]), val);
        e_q[i] = _mm256_and_si256(e, nonzero);
        s[i] = _mm256_and_si256(sv, nonzero);
        sig[i] = _mm256_srli_epi32::<31>(nonzero);
    }

    // from rows (0, 1, 0, 1) of columns (0..8, 0..8, 8..16, 8..16) to
    // samples (top left, bottom left, top right, bottom right) of quads
    // 0 to 7
    let idx = _mm256_setr_epi32(0, 2, 4, 6, 1, 3, 5, 7);
    let mut q = Quads {
        e_q: [zero; 4],
        s: [zero; 4],
        rho: zero,
        e_qmax: zero,
    };
    let mut r = [zero; 4];
    for i in 0..2 {
        let t1 = _mm256_permutevar8x32_epi32(e_q[i], idx);
        let t2 = _mm256_permutevar8x32_epi32(e_q[2 + i], idx);
        q.e_q[i] = _mm256_permute2x128_si256::<0x20>(t1, t2);
        q.e_q[2 + i] = _mm256_permute2x128_si256::<0x31>(t1, t2);

        let t1 = _mm256_permutevar8x32_epi32(s[i], idx);
        let t2 = _mm256_permutevar8x32_epi32(s[2 + i], idx);
        q.s[i] = _mm256_permute2x128_si256::<0x20>(t1, t2);
        q.s[2 + i] = _mm256_permute2x128_si256::<0x31>(t1, t2);

        let t1 = _mm256_permutevar8x32_epi32(sig[i], idx);
        let t2 = _mm256_permutevar8x32_epi32(sig[2 + i], idx);
        r[i] = _mm256_permute2x128_si256::<0x20>(t1, t2);
        r[2 + i] = _mm256_permute2x128_si256::<0x31>(t1, t2);
    }
    q.e_qmax = _mm256_max_epi32(
        _mm256_max_epi32(q.e_q[0], q.e_q[1]),
        _mm256_max_epi32(q.e_q[2], q.e_q[3]),
    );
    q.rho = _mm256_or_si256(
        _mm256_or_si256(r[0], _mm256_slli_epi32::<1>(r[1])),
        _mm256_or_si256(_mm256_slli_epi32::<2>(r[2]), _mm256_slli_epi32::<3>(r[3])),
    );
    q
}

/// `rotate_matrix`: from one vector per sample index (lane = quad) to
/// one vector per two quads (lanes 0 to 3 the samples of quad 2i, 4 to 7
/// those of quad 2i + 1).
#[target_feature(enable = "avx2")]
#[inline]
fn rotate_matrix(m: &mut [__m256i; 4]) {
    let t1 = _mm256_unpacklo_epi32(m[0], m[1]);
    let t2 = _mm256_unpacklo_epi32(m[2], m[3]);
    let t3 = _mm256_unpackhi_epi32(m[0], m[1]);
    let t4 = _mm256_unpackhi_epi32(m[2], m[3]);

    let a0 = _mm256_unpacklo_epi64(t1, t2);
    let a1 = _mm256_unpacklo_epi64(t3, t4);
    let a2 = _mm256_unpackhi_epi64(t1, t2);
    let a3 = _mm256_unpackhi_epi64(t3, t4);

    m[0] = _mm256_permute2x128_si256::<0x20>(a0, a2);
    m[2] = _mm256_permute2x128_si256::<0x31>(a0, a2);
    m[1] = _mm256_permute2x128_si256::<0x20>(a1, a3);
    m[3] = _mm256_permute2x128_si256::<0x31>(a1, a3);
}

/// `proc_ms_encode`: the MagSgn bits of 8 quads.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
fn proc_ms_encode(
    ms: &mut Ms<'_>,
    tuple: __m256i,
    uq: __m256i,
    rho: __m256i,
    s: &mut [__m256i; 4],
) {
    let zero = _mm256_setzero_si256();
    let mut m = [zero; 4];
    // m = (rho & (1 << i)) ? Uq - ((tuple >> i) & 1) : 0
    for (i, m) in m.iter_mut().enumerate() {
        let bit = _mm256_set1_epi32(1 << i);
        let e_k = _mm256_srl_epi32(_mm256_and_si256(tuple, bit), _mm_cvtsi32_si128(i as i32));
        let t = _mm256_sub_epi32(uq, e_k);
        let mask = cmpneq_zero(_mm256_and_si256(rho, bit));
        *m = _mm256_and_si256(mask, t);
    }
    rotate_matrix(&mut m);
    rotate_matrix(s);

    for i in 0..4 {
        let len = lanes_u32(m[i]);
        let mask = _mm256_sub_epi32(_mm256_sllv_epi32(one(), m[i]), one());
        let cwd = lanes_u32(_mm256_and_si256(mask, s[i]));
        for j in [0usize, 4] {
            // samples j, j + 1 and then j + 2, j + 3 (each at most 31
            // bits), together if they fit in 64 bits
            let mut c = cwd[j] as u64 | (cwd[j + 1] as u64) << len[j];
            let mut l = len[j] + len[j + 1];
            let l1 = len[j + 2] + len[j + 3];
            let c1 = cwd[j + 2] as u64 | (cwd[j + 3] as u64) << len[j + 2];
            if l + l1 <= 64 {
                c |= c1.checked_shl(l).unwrap_or(0);
                l += l1;
                ms.encode(c, l);
            } else {
                ms.encode(c, l);
                ms.encode(c1, l1);
            }
        }
    }
    ms.drain();
    ms.flush();
}

/// The state carried from step to step and from row pair to row pair.
struct State<'a> {
    mel: Mel,
    vlc: Vlc,
    ms: Ms<'a>,
    /// `E` of the quads of the row pair above, 8 per vector (and one
    /// vector more, for the quad after the last).
    e_val: [__m256i; 65],
    /// The context bits of the row pair above, likewise.
    cx_val: [__m256i; 65],
    prev_e_val: __m256i,
    prev_cx_val: __m256i,
    prev_cq: i32,
}

/// `encode_x_loop`: one row pair, `row0` and `row1` (`None` below the
/// last row), `width` samples each; `initial` for the first row pair.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
#[inline]
fn x_loop<const INITIAL: bool>(
    st: &mut State<'_>,
    row0: &[u32],
    row1: Option<&[u32]>,
    shift: __m128i,
) -> Option<()> {
    let width = row0.len();
    let n_loop = width.div_ceil(16);
    let ignore = n_loop * 16 - width;
    let zero = _mm256_setzero_si256();
    let ones = _mm256_set1_epi32(-1);
    let right_shift = _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 0);
    let left_shift = _mm256_setr_epi32(7, 0, 1, 2, 3, 4, 5, 6);
    let vlc_tbl: &[u16; 2048] = if INITIAL { &VLC_TBL0 } else { &VLC_TBL1 };
    let uvlc_tbl: &[u32; 33 * 33] = if INITIAL { &UVLC_PAIR1 } else { &UVLC_PAIR2 };

    for x in 0..n_loop {
        if st.ms.pos() >= MS_SIZE32 || st.vlc.pos >= VLC_SIZE || st.mel.pos > MEL_SIZE {
            return None;
        }

        // t = sp[i]
        let c = 16 * x;
        let mut src = [zero; 4];
        if x == n_loop - 1 && ignore != 0 {
            let mut t = [0u32; 16];
            t[..width - c].copy_from_slice(&row0[c..]);
            src[0] = load_u32(&t[..8]);
            src[2] = load_u32(&t[8..]);
            if let Some(row1) = row1 {
                t[..width - c].copy_from_slice(&row1[c..]);
                src[1] = load_u32(&t[..8]);
                src[3] = load_u32(&t[8..]);
            }
        } else {
            src[0] = load_u32(&row0[c..c + 8]);
            src[2] = load_u32(&row0[c + 8..c + 16]);
            if let Some(row1) = row1 {
                src[1] = load_u32(&row1[c..c + 8]);
                src[3] = load_u32(&row1[c + 8..c + 16]);
            }
        }

        let mut q = proc_pixel(&src, shift);

        // max_e[i] = max(lep[i], lep[i + 1]) - 1
        let next = _mm256_insert_epi32::<7>(
            _mm256_permutevar8x32_epi32(st.e_val[x], right_shift),
            _mm256_extract_epi32::<0>(st.e_val[x + 1]),
        );
        let max_e = _mm256_sub_epi32(_mm256_max_epi32(next, st.e_val[x]), one());

        // kappa = (rho & (rho - 1)) ? max(1, max_e) : 1
        let single = _mm256_cmpeq_epi32(
            _mm256_and_si256(q.rho, _mm256_sub_epi32(q.rho, one())),
            zero,
        );
        let kappa = _mm256_max_epi32(
            _mm256_and_si256(single, one()),
            _mm256_and_si256(
                _mm256_xor_si256(single, ones),
                _mm256_max_epi32(max_e, one()),
            ),
        );

        // the context of the next quad (for each lane, that of quad
        // i + 1)
        let cq_next = if INITIAL {
            // c_q[i + 1] = (rho[i] >> 1) | (rho[i] & 1)
            _mm256_or_si256(
                _mm256_srli_epi32::<1>(q.rho),
                _mm256_and_si256(q.rho, one()),
            )
        } else {
            // c_q[i + 1] = (lcxp[i + 1] + (lcxp[i + 2] << 2))
            //            | (((rho[i] & 4) >> 1) | ((rho[i] & 8) >> 2))
            let lcxp1 = _mm256_permutevar8x32_epi32(st.cx_val[x], right_shift);
            let lcxp2 = _mm256_insert_epi64::<3>(
                _mm256_permutevar8x32_epi32(lcxp1, right_shift),
                _mm_cvtsi128_si64(_mm256_castsi256_si128(st.cx_val[x + 1])),
            );
            let lcxp1 =
                _mm256_insert_epi32::<7>(lcxp1, _mm256_extract_epi32::<0>(st.cx_val[x + 1]));
            let t = _mm256_add_epi32(lcxp1, _mm256_slli_epi32::<2>(lcxp2));
            let t = _mm256_or_si256(
                t,
                _mm256_srli_epi32::<1>(_mm256_and_si256(q.rho, _mm256_set1_epi32(4))),
            );
            _mm256_or_si256(
                t,
                _mm256_srli_epi32::<2>(_mm256_and_si256(q.rho, _mm256_set1_epi32(8))),
            )
        };
        let cq =
            _mm256_insert_epi32::<0>(_mm256_permutevar8x32_epi32(cq_next, left_shift), st.prev_cq);
        st.prev_cq = _mm256_extract_epi32::<7>(cq_next);

        // update_lep: lep[i] = max(e_q[1] of quad i, e_q[3] of quad i - 1)
        let prev = _mm256_insert_epi32::<0>(
            _mm256_permutevar8x32_epi32(q.e_q[3], left_shift),
            _mm256_extract_epi32::<0>(st.prev_e_val),
        );
        st.prev_e_val = _mm256_insert_epi32::<0>(zero, _mm256_extract_epi32::<7>(q.e_q[3]));
        st.e_val[x] = _mm256_max_epi32(q.e_q[1], prev);

        // update_lcxp: lcxp[i] = (rho[i] & 2) >> 1 | (rho[i - 1] & 8) >> 3
        let prev = _mm256_insert_epi32::<0>(
            _mm256_permutevar8x32_epi32(q.rho, left_shift),
            _mm256_extract_epi32::<0>(st.prev_cx_val),
        );
        st.prev_cx_val = _mm256_insert_epi32::<0>(zero, _mm256_extract_epi32::<7>(q.rho));
        st.cx_val[x] = _mm256_or_si256(
            _mm256_srli_epi32::<3>(_mm256_and_si256(prev, _mm256_set1_epi32(8))),
            _mm256_srli_epi32::<1>(_mm256_and_si256(q.rho, _mm256_set1_epi32(2))),
        );

        // Uq = max(e_qmax, kappa); u_q = Uq - kappa
        let uq = _mm256_max_epi32(kappa, q.e_qmax);
        let u_q = _mm256_sub_epi32(uq, kappa);

        // eps: which samples reach e_qmax, if u_q > 0
        let mut eps = zero;
        for (i, e_q) in q.e_q.iter().enumerate() {
            let hit = _mm256_srli_epi32::<31>(_mm256_cmpeq_epi32(*e_q, q.e_qmax));
            eps = _mm256_or_si256(eps, _mm256_sll_epi32(hit, _mm_cvtsi32_si128(i as i32)));
        }
        let eps = _mm256_and_si256(_mm256_cmpgt_epi32(u_q, zero), eps);

        // tuple = vlc_tbl[(c_q << 8) + (rho << 4) + eps]
        let index = _mm256_add_epi32(
            _mm256_add_epi32(_mm256_slli_epi32::<8>(cq), _mm256_slli_epi32::<4>(q.rho)),
            eps,
        );
        let index = lanes_u32(index);
        let tuples: [u32; 8] = core::array::from_fn(|i| vlc_tbl[index[i] as usize & 0x7FF] as u32);
        let tuple = load_u32(&tuples);

        // the quads of this step that are in the block
        let i_max = if x == n_loop - 1 { 8 - ignore / 2 } else { 8 };

        // MEL
        if INITIAL {
            let need = lanes_u32(_mm256_cmpeq_epi32(cq, zero));
            let bit = lanes_u32(cmpneq_zero(q.rho));
            let u_q_next = _mm256_permutevar8x32_epi32(u_q, right_shift);
            let bit2 = lanes_u32(_mm256_cmpgt_epi32(
                _mm256_min_epi32(u_q, u_q_next),
                _mm256_set1_epi32(2),
            ));
            let need2 = lanes_u32(_mm256_and_si256(
                _mm256_cmpgt_epi32(u_q, zero),
                _mm256_cmpgt_epi32(u_q_next, zero),
            ));
            let mut i = 0;
            while i < i_max {
                if need[i] != 0 {
                    st.mel.encode(bit[i] != 0);
                }
                if i + 1 < i_max && need[i + 1] != 0 {
                    st.mel.encode(bit[i + 1] != 0);
                }
                if need2[i] != 0 {
                    st.mel.encode(bit2[i] != 0);
                }
                i += 2;
            }
        } else {
            let mut mask =
                _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(cq, zero))) as u32;
            mask &= (1u32 << i_max) - 1;
            if mask != 0 {
                let bit = _mm256_movemask_ps(_mm256_castsi256_ps(cmpneq_zero(q.rho))) as u32;
                while mask != 0 {
                    let i = mask.trailing_zeros();
                    st.mel.encode(bit >> i & 1 != 0);
                    mask &= mask - 1;
                }
            }
        }

        // MagSgn
        proc_ms_encode(&mut st.ms, tuple, uq, q.rho, &mut q.s);

        // VLC: the codewords of a pair of quads, then their UVLC codes
        let mut tuple = [0u32; 10];
        tuple[..8].copy_from_slice(&lanes_u32(_mm256_srli_epi32::<4>(load_u32(&tuples))));
        let mut uqs = [0u32; 10];
        uqs[..8].copy_from_slice(&lanes_u32(u_q));
        if i_max & 1 != 0 {
            tuple[i_max] = 0;
            uqs[i_max] = 0;
        }
        let pair = |i: usize| -> (u64, u32) {
            let mut val = (tuple[i] >> 4) as u64;
            let mut size = tuple[i] & 7;
            val |= ((tuple[i + 1] >> 4) as u64) << size;
            size += tuple[i + 1] & 7;
            let entry = uvlc_tbl[(uqs[i] * 33 + uqs[i + 1]) as usize];
            val |= ((entry >> 5) as u64) << size;
            size += entry & 0x1F;
            (val, size)
        };
        let mut i = 0;
        while i + 2 < i_max {
            let (v1, s1) = pair(i);
            let (v2, s2) = pair(i + 2);
            st.vlc.encode(v1 | v2 << s1, s1 + s2);
            i += 4;
        }
        if i < i_max {
            let (v, s) = pair(i);
            st.vlc.encode(v, s);
        }
    }
    Some(())
}

/// Encodes a code block of 32-bit samples as
/// [`encode_codeblock32`](super::encode_codeblock32) does, appending the
/// same bytes to `coded`, or returns `None` (with `coded` possibly
/// extended) where the generic encoder fails or might.
///
/// The caller has checked the arguments: `missing_msbs <= 29`, `width`
/// from 1 to 1024, `height` at least 1, and `buf` holding
/// `(height - 1) * stride + width` samples.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
pub(super) fn encode32(
    buf: &[u32],
    missing_msbs: u32,
    width: u32,
    height: u32,
    stride: u32,
    coded: &mut Vec<u8>,
) -> Option<u32> {
    let width = width as usize;
    let height = height as usize;
    let stride = stride as usize;
    let p = 30 - missing_msbs;
    let shift = _mm_cvtsi32_si128(p as i32);
    let zero = _mm256_setzero_si256();

    let mut st = State {
        mel: Mel::new(),
        vlc: Vlc::new(),
        ms: Ms::new(coded),
        e_val: [zero; 65],
        cx_val: [zero; 65],
        prev_e_val: zero,
        prev_cx_val: zero,
        prev_cq: 0,
    };
    let n_loop = width.div_ceil(16);

    let mut y = 0;
    while y < height {
        st.e_val[n_loop] = st.prev_e_val;
        // lcxp[0] = (rho[0] & 8) >> 3
        st.cx_val[n_loop] =
            _mm256_srli_epi32::<3>(_mm256_and_si256(st.prev_cx_val, _mm256_set1_epi32(8)));
        st.prev_e_val = zero;
        st.prev_cx_val = zero;

        let row0 = &buf[y * stride..y * stride + width];
        let row1 = if y + 1 < height {
            Some(&buf[(y + 1) * stride..(y + 1) * stride + width])
        } else {
            None
        };
        if y == 0 {
            x_loop::<true>(&mut st, row0, row1, shift)?;
        } else {
            x_loop::<false>(&mut st, row0, row1, shift)?;
        }

        // the context of the first quad of the next row pair
        let t =
            _mm256_permutevar8x32_epi32(st.cx_val[0], _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 0));
        let t = _mm256_add_epi32(_mm256_slli_epi32::<2>(t), st.cx_val[0]);
        st.prev_cq = _mm256_extract_epi32::<0>(t);
        y += 2;
    }

    // Where the generic encoder fails: a MagSgn or VLC bit at or past its
    // buffer's end (the positions only grow, and a bit pending is at
    // `pos`, so this also rejects a few blocks that just fit), or a
    // complete MEL byte past it.
    let State { mel, vlc, ms, .. } = &mut st;
    if ms.pos() >= MS_SIZE32 {
        return None;
    }
    ms.terminate()?;
    vlc.drain();
    if vlc.pos >= VLC_SIZE || mel.pos > MEL_SIZE {
        return None;
    }
    terminate_mel_vlc(mel, vlc)?;

    // the output: MagSgn (already there), then MEL, then VLC (reversed)
    let start = ms.start;
    let length = ms.pos() + mel.pos + vlc.pos;
    let num_bytes = mel.pos + vlc.pos;
    let ms_out = &mut *ms.out;
    ms_out.reserve(num_bytes);
    ms_out.extend_from_slice(&mel.buf[..mel.pos]);
    ms_out.extend(vlc.buf[..vlc.pos].iter().rev());

    // the interface locator word
    let out = &mut ms_out[start..];
    out[length - 1] = (num_bytes >> 4) as u8;
    out[length - 2] = (out[length - 2] & 0xF0) | (num_bytes & 0xF) as u8;
    Some(length as u32)
}

#[cfg(test)]
mod tests {
    use super::super::encoder::{encode, encode32_simd, encode_codeblock32};
    use super::*;

    /// splitmix64
    fn mix(z: &mut u64) -> u64 {
        *z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = *z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    /// Samples in the encoder's sign-magnitude form, of one of several
    /// kinds: random over the whole range, all zero, all of the largest
    /// magnitude, mostly zero, small magnitudes near the bit-plane `p`,
    /// runs of zeros and non-zeros, and magnitudes of every size.
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

    /// Encodes with the AVX2 encoder (which must take the block, on a CPU
    /// that runs it) and the generic one; the bytes must be the same.
    fn check(buf: &[u32], missing_msbs: u32, width: u32, height: u32, stride: u32, what: &str) {
        let mut want = vec![0xAB];
        let want_len = encode(buf, missing_msbs, 1, width, height, stride, &mut want).unwrap();
        let mut got = vec![0xAB];
        let Some(got_len) = encode32_simd(buf, missing_msbs, 1, width, height, stride, &mut got)
        else {
            assert!(!crate::simd::active(), "{what}: the AVX2 encoder declined");
            return;
        };
        assert_eq!(got_len, want_len, "{what}");
        assert!(got == want, "{what}: bytes differ");
    }

    #[test]
    fn matches_generic_on_every_block_size() {
        let mut seed = 0x656e_6332u64;
        let mut sizes = Vec::new();
        for lw in 2..=10 {
            for lh in 2..=10 {
                if lw + lh <= 12 {
                    sizes.push((1u32 << lw, 1u32 << lh));
                }
            }
        }
        for w in 1..=70 {
            for h in [1, 2, 3, 4, 5, 9] {
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
        for (n, &(w, h)) in sizes.iter().enumerate() {
            for kind in 0..7 {
                let missing_msbs = match (n as u64 + kind) % 4 {
                    0 => 0,
                    1 => 29,
                    _ => (mix(&mut seed) % 30) as u32,
                };
                let stride = w + (mix(&mut seed) % 3) as u32 * 5;
                let len = ((h - 1) * stride + w) as usize;
                let buf = samples(kind, len, 30 - missing_msbs, &mut seed);
                let what =
                    format!("{w}x{h} stride {stride} kind {kind} missing_msbs {missing_msbs}");
                check(&buf, missing_msbs, w, h, stride, &what);
            }
        }
    }

    #[test]
    fn matches_generic_at_every_missing_msbs() {
        let mut seed = 0x6d73_6273u64;
        for missing_msbs in 0..=29 {
            for kind in 0..7 {
                for (w, h) in [(64, 64), (128, 32), (37, 21)] {
                    let buf = samples(kind, (w * h) as usize, 30 - missing_msbs, &mut seed);
                    let what = format!("{w}x{h} kind {kind} missing_msbs {missing_msbs}");
                    check(&buf, missing_msbs, w, h, w, &what);
                }
            }
        }
    }

    /// Blocks larger than JPEG 2000 allows fill the generic encoder's
    /// buffers; the dispatching encoder must return the same error, with
    /// nothing appended, and encode the blocks that fit alike.
    #[test]
    fn buffer_errors_match_generic() {
        let mut seed = 0x6f76_6572u64;
        let mut codes = std::collections::BTreeSet::new();
        let mut fits = 0;
        for (w, h, kind, missing_msbs) in [
            (1024, 64, 0, 0),
            (1024, 64, 0, 29),
            (1024, 40, 6, 20),
            (1024, 1024, 3, 10),
            (512, 512, 5, 25),
            (1024, 300, 1, 0),
            (1024, 2048, 3, 0),
            (64, 4000, 4, 12),
            (1024, 32, 6, 16),
        ] {
            let buf = samples(kind, w * h, 30 - missing_msbs, &mut seed);
            for h in [h / 4, h / 2, h] {
                let (w32, h32) = (w as u32, h as u32);
                let mut want = vec![1, 2];
                let want =
                    encode(&buf, missing_msbs, 1, w32, h32, w32, &mut want).map(|n| (n, want));
                let mut got = vec![1, 2];
                let got = encode_codeblock32(&buf, missing_msbs, 1, w32, h32, w32, &mut got)
                    .map(|n| (n, got));
                match (got, want) {
                    (Ok(a), Ok(b)) => {
                        assert!(a == b, "{w}x{h}");
                        fits += 1;
                    }
                    (Err(a), Err(b)) => {
                        assert_eq!(a.code(), b.code(), "{w}x{h}");
                        codes.insert(a.code());
                    }
                    (a, b) => panic!("{w}x{h}: {:?} against {:?}", a.map(|x| x.0), b.map(|x| x.0)),
                }
            }
        }
        // MagSgn and VLC buffers full, and blocks that fit
        assert!(
            codes.contains(&0x00020005) && codes.contains(&0x00020002),
            "{codes:x?}"
        );
        assert!(fits >= 4, "{fits}");
    }

    #[test]
    fn lzcnt_is_exact() {
        let mut values: Vec<u32> = vec![0, 1, u32::MAX];
        for b in 0..32 {
            let v = 1u32 << b;
            values.extend([v, v - 1, v + 1, v | (v >> 1), v | (v - 1)]);
        }
        let mut seed = 0x6c7au64;
        for _ in 0..100_000 {
            let r = mix(&mut seed);
            values.push((r as u32) >> (r >> 59));
        }
        values.resize(values.len().div_ceil(8) * 8, 0);
        let Some(got) = lzcnt_lanes(&values) else {
            return;
        };
        for (v, n) in values.iter().zip(got) {
            assert_eq!(n, v.leading_zeros(), "{v:#x}");
        }
    }

    crate::simd::avx2_dispatch! {
        fn lzcnt_lanes(values: &[u32]) -> Option<Vec<u32>> {
            avx2 => Some(
                values
                    .chunks_exact(8)
                    .flat_map(|c| lanes_u32(lzcnt_epi32(load_u32(c))))
                    .collect(),
            ),
            fallback => None,
        }
    }
}
