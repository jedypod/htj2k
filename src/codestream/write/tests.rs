// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The forward path against OpenJPH: the inputs of every call to the
//! block encoder, while an image is encoded, must be those OpenJPH's
//! encoder was called with for the same image. `tests/data/encoder`
//! holds a selection of those calls, recorded by `oracle/gen_enc_blocks.py`
//! from 23 `ojph_enc` runs; `INDEX.txt` names the run and the number of
//! the call within it. A mismatch here is in the wavelet, colour or
//! sample conversion, or the order blocks are coded in, not in the
//! packets.

#[path = "../../../tests/common/mod.rs"]
mod common;

use super::codeblock::record;
use common::drive;
use common::synth::EncArgs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The runs of `oracle/gen_enc_blocks.py`: name and `ojph_enc` arguments.
const RUNS: &[(&str, &str)] = &[
    ("s16_rct_128x32", "--size 256x64 --comps 3 --depth 16 --signed 1 --rct 1 --levels 5 --block 128x32 --seed 1"),
    ("s16_nlt3_rct_128x32", "--size 256x64 --comps 3 --depth 16 --signed 1 --nlt3 --rct 1 --levels 5 --block 128x32 --pattern noise --seed 2"),
    ("u16_noise_128x32", "--size 160x40 --depth 16 --levels 5 --block 128x32 --pattern noise --seed 3"),
    ("u8_smooth_64", "--size 130x70 --depth 8 --levels 2 --block 64x64 --seed 4"),
    ("s12_noise_32x128", "--size 40x140 --depth 12 --signed 1 --levels 1 --block 32x128 --pattern noise --seed 5"),
    ("u8_noise_4x4", "--size 21x19 --depth 8 --levels 2 --block 4x4 --pattern noise --seed 6"),
    ("u1_noise", "--size 37x29 --depth 1 --levels 3 --block 8x8 --pattern noise --seed 7"),
    ("u8_const", "--size 33x31 --depth 8 --levels 3 --block 16x16 --pattern const --seed 8"),
    ("s24_smooth_odd", "--size 45x27 --depth 24 --signed 1 --levels 3 --block 16x8 --seed 9"),
    ("u28_noise", "--size 30x22 --depth 28 --levels 2 --block 8x16 --pattern noise --seed 10"),
    ("irv_u12", "--size 48x40 --comps 3 --depth 12 --irrev --rct 1 --levels 3 --block 32x32 --seed 11"),
    ("lv0_noise_1024x4", "--size 1024x8 --depth 8 --levels 0 --block 1024x4 --pattern noise --seed 12"),
    ("lv0_smooth_4x1024", "--size 8x1024 --depth 10 --levels 0 --block 4x1024 --seed 13"),
    ("tiles_sub", "--size 37x29 --comps 3 --depth 8 --levels 2 --block 8x8 --tile 16x16 --sub 1x1,2x2,2x1 --seed 14"),
    ("s32_rct_128x32", "--size 256x64 --comps 3 --depth 32 --signed 1 --rct 1 --levels 5 --block 128x32 --seed 21"),
    ("s32_nlt3_128x32", "--size 160x40 --depth 32 --signed 1 --nlt3 --levels 5 --block 128x32 --pattern noise --seed 22"),
    ("u32_noise", "--size 40x36 --depth 32 --levels 3 --block 16x16 --pattern noise --seed 23"),
    ("u32_smooth", "--size 37x29 --depth 32 --levels 2 --block 8x8 --seed 24"),
    ("s31_noise", "--size 33x31 --depth 31 --signed 1 --levels 5 --block 32x32 --pattern noise --seed 25"),
    ("s30_noise_l5", "--size 24x20 --depth 30 --signed 1 --levels 5 --block 8x8 --pattern noise --seed 26"),
    ("u32_const", "--size 19x17 --depth 32 --levels 2 --block 8x8 --pattern const --seed 27"),
    ("lv0_u32_noise_1024x4", "--size 1024x4 --depth 32 --levels 0 --block 1024x4 --pattern noise --seed 28"),
    ("s32_4x4", "--size 13x11 --depth 32 --signed 1 --levels 2 --block 4x4 --pattern noise --seed 29"),
];

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/encoder")
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// One recorded call: what the encoder was given.
struct Recorded {
    file: String,
    record: usize,
    call: record::Call,
}

