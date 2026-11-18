// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Arbitrary transformation kernels (the ATK marker segment), and the two
//! kernels of Part 1: reversible 5/3 and irreversible 9/7.
//!
//! Limitations (as in OpenJPH): arbitrary filters (ARB) are not supported;
//! only one coefficient per step (first order filter); only the
//! even-indexed subsequence in the first reconstruction step, m_init = 0.

use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::Result;

/// One lifting step. OpenJPH keeps this as a union of the irreversible and
/// reversible forms; here both sets of fields are present and only the set
/// matching [`ParamAtk::is_reversible`] is meaningful.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiftingStep {
    /// Irreversible: the lifting coefficient.
    pub irv_a: f32,
    /// Reversible: epsilon, the power of two to divide by.
    pub rev_e: u8,
    /// Reversible: beta, the additive residue.
    pub rev_b: i16,
    /// Reversible: the lifting coefficient.
    pub rev_a: i16,
}

/// A wavelet kernel, as carried by an ATK marker segment.
///
/// The main object (the one the codestream owns) also owns any further
/// kernels (C++: the `next` list, each with `top_atk` pointing back) in
/// `others`; `get_atk` searches the main object, then `others` in order.
#[derive(Debug, Default, PartialEq)]
pub struct ParamAtk {
    /// structure length
    pub(crate) latk: u16,
    /// carries a variety of information
    pub(crate) satk: u16,
    /// only for irreversible scaling factor K
    pub(crate) katk: f32,
    /// the lifting steps; `steps.len()` is Natk
    pub(crate) steps: Vec<LiftingStep>,
    /// further kernels (only used in the main object)
    pub(crate) others: Vec<ParamAtk>,
}

impl Clone for ParamAtk {
    fn clone(&self) -> Self {
        ParamAtk {
            latk: self.latk,
            satk: self.satk,
            katk: self.katk,
            steps: self.steps.clone(),
            others: self.others.clone(),
        }
    }

    /// Copies `source` into the storage `self` already has.
    fn clone_from(&mut self, source: &Self) {
        self.latk = source.latk;
        self.satk = source.satk;
        self.katk = source.katk;
        self.steps.clone_from(&source.steps);
        self.others.clone_from(&source.others);
    }
}

impl ParamAtk {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restart(&mut self) {
        let mut others = std::mem::take(&mut self.others);
        others.clear();
        let mut steps = std::mem::take(&mut self.steps);
        steps.clear();
        *self = Self::new();
        self.others = others;
        self.steps = steps;
    }

    pub fn get_index(&self) -> u8 {
        (self.satk & 0xFF) as u8
    }

    pub fn get_coeff_type(&self) -> u32 {
        ((self.satk >> 8) & 0x7) as u32
    }

    pub fn is_whole_sample(&self) -> bool {
        (self.satk & 0x800) != 0
    }

    pub fn is_reversible(&self) -> bool {
        (self.satk & 0x1000) != 0
    }

    pub fn is_m_init0(&self) -> bool {
        (self.satk & 0x2000) == 0
    }

    pub fn is_using_ws_extension(&self) -> bool {
        (self.satk & 0x4000) != 0
    }

    /// C++ asserts `s < Natk`; this panics on an out-of-range step.
    pub fn get_step(&self, s: u32) -> &LiftingStep {
        &self.steps[s as usize]
    }

    pub fn get_num_steps(&self) -> u32 {
        self.steps.len() as u32
    }

    pub fn get_k(&self) -> f32 {
        self.katk
    }

    /// The C++ spelling of [`Self::get_k`].
    #[allow(non_snake_case)]
    pub fn get_K(&self) -> f32 {
        self.katk
    }

