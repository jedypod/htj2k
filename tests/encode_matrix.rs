// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The encoder against the parameter matrix of `tests/data/matrix`: each
//! case is encoded from its `ojph_enc` command line (the image made by
//! `tests/common/synth.rs`, the parameters set by `tests/common/drive.rs`
//! as `ojph_enc` sets them) and compared with OpenJPH's codestream.

mod common;

use common::decode::decode_to_raw;
use common::drive;
use common::synth::{fnv1a64, parse_matrix_manifest, EncArgs, MatrixCase};
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
/// segment (written by `flush`) or the first SOT. The reversible cases
/// with 0 decompositions are left out: their QCD differs on purpose
/// (see gate 3).
#[test]
fn main_headers_match_openjph() {
    let mut checked = 0;
    for case in cases().iter().filter(|c| !c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        if args.levels0_reversible() {
            continue;
        }
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
    assert_eq!(checked, 193);
}

/// Gate 3: every codestream OpenJPH wrote is reproduced byte for byte,
/// except the reversible ones with 0 decompositions, where OpenJPH's QCD
/// leaves one bit-plane too few and loses the minimum sample (the three
/// MISMATCH cases). There the encoder writes one more bit-plane, so the
/// codestream must differ; that it decodes losslessly with both decoders
/// is gate 8 (`tests/encode_ours.rs`).
#[test]
fn codestreams_match_openjph() {
    let mut failed = Vec::new();
    let (mut checked, mut levels0) = (0, 0);
    for case in cases().iter().filter(|c| !c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let mut out = Vec::new();
        match drive::encode(&args, &mut out) {
            Err(e) => failed.push(format!("{}: {e}", case.name)),
            Ok(()) if args.levels0_reversible() => {
                assert!(out != reference(case), "{}: OpenJPH's bytes", case.name);
                assert!(case.is_lossless() || case.is_mismatch(), "{}", case.name);
                levels0 += 1;
            }
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
    assert_eq!((checked, levels0), (198, 5));
}

/// Gate 4: every configuration OpenJPH refused, by an exception or an
/// assertion, is refused here with an error, and nothing is appended to
/// the output. Where OpenJPH gave an error code, ours is the same.
#[test]
fn refused_cases_return_errors() {
    let mut checked = 0;
    for case in cases().iter().filter(|c| c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let prefix = b"prefix".to_vec();
        let mut out = prefix.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            drive::encode(&args, &mut out)
        }));
        let err = match result {
            Err(_) => panic!("{}: panicked", case.name),
            Ok(Ok(())) => panic!("{}: encoded what OpenJPH refused", case.name),
            Ok(Err(e)) => e,
        };
        assert_eq!(out, prefix, "{}: output changed on error", case.name);
        let ojph_code = case
            .error
            .as_deref()
            .and_then(|t| t.strip_prefix("ojph error 0x"))
            .and_then(|t| u32::from_str_radix(t.get(..8)?, 16).ok());
        if let Some(code) = ojph_code {
            assert_eq!(err.code(), code, "{}: {err}", case.name);
        }
        checked += 1;
    }
    assert_eq!(checked, 16);
}

/// Gate 5: our decoder gives back the input of every lossless case from
/// our codestream, and every lossy case to within OpenJPH's error. The
/// MISMATCH cases, which OpenJPH's own codestreams do not round-trip,
/// are lossless here too.
#[test]
fn decoder_round_trips_encoder_output() {
    let mut lossless = 0;
    let mut lossy = 0;
    for case in cases().iter().filter(|c| !c.is_refused()) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let img = args.synthesise();
        let mut out = Vec::new();
        drive::encode(&args, &mut out).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let got = decode_to_raw(&out).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let want = EncArgs::to_raw(&img);
        assert_eq!(got.len(), want.len(), "{}: decoded size", case.name);
        if case.is_lossless() || case.is_mismatch() {
            assert!(got == want, "{}: decoded image differs", case.name);
            lossless += 1;
        } else {
            let maxerr: i64 = case
                .status
                .rsplit('=')
                .next()
                .and_then(|n| n.trim().parse().ok())
                .unwrap();
            let sample = |b: &[u8]| i64::from(i32::from_le_bytes(b.try_into().unwrap()));
            let worst = got
                .chunks_exact(4)
                .zip(want.chunks_exact(4))
                .map(|(g, w)| (sample(g) - sample(w)).abs())
                .max()
                .unwrap_or(0);
            assert!(worst <= maxerr, "{}: error {worst} > {maxerr}", case.name);
            lossy += 1;
        }
    }
    assert_eq!((lossless, lossy), (166, 32));
}

/// The strips of `tests/data/strips` (`oracle/gen_strips.py`), made
/// with OpenJPH's encoder at 300x8 to 300x200 and 64x8 to 64x32 with
/// 128x32 blocks, RPCL, TLM and tile parts at resolutions, are
/// reproduced byte for byte, and decode with the crate's decoder to
/// the input (reversible) or to OpenJPH's decode (irreversible).
#[test]
fn strip_codestreams_match_openjph() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/strips");
    let text = std::fs::read_to_string(dir.join("MANIFEST.txt")).unwrap();
    let (mut lossless, mut lossy) = (0, 0);
    for case in parse_matrix_manifest(&text) {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let mut out = Vec::new();
        drive::encode(&args, &mut out).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let r = std::fs::read(dir.join(format!("{}.j2c", case.name))).unwrap();
        assert!(
            out == r,
            "{}: {} bytes vs OpenJPH's {}, first difference at {:?}",
            case.name,
            out.len(),
            r.len(),
            out.iter().zip(&r).position(|(a, b)| a != b)
        );
        let got = decode_to_raw(&out).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(
            Some(got.len()),
            case.raw_bytes,
            "{}: decoded size",
            case.name
        );
        assert_eq!(Some(fnv1a64(&got)), case.raw_fnv, "{}: decode", case.name);
        if case.is_lossless() {
            assert!(got == EncArgs::to_raw(&args.synthesise()), "{}", case.name);
            lossless += 1;
        } else {
            assert_eq!(case.status, "lossy", "{}", case.name);
            lossy += 1;
        }
    }
    assert_eq!((lossless, lossy), (11, 2));
}

/// The output vector is only appended to: a codestream written after
/// other bytes leaves them alone, and the vector can be reused, cleared
/// or not, for the next codestream.
#[test]
fn output_vector_is_appended_to_and_reusable() {
    let all = cases();
    let picked: Vec<&MatrixCase> = all
        .iter()
        .filter(|c| !c.is_refused())
        .filter(|c| {
            !EncArgs::parse_command(&c.command)
                .unwrap()
                .levels0_reversible()
        })
        .step_by(17)
        .collect();
    let mut out = b"leading bytes".to_vec();
    for case in &picked {
        let args = EncArgs::parse_command(&case.command).unwrap();
        let start = out.len();
        drive::encode(&args, &mut out).unwrap();
        assert!(out[start..] == reference(case), "{}: appended", case.name);
        assert!(out.starts_with(b"leading bytes"));
    }
    for case in &picked {
        let args = EncArgs::parse_command(&case.command).unwrap();
        out.clear();
        drive::encode(&args, &mut out).unwrap();
        assert!(out == reference(case), "{}: reused", case.name);
    }
}
