// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Golden-vector tests for the HT block encoder: every record in
//! `tests/data/encoder/*.ojbe` (format: `tests/data/encoder/README.md`) is
//! encoded again and must give OpenJPH's bytes and length, or its error.
//! Every block that encodes is then decoded with the crate's own block
//! decoder, which must give back the coded part of the input samples.

use htj2k::coding::{
    decode_codeblock32, decode_codeblock64, encode_codeblock32, encode_codeblock64,
};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/encoder")
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[derive(Debug)]
struct Header {
    bits: u8,
    failed: bool,
    missing_msbs: u32,
    num_passes: u32,
    width: u32,
    height: u32,
    stride: u32,
    length: u32,
    coded_len: usize,
    error_code: u32,
}

fn header(b: &[u8]) -> Header {
    assert_eq!(&b[0..4], b"OJBE", "bad magic");
    Header {
        bits: b[4],
        failed: b[5] != 0,
        missing_msbs: u32_at(b, 8),
        num_passes: u32_at(b, 12),
        width: u32_at(b, 16),
        height: u32_at(b, 20),
        stride: u32_at(b, 24),
        length: u32_at(b, 28),
        coded_len: u32_at(b, 32) as usize,
        error_code: u32_at(b, 36),
    }
}

/// Lays the packed samples out with the recorded stride; the padding gets
/// garbage, which the encoder must not read.
fn with_stride<T: Copy>(h: &Header, packed: &[T], garbage: impl Fn(usize) -> T) -> Vec<T> {
    let (w, st) = (h.width as usize, h.stride as usize);
    let rows = (h.height as usize).max(1);
    let mut buf: Vec<T> = (0..(rows - 1) * st + w).map(garbage).collect();
    for y in 0..h.height as usize {
        buf[y * st..y * st + w].copy_from_slice(&packed[y * w..(y + 1) * w]);
    }
    buf
}

#[derive(Default, Debug)]
struct Stats {
    matched: usize,
    errors: usize,
    round_trips: usize,
    /// blocks beyond what the decoder can read back (64-bit only)
    not_decodable: usize,
}

/// The largest `missing_msbs` for which the 64-bit decoder can decode any
/// cleanup pass (see `run64`).
const MAX_DECODABLE_MISSING_MSBS64: u32 = 55;

/// Checks one record against the encoder: same error, or same length and
/// bytes. Returns the coded bytes on success.
fn check_encode(
    what: &str,
    h: &Header,
    coded_ref: &[u8],
    res: htj2k::Result<u32>,
    out: Vec<u8>,
    stats: &mut Stats,
) -> Option<Vec<u8>> {
    if h.failed {
        let e = res.expect_err(&format!(
            "{what}: OpenJPH raised an error, the port did not"
        ));
        assert!(out.is_empty(), "{what}: output appended on error");
        if h.error_code != 0 {
            assert_eq!(e.code(), h.error_code, "{what}: error code ({e})");
        }
        stats.errors += 1;
        return None;
    }
    let n = res.unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(n, h.length, "{what}: length");
    if out != coded_ref {
        let i = out.iter().zip(coded_ref).position(|(a, b)| a != b);
        panic!(
            "{what}: coded bytes differ (lengths {} / {}, first difference at {i:?})",
            out.len(),
            coded_ref.len()
        );
    }
    stats.matched += 1;
    Some(out)
}

