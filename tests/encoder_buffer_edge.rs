// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The block encoders at the edges of their MagSgn buffer (17477 bytes
//! for 32-bit samples, 24030 for 64-bit) and of their VLC buffer (2880
//! bytes). Blocks are built so that the segment ends at every byte
//! position from below the buffer's size to past it, with and without
//! the bytes that make the writers stuff bits (0xFF in MagSgn, above 0x8F
//! in VLC), and every outcome (the coded bytes, or the error code) is
//! hashed. The expected hashes were recorded from the byte-at-a-time
//! writers the encoder ported from OpenJPH, so a faster writer must fail,
//! or succeed, exactly where they did, with the same bytes.

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
#[derive(Debug)]
struct Sweep {
    hash: u64,
    ok: usize,
    errors: Vec<u32>,
    /// MagSgn lengths of the blocks that encoded, smallest and largest.
    ms_min: usize,
    ms_max: usize,
    /// The same for MEL + VLC.
    mel_vlc_max: usize,
}

/// Encodes `w` x `h` blocks of `bits`-bit samples with `missing_msbs`
/// `mm`, and hashes the outcomes.
fn sweep(bits: u32, mm: u32, w: u32, h: u32, blocks: impl Iterator<Item = Vec<u64>>) -> Sweep {
    let mut f = Fnv::new();
    let mut s = Sweep {
        hash: 0,
        ok: 0,
        errors: Vec::new(),
        ms_min: usize::MAX,
        ms_max: 0,
        mel_vlc_max: 0,
    };
    for b in blocks {
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
                s.mel_vlc_max = s.mel_vlc_max.max(mel_vlc);
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
    s.hash = f.0;
    s
}

// ---------------------------------------------------------------------------
// MagSgn
// ---------------------------------------------------------------------------

/// Block of `len` samples of `bits` bits (sign at the top, magnitude
/// below): `k` samples, in raster order, whose magnitude has its top bit
/// set (so each takes the same number of MagSgn bits) and random or
/// all-ones lower bits (`ones` in 8, giving 0xFF bytes), then one sample
/// with the magnitude shifted right by `shift`, then zeros.
fn ms_block(bits: u32, len: usize, k: usize, shift: u32, ones: u64) -> Vec<u64> {
    let seed = k as u64 ^ ones << 40;
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

/// `ms_block`s for `k` over `ks` and the last sample's shift over all
/// magnitude bits.
fn ms_sweep(bits: u32, mm: u32, w: u32, h: u32, ones: u64, ks: std::ops::Range<usize>) -> Sweep {
    let len = (w * h) as usize;
    let blocks =
        ks.flat_map(move |k| (0..bits - 1).map(move |shift| ms_block(bits, len, k, shift, ones)));
    sweep(bits, mm, w, h, blocks)
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
            let s = ms_sweep(e.bits, e.mm, e.w, e.h, ones, k - 4..k + 1);
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

// ---------------------------------------------------------------------------
// VLC
// ---------------------------------------------------------------------------

/// Block of `len` samples of `bits` bits for a `missing_msbs` that codes
/// the top `planes` magnitude bits: `k` samples, in raster order, of
/// random signs and random values in those planes, so that every quad has
/// a VLC codeword and few MagSgn bits, then one sample of value `last`,
/// then zeros.
fn vlc_block(bits: u32, len: usize, planes: u32, k: usize, last: u64, seed: u64) -> Vec<u64> {
    let shift = bits - 1 - planes;
    let sign = 1u64 << (bits - 1);
    (0..len)
        .map(|j| {
            let r = mix(seed ^ (k as u64) << 32 ^ j as u64);
            let s = if r & 1 != 0 { sign } else { 0 };
            if j < k {
                s | ((r >> 8) % (1 << planes)) << shift
            } else if j == k {
                s | last << shift
            } else {
                0
            }
        })
        .collect()
}

/// One VLC configuration: as [`Edge`], with the planes coded, the first
/// failing `k` for each of three seeds, and the values of the last
/// sample.
struct VlcEdge {
    bits: u32,
    mm: u32,
    planes: u32,
    w: u32,
    h: u32,
    first_failing: [usize; 3],
    hash: u64,
}

const VLC_SEEDS: [u64; 3] = [1, 2, 3];

/// The `k`s swept below and just past `first_failing`.
fn vlc_ks(first_failing: usize) -> impl Iterator<Item = usize> {
    (first_failing - 120..first_failing + 4).step_by(4)
}

const VLC_EDGES: [VlcEdge; 2] = [
    VlcEdge {
        bits: 32,
        mm: 1,
        planes: 2,
        w: 1024,
        h: 32,
        first_failing: [12239, 12211, 12227],
        hash: 0xcec4d6acfb0716a0,
    },
    VlcEdge {
        bits: 32,
        mm: 2,
        planes: 3,
        w: 1024,
        h: 32,
        first_failing: [12436, 12396, 12420],
        hash: 0x51873ad775bfb5bf,
    },
];

/// `vlc_block`s for `k` over `ks` and every value of the last sample.
/// Each sample takes about 2 VLC bits, so `k` over [`vlc_ks`] covers
/// about 30 bytes below the edge.
fn vlc_sweep(e: &VlcEdge, seed: u64, ks: impl Iterator<Item = usize>) -> Sweep {
    let len = (e.w * e.h) as usize;
    let (bits, planes) = (e.bits, e.planes);
    let blocks = ks.flat_map(move |k| {
        (0..1u64 << planes).map(move |last| vlc_block(bits, len, planes, k, last, seed))
    });
    sweep(e.bits, e.mm, e.w, e.h, blocks)
}

#[test]
fn vlc_buffer_edge() {
    let mut bad = Vec::new();
    for e in &VLC_EDGES {
        let mut f = Fnv::new();
        let (mut ok, mut mel_vlc_max) = (0, 0);
        for (&seed, &k) in VLC_SEEDS.iter().zip(&e.first_failing) {
            let s = vlc_sweep(e, seed, vlc_ks(k));
            assert!(s.errors.contains(&0x00020002), "{}-bit: {s:?}", e.bits);
            f.bytes(&s.hash.to_le_bytes());
            ok += s.ok;
            mel_vlc_max = mel_vlc_max.max(s.mel_vlc_max);
        }
        assert!(
            ok > 0 && mel_vlc_max + 4 >= 2880,
            "{}-bit: {mel_vlc_max}",
            e.bits
        );
        eprintln!("{}-bit VLC: hash {:#018x}", e.bits, f.0);
        if f.0 != e.hash {
            bad.push((e.bits, f.0));
        }
    }
    assert!(bad.is_empty(), "outcomes differ: {bad:x?}");
}

// ---------------------------------------------------------------------------

/// Finds the `first_failing` values above, by bisection.
#[test]
#[ignore]
fn explore() {
    let encode_fails = |bits: u32, mm: u32, w: u32, h: u32, b: Vec<u64>| {
        let mut out = Vec::new();
        if bits == 32 {
            let b: Vec<u32> = b.iter().map(|&v| v as u32).collect();
            encode_codeblock32(&b, mm, 1, w, h, w, &mut out).is_err()
        } else {
            encode_codeblock64(&b, mm, 1, w, h, w, &mut out).is_err()
        }
    };
    let first = |len: usize, fails: &dyn Fn(usize) -> bool| {
        let (mut lo, mut hi) = (0usize, len);
        if !fails(hi - 1) {
            return None;
        }
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if fails(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some(hi)
    };
    for e in &EDGES {
        let len = (e.w * e.h) as usize;
        for ones in [0u64, 1, 6] {
            let k = first(len, &|k| {
                encode_fails(e.bits, e.mm, e.w, e.h, ms_block(e.bits, len, k, 0, ones))
            });
            eprintln!(
                "MagSgn {}-bit mm {} {}x{} ones {ones}: first failing k {k:?}",
                e.bits, e.mm, e.w, e.h
            );
            if let Some(k) = k {
                eprintln!(
                    "  {:?}",
                    ms_sweep(e.bits, e.mm, e.w, e.h, ones, k - 4..k + 1)
                );
            }
        }
    }
    for e in &VLC_EDGES {
        let len = (e.w * e.h) as usize;
        for seed in VLC_SEEDS {
            let k = first(len, &|k| {
                encode_fails(
                    e.bits,
                    e.mm,
                    e.w,
                    e.h,
                    vlc_block(e.bits, len, e.planes, k, 0, seed),
                )
            });
            eprintln!(
                "VLC {}-bit {}x{} seed {seed}: first failing k {k:?}",
                e.bits, e.w, e.h
            );
            if let Some(k) = k {
                eprintln!("  {:?}", vlc_sweep(e, seed, vlc_ks(k)));
            }
        }
    }
}
