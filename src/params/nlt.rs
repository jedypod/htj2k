// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The NLT marker segment, non-linearity point transformation
//! (`param_nlt`, `nlt_rec`).
//!
//! C++ `nlt_rec` carves its tables out of one byte buffer (`points_store`)
//! and lets some of them be indexed at -1 or -2. Here each table is its
//! own `Vec`, and an index `i` of the C++ pointer is
//! `i + <TABLE>_OFFSET` in the `Vec`.

use super::cod::ParamCod;
use super::markers::{jp2k_marker, log2_floor, put_u16, put_u32, put_u8};
use super::siz::ParamSiz;
use super::spare::Spare;
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::Result;

/// `ojph::param_nlt::nonlinearity` values (`Tnlt`), as `u8` because the
/// field holds whatever was read.
pub mod nonlinearity {
    /// supported
    pub const OJPH_NLT_NO_NLT: u8 = 0;
    /// not supported
    pub const OJPH_NLT_GAMMA_STYLE_NLT: u8 = 1;
    /// supported
    pub const OJPH_NLT_LUT_STYLE_NLT: u8 = 2;
    /// supported
    pub const OJPH_NLT_BINARY_COMPLEMENT_NLT: u8 = 3;
    /// experimental
    pub const OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT: u8 = 4;
    /// This is used internally and is not part of the standard
    pub const OJPH_NLT_UNDEFINED: u8 = 255;
}

/// `ojph::param_nlt::nonlinearity` as an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NltType {
    NoNlt = 0,
    GammaStyleNlt = 1,
    LutStyleNlt = 2,
    BinaryComplementNlt = 3,
    BinaryComplementPlusLut = 4,
    Undefined = 255,
}

impl NltType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::NoNlt),
            1 => Some(Self::GammaStyleNlt),
            2 => Some(Self::LutStyleNlt),
            3 => Some(Self::BinaryComplementNlt),
            4 => Some(Self::BinaryComplementPlusLut),
            255 => Some(Self::Undefined),
            _ => None,
        }
    }
}

/// `ojph::param_nlt::special_comp_num::ALL_COMPS`
pub const ALL_COMPS: u16 = 65535;

/// `nlt_rec`: for easy exchange and processing of NLT types 2 and 4.
#[derive(Debug, Clone, PartialEq)]
pub struct NltRec {
    /// Decoded image component bit depth parameter
    pub bdnlt: u8,
    /// Type of non-linearity
    pub tnlt: u8,
    /// Dmin
    pub d_min: u32,
    /// Dmax
    pub d_max: u32,
    /// Precision of points in bits
    pub pt_val: u32,
    /// number of points in LUT points from 2 to 8192
    pub num_points: u32,
    /// The marker points, one value per point whatever `bytes_per_point`
    /// (C++: a `void*` to `ui8`/`ui16`/`ui32` values).
    pub marker_points: Vec<u32>,
    /// number of bytes per point, derived from pt_val
    pub bytes_per_point: u8,

    // members for decoding
    /// LUT points for decoding, `num_points + 2` long; C++ `dec_points[i]`
    /// is `dec_points[i + DEC_POINTS_OFFSET]`.
    pub dec_points: Vec<f32>,
    /// float d_min
    pub fd_min: f32,
    /// float d_max
    pub fd_max: f32,
    /// delta is (dmax-dmin) / (num_points - 1)
    pub delta: f32,
    pub inv_delta: f32,
    /// multiplier to convert to final integer
    pub multiplier: f32,

    // members for encoding -- we also use some from decoding
    /// # of points for encoding (larger than decoding)
    pub enc_num_points: u32,
    /// float first LUT point
    pub ft_min: f32,
    /// float last LUT point
    pub ft_max: f32,
    /// use approx encoding non-linearity
    pub approx_encoding_nlt: bool,
    /// approx: LUT points for encoding, `enc_num_points + 4` long; C++
    /// index `i` is `i + ENC_POINTS_OFFSET`.
    pub approx_enc_points: Vec<f32>,
    /// precise: max needed steps to find LUT data node
    pub precise_max_steps: u32,
    /// precise: LUT indices into decoder LUT, `enc_num_points + 4` long;
    /// C++ index `i` is `i + ENC_POINTS_OFFSET`.
    pub precise_enc_dec_indices: Vec<u16>,
    /// precise: decoder LUT in float, `num_points + 4` long; C++ index `i`
    /// is `i + ENC_POINTS_OFFSET`.
    pub precise_dec_lut: Vec<f32>,
}

impl Default for NltRec {
    fn default() -> Self {
        Self::new()
    }
}

