// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Tests that the parameter objects keep storage, never content, across
//! a restart: a restarted object equals a new one, and objects built
//! from kept storage filled with garbage equal objects built new.
//!
//! `PartialEq` ignores the spares by design, so equality here covers
//! every parameter an object holds.

use super::*;
use crate::types::Point;

/// A main COD with COC objects for 4 components, as an encoder's caller
/// sets it, with the kernels resolved.
fn rich_cod() -> ParamCod {
    let mut cod = ParamCod::new();
    cod.set_num_decomposition(5).unwrap();
    cod.set_block_dims(128, 32).unwrap();
    cod.set_reversible(false);
    cod.employ_color_transform(1);
    for c in 0..4 {
        cod.set_num_decomposition_comp(c, 4 + c % 2).unwrap();
        cod.set_block_dims_comp(c, 64, 32).unwrap();
        cod.set_reversible_comp(c, c == 3);
    }
    cod.update_atk(&ParamAtk::new()).unwrap();
    cod
}

/// A COD object with every field set to something no setter gives.
pub(crate) fn garbage_cod(n: u16) -> ParamCod {
    let mut p = ParamCod::new();
    p.type_ = CodType::CodTile;
    p.lcod = 0xBEEF ^ n;
    p.scod = 0xA5;
    p.sgcod = CodSGcod {
        prog_order: 0x77,
        num_layers: 999,
        mc_trans: 9,
    };
    p.spcod.num_decomp = 31;
    p.spcod.block_width = 0xEE;
    p.spcod.precinct_size = [0x5A; 33];
    p.atk.init_irv97();
    p.atk.katk = f32::NAN;
    p.comp_idx = 4000 + n;
    p.children.push(ParamCod::new());
    p.top_num_decomp = 17;
    p.top_mc_trans = 3;
    p
}

#[test]
fn restarted_cod_equals_new() {
    let mut cod = rich_cod();
    cod.restart();
    assert_eq!(cod, ParamCod::new());
    assert_eq!(cod.spare.len(), 4, "the COC objects are kept as spares");
    let mut cod = rich_cod();
    cod.recycle();
    assert_eq!(cod, ParamCod::new());
}

#[test]
fn cod_from_poisoned_spares_equals_new() {
    let mut cod = ParamCod::new();
    for n in 0..6 {
        cod.spare.put(garbage_cod(n));
    }
    cod.set_num_decomposition(5).unwrap();
    cod.set_block_dims(128, 32).unwrap();
    cod.set_reversible(false);
    cod.employ_color_transform(1);
    for c in 0..4 {
        cod.set_num_decomposition_comp(c, 4 + c % 2).unwrap();
        cod.set_block_dims_comp(c, 64, 32).unwrap();
        cod.set_reversible_comp(c, c == 3);
    }
    cod.update_atk(&ParamAtk::new()).unwrap();
    assert_eq!(cod, rich_cod());
    assert_eq!(cod.spare.len(), 2);

    // A COC read from a codestream into a spare: the parser starts from
    // the state a new COC object has.
    let mut cod = rich_cod();
    let segment = {
        let mut out = Vec::new();
        cod.write_coc(&mut out, 4);
        out
    };
    let mut fresh = ParamCod::new();
    fresh.spcod = cod.spcod;
    fresh.sgcod = cod.sgcod;
    let mut kept = fresh.clone();
    for n in 0..4 {
        kept.spare.put(garbage_cod(n));
    }
    let mut f1 = crate::file::MemInfile::new(&segment);
    let mut f2 = crate::file::MemInfile::new(&segment);
    while !f1.eof() {
        f1.seek(2, crate::file::SeekFrom::Current);
        f2.seek(2, crate::file::SeekFrom::Current);
        fresh.read_coc(&mut f1, 4).unwrap();
        kept.read_coc(&mut f2, 4).unwrap();
    }
    assert!(
        fresh.cocs().len() >= 2,
        "{} COC segments",
        fresh.cocs().len()
    );
    assert_eq!(kept, fresh);
}

#[test]
fn cod_clone_from_equals_clone() {
    let rich = rich_cod();
    let mut copy = garbage_cod(1);
    for n in 0..3 {
        copy.children.push(garbage_cod(n));
    }
    copy.clone_from(&rich);
    assert_eq!(copy, rich);
    let mut copy = ParamCod::new();
    copy.clone_from(&rich);
    assert_eq!(copy, rich);
    let mut copy = rich_cod();
    copy.clone_from(&ParamCod::new());
    assert_eq!(copy, ParamCod::new());
}

