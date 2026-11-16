// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The QCD and QCC marker segments (`param_qcd`).
//!
//! As in OpenJPH, one struct serves both; the main-header QCD object owns
//! the QCC objects (`children`, the C++ `next` list).

use super::cod::ParamCod;
use super::dfs::ParamDfs;
use super::markers::{jp2k_marker, put_u16, put_u8};
use super::siz::ParamSiz;
use super::tables::{bibo_gains, sqrt_energy_gains, visual_weights};
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::types::Point;
use crate::Result;

/// `ojph::param_qcd::comp_type`. The numbers are used by the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompType {
    Y = 0,
    Cb = 1,
    Cr = 2,
    Undefined = 0xFF,
}

impl CompType {
    /// `ojph::param_qcd::ui8_2_comp_type`
    pub fn ui8_2_comp_type(c: u8) -> CompType {
        match c {
            0 => CompType::Y,
            1 => CompType::Cb,
            2 => CompType::Cr,
            _ => CompType::Undefined,
        }
    }
}

/// `param_qcd::qcd_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QcdType {
    Undefined = 0,
    QcdMain = 1,
    QccMain = 2,
    /// not implemented
    QcdTile = 3,
    /// not implemented
    QccTile = 4,
}

/// 1 LL subband + 3 subbands for each of at most 32 decomposition levels
pub const MAX_SUBBAND_COUNT: u32 = 97;
const MSC: usize = MAX_SUBBAND_COUNT as usize;

/// `param_qcd`: serves for both QCD and QCC marker segments.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamQcd {
    pub(crate) type_: QcdType,
    /// have the quantization steps been generated
    pub(crate) is_init: bool,
    pub(crate) lqcd: u16,
    pub(crate) sqcd: u8,
    /// The C++ `union { ui8 u8[97]; ui16 u16[97]; } SPqcd`, kept as raw
    /// bytes so that both views alias exactly as on a little-endian host.
    pub(crate) spqcd: [u8; 2 * MSC],
    /// number of subbands
    pub(crate) num_subbands: u32,
    /// enabled if true, and ignored if false
    pub(crate) enabled: bool,
    /// QCD only: the QCC objects
    pub(crate) children: Vec<ParamQcd>,

    // variables used to generate the quantization step sizes
    /// base quantization step size -- all other step sizes are derived
    /// from it.
    pub(crate) base_delta: f32,
    pub(crate) qfactor: f32,
    /// number of coefficients in `weights`; 0 if none
    pub(crate) num_weights: u32,
    /// weight coefficients, in subband order
    /// {LH1, HL1, HH1, ..., LHN, HLN, HHN, LLN}
    pub(crate) weights: [f32; MSC],
    pub(crate) ctype: CompType,
    pub(crate) is_color_trans: bool,
    pub(crate) num_decomps: u32,
    pub(crate) bit_depth: u32,
    pub(crate) is_signed: bool,
    pub(crate) wavelet_kern: u32,
    pub(crate) sampling: Point,

    /// QCC only
    pub(crate) comp_idx: u16,
}

impl Default for ParamQcd {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamQcd {
    pub const OJPH_QCD_UNKNOWN: u16 = 65534;
    pub const OJPH_QCD_DEFAULT: u16 = 65535;
    pub const QFACTOR_UNSET: f32 = 0.0;
    pub const MAX_SUBBAND_COUNT: u32 = MAX_SUBBAND_COUNT;

    /// A main-header QCD object.
    pub fn new() -> Self {
        ParamQcd {
            type_: QcdType::QcdMain,
            is_init: false,
            lqcd: 0,
            sqcd: 0,
            spqcd: [0; 2 * MSC],
            num_subbands: 0,
            enabled: true,
            children: Vec::new(),
            base_delta: -1.0,
            qfactor: Self::QFACTOR_UNSET,
            num_weights: 0,
            weights: [0.0; MSC],
            ctype: CompType::Y,
            is_color_trans: false,
            num_decomps: 0,
            bit_depth: 0,
            is_signed: false,
            wavelet_kern: 0,
            sampling: Point::new(1, 1),
            comp_idx: Self::OJPH_QCD_DEFAULT,
        }
    }

    /// A QCC object (`param_qcd(top_qcd, comp_idx)`).
    pub fn new_qcc(comp_idx: u16) -> Self {
        let mut p = Self::new();
        p.type_ = QcdType::QccMain;
        p.comp_idx = comp_idx;
        p
    }

    pub fn restart(&mut self) {
        let mut children = std::mem::take(&mut self.children);
        children.clear();
        let t = self.type_;
        *self = Self::new();
        self.type_ = t;
        self.children = children;
    }