impl NltRec {
    /// Offset of C++ index 0 in `dec_points`.
    pub const DEC_POINTS_OFFSET: usize = 1;
    /// Offset of C++ index 0 in `approx_enc_points`,
    /// `precise_enc_dec_indices` and `precise_dec_lut`.
    pub const ENC_POINTS_OFFSET: usize = 2;

    pub fn new() -> Self {
        NltRec {
            bdnlt: 0,
            tnlt: nonlinearity::OJPH_NLT_UNDEFINED,
            d_min: 0,
            d_max: 0,
            pt_val: 0,
            num_points: 0,
            marker_points: Vec::new(),
            bytes_per_point: 0,
            dec_points: Vec::new(),
            fd_min: 0.0,
            fd_max: 0.0,
            delta: 0.0,
            inv_delta: 0.0,
            multiplier: 0.0,
            enc_num_points: 0,
            ft_min: 0.0,
            ft_max: 0.0,
            approx_encoding_nlt: false,
            approx_enc_points: Vec::new(),
            precise_max_steps: 0,
            precise_enc_dec_indices: Vec::new(),
            precise_dec_lut: Vec::new(),
        }
    }

    /// Gives `self` the state `new` gives, but keeps the storage of its
    /// tables.
    pub fn init(&mut self) {
        let mut tables = [
            std::mem::take(&mut self.dec_points),
            std::mem::take(&mut self.approx_enc_points),
            std::mem::take(&mut self.precise_dec_lut),
        ];
        let mut marker_points = std::mem::take(&mut self.marker_points);
        let mut indices = std::mem::take(&mut self.precise_enc_dec_indices);
        for t in &mut tables {
            t.clear();
        }
        marker_points.clear();
        indices.clear();
        *self = Self::new();
        let [dec_points, approx_enc_points, precise_dec_lut] = tables;
        self.dec_points = dec_points;
        self.approx_enc_points = approx_enc_points;
        self.precise_dec_lut = precise_dec_lut;
        self.marker_points = marker_points;
        self.precise_enc_dec_indices = indices;
    }

    /// `static ui8 get_bpp(ui32 t)`
    pub fn get_bpp_for(t: u32) -> u8 {
        if t <= 8 {
            1
        } else if t <= 16 {
            2
        } else {
            4
        }
    }

    pub fn get_type(&self) -> u8 {
        self.tnlt
    }
    pub fn get_bit_depth(&self) -> u8 {
        (self.bdnlt & 0x7F) + 1
    }
    pub fn get_bpp(&self) -> u8 {
        Self::get_bpp_for(self.pt_val)
    }
    pub fn is_signed(&self) -> bool {
        (self.bdnlt & 0x80) != 0
    }
    pub fn cal_marker_points_size(&self) -> u32 {
        self.num_points.wrapping_mul(self.get_bpp() as u32)
    }

    /// `1ull << get_bit_depth()` as f64; bit depths here are at most 128,
    /// and C++ only ever sees at most 38.
    fn pow2_bit_depth(&self) -> f64 {
        2f64.powi(self.get_bit_depth() as i32)
    }

    /// Sizes the decoding tables (C++ `cal_store_size_for_decoding` and
    /// `assign_pointers_for_decoding`).
    fn alloc_for_decoding(&mut self) {
        let n = self.num_points as usize;
        self.dec_points.clear();
        self.dec_points.resize(n + 2, 0.0);
        self.approx_enc_points.clear();
        self.marker_points.clear();
        self.marker_points.resize(n, 0);
    }

    /// Sizes the encoding tables (C++ `cal_store_size_for_encoding` and
    /// `assign_pointers_for_encoding`). Errors (0x000501A9, as for a failed
    /// allocation) where the C++ store size would overflow 32 bits.
    fn alloc_for_encoding(&mut self, enc_num_points: u32) -> Result<()> {
        self.enc_num_points = enc_num_points;
        let np = self.num_points as u64;
        let enp = enc_num_points as u64;
        let bpp = self.get_bpp() as u64;
        let len = if self.approx_encoding_nlt {
            (enp + 4) * 4 + np * bpp
        } else {
            (enp + 4) * 2 + (np + 4) * 4 + np * bpp
        };
        if len > u32::MAX as u64 {
            return ojph_error!(0x000501A9, "Failed to allocated memory");
        }
        self.dec_points.clear();
        if self.approx_encoding_nlt {
            self.approx_enc_points.clear();
            self.approx_enc_points.resize(enp as usize + 4, 0.0);
            self.precise_enc_dec_indices.clear();
            self.precise_dec_lut.clear();
        } else {
            self.approx_enc_points.clear();
            self.precise_enc_dec_indices.clear();
            self.precise_enc_dec_indices.resize(enp as usize + 4, 0);
            self.precise_dec_lut.clear();
            self.precise_dec_lut.resize(np as usize + 4, 0.0);
        }
        self.marker_points.clear();
        self.marker_points.resize(np as usize, 0);
        Ok(())
    }

