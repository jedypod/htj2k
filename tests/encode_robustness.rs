// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Feeds the encoder random configurations and random samples, legal or
//! not, and misuses its API. Whatever it is given, it must return, with
//! a codestream or with an error, and must not panic. Run it in a debug
//! build too, so that arithmetic overflow counts as a failure.
//!
//! `random_configurations_never_panic` gives samples that may be out of
//! range. When the encoder accepts a configuration, the codestream must
//! decode with the crate's decoder to planes of the sizes SIZ gives. The
//! samples are not compared there, because out-of-range input is not
//! meant to round-trip. `random_configurations_round_trip` gives the same
//! configurations samples in range: every reversible component must then
//! decode to exactly its input, and every plane must have the expected
//! size.

mod common;

use common::decode::{decode_planes, decode_to_raw};
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
        let siz = enc.access_siz_mut().unwrap();
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
        let cod = enc.access_cod_mut().unwrap();
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
        enc.access_qcd_mut().unwrap().set_irrev_quant(q);
    }
    for c in 0..num_comps {
        if rng.one_in(3) {
            d += &format!(" nlt{c}");
            let _ = enc
                .access_nlt_mut()
                .unwrap()
                .set_nonlinear_transform(c, nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT);
        }
    }
    match rng.below(3) {
        0 => {}
        p => {
            d += &format!(" planar {}", p == 1);
            enc.set_planar(p == 1).unwrap();
        }
    }
    if rng.one_in(2) {
        d += " tlm";
        enc.request_tlm_marker(true).unwrap();
    }
    let (tr, tc) = (rng.one_in(2), rng.one_in(2));
    if tr || tc {
        d += &format!(" tileparts {tr}/{tc}");
        enc.set_tilepart_divisions(tr, tc).unwrap();
    }
    if rng.one_in(5) {
        let limit = 1u64 << rng.range(8, 22);
        d += &format!(" limit {limit}");
        enc.set_memory_limit(limit).unwrap();
    }
    (enc, d)
}

/// What [`encode_random`] encoded.
struct Encoded {
    j2c: Vec<u8>,
    /// Each component's plane size, from SIZ: `ceil(Xsiz/XRsiz) -
    /// ceil(XOsiz/XRsiz)` by the same for y.
    sizes: Vec<(usize, usize)>,
    /// The samples given, per component (only when they are in range).
    input: Vec<Vec<i32>>,
    /// Whether each component is coded reversibly.
    reversible: Vec<bool>,
}

/// The plane sizes SIZ gives, computed here rather than by the crate.
fn expected_sizes(enc: &Encoder) -> Vec<(usize, usize)> {
    let siz = enc.access_siz();
    let (e, o) = (siz.get_image_extent(), siz.get_image_offset());
    (0..siz.get_num_components())
        .map(|c| {
            let d = siz.get_downsampling(c);
            let span = |hi: u32, lo: u32, s: u32| hi.div_ceil(s) as usize - lo.div_ceil(s) as usize;
            (span(e.x, o.x, d.x), span(e.y, o.y, d.y))
        })
        .collect()
}

