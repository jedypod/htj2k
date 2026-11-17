// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The image synthesiser of `oracle/ojph_enc.cpp`, and its command line.
//!
//! The test inputs are the images `ojph_enc` encoded for the reference
//! codestreams. They are made again here, value for value, from the same
//! command line: splitmix64 seeded with `--seed`; for each component the
//! constant value, then the gradient phase, then one draw per sample in
//! raster order (see `oracle/README.md`).
//!
//! This file is shared: the integration tests include it through
//! `tests/common/mod.rs`, and the crate's own unit tests through
//! `#[path]`, so it uses only `std`.

#![allow(dead_code)]

/// splitmix64 (Steele, Lea, Flood 2014), as in `ojph_enc.cpp`.
pub struct SplitMix64 {
    s: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { s: seed }
    }

    pub fn next(&mut self) -> u64 {
        self.s = self.s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A `--coc C:LEVELS:WxH` override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coc {
    pub comp: u32,
    pub levels: u32,
    pub bw: u32,
    pub bh: u32,
}

/// The options of one `ojph_enc` command line, with its defaults.
#[derive(Debug, Clone)]
pub struct EncArgs {
    pub out: String,
    pub width: u32,
    pub height: u32,
    pub num_comps: u32,
    pub depth: u32,
    pub is_signed: bool,
    pub reversible: bool,
    pub qstep: f32,
    pub rct: bool,
    pub levels: u32,
    pub bw: u32,
    pub bh: u32,
    pub precincts: Vec<(u32, u32)>,
    pub cocs: Vec<Coc>,
    pub prog: String,
    pub tile: Option<(u32, u32)>,
    pub tile_off: (u32, u32),
    pub img_off: (u32, u32),
    pub tlm: bool,
    pub tp_res: bool,
    pub tp_comp: bool,
    pub nlt_all: bool,
    pub nlt_comps: Vec<u32>,
    /// One `(dx, dy)` per component, padded with `(1, 1)`.
    pub sub: Vec<(u32, u32)>,
    /// `--planar` resolved with `ojph_enc`'s default rule.
    pub planar: bool,
    pub seed: u64,
    pub pattern: String,
}

/// `atoi`: leading digits, with an optional sign; 0 if there are none.
fn atoi(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut v: i64 = 0;
    for c in digits.bytes() {
        if !c.is_ascii_digit() {
            break;
        }
        v = v.wrapping_mul(10).wrapping_add((c - b'0') as i64);
    }
    if neg {
        -v
    } else {
        v
    }
}

/// `strtoull(s, NULL, 0)` for the decimal and `0x` forms.
fn strtoull0(s: &str) -> u64 {
    let s = s.trim();
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).unwrap_or(0)
    } else {
        s.parse().unwrap_or(0)
    }
}

fn parse_pair(s: &str, sep: char) -> Result<(u32, u32), String> {
    let (a, b) = s.split_once(sep).ok_or_else(|| format!("bad pair {s}"))?;
    let a = a.parse().map_err(|_| format!("bad pair {s}"))?;
    let b = b.parse().map_err(|_| format!("bad pair {s}"))?;
    Ok((a, b))
}

