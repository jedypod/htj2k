// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoder's tree after `reset` is the tree `write_headers` builds:
//! every field that encoding changes is put back, whether the image was
//! encoded and flushed, exchanged in part, or failed half way.

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
    for tile in enc.tiles.iter_mut() {
        tile.fill_buffers(0);
    }
    format!("{:?}", enc.tiles)
}

/// How far to take an encoder before resetting its tree.
#[derive(Debug, Clone, Copy)]
enum Run {
    /// Every line, and `flush`.
    Flushed,
    /// The first `n` lines.
    Part(usize),
    /// Every line, with a memory limit that coded data soon exceeds.
    Failed,
}

/// Encodes `a`'s image as far as `run` says, then resets the tree.
fn used_then_reset(a: &EncArgs, img: &[Vec<i64>], run: Run) -> String {
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
    for tile in enc.tiles.iter_mut() {
        tile.reset();
    }
    tree_text(&mut enc)
}

#[test]
fn reset_restores_the_tree_write_headers_builds() {
    for config in CONFIGS {
        let a = args(config);
        let img = a.synthesise();
        let fresh = tree_text(&mut drive::headers(&a, &mut Vec::new()).unwrap());
        for run in [Run::Flushed, Run::Part(5), Run::Failed] {
            let used = used_then_reset(&a, &img, run);
            assert!(used == fresh, "{config}: tree differs after {run:?}");
        }
    }
}