/// Encodes one random configuration with random samples: in range for
/// each component's depth and signedness if `in_range`, else anything.
/// Returns what was encoded, or the code of the error that refused it.
fn encode_random(rng: &mut Rng, desc: &mut String, in_range: bool) -> Result<Encoded, u32> {
    let (mut enc, d) = random_encoder(rng);
    *desc = d;
    let mut out = b"xyz".to_vec();
    if let Err(e) = enc.write_headers(&mut out, &[]) {
        assert_eq!(out, b"xyz", "write_headers wrote and failed");
        return Err(e.code());
    }
    let sizes = expected_sizes(&enc);
    let nc = sizes.len() as u32;
    let reversible: Vec<bool> = (0..nc)
        .map(|c| enc.access_cod().is_reversible_comp(c))
        .collect();
    // The legal range of each component.
    let ranges: Vec<(i64, i64)> = (0..nc)
        .map(|c| {
            let siz = enc.access_siz();
            let b = siz.get_bit_depth(c);
            if siz.is_signed(c) {
                (-(1i64 << (b - 1)), (1i64 << (b - 1)) - 1)
            } else {
                (0, (1i64 << b) - 1)
            }
        })
        .collect();
    let consts: Vec<i64> = ranges
        .iter()
        .map(|&(lo, hi)| lo + (rng.next() % (hi - lo + 1) as u64) as i64)
        .collect();
    let mut input: Vec<Vec<i32>> = vec![Vec::new(); nc as usize];
    // Out of range: 0 in a 16-bit range, 1 any i32, 2 zero, 3 extremes.
    // In range: 0 uniform, 1 a gradient, 2 a constant, 3 the two ends.
    let mode = rng.below(4);
    let mut lines = 0;
    loop {
        match enc.exchange() {
            Ok(Some((c, line))) => {
                lines += 1;
                assert!(lines <= MAX_LINES, "too many lines");
                if !in_range {
                    for s in line.iter_mut() {
                        *s = match mode {
                            0 => (rng.next() as i16).into(),
                            1 => rng.next() as i32,
                            2 => 0,
                            _ => [i32::MIN, i32::MAX, -1, 0][rng.below(4) as usize],
                        };
                    }
                    continue;
                }
                let c = c as usize;
                let (w, h) = sizes[c];
                assert!(line.len() >= w, "line shorter than the component");
                let y = input[c].len() / w.max(1);
                assert!(y < h, "too many lines of component {c}");
                let (lo, hi) = ranges[c];
                let span = (hi - lo + 1) as u64;
                for (x, s) in line[..w].iter_mut().enumerate() {
                    let v = match mode {
                        0 => lo + (rng.next() % span) as i64,
                        1 => {
                            lo + ((x as u64 * 7 + y as u64 * 13) * (span / 61).max(1) % span) as i64
                        }
                        2 => consts[c],
                        _ => [lo, hi][rng.below(2) as usize],
                    };
                    // Unsigned 32-bit values go in as their bit pattern.
                    *s = v as u32 as i32;
                    input[c].push(*s);
                }
            }
            Ok(None) => break,
            Err(e) => return Err(e.code()),
        }
    }
    let before = out.len();
    match enc.flush(&mut out) {
        Ok(()) => Ok(Encoded {
            j2c: out[3..].to_vec(),
            sizes,
            input,
            reversible,
        }),
        Err(e) => {
            assert_eq!(out.len(), before, "flush wrote and failed");
            Err(e.code())
        }
    }
}

/// Decodes `e` and checks its plane sizes, and, if `exact`, that each
/// reversible component gives back its input.
fn check_decode(e: &Encoded, exact: bool) -> Result<(), String> {
    let planes = catch_unwind(|| decode_planes(&e.j2c))
        .map_err(|_| "decoder panicked".to_string())?
        .map_err(|err| format!("does not decode ({err})"))?;
    let got: Vec<(usize, usize)> = planes.iter().map(|p| (p.width, p.height)).collect();
    if got != e.sizes {
        return Err(format!("planes {got:?}, expected {:?}", e.sizes));
    }
    if exact {
        for (c, p) in planes.iter().enumerate() {
            if e.reversible[c] && p.samples != e.input[c] {
                let at = p.samples.iter().zip(&e.input[c]).position(|(a, b)| a != b);
                return Err(format!("component {c} differs (first at {at:?})"));
            }
        }
    }
    Ok(())
}

