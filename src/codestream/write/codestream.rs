// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The codestream being written, after the write side of
//! `ojph_codestream.cpp` and `ojph_codestream_local.cpp`.

use super::tile::Tile;
use super::Ctx;
use crate::codestream::Budget;
use crate::error::ojph_error;
use crate::line::LineBuf;
use crate::params::marker::{COM, EOC, SOC};
use crate::params::tilepart_divisions::{
    OJPH_TILEPART_COMPONENTS, OJPH_TILEPART_MASK, OJPH_TILEPART_NO_DIVISIONS,
    OJPH_TILEPART_RESOLUTIONS,
};
use crate::params::{
    CommentExchange, ParamAtk, ParamCap, ParamCod, ParamNlt, ParamQcd, ParamSiz, ParamTlm,
    ProgressionOrder,
};
use crate::types::{div_ceil, Point, Rect, Size};
use crate::Result;

/// The default for [`Encoder::set_memory_limit`]: 4 GiB.
const DEFAULT_MEMORY_LIMIT: u64 = 1 << 32;

/// Where an [`Encoder`] is in its sequence of calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Parameters may be set; `write_headers` has not been called.
    Setup,
    /// The main header is written; lines are being exchanged.
    Headers,
    /// Every line has been pushed; the codestream can be flushed.
    Pushed,
    /// The codestream is complete.
    Flushed,
    /// A call failed part way; the encoder cannot be used further.
    Failed,
}

/// An HTJ2K codestream being encoded to memory.
///
/// The calls follow OpenJPH's `ojph::codestream` when it writes:
///
/// 1. [`Encoder::new`];
/// 2. the parameters: [`access_siz_mut`](Self::access_siz_mut),
///    [`access_cod_mut`](Self::access_cod_mut),
///    [`access_qcd_mut`](Self::access_qcd_mut) and
///    [`access_nlt_mut`](Self::access_nlt_mut), and optionally
///    [`set_planar`](Self::set_planar),
///    [`set_tilepart_divisions`](Self::set_tilepart_divisions),
///    [`request_tlm_marker`](Self::request_tlm_marker),
///    [`set_memory_limit`](Self::set_memory_limit) and
///    [`set_version_comment`](Self::set_version_comment);
/// 3. [`write_headers`](Self::write_headers), which checks the
///    parameters, builds the tiles and appends the main header to a
///    `Vec<u8>`;
/// 4. [`exchange`](Self::exchange), once per line of every component and
///    once more, which hands out each line to fill and takes it back;
/// 5. [`flush`](Self::flush), which appends the tiles and EOC.
///
/// The parameters are fixed by `write_headers`: from then on every setter
/// and every `access_*_mut` returns an error (0x0008000B) and changes
/// nothing, so the stream being written cannot be altered half way.
///
/// ```
/// use htj2k::codestream::Encoder;
/// use htj2k::types::{Point, Size};
///
/// let mut enc = Encoder::new();
/// let siz = enc.access_siz_mut()?;
/// siz.set_image_extent(Point::new(64, 32));
/// siz.set_tile_size(Size::new(64, 32));
/// siz.set_num_components(1);
/// siz.set_component(0, Point::new(1, 1), 16, true)?;
/// enc.access_cod_mut()?.set_block_dims(64, 32)?;
///
/// let mut out = Vec::new();
/// enc.write_headers(&mut out, &[])?;
/// while let Some((_comp, samples)) = enc.exchange()? {
///     samples.fill(-7);
/// }
/// enc.flush(&mut out)?;
/// # Ok::<(), htj2k::Error>(())
/// ```
#[derive(Debug)]
pub struct Encoder {
    state: State,
    planar: Option<bool>,
    tilepart_div: u32,
    need_tlm: bool,
    memory_limit: u64,
    version_comment: Option<Vec<u8>>,

    num_tiles: Size,
    num_comps: u32,
    /// The number of lines and the width of each component.
    comp_size: Vec<Size>,
    /// Tile parts in all, for TLM.
    num_tileparts: u32,
    tiles: Vec<Tile>,
    /// The lines handed out by `exchange`, one per component.
    lines: Vec<LineBuf>,
    /// True once `exchange` has handed out a line it has not taken back.
    handed_out: bool,
    cur_comp: u32,
    cur_line: u32,
    cur_tile_row: u32,
    budget: Budget,
    /// Holds the tag trees while packet headers are coded.
    precinct_scratch: Vec<u8>,

