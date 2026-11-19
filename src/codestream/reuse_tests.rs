// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The decoder's tree after `reset` is the tree `create` builds: every
//! field that decoding changes is put back, whether the codestream was
//! decoded in full, in part, or failed half way. Kept memory, the
//! parameter objects included, filled with garbage changes nothing.

use super::{Codestream, DecoderMemory};
use crate::params::reuse_tests::poison_params;
use crate::params::{ParamAtk, ParamCod, ParamDfs, ParamNlt, ParamQcd, ParamSiz};
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
    for tile in cs.mem.tree.tiles.iter_mut() {
        tile.fill_buffers(0);
    }
    format!("{:?}", cs.mem.tree.tiles)
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
    for tile in cs.mem.tree.tiles.iter_mut() {
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

/// What decoding gives: every line pulled, with its component, the
/// error code that stopped it, if one did, and the parameters as the
/// decoder left them.
#[derive(Debug, PartialEq)]
struct Decoded {
    lines: Vec<(u32, Vec<i32>)>,
    error: Option<u32>,
    params: Params,
}

/// The parameter objects of a decoder, compared by their parameters
/// (spares compare equal).
type Params = (ParamSiz, ParamCod, ParamQcd, ParamNlt, ParamDfs, ParamAtk);

fn params(cs: &Codestream) -> Params {
    (
        cs.siz.clone(),
        cs.cod.clone(),
        cs.qcd.clone(),
        cs.nlt.clone(),
        cs.dfs.clone(),
        cs.atk.clone(),
    )
}

/// Decodes `data` with `cs`, resiliently or not.
fn decode<'a>(cs: &mut Codestream<'a>, data: &'a [u8], resilient: bool) -> Decoded {
    let mut lines = Vec::new();
    let mut run = || -> crate::Result<()> {
        if resilient {
            cs.enable_resilience()?;
        }
        cs.read_headers(data)?;
        cs.create()?;
        while let Some((c, line)) = cs.pull()? {
            lines.push((c, line.i32()[..line.size].to_vec()));
        }
        Ok(())
    };
    let error = run().err().map(|e| e.code());
    Decoded {
        lines,
        error,
        params: params(cs),
    }
}

/// Fills every buffer `mem` keeps, in both trees, with the bit pattern
/// `bits`, and its parameter objects with garbage.
fn poison(mem: &mut DecoderMemory, bits: u32) {
    poison_params(&mut mem.params);
    for tree in [&mut mem.tree, &mut mem.spare] {
        for tile in tree.tiles.iter_mut() {
            tile.fill_buffers(bits);
        }
        for line in tree.lines.iter_mut() {
            line.fill_bits(bits);
        }
    }
}

/// Decodes `next` with the memory left by decoding `first`, after
/// poisoning it, and requires what a new decoder gives. Returns whether
/// the memory's tiles were reused rather than built again.
fn check_poisoned(name: &str, first: &[u8], next: &[u8], resilient: bool, bits: u32) -> bool {
    let want = decode(&mut Codestream::new(), next, resilient);
    let mut cs = Codestream::new();
    decode(&mut cs, first, resilient);
    let mut mem = cs.into_memory();
    poison(&mut mem, bits);
    let before = mem.tree.tiles.as_ptr();
    let mut cs = Codestream::with_memory(mem);
    let got = decode(&mut cs, next, resilient);
    assert!(
        got == want,
        "{name}: a reused decoder differs from a new one (resilient {resilient}, poison {bits:08x})"
    );
    !cs.mem.tree.tiles.is_empty() && cs.mem.tree.tiles.as_ptr() == before
}

/// No sample of a kept buffer is read before it is written: decoders
/// whose every kept buffer is filled with garbage first return exactly
/// what new decoders do, after decoding the same codestream, a cut or
/// damaged copy of it, or the other way round.
#[test]
fn poisoned_memory_decodes_as_new() {
    for (name, data) in test_codestreams() {
        for bits in [0xA5A5_A5A5, 0xFFFF_FFFF, 0x7FC0_0001] {
            let reused = check_poisoned(&name, &data, &data, false, bits);
            assert!(reused, "{name}: the tiles were not reused");
        }
        for (_, broken) in broken_copies(&data) {
            for resilient in [false, true] {
                check_poisoned(&name, &broken, &data, resilient, 0xA5A5_A5A5);
                check_poisoned(&name, &data, &broken, resilient, 0xA5A5_A5A5);
            }
        }
    }
}

