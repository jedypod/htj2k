// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The codestream, after `ojph_codestream.cpp` and
//! `ojph_codestream_local.cpp`.

use crate::codestream::tile::Tile;
use crate::codestream::{Budget, Build};
use crate::error::ojph_error;
use crate::file::{MemInfile, SeekFrom};
use crate::line::LineBuf;
use crate::params::marker::{
    ATK, CAP, COC, COD, COM, CPF, CRG, DFS, EOC, NLT, PLM, PLT, POC, PPM, PPT, PRF, QCC, QCD, RGN,
    SIZ, SOC, SOD, SOT, TLM,
};
use crate::params::{
    ParamAtk, ParamCap, ParamCod, ParamDfs, ParamNlt, ParamQcd, ParamSiz, ParamSot,
};
use crate::types::{div_ceil, Point, Rect, Size};
use crate::Result;

/// The default for [`Codestream::set_memory_limit`]: 4 GiB.
const DEFAULT_MEMORY_LIMIT: u64 = 1 << 32;

/// An HTJ2K codestream being decoded from memory.
///
/// The calls follow OpenJPH's `ojph::codestream`:
///
/// 1. [`Codestream::new`], then optionally
///    [`enable_resilience`](Self::enable_resilience);
/// 2. [`read_headers`](Self::read_headers), after which the marker segments
///    can be looked at with [`access_siz`](Self::access_siz) and the like;
/// 3. optionally [`restrict_input_resolution`](Self::restrict_input_resolution),
///    [`set_planar`](Self::set_planar) and
///    [`set_memory_limit`](Self::set_memory_limit);
/// 4. [`create`](Self::create), which parses every tile part;
/// 5. [`pull`](Self::pull), once per line of every component.
#[derive(Debug)]
pub struct Codestream<'a> {
    infile: Option<MemInfile<'a>>,
    precinct_scratch: Vec<u8>,

    cur_line: u32,
    cur_comp: u32,
    cur_tile_row: u32,
    resilient: bool,
    skipped_res_for_read: u32,
    skipped_res_for_recon: u32,
    memory_limit: u64,
    /// Set by `create`.
    created: bool,

    num_tiles: Size,
    tiles: Vec<Tile>,
    /// The lines handed out by `pull`, one per component.
    lines: Vec<LineBuf>,
    num_comps: u32,
    /// The number of lines and the width of each component.
    recon_comp_size: Vec<Size>,
    planar: bool,

    siz: ParamSiz,
    cod: ParamCod,
    cap: ParamCap,
    qcd: ParamQcd,
    nlt: ParamNlt,
    // these are from Part 2 of the standard
    dfs: ParamDfs,
    atk: ParamAtk,
}