    siz: ParamSiz,
    cod: ParamCod,
    cap: ParamCap,
    qcd: ParamQcd,
    tlm: ParamTlm,
    nlt: ParamNlt,
    atk: ParamAtk,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    /// The version comment written by default: this crate's name and
    /// version, in the form of OpenJPH's `"OpenJPH Ver 0.33.0."`.
    pub const DEFAULT_VERSION_COMMENT: &'static str =
        concat!("htj2k Ver ", env!("CARGO_PKG_VERSION"), ".");

    pub fn new() -> Self {
        Encoder {
            state: State::Setup,
            planar: None,
            tilepart_div: OJPH_TILEPART_NO_DIVISIONS,
            need_tlm: false,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            version_comment: Some(Self::DEFAULT_VERSION_COMMENT.as_bytes().to_vec()),
            num_tiles: Size::default(),
            num_comps: 0,
            comp_size: Vec::new(),
            num_tileparts: 0,
            tiles: Vec::new(),
            lines: Vec::new(),
            handed_out: false,
            cur_comp: 0,
            cur_line: 0,
            cur_tile_row: 0,
            budget: Budget::new(DEFAULT_MEMORY_LIMIT),
            precinct_scratch: Vec::new(),
            siz: ParamSiz::default(),
            cod: ParamCod::default(),
            cap: ParamCap::default(),
            qcd: ParamQcd::default(),
            tlm: ParamTlm::default(),
            nlt: ParamNlt::default(),
            atk: ParamAtk::default(),
        }
    }

    /// The SIZ marker segment: image, tile and component sizes.
    pub fn access_siz(&self) -> &ParamSiz {
        &self.siz
    }

    /// The SIZ marker segment, to set before
    /// [`write_headers`](Self::write_headers).
    pub fn access_siz_mut(&mut self) -> Result<&mut ParamSiz> {
        self.check_setup()?;
        Ok(&mut self.siz)
    }

    /// The COD marker segment (and its COC children): coding style.
    pub fn access_cod(&self) -> &ParamCod {
        &self.cod
    }

    /// The COD marker segment, to set before
    /// [`write_headers`](Self::write_headers).
    pub fn access_cod_mut(&mut self) -> Result<&mut ParamCod> {
        self.check_setup()?;
        Ok(&mut self.cod)
    }

    /// The QCD marker segment (and its QCC children): quantization.
    pub fn access_qcd(&self) -> &ParamQcd {
        &self.qcd
    }

    /// The QCD marker segment, to set before
    /// [`write_headers`](Self::write_headers).
    pub fn access_qcd_mut(&mut self) -> Result<&mut ParamQcd> {
        self.check_setup()?;
        Ok(&mut self.qcd)
    }

    /// The NLT marker segments: non-linearity point transformations.
    pub fn access_nlt(&self) -> &ParamNlt {
        &self.nlt
    }

    /// The NLT marker segments, to set before
    /// [`write_headers`](Self::write_headers).
    pub fn access_nlt_mut(&mut self) -> Result<&mut ParamNlt> {
        self.check_setup()?;
        Ok(&mut self.nlt)
    }

    /// Whether lines are exchanged one component at a time (all the lines
    /// of component 0, then of component 1, ...), or one line of every
    /// component in turn. Planar cannot be used with the colour
    /// transform, and interleaved cannot be used when components differ
    /// in height (subsampling).
    ///
    /// If this is never called, [`write_headers`](Self::write_headers)
    /// follows OpenJPH: interleaved without the colour transform, and
    /// planar with it. Planar with the colour transform cannot be
    /// encoded, so in that case `write_headers` fails with 0x00030021, as
    /// it does after `set_planar(true)`; call `set_planar(false)` to use
    /// the colour transform. (OpenJPH writes a wrong image instead.)
    pub fn set_planar(&mut self, planar: bool) -> Result<()> {
        self.check_setup()?;
        self.planar = Some(planar);
        Ok(())
    }

    /// Whether the exchange is planar: as set by
    /// [`set_planar`](Self::set_planar), or, if it was never called,
    /// whether OpenJPH's rule would pick planar with the parameters as
    /// they are now (true with the colour transform).
    pub fn is_planar(&self) -> bool {
        self.planar
            .unwrap_or_else(|| self.cod.is_employing_color_transform())
    }

