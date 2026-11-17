// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoder against the parameter matrix of `tests/data/matrix`: each
//! case is encoded from its `ojph_enc` command line (the image made by
//! `tests/common/synth.rs`, the parameters set by `tests/common/drive.rs`
//! as `ojph_enc` sets them) and compared with OpenJPH's codestream.

mod common;

use common::drive;
use common::synth::{parse_matrix_manifest, EncArgs, MatrixCase};
use std::path::{Path, PathBuf};

fn matrix_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/matrix")
}

fn cases() -> Vec<MatrixCase> {
    let text = std::fs::read_to_string(matrix_dir().join("MANIFEST.txt")).unwrap();
    parse_matrix_manifest(&text)
}

fn reference(case: &MatrixCase) -> Vec<u8> {
    std::fs::read(matrix_dir().join(format!("{}.j2c", case.name))).unwrap()
}

/// Gate 1: the main header, from SOC to the last COM, is the start of
/// OpenJPH's codestream, and what follows it there is a TLM marker
/// segment (written by `flush`) or the first SOT.
#[test]
fn main_headers_match_openjph() {
    let mut checked = 0;
    for case in cases().iter().filter(|c| !c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let mut out = Vec::new();
        if let Err(e) = drive::headers(&args, &mut out) {
            panic!("{}: write_headers failed: {e}", case.name);
        }
        let r = reference(case);
        assert!(
            r.starts_with(&out),
            "{}: main header differs from OpenJPH's",
            case.name
        );
        let next = &r[out.len()..out.len() + 2];
        assert!(
            next == [0xFF, 0x90] || next == [0xFF, 0x55],
            "{}: OpenJPH's header continues with {:02X?}",
            case.name,
            next
        );
        checked += 1;
    }
    assert_eq!(checked, 150);
}

/// Gate 3: every codestream OpenJPH wrote, the three MISMATCH cases
/// included, is reproduced byte for byte.
#[test]
fn codestreams_match_openjph() {
    let mut failed = Vec::new();
    let mut checked = 0;
    for case in cases().iter().filter(|c| !c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let mut out = Vec::new();
        match drive::encode(&args, &mut out) {
            Err(e) => failed.push(format!("{}: {e}", case.name)),
            Ok(()) => {
                let r = reference(case);
                if out != r {
                    let at = out.iter().zip(&r).position(|(a, b)| a != b);
                    failed.push(format!(
                        "{}: {} bytes vs OpenJPH's {}, first difference at {:?}",
                        case.name,
                        out.len(),
                        r.len(),
                        at
                    ));
                }
            }
        }
        checked += 1;
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert_eq!(checked, 150);
}
