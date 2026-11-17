// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! A decoder that reuses the memory of earlier codestreams returns what a
//! new decoder returns, and an encoder reset between codestreams writes
//! what a new encoder writes, whatever came before: codestreams of the
//! same shape or of another, damaged ones, failures and memory limits.

mod common;

use std::path::Path;

use common::drive;
use common::synth::{parse_matrix_manifest, EncArgs};
use htj2k::codestream::{Codestream, DecoderMemory, Encoder};

/// Every codestream of `tests/data/matrix` and `tests/data/strips`.
fn codestreams() -> Vec<(String, Vec<u8>)> {
    let mut all = Vec::new();
    for sub in ["matrix", "strips"] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(sub);
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .expect("test data directory")
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "j2c"))
            .collect();
        names.sort();
        for path in names {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            all.push((name, std::fs::read(&path).unwrap()));
        }
    }
    assert!(all.len() >= 200, "only {} codestreams found", all.len());
    all
}

/// What decoding gives: every line pulled, with its component, and the
/// error code that stopped it, if one did.
#[derive(Debug, PartialEq)]
struct Decoded {
    lines: Vec<(u32, Vec<i32>)>,
    error: Option<u32>,
}

/// How to decode: the memory limit, resilience, skipped resolutions, and
/// whether to pull a codestream without the colour transform
/// interleaved rather than planar.
#[derive(Debug, Clone, Copy, Default)]
struct Options {
    limit: Option<u64>,
    resilient: bool,
    skipped: u32,
    flip_planar: bool,
}

/// Decodes `data` with `cs`.
fn decode<'a>(mut cs: Codestream<'a>, data: &'a [u8], o: Options) -> (Decoded, DecoderMemory) {
    let mut lines = Vec::new();
    let mut run = || -> htj2k::Result<()> {
        if o.resilient {
            cs.enable_resilience()?;
        }
        if let Some(limit) = o.limit {
            cs.set_memory_limit(limit);
        }
        cs.read_headers(data)?;
        if o.skipped > 0 {
            cs.restrict_input_resolution(o.skipped, o.skipped)?;
        }
        if o.flip_planar && !cs.access_cod().is_employing_color_transform() {
            cs.set_planar(!cs.is_planar());
        }
        cs.create()?;
        while let Some((c, line)) = cs.pull()? {
            lines.push((c, line.i32()[..line.size].to_vec()));
        }
        Ok(())
    };
    let error = run().err().map(|e| e.code());
    (Decoded { lines, error }, cs.into_memory())
}

/// What a new decoder gives.
fn fresh(data: &[u8], o: Options) -> Decoded {
    decode(Codestream::new(), data, o).0
}

/// A cut-short and a damaged copy of `data`.
fn broken_copies(data: &[u8]) -> [Vec<u8>; 2] {
    let cut = data[..data.len() * 3 / 5].to_vec();
    let mut bad = data.to_vec();
    for i in (bad.len() / 3..bad.len().saturating_sub(2)).step_by(97) {
        bad[i] ^= 0x5A;
    }
    [cut, bad]
}

/// Every codestream, then the same again, then one of another shape,
/// through one memory: each decode is what a new decoder gives.
#[test]
fn sequences_through_one_memory_decode_as_new() {
    let all = codestreams();
    let n = all.len();
    let mut mem = DecoderMemory::new();
    for i in 0..n {
        // the same codestream twice, pulled planar and then interleaved
        // (the order of the lines is not part of the kept tree), then
        // another
        for (k, j) in [i, i, (i * 7 + 3) % n].into_iter().enumerate() {
            let (name, data) = &all[j];
            let o = Options {
                skipped: (j % 3 == 0) as u32,
                flip_planar: k == 1,
                ..Options::default()
            };
            let (got, m) = decode(Codestream::with_memory(mem), data, o);
            mem = m;
            assert_eq!(got, fresh(data, o), "{name} after {}", all[i].0);
        }
    }
}

/// A cut or damaged codestream, decoded resiliently or not, leaves
/// nothing behind that changes the next one, of the same shape or not.
#[test]
fn corrupt_then_valid_decodes_as_new() {
    let all = codestreams();
    for (i, (name, data)) in all.iter().enumerate() {
        let other = &all[(i + 1) % all.len()].1;
        for broken in broken_copies(data) {
            for resilient in [false, true] {
                let o = Options {
                    resilient,
                    ..Options::default()
                };
                let (got, mem) = decode(Codestream::new(), &broken, o);
                assert_eq!(got, fresh(&broken, o), "{name}");
                let (got, mem) = decode(Codestream::with_memory(mem), data, o);
                assert_eq!(got, fresh(data, o), "{name} after a broken copy");
                let (got, _) = decode(Codestream::with_memory(mem), other, o);
                assert_eq!(got, fresh(other, o), "the next after {name}");
            }
        }
    }
}