    pub fn prepare_for_decoding(&mut self) {
        let d = 1.0f64 / ((1u64 << 32) - 1) as f64;
        self.fd_min = (self.d_min as f64 * d) as f32;
        self.fd_max = (self.d_max as f64 * d) as f32;
        let np = self.num_points;
        self.delta = (self.fd_max - self.fd_min) / np.wrapping_sub(1) as f32;
        self.inv_delta = np.wrapping_sub(1) as f32 / (self.fd_max - self.fd_min);
        self.multiplier = self.pow2_bit_depth() as f32;
        let divider = (1.0f64 / self.pow2_bit_depth()) as f32;
        if self.bytes_per_point == 1 || self.bytes_per_point == 2 || self.bytes_per_point == 4 {
            let n = np as usize;
            if n == 0 || self.dec_points.len() < n + 2 || self.marker_points.len() < n {
                return;
            }
            let o = Self::DEC_POINTS_OFFSET;
            for i in 0..n {
                self.dec_points[o + i] = self.marker_points[i] as f32 * divider;
            }
            self.dec_points[0] = self.dec_points[o];
            self.dec_points[o + n] = self.dec_points[o + n - 1];
        }
        // else: C++ assert(0)
    }

    fn prepare_for_encoding_approx(&mut self) {
        let d = 1.0f64 / ((1u64 << 32) - 1) as f64;
        self.fd_min = (self.d_min as f64 * d) as f32;
        self.fd_max = (self.d_max as f64 * d) as f32;

        let enc = self.enc_num_points as usize;
        let o = Self::ENC_POINTS_OFFSET;
        // enc_points holds positions in the domain of the LUT, whose first
        // and last entries are fd_min and fd_max, not LUT values
        {
            let a = &mut self.approx_enc_points;
            a[o - 2] = self.fd_min;
            a[o - 1] = self.fd_min;
            a[o] = self.fd_min;
            a[o + enc - 1] = self.fd_max;
            a[o + enc] = self.fd_max;
            a[o + enc + 1] = self.fd_max;
        }

        // create lookup table for encoding
        let mul = ((1u64 << self.pt_val) - 1) as f32;
        let div = 1.0f32 / mul;
        if self.bytes_per_point == 1 || self.bytes_per_point == 2 || self.bytes_per_point == 4 {
            // prepare_for_encoding_approx_for_type<T>
            let p = &self.marker_points;
            let num_points = self.num_points;
            let enc_num_points = self.enc_num_points;
            let fd_min = self.fd_min;
            let fd_max = self.fd_max;
            let ft_min = p[0] as f32 * div;
            let ft_max = p[num_points as usize - 1] as f32 * div;
            let delta = (ft_max - ft_min) / (enc_num_points - 1) as f32;
            let inv_delta = (enc_num_points - 1) as f32 / (ft_max - ft_min);

            let mut k = 0u32;
            let mut y_k = p[k as usize] as f32 * div;
            let mut y_kp1 = p[k as usize + 1] as f32 * div;
            let dt = (fd_max - fd_min) / (num_points - 1) as f32;
            let mut d_k = fd_min;
            let mut d_kp1 = fd_min + dt;
            for i in 1..enc_num_points.saturating_sub(1) {
                let z = ft_min + i as f32 * delta;
                while k + 1 < num_points - 1 && z >= y_kp1 {
                    k += 1;
                    d_k = d_kp1;
                    d_kp1 = fd_min + (k + 1) as f32 * dt;
                    y_k = y_kp1;
                    y_kp1 = p[k as usize + 1] as f32 * div;
                }
                self.approx_enc_points[o + i as usize] = d_k + (z - y_k) * dt / (y_kp1 - y_k);
            }
            self.ft_min = ft_min;
            self.ft_max = ft_max;
            self.delta = delta;
            self.inv_delta = inv_delta;
        }

        self.multiplier = (1.0f64 / self.pow2_bit_depth()) as f32;
    }

