// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Tests of the transform kernels: bit-exact comparison against golden
//! vectors produced by OpenJPH's generic C++ kernels
//! (`oracle/kernels_dump.cpp`, format in `tests/data/kernels/README.md`),
//! and round-trip property tests.
//!
//! These are unit tests rather than integration tests because building a
//! [`ParamAtk`] needs its `pub(crate)` fields.

use super::codeblock_fun::*;
use super::colour::*;
use super::wavelet::*;
use crate::line::{LineBuf, LineData};
use crate::params::{LiftingStep, ParamAtk};

// ---------------------------------------------------------------------------
// Golden-vector file reader
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Field {
    I32(Vec<i32>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    U16(Vec<u16>),
}

impl Field {
    fn i32(&self) -> &[i32] {
        match self {
            Field::I32(v) => v,
            f => panic!("expected i32 field, got {f:?}"),
        }
    }
    fn i64(&self) -> &[i64] {
        match self {
            Field::I64(v) => v,
            f => panic!("expected i64 field, got {f:?}"),
        }
    }
    fn f32(&self) -> &[f32] {
        match self {
            Field::F32(v) => v,
            f => panic!("expected f32 field, got {f:?}"),
        }
    }
    fn u32(&self) -> &[u32] {
        match self {
            Field::U32(v) => v,
            f => panic!("expected u32 field, got {f:?}"),
        }
    }
    fn u64(&self) -> &[u64] {
        match self {
            Field::U64(v) => v,
            f => panic!("expected u64 field, got {f:?}"),
        }
    }
    fn u16(&self) -> &[u16] {
        match self {
            Field::U16(v) => v,
            f => panic!("expected u16 field, got {f:?}"),
        }
    }

    /// A line buffer holding this field as its full backing vector, with
    /// `pre_size == 1` (the layout of the dump).
    fn line(&self) -> LineBuf {
        let (len, data) = match self {
            Field::I32(v) => (v.len(), LineData::I32(v.clone())),
            Field::I64(v) => (v.len(), LineData::I64(v.clone())),
            Field::F32(v) => (v.len(), LineData::F32(v.clone())),
            f => panic!("not a line: {f:?}"),
        };
        LineBuf {
            size: len - 1,
            pre_size: 1,
            data,
        }
    }
}

/// The canonical pattern every NaN is compared as.
const NAN_BITS: u64 = 0x7FC0_0000;

/// Bit patterns of the samples of a field or line, for exact comparison.
///
/// Floats are compared by bit pattern, except that all NaNs compare
/// equal: Rust (LLVM) does not preserve NaN signs and payloads (it may,
/// for example, commute the operands of a multiplication, and x86 returns
/// the first NaN operand), so an optimised build can produce a NaN of
/// another sign than the C++. Debug builds match the C++ NaN bit for bit.
fn bits_of_field(f: &Field) -> Vec<u64> {
    match f {
        Field::I32(v) => v.iter().map(|&x| x as u32 as u64).collect(),
        Field::I64(v) => v.iter().map(|&x| x as u64).collect(),
        Field::F32(v) => v
            .iter()
            .map(|&x| {
                if x.is_nan() {
                    NAN_BITS
                } else {
                    x.to_bits() as u64
                }
            })
            .collect(),
        Field::U32(v) => v.iter().map(|&x| x as u64).collect(),
        Field::U64(v) => v.clone(),
        Field::U16(v) => v.iter().map(|&x| x as u64).collect(),
    }
}

fn bits_of_line(l: &LineBuf) -> Vec<u64> {
    match &l.data {
        LineData::I32(v) => bits_of_field(&Field::I32(v.clone())),
        LineData::I64(v) => bits_of_field(&Field::I64(v.clone())),
        LineData::F32(v) => bits_of_field(&Field::F32(v.clone())),
        LineData::None => Vec::new(),
    }
}

fn bits_of_f32s(v: &[f32]) -> Vec<u64> {
    bits_of_field(&Field::F32(v.to_vec()))
}

fn read_records(name: &str) -> Vec<Vec<Field>> {
    let path = format!("{}/tests/data/kernels/{}", env!("CARGO_MANIFEST_DIR"), name);
    let buf = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let mut pos = 0usize;
    let mut take = |n: usize| -> &[u8] {
        let s = &buf[pos..pos + n];
        pos += n;
        s
    };
    let u32_at = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let mut records = Vec::new();
    let mut remaining = buf.len();
    while remaining > 0 {
        let nf = u32_at(take(4));
        let mut used = 4;
        let mut fields = Vec::with_capacity(nf as usize);
        for _ in 0..nf {
            let tag = take(1)[0];
            let count = u32_at(take(4)) as usize;
            let size = match tag {
                1 | 3 | 4 => 4,
                2 | 5 => 8,
                6 => 2,
                t => panic!("{name}: bad tag {t}"),
            };
            let d = take(size * count);
            used += 5 + size * count;
            fields.push(match tag {
                1 => Field::I32(
                    d.chunks_exact(4)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                2 => Field::I64(
                    d.chunks_exact(8)
                        .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                3 => Field::F32(
                    d.chunks_exact(4)
                        .map(|c| f32::from_bits(u32_at(c)))
                        .collect(),
                ),
                4 => Field::U32(d.chunks_exact(4).map(u32_at).collect()),
                5 => Field::U64(
                    d.chunks_exact(8)
                        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
                _ => Field::U16(
                    d.chunks_exact(2)
                        .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                ),
            });
        }
        remaining -= used;
        records.push(fields);
    }
    assert!(!records.is_empty(), "{name}: no records");
    records
}

// ---------------------------------------------------------------------------
// Kernels, as init_rev53 / init_irv97 in ojph_params.cpp
// ---------------------------------------------------------------------------

fn rev_step(a: i16, b: i16, e: u8) -> LiftingStep {
    LiftingStep {
        rev_a: a,
        rev_b: b,
        rev_e: e,
        ..Default::default()
    }
}

fn irv_step(a: f32) -> LiftingStep {
    LiftingStep {
        irv_a: a,
        ..Default::default()
    }
}

fn rev53() -> ParamAtk {
    let steps = vec![rev_step(1, 2, 2), rev_step(-1, 1, 1)];
    ParamAtk {
        latk: (5 + 2 * 2 + 2 + 2) as u16,
        satk: 0x5801,
        katk: 0.0,
        steps,
        others: Vec::new(),
    }
}

fn irv97() -> ParamAtk {
    let steps = vec![
        irv_step(0.443506852043971f64 as f32),
        irv_step(0.882911075530934f64 as f32),
        irv_step(-0.052980118572961f64 as f32),
        irv_step(-1.586134342059924f64 as f32),
    ];
    ParamAtk {
        latk: (5 + 4 + 4 * (1 + 4)) as u16,
        satk: 0x4a00,
        katk: 1.230174104914001f64 as f32,
        steps,
        others: Vec::new(),
    }
}

/// A kernel from the steps field of a dump record: `[a, b, e]*` for a
/// reversible kernel, `[K, a*]` for an irreversible one.
fn atk_from_steps(f: &Field) -> ParamAtk {
    match f {
        Field::I32(v) => {
            let steps = v
                .chunks_exact(3)
                .map(|c| rev_step(c[0] as i16, c[1] as i16, c[2] as u8))
                .collect();
            ParamAtk {
                latk: 0,
                satk: 0x5801,
                katk: 0.0,
                steps,
                others: Vec::new(),
            }
        }
        Field::F32(v) => {
            let steps = v[1..].iter().map(|&a| irv_step(a)).collect();
            ParamAtk {
                latk: 0,
                satk: 0x4a00,
                katk: v[0],
                steps,
                others: Vec::new(),
            }
        }
        f => panic!("bad steps field {f:?}"),
    }
}

// ---------------------------------------------------------------------------
// Golden vectors
// ---------------------------------------------------------------------------

#[test]
fn golden_kernels_match_helpers() {
    // The 5/3 and 9/7 kernels that get_atk(1) / get_atk(0) produced match
    // the test helpers bit for bit.
    let recs = read_records("wavelet_horz.bin");
    let rev = recs.iter().find(|r| r[0].u32()[0] == 0).unwrap();
    let a = atk_from_steps(&rev[1]);
    assert_eq!(a.steps, rev53().steps);
    let irv = recs.iter().find(|r| r[0].u32()[0] == 2).unwrap();
    let a = atk_from_steps(&irv[1]);
    let b = irv97();
    assert_eq!(a.get_k().to_bits(), b.get_k().to_bits());
    for (x, y) in a.steps.iter().zip(&b.steps) {
        assert_eq!(x.irv_a.to_bits(), y.irv_a.to_bits());
    }
    assert_eq!(a.steps.len(), b.steps.len());
}

#[test]
fn golden_wavelet_horz() {
    let recs = read_records("wavelet_horz.bin");
    for (i, r) in recs.iter().enumerate() {
        let p = r[0].u32();
        let (kind, width, even, ana) = (p[0], p[1], p[2] != 0, p[3] != 0);
        let atk = atk_from_steps(&r[1]);
        let (mut a, mut b, mut c) = (r[2].line(), r[3].line(), r[4].line());
        match (kind, ana) {
            (2, true) => irv_horz_ana(&atk, &mut a, &mut b, &c, width, even),
            (2, false) => irv_horz_syn(&atk, &mut a, &mut b, &mut c, width, even),
            (_, true) => rev_horz_ana(&atk, &mut a, &mut b, &c, width, even),
            (_, false) => rev_horz_syn(&atk, &mut a, &mut b, &mut c, width, even),
        }
        for (j, l) in [&a, &b, &c].iter().enumerate() {
            assert_eq!(
                bits_of_line(l),
                bits_of_field(&r[5 + j]),
                "record {i} params {p:?} line {j}"
            );
        }
    }
    eprintln!("wavelet_horz: {} records", recs.len());
}

#[test]
fn golden_wavelet_vert() {
    let recs = read_records("wavelet_vert.bin");
    for (i, r) in recs.iter().enumerate() {
        let p = r[0].u32();
        let (kind, repeat, synthesis, alias) = (p[0], p[1], p[2] != 0, p[3] != 0);
        let (sig, other, mut aug) = (r[2].line(), r[3].line(), r[4].line());
        let oth = if alias { &sig } else { &other };
        match kind {
            0 | 1 => {
                let s = r[1].i32();
                rev_vert_step(
                    &rev_step(s[0] as i16, s[1] as i16, s[2] as u8),
                    &sig,
                    oth,
                    &mut aug,
                    repeat,
                    synthesis,
                )
            }
            2 => irv_vert_step(
                &irv_step(r[1].f32()[0]),
                &sig,
                oth,
                &mut aug,
                repeat,
                synthesis,
            ),
            _ => irv_vert_times_k(r[1].f32()[0], &mut aug, repeat),
        }
        assert_eq!(
            bits_of_line(&aug),
            bits_of_field(&r[5]),
            "record {i} params {p:?}"
        );
    }
    eprintln!("wavelet_vert: {} records", recs.len());
}

#[test]
fn golden_colour() {
    let recs = read_records("colour.bin");
    // record 0: CT_CNST
    let c = recs[0][0].f32();
    let ours = [
        ct_cnst::ALPHA_RF,
        ct_cnst::ALPHA_GF,
        ct_cnst::ALPHA_BF,
        ct_cnst::BETA_CB_F,
        ct_cnst::BETA_CR_F,
        ct_cnst::GAMMA_CR2R,
        ct_cnst::GAMMA_CB2B,
        ct_cnst::GAMMA_CR2G,
        ct_cnst::GAMMA_CB2G,
    ];
    assert_eq!(bits_of_f32s(c), bits_of_f32s(&ours), "CT_CNST");

    for (i, r) in recs.iter().enumerate().skip(1) {
        let p = r[0].u32();
        let func = p[0];
        match func {
            0 | 1 => {
                let (so, doff, width) = (p[2], p[3], p[4]);
                let shift = r[1].i64()[0];
                let src = r[2].line();
                let mut dst = r[3].line();
                if func == 0 {
                    rev_convert(&src, so, &mut dst, doff, shift, width);
                } else {
                    rev_convert_nlt_type3(&src, so, &mut dst, doff, shift, width);
                }
                assert_eq!(
                    bits_of_line(&dst),
                    bits_of_field(&r[4]),
                    "record {i} params {p:?}"
                );
            }
            2..=5 => {
                let (bd, is_signed, off, width) = (p[1], p[2] != 0, p[3], p[4]);
                let src = r[1].line();
                let mut dst = r[2].line();
                match func {
                    2 => irv_convert_to_integer(&src, &mut dst, off, bd, is_signed, width),
                    3 => {
                        irv_convert_to_integer_nlt_type3(&src, &mut dst, off, bd, is_signed, width)
                    }
                    4 => irv_convert_to_float(&src, off, &mut dst, bd, is_signed, width),
                    _ => irv_convert_to_float_nlt_type3(&src, off, &mut dst, bd, is_signed, width),
                }
                assert_eq!(
                    bits_of_line(&dst),
                    bits_of_field(&r[3]),
                    "record {i} params {p:?}"
                );
            }
            6 | 7 => {
                let repeat = p[2];
                let mut l: Vec<LineBuf> = (1..7).map(|j| r[j].line()).collect();
                let (rgb, ycc) = l.split_at_mut(3);
                let [rr, gg, bb] = rgb else { unreachable!() };
                let [yy, cb, cr] = ycc else { unreachable!() };
                if func == 6 {
                    rct_forward(rr, gg, bb, yy, cb, cr, repeat);
                } else {
                    rct_backward(yy, cb, cr, rr, gg, bb, repeat);
                }
                for (j, line) in l.iter().enumerate() {
                    assert_eq!(
                        bits_of_line(line),
                        bits_of_field(&r[7 + j]),
                        "record {i} params {p:?} line {j}"
                    );
                }
            }
            _ => {
                let repeat = p[2];
                let n = repeat as usize;
                let (mut o0, mut o1, mut o2) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
                let (a, b, c) = (r[1].f32(), r[2].f32(), r[3].f32());
                if func == 8 {
                    ict_forward(a, b, c, &mut o0, &mut o1, &mut o2, repeat);
                } else {
                    ict_backward(a, b, c, &mut o0, &mut o1, &mut o2, repeat);
                }
                for (j, o) in [&o0, &o1, &o2].iter().enumerate() {
                    assert_eq!(
                        bits_of_f32s(o),
                        bits_of_field(&r[4 + j]),
                        "record {i} params {p:?} out {j}"
                    );
                }
            }
        }
    }
    eprintln!("colour: {} records", recs.len());
}

#[test]
fn golden_nlt() {
    let recs = read_records("nlt.bin");
    for (i, r) in recs.iter().enumerate() {
        let p = r[0].u32();
        let f = r[1].f32();
        let (variant, width, off) = (p[0], p[7], p[8]);
        let rec = NltLut {
            nlt_type: p[1] as u8,
            bit_depth: p[2] as u8,
            is_signed: p[3] != 0,
            approx_encoding_nlt: p[4] != 0,
            precise_max_steps: p[5],
            num_points: p[6],
            multiplier: f[0],
            fd_min: f[1],
            fd_max: f[2],
            delta: f[3],
            inv_delta: f[4],
            ft_min: f[5],
            ft_max: f[6],
            dec_points: r[2].f32(),
            approx_enc_points: r[3].f32(),
            precise_enc_dec_indices: r[4].u16(),
            precise_dec_lut: r[5].f32(),
        };
        let src = r[6].line();
        let mut dst = r[7].line();
        // the bit_depth and is_signed arguments are unused; the dump passes
        // 7 and !is_signed
        if variant == 0 {
            irv_convert_to_integer_nlt(&src, &mut dst, off, 7, !rec.is_signed, width, &rec);
        } else {
            irv_convert_to_float_nlt(&src, off, &mut dst, 7, !rec.is_signed, width, &rec);
        }
        assert_eq!(
            bits_of_line(&dst),
            bits_of_field(&r[8]),
            "record {i} params {p:?}"
        );
    }
    eprintln!("nlt: {} records", recs.len());
}

#[test]
fn golden_codeblock() {
    let recs = read_records("codeblock.bin");
    for (i, r) in recs.iter().enumerate() {
        let p = r[0].u32();
        let (func, k_max, count) = (p[0], p[1], p[2]);
        let n = count as usize;
        let delta = r[1].f32()[0];
        let msg = format!("record {i} params {p:?}");
        match func {
            0 => {
                let mut dp = vec![0xDEAD_BEEFu32; n];
                let mut mv = r[3].u32()[0];
                rev_tx_to_cb32(r[2].i32(), &mut dp, k_max, delta, count, &mut mv);
                assert_eq!(dp, r[4].u32(), "{msg}");
                assert_eq!(mv, r[5].u32()[0], "{msg}");
            }
            1 => {
                let mut dp = vec![0xDEAD_BEEFu64; n];
                let mut mv = r[3].u64()[0];
                rev_tx_to_cb64(r[2].i64(), &mut dp, k_max, delta, count, &mut mv);
                assert_eq!(dp, r[4].u64(), "{msg}");
                assert_eq!(mv, r[5].u64()[0], "{msg}");
            }
            2 => {
                let mut dp = vec![0xDEAD_BEEFu32; n];
                let mut mv = r[3].u32()[0];
                irv_tx_to_cb32(r[2].f32(), &mut dp, k_max, delta, count, &mut mv);
                assert_eq!(dp, r[4].u32(), "{msg}");
                assert_eq!(mv, r[5].u32()[0], "{msg}");
            }
            3 => {
                let mut dp = vec![0x5A5A_5A5Ai32; n];
                rev_tx_from_cb32(r[2].u32(), &mut dp, k_max, delta, count);
                assert_eq!(dp, r[3].i32(), "{msg}");
            }
            4 => {
                let mut dp = vec![0x5A5A_5A5Ai64; n];
                rev_tx_from_cb64(r[2].u64(), &mut dp, k_max, delta, count);
                assert_eq!(dp, r[3].i64(), "{msg}");
            }
            5 => {
                let mut dp = vec![0.0f32; n];
                irv_tx_from_cb32(r[2].u32(), &mut dp, k_max, delta, count);
                assert_eq!(bits_of_f32s(&dp), bits_of_field(&r[3]), "{msg}");
            }
            _ => {
                let mut dp = vec![0.0f32; n];
                irv_tx_from_cb64(r[2].u64(), &mut dp, k_max, delta, count);
                assert_eq!(bits_of_f32s(&dp), bits_of_field(&r[3]), "{msg}");
            }
        }
    }
    eprintln!("codeblock: {} records", recs.len());
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// A value in `[-2^(bits-1), 2^(bits-1))`.
    fn signed(&mut self, bits: u32) -> i64 {
        (self.next() >> (64 - bits)) as i64 - (1i64 << (bits - 1))
    }
}

fn line_i32(size: usize) -> LineBuf {
    LineBuf::new_i32(size + 1, 1)
}

fn line_i64(size: usize) -> LineBuf {
    LineBuf::new_i64(size + 1, 1)
}

#[test]
fn rev_analysis_then_synthesis_is_identity() {
    let mut rng = Rng(1);
    let kernels = [
        rev53(),
        ParamAtk {
            latk: 0,
            satk: 0x5801,
            katk: 0.0,
            steps: vec![rev_step(-1, 3, 2), rev_step(3, -5, 3)],
            others: Vec::new(),
        },
        ParamAtk {
            latk: 0,
            satk: 0x5801,
            katk: 0.0,
            steps: vec![rev_step(1, 0, 0), rev_step(-2, 1, 1)],
            others: Vec::new(),
        },
    ];
    let mut cases = 0;
    for atk in &kernels {
        for width in 1..=70u32 {
            for even in [true, false] {
                let w = width as usize;
                let half = w.div_ceil(2);
                // 32-bit
                let mut src = line_i32(w);
                for v in &mut src.i32_mut()[..w] {
                    *v = rng.signed(24) as i32;
                }
                let (mut l, mut h, mut dst) = (line_i32(half), line_i32(half), line_i32(w));
                rev_horz_ana(atk, &mut l, &mut h, &src, width, even);
                rev_horz_syn(atk, &mut dst, &mut l, &mut h, width, even);
                assert_eq!(
                    &dst.i32()[..w],
                    &src.i32()[..w],
                    "i32 width {width} even {even}"
                );
                // 64-bit
                let mut src = line_i64(w);
                for v in &mut src.i64_mut()[..w] {
                    *v = rng.signed(50);
                }
                let (mut l, mut h, mut dst) = (line_i64(half), line_i64(half), line_i64(w));
                rev_horz_ana(atk, &mut l, &mut h, &src, width, even);
                rev_horz_syn(atk, &mut dst, &mut l, &mut h, width, even);
                assert_eq!(
                    &dst.i64()[..w],
                    &src.i64()[..w],
                    "i64 width {width} even {even}"
                );
                cases += 2;
            }
        }
    }
    // vertical steps: synthesis undoes analysis, even with wrapping
    for atk in &kernels {
        for s in &atk.steps {
            let n = 37;
            let mut sig = line_i32(n);
            let mut aug = line_i32(n);
            for v in sig.i32_mut()[..n]
                .iter_mut()
                .chain(aug.i32_mut()[..n].iter_mut())
            {
                *v = rng.next() as i32;
            }
            let orig = aug.clone();
            rev_vert_step(s, &sig, &sig, &mut aug, n as u32, false);
            rev_vert_step(s, &sig, &sig, &mut aug, n as u32, true);
            assert_eq!(aug, orig);
            cases += 1;
        }
    }
    eprintln!("rev identity: {cases} cases");
}

#[test]
fn rct_forward_then_backward_is_identity() {
    let mut rng = Rng(2);
    for wide in [false, true] {
        for repeat in [0u32, 1, 2, 7, 33, 64] {
            let n = repeat as usize;
            let mut rgb: Vec<LineBuf> = (0..3).map(|_| line_i32(n)).collect();
            for l in &mut rgb {
                for v in &mut l.i32_mut()[..n] {
                    // RCT is exact while 2 * g and the sums fit
                    *v = rng.signed(if wide { 32 } else { 28 }) as i32;
                }
            }
            let mut ycc: Vec<LineBuf> = (0..3)
                .map(|_| if wide { line_i64(n) } else { line_i32(n) })
                .collect();
            {
                let [y, cb, cr] = &mut ycc[..] else {
                    unreachable!()
                };
                rct_forward(&rgb[0], &rgb[1], &rgb[2], y, cb, cr, repeat);
            }
            let mut out: Vec<LineBuf> = (0..3).map(|_| line_i32(n)).collect();
            {
                let [r, g, b] = &mut out[..] else {
                    unreachable!()
                };
                rct_backward(&ycc[0], &ycc[1], &ycc[2], r, g, b, repeat);
            }
            for c in 0..3 {
                assert_eq!(
                    &out[c].i32()[..n],
                    &rgb[c].i32()[..n],
                    "wide {wide} repeat {repeat} comp {c}"
                );
            }
        }
    }
}

#[test]
fn rev_tx_to_cb_then_from_cb_is_identity() {
    let mut rng = Rng(3);
    for k_max in 1..=31u32 {
        let n = 50;
        let src: Vec<i32> = (0..n)
            .map(|_| {
                let m = (rng.next() as u32) >> (32 - k_max); // |v| < 2^k_max
                if rng.next() & 1 == 0 {
                    m as i32
                } else {
                    (m as i32).wrapping_neg()
                }
            })
            .collect();
        let mut cb = vec![0u32; n];
        let mut mv = 0u32;
        rev_tx_to_cb32(&src, &mut cb, k_max, 0.0, n as u32, &mut mv);
        let mut back = vec![0i32; n];
        rev_tx_from_cb32(&cb, &mut back, k_max, 0.0, n as u32);
        assert_eq!(back, src, "k_max {k_max}");
        assert_eq!(mv, cb.iter().fold(0, |a, &v| a | (v & 0x7FFF_FFFF)));
    }
    for k_max in 1..=63u32 {
        let n = 50;
        let src: Vec<i64> = (0..n)
            .map(|_| {
                let m = rng.next() >> (64 - k_max);
                if rng.next() & 1 == 0 {
                    m as i64
                } else {
                    (m as i64).wrapping_neg()
                }
            })
            .collect();
        let mut cb = vec![0u64; n];
        let mut mv = 0u64;
        rev_tx_to_cb64(&src, &mut cb, k_max, 0.0, n as u32, &mut mv);
        let mut back = vec![0i64; n];
        rev_tx_from_cb64(&cb, &mut back, k_max, 0.0, n as u32);
        assert_eq!(back, src, "k_max {k_max}");
    }
}

#[test]
fn irv_analysis_then_synthesis_is_close() {
    // Not exact, but a sanity check of the float path end to end.
    let mut rng = Rng(4);
    let atk = irv97();
    for width in 1..=70u32 {
        for even in [true, false] {
            let w = width as usize;
            let half = w.div_ceil(2);
            let mut src = LineBuf::new_f32(w + 1, 1);
            for v in &mut src.f32_mut()[..w] {
                *v = (rng.next() >> 40) as f32 / 16_777_216.0 - 0.5;
            }
            let (mut l, mut h, mut dst) = (
                LineBuf::new_f32(half + 1, 1),
                LineBuf::new_f32(half + 1, 1),
                LineBuf::new_f32(w + 1, 1),
            );
            irv_horz_ana(&atk, &mut l, &mut h, &src, width, even);
            irv_horz_syn(&atk, &mut dst, &mut l, &mut h, width, even);
            for (a, b) in dst.f32()[..w].iter().zip(&src.f32()[..w]) {
                assert!(
                    (a - b).abs() < 1e-5,
                    "width {width} even {even}: {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn mem_clear_and_find_max_val() {
    let mut a = [1u32, 2, 3];
    mem_clear(&mut a);
    assert_eq!(a, [0, 0, 0]);
    let mut b = [5u64; 4];
    mem_clear(&mut b[1..]);
    assert_eq!(b, [5, 0, 0, 0]);
    assert_eq!(find_max_val32(&[7, 9]), 7);
    assert_eq!(find_max_val64(&[u64::MAX, 1]), u64::MAX);
}

// ---------------------------------------------------------------------------
// The `simd` feature's vectorised kernels against the scalar ones
// ---------------------------------------------------------------------------

#[cfg(feature = "simd")]
mod vectorised {
    use super::super::vectorised as v;
    use super::*;

    /// Special floats: zeros, infinities, NaNs, the limits of `i32` and of
    /// the float ranges, halves and their neighbours.
    const SPECIAL: [f32; 24] = [
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        f32::MAX,
        f32::MIN,
        2147483648.0,
        -2147483648.0,
        2147483520.0,
        -2147483520.0,
        4294967296.0,
        0.5,
        -0.5,
        0.49999997,
        -0.49999997,
        0.50000006,
        -0.50000006,
        1.0,
        -1.0,
        1.5,
    ];

    /// A float: usually in `[-scale, scale]`, often a half-integer
    /// multiple of `1 / unit` (a rounding tie), sometimes special.
    pub(super) fn float(rng: &mut Rng, scale: f32, unit: f32) -> f32 {
        let r = rng.next();
        match r % 16 {
            0 => SPECIAL[(r >> 8) as usize % SPECIAL.len()],
            1 | 2 => ((r >> 8) as i32 % (2 * unit as i32 + 1)) as f32 / unit + 0.5 / unit,
            3 => f32::from_bits((r >> 16) as u32),
            _ => ((r >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * scale,
        }
    }

    /// Lengths: short ones, around the chunk size, and long ones.
    pub(super) fn lengths() -> Vec<usize> {
        let mut l: Vec<usize> = (0..=20).collect();
        l.extend([31, 32, 33, 63, 64, 65, 127, 128, 129, 200, 1000, 4100]);
        l
    }

    fn f32_line(vals: &[f32]) -> LineBuf {
        let mut l = LineBuf::new_f32(vals.len() + 1, 1);
        l.f32_mut()[..vals.len()].copy_from_slice(vals);
        l
    }

    /// The record of a golden NLT test vector.
    fn nlt_rec(r: &[Field]) -> NltLut<'_> {
        let p = r[0].u32();
        let f = r[1].f32();
        NltLut {
            nlt_type: p[1] as u8,
            bit_depth: p[2] as u8,
            is_signed: p[3] != 0,
            approx_encoding_nlt: p[4] != 0,
            precise_max_steps: p[5],
            num_points: p[6],
            multiplier: f[0],
            fd_min: f[1],
            fd_max: f[2],
            delta: f[3],
            inv_delta: f[4],
            ft_min: f[5],
            ft_max: f[6],
            dec_points: r[2].f32(),
            approx_enc_points: r[3].f32(),
            precise_enc_dec_indices: r[4].u16(),
            precise_dec_lut: r[5].f32(),
        }
    }

    /// An integer: usually of `bits` bits, sometimes an extreme.
    pub(super) fn int(rng: &mut Rng, bits: u32) -> i32 {
        let r = rng.next();
        match r % 16 {
            0 => [0, -1, 1, i32::MIN, i32::MAX, i32::MIN + 1][(r >> 8) as usize % 6],
            1 => (r >> 32) as i32,
            _ => rng.signed(bits.clamp(2, 32)) as i32,
        }
    }

    /// A code-block sample: sign-magnitude, of any magnitude.
    fn cb_sample(rng: &mut Rng) -> u32 {
        let r = rng.next();
        match r % 8 {
            0 => [0, 0x8000_0000, 0x7FFF_FFFF, 0xFFFF_FFFF, 1, 0x8000_0001][(r >> 8) as usize % 6],
            _ => (r >> 32) as u32 >> ((r >> 8) % 32),
        }
    }

    #[test]
    fn tx_to_cb32() {
        let mut rng = Rng(0x7478_6362);
        for n in lengths() {
            for k_max in [0, 1, 7, 15, 16, 29, 30, 31] {
                let ints: Vec<i32> = (0..n).map(|_| int(&mut rng, 32 - k_max.min(30))).collect();
                let (mut want, mut got) = (vec![1u32; n + 1], vec![1u32; n + 1]);
                let mv0 = (rng.next() >> 40) as u32;
                let (mut mv_want, mut mv_got) = (mv0, mv0);
                rev_tx_to_cb32_scalar(&ints, &mut want, k_max, 0.0, n as u32, &mut mv_want);
                v::rev_tx_to_cb32(&ints, &mut got, k_max, 0.0, n as u32, &mut mv_got);
                assert_eq!(
                    (want.clone(), mv_want),
                    (got.clone(), mv_got),
                    "rev {n} {k_max}"
                );

                let delta_inv = [1.0, 0.5, 1e3, 1e-3, 65536.0, 3.0e9][k_max as usize % 6];
                let floats: Vec<f32> = (0..n).map(|_| float(&mut rng, 4.0, 2.0)).collect();
                let mv1 = mv_got;
                irv_tx_to_cb32_scalar(&floats, &mut want, k_max, delta_inv, n as u32, &mut mv_want);
                v::irv_tx_to_cb32(&floats, &mut got, k_max, delta_inv, n as u32, &mut mv_got);
                assert_eq!((&want, mv_want), (&got, mv_got), "irv {n} {delta_inv}");
                // the intrinsics kernel, where it runs
                let mut mv_got = mv1;
                got.fill(1);
                if v::irv_tx_to_cb32_avx2(&floats, &mut got, delta_inv, n as u32, &mut mv_got) {
                    assert_eq!((&want, mv_want), (&got, mv_got), "avx2 {n} {delta_inv}");
                }
            }
        }
    }

    #[test]
    fn tx_from_cb32() {
        let mut rng = Rng(0x6672_6f6d);
        for n in lengths() {
            let src: Vec<u32> = (0..n).map(|_| cb_sample(&mut rng)).collect();
            for k_max in [0, 1, 7, 15, 16, 29, 30, 31] {
                let (mut want, mut got) = (vec![7i32; n + 1], vec![7i32; n + 1]);
                rev_tx_from_cb32_scalar(&src, &mut want, k_max, 0.0, n as u32);
                v::rev_tx_from_cb32(&src, &mut got, k_max, 0.0, n as u32);
                assert_eq!(want, got, "rev {n} {k_max}");
            }
            for delta in [
                1.0f32,
                0.5,
                1e-3,
                3.0e-9,
                1e30,
                f32::INFINITY,
                f32::NAN,
                -0.0,
            ] {
                let (mut want, mut got) = (vec![7f32; n + 1], vec![7f32; n + 1]);
                irv_tx_from_cb32_scalar(&src, &mut want, 0, delta, n as u32);
                v::irv_tx_from_cb32(&src, &mut got, 0, delta, n as u32);
                assert_eq!(bits_of_f32s(&want), bits_of_f32s(&got), "irv {n} {delta}");
            }
        }
    }

    /// Lines of random samples (`bits`-bit integers, or floats in
    /// [-1, 1]) and room for the extensions.
    fn rand_line(rng: &mut Rng, kind: u8, n: usize, bits: u32) -> LineBuf {
        let mut l = match kind {
            0 => line_i32(n + 1),
            1 => line_i64(n + 1),
            _ => LineBuf::new_f32(n + 2, 1),
        };
        match kind {
            0 => l
                .i32_full_mut()
                .iter_mut()
                .for_each(|v| *v = rng.signed(bits) as i32),
            1 => l
                .i64_full_mut()
                .iter_mut()
                .for_each(|v| *v = rng.signed(bits)),
            _ => l
                .f32_full_mut()
                .iter_mut()
                .for_each(|v| *v = rng.signed(24) as f32 / (1 << 23) as f32),
        }
        l
    }

    /// The wavelet kernels compiled by multiversion against their bodies
    /// (the same source; this checks the wiring and that nothing in the
    /// AVX2 versions changes the results), on every line written,
    /// extension samples included.
    #[test]
    fn wavelet() {
        let mut rng = Rng(0x7761_7665);
        let rev = [
            rev53(),
            ParamAtk {
                latk: 0,
                satk: 0x5801,
                katk: 0.0,
                steps: vec![rev_step(-1, 3, 2), rev_step(3, -5, 3), rev_step(1, 0, 0)],
                others: Vec::new(),
            },
        ];
        let irv = [
            irv97(),
            ParamAtk {
                latk: 0,
                satk: 0x4a00,
                katk: 0.75,
                steps: vec![irv_step(0.3), irv_step(-1.7)],
                others: Vec::new(),
            },
        ];
        let widths: Vec<u32> = (0..=40)
            .chain([63, 64, 65, 127, 128, 129, 1000, 1001])
            .collect();
        for &width in &widths {
            let w = width as usize;
            let half = w.div_ceil(2) + 1;
            for even in [true, false] {
                for (kind, atks) in [(0u8, &rev), (1, &rev), (2, &irv)] {
                    for atk in atks {
                        let bits = if kind == 1 { 50 } else { 24 };
                        let src = rand_line(&mut rng, kind, w, bits);
                        let (l0, h0) = (
                            rand_line(&mut rng, kind, half, bits),
                            rand_line(&mut rng, kind, half, bits),
                        );
                        let what = format!("kind {kind} width {width} even {even}");
                        // analysis
                        let (mut l1, mut h1, mut l2, mut h2) =
                            (l0.clone(), h0.clone(), l0.clone(), h0.clone());
                        if kind == 2 {
                            irv_horz_ana_body(atk, &mut l1, &mut h1, &src, width, even);
                            v::irv_horz_ana(atk, &mut l2, &mut h2, &src, width, even);
                        } else {
                            rev_horz_ana_body(atk, &mut l1, &mut h1, &src, width, even);
                            v::rev_horz_ana(atk, &mut l2, &mut h2, &src, width, even);
                        }
                        assert_eq!(bits_of_line(&l1), bits_of_line(&l2), "ana l {what}");
                        assert_eq!(bits_of_line(&h1), bits_of_line(&h2), "ana h {what}");
                        // synthesis
                        let (mut d1, mut d2) = (src.clone(), src.clone());
                        let (mut l1, mut h1, mut l2, mut h2) =
                            (l0.clone(), h0.clone(), l0.clone(), h0);
                        if kind == 2 {
                            irv_horz_syn_body(atk, &mut d1, &mut l1, &mut h1, width, even);
                            v::irv_horz_syn(atk, &mut d2, &mut l2, &mut h2, width, even);
                        } else {
                            rev_horz_syn_body(atk, &mut d1, &mut l1, &mut h1, width, even);
                            v::rev_horz_syn(atk, &mut d2, &mut l2, &mut h2, width, even);
                        }
                        assert_eq!(bits_of_line(&d1), bits_of_line(&d2), "syn dst {what}");
                        assert_eq!(bits_of_line(&l1), bits_of_line(&l2), "syn l {what}");
                        assert_eq!(bits_of_line(&h1), bits_of_line(&h2), "syn h {what}");
                        // vertical steps
                        if !even {
                            continue;
                        }
                        let (sig, oth) = (
                            rand_line(&mut rng, kind, w, bits),
                            rand_line(&mut rng, kind, w, bits),
                        );
                        for step in &atk.steps {
                            for synthesis in [false, true] {
                                let (mut a1, mut a2) = (src.clone(), src.clone());
                                if kind == 2 {
                                    irv_vert_step_body(step, &sig, &oth, &mut a1, width, synthesis);
                                    v::irv_vert_step(step, &sig, &oth, &mut a2, width, synthesis);
                                } else {
                                    rev_vert_step_body(step, &sig, &oth, &mut a1, width, synthesis);
                                    v::rev_vert_step(step, &sig, &oth, &mut a2, width, synthesis);
                                }
                                assert_eq!(bits_of_line(&a1), bits_of_line(&a2), "vert {what}");
                            }
                        }
                        if kind == 2 {
                            let (mut a1, mut a2) = (src.clone(), src.clone());
                            irv_vert_times_k_body(1.25, &mut a1, width);
                            v::irv_vert_times_k(1.25, &mut a2, width);
                            assert_eq!(bits_of_line(&a1), bits_of_line(&a2), "times_k {what}");
                        }
                    }
                }
            }
        }
    }

    /// The colour kernels (vectorised or compiled by multiversion)
    /// against their scalar bodies.
    #[test]
    fn colour() {
        let mut rng = Rng(0x636f_6c6f);
        for n in lengths() {
            let off = (rng.next() % 4) as u32;
            let w = n as u32;
            // reversible conversions, all three line combinations
            for (sk, dk) in [(0u8, 0u8), (0, 1), (1, 0)] {
                let src = rand_line(&mut rng, sk, n + 4, if sk == 1 { 50 } else { 32 });
                let dst = rand_line(&mut rng, dk, n + 4, 8);
                let shift = rng.signed(40);
                for nlt3 in [false, true] {
                    let (mut a, mut b) = (dst.clone(), dst.clone());
                    if nlt3 {
                        rev_convert_nlt_type3_body(&src, off, &mut a, off, shift, w);
                        v::rev_convert_nlt_type3(&src, off, &mut b, off, shift, w);
                    } else {
                        rev_convert_body(&src, off, &mut a, off, shift, w);
                        v::rev_convert(&src, off, &mut b, off, shift, w);
                    }
                    assert_eq!(a, b, "rev_convert {sk}{dk} nlt3 {nlt3} {n}");
                }
            }
            for bit_depth in [1, 2, 8, 12, 16, 17, 24, 31, 32] {
                for signed in [false, true] {
                    // integers to floats
                    let src = rand_line(&mut rng, 0, n + 4, bit_depth.max(2));
                    let dst = LineBuf::new_f32(n + 1, 1);
                    let (mut a, mut b) = (dst.clone(), dst.clone());
                    irv_convert_to_float_body(&src, off, &mut a, bit_depth, signed, w);
                    v::irv_convert_to_float(&src, off, &mut b, bit_depth, signed, w);
                    assert_eq!(
                        bits_of_line(&a),
                        bits_of_line(&b),
                        "to_float {bit_depth} {n}"
                    );
                    let (mut a, mut b) = (dst.clone(), dst);
                    irv_convert_to_float_nlt_type3_body(&src, off, &mut a, bit_depth, signed, w);
                    v::irv_convert_to_float_nlt_type3(&src, off, &mut b, bit_depth, signed, w);
                    assert_eq!(
                        bits_of_line(&a),
                        bits_of_line(&b),
                        "to_float3 {bit_depth} {n}"
                    );
                    // floats to integers
                    // with halves (after scaling) and the limits
                    let unit = (1u64 << bit_depth) as f32;
                    let vals: Vec<f32> = (0..n)
                        .map(|_| match rng.next() % 4 {
                            0 => (rng.signed(bit_depth.min(24) + 1) as f32 + 0.5) / unit,
                            1 => [
                                -0.5f32,
                                0.5,
                                -0.5 - 1.0 / unit,
                                0.5 - 1.0 / unit,
                                0.5 - 0.5 / unit,
                            ][(rng.next() % 5) as usize],
                            _ => float(&mut rng, 0.7, 4096.0),
                        })
                        .collect();
                    let src = f32_line(&vals);
                    let dst = rand_line(&mut rng, 0, n + 4, 8);
                    for nlt3 in [false, true] {
                        let (mut a, mut b, mut c) = (dst.clone(), dst.clone(), dst.clone());
                        if nlt3 {
                            irv_convert_to_integer_nlt0or3::<true>(
                                &src, &mut a, off, bit_depth, signed, w,
                            );
                            v::irv_convert_to_integer_nlt0or3::<true>(
                                &src, &mut b, off, bit_depth, signed, w,
                            );
                        } else {
                            irv_convert_to_integer_nlt0or3::<false>(
                                &src, &mut a, off, bit_depth, signed, w,
                            );
                            v::irv_convert_to_integer_nlt0or3::<false>(
                                &src, &mut b, off, bit_depth, signed, w,
                            );
                        }
                        let what = format!("to_integer nlt3 {nlt3} {bit_depth} {signed} {n}");
                        assert_eq!(a, b, "{what}");
                        if v::irv_convert_to_integer_avx2(
                            &src, &mut c, off, bit_depth, signed, w, nlt3,
                        ) {
                            assert_eq!(a, c, "avx2 {what}");
                        } else {
                            assert!(!crate::simd::active(), "{what}: the AVX2 kernel declined");
                        }
                    }
                }
            }
            // colour transforms
            for kind in [0u8, 1] {
                let ins: Vec<LineBuf> = (0..3).map(|_| rand_line(&mut rng, 0, n, 31)).collect();
                let outs: Vec<LineBuf> = (0..3).map(|_| rand_line(&mut rng, kind, n, 8)).collect();
                let (mut a, mut b) = (outs.clone(), outs.clone());
                let [a0, a1, a2] = &mut a[..] else {
                    unreachable!()
                };
                rct_forward_body(&ins[0], &ins[1], &ins[2], a0, a1, a2, w);
                let [b0, b1, b2] = &mut b[..] else {
                    unreachable!()
                };
                v::rct_forward(&ins[0], &ins[1], &ins[2], b0, b1, b2, w);
                assert_eq!(a, b, "rct_forward {kind} {n}");
                let ys: Vec<LineBuf> = (0..3)
                    .map(|_| rand_line(&mut rng, kind, n, if kind == 1 { 50 } else { 31 }))
                    .collect();
                let outs: Vec<LineBuf> = (0..3).map(|_| rand_line(&mut rng, 0, n, 8)).collect();
                let (mut a, mut b) = (outs.clone(), outs);
                let [a0, a1, a2] = &mut a[..] else {
                    unreachable!()
                };
                rct_backward_body(&ys[0], &ys[1], &ys[2], a0, a1, a2, w);
                let [b0, b1, b2] = &mut b[..] else {
                    unreachable!()
                };
                v::rct_backward(&ys[0], &ys[1], &ys[2], b0, b1, b2, w);
                assert_eq!(a, b, "rct_backward {kind} {n}");
            }
            let ins: Vec<Vec<f32>> = (0..3)
                .map(|_| (0..n).map(|_| float(&mut rng, 1.0, 4096.0)).collect())
                .collect();
            for backward in [false, true] {
                let (mut a, mut b) = (vec![vec![0f32; n]; 3], vec![vec![0f32; n]; 3]);
                let [a0, a1, a2] = &mut a[..] else {
                    unreachable!()
                };
                let [b0, b1, b2] = &mut b[..] else {
                    unreachable!()
                };
                if backward {
                    ict_backward_body(&ins[0], &ins[1], &ins[2], a0, a1, a2, w);
                    v::ict_backward(&ins[0], &ins[1], &ins[2], b0, b1, b2, w);
                } else {
                    ict_forward_body(&ins[0], &ins[1], &ins[2], a0, a1, a2, w);
                    v::ict_forward(&ins[0], &ins[1], &ins[2], b0, b1, b2, w);
                }
                for (x, y) in a.iter().zip(&b) {
                    assert_eq!(bits_of_f32s(x), bits_of_f32s(y), "ict {backward} {n}");
                }
            }
        }
    }

    #[test]
    fn irv_convert_to_float_nlt_precise_1step() {
        let recs = read_records("nlt.bin");
        let mut rng = Rng(0x6e6c_7466);
        let mut checked = 0;
        for r in recs.iter().filter(|r| r[0].u32()[0] != 0) {
            let rec = nlt_rec(r);
            if rec.approx_encoding_nlt || rec.precise_max_steps != 1 {
                continue;
            }
            let bits = u32::from(rec.bit_depth) + 1;
            for n in lengths() {
                let src: Vec<i32> = (0..n).map(|_| int(&mut rng, bits)).collect();
                let mut want = vec![0f32; n];
                let mut got = vec![0f32; n];
                if rec.nlt_type == NLT_LUT_STYLE {
                    irv_convert_to_float_nlt2or4_precise_1step::<false>(&src, &mut want, &rec);
                    v::irv_convert_to_float_nlt2or4_precise_1step::<false>(&src, &mut got, &rec);
                } else {
                    irv_convert_to_float_nlt2or4_precise_1step::<true>(&src, &mut want, &rec);
                    v::irv_convert_to_float_nlt2or4_precise_1step::<true>(&src, &mut got, &rec);
                }
                assert_eq!(bits_of_f32s(&want), bits_of_f32s(&got), "{:?}, {n}", r[0]);
                checked += 1;
            }
        }
        assert!(checked > 30, "{checked}");
    }

    #[test]
    fn irv_convert_to_integer_nlt() {
        let recs = read_records("nlt.bin");
        let mut rng = Rng(0x6e6c_7469);
        let mut checked = 0;
        for r in recs.iter().filter(|r| r[0].u32()[0] == 0) {
            let p = r[0].u32();
            let rec = nlt_rec(r);
            for n in lengths() {
                let src: Vec<f32> = (0..n).map(|_| float(&mut rng, 0.6, 4096.0)).collect();
                let line = f32_line(&src);
                let mut want = LineBuf::new_i32(n + 3, 1);
                let mut got = vec![0i32; n];
                if rec.nlt_type == NLT_LUT_STYLE {
                    irv_convert_to_integer_nlt2or4::<false>(&line, &mut want, 2, n as u32, &rec);
                    v::irv_convert_to_integer_nlt2or4::<false>(&src, &mut got, &rec);
                } else {
                    irv_convert_to_integer_nlt2or4::<true>(&line, &mut want, 2, n as u32, &rec);
                    v::irv_convert_to_integer_nlt2or4::<true>(&src, &mut got, &rec);
                }
                assert_eq!(&want.i32()[2..2 + n], &got[..], "params {p:?}, {n} samples");
                checked += 1;
            }
        }
        assert!(checked > 30, "{checked}");
    }
}
