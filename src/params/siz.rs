// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The SIZ marker segment (`param_siz`, `siz_comp_info`).

use super::cod::ParamCod;
use super::dfs::ParamDfs;
use super::markers::{jp2k_marker, put_u16, put_u32, put_u8};
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::types::{div_ceil, Point, Size};
use crate::Result;

/// `siz_comp_info`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizCompInfo {
    pub ssiz: u8,
    pub xrsiz: u8,
    pub yrsiz: u8,
}

/// `param_siz`: image and tile size (SIZ).
///
/// The C++ object keeps pointers to the COD and DFS objects (`link`,
/// `set_cod`), used only by `get_recon_*`; here `link`/`set_cod` store
/// copies of them, so call them again if those objects change.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSiz {
    pub(crate) lsiz: u16,
    pub(crate) rsiz: u16,
    pub(crate) xsiz: u32,
    pub(crate) ysiz: u32,
    pub(crate) xosiz: u32,
    pub(crate) yosiz: u32,
    pub(crate) xtsiz: u32,
    pub(crate) ytsiz: u32,
    pub(crate) xtosiz: u32,
    pub(crate) ytosiz: u32,
    pub(crate) csiz: u16,
    pub(crate) cptr: Vec<SizCompInfo>,
    pub(crate) skipped_resolutions: u32,
    pub(crate) ws_kern_support_needed: bool,
    pub(crate) dfs_support_needed: bool,
    /// copy of the linked COD (C++: `const param_cod* cod`)
    pub(crate) cod: Option<Box<ParamCod>>,
    /// copy of the linked DFS (C++: `const param_dfs* dfs`)
    pub(crate) dfs: Option<Box<ParamDfs>>,
}

impl Default for ParamSiz {
    fn default() -> Self {
        Self::new()
    }
}

impl ParamSiz {
    pub const RSIZ_NLT_FLAG: u16 = 0x200;
    pub const RSIZ_HT_FLAG: u16 = 0x4000;
    pub const RSIZ_EXT_FLAG: u16 = 0x8000;

    pub fn new() -> Self {
        ParamSiz {
            lsiz: 0,
            rsiz: Self::RSIZ_HT_FLAG,
            xsiz: 0,
            ysiz: 0,
            xosiz: 0,
            yosiz: 0,
            xtsiz: 0,
            ytsiz: 0,
            xtosiz: 0,
            ytosiz: 0,
            csiz: 0,
            cptr: Vec::new(),
            skipped_resolutions: 0,
            ws_kern_support_needed: false,
            dfs_support_needed: false,
            cod: None,
            dfs: None,
        }
    }

    pub fn init(&mut self) {
        let mut cptr = std::mem::take(&mut self.cptr);
        cptr.clear();
        *self = Self::new();
        self.cptr = cptr;
    }

    pub fn set_num_components(&mut self, num_comps: u32) {
        self.csiz = num_comps as u16;
        self.cptr.clear();
        self.cptr.resize(self.csiz as usize, SizCompInfo::default());
    }

    /// Both `local::param_siz::set_comp_info` and the public
    /// `param_siz::set_component`.
    ///
    /// The C++ asserts `comp_num < Csiz` and a non-zero downsampling, and
    /// truncates the values to the 8-bit SIZ fields. Here each of these is
    /// an error instead: a `comp_num` that is not a component
    /// (0x0005005A), a downsampling factor outside 1 to 255 (0x0005005B),
    /// or a bit depth outside 1 to 38, the range SIZ can carry and the
    /// reader accepts (0x0005005C).
    pub fn set_comp_info(
        &mut self,
        comp_num: u32,
        downsampling: Point,
        bit_depth: u32,
        is_signed: bool,
    ) -> Result<()> {
        if comp_num >= self.cptr.len() as u32 {
            return ojph_error!(
                0x0005005A,
                "component {} does not exist; the image has {} components",
                comp_num,
                self.cptr.len()
            );
        }
        if !(1..=255).contains(&downsampling.x) || !(1..=255).contains(&downsampling.y) {
            return ojph_error!(
                0x0005005B,
                "downsampling factors must be between 1 and 255, not {}x{}",
                downsampling.x,
                downsampling.y
            );
        }
        if !(1..=38).contains(&bit_depth) {
            return ojph_error!(
                0x0005005C,
                "bit depth must be between 1 and 38, not {}",
                bit_depth
            );
        }
        let c = &mut self.cptr[comp_num as usize];
        c.ssiz = (bit_depth
            .wrapping_sub(1)
            .wrapping_add(if is_signed { 0x80 } else { 0 })) as u8;
        c.xrsiz = downsampling.x as u8;
        c.yrsiz = downsampling.y as u8;
        Ok(())
    }

