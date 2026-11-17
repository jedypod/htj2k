// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Feeds the encoder random configurations and random samples, legal or
//! not, and misuses its API. Whatever it is given, it must return, with
//! a codestream or with an error, and must not panic. Run it in a debug
//! build too, so that arithmetic overflow counts as a failure.
//!
//! When the encoder accepts a configuration, its codestream must decode
//! with the crate's decoder to images of the expected sizes. The samples
//! are not compared: out-of-range input is not meant to round-trip, and
//! the OpenJPH bugs listed in `oracle/README.md` are reproduced on
//! purpose.

mod common;

use common::decode::decode_to_raw;
use htj2k::codestream::Encoder;
use htj2k::params::{nonlinearity, CommentExchange};
use htj2k::types::{Point, Size};
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Rng(u64);

impl Rng {
    /// splitmix64
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In `0..n`.
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }

    /// In `lo..=hi`.
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.below(hi - lo + 1)
    }

    /// True one time in `n`.
    fn one_in(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
}

/// Bounds the lines one test pushes, whatever the configuration says.
const MAX_LINES: usize = 1 << 14;

/// Sets every parameter at random, mostly to legal values. Setter errors
/// are expected; they only mean that parameter keeps its old value.
fn random_encoder(rng: &mut Rng) -> (Encoder, String) {
    let mut enc = Encoder::new();
    let mut d = String::new();
    let w = if rng.one_in(30) {
        rng.range(1, 300)
    } else {
        rng.range(1, 70)
    };
    let h = rng.range(1, 40);
    let (ox, oy) = if rng.one_in(3) {
        (rng.below(12), rng.below(12))
    } else {
        (0, 0)
    };
    let num_comps = if rng.one_in(40) {
        rng.below(20)
    } else {
        rng.range(1, 4)
    };
    let depth = if rng.one_in(30) {
        rng.below(40)
    } else {
        rng.range(1, 32)
    };
    let signed = rng.one_in(2);
    d += &format!("{w}x{h}+{ox},{oy} comps {num_comps} depth {depth} signed {signed}");
    {
        let siz = enc.access_siz_mut();
        siz.set_image_extent(Point::new(w + ox, h + oy));
        siz.set_image_offset(Point::new(ox, oy));
        if rng.one_in(3) {
            let t = Size::new(rng.range(1, 48), rng.range(1, 32));
            let slack = u32::from(rng.one_in(10));
            let to = Point::new(rng.below(ox + 1 + slack), rng.below(oy + 1 + slack));
            d += &format!(" tile {}x{}+{},{}", t.w, t.h, to.x, to.y);
            siz.set_tile_size(t);
            siz.set_tile_offset(to);
        }
        siz.set_num_components(num_comps);
        for c in 0..num_comps {
            let sub = if rng.one_in(50) {
                Point::new(rng.below(5), rng.below(5))
            } else if rng.one_in(4) {
                Point::new(rng.range(1, 4), rng.range(1, 4))
            } else {
                Point::new(1, 1)
            };
            let dc = if rng.one_in(10) {
                rng.range(1, 32)
            } else {
                depth
            };
            if sub != Point::new(1, 1) || dc != depth {
                d += &format!(" c{c}:{}x{}/{dc}", sub.x, sub.y);
            }
            let _ = siz.set_component(c, sub, dc, signed);
        }
    }
    {
        let cod = enc.access_cod_mut();
        let levels = if rng.one_in(20) {
            rng.below(40)
        } else {
            rng.below(7)
        };
        let (bw, bh) = if rng.one_in(20) {
            (rng.below(2000), rng.below(2000))
        } else {
            let lw = rng.range(2, 10);
            let lh = rng.range(2, 12 - lw.min(10)).min(10);
            (1 << lw, 1 << lh)
        };
        d += &format!(" levels {levels} block {bw}x{bh}");
        let _ = cod.set_num_decomposition(levels);
        let _ = cod.set_block_dims(bw, bh);
        if rng.one_in(3) {
            let p: Vec<Size> = (0..rng.range(1, 4))
                .map(|_| {
                    let lo = u32::from(!rng.one_in(10));
                    Size::new(1 << rng.range(lo, 8), 1 << rng.range(lo, 8))
                })
                .collect();
            d += &format!(" precincts {p:?}");
            let _ = cod.set_precinct_size(&p);
        }
        let prog = ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL", "XXXX"][rng.below(6) as usize];
        d += &format!(" {prog}");
        let _ = cod.set_progression_order(prog);
        let rct = if num_comps >= 3 {
            rng.one_in(2)
        } else {
            rng.one_in(20)
        };
        let reversible = !rng.one_in(3);
        d += &format!(" rct {rct} rev {reversible}");
        cod.set_color_transform(rct);
        cod.set_reversible(reversible);
        if rng.one_in(4) && num_comps > 0 {
            let c = rng.below(num_comps);
            let l = rng.below(7);
            let (cw, ch) = (1 << rng.range(2, 6), 1 << rng.range(2, 6));
            d += &format!(" coc {c}:{l}:{cw}x{ch}");
            let _ = cod.set_num_decomposition_comp(c, l);
            let _ = cod.set_block_dims_comp(c, cw, ch);
        }
    }
    if rng.one_in(3) {
        let q = [0.0, 1e-6, 0.0001, 0.01, 0.5, 2.0, f32::NAN][rng.below(7) as usize];
        d += &format!(" qstep {q}");
        enc.access_qcd_mut().set_irrev_quant(q);
    }
    for c in 0..num_comps {
        if rng.one_in(3) {
            d += &format!(" nlt{c}");
            let _ = enc
                .access_nlt_mut()
                .set_nonlinear_transform(c, nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT);
        }
    }
    match rng.below(3) {
        0 => {}
        p => {
            d += &format!(" planar {}", p == 1);
            enc.set_planar(p == 1);
        }
    }
    if rng.one_in(2) {
        d += " tlm";
        enc.request_tlm_marker(true);
    }
    let (tr, tc) = (rng.one_in(2), rng.one_in(2));
    if tr || tc {
        d += &format!(" tileparts {tr}/{tc}");
        enc.set_tilepart_divisions(tr, tc);
    }
    if rng.one_in(5) {
        let limit = 1u64 << rng.range(8, 22);
        d += &format!(" limit {limit}");
        enc.set_memory_limit(limit);
    }
    (enc, d)
}

