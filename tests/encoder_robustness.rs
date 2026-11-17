// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Robustness tests for the HT block encoders: random samples and random
//! arguments must never make them panic (run in debug, so arithmetic
//! overflow would panic too). Within the documented ranges an error may
//! only be one of the C++'s buffer-full errors, and outside them the
//! documented argument error. Every block JPEG 2000 allows that encodes
//! is also decoded again, which must give back the coded bits of every
//! sample.

use htj2k::coding::{
    decode_codeblock32, decode_codeblock64, encode_codeblock32, encode_codeblock64,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Rng(u64);

impl Rng {
    /// splitmix64
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to n - 1 (0 for n = 0).
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }

    fn chance(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

/// What differs between the 32- and 64-bit encoders, for the test.
trait Sample: Copy + Default + PartialEq + std::fmt::Debug {
    const BITS: u32;
    /// The largest `missing_msbs` the encoder accepts.
    const MAX_MM: u32;
    /// The largest `missing_msbs` for which the decoder can read back every
    /// block (see `tests/encoder.rs`).
    const MAX_DECODABLE_MM: u32;
    /// Whether every block JPEG 2000 allows fits the encoder's buffers
    /// (true for 32 bits; with 64 the MagSgn buffer can fill).
    const ALWAYS_FITS: bool;
    fn from_u64(v: u64) -> Self;
    fn to_u64(self) -> u64;
    #[allow(clippy::too_many_arguments)]
    fn encode(
        buf: &[Self],
        mm: u32,
        np: u32,
        w: u32,
        h: u32,
        st: u32,
        out: &mut Vec<u8>,
    ) -> htj2k::Result<u32>;
    fn decode(coded: &[u8], dec: &mut [Self], mm: u32, w: u32, h: u32) -> bool;
}

impl Sample for u32 {
    const BITS: u32 = 32;
    const MAX_MM: u32 = 29;
    const MAX_DECODABLE_MM: u32 = 29;
    const ALWAYS_FITS: bool = true;
    fn from_u64(v: u64) -> Self {
        v as u32
    }
    fn to_u64(self) -> u64 {
        self as u64
    }
    fn encode(
        b: &[u32],
        mm: u32,
        np: u32,
        w: u32,
        h: u32,
        st: u32,
        o: &mut Vec<u8>,
    ) -> htj2k::Result<u32> {
        encode_codeblock32(b, mm, np, w, h, st, o)
    }
    fn decode(coded: &[u8], dec: &mut [u32], mm: u32, w: u32, h: u32) -> bool {
        decode_codeblock32(coded, dec, mm, 1, coded.len() as u32, 0, w, h, w, false)
    }
}

impl Sample for u64 {
    const BITS: u32 = 64;
    const MAX_MM: u32 = 61;
    const MAX_DECODABLE_MM: u32 = 55;
    const ALWAYS_FITS: bool = false;
    fn from_u64(v: u64) -> Self {
        v
    }
    fn to_u64(self) -> u64 {
        self
    }
    fn encode(
        b: &[u64],
        mm: u32,
        np: u32,
        w: u32,
        h: u32,
        st: u32,
        o: &mut Vec<u8>,
    ) -> htj2k::Result<u32> {
        encode_codeblock64(b, mm, np, w, h, st, o)
    }
    fn decode(coded: &[u8], dec: &mut [u64], mm: u32, w: u32, h: u32) -> bool {
        decode_codeblock64(coded, dec, mm, 1, coded.len() as u32, 0, w, h, w, false)
    }
}

/// One random sample for `missing_msbs` `mm`, in the given style.
fn sample<T: Sample>(rng: &mut Rng, style: u64, mm: u32, planes: u32) -> T {
    let bits = T::BITS;
    let sign = 1u64 << (bits - 1);
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let p = bits - 2 - mm.min(T::MAX_MM);
    let r = rng.next();
    let v = match style {
        0 => r,                                                           // every bit random
        1 => ((r & (sign - 1)) >> (bits - 1 - planes)) << p | (r & sign), // top planes of the coded range
        2 if rng.chance(30) => r,                                         // sparse
        2 => 0,
        3 => r & sign,                 // zeros, some signed
        4 => (sign - 1) | (r & sign),  // largest magnitudes
        5 => (1u64 << p) | (r & sign), // magnitude 1
        _ => (r & 0xFF) << p,          // small magnitudes
    };
    T::from_u64(v & mask)
}

#[derive(Default, Debug)]
struct Counts {
    encoded: usize,
    round_trips: usize,
    buffer_errors: usize,
    argument_errors: usize,
}

/// Encodes one random block; panics (caught by the caller) on any
/// failed check.
fn one_block<T: Sample>(rng: &mut Rng, counts: &mut Counts) {
    // dimensions: mostly what JPEG 2000 allows (nominal sizes 2^xcb by
    // 2^ycb, each 4 to 1024 with a product of at most 4096, and the block
    // no larger), sometimes larger
    let (w, h) = match rng.below(20) {
        0 | 1 => (1 + rng.below(1024) as u32, rng.below(40) as u32),
        2 | 3 => (1021 + rng.below(4) as u32, 1 + rng.below(4) as u32),
        4 => (0, rng.next() as u32), // any height, up to u32::MAX
        _ => {
            let xcb = 2 + rng.below(9) as u32;
            let ycb = 2 + rng.below(11 - xcb as u64).min(8) as u32;
            let w = 1 + rng.below(1 << xcb) as u32;
            (w, 1 + rng.below(1 << ycb) as u32)
        }
    };
    let legal = w >= 1 && h >= 1 && h <= 4096 / w.max(4).next_power_of_two() && h <= 1024;
    let stride = match rng.below(8) {
        0 => rng.below(w as u64 + 1) as u32, // overlapping rows
        _ => w + rng.below(9) as u32,
    };
    let mm = rng.below(T::MAX_MM as u64 + 1) as u32;
    let planes = 1 + rng.below(mm as u64 + 1) as u32;
    let style = rng.below(7);
    let needed = if w == 0 {
        0
    } else if h == 0 {
        w as usize
    } else {
        (h as usize - 1) * stride as usize + w as usize
    };
    let mut buf: Vec<T> = (0..needed)
        .map(|_| sample::<T>(rng, style, mm, planes))
        .collect();

    // occasionally break one argument
    let (mut mm_arg, mut np, mut w_arg, mut expect) = (mm, 1, w, None);
    match rng.below(40) {
        0 => {
            np = [0, 2, 3, u32::MAX][rng.below(4) as usize];
            expect = Some(0x00020010);
        }
        1 => {
            mm_arg = T::MAX_MM + 1 + rng.below(40) as u32;
            expect = Some(0x00020011);
        }
        2 => {
            w_arg = 1025 + rng.below(3000) as u32;
            expect = Some(0x00020012);
        }
        3 if needed > 0 => {
            buf.truncate(rng.below(needed as u64) as usize);
            expect = Some(0x00020013);
        }
        _ => {}
    }

    let mut out = vec![0x77u8; rng.below(3) as usize];
    let before = out.len();
    let res = T::encode(&buf, mm_arg, np, w_arg, h, stride, &mut out);
    let what = format!(
        "{}-bit {w_arg}x{h} stride {stride} mm {mm_arg} passes {np} style {style}",
        T::BITS
    );
    assert!(
        out[..before].iter().all(|&b| b == 0x77),
        "{what}: existing bytes changed"
    );
    match (res, expect) {
        (Err(e), Some(code)) => {
            assert_eq!(e.code(), code, "{what}: {e}");
            assert_eq!(out.len(), before, "{what}: appended on error");
            counts.argument_errors += 1;
        }
        (Ok(_), Some(code)) => panic!("{what}: expected error {code:#010x}"),
        (Err(e), None) => {
            assert!((0x00020001..=0x00020006).contains(&e.code()), "{what}: {e}");
            assert_eq!(out.len(), before, "{what}: appended on error");
            assert!(
                !legal || !T::ALWAYS_FITS,
                "{what}: a legal block failed: {e}"
            );
            counts.buffer_errors += 1;
        }
        (Ok(n), None) => {
            let coded = &out[before..];
            assert_eq!(coded.len(), n as usize, "{what}: length");
            assert!(n >= 2, "{what}: length {n}");
            counts.encoded += 1;
            if legal && mm <= T::MAX_DECODABLE_MM {
                round_trip(&what, &buf, coded, mm, w, h, stride);
                counts.round_trips += 1;
            }
        }
    }
}

/// Decodes `coded` and compares with the coded part of each sample: the
/// sign and magnitude bits `p` and up, plus the decoder's half-bin bit.
fn round_trip<T: Sample>(
    what: &str,
    buf: &[T],
    coded: &[u8],
    mm: u32,
    w: u32,
    h: u32,
    stride: u32,
) {
    let mut dec = vec![T::default(); (w * h) as usize];
    assert!(
        T::decode(coded, &mut dec, mm, w, h),
        "{what}: the decoder rejected the block"
    );
    let bits = T::BITS;
    let sign = 1u64 << (bits - 1);
    let p = bits - 2 - mm;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let t = buf[y * stride as usize + x].to_u64();
            let mu = (t & (sign - 1)) >> p;
            let want = if mu == 0 {
                0
            } else {
                (t & sign) | ((2 * mu + 1) << (p - 1))
            };
            assert_eq!(
                dec[y * w as usize + x].to_u64(),
                want,
                "{what}: sample ({x}, {y})"
            );
        }
    }
}

fn run<T: Sample>(seed: u64) {
    // HTJ2K_ROBUSTNESS_ROUNDS multiplies the number of blocks, for a
    // longer run than the default.
    let scale = std::env::var("HTJ2K_ROBUSTNESS_ROUNDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    for i in 0..1500 * scale {
        let mut rng = Rng(seed ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
        let r = catch_unwind(AssertUnwindSafe(|| one_block::<T>(&mut rng, &mut counts)));
        if r.is_err() {
            failures.push(i);
        }
    }
    eprintln!("{}-bit: {counts:?}", T::BITS);
    assert!(
        failures.is_empty(),
        "{} blocks failed: {failures:?}",
        failures.len()
    );
    // guard against the generator drifting out of the interesting cases
    assert!(
        counts.round_trips > 500 && counts.argument_errors > 50,
        "{counts:?}"
    );
    assert!(counts.buffer_errors > 0, "{counts:?}");
}

#[test]
fn random_blocks32() {
    run::<u32>(0x3232);
}

#[test]
fn random_blocks64() {
    run::<u64>(0x6464);
}
