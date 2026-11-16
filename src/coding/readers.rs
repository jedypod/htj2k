// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Bit readers shared by the HT block decoders (port of the static helper
//! functions at the top of `ojph_block_decoder32.cpp` and
//! `ojph_block_decoder64.cpp`).
//!
//! # Positions instead of pointers
//!
//! The C++ readers hold raw pointers into the code-block buffer; here every
//! reader holds an index into the `coded_data` slice. Every read the C++
//! performs is guarded by the reader's `size` counter, so for arguments that
//! satisfy `lengths1 + lengths2 <= coded_data.len()` (checked by the
//! decoders) no read ever leaves the slice: the generic C++ readers never
//! touch the 8 prefix / 16 suffix padding bytes that OpenJPH allocates
//! around a code-block, they synthesise `0xFF` (MEL, MagSgn) or `0x00`
//! (VLC, SPP, MRP) once a segment is exhausted, and so do these ports.
//!
//! # Alignment
//!
//! The C++ `*_init` functions first read 1 to 4 single bytes so that later
//! 32-bit loads are 4-byte aligned in memory; how many depends on the
//! address of the buffer. The resulting bit sequence does not depend on
//! that number (each byte is unstuffed exactly the same way whichever
//! chunk it is read in, and the decoders never look at more bits than the
//! readers guarantee to hold), so these ports simply pretend that
//! `coded_data` starts at an address that is a multiple of 4.

/// Reads a little-endian `u32` at `pos` (caller guarantees it is in range).
#[inline(always)]
fn load_le_u32(data: &[u8], pos: usize) -> u32 {
    let b = &data[pos..pos + 4];
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

// ---------------------------------------------------------------------------
// MEL
// ---------------------------------------------------------------------------

/// MEL decoder state (`dec_mel_st`).
pub(crate) struct Mel<'a> {
    data: &'a [u8],
    pos: usize,
    tmp: u64,
    bits: i32,
    size: i32,
    unstuff: bool,
    k: i32,
    num_runs: i32,
    runs: u64,
}

impl<'a> Mel<'a> {
    /// `mel_init`: `lcup` is the length of the cleanup pass, `scup` the
    /// length of the MEL + VLC segments (`2 <= scup <= lcup`).
    #[inline]
    pub(crate) fn new(data: &'a [u8], lcup: i32, scup: i32) -> Self {
        let pos = (lcup - scup) as usize;
        let mut m = Mel {
            data,
            pos,
            tmp: 0,
            bits: 0,
            size: scup - 1,
            unstuff: false,
            k: 0,
            num_runs: 0,
            runs: 0,
        };
        let num = 4 - (m.pos & 0x3) as i32;
        for _ in 0..num {
            let mut d: u64 = if m.size > 0 {
                m.data[m.pos] as u64
            } else {
                0xFF
            };
            if m.size == 1 {
                d |= 0xF;
            }
            if m.size > 0 {
                m.pos += 1;
            }
            m.size -= 1;
            let d_bits = 8 - m.unstuff as i32;
            m.tmp = (m.tmp << d_bits) | d;
            m.bits += d_bits;
            m.unstuff = (d & 0xFF) == 0xFF;
        }
        m.tmp <<= 64 - m.bits;
        m
    }

    /// `mel_read`
    #[inline]
    fn read(&mut self) {
        if self.bits > 32 {
            return;
        }
        let mut val: u32 = 0xFFFF_FFFF;
        if self.size > 4 {
            val = load_le_u32(self.data, self.pos);
            self.pos += 4;
            self.size -= 4;
        } else if self.size > 0 {
            let mut i = 0;
            while self.size > 1 {
                let v = self.data[self.pos] as u32;
                self.pos += 1;
                let m = !(0xFFu32 << i);
                val = (val & m) | (v << i);
                self.size -= 1;
                i += 8;
            }
            let v = self.data[self.pos] as u32 | 0xF;
            self.pos += 1;
            let m = !(0xFFu32 << i);
            val = (val & m) | (v << i);
            self.size -= 1;
        }

        let mut bits = 32 - self.unstuff as i32;
        let mut t = val & 0xFF;
        let mut unstuff = (val & 0xFF) == 0xFF;
        bits -= unstuff as i32;
        t <<= 8 - unstuff as u32;

        t |= (val >> 8) & 0xFF;
        unstuff = ((val >> 8) & 0xFF) == 0xFF;
        bits -= unstuff as i32;
        t <<= 8 - unstuff as u32;

        t |= (val >> 16) & 0xFF;
        unstuff = ((val >> 16) & 0xFF) == 0xFF;
        bits -= unstuff as i32;
        t <<= 8 - unstuff as u32;

        t |= (val >> 24) & 0xFF;
        self.unstuff = ((val >> 24) & 0xFF) == 0xFF;

        self.tmp |= (t as u64) << (64 - bits - self.bits);
        self.bits += bits;
    }

