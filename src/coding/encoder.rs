// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! HT block encoder (port of the generic `ojph_block_encoder.cpp`).
//!
//! The C++ has two copies of the encoder, `ojph_encode_codeblock32` and
//! `ojph_encode_codeblock64`, which differ only in the sample type, the
//! bit-plane offset (`p = 30 - missing_msbs` or `62 - missing_msbs`), the
//! size of the MagSgn buffer, and the UVLC extension bits that only the
//! 64-bit one writes. Here one generic function, [`encode`], follows their
//! common code line by line, and the [`Sample`] trait carries those
//! differences. Like OpenJPH, the encoder writes only the cleanup pass.

use super::readers::has_ff_byte;
use super::tables::{VlcSrc, TBL0, TBL1};
use crate::error::{ojph_error, Result};

// ---------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------

/// Builds a VLC encoding table from a source table (`vlc_init_tables`).
/// Index is `(c_q << 8) + (rho << 4) + eps`; an entry is
/// `(cwd << 8) + (cwd_len << 4) + e_k`. For `eps != 0` the last of the
/// matching entries with the most bits set in `e_k` wins, as in the C++.
const fn build_vlc_enc(src: &[VlcSrc]) -> [u16; 2048] {
    let mut tgt = [0u16; 2048];
    let mut i = 0;
    while i < 2048 {
        let c_q = (i >> 8) as i32;
        let rho = ((i >> 4) & 0xF) as i32;
        let emb = (i & 0xF) as i32;
        if (emb & rho) != emb || (rho == 0 && c_q == 0) {
            tgt[i] = 0;
        } else {
            let mut best: usize = usize::MAX;
            if emb != 0 {
                // u_off = 1
                let mut best_e_k: i32 = -1;
                let mut j = 0;
                while j < src.len() {
                    let e = src[j];
                    if e[0] == c_q && e[1] == rho && e[2] == 1 && (emb & e[3]) == e[4] {
                        // the smallest cwd with the highest number of bits
                        // set in e_k
                        let ones_count = (e[3] as u32).count_ones() as i32;
                        if ones_count >= best_e_k {
                            best = j;
                            best_e_k = ones_count;
                        }
                    }
                    j += 1;
                }
            } else {
                // u_off = 0
                let mut j = 0;
                while j < src.len() {
                    let e = src[j];
                    if e[0] == c_q && e[1] == rho && e[2] == 0 {
                        best = j;
                        break;
                    }
                    j += 1;
                }
            }
            assert!(best != usize::MAX); // assert(best_entry), at compile time
            let e = src[best];
            tgt[i] = ((e[5] << 8) + (e[6] << 4) + e[3]) as u16;
        }
        i += 1;
    }
    tgt
}

/// VLC encoding table for the initial row of quads (`vlc_tbl0`).
static VLC_TBL0: [u16; 2048] = build_vlc_enc(&TBL0);
/// VLC encoding table for the other rows of quads (`vlc_tbl1`).
static VLC_TBL1: [u16; 2048] = build_vlc_enc(&TBL1);

/// One UVLC codeword: prefix, suffix and extension, each with its length
/// (`uvlc_tbl_struct`).
#[derive(Clone, Copy)]
struct Uvlc {
    pre: u8,
    pre_len: u8,
    suf: u8,
    suf_len: u8,
    ext: u8,
    ext_len: u8,
}

const NUM_UVLC_ENTRIES: usize = 75;

/// `uvlc_init_tables`: codes for `u_q` of 0 to 74.
const fn build_uvlc() -> [Uvlc; NUM_UVLC_ENTRIES] {
    const fn e(pre: u8, pre_len: u8, suf: u8, suf_len: u8, ext: u8, ext_len: u8) -> Uvlc {
        Uvlc {
            pre,
            pre_len,
            suf,
            suf_len,
            ext,
            ext_len,
        }
    }
    let mut t = [e(0, 0, 0, 0, 0, 0); NUM_UVLC_ENTRIES];
    t[1] = e(1, 1, 0, 0, 0, 0);
    t[2] = e(2, 2, 0, 0, 0, 0);
    t[3] = e(4, 3, 0, 1, 0, 0);
    t[4] = e(4, 3, 1, 1, 0, 0);
    let mut i = 5;
    while i < 33 {
        t[i] = e(0, 3, (i - 5) as u8, 5, 0, 0);
        i += 1;
    }
    while i < NUM_UVLC_ENTRIES {
        t[i] = e(0, 3, (28 + (i - 33) % 4) as u8, 5, ((i - 33) / 4) as u8, 4);
        i += 1;
    }
    t
}

static UVLC_TBL: [Uvlc; NUM_UVLC_ENTRIES] = build_uvlc();

// ---------------------------------------------------------------------------
// MEL
// ---------------------------------------------------------------------------

/// Size of the MEL part of the C++'s `mel_vlc_buf`.
const MEL_SIZE: usize = 192;
/// Size of the VLC part of the C++'s `mel_vlc_buf` (3072 - 192).
const VLC_SIZE: usize = 3072 - MEL_SIZE;
/// The largest MagSgn buffer (the 64-bit encoder's `ms_size`,
/// `(22528 * 16 + 14) / 15` in the C++).
const MS_CAP: usize = (22528usize * 16).div_ceil(15);

/// `mel_struct`; `buf_size` is `MEL_SIZE`.
struct Mel {
    buf: [u8; MEL_SIZE],
    pos: usize,
    remaining_bits: i32,
    tmp: i32,
    run: i32,
    k: i32,
    threshold: i32,
}

/// MEL exponents.
const MEL_EXP: [i32; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 5];

impl Mel {
    /// `mel_init`.
    fn new() -> Self {
        Mel {
            buf: [0; MEL_SIZE],
            pos: 0,
            remaining_bits: 8,
            tmp: 0,
            run: 0,
            k: 0,
            threshold: 1, // this is 1 << MEL_EXP[k]
        }
    }

    /// `mel_emit_bit`.
    #[inline(always)]
    fn emit_bit(&mut self, v: i32) -> Result<()> {
        self.tmp = (self.tmp << 1) + v;
        self.remaining_bits -= 1;
        if self.remaining_bits == 0 {
            if self.pos >= MEL_SIZE {
                return ojph_error!(0x00020001, "mel encoder's buffer is full");
            }
            self.buf[self.pos] = self.tmp as u8;
            self.pos += 1;
            self.remaining_bits = if self.tmp == 0xFF { 7 } else { 8 };
            self.tmp = 0;
        }
        Ok(())
    }

