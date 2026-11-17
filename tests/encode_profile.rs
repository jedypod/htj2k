// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoder at large sizes and over quality sweeps, against the
//! sizes and hashes of OpenJPH's codestreams in
//! `tests/data/profile/MANIFEST.txt` (`oracle/gen_profile.py`): short,
//! wide images from 8192x256 down to 8192x1 and 1x256 with 128x32
//! blocks, RPCL, TLM and tile parts at resolutions, reversible with NLT
//! type 3 and the RCT or irreversible with NLT type 4 and the ICT, with
//! reversible extra components, sizes of 1 mod 32 with a COM segment,
//! and every Qfactor and a range of explicit per-component steps on a
//! small image. Each output is also decoded with the crate's decoder
//! and compared with OpenJPH's decode.

mod common;

use common::decode::decode_to_raw;
use common::drive;
use common::synth::{fnv1a64, EncArgs};
use std::path::Path;

struct Case {
    name: String,
    status: String,
    j2c: (usize, u64),
    header: (usize, u64),
    raw: (usize, u64),
    command: String,
}

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/profile");
    let text = std::fs::read_to_string(dir.join("MANIFEST.txt")).unwrap();
    let pair = |n: &str, h: &str| (n.parse().unwrap(), u64::from_str_radix(h, 16).unwrap());
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            Case {
                name: f[0].to_string(),
                status: f[1].to_string(),
                j2c: pair(f[2], f[3]),
                header: pair(f[4], f[5]),
                raw: pair(f[6], f[7]),
                command: f[8].to_string(),
            }
        })
        .collect()
}

/// The length of the main header: up to the first SOT marker.
fn main_header_len(data: &[u8]) -> Option<usize> {
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        if data[i + 1] == 0x90 {
            return Some(i);
        }
        i += 2 + u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
    }
    None
}

/// Where OpenJPH's codestream differs from ours, if the oracle has been
/// run here and left it in `oracle/build/profile`.
fn first_difference(name: &str, ours: &[u8]) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("oracle/build/profile")
        .join(format!("{name}.j2c"));
    match std::fs::read(path) {
        Ok(r) => match ours.iter().zip(&r).position(|(a, b)| a != b) {
            Some(at) => format!("first difference at byte {at}"),
            None => format!("one is a prefix of the other ({} bytes)", r.len()),
        },
        Err(_) => "run oracle/gen_profile.py to locate the difference".into(),
    }
}

/// Gate 6: every codestream is OpenJPH's, main header first (so a
/// parameter difference is told apart from one in the coded data), and
/// decodes with the crate's decoder to exactly ojph_dec's samples,
/// which for the lossless cases are the encoder input.
#[test]
fn profile_codestreams_match_openjph() {
    let mut failed = Vec::new();
    let (mut lossless, mut lossy) = (0, 0);
    let all = cases();
    for case in &all {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let img = args.synthesise();
        let mut out = Vec::new();
        if let Err(e) = drive::encode(&args, &mut out) {
            failed.push(format!("{}: {e}", case.name));
            continue;
        }
        // The TLM segment, which `flush` writes, is part of the main
        // header, so it is compared after the whole image is encoded.
        let (hl, hfnv) = case.header;
        let ours_hl = main_header_len(&out);
        if ours_hl != Some(hl) || fnv1a64(&out[..hl.min(out.len())]) != hfnv {
            failed.push(format!(
                "{}: main header differs ({:?} bytes vs {hl}); {}",
                case.name,
                ours_hl,
                first_difference(&case.name, &out)
            ));
            continue;
        }
        let (n, fnv) = case.j2c;
        if out.len() != n || fnv1a64(&out) != fnv {
            failed.push(format!(
                "{}: {} bytes vs OpenJPH's {n}; {}",
                case.name,
                out.len(),
                first_difference(&case.name, &out)
            ));
            continue;
        }
        let got = decode_to_raw(&out).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let (rn, rfnv) = case.raw;
        if got.len() != rn || fnv1a64(&got) != rfnv {
            failed.push(format!("{}: decode differs from ojph_dec's", case.name));
            continue;
        }
        match case.status.as_str() {
            "lossless" => {
                assert!(
                    got == EncArgs::to_raw(&img),
                    "{}: decoded image differs from the input",
                    case.name
                );
                lossless += 1;
            }
            "lossy" => lossy += 1,
            s => panic!("{}: status {s}", case.name),
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {}:\n{}",
        failed.len(),
        all.len(),
        failed.join("\n")
    );
    assert_eq!((lossless, lossy), (24, 329));
}