    fn prepare_for_encoding_precise(&mut self) {
        let d = 1.0f64 / ((1u64 << 32) - 1) as f64;
        self.fd_min = (self.d_min as f64 * d) as f32;
        self.fd_max = (self.d_max as f64 * d) as f32;

        let mul = ((1u64 << self.pt_val) - 1) as f32;
        let div = 1.0f32 / mul;

        let n = self.num_points as usize;
        let o = Self::ENC_POINTS_OFFSET;
        // convert decoder lookup table to floats
        if self.bytes_per_point == 1 || self.bytes_per_point == 2 || self.bytes_per_point == 4 {
            for i in 0..n {
                self.precise_dec_lut[o + i] = self.marker_points[i] as f32 * div;
            }
            self.ft_min = self.precise_dec_lut[o];
            self.ft_max = self.precise_dec_lut[o + n - 1];
        }
        {
            let l = &mut self.precise_dec_lut;
            l[o - 2] = l[o];
            l[o - 1] = l[o];
            l[o + n] = l[o + n - 1];
            l[o + n + 1] = l[o + n - 1];
        }
        self.delta = (self.fd_max - self.fd_min) / (self.num_points - 1) as f32;
        self.inv_delta = (self.enc_num_points - 1) as f32 / (self.ft_max - self.ft_min);

        self.multiplier = (1.0f64 / self.pow2_bit_depth()) as f32;

        // find index into decoder lookup table
        let td = (self.ft_max - self.ft_min) / (self.enc_num_points - 1) as f32;
        self.precise_max_steps = 0;
        let mut k: u16 = 0;
        let mut old_k: u16 = 0;
        for i in 0..self.enc_num_points as usize {
            let z = self.ft_min + i as f32 * td;
            while (k as u32) < self.num_points - 1 && z >= self.precise_dec_lut[o + k as usize + 1]
            {
                k += 1;
            }
            self.precise_enc_dec_indices[o + i] = k;
            self.precise_max_steps = self.precise_max_steps.max((k - old_k) as u32);
            old_k = k;
        }
    }

    pub fn prepare_for_encoding(&mut self) {
        if self.approx_encoding_nlt {
            self.prepare_for_encoding_approx();
        } else {
            self.prepare_for_encoding_precise();
        }
    }
}

/// `param_nlt`. The main object (component `ALL_COMPS`) owns the objects
/// for individual components (C++: the `next` list) in `others`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamNlt {
    /// length of the marker segment excluding marker
    pub(crate) lnlt: u16,
    /// Component involved in the transformation
    pub(crate) cnlt: u16,
    /// NLT properties
    pub(crate) rec: NltRec,
    /// true if this object is used
    pub(crate) enabled: bool,
    /// per-component objects (only used in the main object)
    pub(crate) others: Vec<ParamNlt>,
    /// Per-component objects removed by `restart`, whose storage the next
    /// ones reuse (only used in the main object).
    pub(crate) spare: Spare<ParamNlt>,
}

impl Default for ParamNlt {
    fn default() -> Self {
        Self::new()
    }
}

/// Where `get_nlt_object` found an object.
#[derive(Clone, Copy)]
enum Loc {
    Top,
    Other(usize),
}

impl ParamNlt {
    pub fn new() -> Self {
        ParamNlt {
            lnlt: 0,
            cnlt: ALL_COMPS,
            rec: NltRec::new(),
            enabled: false,
            others: Vec::new(),
            spare: Spare::new(),
        }
    }

    fn init(&mut self) {
        self.lnlt = 0;
        self.cnlt = ALL_COMPS;
        self.rec.init();
        self.enabled = false;
    }

    /// Removes the per-component objects and returns every parameter to
    /// its default, keeping the storage of the objects and their tables
    /// for the next ones.
    pub fn restart(&mut self) {
        self.spare.put_all(&mut self.others);
        self.init();
    }

