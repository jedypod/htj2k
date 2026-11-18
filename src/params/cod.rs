// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The COD and COC marker segments (`param_cod`, `cod_SPcod`, `cod_SGcod`).
//!
//! As in OpenJPH, one struct serves both: the main-header COD object owns
//! the COC objects (`children`, the C++ `next` list). A COC object needs two
//! values of its parent COD (the number of decompositions, when DFS is in
//! use, and the colour transform flag, both reached through `top_cod` in
//! C++); here the parent keeps copies of them in each child up to date.

use super::atk::ParamAtk;
use super::markers::{
    jp2k_marker, log2_floor, put_u16, put_u8, ProgressionOrder, OJPH_PO_CPRL, OJPH_PO_LRCP,
    OJPH_PO_PCRL, OJPH_PO_RLCP, OJPH_PO_RPCL, OJPH_PO_STRING_CPRL, OJPH_PO_STRING_LRCP,
    OJPH_PO_STRING_PCRL, OJPH_PO_STRING_RLCP, OJPH_PO_STRING_RPCL,
};
use super::siz::ParamSiz;
use super::spare::Spare;
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::types::Size;
use crate::Result;

/// `cod_SPcod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodSPcod {
    pub num_decomp: u8,
    pub block_width: u8,
    pub block_height: u8,
    pub block_style: u8,
    pub wavelet_trans: u8,
    /// num_decomp is in `[0, 32]`
    pub precinct_size: [u8; 33],
}

impl Default for CodSPcod {
    fn default() -> Self {
        CodSPcod {
            num_decomp: 5,
            block_width: 4,    // 64
            block_height: 4,   // 64
            block_style: 0x40, // HT mode
            wavelet_trans: 0,  // reversible 5 / 3
            precinct_size: [0; 33],
        }
    }
}

impl CodSPcod {
    pub fn get_log_block_dims(&self) -> Size {
        Size::new(self.block_width as u32 + 2, self.block_height as u32 + 2)
    }
    pub fn get_block_dims(&self) -> Size {
        let t = self.get_log_block_dims();
        Size::new(1u32 << t.w, 1u32 << t.h)
    }
    /// C++ asserts `res_num <= num_decomp`; an index outside the array
    /// reads as 0 here.
    pub fn get_log_precinct_size(&self, res_num: u32) -> Size {
        let p = self
            .precinct_size
            .get(res_num as usize)
            .copied()
            .unwrap_or(0);
        Size::new((p & 0xF) as u32, (p >> 4) as u32)
    }
}

/// `cod_SGcod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodSGcod {
    pub prog_order: u8,
    pub num_layers: u16,
    pub mc_trans: u8,
}

impl Default for CodSGcod {
    fn default() -> Self {
        CodSGcod {
            prog_order: OJPH_PO_RPCL as u8,
            num_layers: 1,
            mc_trans: 0,
        }
    }
}

/// `param_cod::cod_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodType {
    Undefined = 0,
    CodMain = 1,
    CocMain = 2,
    /// not implemented
    CodTile = 3,
    /// not implemented
    CocTile = 4,
}

/// `param_cod`: serves for both COD and COC marker segments.
#[derive(Debug, PartialEq)]
pub struct ParamCod {
    pub(crate) type_: CodType,
    /// serves as Lcod and Lcoc
    pub(crate) lcod: u16,
    /// serves as Scod and Scoc
    pub(crate) scod: u8,
    /// Used in COD and copied to COC
    pub(crate) sgcod: CodSGcod,
    /// serves as SPcod and SPcoc
    pub(crate) spcod: CodSPcod,
    /// The resolved kernel (C++: `const param_atk* atk`), set by
    /// `update_atk`; an empty kernel before that (C++: NULL).
    pub(crate) atk: ParamAtk,
    /// COC only: component index of this COC structure
    pub(crate) comp_idx: u16,
    /// COD only: the COC objects (C++: the `next` list)
    pub(crate) children: Vec<ParamCod>,
    /// COC only: copy of the parent's `SPcod.num_decomp` (C++ reads
    /// `top_cod->get_num_decompositions()`)
    pub(crate) top_num_decomp: u8,
    /// COC only: copy of the parent's `SGCod.mc_trans` (C++ reads
    /// `top_cod->is_employing_color_transform()`)
    pub(crate) top_mc_trans: u8,
    /// COD only: COC objects removed by a restart, whose storage the
    /// next COC objects reuse.
    pub(crate) spare: Spare<ParamCod>,
}

