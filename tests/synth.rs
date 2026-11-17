// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Checks the test-image synthesiser (`tests/common/synth.rs`) against
//! `oracle/ojph_enc`: for every lossless case of the matrix, the image
//! made from the manifest's command line must be the committed `.raw`,
//! which is exactly what `ojph_enc` encoded.

mod common;

use common::synth::{fnv1a64, parse_matrix_manifest, EncArgs};
use std::path::{Path, PathBuf};

fn matrix_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/matrix")
}

#[test]
fn synthesiser_reproduces_every_lossless_raw() {
    let text = std::fs::read_to_string(matrix_dir().join("MANIFEST.txt")).unwrap();
    let cases = parse_matrix_manifest(&text);
    let mut checked = 0;
    for case in cases.iter().filter(|c| c.is_lossless()) {
        let args =
            EncArgs::parse_command(&case.command).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        let raw = EncArgs::to_raw(&args.synthesise());
        let reference = std::fs::read(matrix_dir().join(format!("{}.raw", case.name))).unwrap();
        assert!(raw == reference, "{}: synthesised image differs", case.name);
        assert_eq!(Some(fnv1a64(&raw)), case.raw_fnv, "{}: hash", case.name);
        checked += 1;
    }
    assert_eq!(checked, 152);
}
