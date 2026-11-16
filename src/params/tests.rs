// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Tests for the marker segments.
//!
//! The `*.txt` files under `tests/data/params` were printed by a small C++
//! program linked against OpenJPH 0.33.0 (`params_dump.cpp`, kept with
//! those files); the `dump_*` functions below print the same text from the
//! Rust port, so each comparison checks parsed fields and derived values
//! (K_max, step sizes, precision, recon sizes) against the C++ code.

use super::*;
use crate::file::{MemInfile, SeekFrom};
use crate::types::{Point, Size};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/params")
}

fn read_data(name: &str) -> Vec<u8> {
    let p = data_dir().join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {}", p.display(), e))
}

fn read_text(name: &str) -> String {
    String::from_utf8(read_data(name)).unwrap()
}

/// Compares line by line, so a failure names the first differing line.
fn assert_same_text(got: &str, want: &str, what: &str) {
    let g: Vec<&str> = got.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    for (i, (a, b)) in g.iter().zip(w.iter()).enumerate() {
        assert_eq!(a, b, "{}: line {} differs", what, i + 1);
    }
    assert_eq!(g.len(), w.len(), "{}: line counts differ", what);
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

/// All main-header parameter objects, plus the raw bytes (marker
/// included) of each segment by marker code, in file order.
#[derive(Default)]
struct Header {
    siz: ParamSiz,
    cap: Option<ParamCap>,
    cod: ParamCod,
    qcd: ParamQcd,
    nlt: ParamNlt,
    dfs: ParamDfs,
    atk: ParamAtk,
    raw: Vec<(u16, Vec<u8>)>,
}

fn parse_main_header(data: &[u8]) -> crate::Result<Header> {
    let mut h = Header::default();
    let mut f = MemInfile::new(data);
    assert_eq!(f.read_u16(), Some(marker::SOC));
    while let Some(m) = f.read_u16() {
        let start = f.tell() - 2;
        match m {
            marker::SIZ => h.siz.read(&mut f)?,
            marker::CAP => {
                let mut c = ParamCap::new();
                c.read(&mut f)?;
                h.cap = Some(c);
            }
            marker::COD => h.cod.read(&mut f)?,
            marker::COC => h.cod.read_coc(&mut f, h.siz.get_num_components())?,
            marker::QCD => h.qcd.read(&mut f)?,
            marker::QCC => h.qcd.read_qcc(&mut f, h.siz.get_num_components())?,
            marker::NLT => h.nlt.read(&mut f)?,
            marker::DFS => h.dfs.read(&mut f)?,
            marker::ATK => h.atk.read(&mut f)?,
            marker::SOT => break,
            _ => {
                let l = f.read_u16().unwrap();
                assert!(f.seek(l as i64 - 2, SeekFrom::Current));
            }
        }
        h.raw.push((m, data[start..f.tell()].to_vec()));
    }
    h.cod.update_atk(&h.atk)?;
    h.siz
        .link(&h.cod, if h.dfs.exists() { Some(&h.dfs) } else { None });
    Ok(h)
}

fn kmax_and_delta(out: &mut String, h: &Header, c: u32, rev: bool) {
    let cdp = h.cod.get_coc(c);
    let qp = h.qcd.get_qcc(c);
    let nd = cdp.get_num_decompositions();
    let d = if cdp.is_dfs_defined() && h.dfs.exists() {
        h.dfs.get_dfs(cdp.get_dfs_index())
    } else {
        None
    };
    let bands = |r: u32| if r == 0 { 0..=0 } else { 1..=3 };
    write!(out, "KMAX {}", c).unwrap();
    for r in 0..=nd {
        for b in bands(r) {
            write!(out, " {}", qp.get_Kmax(d, nd, r, b)).unwrap();
        }
    }
    out.push('\n');
    if !rev {
        write!(out, "DELTA {}", c).unwrap();
        for r in 0..=nd {
            for b in bands(r) {
                let v = qp.get_irrev_delta(d, nd, c, r, b).unwrap();
                write!(out, " {:08x}", v.to_bits()).unwrap();
            }
        }
        out.push('\n');
    }
}

/// `print_derived` of params_dump.cpp.
fn dump_derived(out: &mut String, h: &Header) {
    let nc = h.siz.get_num_components();
    writeln!(out, "MAGB {}", h.qcd.get_MAGB()).unwrap();
    for c in 0..nc {
        let cdp = h.cod.get_coc(c);
        let qp = h.qcd.get_qcc(c);
        let nd = cdp.get_num_decompositions();
        let rev = cdp.access_atk().is_reversible();
        let rd = h.siz.get_recon_downsampling(c);
        writeln!(
            out,
            "COMP {} w={} h={} rw={} rh={} rds={}x{} nd={} rev={} guard={} prec={}",
            c,
            h.siz.get_width(c),
            h.siz.get_height(c),
            h.siz.get_recon_width(c),
            h.siz.get_recon_height(c),
            rd.x,
            rd.y,
            nd,
            rev as i32,
            qp.get_num_guard_bits(),
            qp.propose_precision(cdp, &h.cod, &h.qcd)
        )
        .unwrap();
        kmax_and_delta(out, h, c, rev);
        match h.nlt.get_nlt_rec(c) {
            Some(rec) => writeln!(
                out,
                "NLTREC {} type={} bd={} signed={}",
                c,
                rec.get_type(),
                rec.get_bit_depth(),
                rec.is_signed() as i32
            )
            .unwrap(),
            None => writeln!(out, "NLTREC {} none", c).unwrap(),
        }
    }
}

/// `dump_file` of params_dump.cpp, without the FILE line.
fn dump_header(h: &Header) -> String {
    let mut out = String::new();
    let s = &h.siz;
    if let Some(cap) = &h.cap {
        writeln!(out, "CAP pcap={:08x}", cap.get_Pcap()).unwrap();
    }
    writeln!(
        out,
        "SIZ rsiz={:04x} x={} y={} xo={} yo={} xt={} yt={} xto={} yto={} c={}",
        s.rsiz, s.xsiz, s.ysiz, s.xosiz, s.yosiz, s.xtsiz, s.ytsiz, s.xtosiz, s.ytosiz, s.csiz
    )
    .unwrap();
    for c in 0..s.csiz as u32 {
        let ci = s.cptr[c as usize];
        writeln!(
            out,
            "SSIZ {} {:02x} {} {} depth={} signed={}",
            c,
            ci.ssiz,
            ci.xrsiz,
            ci.yrsiz,
            s.get_bit_depth(c),
            s.is_signed(c) as i32
        )
        .unwrap();
    }
    for (i, p) in std::iter::once(&h.cod)
        .chain(h.cod.cocs().iter())
        .enumerate()
    {
        write!(
            out,
            "{} idx={} scod={:02x} prog={} layers={} mct={} nd={} bw={} bh={} bs={:02x} wt={} rev={} ct={}",
            if i == 0 { "COD" } else { "COC" },
            p.comp_idx,
            p.scod,
            p.sgcod.prog_order,
            p.sgcod.num_layers,
            p.sgcod.mc_trans,
            p.get_num_decompositions(),
            p.spcod.block_width,
            p.spcod.block_height,
            p.spcod.block_style,
            p.spcod.wavelet_trans,
            p.is_reversible() as i32,
            p.is_employing_color_transform() as i32
        )
        .unwrap();
        out.push_str(" lps=");
        for r in 0..=p.get_num_decompositions() {
            let z = p.get_log_precinct_size(r);
            write!(out, "{}x{},", z.w, z.h).unwrap();
        }
        out.push('\n');
    }
    for (i, p) in std::iter::once(&h.qcd)
        .chain(h.qcd.qccs().iter())
        .enumerate()
    {
        write!(
            out,
            "{} idx={} sqcd={:02x} n={} v=",
            if i == 0 { "QCD" } else { "QCC" },
            p.comp_idx,
            p.sqcd,
            p.num_subbands
        )
        .unwrap();
        for k in 0..p.num_subbands {
            if p.sqcd & 0x1F == 0 {
                write!(out, "{:02x},", p.get_spqcd_u8(k)).unwrap();
            } else {
                write!(out, "{:04x},", p.get_spqcd_u16(k)).unwrap();
            }
        }
        out.push('\n');
    }
    for p in std::iter::once(&h.nlt).chain(h.nlt.others().iter()) {
        let r = &p.rec;
        writeln!(
            out,
            "NLT c={} en={} bd={:02x} t={} np={} dmin={} dmax={} pt={}",
            p.cnlt, p.enabled as i32, r.bdnlt, r.tnlt, r.num_points, r.d_min, r.d_max, r.pt_val
        )
        .unwrap();
    }
    dump_derived(&mut out, h);
    out
}

fn dump_file(name: &str, data: &[u8]) -> String {
    let mut out = format!("FILE {}\n", name);
    match parse_main_header(data) {
        Ok(h) => out.push_str(&dump_header(&h)),
        Err(_) => out.push_str("ERROR\n"),
    }
    out
}

/// Re-writes SIZ, CAP, COD+COC and QCD+QCC of a parsed header and checks
/// the bytes equal the file's (which OpenJPH's `write` produced).
fn check_rewrite(name: &str, h: &mut Header) {
    let collect = |codes: &[u16]| -> Vec<u8> {
        h.raw
            .iter()
            .filter(|(m, _)| codes.contains(m))
            .flat_map(|(_, b)| b.clone())
            .collect()
    };
    let want_siz = collect(&[marker::SIZ]);
    let want_cap = collect(&[marker::CAP]);
    let want_cod = collect(&[marker::COD, marker::COC]);
    let want_qcd = collect(&[marker::QCD, marker::QCC]);
    let nc = h.siz.get_num_components();

    let mut v = Vec::new();
    assert!(h.siz.write(&mut v));
    assert_eq!(hex(&v), hex(&want_siz), "{}: SIZ", name);
    if let Some(cap) = h.cap.as_mut() {
        let mut v = Vec::new();
        assert!(cap.write(&mut v));
        assert_eq!(hex(&v), hex(&want_cap), "{}: CAP", name);
    }
    let mut v = Vec::new();
    assert!(h.cod.write(&mut v));
    assert!(h.cod.write_coc(&mut v, nc));
    assert_eq!(hex(&v), hex(&want_cod), "{}: COD/COC", name);
    let mut v = Vec::new();
    assert!(h.qcd.write(&mut v));
    assert!(h.qcd.write_qcc(&mut v, nc));
    assert_eq!(hex(&v), hex(&want_qcd), "{}: QCD/QCC", name);
}

/// Regenerates QCD/QCC and CAP from the parsed SIZ and COD, the way the
/// OpenJPH encoder (oracle/ojph_enc.cpp) did, and compares the bytes with
/// the file's. `qstep` is the encoder's `set_irrev_quant` argument.
fn check_regenerate(name: &str, h: &Header, qstep: f32) {
    let collect = |codes: &[u16]| -> Vec<u8> {
        h.raw
            .iter()
            .filter(|(m, _)| codes.contains(m))
            .flat_map(|(_, b)| b.clone())
            .collect()
    };
    let nc = h.siz.get_num_components();
    let mut qcd = ParamQcd::new();
    if !h.cod.is_reversible() {
        qcd.set_irrev_quant(qstep);
    }
    qcd.check_validity(&h.siz, &h.cod).unwrap();
    let mut v = Vec::new();
    qcd.write(&mut v);
    qcd.write_qcc(&mut v, nc);
    assert_eq!(
        hex(&v),
        hex(&collect(&[marker::QCD, marker::QCC])),
        "{}: regenerated QCD/QCC",
        name
    );
    if h.cap.is_some() {
        let mut cap = ParamCap::new();
        cap.check_validity(&h.cod, &qcd);
        let mut v = Vec::new();
        cap.write(&mut v);
        assert_eq!(
            hex(&v),
            hex(&collect(&[marker::CAP])),
            "{}: regenerated CAP",
            name
        );
    }
}

/// The codestreams copied into tests/data/params, and the encoder's
/// `--qstep` for each (1/256 is ojph_enc's default).
const FILES: &[(&str, f32)] = &[
    ("prog_lrcp.j2c", 1.0 / 256.0),
    ("prog_pcrl_sub.j2c", 1.0 / 256.0),
    ("prog_rpcl_tiles.j2c", 1.0 / 256.0),
    ("levels32_1x1.j2c", 1.0 / 256.0),
    ("irv_u8.j2c", 1.0 / 256.0),
    ("irv_rct_u12.j2c", 1.0 / 256.0),
    ("irv_s16.j2c", 0.001),
    ("coc_levels_rpcl.j2c", 1.0 / 256.0),
    ("nlt3_comp1_of3.j2c", 1.0 / 256.0),
    ("nlt3_rct.j2c", 1.0 / 256.0),
    ("s16_rct_4c_300x8.j2c", 1.0 / 256.0),
];

#[test]
fn main_headers_match_cpp_dump() {
    let want = read_text("files.txt");
    let mut got = String::new();
    for (name, _) in FILES {
        got.push_str(&dump_file(name, &read_data(name)));
    }
    assert_same_text(&got, &want, "files.txt");
}

#[test]
fn main_headers_rewrite_byte_exact() {
    for (name, _) in FILES {
        let mut h = parse_main_header(&read_data(name)).unwrap();
        check_rewrite(name, &mut h);
    }
}

#[test]
fn quantization_regenerates_encoder_output() {
    for (name, qstep) in FILES {
        let h = parse_main_header(&read_data(name)).unwrap();
        check_regenerate(name, &h, *qstep);
    }
}

/// Optional sweep over whole directories of codestreams, with the C++
/// dumps alongside: set HTJ2K_PARAMS_SWEEP to `dir1:dir2:...`; each
/// `<dir>/<name>.j2c` is compared with `<dumps>/dumps_<dirname>_<name>.txt`
/// where `<dumps>` is HTJ2K_PARAMS_SWEEP_DUMPS. Also checks the byte-exact
/// rewrite, and the regeneration where the encoder's qstep is the default.
#[test]
fn sweep_directories_if_requested() {
    let dirs = match std::env::var("HTJ2K_PARAMS_SWEEP") {
        Ok(v) => v,
        Err(_) => return,
    };
    let dumps = PathBuf::from(std::env::var("HTJ2K_PARAMS_SWEEP_DUMPS").unwrap());
    let mut n = 0;
    for dir in dirs.split(':') {
        let dir = Path::new(dir);
        let dname = dir.file_name().unwrap().to_str().unwrap().to_string();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            if p.extension().and_then(|e| e.to_str()) != Some("j2c") {
                continue;
            }
            let stem = p.file_stem().unwrap().to_str().unwrap();
            let fname = p.file_name().unwrap().to_str().unwrap();
            let data = std::fs::read(&p).unwrap();
            let want = std::fs::read_to_string(dumps.join(format!("dumps_{}_{}.txt", dname, stem)))
                .unwrap();
            assert_same_text(&dump_file(fname, &data), &want, fname);
            if let Ok(mut h) = parse_main_header(&data) {
                check_rewrite(fname, &mut h);
                if h.cod.is_reversible() || stem != "irv_s16" {
                    check_regenerate(fname, &h, 1.0 / 256.0);
                }
            }
            n += 1;
        }
    }
    eprintln!("swept {} codestreams", n);
}

