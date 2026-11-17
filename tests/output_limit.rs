// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! `Encoder::set_output_limit`: a codestream that fits is written byte
//! for byte as without the limit; one that does not is refused with
//! 0x00080070, by `write_headers`, `exchange` or `flush`, with nothing
//! appended to the output; and the encoder, reset, then writes what a new
//! one writes.

mod common;

use std::path::Path;

use common::drive;
use common::synth::{parse_matrix_manifest, EncArgs};
use htj2k::codestream::Encoder;

const OUTPUT_LIMIT: u32 = 0x00080070;

/// The `ojph_enc` command lines of `tests/data/matrix` and
/// `tests/data/strips` that encode.
fn cases() -> Vec<(String, EncArgs)> {
    let mut all = Vec::new();
    for sub in ["matrix", "strips"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(sub)
            .join("MANIFEST.txt");
        let text = std::fs::read_to_string(path).unwrap();
        for case in parse_matrix_manifest(&text) {
            if case.is_refused() {
                continue;
            }
            all.push((case.name, EncArgs::parse_command(&case.command).unwrap()));
        }
    }
    assert!(all.len() >= 200, "only {} cases", all.len());
    all
}

/// Where an encode stopped.
#[derive(Debug, PartialEq)]
enum Stop {
    WriteHeaders,
    /// After this many lines were handed out.
    Exchange(usize),
    Flush,
}

/// The result of encoding `a`'s image with `enc` (new or reset) after a
/// prefix of 3 bytes in the output: the codestream, or the error code and
/// where it came from, with the output checked to be as the last call
/// that succeeded left it; and the encoder.
fn encode(
    enc: Encoder,
    a: &EncArgs,
    img: &[Vec<i64>],
    limit: Option<u64>,
) -> (Result<Vec<u8>, (u32, Stop)>, Encoder) {
    let mut enc = drive::configure_on(enc, a).unwrap();
    if let Some(limit) = limit {
        enc.set_output_limit(limit).unwrap();
    }
    let mut out = b"pre".to_vec();
    let mut kept = out.clone();
    let comments = drive::comments(a).unwrap();
    let result = (|| {
        enc.write_headers(&mut out, &comments)
            .map_err(|e| (e.code(), Stop::WriteHeaders))?;
        kept.clone_from(&out);
        let sizes = a.comp_sizes();
        let mut row = vec![0usize; img.len()];
        let mut lines = 0;
        loop {
            match enc.exchange() {
                Ok(Some((c, line))) => {
                    let c = c as usize;
                    let w = sizes[c].0 as usize;
                    let src = &img[c][row[c] * w..(row[c] + 1) * w];
                    for (d, &s) in line.iter_mut().zip(src) {
                        *d = s as u32 as i32;
                    }
                    row[c] += 1;
                    lines += 1;
                }
                Ok(None) => break,
                Err(e) => return Err((e.code(), Stop::Exchange(lines))),
            }
        }
        enc.flush(&mut out).map_err(|e| (e.code(), Stop::Flush))
    })();
    if result.is_err() {
        assert_eq!(out, kept, "a failed call appended to the output");
    }
    (result.map(|()| out[3..].to_vec()), enc)
}

/// Every case: no limit, a limit of exactly its length, or more, writes
/// the same bytes; one byte less is refused, by `flush` at the latest;
/// and the encoder, reset, writes what a new one writes.
#[test]
fn limit_at_the_length_and_one_below() {
    for (name, a) in cases() {
        let img = a.synthesise();
        let (want, _) = encode(Encoder::new(), &a, &img, None);
        let want = want.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let len = want.len() as u64;
        for limit in [len, len + 1, u64::MAX] {
            let (got, _) = encode(Encoder::new(), &a, &img, Some(limit));
            assert_eq!(got.as_ref(), Ok(&want), "{name}: limit {limit}");
        }
        let (got, mut enc) = encode(Encoder::new(), &a, &img, Some(len - 1));
        let (code, _) = got.expect_err(&name);
        assert_eq!(code, OUTPUT_LIMIT, "{name}");
        enc.reset();
        let (got, mut enc) = encode(enc, &a, &img, Some(len));
        assert_eq!(got.as_ref(), Ok(&want), "{name}: after the limit was hit");
        enc.reset();
        let (got, _) = encode(enc, &a, &img, None);
        assert_eq!(got.as_ref(), Ok(&want), "{name}: reset clears the limit");
    }
}

/// Lower limits stop the encoder earlier: below the main header in
/// `write_headers`, and well below the length in `exchange`, and for
/// images with more than one row of code blocks, before the last line is
/// handed out, so that the rest of the image is not coded. (With a single
/// row, the blocks are coded when the last line is pushed.)
#[test]
fn low_limits_stop_early() {
    let mut early = 0;
    for (name, a) in cases() {
        let img = a.synthesise();
        let want = encode(Encoder::new(), &a, &img, None).0.unwrap();
        let total_lines: usize = a.comp_sizes().iter().map(|&(_, h)| h as usize).sum();
        let (got, _) = encode(Encoder::new(), &a, &img, Some(0));
        assert_eq!(got, Err((OUTPUT_LIMIT, Stop::WriteHeaders)), "{name}");
        let limit = want.len() as u64 / 3;
        let (got, _) = encode(Encoder::new(), &a, &img, Some(limit));
        let (code, stop) = got.expect_err(&name);
        assert_eq!(code, OUTPUT_LIMIT, "{name}");
        if matches!(stop, Stop::Exchange(lines) if lines < total_lines) {
            early += 1;
        }
    }
    assert!(
        early > 50,
        "only {early} cases stopped before the last line"
    );
}

/// The limit is a setter like the others: refused after `write_headers`.
#[test]
fn set_after_write_headers_is_an_error() {
    let (_, a) = &cases()[0];
    let mut enc = drive::configure(a).unwrap();
    enc.write_headers(&mut Vec::new(), &[]).unwrap();
    let err = enc.set_output_limit(10).unwrap_err();
    assert_eq!(err.code(), 0x0008000B);
}