#[test]
fn resolve_atk_into_equals_resolve_atk() {
    let main = ParamAtk::new();
    for index in [0, 1, 2] {
        let mut out = garbage_cod(0).atk;
        out.others.push(ParamAtk::new());
        let before = out.clone();
        let ok = main.resolve_atk_into(index, &mut out);
        match main.resolve_atk(index) {
            Some(want) => {
                assert!(ok);
                assert_eq!(out, want, "index {index}");
            }
            None => {
                assert!(!ok);
                // left as it was (NaN K: compare the parts that compare)
                assert_eq!(out.steps, before.steps);
                assert_eq!(out.others, before.others);
            }
        }
    }
}

#[test]
fn restarted_qcd_equals_new() {
    let mut qcd = ParamQcd::new();
    qcd.set_irrev_quant(0.001);
    for c in 0..4 {
        qcd.set_irrev_quant_comp(c, 0.002 * (c + 1) as f32);
    }
    qcd.recycle();
    assert_eq!(qcd, ParamQcd::new());
}

/// A SIZ for 4 components, linked to `cod`.
fn rich_siz(cod: &ParamCod) -> ParamSiz {
    let mut siz = ParamSiz::new();
    siz.set_image_extent(Point::new(1920, 32));
    siz.set_num_components(4);
    for c in 0..4 {
        siz.set_comp_info(c, Point::new(1, 1), 16, true).unwrap();
    }
    siz.set_cod(cod);
    siz.link(cod, None);
    siz
}

#[test]
fn restarted_siz_equals_new() {
    let cod = rich_cod();
    let mut siz = rich_siz(&cod);
    siz.init();
    assert_eq!(siz, ParamSiz::new());
    assert_eq!(siz.spare_cod.len(), 1, "the COD copy is kept");

    // a copy made into the kept storage, filled with garbage
    *siz.spare_cod.items_mut()[0] = garbage_cod(9);
    siz.set_image_extent(Point::new(1920, 32));
    siz.set_num_components(4);
    for c in 0..4 {
        siz.set_comp_info(c, Point::new(1, 1), 16, true).unwrap();
    }
    siz.set_cod(&cod);
    siz.link(&cod, None);
    assert_eq!(siz, rich_siz(&cod));
    assert_eq!(siz.spare_cod.len(), 0);
}

/// An increasing table of `n` points of up to 16 bits.
fn lut(n: u32, seed: u32) -> Vec<u32> {
    (0..n).map(|i| i * (65535 / n) + (i * seed) % 7).collect()
}

/// An NLT with a type 3 entry and type 4 entries for 3 components, with
/// both kinds of encoding table.
fn rich_nlt() -> ParamNlt {
    let mut nlt = ParamNlt::new();
    nlt.set_nonlinear_transform(3, nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT)
        .unwrap();
    for c in 0..3 {
        let pts = lut(300 + 50 * c, c);
        nlt.set_nonlinear_transform_lut(
            c,
            16,
            true,
            0,
            u32::MAX,
            16,
            pts.len() as u16,
            &pts,
            nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT,
            c == 1,
            0,
        )
        .unwrap();
    }
    nlt
}

/// An NLT object with every field and table set to garbage.
pub(crate) fn garbage_nlt(n: u16) -> ParamNlt {
    let mut p = ParamNlt::new();
    p.lnlt = 0xDEAD;
    p.cnlt = 9000 + n;
    p.enabled = true;
    let r = &mut p.rec;
    r.bdnlt = 0xFF;
    r.tnlt = 77;
    r.d_min = 5;
    r.d_max = 3;
    r.pt_val = 99;
    r.num_points = 12345;
    r.marker_points = vec![0xFFFF_FFFF; 1000];
    r.bytes_per_point = 3;
    r.dec_points = vec![f32::NAN; 1000];
    r.fd_min = 1e30;
    r.delta = -1.0;
    r.enc_num_points = 7;
    r.approx_encoding_nlt = true;
    r.approx_enc_points = vec![-7.0; 2000];
    r.precise_max_steps = 1234;
    r.precise_enc_dec_indices = vec![0xFFFF; 2000];
    r.precise_dec_lut = vec![f32::INFINITY; 1000];
    p.others.push(ParamNlt::new());
    p
}

#[test]
fn restarted_nlt_equals_new() {
    let mut nlt = rich_nlt();
    nlt.restart();
    assert_eq!(nlt, ParamNlt::new());
    assert_eq!(nlt.spare.len(), 4, "the per-component objects are kept");

    let mut rec = garbage_nlt(0).rec;
    rec.init();
    assert_eq!(rec, NltRec::new());
}