    // ---- the SPqcd union views ----
    #[inline]
    fn sp_u8(&self, i: usize) -> u8 {
        self.spqcd[i]
    }
    #[inline]
    fn set_sp_u8(&mut self, i: usize, v: u8) {
        self.spqcd[i] = v;
    }
    #[inline]
    fn sp_u16(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.spqcd[2 * i], self.spqcd[2 * i + 1]])
    }
    #[inline]
    fn set_sp_u16(&mut self, i: usize, v: u16) {
        let b = v.to_le_bytes();
        self.spqcd[2 * i] = b[0];
        self.spqcd[2 * i + 1] = b[1];
    }

    /// `SPqcd.u8[i]`, for reversible step sizes (exponents << 3).
    pub fn get_spqcd_u8(&self, i: u32) -> u8 {
        self.spqcd.get(i as usize).copied().unwrap_or(0)
    }
    /// `SPqcd.u16[i]`, for irreversible step sizes.
    pub fn get_spqcd_u16(&self, i: u32) -> u16 {
        if (i as usize) < MSC {
            self.sp_u16(i as usize)
        } else {
            0
        }
    }
    #[allow(non_snake_case)]
    pub fn get_Sqcd(&self) -> u8 {
        self.sqcd
    }
    pub fn get_num_subbands(&self) -> u32 {
        self.num_subbands
    }
    pub fn get_type(&self) -> QcdType {
        self.type_
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn decode_spqcd(v: u8) -> u8 {
        v >> 3
    }
    fn encode_spqcd_u8(v: u8) -> u8 {
        v << 3
    }

    // ------------------------------------------------------------------

    pub fn check_validity(&mut self, siz: &ParamSiz, cod: &ParamCod) -> Result<()> {
        // C++ asserts type == QCD_MAIN
        let num_comps = siz.get_num_components();
        self.trim_non_existing_components(num_comps);

        // initialize QCD based on the first component that is (a) associated
        // with COD and (b) does not have a COC, or the first component
        // othewise.
        let mut qcd_comp = 0u32;
        for c in 0..num_comps {
            if std::ptr::eq(cod.get_coc(c), cod) && self.qcc_index(c).is_none() {
                qcd_comp = c;
                break;
            }
        }

        // check if only the top QCD has qfactor set, if so, check if any
        // of the first component has COC and qfactor set properly
        if self.qfactor != Self::QFACTOR_UNSET {
            let qf = self.qfactor;
            if num_comps < 3 {
                // one or two components
                for i in 0..num_comps {
                    if self.qcc_index(i).is_none() {
                        self.add_qcc_object(i);
                        self.set_qfactor_comp(i, CompType::Y, qf)?;
                    }
                }
            } else {
                for i in 0..num_comps {
                    if self.qcc_index(i).is_none() {
                        self.add_qcc_object(i);
                        let ci = if i < 3 { i as u8 } else { 0 };
                        let t = CompType::ui8_2_comp_type(ci);
                        self.set_qfactor_comp(i, t, qf)?;
                    }
                }
            }
        }

        self.make_quant_steps(qcd_comp, cod, siz)?;

        // initialize every QCC, creating one for every component that (a)
        // cannot use QCD and (b) does not already have a QCC
        // NOTE: Qfactor always creates a QCC and QCD cannot be reused
        for c in 0..num_comps {
            let coc = cod.get_coc(c);
            let idx = match self.qcc_index(c) {
                Some(i) => i,
                None => {
                    // if none exists, do not create one if QCD can be reused
                    if !self.is_qcc_needed(c, coc, siz)? {
                        continue;
                    }
                    let bd = self.base_delta;
                    self.add_qcc_object(c).set_delta(bd);
                    self.children.len() - 1
                }
            };
            self.children[idx].make_quant_steps(c, coc, siz)?;
        }
        Ok(())
    }

    /// `cod` is the COD or COC object that applies to `comp_num`.
    pub fn make_quant_steps(
        &mut self,
        comp_num: u32,
        cod: &ParamCod,
        siz: &ParamSiz,
    ) -> Result<()> {
        if self.is_init {
            return ojph_error!(0x000501C1, "Quantization step sizes already initialized.");
        }
        self.is_init = true;

        self.num_decomps = cod.get_num_decompositions();
        self.bit_depth = siz.get_bit_depth(comp_num);
        self.is_signed = siz.is_signed(comp_num);
        self.is_color_trans = cod.is_employing_color_transform();
        self.wavelet_kern = cod.get_wavelet_kern() as u32;
        self.sampling = siz.get_downsampling(comp_num);
        self.num_subbands = 1 + 3 * self.num_decomps;

        if self.num_weights != 0 {
            if self.wavelet_kern != ParamCod::DWT_IRV97 as u32 {
                return ojph_error!(
                    0x000501C2,
                    "Quantization weights for component {} can only be used with the irreversible transform.",
                    comp_num
                );
            }
            if self.qfactor != Self::QFACTOR_UNSET {
                return ojph_error!(
                    0x000501C3,
                    "Quantization weights for component {} cannot be used together with qfactor.",
                    comp_num
                );
            }
            if self.num_weights != self.num_subbands {
                return ojph_error!(
                    0x000501C4,
                    "Component {} has {} quantization weights, but {} are needed for {} decomposition levels.",
                    comp_num,
                    self.num_weights,
                    self.num_subbands,
                    self.num_decomps
                );
            }
        }

        if self.wavelet_kern == ParamCod::DWT_REV53 as u32 {
            let ct = if comp_num < 3 {
                self.is_color_trans
            } else {
                false
            };
            self.set_rev_quant(self.num_decomps, self.bit_depth, ct)?;
        } else if self.wavelet_kern == ParamCod::DWT_IRV97 as u32 {
            if self.base_delta == -1.0 {
                let t = self.bit_depth.min(16);
                self.base_delta = 1.0f32 / (1u32 << t) as f32;
            }
            // else if qfactor is set: OJPH_WARN 0x000501C5, qstep ignored
            self.set_irrev_quant_decomps(self.num_decomps)?;
        }
        Ok(())
    }

    pub fn is_qcc_needed(&self, comp_num: u32, cod: &ParamCod, siz: &ParamSiz) -> Result<bool> {
        if !self.is_init {
            return ojph_error!(0x000501D1, "Quantization step sizes not initialized.");
        }
        Ok(self.num_decomps != cod.get_num_decompositions()
            || self.bit_depth != siz.get_bit_depth(comp_num)
            || self.is_signed != siz.is_signed(comp_num)
            || self.is_color_trans != cod.is_employing_color_transform()
            || self.wavelet_kern != cod.get_wavelet_kern() as u32)
    }

    pub fn set_delta(&mut self, delta: f32) {
        self.base_delta = delta;
    }

    /// `set_delta(float delta, size_t num_weights, const float *weights)`
    pub fn set_delta_weights(&mut self, delta: f32, weights: &[f32]) -> Result<()> {
        self.set_delta(delta);
        self.set_weights(weights)
    }

    /// C++ prints the float with `%i` (garbage); this prints it properly.
    pub fn set_qfactor(&mut self, qfactor: f32) -> Result<()> {
        if !(1.0..=100.0).contains(&qfactor) {
            return ojph_error!(
                0x00050181,
                "Qfactor must be between 1 and 100, but was set to {}.",
                qfactor
            );
        }
        self.qfactor = qfactor;
        Ok(())
    }

    /// C++ takes `(size_t num_weights, const float *weights)`.
    pub fn set_weights(&mut self, weights: &[f32]) -> Result<()> {
        let num_weights = weights.len();
        if num_weights == 0 || num_weights > MSC {
            return ojph_error!(
                0x000501A1,
                "The number of quantization weights must be between 1 and {}, but {} were provided.",
                MAX_SUBBAND_COUNT,
                num_weights
            );
        }
        for w in weights {
            // written this way so that NaN is rejected, as in C++
            #[allow(clippy::neg_cmp_op_on_partial_ord)]
            if !(*w > 0.0) {
                return ojph_error!(0x000501A2, "Quantization weights must be positive.");
            }
        }
        self.weights[..num_weights].copy_from_slice(weights);
        self.num_weights = num_weights as u32;
        Ok(())
    }

    fn set_rev_quant(
        &mut self,
        num_decomps: u32,
        bit_depth: u32,
        is_employing_color_transform: bool,
    ) -> Result<()> {
        let mut b = bit_depth;
        b += if is_employing_color_transform { 1 } else { 0 }; // 1 bit for RCT
        let mut s = 0usize;
        let bibo_l = bibo_gains::get_bibo_gain_l(num_decomps, true) as f64;
        let mut x = ((bibo_l * bibo_l).ln() / std::f64::consts::LN_2).ceil() as u32;
        self.set_sp_u8(s, b.wrapping_add(x) as u8);
        s += 1;
        let mut max_b_plus_x = b.wrapping_add(x);
        let mut d = num_decomps;
        while d > 0 {
            let bibo_l = bibo_gains::get_bibo_gain_l(d, true) as f64;
            let bibo_h = bibo_gains::get_bibo_gain_h(d - 1, true) as f64;
            x = ((bibo_h * bibo_l).ln() / std::f64::consts::LN_2).ceil() as u32;
            self.set_sp_u8(s, b.wrapping_add(x) as u8);
            s += 1;
            max_b_plus_x = max_b_plus_x.max(b.wrapping_add(x));
            self.set_sp_u8(s, b.wrapping_add(x) as u8);
            s += 1;
            max_b_plus_x = max_b_plus_x.max(b.wrapping_add(x));
            x = ((bibo_h * bibo_h).ln() / std::f64::consts::LN_2).ceil() as u32;
            self.set_sp_u8(s, b.wrapping_add(x) as u8);
            s += 1;
            max_b_plus_x = max_b_plus_x.max(b.wrapping_add(x));
            d -= 1;
        }

        if max_b_plus_x > 38 {
            return ojph_error!(
                0x00050151,
                "The specified combination of bit_depth, colour transform, and type of wavelet transform requires more than 38 bits; it requires {} bits. This is beyond what is allowed in the JPEG2000 image coding format.",
                max_b_plus_x
            );
        }

        let guard_bits: i32 = 1.max(max_b_plus_x as i32 - 31);
        self.sqcd = (guard_bits << 5) as u8;
        for s in 0..(1 + 3 * num_decomps as usize) {
            let v = self.sp_u8(s);
            self.set_sp_u8(s, Self::encode_spqcd_u8((v as i32 - guard_bits) as u8));
        }
        Ok(())
    }

    fn set_irrev_quant_decomps(&mut self, num_decomps: u32) -> Result<()> {
        let guard_bits = 1i32;
        self.sqcd = ((guard_bits << 5) | 0x2) as u8; // one guard bit, scalar quantization

        let mut g_c = 1.0f32;
        let mut delta_ref = self.base_delta;
        let mut power = 1.0f32;
        let mut weights: &[f32; 19] = visual_weights::get_no_weights();

        if self.qfactor != Self::QFACTOR_UNSET {
            let format = visual_weights::get_format(&self.sampling);
            if format == visual_weights::VW_COLOUR_FORMAT_ERROR {
                return ojph_error!(
                    0x00050161,
                    "Qfactor can only be used on components with 4:4:4, 4:2:2 or 4:2:0 sampling"
                );
            }
            if self.ctype == CompType::Y && self.sampling.x != 1 && self.sampling.y != 1 {
                return ojph_error!(
                    0x00050162,
                    "Qfactor can only be used for a Y or luminance component when it is not downsampled."
                );
            }

            // calculate component gain
            g_c = visual_weights::get_gain(self.ctype);

            // calculate delta_ref & power
            let (dr, p) = visual_weights::get_delta_ref(self.qfactor, self.bit_depth);
            delta_ref = dr;
            power = p;

            // find visual weight
            weights = visual_weights::get_weights(format, self.ctype);
        }

        // returns sqrt(w_b), the square root of the weight, for a
        // decomposition level and subband (0:LL, 1:HL, 2:LH, 3:HH);
        // coefficients are ordered {LH1, HL1, HH1, ..., LLN}
        let num_weights = self.num_weights as usize;
        let own_weights = self.weights;
        let get_sqrt_weight = |d: u32, sb: u32| -> f32 {
            if num_weights == 0 {
                return visual_weights::get_weight(weights, d, sb);
            }
            if sb == 0 {
                return own_weights[num_weights - 1];
            }
            const POS: [u32; 4] = [0, 1, 0, 2];
            own_weights[((d - 1) * 3 + POS[sb as usize]) as usize]
        };

        // LL band
        let mut b = 0u32;
        let gain_l = sqrt_energy_gains::get_gain_l(num_decomps, false);
        let mut w_b = get_sqrt_weight(num_decomps, b);
        w_b = w_b.powf(power);
        self.encode_spqcd(b, delta_ref / (gain_l * gain_l * g_c * w_b));
        b += 1;

        // LL, HL, LH, HH, HL, LH, HH...
        let mut d = num_decomps;
        while d > 0 {
            // compute square root of the enery gain factor W_g
            let gain_l = sqrt_energy_gains::get_gain_l(d, false);
            let gain_h = sqrt_energy_gains::get_gain_h(d - 1, false);

            w_b = get_sqrt_weight(d, 1);
            w_b = w_b.powf(power);
            self.encode_spqcd(b, delta_ref / (gain_h * gain_l * g_c * w_b));
            b += 1;
            w_b = get_sqrt_weight(d, 2);
            w_b = w_b.powf(power);
            self.encode_spqcd(b, delta_ref / (gain_l * gain_h * g_c * w_b));
            b += 1;
            w_b = get_sqrt_weight(d, 3);
            w_b = w_b.powf(power);
            self.encode_spqcd(b, delta_ref / (gain_h * gain_h * g_c * w_b));
            b += 1;
            d -= 1;
        }
        Ok(())
    }

    /// C++ loops forever for `delta <= 0`; this stops doubling after 256
    /// steps (any positive finite f32 needs fewer than 160).
    fn encode_spqcd(&mut self, subband_index: u32, delta: f32) {
        let mut delta = delta;
        let mut exp: i32 = 0;
        while delta < 1.0 && exp < 256 {
            exp += 1;
            delta *= 2.0;
        }
        let mut mantissa = ((delta * (1i32 << 11) as f32) as f64).round() as i32 - (1 << 11);
        // with rounding, there is a risk that the mantissa becomes
        // equal to 1<<11
        mantissa = if mantissa < (1 << 11) {
            mantissa
        } else {
            0x7FF
        };
        if (subband_index as usize) < MSC {
            self.set_sp_u16(subband_index as usize, ((exp << 11) | mantissa) as u16);
        }
    }

    #[allow(non_snake_case)]
    pub fn get_MAGB(&self) -> u32 {
        let mut b = 0u32;
        for p in std::iter::once(self).chain(self.children.iter()) {
            // this can be written better, but it is only executed once
            // this assumes a bi-directional wavelet (conventional DWT)
            let num_decomps = p.num_subbands.wrapping_sub(1) / 3;
            let n = (p.num_subbands as usize).min(MSC);

            let irrev = p.sqcd & 0x1F;
            if irrev == 0 {
                // reversible
                for i in 0..n {
                    let mut t = Self::decode_spqcd(p.sp_u8(i)) as u32;
                    t = t.wrapping_add(p.get_num_guard_bits().wrapping_sub(1));
                    b = b.max(t);
                }
            } else if irrev == 2 {
                // scalar expounded
                for i in 0..n {
                    // decompsition level
                    let nb = num_decomps.wrapping_sub(if i != 0 { (i as u32 - 1) / 3 } else { 0 });
                    let t = ((p.sp_u16(i) >> 11) as u32)
                        .wrapping_add(p.get_num_guard_bits())
                        .wrapping_sub(nb);
                    b = b.max(t);
                }
            }
            // else: C++ assert(0)
        }
        b
    }

    /// Index of the step size for a subband, as `get_Kmax` and
    /// `get_irrev_delta` compute it, including the fallback to the last
    /// subband (OJPH_INFO 0x00050111 / 0x00050102).
    fn subband_index(
        &self,
        dfs: Option<&ParamDfs>,
        num_decompositions: u32,
        resolution: u32,
        subband: u32,
    ) -> usize {
        let mut idx = match dfs {
            Some(d) if d.exists() => d.get_subband_idx(num_decompositions, resolution, subband),
            _ => {
                if resolution != 0 {
                    (resolution - 1).wrapping_mul(3).wrapping_add(subband)
                } else {
                    0
                }
            }
        };
        if idx >= self.num_subbands {
            // C++ would index out of bounds if num_subbands == 0
            idx = self.num_subbands.saturating_sub(1);
        }
        (idx as usize).min(MSC - 1)
    }

    /// `dfs` is the DFS segment that applies (C++ passes NULL if none).
    #[allow(non_snake_case)]
    pub fn get_Kmax(
        &self,
        dfs: Option<&ParamDfs>,
        num_decompositions: u32,
        resolution: u32,
        subband: u32,
    ) -> u32 {
        let idx = self.subband_index(dfs, num_decompositions, resolution, subband);

        let irrev = self.sqcd & 0x1F;
        let mut num_bits = 0u32;
        if irrev == 0 {
            // reversible; this is (10.22) from the J2K book
            num_bits = Self::decode_spqcd(self.sp_u8(idx)) as u32;
            num_bits = if num_bits == 0 { 0 } else { num_bits - 1 };
        } else if irrev == 2 {
            // scalar expounded
            num_bits = ((self.sp_u16(idx) >> 11) as u32).wrapping_sub(1);
        }
        // else: C++ assert(0)
        num_bits.wrapping_add(self.get_num_guard_bits())
    }

    /// Snake-case spelling of [`Self::get_Kmax`].
    pub fn get_kmax(
        &self,
        dfs: Option<&ParamDfs>,
        num_decompositions: u32,
        resolution: u32,
        subband: u32,
    ) -> u32 {
        self.get_Kmax(dfs, num_decompositions, resolution, subband)
    }

    #[allow(non_snake_case)]
    fn get_largest_Kmax(&self) -> u32 {
        let irrev = self.sqcd & 0x1F;
        let mut num_bits = 0u32;
        let n = (self.num_subbands as usize).min(MSC);
        if irrev == 0 {
            // reversible; this is (10.22) from the J2K book
            for i in 0..n {
                let t = Self::decode_spqcd(self.sp_u8(i)) as u32;
                num_bits = num_bits.max(if t == 0 { 0 } else { t - 1 });
            }
        } else if irrev == 2 {
            // scalar expounded
            for i in 0..n {
                let t = ((self.sp_u16(i) >> 11) as u32).wrapping_sub(1);
                num_bits = num_bits.max(t);
            }
        }
        // else: C++ assert(0)
        num_bits.wrapping_add(self.get_num_guard_bits())
    }

    /// `cod` is the COD/COC object of the component. C++ reaches the main
    /// COD through `cod->get_coc(OJPH_COD_DEFAULT)` and the other QCC
    /// objects through `top_qcd`; here both are arguments (pass `self` as
    /// `top_qcd` when this is the main QCD).
    pub fn propose_precision(
        &self,
        cod: &ParamCod,
        main_cod: &ParamCod,
        top_qcd: &ParamQcd,
    ) -> u32 {
        let comp_idx = cod.get_comp_idx();
        let mut precision = 0u32;
        let main = main_cod.get_coc(ParamCod::OJPH_COD_DEFAULT as u32);
        if main.is_employing_color_transform() && comp_idx < 3 {
            for i in 0..3 {
                let p = top_qcd.get_qcc(i);
                precision = precision.max(p.get_largest_Kmax());
            }
        } else {
            precision = self.get_largest_Kmax();
        }
        // ``precision'' now holds the largest K_max, which excludes the sign
        // bit.
        // + 1 for the sign bit
        // + 1 because my block decoder/encoder does not supports up to 30
        //     bits (not 31), so we bump it by one more bit.
        precision.wrapping_add(1 + 1)
    }

    pub fn get_num_guard_bits(&self) -> u32 {
        (self.sqcd >> 5) as u32
    }

    /// `dfs` is the DFS segment that applies (C++ passes NULL if none).
    /// C++ indexes `arr[subband]` without a check; `subband` above 3 uses
    /// the factor of 3 here.
    pub fn get_irrev_delta(
        &self,
        dfs: Option<&ParamDfs>,
        num_decompositions: u32,
        comp_num: u32,
        resolution: u32,
        subband: u32,
    ) -> Result<f32> {
        const ARR: [f32; 4] = [1.0, 2.0, 2.0, 4.0];
        if (self.sqcd & 0x1F) != 2 {
            return ojph_error!(
                0x00050101,
                "There is something wrong in the configuration of the codestream; for component {}, the codestream defines an irreversible transform, for which the codestream provides a reversible (no quantization) step sizes in Sqcd/Sqcc.",
                comp_num
            );
        }
        let idx = self.subband_index(dfs, num_decompositions, resolution, subband);
        let v = self.sp_u16(idx);
        let eps = (v >> 11) as u32;
        let mut mantissa = (((v & 0x7FF) | 0x800) as f32) * ARR[(subband as usize).min(3)];
        mantissa /= (1u32 << 11) as f32;
        mantissa /= (1u32 << eps) as f32;
        Ok(mantissa)
    }

    fn write_spqcd(&self, file: &mut Vec<u8>, irrev: u8) {
        let n = (self.num_subbands as usize).min(MSC);
        if irrev == 0 {
            for i in 0..n {
                put_u8(file, self.sp_u8(i));
            }
        } else if irrev == 2 {
            for i in 0..n {
                put_u16(file, self.sp_u16(i));
            }
        }
        // else: C++ assert(0)
    }

    pub fn write(&mut self, file: &mut Vec<u8>) -> bool {
        let irrev = self.sqcd & 0x1F;

        // marker size excluding header
        self.lqcd = 3;
        if irrev == 0 {
            self.lqcd = (self.lqcd as u32).wrapping_add(self.num_subbands) as u16;
        } else if irrev == 2 {
            self.lqcd = (self.lqcd as u32).wrapping_add(2 * self.num_subbands) as u16;
        }
        // else: C++ assert(0)

        put_u16(file, jp2k_marker::QCD);
        put_u16(file, self.lqcd);
        put_u8(file, self.sqcd);
        self.write_spqcd(file, irrev);
        true
    }

    pub fn write_qcc(&mut self, file: &mut Vec<u8>, num_comps: u32) -> bool {
        // C++ asserts type == QCD_MAIN
        let mut result = true;
        for p in &mut self.children {
            if p.enabled {
                result &= p.internal_write_qcc(file, num_comps);
            }
        }
        result
    }

    fn internal_write_qcc(&mut self, file: &mut Vec<u8>, num_comps: u32) -> bool {
        let irrev = self.sqcd & 0x1F;

        // marker size excluding header
        self.lqcd = 4 + if num_comps < 257 { 0 } else { 1 };
        if irrev == 0 {
            self.lqcd = (self.lqcd as u32).wrapping_add(self.num_subbands) as u16;
        } else if irrev == 2 {
            self.lqcd = (self.lqcd as u32).wrapping_add(2 * self.num_subbands) as u16;
        }
        // else: C++ assert(0)

        put_u16(file, jp2k_marker::QCC);
        put_u16(file, self.lqcd);
        if num_comps < 257 {
            put_u8(file, self.comp_idx as u8);
        } else {
            put_u16(file, self.comp_idx);
        }
        put_u8(file, self.sqcd);
        self.write_spqcd(file, irrev);
        true
    }

    pub fn trim_non_existing_components(&mut self, num_comps: u32) {
        // C++ asserts type == QCD_MAIN && comp_idx == OJPH_QCD_DEFAULT
        for p in &mut self.children {
            p.enabled = (p.comp_idx as u32) < num_comps;
        }
    }

    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        self.lqcd = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x00050081, "error reading QCD marker"),
        };
        self.sqcd = match file.read_u8() {
            Some(v) => v,
            None => return ojph_error!(0x00050082, "error reading QCD marker"),
        };
        let lqcd = self.lqcd;
        if (self.sqcd & 0x1F) == 0 {
            self.num_subbands = (lqcd as i32 - 3) as u32;
            if self.num_subbands == 0 {
                return ojph_error!(
                    0x0005008A,
                    "QCD marker segment that specifies no quantization informtion"
                );
            }
            if self.num_subbands > MAX_SUBBAND_COUNT || lqcd as u32 != 3 + self.num_subbands {
                return ojph_error!(0x00050083, "wrong Lqcd value of {} in QCD marker", lqcd);
            }
            for i in 0..self.num_subbands as usize {
                match file.read_u8() {
                    Some(v) => self.set_sp_u8(i, v),
                    None => return ojph_error!(0x00050084, "error reading QCD marker"),
                }
            }
        } else if (self.sqcd & 0x1F) == 1 {
            self.num_subbands = 0;
            return ojph_error!(
                0x00050089,
                "Scalar derived quantization is not supported yet in QCD marker"
            );
            // (C++ then checks Lqcd != 5, error 0x00050085, unreachable)
        } else if (self.sqcd & 0x1F) == 2 {
            self.num_subbands = ((lqcd as i32 - 3) / 2) as u32;
            if self.num_subbands == 0 {
                return ojph_error!(
                    0x0005008B,
                    "QCD marker segment that specifies no quantization informtion"
                );
            }
            if self.num_subbands > MAX_SUBBAND_COUNT || lqcd as u32 != 3 + 2 * self.num_subbands {
                return ojph_error!(0x00050086, "wrong Lqcd value of {} in QCD marker", lqcd);
            }
            for i in 0..self.num_subbands as usize {
                match file.read_u16() {
                    Some(v) => self.set_sp_u16(i, v),
                    None => return ojph_error!(0x00050087, "error reading QCD marker"),
                }
            }
        } else {
            return ojph_error!(0x00050088, "wrong Sqcd value in QCD marker");
        }
        Ok(())
    }

    /// Reads a QCC segment into this QCC object: the C++ `read_qcc` member
    /// of the QCC object; the main object's [`Self::read_qcc`] calls it.
    pub fn read_qcc_segment(&mut self, file: &mut MemInfile, num_comps: u32) -> Result<()> {
        self.lqcd = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x000500A1, "error reading QCC marker"),
        };
        if num_comps < 257 {
            match file.read_u8() {
                Some(v) => self.comp_idx = v as u16,
                None => return ojph_error!(0x000500A2, "error reading QCC marker"),
            }
        } else {
            match file.read_u16() {
                Some(v) => self.comp_idx = v,
                None => return ojph_error!(0x000500A3, "error reading QCC marker"),
            }
        }
        self.sqcd = match file.read_u8() {
            Some(v) => v,
            None => return ojph_error!(0x000500A4, "error reading QCC marker"),
        };
        let lqcd = self.lqcd;
        // C++ `offset` is ui32, so the subtractions below are unsigned
        let offset: u32 = if num_comps < 257 { 4 } else { 5 };
        if (self.sqcd & 0x1F) == 0 {
            self.num_subbands = (lqcd as u32).wrapping_sub(offset);
            if self.num_subbands == 0 {
                return ojph_error!(
                    0x000500AC,
                    "QCC marker segment that specifies no quantization informtion"
                );
            }
            if self.num_subbands > MAX_SUBBAND_COUNT
                || lqcd as u32 != offset.wrapping_add(self.num_subbands)
            {
                return ojph_error!(0x000500A5, "wrong Lqcd value of {} in QCC marker", lqcd);
            }
            for i in 0..self.num_subbands as usize {
                match file.read_u8() {
                    Some(v) => self.set_sp_u8(i, v),
                    None => return ojph_error!(0x000500A6, "error reading QCC marker"),
                }
            }
        } else if (self.sqcd & 0x1F) == 1 {
            self.num_subbands = 0;
            return ojph_error!(
                0x000500AB,
                "Scalar derived quantization is not supported yet in QCC marker"
            );
            // (C++ then checks Lqcd != offset, error 0x000500A7, unreachable)
        } else if (self.sqcd & 0x1F) == 2 {
            self.num_subbands = (lqcd as u32).wrapping_sub(offset) / 2;
            if self.num_subbands == 0 {
                return ojph_error!(
                    0x000500AD,
                    "QCC marker segment that specifies no quantization informtion"
                );
            }
            if self.num_subbands > MAX_SUBBAND_COUNT
                || lqcd as u32 != offset.wrapping_add(self.num_subbands.wrapping_mul(2))
            {
                return ojph_error!(0x000500A8, "wrong Lqcc value of {} in QCC marker", lqcd);
            }
            for i in 0..self.num_subbands as usize {
                match file.read_u16() {
                    Some(v) => self.set_sp_u16(i, v),
                    None => return ojph_error!(0x000500A9, "error reading QCC marker"),
                }
            }
        } else {
            return ojph_error!(0x000500AA, "wrong Sqcc value in QCC marker");
        }
        Ok(())
    }

    /// What the C++ codestream does on a QCC marker, called on the main
    /// QCD object: `p = qcd.add_qcc_object(OJPH_QCD_UNKNOWN);
    /// p->read_qcc(file, num_comps);` followed by the codestream's checks
    /// (`ojph_codestream_local.cpp`): error 0x00030054 for a component
    /// index beyond `num_comps`, and error 0x00030055 when an earlier QCC
    /// has the same component index. As in C++, the object stays appended
    /// even when reading fails.
    pub fn read_qcc(&mut self, file: &mut MemInfile, num_comps: u32) -> Result<()> {
        let p = self.add_qcc_object(Self::OJPH_QCD_UNKNOWN as u32);
        p.read_qcc_segment(file, num_comps)?;
        let child = self.children.len() - 1;
        let idx = self.children[child].comp_idx;
        if idx as u32 >= num_comps {
            return ojph_error!(
                0x00030054,
                "The codestream carries a QCC marker segment for a component indexed by {}, which is more than the allowed index number, since the codestream has {} components",
                idx,
                num_comps
            );
        }
        if self.is_duplicate_qcc(child) {
            return ojph_error!(
                0x00030055,
                "The codestream has two QCC marker segments for one component of index {}",
                idx
            );
        }
        Ok(())
    }

    /// True when `get_qcc(comp_idx)` would find a QCC object other than the
    /// one at `children[child]` with the same index (the C++ test after
    /// reading a QCC segment).
    pub fn is_duplicate_qcc(&self, child: usize) -> bool {
        let idx = match self.children.get(child) {
            Some(c) => c.comp_idx,
            None => return false,
        };
        if idx == self.comp_idx {
            // get_qcc finds this main object first (OJPH_QCD_DEFAULT)
            return true;
        }
        match self.children.iter().position(|c| c.comp_idx == idx) {
            Some(first) => first != child,
            None => false,
        }
    }

    /// The QCC objects, in the order they were added.
    pub fn qccs(&self) -> &[ParamQcd] {
        &self.children
    }

    /// `set_delta(ui32 comp_idx, float delta)`
    pub fn set_delta_comp(&mut self, comp_idx: u32, delta: f32) {
        // C++ asserts type == QCD_MAIN
        self.get_or_add_qcc(comp_idx).set_delta(delta);
    }

    /// `set_delta(ui32 comp_idx, float delta, size_t num_weights, const
    /// float *weights)`
    pub fn set_delta_comp_weights(
        &mut self,
        comp_idx: u32,
        delta: f32,
        weights: &[f32],
    ) -> Result<()> {
        let p = self.get_or_add_qcc(comp_idx);
        p.set_delta(delta);
        p.set_weights(weights)
    }

    /// `set_qfactor(ui32 comp_idx, comp_type ctype, float qfactor)`
    pub fn set_qfactor_comp(&mut self, comp_idx: u32, ctype: CompType, qfactor: f32) -> Result<()> {
        if !(1.0..=100.0).contains(&qfactor) {
            return ojph_error!(
                0x00050191,
                "Qfactor must be between 1 and 100, but was set to {}.",
                qfactor
            );
        }
        let p = self.get_or_add_qcc(comp_idx);
        p.qfactor = qfactor;
        p.ctype = ctype;
        Ok(())
    }

    /// Index of the QCC object `get_qcc(comp_idx)` would return; `None`
    /// when that is this object (C++ starts the search here, and this
    /// object's index is `OJPH_QCD_DEFAULT`).
    fn qcc_index(&self, comp_idx: u32) -> Option<usize> {
        if self.comp_idx as u32 == comp_idx {
            return None;
        }
        self.children
            .iter()
            .position(|p| p.comp_idx as u32 == comp_idx)
    }

    fn get_or_add_qcc(&mut self, comp_idx: u32) -> &mut ParamQcd {
        match self.qcc_index(comp_idx) {
            Some(i) => &mut self.children[i],
            None => self.add_qcc_object(comp_idx),
        }
    }

    /// The QCC object for `comp_idx`, or this QCD object if there is none.
    /// On a QCC object (which has no children here) this returns itself.
    pub fn get_qcc(&self, comp_idx: u32) -> &ParamQcd {
        match self.qcc_index(comp_idx) {
            Some(i) => &self.children[i],
            None => self,
        }
    }

    pub fn get_qcc_mut(&mut self, comp_idx: u32) -> &mut ParamQcd {
        match self.qcc_index(comp_idx) {
            Some(i) => &mut self.children[i],
            None => self,
        }
    }

    pub fn add_qcc_object(&mut self, comp_idx: u32) -> &mut ParamQcd {
        // C++ asserts type == QCD_MAIN
        self.children.push(ParamQcd::new_qcc(comp_idx as u16));
        self.children.last_mut().unwrap()
    }

    pub fn get_comp_idx(&self) -> u16 {
        self.comp_idx
    }

    // ------------------------------------------------------------------
    // public API (`ojph::param_qcd`)

    pub fn set_irrev_quant(&mut self, delta: f32) {
        self.set_delta(delta);
    }

    /// `set_irrev_quant(float delta, size_t num_weights, const float[])`
    pub fn set_irrev_quant_weights(&mut self, delta: f32, weights: &[f32]) -> Result<()> {
        self.set_delta_weights(delta, weights)
    }

    /// `set_irrev_quant(ui32 comp_idx, float delta)`
    pub fn set_irrev_quant_comp(&mut self, comp_idx: u32, delta: f32) {
        self.set_delta_comp(comp_idx, delta);
    }

    /// `set_irrev_quant(ui32 comp_idx, float delta, size_t, const float[])`
    pub fn set_irrev_quant_comp_weights(
        &mut self,
        comp_idx: u32,
        delta: f32,
        weights: &[f32],
    ) -> Result<()> {
        self.set_delta_comp_weights(comp_idx, delta, weights)
    }
}
