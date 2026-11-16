// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The CAP marker segment, extended capabilities (`param_cap`).

use super::cod::ParamCod;
use super::markers::{jp2k_marker, population_count, put_u16, put_u32};
use super::qcd::ParamQcd;
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::Result;

/// `param_cap`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamCap {
    pub(crate) lcap: u16,
    pub(crate) pcap: u32,
    /// a maximum of 32
    pub(crate) ccap: [u16; 32],
}

impl Default for ParamCap {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamCap {
    pub fn new() -> Self {
        ParamCap {
            lcap: 8,
            pcap: 0x0002_0000, // for jph, Pcap^15 must be set, the 15th MSB
            ccap: [0; 32],
        }
    }

    pub fn check_validity(&mut self, cod: &ParamCod, qcd: &ParamQcd) {
        if cod.get_wavelet_kern() == ParamCod::DWT_REV53 {
            self.ccap[0] &= 0xFFDF;
        } else {
            self.ccap[0] |= 0x0020;
        }
        self.ccap[0] &= 0xFFE0;
        let b = qcd.get_MAGB();
        let bp: u32 = if b <= 8 {
            0
        } else if b < 28 {
            b - 8
        } else {
            13 + (b >> 2)
        };
        self.ccap[0] |= bp as u16;
    }

    #[allow(non_snake_case)]
    pub fn get_Pcap(&self) -> u32 {
        self.pcap
    }

    #[allow(non_snake_case)]
    pub fn get_Ccap(&self, i: u32) -> u16 {
        self.ccap.get(i as usize).copied().unwrap_or(0)
    }

    pub fn write(&mut self, file: &mut Vec<u8>) -> bool {
        // marker size excluding header
        self.lcap = 8;
        put_u16(file, jp2k_marker::CAP);
        put_u16(file, self.lcap);
        put_u32(file, self.pcap);
        put_u16(file, self.ccap[0]);
        true
    }

    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        self.lcap = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x00050061, "error reading CAP marker"),
        };
        self.pcap = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050062, "error reading CAP marker"),
        };
        let count = population_count(self.pcap);
        if self.pcap & 0xFFFD_FFFF != 0 {
            return ojph_error!(
                0x00050063,
                "error Pcap in CAP has options that are not supported"
            );
        }
        if (self.pcap & 0x0002_0000) == 0 {
            return ojph_error!(
                0x00050064,
                "error Pcap should have its 15th MSB set, Pcap^15.  This is not a JPH file"
            );
        }
        // count is 1 here, as only bit 17 may be set
        for i in 0..count as usize {
            match file.read_u16() {
                Some(v) => self.ccap[i] = v,
                None => return ojph_error!(0x00050065, "error reading CAP marker"),
            }
        }
        if self.lcap as u32 != 6 + 2 * count {
            return ojph_error!(0x00050066, "error in CAP marker length");
        }
        Ok(())
    }
}
