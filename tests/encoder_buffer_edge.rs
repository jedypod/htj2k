// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The block encoders at the edge of their MagSgn buffer (17477 bytes for
//! 32-bit samples, 24030 for 64-bit). Blocks are built so that the MagSgn
//! segment ends at every byte position from well below the buffer's size
//! to past it, with and without 0xFF bytes (and so bit stuffing) near the
//! end, and every outcome (the coded bytes, or the error code) is hashed.
//! The expected hashes were recorded from the byte-at-a-time MagSgn writer
//! the encoder ported from OpenJPH, so a faster writer must fail, or
//! succeed, exactly where it did, with the same bytes.

use htj2k::coding::{encode_codeblock32, encode_codeblock64};

/// FNV-1a-64.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
}

/// splitmix64
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The outcomes of a sweep.
#[derive(Default, Debug)]
struct Sweep {
    hash: u64,
    ok: usize,
    errors: Vec<u32>,
    /// MagSgn lengths of the blocks that encoded, smallest and largest.
    ms_min: usize,
    ms_max: usize,
}

/// Block of `width` x `height` samples of `bits` bits (sign at the top,
/// magnitude below): `k` samples, in raster order, whose magnitude has its
/// top bit set (so each takes the same number of MagSgn bits) and random
/// or all-ones lower bits (`ones` in 8, giving 0xFF bytes), then one sample
/// with the magnitude shifted right by `shift`, then zeros.
fn block(bits: u32, len: usize, k: usize, shift: u32, ones: u64, seed: u64) -> Vec<u64> {
    let top = 1u64 << (bits - 2);
    let mask = (top << 1) - 1;
    let sign = 1u64 << (bits - 1);
    (0..len)
        .map(|j| {
            let r = mix(seed ^ (j as u64) << 20);
            let mag = if r % 8 < ones {
                mask
            } else {
                top | (r >> 16) & mask
            };
            let s = if r & 1 != 0 { sign } else { 0 };
            if j < k {
                s | mag
            } else if j == k {
                s | (mag >> shift)
            } else {
                0
            }
        })
        .collect()
}

/// Encodes every block of a sweep: `k` over `ks` and the last sample's
/// shift over all magnitude bits.
fn sweep1(bits: u32, mm: u32, w: u32, h: u32, ones: u64, ks: std::ops::Range<usize>) -> Sweep {
    let mut f = Fnv::new();
    let mut s = Sweep {
        ms_min: usize::MAX,
        ..Sweep::default()
    };
    {
        for k in ks.clone() {
            for shift in 0..bits - 1 {
                let b = block(
                    bits,
                    (w * h) as usize,
                    k,
                    shift,
                    ones,
                    k as u64 ^ ones << 40,
                );
                let mut out = Vec::new();
                let res = if bits == 32 {
                    let b: Vec<u32> = b.iter().map(|&v| v as u32).collect();
                    encode_codeblock32(&b, mm, 1, w, h, w, &mut out)
                } else {
                    encode_codeblock64(&b, mm, 1, w, h, w, &mut out)
                };
                match res {
                    Ok(n) => {
                        assert_eq!(n as usize, out.len());
                        f.u32(n);
                        f.bytes(&out);
                        // the interface locator word gives MEL + VLC
                        let n = n as usize;
                        let mel_vlc = ((out[n - 1] as usize) << 4) | (out[n - 2] & 0xF) as usize;
                        let ms = n - mel_vlc;
                        s.ms_min = s.ms_min.min(ms);
                        s.ms_max = s.ms_max.max(ms);
                        s.ok += 1;
                    }
                    Err(e) => {
                        assert!(out.is_empty());
                        f.u32(0xFFFF_FFFF);
                        f.u32(e.code());
                        if !s.errors.contains(&e.code()) {
                            s.errors.push(e.code());
                        }
                    }
                }
            }
        }
    }
    s.hash = f.0;
    s
}