fn run32(what: &str, h: &Header, samples: &[u8], coded_ref: &[u8], stats: &mut Stats) {
    let packed: Vec<u32> = (0..samples.len() / 4)
        .map(|i| u32_at(samples, 4 * i))
        .collect();
    let buf = with_stride(h, &packed, |i| {
        0x5A5A_5A5A ^ (i as u32).wrapping_mul(0x9E37_79B9)
    });
    // a non-empty Vec, so that appending (not overwriting) is checked
    let mut out = vec![0xEEu8; 3];
    let res = encode_codeblock32(
        &buf,
        h.missing_msbs,
        h.num_passes,
        h.width,
        h.height,
        h.stride,
        &mut out,
    );
    assert_eq!(&out[..3], &[0xEE; 3], "{what}: existing bytes changed");
    out.drain(..3);
    let Some(coded) = check_encode(what, h, coded_ref, res, out, stats) else {
        return;
    };

    // round trip through the crate's decoder
    let (w, ht) = (h.width as usize, h.height as usize);
    let mut dec = vec![0u32; w * ht];
    let ok = decode_codeblock32(
        &coded,
        &mut dec,
        h.missing_msbs,
        1,
        coded.len() as u32,
        0,
        h.width,
        h.height,
        h.width,
        false,
    );
    assert!(ok, "{what}: the decoder rejected the encoder's output");
    let p = 30 - h.missing_msbs;
    for (i, (&t, &d)) in packed.iter().zip(&dec).enumerate() {
        let mu = (t & 0x7FFF_FFFF) >> p;
        let want = if mu == 0 {
            0
        } else {
            (t & 0x8000_0000) | ((2 * mu + 1) << (p - 1))
        };
        assert_eq!(
            d,
            want,
            "{what}: round trip of sample ({}, {})",
            i % w,
            i / w
        );
    }
    stats.round_trips += 1;
}

fn run64(what: &str, h: &Header, samples: &[u8], coded_ref: &[u8], stats: &mut Stats) {
    let packed: Vec<u64> = (0..samples.len() / 8)
        .map(|i| u64_at(samples, 8 * i))
        .collect();
    let buf = with_stride(h, &packed, |i| {
        0x5A5A_5A5A_5A5A_5A5A ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
    });
    let mut out = vec![0xEEu8; 3];
    let res = encode_codeblock64(
        &buf,
        h.missing_msbs,
        h.num_passes,
        h.width,
        h.height,
        h.stride,
        &mut out,
    );
    assert_eq!(&out[..3], &[0xEE; 3], "{what}: existing bytes changed");
    out.drain(..3);
    let Some(coded) = check_encode(what, h, coded_ref, res, out, stats) else {
        return;
    };

    // OpenJPH's 64-bit decoder, and so the port, refills its MagSgn window
    // only to 57 bits per sample (`frwd_fetch64`), while a sample takes up
    // to `missing_msbs + 2` bits; above 55 the largest magnitudes cannot
    // be decoded. Those blocks are checked against OpenJPH's bytes only.
    if h.missing_msbs > MAX_DECODABLE_MISSING_MSBS64 {
        stats.not_decodable += 1;
        return;
    }
    let (w, ht) = (h.width as usize, h.height as usize);
    let mut dec = vec![0u64; w * ht];
    let ok = decode_codeblock64(
        &coded,
        &mut dec,
        h.missing_msbs,
        1,
        coded.len() as u32,
        0,
        h.width,
        h.height,
        h.width,
        false,
    );
    assert!(ok, "{what}: the decoder rejected the encoder's output");
    let p = 62 - h.missing_msbs;
    for (i, (&t, &d)) in packed.iter().zip(&dec).enumerate() {
        let mu = (t & 0x7FFF_FFFF_FFFF_FFFF) >> p;
        let want = if mu == 0 {
            0
        } else {
            (t & 0x8000_0000_0000_0000) | ((2 * mu + 1) << (p - 1))
        };
        assert_eq!(
            d,
            want,
            "{what}: round trip of sample ({}, {})",
            i % w,
            i / w
        );
    }
    stats.round_trips += 1;
}

