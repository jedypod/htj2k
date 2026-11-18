// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Codestream marker segments and their parameters, after
//! `ojph_params.cpp`, `ojph_params_local.h` and the public wrappers in
//! `ojph_params.h`.
//!
//! There is one struct per marker segment carrying both the `local::`
//! methods and the public `ojph::` setters/getters. Methods keep the C++
//! names in snake case (a few keep C++ capitals, e.g. `get_Kmax`, with a
//! snake-case alias where callers asked for one). Every C++ `OJPH_ERROR`
//! becomes an `Err` with the same code and text, so functions that can
//! raise one return `crate::Result`.
//!
//! Ownership replaces the C++ pointers between objects: the main COD, QCD,
//! NLT, DFS and ATK objects own their per-component / further objects in a
//! `Vec` (C++ `next` lists), and objects that pointed at others either
//! get them as arguments or keep copies. The C++ `avail` free lists become
//! `Spare` lists: `restart` moves the per-component COD and NLT objects
//! there, and the next ones added reuse their storage, so that a
//! codestream with the parameters of the last one allocates nothing for
//! them.
//!
//! # Signatures that differ from C++
//!
//! | C++ | Rust | why |
//! |-----|------|-----|
//! | `X::write(outfile_base*)` | `write(&mut self, &mut Vec<u8>) -> bool` | output is a byte vector; always true |
//! | `X::read(infile_base*)` (void/bool) | `read(&mut self, &mut MemInfile) -> Result<()>` | errors are returned; `ParamSot::read` keeps `Result<bool>` |
//! | `param_siz::link(const param_cod*)`, `link(const param_dfs*)`, `set_cod(const param_cod&)` | `link(&ParamCod, Option<&ParamDfs>)`, `set_cod(&ParamCod)` | store *copies*; call again if COD/DFS change |
//! | `param_siz::get_num_components() -> ui16` | `-> u32` | callers widen it |
//! | `param_siz::set_comp_info` / public `set_component` (void, asserts) | both exist, `-> Result<()>` | same function; errors for a missing component, a downsampling outside 1..=255 or a depth outside 1..=38 |
//! | `param_cod::get_num_decompositions() -> ui8` | `-> u32` | callers widen it |
//! | `param_cod::get_coc(comp)` on a COC object | returns the COC itself | children have no back pointer; call it on the main COD |
//! | `p = cod.add_coc_object(UNKNOWN); p->read_coc(file, n, &cod)` + codestream checks | `cod.read_coc(file, n)` on the main COD | adds, reads, then the codestream's duplicate check (error 0x00030057); the per-object parser is `read_coc_segment(file, n, &top)` |
//! | `param_cod::update_atk(param_atk*)` | `update_atk(&ParamAtk)` | each COD/COC stores a resolved copy of its kernel; `atk` is not modified (C++ `get_atk` may append the default 9/7 or 5/3 kernel; `ParamAtk::resolve_atk` returns the same kernel without doing so) |
//! | `param_cod::access_atk() -> const param_atk*` | `-> &ParamAtk` | an empty kernel (no steps, irreversible) before `update_atk` instead of NULL |
//! | public `param_cod::set_num_decomposition(comp, n)`, `set_block_dims(comp, w, h)`, `set_precinct_size(comp, n, size*)`, `set_reversible(comp, b)` and the `(comp, ...)` getters | `*_comp` suffix, e.g. `set_num_decomposition_comp`, `get_block_dims_comp` | no overloading |
//! | `set_precinct_size(int num_levels, size*)` | `set_precinct_size(&[Size])` | empty slice = `num_levels == 0 \|\| NULL` |
//! | `get_progression_order() -> int` | same, plus `get_progression_order_enum() -> Option<ProgressionOrder>` | a codestream may carry any value |
//! | `param_qcd::set_delta(delta, n, w*)`, `set_delta(comp, delta)`, `set_delta(comp, delta, n, w*)` | `set_delta_weights`, `set_delta_comp`, `set_delta_comp_weights` | no overloading; weights as `&[f32]` |
//! | `param_qcd::set_weights(n, w*)` | `set_weights(&[f32]) -> Result<()>` | |
//! | `param_qcd::set_qfactor(comp, ctype, q)` | `set_qfactor_comp` | no overloading |
//! | private `param_qcd::set_irrev_quant(num_decomps)` | `set_irrev_quant_decomps` | name taken by the public `set_irrev_quant(delta)` |
//! | public `param_qcd::set_irrev_quant(delta, n, w[])`, `(comp, delta)`, `(comp, delta, n, w[])` | `set_irrev_quant_weights`, `set_irrev_quant_comp`, `set_irrev_quant_comp_weights` | no overloading |
//! | `p = qcd.add_qcc_object(UNKNOWN); p->read_qcc(file, n)` + codestream checks | `qcd.read_qcc(file, n)` on the main QCD | adds, reads, then the codestream's checks (errors 0x00030054, 0x00030055); the per-object parser is `read_qcc_segment` |
//! | `param_qcd::get_Kmax(const param_dfs*, ...)`, `get_irrev_delta(const param_dfs*, ...)` | `Option<&ParamDfs>` | `get_Kmax` returns `u32` (no error in C++; alias `get_kmax`); `get_irrev_delta` returns `Result<f32>` (error 0x00050101) |
//! | `param_qcd::propose_precision(const param_cod*)` | `propose_precision(cod, main_cod, top_qcd)` | C++ reaches the main COD and the other QCCs through back pointers |
//! | `param_qcd::get_qcc(comp)` on a QCC object | returns the QCC itself | as `get_coc` |
//! | `param_nlt::set_nonlinear_transform(comp, bd, signed, dmin, dmax, pt, n, void* points, type, approx, enc)` | `set_nonlinear_transform_lut(..., points: &[u32], ...)` | values, truncated to the `pt_val` width; fewer than `n` is error 0x000501A5 |
//! | `param_nlt::get_nonlinear_transform(comp, ui8&, bool&, ui8&) -> bool` | `-> Option<(u8, bool, u8)>` | |
//! | `param_nlt::find_unsupported_nlt -> int` (-1 if none) | `-> Option<u32>` | |
//! | `param_nlt::get_nlt_rec -> const nlt_rec*` | `-> Option<&NltRec>` | |
//! | `nlt_rec` tables (`dec_points[-1]`, `approx_enc_points[-2]`, ...) | separate `Vec`s, C++ index `i` at `i + NltRec::DEC_POINTS_OFFSET` / `ENC_POINTS_OFFSET` | no pointer arithmetic |
//! | `nlt_rec::marker_points` (`void*` to ui8/ui16/ui32) | `Vec<u32>` of values | |
//! | `param_dfs::get_dfs(int)` | `get_dfs(u16) -> Option<&ParamDfs>` | the argument is always `get_dfs_index()` |
//! | `param_atk::get_atk(int) -> param_atk*` | `get_atk(&mut self, i32) -> Option<&ParamAtk>` | |
//! | `param_atk::get_K()` | `get_k()` (and `get_K()`) | |
//! | `param_sot::init(...)` with default arguments | all four arguments required | |
//! | `param_sot::write(file, len, TPsot, TNsot)` | `write_with_parts` | no overloading |
//! | `param_tlm::init(n, Ttlm_Ptlm_pair* store)` | `init(n) -> Result<()>` | owns its storage |
//! | `comment_exchange::set_string(const char*)`, `set_data(const char*, ui16)` | `set_string(&str)`, `set_data(&[u8])`, both `-> Result<()>` | owns a copy of the bytes |
//! | `JP2K_MARKER` enum | `marker` (= `jp2k_marker`) module of `u16` constants | matched against values read |