/// Random configurations with random, possibly out-of-range, samples.
#[test]
fn random_configurations_never_panic() {
    let iterations = if cfg!(debug_assertions) { 1500 } else { 6000 };
    let mut rng = Rng(0x00C0_FFEE);
    let mut encoded = 0;
    let mut refused = std::collections::BTreeMap::<u32, u32>::new();
    let mut problems = Vec::new();
    for i in 0..iterations {
        let mut desc = String::new();
        match catch_unwind(AssertUnwindSafe(|| {
            encode_random(&mut rng, &mut desc, false)
        })) {
            Err(_) => problems.push(format!("{i}: encoder panicked: {desc}")),
            Ok(Err(code)) => *refused.entry(code).or_default() += 1,
            Ok(Ok(e)) => {
                encoded += 1;
                if let Err(err) = check_decode(&e, false) {
                    problems.push(format!("{i}: {err}: {desc}"));
                }
            }
        }
    }
    eprintln!(
        "{encoded} encoded, {} refused, {} problems",
        refused.values().sum::<u32>(),
        problems.len()
    );
    for (code, n) in &refused {
        eprintln!("  refused with 0x{code:08X}: {n}");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert!(encoded > iterations / 4, "too few configurations encoded");
}

/// The same random configurations with samples in range: every
/// reversible component decodes to exactly its input.
#[test]
fn random_configurations_round_trip() {
    let iterations = if cfg!(debug_assertions) { 1500 } else { 6000 };
    let mut rng = Rng(0x005E_ED0F_2026);
    let (mut encoded, mut exact, mut refused) = (0, 0, 0);
    let mut problems = Vec::new();
    for i in 0..iterations {
        let mut desc = String::new();
        match catch_unwind(AssertUnwindSafe(|| {
            encode_random(&mut rng, &mut desc, true)
        })) {
            Err(_) => problems.push(format!("{i}: encoder panicked: {desc}")),
            Ok(Err(_)) => refused += 1,
            Ok(Ok(e)) => {
                encoded += 1;
                exact += e.reversible.iter().filter(|&&r| r).count();
                if let Err(err) = check_decode(&e, true) {
                    problems.push(format!("{i}: {err}: {desc}"));
                }
            }
        }
    }
    eprintln!(
        "{encoded} encoded ({exact} reversible components compared), {refused} refused, {} problems",
        problems.len()
    );
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
                enc.set_memory_limit(l).unwrap();
            }
            let siz = enc.access_siz_mut().unwrap();
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
    let siz = enc.access_siz_mut().unwrap();
    siz.set_image_extent(Point::new(300, 300));
    siz.set_tile_size(Size::new(1, 1));
    siz.set_num_components(1);
    siz.set_component(0, Point::new(1, 1), 8, false).unwrap();
    assert!(enc.write_headers(&mut Vec::new(), &[]).is_err());

    // more than 255 tile parts in a tile (TNsot is 8 bits): 300
    // components, each with 1 decomposition, divided at both
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut().unwrap();
    siz.set_image_extent(Point::new(4, 4));
    siz.set_num_components(300);
    for c in 0..300 {
        siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
    }
    enc.access_cod_mut()
        .unwrap()
        .set_num_decomposition(1)
        .unwrap();
    enc.access_cod_mut()
        .unwrap()
        .set_progression_order("CPRL")
        .unwrap();
    enc.set_planar(true).unwrap();
    enc.set_tilepart_divisions(true, true).unwrap();
    enc.request_tlm_marker(true).unwrap();
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
    let siz = enc.access_siz_mut().unwrap();
    siz.set_image_extent(Point::new(2, 1));
    siz.set_num_components(16384);
    for c in 0..16384 {
        siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
    }
    enc.access_cod_mut()
        .unwrap()
        .set_num_decomposition(0)
        .unwrap();
    enc.set_planar(true).unwrap();
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
    let siz = enc.access_siz_mut().unwrap();
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
    let cod = enc.access_cod_mut().unwrap();
    cod.set_num_decomposition(0).unwrap();
    cod.set_precinct_size(&[Size::new(16, 16)]).unwrap();
    cod.set_num_decomposition(2).unwrap();
    let err = enc.write_headers(&mut Vec::new(), &[]).unwrap_err();
    assert_eq!(err.code(), 0x00080009);
}