// ---------------------------------------------------------------------------
// Encoder-side parameter generation, against `params_dump quant`.

fn quant_config(k: i32) -> String {
    let mut siz = ParamSiz::new();
    let mut cod = ParamCod::new();
    let mut qcd = ParamQcd::new();
    let mut cap = ParamCap::new();
    let mut nlt = ParamNlt::new();
    let atk = ParamAtk::new();
    let (mut nc, mut depth, mut levels) = (3u32, 8u32, 5u32);
    let (mut sgn, mut rev, mut rct) = (false, true, false);
    let mut sub = [Point::new(1, 1); 4];
    match k {
        0 => rct = true,
        1 => {
            rev = false;
            depth = 12;
            levels = 6;
            rct = true;
        }
        2 => {
            rev = false;
            depth = 10;
            sub[1] = Point::new(2, 2);
            sub[2] = Point::new(2, 2);
        }
        3 => {
            rev = false;
            nc = 1;
            depth = 16;
            levels = 2;
        }
        4 => {
            depth = 16;
            sgn = true;
            levels = 3;
        }
        5 => {
            nc = 4;
            rev = false;
            levels = 4;
            sub[1] = Point::new(2, 1);
            sub[2] = Point::new(2, 1);
        }
        6 => {
            nc = 1;
            depth = 32;
            levels = 32;
        }
        7 => {
            nc = 2;
            depth = 16;
            sgn = true;
            levels = 1;
            rev = false;
        }
        // NLT type 3 for ALL_COMPS: uniform components (8), mixed bit
        // depths (9), one component with its own type 0 and mixed
        // signedness (10)
        8..=10 => {
            depth = 16;
            sgn = true;
            levels = 2;
        }
        _ => unreachable!(),
    }
    siz.set_image_extent(Point::new(67, 45));
    siz.set_image_offset(Point::new(3, 1));
    siz.set_tile_size(Size::new(67, 45));
    siz.set_num_components(nc);
    for c in 0..nc {
        let d = if k == 9 && c == 2 { 12 } else { depth };
        let s = if k == 10 && c == 2 { false } else { sgn };
        siz.set_component(c, sub[c as usize], d, s);
    }
    cod.set_num_decomposition(levels).unwrap();
    cod.set_block_dims(32, 32).unwrap();
    cod.set_color_transform(rct);
    cod.set_reversible(rev);
    if k == 1 {
        cod.set_progression_order("LRCP").unwrap();
        cod.set_precinct_size(&[Size::new(64, 64), Size::new(128, 128)])
            .unwrap();
    }
    if k == 2 {
        qcd.set_qfactor(85.0).unwrap();
    }
    if k == 3 {
        let w = [1.0f32, 1.5, 2.0, 0.5, 0.75, 1.25, 3.0];
        qcd.set_irrev_quant_weights(0.002, &w).unwrap();
    }
    if k == 4 {
        cod.set_num_decomposition_comp(1, 2).unwrap();
        cod.set_block_dims_comp(1, 16, 64).unwrap();
        cod.set_reversible_comp(2, false);
        qcd.set_irrev_quant_comp(2, 0.01);
        cod.set_progression_order("CPRL").unwrap();
    }
    if k == 5 {
        qcd.set_qfactor(97.5).unwrap();
        qcd.set_qfactor_comp(3, CompType::Y, 60.0).unwrap();
        cod.set_progression_order("LRCP").unwrap();
    }
    if k == 7 {
        nlt.set_nonlinear_transform(0, 3).unwrap();
        qcd.set_qfactor(70.0).unwrap();
    }
    if k >= 8 {
        nlt.set_nonlinear_transform(ALL_COMPS as u32, 3).unwrap();
    }
    if k == 10 {
        nlt.set_nonlinear_transform(1, 0).unwrap();
    }
    let r = (|| -> crate::Result<()> {
        siz.set_cod(&cod);
        siz.check_validity()?;
        cod.check_validity(&siz)?;
        cod.update_atk(&atk)?;
        qcd.check_validity(&siz, &cod)?;
        cap.check_validity(&cod, &qcd);
        nlt.check_validity(&mut siz, &cod)?;
        Ok(())
    })();
    if r.is_err() {
        return format!("CONFIG {} ERROR\n", k);
    }
    let mut out = format!("CONFIG {}\n", k);
    let mut v = Vec::new();
    siz.write(&mut v);
    writeln!(out, "SIZ {}", hex(&v)).unwrap();
    let mut v = Vec::new();
    cap.write(&mut v);
    writeln!(out, "CAP {}", hex(&v)).unwrap();
    let mut v = Vec::new();
    cod.write(&mut v);
    cod.write_coc(&mut v, nc);
    writeln!(out, "COD {}", hex(&v)).unwrap();
    let mut v = Vec::new();
    qcd.write(&mut v);
    qcd.write_qcc(&mut v, nc);
    writeln!(out, "QCD {}", hex(&v)).unwrap();
    let mut v = Vec::new();
    nlt.write(&mut v);
    writeln!(out, "NLT {}", hex(&v)).unwrap();
    let h = Header {
        siz,
        cap: Some(cap),
        cod,
        qcd,
        nlt,
        ..Default::default()
    };
    dump_derived(&mut out, &h);
    out
}

