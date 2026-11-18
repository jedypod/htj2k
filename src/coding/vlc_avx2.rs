// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The VLC and MEL step of the HT block decoder for 32-bit samples, as
//! OpenJPH's AVX2 decoder does it (`decode_cb_step1_vlc`, `destuff_vlc`,
//! `destuff_rev`, `drefill` and `dconsume` in
//! `ojph_block_decoder_avx2.cpp`), for the `simd` feature.
//!
//! The step is serial and scalar in OpenJPH too. What makes it cheaper
//! than the generic decoder's is the way it reads the VLC segment:
//!
//! * the whole segment is unstuffed once into a flat buffer
//!   ([`destuff_vlc`]), eight bytes at a time where no byte needs
//!   unstuffing, instead of four bytes at a time with the unstuffing
//!   tests in the decoding loop;
//! * the bit window is then refilled with one unaligned load and no
//!   branch ([`Window::refill`]), leaving 56 to 63 bits;
//! * in non-initial rows, both `u_q` of a quad pair come from one lookup
//!   in a 4096-entry table (`UVLC_TBL1_WIDE`) when their codewords fit in
//!   10 bits.
//!
//! The scratch it writes is that of [`super::decoder32`]'s scalar step
//! for every input, including malformed ones: the unstuffed buffer holds
//! exactly the bits the scalar reader delivers, zeros past the end
//! included, and a quad pair never consumes more bits than either reader
//! guarantees to hold (see the tests). MEL decoding is the scalar
//! reader's, unchanged.

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

use super::readers::Mel;
use super::tables::{UVLC_TBL0, UVLC_TBL1, UVLC_TBL1_WIDE, VLC_TBL0, VLC_TBL1};
use super::SCRATCH_LEN;
use std::cell::Cell;

/// The most unstuffed bytes [`destuff_vlc`] writes: the VLC segment has
/// fewer than 4079 bytes (`scup` is at most 4079).
const VLC_CAP: usize = 4096;
/// The unstuffing buffer: the data, the byte holding its last bits, and
/// zero bytes for the 8-byte loads of [`Window::refill`] past the end.
const VLC_BUF: usize = VLC_CAP + 1 + 8;

/// The 64 bits at `s[i..i + 8]`, little-endian.
#[inline(always)]
fn le64(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().unwrap())
}

/// `destuff_vlc` and `destuff_rev`: unstuffs the VLC segment, read
/// backward from `data[lcup - 2]`, into `dst` as the scalar reader
/// (`Rev::new_vlc`) delivers its bits: the first byte gives its upper 4
/// bits (3 if its bits 4 to 6 are all 1s); after that a byte gives 7
/// bits (its MSB is OR'ed into the next byte's first bit) when the byte
/// before it was above 0x8F and its own low 7 bits are all 1s, else 8;
/// past the `scup - 2` bytes every bit is 0. `dst` must be all zeros;
/// returns the offset from which every byte is 0.
fn destuff_vlc(data: &[u8], lcup: i32, scup: i32, dst: &mut [u8; VLC_BUF]) -> usize {
    /// The output position and the bits not yet written.
    struct Out {
        o: usize,
        acc: u64, // the low `nb` bits are pending
        nb: u32,  // below 8 between bytes
        unstuff: bool,
    }
    impl Out {
        #[inline(always)]
        fn byte(&mut self, d: u8, dst: &mut [u8; VLC_BUF]) {
            self.acc |= (d as u64) << self.nb;
            self.nb += 8 - (self.unstuff && (d & 0x7F) == 0x7F) as u32;
            self.unstuff = d > 0x8F;
            if self.nb >= 8 {
                dst[self.o] = self.acc as u8;
                self.o += 1;
                self.acc >>= 8;
                self.nb -= 8;
            }
        }
    }
    let top = (lcup - 2) as usize;
    let d = data[top];
    let acc = (d >> 4) as u64;
    let mut out = Out {
        o: 0,
        acc,
        nb: 4 - ((acc & 7) == 7) as u32,
        unstuff: d > 0x8F, // `(d | 0xF) > 0x8F` in the C++
    };
    // the segment's other bytes, data[end..top], read from top - 1 down
    let n = (scup - 2).max(0) as usize;
    let end = top - n;
    let mut p = top; // one past the next byte to read
    if p > end && out.o < VLC_CAP {
        p -= 1;
        out.byte(data[p], dst);
    }
    // 8 bytes at a time while none of them needs unstuffing: from here
    // on the state for a byte is whether the byte above it (already
    // read) is above 0x8F
    const LOW7: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    while p >= end + 8 && out.o + 16 <= VLC_CAP {
        let v = le64(data, p - 8); // data[p - 8 .. p]
        let above = le64(data, p - 7); // the byte above each of them
        let low7_ones = (v & LOW7).wrapping_add(0x0101_0101_0101_0101) & HIGH;
        let over_8f = (above & LOW7).wrapping_add(0x7070_7070_7070_7070) & above & HIGH;
        if low7_ones & over_8f != 0 {
            for _ in 0..8 {
                p -= 1;
                out.byte(data[p], dst);
            }
        } else {
            // data[p - 1] first
            let w = v.swap_bytes();
            let o = out.o;
            dst[o..o + 8].copy_from_slice(&(out.acc | (w << out.nb)).to_le_bytes());
            out.acc = if out.nb != 0 { w >> (64 - out.nb) } else { 0 };
            out.o += 8;
            p -= 8;
            out.unstuff = data[p] > 0x8F;
        }
    }
    while p > end && out.o < VLC_CAP {
        p -= 1;
        out.byte(data[p], dst);
    }
    let o = out.o;
    dst[o] = out.acc as u8;
    o + 1
}

