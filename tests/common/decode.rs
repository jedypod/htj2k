// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Decoding a codestream as `oracle/ojph_dec` does, for comparison with
//! its `.raw` output.

use htj2k::codestream::Codestream;

/// Decodes a codestream the way `ojph_dec` does, to its `.raw` layout:
/// the components one after the other, each sample as 4 little-endian
/// bytes.
pub fn decode_to_raw(j2c: &[u8]) -> Result<Vec<u8>, String> {
    let mut cs = Codestream::new();
    cs.read_headers(j2c)
        .map_err(|e| format!("read_headers: {e}"))?;
    let siz = cs.access_siz();
    let nc = siz.get_num_components();
    let mut w = Vec::new();
    let mut h = Vec::new();
    for c in 0..nc {
        w.push(siz.get_recon_width(c) as usize);
        h.push(siz.get_recon_height(c) as usize);
    }
    // As `ojph_dec`: interleaved for the colour transform, which planar
    // `pull` returns uninverted for components 1 and 2, unless the
    // heights differ, which needs planar.
    let same_heights = h.iter().all(|&x| x == h[0]);
    let planar = !(same_heights && cs.access_cod().is_employing_color_transform());
    cs.set_planar(planar);
    cs.create().map_err(|e| format!("create: {e}"))?;

    let mut planes: Vec<Vec<u8>> = (0..nc as usize).map(|c| vec![0; w[c] * h[c] * 4]).collect();
    let mut row = vec![0usize; nc as usize];
    let total: usize = h.iter().sum();
    for n in 0..total {
        let (c, line) = cs
            .pull()
            .map_err(|e| format!("pull {n}: {e}"))?
            .ok_or_else(|| format!("pull {n}: no more lines"))?;
        let c = c as usize;
        if c >= nc as usize || row[c] >= h[c] {
            return Err(format!("pull {n}: unexpected line of component {c}"));
        }
        let dst = &mut planes[c][row[c] * w[c] * 4..][..w[c] * 4];
        for (bytes, v) in dst.chunks_exact_mut(4).zip(&line.i32()[..w[c]]) {
            bytes.copy_from_slice(&v.to_le_bytes());
        }
        row[c] += 1;
    }
    Ok(planes.concat())
}