/// The smallest memory limit with which a new decoder decodes `data`.
fn smallest_limit(data: &[u8]) -> u64 {
    let ok = |limit| {
        let o = Options {
            limit: Some(limit),
            ..Options::default()
        };
        fresh(data, o).error.is_none()
    };
    let (mut lo, mut hi) = (0u64, 1 << 32);
    assert!(ok(hi));
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// A reused decoder accepts and refuses exactly the limits a new one
/// does, whether its memory was built by the same codestream (with a
/// larger limit) or by another.
#[test]
fn memory_limit_holds_with_reused_memory() {
    let all = codestreams();
    let names = [
        "s16_rct_4c_300x200.j2c",
        "s32_rct_4c_300x32.j2c",
        "tiles_off_61x47.j2c",
        "blk4x4_37x29.j2c",
    ];
    let pick = |n: &str| &all.iter().find(|(name, _)| name == n).unwrap().1;
    for name in names {
        let data = pick(name);
        let need = smallest_limit(data);
        for prior in names {
            for (limit, ok) in [(need - 1, false), (need, true), (1 << 32, true)] {
                let (_, mem) = decode(Codestream::new(), pick(prior), Options::default());
                let o = Options {
                    limit: Some(limit),
                    ..Options::default()
                };
                let (got, mem) = decode(Codestream::with_memory(mem), data, o);
                assert_eq!(got, fresh(data, o), "{name} after {prior}, limit {limit}");
                let want = if ok { None } else { Some(0x00070001) };
                assert_eq!(got.error, want, "{name} after {prior}, limit {limit}");
                // and the memory still serves after a refusal
                let (got, _) = decode(Codestream::with_memory(mem), data, Options::default());
                assert_eq!(got, fresh(data, Options::default()), "{name} again");
            }
        }
    }
}

/// `reset` decodes inputs that share one lifetime, and restores the
/// settings of a new decoder: the memory limit and resilience.
#[test]
fn reset_decodes_as_new() {
    let all = codestreams();
    let joined: Vec<u8> = all.iter().take(40).flat_map(|(_, d)| d.clone()).collect();
    let mut cs = Codestream::new();
    let mut at = 0;
    for (name, data) in all.iter().take(40) {
        let slice = &joined[at..at + data.len()];
        at += data.len();
        cs.reset();
        // a tiny limit or resilience set before reset is gone after it
        cs.set_memory_limit(1);
        cs.enable_resilience().unwrap();
        cs.reset();
        assert!(cs.enable_resilience().is_ok());
        cs.reset();
        cs.read_headers(slice).unwrap();
        cs.create().unwrap();
        let mut lines = Vec::new();
        while let Some((c, line)) = cs.pull().unwrap() {
            lines.push((c, line.i32()[..line.size].to_vec()));
        }
        assert_eq!(
            Decoded { lines, error: None },
            fresh(data, Options::default()),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------
// The encoder
// ---------------------------------------------------------------------

/// The `ojph_enc` command lines of `tests/data/matrix` and
/// `tests/data/strips`, refused ones included.
fn enc_cases() -> Vec<(String, EncArgs)> {
    let mut all = Vec::new();
    for sub in ["matrix", "strips"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(sub)
            .join("MANIFEST.txt");
        let text = std::fs::read_to_string(path).unwrap();
        for case in parse_matrix_manifest(&text) {
            if let Ok(a) = EncArgs::parse_command(&case.command) {
                all.push((case.name, a));
            }
        }
    }
    assert!(all.len() >= 200, "only {} cases", all.len());
    all
}

/// How far to take an encoder, and with what memory limit.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Run {
    /// Every line, and `flush`.
    Whole,
    /// Every line, without writing to them, and `flush`.
    Unfilled,
    /// The first 3 lines, then abandoned.
    Abandoned,
}

/// Encodes `a`'s image with `enc` (new or reset), as far as `run` says
/// and with the memory limit `limit` if given: the codestream, or the
/// error code, and the encoder.
fn encode(
    enc: Encoder,
    a: &EncArgs,
    img: &[Vec<i64>],
    run: Run,
    limit: Option<u64>,
) -> (Result<Vec<u8>, u32>, Encoder) {
    let mut out = Vec::new();
    let mut kept = None;
    let go = || -> htj2k::Result<()> {
        let mut e = drive::configure_on(enc, a)?;
        let r = (|| {
            if let Some(limit) = limit {
                e.set_memory_limit(limit)?;
            }
            e.write_headers(&mut out, &drive::comments(a)?)?;
            match run {
                Run::Whole => drive::push_image(&mut e, a, img)?,
                Run::Unfilled => while e.exchange()?.is_some() {},
                Run::Abandoned => {
                    for _ in 0..3 {
                        e.exchange()?;
                    }
                    return Ok(());
                }
            }
            e.flush(&mut out)
        })();
        kept = Some(e);
        r
    };
    let r = go().map(|()| out.clone()).map_err(|e| e.code());
    (r, kept.unwrap_or_default())
}

/// What a new encoder gives.
fn enc_fresh(a: &EncArgs, img: &[Vec<i64>], run: Run, limit: Option<u64>) -> Result<Vec<u8>, u32> {
    encode(Encoder::new(), a, img, run, limit).0
}

/// Every case, then the same again, then one of another shape, through
/// one encoder reset between them: each is what a new encoder writes,
/// or the same error. The parameters, the version comment, TLM, tile
/// parts and planar exchange differ from case to case, so any setting
/// that outlived `reset` would show.
#[test]
fn encoder_sequences_through_one_encoder_write_as_new() {
    let all = enc_cases();
    let images: Vec<_> = all.iter().map(|(_, a)| a.synthesise()).collect();
    let n = all.len();
    let mut enc = Encoder::new();
    for i in 0..n {
        for j in [i, i, (i * 7 + 3) % n] {
            let (name, a) = &all[j];
            let run = if j % 5 == 4 {
                Run::Unfilled
            } else {
                Run::Whole
            };
            enc.reset();
            let (got, e) = encode(enc, a, &images[j], run, None);
            enc = e;
            assert_eq!(
                got,
                enc_fresh(a, &images[j], run, None),
                "{name} after {}",
                all[i].0
            );
        }
    }
}

/// An encoder abandoned part way, or failed on its memory limit, writes
/// what a new one writes after `reset`, for the same case or another.
#[test]
fn encoder_failed_then_valid_writes_as_new() {
    let all = enc_cases();
    for (i, (name, a)) in all.iter().enumerate().step_by(3) {
        let img = a.synthesise();
        let want = enc_fresh(a, &img, Run::Whole, None);
        let (other_name, other) = &all[(i + 1) % all.len()];
        let other_img = other.synthesise();
        for (first, first_img) in [(a, &img), (other, &other_img)] {
            for (run, limit) in [(Run::Abandoned, None), (Run::Whole, Some(2000))] {
                let (_, mut enc) = encode(Encoder::new(), first, first_img, run, limit);
                enc.reset();
                let (got, _) = encode(enc, a, &img, Run::Whole, None);
                assert_eq!(
                    got, want,
                    "{name} after {run:?} {limit:?} of it or {other_name}"
                );
            }
        }
    }
}

/// The smallest memory limit with which a new encoder encodes `a`.
fn enc_smallest_limit(a: &EncArgs, img: &[Vec<i64>]) -> u64 {
    let ok = |limit| enc_fresh(a, img, Run::Whole, Some(limit)).is_ok();
    let (mut lo, mut hi) = (0u64, 1 << 32);
    assert!(ok(hi));
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// A reset encoder accepts and refuses exactly the limits a new one
/// does, whether its buffers were built by the same case (with a larger
/// limit) or by another, and still serves after a refusal.
#[test]
fn encoder_memory_limit_holds_after_reset() {
    let all = enc_cases();
    let names = [
        "s16_rct_4c_300x200",
        "s32_rct_4c_300x32",
        "tiles_off_61x47",
        "irv_rct_tiles_prec",
    ];
    let pick = |n: &str| &all.iter().find(|(name, _)| name == n).unwrap().1;
    for name in names {
        let a = pick(name);
        let img = a.synthesise();
        let need = enc_smallest_limit(a, &img);
        for prior in names {
            let p = pick(prior);
            let p_img = p.synthesise();
            for (limit, ok) in [(need - 1, false), (need, true)] {
                let (_, mut enc) = encode(Encoder::new(), p, &p_img, Run::Whole, None);
                enc.reset();
                let (got, mut enc) = encode(enc, a, &img, Run::Whole, Some(limit));
                assert_eq!(got.is_ok(), ok, "{name} after {prior}, limit {limit}");
                assert_eq!(got, enc_fresh(a, &img, Run::Whole, Some(limit)), "{name}");
                enc.reset();
                let (got, _) = encode(enc, a, &img, Run::Whole, None);
                assert_eq!(got, enc_fresh(a, &img, Run::Whole, None), "{name} again");
            }
        }
    }
}
