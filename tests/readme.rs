// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Runs the code examples of README.md.
//!
//! The README's `rust` blocks are copied into the two modules below.
//! `readme_blocks_match` requires every such block to appear in this
//! file verbatim, so the README cannot drift from the tested code.

#[rustfmt::skip]
mod decode_example {
// README: decoding
use htj2k::codestream::Codestream;

fn decode(data: &[u8]) -> htj2k::Result<Vec<Vec<i32>>> {
    let mut cs = Codestream::new();
    cs.set_memory_limit(256 << 20);
    cs.read_headers(data)?;
    cs.create()?;
    let mut planes = vec![Vec::new(); cs.access_siz().get_num_components() as usize];
    while let Some((comp, line)) = cs.pull()? {
        planes[comp as usize].extend_from_slice(line.i32());
    }
    Ok(planes)
}
// END

    pub fn run(data: &[u8]) -> htj2k::Result<Vec<Vec<i32>>> {
        decode(data)
    }
}

#[rustfmt::skip]
mod encode_example {
// README: encoding
use htj2k::codestream::Encoder;
use htj2k::types::Point;

fn encode(width: u32, height: u32, rgb: &[Vec<i32>; 3]) -> htj2k::Result<Vec<u8>> {
    let mut enc = Encoder::new();
    let siz = enc.access_siz_mut()?;
    siz.set_image_extent(Point::new(width, height));
    siz.set_num_components(3);
    for c in 0..3 {
        siz.set_component(c, Point::new(1, 1), 16, true)?;
    }
    let cod = enc.access_cod_mut()?;
    cod.set_reversible(true);
    cod.set_color_transform(true);
    cod.set_block_dims(128, 32)?;
    cod.set_num_decomposition(5)?;
    cod.set_progression_order("RPCL")?;
    enc.set_planar(false)?;

    let mut out = Vec::new();
    enc.write_headers(&mut out, &[])?;
    let mut rows = [0usize; 3];
    while let Some((comp, samples)) = enc.exchange()? {
        let c = comp as usize;
        let start = rows[c] * width as usize;
        samples.copy_from_slice(&rgb[c][start..start + width as usize]);
        rows[c] += 1;
    }
    enc.flush(&mut out)?;
    Ok(out)
}
// END

    pub fn run(width: u32, height: u32, rgb: &[Vec<i32>; 3]) -> htj2k::Result<Vec<u8>> {
        encode(width, height, rgb)
    }
}

/// The encoding example's output decodes, through the decoding example,
/// to exactly its input: the coding is reversible.
#[test]
fn readme_examples_round_trip() {
    let (w, h) = (301u32, 45u32);
    let rgb: [Vec<i32>; 3] = std::array::from_fn(|c| {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as i32, (i / w) as i32);
                ((x * 97 + y * 211 + c as i32 * 7919) % 65536) - 32768
            })
            .collect()
    });
    let data = encode_example::run(w, h, &rgb).expect("encode");
    let planes = decode_example::run(&data).expect("decode");
    assert_eq!(planes.len(), 3);
    for c in 0..3 {
        assert_eq!(planes[c], rgb[c], "component {c}");
    }
}

/// Every `rust` block in README.md appears verbatim in this file.
#[test]
fn readme_blocks_match() {
    let readme = include_str!("../README.md");
    let this = include_str!("readme.rs");
    let mut blocks = Vec::new();
    let mut rest = readme;
    while let Some(start) = rest.find("```rust\n") {
        let body = &rest[start + "```rust\n".len()..];
        let end = body.find("```").expect("unterminated rust block");
        blocks.push(&body[..end]);
        rest = &body[end + 3..];
    }
    assert_eq!(blocks.len(), 2, "README.md should have two rust blocks");
    for block in blocks {
        assert!(
            this.contains(block),
            "README.md block not in tests/readme.rs:\n{block}"
        );
    }
}
