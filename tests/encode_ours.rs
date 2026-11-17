// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Codestreams the encoder writes differently from OpenJPH on purpose,
//! or that OpenJPH cannot write, checked against OpenJPH's decoder:
//! `oracle/gen_ours.py` encoded each case with this encoder, decoded it
//! with `ojph_dec` and recorded the codestream's size and hash with the
//! verdict in `tests/data/ours/MANIFEST.txt`. Here each case must give
//! exactly that codestream again, so the verdict still applies, and must
//! round-trip through the crate's decoder too.

mod common;

use common::decode::decode_to_raw;
use common::drive;
use common::synth::{fnv1a64, EncArgs};
use std::path::Path;

struct Case {
    name: String,
    status: String,
    j2c_bytes: usize,
    j2c_fnv: u64,
    command: String,
}

fn cases() -> Vec<Case> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ours");
    let text = std::fs::read_to_string(dir.join("MANIFEST.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            Case {
                name: f[0].to_string(),
                status: f[1].to_string(),
                j2c_bytes: f[2].parse().unwrap(),
                j2c_fnv: u64::from_str_radix(f[3], 16).unwrap(),
                command: f[4].to_string(),
            }
        })
        .collect()
}

/// Gate 8: every case decodes losslessly with OpenJPH (by the manifest,
/// for this exact codestream) and with the crate's decoder.
#[test]
fn our_codestreams_decode_losslessly_with_both_decoders() {
    let mut failed = Vec::new();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for case in cases() {
        assert_eq!(case.status, "lossless", "{}: ojph_dec's verdict", case.name);
        let args = EncArgs::parse_command(&case.command).unwrap();
        let img = args.synthesise();
        let mut out = Vec::new();
        if let Err(e) = drive::encode(&args, &mut out) {
            failed.push(format!("{}: {e}", case.name));
            continue;
        }
        if out.len() != case.j2c_bytes || fnv1a64(&out) != case.j2c_fnv {
            failed.push(format!(
                "{}: {} bytes, not the {} ojph_dec checked; run oracle/gen_ours.py",
                case.name,
                out.len(),
                case.j2c_bytes
            ));
            continue;
        }
        match decode_to_raw(&out) {
            Ok(raw) if raw == EncArgs::to_raw(&img) => {}
            Ok(_) => failed.push(format!("{}: decoded image differs", case.name)),
            Err(e) => failed.push(format!("{}: {e}", case.name)),
        }
        let group = case.name.split('_').next().unwrap().to_string();
        *counts.entry(group).or_default() += 1;
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    eprintln!("{counts:?}");
    assert_eq!(counts.get("levels0"), Some(&45));
}
