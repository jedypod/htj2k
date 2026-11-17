// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! The SOT (`param_sot`) and TLM (`param_tlm`) marker segments.

use super::markers::{jp2k_marker, put_u16, put_u32, put_u8};
use crate::error::ojph_error;
use crate::file::MemInfile;
use crate::Result;

/// `param_sot`: start of tile-part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParamSot {
    pub(crate) lsot: u16,
    pub(crate) isot: u16,
    pub(crate) psot: u32,
    pub(crate) tpsot: u8,
    pub(crate) tnsot: u8,
}

impl ParamSot {
    /// C++ `init(ui32 payload_length = 0, ui16 tile_idx = 0, ui8
    /// tile_part_index = 0, ui8 num_tile_parts = 0)`; Rust has no default
    /// arguments, so all four are given.
    pub fn init(
        &mut self,
        payload_length: u32,
        tile_idx: u16,
        tile_part_index: u8,
        num_tile_parts: u8,
    ) {
        self.lsot = 10;
        self.psot = payload_length.wrapping_add(12); // total = payload + SOT marker
        self.isot = tile_idx;
        self.tpsot = tile_part_index;
        self.tnsot = num_tile_parts;
    }

    /// `write(outfile_base*, ui32 payload_len)`
    pub fn write(&mut self, file: &mut Vec<u8>, payload_len: u32) -> bool {
        self.psot = payload_len.wrapping_add(14); // inc. SOT marker, field & SOD
        put_u16(file, jp2k_marker::SOT);
        put_u16(file, self.lsot);
        put_u16(file, self.isot);
        put_u32(file, self.psot);
        put_u8(file, self.tpsot);
        put_u8(file, self.tnsot);
        true
    }

    /// `write(outfile_base*, ui32 payload_len, ui8 TPsot, ui8 TNsot)`
    #[allow(non_snake_case)]
    pub fn write_with_parts(
        &self,
        file: &mut Vec<u8>,
        payload_len: u32,
        TPsot: u8,
        TNsot: u8,
    ) -> bool {
        put_u16(file, jp2k_marker::SOT);
        put_u16(file, self.lsot);
        put_u16(file, self.isot);
        put_u32(file, payload_len.wrapping_add(14));
        put_u8(file, TPsot);
        put_u8(file, TNsot);
        true
    }

    /// In resilient mode a failure clears the fields and returns
    /// `Ok(false)` (C++ also prints an OJPH_INFO); otherwise it is an
    /// error.
    pub fn read(&mut self, file: &mut MemInfile, resilient: bool) -> Result<bool> {
        macro_rules! fail {
            ($code:expr, $msg:literal) => {{
                if resilient {
                    // OJPH_INFO($code, $msg)
                    *self = ParamSot::default();
                    return Ok(false);
                } else {
                    return ojph_error!($code, $msg);
                }
            }};
        }
        match file.read_u16() {
            Some(v) => self.lsot = v,
            None => fail!(0x00050091, "error reading SOT marker"),
        }
        if self.lsot != 10 {
            fail!(0x00050092, "error in SOT length");
        }
        match file.read_u16() {
            Some(v) => self.isot = v,
            None => {
                if resilient {
                    fail!(0x00050093, "error reading tile index");
                } else {
                    fail!(0x00050093, "error reading SOT tile index");
                }
            }
        }
        if self.isot == 0xFFFF {
            fail!(0x00050094, "tile index in SOT marker cannot be 0xFFFF");
        }
        match file.read_u32() {
            Some(v) => self.psot = v,
            None => fail!(0x00050095, "error reading SOT marker"),
        }
        match file.read_u8() {
            Some(v) => self.tpsot = v,
            None => fail!(0x00050096, "error reading SOT marker"),
        }
        match file.read_u8() {
            Some(v) => self.tnsot = v,
            None => fail!(0x00050097, "error reading SOT marker"),
        }
        Ok(true)
    }

