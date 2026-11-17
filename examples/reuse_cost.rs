// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! What reusing a decoder's memory or an encoder saves per codestream,
//! for a sequence of same-shaped codestreams.
//!
//! ```sh
//! cargo run --release --example reuse_cost             # allocations
//! cargo run --release --example reuse_cost -- perf     # instructions
//! cargo run --release --example reuse_cost -- run SIDE MODE N CASE
//! ```
//!
//! Each case is an `ojph_enc` command line (`tests/common/synth.rs`
//! synthesises the image); see [`CASES`]. Each side, `dec` and `enc`,
//! runs in two modes:
//!
//! * `fresh`: every codestream gets a new `Codestream` (decode) or
//!   `Encoder` (encode), as before reuse existed.
//! * `reused`: one `DecoderMemory` passes from each `Codestream` to the
//!   next through `into_memory` and `with_memory`; one `Encoder` is
//!   `reset` between codestreams.
//!
//! The decode pulls every line; the encode configures the encoder,
//! writes the headers, fills every line through `exchange` and flushes.
//! The input (the codestream, or the image as 32-bit samples) and the
//! output vector are made before the loop and are not counted.
//!
//! With no arguments, a counting global allocator measures, per
//! codestream (the mean over 8, after one to warm up): allocations
//! (calls to `alloc`, `alloc_zeroed` and `realloc`), the bytes they ask
//! for, and the peak of live heap bytes above what was live when the
//! codestream started. `held` is what stays allocated between
//! codestreams, in the reused mode: the kept memory.
//!
//! `perf` runs `run` under `perf stat -e instructions:u` for N and for 2N
//! codestreams and prints (count(2N) - count(N)) / N, which removes the
//! start-up and the synthesis, as `bench/` does. Instructions, not
//! cycles, so that the result does not depend on the machine's load.

#[path = "../tests/common/mod.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::{exit, Command};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use common::drive;
use common::synth::EncArgs;
use htj2k::codestream::{Codestream, DecoderMemory, Encoder};

/// The cases: a name, the `ojph_enc` options and N for `perf`.
const CASES: &[(&str, &str, u32)] = &[
    (
        "2048x32 1c 16-bit",
        "--size 2048x32 --depth 16 --signed 1 --nlt3 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R",
        200,
    ),
    (
        "256x32 1c 16-bit",
        "--size 256x32 --depth 16 --signed 1 --nlt3 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R",
        1000,
    ),
    (
        "1920x32 4c 16-bit",
        "--size 1920x32 --comps 4 --depth 16 --signed 1 --nlt3 --rct 1 --planar 0 --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R",
        100,
    ),
    (
        "3840x2160 3c 12-bit",
        "--size 3840x2160 --comps 3 --depth 12 --rct 1",
        2,
    ),
];

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let now = CUR.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(now, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size(), Relaxed);
            grew(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size(), Relaxed);
            grew(layout.size());
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
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(new_size, Relaxed);
            CUR.fetch_sub(layout.size(), Relaxed);
            grew(new_size);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const USAGE: &str = "usage: reuse_cost [perf | run dec|enc fresh|reused N CASE]";

fn die(msg: &str) -> ! {
    eprintln!("reuse_cost: {msg}");
    exit(2)
}

/// One case's inputs: the encoder's arguments, the image as 32-bit
/// samples and the codestream it encodes to.
struct Input {
    args: EncArgs,
    sizes: Vec<(u32, u32)>,
    img: Vec<Vec<i32>>,
    j2c: Vec<u8>,
}

impl Input {
    fn new(options: &str) -> Self {
        let args = EncArgs::parse(&options.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_else(|e| die(&format!("{options}: {e}")));
        let img = args
            .synthesise()
            .into_iter()
            .map(|c| c.into_iter().map(|s| s as u32 as i32).collect())
            .collect();
        let mut j2c = Vec::new();
        drive::encode(&args, &mut j2c).unwrap();
        Input {
            sizes: args.comp_sizes(),
            args,
            img,
            j2c,
        }
    }
}

/// Decodes `j2c` with `cs`, new or with kept memory, and returns the
/// memory.
fn decode<'a>(mut cs: Codestream<'a>, j2c: &'a [u8]) -> DecoderMemory {
    cs.read_headers(j2c).unwrap();
    cs.create().unwrap();
    while cs.pull().unwrap().is_some() {}
    cs.into_memory()
}

/// Encodes the image with `enc`, new or reset, into `out`, and returns
/// the encoder.
fn encode(enc: Encoder, input: &Input, out: &mut Vec<u8>) -> Encoder {
    let mut enc = drive::configure_on(enc, &input.args).unwrap();
    enc.write_headers(out, &[]).unwrap();
    let mut row = vec![0usize; input.img.len()];
    while let Some((c, line)) = enc.exchange().unwrap() {
        let c = c as usize;
        let w = input.sizes[c].0 as usize;
        line.copy_from_slice(&input.img[c][row[c] * w..(row[c] + 1) * w]);
        row[c] += 1;
    }
    enc.flush(out).unwrap();
    enc
}