    /// Splits each tile into tile parts at resolutions, at components, or
    /// both. [`write_headers`](Self::write_headers) adjusts the request to
    /// the progression order, as OpenJPH does.
    pub fn set_tilepart_divisions(
        &mut self,
        at_resolutions: bool,
        at_components: bool,
    ) -> Result<()> {
        self.check_setup()?;
        let mut value = OJPH_TILEPART_NO_DIVISIONS;
        if at_resolutions {
            value |= OJPH_TILEPART_RESOLUTIONS;
        }
        if at_components {
            value |= OJPH_TILEPART_COMPONENTS;
        }
        self.tilepart_div = value;
        Ok(())
    }

    pub fn is_tilepart_division_at_resolutions(&self) -> bool {
        self.tilepart_div & OJPH_TILEPART_RESOLUTIONS != 0
    }

    pub fn is_tilepart_division_at_components(&self) -> bool {
        self.tilepart_div & OJPH_TILEPART_COMPONENTS != 0
    }

    /// Asks for a TLM marker segment, which lists the length of every tile
    /// part.
    pub fn request_tlm_marker(&mut self, needed: bool) -> Result<()> {
        self.check_setup()?;
        self.need_tlm = needed;
        Ok(())
    }

    pub fn is_tlm_requested(&self) -> bool {
        self.need_tlm
    }

    /// Limits the memory the encoder may allocate for its tiles and the
    /// coded data it holds until [`flush`](Self::flush). OpenJPH has no
    /// such limit. The default is 4 GiB.
    pub fn set_memory_limit(&mut self, bytes: u64) -> Result<()> {
        self.check_setup()?;
        self.memory_limit = bytes;
        Ok(())
    }

    /// The text of the COM marker segment that
    /// [`write_headers`](Self::write_headers) writes before the caller's
    /// comments, or `None` for no such segment. OpenJPH always writes
    /// `"OpenJPH Ver <version>."`; the default here is
    /// [`DEFAULT_VERSION_COMMENT`](Self::DEFAULT_VERSION_COMMENT). Pass
    /// OpenJPH's string to produce its bytes exactly.
    pub fn set_version_comment(&mut self, comment: Option<&str>) -> Result<()> {
        self.check_setup()?;
        self.version_comment = comment.map(|s| s.as_bytes().to_vec());
        Ok(())
    }

    /// The error (0x0008000B) that every setter and every `access_*_mut`
    /// returns once [`write_headers`](Self::write_headers) has been
    /// called, whether it succeeded or not. Such a call changes nothing.
    /// (OpenJPH lets these calls change the stream being written.)
    fn check_setup(&self) -> Result<()> {
        if self.state != State::Setup {
            return ojph_error!(
                0x0008000B,
                "the parameters can only be set before write_headers is called"
            );
        }
        Ok(())
    }

    /// Checks the parameters, builds the tiles and appends the main
    /// header, from SOC to the last COM marker segment, to `out`.
    /// `comments` are written as COM marker segments after the version
    /// comment.
    ///
    /// Nothing is appended if this fails.
    pub fn write_headers(&mut self, out: &mut Vec<u8>, comments: &[CommentExchange]) -> Result<()> {
        if self.state != State::Setup {
            return ojph_error!(0x00080001, "write_headers can only be called once");
        }
        // A failure leaves parameters half finalised; do not allow a retry.
        self.state = State::Failed;
        // CommentExchange::set_string checks the length it had before,
        // as OpenJPH does, so it can hold 65532 to 65535 bytes; Lcom
        // would wrap and the header would be corrupt.
        if let Some(i) = comments.iter().position(|c| c.len() > 65531) {
            return ojph_error!(
                0x0008000A,
                "comment {} is {} bytes long; a COM marker segment holds at most 65531",
                i,
                comments[i].len()
            );
        }
        self.prepare()?;
        self.write_main_header(out, comments);
        self.state = State::Headers;
        Ok(())
    }