/// Codestreams with per-component segments (type 4 nonlinearities,
/// QCC or COC segments) followed by plain ones, and the reverse, through
/// memory filled with garbage: what one leaves in the kept parameter
/// objects changes nothing in the next.
#[test]
fn poisoned_memory_between_parameter_sets_decodes_as_new() {
    let all = test_codestreams();
    let per_component = |name: &str| {
        ["nlt4", "qstep_comp", "rev_comps", "irrev_comps", "coc"]
            .iter()
            .any(|k| name.contains(k))
    };
    let rich: Vec<_> = all.iter().filter(|(n, _)| per_component(n)).collect();
    let plain: Vec<_> = all
        .iter()
        .filter(|(n, _)| !per_component(n))
        .step_by(20)
        .collect();
    assert!(rich.len() >= 20 && plain.len() >= 10);
    let bits = 0xA5A5_A5A5;
    for (r, rd) in &rich {
        for (p, pd) in &plain {
            check_poisoned(&[p.as_str(), r].join(" after "), rd, pd, false, bits);
            check_poisoned(&[r.as_str(), p].join(" after "), pd, rd, false, bits);
        }
        for (r2, rd2) in rich.iter().step_by(5) {
            check_poisoned(&[r2.as_str(), r].join(" after "), rd, rd2, false, bits);
        }
    }
}

/// Three strips of one image, coded with the same parameters but for
/// the height: 300x32, 300x8 and 300x200, four components.
fn strips() -> [Vec<u8>; 3] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/strips");
    ["300x32", "300x8", "300x200"]
        .map(|size| std::fs::read(root.join(format!("s16_rct_4c_{size}.j2c"))).unwrap())
}

/// Decodes `data` through `mem` with the memory limit `limit`, requires
/// what a new decoder with that limit gives, and returns the memory and
/// the error code, if any.
fn through(mem: DecoderMemory, data: &[u8], limit: u64) -> (DecoderMemory, Option<u32>) {
    let mut fresh = Codestream::new();
    fresh.set_memory_limit(limit);
    let want = decode(&mut fresh, data, false);
    let mut cs = Codestream::with_memory(mem);
    cs.set_memory_limit(limit);
    let got = decode(&mut cs, data, false);
    assert!(
        got == want,
        "a reused decoder differs from a new one (limit {limit})"
    );
    (cs.into_memory(), got.error)
}

/// The memory after decoding `first` and then `second`, with no limit to
/// speak of: it holds both trees.
fn holding(first: &[u8], second: &[u8]) -> DecoderMemory {
    let (mem, _) = through(DecoderMemory::new(), first, u64::MAX);
    let (mem, _) = through(mem, second, u64::MAX);
    assert!(mem.tree.built_for.is_some() && mem.spare.built_for.is_some());
    mem
}

/// What building the tree for `data` is charged.
fn charge(data: &[u8]) -> u64 {
    through(DecoderMemory::new(), data, u64::MAX).0.tree.charge
}

/// Two shapes in turn, such as full strips and a shorter last one, each
/// reuse their own tree, and decode as new even with both trees filled
/// with garbage.
#[test]
fn two_shapes_in_turn_reuse_a_tree_each() {
    let [a, b, _] = strips();
    let mut mem = holding(&a, &b);
    let (tiles_a, tiles_b) = (mem.spare.tiles.as_ptr(), mem.tree.tiles.as_ptr());
    for (data, tiles, other) in [
        (&a, tiles_a, tiles_b),
        (&b, tiles_b, tiles_a),
        (&a, tiles_a, tiles_b),
        (&a, tiles_a, tiles_b),
    ] {
        poison(&mut mem, 0xA5A5_A5A5);
        (mem, _) = through(mem, data, u64::MAX);
        assert_eq!(mem.tree.tiles.as_ptr(), tiles, "the tree was not reused");
        assert_eq!(mem.spare.tiles.as_ptr(), other, "the other tree went");
    }
}

/// A third shape frees the tree used least recently and keeps the last;
/// a cycle of three rebuilds each time, and decodes as new.
#[test]
fn a_third_shape_replaces_the_least_recently_used_tree() {
    let [a, b, c] = strips();
    let mut mem = holding(&a, &b);
    let tiles_b = mem.tree.tiles.as_ptr();
    (mem, _) = through(mem, &c, u64::MAX);
    assert_eq!(mem.spare.tiles.as_ptr(), tiles_b, "the tree used last went");
    let tiles_c = mem.tree.tiles.as_ptr();
    (mem, _) = through(mem, &b, u64::MAX);
    assert_eq!(mem.tree.tiles.as_ptr(), tiles_b);
    assert_eq!(mem.spare.tiles.as_ptr(), tiles_c);
    for data in [&a, &b, &c, &a, &b, &c] {
        poison(&mut mem, 0xFFFF_FFFF);
        (mem, _) = through(mem, data, u64::MAX);
    }
}

