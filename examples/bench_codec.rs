// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! In-process, single-threaded, memory-to-memory timing harness, the
//! counterpart of `oracle/bench_codec.cpp` (see `bench/README.md`).
//!
//! ```text
//! cargo run --release --example bench_codec -- check    <ojph_enc options>
//! cargo run --release --example bench_codec -- enc N    <ojph_enc options>
//! cargo run --release --example bench_codec -- dec N    <ojph_enc options>
//! cargo run --release --example bench_codec -- dump     <ojph_enc options>
//! ```
//!
//! The options are `oracle/ojph_enc`'s, without `-o`: the image is the
//! one `ojph_enc` synthesises (`tests/common/synth.rs`) and the encoder
//! gets the same calls (`tests/common/drive.rs`), including OpenJPH's
//! version comment, so the codestreams can be compared byte for byte.
//!
//! The image is synthesised once, before the loop, and converted to the
//! 32-bit samples that `exchange` takes. For `dec` the codestream is
//! encoded once, also before the loop. Each encode builds an `Encoder`,
//! writes to a new `Vec` and fills every line through `exchange`; each
//! decode builds a `Codestream` and pulls every line into buffers
//! allocated before the loop. Nothing is written to disk.
//!
//! The output is one line of `key=value` pairs, as the C++ harness
//! prints: the size and FNV-1a-64 of the codestream (`check`, `enc`) and
//! of the decoded samples in `ojph_dec`'s `.raw` layout (`check`, `dec`),
//! from the last iteration. `dump` writes the decode itself, in the
//! `.raw` layout, to standard output. The timing is left to `perf stat`; run N and
//! 2N iterations and take the difference to remove the set-up.

#[path = "../tests/common/mod.rs"]
mod common;

use std::process::exit;

use common::drive;
use common::synth::EncArgs;
use htj2k::codestream::Codestream;

const USAGE: &str = "usage: bench_codec check|dump|enc N|dec N [ojph_enc options, without -o]";

fn die(msg: &str) -> ! {
    eprintln!("bench_codec: {msg}");
    exit(2)
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

/// One encode of `img` (the low 32 bits of every sample, per component).
fn encode(a: &EncArgs, sizes: &[(u32, u32)], img: &[Vec<i32>]) -> htj2k::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut cs = drive::headers(a, &mut out)?;
    let mut row = vec![0usize; img.len()];
    while let Some((c, line)) = cs.exchange()? {
        let c = c as usize;
        let w = sizes[c].0 as usize;
        if row[c] >= sizes[c].1 as usize {
            die("exchange asked for an unexpected component/line");
        }
        line.copy_from_slice(&img[c][row[c] * w..(row[c] + 1) * w]);
        row[c] += 1;
    }
    cs.flush(&mut out)?;
    Ok(out)
}

/// Decoded planes in the `.raw` layout's order.
#[derive(Default)]
struct Planes {
    w: Vec<usize>,
    h: Vec<usize>,
    data: Vec<Vec<i32>>,
}

/// One decode of `j2c` into `p`, whose buffers are reused.
fn decode(j2c: &[u8], p: &mut Planes) -> htj2k::Result<()> {
    let mut cs = Codestream::new();
    cs.read_headers(j2c)?;
    let siz = cs.access_siz();
    let nc = siz.get_num_components() as usize;
    if p.data.len() != nc {
        p.w = vec![0; nc];
        p.h = vec![0; nc];
        p.data = vec![Vec::new(); nc];
    }
    let mut total = 0;
    for c in 0..nc {
        p.w[c] = siz.get_recon_width(c as u32) as usize;
        p.h[c] = siz.get_recon_height(c as u32) as usize;
        total += p.h[c];
        p.data[c].resize(p.w[c] * p.h[c], 0);
    }
    // As `ojph_dec`: interleaved for the colour transform, else planar.
    let same_heights = p.h.iter().all(|&h| h == p.h[0]);
    let planar = !(same_heights && cs.access_cod().is_employing_color_transform());
    cs.set_planar(planar);
    cs.create()?;
    let mut row = vec![0usize; nc];
    for _ in 0..total {
        let Some((c, line)) = cs.pull()? else {
            die("pull returned no line");
        };
        let c = c as usize;
        if c >= nc || row[c] >= p.h[c] {
            die("pull returned an unexpected line");
        }
        let w = p.w[c];
        p.data[c][row[c] * w..][..w].copy_from_slice(&line.i32()[..w]);
        row[c] += 1;
    }
    Ok(())
}

fn j2c_summary(j2c: &[u8]) -> String {
    let mut f = Fnv::new();
    f.bytes(j2c);
    format!("j2c_bytes={} j2c_fnv={:016x}", j2c.len(), f.0)
}

fn raw_summary(p: &Planes) -> String {
    let mut f = Fnv::new();
    let mut n = 0;
    for d in &p.data {
        for v in d {
            f.bytes(&v.to_le_bytes());
        }
        n += d.len() * 4;
    }
    format!("raw_bytes={} raw_fnv={:016x}", n, f.0)
}

fn main() {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let (mode, iters, rest) = match words.first().map(String::as_str) {
        Some(m @ ("check" | "dump")) => (m, 1u64, &words[1..]),
        Some(m @ ("enc" | "dec")) => {
            let n = words
                .get(1)
                .and_then(|n| n.parse::<u64>().ok())
                .filter(|&n| n >= 1)
                .unwrap_or_else(|| die(&format!("{m} needs an iteration count")));
            (m, n, &words[2..])
        }
        _ => die(USAGE),
    };
    if rest.iter().any(|w| w == "-o" || w == "--raw-out") {
        die("bench_codec writes no files");
    }
    let mut parse: Vec<&str> = vec!["-o", "-"];
    parse.extend(rest.iter().map(String::as_str));
    let a = EncArgs::parse(&parse).unwrap_or_else(|e| die(&e));

    let sizes = a.comp_sizes();
    let img: Vec<Vec<i32>> = a
        .synthesise()
        .iter()
        .map(|c| c.iter().map(|&s| s as u32 as i32).collect())
        .collect();
    let fail = |e: htj2k::Error| -> ! {
        eprintln!("bench_codec: htj2k error: {e}");
        exit(1)
    };

    let line = if mode == "enc" {
        let mut j2c = Vec::new();
        for _ in 0..iters {
            j2c = encode(&a, &sizes, &img).unwrap_or_else(|e| fail(e));
        }
        j2c_summary(&j2c)
    } else {
        let j2c = encode(&a, &sizes, &img).unwrap_or_else(|e| fail(e));
        let mut p = Planes::default();
        for _ in 0..iters {
            decode(&j2c, &mut p).unwrap_or_else(|e| fail(e));
        }
        if mode == "dump" {
            use std::io::Write;
            let mut raw = Vec::new();
            for d in &p.data {
                for v in d {
                    raw.extend_from_slice(&v.to_le_bytes());
                }
            }
            std::io::stdout()
                .write_all(&raw)
                .unwrap_or_else(|e| die(&format!("cannot write the decode: {e}")));
            return;
        }
        if mode == "check" {
            format!("{} {}", j2c_summary(&j2c), raw_summary(&p))
        } else {
            raw_summary(&p)
        }
    };
    println!("{line}");
}
