// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! A decoder that reuses the memory of earlier codestreams returns what a
//! new decoder returns, whatever came before: codestreams of the same
//! shape or of another, damaged ones, failures and memory limits.

use std::path::Path;

use htj2k::codestream::{Codestream, DecoderMemory};

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