#[test]
fn golden_encoder_records() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ojbe"))
        .collect();
    files.sort();
    let mut s32 = Stats::default();
    let mut s64 = Stats::default();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        let (mut off, mut idx) = (0, 0);
        while off < data.len() {
            let h = header(&data[off..]);
            let sz = h.bits as usize / 8;
            let ns = h.width as usize * h.height as usize * sz;
            let samples = &data[off + 40..off + 40 + ns];
            let coded = &data[off + 40 + ns..off + 40 + ns + h.coded_len];
            let what = format!(
                "{name} record {idx} ({}-bit {}x{} stride {} mmsb {})",
                h.bits, h.width, h.height, h.stride, h.missing_msbs
            );
            match h.bits {
                32 => run32(&what, &h, samples, coded, &mut s32),
                64 => run64(&what, &h, samples, coded, &mut s64),
                b => panic!("{what}: bits {b}"),
            }
            off += 40 + ns + h.coded_len;
            idx += 1;
        }
    }
    eprintln!("32-bit: {s32:?}");
    eprintln!("64-bit: {s64:?}");
    // guard against vectors silently going missing
    assert!(s32.matched >= 500 && s32.errors >= 3, "{s32:?}");
    assert!(s64.matched >= 400 && s64.errors >= 3, "{s64:?}");
    assert_eq!(s32.matched, s32.round_trips);
    assert_eq!(s64.matched, s64.round_trips + s64.not_decodable);
    assert!(s64.not_decodable < 50, "{s64:?}");
}

/// The arguments the C++ asserts on or mishandles are refused, with
/// nothing appended.
#[test]
fn argument_errors32() {
    let buf = vec![0x4000_0000u32; 64 * 64];
    let code = |mm, np, w, h, st, len: usize| {
        let mut out = Vec::new();
        let r = encode_codeblock32(&buf[..len], mm, np, w, h, st, &mut out);
        assert_eq!(out.len(), *r.as_ref().unwrap_or(&0) as usize);
        r.map_err(|e| e.code())
    };
    assert_eq!(code(0, 2, 8, 8, 8, 64), Err(0x00020010));
    assert_eq!(code(0, 0, 8, 8, 8, 64), Err(0x00020010));
    assert_eq!(code(30, 1, 8, 8, 8, 64), Err(0x00020011));
    assert_eq!(code(0, 1, 1025, 1, 1025, 1025), Err(0x00020012));
    assert_eq!(code(0, 1, 8, 8, 8, 63), Err(0x00020013));
    assert_eq!(code(0, 1, 8, 8, 9, 70), Err(0x00020013));
    assert!(code(0, 1, 8, 8, 9, 71).is_ok());
    // height 0 reads row 0 only; width 0 reads nothing
    assert_eq!(code(0, 1, 8, 0, 8, 7), Err(0x00020013));
    assert!(code(0, 1, 8, 0, 8, 8).is_ok());
    assert!(code(0, 1, 0, 8, 8, 0).is_ok());
    // the C++ never returns for this one (its row counter wraps)
    assert!(code(0, 1, 0, u32::MAX, 0, 0).is_ok());
    // stride below width is allowed: rows overlap
    assert!(code(29, 1, 8, 8, 4, 36).is_ok());
}

#[test]
fn argument_errors64() {
    let buf = vec![0x4000_0000_0000_0000u64; 64 * 64];
    let code = |mm, np, w, h, st, len: usize| {
        let mut out = Vec::new();
        let r = encode_codeblock64(&buf[..len], mm, np, w, h, st, &mut out);
        assert_eq!(out.len(), *r.as_ref().unwrap_or(&0) as usize);
        r.map_err(|e| e.code())
    };
    assert_eq!(code(0, 3, 8, 8, 8, 64), Err(0x00020010));
    assert_eq!(code(62, 1, 8, 8, 8, 64), Err(0x00020011));
    assert!(code(61, 1, 8, 8, 8, 64).is_ok());
    assert_eq!(code(0, 1, 2048, 2, 2048, 4096), Err(0x00020012));
    assert_eq!(code(0, 1, 64, 64, 64, 4095), Err(0x00020013));
    assert!(code(0, 1, 64, 64, 64, 4096).is_ok());
    assert!(code(0, 1, 0, u32::MAX, 7, 0).is_ok());
}