impl Clone for ParamCod {
    fn clone(&self) -> Self {
        ParamCod {
            type_: self.type_,
            lcod: self.lcod,
            scod: self.scod,
            sgcod: self.sgcod,
            spcod: self.spcod,
            atk: self.atk.clone(),
            comp_idx: self.comp_idx,
            children: self.children.clone(),
            top_num_decomp: self.top_num_decomp,
            top_mc_trans: self.top_mc_trans,
            spare: Spare::new(),
        }
    }

    /// Copies `source` into the storage `self` already has; `self` keeps
    /// its spares.
    fn clone_from(&mut self, source: &Self) {
        self.type_ = source.type_;
        self.lcod = source.lcod;
        self.scod = source.scod;
        self.sgcod = source.sgcod;
        self.spcod = source.spcod;
        self.atk.clone_from(&source.atk);
        self.comp_idx = source.comp_idx;
        self.children.clone_from(&source.children);
        self.top_num_decomp = source.top_num_decomp;
        self.top_mc_trans = source.top_mc_trans;
    }
}

impl Default for ParamCod {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamCod {
    pub const OJPH_COD_UNKNOWN: u16 = 65534;
    pub const OJPH_COD_DEFAULT: u16 = 65535;

    /// `BLOCK_CODING_STYLES`
    pub const VERT_CAUSAL_MODE: u8 = 0x8;
    pub const HT_MODE: u8 = 0x40;

    /// `dwt_type`
    pub const DWT_IRV97: u8 = 0;
    pub const DWT_REV53: u8 = 1;

    /// A main-header COD object (`param_cod()` with `top_cod == NULL`).
    pub fn new() -> Self {
        ParamCod {
            type_: CodType::CodMain,
            lcod: 0,
            scod: 0,
            sgcod: CodSGcod::default(),
            spcod: CodSPcod::default(),
            atk: ParamAtk::new(),
            comp_idx: Self::OJPH_COD_DEFAULT,
            children: Vec::new(),
            top_num_decomp: 0,
            top_mc_trans: 0,
            spare: Spare::new(),
        }
    }

    /// A COC object for `top_cod` (`param_cod(top_cod, comp_idx)`).
    pub fn new_coc(top_cod: &ParamCod, comp_idx: u16) -> Self {
        let mut p = Self::new();
        p.make_coc(top_cod, comp_idx);
        p
    }

    /// Turns `self`, in the state `new` gives, into what `new_coc` gives.
    fn make_coc(&mut self, top_cod: &ParamCod, comp_idx: u16) {
        self.type_ = CodType::CocMain;
        self.comp_idx = comp_idx;
        self.top_num_decomp = top_cod.spcod.num_decomp;
        self.top_mc_trans = top_cod.sgcod.mc_trans;
    }

    /// What `new_coc(self, comp_idx)` gives, in the storage of a spare COC
    /// object if there is one.
    fn take_coc(&mut self, comp_idx: u16) -> ParamCod {
        match self.spare.take() {
            Some(mut p) => {
                p.recycle();
                p.make_coc(self, comp_idx);
                p
            }
            None => Self::new_coc(self, comp_idx),
        }
    }

    /// Gives `self` the state `new` gives, but keeps its storage: the COC
    /// objects go to the spares, and the kernel keeps its steps' storage.
    pub(crate) fn recycle(&mut self) {
        let mut children = std::mem::take(&mut self.children);
        let mut spare = std::mem::take(&mut self.spare);
        spare.put_all(&mut children);
        let mut atk = std::mem::take(&mut self.atk);
        atk.restart();
        *self = Self::new();
        self.children = children;
        self.spare = spare;
        self.atk = atk;
    }

    /// Removes the COC objects and returns every parameter to its
    /// default, but keeps the object's type and, on a COC object, the
    /// values copied from its COD object.
    pub fn restart(&mut self) {
        let t = self.type_;
        let tn = self.top_num_decomp;
        let tm = self.top_mc_trans;
        self.recycle();
        self.type_ = t;
        self.top_num_decomp = tn;
        self.top_mc_trans = tm;
    }

    /// Refreshes the parent values each COC object keeps.
    fn sync_children(&mut self) {
        let n = self.spcod.num_decomp;
        let m = self.sgcod.mc_trans;
        for c in &mut self.children {
            c.top_num_decomp = n;
            c.top_mc_trans = m;
        }
    }