    /// Everything `write_headers` does before it writes: the parameter
    /// checks and finalisation, and the tile structure.
    fn prepare(&mut self) -> Result<()> {
        let num_comps = self.siz.get_num_components();
        if num_comps == 0 {
            return ojph_error!(0x00080002, "the image must have at least one component");
        }
        for c in 0..num_comps {
            let d = self.siz.get_downsampling(c);
            if d.x == 0 || d.y == 0 {
                return ojph_error!(
                    0x00080003,
                    "component {} has no downsampling factor; set_component was not called for it",
                    c
                );
            }
        }

        // finalize
        self.siz.set_cod(&self.cod);
        // set the tile size if it was not set by the user
        let tile_size = self.siz.get_tile_size();
        if tile_size.h == 0 && tile_size.w == 0 {
            // OpenJPH adds the offset to the extent, which is already
            // measured from the origin; the tile is larger than needed
            // but the codestream is the same.
            let img_offset = self.siz.get_image_offset();
            let img_extent = self.siz.get_image_extent();
            self.siz.set_tile_size(Size::new(
                img_extent.x.wrapping_add(img_offset.x),
                img_extent.y.wrapping_add(img_offset.y),
            ));
        }
        self.siz.check_validity()?;
        self.cod.check_validity(&self.siz)?;
        self.cod.update_atk(&self.atk)?;
        self.qcd.check_validity(&self.siz, &self.cod)?;
        self.cap.check_validity(&self.cod, &self.qcd);
        self.nlt.check_validity(&mut self.siz, &self.cod)?;
        self.siz.link(&self.cod, None);

        let Some(po) = self.cod.get_progression_order_enum() else {
            return ojph_error!(0x00080004, "unknown progression order");
        };
        let td = &mut self.tilepart_div;
        if (po == ProgressionOrder::Lrcp || po == ProgressionOrder::Rlcp)
            && *td == OJPH_TILEPART_COMPONENTS
        {
            // OJPH_INFO 0x00030021: for LRCP and RLCP progression orders,
            // tilepart divisions at the component level, means that we
            // have a tilepart for every resolution and component.
            *td |= OJPH_TILEPART_RESOLUTIONS;
        }
        if po == ProgressionOrder::Rpcl && (*td & OJPH_TILEPART_COMPONENTS) != 0 {
            // OJPH_WARN 0x00030021: a tile part for every precinct
            *td &= !OJPH_TILEPART_COMPONENTS;
        }
        if po == ProgressionOrder::Pcrl && *td != 0 {
            // OJPH_WARN 0x00030022
            *td = 0;
        }
        if po == ProgressionOrder::Cprl && (*td & OJPH_TILEPART_RESOLUTIONS) != 0 {
            // OJPH_WARN 0x00030023
            *td &= !OJPH_TILEPART_RESOLUTIONS;
        }

        let employ_color_transform = self.cod.is_employing_color_transform();
        // OpenJPH's default: planar with the colour transform, which it
        // cannot encode correctly, and interleaved without. The first is
        // refused as an explicit set_planar(true) is.
        let planar = self.planar.unwrap_or(employ_color_transform);
        if planar && employ_color_transform {
            return ojph_error!(
                0x00030021,
                "the planar interface option cannot be used when colour transform is employed"
            );
        }
        self.planar = Some(planar);

        // The component sizes, and the checks of the exchange order that
        // OpenJPH leaves to the caller (it loops forever on them).
        let mut comp_size = Vec::with_capacity(num_comps as usize);
        for c in 0..num_comps {
            comp_size.push(Size::new(self.siz.get_width(c), self.siz.get_height(c)));
        }
        for (c, s) in comp_size.iter().enumerate() {
            if s.h == 0 {
                return ojph_error!(
                    0x00080005,
                    "component {} has no lines; it is subsampled more than the image is high",
                    c
                );
            }
            if !planar && s.h != comp_size[0].h {
                return ojph_error!(
                    0x00080006,
                    "component {} has {} lines and component 0 has {}; components of different heights must be exchanged in planar mode",
                    c,
                    s.h,
                    comp_size[0].h
                );
            }
        }

        // A layer-first progression cannot have components with different
        // numbers of decompositions. Without tile parts at components
        // OpenJPH asserts `res_num <= num_decomps` in
        // tile_comp::write_precincts. With them it writes the stream, but
        // counts num_comps * (max + 1) tile parts in TNsot and numbers
        // them as if every component had every resolution, so TNsot is
        // wrong, TPsot skips numbers and either may wrap at 8 bits; no
        // decoder accepts the result (OpenJPH's gives 0x00030091).
        let decomps: Vec<u32> = (0..num_comps)
            .map(|c| self.cod.get_coc(c).get_num_decompositions())
            .collect();
        let max_decs = decomps.iter().copied().max().unwrap_or(0);
        if (po == ProgressionOrder::Lrcp || po == ProgressionOrder::Rlcp)
            && decomps.iter().any(|&d| d != max_decs)
        {
            if (self.tilepart_div & OJPH_TILEPART_COMPONENTS) == 0 {
                return ojph_error!(
                    0x00080007,
                    "with the {} progression order, all components must have the same number of decompositions",
                    po.as_str()
                );
            }
            return ojph_error!(
                0x0008000C,
                "with the {} progression order and tile parts at components, all components must have the same number of decompositions; OpenJPH writes a tile-part count and tile-part indices that no decoder accepts",
                po.as_str()
            );
        }

        // set_precinct_size checks sizes only up to the decompositions of
        // the time. Raising them afterwards, or carrying on after it
        // failed, leaves precincts of 1 sample above resolution 0, which
        // OpenJPH writes and then cannot decode (0x0005007F). The COD is
        // written even when every component has a COC, so it is checked
        // too.
        let cods = std::iter::once((None, &self.cod))
            .chain((0..num_comps).map(|c| (Some(c), self.cod.get_coc(c))));
        for (comp, cod) in cods {
            for r in 1..=cod.get_num_decompositions() {
                let p = cod.get_log_precinct_size(r);
                if p.w == 0 || p.h == 0 {
                    let which = match comp {
                        None => "the COD segment".to_string(),
                        Some(c) => format!("component {c}"),
                    };
                    return ojph_error!(
                        0x00080009,
                        "{} has a precinct size of {}x{} at resolution {}; only resolution 0 can have precincts 1 sample wide or high (set the precinct sizes after the number of decompositions)",
                        which,
                        1u32 << p.w,
                        1u32 << p.h,
                        r
                    );
                }
            }
        }

        // pre_alloc: the tile grid
        let sz = &self.siz;
        let tile_size = sz.get_tile_size();
        let num_tiles = Size::new(
            div_ceil(
                sz.get_image_extent().x.wrapping_sub(sz.get_tile_offset().x),
                tile_size.w,
            ),
            div_ceil(
                sz.get_image_extent().y.wrapping_sub(sz.get_tile_offset().y),
                tile_size.h,
            ),
        );
        if num_tiles.area() > 65535 {
            return ojph_error!(0x00030011, "the number of tiles cannot exceed 65535");
        }
        if num_tiles.area() == 0 {
            return ojph_error!(0x00030012, "the number of tiles cannot be 0");
        }

        let num_tileparts_per_tile = match self.tilepart_div & OJPH_TILEPART_MASK {
            OJPH_TILEPART_COMPONENTS => num_comps,
            OJPH_TILEPART_RESOLUTIONS => 1 + max_decs,
            t if t == OJPH_TILEPART_COMPONENTS | OJPH_TILEPART_RESOLUTIONS => {
                decomps.iter().map(|d| d + 1).sum()
            }
            _ => 1,
        };
        if num_tileparts_per_tile > 255 {
            return ojph_error!(
                0x000300D1,
                "Trying to create {} tileparts; a tile cannot have more than 255 tile parts.",
                num_tileparts_per_tile
            );
        }
        let num_tileparts = num_tileparts_per_tile * num_tiles.area() as u32;
        if self.need_tlm {
            self.tlm.init(num_tileparts)?;
        }

        if let Some(v) = &self.version_comment {
            if v.len() > 65531 {
                return ojph_error!(
                    0x00080008,
                    "the version comment cannot be longer than 65531 bytes"
                );
            }
        }

        // finalize_alloc: the tiles, and the lines exchanged with the
        // caller
        let mut budget = Budget::new(self.memory_limit);
        let ctx = Ctx {
            siz: &self.siz,
            cod: &self.cod,
            qcd: &self.qcd,
            nlt: &self.nlt,
        };
        let sz = ctx.siz;
        let (image_offset, image_extent) = (sz.get_image_offset(), sz.get_image_extent());
        let tile_offset = sz.get_tile_offset();
        budget.take(num_tiles.area() * std::mem::size_of::<Tile>() as u64)?;
        let mut tiles = Vec::with_capacity(num_tiles.area() as usize);
        // The grid is worked out in 64 bits; OpenJPH lets the far edge of
        // the last tile wrap.
        for y in 0..num_tiles.h as u64 {
            let y0 = tile_offset.y as u64 + y * tile_size.h as u64;
            let y1 = y0 + tile_size.h as u64; // end of tile
            let org_y = y0.max(image_offset.y as u64);
            let end_y = y1.min(image_extent.y as u64);
            let mut offset = 0;
            for x in 0..num_tiles.w as u64 {
                let x0 = tile_offset.x as u64 + x * tile_size.w as u64;
                let x1 = x0 + tile_size.w as u64;
                let org_x = x0.max(image_offset.x as u64);
                let end_x = x1.min(image_extent.x as u64);
                if end_x < org_x || end_y < org_y {
                    return ojph_error!(0x00070032, "a tile lies outside the image");
                }
                let tile_rect = Rect {
                    org: Point::new(org_x as u32, org_y as u32),
                    siz: Size::new((end_x - org_x) as u32, (end_y - org_y) as u32),
                };
                let idx = (y * num_tiles.w as u64 + x) as u16;
                tiles.push(Tile::new(&ctx, &mut budget, tile_rect, idx, &mut offset)?);
            }
        }
        let mut lines = Vec::with_capacity(num_comps as usize);
        for s in &comp_size {
            lines.push(budget.line_i32(s.w as usize, 0)?);
        }

        self.num_tiles = num_tiles;
        self.num_comps = num_comps;
        self.comp_size = comp_size;
        self.num_tileparts = num_tileparts;
        self.tiles = tiles;
        self.lines = lines;
        self.budget = budget;
        self.cur_comp = 0;
        self.cur_line = 0;
        self.cur_tile_row = 0;
        self.handed_out = false;
        Ok(())
    }