#![allow(dead_code)]

mod atk;
mod cap;
mod cod;
mod comment;
mod dfs;
mod markers;
mod nlt;
mod qcd;
mod siz;
mod sot;
mod spare;
mod tables;

pub use atk::{LiftingStep, ParamAtk};
pub use cap::ParamCap;
pub use cod::{CodSGcod, CodSPcod, CodType, ParamCod};
pub use comment::CommentExchange;
pub use dfs::{DfsDwtType, ParamDfs};
pub use markers::jp2k_marker as marker;
pub use markers::{
    jp2k_marker, tilepart_divisions, ProfileNum, ProgressionOrder, OJPH_PN_STRING_BROADCAST,
    OJPH_PN_STRING_CINEMA2K, OJPH_PN_STRING_CINEMA4K, OJPH_PN_STRING_CINEMAS2K,
    OJPH_PN_STRING_CINEMAS4K, OJPH_PN_STRING_IMF, OJPH_PN_STRING_PROFILE0, OJPH_PN_STRING_PROFILE1,
    OJPH_PO_CPRL, OJPH_PO_LRCP, OJPH_PO_PCRL, OJPH_PO_RLCP, OJPH_PO_RPCL, OJPH_PO_STRING_CPRL,
    OJPH_PO_STRING_LRCP, OJPH_PO_STRING_PCRL, OJPH_PO_STRING_RLCP, OJPH_PO_STRING_RPCL,
};
pub use nlt::{nonlinearity, NltRec, NltType, ParamNlt, ALL_COMPS};
pub use qcd::{CompType, ParamQcd, QcdType, MAX_SUBBAND_COUNT};
pub use siz::{ParamSiz, SizCompInfo};
pub use sot::{ParamSot, ParamTlm, TtlmPtlmPair};

#[cfg(test)]
pub(crate) mod reuse_tests;
#[cfg(test)]
mod tests;