    pub fn get_type(&self) -> CodType {
        self.type_
    }

    pub fn set_reversible(&mut self, reversible: bool) {
        self.spcod.wavelet_trans = if reversible {
            Self::DWT_REV53
        } else {
            Self::DWT_IRV97
        };
    }

    pub fn employ_color_transform(&mut self, val: u8) {
        // C++ asserts val is 0 or 1 and type is UNDEFINED or COD_MAIN
        self.sgcod.mc_trans = val;
        self.sync_children();
    }

    pub fn check_validity(&self, siz: &ParamSiz) -> Result<()> {
        // check that colour transform and match number of components and
        // downsampling
        let num_comps = siz.get_num_components() as i32;
        if self.sgcod.mc_trans == 1 && num_comps < 3 {
            return ojph_error!(
                0x00040011,
                "color transform can only be employed when the image has 3 or more color components"
            );
        }

        if self.sgcod.mc_trans == 1 {
            let mut test_signedness = false;
            let mut test_bit_depth = false;
            let mut test_downsampling = false;
            let p = siz.get_downsampling(0);
            let bit_depth = siz.get_bit_depth(0);
            let is_signed = siz.is_signed(0);
            for i in 1..3u32 {
                let p1 = siz.get_downsampling(i);
                test_downsampling = test_downsampling || (p.x != p1.x || p.y != p1.y);
                test_bit_depth = test_bit_depth || (bit_depth != siz.get_bit_depth(i));
                test_signedness = test_signedness || (is_signed != siz.is_signed(i));
            }
            if test_downsampling {
                return ojph_error!(
                    0x00040012,
                    "when color transform is used, the first 3 colour components must have the same downsampling factor."
                );
            }
            if test_bit_depth {
                return ojph_error!(
                    0x00040014,
                    "when color transform is used, the first 3 colour components must have the same bit depth."
                );
            }
            if test_signedness {
                return ojph_error!(
                    0x00040015,
                    "when color transform is used, the first 3 colour components must have the same signedness (signed or unsigned)."
                );
            }
        }

        // check the progression order matches downsampling
        if self.sgcod.prog_order as i32 == OJPH_PO_RPCL
            || self.sgcod.prog_order as i32 == OJPH_PO_PCRL
        {
            let num_comps = siz.get_num_components();
            for i in 0..num_comps {
                let r = siz.get_downsampling(i);
                if (r.x & r.x.wrapping_sub(1)) != 0 || (r.y & r.y.wrapping_sub(1)) != 0 {
                    return ojph_error!(
                        0x00040013,
                        "For RPCL and PCRL progression orders,component downsampling factors have to be powers of 2"
                    );
                }
            }
        }
        Ok(())
    }

    /// C++ returns `ui8`; `u32` here as every caller widens it.
    pub fn get_num_decompositions(&self) -> u32 {
        match self.type_ {
            CodType::CodMain => self.spcod.num_decomp as u32,
            CodType::CocMain => {
                if self.is_dfs_defined() {
                    self.top_num_decomp as u32
                } else {
                    self.spcod.num_decomp as u32
                }
            }
            // C++: assert(0); return 0
            _ => 0,
        }
    }

    pub fn get_block_dims(&self) -> Size {
        self.spcod.get_block_dims()
    }

    pub fn get_log_block_dims(&self) -> Size {
        self.spcod.get_log_block_dims()
    }

    pub fn get_wavelet_kern(&self) -> u8 {
        self.spcod.wavelet_trans
    }

    /// For a kernel index above 1 this needs `update_atk` to have run; C++
    /// asserts that, and this returns false if it has not.
    pub fn is_reversible(&self) -> bool {
        if self.spcod.wavelet_trans <= 1 {
            self.get_wavelet_kern() == Self::DWT_REV53
        } else {
            self.atk.is_reversible()
        }
    }

    pub fn is_employing_color_transform(&self) -> bool {
        if self.type_ == CodType::CodMain || self.type_ == CodType::CodTile {
            self.sgcod.mc_trans == 1
        } else {
            self.top_mc_trans == 1
        }
    }

    pub fn get_precinct_size(&self, res_num: u32) -> Size {
        let t = self.get_log_precinct_size(res_num);
        Size::new(1u32 << t.w, 1u32 << t.h)
    }

    pub fn get_log_precinct_size(&self, res_num: u32) -> Size {
        if self.scod & 1 != 0 {
            self.spcod.get_log_precinct_size(res_num)
        } else {
            Size::new(15, 15)
        }
    }