/// Whether OpenJPH would write a codestream that it cannot decode:
/// LRCP or RLCP, tile parts at components, and components with
/// different numbers of decompositions. TNsot counts a tile part for
/// every resolution of the component with the most, but none is written
/// for resolutions a component lacks, and TPsot skips their numbers.
/// The encoder reproduces this, as it does OpenJPH's other bugs.
fn openjph_tile_part_bug(enc: &Encoder) -> bool {
    let cod = enc.access_cod();
    let n = enc.access_siz().get_num_components();
    let prog = cod.get_progression_order_as_string();
    let d0 = cod.get_coc(0).get_num_decompositions();
    (prog == "LRCP" || prog == "RLCP")
        && enc.is_tilepart_division_at_components()
        && (1..n).any(|c| cod.get_coc(c).get_num_decompositions() != d0)
}

/// Encodes one random configuration with random samples. Returns the
/// codestream and whether it is one that OpenJPH writes but cannot
/// decode, or the code of the error that refused it.
fn encode_random(rng: &mut Rng, desc: &mut String) -> Result<(Vec<u8>, bool), u32> {
    let (mut enc, d) = random_encoder(rng);
    *desc = d;
    let mut out = b"xyz".to_vec();
    if let Err(e) = enc.write_headers(&mut out, &[]) {
        assert_eq!(out, b"xyz", "write_headers wrote and failed");
        return Err(e.code());
    }
    let known_bad = openjph_tile_part_bug(&enc);
    // 0: in a 16-bit range, 1: any i32, 2: zero, 3: extremes only
    let mode = rng.below(4);
    let mut lines = 0;
    loop {
        match enc.exchange() {
            Ok(Some((_, line))) => {
                lines += 1;
                assert!(lines <= MAX_LINES, "too many lines");
                for s in line.iter_mut() {
                    *s = match mode {
                        0 => (rng.next() as i16).into(),
                        1 => rng.next() as i32,
                        2 => 0,
                        _ => [i32::MIN, i32::MAX, -1, 0][rng.below(4) as usize],
                    };
                }
            }
            Ok(None) => break,
            Err(e) => return Err(e.code()),
        }
    }
    let before = out.len();
    match enc.flush(&mut out) {
        Ok(()) => Ok((out[3..].to_vec(), known_bad)),
        Err(e) => {
            assert_eq!(out.len(), before, "flush wrote and failed");
            Err(e.code())
        }
    }
}