/// A bit window over the unstuffed VLC segment (`drefill`, `dconsume`).
struct Window<'a> {
    buf: &'a [u8; VLC_BUF],
    limit: usize,
    val: u64,
    bits: u32,
    off: usize,
}

impl Window<'_> {
    /// `drefill`: tops the window up to 56 to 63 bits with one load. Past
    /// the end of the data the load is clamped to `limit`, where every
    /// byte is 0, which is what the stream holds there.
    #[inline(always)]
    fn refill(&mut self) {
        let v = le64(self.buf, self.off.min(self.limit));
        self.val |= v << self.bits;
        self.off += ((63 - self.bits) >> 3) as usize;
        self.bits |= 56;
    }

    /// `dconsume`
    #[inline(always)]
    fn consume(&mut self, n: u32) {
        self.val >>= n;
        self.bits -= n;
    }
}

/// Step 1 of the cleanup pass (`decode_vlc_mel32` in
/// [`super::decoder32`]), with the same `scratch`.
#[target_feature(enable = "avx2,bmi1,bmi2,lzcnt,popcnt")]
pub(super) fn decode_vlc_mel32(
    scratch: &mut [u16; SCRATCH_LEN],
    data: &[u8],
    lcup: i32,
    scup: i32,
    width: u32,
    height: u32,
    sstr: usize,
) {
    let mut buf = [0u8; VLC_BUF];
    let limit = destuff_vlc(data, lcup, scup, &mut buf).min(VLC_CAP + 1);
    let mut vlc = Window {
        buf: &buf,
        limit,
        val: 0,
        bits: 0,
        off: 0,
    };
    let mut mel = Mel::new(data, lcup, scup);
    let mut run = mel.get_run();

    // Each row is a slice checked once, so that the indexing in the
    // loops needs no checks: a pair of quads at x = 4j writes entries
    // 4j to 4j + 3 of its row, and the row ends with two zero entries.
    let pairs = width.div_ceil(4) as usize;

    // initial quad row
    let row = &mut scratch[..4 * pairs + 2];
    let mut c_q: u32 = 0;
    for j in 0..pairs {
        let (sp, x) = (4 * j, 4 * j as u32);
        // first quad
        vlc.refill();
        let mut t0 = VLC_TBL0[((c_q + (vlc.val as u32 & 0x7F)) & 0x3FF) as usize];
        if c_q == 0 {
            run -= 2;
            t0 = if run == -1 { t0 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        row[sp] = t0;

        c_q = ((t0 as u32 & 0x10) << 3) | ((t0 as u32 & 0xE0) << 2);
        vlc.consume(t0 as u32 & 0x7);

        // second quad
        let more = x + 2 < width;
        let mut t1 = VLC_TBL0[((c_q + (vlc.val as u32 & 0x7F)) & 0x3FF) as usize];
        if c_q == 0 && more {
            run -= 2;
            t1 = if run == -1 { t1 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        t1 = if more { t1 } else { 0 };
        row[sp + 2] = t1;

        c_q = ((t1 as u32 & 0x10) << 3) | ((t1 as u32 & 0xE0) << 2);
        vlc.consume(t1 as u32 & 0x7);

        // decode u
        let mut uvlc_mode = ((t0 as u32 & 0x8) << 3) | ((t1 as u32 & 0x8) << 4);
        if uvlc_mode == 0xC0 {
            run -= 2;
            uvlc_mode += if run == -1 { 0x40 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        let mut uvlc_entry = UVLC_TBL0[(uvlc_mode + (vlc.val as u32 & 0x3F)) as usize] as u32;
        vlc.consume(uvlc_entry & 0x7);
        uvlc_entry >>= 3;
        let mut len = uvlc_entry & 0xF;
        let tmp = vlc.val as u32 & ((1u32 << len) - 1);
        vlc.consume(len);
        uvlc_entry >>= 4;
        len = uvlc_entry & 0x7;
        uvlc_entry >>= 3;
        row[sp + 1] = (1 + (uvlc_entry & 7) + (tmp & !(0xFFu32 << len))) as u16;
        row[sp + 3] = (1 + (uvlc_entry >> 3) + (tmp >> len)) as u16;
    }
    row[4 * pairs] = 0;
    row[4 * pairs + 1] = 0;

    // non-initial quad rows
    let mut y = 2u32;
    while y < height {
        // the row above and this one, as views of one slice of cells: for
        // some widths the last pair's context reads the first entries of
        // this row, as in the C++
        let start = ((y >> 1) as usize - 1) * sstr;
        let rows = Cell::from_mut(&mut scratch[start..start + sstr + 4 * pairs + 2]);
        let rows = rows.as_slice_of_cells();
        let above = &rows[..4 * pairs + 1];
        let here = &rows[sstr..sstr + 4 * pairs + 2];
        let mut c_q: u32 = 0;
        for j in 0..pairs {
            let (sp, x) = (4 * j, 4 * j as u32);
            c_q |= (above[sp].get() as u32 & 0xA0) << 2;
            c_q |= (above[sp + 2].get() as u32 & 0x20) << 4;

            vlc.refill();
            let mut t0 = VLC_TBL1[((c_q + (vlc.val as u32 & 0x7F)) & 0x3FF) as usize];
            if c_q == 0 {
                run -= 2;
                t0 = if run == -1 { t0 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            here[sp].set(t0);

            c_q = ((t0 as u32 & 0x40) << 2) | ((t0 as u32 & 0x80) << 1);
            c_q |= above[sp].get() as u32 & 0x80;
            c_q |= (above[sp + 2].get() as u32 & 0xA0) << 2;
            c_q |= (above[sp + 4].get() as u32 & 0x20) << 4;

            vlc.consume(t0 as u32 & 0x7);

            let more = x + 2 < width;
            let mut t1 = VLC_TBL1[((c_q + (vlc.val as u32 & 0x7F)) & 0x3FF) as usize];
            if c_q == 0 && more {
                run -= 2;
                t1 = if run == -1 { t1 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            t1 = if more { t1 } else { 0 };
            here[sp + 2].set(t1);

            c_q = ((t1 as u32 & 0x40) << 2) | ((t1 as u32 & 0x80) << 1);
            c_q |= above[sp + 2].get() as u32 & 0x80;

            vlc.consume(t1 as u32 & 0x7);

            // both u_q from the wide table when their codewords fit in
            // its 10 bits
            let mode = ((t0 as u32 >> 3) & 1) | (((t1 as u32 >> 3) & 1) << 1);
            let wide = UVLC_TBL1_WIDE[((mode << 10) | (vlc.val as u32 & 0x3FF)) as usize];
            let total = wide & 0x1F;
            if total < 0x1F {
                here[sp + 1].set(((wide >> 5) & 0xFF) as u16);
                here[sp + 3].set(((wide >> 13) & 0xFF) as u16);
                vlc.consume(total);
            } else {
                let uvlc_mode = mode << 6;
                let mut uvlc_entry =
                    UVLC_TBL1[(uvlc_mode + (vlc.val as u32 & 0x3F)) as usize] as u32;
                vlc.consume(uvlc_entry & 0x7);
                uvlc_entry >>= 3;
                let mut len = uvlc_entry & 0xF;
                let tmp = vlc.val as u32 & ((1u32 << len) - 1);
                vlc.consume(len);
                uvlc_entry >>= 4;
                len = uvlc_entry & 0x7;
                uvlc_entry >>= 3;
                here[sp + 1].set(((uvlc_entry & 7) + (tmp & !(0xFFu32 << len))) as u16);
                here[sp + 3].set(((uvlc_entry >> 3) + (tmp >> len)) as u16);
            }
        }
        here[4 * pairs].set(0);
        here[4 * pairs + 1].set(0);
        y += 2;
    }
}

#[cfg(test)]
mod tests {
    use super::super::readers::Rev;
    use super::super::{check_args, decoder32};
    use super::*;

    /// splitmix64
    fn mix(z: &mut u64) -> u64 {
        *z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = *z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    /// Bytes that exercise unstuffing in proportions that vary: random,
    /// or rich in 0xFF, in bytes with all low 7 bits set and in bytes
    /// around 0x8F; or all zeros, or all 0xFF.
    fn bytes(n: usize, seed: &mut u64) -> Vec<u8> {
        let style = mix(seed) % 6;
        (0..n)
            .map(|_| {
                let r = mix(seed);
                match (style, r % 8) {
                    (0, _) => r as u8,
                    (4, _) => 0,
                    (5, _) => 0xFF,
                    (_, 0 | 1) => 0xFF,
                    (_, 2) => 0x7F | (r as u8 & 0x80),
                    (_, 3) => 0x8F + ((r >> 8) % 3) as u8,
                    (3, _) => 0xFF,
                    _ => r as u8,
                }
            })
            .collect()
    }

    /// The unstuffed buffer holds the bits the scalar reader gives, at
    /// every position the decoder can reach, past the end of the data
    /// too, for refills after any consumption a quad pair can make.
    #[test]
    fn destuff_matches_rev() {
        let mut seed = 0x766c_6331u64;
        for round in 0..20000 {
            let lcup = 2 + (mix(&mut seed) % if round % 10 == 0 { 5000 } else { 300 }) as i32;
            let scup = 2 + (mix(&mut seed) % (lcup.min(4079) as u64 - 1)) as i32;
            let data = bytes(lcup as usize, &mut seed);
            let mut buf = [0u8; VLC_BUF];
            let limit = destuff_vlc(&data, lcup, scup, &mut buf).min(VLC_CAP + 1);
            assert!(buf[limit..].iter().all(|&b| b == 0), "round {round}");
            let mut w = Window {
                buf: &buf,
                limit,
                val: 0,
                bits: 0,
                off: 0,
            };
            let mut rev = Rev::new_vlc(&data, lcup, scup);
            for step in 0..(3 * scup + 40) {
                let want = rev.fetch();
                w.refill();
                assert_eq!(w.val as u32, want, "round {round}, step {step}");
                let k = (mix(&mut seed) % 31) as u32;
                rev.advance(k);
                w.consume(k);
            }
        }
    }

    /// The narrow decode of both `u_q` of a non-initial quad pair, as in
    /// the scalar step: (u_q0, u_q1, bits consumed).
    fn narrow(mode: u32, v: u32) -> (u32, u32, u32) {
        let mut e = UVLC_TBL1[((mode << 6) + (v & 0x3F)) as usize] as u32;
        let pre = e & 0x7;
        let v = v >> pre;
        e >>= 3;
        let len = e & 0xF;
        let tmp = v & ((1u32 << len) - 1);
        let total = pre + len;
        e >>= 4;
        let len0 = e & 0x7;
        e >>= 3;
        (
            (e & 7) + (tmp & !(0xFFu32 << len0)),
            (e >> 3) + (tmp >> len0),
            total,
        )
    }

    /// Every entry of the wide table that is used gives what the narrow
    /// decode gives, whatever the bits past its 10.
    #[test]
    fn wide_table_matches_narrow() {
        let mut seed = 0x7769_6465u64;
        let mut used = 0;
        for mode in 0..4u32 {
            for v in 0..1024u32 {
                let e = UVLC_TBL1_WIDE[((mode << 10) | v) as usize];
                if e & 0x1F == 0x1F {
                    continue;
                }
                used += 1;
                for _ in 0..16 {
                    let high = (mix(&mut seed) as u32) << 10;
                    let want = narrow(mode, v | high);
                    let got = ((e >> 5) & 0xFF, (e >> 13) & 0xFF, e & 0x1F);
                    assert_eq!(got, want, "mode {mode}, bits {v:#x}");
                }
            }
        }
        assert!(used > 2000, "{used}");
    }

    /// A quad pair consumes at most 30 bits, so it never needs bits the
    /// scalar reader (32 after a fetch) or the window (56 after a refill)
    /// does not hold.
    #[test]
    fn quad_pair_consumption_bound() {
        let cwd = |t: &[u16]| t.iter().map(|&e| e as u32 & 7).max().unwrap();
        let uvlc = |t: &[u16]| {
            t.iter()
                .map(|&e| (e as u32 & 7) + ((e as u32 >> 3) & 0xF))
                .max()
                .unwrap()
        };
        let initial = 2 * cwd(&VLC_TBL0[..]) + uvlc(&UVLC_TBL0[..]);
        let other = 2 * cwd(&VLC_TBL1[..]) + uvlc(&UVLC_TBL1[..]);
        assert!(initial <= 30 && other <= 30, "{initial} {other}");
    }

    crate::simd::avx2_dispatch! {
        /// The AVX2 step 1 on a copy of `scratch`.
        fn step1(
            scratch: &[u16; SCRATCH_LEN],
            data: &[u8],
            lcup: i32,
            scup: i32,
            size: (u32, u32),
            sstr: usize,
        ) -> Option<Box<[u16; SCRATCH_LEN]>> {
            avx2 => {
                let mut s = Box::new(*scratch);
                decode_vlc_mel32(&mut s, data, lcup, scup, size.0, size.1, sstr);
                Some(s)
            },
            fallback => None,
        }
    }

    /// The scratch of step 1 is the scalar step's, entry for entry, on
    /// random and damaged segments of every block size.
    #[test]
    fn step1_matches_scalar() {
        let mut seed = 0x7374_6570u64;
        let mut sizes: Vec<(u32, u32)> = Vec::new();
        for lw in 2..=10 {
            for lh in 2..=10 {
                if lw + lh <= 12 {
                    sizes.push((1 << lw, 1 << lh));
                }
            }
        }
        for w in 1..=24 {
            for h in [1, 2, 3, 4, 5, 7, 8] {
                sizes.push((w, h));
            }
        }
        sizes.extend([
            (1024, 1),
            (1023, 3),
            (17, 240),
            (33, 124),
            (1, 4096),
            (3, 1365),
        ]);
        let mut ran = 0;
        for round in 0..20000 {
            let (w, h) = sizes[mix(&mut seed) as usize % sizes.len()];
            let lcup = 2 + (mix(&mut seed) % if round % 4 == 0 { 5000 } else { 400 }) as i32;
            let mut data = bytes(lcup as usize, &mut seed);
            // a valid scup (2 to min(lcup, 4079)), as decode checks
            let scup = 2 + (mix(&mut seed) % (lcup.min(4079) as u64 - 1)) as i32;
            let l = lcup as usize;
            data[l - 1] = (scup >> 4) as u8;
            data[l - 2] = (data[l - 2] & 0xF0) | (scup & 0xF) as u8;
            let dec_len = (w * h) as usize;
            let Some(layout) = check_args(l, dec_len, 1, lcup as u32, 0, w, h, w) else {
                continue;
            };
            // garbage in half the rounds: step 1 must write what it reads
            let mut want = Box::new([0u16; SCRATCH_LEN]);
            if round % 2 == 1 {
                for (i, v) in want.iter_mut().enumerate() {
                    *v = (i as u16).wrapping_mul(40503);
                }
            }
            let Some(got) = step1(&want, &data, lcup, scup, (w, h), layout.sstr) else {
                return;
            };
            decoder32::decode_vlc_mel32(&mut want, &data, lcup, scup, w, h, layout.sstr);
            if want != got {
                let i = want.iter().zip(got.iter()).position(|(a, b)| a != b);
                panic!("round {round}: {w}x{h} lcup {lcup} scup {scup}: entry {i:?}");
            }
            ran += 1;
        }
        assert!(ran > 15000, "{ran}");
    }
}