    /// Exchanges lines with the caller, after OpenJPH's
    /// `exchange(line, next_component)`.
    ///
    /// Each call returns the number of a component and a line of it to
    /// fill: its samples, as many as the component is wide, left to
    /// right. The next call takes that line back and encodes it (OpenJPH
    /// passes the line back as an argument; here the encoder keeps it, so
    /// it cannot be swapped for one of the wrong size or kind). After the
    /// last line has been taken back the call returns `None`, so every
    /// line is pushed by
    ///
    /// ```ignore
    /// while let Some((comp, samples)) = encoder.exchange()? {
    ///     fill(comp, samples);
    /// }
    /// ```
    ///
    /// In planar mode every line of component 0 is asked for first, then
    /// those of component 1, and so on; otherwise one line of each
    /// component in turn. Samples are the image's values: `0 .. 2^B` for
    /// an unsigned component of depth `B`, `-2^(B-1) .. 2^(B-1)` for a
    /// signed one. An unsigned 32-bit sample above `i32::MAX` is passed as
    /// its bit pattern (`u32 as i32`), as OpenJPH's `exchange` takes it.
    pub fn exchange(&mut self) -> Result<Option<(u32, &mut [i32])>> {
        match self.state {
            State::Headers => {}
            State::Pushed => return Ok(None),
            State::Setup => {
                return ojph_error!(0x00080040, "write_headers must be called before exchange")
            }
            State::Failed => {
                return ojph_error!(0x00080041, "the encoder has failed and cannot be used")
            }
            State::Flushed => {
                return ojph_error!(0x00080043, "the codestream has already been flushed")
            }
        }
        if self.handed_out {
            if let Err(e) = self.push_line() {
                self.state = State::Failed;
                return Err(e);
            }
            if self.advance() {
                self.handed_out = false;
                self.state = State::Pushed;
                return Ok(None);
            }
        }
        self.handed_out = true;
        let c = self.cur_comp as usize;
        let w = self.comp_size[c].w as usize;
        Ok(Some((self.cur_comp, &mut self.lines[c].i32_mut()[..w])))
    }