/// Something that codes one codestream after another.
struct Runner<'a> {
    input: &'a Input,
    reused: bool,
    memory: Option<DecoderMemory>,
    encoder: Option<Encoder>,
    out: Vec<u8>,
}

impl<'a> Runner<'a> {
    fn new(input: &'a Input, reused: bool) -> Self {
        Runner {
            input,
            reused,
            memory: None,
            encoder: None,
            out: Vec::with_capacity(2 * input.j2c.len() + 4096),
        }
    }

    fn decode(&mut self) {
        let cs = match self.memory.take() {
            Some(memory) => Codestream::with_memory(memory),
            None => Codestream::new(),
        };
        let memory = decode(cs, &self.input.j2c);
        if self.reused {
            self.memory = Some(memory);
        }
    }

    fn encode(&mut self) {
        self.out.clear();
        let enc = match self.encoder.take() {
            Some(mut enc) => {
                enc.reset();
                enc
            }
            None => Encoder::new(),
        };
        let enc = encode(enc, self.input, &mut self.out);
        assert_eq!(self.out, self.input.j2c);
        if self.reused {
            self.encoder = Some(enc);
        }
    }

    fn one(&mut self, side: &str) {
        match side {
            "dec" => self.decode(),
            "enc" => self.encode(),
            _ => die(USAGE),
        }
    }
}

/// The mean allocations, bytes and peak per codestream, and the bytes
/// held between codestreams.
fn measure(input: &Input, side: &str, reused: bool) -> [usize; 4] {
    const RUNS: usize = 8;
    let mut runner = Runner::new(input, reused);
    let base = CUR.load(Relaxed);
    runner.one(side);
    let (mut allocs, mut bytes, mut peak) = (0, 0, 0);
    for _ in 0..RUNS {
        let start = CUR.load(Relaxed);
        PEAK.store(start, Relaxed);
        let (a, b) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
        runner.one(side);
        allocs += ALLOCS.load(Relaxed) - a;
        bytes += BYTES.load(Relaxed) - b;
        peak += PEAK.load(Relaxed) - start;
    }
    let held = CUR.load(Relaxed) - base;
    [allocs / RUNS, bytes / RUNS, peak / RUNS, held]
}

fn table() {
    println!("| case | side | mode | allocations | bytes | peak | held |");
    println!("|---|---|---|---:|---:|---:|---:|");
    for &(name, options, _) in CASES {
        let input = Input::new(options);
        for side in ["dec", "enc"] {
            for reused in [false, true] {
                let [allocs, bytes, peak, held] = measure(&input, side, reused);
                let mode = if reused { "reused" } else { "fresh" };
                println!("| {name} | {side} | {mode} | {allocs} | {bytes} | {peak} | {held} |");
            }
        }
    }
}

/// `perf stat -e instructions:u` of `run side mode n case`.
fn instructions(side: &str, mode: &str, n: u32, case: usize) -> u64 {
    let exe = std::env::current_exe().unwrap();
    let output = Command::new("perf")
        .args(["stat", "-x,", "-e", "instructions:u", "--"])
        .arg(exe)
        .args(["run", side, mode, &n.to_string(), &case.to_string()])
        .output()
        .unwrap_or_else(|e| die(&format!("perf: {e}")));
    if !output.status.success() {
        die(&String::from_utf8_lossy(&output.stderr));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr
        .lines()
        .find(|l| l.contains("instructions"))
        .and_then(|l| l.split(',').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| die(&format!("no count in perf's output: {stderr}")))
}

fn perf() {
    println!("| case | side | fresh | reused | saved |");
    println!("|---|---|---:|---:|---:|");
    for (case, &(name, _, n)) in CASES.iter().enumerate() {
        for side in ["dec", "enc"] {
            let per = |mode| {
                let one = instructions(side, mode, n, case);
                let two = instructions(side, mode, 2 * n, case);
                two.saturating_sub(one) / u64::from(n)
            };
            let (fresh, reused) = (per("fresh"), per("reused"));
            let saved = 100.0 * (fresh as f64 - reused as f64) / fresh as f64;
            println!("| {name} | {side} | {fresh} | {reused} | {saved:.1}% |");
        }
    }
}

fn run(args: &[String]) {
    let [side, mode, n, case] = args else {
        die(USAGE)
    };
    let reused = match mode.as_str() {
        "fresh" => false,
        "reused" => true,
        _ => die(USAGE),
    };
    let n: u32 = n.parse().unwrap_or_else(|_| die(USAGE));
    let case: usize = case.parse().unwrap_or_else(|_| die(USAGE));
    let &(_, options, _) = CASES.get(case).unwrap_or_else(|| die(USAGE));
    let input = Input::new(options);
    let mut runner = Runner::new(&input, reused);
    for _ in 0..n {
        runner.one(side);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => table(),
        Some("perf") => perf(),
        Some("run") => run(&args[1..]),
        _ => die(USAGE),
    }
}
