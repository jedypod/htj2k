// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Codestream marker codes (`JP2K_MARKER` in `ojph_params_local.h`), the
//! public enums of that header, and small big-endian writing helpers.

/// `JP2K_MARKER`: the two-byte marker codes, as `u16` constants so they can
/// be matched against a value read from the codestream.
pub mod jp2k_marker {
    /// start of codestream (required)
    pub const SOC: u16 = 0xFF4F;
    /// extended capability
    pub const CAP: u16 = 0xFF50;
    /// image and tile size (required)
    pub const SIZ: u16 = 0xFF51;
    /// coding style default (required)
    pub const COD: u16 = 0xFF52;
    /// coding style component
    pub const COC: u16 = 0xFF53;
    /// tile-part lengths
    pub const TLM: u16 = 0xFF55;
    /// profile
    pub const PRF: u16 = 0xFF56;
    /// packet length, main header
    pub const PLM: u16 = 0xFF57;
    /// packet length, tile-part header
    pub const PLT: u16 = 0xFF58;
    /// corresponding profile values
    pub const CPF: u16 = 0xFF59;
    /// quantization default (required)
    pub const QCD: u16 = 0xFF5C;
    /// quantization component
    pub const QCC: u16 = 0xFF5D;
    /// region of interest
    pub const RGN: u16 = 0xFF5E;
    /// progression order change
    pub const POC: u16 = 0xFF5F;
    /// packed packet headers, main header
    pub const PPM: u16 = 0xFF60;
    /// packed packet headers, tile-part header
    pub const PPT: u16 = 0xFF61;
    /// component registration
    pub const CRG: u16 = 0xFF63;
    /// comment
    pub const COM: u16 = 0xFF64;
    /// downsampling factor styles
    pub const DFS: u16 = 0xFF72;
    /// arbitrary decomposition styles
    pub const ADS: u16 = 0xFF73;
    /// non-linearity point transformation
    pub const NLT: u16 = 0xFF76;
    /// arbitrary transformation kernels
    pub const ATK: u16 = 0xFF79;
    /// start of tile-part
    pub const SOT: u16 = 0xFF90;
    /// start of packet
    pub const SOP: u16 = 0xFF91;
    /// end of packet
    pub const EPH: u16 = 0xFF92;
    /// start of data
    pub const SOD: u16 = 0xFF93;
    /// end of codestream (required)
    pub const EOC: u16 = 0xFFD9;
}

/// `PROGRESSION_ORDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ProgressionOrder {
    Lrcp = 0,
    Rlcp = 1,
    Rpcl = 2,
    Pcrl = 3,
    Cprl = 4,
}

impl ProgressionOrder {
    /// The order with numeric value `v`, if it is one of the five.
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Lrcp),
            1 => Some(Self::Rlcp),
            2 => Some(Self::Rpcl),
            3 => Some(Self::Pcrl),
            4 => Some(Self::Cprl),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lrcp => OJPH_PO_STRING_LRCP,
            Self::Rlcp => OJPH_PO_STRING_RLCP,
            Self::Rpcl => OJPH_PO_STRING_RPCL,
            Self::Pcrl => OJPH_PO_STRING_PCRL,
            Self::Cprl => OJPH_PO_STRING_CPRL,
        }
    }
}

pub const OJPH_PO_LRCP: i32 = 0;
pub const OJPH_PO_RLCP: i32 = 1;
pub const OJPH_PO_RPCL: i32 = 2;
pub const OJPH_PO_PCRL: i32 = 3;
pub const OJPH_PO_CPRL: i32 = 4;

pub const OJPH_PO_STRING_LRCP: &str = "LRCP";
pub const OJPH_PO_STRING_RLCP: &str = "RLCP";
pub const OJPH_PO_STRING_RPCL: &str = "RPCL";
pub const OJPH_PO_STRING_PCRL: &str = "PCRL";
pub const OJPH_PO_STRING_CPRL: &str = "CPRL";

/// `OJPH_PROFILE_NUM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ProfileNum {
    Undefined = 0,
    Profile0 = 1,
    Profile1 = 2,
    Cinema2k = 3,
    Cinema4k = 4,
    Cinemas2k = 5,
    Cinemas4k = 6,
    Broadcast = 7,
    Imf = 8,
}

impl ProfileNum {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undefined => "",
            Self::Profile0 => OJPH_PN_STRING_PROFILE0,
            Self::Profile1 => OJPH_PN_STRING_PROFILE1,
            Self::Cinema2k => OJPH_PN_STRING_CINEMA2K,
            Self::Cinema4k => OJPH_PN_STRING_CINEMA4K,
            Self::Cinemas2k => OJPH_PN_STRING_CINEMAS2K,
            Self::Cinemas4k => OJPH_PN_STRING_CINEMAS4K,
            Self::Broadcast => OJPH_PN_STRING_BROADCAST,
            Self::Imf => OJPH_PN_STRING_IMF,
        }
    }
}

pub const OJPH_PN_STRING_PROFILE0: &str = "PROFILE0";
pub const OJPH_PN_STRING_PROFILE1: &str = "PROFILE1";
pub const OJPH_PN_STRING_CINEMA2K: &str = "CINEMA2K";
pub const OJPH_PN_STRING_CINEMA4K: &str = "CINEMA4K";
pub const OJPH_PN_STRING_CINEMAS2K: &str = "CINEMAS2K";
pub const OJPH_PN_STRING_CINEMAS4K: &str = "CINEMAS4K";
pub const OJPH_PN_STRING_BROADCAST: &str = "BROADCAST";
pub const OJPH_PN_STRING_IMF: &str = "IMF";

/// `OJPH_TILEPART_DIVISIONS`: bit flags, kept as `u32` constants because
/// they are OR-ed together.
pub mod tilepart_divisions {
    /// no divisions to tile parts
    pub const OJPH_TILEPART_NO_DIVISIONS: u32 = 0x0;
    pub const OJPH_TILEPART_RESOLUTIONS: u32 = 0x1;
    pub const OJPH_TILEPART_COMPONENTS: u32 = 0x2;
    /// these are meaningless with HTJ2K
    pub const OJPH_TILEPART_LAYERS: u32 = 0x4;
    /// mask used for testing
    pub const OJPH_TILEPART_MASK: u32 = 0x3;
}

// ---------------------------------------------------------------------------
// Big-endian writing; the C++ `outfile_base::write` of `swap_bytes_if_le`
// values. Writing to a `Vec<u8>` cannot fail.

#[inline]
pub(crate) fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

#[inline]
pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

#[inline]
pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// `population_count` from `ojph_arch.h`.
#[inline]
pub(crate) fn population_count(v: u32) -> u32 {
    v.count_ones()
}

/// `31 - count_leading_zeros(v)`, wrapping like the C++ for `v == 0`
/// (`count_leading_zeros(0)` is 32 there, giving `0xFFFFFFFF`).
#[inline]
pub(crate) fn log2_floor(v: u32) -> u32 {
    31u32.wrapping_sub(v.leading_zeros())
}