    /// `mel_encode`.
    #[inline(always)]
    fn encode(&mut self, bit: bool) -> Result<()> {
        if !bit {
            self.run += 1;
            if self.run >= self.threshold {
                self.emit_bit(1)?;
                self.run = 0;
                self.k = (self.k + 1).min(12);
                self.threshold = 1 << MEL_EXP[self.k as usize];
            }
        } else {
            self.emit_bit(0)?;
            let mut t = MEL_EXP[self.k as usize];
            while t > 0 {
                t -= 1;
                self.emit_bit((self.run >> t) & 1)?;
            }
            self.run = 0;
            self.k = (self.k - 1).max(0);
            self.threshold = 1 << MEL_EXP[self.k as usize];
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VLC
// ---------------------------------------------------------------------------

/// How far below `VLC_SIZE` the VLC writer stops deferring (see [`Vlc`]).
/// One call adds at most 32 bits to at most 31 pending, and 63 bits make
/// at most 9 bytes.
const VLC_SLACK: usize = 16;

/// Whether any of the low 3 bytes of `v` is above 0x8F, that is, has its
/// top bit and one of the next three set.
#[inline(always)]
fn has_byte_above_8f_in_3(v: u32) -> bool {
    let top = v & 0x0080_8080;
    let mid = ((v & 0x0070_7070) + 0x0070_7070) & 0x0080_8080;
    top & mid != 0
}

/// `vlc_struct`. The C++ writes backwards from the last byte of its
/// buffer, `buf[-pos]`; here that byte is `buf[VLC_SIZE - 1 - pos]`.
///
/// The C++ fills a byte bit group by bit group and writes it when
/// complete. A byte after one above 0x8F holds 7 bits if those are all
/// ones (it is written as 0x7F), and 8 otherwise. Here, as for [`Ms`],
/// while at least [`VLC_SLACK`] bytes are free the bits gather in `acc`
/// (`used_bits` of them, fewer than 32 between calls) and are written
/// once 32 are pending: 4 bytes at once if the last byte written was not
/// above 0x8F and none of the first 3 is (so all 4 hold 8 bits), else
/// byte by byte by the rule above. Nearer the end, a call writes the
/// complete bytes pending and runs the C++'s loop, so the buffer-full
/// error comes from the same call. `last_greater_than_8f` is that of the
/// last byte written; the C++ clears it once the next byte has 7 bits that
/// are not all ones, which [`Self::flush_bytes`] does too.
struct Vlc {
    buf: [u8; VLC_SIZE],
    pos: usize,
    used_bits: i32,
    acc: u64,
    last_greater_than_8f: bool,
}

impl Vlc {
    /// `vlc_init`.
    fn new() -> Self {
        let mut buf = [0; VLC_SIZE];
        buf[VLC_SIZE - 1] = 0xFF;
        Vlc {
            buf,
            pos: 1, // locations will be all -pos
            used_bits: 4,
            acc: 0xF,
            last_greater_than_8f: true,
        }
    }

    /// Writes the next byte: 7 bits of `acc` if the last byte was above
    /// 0x8F and they are all ones, else 8. `acc` must hold 8 bits, or 7
    /// in the first case.
    #[inline(always)]
    fn emit_byte(&mut self) {
        let seven = self.last_greater_than_8f && self.acc & 0x7F == 0x7F;
        let n = 8 - seven as i32;
        let byte = (self.acc & ((1u64 << n) - 1)) as u8;
        self.buf[VLC_SIZE - 1 - self.pos] = byte;
        self.pos += 1;
        self.acc >>= n;
        self.used_bits -= n;
        self.last_greater_than_8f = byte > 0x8F;
    }

    /// Adds `len` (1 to 32) bits of `cwd`, and writes bytes once 32 or
    /// more are pending.
    #[inline(always)]
    fn put(&mut self, cwd: u64, len: i32) {
        self.acc |= (cwd & ((1u64 << len) - 1)) << self.used_bits;
        self.used_bits += len;
        if self.used_bits >= 32 {
            let low = self.acc as u32;
            if !self.last_greater_than_8f && !has_byte_above_8f_in_3(low) {
                let at = VLC_SIZE - self.pos - 4;
                self.buf[at..at + 4].copy_from_slice(&low.to_be_bytes());
                self.pos += 4;
                self.acc >>= 32;
                self.used_bits -= 32;
                self.last_greater_than_8f = low >> 24 > 0x8F;
            } else {
                while self.used_bits >= 8 {
                    self.emit_byte();
                }
            }
        }
    }

    /// Writes the complete bytes pending in `acc`, leaving the state the
    /// C++'s would have.
    #[inline(always)]
    fn flush_bytes(&mut self) {
        while self.used_bits >= 8
            || (self.used_bits == 7 && self.last_greater_than_8f && self.acc & 0x7F == 0x7F)
        {
            self.emit_byte();
        }
        if self.used_bits == 7 && self.last_greater_than_8f {
            // 7 bits that are not all ones: the byte takes an 8th
            self.last_greater_than_8f = false;
        }
    }

    /// `vlc_encode`.
    #[inline(always)]
    fn encode(&mut self, cwd: i32, cwd_len: i32) -> Result<()> {
        if cwd_len <= 0 {
            return Ok(());
        }
        if cwd_len > 32 || self.pos + VLC_SLACK > VLC_SIZE {
            return self.encode_near_end(cwd, cwd_len);
        }
        self.put(cwd as u32 as u64, cwd_len);
        Ok(())
    }

    /// `vlc_encode` as the C++ writes it, for the end of the buffer.
    #[inline(never)]
    fn encode_near_end(&mut self, mut cwd: i32, mut cwd_len: i32) -> Result<()> {
        self.flush_bytes();
        while cwd_len > 0 {
            if self.pos >= VLC_SIZE {
                return ojph_error!(0x00020002, "vlc encoder's buffer is full");
            }
            let mut avail_bits = 8 - self.last_greater_than_8f as i32 - self.used_bits;
            let t = avail_bits.min(cwd_len);
            self.acc |= ((cwd & ((1 << t) - 1)) as u64) << self.used_bits;
            self.used_bits += t;
            avail_bits -= t;
            cwd_len -= t;
            cwd >>= t;
            if avail_bits == 0 {
                if self.last_greater_than_8f && self.acc != 0x7F {
                    self.last_greater_than_8f = false;
                    continue; // one empty bit remaining
                }
                self.buf[VLC_SIZE - 1 - self.pos] = self.acc as u8;
                self.pos += 1;
                self.last_greater_than_8f = self.acc > 0x8F;
                self.acc = 0;
                self.used_bits = 0;
            }
        }
        Ok(())
    }

    /// Encodes a UVLC table entry's prefix, suffix or extension.
    #[inline(always)]
    fn encode_u8(&mut self, cwd: u8, cwd_len: u8) -> Result<()> {
        self.encode(cwd as i32, cwd_len as i32)
    }
}

/// `terminate_mel_vlc`.
fn terminate_mel_vlc(mel: &mut Mel, vlc: &mut Vlc) -> Result<()> {
    if mel.run > 0 {
        mel.emit_bit(1)?;
    }

    vlc.flush_bytes();
    let vlc_tmp = vlc.acc as i32; // at most 7 bits
    mel.tmp <<= mel.remaining_bits;
    let mel_mask = (0xFF << mel.remaining_bits) & 0xFF;
    let vlc_mask = 0xFF >> (8 - vlc.used_bits);
    if (mel_mask | vlc_mask) == 0 {
        return Ok(()); // last mel byte cannot be 0xFF, since then
                       // mel.remaining_bits would be < 8
    }
    if mel.pos >= MEL_SIZE {
        return ojph_error!(0x00020003, "mel encoder's buffer is full");
    }
    let fuse = mel.tmp | vlc_tmp;
    if (((fuse ^ mel.tmp) & mel_mask) | ((fuse ^ vlc_tmp) & vlc_mask)) == 0
        && fuse != 0xFF
        && vlc.pos > 1
    {
        mel.buf[mel.pos] = fuse as u8;
        mel.pos += 1;
    } else {
        if vlc.pos >= VLC_SIZE {
            return ojph_error!(0x00020004, "vlc encoder's buffer is full");
        }
        mel.buf[mel.pos] = mel.tmp as u8; // mel.tmp cannot be 0xFF
        mel.pos += 1;
        vlc.buf[VLC_SIZE - 1 - vlc.pos] = vlc_tmp as u8;
        vlc.pos += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MagSgn
// ---------------------------------------------------------------------------

/// How far below `buf_size` the MagSgn writer stops deferring (see
/// [`Ms`]). One call adds at most 64 bits to at most 31 pending, and 95
/// bits make at most 13 bytes, so with 16 bytes free no call can reach
/// the end of the buffer.
const MS_SLACK: usize = 16;

/// `ms_struct`; `buf_size` is the encoder's `ms_size`, at most `MS_CAP`.
///
/// The C++ writes a byte as soon as it is complete. Here, while at least
/// [`MS_SLACK`] bytes of the buffer are free, the bits gather in `acc`
/// (`used_bits` of them, fewer than 32 between calls) and go out 32 at
/// a time: as 4 bytes at once when the writer is not stuffing
/// (`max_bits == 8`) and none of the 4 is 0xFF, else byte by byte by the
/// C++'s rule (a byte after 0xFF holds 7 bits). That gives the C++'s
/// bytes, and no call can fill the buffer. Nearer the end, every call
/// first writes the complete bytes still pending and then runs the C++'s
/// loop, so the buffer-full error comes from the same call as in the C++.
struct Ms {
    buf: [u8; MS_CAP],
    pos: usize,
    buf_size: usize,
    max_bits: i32,
    used_bits: i32,
    acc: u64,
}

impl Ms {
    /// `ms_init`.
    fn new(buf_size: usize) -> Self {
        Ms {
            buf: [0; MS_CAP],
            pos: 0,
            buf_size,
            max_bits: 8,
            used_bits: 0,
            acc: 0,
        }
    }

    /// Writes the next byte, `max_bits` bits of `acc`, which must hold
    /// them.
    #[inline(always)]
    fn emit_byte(&mut self) {
        let byte = (self.acc & ((1u64 << self.max_bits) - 1)) as u8;
        self.buf[self.pos] = byte;
        self.pos += 1;
        self.acc >>= self.max_bits;
        self.used_bits -= self.max_bits;
        self.max_bits = if byte == 0xFF { 7 } else { 8 };
    }

    /// Adds `len` (at most 32) bits of `cwd`, which has no others, and
    /// writes bytes while 32 bits or more are pending.
    #[inline(always)]
    fn put(&mut self, cwd: u64, len: i32) {
        self.acc |= cwd << self.used_bits;
        self.used_bits += len;
        if self.used_bits >= 32 {
            let low = self.acc as u32;
            if self.max_bits == 8 && !has_ff_byte(low) {
                self.buf[self.pos..self.pos + 4].copy_from_slice(&low.to_le_bytes());
                self.pos += 4;
                self.acc >>= 32;
                self.used_bits -= 32;
            } else {
                while self.used_bits >= 32 {
                    self.emit_byte();
                }
            }
        }
    }

    /// Writes the complete bytes pending in `acc`, leaving the state the
    /// C++'s would have.
    #[inline(always)]
    fn flush_bytes(&mut self) {
        while self.used_bits >= self.max_bits {
            self.emit_byte();
        }
    }

    /// `ms_encode` and `ms_encode64` (identical but for the type of
    /// `cwd`; `cwd` here holds either). `cwd` has no bits above
    /// `cwd_len`, which is below 64.
    #[inline(always)]
    fn encode(&mut self, cwd: u64, cwd_len: i32) -> Result<()> {
        debug_assert!(cwd_len < 64 && cwd >> cwd_len.max(0) == 0);
        if cwd_len <= 0 {
            return Ok(());
        }
        if self.pos + MS_SLACK > self.buf_size {
            return self.encode_near_end(cwd, cwd_len);
        }
        if cwd_len > 32 {
            self.put(cwd & 0xFFFF_FFFF, 32);
            self.put(cwd >> 32, cwd_len - 32);
        } else {
            self.put(cwd, cwd_len);
        }
        Ok(())
    }

    /// `ms_encode` as the C++ writes it, for the end of the buffer.
    #[inline(never)]
    fn encode_near_end(&mut self, mut cwd: u64, mut cwd_len: i32) -> Result<()> {
        self.flush_bytes();
        while cwd_len > 0 {
            if self.pos >= self.buf_size {
                return ojph_error!(0x00020005, "magnitude sign encoder's buffer is full");
            }
            let t = (self.max_bits - self.used_bits).min(cwd_len);
            self.acc |= (cwd & ((1u64 << t) - 1)) << self.used_bits;
            self.used_bits += t;
            cwd >>= t;
            cwd_len -= t;
            if self.used_bits >= self.max_bits {
                self.emit_byte();
            }
        }
        Ok(())
    }

    /// `ms_terminate`.
    fn terminate(&mut self) -> Result<()> {
        self.flush_bytes();
        if self.used_bits != 0 {
            let t = self.max_bits - self.used_bits; // unused bits
            let tmp = self.acc as u32 | (0xFF & ((1u32 << t) - 1)) << self.used_bits;
            self.used_bits += t;
            if tmp != 0xFF {
                if self.pos >= self.buf_size {
                    return ojph_error!(0x00020006, "magnitude sign encoder's buffer is full");
                }
                self.buf[self.pos] = tmp as u8;
                self.pos += 1;
            }
        } else if self.max_bits == 7 {
            // a byte of 0xFF was the last one written
            self.pos -= 1;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// the encoder
// ---------------------------------------------------------------------------

/// What differs between `ojph_encode_codeblock32` and `64`.
pub(crate) trait Sample: Copy {
    const ZERO: Self;
    /// `p` for `missing_msbs` = 0: 30 or 62.
    const P0: u32;
    /// The largest `missing_msbs` accepted (see [`encode`]).
    const MAX_MISSING_MSBS: u32;
    /// `ms_size`, the capacity of the MagSgn buffer.
    const MS_SIZE: usize;
    /// Whether UVLC extension bits are written (64-bit only).
    const UVLC_EXT: bool;

    /// The block of code repeated for every sample in the C++:
    ///
    /// ```c++
    /// val = t + t;   //multiply by 2 and get rid of sign
    /// val >>= p;     // 2 \mu_p + x
    /// val &= ~1u;    // 2 \mu_p
    /// if (val) {
    ///   e_q[i] = 32 - (int)count_leading_zeros(--val); //2\mu_p - 1
    ///   s[i] = --val + (t >> 31); //v_n = 2(\mu_p-1) + s_n
    /// }
    /// ```
    ///
    /// returning `Some((e_q, s))` for a significant sample.
    fn quantise(self, p: u32) -> Option<(i32, u64)>;
}

impl Sample for u32 {
    const ZERO: Self = 0;
    const P0: u32 = 30;
    const MAX_MISSING_MSBS: u32 = 29;
    const MS_SIZE: usize = (16384usize * 16).div_ceil(15); // (16384*16+14)/15
    const UVLC_EXT: bool = false;

    #[inline(always)]
    fn quantise(self, p: u32) -> Option<(i32, u64)> {
        let t = self;
        let mut val = t.wrapping_add(t);
        val >>= p;
        val &= !1u32;
        if val != 0 {
            val -= 1;
            let e_q = 32 - val.leading_zeros() as i32;
            val -= 1;
            Some((e_q, (val + (t >> 31)) as u64))
        } else {
            None
        }
    }
}

impl Sample for u64 {
    const ZERO: Self = 0;
    const P0: u32 = 62;
    const MAX_MISSING_MSBS: u32 = 61;
    // 38 bits/sample + 1 color + 4 wavelet = 43 bits per sample, for 4096
    // samples, rounded up to 22528 bytes, times 16/15 for bit stuffing
    const MS_SIZE: usize = MS_CAP;
    const UVLC_EXT: bool = true;

    #[inline(always)]
    fn quantise(self, p: u32) -> Option<(i32, u64)> {
        let t = self;
        let mut val = t.wrapping_add(t);
        val >>= p;
        val &= !1u64;
        if val != 0 {
            val -= 1;
            let e_q = 64 - val.leading_zeros() as i32;
            val -= 1;
            Some((e_q, val + (t >> 63)))
        } else {
            None
        }
    }
}

/// The per-pair-of-quads state of the C++ (`e_qmax`, `e_q`, `rho`, `s`).
/// Samples are numbered as in the C++: 0 to 3 in the first quad (column
/// by column, top then bottom), 4 to 7 in the second.
#[derive(Default)]
struct Quads {
    e_qmax: [i32; 2],
    e_q: [i32; 8],
    rho: [i32; 2],
    s: [u64; 8],
}

impl Quads {
    /// Adds sample `n` (0 to 7). The C++ assigns `rho` and `e_qmax` for the
    /// first sample of a quad and accumulates for the others; both start
    /// at 0, so accumulating throughout is the same.
    #[inline(always)]
    fn add<T: Sample>(&mut self, n: usize, t: T, p: u32) {
        if let Some((e_q, s)) = t.quantise(p) {
            let q = n >> 2;
            self.rho[q] += 1 << (n & 3);
            self.e_q[n] = e_q;
            self.e_qmax[q] = self.e_qmax[q].max(e_q);
            self.s[n] = s;
        }
    }

    /// `eps` of quad `q` for `u_q`.
    #[inline(always)]
    fn eps(&self, q: usize, u_q: i32) -> i32 {
        let mut eps = 0;
        if u_q > 0 {
            let e = &self.e_q[4 * q..4 * q + 4];
            let m = self.e_qmax[q];
            eps |= (e[0] == m) as i32;
            eps |= ((e[1] == m) as i32) << 1;
            eps |= ((e[2] == m) as i32) << 2;
            eps |= ((e[3] == m) as i32) << 3;
        }
        eps
    }

    /// Writes the VLC codeword of quad `q` (and a MEL symbol when its
    /// context is 0) and the MagSgn bits of its four samples; returns the
    /// table entry (`tuple0` / `tuple1`).
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn code_quad(
        &self,
        q: usize,
        tbl: &[u16; 2048],
        c_q: i32,
        eps: i32,
        u_cap: i32,
        vlc: &mut Vlc,
        mel: &mut Mel,
        ms: &mut Ms,
    ) -> Result<()> {
        let rho = self.rho[q];
        let tuple = tbl[((c_q << 8) + (rho << 4) + eps) as usize] as i32;
        vlc.encode(tuple >> 8, (tuple >> 4) & 7)?;

        if c_q == 0 {
            mel.encode(rho != 0)?;
        }

        for i in 0..4 {
            let m = if rho & (1 << i) != 0 {
                u_cap - ((tuple >> i) & 1)
            } else {
                0
            };
            ms.encode(self.s[4 * q + i] & ((1u64 << m) - 1), m)?;
        }
        Ok(())
    }
}

/// Validates the arguments the C++ does not check, then encodes.
///
/// * `num_passes` must be 1 (`assert(num_passes == 1)` in the C++);
/// * `missing_msbs <= T::MAX_MISSING_MSBS`: above that the C++ shifts by
///   32 (or 64) or more, which is undefined;
/// * `width <= 1024`, the most for which the C++'s `e_val[513]` and
///   `cx_val[513]` are (nearly, see below) large enough;
/// * `buf` holds every sample the C++ reads.
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode<T: Sample>(
    buf: &[T],
    missing_msbs: u32,
    num_passes: u32,
    width: u32,
    height: u32,
    stride: u32,
    coded: &mut Vec<u8>,
) -> Result<u32> {
    if num_passes != 1 {
        return ojph_error!(
            0x00020010,
            "the block encoder writes only the cleanup pass (num_passes {} != 1)",
            num_passes
        );
    }
    if missing_msbs > T::MAX_MISSING_MSBS {
        return ojph_error!(
            0x00020011,
            "missing_msbs {} is more than the block encoder supports ({})",
            missing_msbs,
            T::MAX_MISSING_MSBS
        );
    }
    if width > 1024 {
        return ojph_error!(0x00020012, "code block width {} is more than 1024", width);
    }
    // The C++ reads row 0 whatever the height, then rows 1 to height - 1.
    let needed = if width == 0 {
        0
    } else {
        (height.max(1) as u64 - 1) * stride as u64 + width as u64
    };
    if needed > buf.len() as u64 {
        return ojph_error!(
            0x00020013,
            "code block samples ({} needed) do not fit the buffer ({})",
            needed,
            buf.len()
        );
    }

    let mut mel = Mel::new();
    let mut vlc = Vlc::new();
    let mut ms = Ms::new(T::MS_SIZE);

    let p = T::P0 - missing_msbs;
    let stride = stride as usize;

    // e_val: E values for a line (these are the highest set bit)
    // cx_val: is the context values
    // Each byte stores the info for the 2 sample. For E, it is maximum
    // of the two samples, while for cx, it is the OR of these two samples.
    // The maximum is between the pixel at the bottom left of one quad
    // and the bottom right of the earlier quad. The same is true for cx.
    // The C++ has 513 entries, and for a width of 1023 or 1024 indexes
    // entry 513 (one past its end): it writes 0 to e_val[513] after the
    // initial row and reads e_val[513] and cx_val[513] at the end of the
    // later rows, into values it then never uses. One more entry here
    // makes those accesses defined without changing the output.
    let mut e_val = [0u8; 514];
    let mut cx_val = [0u8; 514];
    let mut lep = 0usize;
    e_val[lep] = 0;
    let mut lcxp = 0usize;
    cx_val[lcxp] = 0;

    // initial row of quads
    let mut q = Quads::default();
    let mut c_q0: i32 = 0;
    let mut x = 0u32;
    while x < width {
        // prepare two quads
        let mut sp = x as usize;
        q.add(0, buf[sp], p);
        q.add(
            1,
            if height > 1 {
                buf[sp + stride]
            } else {
                T::ZERO
            },
            p,
        );
        sp += 1;

        if x + 1 < width {
            q.add(2, buf[sp], p);
            q.add(
                3,
                if height > 1 {
                    buf[sp + stride]
                } else {
                    T::ZERO
                },
                p,
            );
            sp += 1;
        }

        let uq0 = q.e_qmax[0].max(1); // kappa_q = 1
        let u_q0 = uq0 - 1; // kappa_q = 1
        let mut u_q1 = 0;

        let eps0 = q.eps(0, u_q0);
        e_val[lep] = e_val[lep].max(q.e_q[1] as u8);
        lep += 1;
        e_val[lep] = q.e_q[3] as u8;
        cx_val[lcxp] |= ((q.rho[0] & 2) >> 1) as u8;
        lcxp += 1;
        cx_val[lcxp] = ((q.rho[0] & 8) >> 3) as u8;

        q.code_quad(0, &VLC_TBL0, c_q0, eps0, uq0, &mut vlc, &mut mel, &mut ms)?;

        if x + 2 < width {
            q.add(4, buf[sp], p);
            q.add(
                5,
                if height > 1 {
                    buf[sp + stride]
                } else {
                    T::ZERO
                },
                p,
            );
            sp += 1;

            if x + 3 < width {
                q.add(6, buf[sp], p);
                q.add(
                    7,
                    if height > 1 {
                        buf[sp + stride]
                    } else {
                        T::ZERO
                    },
                    p,
                );
            }

            let c_q1 = (q.rho[0] >> 1) | (q.rho[0] & 1);
            let uq1 = q.e_qmax[1].max(1); // kappa_q = 1
            u_q1 = uq1 - 1; // kappa_q = 1

            let eps1 = q.eps(1, u_q1);
            e_val[lep] = e_val[lep].max(q.e_q[5] as u8);
            lep += 1;
            e_val[lep] = q.e_q[7] as u8;
            // the C++ has `lcxp[0] |= (ui8)(lcxp[0] | ...)` here, which is
            // the same as the plain `|=` of the first quad
            cx_val[lcxp] |= ((q.rho[1] & 2) >> 1) as u8;
            lcxp += 1;
            cx_val[lcxp] = ((q.rho[1] & 8) >> 3) as u8;

            q.code_quad(1, &VLC_TBL0, c_q1, eps1, uq1, &mut vlc, &mut mel, &mut ms)?;
        }

        if u_q0 > 0 && u_q1 > 0 {
            mel.encode(u_q0.min(u_q1) > 2)?;
        }

        if u_q0 > 2 && u_q1 > 2 {
            let (a, b) = (UVLC_TBL[(u_q0 - 2) as usize], UVLC_TBL[(u_q1 - 2) as usize]);
            vlc.encode_u8(a.pre, a.pre_len)?;
            vlc.encode_u8(b.pre, b.pre_len)?;
            vlc.encode_u8(a.suf, a.suf_len)?;
            vlc.encode_u8(b.suf, b.suf_len)?;
            if T::UVLC_EXT {
                vlc.encode_u8(a.ext, a.ext_len)?;
                vlc.encode_u8(b.ext, b.ext_len)?;
            }
        } else if u_q0 > 2 && u_q1 > 0 {
            let a = UVLC_TBL[u_q0 as usize];
            vlc.encode_u8(a.pre, a.pre_len)?;
            vlc.encode(u_q1 - 1, 1)?;
            vlc.encode_u8(a.suf, a.suf_len)?;
            if T::UVLC_EXT {
                vlc.encode_u8(a.ext, a.ext_len)?;
            }
        } else {
            let (a, b) = (UVLC_TBL[u_q0 as usize], UVLC_TBL[u_q1 as usize]);
            vlc.encode_u8(a.pre, a.pre_len)?;
            vlc.encode_u8(b.pre, b.pre_len)?;
            vlc.encode_u8(a.suf, a.suf_len)?;
            vlc.encode_u8(b.suf, b.suf_len)?;
            if T::UVLC_EXT {
                vlc.encode_u8(a.ext, a.ext_len)?;
                vlc.encode_u8(b.ext, b.ext_len)?;
            }
        }

        // prepare for next iteration
        c_q0 = (q.rho[1] >> 1) | (q.rho[1] & 1);
        q = Quads::default();
        x += 4;
    }

    e_val[lep + 1] = 0;

    // With a width of 0 the later rows code nothing, so they are skipped:
    // the C++ steps through them all, and for a height of u32::MAX never
    // stops (`y += 2` wraps). With any other width every row adds VLC or
    // MEL bits, so a buffer fills long before `y` could overflow.
    let mut y = 2u32;
    while width > 0 && y < height {
        lep = 0;
        let mut max_e = e_val[lep].max(e_val[lep + 1]) as i32 - 1;
        e_val[lep] = 0;
        lcxp = 0;
        c_q0 = cx_val[lcxp] as i32 + ((cx_val[lcxp + 1] as i32) << 2);
        cx_val[lcxp] = 0;

        let below = y + 1 < height;
        let row = y as usize * stride;
        let mut x = 0u32;
        while x < width {
            // prepare two quads
            let mut sp = row + x as usize;
            q.add(0, buf[sp], p);
            q.add(1, if below { buf[sp + stride] } else { T::ZERO }, p);
            sp += 1;

            if x + 1 < width {
                q.add(2, buf[sp], p);
                q.add(3, if below { buf[sp + stride] } else { T::ZERO }, p);
                sp += 1;
            }

            let rho0 = q.rho[0];
            let mut kappa = if rho0 & (rho0 - 1) != 0 {
                max_e.max(1)
            } else {
                1
            };
            let uq0 = q.e_qmax[0].max(kappa);
            let u_q0 = uq0 - kappa;
            let mut u_q1 = 0;

            let eps0 = q.eps(0, u_q0);
            e_val[lep] = e_val[lep].max(q.e_q[1] as u8);
            lep += 1;
            max_e = e_val[lep].max(e_val[lep + 1]) as i32 - 1;
            e_val[lep] = q.e_q[3] as u8;
            cx_val[lcxp] |= ((rho0 & 2) >> 1) as u8;
            lcxp += 1;
            let mut c_q1 = cx_val[lcxp] as i32 + ((cx_val[lcxp + 1] as i32) << 2);
            cx_val[lcxp] = ((rho0 & 8) >> 3) as u8;

            q.code_quad(0, &VLC_TBL1, c_q0, eps0, uq0, &mut vlc, &mut mel, &mut ms)?;

            if x + 2 < width {
                q.add(4, buf[sp], p);
                q.add(5, if below { buf[sp + stride] } else { T::ZERO }, p);
                sp += 1;

                if x + 3 < width {
                    q.add(6, buf[sp], p);
                    q.add(7, if below { buf[sp + stride] } else { T::ZERO }, p);
                }

                let rho1 = q.rho[1];
                kappa = if rho1 & (rho1 - 1) != 0 {
                    max_e.max(1)
                } else {
                    1
                };
                c_q1 |= ((rho0 & 4) >> 1) | ((rho0 & 8) >> 2);
                let uq1 = q.e_qmax[1].max(kappa);
                u_q1 = uq1 - kappa;

                let eps1 = q.eps(1, u_q1);
                e_val[lep] = e_val[lep].max(q.e_q[5] as u8);
                lep += 1;
                max_e = e_val[lep].max(e_val[lep + 1]) as i32 - 1;
                e_val[lep] = q.e_q[7] as u8;
                cx_val[lcxp] |= ((rho1 & 2) >> 1) as u8;
                lcxp += 1;
                c_q0 = cx_val[lcxp] as i32 + ((cx_val[lcxp + 1] as i32) << 2);
                cx_val[lcxp] = ((rho1 & 8) >> 3) as u8;

                q.code_quad(1, &VLC_TBL1, c_q1, eps1, uq1, &mut vlc, &mut mel, &mut ms)?;
            }

            let (a, b) = (UVLC_TBL[u_q0 as usize], UVLC_TBL[u_q1 as usize]);
            vlc.encode_u8(a.pre, a.pre_len)?;
            vlc.encode_u8(b.pre, b.pre_len)?;
            vlc.encode_u8(a.suf, a.suf_len)?;
            vlc.encode_u8(b.suf, b.suf_len)?;
            if T::UVLC_EXT {
                vlc.encode_u8(a.ext, a.ext_len)?;
                vlc.encode_u8(b.ext, b.ext_len)?;
            }

            // prepare for next iteration
            c_q0 |= ((q.rho[1] & 4) >> 1) | ((q.rho[1] & 8) >> 2);
            q = Quads::default();
            x += 4;
        }
        y += 2;
    }

    terminate_mel_vlc(&mut mel, &mut vlc)?;
    ms.terminate()?;

    // copy to the output: MagSgn, then MEL, then VLC
    let length = ms.pos + mel.pos + vlc.pos;
    let start = coded.len();
    coded.reserve(length);
    coded.extend_from_slice(&ms.buf[..ms.pos]);
    coded.extend_from_slice(&mel.buf[..mel.pos]);
    coded.extend_from_slice(&vlc.buf[VLC_SIZE - vlc.pos..]);

    // put in the interface locator word; `length >= 2`: VLC holds its
    // initial 0xFF byte, and the termination either finds a complete VLC
    // byte already written or writes one more byte
    let num_bytes = mel.pos + vlc.pos;
    let out = &mut coded[start..];
    out[length - 1] = (num_bytes >> 4) as u8;
    out[length - 2] = (out[length - 2] & 0xF0) | (num_bytes & 0xF) as u8;

    Ok(length as u32)
}

/// Encodes one code block of 32-bit samples with the HT cleanup pass; port
/// of OpenJPH's generic `ojph_encode_codeblock32`, producing the same bytes.
///
/// * `buf` holds the samples, sample `(x, y)` at `y * stride + x`, in the
///   form OpenJPH's encoder takes, as written by
///   [`rev_tx_to_cb32`](crate::transform::codeblock_fun::rev_tx_to_cb32)
///   (or `irv_tx_to_cb32`): sign-magnitude, the sign in bit 31 and the
///   magnitude in bits 30 to 0. With `p = 30 - missing_msbs`, the magnitude
///   bits from `p` up are coded and the bits below `p` are ignored.
///   `buf` must hold `(height - 1) * stride + width` samples (`width` for a
///   `height` of 0, for which the C++ still reads row 0); the encoder reads
///   no others, so `stride` may also be less than `width`.
/// * `missing_msbs` is 0 to 29. OpenJPH's caller, `codeblock::encode`,
///   passes `K_max - 1`, which makes `p` the shift `rev_tx_to_cb32`
///   applied; it also never calls the encoder for a block whose samples
///   are all zero (the codestream then signals an empty block).
/// * `num_passes` must be 1: OpenJPH writes only the cleanup pass.
/// * `width` is at most 1024; any `height` is accepted, but the encoder's
///   buffers, of the C++'s fixed sizes, only suffice for blocks of up to
///   4096 samples that JPEG 2000 allows.
///
/// On success the coded bytes (the cleanup segment, `lengths[0]` in the
/// C++) are appended to `coded` and their number is returned; it is at
/// least 2. The C++ writes them to a `coded_lists` buffer it takes from a
/// `mem_elastic_allocator`; the caller here owns the `Vec`.
///
/// Errors, with nothing appended to `coded`:
///
/// * the C++'s own errors, with its codes, when one of its fixed-size
///   buffers would overflow: `0x00020001` and `0x00020003` (MEL, 192
///   bytes), `0x00020002` and `0x00020004` (VLC, 2880 bytes), `0x00020005`
///   and `0x00020006` (MagSgn, 17477 bytes);
/// * for arguments on which the C++ would assert or have undefined
///   behaviour, codes OpenJPH does not have: `0x00020010` for a
///   `num_passes` other than 1, `0x00020011` for `missing_msbs` above 29,
///   `0x00020012` for a `width` above 1024 and `0x00020013` for a `buf`
///   too small.
///
/// The scratch buffers, about 27 kB, are on the stack. (The C++'s 32-bit
/// encoder has a smaller MagSgn buffer, 21 kB in all; the port gives both
/// widths the 64-bit size and limits the 32-bit one to 17477 bytes.)
#[allow(clippy::too_many_arguments)]
pub fn encode_codeblock32(
    buf: &[u32],
    missing_msbs: u32,
    num_passes: u32,
    width: u32,
    height: u32,
    stride: u32,
    coded: &mut Vec<u8>,
) -> Result<u32> {
    encode(buf, missing_msbs, num_passes, width, height, stride, coded)
}

/// Encodes one code block of 64-bit samples with the HT cleanup pass; port
/// of OpenJPH's generic `ojph_encode_codeblock64`, producing the same bytes.
///
/// As [`encode_codeblock32`], except that:
///
/// * samples are 64-bit, as written by
///   [`rev_tx_to_cb64`](crate::transform::codeblock_fun::rev_tx_to_cb64):
///   the sign in bit 63, the magnitude in bits 62 to 0, and
///   `p = 62 - missing_msbs`. OpenJPH uses this encoder for components
///   whose `K_max` is more than 30;
/// * `missing_msbs` is 0 to 61 (error `0x00020011` above);
/// * the MagSgn buffer has 24030 bytes. From a `missing_msbs` of about 43
///   up, a full 4096-sample block of the largest magnitudes no longer
///   fits, and the encoder returns error `0x00020005` where the C++ raises
///   it;
/// * quads with `u_q` above 32 get UVLC extension bits, which the 32-bit
///   encoder never needs.
///
/// A block with `missing_msbs` above 55 is encoded as OpenJPH encodes it,
/// but OpenJPH's 64-bit decoder, and so [`decode_codeblock64`], cannot
/// always decode it: they read at most 57 MagSgn bits per sample, and a
/// sample can take `missing_msbs + 2`. Codestreams stay far below that:
/// OpenJPH passes `K_max - 1`, which is at most 34 in the test vectors
/// encoded from 32-bit images (5 levels, colour transform).
///
/// [`decode_codeblock64`]: super::decode_codeblock64
///
/// The scratch buffers (about 27 kB, as in the C++) are on the stack.
#[allow(clippy::too_many_arguments)]
pub fn encode_codeblock64(
    buf: &[u64],
    missing_msbs: u32,
    num_passes: u32,
    width: u32,
    height: u32,
    stride: u32,
    coded: &mut Vec<u8>,
) -> Result<u32> {
    encode(buf, missing_msbs, num_passes, width, height, stride, coded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MagSgn writer as the C++ has it, one byte at a time.
    struct RefMs {
        buf: Vec<u8>,
        buf_size: usize,
        max_bits: i32,
        used_bits: i32,
        tmp: u32,
    }

    impl RefMs {
        fn encode(&mut self, mut cwd: u64, mut cwd_len: i32) -> std::result::Result<(), u32> {
            while cwd_len > 0 {
                if self.buf.len() >= self.buf_size {
                    return Err(0x00020005);
                }
                let t = (self.max_bits - self.used_bits).min(cwd_len);
                self.tmp |= ((cwd & ((1u64 << t) - 1)) << self.used_bits) as u32;
                self.used_bits += t;
                cwd >>= t;
                cwd_len -= t;
                if self.used_bits >= self.max_bits {
                    self.buf.push(self.tmp as u8);
                    self.max_bits = if self.tmp == 0xFF { 7 } else { 8 };
                    self.tmp = 0;
                    self.used_bits = 0;
                }
            }
            Ok(())
        }

        fn terminate(&mut self) -> std::result::Result<(), u32> {
            if self.used_bits != 0 {
                let t = self.max_bits - self.used_bits;
                self.tmp |= (0xFF & ((1u32 << t) - 1)) << self.used_bits;
                if self.tmp != 0xFF {
                    if self.buf.len() >= self.buf_size {
                        return Err(0x00020006);
                    }
                    self.buf.push(self.tmp as u8);
                }
            } else if self.max_bits == 7 {
                self.buf.pop();
            }
            Ok(())
        }
    }

    /// splitmix64
    fn mix(z: &mut u64) -> u64 {
        *z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = *z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    /// `Ms` against `RefMs` on random codewords, many of them all ones
    /// (for 0xFF bytes, and so stuffing), into buffers of 0 to 80 bytes
    /// and full-size ones: every call must succeed or fail alike, and the
    /// bytes written must be the same.
    #[test]
    fn ms_matches_byte_writer() {
        let mut seed = 0x6d73u64;
        for round in 0..20_000usize {
            let buf_size = if round % 50 == 0 { MS_CAP } else { round % 81 };
            let mut ms = Ms::new(buf_size);
            let mut rm = RefMs {
                buf: Vec::new(),
                buf_size,
                max_bits: 8,
                used_bits: 0,
                tmp: 0,
            };
            let ones = mix(&mut seed) % 4;
            let calls = mix(&mut seed) % 400;
            let mut failed = false;
            for _ in 0..calls {
                let r = mix(&mut seed);
                let len = match r % 4 {
                    0 => (r >> 8) % 64,
                    1 => (r >> 8) % 9,
                    _ => (r >> 8) % 33,
                } as i32;
                let mask = if len == 0 { 0 } else { u64::MAX >> (64 - len) };
                let cwd = if (r >> 16) % 4 < ones {
                    mask
                } else {
                    mix(&mut seed) & mask
                };
                let a = ms.encode(cwd, len).map_err(|e| e.code());
                let b = rm.encode(cwd, len);
                assert_eq!(a, b, "round {round}");
                if a.is_err() {
                    failed = true;
                    break;
                }
            }
            if !failed {
                let a = ms.terminate().map_err(|e| e.code());
                assert_eq!(a, rm.terminate(), "round {round}");
                assert_eq!(&ms.buf[..ms.pos], &rm.buf[..], "round {round}");
            }
        }
    }

    /// The VLC writer as the C++ has it, one byte at a time (in writing
    /// order; the real buffer is filled backwards).
    struct RefVlc {
        buf: Vec<u8>,
        used_bits: i32,
        tmp: i32,
        last_greater_than_8f: bool,
    }

    impl RefVlc {
        fn encode(&mut self, mut cwd: i32, mut cwd_len: i32) -> std::result::Result<(), u32> {
            while cwd_len > 0 {
                if self.buf.len() >= VLC_SIZE {
                    return Err(0x00020002);
                }
                let mut avail_bits = 8 - self.last_greater_than_8f as i32 - self.used_bits;
                let t = avail_bits.min(cwd_len);
                self.tmp |= (cwd & ((1 << t) - 1)) << self.used_bits;
                self.used_bits += t;
                avail_bits -= t;
                cwd_len -= t;
                cwd >>= t;
                if avail_bits == 0 {
                    if self.last_greater_than_8f && self.tmp != 0x7F {
                        self.last_greater_than_8f = false;
                        continue;
                    }
                    self.buf.push(self.tmp as u8);
                    self.last_greater_than_8f = self.tmp > 0x8F;
                    self.tmp = 0;
                    self.used_bits = 0;
                }
            }
            Ok(())
        }
    }

    /// `Vlc` against `RefVlc` on random codewords, from 0 to 40 bits but
    /// mostly up to 8 as the encoder writes them, many all ones or with
    /// their top bits set (for bytes above 0x8F and 0x7F, and so
    /// stuffing), up to and past the end of the buffer: every call must
    /// succeed or fail alike, and the bytes written and the state
    /// `terminate_mel_vlc` reads must be the same.
    #[test]
    fn vlc_matches_byte_writer() {
        let mut seed = 0x766cu64;
        for round in 0..3000usize {
            let mut vlc = Vlc::new();
            let mut rv = RefVlc {
                buf: vec![0xFF],
                used_bits: 4,
                tmp: 0xF,
                last_greater_than_8f: true,
            };
            let style = mix(&mut seed) % 4;
            let calls = mix(&mut seed) % 6000;
            let mut failed = false;
            for _ in 0..calls {
                let r = mix(&mut seed);
                let len = match r % 16 {
                    0 => (r >> 8) % 41,
                    _ => (r >> 8) % 9,
                } as i32;
                let cwd = match (r >> 16) % 4 {
                    k if k < style => -1,
                    3 => ((r >> 24) as i32) | !0x7F,
                    _ => (r >> 24) as i32,
                };
                let a = vlc.encode(cwd, len).map_err(|e| e.code());
                let b = rv.encode(cwd, len);
                assert_eq!(a, b, "round {round}");
                if a.is_err() {
                    failed = true;
                    break;
                }
            }
            if !failed {
                vlc.flush_bytes();
                assert_eq!(vlc.used_bits, rv.used_bits, "round {round}");
                assert_eq!(vlc.acc as i32, rv.tmp, "round {round}");
                assert_eq!(
                    vlc.last_greater_than_8f, rv.last_greater_than_8f,
                    "round {round}"
                );
            }
            let written: Vec<u8> = vlc.buf[VLC_SIZE - vlc.pos..]
                .iter()
                .rev()
                .copied()
                .collect();
            if failed {
                // the bytes written before the failing call
                assert!(rv.buf.starts_with(&written) || written.starts_with(&rv.buf));
            } else {
                assert_eq!(written, rv.buf, "round {round}");
            }
        }
    }
}
