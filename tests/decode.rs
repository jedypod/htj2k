// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Decodes every codestream under `tests/data` and compares the samples
//! with what OpenJPH's decoder produced for it (`oracle/ojph_dec.cpp`).
//!
//! Each `MANIFEST.txt` gives the FNV-1a hash of the reference output of
//! every codestream; the output itself is kept, as `<name>.raw`, for all
//! of the matrix and for some of the strips.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use htj2k::codestream::Codestream;

fn data_dir(sub: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(sub)
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Decodes a codestream the way `ojph_dec` does, to its `.raw` layout:
/// the components one after the other, each sample as 4 little-endian
/// bytes.
fn decode(j2c: &[u8]) -> Result<Vec<u8>, String> {
    let mut cs = Codestream::new();
    cs.read_headers(j2c)
        .map_err(|e| format!("read_headers: {e}"))?;
    let siz = cs.access_siz();
    let nc = siz.get_num_components();
    let mut w = Vec::new();
    let mut h = Vec::new();
    let mut subsampled = false;
    for c in 0..nc {
        w.push(siz.get_recon_width(c) as usize);
        h.push(siz.get_recon_height(c) as usize);
        let d = siz.get_downsampling(c);
        subsampled |= d.x != 1 || d.y != 1;
    }
    let planar = subsampled || !cs.access_cod().is_employing_color_transform();
    cs.set_planar(planar);
    cs.create().map_err(|e| format!("create: {e}"))?;

    let mut planes: Vec<Vec<u8>> = (0..nc as usize).map(|c| vec![0; w[c] * h[c] * 4]).collect();
    let mut row = vec![0usize; nc as usize];
    let total: usize = h.iter().sum();
    for n in 0..total {
        let (c, line) = cs
            .pull()
            .map_err(|e| format!("pull {n}: {e}"))?
            .ok_or_else(|| format!("pull {n}: no more lines"))?;
        let c = c as usize;
        if c >= nc as usize || row[c] >= h[c] {
            return Err(format!("pull {n}: unexpected line of component {c}"));
        }
        let dst = &mut planes[c][row[c] * w[c] * 4..][..w[c] * 4];
        for (bytes, v) in dst.chunks_exact_mut(4).zip(&line.i32()[..w[c]]) {
            bytes.copy_from_slice(&v.to_le_bytes());
        }
        row[c] += 1;
    }
    Ok(planes.concat())
}

/// Checks every codestream listed in the manifest of `sub`, whose hash is
/// in column `hash_col`.
fn check_dir(sub: &str, hash_col: usize) {
    let dir = data_dir(sub);
    let manifest = std::fs::read_to_string(dir.join("MANIFEST.txt")).expect("manifest");
    let mut failures = String::new();
    let mut count = 0;
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('|').map(str::trim).collect();
        let name = cols[0];
        if cols[1].starts_with("REFUSED") || cols[1].starts_with("DECODE-FAILED") {
            continue;
        }
        let want_hash = u64::from_str_radix(cols[hash_col], 16).expect("hash");
        let j2c = std::fs::read(dir.join(format!("{name}.j2c"))).expect("codestream");
        count += 1;
        let got = match std::panic::catch_unwind(|| decode(&j2c)) {
            Ok(Ok(got)) => got,
            Ok(Err(e)) => {
                writeln!(failures, "{name}: {e}").unwrap();
                continue;
            }
            Err(_) => {
                writeln!(failures, "{name}: panicked").unwrap();
                continue;
            }
        };
        if fnv1a64(&got) == want_hash {
            continue;
        }
        match std::fs::read(dir.join(format!("{name}.raw"))) {
            Ok(want) if want.len() == got.len() => {
                let differing = got
                    .chunks(4)
                    .zip(want.chunks(4))
                    .filter(|(a, b)| a != b)
                    .count();
                let first = got.chunks(4).zip(want.chunks(4)).position(|(a, b)| a != b);
                writeln!(
                    failures,
                    "{name}: {differing} of {} samples differ, first at sample {first:?}",
                    got.len() / 4
                )
                .unwrap();
            }
            Ok(want) => writeln!(
                failures,
                "{name}: {} bytes, expected {}",
                got.len(),
                want.len()
            )
            .unwrap(),
            Err(_) => writeln!(failures, "{name}: hash differs").unwrap(),
        }
    }
    assert!(count > 0, "no codestreams in {sub}");
    assert!(
        failures.is_empty(),
        "{} of {count} failed:\n{failures}",
        failures.lines().count()
    );
}

/// The parameter matrix made with OpenJPH's encoder.
#[test]
fn matrix() {
    check_dir("matrix", 4);
}

/// The strips made with OpenJPH's encoder: 300 samples wide, 8 to 200
/// lines high, with 128x32 blocks, NLT type 3 and the RCT.
#[test]
fn strips() {
    check_dir("strips", 4);
}