    pub fn packets_may_use_sop(&self) -> bool {
        if self.type_ == CodType::CodMain || self.type_ == CodType::CodTile {
            return (self.scod & 2) == 2;
        }
        false
    }

    pub fn packets_use_eph(&self) -> bool {
        if self.type_ == CodType::CodMain || self.type_ == CodType::CodTile {
            return (self.scod & 4) == 4;
        }
        false
    }

    pub fn get_block_vertical_causality(&self) -> bool {
        (self.spcod.block_style & Self::VERT_CAUSAL_MODE) != 0
    }

    /// The resolved kernel; an empty kernel (no steps, irreversible)
    /// before `update_atk` (C++: NULL).
    pub fn access_atk(&self) -> &ParamAtk {
        &self.atk
    }

    // ------------------------------------------------------------------
    // COC only

    pub fn is_dfs_defined(&self) -> bool {
        (self.spcod.num_decomp & 0x80) != 0
    }

    /// cannot be more than 15
    pub fn get_dfs_index(&self) -> u16 {
        (self.spcod.num_decomp & 0xF) as u16
    }

    pub fn get_comp_idx(&self) -> u32 {
        self.comp_idx as u32
    }

    // ------------------------------------------------------------------
    // writing and reading

    pub fn write(&mut self, file: &mut Vec<u8>) -> bool {
        // C++ asserts type == COD_MAIN
        // marker size excluding header
        self.lcod = 12;
        self.lcod = (self.lcod as u32
            + if self.scod & 1 != 0 {
                1 + self.spcod.num_decomp as u32
            } else {
                0
            }) as u16;

        put_u16(file, jp2k_marker::COD);
        put_u16(file, self.lcod);
        put_u8(file, self.scod);
        put_u8(file, self.sgcod.prog_order);
        put_u16(file, self.sgcod.num_layers);
        put_u8(file, self.sgcod.mc_trans);
        put_u8(file, self.spcod.num_decomp);
        put_u8(file, self.spcod.block_width);
        put_u8(file, self.spcod.block_height);
        put_u8(file, self.spcod.block_style);
        put_u8(file, self.spcod.wavelet_trans);
        if self.scod & 1 != 0 {
            for i in 0..=self.spcod.num_decomp as usize {
                // C++ would read past the array for num_decomp > 32
                put_u8(file, self.spcod.precinct_size.get(i).copied().unwrap_or(0));
            }
        }
        true
    }

    pub fn write_coc(&mut self, file: &mut Vec<u8>, num_comps: u32) -> bool {
        // C++ asserts type == COD_MAIN
        let mut result = true;
        for p in &mut self.children {
            if (p.comp_idx as u32) < num_comps {
                result &= p.internal_write_coc(file, num_comps);
            }
        }
        result
    }

    fn internal_write_coc(&mut self, file: &mut Vec<u8>, num_comps: u32) -> bool {
        // C++ asserts type == COC_MAIN
        // marker size excluding header
        self.lcod = if num_comps < 257 { 9 } else { 10 };
        self.lcod = (self.lcod as u32
            + if self.scod & 1 != 0 {
                1 + self.spcod.num_decomp as u32
            } else {
                0
            }) as u16;

        put_u16(file, jp2k_marker::COC);
        put_u16(file, self.lcod);
        if num_comps < 257 {
            put_u8(file, self.comp_idx as u8);
        } else {
            put_u16(file, self.comp_idx);
        }
        put_u8(file, self.scod);
        put_u8(file, self.spcod.num_decomp);
        put_u8(file, self.spcod.block_width);
        put_u8(file, self.spcod.block_height);
        put_u8(file, self.spcod.block_style);
        put_u8(file, self.spcod.wavelet_trans);
        if self.scod & 1 != 0 {
            for i in 0..=self.spcod.num_decomp as usize {
                put_u8(file, self.spcod.precinct_size.get(i).copied().unwrap_or(0));
            }
        }
        true
    }

    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        // C++ asserts type == COD_MAIN
        macro_rules! rd {
            ($e:expr, $code:expr) => {
                match $e {
                    Some(v) => v,
                    None => return ojph_error!($code, "error reading COD segment"),
                }
            };
        }
        self.lcod = rd!(file.read_u16(), 0x00050071);
        self.scod = rd!(file.read_u8(), 0x00050072);
        self.sgcod.prog_order = rd!(file.read_u8(), 0x00050073);
        self.sgcod.num_layers = rd!(file.read_u16(), 0x00050074);
        self.sgcod.mc_trans = rd!(file.read_u8(), 0x00050075);
        self.spcod.num_decomp = rd!(file.read_u8(), 0x00050076);
        self.spcod.block_width = rd!(file.read_u8(), 0x00050077);
        self.spcod.block_height = rd!(file.read_u8(), 0x00050078);
        self.spcod.block_style = rd!(file.read_u8(), 0x00050079);
        self.spcod.wavelet_trans = rd!(file.read_u8(), 0x0005007A);
        self.sync_children();