    /// `mel_decode`
    #[inline]
    fn decode(&mut self) {
        const MEL_EXP: [i32; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 5];
        if self.bits < 6 {
            self.read();
        }
        while self.bits >= 6 && self.num_runs < 8 {
            let eval = MEL_EXP[self.k as usize];
            let mut run: i32;
            if self.tmp & (1u64 << 63) != 0 {
                run = 1 << eval;
                run -= 1;
                self.k = if self.k + 1 < 12 { self.k + 1 } else { 12 };
                self.tmp <<= 1;
                self.bits -= 1;
                run <<= 1;
            } else {
                run = (self.tmp >> (63 - eval)) as i32 & ((1 << eval) - 1);
                self.k = if self.k - 1 > 0 { self.k - 1 } else { 0 };
                self.tmp <<= eval + 1;
                self.bits -= eval + 1;
                run = (run << 1) + 1;
            }
            let sh = self.num_runs * 7;
            self.runs &= !(0x3Fu64 << sh);
            self.runs |= (run as u64) << sh;
            self.num_runs += 1;
        }
    }

    /// `mel_get_run`
    #[inline]
    pub(crate) fn get_run(&mut self) -> i32 {
        if self.num_runs == 0 {
            self.decode();
        }
        let t = (self.runs & 0x7F) as i32;
        self.runs >>= 7;
        self.num_runs -= 1;
        t
    }
}

// ---------------------------------------------------------------------------
// Backward-growing segments (VLC, MRP)
// ---------------------------------------------------------------------------

/// Reader for a backward-growing segment (`rev_struct`). `pos` is the index
/// of the next byte to read; it may become -1 once the segment (which may
/// start at index 0) is exhausted, but is never dereferenced then.
pub(crate) struct Rev<'a> {
    data: &'a [u8],
    pos: isize,
    pub(crate) tmp: u64,
    pub(crate) bits: u32,
    size: i32,
    unstuff: bool,
}

/// Unstuffs 4 bytes read backward (`val`'s MSB byte first), shared by
/// `rev_read` and `rev_read_mrp`, which have identical bodies apart from
/// the early return.
#[inline(always)]
fn rev_unstuff4(val: u32, prev_unstuff: bool) -> (u32, u32, bool) {
    let mut tmp = val >> 24;
    let mut bits = 8 - (prev_unstuff && ((val >> 24) & 0x7F) == 0x7F) as u32;
    let mut unstuff = (val >> 24) > 0x8F;

    tmp |= ((val >> 16) & 0xFF) << bits;
    bits += 8 - (unstuff && ((val >> 16) & 0x7F) == 0x7F) as u32;
    unstuff = ((val >> 16) & 0xFF) > 0x8F;

    tmp |= ((val >> 8) & 0xFF) << bits;
    bits += 8 - (unstuff && ((val >> 8) & 0x7F) == 0x7F) as u32;
    unstuff = ((val >> 8) & 0xFF) > 0x8F;

    tmp |= (val & 0xFF) << bits;
    bits += 8 - (unstuff && (val & 0x7F) == 0x7F) as u32;
    unstuff = (val & 0xFF) > 0x8F;
    (tmp, bits, unstuff)
}

impl<'a> Rev<'a> {
    /// Reads 4 bytes backward (or the remaining 1-3 followed by zeros, or
    /// 4 zeros once exhausted) into a big-endian-ordered `u32`.
    #[inline(always)]
    fn load4(&mut self) -> u32 {
        let mut val = 0u32;
        if self.size > 3 {
            val = load_le_u32(self.data, (self.pos - 3) as usize);
            self.pos -= 4;
            self.size -= 4;
        } else if self.size > 0 {
            let mut i = 24;
            while self.size > 0 {
                let v = self.data[self.pos as usize] as u32;
                self.pos -= 1;
                val |= v << i;
                self.size -= 1;
                i -= 8;
            }
        }
        val
    }

    /// `rev_read` (32-bit decoder's VLC reader) and `rev_read_mrp`.
    #[inline]
    fn read(&mut self) {
        if self.bits > 32 {
            return;
        }
        let val = self.load4();
        let (tmp, bits, unstuff) = rev_unstuff4(val, self.unstuff);
        self.tmp |= (tmp as u64) << self.bits;
        self.bits += bits;
        self.unstuff = unstuff;
    }