    /// Finds the kernel with `index`. If this object is unused and `index`
    /// is 0 or 1, it becomes the 9/7 or 5/3 kernel; if `index` is 0 or 1
    /// and not found, such a kernel is added. Returns `None` if not found.
    pub fn get_atk(&mut self, index: i32) -> Option<&ParamAtk> {
        if self.latk == 0 {
            // This atk object is not used, initialize it to either 0 (irv97)
            // or 1 (rev53), and use it.  If index is not 0 nor 1, then index
            // must have been read from file previously, otherwise it is an
            // error.
            if index == 0 {
                self.init_irv97();
                return Some(self);
            } else if index == 1 {
                self.init_rev53();
                return Some(self);
            }
        }

        if self.get_index() as i32 == index {
            return Some(self);
        }
        if let Some(i) = self
            .others
            .iter()
            .position(|p| p.get_index() as i32 == index)
        {
            return Some(&self.others[i]);
        }

        if index == 0 || index == 1 {
            // The index was not found, add an atk object only if the index
            // is either 0 or 1
            let p = self.add_object();
            if index == 0 {
                p.init_irv97();
            } else {
                p.init_rev53();
            }
            return Some(p);
        }
        None
    }

    /// What [`Self::get_atk`] returns, as an owned copy (without `others`),
    /// but without changing `self`: an unused object or a missing index 0
    /// or 1 gives the default kernel that `get_atk` would have created.
    pub fn resolve_atk(&self, index: i32) -> Option<ParamAtk> {
        let mut r = ParamAtk::new();
        self.resolve_atk_into(index, &mut r).then_some(r)
    }

    /// [`Self::resolve_atk`] into `out`, reusing its storage. Returns
    /// false, and leaves `out` as it was, where `resolve_atk` returns
    /// `None`.
    pub(crate) fn resolve_atk_into(&self, index: i32, out: &mut ParamAtk) -> bool {
        let found = if self.latk == 0 && (index == 0 || index == 1) {
            None
        } else {
            self.find(index)
        };
        match found {
            Some(p) => {
                out.latk = p.latk;
                out.satk = p.satk;
                out.katk = p.katk;
                out.steps.clone_from(&p.steps);
                out.others.clear();
            }
            None if index == 0 || index == 1 => {
                out.restart();
                if index == 0 {
                    out.init_irv97();
                } else {
                    out.init_rev53();
                }
            }
            None => return false,
        }
        true
    }

    /// Non-mutating search of this object and `others` by index, as the
    /// C++ `get_atk` does once the index is not 0 or 1 or this is in use.
    fn find(&self, index: i32) -> Option<&ParamAtk> {
        if self.get_index() as i32 == index {
            return Some(self);
        }
        self.others.iter().find(|p| p.get_index() as i32 == index)
    }

    /// Reads a coefficient into `k`; false at the end of the data. As in
    /// C++, coefficient types 5 to 7 read nothing, leave `k` unchanged and
    /// succeed.
    fn read_coefficient_f32(&self, file: &mut MemInfile, k: &mut f32, bytes: &mut i32) -> bool {
        let coeff_type = self.get_coeff_type();
        if coeff_type == 0 {
            // 8bit
            let Some(v) = file.read_u8() else {
                return false;
            };
            *bytes -= 1;
            *k = v as f32;
        } else if coeff_type == 1 {
            // 16bit
            let Some(v) = file.read_u16() else {
                return false;
            };
            *bytes -= 2;
            *k = v as f32;
        } else if coeff_type == 2 {
            // float
            let Some(i) = file.read_u32() else {
                return false;
            };
            *bytes -= 4;
            *k = f32::from_bits(i);
        } else if coeff_type == 3 {
            // double
            let Some(v) = read_u64(file) else {
                return false;
            };
            *bytes -= 8;
            *k = f64::from_bits(v) as f32;
        } else if coeff_type == 4 {
            // 128 bit float
            let Some(v) = read_u64(file) else {
                return false;
            };
            *bytes -= 8;
            // v1 not needed
            if read_u64(file).is_none() {
                return false;
            }
            *bytes -= 8;

            // convert the MSB of 128b float to 32b float
            // 32b float has 1 sign bit, 8 exponent (offset 127), 23 mantissa
            // 128b float has 1 sign bit, 15 exponent (offset 16383), 112
            // mantissa
            let mut e = ((v >> 48) & 0x7FFF) as i32; // exponent
            e -= 16383;
            e += 127;
            e &= 0xFF; // removes MSBs if negative
            e <<= 23; // move bits to their location
            let mut i = 0u32;
            i |= ((v >> 32) as u32) & 0x8000_0000; // copy sign bit
            i |= e as u32; // copy exponent
            i |= ((v >> 25) & 0x007F_FFFF) as u32; // copy 23 mantissa
            *k = f32::from_bits(i);
        }
        true
    }