        if self.get_num_decompositions() > 32
            || self.spcod.block_width > 8
            || self.spcod.block_height > 8
            || self.spcod.block_width as u32 + self.spcod.block_height as u32 > 8
            || (self.spcod.block_style & 0x40) != 0x40
            || (self.spcod.block_style & 0xB7) != 0x00
        {
            return ojph_error!(0x0005007D, "wrong settings in a COD-SPcod parameter");
        }
        if (self.spcod.block_style & 0x40) != 0x40 || (self.spcod.block_style & 0xB7) != 0x00 {
            return ojph_error!(0x0005007E, "unsupported settings in a COD-SPcod parameter");
        }

        let num_decompositions = self.get_num_decompositions();
        if self.scod & 1 != 0 {
            for i in 0..=num_decompositions as usize {
                let v = rd!(file.read_u8(), 0x0005007B);
                self.spcod.precinct_size[i] = v;
                if i != 0 && ((v & 0x0F) == 0 || (v >> 4) == 0) {
                    return ojph_error!(
                        0x0005007F,
                        "Precinct width or height for resolutions other than the coarsest must be larger than 1; here, they are {} and {}, respectively.",
                        1u32 << (v & 0x0F),
                        1u32 << (v >> 4)
                    );
                }
            }
        }
        let expected = 12u32
            + if self.scod & 1 != 0 {
                1 + self.spcod.num_decomp as u32
            } else {
                0
            };
        if self.lcod as u32 != expected {
            return ojph_error!(0x0005007C, "error in COD segment length");
        }
        Ok(())
    }

    /// Reads a COC segment into this (detached) COC object; `top_cod` is
    /// the main COD. This is the C++ `read_coc` member of the COC object;
    /// the main object's [`Self::read_coc`] calls it.
    pub fn read_coc_segment(
        &mut self,
        file: &mut MemInfile,
        num_comps: u32,
        top_cod: &ParamCod,
    ) -> Result<()> {
        // C++ asserts type == COC_MAIN and top_cod != NULL
        self.sgcod = top_cod.sgcod;
        self.top_num_decomp = top_cod.spcod.num_decomp;
        self.top_mc_trans = top_cod.sgcod.mc_trans;
        macro_rules! rd {
            ($e:expr, $code:expr) => {
                match $e {
                    Some(v) => v,
                    None => return ojph_error!($code, "error reading COC segment"),
                }
            };
        }
        self.lcod = rd!(file.read_u16(), 0x00050121);
        if num_comps < 257 {
            let t = rd!(file.read_u8(), 0x00050122);
            self.comp_idx = t as u16;
        } else {
            self.comp_idx = rd!(file.read_u16(), 0x00050123);
        }
        self.scod = rd!(file.read_u8(), 0x00050124);
        // OJPH_WARN 0x00050011 if Scod & 0xF8: unsupported options in Scoc
        self.spcod.num_decomp = rd!(file.read_u8(), 0x00050125);
        self.spcod.block_width = rd!(file.read_u8(), 0x00050126);
        self.spcod.block_height = rd!(file.read_u8(), 0x00050127);
        self.spcod.block_style = rd!(file.read_u8(), 0x00050128);
        self.spcod.wavelet_trans = rd!(file.read_u8(), 0x00050129);

        if self.get_num_decompositions() > 32
            || self.spcod.block_width > 8
            || self.spcod.block_height > 8
            || self.spcod.block_width as u32 + self.spcod.block_height as u32 > 8
            || (self.spcod.block_style & 0x40) != 0x40
            || (self.spcod.block_style & 0xB7) != 0x00
        {
            return ojph_error!(0x0005012C, "wrong settings in a COC-SPcoc parameter");
        }
        if (self.spcod.block_style & 0x40) != 0x40 || (self.spcod.block_style & 0xB7) != 0x00 {
            return ojph_error!(0x0005012D, "unsupported settings in a COC-SPcoc parameter");
        }

        let num_decompositions = self.get_num_decompositions();
        if self.scod & 1 != 0 {
            for i in 0..=num_decompositions as usize {
                let v = rd!(file.read_u8(), 0x0005012A);
                self.spcod.precinct_size[i] = v;
                if i != 0 && ((v & 0x0F) == 0 || (v >> 4) == 0) {
                    return ojph_error!(
                        0x0005012E,
                        "Precinct width or height for resolutions other than the coarsest must be larger than 1; here, they are {} and {}, respectively.",
                        1u32 << (v & 0x0F),
                        1u32 << (v >> 4)
                    );
                }
            }
        }
        let mut t = 9u32;
        t += if num_comps < 257 { 0 } else { 1 };
        t += if self.scod & 1 != 0 {
            1 + num_decompositions
        } else {
            0
        };
        if self.lcod as u32 != t {
            return ojph_error!(0x0005012B, "error in COC segment length");
        }
        Ok(())
    }

