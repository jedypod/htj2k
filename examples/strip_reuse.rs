// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! What reusing a decoder's memory costs per codestream for an image
//! coded as a sequence of fixed-height strips, each its own codestream,
//! whose last strip is shorter.
//!
//! ```sh
//! cargo run --release --example strip_reuse             # allocations
//! cargo run --release --example strip_reuse -- perf     # instructions
//! cargo run --release --example strip_reuse -- run fresh|reused N CASE
//! ```
//!
//! Each case is an `ojph_enc` command line without `--size`
//! (`tests/common/synth.rs` synthesises each strip), a strip height, and
//! an image height; see [`CASES`]. An image of height H in strips of S
//! lines is H / S strips of S lines and, if S does not divide H, one
//! strip of H mod S lines. The strips of one height are one codestream,
//! encoded before the loop.
//!
//! With no arguments, a counting global allocator measures the
//! allocations (calls to `alloc`, `alloc_zeroed` and `realloc`) of each
//! codestream as one `DecoderMemory` passes through `into_memory` and
//! `with_memory`: the strips of one image, then the first strip of the
//! next. One image and the next image's first strip are decoded first,
//! to warm the memory up. The table gives the mean over the full strips
//! after the first, the short strip, the first strip of the next image,
//! and the whole image; and the bytes the memory holds between
//! codestreams (`held`).
//!
//! `perf` runs `run` under `perf stat -e instructions:u` for N and for
//! 2N images and prints (count(2N) - count(N)) / (N * strips), the
//! user-space instructions per codestream, without the start-up and the
//! encoding. `fresh` gives every codestream a new `Codestream`; `reused`
//! passes one memory along, as above.

#[path = "../tests/common/mod.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::{exit, Command};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use common::drive;
use common::synth::EncArgs;
use htj2k::codestream::{Codestream, DecoderMemory};

/// RGBA-like 16-bit signed samples, the RCT on the first three, coded as
/// the benchmark's strips: lossless.
const LOSSLESS: &str = "--comps 4 --depth 16 --signed 1 --nlt3 --rct 1 --planar 0 \
    --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R";

/// The same samples, irreversible with the colour transform on the first
/// three components, the fourth reversible.
const LOSSY: &str = "--comps 4 --depth 16 --signed 1 --irrev --rct 1 --rev-comps 3 \
    --nlt3-comps 3 --qstep 0.0005 --planar 0 \
    --levels 5 --block 128x32 --prog RPCL --tlm --tileparts R";

/// One case: an image of `height` lines, `width` wide, in strips of
/// `strip` lines, each coded with `options`.
struct Case {
    name: &'static str,
    options: &'static str,
    width: u32,
    strip: u32,
    height: u32,
    /// Whether each strip is coded padded to 1 modulo 32 lines and
    /// columns, as some callers pad irreversible codestreams for 5
    /// decomposition levels; the decoder then sees one shape for every
    /// strip whose height pads to the same length.
    pad: bool,
    /// The images per run for `perf` (N).
    n: u32,
}

const fn case(name: &'static str, options: &'static str, strip: u32, height: u32) -> Case {
    Case {
        name,
        options,
        width: 300,
        strip,
        height,
        pad: false,
        n: 20,
    }
}

/// The cases. All are 300 wide; the 1024-line ones have no short strip.
/// Padded, a 256-line strip is coded as 257 lines, and so is a short
/// strip of 225 to 256 lines, but not a shorter one.
const CASES: &[Case] = &[
    case("lossless, 256-line strips, 1024 lines", LOSSLESS, 256, 1024),
    case("lossless, 256-line strips, 1000 lines", LOSSLESS, 256, 1000),
    case("lossless, 32-line strips, 1000 lines", LOSSLESS, 32, 1000),
    case("lossy, 256-line strips, 1024 lines", LOSSY, 256, 1024),
    case("lossy, 256-line strips, 1000 lines", LOSSY, 256, 1000),
    case("lossy, 32-line strips, 1000 lines", LOSSY, 32, 1000),
    Case {
        pad: true,
        ..case("lossy padded, 256-line strips, 1000 lines", LOSSY, 256, 1000)
    },
    Case {
        pad: true,
        ..case("lossy padded, 32-line strips, 1000 lines", LOSSY, 32, 1000)
    },
    // the 76-line last strip pads to 97 lines, not 257
    Case {
        pad: true,
        ..case("lossy padded, 256-line strips, 1100 lines", LOSSY, 256, 1100)
    },
];