    pub fn get_comp_num(&self) -> u16 {
        self.cnlt
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn rec(&self) -> &NltRec {
        &self.rec
    }
    /// The per-component objects.
    pub fn others(&self) -> &[ParamNlt] {
        &self.others
    }

    fn locate(&self, comp_num: u32) -> Option<Loc> {
        // top level must be ALL_COMPS; cannot call this functions from
        // non-top nlt
        if self.cnlt as u32 == comp_num {
            return Some(Loc::Top);
        }
        self.others
            .iter()
            .position(|p| p.cnlt as u32 == comp_num)
            .map(Loc::Other)
    }

    fn at_mut(&mut self, loc: Loc) -> &mut ParamNlt {
        match loc {
            Loc::Top => self,
            Loc::Other(i) => &mut self.others[i],
        }
    }

    fn get_nlt_object(&self, comp_num: u32) -> Option<&ParamNlt> {
        match self.locate(comp_num)? {
            Loc::Top => Some(self),
            Loc::Other(i) => Some(&self.others[i]),
        }
    }

    fn get_or_add(&mut self, comp_num: u32) -> &mut ParamNlt {
        match self.locate(comp_num) {
            Some(loc) => self.at_mut(loc),
            None => self.add_object(comp_num),
        }
    }

    fn add_object(&mut self, comp_num: u32) -> &mut ParamNlt {
        // C++ asserts comp_num != ALL_COMPS and that this is the top level
        let mut p = match self.spare.take() {
            Some(mut p) => {
                p.restart();
                p
            }
            None => ParamNlt::new(),
        };
        p.cnlt = comp_num as u16;
        self.others.push(p);
        self.others.last_mut().unwrap()
    }

    fn is_any_enabled(&self) -> bool {
        // check if any field is enabled
        self.enabled || self.others.iter().any(|p| p.enabled)
    }

    fn trim_non_existing_components(&mut self, num_comps: u32) {
        for p in &mut self.others {
            if p.enabled && p.cnlt as u32 >= num_comps {
                p.enabled = false;
                // OJPH_INFO 0x00050161: NLT segment for non-existing
                // component removed
            }
        }
    }

    /// Returns the index of the first component that uses a LUT
    /// nonlinearity type (types 2 or 4) with a reversible wavelet, which is
    /// not supported yet, or `None` (C++: -1) when there is no such
    /// component.
    pub fn find_unsupported_nlt(&self, siz: &ParamSiz, cod: &ParamCod) -> Option<u32> {
        let num_comps = siz.get_num_components();
        for c in 0..num_comps {
            let rec = match self.get_nlt_rec(c) {
                Some(r) => r,
                None => continue,
            };
            let t = rec.get_type();
            // the LUT style nonlinearities are implemented for the
            // irreversible (9/7) wavelet only
            if (t == nonlinearity::OJPH_NLT_LUT_STYLE_NLT
                || t == nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT)
                && cod.get_coc(c).is_reversible()
            {
                return Some(c);
            }
        }
        None
    }

    pub fn check_validity(&mut self, siz: &mut ParamSiz, cod: &ParamCod) -> Result<()> {
        if !self.is_any_enabled() {
            return Ok(());
        }

        // the reversible wavelet cannot be used with a LUT style
        // nonlinearity
        if let Some(comp) = self.find_unsupported_nlt(siz, cod) {
            return ojph_error!(
                0x000501B1,
                "The LUT style nonlinearities (type 2, LUT style, and type 4, binary complement followed by a LUT) are supported with the irreversible (9/7) wavelet only; component {} of this codestream is coded with the reversible (5/3) wavelet. Use the irreversible wavelet for that component, use the binary complement nonlinearity (type 3), or do not use a nonlinearity with it.",
                comp
            );
        }

        if self.enabled && self.rec.tnlt == nonlinearity::OJPH_NLT_UNDEFINED {
            self.enabled = false;
        }

        let num_comps = siz.get_num_components();
        if self.enabled && self.rec.tnlt == nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT {
            let mut all_same = true;

            // first stage; find out if all components captured by the
            // default entry (ALL_COMPS) has the same bit_depth/signedness,
            // while doing this, set the BDnlt for components not captured
            // by the default entry (ALL_COMPS)
            let mut bit_depth = 0u32; // unknown yet
            let mut is_signed = false; // unknown yet
            for c in 0..num_comps {
                let loc = self.locate(c);
                let enabled = match loc {
                    Some(l) => self.at_mut(l).enabled,
                    None => false,
                };
                if !enabled {
                    // captured by ALL_COMPS
                    if bit_depth != 0 {
                        // we have seen an undefined component previously
                        all_same = all_same && (bit_depth == siz.get_bit_depth(c));
                        all_same = all_same && (is_signed == siz.is_signed(c));
                    } else {
                        // this is the first component which has not type 3
                        // nlt definition
                        bit_depth = siz.get_bit_depth(c);
                        is_signed = siz.is_signed(c);
                    }
                } else if let Some(l) = loc {
                    // can be type 0 or type 3
                    let p = self.at_mut(l);
                    p.rec.bdnlt = siz.get_bit_depth(c).wrapping_sub(1) as u8;
                    p.rec.bdnlt |= if siz.is_signed(c) { 0x80 } else { 0 };
                }
            }

            if all_same && bit_depth != 0 {
                // all the same, and some components are captured by
                // ALL_COMPS
                self.rec.bdnlt = (bit_depth - 1) as u8;
                self.rec.bdnlt |= if is_signed { 0x80 } else { 0 };
            } else if !all_same {
                // have different settings or no component is captured by
                // ALL_COMPS
                self.enabled = false;
                for c in 0..num_comps {
                    let loc = self.locate(c);
                    let enabled = match loc {
                        Some(l) => self.at_mut(l).enabled,
                        None => false,
                    };
                    if !enabled {
                        // captured by ALL_COMPS
                        let p = match loc {
                            Some(l) => self.at_mut(l),
                            None => self.add_object(c),
                        };
                        p.enabled = true;
                        p.rec.tnlt = nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT;
                        p.rec.bdnlt = siz.get_bit_depth(c).wrapping_sub(1) as u8;
                        p.rec.bdnlt |= if siz.is_signed(c) { 0x80 } else { 0 };
                    }
                }
            }
        } else {
            // fill NLT segment markers with correct information
            for c in 0..num_comps {
                if let Some(l) = self.locate(c) {
                    let p = self.at_mut(l);
                    if p.enabled {
                        // can be type 0 or type 3
                        p.rec.bdnlt = siz.get_bit_depth(c).wrapping_sub(1) as u8;
                        p.rec.bdnlt |= if siz.is_signed(c) { 0x80 } else { 0 };
                    }
                }
            }
        }

        self.trim_non_existing_components(num_comps);

        if self.is_any_enabled() {
            siz.set_Rsiz_flag(ParamSiz::RSIZ_EXT_FLAG | ParamSiz::RSIZ_NLT_FLAG);
        }
        Ok(())
    }

    /// `set_nonlinear_transform(ui32 comp_num, ui8 nl_type)`
    pub fn set_nonlinear_transform(&mut self, comp_num: u32, nl_type: u8) -> Result<()> {
        if nl_type != nonlinearity::OJPH_NLT_NO_NLT
            && nl_type != nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT
        {
            return ojph_error!(
                0x00050171,
                "Nonliearities other than type 0 (No Nonlinearity) or type 3 (Binary Binary Complement to Sign Magnitude Conversion) are not supported in this function call"
            );
        }
        let p = self.get_or_add(comp_num);
        p.lnlt = 6;
        p.rec.tnlt = nl_type;
        p.enabled = true;
        Ok(())
    }

    /// The LUT form of `set_nonlinear_transform`. C++ takes `ui16
    /// num_points, void* points` where `points` is an array of `ui8`,
    /// `ui16` or `ui32` according to `pt_val`; here `points` holds the
    /// values (at least `num_points` of them), each truncated to that
    /// width as reading the C++ array would. A `points` shorter than
    /// `num_points` is an error (0x000501A5) where C++ would read past it.
    #[allow(clippy::too_many_arguments)]
    pub fn set_nonlinear_transform_lut(
        &mut self,
        comp_num: u32,
        decoded_bit_depth: u8,
        decoded_signedness: bool,
        d_min: u32,
        d_max: u32,
        pt_val: u8,
        num_points: u16,
        points: &[u32],
        nl_type: u8,
        approx_encoding_nlt: bool,
        num_enc_points: u32,
    ) -> Result<()> {
        if nl_type != nonlinearity::OJPH_NLT_LUT_STYLE_NLT
            && nl_type != nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT
        {
            return ojph_error!(
                0x000501A1,
                "Nonliearities other than type 1 (LUT Nonlinearity) or type 4 (Binary Binary Complement to Sign Magnitude Conversion followed by LUT Nonlinearity) are not supported in this function call"
            );
        }
        if pt_val == 0 || pt_val > 32 {
            return ojph_error!(
                0x000501A2,
                "pt_val must be larger than 0 and smaller than or equal to 32; you provided, {}",
                pt_val
            );
        }
        if decoded_bit_depth == 0 || decoded_bit_depth > 38 {
            return ojph_error!(
                0x000501A3,
                "decoded_bit_depth must be larger than 0 and smaller than or equal to 38; you provided, {}",
                decoded_bit_depth
            );
        }
        if d_min >= d_max {
            return ojph_error!(
                0x000501A4,
                "d_max must be larger than d_min; you provided {} and {}",
                d_min,
                d_max
            );
        }
        if !(2..=8192).contains(&num_points) || points.len() < num_points as usize {
            return ojph_error!(
                0x000501A5,
                "The number of points must be larger or equal to 2 and smaller or equal to 8192 -- the code will convert this number to a number in the range 1 to 8191 for the NLT marker segement; you provided {}",
                num_points
            );
        }
        if num_enc_points == 1 {
            return ojph_error!(0x000501AA, "The num_enc_points cannot be 1");
        }

        let p = self.get_or_add(comp_num);

        p.lnlt = (17u32 + num_points as u32 * NltRec::get_bpp_for(pt_val as u32) as u32) as u16;
        p.rec.bdnlt = (decoded_bit_depth - 1) | if decoded_signedness { 0x80 } else { 0 };
        p.rec.tnlt = nl_type;

        p.rec.d_min = d_min;
        p.rec.d_max = d_max;
        p.rec.pt_val = pt_val as u32;
        p.rec.num_points = num_points as u32;
        p.rec.bytes_per_point = NltRec::get_bpp_for(pt_val as u32);
        p.rec.approx_encoding_nlt = approx_encoding_nlt;

        let bpp = p.rec.bytes_per_point;
        let pts = &points[..num_points as usize];
        // each value truncated to the width of a point
        let pt = |k: usize| match bpp {
            1 => pts[k] as u8 as u32,
            2 => pts[k] as u16 as u32,
            _ => pts[k],
        };

        // Check that the LUT has increasing entries or has almost flat
        // segments
        let code = match bpp {
            1 => 0x000501A6,
            2 => 0x000501A7,
            _ => 0x000501A8,
        };
        let mut smallest_gap = u32::MAX;
        for k in 0..(num_points as usize - 1) {
            if pt(k + 1) > pt(k) {
                smallest_gap = smallest_gap.min(pt(k + 1) - pt(k));
            } else {
                return ojph_error!(
                    code,
                    "The LUT must have increasing sequence of numbers; here we have at LUT[{}+1] <= LUT[{}], with values {} and {}.",
                    k,
                    k,
                    pt(k + 1),
                    pt(k)
                );
            }
        }
        let v_min = pt(0);
        let v_max = pt(num_points as usize - 1);

        // find ceil of the ratio to a power of 2
        let mut ienc_pnts = num_enc_points;
        if num_enc_points == 0 {
            let enc_pnts = ((v_max - v_min) as f32 / smallest_gap as f32).ceil();
            if enc_pnts > 8192.0 {
                ienc_pnts = 8192;
                // OJPH_WARN 0x000501A1: encoding LUT limited to 8192 entries
            } else {
                ienc_pnts = enc_pnts as u32;
                ienc_pnts = log2_floor(ienc_pnts).wrapping_add(1); // 32 - clz
                ienc_pnts = 1u32 << ienc_pnts;
                ienc_pnts = ienc_pnts.max(2);
            }
        }

        p.rec.alloc_for_encoding(ienc_pnts)?;
        for (k, m) in p.rec.marker_points.iter_mut().enumerate() {
            *m = pt(k);
        }
        p.rec.prepare_for_encoding();

        p.enabled = true;
        Ok(())
    }

    /// Returns `(bit_depth, is_signed, nl_type)`, or `None` (C++: false).
    pub fn get_nonlinear_transform(&self, comp_num: u32) -> Option<(u8, bool, u8)> {
        // top level must be ALL_COMPS
        let p = match self.get_nlt_object(comp_num) {
            Some(p) if p.enabled => p,
            _ => self,
        };
        if p.enabled {
            let mut bit_depth = (p.rec.bdnlt & 0x7F) + 1;
            bit_depth = if bit_depth <= 38 { bit_depth } else { 38 };
            let is_signed = (p.rec.bdnlt & 0x80) == 0x80;
            return Some((bit_depth, is_signed, p.rec.tnlt));
        }
        None
    }

    pub fn write(&self, file: &mut Vec<u8>) -> bool {
        if !self.is_any_enabled() {
            return true;
        }
        for p in std::iter::once(self).chain(self.others.iter()) {
            if !p.enabled {
                continue;
            }
            put_u16(file, jp2k_marker::NLT);
            put_u16(file, p.lnlt);
            put_u16(file, p.cnlt);
            put_u8(file, p.rec.bdnlt);
            put_u8(file, p.rec.tnlt);
            if p.rec.tnlt == nonlinearity::OJPH_NLT_LUT_STYLE_NLT
                || p.rec.tnlt == nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT
            {
                put_u16(file, p.rec.num_points.wrapping_sub(1) as u16);
                put_u32(file, p.rec.d_min);
                put_u32(file, p.rec.d_max);
                put_u8(file, p.rec.pt_val as u8);
                let n = (p.rec.num_points as usize).min(p.rec.marker_points.len());
                for &v in &p.rec.marker_points[..n] {
                    match p.rec.bytes_per_point {
                        1 => put_u8(file, v as u8),
                        2 => put_u16(file, v as u16),
                        _ => put_u32(file, v),
                    }
                }
            }
        }
        true
    }

    /// C++ returns `bool` (always true, or throws).
    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        let buf2_len = file.read_u16();
        let buf2_comp = file.read_u16();
        let buf1_bdnlt = file.read_u8();
        let buf1_tnlt = file.read_u8();
        let (length, comp, bdnlt, tnlt) = match (buf2_len, buf2_comp, buf1_bdnlt, buf1_tnlt) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return ojph_error!(0x000501E1, "Error reading NLT marker, Location 1"),
        };