/// With no tile size set, the tile is extent + offset, as in OpenJPH.
/// Where that sum does not fit in 32 bits OpenJPH wraps it and refuses a
/// legal image ("a tile lies outside the image"); here the tile reaches
/// the edge of the canvas and the image encodes.
#[test]
fn default_tile_size_does_not_wrap() {
    for (ext, off) in [
        // the review's 65535-wide image ending at the edge of the canvas
        ((u32::MAX, 3), (0xFFFF_0000, 1)),
        // wraps in y, not in x
        ((40, u32::MAX), (7, u32::MAX - 20)),
        // extent + offset is exactly 2^32
        ((0x8000_0000 + 77, 2), (0x8000_0000 - 77, 0)),
    ] {
        let mut enc = Encoder::new();
        let siz = enc.access_siz_mut().unwrap();
        siz.set_image_extent(Point::new(ext.0, ext.1));
        siz.set_image_offset(Point::new(off.0, off.1));
        siz.set_num_components(1);
        siz.set_component(0, Point::new(1, 1), 12, false).unwrap();
        let cod = enc.access_cod_mut().unwrap();
        cod.set_num_decomposition(3).unwrap();
        cod.set_reversible(true);
        let mut out = Vec::new();
        enc.write_headers(&mut out, &[]).unwrap();
        let (w, h) = ((ext.0 - off.0) as usize, (ext.1 - off.1) as usize);
        let mut want = Vec::new();
        let mut n = 0u32;
        while let Some((_, line)) = enc.exchange().unwrap() {
            assert_eq!(line.len(), w);
            for s in line.iter_mut() {
                n = n.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *s = (n >> 20) as i32;
                want.extend_from_slice(&s.to_le_bytes());
            }
        }
        enc.flush(&mut out).unwrap();
        assert_eq!(want.len(), w * h * 4);
        let got = decode_to_raw(&out).unwrap();
        let first = got.iter().zip(&want).position(|(a, b)| a != b);
        assert!(
            got == want,
            "{ext:?} {off:?}: first difference at {first:?}"
        );
    }
}

