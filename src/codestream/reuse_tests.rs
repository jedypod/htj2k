// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The decoder's tree after `reset` is the tree `create` builds: every
//! field that decoding changes is put back, whether the codestream was
//! decoded in full, in part, or failed half way.

use super::Codestream;
use std::path::Path;

/// Every codestream of `tests/data/matrix` and `tests/data/strips`.
pub(crate) fn test_codestreams() -> Vec<(String, Vec<u8>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let mut out = Vec::new();
    for dir in ["matrix", "strips"] {
        let mut names: Vec<_> = std::fs::read_dir(root.join(dir))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "j2c"))
            .collect();
        names.sort();
        for p in names {
            let name = format!("{dir}/{}", p.file_name().unwrap().to_string_lossy());
            out.push((name, std::fs::read(&p).unwrap()));
        }
    }
    out
}

/// A cut-short and a damaged copy of `data`, on which `create` or
/// `pull` may fail.
pub(crate) fn broken_copies(data: &[u8]) -> [(&'static str, Vec<u8>); 2] {
    let cut = data[..data.len() * 3 / 5].to_vec();
    let mut bad = data.to_vec();
    for i in (bad.len() / 3..bad.len().saturating_sub(2)).step_by(97) {
        bad[i] ^= 0x5A;
    }
    [("cut", cut), ("damaged", bad)]
}

/// The tree of `cs` with every sample buffer zeroed, as text.
fn tree_text(cs: &mut Codestream) -> String {
    for tile in cs.tiles.iter_mut() {
        tile.fill_buffers(0);
    }
    format!("{:?}", cs.tiles)
}

/// A decoder of `data` after `create`, which may have failed after
/// building the tree; `None` if the tree was never built.
fn created(data: &[u8], resilient: bool) -> Option<Codestream<'_>> {
    let mut cs = Codestream::new();
    if resilient {
        cs.enable_resilience().unwrap();
    }
    cs.read_headers(data).ok()?;
    let _ = cs.create();
    cs.created.then_some(cs)
}

/// The tree `create` builds for `data`, before it parses any packet.
fn fresh_tree(data: &[u8], resilient: bool) -> Option<String> {
    let mut cs = Codestream::new();
    if resilient {
        cs.enable_resilience().unwrap();
    }
    cs.read_headers(data).ok()?;
    cs.alloc().ok()?;
    Some(tree_text(&mut cs))
}

/// Decodes `data` (which may fail), pulling at most `lines` lines, then
/// resets the tree.
fn used_then_reset(data: &[u8], resilient: bool, lines: usize) -> Option<String> {
    let mut cs = created(data, resilient)?;
    for _ in 0..lines {
        match cs.pull() {
            Ok(Some(_)) => {}
            _ => break,
        }
    }
    for tile in cs.tiles.iter_mut() {
        tile.reset();
    }
    Some(tree_text(&mut cs))
}

#[test]
fn reset_restores_the_tree_create_builds() {
    let mut checked = 0;
    for (name, data) in test_codestreams() {
        let fresh = fresh_tree(&data, false).unwrap_or_else(|| panic!("{name}: no tree"));
        // in full, and part way
        for lines in [usize::MAX, 7] {
            let used = used_then_reset(&data, false, lines).unwrap();
            assert!(used == fresh, "{name}: tree differs after {lines} lines");
        }
        for (what, d) in broken_copies(&data) {
            for resilient in [false, true] {
                let fresh = fresh_tree(&d, resilient);
                let used = used_then_reset(&d, resilient, usize::MAX);
                assert!(
                    used == fresh,
                    "{name}: tree differs after a {what} codestream (resilient {resilient})"
                );
            }
        }
        checked += 1;
    }
    assert!(checked > 200, "only {checked} codestreams");
}