    fn read_coefficient_i16(&self, file: &mut MemInfile, bytes: &mut i32) -> Option<i16> {
        let coeff_type = self.get_coeff_type();
        if coeff_type == 0 {
            let v = file.read_u8()? as i8;
            *bytes -= 1;
            Some(v as i16)
        } else if coeff_type == 1 {
            let v = file.read_u16()?;
            *bytes -= 2;
            Some(v as i16)
        } else {
            None
        }
    }

    /// Reads an ATK segment; if this object is already in use, the segment
    /// goes into a new object appended to `others`.
    /// C++ returns `bool` (always true, or throws).
    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        if self.latk != 0 {
            // this param_atk is used
            let mut p = ParamAtk::new();
            let mut tmp_idx = None;
            let r = p.read_segment(file, Some(&*self), &mut tmp_idx);
            self.others.push(p);
            // C++ checks for a repeated index with top_atk->get_atk(), while
            // the new object is already in the chain with index 0; for
            // index 1, if absent, that call appends a 5/3 kernel (before
            // the error for index 1 is raised). Reproduce that side effect.
            if tmp_idx == Some(1) && self.find(1).is_none() {
                self.add_object().init_rev53();
            }
            return r;
        }
        self.read_segment(file, None, &mut None)
    }

    /// The body of C++ `read` after the "already used" test; `top_atk` is
    /// the main object when this is one of its further kernels.
    /// `tmp_idx` receives the index from Satk once read.
    fn read_segment(
        &mut self,
        file: &mut MemInfile,
        top_atk: Option<&ParamAtk>,
        tmp_idx_out: &mut Option<i32>,
    ) -> Result<()> {
        self.latk = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x000500E1, "error reading ATK-Latk parameter"),
        };
        let mut bytes: i32 = self.latk as i32 - 2;
        let temp_satk = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x000500E2, "error reading ATK-Satk parameter"),
        };
        bytes -= 2;
        let tmp_idx = (temp_satk & 0xFF) as i32;
        *tmp_idx_out = Some(tmp_idx);
        // C++ calls top_atk->get_atk(tmp_idx), which can only differ from a
        // plain search for indices 0 and 1, which are rejected anyway (its
        // side effect for index 1 is reproduced by the caller, `read`).
        let repeated = match top_atk {
            Some(t) => t.find(tmp_idx).is_some(),
            None => false,
        };
        if repeated || tmp_idx == 0 || tmp_idx == 1 {
            return ojph_error!(
                0x000500F3,
                "ATK-Satk parameter sets ATK marker index to the illegal value of {}. ATK-Satk should be in (2-255) and, I believe, must not be repeated; otherwise, it would be unclear what marker segment must be employed when an index is repeated.",
                tmp_idx
            );
        }
        self.satk = temp_satk;
        if !self.is_m_init0() {
            // only even-indexed is supported
            return ojph_error!(
                0x000500E3,
                "ATK-Satk parameter sets m_init to 1, requiring odd-indexed subsequence in first reconstruction step, which is not supported yet."
            );
        }
        if !self.is_whole_sample() {
            // ARB filter not supported
            return ojph_error!(
                0x000500E4,
                "ATK-Satk parameter specified ARB filter, which is not supported yet."
            );
        }
        if self.is_reversible() && self.get_coeff_type() >= 2 {
            // reversible & float
            return ojph_error!(
                0x000500E5,
                "ATK-Satk parameter does not make sense. It employs floats with reversible filtering."
            );
        }
        if !self.is_using_ws_extension() {
            // only sym. ext is supported
            return ojph_error!(
                0x000500E6,
                "ATK-Satk parameter requires constant boundary extension, which is not supported yet."
            );
        }
        if !self.is_reversible() {
            let mut k = self.katk;
            if !self.read_coefficient_f32(file, &mut k, &mut bytes) {
                return ojph_error!(0x000500E7, "error reading ATK-Katk parameter");
            }
            self.katk = k;
        }
        let natk = match file.read_u8() {
            Some(v) => v,
            None => return ojph_error!(0x000500E8, "error reading ATK-Natk parameter"),
        };
        bytes -= 1;
        // Natk is at most 255, so this allocation is bounded
        self.steps.clear();
        self.steps.resize(natk as usize, LiftingStep::default());

        if self.is_reversible() {
            for s in 0..natk as usize {
                let e = match file.read_u8() {
                    Some(v) => v,
                    None => return ojph_error!(0x000500E9, "error reading ATK-Eatk parameter"),
                };
                self.steps[s].rev_e = e;
                bytes -= 1;
                let b = match file.read_u16() {
                    Some(v) => v as i16,
                    None => return ojph_error!(0x000500EA, "error reading ATK-Batk parameter"),
                };
                self.steps[s].rev_b = b;
                bytes -= 2;
                let lcatk = match file.read_u8() {
                    Some(v) => v,
                    None => return ojph_error!(0x000500EB, "error reading ATK-LCatk parameter"),
                };
                bytes -= 1;
                if lcatk == 0 {
                    return ojph_error!(
                        0x000500EC,
                        "Encountered a ATK-LCatk value of zero; something is wrong."
                    );
                }
                if lcatk > 1 {
                    return ojph_error!(
                        0x000500ED,
                        "ATK-LCatk value greater than 1; that is, a multitap filter is not supported"
                    );
                }
                match self.read_coefficient_i16(file, &mut bytes) {
                    Some(a) => self.steps[s].rev_a = a,
                    None => return ojph_error!(0x000500EE, "Error reding ATK-Aatk parameter"),
                }
            }
        } else {
            for s in 0..natk as usize {
                let lcatk = match file.read_u8() {
                    Some(v) => v,
                    None => return ojph_error!(0x000500EF, "error reading ATK-LCatk parameter"),
                };
                bytes -= 1;
                if lcatk == 0 {
                    return ojph_error!(
                        0x000500F0,
                        "Encountered a ATK-LCatk value of zero; something is wrong."
                    );
                }
                if lcatk > 1 {
                    return ojph_error!(
                        0x000500F1,
                        "ATK-LCatk value greater than 1; that is, a multitap filter is not supported."
                    );
                }
                let mut a = self.steps[s].irv_a;
                if !self.read_coefficient_f32(file, &mut a, &mut bytes) {
                    return ojph_error!(0x000500F2, "Error reding ATK-Aatk parameter");
                }
                self.steps[s].irv_a = a;
            }
        }
        if bytes != 0 {
            return ojph_error!(
                0x000500F3,
                "The length of an ATK marker segment (ATK-Latk) is not correct"
            );
        }
        Ok(())
    }

    pub(crate) fn init_irv97(&mut self) {
        self.satk = 0x4a00; // illegal because ATK = 0
        self.katk = 1.230174104914001f64 as f32;
        let natk = 4u16;
        // next is (A-4) in T.801 second line
        self.latk = 5 + natk + 4 * (1 + natk);
        self.steps.clear();
        self.steps.resize(natk as usize, LiftingStep::default());
        self.steps[0].irv_a = 0.443506852043971f64 as f32;
        self.steps[1].irv_a = 0.882911075530934f64 as f32;
        self.steps[2].irv_a = -0.052980118572961f64 as f32;
        self.steps[3].irv_a = -1.586134342059924f64 as f32;
    }

    pub(crate) fn init_rev53(&mut self) {
        self.satk = 0x5801; // illegal because ATK = 1
        let natk = 2u16;
        // next is (A-4) in T.801 fourth line
        self.latk = 5 + 2 * natk + (natk + natk);
        self.steps.clear();
        self.steps.resize(natk as usize, LiftingStep::default());
        self.steps[0].rev_a = 1;
        self.steps[0].rev_b = 2;
        self.steps[0].rev_e = 2;
        self.steps[1].rev_a = -1;
        self.steps[1].rev_b = 1;
        self.steps[1].rev_e = 1;
    }

    fn add_object(&mut self) -> &mut ParamAtk {
        self.others.push(ParamAtk::new());
        self.others.last_mut().unwrap()
    }

    /// All kernels held: this object, then the further ones.
    pub fn iter(&self) -> impl Iterator<Item = &ParamAtk> {
        std::iter::once(self).chain(self.others.iter())
    }
}

fn read_u64(file: &mut MemInfile) -> Option<u64> {
    let b = file.remaining().get(..8)?;
    let v = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    file.seek(8, crate::file::SeekFrom::Current);
    Some(v)
}