#[test]
fn quantization_configs_match_cpp() {
    let want = read_text("quant.txt");
    let mut got = String::new();
    for k in 0..11 {
        got.push_str(&quant_config(k));
    }
    assert_same_text(&got, &want, "quant.txt");
}

// ---------------------------------------------------------------------------
// ATK and DFS, against `params_dump atk` and `params_dump segs ...`.

fn dump_kernel(out: &mut String, tag: &str, p: &ParamAtk) {
    write!(
        out,
        "{} latk={} satk={:04x} k={:08x} n={}",
        tag,
        p.latk,
        p.satk,
        p.katk.to_bits(),
        p.get_num_steps()
    )
    .unwrap();
    for s in &p.steps {
        if p.is_reversible() {
            write!(out, " e={} b={} a={}", s.rev_e, s.rev_b, s.rev_a).unwrap();
        } else {
            write!(out, " {:08x}", s.irv_a.to_bits()).unwrap();
        }
    }
    out.push('\n');
}

#[test]
fn default_kernels_match_cpp() {
    let mut a = ParamAtk::new();
    let mut out = String::new();
    let p97 = a.get_atk(0).unwrap().clone();
    dump_kernel(&mut out, "IRV97", &p97);
    let p53 = a.get_atk(1).unwrap().clone();
    dump_kernel(&mut out, "REV53", &p53);
    assert_same_text(&out, &read_text("atk.txt"), "atk.txt");
    // the first kernel asked for takes over the unused object, the second
    // is appended; asking again finds them
    assert_eq!(a.get_index(), 0);
    assert_eq!(a.others.len(), 1);
    assert!(a.get_atk(1).unwrap().is_reversible());
    assert_eq!(a.others.len(), 1);
    assert!(a.get_atk(7).is_none());
    // resolve_atk gives the same kernels without changing anything
    let b = ParamAtk::new();
    assert_eq!(b.resolve_atk(0).unwrap(), p97);
    assert_eq!(b.resolve_atk(1).unwrap(), p53);
    assert!(b.resolve_atk(2).is_none());
    assert_eq!(b, ParamAtk::new());
}

