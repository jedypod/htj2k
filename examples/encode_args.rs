// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Encodes an `oracle/ojph_enc` command line with the crate's encoder,
//! for the oracle to decode the result with OpenJPH
//! (`oracle/gen_ours.py`).
//!
//! ```text
//! cargo run --release --example encode_args -- -o out.j2c [--raw-out in.raw] <ojph_enc options>
//! ```
//!
//! The image is `ojph_enc`'s (`tests/common/synth.rs`), and the
//! parameters are set as `ojph_enc` sets them (`tests/common/drive.rs`).
//! `--raw-out` writes the encoder input in the `.raw` layout.

#[path = "../tests/common/mod.rs"]
mod common;

use common::drive;
use common::synth::EncArgs;

fn main() {
    let mut words: Vec<String> = std::env::args().skip(1).collect();
    let mut raw_out = None;
    if let Some(i) = words.iter().position(|w| w == "--raw-out") {
        if i + 1 >= words.len() {
            eprintln!("encode_args: missing value for --raw-out");
            std::process::exit(2);
        }
        raw_out = Some(words.remove(i + 1));
        words.remove(i);
    }
    let cmd = format!("ojph_enc {}", words.join(" "));
    let args = match EncArgs::parse_command(&cmd) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("encode_args: {e}");
            std::process::exit(2);
        }
    };
    let img = args.synthesise();
    if let Some(p) = raw_out {
        std::fs::write(&p, EncArgs::to_raw(&img)).expect("cannot write --raw-out");
    }
    let mut out = Vec::new();
    let result = drive::headers(&args, &mut out).and_then(|mut cs| {
        drive::push_image(&mut cs, &args, &img).and_then(|_| cs.flush(&mut out))
    });
    if let Err(e) = result {
        eprintln!("encode_args: {e}");
        std::process::exit(1);
    }
    std::fs::write(&args.out, &out).expect("cannot write -o");
}