/// `n` padded to 1 modulo 32, at least 1.
fn padded(n: u32) -> u32 {
    n + (33 - n % 32) % 32
}

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static CUR: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            CUR.fetch_add(layout.size(), Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            ALLOCS.fetch_add(1, Relaxed);
            CUR.fetch_add(layout.size(), Relaxed);
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
            CUR.fetch_sub(layout.size(), Relaxed);
            CUR.fetch_add(new_size, Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const USAGE: &str = "usage: strip_reuse [perf | run fresh|reused N CASE]";

fn die(msg: &str) -> ! {
    eprintln!("strip_reuse: {msg}");
    exit(2)
}

/// One case's codestreams: the full strip, the short one if there is
/// one, and the strip heights of one image, as indices into `j2c`.
struct Image {
    j2c: Vec<Vec<u8>>,
    strips: Vec<usize>,
}

impl Image {
    fn new(c: &Case) -> Self {
        let pad = |n| if c.pad { padded(n) } else { n };
        let encode = |h: u32| {
            let line = format!("--size {}x{} {}", pad(c.width), pad(h), c.options);
            let words: Vec<&str> = line.split_whitespace().collect();
            let args =
                EncArgs::parse(&words).unwrap_or_else(|e| die(&format!("{line}: {e}")));
            let mut j2c = Vec::new();
            drive::encode(&args, &mut j2c).unwrap();
            j2c
        };
        let mut j2c = vec![encode(c.strip)];
        let mut strips = vec![0; (c.height / c.strip) as usize];
        if c.height % c.strip != 0 {
            j2c.push(encode(c.height % c.strip));
            strips.push(1);
        }
        Image { j2c, strips }
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

/// Decodes one strip through `memory` (or a new decoder if `None`), and
/// keeps the memory if `reused`.
fn strip(memory: &mut Option<DecoderMemory>, j2c: &[u8], reused: bool) {
    let cs = match memory.take() {
        Some(m) => Codestream::with_memory(m),
        None => Codestream::new(),
    };
    let m = decode(cs, j2c);
    if reused {
        *memory = Some(m);
    }
}

/// The allocations of each strip of one image and of the next image's
/// first strip, through one warmed-up memory, and the bytes it holds.
fn measure(image: &Image) -> (Vec<usize>, usize) {
    let mut memory = None;
    let base = CUR.load(Relaxed);
    for &s in image.strips.iter().chain([&0]) {
        strip(&mut memory, &image.j2c[s], true);
    }
    let mut allocs = Vec::new();
    for &s in image.strips.iter().chain([&0]) {
        let a = ALLOCS.load(Relaxed);
        strip(&mut memory, &image.j2c[s], true);
        allocs.push(ALLOCS.load(Relaxed) - a);
    }
    (allocs, CUR.load(Relaxed) - base)
}

fn table() {
    println!("| case | full strip | short strip | next image's first | per image | held |");
    println!("|---|---:|---:|---:|---:|---:|");
    for c in CASES {
        let image = Image::new(c);
        let (allocs, held) = measure(&image);
        let n = image.strips.len();
        let full: Vec<usize> = (1..n)
            .filter(|&i| image.strips[i] == 0)
            .map(|i| allocs[i])
            .collect();
        let mean = full.iter().sum::<usize>() as f64 / full.len().max(1) as f64;
        let short = if image.strips[n - 1] == 1 {
            allocs[n - 1].to_string()
        } else {
            "-".to_string()
        };
        let per_image: usize = allocs[1..].iter().sum();
        println!(
            "| {} | {mean:.1} | {short} | {} | {per_image} | {held} |",
            c.name, allocs[n]
        );
    }
}

/// `perf stat -e instructions:u` of `run mode n case`.
fn instructions(mode: &str, n: u32, case: usize) -> u64 {
    let exe = std::env::current_exe().unwrap();
    let output = Command::new("perf")
        .args(["stat", "-x,", "-e", "instructions:u", "--"])
        .arg(exe)
        .args(["run", mode, &n.to_string(), &case.to_string()])
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
    println!("| case | fresh | reused | saved |");
    println!("|---|---:|---:|---:|");
    for (case, c) in CASES.iter().enumerate() {
        let (name, n) = (c.name, c.n);
        let strips = u64::from(c.height.div_ceil(c.strip));
        let per = |mode| {
            let one = instructions(mode, n, case);
            let two = instructions(mode, 2 * n, case);
            two.saturating_sub(one) / (u64::from(n) * strips)
        };
        let (fresh, reused) = (per("fresh"), per("reused"));
        let saved = 100.0 * (fresh as f64 - reused as f64) / fresh as f64;
        println!("| {name} | {fresh} | {reused} | {saved:.1}% |");
    }
}

fn run(args: &[String]) {
    let [mode, n, case] = args else {
        die(USAGE)
    };
    let reused = match mode.as_str() {
        "fresh" => false,
        "reused" => true,
        _ => die(USAGE),
    };
    let n: u32 = n.parse().unwrap_or_else(|_| die(USAGE));
    let case: usize = case.parse().unwrap_or_else(|_| die(USAGE));
    let image = Image::new(CASES.get(case).unwrap_or_else(|| die(USAGE)));
    let mut memory = None;
    for _ in 0..n {
        for &s in &image.strips {
            strip(&mut memory, &image.j2c[s], reused);
        }
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
