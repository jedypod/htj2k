// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The DFS marker segment, downsampling factor styles (`param_dfs`).

use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::types::Point;
use crate::Result;

/// `param_dfs::dfs_dwt_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DfsDwtType {
    /// no wavelet transform
    NoDwt = 0,
    /// bidirectional DWT (this the conventional DWT)
    BidirDwt = 1,
    /// horizontal only DWT transform
    HorzDwt = 2,
    /// vertical only DWT transform
    VertDwt = 3,
}

impl DfsDwtType {
    fn from_bits(v: u32) -> Self {
        match v & 3 {
            0 => DfsDwtType::NoDwt,
            1 => DfsDwtType::BidirDwt,
            2 => DfsDwtType::HorzDwt,
            _ => DfsDwtType::VertDwt,
        }
    }
}

/// `param_dfs`. The main object owns the further DFS segments read after
/// the first (C++: the `next` list) in `others`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamDfs {
    /// length of the segment marker
    pub(crate) ldfs: u16,
    /// index of this DFS marker segment
    pub(crate) sdfs: u16,
    /// number of elements in Ddfs, 2 bits per sub-level
    pub(crate) ids: u8,
    /// a string defining number of decomposition sub-levels;
    /// 8 bytes should be enough for 32 levels
    pub(crate) ddfs: [u8; 8],
    /// further DFS segments (only used in the first object)
    pub(crate) others: Vec<ParamDfs>,
}

impl ParamDfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restart(&mut self) {
        let mut others = std::mem::take(&mut self.others);
        others.clear();
        *self = Self::new();
        self.others = others;
    }

    pub fn exists(&self) -> bool {
        self.ldfs != 0
    }

    /// Returns the dfs structure with `Sdfs == index`, or `None` if not
    /// found.
    /// C++ takes an `int`; the index is `get_dfs_index()`, a `ui16`.
    pub fn get_dfs(&self, index: u16) -> Option<&ParamDfs> {
        if self.sdfs == index {
            return Some(self);
        }
        self.others.iter().find(|p| p.sdfs == index)
    }

    /// `decomp_level` is the decomposition level, starting from 1 for
    /// highest resolution to num_decomps for the coarsest resolution.
    ///
    /// For `decomp_level == 0`, or an object with `Ids == 0`, C++ indexes
    /// far outside `Ddfs`; this returns `NoDwt` instead.
    pub fn get_dwt_type(&self, decomp_level: u32) -> DfsDwtType {
        let decomp_level = decomp_level.min(self.ids as u32);
        let d = decomp_level.wrapping_sub(1); // decomp_level starts from 1
        let idx = d >> 2; // complete bytes
        let bits = d & 0x3; // bit within the bytes
        let byte = match self.ddfs.get(idx as usize) {
            Some(b) => *b as u32,
            None => return DfsDwtType::NoDwt,
        };
        DfsDwtType::from_bits((byte >> (6 - 2 * bits)) & 0x3)
    }

    pub fn get_subband_idx(&self, num_decompositions: u32, resolution: u32, subband: u32) -> u32 {
        // C++ asserts (resolution == 0 && subband == 0) ||
        //   (resolution > 0 && subband > 0 && subband < 4)
        const NS: [u32; 4] = [0, 3, 1, 1];

        let mut idx = 0u32;
        if resolution > 0 {
            idx = 0;
            let mut i = 1u32;
            while i < resolution {
                let t = self.get_dwt_type(num_decompositions.wrapping_sub(i).wrapping_add(1));
                idx = idx.wrapping_add(NS[t as usize]);
                i += 1;
            }
            let t = self.get_dwt_type(num_decompositions.wrapping_sub(i).wrapping_add(1));
            idx = idx.wrapping_add(subband);
            if t == DfsDwtType::VertDwt && subband == 2 {
                idx = idx.wrapping_sub(1);
            }
        }
        idx
    }

    pub fn get_res_downsamp(&self, skipped_resolutions: u32) -> Point {
        let mut factor = Point::new(1, 1);
        let mut decomp_level = 1u32;
        let mut skipped_resolutions = skipped_resolutions;
        while skipped_resolutions > 0 {
            let t = self.get_dwt_type(decomp_level);
            if t == DfsDwtType::BidirDwt {
                factor.x = factor.x.wrapping_mul(2);
                factor.y = factor.y.wrapping_mul(2);
            } else if t == DfsDwtType::HorzDwt {
                factor.x = factor.x.wrapping_mul(2);
            } else if t == DfsDwtType::VertDwt {
                factor.y = factor.y.wrapping_mul(2);
            }
            decomp_level += 1;
            skipped_resolutions -= 1;
        }
        factor
    }

    /// Reads a DFS segment; if this object is already in use, the segment
    /// goes into a new object appended to `others`.
    /// C++ returns `bool` (always true, or throws).
    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        if self.ldfs != 0 {
            // this param_dfs is used
            let mut p = ParamDfs::new();
            let r = p.read(file);
            self.others.push(p);
            return r;
        }

        self.ldfs = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x000500D1, "error reading DFS-Ldfs parameter"),
        };
        self.sdfs = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x000500D2, "error reading DFS-Sdfs parameter"),
        };
        if self.sdfs > 15 {
            return ojph_error!(
                0x000500D3,
                "The DFS-Sdfs parameter is {}, which is larger than the permissible 15",
                self.sdfs
            );
        }
        let l_ids = match file.read_u8() {
            Some(v) => v,
            None => return ojph_error!(0x000500D4, "error reading DFS-Ids parameter"),
        };
        if l_ids == 0 {
            return ojph_error!(
                0x000500D8,
                "The value of the Ids member in the DFS marker segment cannot be 0"
            );
        }
        const MAX_DDFS: u8 = 8 * 4;
        // OJPH_INFO 0x000500D5 if l_ids > MAX_DDFS
        self.ids = if l_ids < MAX_DDFS { l_ids } else { MAX_DDFS };
        let mut i = 0u32;
        while i < self.ids as u32 {
            match file.read_u8() {
                Some(v) => self.ddfs[(i / 4) as usize] = v,
                None => return ojph_error!(0x000500D6, "error reading DFS-Ddfs parameters"),
            }
            i += 4;
        }
        let mut i = self.ids as u32;
        while i < l_ids as u32 {
            if file.read_u8().is_none() {
                return ojph_error!(0x000500D7, "error reading DFS-Ddfs parameters");
            }
            i += 4;
        }
        Ok(())
    }
}
