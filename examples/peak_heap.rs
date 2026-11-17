// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Measures the peak heap use of decoding, with a counting global
//! allocator.
//!
//! ```sh
//! cargo run --release --example peak_heap            # table of cases
//! cargo run --release --example peak_heap -- slope   # bytes per column
//! cargo run --release --example peak_heap -- FILE... # given codestreams
//! cargo run --release --example peak_heap -- time FILE N  # N decodes
//! ```
//!
//! The built-in cases are synthesised: a single tile, one component,
//! reversible 5/3 with 5 decomposition levels, HT blocks, one quality
//! layer and every packet empty. Memory use does not depend on the coded
//! data (only the decoded size and the coding parameters), so empty
//! packets measure the same buffers as real ones. A depth of 16 decodes
//! with 32-bit block buffers, a depth of 32 with 64-bit ones.
//!
//! The peak counts every byte live on the heap between the start of
//! `read_headers` and the last `pull`, including the codestream object
//! itself but not the input bytes, which are allocated before.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use htj2k::codestream::Codestream;

struct Counting;

static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            let now = CUR.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            let now = CUR.fetch_add(layout.size(), Relaxed) + layout.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        CUR.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = System.realloc(p, layout, new_size);
        if !q.is_null() {
            CUR.fetch_sub(layout.size(), Relaxed);
            let now = CUR.fetch_add(new_size, Relaxed) + new_size;
            PEAK.fetch_max(now, Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// A codestream of one `w` x `h` component of `depth` bits, coded with
/// `cb_w` x `cb_h` blocks and every packet empty.
fn synth(w: u32, h: u32, depth: u8, cb_w: u32, cb_h: u32) -> Vec<u8> {
    const LEVELS: u8 = 5;
    let mut v = Vec::new();
    let seg = |v: &mut Vec<u8>, marker: u16, body: &[u8]| {
        v.extend_from_slice(&marker.to_be_bytes());
        v.extend_from_slice(&(body.len() as u16 + 2).to_be_bytes());
        v.extend_from_slice(body);
    };
    v.extend_from_slice(&[0xFF, 0x4F]); // SOC

    let mut siz = Vec::new();
    siz.extend_from_slice(&0x4000u16.to_be_bytes()); // Rsiz: HTJ2K
    for x in [w, h, 0, 0, w, h, 0, 0] {
        siz.extend_from_slice(&x.to_be_bytes());
    }
    siz.extend_from_slice(&1u16.to_be_bytes()); // one component
    siz.extend_from_slice(&[0x80 | (depth - 1), 1, 1]); // signed
    seg(&mut v, 0xFF51, &siz);

    let log = |n: u32| n.trailing_zeros() as u8;
    let cod = [
        0,      // Scod: default precincts
        0,      // LRCP
        0,      // one layer (high byte)
        1,      // one layer
        0,      // no colour transform
        LEVELS, // decomposition levels
        log(cb_w) - 2,
        log(cb_h) - 2,
        0x40, // HT block coding
        1,    // reversible 5/3
    ];
    seg(&mut v, 0xFF52, &cod);

    // No quantization; exponents as an encoder would pick them, capped at
    // the 5 bits the field has. Two guard bits.
    let guard = 2u8;
    let mut qcd = vec![guard << 5];
    for b in 0..1 + 3 * LEVELS as usize {
        let gain = if b == 0 { 0 } else { [1, 1, 2][(b - 1) % 3] };
        qcd.push((depth + gain).min(31) << 3);
    }
    seg(&mut v, 0xFF5C, &qcd);

    // One empty packet (a single zero byte) per resolution.
    let packets = LEVELS as u32 + 1;
    let psot = 12 + 2 + packets;
    v.extend_from_slice(&[0xFF, 0x90, 0, 10, 0, 0]);
    v.extend_from_slice(&psot.to_be_bytes());
    v.extend_from_slice(&[0, 1]);
    v.extend_from_slice(&[0xFF, 0x93]); // SOD
    v.extend(std::iter::repeat_n(0u8, packets as usize));
    v.extend_from_slice(&[0xFF, 0xD9]); // EOC
    v
}

/// Decodes `data` and returns the peak heap use, in bytes, and the number
/// of lines decoded.
fn measure(data: &[u8]) -> (usize, usize) {
    let base = CUR.load(Relaxed);
    PEAK.store(base, Relaxed);
    let mut lines = 0;
    {
        let mut cs = Codestream::new();
        cs.read_headers(data).expect("read_headers");
        cs.create().expect("create");
        while cs.pull().expect("pull").is_some() {
            lines += 1;
        }
    }
    (PEAK.load(Relaxed) - base, lines)
}

fn kib(n: usize) -> String {
    format!("{:.1} KiB", n as f64 / 1024.0)
}

fn table() {
    let cases = [
        (32, 32, 64, 64),
        (64, 64, 64, 64),
        (128, 32, 128, 32),
        (8192, 32, 128, 32),
        (8192, 32, 64, 64),
        (8192, 256, 64, 64),
    ];
    println!("| image | blocks | 32-bit | 64-bit |");
    println!("|---|---|---|---|");
    for (w, h, cb_w, cb_h) in cases {
        let (p32, _) = measure(&synth(w, h, 16, cb_w, cb_h));
        let (p64, _) = measure(&synth(w, h, 32, cb_w, cb_h));
        println!(
            "| {w}x{h} | {cb_w}x{cb_h} | {} | {} |",
            kib(p32),
            kib(p64)
        );
    }
}

/// The cost of each column of width, from two widths, and what is left
/// at width 0.
fn slope() {
    println!("| image height | blocks | B per column | fixed |");
    println!("|---|---|---|---|");
    for h in [32, 256, 8192] {
        for (cb_w, cb_h) in [(128, 32), (64, 64), (32, 128), (16, 256)] {
            let (a, _) = measure(&synth(4096, h, 32, cb_w, cb_h));
            let (b, _) = measure(&synth(8192, h, 32, cb_w, cb_h));
            let per = (b - a) as f64 / 4096.0;
            let fixed = a as f64 - per * 4096.0;
            println!("| {h} | {cb_w}x{cb_h} | {per:.1} | {} |", kib(fixed as usize));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => table(),
        Some("slope") => slope(),
        Some("time") => {
            let data = std::fs::read(&args[1]).expect("read");
            let n: u32 = args[2].parse().expect("count");
            let start = std::time::Instant::now();
            for _ in 0..n {
                measure(&data);
            }
            let per = start.elapsed().as_secs_f64() / n as f64;
            println!("{}: {:.3} ms per decode", args[1], per * 1e3);
        }
        Some(_) => {
            for path in &args {
                let data = std::fs::read(path).expect("read");
                let (peak, lines) = measure(&data);
                println!("{path}: {} over {lines} lines", kib(peak));
            }
        }
    }
}