    /// What the C++ codestream does on a COC marker, called on the main
    /// COD object: `p = cod.add_coc_object(OJPH_COD_UNKNOWN);
    /// p->read_coc(file, num_comps, &cod);` followed by the codestream's
    /// checks (`ojph_codestream_local.cpp`): OJPH_INFO 0x00030056 for a
    /// component index beyond `num_comps` (dropped), and error 0x00030057
    /// when an earlier COC has the same component index. As in C++, the
    /// object stays appended even when reading fails.
    pub fn read_coc(&mut self, file: &mut MemInfile, num_comps: u32) -> Result<()> {
        let mut p = self.take_coc(Self::OJPH_COD_UNKNOWN);
        let r = p.read_coc_segment(file, num_comps, self);
        self.children.push(p);
        r?;
        let child = self.children.len() - 1;
        if self.is_duplicate_coc(child) {
            return ojph_error!(
                0x00030057,
                "The codestream has two COC marker segments for one component of index {}",
                self.children[child].comp_idx
            );
        }
        Ok(())
    }

    /// True when `get_coc(comp_idx)` would find a COC object other than
    /// the one at `children[child]` with the same index: the C++ test
    /// `p != q && p->get_comp_idx() == q->get_comp_idx()` after reading.
    pub fn is_duplicate_coc(&self, child: usize) -> bool {
        let idx = match self.children.get(child) {
            Some(c) => c.comp_idx,
            None => return false,
        };
        if idx == self.comp_idx {
            // get_coc finds this main object first (OJPH_COD_DEFAULT)
            return true;
        }
        match self.children.iter().position(|c| c.comp_idx == idx) {
            Some(first) => first != child,
            None => false,
        }
    }

    /// The COC objects, in the order they were added.
    pub fn cocs(&self) -> &[ParamCod] {
        &self.children
    }

    /// C++ takes a `param_atk*` and stores pointers into it; here each
    /// COD/COC object stores a copy of the kernel it resolves to. C++
    /// `get_atk` may add the default 9/7 or 5/3 kernel to the ATK chain;
    /// here `atk` is left unchanged and the copy is made directly
    /// ([`ParamAtk::resolve_atk`]), which gives the same kernels.
    pub fn update_atk(&mut self, atk: &ParamAtk) -> Result<()> {
        // C++ asserts type == COD_MAIN
        if !atk.resolve_atk_into(self.spcod.wavelet_trans as i32, &mut self.atk) {
            return ojph_error!(
                0x00050131,
                "A COD segment employs the DWT kernel atk = {}, but a corresponding ATK segment cannot be found.",
                self.spcod.wavelet_trans
            );
        }
        let top_wt = self.spcod.wavelet_trans;
        for p in &mut self.children {
            if !atk.resolve_atk_into(p.spcod.wavelet_trans as i32, &mut p.atk) {
                // C++ prints the COD's kernel index here, not the COC's
                return ojph_error!(
                    0x00050132,
                    "A COC segment employs the DWT kernel atk = {}, but a corresponding ATK segment cannot be found",
                    top_wt
                );
            }
        }
        Ok(())
    }

    /// The COC object for `comp_idx`, or this COD object if there is none.
    /// As in C++ the search starts with this object, whose own index is
    /// `OJPH_COD_DEFAULT`. On a COC object (which has no children here)
    /// this returns itself; C++ would search its parent's list.
    pub fn get_coc(&self, comp_idx: u32) -> &ParamCod {
        if self.comp_idx as u32 == comp_idx {
            return self;
        }
        for p in &self.children {
            if p.comp_idx as u32 == comp_idx {
                return p;
            }
        }
        self
    }