impl EncArgs {
    /// Parses the arguments after `ojph_enc`, as `ojph_enc.cpp` does.
    /// Errors are the conditions on which `ojph_enc` itself dies.
    pub fn parse(args: &[&str]) -> Result<EncArgs, String> {
        let mut a = EncArgs {
            out: String::new(),
            width: 64,
            height: 64,
            num_comps: 1,
            depth: 8,
            is_signed: false,
            reversible: true,
            qstep: 1.0 / 256.0,
            rct: false,
            levels: 5,
            bw: 64,
            bh: 64,
            precincts: Vec::new(),
            cocs: Vec::new(),
            prog: "LRCP".into(),
            tile: None,
            tile_off: (0, 0),
            img_off: (0, 0),
            tlm: false,
            tp_res: false,
            tp_comp: false,
            nlt_all: false,
            nlt_comps: Vec::new(),
            sub: Vec::new(),
            planar: false,
            seed: 1,
            pattern: "smooth".into(),
        };
        let mut planar: i64 = -1;
        let mut i = 0;
        while i < args.len() {
            let opt = args[i];
            let mut val = || -> Result<&str, String> {
                i += 1;
                args.get(i)
                    .copied()
                    .ok_or_else(|| format!("missing value for {opt}"))
            };
            match opt {
                "-o" => a.out = val()?.to_string(),
                "--raw-out" => {
                    val()?;
                }
                "--size" => (a.width, a.height) = parse_pair(val()?, 'x')?,
                "--comps" => a.num_comps = atoi(val()?) as u32,
                "--depth" => a.depth = atoi(val()?) as u32,
                "--signed" => a.is_signed = atoi(val()?) != 0,
                "--irrev" => a.reversible = false,
                // (float)atof(...)
                "--qstep" => a.qstep = val()?.parse::<f64>().unwrap_or(0.0) as f32,
                "--rct" => a.rct = atoi(val()?) != 0,
                "--levels" => a.levels = atoi(val()?) as u32,
                "--block" => (a.bw, a.bh) = parse_pair(val()?, 'x')?,
                "--precincts" => {
                    for p in val()?.split(',') {
                        a.precincts.push(parse_pair(p, 'x')?);
                    }
                }
                "--coc" => {
                    let v = val()?;
                    let (c, rest) = v.split_once(':').ok_or("bad --coc")?;
                    let (l, wh) = rest.split_once(':').ok_or("bad --coc")?;
                    let (bw, bh) = parse_pair(wh, 'x')?;
                    a.cocs.push(Coc {
                        comp: c.parse().map_err(|_| "bad --coc")?,
                        levels: l.parse().map_err(|_| "bad --coc")?,
                        bw,
                        bh,
                    });
                }
                "--prog" => a.prog = val()?.to_string(),
                "--tile" => a.tile = Some(parse_pair(val()?, 'x')?),
                "--tile-off" => a.tile_off = parse_pair(val()?, ',')?,
                "--img-off" => a.img_off = parse_pair(val()?, ',')?,
                "--tlm" => a.tlm = true,
                "--tileparts" => {
                    let t = val()?;
                    a.tp_res = t.contains('R');
                    a.tp_comp = t.contains('C');
                }
                "--nlt3" => a.nlt_all = true,
                "--nlt3-comps" => {
                    for p in val()?.split(',') {
                        a.nlt_comps.push(atoi(p) as u32);
                    }
                }
                "--sub" => {
                    for p in val()?.split(',') {
                        a.sub.push(parse_pair(p, 'x')?);
                    }
                }
                "--planar" => planar = atoi(val()?),
                "--seed" => a.seed = strtoull0(val()?),
                "--pattern" => a.pattern = val()?.to_string(),
                _ => return Err(format!("unknown option {opt}")),
            }
            i += 1;
        }
        if !(1..=32).contains(&a.depth) {
            return Err("depth must be 1..32".into());
        }
        if a.num_comps < 1 {
            return Err("need at least one component".into());
        }
        if !["smooth", "noise", "const", "zero"].contains(&a.pattern.as_str()) {
            return Err(format!("unknown pattern {}", a.pattern));
        }
        while (a.sub.len() as u32) < a.num_comps {
            a.sub.push((1, 1));
        }
        let subsampled = a.sub[..a.num_comps as usize]
            .iter()
            .any(|&(x, y)| x != 1 || y != 1);
        if planar < 0 {
            planar = if a.rct {
                0
            } else if subsampled || a.num_comps == 1 {
                1
            } else {
                0
            };
        }
        a.planar = planar != 0;
        Ok(a)
    }

    /// Parses a manifest command (`ojph_enc -o ...`).
    pub fn parse_command(cmd: &str) -> Result<EncArgs, String> {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        match words.split_first() {
            Some((&"ojph_enc", rest)) => EncArgs::parse(rest),
            _ => Err(format!("not an ojph_enc command: {cmd}")),
        }
    }

    /// `Xsiz`, `Ysiz`: the image offset plus the size.
    pub fn extent(&self) -> (u32, u32) {
        (self.img_off.0 + self.width, self.img_off.1 + self.height)
    }

