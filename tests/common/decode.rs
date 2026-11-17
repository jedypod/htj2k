// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Decoding a codestream as `oracle/ojph_dec` does, for comparison with
//! its `.raw` output.

use htj2k::codestream::Codestream;

/// One decoded component: its width, height and samples, row by row.
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub samples: Vec<i32>,
}

/// Decodes a codestream the way `ojph_dec` does, to its `.raw` layout:
/// the components one after the other, each sample as 4 little-endian
/// bytes.
pub fn decode_to_raw(j2c: &[u8]) -> Result<Vec<u8>, String> {
    Ok(decode_planes(j2c)?
        .iter()
        .flat_map(|p| p.samples.iter().flat_map(|v| v.to_le_bytes()))
        .collect())
}

/// Decodes a codestream as `ojph_dec` does, to one plane per component
/// at the reconstructed size from SIZ.
pub fn decode_planes(j2c: &[u8]) -> Result<Vec<Plane>, String> {
    let mut cs = Codestream::new();
    cs.read_headers(j2c)
        .map_err(|e| format!("read_headers: {e}"))?;
    let siz = cs.access_siz();
    let nc = siz.get_num_components() as usize;
    let mut planes = Vec::new();
    for c in 0..nc as u32 {
        let (width, height) = (
            siz.get_recon_width(c) as usize,
            siz.get_recon_height(c) as usize,
        );
        planes.push(Plane {
            width,
            height,
            samples: vec![0; width * height],
        });
    }
    // As `ojph_dec`: interleaved for the colour transform, which planar
    // `pull` returns uninverted for components 1 and 2, unless the
    // heights differ, which needs planar.
    let same_heights = planes.iter().all(|p| p.height == planes[0].height);
    let planar = !(same_heights && cs.access_cod().is_employing_color_transform());
    cs.set_planar(planar);
    cs.create().map_err(|e| format!("create: {e}"))?;

    let mut row = vec![0usize; nc];
    let total: usize = planes.iter().map(|p| p.height).sum();
    for n in 0..total {
        let (c, line) = cs
            .pull()
            .map_err(|e| format!("pull {n}: {e}"))?
            .ok_or_else(|| format!("pull {n}: no more lines"))?;
        let c = c as usize;
        if c >= nc || row[c] >= planes[c].height {
            return Err(format!("pull {n}: unexpected line of component {c}"));
        }
        let w = planes[c].width;
        planes[c].samples[row[c] * w..][..w].copy_from_slice(&line.i32()[..w]);
        row[c] += 1;
    }
    Ok(planes)
}