    pub fn get_coc_mut(&mut self, comp_idx: u32) -> &mut ParamCod {
        if self.comp_idx as u32 == comp_idx {
            return self;
        }
        match self
            .children
            .iter()
            .position(|p| p.comp_idx as u32 == comp_idx)
        {
            Some(i) => &mut self.children[i],
            None => self,
        }
    }

    pub fn add_coc_object(&mut self, comp_idx: u32) -> &mut ParamCod {
        // C++ asserts type == COD_MAIN
        let p = self.take_coc(comp_idx as u16);
        self.children.push(p);
        self.children.last_mut().unwrap()
    }

    pub fn get_or_add_coc(&mut self, comp_idx: u32) -> &mut ParamCod {
        // C++: p = get_coc(comp_idx); if (p == this) p = add_coc_object(...)
        match self
            .children
            .iter()
            .position(|p| p.comp_idx as u32 == comp_idx)
        {
            Some(i) if self.comp_idx as u32 != comp_idx => &mut self.children[i],
            _ => self.add_coc_object(comp_idx),
        }
    }

    // ------------------------------------------------------------------
    // public API (`ojph::param_cod`)

    pub fn set_num_decomposition(&mut self, num_decompositions: u32) -> Result<()> {
        if num_decompositions > 32 {
            return ojph_error!(
                0x00050001,
                "maximum number of decompositions cannot exceed 32"
            );
        }
        self.spcod.num_decomp = num_decompositions as u8;
        self.sync_children();
        Ok(())
    }

    pub fn set_block_dims(&mut self, width: u32, height: u32) -> Result<()> {
        let log_width = log2_floor(width);
        let log_height = log2_floor(height);
        if width == 0
            || width != 1u32.wrapping_shl(log_width)
            || height == 0
            || height != 1u32.wrapping_shl(log_height)
            || log_width < 2
            || log_height < 2
            || log_width + log_height > 12
        {
            return ojph_error!(0x00050011, "incorrect code block dimensions");
        }
        self.spcod.block_width = (log_width - 2) as u8;
        self.spcod.block_height = (log_height - 2) as u8;
        Ok(())
    }

    /// C++ takes `(int num_levels, size* precinct_size)`; an empty slice is
    /// the `num_levels == 0 || precinct_size == NULL` case.
    pub fn set_precinct_size(&mut self, precinct_size: &[Size]) -> Result<()> {
        let num_levels = precinct_size.len();
        if num_levels == 0 {
            self.scod &= 0xFE;
        } else {
            self.scod |= 1;
            for i in 0..=self.spcod.num_decomp as usize {
                let t = precinct_size[if i < num_levels { i } else { num_levels - 1 }];
                let ppx = log2_floor(t.w);
                let ppy = log2_floor(t.h);
                if t.w == 0 || t.h == 0 {
                    return ojph_error!(0x00050021, "precinct width or height cannot be 0");
                }
                if t.w != (1u32 << ppx) || t.h != (1u32 << ppy) {
                    return ojph_error!(
                        0x00050022,
                        "precinct width and height should be a power of 2"
                    );
                }
                if ppx > 15 || ppy > 15 {
                    return ojph_error!(0x00050023, "precinct size is too large");
                }
                if i > 0 && (ppx == 0 || ppy == 0) {
                    return ojph_error!(0x00050024, "precinct size is too small");
                }
                if i < self.spcod.precinct_size.len() {
                    self.spcod.precinct_size[i] = (ppx | (ppy << 4)) as u8;
                }
            }
        }
        Ok(())
    }

    pub fn set_progression_order(&mut self, name: &str) -> Result<()> {
        let prog_order;
        if name.len() == 4 {
            if name == OJPH_PO_STRING_LRCP {
                prog_order = OJPH_PO_LRCP;
            } else if name == OJPH_PO_STRING_RLCP {
                prog_order = OJPH_PO_RLCP;
            } else if name == OJPH_PO_STRING_RPCL {
                prog_order = OJPH_PO_RPCL;
            } else if name == OJPH_PO_STRING_PCRL {
                prog_order = OJPH_PO_PCRL;
            } else if name == OJPH_PO_STRING_CPRL {
                prog_order = OJPH_PO_CPRL;
            } else {
                return ojph_error!(0x00050031, "unknown progression order");
            }
        } else {
            return ojph_error!(0x00050032, "improper progression order");
        }
        self.sgcod.prog_order = prog_order as u8;
        Ok(())
    }