/// A build that fails is not kept. Refused by the limit, it also frees
/// the kept tree, which the limit has no room for beside it; the memory
/// then serves the next codestreams as an empty one would.
#[test]
fn a_failed_build_is_not_kept() {
    let [a, b, c] = strips();
    for limit in [1, charge(&c) - 1] {
        let (mut mem, error) = through(holding(&a, &b), &c, limit);
        assert_eq!(error, Some(0x00070001));
        assert!(mem.tree.built_for.is_none() && mem.tree.tiles.is_empty());
        assert!(mem.spare.built_for.is_none() && mem.spare.tiles.is_empty());
        // another failure, then two shapes in turn
        (mem, _) = through(mem, &c, limit);
        (mem, _) = through(mem, &b, u64::MAX);
        assert!(mem.spare.built_for.is_none(), "the failed build was kept");
        let tiles_b = mem.tree.tiles.as_ptr();
        (mem, _) = through(mem, &a, u64::MAX);
        (mem, _) = through(mem, &b, u64::MAX);
        assert_eq!(mem.tree.tiles.as_ptr(), tiles_b);
    }
}

/// `data` with the bit depth of its first NLT segment changed, so that
/// its headers read but building its tree fails: the NLT segment no
/// longer matches SIZ.
fn with_mismatched_nlt(data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    let at = d.windows(2).position(|w| w == [0xFF, 0x76]).unwrap();
    d[at + 6] ^= 1; // BDnlt, after the marker, Lnlt and Cnlt
    d
}

/// A build that fails for another reason than the limit is not kept,
/// and leaves the kept tree in place, to serve after it.
#[test]
fn a_failed_build_does_not_displace_the_kept_tree() {
    let [a, b, c] = strips();
    let mem = holding(&a, &b);
    let tiles_b = mem.tree.tiles.as_ptr();
    let (mem, error) = through(mem, &with_mismatched_nlt(&a), u64::MAX);
    assert_eq!(error, Some(0x000300A1));
    assert!(mem.tree.built_for.is_none() && mem.tree.tiles.is_empty());
    assert_eq!(mem.spare.tiles.as_ptr(), tiles_b);
    // a third shape keeps b, which is then reused
    let (mem, _) = through(mem, &c, u64::MAX);
    assert_eq!(mem.spare.tiles.as_ptr(), tiles_b);
    let (mem, _) = through(mem, &b, u64::MAX);
    assert_eq!(mem.tree.tiles.as_ptr(), tiles_b);
}

/// The tree in use is charged what it took to build, so a reused memory
/// accepts and refuses the limits a new decoder does. The other kept
/// tree is charged nothing, but is kept only while the two fit within
/// the limit together, whether the codestream reuses a tree or builds
/// one.
#[test]
fn the_other_tree_is_kept_only_within_the_limit() {
    let [a, b, c] = strips();
    let (ca, cb, cc) = (charge(&a), charge(&b), charge(&c));
    assert!(cb < ca && ca < cc);
    let kept = |mem: &DecoderMemory| mem.spare.built_for.is_some();

    // reusing the other tree (a) or the last (b): kept if both fit
    for (data, used) in [(&a, ca), (&b, cb)] {
        let (mem, error) = through(holding(&a, &b), data, ca + cb);
        assert!(error.is_none() && kept(&mem));
        let (mem, error) = through(holding(&a, &b), data, ca + cb - 1);
        assert!(error.is_none() && !kept(&mem));
        // exactly the tree's charge passes, one byte less fails
        let (mem, error) = through(holding(&a, &b), data, used);
        assert!(error.is_none() && !kept(&mem));
        let (_, error) = through(mem, data, used - 1);
        assert_eq!(error, Some(0x00070001));
        // refused, the reused tree is kept, but not the other beside it
        let (mem, error) = through(holding(&a, &b), data, used - 1);
        assert_eq!(error, Some(0x00070001));
        assert!(mem.tree.built_for.is_some() && !kept(&mem));
    }

    // building a third (c) keeps b, used last, if both fit; otherwise
    // the build reaches b's reserve, frees b and starts again
    let (mem, error) = through(holding(&a, &b), &c, cc + cb);
    assert!(error.is_none() && kept(&mem) && mem.spare.charge == cb);
    let (mem, error) = through(holding(&a, &b), &c, cc + cb - 1);
    assert!(error.is_none() && !kept(&mem));
    let (mem, error) = through(holding(&a, &b), &c, cc);
    assert!(error.is_none() && !kept(&mem));
    let (mem, error) = through(holding(&a, &b), &c, cc - 1);
    assert_eq!(error, Some(0x00070001));
    assert!(!kept(&mem) && mem.tree.built_for.is_none());

    // a kept tree larger than the limit is freed before the build
    let (mem, error) = through(holding(&a, &c), &b, cb);
    assert!(error.is_none() && !kept(&mem));
    let (mem, error) = through(mem, &c, u64::MAX);
    assert!(error.is_none() && kept(&mem) && mem.spare.charge == cb);
}