    /// The width and height of each component.
    pub fn comp_sizes(&self) -> Vec<(u32, u32)> {
        let (xs, ys) = self.extent();
        let (ox, oy) = self.img_off;
        (0..self.num_comps as usize)
            .map(|c| {
                let (dx, dy) = self.sub[c];
                (
                    xs.div_ceil(dx) - ox.div_ceil(dx),
                    ys.div_ceil(dy) - oy.div_ceil(dy),
                )
            })
            .collect()
    }

    /// The legal sample range, `lo..=hi`.
    pub fn range(&self) -> (i64, i64) {
        if self.is_signed {
            (-(1i64 << (self.depth - 1)), (1i64 << (self.depth - 1)) - 1)
        } else {
            (0, (1i64 << self.depth) - 1)
        }
    }

    /// The image `ojph_enc` encodes: `img[c][y * width_c + x]`.
    pub fn synthesise(&self) -> Vec<Vec<i64>> {
        let (lo, hi) = self.range();
        let span = (hi - lo) as u64 + 1; // <= 2^32
        let mut rng = SplitMix64::new(self.seed);
        let mut img = Vec::with_capacity(self.num_comps as usize);
        for (cw, ch) in self.comp_sizes() {
            let mut v = Vec::with_capacity(cw as usize * ch as usize);
            let cval = lo + (rng.next() % span) as i64;
            let ph = rng.next();
            let fx = 0.5 + (ph & 0xFF) as f64 / 512.0;
            let fy = 0.5 + ((ph >> 8) & 0xFF) as f64 / 512.0;
            let namp = ((span >> 6) as i64).max(1);
            for y in 0..ch {
                for x in 0..cw {
                    let s = match self.pattern.as_str() {
                        "zero" => 0,
                        "const" => cval,
                        "noise" => lo + (rng.next() % span) as i64,
                        _ => {
                            let tx = if cw > 1 {
                                x as f64 / (cw - 1) as f64
                            } else {
                                0.0
                            };
                            let ty = if ch > 1 {
                                y as f64 / (ch - 1) as f64
                            } else {
                                0.0
                            };
                            let t = (fx * tx + fy * ty) / (fx + fy);
                            let base = lo + (t * (span - 1) as f64) as i64;
                            let n = (rng.next() % (2 * namp as u64 + 1)) as i64 - namp;
                            (base + n).clamp(lo, hi)
                        }
                    };
                    v.push(s);
                }
            }
            img.push(v);
        }
        img
    }

    /// `img` in the `.raw` layout: components in turn, each sample as the
    /// low 32 bits, little-endian.
    pub fn to_raw(img: &[Vec<i64>]) -> Vec<u8> {
        let mut out = Vec::new();
        for comp in img {
            for &v in comp {
                out.extend_from_slice(&(v as u32).to_le_bytes());
            }
        }
        out
    }
}

/// FNV-1a 64, as the manifests use.
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// One line of `tests/data/matrix/MANIFEST.txt`.
#[derive(Debug, Clone)]
pub struct MatrixCase {
    pub name: String,
    pub status: String,
    pub j2c_bytes: Option<usize>,
    pub raw_bytes: Option<usize>,
    pub raw_fnv: Option<u64>,
    pub command: String,
    pub error: Option<String>,
}

impl MatrixCase {
    pub fn is_lossless(&self) -> bool {
        self.status == "lossless"
    }
    pub fn is_lossy(&self) -> bool {
        self.status.starts_with("lossy")
    }
    pub fn is_mismatch(&self) -> bool {
        self.status.starts_with("MISMATCH")
    }
    pub fn is_refused(&self) -> bool {
        self.status == "REFUSED"
    }
}

/// Parses `MANIFEST.txt` of the matrix.
pub fn parse_matrix_manifest(text: &str) -> Vec<MatrixCase> {
    let mut cases = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.splitn(7, '|').map(str::trim).collect();
        assert!(f.len() >= 6, "bad manifest line {line}");
        cases.push(MatrixCase {
            name: f[0].to_string(),
            status: f[1].to_string(),
            j2c_bytes: f[2].parse().ok(),
            raw_bytes: f[3].parse().ok(),
            raw_fnv: u64::from_str_radix(f[4], 16).ok(),
            command: f[5].to_string(),
            error: f.get(6).map(|s| s.to_string()),
        });
    }
    cases
}