    /// `rev_init`: VLC reader of the 32-bit decoder.
    #[inline]
    pub(crate) fn new_vlc(data: &'a [u8], lcup: i32, scup: i32) -> Self {
        let mut r = Rev {
            data,
            pos: (lcup - 2) as isize,
            tmp: 0,
            bits: 0,
            size: scup - 2,
            unstuff: false,
        };
        let d = r.data[r.pos as usize] as u32;
        r.pos -= 1;
        r.tmp = (d >> 4) as u64;
        r.bits = 4 - ((r.tmp & 7) == 7) as u32;
        #[allow(clippy::ineffective_bit_mask)] // as written in the C++
        let unstuff = (d | 0xF) > 0x8F;
        r.unstuff = unstuff;

        let num = 1 + (r.pos & 0x3) as i32;
        let tnum = if num < r.size { num } else { r.size };
        for _ in 0..tnum {
            let d = r.data[r.pos as usize] as u64;
            r.pos -= 1;
            let d_bits = 8 - (r.unstuff && (d & 0x7F) == 0x7F) as u32;
            r.tmp |= d << r.bits;
            r.bits += d_bits;
            r.unstuff = d > 0x8F;
        }
        r.size -= tnum;
        r.read();
        r
    }

    /// `rev_fetch` / `rev_fetch_mrp`: the next (at least) 32 bits.
    #[inline(always)]
    pub(crate) fn fetch(&mut self) -> u32 {
        if self.bits < 32 {
            self.read();
            if self.bits < 32 {
                self.read();
            }
        }
        self.tmp as u32
    }

    /// `rev_advance` / `rev_advance_mrp` (and `rev_advance64`).
    #[inline(always)]
    pub(crate) fn advance(&mut self, num_bits: u32) -> u64 {
        self.tmp = self.tmp.wrapping_shr(num_bits);
        self.bits = self.bits.wrapping_sub(num_bits);
        self.tmp
    }

    /// `rev_init_mrp`
    #[inline]
    pub(crate) fn new_mrp(data: &'a [u8], lcup: i32, len2: i32) -> Self {
        let mut r = Rev {
            data,
            pos: (lcup + len2 - 1) as isize,
            tmp: 0,
            bits: 0,
            size: len2,
            unstuff: true,
        };
        let num = 1 + (r.pos & 0x3) as i32;
        for _ in 0..num {
            let d: u64 = if r.size > 0 {
                let v = r.data[r.pos as usize] as u64;
                r.pos -= 1;
                v
            } else {
                0
            };
            r.size -= 1;
            let d_bits = 8 - (r.unstuff && (d & 0x7F) == 0x7F) as u32;
            r.tmp |= d << r.bits;
            r.bits += d_bits;
            r.unstuff = d > 0x8F;
        }
        r.read();
        r
    }

    /// `rev_init8` (64-bit decoder's VLC reader).
    #[inline]
    pub(crate) fn new_vlc8(data: &'a [u8], lcup: i32, scup: i32) -> Self {
        let mut r = Rev {
            data,
            pos: (lcup - 2) as isize,
            tmp: 0,
            bits: 0,
            size: scup - 2,
            unstuff: false,
        };
        let mut val = r.data[r.pos as usize];
        r.pos -= 1;
        val >>= 4;
        let t = ((val & 0x7) == 0x7) as u32;
        val &= (0xFu32 >> t) as u8;
        r.tmp = val as u64;
        r.bits = 4 - t;
        r.unstuff = val > 0x8;
        r
    }

    /// `rev_read8`
    #[inline(always)]
    fn read8(&mut self) {
        let mut val: u8 = 0;
        if self.size > 0 {
            val = self.data[self.pos as usize];
            self.pos -= 1;
            self.size -= 1;
        }
        let t = (self.unstuff && (val & 0x7F) == 0x7F) as u32;
        val &= (0xFFu32 >> t) as u8;
        self.tmp |= (val as u64) << self.bits;
        self.bits += 8 - t;
        self.unstuff = val > 0x8F;
    }

    /// `rev_fetch64`: at least 57 bits.
    #[inline(always)]
    pub(crate) fn fetch64(&mut self) -> u64 {
        while self.bits <= 56 {
            self.read8();
        }
        self.tmp
    }
}

