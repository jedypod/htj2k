// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Arbitrary transformation kernels (the ATK marker segment), and the two
//! kernels of Part 1: reversible 5/3 and irreversible 9/7.

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParamAtk {
    pub(crate) latk: u16,
    pub(crate) satk: u16,
    pub(crate) katk: f32,
    pub(crate) steps: Vec<LiftingStep>,
}

impl ParamAtk {
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

    pub fn get_step(&self, s: u32) -> &LiftingStep {
        &self.steps[s as usize]
    }

    pub fn get_num_steps(&self) -> u32 {
        self.steps.len() as u32
    }

    pub fn get_k(&self) -> f32 {
        self.katk
    }
}