    pub fn get_tile_index(&self) -> u16 {
        self.isot
    }
    pub fn get_payload_length(&self) -> u32 {
        if self.psot > 0 {
            self.psot.wrapping_sub(12)
        } else {
            0
        }
    }
    pub fn get_tile_part_index(&self) -> u8 {
        self.tpsot
    }
    pub fn get_num_tile_parts(&self) -> u8 {
        self.tnsot
    }
}

/// `param_tlm::Ttlm_Ptlm_pair`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(non_snake_case)]
pub struct TtlmPtlmPair {
    pub Ttlm: u16,
    pub Ptlm: u32,
}

/// `param_tlm`: tile-part lengths. C++ is handed external storage for the
/// pairs; here it owns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamTlm {
    pub(crate) pairs: Vec<TtlmPtlmPair>,
    pub(crate) num_pairs: u32,
    pub(crate) next_pair_index: u32,
}

impl ParamTlm {
    /// Ltlm is 16 bits and spans itself, Ztlm, Stlm and 6 bytes per entry
    pub const MAX_PAIRS_PER_SEG: u32 = (65535 - 4) / 6;
    /// Ztlm is 8 bits
    pub const MAX_SEGMENTS: u32 = 256;

    pub fn new() -> Self {
        Self::default()
    }

    /// C++ `init(ui32 num_pairs, Ttlm_Ptlm_pair* store)`; the store is
    /// allocated here.
    pub fn init(&mut self, num_pairs: u32) -> Result<()> {
        if num_pairs > Self::MAX_PAIRS_PER_SEG * Self::MAX_SEGMENTS {
            return ojph_error!(
                0x000500B1,
                "Trying to store {} tileparts in TLM markers, but at most {} can be indexed; a codestream can carry {} TLM marker segments of {} entries each.",
                num_pairs,
                Self::MAX_PAIRS_PER_SEG * Self::MAX_SEGMENTS,
                Self::MAX_SEGMENTS,
                Self::MAX_PAIRS_PER_SEG
            );
        }
        self.num_pairs = num_pairs;
        self.pairs.clear();
        self.pairs
            .resize(num_pairs as usize, TtlmPtlmPair::default());
        self.next_pair_index = 0;
        Ok(())
    }

    /// C++ asserts there is room; a pair beyond `num_pairs` is dropped.
    #[allow(non_snake_case)]
    pub fn set_next_pair(&mut self, Ttlm: u16, Ptlm: u32) {
        if let Some(p) = self.pairs.get_mut(self.next_pair_index as usize) {
            p.Ttlm = Ttlm;
            p.Ptlm = Ptlm.wrapping_add(14);
            self.next_pair_index += 1;
        }
    }

    /// The bytes [`write`](Self::write) appends: 6 for each marker
    /// segment and 6 for each pair.
    pub(crate) fn written_len(&self) -> u64 {
        let segments = self.num_pairs.div_ceil(Self::MAX_PAIRS_PER_SEG).max(1);
        6 * u64::from(segments) + 6 * u64::from(self.num_pairs)
    }

    pub fn write(&self, file: &mut Vec<u8>) -> bool {
        // C++ asserts next_pair_index == num_pairs
        let mut written = 0u32;
        let mut z = 0u32;
        loop {
            let left = self.num_pairs - written;
            let count = left.min(Self::MAX_PAIRS_PER_SEG);
            let ztlm = z as u8;
            let stlm = 0x60u8; // 2-byte Ttlm, 4-byte Ptlm

            put_u16(file, jp2k_marker::TLM);
            put_u16(file, (4 + 6 * count) as u16);
            put_u8(file, ztlm);
            put_u8(file, stlm);
            for p in &self.pairs[written as usize..(written + count) as usize] {
                put_u16(file, p.Ttlm);
                put_u32(file, p.Ptlm);
            }
            written += count;
            z += 1;
            if written >= self.num_pairs {
                break;
            }
        }
        true
    }
}