/// Hand-built ATK and DFS segment bodies (after the marker) are in
/// `segs.args` as `kind hex` pairs, the arguments given to `params_dump
/// segs`, which printed segs.txt. Several are meant to fail.
#[test]
fn atk_dfs_segments_match_cpp() {
    let segs = segs();
    let mut atk = ParamAtk::new();
    let mut dfs = ParamDfs::new();
    let mut out = String::new();
    for (kind, body) in &segs {
        let mut f = MemInfile::new(body);
        let r = if *kind == "atk" {
            atk.read(&mut f)
        } else {
            dfs.read(&mut f)
        };
        writeln!(out, "{} {}", kind, if r.is_ok() { "ok" } else { "ERROR" }).unwrap();
    }
    for p in atk.iter() {
        write!(
            out,
            "ATK idx={} latk={} satk={:04x} rev={} k={:08x} n={}",
            p.get_index(),
            p.latk,
            p.satk,
            p.is_reversible() as i32,
            p.katk.to_bits(),
            p.get_num_steps()
        )
        .unwrap();
        for s in &p.steps {
            if p.is_reversible() {
                write!(out, " e={} b={} a={}", s.rev_e, s.rev_b, s.rev_a).unwrap();
            } else {
                write!(out, " {:08x}", s.irv_a.to_bits()).unwrap();
            }
        }
        out.push('\n');
    }
    for p in std::iter::once(&dfs).chain(dfs.others.iter()) {
        write!(out, "DFS s={} ids={}", p.sdfs, p.ids).unwrap();
        if p.ids == 0 {
            // params_dump skips these: C++ indexes out of bounds
            out.push('\n');
            continue;
        }
        for l in 1..=8 {
            write!(out, " {}", p.get_dwt_type(l) as u8).unwrap();
        }
        let f = p.get_res_downsamp(3);
        write!(out, " rd3={}x{} sb=", f.x, f.y).unwrap();
        for r in 1..=4 {
            for s in 1..=3 {
                write!(out, "{},", p.get_subband_idx(5, r, s)).unwrap();
            }
        }
        out.push('\n');
    }
    assert_same_text(&out, &read_text("segs.txt"), "segs.txt");
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn segs() -> Vec<(&'static str, Vec<u8>)> {
    read_text("segs.args")
        .split_whitespace()
        .collect::<Vec<_>>()
        .chunks(2)
        .map(|c| (if c[0] == "atk" { "atk" } else { "dfs" }, unhex(c[1])))
        .collect()
}

// ---------------------------------------------------------------------------
// Round trips and error paths that need no C++ reference.

#[test]
fn sot_round_trip_and_resilience() {
    let mut s = ParamSot::default();
    s.init(0, 7, 1, 3);
    let mut v = Vec::new();
    s.write(&mut v, 1000);
    assert_eq!(&v[..2], &[0xFF, 0x90]);
    let mut r = ParamSot::default();
    let mut f = MemInfile::new(&v[2..]);
    assert!(r.read(&mut f, false).unwrap());
    assert_eq!(
        (
            r.get_tile_index(),
            r.get_tile_part_index(),
            r.get_num_tile_parts()
        ),
        (7, 1, 3)
    );
    assert_eq!(r.get_payload_length(), 1002); // Psot = payload + 14, minus 12
    let mut v2 = Vec::new();
    s.write_with_parts(&mut v2, 1000, 2, 5);
    assert_eq!(&v2[10..], &[2, 5]);
    // truncated: error, or Ok(false) and cleared fields when resilient
    let mut f = MemInfile::new(&v[2..8]);
    assert_eq!(r.read(&mut f, false).unwrap_err().code(), 0x00050095);
    let mut f = MemInfile::new(&v[2..8]);
    assert!(!r.read(&mut f, true).unwrap());
    assert_eq!(r, ParamSot::default());
    // Psot below 12 must not panic
    let bad = [0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01];
    let mut f = MemInfile::new(&bad);
    assert!(r.read(&mut f, false).unwrap());
    assert_eq!(r.get_payload_length(), 5u32.wrapping_sub(12));
}

#[test]
fn tlm_write_splits_segments() {
    let mut t = ParamTlm::new();
    let n = ParamTlm::MAX_PAIRS_PER_SEG + 2;
    t.init(n).unwrap();
    for i in 0..n {
        t.set_next_pair(i as u16, i);
    }
    let mut v = Vec::new();
    t.write(&mut v);
    let first = 6 + 6 * ParamTlm::MAX_PAIRS_PER_SEG as usize;
    assert_eq!(v.len(), first + 6 + 12);
    assert_eq!(&v[..6], &[0xFF, 0x55, 0xFF, 0xFA, 0, 0x60]); // 4 + 6 * 10921
    assert_eq!(&v[first..first + 6], &[0xFF, 0x55, 0, 16, 1, 0x60]);
    assert_eq!(&v[6..12], &[0, 0, 0, 0, 0, 14]);
    assert_eq!(
        t.init(ParamTlm::MAX_PAIRS_PER_SEG * ParamTlm::MAX_SEGMENTS + 1)
            .unwrap_err()
            .code(),
        0x000500B1
    );
}

#[test]
fn nlt_lut_round_trip() {
    let mut n = ParamNlt::new();
    let pts: Vec<u32> = (0..9).map(|i| i * i * 100 + i).collect();
    n.set_nonlinear_transform_lut(1, 12, true, 10, 1 << 30, 14, 9, &pts, 2, false, 0)
        .unwrap();
    n.set_nonlinear_transform(0, 3).unwrap();
    let mut v = Vec::new();
    assert!(n.write(&mut v));
    let mut r = ParamNlt::new();
    let mut f = MemInfile::new(&v);
    while let Some(m) = f.read_u16() {
        assert_eq!(m, marker::NLT);
        r.read(&mut f).unwrap();
    }
    let a = r.get_nlt_rec(1).unwrap();
    assert_eq!(
        (a.get_type(), a.get_bit_depth(), a.is_signed()),
        (2, 12, true)
    );
    assert_eq!(
        (a.d_min, a.d_max, a.pt_val, a.num_points),
        (10, 1 << 30, 14, 9)
    );
    assert_eq!(a.marker_points, pts);
    assert_eq!(a.dec_points.len(), 9 + 2);
    assert_eq!(a.dec_points[0], a.dec_points[1]);
    assert_eq!(a.dec_points[10], a.dec_points[9]);
    assert_eq!(r.get_nonlinear_transform(0), Some((1, false, 3)));
    assert_eq!(r.get_nonlinear_transform(5), None);
    // the encoder side built its tables
    let e = n.get_nlt_rec(1).unwrap();
    assert!(e.enc_num_points >= 2);
    assert_eq!(
        e.precise_enc_dec_indices.len(),
        e.enc_num_points as usize + 4
    );
}

#[test]
fn nlt_rejects_bad_segments() {
    let cases: &[(&[u8], u32)] = &[
        (&[0, 6, 0, 0, 7], 0x000501E1),
        (&[0, 6, 0, 0, 7, 1], 0x00050141),
        (&[0, 7, 0, 0, 7, 3], 0x00050142),
        (&[0, 6, 0x40, 0, 7, 3], 0x00050143),
        (&[0, 6, 0, 0, 0x7F, 3], 0x00050144),
        (&[0, 20, 0, 0, 7, 2, 0, 0], 0x000501E2),
        (
            &[0, 20, 0, 0, 7, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 8],
            0x00050145,
        ),
        (
            &[0, 20, 0, 0, 7, 2, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 8],
            0x00050146,
        ),
        (
            &[0, 20, 0, 0, 7, 2, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2, 33],
            0x00050147,
        ),
        (
            &[0, 20, 0, 0, 7, 2, 0, 1, 0, 0, 0, 1, 0, 0, 0, 2, 8, 5],
            0x000501E3,
        ),
    ];
    for (bytes, code) in cases {
        let mut n = ParamNlt::new();
        let mut f = MemInfile::new(bytes);
        assert_eq!(n.read(&mut f).unwrap_err().code(), *code, "{:02x?}", bytes);
    }
}

#[test]
fn coc_and_qcc_for_default_index_are_rejected() {
    // With 257+ components a COC/QCC may carry index 0xFFFF; C++ get_coc /
    // get_qcc find the main object (index OJPH_*_DEFAULT) first, so the
    // codestream reports a duplicate (COC) or an out-of-range index (QCC).
    let coc = [0, 10, 0xFF, 0xFF, 0, 5, 4, 4, 0x40, 1];
    let mut cod = ParamCod::new();
    let e = cod.read_coc(&mut MemInfile::new(&coc), 300).unwrap_err();
    assert_eq!(e.code(), 0x00030057);
    assert!(std::ptr::eq(cod.get_coc(0xFFFF), &cod));
    let qcc = [0, 6, 0xFF, 0xFF, 0, 0x40];
    let mut qcd = ParamQcd::new();
    assert_eq!(
        qcd.read_qcc(&mut MemInfile::new(&qcc), 300)
            .unwrap_err()
            .code(),
        0x00030054
    );
    assert!(std::ptr::eq(qcd.get_qcc(0xFFFF), &qcd));
}

#[test]
fn comment_exchange() {
    let mut c = CommentExchange::new();
    c.set_string("hello").unwrap();
    assert_eq!((c.data(), c.len(), c.get_Rcom()), (&b"hello"[..], 5, 1));
    c.set_data(&[1, 2, 3]).unwrap();
    assert_eq!((c.data(), c.len(), c.get_Rcom()), (&[1u8, 2, 3][..], 3, 0));
    assert_eq!(c.set_data(&vec![0; 65532]).unwrap_err().code(), 0x000500C2);
}

#[test]
fn public_setters_validate() {
    let mut cod = ParamCod::new();
    assert_eq!(
        cod.set_num_decomposition(33).unwrap_err().code(),
        0x00050001
    );
    assert_eq!(cod.set_block_dims(0, 64).unwrap_err().code(), 0x00050011);
    assert_eq!(cod.set_block_dims(128, 64).unwrap_err().code(), 0x00050011);
    assert_eq!(cod.set_block_dims(48, 64).unwrap_err().code(), 0x00050011);
    cod.set_block_dims(1024, 4).unwrap();
    assert_eq!(cod.get_block_dims(), Size::new(1024, 4));
    assert_eq!(
        cod.set_precinct_size(&[Size::new(0, 4)])
            .unwrap_err()
            .code(),
        0x00050021
    );
    assert_eq!(
        cod.set_precinct_size(&[Size::new(3, 4)])
            .unwrap_err()
            .code(),
        0x00050022
    );
    assert_eq!(
        cod.set_precinct_size(&[Size::new(1 << 16, 4)])
            .unwrap_err()
            .code(),
        0x00050023
    );
    assert_eq!(
        cod.set_precinct_size(&[Size::new(4, 4), Size::new(1, 4)])
            .unwrap_err()
            .code(),
        0x00050024
    );
    assert_eq!(
        cod.set_progression_order("ABCD").unwrap_err().code(),
        0x00050031
    );
    assert_eq!(
        cod.set_progression_order("RPC").unwrap_err().code(),
        0x00050032
    );
    for po in ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"] {
        cod.set_progression_order(po).unwrap();
        assert_eq!(cod.get_progression_order_as_string(), po);
        assert_eq!(cod.get_progression_order_enum().unwrap().as_str(), po);
    }
    let mut qcd = ParamQcd::new();
    assert_eq!(qcd.set_qfactor(0.5).unwrap_err().code(), 0x00050181);
    assert_eq!(
        qcd.set_qfactor_comp(1, CompType::Cb, 101.0)
            .unwrap_err()
            .code(),
        0x00050191
    );
    assert_eq!(
        qcd.set_irrev_quant_weights(0.1, &[]).unwrap_err().code(),
        0x000501A1
    );
    assert_eq!(
        qcd.set_irrev_quant_weights(0.1, &[1.0, 0.0])
            .unwrap_err()
            .code(),
        0x000501A2
    );
    // the colour transform needs three matching components
    let mut siz = ParamSiz::new();
    siz.set_num_components(3);
    for c in 0..3 {
        siz.set_component(c, Point::new(1, 1), if c == 2 { 10 } else { 8 }, false);
    }
    let mut cod = ParamCod::new();
    cod.set_color_transform(true);
    assert_eq!(cod.check_validity(&siz).unwrap_err().code(), 0x00040014);
}