impl Default for Codestream<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Codestream<'a> {
    pub fn new() -> Self {
        Codestream {
            infile: None,
            precinct_scratch: Vec::new(),
            cur_line: 0,
            cur_comp: 0,
            cur_tile_row: 0,
            resilient: false,
            skipped_res_for_read: 0,
            skipped_res_for_recon: 0,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            created: false,
            num_tiles: Size::default(),
            tiles: Vec::new(),
            lines: Vec::new(),
            num_comps: 0,
            recon_comp_size: Vec::new(),
            planar: false,
            siz: ParamSiz::default(),
            cod: ParamCod::default(),
            cap: ParamCap::default(),
            qcd: ParamQcd::default(),
            nlt: ParamNlt::default(),
            dfs: ParamDfs::default(),
            atk: ParamAtk::default(),
        }
    }

    /// The SIZ marker segment: image, tile and component sizes.
    pub fn access_siz(&self) -> &ParamSiz {
        &self.siz
    }

    /// The COD marker segment: coding style.
    pub fn access_cod(&self) -> &ParamCod {
        &self.cod
    }

    /// The QCD marker segment: quantization.
    pub fn access_qcd(&self) -> &ParamQcd {
        &self.qcd
    }

    /// The NLT marker segments: non-linearity point transformations.
    pub fn access_nlt(&self) -> &ParamNlt {
        &self.nlt
    }

    /// Makes the decoder carry on past errors in tile parts, producing
    /// what it can. Must be called before [`read_headers`](Self::read_headers).
    pub fn enable_resilience(&mut self) -> Result<()> {
        if self.infile.is_some() {
            return ojph_error!(
                0x000300A3,
                "Codestream resilience must be enabled before reading file headers.\n"
            );
        }
        self.resilient = true;
        Ok(())
    }

    /// Limits the memory that [`create`](Self::create) may allocate for
    /// buffers whose size comes from the codestream. OpenJPH has no such
    /// limit.
    pub fn set_memory_limit(&mut self, bytes: u64) {
        self.memory_limit = bytes;
    }

    /// Whether [`pull`](Self::pull) returns all the lines of one component
    /// before going to the next, or one line of every component in turn.
    pub fn is_planar(&self) -> bool {
        self.planar
    }

    /// Chooses the order of lines from [`pull`](Self::pull). After
    /// [`read_headers`](Self::read_headers) it is planar unless the colour
    /// transform is used.
    pub fn set_planar(&mut self, planar: bool) {
        self.planar = planar;
    }

    /// Reads the main header of `data`, up to the first tile part.
    pub fn read_headers(&mut self, data: &'a [u8]) -> Result<()> {
        let mut infile = MemInfile::new(data);
        let file = &mut infile;
        let marker_list = [
            SOC, SIZ, CAP, PRF, CPF, COD, COC, QCD, QCC, RGN, POC, PPM, TLM, PLM, CRG, COM, DFS,
            ATK, NLT, SOT,
        ];
        find_marker(file, &marker_list[..1]); // find SOC
        find_marker(file, &marker_list[1..2]); // find SIZ
        self.siz.read(file)?;
        let mut received_markers = 0; // check that COD, & QCD received
        loop {
            match find_marker(file, &marker_list[2..]) {
                Some(0) => self.cap.read(file)?,
                // Skipping a PRF, CPF, TLM, PLM or COM marker segment
                // should not cause any issues. OpenJPH warns that RGN, POC
                // and PPM are not supported yet, and notes that CRG is
                // ignored.
                Some(1 | 2 | 7 | 8 | 9 | 10 | 11 | 12 | 13) => {
                    skip_marker(file, false)?;
                }
                Some(3) => {
                    self.cod.read(file)?;
                    received_markers |= 1;
                    let num_qlayers = self.cod.get_num_layers();
                    if num_qlayers != 1 {
                        return ojph_error!(
                            0x00030053,
                            "The current implementation supports 1 quality layer only.  This codestream has {} quality layers",
                            num_qlayers
                        );
                    }
                }
                Some(4) => self.cod.read_coc(file, self.siz.get_num_components())?,
                Some(5) => {
                    self.qcd.read(file)?;
                    received_markers |= 2;
                }
                Some(6) => self.qcd.read_qcc(file, self.siz.get_num_components())?,
                Some(14) => self.dfs.read(file)?,
                Some(15) => self.atk.read(file)?,
                Some(16) => self.nlt.read(file)?,
                Some(17) => break,
                _ => {
                    return ojph_error!(0x00030051, "File ended before finding a tile segment");
                }
            }
        }

        self.cod.update_atk(&self.atk)?;
        self.siz.link(
            &self.cod,
            if self.dfs.exists() {
                Some(&self.dfs)
            } else {
                None
            },
        );

        if received_markers != 3 {
            return ojph_error!(0x00030052, "markers error, COD and QCD are required");
        }

        // A LUT style nonlinearity, type 2 or type 4, cannot currently be
        // applied with the reversible wavelet.  A codestream may still carry
        // that combination, but this library cannot decode this codestream
        // yet.
        if let Some(unsupported_nlt) = self.nlt.find_unsupported_nlt(&self.siz, &self.cod) {
            return ojph_error!(
                0x00030059,
                "The codestream uses a LUT style nonlinearity (type 2 or type 4) together with the reversible (5/3) wavelet for component {}.  This does not make sense since LUT itself is not reversible. This library does not implement the processing path needed for this conbination.",
                unsupported_nlt
            );
        }

        self.infile = Some(infile);
        self.planar = !self.cod.is_employing_color_transform();
        Ok(())
    }

    /// Decodes at a lower resolution: `skipped_res_for_read` resolutions
    /// are not parsed, and `skipped_res_for_recon` are not reconstructed.
    pub fn restrict_input_resolution(
        &mut self,
        skipped_res_for_read: u32,
        skipped_res_for_recon: u32,
    ) -> Result<()> {
        if skipped_res_for_read < skipped_res_for_recon {
            return ojph_error!(
                0x000300A1,
                "skipped_resolution for data {} must be equal or smaller than  skipped_resolution for reconstruction {}\n",
                skipped_res_for_read,
                skipped_res_for_recon
            );
        }
        if skipped_res_for_read > self.cod.get_num_decompositions() {
            return ojph_error!(
                0x000300A2,
                "skipped_resolution for data {} must be smaller than  the number of decomposition levels {}\n",
                skipped_res_for_read,
                self.cod.get_num_decompositions()
            );
        }
        self.skipped_res_for_read = skipped_res_for_read;
        self.skipped_res_for_recon = skipped_res_for_recon;
        self.siz.set_skipped_resolutions(skipped_res_for_recon);
        Ok(())
    }

    /// Builds the tiles, after `pre_alloc` and `finalize_alloc`.
    fn alloc(&mut self) -> Result<()> {
        let sz = &self.siz;
        let image_extent = sz.get_image_extent();
        let image_offset = sz.get_image_offset();
        let tile_offset = sz.get_tile_offset();
        let tile_size = sz.get_tile_size();
        if tile_size.w == 0 || tile_size.h == 0 {
            return ojph_error!(0x00070031, "the tile size cannot be 0");
        }
        // SIZ validation makes the extent no smaller than the tile offset;
        // if it were, there would be no tiles
        let num_tiles = Size::new(
            div_ceil(image_extent.x.saturating_sub(tile_offset.x), tile_size.w),
            div_ceil(image_extent.y.saturating_sub(tile_offset.y), tile_size.h),
        );
        if num_tiles.area() > 65535 {
            return ojph_error!(0x00030011, "the number of tiles cannot exceed 65535");
        }
        if num_tiles.area() == 0 {
            return ojph_error!(0x00030012, "the number of tiles cannot be 0");
        }
        self.num_tiles = num_tiles;

        let mut budget = Budget::new(self.memory_limit);
        let build = Build {
            siz: &self.siz,
            cod: &self.cod,
            qcd: &self.qcd,
            dfs: if self.dfs.exists() {
                Some(&self.dfs)
            } else {
                None
            },
            nlt: &self.nlt,
            skipped_res_for_read: self.skipped_res_for_read,
            skipped_res_for_recon: self.skipped_res_for_recon,
            resilient: self.resilient,
        };

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
                tiles.push(Tile::new(&build, &mut budget, tile_rect, &mut offset)?);
            }
        }

        // These lines are used by the codestream to exchange data with the
        // external world.
        let num_comps = sz.get_num_components();
        let mut lines = Vec::with_capacity(num_comps as usize);
        let mut recon_comp_size = Vec::with_capacity(num_comps as usize);
        for i in 0..num_comps {
            let cw = sz.get_recon_width(i);
            recon_comp_size.push(Size::new(cw, sz.get_recon_height(i)));
            lines.push(budget.line_i32(cw as usize, 0)?);
        }

        self.tiles = tiles;
        self.num_comps = num_comps;
        self.lines = lines;
        self.recon_comp_size = recon_comp_size;
        self.cur_comp = 0;
        self.cur_line = 0;
        self.cur_tile_row = 0;
        Ok(())
    }

    /// Parses every tile part of the codestream, ready for
    /// [`pull`](Self::pull). `ojph::codestream::create` on the decoding
    /// side, which is `local::codestream::read`.
    pub fn create(&mut self) -> Result<()> {
        if self.infile.is_none() {
            return ojph_error!(0x00070033, "read_headers must be called before create");
        }
        if self.created {
            return ojph_error!(0x00070034, "create has already been called");
        }
        self.alloc()?;
        self.created = true;

        let resilient = self.resilient;
        let Some(infile) = self.infile.as_mut() else {
            return ojph_error!(0x00070033, "read_headers must be called before create");
        };
        loop {
            let mut sot = ParamSot::default();
            if sot.read(infile, resilient)? {
                let tile_start_location = infile.tell() as u64;
                let mut skip_tile = false;

                if u64::from(sot.get_tile_index()) >= self.num_tiles.area() {
                    if resilient {
                        skip_tile = true; // skip the faulty tile
                    } else {
                        return ojph_error!(0x00030061, "wrong tile index");
                    }
                }

                if !skip_tile {
                    let sod_found = if sot.get_tile_part_index() != 0 {
                        // tile part
                        if sot.get_num_tile_parts() != 0
                            && sot.get_tile_part_index() >= sot.get_num_tile_parts()
                            && !resilient
                        {
                            return ojph_error!(
                                0x00030062,
                                "error in tile part number, should be smaller than total number of tile parts"
                            );
                        }
                        // OpenJPH warns that POC, PPT and NLT marker
                        // segments in a tile are not supported yet
                        find_sod(
                            infile,
                            &[POC, PPT, PLT, COM, NLT, SOD],
                            &sot,
                            resilient,
                            0x00030063,
                        )?
                    } else {
                        // first tile part; likewise for COD, COC, QCD,
                        // QCC, RGN, POC, PPT and NLT
                        find_sod(
                            infile,
                            &[COD, COC, QCD, QCC, RGN, POC, PPT, PLT, COM, NLT, SOD],
                            &sot,
                            resilient,
                            0x00030065,
                        )?
                    };
                    if sod_found {
                        self.tiles[usize::from(sot.get_tile_index())].parse_tile_header(
                            &sot,
                            &mut self.precinct_scratch,
                            infile,
                            tile_start_location,
                        )?;
                    }
                }
            }

            // check the next marker; either SOT or EOC,
            // if something is broken, just an end of file
            match find_marker(infile, &[SOT, EOC]) {
                Some(0) => {}
                // EOC, or the file terminated early
                _ => break,
            }
        }
        Ok(())
    }

    /// Decodes the next line. Returns the number of the component it
    /// belongs to and the line, whose samples are always 32-bit integers,
    /// or `None` when every line has been returned.
    ///
    /// In planar mode all the lines of component 0 come first, then those
    /// of component 1, and so on; otherwise one line of each component in
    /// turn, which needs all the components to have the same height.
    pub fn pull(&mut self) -> Result<Option<(u32, &LineBuf)>> {
        if !self.created {
            return ojph_error!(0x00070035, "create must be called before pull");
        }
        // OpenJPH leaves pulling past the last line undefined.
        if self.cur_comp >= self.num_comps {
            return Ok(None);
        }
        if !self.planar && self.cur_line >= self.recon_comp_size[0].h {
            return Ok(None);
        }
        let Some(infile) = self.infile.as_ref() else {
            return ojph_error!(0x00070033, "read_headers must be called before create");
        };
        let data = infile.data();

        let cur_comp = self.cur_comp;
        let line = &mut self.lines[cur_comp as usize];
        let mut failures = 0;
        loop {
            let mut success = true;
            let row = self.cur_tile_row as usize * self.num_tiles.w as usize;
            for tile in &mut self.tiles[row..row + self.num_tiles.w as usize] {
                if !tile.pull(line, cur_comp, data)? {
                    success = false;
                    break;
                }
            }
            if success {
                break;
            }
            self.cur_tile_row += 1;
            if self.cur_tile_row >= self.num_tiles.h {
                self.cur_tile_row = 0;
            }
            // Once every row of tiles has refused, none ever will give a
            // line; OpenJPH would loop forever.
            failures += 1;
            if failures > self.num_tiles.h {
                return ojph_error!(
                    0x00070036,
                    "component {} has no more lines; components of different heights must be pulled in planar mode",
                    cur_comp
                );
            }
        }
        let comp_num = cur_comp;

        if self.planar {
            // process one component at a time
            self.cur_line += 1;
            if self.cur_line >= self.recon_comp_size[cur_comp as usize].h {
                self.cur_line = 0;
                self.cur_tile_row = 0;
                self.cur_comp += 1;
            }
        } else {
            // process all components for a line
            self.cur_comp += 1;
            if self.cur_comp >= self.num_comps {
                self.cur_comp = 0;
                self.cur_line += 1;
            }
        }

        Ok(Some((comp_num, &self.lines[comp_num as usize])))
    }
}