/// The records of `enc32.ojbe` and `enc64.ojbe`, by run name and call
/// number.
fn recorded() -> HashMap<(String, usize), Recorded> {
    let index = std::fs::read_to_string(data_dir().join("INDEX.txt")).unwrap();
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut out = HashMap::new();
    for line in index.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split('|').map(str::trim).collect();
        if f[0] == "synth" {
            continue;
        }
        let data = files
            .entry(f[0].to_string())
            .or_insert_with(|| std::fs::read(data_dir().join(format!("{}.ojbe", f[0]))).unwrap());
        let off: usize = f[2].parse().unwrap();
        let b = &data[off..];
        assert_eq!(&b[0..4], b"OJBE");
        let bits = b[4] as u32;
        let (w, h) = (u32_at(b, 16), u32_at(b, 20));
        let n = (w * h) as usize;
        let samples = (0..n)
            .map(|i| {
                let o = 40 + i * (bits as usize / 8);
                if bits == 32 {
                    u32_at(b, o) as u64
                } else {
                    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
                }
            })
            .collect();
        let call = record::Call {
            bits,
            missing_msbs: u32_at(b, 8),
            width: w,
            height: h,
            samples,
        };
        let key = (f[4].to_string(), f[5].parse().unwrap());
        out.insert(
            key,
            Recorded {
                file: f[0].to_string(),
                record: f[1].parse().unwrap(),
                call,
            },
        );
    }
    out
}

/// The block-encoder calls made while encoding a run's image.
fn calls_of(args: &str) -> Vec<record::Call> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let a = EncArgs::parse(&words).unwrap();
    let img = a.synthesise();
    let mut out = Vec::new();
    let mut cs = drive::headers(&a, &mut out).unwrap();
    record::start();
    let pushed = drive::push_image(&mut cs, &a, &img);
    let calls = record::take();
    pushed.unwrap();
    calls
}

/// Whether `ours` is OpenJPH's call `theirs` with the encoder's extra
/// bit-plane for reversible coding without decompositions (see
/// `ParamQcd::set_rev_quant`): same block, `missing_msbs` one more, and
/// each sample with the same sign and its magnitude one bit lower in the
/// field, as K_max is one more. OpenJPH's field is ours shifted up one
/// bit, so where our top magnitude bit is set OpenJPH lost it (the
/// minimum sample, among others). Returns how many samples lost it.
fn one_more_bitplane(ours: &record::Call, theirs: &record::Call) -> Option<usize> {
    let sign = 1u64 << (ours.bits - 1);
    if (ours.bits, ours.width, ours.height) != (theirs.bits, theirs.width, theirs.height)
        || ours.missing_msbs != theirs.missing_msbs + 1
        || ours.samples.len() != theirs.samples.len()
    {
        return None;
    }
    let mut lost = 0;
    for (&o, &t) in ours.samples.iter().zip(&theirs.samples) {
        let (om, tm) = (o & (sign - 1), t & (sign - 1));
        if o & sign != t & sign || (om << 1) & (sign - 1) != tm {
            return None;
        }
        lost += usize::from(om & (sign >> 1) != 0);
    }
    Some(lost)
}

/// Gate 2: every recorded call is made, with the same inputs, at the same
/// position in the sequence of calls. The runs without decompositions
/// (`lv0_*`) are compared after [`one_more_bitplane`]: there the encoder
/// departs from OpenJPH on purpose, so the minimum sample survives.
#[test]
fn block_encoder_inputs_match_openjph() {
    let rec = recorded();
    let mut matched = 0;
    let mut truncated = 0;
    for &(name, args) in RUNS {
        let calls = calls_of(args);
        let mut keys: Vec<usize> = rec
            .keys()
            .filter(|(n, _)| n == name)
            .map(|&(_, k)| k)
            .collect();
        keys.sort_unstable();
        assert!(!keys.is_empty(), "{name}: no records");
        for k in keys {
            let r = &rec[&(name.to_string(), k)];
            let Some(ours) = calls.get(k) else {
                panic!(
                    "{name}: only {} calls, OpenJPH's call {k} is missing",
                    calls.len()
                );
            };
            if name.starts_with("lv0") {
                match one_more_bitplane(ours, &r.call) {
                    Some(lost) => truncated += lost,
                    None => panic!("{name}: call {k} is not OpenJPH's with one more bit-plane"),
                }
                matched += 1;
                continue;
            }
            assert!(
                *ours == r.call,
                "{name}: call {k} ({} record {}) differs: ours {}-bit {}x{} mm {}, OpenJPH {}-bit {}x{} mm {}",
                r.file,
                r.record,
                ours.bits,
                ours.width,
                ours.height,
                ours.missing_msbs,
                r.call.bits,
                r.call.width,
                r.call.height,
                r.call.missing_msbs
            );
            matched += 1;
        }
    }
    // every record of enc32.ojbe (287) and enc64.ojbe (168)
    assert_eq!(matched, 287 + 168);
    eprintln!("{truncated} samples OpenJPH truncates are kept in the lv0 runs");
}