#[test]
fn truncated_main_header_segments_error_not_panic() {
    // every prefix of every committed header segment must give an error
    for (name, _) in FILES {
        let h = parse_main_header(&read_data(name)).unwrap();
        for (m, bytes) in &h.raw {
            for len in 2..bytes.len() {
                let body = &bytes[2..len];
                let mut f = MemInfile::new(body);
                let num_comps = h.siz.get_num_components();
                let r = match *m {
                    marker::SIZ => ParamSiz::new().read(&mut f),
                    marker::CAP => ParamCap::new().read(&mut f),
                    marker::COD => ParamCod::new().read(&mut f),
                    marker::COC => ParamCod::new().read_coc(&mut f, num_comps),
                    marker::QCD => ParamQcd::new().read(&mut f),
                    marker::QCC => ParamQcd::new().read_qcc(&mut f, num_comps),
                    marker::NLT => ParamNlt::new().read(&mut f),
                    _ => continue,
                };
                assert!(
                    r.is_err(),
                    "{}: {:04x} truncated to {} bytes parsed",
                    name,
                    m,
                    len
                );
            }
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    // a small xorshift generator; every reader on arbitrary input
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20000 {
        let len = (next() % 64) as usize;
        let mut b: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if len >= 2 && next() % 2 == 0 {
            // plausible length field
            b[0] = 0;
            b[1] = len as u8;
        }
        let nc = (next() % 300) as u32;
        let mut f = MemInfile::new(&b);
        let _ = ParamSiz::new().read(&mut f);
        let _ = ParamCap::new().read(&mut MemInfile::new(&b));
        let mut cod = ParamCod::new();
        let _ = cod.read(&mut MemInfile::new(&b));
        let _ = cod.read_coc(&mut MemInfile::new(&b), nc);
        let mut qcd = ParamQcd::new();
        let _ = qcd.read(&mut MemInfile::new(&b));
        let _ = qcd.read_qcc(&mut MemInfile::new(&b), nc);
        let mut nlt = ParamNlt::new();
        let _ = nlt.read(&mut MemInfile::new(&b));
        let mut dfs = ParamDfs::new();
        let _ = dfs.read(&mut MemInfile::new(&b));
        let _ = dfs.read(&mut MemInfile::new(&b));
        for l in 0..40 {
            let _ = dfs.get_dwt_type(l);
            let _ = dfs.get_subband_idx(l, l % 5, 1 + l % 3);
        }
        let mut atk = ParamAtk::new();
        let _ = atk.read(&mut MemInfile::new(&b));
        let _ = atk.read(&mut MemInfile::new(&b));
        let _ = ParamSot::default().read(&mut MemInfile::new(&b), next() % 2 == 0);
        // derived values on whatever was parsed
        let _ = cod.update_atk(&atk);
        for r in 0..4 {
            for s in 0..4 {
                let _ = qcd.get_Kmax(Some(&dfs), 3, r, s);
                let _ = qcd.get_irrev_delta(Some(&dfs), 3, 0, r, s);
            }
        }
        let _ = qcd.get_MAGB();
        let _ = qcd.propose_precision(&cod, &cod, &qcd);
        let _ = cod.get_log_precinct_size(40);
    }
}

// ---------------------------------------------------------------------------
// LUT nonlinearities (types 2 and 4), against `params_dump nlt`: encoder
// tables after `set_nonlinear_transform`, the written segment, and decoder
// tables after reading it back. Tables are compared by FNV-1a hashes over
// the ranges C++ may index.

fn fnv(h: &mut u64, v: u32) {
    for i in 0..4 {
        *h ^= ((v >> (8 * i)) & 0xFF) as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn dump_rec(out: &mut String, tag: &str, r: &NltRec) {
    write!(
        out,
        "{} t={} bd={:02x} np={} pt={} bpp={} fd={:08x},{:08x} ft={:08x},{:08x} delta={:08x} inv={:08x} mul={:08x} enc={} steps={}",
        tag,
        r.tnlt,
        r.bdnlt,
        r.num_points,
        r.pt_val,
        r.bytes_per_point,
        r.fd_min.to_bits(),
        r.fd_max.to_bits(),
        r.ft_min.to_bits(),
        r.ft_max.to_bits(),
        r.delta.to_bits(),
        r.inv_delta.to_bits(),
        r.multiplier.to_bits(),
        r.enc_num_points,
        r.precise_max_steps
    )
    .unwrap();
    let hash_f = |v: &[f32]| {
        let mut h = 0xcbf29ce484222325u64;
        for x in v {
            fnv(&mut h, x.to_bits());
        }
        h
    };
    if !r.dec_points.is_empty() {
        write!(out, " dec={:016x}", hash_f(&r.dec_points)).unwrap();
    }
    if !r.approx_enc_points.is_empty() {
        write!(out, " approx={:016x}", hash_f(&r.approx_enc_points)).unwrap();
    }
    if !r.precise_enc_dec_indices.is_empty() {
        let o = NltRec::ENC_POINTS_OFFSET;
        let mut h = 0xcbf29ce484222325u64;
        for &x in &r.precise_enc_dec_indices[o..o + r.enc_num_points as usize] {
            fnv(&mut h, x as u32);
        }
        write!(out, " idx={:016x}", h).unwrap();
        write!(out, " lut={:016x}", hash_f(&r.precise_dec_lut)).unwrap();
    }
    out.push('\n');
}

fn nlt_lut_case(k: i32) -> String {
    let (mut pt, mut enc, mut approx, mut ty, mut bd, mut sg) =
        (8u32, 0u32, false, 2u8, 12u8, false);
    let (dmin, dmax) = (1000u32, 4_000_000_000u32);
    let v: Vec<u32> = match k {
        0 => {
            let mut v: Vec<u32> = (0..17u32).map(|i| i * i * 255 / 256).collect();
            v[0] = 0;
            for i in 1..v.len() {
                if v[i] <= v[i - 1] {
                    v[i] = v[i - 1] + 1;
                }
            }
            v
        }
        1 => {
            pt = 14;
            approx = true;
            (0..100u32).map(|i| i * 150 + (i % 7)).collect()
        }
        2 => {
            pt = 20;
            ty = 4;
            sg = true;
            bd = 30;
            (0..300u32).map(|i| i * i * 3 + 5).collect()
        }
        3 => {
            pt = 16;
            enc = 500;
            (0..40u32)
                .map(|i| 1000 + i * 1000 + if i > 20 { 37 } else { 0 })
                .collect()
        }
        4 => {
            pt = 32;
            approx = true;
            enc = 64;
            (0..9u32)
                .map(|i| i.wrapping_mul(400_000_000).wrapping_add(3))
                .collect()
        }
        5 => {
            pt = 4;
            (0..2u32).map(|i| i * 15).collect()
        }
        _ => unreachable!(),
    };
    let mut out = format!("NLTCASE {}\n", k);
    let mut nlt = ParamNlt::new();
    let np = v.len() as u16;
    if nlt
        .set_nonlinear_transform_lut(1, bd, sg, dmin, dmax, pt as u8, np, &v, ty, approx, enc)
        .is_err()
    {
        out.push_str("SET ERROR\n");
        return out;
    }
    dump_rec(&mut out, "ENC", &nlt.others[0].rec);
    let mut w = Vec::new();
    nlt.write(&mut w);
    writeln!(out, "NLT {}", hex(&w)).unwrap();
    let mut r = ParamNlt::new();
    r.read(&mut MemInfile::new(&w[2..])).unwrap();
    dump_rec(&mut out, "DEC", &r.others[0].rec);
    out
}

#[test]
fn nlt_lut_tables_match_cpp() {
    let mut got = String::new();
    for k in 0..6 {
        got.push_str(&nlt_lut_case(k));
    }
    assert_same_text(&got, &read_text("nlt.txt"), "nlt.txt");
}