/// Reads on to the next marker whose low byte is that of one of
/// `char_list`, and returns its index in the list; `None` at the end of
/// the file.
fn find_marker(f: &mut MemInfile, char_list: &[u16]) -> Option<usize> {
    while !f.eof() {
        if f.read_u8()? == 0xFF {
            let new_char = f.read_u8()?;
            if let Some(i) = char_list.iter().position(|&c| new_char == (c & 0xFF) as u8) {
                return Some(i);
            }
        }
    }
    None
}

/// Skips a marker segment. Returns false if the file ends before its
/// length, which is an error unless `resilient`.
fn skip_marker(file: &mut MemInfile, resilient: bool) -> Result<bool> {
    let Some(com_len) = file.read_u16() else {
        if resilient {
            return Ok(false);
        }
        return ojph_error!(0x00030041, "error reading marker");
    };
    file.seek(com_len as i64 - 2, SeekFrom::Current);
    Ok(true)
}

/// Skips the marker segments of a tile-part header, up to SOD, the last of
/// `markers`; all the others are skipped. Returns whether SOD was found.
/// `code` is OpenJPH's error code for a file that ends first, and
/// `code + 1` for one that ends inside a marker segment.
fn find_sod(
    infile: &mut MemInfile,
    markers: &[u16],
    sot: &ParamSot,
    resilient: bool,
    code: u32,
) -> Result<bool> {
    loop {
        match find_marker(infile, markers) {
            Some(i) if i + 1 == markers.len() => return Ok(true),
            Some(_) => {
                if !skip_marker(infile, resilient)? {
                    // only when resilient
                    return Ok(false);
                }
            }
            None => {
                if resilient {
                    return Ok(false);
                }
                return ojph_error!(
                    code,
                    "File terminated early before start of data is found for tile indexed {} and tile part {}",
                    sot.get_tile_index(),
                    sot.get_tile_part_index()
                );
            }
        }
    }
}