    pub fn set_color_transform(&mut self, color_transform: bool) {
        self.employ_color_transform(if color_transform { 1 } else { 0 });
    }

    /// `set_num_decomposition(ui32 comp_idx, ui32 num_decompositions)`
    pub fn set_num_decomposition_comp(
        &mut self,
        comp_idx: u32,
        num_decompositions: u32,
    ) -> Result<()> {
        self.get_or_add_coc(comp_idx)
            .set_num_decomposition(num_decompositions)
    }

    /// `set_block_dims(ui32 comp_idx, ui32 width, ui32 height)`
    pub fn set_block_dims_comp(&mut self, comp_idx: u32, width: u32, height: u32) -> Result<()> {
        self.get_or_add_coc(comp_idx).set_block_dims(width, height)
    }

    /// `set_precinct_size(ui32 comp_idx, int num_levels, size*)`
    pub fn set_precinct_size_comp(&mut self, comp_idx: u32, precinct_size: &[Size]) -> Result<()> {
        self.get_or_add_coc(comp_idx)
            .set_precinct_size(precinct_size)
    }

    /// `set_reversible(ui32 comp_idx, bool reversible)`
    pub fn set_reversible_comp(&mut self, comp_idx: u32, reversible: bool) {
        self.get_or_add_coc(comp_idx).set_reversible(reversible)
    }

    pub fn get_progression_order(&self) -> i32 {
        self.sgcod.prog_order as i32
    }

    /// [`Self::get_progression_order`] as an enum; `None` for a value read
    /// from a codestream that is not one of the five orders.
    pub fn get_progression_order_enum(&self) -> Option<ProgressionOrder> {
        ProgressionOrder::from_i32(self.sgcod.prog_order as i32)
    }

    /// C++ asserts on an unknown value and returns "".
    pub fn get_progression_order_as_string(&self) -> &'static str {
        match self.sgcod.prog_order as i32 {
            OJPH_PO_LRCP => OJPH_PO_STRING_LRCP,
            OJPH_PO_RLCP => OJPH_PO_STRING_RLCP,
            OJPH_PO_RPCL => OJPH_PO_STRING_RPCL,
            OJPH_PO_PCRL => OJPH_PO_STRING_PCRL,
            OJPH_PO_CPRL => OJPH_PO_STRING_CPRL,
            _ => "",
        }
    }

    pub fn get_num_layers(&self) -> i32 {
        self.sgcod.num_layers as i32
    }

    pub fn is_using_color_transform(&self) -> bool {
        self.is_employing_color_transform()
    }

    /// `get_num_decompositions(ui32 comp_idx)`
    pub fn get_num_decompositions_comp(&self, comp_idx: u32) -> u32 {
        self.get_coc(comp_idx).get_num_decompositions()
    }
    /// `get_block_dims(ui32 comp_idx)`
    pub fn get_block_dims_comp(&self, comp_idx: u32) -> Size {
        self.get_coc(comp_idx).get_block_dims()
    }
    /// `get_log_block_dims(ui32 comp_idx)`
    pub fn get_log_block_dims_comp(&self, comp_idx: u32) -> Size {
        self.get_coc(comp_idx).get_log_block_dims()
    }
    /// `is_reversible(ui32 comp_idx)`
    pub fn is_reversible_comp(&self, comp_idx: u32) -> bool {
        self.get_coc(comp_idx).is_reversible()
    }
    /// `get_precinct_size(ui32 comp_idx, ui32 level_num)`
    pub fn get_precinct_size_comp(&self, comp_idx: u32, level_num: u32) -> Size {
        self.get_coc(comp_idx).get_precinct_size(level_num)
    }
    /// `get_log_precinct_size(ui32 comp_idx, ui32 level_num)`
    pub fn get_log_precinct_size_comp(&self, comp_idx: u32, level_num: u32) -> Size {
        self.get_coc(comp_idx).get_log_precinct_size(level_num)
    }
    /// `get_block_vertical_causality(ui32 comp_idx)`
    pub fn get_block_vertical_causality_comp(&self, comp_idx: u32) -> bool {
        self.get_coc(comp_idx).get_block_vertical_causality()
    }
}