/// One configuration: the encoder (`bits`), `missing_msbs`, the block's
/// size, its MagSgn buffer size and, for each mix of 0xFF bytes (`ones`
/// of 0, 1 and 6 in 8), the first `k` whose block does not fit when the
/// last sample is unshifted (found by `explore`).
struct Edge {
    bits: u32,
    mm: u32,
    w: u32,
    h: u32,
    ms_size: usize,
    first_failing: [usize; 3],
    hash: u64,
}

const EDGES: [Edge; 4] = [
    Edge {
        bits: 32,
        mm: 29,
        w: 64,
        h: 96,
        ms_size: 17477,
        first_failing: [4654, 4621, 4447],
        hash: 0x2cd1beb861d5cbb2,
    },
    Edge {
        bits: 32,
        mm: 29,
        w: 1024,
        h: 8,
        ms_size: 17477,
        first_failing: [4648, 4615, 4445],
        hash: 0xf331a69351182ca2,
    },
    Edge {
        bits: 64,
        mm: 50,
        w: 64,
        h: 64,
        ms_size: 24030,
        first_failing: [3766, 3738, 3609],
        hash: 0x52fd0192c38b8555,
    },
    Edge {
        bits: 64,
        mm: 58,
        w: 64,
        h: 64,
        ms_size: 24030,
        first_failing: [3255, 3230, 3107],
        hash: 0x248f3a23f3578fa6,
    },
];

#[test]
fn magsgn_buffer_edge() {
    let mut bad = Vec::new();
    for e in &EDGES {
        let mut f = Fnv::new();
        let (mut ok, mut ms_max) = (0, 0);
        for (ones, &k) in [0u64, 1, 6].into_iter().zip(&e.first_failing) {
            let s = sweep1(e.bits, e.mm, e.w, e.h, ones, k - 4..k + 1);
            assert_eq!(s.errors, [0x00020005], "{}-bit mm {}: {s:?}", e.bits, e.mm);
            assert!(s.ms_max <= e.ms_size, "{s:?}");
            f.bytes(&s.hash.to_le_bytes());
            ok += s.ok;
            ms_max = ms_max.max(s.ms_max);
        }
        // the sweep must reach the edge: blocks that fill the buffer to
        // its last byte or nearly, and blocks one bit too large
        assert!(
            ok > 0 && ms_max + 2 >= e.ms_size,
            "{}-bit mm {}: {ms_max}",
            e.bits,
            e.mm
        );
        eprintln!(
            "{}-bit mm {} {}x{}: hash {:#018x}",
            e.bits, e.mm, e.w, e.h, f.0
        );
        if f.0 != e.hash {
            bad.push((e.bits, e.mm, e.w, e.h, f.0));
        }
    }
    assert!(bad.is_empty(), "outcomes differ: {bad:x?}");
}

#[test]
#[ignore]
fn explore() {
    for (bits, mm, w, h) in [
        (32u32, 29u32, 64u32, 96u32),
        (32, 29, 1024, 8),
        (64, 50, 64, 64),
        (64, 58, 64, 64),
    ] {
        for ones in [0u64, 1, 6] {
            let fails = |k: usize| {
                let b = block(bits, (w * h) as usize, k, 0, ones, k as u64 ^ ones << 40);
                let mut out = Vec::new();
                if bits == 32 {
                    let b: Vec<u32> = b.iter().map(|&v| v as u32).collect();
                    encode_codeblock32(&b, mm, 1, w, h, w, &mut out).is_err()
                } else {
                    encode_codeblock64(&b, mm, 1, w, h, w, &mut out).is_err()
                }
            };
            let (mut lo, mut hi) = (0usize, (w * h) as usize);
            if !fails(hi - 1) {
                eprintln!("{bits} mm {mm} {w}x{h} ones {ones}: never fails");
                continue;
            }
            while lo + 1 < hi {
                let mid = (lo + hi) / 2;
                if fails(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let s = sweep1(bits, mm, w, h, ones, hi - 3..hi + 2);
            eprintln!("{bits} mm {mm} {w}x{h} ones {ones}: first failing k {hi}: {s:?}");
        }
    }
}