// ---------------------------------------------------------------------------
// Forward-growing segments (MagSgn, SPP)
// ---------------------------------------------------------------------------

/// Reader for a forward-growing segment (`frwd_struct32` /
/// `frwd_struct64`). `X` is the byte value fed in once the segment is
/// exhausted (0xFF for MagSgn, 0 for SPP).
pub(crate) struct Frwd<'a, const X: u8> {
    data: &'a [u8],
    pos: usize,
    pub(crate) tmp: u64,
    pub(crate) bits: u32,
    unstuff: u32,
    size: i32,
}

impl<'a, const X: u8> Frwd<'a, X> {
    /// `frwd_read<X>`: 32 bits (fewer after unstuffing).
    #[inline]
    fn read(&mut self) {
        let fill: u32 = if X != 0 { 0xFFFF_FFFF } else { 0 };
        let mut val: u32;
        if self.size > 3 {
            val = load_le_u32(self.data, self.pos);
            self.pos += 4;
            self.size -= 4;
        } else if self.size > 0 {
            let mut i = 0;
            val = fill;
            while self.size > 0 {
                let v = self.data[self.pos] as u32;
                self.pos += 1;
                let m = !(0xFFu32 << i);
                val = (val & m) | (v << i);
                self.size -= 1;
                i += 8;
            }
        } else {
            val = fill;
        }

        let mut bits = 8 - self.unstuff;
        let mut t = val & 0xFF;
        let mut unstuff = ((val & 0xFF) == 0xFF) as u32;

        t |= ((val >> 8) & 0xFF) << bits;
        bits += 8 - unstuff;
        unstuff = (((val >> 8) & 0xFF) == 0xFF) as u32;

        t |= ((val >> 16) & 0xFF) << bits;
        bits += 8 - unstuff;
        unstuff = (((val >> 16) & 0xFF) == 0xFF) as u32;

        t |= ((val >> 24) & 0xFF) << bits;
        bits += 8 - unstuff;
        self.unstuff = (((val >> 24) & 0xFF) == 0xFF) as u32;

        self.tmp |= (t as u64) << self.bits;
        self.bits += bits;
    }

    /// `frwd_init<X>`: segment of `size` bytes starting at `pos`.
    #[inline]
    pub(crate) fn new(data: &'a [u8], pos: usize, size: i32) -> Self {
        let mut f = Frwd {
            data,
            pos,
            tmp: 0,
            bits: 0,
            unstuff: 0,
            size,
        };
        let num = 4 - (f.pos & 0x3);
        for _ in 0..num {
            let d: u64 = if f.size > 0 {
                let v = f.data[f.pos] as u64;
                f.pos += 1;
                v
            } else {
                X as u64
            };
            f.size -= 1;
            f.tmp |= d << f.bits;
            f.bits += 8 - f.unstuff;
            f.unstuff = ((d & 0xFF) == 0xFF) as u32;
        }
        f.read();
        f
    }

    /// `frwd_advance`
    #[inline(always)]
    pub(crate) fn advance(&mut self, num_bits: u32) {
        self.tmp = self.tmp.wrapping_shr(num_bits);
        self.bits = self.bits.wrapping_sub(num_bits);
    }

    /// `frwd_fetch<X>`: the next (at least) 32 bits.
    #[inline(always)]
    pub(crate) fn fetch(&mut self) -> u32 {
        if self.bits < 32 {
            self.read();
            if self.bits < 32 {
                self.read();
            }
        }
        self.tmp as u32
    }

    /// `frwd_init8<X>`
    #[inline]
    pub(crate) fn new8(data: &'a [u8], pos: usize, size: i32) -> Self {
        let mut f = Frwd {
            data,
            pos,
            tmp: 0,
            bits: 0,
            unstuff: 0,
            size,
        };
        f.read8();
        f
    }

    /// `frwd_read8<X>`
    #[inline(always)]
    fn read8(&mut self) {
        let mut val: u8 = X;
        if self.size > 0 {
            val = self.data[self.pos];
            self.pos += 1;
            self.size -= 1;
        }
        let t = (self.unstuff != 0) as u32;
        val &= (0xFFu32 >> t) as u8;
        self.unstuff = (val == 0xFF) as u32;
        self.tmp |= (val as u64) << self.bits;
        self.bits += 8 - t;
    }

    /// `frwd_fetch64<X>`: at least 57 bits.
    #[inline(always)]
    pub(crate) fn fetch64(&mut self) -> u64 {
        while self.bits <= 56 {
            self.read8();
        }
        self.tmp
    }
}