    /// Appends the rest of the codestream to `out`: the TLM marker segment
    /// if requested, every tile part, and EOC. `out` is normally the
    /// vector [`write_headers`](Self::write_headers) appended to.
    ///
    /// Every line must have been exchanged first. (OpenJPH writes what it
    /// has.) Nothing is appended if this fails, and the encoder cannot
    /// then be used further.
    pub fn flush(&mut self, out: &mut Vec<u8>) -> Result<()> {
        match self.state {
            State::Pushed => {}
            State::Setup | State::Headers => {
                return ojph_error!(
                    0x00080044,
                    "every line must be exchanged before the codestream is flushed"
                )
            }
            State::Failed => {
                return ojph_error!(0x00080041, "the encoder has failed and cannot be used")
            }
            State::Flushed => {
                return ojph_error!(0x00080043, "the codestream has already been flushed")
            }
        }
        let start = out.len();
        let result = self.write_tiles(out);
        if result.is_err() {
            out.truncate(start);
            self.state = State::Failed;
        } else {
            self.state = State::Flushed;
        }
        result
    }

    fn write_tiles(&mut self, out: &mut Vec<u8>) -> Result<()> {
        for tile in self.tiles.iter_mut() {
            tile.prepare_for_flush(&mut self.precinct_scratch)?;
        }
        self.precinct_scratch = Vec::new();
        if self.need_tlm {
            // write tlm
            for tile in &self.tiles {
                tile.fill_tlm(&mut self.tlm, self.tilepart_div)?;
            }
            self.tlm.write(out);
        }
        for tile in self.tiles.iter_mut() {
            tile.flush(out, self.tilepart_div)?;
        }
        put_u16(out, EOC);
        Ok(())
    }