#[test]
fn random_configurations_never_panic() {
    let iterations = if cfg!(debug_assertions) { 1500 } else { 6000 };
    let mut rng = Rng(0x00C0_FFEE);
    let (mut encoded, mut known_bad) = (0, 0);
    let mut refused = std::collections::BTreeMap::<u32, u32>::new();
    let mut problems = Vec::new();
    for i in 0..iterations {
        let mut desc = String::new();
        match catch_unwind(AssertUnwindSafe(|| encode_random(&mut rng, &mut desc))) {
            Err(_) => problems.push(format!("{i}: encoder panicked: {desc}")),
            Ok(Err(code)) => *refused.entry(code).or_default() += 1,
            Ok(Ok((j2c, bad))) => {
                encoded += 1;
                match catch_unwind(|| decode_to_raw(&j2c)) {
                    Err(_) => problems.push(format!("{i}: decoder panicked: {desc}")),
                    Ok(Err(e)) if bad && e.contains("0x00030091") => known_bad += 1,
                    Ok(Err(e)) => problems.push(format!("{i}: does not decode ({e}): {desc}")),
                    Ok(Ok(_)) => {}
                }
            }
        }
    }
    eprintln!(
        "{encoded} encoded ({known_bad} with OpenJPH's tile-part bug), {} refused, {} problems",
        refused.values().sum::<u32>(),
        problems.len()
    );
    for (code, n) in &refused {
        eprintln!("  refused with 0x{code:08X}: {n}");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(encoded > iterations / 4, "too few configurations encoded");
}

/// Images too large for the memory limit are refused by `write_headers`
/// before anything is allocated; with the default limit (4 GiB) too.
#[test]
fn huge_images_are_refused_before_allocating() {
    // Lines are allocated whole, so a wide image needs the memory and a
    // tall one does not.
    let wide = [(u32::MAX, 1, 1), (u32::MAX, u32::MAX, 1), (1 << 31, 64, 4)];
    let more = [(1 << 20, 1 << 20, 3), (1 << 16, 4, 1)];
    for (limit, small) in [(None, false), (Some(1u64 << 20), true)] {
        let sizes: Vec<_> = wide
            .iter()
            .chain(if small { &more[..] } else { &[] })
            .collect();
        for &&(w, h, n) in &sizes {
            let mut enc = Encoder::new();
            if let Some(l) = limit {
                enc.set_memory_limit(l);
            }
            let siz = enc.access_siz_mut();
            siz.set_image_extent(Point::new(w, h));
            siz.set_num_components(n);
            for c in 0..n {
                siz.set_component(c, Point::new(1, 1), 32, true).unwrap();
            }
            let mut out = Vec::new();
            let r = enc.write_headers(&mut out, &[]);
            assert!(r.is_err(), "{w}x{h}x{n} (small limit {small}) was accepted");
            assert!(out.is_empty());
        }
    }
}

/// Configurations at the counting limits of the codestream format.
#[test]
fn format_limits_are_errors_or_codestreams() {
    // more than 65535 tiles (Isot is 16 bits)
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut();
    siz.set_image_extent(Point::new(300, 300));
    siz.set_tile_size(Size::new(1, 1));
    siz.set_num_components(1);
    siz.set_component(0, Point::new(1, 1), 8, false).unwrap();
    assert!(enc.write_headers(&mut Vec::new(), &[]).is_err());

    // more than 255 tile parts in a tile (TNsot is 8 bits): 300
    // components, each with 1 decomposition, divided at both
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut();
    siz.set_image_extent(Point::new(4, 4));
    siz.set_num_components(300);
    for c in 0..300 {
        siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
    }
    enc.access_cod_mut().set_num_decomposition(1).unwrap();
    enc.access_cod_mut().set_progression_order("CPRL").unwrap();
    enc.set_planar(true);
    enc.set_tilepart_divisions(true, true);
    enc.request_tlm_marker(true);
    let mut out = Vec::new();
    let r = catch_unwind(AssertUnwindSafe(|| -> htj2k::Result<()> {
        enc.write_headers(&mut out, &[])?;
        while let Some((_, l)) = enc.exchange()? {
            l.fill(1);
        }
        enc.flush(&mut out)
    }));
    assert!(matches!(r, Ok(Err(_))), "300 x 2 tile parts: {r:?}");

    // 16384 components, the most SIZ allows, through to a codestream
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut();
    siz.set_image_extent(Point::new(2, 1));
    siz.set_num_components(16384);
    for c in 0..16384 {
        siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
    }
    enc.access_cod_mut().set_num_decomposition(0).unwrap();
    enc.set_planar(true);
    let mut out = Vec::new();
    enc.write_headers(&mut out, &[]).unwrap();
    while let Some((_, l)) = enc.exchange().unwrap() {
        l.fill(7);
    }
    enc.flush(&mut out).unwrap();
    let raw = decode_to_raw(&out).unwrap();
    assert_eq!(raw.len(), 16384 * 2 * 4);

    // 65536 components: Csiz is 16 bits, and the count wraps to 0 as in
    // OpenJPH, so there are no components to set
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut();
    siz.set_image_extent(Point::new(2, 1));
    siz.set_num_components(65536);
    assert!(siz.set_component(0, Point::new(1, 1), 8, false).is_err());
    assert!(enc.write_headers(&mut Vec::new(), &[]).is_err());
}

/// Precinct sizes are checked when they are set, against the
/// decompositions of the time. Raising the decompositions afterwards
/// leaves precincts of 1x1 above resolution 0, which OpenJPH writes but
/// cannot decode; `write_headers` refuses them.
#[test]
fn precincts_set_before_decompositions() {
    let mut enc = small_encoder();
    let cod = enc.access_cod_mut();
    cod.set_num_decomposition(0).unwrap();
    cod.set_precinct_size(&[Size::new(16, 16)]).unwrap();
    cod.set_num_decomposition(2).unwrap();
    let err = enc.write_headers(&mut Vec::new(), &[]).unwrap_err();
    assert_eq!(err.code(), 0x00080009);
}

fn small_encoder() -> Encoder {
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut();
    siz.set_image_extent(Point::new(8, 4));
    siz.set_num_components(1);
    siz.set_component(0, Point::new(1, 1), 8, false).unwrap();
    enc
}

/// Calls in the wrong order are errors, and the output is left alone.
#[test]
fn misuse_is_an_error() {
    let none: &[CommentExchange] = &[];

    // exchange or flush before write_headers
    let mut enc = small_encoder();
    assert!(enc.exchange().is_err());
    let mut out = vec![1, 2, 3];
    assert!(enc.flush(&mut out).is_err());
    assert_eq!(out, [1, 2, 3]);

    // write_headers twice; flush before every line is exchanged
    let mut enc = small_encoder();
    enc.write_headers(&mut out, none).unwrap();
    let after_headers = out.clone();
    assert!(enc.write_headers(&mut out, none).is_err());
    assert!(enc.flush(&mut out).is_err());
    assert_eq!(out, after_headers);

    // exchange and flush after flush
    let mut enc = small_encoder();
    let mut out = Vec::new();
    enc.write_headers(&mut out, none).unwrap();
    while let Some((_, l)) = enc.exchange().unwrap() {
        l.fill(3);
    }
    assert!(enc.exchange().unwrap().is_none() || enc.exchange().is_err());
    enc.flush(&mut out).unwrap();
    let done = out.clone();
    assert!(enc.exchange().is_err());
    assert!(enc.flush(&mut out).is_err());
    assert!(enc.write_headers(&mut out, none).is_err());
    assert_eq!(out, done);

    // after a failed write_headers, nothing works
    let mut enc = Encoder::new();
    let mut out = Vec::new();
    assert!(enc.write_headers(&mut out, none).is_err());
    assert!(enc.write_headers(&mut out, none).is_err());
    assert!(enc.exchange().is_err());
    assert!(enc.flush(&mut out).is_err());
    assert!(out.is_empty());
}