#[test]
fn nlt_from_poisoned_spares_equals_new() {
    // set by the caller
    let mut nlt = ParamNlt::new();
    for n in 0..6 {
        nlt.spare.put(garbage_nlt(n));
    }
    nlt.set_nonlinear_transform(3, nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT)
        .unwrap();
    for c in 0..3 {
        let pts = lut(300 + 50 * c, c);
        nlt.set_nonlinear_transform_lut(
            c,
            16,
            true,
            0,
            u32::MAX,
            16,
            pts.len() as u16,
            &pts,
            nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT,
            c == 1,
            0,
        )
        .unwrap();
    }
    assert_eq!(nlt, rich_nlt());

    // read from a codestream
    let mut segments = Vec::new();
    rich_nlt().write(&mut segments);
    let read = |nlt: &mut ParamNlt| {
        let mut f = crate::file::MemInfile::new(&segments);
        while !f.eof() {
            f.seek(2, crate::file::SeekFrom::Current);
            nlt.read(&mut f).unwrap();
        }
    };
    let mut fresh = ParamNlt::new();
    read(&mut fresh);
    assert_eq!(fresh.others().len(), 4);
    let mut kept = ParamNlt::new();
    for n in 0..6 {
        kept.spare.put(garbage_nlt(n));
    }
    read(&mut kept);
    assert_eq!(kept, fresh);

    // a restarted object reads the same, and the same again
    kept.restart();
    read(&mut kept);
    assert_eq!(kept, fresh);
}

#[test]
fn nlt_lut_errors_leave_the_same_state() {
    // A decreasing table is refused after the object is found or added;
    // what is left must not depend on kept storage.
    let bad = [10, 20, 15, 30];
    let mut fresh = ParamNlt::new();
    let mut kept = ParamNlt::new();
    kept.spare.put(garbage_nlt(0));
    for nlt in [&mut fresh, &mut kept] {
        let r = nlt.set_nonlinear_transform_lut(
            0,
            16,
            true,
            0,
            u32::MAX,
            16,
            4,
            &bad,
            nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT,
            false,
            0,
        );
        assert!(r.is_err());
    }
    assert_eq!(kept, fresh);
}

/// A QCD object with every field set to something no setter gives.
pub(crate) fn garbage_qcd(n: u16) -> ParamQcd {
    let mut p = ParamQcd::new();
    p.type_ = QcdType::QccMain;
    p.is_init = true;
    p.lqcd = 0xF00D;
    p.sqcd = 0xFF;
    p.spqcd = [0xC3; 2 * MAX_SUBBAND_COUNT as usize];
    p.num_subbands = 999;
    p.enabled = false;
    p.base_delta = f32::NAN;
    p.qfactor = 1e9;
    p.num_weights = 77;
    p.weights = [f32::NAN; MAX_SUBBAND_COUNT as usize];
    p.is_color_trans = true;
    p.num_decomps = 40;
    p.bit_depth = 99;
    p.is_signed = true;
    p.wavelet_kern = 7;
    p.sampling = Point::new(13, 17);
    p.comp_idx = 5000 + n;
    p
}

/// Fills every kept parameter object, and its spares, with garbage.
pub(crate) fn poison_params(p: &mut crate::codestream::KeptParams) {
    for n in 0..3 {
        p.cod.spare.put(garbage_cod(n));
        p.cod.children.push(garbage_cod(10 + n));
        p.nlt.spare.put(garbage_nlt(n));
        p.nlt.others.push(garbage_nlt(10 + n));
        p.qcd.children.push(garbage_qcd(n));
    }
    let children = std::mem::take(&mut p.cod.children);
    let spare = std::mem::take(&mut p.cod.spare);
    p.cod = garbage_cod(20);
    p.cod.children = children;
    p.cod.spare = spare;
    let children = std::mem::take(&mut p.qcd.children);
    p.qcd = garbage_qcd(20);
    p.qcd.children = children;
    p.nlt.rec = garbage_nlt(20).rec;
    p.nlt.lnlt = 0xBAD;
    p.nlt.cnlt = 3;
    p.nlt.enabled = true;
    let s = &mut p.siz;
    s.lsiz = 0xBAD;
    s.rsiz = 0xFFFF;
    s.xsiz = 0xDEAD_BEEF;
    s.ysiz = 7;
    s.xtsiz = 3;
    s.csiz = 3;
    s.cptr = vec![SizCompInfo::default(); 3];
    s.skipped_resolutions = 9;
    s.ws_kern_support_needed = true;
    s.cod = Some(Box::new(garbage_cod(21)));
    s.spare_cod.put(Box::new(garbage_cod(22)));
    s.dfs = Some(Box::new(ParamDfs::new()));
    p.atk.init_rev53();
    p.atk.katk = f32::NAN;
    p.dfs.ldfs = 0xBAD;
    p.dfs.others.push(ParamDfs::new());
}