    /// The public-API name of [`Self::set_comp_info`].
    pub fn set_component(
        &mut self,
        comp_num: u32,
        downsampling: Point,
        bit_depth: u32,
        is_signed: bool,
    ) -> Result<()> {
        self.set_comp_info(comp_num, downsampling, bit_depth, is_signed)
    }

    pub fn set_image_extent(&mut self, dims: Point) {
        self.xsiz = dims.x;
        self.ysiz = dims.y;
    }
    pub fn get_image_extent(&self) -> Point {
        Point::new(self.xsiz, self.ysiz)
    }
    pub fn set_tile_size(&mut self, s: Size) {
        self.xtsiz = s.w;
        self.ytsiz = s.h;
    }
    pub fn get_tile_size(&self) -> Size {
        Size::new(self.xtsiz, self.ytsiz)
    }
    pub fn set_image_offset(&mut self, offset: Point) {
        self.xosiz = offset.x;
        self.yosiz = offset.y;
    }
    pub fn get_image_offset(&self) -> Point {
        Point::new(self.xosiz, self.yosiz)
    }
    pub fn set_tile_offset(&mut self, offset: Point) {
        self.xtosiz = offset.x;
        self.ytosiz = offset.y;
    }
    pub fn get_tile_offset(&self) -> Point {
        Point::new(self.xtosiz, self.ytosiz)
    }

    pub fn check_validity(&self) -> Result<()> {
        if self.xsiz == 0 || self.ysiz == 0 || self.xtsiz == 0 || self.ytsiz == 0 {
            return ojph_error!(0x00040001, "Image extent and/or tile size cannot be zero");
        }
        if self.xtosiz > self.xosiz || self.ytosiz > self.yosiz {
            return ojph_error!(
                0x00040002,
                "Tile offset has to be smaller than the image offset"
            );
        }
        // C++ adds in ui32 and may wrap
        if self.xtsiz.wrapping_add(self.xtosiz) <= self.xosiz
            || self.ytsiz.wrapping_add(self.ytosiz) <= self.yosiz
        {
            return ojph_error!(
                0x00040003,
                "The top left tile must intersect with the image"
            );
        }
        if self.xsiz <= self.xosiz || self.ysiz <= self.yosiz {
            return ojph_error!(
                0x00040004,
                "The image extent must be larger than the image offset"
            );
        }
        Ok(())
    }

    /// C++ returns `ui16`; `u32` here as every caller widens it.
    pub fn get_num_components(&self) -> u32 {
        self.csiz as u32
    }
    pub fn get_bit_depth(&self, comp_num: u32) -> u32 {
        (self.cptr[comp_num as usize].ssiz & 0x7F) as u32 + 1
    }
    pub fn is_signed(&self, comp_num: u32) -> bool {
        (self.cptr[comp_num as usize].ssiz & 0x80) != 0
    }
    pub fn get_downsampling(&self, comp_num: u32) -> Point {
        let c = &self.cptr[comp_num as usize];
        Point::new(c.xrsiz as u32, c.yrsiz as u32)
    }

