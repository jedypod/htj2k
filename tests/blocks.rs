// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Golden-vector tests for the HT block decoders: every record in
//! `tests/data/blocks/*.ojbk` (format: `tests/data/blocks/README.md`) and
//! `tests/data/blocks/synth/*.ojbh` (format: `synth/README.md`) is decoded
//! and compared with what OpenJPH's generic C++ decoder produced.

use htj2k::coding::{decode_codeblock32, decode_codeblock64};
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/blocks")
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o],
        b[o + 1],
        b[o + 2],
        b[o + 3],
        b[o + 4],
        b[o + 5],
        b[o + 6],
        b[o + 7],
    ])
}

fn fnv1a64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

struct Header {
    magic: [u8; 4],
    bits: u8,
    stripe_causal: bool,
    result: bool,
    missing_msbs: u32,
    num_passes: u32,
    lengths1: u32,
    lengths2: u32,
    width: u32,
    height: u32,
    stride: u32,
    coded_len: usize,
}

fn header(b: &[u8]) -> Header {
    Header {
        magic: [b[0], b[1], b[2], b[3]],
        bits: b[4],
        stripe_causal: b[5] != 0,
        result: b[6] != 0,
        missing_msbs: u32_at(b, 8),
        num_passes: u32_at(b, 12),
        lengths1: u32_at(b, 16),
        lengths2: u32_at(b, 20),
        width: u32_at(b, 24),
        height: u32_at(b, 28),
        stride: u32_at(b, 32),
        coded_len: u32_at(b, 36) as usize,
    }
}

#[derive(Default, Debug)]
struct Stats {
    records: usize,
    dec32: usize,
    dec64: usize,
    passes: [usize; 4],
    false_results: usize,
    causal: usize,
}

impl Stats {
    fn count(&mut self, h: &Header) {
        self.records += 1;
        if h.bits == 32 {
            self.dec32 += 1;
        } else {
            self.dec64 += 1;
        }
        self.passes[(h.num_passes as usize).min(3)] += 1;
        self.false_results += !h.result as usize;
        self.causal += h.stripe_causal as usize;
    }
}

/// Decodes with the Rust port into a buffer of `rows` rows of `stride`
/// samples, filled with zeros; returns (result, buffer as LE bytes).
fn run(h: &Header, coded: &[u8], rows: usize) -> (bool, Vec<u8>) {
    let n = rows * h.stride as usize;
    if h.bits == 32 {
        let mut out = vec![0u32; n];
        let r = decode_codeblock32(
            coded,
            &mut out,
            h.missing_msbs,
            h.num_passes,
            h.lengths1,
            h.lengths2,
            h.width,
            h.height,
            h.stride,
            h.stripe_causal,
        );
        (r, out.iter().flat_map(|v| v.to_le_bytes()).collect())
    } else {
        let mut out = vec![0u64; n];
        let r = decode_codeblock64(
            coded,
            &mut out,
            h.missing_msbs,
            h.num_passes,
            h.lengths1,
            h.lengths2,
            h.width,
            h.height,
            h.stride,
            h.stripe_causal,
        );
        (r, out.iter().flat_map(|v| v.to_le_bytes()).collect())
    }
}

/// `OJBK` records: the samples of the `height` x `width` block follow the
/// coded bytes. The port gets a buffer of exactly `height` rows (it must
/// drop the C++'s writes to row `height` for odd heights).
fn check_ojbk(name: &str, idx: usize, h: &Header, rec: &[u8], coded: &[u8]) {
    let sz = h.bits as usize / 8;
    let (r, out) = run(h, coded, h.height as usize);
    assert_eq!(r, h.result, "{name} record {idx}: return value");
    if !h.result {
        return;
    }
    let samples = &rec[40 + h.coded_len..];
    let row_bytes = h.width as usize * sz;
    for y in 0..h.height as usize {
        let got = &out[y * h.stride as usize * sz..][..row_bytes];
        let want = &samples[y * row_bytes..][..row_bytes];
        if got != want {
            let x = (0..h.width as usize)
                .find(|&x| got[x * sz..][..sz] != want[x * sz..][..sz])
                .unwrap();
            panic!(
                "{name} record {idx} ({}-bit, {}x{}, mmsb {}, passes {}): first mismatch at ({x}, {y})",
                h.bits, h.width, h.height, h.missing_msbs, h.num_passes
            );
        }
    }
}

/// `OJBH` records (`tests/data/blocks/synth/synth.ojbh`: synthetic calls,
/// see the README there): the C++ was given a zero-filled buffer of `height + 4` rows of
/// `stride` samples, and the record stores the FNV-1a-64 of that whole
/// buffer after the call (8 bytes), which therefore also covers writes the
/// C++ made outside the block, and partial output when it returned false.
fn check_ojbh(name: &str, idx: usize, h: &Header, rec: &[u8], coded: &[u8]) {
    let (r, out) = run(h, coded, h.height as usize + 4);
    assert_eq!(
        r,
        h.result,
        "{name} record {idx}: return value ({h_dbg})",
        h_dbg = dbg_header(h)
    );
    let want = u64_at(rec, 40 + h.coded_len);
    assert_eq!(
        fnv1a64(out),
        want,
        "{name} record {idx}: buffer hash ({})",
        dbg_header(h)
    );
}

fn dbg_header(h: &Header) -> String {
    format!(
        "{}-bit {}x{} stride {} mmsb {} passes {} l1 {} l2 {} causal {} result {}",
        h.bits,
        h.width,
        h.height,
        h.stride,
        h.missing_msbs,
        h.num_passes,
        h.lengths1,
        h.lengths2,
        h.stripe_causal,
        h.result
    )
}

#[test]
fn golden_block_records() {
    let list = |dir: PathBuf, ext: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == ext))
            .collect();
        v.sort();
        v
    };
    let mut files = list(data_dir(), "ojbk");
    files.extend(list(data_dir().join("synth"), "ojbh"));
    let mut ojbk = Stats::default();
    let mut ojbh = Stats::default();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        let mut off = 0;
        let mut idx = 0;
        while off < data.len() {
            let h = header(&data[off..]);
            assert!(h.bits == 32 || h.bits == 64, "{name} record {idx}: bits");
            assert_eq!(h.coded_len, h.lengths1 as usize + h.lengths2 as usize);
            let tail = match &h.magic {
                b"OJBK" => h.width as usize * h.height as usize * (h.bits as usize / 8),
                b"OJBH" => 8,
                m => panic!("{name} record {idx} at {off}: bad magic {m:?}"),
            };
            let len = 40 + h.coded_len + tail;
            let rec = &data[off..off + len];
            let coded = &rec[40..40 + h.coded_len];
            if &h.magic == b"OJBK" {
                check_ojbk(&name, idx, &h, rec, coded);
                ojbk.count(&h);
            } else {
                check_ojbh(&name, idx, &h, rec, coded);
                ojbh.count(&h);
            }
            off += len;
            idx += 1;
        }
    }
    eprintln!("codestream records (OJBK): {ojbk:?}");
    eprintln!("synthetic records (OJBH): {ojbh:?}");
    // guard against vectors silently going missing
    assert!(ojbk.dec32 > 0 && ojbk.dec64 > 0, "{ojbk:?}");
    assert!(
        ojbh.dec32 > 0 && ojbh.dec64 > 0 && ojbh.causal > 0 && ojbh.false_results > 0,
        "{ojbh:?}"
    );
    assert!(
        ojbh.passes[1] > 0 && ojbh.passes[2] > 0 && ojbh.passes[3] > 0,
        "{ojbh:?}"
    );
}
