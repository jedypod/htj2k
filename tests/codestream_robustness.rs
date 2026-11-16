// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Feeds damaged codestreams to the decoder. Whatever the bytes are, it
//! must return, with lines or with an error, and must not panic.
//!
//! The inputs are the codestreams under `tests/data`, truncated, with
//! bits flipped and with bytes overwritten, decoded with and without
//! resilience. Run it in a debug build, so that arithmetic overflow and
//! `debug_assert!` count as failures too.

use std::path::Path;

use htj2k::codestream::Codestream;

/// Enough for every test image, and little enough that a damaged SIZ
/// asking for a huge image is turned away quickly.
const MEMORY_LIMIT: u64 = 64 << 20;

/// A damaged SIZ can describe a very tall image that still fits the memory
/// limit; this many of its lines are enough.
const MAX_PULLS: usize = 1 << 16;

struct Rng(u64);

impl Rng {
    /// splitmix64
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Decodes as far as the codestream allows. Returns the number of lines
/// pulled; errors are expected and ignored.
fn decode(j2c: &[u8], resilient: bool, skipped: u32, planar: Option<bool>) -> usize {
    let mut cs = Codestream::new();
    if resilient {
        cs.enable_resilience().unwrap();
    }
    cs.set_memory_limit(MEMORY_LIMIT);
    if cs.read_headers(j2c).is_err() {
        return 0;
    }
    if skipped > 0 && cs.restrict_input_resolution(skipped, skipped).is_err() {
        return 0;
    }
    if let Some(planar) = planar {
        cs.set_planar(planar);
    }
    if cs.create().is_err() {
        return 0;
    }
    let siz = cs.access_siz();
    let total: u64 = (0..siz.get_num_components())
        .map(|c| u64::from(siz.get_recon_height(c)))
        .sum();
    let mut lines = 0;
    while lines < MAX_PULLS {
        match cs.pull() {
            Ok(Some(_)) => lines += 1,
            _ => break,
        }
        assert!(
            lines as u64 <= total,
            "pull returned more lines than the image has"
        );
    }
    lines
}

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
    assert!(all.len() >= 160, "only {} codestreams found", all.len());
    all
}

/// Runs `decode` on every mutation `mutate` makes of every codestream,
/// `rounds` times each, and reports the mutations that panic.
fn check(rounds: usize, mutate: impl Fn(&mut Rng, &[u8]) -> Vec<u8>) {
    // HTJ2K_ROBUSTNESS_ROUNDS multiplies the number of rounds, for a longer
    // run than the default.
    let scale = std::env::var("HTJ2K_ROBUSTNESS_ROUNDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let rounds = rounds * scale;
    let mut failures = Vec::new();
    let mut runs = 0;
    for (index, (name, j2c)) in codestreams().iter().enumerate() {
        let mut rng = Rng(index as u64);
        // large streams take longer to decode, so they get fewer rounds
        let rounds = if j2c.len() > 20_000 {
            rounds / 8 + 1
        } else {
            rounds
        };
        for round in 0..rounds {
            let bytes = mutate(&mut rng, j2c);
            let resilient = rng.below(2) == 1;
            let skipped = if rng.below(4) == 0 {
                rng.below(4) as u32
            } else {
                0
            };
            let planar = [None, Some(false), Some(true)][rng.below(3)];
            runs += 1;
            let result = std::panic::catch_unwind(|| decode(&bytes, resilient, skipped, planar));
            if result.is_err() {
                failures.push(format!(
                    "{name} round {round} (resilient {resilient}, skipped {skipped}, planar {planar:?})"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {runs} runs panicked:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn undamaged_streams_with_other_options() {
    // skipped resolutions, resilience and both line orders on good data
    check(6, |_, j2c| j2c.to_vec());
}

#[test]
fn truncated_streams() {
    check(24, |rng, j2c| j2c[..rng.below(j2c.len())].to_vec());
}

#[test]
fn bit_flips() {
    check(48, |rng, j2c| {
        let mut bytes = j2c.to_vec();
        for _ in 0..1 + rng.below(3) {
            // half of the flips go into the headers, where they do the
            // most varied damage
            let span = if rng.below(2) == 0 {
                bytes.len().min(160)
            } else {
                bytes.len()
            };
            let at = rng.below(span);
            bytes[at] ^= 1 << rng.below(8);
        }
        bytes
    });
}

#[test]
fn overwritten_bytes() {
    check(48, |rng, j2c| {
        let mut bytes = j2c.to_vec();
        let span = if rng.below(2) == 0 {
            bytes.len().min(160)
        } else {
            bytes.len()
        };
        let at = rng.below(span);
        let len = (1 + rng.below(4)).min(bytes.len() - at);
        for b in &mut bytes[at..at + len] {
            *b = match rng.below(4) {
                0 => 0x00,
                1 => 0xFF,
                _ => rng.next() as u8,
            };
        }
        bytes
    });
}

/// A colour-transformed codestream whose third component needs 64-bit
/// samples while the first two need 32. The main COD's colour transform
/// makes the first three components share one width only for components
/// with a COC of their own, so a QCC alone can split them. OpenJPH asserts
/// here; the port must return an error, not panic in the inverse RCT.
#[test]
fn colour_components_of_different_widths() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/matrix/prog_lrcp.j2c");
    let mut j2c = std::fs::read(path).unwrap();
    // turn on the colour transform in the COD: Lcod, Scod, progression
    // order and number of layers come before the MCT byte
    let cod = j2c
        .windows(2)
        .position(|w| w == [0xFF, 0x52])
        .expect("COD marker");
    assert_eq!(j2c[cod + 8], 0, "expected no colour transform");
    j2c[cod + 8] = 1;
    // find the QCD marker segment in the main header
    let qcd = j2c
        .windows(2)
        .position(|w| w == [0xFF, 0x5C])
        .expect("QCD marker");
    let len = usize::from(u16::from_be_bytes([j2c[qcd + 2], j2c[qcd + 3]]));
    let body = &j2c[qcd + 4..qcd + 2 + len];
    assert_eq!(body[0] & 0x1F, 0, "expected a reversible QCD");
    // QCC for component 2: Lqcc, Cqcc, then the QCD's Sqcd and exponents,
    // each exponent raised to the 5-bit maximum so K_max exceeds 30
    let mut qcc = vec![0xFF, 0x5D];
    qcc.extend_from_slice(&(len as u16 + 1).to_be_bytes());
    qcc.push(2);
    qcc.push(body[0]);
    qcc.extend(body[1..].iter().map(|&e| e | 0xF8));
    let mut damaged = j2c[..qcd + 2 + len].to_vec();
    damaged.extend_from_slice(&qcc);
    damaged.extend_from_slice(&j2c[qcd + 2 + len..]);

    let mut cs = Codestream::new();
    cs.read_headers(&damaged).expect("headers");
    let err = cs.create().expect_err("create must refuse the codestream");
    assert_eq!(err.code(), 0x00070026, "{err}");
}
