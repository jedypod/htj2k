// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoder's tree after `reset` is the tree `write_headers` builds:
//! every field that encoding changes is put back, whether the image was
//! encoded and flushed, exchanged in part, or failed half way. And an
//! encoder reset after any of those writes what a new one writes, even
//! with every buffer it keeps filled with garbage.

use super::super::tests::common::{drive, synth::EncArgs};
use super::Encoder;
use crate::codestream::Budget;

/// `ojph_enc` arguments covering several rows of code blocks with a
/// short last one, tiles, subsampling, precincts, irreversible coding,
/// NLT, 64-bit samples, every progression order and tile parts.
pub(crate) const CONFIGS: &[&str] = &[
    "--size 300x200 --comps 4 --depth 16 --signed 1 --nlt3 --rct 1 --planar 0 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R",
    "--size 2048x32 --depth 16 --signed 1 --nlt3 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R",
    "--size 256x32 --depth 16 --signed 1 --nlt3 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R --pattern noise",
    "--size 300x40 --comps 3 --depth 16 --signed 1 --irrev --rct 1 --planar 0 --nlt4 0:513 --nlt4 1:513 --nlt4 2:513 --qfactor 90 --levels 5 --block 128x32 --prog RPCL",
    "--size 130x70 --comps 3 --depth 32 --signed 1 --rct 1 --planar 0 --levels 3 --block 32x32 --prog LRCP --tileparts RC --tlm",
    "--size 37x29 --comps 3 --depth 8 --levels 2 --block 8x8 --tile 16x16 --sub 1x1,2x2,2x1 --planar 1 --prog PCRL",
    "--size 61x45 --comps 2 --depth 12 --levels 3 --block 16x16 --precincts 64x64,32x32 --tile 40x30 --tile-off 3,2 --img-off 5,4 --prog CPRL --tileparts C --planar 1",
    "--size 45x27 --depth 24 --signed 1 --levels 3 --block 16x8 --prog RLCP --irrev --qstep 0.01",
    "--size 33x31 --depth 10 --levels 0 --block 8x8 --pattern noise",
];

fn args(config: &str) -> EncArgs {
    EncArgs::parse(&config.split_whitespace().collect::<Vec<_>>()).unwrap()
}

/// The tree of `enc` with every sample buffer zeroed, as text.
fn tree_text(enc: &mut Encoder) -> String {
    for tile in enc.mem.tiles.iter_mut() {
        tile.fill_buffers(0);
    }
    format!("{:?}", enc.mem.tiles)
}

/// How far to take an encoder before resetting it.
#[derive(Debug, Clone, Copy)]
enum Run {
    /// Every line, and `flush`.
    Flushed,
    /// The first `n` lines.
    Part(usize),
    /// Every line, with a memory limit that coded data soon exceeds.
    Failed,
}

/// An encoder of `a`'s image, taken as far as `run` says.
fn used(a: &EncArgs, img: &[Vec<i64>], run: Run) -> Encoder {
    let mut out = Vec::new();
    let mut enc = drive::headers(a, &mut out).unwrap();
    match run {
        Run::Flushed => {
            drive::push_image(&mut enc, a, img).unwrap();
            enc.flush(&mut out).unwrap();
        }
        Run::Part(n) => {
            for _ in 0..n {
                if enc.exchange().unwrap().is_none() {
                    break;
                }
            }
        }
        Run::Failed => {
            enc.budget = Budget::new(64);
            assert!(drive::push_image(&mut enc, a, img).is_err());
        }
    }
    enc
}

#[test]
fn reset_restores_the_tree_write_headers_builds() {
    for config in CONFIGS {
        let a = args(config);
        let img = a.synthesise();
        let fresh = tree_text(&mut drive::headers(&a, &mut Vec::new()).unwrap());
        for run in [Run::Flushed, Run::Part(5), Run::Failed] {
            let mut enc = used(&a, &img, run);
            for tile in enc.mem.tiles.iter_mut() {
                tile.reset();
            }
            assert!(
                tree_text(&mut enc) == fresh,
                "{config}: tree differs after {run:?}"
            );
        }
    }
}

/// Encodes `a`'s image with `enc`, which must be new or reset: the
/// codestream, or the error code, and the encoder.
fn encode_with(enc: Encoder, a: &EncArgs, img: &[Vec<i64>]) -> (Result<Vec<u8>, u32>, Encoder) {
    let mut out = Vec::new();
    let mut kept = None;
    let run = || -> crate::Result<()> {
        let mut e = drive::configure_on(enc, a)?;
        let r = (|| {
            e.write_headers(&mut out, &drive::comments(a)?)?;
            drive::push_image(&mut e, a, img)?;
            e.flush(&mut out)
        })();
        kept = Some(e);
        r
    };
    let r = run().map(|()| out.clone()).map_err(|e| e.code());
    (r, kept.unwrap_or_default())
}

/// Fills every buffer `enc` keeps with the bit pattern `bits`.
fn poison(enc: &mut Encoder, bits: u32) {
    for tile in enc.mem.tiles.iter_mut() {
        tile.fill_buffers(bits);
    }
    for line in enc.mem.lines.iter_mut() {
        line.fill_bits(bits);
    }
}

/// No sample of a kept buffer is read before it is written: an encoder
/// reset after a full, partial or failed run of the same configuration
/// or of another, with every kept buffer filled with garbage, writes
/// what a new encoder writes, and reuses its tiles when the
/// configuration is the same.
#[test]
fn poisoned_memory_encodes_as_new() {
    for (i, config) in CONFIGS.iter().enumerate() {
        let a = args(config);
        let img = a.synthesise();
        let want = encode_with(Encoder::new(), &a, &img).0;
        assert!(want.is_ok(), "{config}: {want:?}");
        let other = args(CONFIGS[(i + 1) % CONFIGS.len()]);
        let other_img = other.synthesise();
        for run in [Run::Flushed, Run::Part(5), Run::Failed] {
            for bits in [0xA5A5_A5A5, 0xFFFF_FFFF, 0x7FC0_0001] {
                let mut enc = used(&a, &img, run);
                enc.reset();
                poison(&mut enc, bits);
                let before = enc.mem.tiles.as_ptr();
                let (got, enc) = encode_with(enc, &a, &img);
                assert!(got == want, "{config}: differs after {run:?}, {bits:08x}");
                assert!(enc.mem.tiles.as_ptr() == before, "{config}: not reused");

                let mut enc = used(&other, &other_img, run);
                enc.reset();
                poison(&mut enc, bits);
                let (got, _) = encode_with(enc, &a, &img);
                assert!(got == want, "{config}: differs after another, {run:?}");
            }
        }
    }
}