    /// Pushes the line of component `cur_comp` to the tiles of the
    /// current row of tiles; `tile::push` over a row in OpenJPH.
    fn push_line(&mut self) -> Result<()> {
        let c = self.cur_comp;
        let line = &self.lines[c as usize];
        let mut failures = 0;
        loop {
            let mut success = true;
            let row = self.cur_tile_row as usize * self.num_tiles.w as usize;
            for tile in &mut self.tiles[row..row + self.num_tiles.w as usize] {
                if !tile.push(line, c, &mut self.budget)? {
                    success = false;
                    break;
                }
            }
            if !success {
                self.cur_tile_row += 1;
            }
            if self.cur_tile_row >= self.num_tiles.h {
                self.cur_tile_row = 0;
            }
            if success {
                return Ok(());
            }
            // OpenJPH loops forever once no row of tiles takes the line.
            failures += 1;
            if failures > self.num_tiles.h {
                return ojph_error!(
                    0x00080042,
                    "internal error: no tile takes line {} of component {}",
                    self.cur_line,
                    c
                );
            }
        }
    }

    /// Moves on to the next line to ask for; true when there is none.
    fn advance(&mut self) -> bool {
        if self.is_planar() {
            // process one component at a time
            self.cur_line += 1;
            if self.cur_line >= self.comp_size[self.cur_comp as usize].h {
                self.cur_line = 0;
                self.cur_tile_row = 0;
                self.cur_comp += 1;
                if self.cur_comp >= self.num_comps {
                    return true;
                }
            }
        } else {
            // process all components for a line
            self.cur_comp += 1;
            if self.cur_comp >= self.num_comps {
                self.cur_comp = 0;
                self.cur_line += 1;
                if self.cur_line >= self.comp_size[0].h {
                    return true;
                }
            }
        }
        false
    }

    /// Writes SOC up to the comments.
    fn write_main_header(&mut self, out: &mut Vec<u8>, comments: &[CommentExchange]) {
        // Writing to a vector cannot fail; OpenJPH's "Error writing to
        // file" errors have no counterpart.
        put_u16(out, SOC);
        self.siz.write(out);
        self.cap.write(out);
        self.cod.write(out);
        self.cod.write_coc(out, self.num_comps);
        self.qcd.write(out);
        self.qcd.write_qcc(out, self.num_comps);
        self.nlt.write(out);

        if let Some(v) = &self.version_comment {
            put_u16(out, COM);
            put_u16(out, (v.len() + 4) as u16);
            put_u16(out, 1); // General use (IS 8859-15:1999 (Latin) values)
            out.extend_from_slice(v);
        }
        for c in comments {
            let data = c.data();
            put_u16(out, COM);
            put_u16(out, (data.len() as u16).wrapping_add(4));
            put_u16(out, c.get_Rcom());
            out.extend_from_slice(data);
        }
    }
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