        if tnlt != nonlinearity::OJPH_NLT_NO_NLT
            && tnlt != nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT
            && tnlt != nonlinearity::OJPH_NLT_LUT_STYLE_NLT
            && tnlt != nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT
        {
            // the C++ format string has no conversion for the value
            return ojph_error!(
                0x00050141,
                "Unsupported nonlinearity type in an NLT marker segment, with a value of"
            );
        }

        if (tnlt == nonlinearity::OJPH_NLT_NO_NLT
            || tnlt == nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_NLT)
            && length != 6
        {
            return ojph_error!(
                0x00050142,
                "Somethins is wrong with this NLT marker segment -- it has an incorrect length of {}",
                length
            );
        }

        if comp > 16383 && comp != ALL_COMPS {
            return ojph_error!(
                0x00050143,
                "Something is wrong with this NLT marker segment -- it has an incorrect component index of {}",
                comp
            );
        }

        let p = self.get_or_add(comp as u32);
        p.enabled = true;
        p.cnlt = comp;
        p.rec.bdnlt = bdnlt;
        if (bdnlt & 0x7F) as u32 + 1 > 38 {
            return ojph_error!(
                0x00050144,
                "The NLT marker segment has an invalid bit depth of {}",
                (bdnlt & 0x7F) as u32 + 1
            );
        }
        p.rec.tnlt = tnlt;