/// Canvas coordinates at and beyond 2^31, in every progression order:
/// the spatial orders search for the precinct nearest the top left,
/// which OpenJPH starts from INT_MAX, so beyond it nothing was found and
/// flush failed (0x00080024) once component or resolution 0 ran out.
#[test]
fn canvas_beyond_2_31() {
    use common::drive;
    use common::synth::EncArgs;
    // image offset, and a tile grid offset whose first tile meets it
    let offsets = [
        ("2147483647,2147483646", "2147483640,2147483640"),
        ("3000000000,3000000005", "2999999995,2999999999"),
        ("5,4294967200", "0,4294967195"),
    ];
    let mut checked = 0;
    for (off, tile_off) in offsets {
        for prog in ["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"] {
            for extra in [
                "--comps 3 --levels 2 --block 8x8 --precincts 8x8,16x16".to_string(),
                format!("--comps 3 --levels 3 --block 8x8 --sub 1x1,2x2,2x1 --planar 1 --tile 16x8 --tile-off {tile_off}"),
                "--comps 2 --levels 2 --block 4x4 --coc 1:1:4x4".to_string(),
            ] {
                if extra.contains("--coc") && prog.ends_with("CP") {
                    continue; // refused: unequal decompositions
                }
                let cmd = format!(
                    "ojph_enc -o a.j2c --size 37x29 --img-off {off} --prog {prog} {extra} --seed 9"
                );
                let args = EncArgs::parse_command(&cmd).unwrap();
                let mut out = Vec::new();
                drive::encode(&args, &mut out).unwrap_or_else(|e| panic!("{cmd}: {e}"));
                let got = decode_to_raw(&out).unwrap_or_else(|e| panic!("{cmd}: {e}"));
                assert!(got == EncArgs::to_raw(&args.synthesise()), "{cmd}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 3 * 5 * 3 - 3 * 2);
}

/// The parameter objects are shared with the decoder, so
/// `access_siz_mut` and friends reach decoder-side calls (`read`,
/// `set_skipped_resolutions`, `employ_color_transform(u8)`,
/// `set_Rsiz_flag`). State they set that the writer does not implement
/// is refused by `write_headers` (0x0008000D) instead of being ignored.
#[test]
fn decoder_side_state_is_refused() {
    use htj2k::file::MemInfile;
    // a 2x2-tile image, so skipped resolutions would shift tiles
    let base = || {
        let mut enc = Encoder::new();
        let siz = enc.access_siz_mut().unwrap();
        siz.set_image_extent(Point::new(16, 16));
        siz.set_tile_size(Size::new(8, 8));
        siz.set_num_components(3);
        for c in 0..3 {
            siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
        }
        enc.access_cod_mut()
            .unwrap()
            .set_num_decomposition(2)
            .unwrap();
        enc
    };
    // A COD segment after its marker: Lcod, Scod, SGcod (order, layers,
    // MCT), SPcod (levels, block size, style, kernel).
    let cod = |scod: u8, layers: u16, mct: u8, style: u8, kernel: u8| {
        let l = layers.to_be_bytes();
        vec![0, 12, scod, 0, l[0], l[1], mct, 2, 2, 2, style, kernel]
    };
    type Setup = Box<dyn Fn(&mut Encoder)>;
    let read_cod = |bytes: Vec<u8>| -> Setup {
        Box::new(move |e: &mut Encoder| {
            let mut f = MemInfile::new(&bytes);
            e.access_cod_mut().unwrap().read(&mut f).unwrap();
        })
    };
    let cases: Vec<(&str, Setup)> = vec![
        (
            "skipped resolutions",
            Box::new(|e| e.access_siz_mut().unwrap().set_skipped_resolutions(1)),
        ),
        (
            "Rsiz capability bit",
            Box::new(|e| e.access_siz_mut().unwrap().set_Rsiz_flag(0x20)),
        ),
        (
            "Rsiz without HT",
            Box::new(|e| e.access_siz_mut().unwrap().reset_Rsiz_flag(0x4000)),
        ),
        ("SOP", read_cod(cod(0x02, 1, 0, 0x40, 1))),
        ("EPH", read_cod(cod(0x04, 1, 0, 0x40, 1))),
        ("two layers", read_cod(cod(0, 2, 0, 0x40, 1))),
        ("vertical causal", read_cod(cod(0, 1, 0, 0x48, 1))),
        ("ATK kernel", read_cod(cod(0, 1, 0, 0x40, 2))),
        (
            "MCT value 2",
            Box::new(|e| e.access_cod_mut().unwrap().employ_color_transform(2)),
        ),
    ];
    // the same COD read with none of these is fine
    let mut enc = base();
    read_cod(cod(0, 1, 0, 0x40, 1))(&mut enc);
    let mut out = Vec::new();
    enc.write_headers(&mut out, &[]).unwrap();
    for (name, setup) in &cases {
        let mut enc = base();
        setup(&mut enc);
        let mut out = b"keep".to_vec();
        let err = enc.write_headers(&mut out, &[]).expect_err(name);
        assert_eq!(err.code(), 0x0008000D, "{name}: {err}");
        assert_eq!(out, b"keep", "{name}");
    }

    // An NLT type 4 read from a codestream has no encoding table.
    let mut enc = small_encoder();
    enc.access_cod_mut().unwrap().set_reversible(false);
    let points: Vec<u32> = (0..16).map(|i| i * 4000).collect();
    enc.access_nlt_mut()
        .unwrap()
        .set_nonlinear_transform_lut(
            0,
            16,
            false,
            0,
            0xFFFF,
            16,
            16,
            &points,
            nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT,
            false,
            0,
        )
        .unwrap();
    let mut out = Vec::new();
    enc.write_headers(&mut out, &[]).unwrap();
    let at = out
        .windows(2)
        .position(|w| w == htj2k::params::marker::NLT.to_be_bytes())
        .unwrap();
    let mut enc = small_encoder();
    enc.access_cod_mut().unwrap().set_reversible(false);
    let mut f = MemInfile::new(&out[at + 2..]);
    enc.access_nlt_mut().unwrap().read(&mut f).unwrap();
    let err = enc.write_headers(&mut Vec::new(), &[]).unwrap_err();
    assert_eq!(err.code(), 0x0008000D, "{err}");
}

/// LRCP or RLCP with components of different decomposition counts is
/// refused: without tile parts at components OpenJPH asserts
/// (0x00080007), with them it writes a TNsot and TPsot no decoder
/// accepts (0x0008000C). Equal counts still encode, with tile parts.
#[test]
fn layer_first_needs_equal_decompositions() {
    use common::drive;
    use common::synth::EncArgs;
    let base = "ojph_enc -o a.j2c --size 33x27 --comps 3 --levels 3 --block 16x16";
    for prog in ["LRCP", "RLCP"] {
        for (tp, code) in [("", 0x00080007), ("C", 0x0008000C), ("RC", 0x0008000C)] {
            let tp = if tp.is_empty() {
                String::new()
            } else {
                format!(" --tileparts {tp}")
            };
            let cmd = format!("{base} --coc 1:1:4x8 --prog {prog}{tp}");
            let args = EncArgs::parse_command(&cmd).unwrap();
            let mut out = Vec::new();
            let err = drive::encode(&args, &mut out).unwrap_err();
            assert_eq!(err.code(), code, "{cmd}");
            assert!(out.is_empty(), "{cmd}");

            let cmd = format!("{base} --coc 1:3:4x8 --prog {prog} --tlm{tp}");
            let args = EncArgs::parse_command(&cmd).unwrap();
            let mut out = Vec::new();
            drive::encode(&args, &mut out).unwrap();
            assert_eq!(
                decode_to_raw(&out).unwrap(),
                EncArgs::to_raw(&args.synthesise()),
                "{cmd}"
            );
        }
    }
    // The review's example: TNsot would be 600 and TPsot reach 500,
    // both truncated to 8 bits, though the tile parts number 105.
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut().unwrap();
    siz.set_image_extent(Point::new(4, 4));
    siz.set_num_components(100);
    for c in 0..100 {
        siz.set_component(c, Point::new(1, 1), 8, false).unwrap();
    }
    let cod = enc.access_cod_mut().unwrap();
    cod.set_num_decomposition(0).unwrap();
    cod.set_num_decomposition_comp(7, 5).unwrap();
    cod.set_progression_order("LRCP").unwrap();
    enc.set_tilepart_divisions(false, true).unwrap();
    let err = enc.write_headers(&mut Vec::new(), &[]).unwrap_err();
    assert_eq!(err.code(), 0x0008000C);
}

/// Components with no lines: exchange never asks for them and flush
/// does not wait for them, when no component with lines follows (the
/// matrix's `zh_*` cases check the bytes against OpenJPH). One followed
/// by a component with lines is refused (0x00080005): OpenJPH's planar
/// exchange and its decoder both loop forever on it.
#[test]
fn components_with_no_lines() {
    // 8x1 at y offset 1: a component subsampled 2 in y has no lines
    let encode = |subs: &[u32], planar: bool| -> htj2k::Result<(Vec<u32>, Vec<u8>)> {
        let mut enc = Encoder::new();
        let siz = enc.access_siz_mut()?;
        siz.set_image_extent(Point::new(8, 2));
        siz.set_image_offset(Point::new(0, 1));
        siz.set_num_components(subs.len() as u32);
        for (c, &dy) in subs.iter().enumerate() {
            siz.set_component(c as u32, Point::new(1, dy), 8, false)?;
        }
        let cod = enc.access_cod_mut()?;
        cod.set_reversible(true);
        cod.set_num_decomposition(2)?;
        enc.set_planar(planar)?;
        let mut out = Vec::new();
        enc.write_headers(&mut out, &[])?;
        let mut order = Vec::new();
        while let Some((c, line)) = enc.exchange()? {
            order.push(c);
            line.fill(10 * c as i32 + 1);
        }
        enc.flush(&mut out)?;
        Ok((order, out))
    };
    let (order, out) = encode(&[1, 1, 2, 2], true).unwrap();
    assert_eq!(order, [0, 1]);
    let want: Vec<u8> = [1i32, 11]
        .iter()
        .flat_map(|v| std::iter::repeat_n(v.to_le_bytes(), 8).flatten())
        .collect();
    assert_eq!(decode_to_raw(&out).unwrap(), want);
    // every component empty, planar or not: nothing to exchange
    for planar in [false, true] {
        let (order, out) = encode(&[2, 2], planar).unwrap();
        assert!(order.is_empty());
        assert!(decode_to_raw(&out).unwrap().is_empty());
    }
    for subs in [&[2, 1][..], &[1, 2, 1], &[2, 2, 1, 2]] {
        let err = encode(subs, true).unwrap_err();
        assert_eq!(err.code(), 0x00080005, "{subs:?}");
    }
}

/// Without `set_planar`, the exchange follows OpenJPH's rule
/// (interleaved, or planar with the colour transform), except that
/// planar with the colour transform is refused as an explicit
/// `set_planar(true)` is.
#[test]
fn planar_default_follows_openjph() {
    // 3 components of 4x3, the colour transform on or off
    let encode = |rct: bool, planar: Option<bool>| -> htj2k::Result<Vec<u8>> {
        let mut enc = Encoder::new();
        let siz = enc.access_siz_mut()?;
        siz.set_image_extent(Point::new(4, 3));
        siz.set_num_components(3);
        for c in 0..3 {
            siz.set_component(c, Point::new(1, 1), 8, false)?;
        }
        enc.access_cod_mut()?.set_color_transform(rct);
        if let Some(p) = planar {
            enc.set_planar(p)?;
        }
        assert_eq!(enc.is_planar(), planar.unwrap_or(rct));
        let mut out = b"keep".to_vec();
        if let Err(e) = enc.write_headers(&mut out, &[]) {
            assert_eq!(out, b"keep");
            return Err(e);
        }
        let mut order = Vec::new();
        while let Some((c, line)) = enc.exchange()? {
            order.push(c);
            line.fill(c as i32 * 40);
        }
        // interleaved: one line of each component in turn
        assert_eq!(order, [0, 1, 2, 0, 1, 2, 0, 1, 2]);
        enc.flush(&mut out)?;
        Ok(out)
    };
    assert_eq!(
        encode(false, None).unwrap(),
        encode(false, Some(false)).unwrap()
    );
    assert_eq!(encode(true, None).unwrap_err().code(), 0x00030021);
    assert_eq!(encode(true, Some(true)).unwrap_err().code(), 0x00030021);
    let rct = encode(true, Some(false)).unwrap();
    let raw = decode_to_raw(&rct[4..]).unwrap();
    let want: Vec<u8> = (0..3)
        .flat_map(|c: i32| std::iter::repeat_n((c * 40).to_le_bytes(), 12).flatten())
        .collect();
    assert_eq!(raw, want);
}

fn small_encoder() -> Encoder {
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut().unwrap();
    siz.set_image_extent(Point::new(8, 4));
    siz.set_num_components(1);
    siz.set_component(0, Point::new(1, 1), 8, false).unwrap();
    enc
}

/// Caller comments are written after the version comment. One too long
/// for a COM segment is refused. `set_string`, like OpenJPH, checks the
/// previous length, so it accepts one.
#[test]
fn comments() {
    let mut ok = CommentExchange::new();
    ok.set_string("hello").unwrap();
    let mut enc = small_encoder();
    enc.set_version_comment(None).unwrap();
    let mut out = Vec::new();
    enc.write_headers(&mut out, std::slice::from_ref(&ok))
        .unwrap();
    let com = [0xFF, 0x64, 0, 9, 0, 1, b'h', b'e', b'l', b'l', b'o'];
    assert!(out.ends_with(&com));

    let mut long = CommentExchange::new();
    long.set_string(&"x".repeat(65534)).unwrap();
    let mut enc = small_encoder();
    let mut out = b"keep".to_vec();
    let err = enc.write_headers(&mut out, &[ok, long]).unwrap_err();
    assert_eq!(err.code(), 0x0008000A);
    assert_eq!(out, b"keep");
}

/// Every setter and `access_*_mut` call `f` can make after
/// `write_headers`, each expected to fail with 0x0008000B.
type Setter = fn(&mut Encoder) -> htj2k::Result<()>;

fn late_setters() -> Vec<(&'static str, Setter)> {
    vec![
        ("access_siz_mut", |e| e.access_siz_mut().map(|_| ())),
        ("access_cod_mut", |e| e.access_cod_mut().map(|_| ())),
        ("access_qcd_mut", |e| e.access_qcd_mut().map(|_| ())),
        ("access_nlt_mut", |e| e.access_nlt_mut().map(|_| ())),
        ("set_planar(true)", |e| e.set_planar(true)),
        ("set_planar(false)", |e| e.set_planar(false)),
        ("set_tilepart_divisions(true, true)", |e| {
            e.set_tilepart_divisions(true, true)
        }),
        ("set_tilepart_divisions(true, false)", |e| {
            e.set_tilepart_divisions(true, false)
        }),
        ("request_tlm_marker(true)", |e| e.request_tlm_marker(true)),
        ("request_tlm_marker(false)", |e| e.request_tlm_marker(false)),
        ("set_memory_limit(0)", |e| e.set_memory_limit(0)),
        ("set_version_comment(None)", |e| e.set_version_comment(None)),
    ]
}

/// The scenarios of the review: a setter called after `write_headers`
/// (before any line, half way through, or after every line) used to
/// change the stream being written. Now each call fails with 0x0008000B,
/// and the codestream is the one the parameters at `write_headers` give.
#[test]
fn setters_after_write_headers_are_errors() {
    use common::drive;
    use common::synth::EncArgs;
    let commands = [
        // RPCL, then tile parts at resolutions and components: no SOT
        "ojph_enc -o a.j2c --size 40x30 --comps 3 --levels 2 --block 8x8 --prog RPCL --seed 5",
        // TLM requested late: a TLM with no entries
        "ojph_enc -o a.j2c --size 40x30 --comps 3 --tile 24x16 --levels 2 --block 8x8 --seed 6",
        // TLM, then tile parts late: more tile parts than TLM entries
        "ojph_enc -o a.j2c --size 40x30 --comps 3 --tile 24x16 --levels 2 --block 8x8 --tlm --seed 7",
        // RCT, then planar late: components overwrite each other's lines
        "ojph_enc -o a.j2c --size 19x17 --comps 3 --rct 1 --planar 0 --levels 2 --block 8x8 --seed 8",
    ];
    for cmd in commands {
        let args = EncArgs::parse_command(cmd).unwrap();
        let img = args.synthesise();
        let mut want = Vec::new();
        drive::encode(&args, &mut want).unwrap();
        for (name, f) in late_setters() {
            // when: 0 = straight after write_headers, 1 = after the first
            // line, 2 = after the last line, 3 = after flush
            for when in 0..4 {
                let mut out = Vec::new();
                let mut cs = drive::headers(&args, &mut out).unwrap();
                let check = |cs: &mut Encoder| {
                    let err = f(cs).expect_err(name);
                    assert_eq!(err.code(), 0x0008000B, "{cmd}: {name}");
                };
                if when == 0 {
                    check(&mut cs);
                }
                let sizes = args.comp_sizes();
                let mut row = vec![0usize; img.len()];
                let mut lines = 0;
                while let Some((c, line)) = cs.exchange().unwrap() {
                    let c = c as usize;
                    let w = sizes[c].0 as usize;
                    let src = &img[c][row[c] * w..(row[c] + 1) * w];
                    for (d, &s) in line.iter_mut().zip(src) {
                        *d = s as u32 as i32;
                    }
                    row[c] += 1;
                    lines += 1;
                    if when == 1 && lines == 1 {
                        check(&mut cs);
                    }
                }
                if when == 2 {
                    check(&mut cs);
                }
                cs.flush(&mut out).unwrap();
                if when == 3 {
                    check(&mut cs);
                }
                assert!(out == want, "{cmd}: {name} at {when} changed the stream");
            }
        }
        assert_eq!(
            decode_to_raw(&want).unwrap(),
            EncArgs::to_raw(&img),
            "{cmd}"
        );
    }

    // and after a failed write_headers
    let mut enc = Encoder::new();
    assert!(enc.write_headers(&mut Vec::new(), &[]).is_err());
    for (name, f) in late_setters() {
        assert_eq!(f(&mut enc).expect_err(name).code(), 0x0008000B, "{name}");
    }
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
