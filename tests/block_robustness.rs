// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Robustness tests for the HT block decoders: random, truncated and
//! bit-flipped inputs with random arguments must never panic (run in
//! debug, so arithmetic overflow would panic too) and never write outside
//! `decoded_data`.

use htj2k::coding::{decode_codeblock32, decode_codeblock64};

/// xorshift64*: deterministic, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Makes the last two cleanup bytes encode a plausible `scup` (2..=lcup)
/// most of the time, so decoding gets past the first check.
fn fix_scup(rng: &mut Rng, coded: &mut [u8], lcup: usize) {
    if lcup < 2 || rng.below(8) == 0 {
        return;
    }
    let scup = 2 + rng.below((lcup.min(4079) - 1) as u64) as usize;
    coded[lcup - 1] = (scup >> 4) as u8;
    coded[lcup - 2] = (coded[lcup - 2] & 0xF0) | (scup & 0xF) as u8;
}

#[allow(clippy::too_many_arguments)]
fn run_both(
    rng: &mut Rng,
    coded: &[u8],
    mmsb32: u32,
    mmsb64: u32,
    passes: u32,
    l1: u32,
    l2: u32,
    w: u32,
    h: u32,
    stride: u32,
    causal: bool,
    extra_rows: usize,
) {
    let n = stride as usize * (h as usize + extra_rows);
    let canary = rng.next();
    let mut out32 = vec![canary as u32; n + 16];
    let _ = decode_codeblock32(
        coded,
        &mut out32[..n],
        mmsb32,
        passes,
        l1,
        l2,
        w,
        h,
        stride,
        causal,
    );
    assert!(out32[n..].iter().all(|&v| v == canary as u32));
    let mut out64 = vec![canary; n + 16];
    let _ = decode_codeblock64(
        coded,
        &mut out64[..n],
        mmsb64,
        passes,
        l1,
        l2,
        w,
        h,
        stride,
        causal,
    );
    assert!(out64[n..].iter().all(|&v| v == canary));
}

#[test]
fn random_inputs_do_not_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for iter in 0..10_000u32 {
        // geometry: mostly sensible, sometimes extreme
        let (w, h) = match rng.below(4) {
            0 => (1 + rng.below(64) as u32, 1 + rng.below(64) as u32),
            1 => {
                let w = 1u32 << (2 + rng.below(9));
                let h = (4096 / w).min(1024);
                (
                    1 + rng.below(w as u64) as u32,
                    1 + rng.below(h as u64) as u32,
                )
            }
            2 => (rng.below(1100) as u32, rng.below(1100) as u32),
            _ => (
                rng.next() as u32 >> rng.below(32),
                rng.next() as u32 >> rng.below(32),
            ),
        };
        let stride = match rng.below(4) {
            0 => w.saturating_add(rng.below(8) as u32),
            1 => w,
            2 => rng.below(70) as u32,
            _ => w.saturating_add(rng.below(3) as u32).min(5000),
        };
        if (stride as u64) * (h as u64 + 2) > 8_000_000 {
            continue;
        }
        let l1 = match rng.below(8) {
            0 => rng.below(4) as u32,
            1 => rng.below(4096) as u32,
            _ => rng.below(600) as u32,
        };
        let l2 = if rng.below(2) == 0 {
            0
        } else {
            rng.below(300) as u32
        };
        let mut coded = rng.bytes((l1 + l2) as usize);
        fix_scup(&mut rng, &mut coded, l1 as usize);
        // sometimes hand a shorter slice than the lengths claim
        let clen = if rng.below(16) == 0 {
            rng.below(coded.len() as u64 + 1) as usize
        } else {
            coded.len()
        };
        let passes = if rng.below(16) == 0 {
            rng.next() as u32
        } else {
            rng.below(5) as u32
        };
        let mmsb32 = if rng.below(16) == 0 {
            rng.next() as u32
        } else {
            rng.below(33) as u32
        };
        let mmsb64 = if rng.below(16) == 0 {
            rng.next() as u32
        } else {
            rng.below(66) as u32
        };
        let causal = rng.below(2) == 0;
        let extra = rng.below(5) as usize;
        let _ = iter;
        run_both(
            &mut rng,
            &coded[..clen],
            mmsb32,
            mmsb64,
            passes,
            l1,
            l2,
            w,
            h,
            stride,
            causal,
            extra,
        );
    }
}

/// Every width from 0 to 1025 with the largest heights the decoders accept
/// (and a few more), all pass counts, random data with large
/// `missing_msbs` so that the MagSgn step rarely fails early and the
/// SigProp/MagRef passes run over the whole block.
#[test]
fn geometry_sweep_does_not_panic() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for w in 0..=1025u32 {
        for passes in 1..=3u32 {
            // find the largest accepted height by bisection on a pure
            // argument check (zero-length refinement data keeps it cheap)
            let mut hs: Vec<u32> = vec![0, 1, 2, 3, 4, 5, 7, 8];
            let maxh = largest_height(w, passes);
            for d in 0..6 {
                hs.push(maxh.saturating_sub(d));
                hs.push(maxh + d);
            }
            for &h in &hs {
                if (w as u64) * (h as u64) > 40_000 {
                    continue;
                }
                let l1 = 2 + rng.below(400) as u32;
                let l2 = if passes > 1 {
                    1 + rng.below(200) as u32
                } else {
                    0
                };
                let mut coded = rng.bytes((l1 + l2) as usize);
                fix_scup(&mut rng, &mut coded, l1 as usize);
                let stride = w + rng.below(9) as u32;
                let causal = rng.below(2) == 0;
                run_both(
                    &mut rng, &coded, 28, 60, passes, l1, l2, w, h, stride, causal, 4,
                );
            }
        }
    }
}

/// Largest height (up to 4096) the argument checks accept for width `w`,
/// found by bisection: a minimal cleanup pass (2 bytes, `scup` = 2, all
/// zero otherwise) with `missing_msbs` = 28 decodes successfully whenever
/// the geometry is accepted (checked by `minimal_block_decodes`).
fn largest_height(w: u32, passes: u32) -> u32 {
    let ok = |h: u32| minimal_ok(w, h, passes);
    let (mut lo, mut hi) = (0u32, 4097u32);
    if !ok(0) {
        return 0;
    }
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn minimal_ok(w: u32, h: u32, passes: u32) -> bool {
    // lcup = 2; byte 0 low nibble = 2, byte 1 = 0: scup = 2
    let coded = [0x02u8, 0x00, 0x00, 0x00];
    let stride = w.max(1);
    let mut out = vec![1u32; stride as usize * (h as usize + 1)];
    let l2 = if passes > 1 { 2 } else { 0 };
    decode_codeblock32(&coded, &mut out, 28, passes, 2, l2, w, h, stride, false)
}

#[test]
fn minimal_block_decodes() {
    // every block size JPEG 2000 allows is accepted
    for xcb in 2..=10u32 {
        for ycb in 2..=10u32 {
            if xcb + ycb > 12 {
                continue;
            }
            for passes in 1..=3 {
                for (w, h) in [
                    (1 << xcb, 1 << ycb),
                    (1, 1 << ycb),
                    (1 << xcb, 1),
                    ((1 << xcb) - 1, (1 << ycb) - 1),
                ] {
                    assert!(minimal_ok(w, h, passes), "{w}x{h} passes {passes}");
                }
            }
        }
    }
    assert!(!minimal_ok(1025, 1, 1));
    assert!(!minimal_ok(7, 585, 1));
}