    pub fn write(&mut self, file: &mut Vec<u8>) -> bool {
        // marker size excluding header
        self.lsiz = (38u32 + 3 * self.csiz as u32) as u16;
        put_u16(file, jp2k_marker::SIZ);
        put_u16(file, self.lsiz);
        put_u16(file, self.rsiz);
        put_u32(file, self.xsiz);
        put_u32(file, self.ysiz);
        put_u32(file, self.xosiz);
        put_u32(file, self.yosiz);
        put_u32(file, self.xtsiz);
        put_u32(file, self.ytsiz);
        put_u32(file, self.xtosiz);
        put_u32(file, self.ytosiz);
        put_u16(file, self.csiz);
        for c in self.cptr.iter().take(self.csiz as usize) {
            put_u8(file, c.ssiz);
            put_u8(file, c.xrsiz);
            put_u8(file, c.yrsiz);
        }
        true
    }

    pub fn read(&mut self, file: &mut MemInfile) -> Result<()> {
        self.lsiz = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x00050041, "error reading SIZ marker"),
        };
        let num_comps: i32 = (self.lsiz as i32 - 38) / 3;
        if self.lsiz as i32 != 38 + 3 * num_comps {
            return ojph_error!(0x00050042, "error in SIZ marker length");
        }
        self.rsiz = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x00050043, "error reading SIZ marker"),
        };
        if (self.rsiz & 0x4000) == 0 {
            return ojph_error!(
                0x00050044,
                "Rsiz bit 14 is not set (this is not a JPH file)"
            );
        }
        // OJPH_WARN 0x00050001: Rsiz in SIZ has unimplemented fields
        self.xsiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050045, "error reading SIZ marker"),
        };
        self.ysiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050046, "error reading SIZ marker"),
        };
        let t_xosiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050047, "error reading SIZ marker"),
        };
        let t_yosiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050048, "error reading SIZ marker"),
        };
        self.set_image_offset(Point::new(t_xosiz, t_yosiz));
        let t_xtsiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x00050049, "error reading SIZ marker"),
        };
        let t_ytsiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x0005004A, "error reading SIZ marker"),
        };
        self.set_tile_size(Size::new(t_xtsiz, t_ytsiz));
        let t_xtosiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x0005004B, "error reading SIZ marker"),
        };
        let t_ytosiz = match file.read_u32() {
            Some(v) => v,
            None => return ojph_error!(0x0005004C, "error reading SIZ marker"),
        };
        self.set_tile_offset(Point::new(t_xtosiz, t_ytosiz));
        self.csiz = match file.read_u16() {
            Some(v) => v,
            None => return ojph_error!(0x0005004D, "error reading SIZ marker"),
        };
        if self.csiz as i32 != num_comps {
            return ojph_error!(0x0005004E, "Csiz does not match the SIZ marker size");
        }
        if self.csiz == 0 {
            return ojph_error!(0x0005004F, "Wrong Csiz value of 0 in SIZ marker segment");
        }
        self.set_num_components(self.csiz as u32);
        for c in 0..self.csiz as usize {
            let ssiz = match file.read_u8() {
                Some(v) => v,
                None => return ojph_error!(0x00050051, "error reading SIZ marker"),
            };
            self.cptr[c].ssiz = ssiz;
            let xrsiz = match file.read_u8() {
                Some(v) => v,
                None => return ojph_error!(0x00050052, "error reading SIZ marker"),
            };
            self.cptr[c].xrsiz = xrsiz;
            let yrsiz = match file.read_u8() {
                Some(v) => v,
                None => return ojph_error!(0x00050053, "error reading SIZ marker"),
            };
            self.cptr[c].yrsiz = yrsiz;
            if (ssiz & 0x7F) > 37 {
                return ojph_error!(0x00050054, "Wrong SIZ-SSiz value of {}", ssiz);
            }
            if xrsiz == 0 {
                return ojph_error!(0x00050055, "Wrong SIZ-XRsiz value of {}", xrsiz);
            }
            if yrsiz == 0 {
                return ojph_error!(0x00050056, "Wrong SIZ-YRsiz value of {}", yrsiz);
            }
        }

        self.ws_kern_support_needed = (self.rsiz & 0x20) != 0;
        self.dfs_support_needed = (self.rsiz & 0x80) != 0;

        self.check_validity()
    }

    pub fn set_skipped_resolutions(&mut self, skipped_resolutions: u32) {
        self.skipped_resolutions = skipped_resolutions;
    }

    pub fn get_width(&self, comp_num: u32) -> u32 {
        let ds = self.cptr[comp_num as usize].xrsiz as u32;
        div_ceil(self.xsiz, ds).wrapping_sub(div_ceil(self.xosiz, ds))
    }

    pub fn get_height(&self, comp_num: u32) -> u32 {
        let ds = self.cptr[comp_num as usize].yrsiz as u32;
        div_ceil(self.ysiz, ds).wrapping_sub(div_ceil(self.yosiz, ds))
    }

    /// `set_cod(const param_cod&)`: stores a copy (C++: a pointer).
    pub fn set_cod(&mut self, cod: &ParamCod) {
        self.cod = Some(Box::new(cod.clone()));
    }

    /// C++ has `link(const param_cod*)` and `link(const param_dfs*)`; this
    /// does both (`dfs` may be `None`, as the C++ pointer may stay NULL)
    /// and stores copies.
    pub fn link(&mut self, cod: &ParamCod, dfs: Option<&ParamDfs>) {
        self.cod = Some(Box::new(cod.clone()));
        self.dfs = dfs.map(|d| Box::new(d.clone()));
    }

    /// Uses the linked COD and DFS. With no COD linked (C++ would
    /// dereference NULL) the component is treated as having no COC.
    pub fn get_recon_downsampling(&self, comp_num: u32) -> Point {
        // C++ evaluates 1u << skipped_resolutions, undefined for >= 32;
        // here such a shift gives 0.
        let f = 1u32.checked_shl(self.skipped_resolutions).unwrap_or(0);
        let mut factor = Point::new(f, f);
        if let (Some(dfs), Some(cod)) = (self.dfs.as_deref(), self.cod.as_deref()) {
            let cdp = cod.get_coc(comp_num);
            if cdp.is_dfs_defined() {
                let dfs_idx = cdp.get_dfs_index();
                if let Some(d) = dfs.get_dfs(dfs_idx) {
                    factor = d.get_res_downsamp(self.skipped_resolutions);
                }
            }
        }
        let c = &self.cptr[comp_num as usize];
        factor.x = factor.x.wrapping_mul(c.xrsiz as u32);
        factor.y = factor.y.wrapping_mul(c.yrsiz as u32);
        factor
    }

    pub fn get_recon_size(&self, comp_num: u32) -> Point {
        let factor = self.get_recon_downsampling(comp_num);
        // guard against division by zero (only possible with an absurd
        // skipped_resolutions); C++ would trap here
        let fx = factor.x.max(1);
        let fy = factor.y.max(1);
        Point::new(
            div_ceil(self.xsiz, fx).wrapping_sub(div_ceil(self.xosiz, fx)),
            div_ceil(self.ysiz, fy).wrapping_sub(div_ceil(self.yosiz, fy)),
        )
    }

    pub fn get_recon_width(&self, comp_num: u32) -> u32 {
        self.get_recon_size(comp_num).x
    }

    pub fn get_recon_height(&self, comp_num: u32) -> u32 {
        self.get_recon_size(comp_num).y
    }

    pub fn is_ws_kern_support_needed(&self) -> bool {
        self.ws_kern_support_needed
    }
    pub fn is_dfs_support_needed(&self) -> bool {
        self.dfs_support_needed
    }

    #[allow(non_snake_case)]
    pub fn set_Rsiz_flag(&mut self, flag: u16) {
        self.rsiz |= flag;
    }
    #[allow(non_snake_case)]
    pub fn reset_Rsiz_flag(&mut self, flag: u16) {
        self.rsiz &= !flag;
    }
}
