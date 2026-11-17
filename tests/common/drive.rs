// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Drives [`htj2k::codestream::Encoder`] from an `ojph_enc` command line,
//! with the same calls, in the same order, as `oracle/ojph_enc.cpp`.

use super::synth::EncArgs;
use htj2k::codestream::Encoder;
use htj2k::params::{nonlinearity, CommentExchange};
use htj2k::types::{Point, Size};

/// The version comment OpenJPH 0.33.0 writes.
pub const OPENJPH_VERSION_COMMENT: &str = "OpenJPH Ver 0.33.0.";

/// An encoder set up as `ojph_enc` sets up `ojph::codestream`, up to (not
/// including) `write_headers`.
pub fn configure(a: &EncArgs) -> htj2k::Result<Encoder> {
    let mut cs = Encoder::new();
    cs.set_version_comment(Some(OPENJPH_VERSION_COMMENT))?;
    let (xs, ys) = a.extent();
    {
        let siz = cs.access_siz_mut()?;
        siz.set_image_extent(Point::new(xs, ys));
        siz.set_image_offset(Point::new(a.img_off.0, a.img_off.1));
        match a.tile {
            Some((w, h)) => siz.set_tile_size(Size::new(w, h)),
            None => siz.set_tile_size(Size::new(xs, ys)),
        }
        siz.set_tile_offset(Point::new(a.tile_off.0, a.tile_off.1));
        siz.set_num_components(a.num_comps);
        for c in 0..a.num_comps {
            let (dx, dy) = a.sub[c as usize];
            let (depth, signed) = a.format(c);
            siz.set_component(c, Point::new(dx, dy), depth, signed)?;
        }
    }
    {
        let cod = cs.access_cod_mut()?;
        cod.set_num_decomposition(a.levels)?;
        cod.set_block_dims(a.bw, a.bh)?;
        if !a.precincts.is_empty() {
            let p: Vec<Size> = a.precincts.iter().map(|&(w, h)| Size::new(w, h)).collect();
            cod.set_precinct_size(&p)?;
        }
        cod.set_progression_order(&a.prog)?;
        cod.set_color_transform(a.rct);
        cod.set_reversible(a.reversible);
        for c in &a.cocs {
            cod.set_num_decomposition_comp(c.comp, c.levels)?;
            cod.set_block_dims_comp(c.comp, c.bw, c.bh)?;
            cod.set_reversible_comp(c.comp, a.reversible);
        }
    }
    if !a.reversible {
        cs.access_qcd_mut()?.set_irrev_quant(a.qstep);
    }
    for c in 0..a.num_comps {
        let want = (a.nlt_all && a.format(c).1) || a.nlt_comps.contains(&c);
        if want {
            cs.access_nlt_mut()?
                .set_nonlinear_transform(c, nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT)?;
        }
    }
    cs.set_planar(a.planar)?;
    if a.tlm {
        cs.request_tlm_marker(true)?;
    }
    if a.tp_res || a.tp_comp {
        cs.set_tilepart_divisions(a.tp_res, a.tp_comp)?;
    }
    Ok(cs)
}

/// `configure` and `write_headers` into `out`.
pub fn headers(a: &EncArgs, out: &mut Vec<u8>) -> htj2k::Result<Encoder> {
    let mut cs = configure(a)?;
    cs.write_headers(out, &[] as &[CommentExchange])?;
    Ok(cs)
}

/// Feeds `img` (from [`EncArgs::synthesise`]) to `cs` through
/// `exchange`, as `ojph_enc` does: unsigned 32-bit values above
/// `i32::MAX` go in as their bit pattern.
pub fn push_image(cs: &mut Encoder, a: &EncArgs, img: &[Vec<i64>]) -> htj2k::Result<()> {
    let sizes = a.comp_sizes();
    let mut row = vec![0usize; img.len()];
    while let Some((c, line)) = cs.exchange()? {
        let c = c as usize;
        let w = sizes[c].0 as usize;
        assert!(
            row[c] < sizes[c].1 as usize,
            "exchange asked for too many lines"
        );
        let src = &img[c][row[c] * w..(row[c] + 1) * w];
        for (d, &s) in line.iter_mut().zip(src) {
            *d = s as u32 as i32;
        }
        row[c] += 1;
    }
    for (c, r) in row.iter().enumerate() {
        assert_eq!(
            *r, sizes[c].1 as usize,
            "component {c} was not fully exchanged"
        );
    }
    Ok(())
}

/// Encodes `a`'s synthesised image, appending the codestream to `out`.
pub fn encode(a: &EncArgs, out: &mut Vec<u8>) -> htj2k::Result<()> {
    let img = a.synthesise();
    let mut cs = headers(a, out)?;
    push_image(&mut cs, a, &img)?;
    cs.flush(out)
}