        if tnlt == nonlinearity::OJPH_NLT_LUT_STYLE_NLT
            || tnlt == nonlinearity::OJPH_NLT_BINARY_COMPLEMENT_PLUS_LUT
        {
            let a = file.read_u16();
            let b = file.read_u32();
            let c = file.read_u32();
            let d = file.read_u8();
            let (num_points, d_min, d_max, pt_val) = match (a, b, c, d) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => return ojph_error!(0x000501E2, "Error reading NLT marker, Location 2"),
            };

            if !(1..=8191).contains(&num_points) {
                // the C++ format string has no conversion for the value
                return ojph_error!(
                    0x00050145,
                    "The NLT marker segment has an invalid number of points field"
                );
            }
            if d_min >= d_max {
                return ojph_error!(
                    0x00050146,
                    "In an NLT marker segment, Dmin must smaller than Dmax; here we have {} and {}",
                    d_min,
                    d_max
                );
            }
            if pt_val == 0 || pt_val > 32 {
                return ojph_error!(
                    0x00050147,
                    "In an NLT marker segment, the value of pt_val must be larger than 0 and smaller or equal to 32; here, we have {}",
                    pt_val
                );
            }

            p.rec.num_points = num_points as u32 + 1;
            p.rec.d_min = d_min;
            p.rec.d_max = d_max;
            p.rec.pt_val = pt_val as u32;
            p.rec.bytes_per_point = NltRec::get_bpp_for(pt_val as u32);

            // read the points (num_points <= 8192, so the size is bounded)
            p.rec.alloc_for_decoding();
            let n = p.rec.num_points as usize;
            let mut ok = true;
            for i in 0..n {
                let v = match p.rec.bytes_per_point {
                    1 => file.read_u8().map(|v| v as u32),
                    2 => file.read_u16().map(|v| v as u32),
                    _ => file.read_u32(),
                };
                match v {
                    Some(v) => p.rec.marker_points[i] = v,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                return ojph_error!(0x000501E3, "Error reading NLT marker, Location 3");
            }
            p.rec.prepare_for_decoding();
        }
        Ok(())
    }

    /// The record that applies to `comp_num`: its own enabled object, else
    /// the main object if enabled, else `None`.
    pub fn get_nlt_rec(&self, comp_num: u32) -> Option<&NltRec> {
        match self.get_nlt_object(comp_num) {
            Some(p) if p.enabled => Some(&p.rec),
            _ => {
                if self.enabled {
                    Some(&self.rec)
                } else {
                    None
                }
            }
        }
    }
}
