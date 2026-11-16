// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! HT block decoder for 64-bit samples (port of the generic
//! `ojph_block_decoder64.cpp`).
//!
//! The C++ performs no range check on `missing_msbs` here, so for
//! `missing_msbs > 60` some shift amounts are out of range (undefined
//! behaviour in C++). This port uses `wrapping_shl`, i.e. the shift count is
//! taken modulo the operand width, which is what x86-64 (and AArch64)
//! shift instructions do with the code the C++ compiles to.
//!
//! # Divergence from OpenJPH 0.33.0
//!
//! The significance propagation pass gives a newly significant sample the
//! value `3 << (p - 2)`, where `p = 62 - missing_msbs`. OpenJPH computes
//! it as `3u << (p - 2)`, a 32-bit shift
//! (`ojph_block_decoder64.cpp:1549`), although `p - 2` can be as large as
//! 60. For `missing_msbs` of 29 the result loses its top bit, and for 28
//! or less the shift is undefined behaviour. This port shifts in 64 bits,
//! which is what the standard asks for, so its SigProp output differs from
//! OpenJPH's for `missing_msbs <= 29`. The 32-bit decoder is unaffected,
//! and so is any block coded with the cleanup pass only.

use super::readers::{Frwd, Mel, Rev};
use super::tables::{UVLC_BIAS, UVLC_TBL0, UVLC_TBL1, VLC_TBL0, VLC_TBL1};
use super::{check_args, put, xor, ScratchLayout, SCRATCH_LEN, V_N_SIZE};

/// Decodes one MagSgn sample of a quad (the block repeated four times per
/// quad in the C++). Returns `(val, v_n)`; `v_n` is 0 for an insignificant
/// sample.
#[inline(always)]
fn magsgn_sample(ms: &mut Frwd<'_, 0xFF>, inf: u32, bit: u32, u_q: u32, p: u32) -> (u64, u64) {
    if inf & (1 << (4 + bit)) != 0 {
        let ms_val = ms.fetch64();
        let m_n = u_q.wrapping_sub((inf >> (12 + bit)) & 1); // remove e_k
        ms.advance(m_n);
        let mut val = ms_val << 63; // sign bit
        let mut v_n = ms_val & 1u64.wrapping_shl(m_n).wrapping_sub(1);
        v_n |= (((inf >> (8 + bit)) & 1) as u64).wrapping_shl(m_n); // EMB e_1
        v_n |= 1; // center of bin
        val |= v_n.wrapping_add(2).wrapping_shl(p.wrapping_sub(1));
        (val, v_n)
    } else {
        (0, 0)
    }
}

/// Decodes one HTJ2K code-block into 64-bit sign-magnitude samples; port
/// of OpenJPH's generic `ojph_decode_codeblock64`, with the same argument
/// order and the same return value for every input.
///
/// * `coded_data` holds the cleanup pass (`lengths1` bytes) followed by the
///   SigProp/MagRef passes (`lengths2` bytes). It may be longer; no
///   padding is needed after the data (OpenJPH's callers allocate 8 prefix
///   and 16 suffix bytes, but the generic C++ decoder never reads them:
///   all of its reads are bounded by segment lengths, and exhausted
///   segments are extended with synthetic `0xFF`/`0x00` bytes, which this
///   port reproduces). The input is never modified.
/// * `decoded_data` receives sample `(x, y)` at `y * stride + x`; it must
///   hold at least `stride * height` samples.
///
/// Returns `false`, like the C++, for malformed data and unsupported
/// parameters (more than 3 passes, `lengths1 < 2`,
/// invalid MEL/VLC length, a `U_q` larger than `missing_msbs + 2`).
/// It also returns `false`, where the C++ would access memory out of
/// bounds, when `coded_data` is shorter than `lengths1 + lengths2`,
/// `decoded_data` is too small, or the block is so large that the C++
/// would overrun its fixed-size stack scratch arrays (never the case for
/// a block size JPEG 2000 allows; see `check_args` in `coding/mod.rs`).
///
/// Writes the C++ makes outside the block are dropped when they fall
/// outside `decoded_data` (and performed when inside it, exactly like the
/// C++): the cleanup pass always writes rows in pairs, so an odd `height`
/// produces writes to row `height`, and corrupt data can make the MagRef
/// pass touch samples to the right of or below the block. Pass a buffer
/// with OpenJPH's layout if those side effects matter.
///
/// Scratch memory (about 13 kB, the same arrays the C++ keeps on its
/// stack) lives on the stack; nothing is allocated on the heap.
#[allow(clippy::too_many_arguments)]
pub fn decode_codeblock64(
    coded_data: &[u8],
    decoded_data: &mut [u64],
    missing_msbs: u32,
    mut num_passes: u32,
    lengths1: u32,
    lengths2: u32,
    width: u32,
    height: u32,
    stride: u32,
    stripe_causal: bool,
) -> bool {
    if num_passes > 1 && lengths2 == 0 {
        // OJPH_WARN(0x00010001): more than one pass but zero length.
        num_passes = 1;
    }
    if num_passes > 3 {
        return false; // OJPH_WARN(0x00010002)
    }
    // (the C++ has the 32-bit decoder's missing_msbs checks commented out)
    let p = 62u32.wrapping_sub(missing_msbs);

    if lengths1 < 2 {
        return false; // OJPH_WARN(0x00010006)
    }
    let layout = match check_args(
        coded_data.len(),
        decoded_data.len(),
        num_passes,
        lengths1,
        lengths2,
        width,
        height,
        stride,
    ) {
        Some(l) => l,
        None => return false,
    };

    let lcup = lengths1 as i32;
    let scup = ((coded_data[(lcup - 1) as usize] as i32) << 4)
        + (coded_data[(lcup - 2) as usize] as i32 & 0xF);
    if scup < 2 || scup > lcup || scup > 4079 {
        return false;
    }

    let mut scratch = [0u16; SCRATCH_LEN];
    let mmsbp2 = missing_msbs.wrapping_add(2);

    decode_vlc_mel64(
        &mut scratch,
        coded_data,
        lcup,
        scup,
        width,
        height,
        layout.sstr,
    );

    if !decode_magsgn64(
        &scratch,
        coded_data,
        decoded_data,
        lcup,
        scup,
        width,
        height,
        stride,
        p,
        mmsbp2,
        layout,
    ) {
        return false;
    }

    if num_passes > 1 {
        let mstr = super::sigma_from_quads(&mut scratch, width, height, layout.sstr);
        spp64(
            &scratch,
            coded_data,
            decoded_data,
            lengths1,
            lengths2,
            width,
            height,
            stride,
            mstr,
            p,
            stripe_causal,
        );
        if num_passes > 2 {
            mrp64(
                &scratch,
                coded_data,
                decoded_data,
                lengths1,
                lengths2,
                width,
                height,
                stride,
                mstr,
                p,
            );
        }
    }
    true
}

/// Step 1 of the cleanup pass: decode the VLC and MEL segments into the
/// interleaved `scratch` (`inf`, `u_q` per quad).
fn decode_vlc_mel64(
    scratch: &mut [u16; SCRATCH_LEN],
    data: &[u8],
    lcup: i32,
    scup: i32,
    width: u32,
    height: u32,
    sstr: usize,
) {
    let mut mel = Mel::new(data, lcup, scup);
    let mut vlc = Rev::new_vlc8(data, lcup, scup);

    let mut run = mel.get_run();

    let mut c_q: u32 = 0;
    let mut sp: usize = 0;
    // initial quad row
    let mut x = 0u32;
    while x < width {
        // first quad
        let mut vlc_val = vlc.fetch64();
        let mut t0 = VLC_TBL0[(c_q + (vlc_val & 0x7F) as u32) as usize];
        if c_q == 0 {
            run -= 2;
            t0 = if run == -1 { t0 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        scratch[sp] = t0;
        x += 2;

        c_q = ((t0 as u32 & 0x10) << 3) | ((t0 as u32 & 0xE0) << 2);
        vlc_val = vlc.advance(t0 as u32 & 0x7);

        // second quad
        let mut t1 = VLC_TBL0[(c_q + (vlc_val & 0x7F) as u32) as usize];
        if c_q == 0 && x < width {
            run -= 2;
            t1 = if run == -1 { t1 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        t1 = if x < width { t1 } else { 0 };
        scratch[sp + 2] = t1;
        x += 2;

        c_q = ((t1 as u32 & 0x10) << 3) | ((t1 as u32 & 0xE0) << 2);
        vlc_val = vlc.advance(t1 as u32 & 0x7);

        // decode u
        let mut uvlc_mode = ((t0 as u32 & 0x8) << 3) | ((t1 as u32 & 0x8) << 4);
        if uvlc_mode == 0xC0 {
            run -= 2;
            uvlc_mode += if run == -1 { 0x40 } else { 0 };
            if run < 0 {
                run = mel.get_run();
            }
        }
        let idx = (uvlc_mode + (vlc_val & 0x3F) as u32) as usize;
        let mut uvlc_entry = UVLC_TBL0[idx] as u32;
        let u_bias = UVLC_BIAS[idx] as i32;
        vlc_val = vlc.advance(uvlc_entry & 0x7);
        uvlc_entry >>= 3;
        let mut len = uvlc_entry & 0xF;
        let tmp = (vlc_val & ((1u64 << len) - 1)) as u32;
        vlc_val = vlc.advance(len);
        uvlc_entry >>= 4;
        len = uvlc_entry & 0x7;
        uvlc_entry >>= 3;
        let mut u_q0 = ((uvlc_entry & 7) + (tmp & !(0xFFu32 << len))) as u16;
        let mut u_q1 = ((uvlc_entry >> 3) + (tmp >> len)) as u16;

        // u_q extensions, needed only when u_q > 32 (int arithmetic in C++)
        let cond0 = u_q0 as i32 - (u_bias & 0x3) > 32;
        let u_ext = if cond0 { (vlc_val & 0xF) as u16 } else { 0 };
        vlc_val = vlc.advance(if cond0 { 4 } else { 0 });
        u_q0 = u_q0.wrapping_add(u_ext << 2);
        scratch[sp + 1] = u_q0.wrapping_add(1); // kappa = 1
        let cond1 = u_q1 as i32 - (u_bias >> 2) > 32;
        let u_ext = if cond1 { (vlc_val & 0xF) as u16 } else { 0 };
        vlc.advance(if cond1 { 4 } else { 0 });
        u_q1 = u_q1.wrapping_add(u_ext << 2);
        scratch[sp + 3] = u_q1.wrapping_add(1); // kappa = 1
        sp += 4;
    }
    scratch[sp] = 0;
    scratch[sp + 1] = 0;

    // non-initial quad rows
    let mut y = 2u32;
    while y < height {
        c_q = 0;
        let mut sp = (y >> 1) as usize * sstr;
        let mut x = 0u32;
        while x < width {
            c_q |= (scratch[sp - sstr] as u32 & 0xA0) << 2;
            c_q |= (scratch[sp + 2 - sstr] as u32 & 0x20) << 4;

            let mut vlc_val = vlc.fetch64();
            let mut t0 = VLC_TBL1[(c_q + (vlc_val & 0x7F) as u32) as usize];
            if c_q == 0 {
                run -= 2;
                t0 = if run == -1 { t0 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            scratch[sp] = t0;
            x += 2;

            c_q = ((t0 as u32 & 0x40) << 2) | ((t0 as u32 & 0x80) << 1);
            c_q |= scratch[sp - sstr] as u32 & 0x80;
            c_q |= (scratch[sp + 2 - sstr] as u32 & 0xA0) << 2;
            c_q |= (scratch[sp + 4 - sstr] as u32 & 0x20) << 4;

            vlc_val = vlc.advance(t0 as u32 & 0x7);

            let mut t1 = VLC_TBL1[(c_q + (vlc_val & 0x7F) as u32) as usize];
            if c_q == 0 && x < width {
                run -= 2;
                t1 = if run == -1 { t1 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            t1 = if x < width { t1 } else { 0 };
            scratch[sp + 2] = t1;
            x += 2;

            c_q = ((t1 as u32 & 0x40) << 2) | ((t1 as u32 & 0x80) << 1);
            c_q |= scratch[sp + 2 - sstr] as u32 & 0x80;

            vlc_val = vlc.advance(t1 as u32 & 0x7);

            let uvlc_mode = ((t0 as u32 & 0x8) << 3) | ((t1 as u32 & 0x8) << 4);
            let mut uvlc_entry = UVLC_TBL1[(uvlc_mode + (vlc_val & 0x3F) as u32) as usize] as u32;
            vlc_val = vlc.advance(uvlc_entry & 0x7);
            uvlc_entry >>= 3;
            let mut len = uvlc_entry & 0xF;
            let tmp = (vlc_val & ((1u64 << len) - 1)) as u32;
            vlc_val = vlc.advance(len);
            uvlc_entry >>= 4;
            len = uvlc_entry & 0x7;
            uvlc_entry >>= 3;
            let mut u_q0 = ((uvlc_entry & 7) + (tmp & !(0xFFu32 << len))) as u16;
            let mut u_q1 = ((uvlc_entry >> 3) + (tmp >> len)) as u16;

            // u_q extensions, needed only when u_q > 32
            let cond0 = u_q0 > 32;
            let u_ext = if cond0 { (vlc_val & 0xF) as u16 } else { 0 };
            vlc_val = vlc.advance(if cond0 { 4 } else { 0 });
            u_q0 = u_q0.wrapping_add(u_ext << 2);
            scratch[sp + 1] = u_q0;
            let cond1 = u_q1 > 32;
            let u_ext = if cond1 { (vlc_val & 0xF) as u16 } else { 0 };
            vlc.advance(if cond1 { 4 } else { 0 });
            u_q1 = u_q1.wrapping_add(u_ext << 2);
            scratch[sp + 3] = u_q1;
            sp += 4;
        }
        scratch[sp] = 0;
        scratch[sp + 1] = 0;
        y += 2;
    }
}

/// Step 2 of the cleanup pass: decode the MagSgn segment. Returns `false`
/// when a `U_q` exceeds `mmsbp2`.
#[allow(clippy::too_many_arguments)]
fn decode_magsgn64(
    scratch: &[u16; SCRATCH_LEN],
    data: &[u8],
    dec: &mut [u64],
    lcup: i32,
    scup: i32,
    width: u32,
    height: u32,
    stride: u32,
    p: u32,
    mmsbp2: u32,
    layout: ScratchLayout,
) -> bool {
    let sstr = layout.sstr;
    let stride = stride as usize;
    let mut v_n_scratch = [0u64; V_N_SIZE];
    let mut magsgn = Frwd::<0xFF>::new8(data, 0, lcup - scup);

    let mut y = 0u32;
    while y == 0 || y < height {
        let initial = y == 0;
        let mut sp = (y >> 1) as usize * sstr;
        let mut vp = 0usize;
        let mut dp = y as usize * stride;
        let mut prev_v_n = 0u64;
        let mut x = 0u32;
        while x < width {
            let inf = scratch[sp] as u32;
            let u_q = scratch[sp + 1] as u32;
            let big_u_q = if initial {
                u_q
            } else {
                let mut gamma = inf & 0xF0;
                gamma &= gamma.wrapping_sub(0x10);
                let emax = 63 - (2 | v_n_scratch[vp] | v_n_scratch[vp + 1]).leading_zeros();
                let kappa = if gamma != 0 { emax } else { 1 };
                u_q.wrapping_add(kappa)
            };
            if big_u_q > mmsbp2 {
                return false;
            }

            let (val, _) = magsgn_sample(&mut magsgn, inf, 0, big_u_q, p);
            put(dec, dp, val);
            let (val, v_n) = magsgn_sample(&mut magsgn, inf, 1, big_u_q, p);
            put(dec, dp + stride, val);
            v_n_scratch[vp] = prev_v_n | v_n;
            prev_v_n = 0;
            dp += 1;
            x += 1;
            if x >= width {
                vp += 1;
                break;
            }

            let (val, _) = magsgn_sample(&mut magsgn, inf, 2, big_u_q, p);
            put(dec, dp, val);
            let (val, v_n) = magsgn_sample(&mut magsgn, inf, 3, big_u_q, p);
            put(dec, dp + stride, val);
            prev_v_n = v_n;
            dp += 1;
            x += 1;
            sp += 2;
            vp += 1;
        }
        v_n_scratch[vp] = prev_v_n;
        y += 2;
    }
    true
}

/// Significance propagation pass.
#[allow(clippy::too_many_arguments, clippy::explicit_counter_loop)]
fn spp64(
    sigma: &[u16; SCRATCH_LEN],
    data: &[u8],
    dec: &mut [u64],
    lengths1: u32,
    lengths2: u32,
    width: u32,
    height: u32,
    stride: u32,
    mstr: usize,
    p: u32,
    stripe_causal: bool,
) {
    let stride = stride as usize;
    let mut prev_row_sig = [0u16; 256 + 8];
    let mut sigprop = Frwd::<0>::new(data, lengths1 as usize, lengths2 as i32);

    let mut y = 0u32;
    while y < height {
        let mut pattern: u32 = 0xFFFF;
        if height - y < 4 {
            pattern = 0x7777;
            if height - y < 3 {
                pattern = 0x3333;
                if height - y < 2 {
                    pattern = 0x1111;
                }
            }
        }

        let mut prev: u32 = 0;
        let mut prev_sig = 0usize;
        let mut cur_sig = (y >> 2) as usize * mstr;
        let dpp = y as usize * stride;
        let mut x = 0u32;
        while x < width {
            let s = (x as i32 + 4 - width as i32).max(0);
            pattern >>= s * 4;

            let ps = prev_row_sig[prev_sig] as u32 | ((prev_row_sig[prev_sig + 1] as u32) << 16);
            let ns = sigma[cur_sig + mstr] as u32 | ((sigma[cur_sig + mstr + 1] as u32) << 16);
            let mut u = (ps & 0x8888_8888) >> 3;
            if !stripe_causal {
                u |= (ns & 0x1111_1111) << 3;
            }

            let cs = sigma[cur_sig] as u32 | ((sigma[cur_sig + 1] as u32) << 16);
            let mut mbr = cs;
            mbr |= (cs & 0x7777_7777) << 1;
            mbr |= (cs & 0xEEEE_EEEE) >> 1;
            mbr |= u;
            let t = mbr;
            mbr |= t << 4;
            mbr |= t >> 4;
            mbr |= prev >> 12;

            mbr &= pattern;
            mbr &= !cs;

            let mut new_sig = mbr;
            if new_sig != 0 {
                let mut cwd = sigprop.fetch() as u64;
                let mut cnt = 0u32;
                let mut col_mask = 0xFu32;
                let inv_sig = !cs & pattern;
                let mut i = 0;
                while i < 16 {
                    if (col_mask & new_sig) != 0 {
                        let mut sample_mask = 0x1111u32 & col_mask;
                        if new_sig & sample_mask != 0 {
                            new_sig &= !sample_mask;
                            if cwd & 1 != 0 {
                                new_sig |= (0x33u32 << i) & inv_sig;
                            }
                            cwd >>= 1;
                            cnt += 1;
                        }
                        sample_mask <<= 1;
                        if new_sig & sample_mask != 0 {
                            new_sig &= !sample_mask;
                            if cwd & 1 != 0 {
                                new_sig |= (0x76u32 << i) & inv_sig;
                            }
                            cwd >>= 1;
                            cnt += 1;
                        }
                        sample_mask <<= 1;
                        if new_sig & sample_mask != 0 {
                            new_sig &= !sample_mask;
                            if cwd & 1 != 0 {
                                new_sig |= (0xECu32 << i) & inv_sig;
                            }
                            cwd >>= 1;
                            cnt += 1;
                        }
                        sample_mask <<= 1;
                        if new_sig & sample_mask != 0 {
                            new_sig &= !sample_mask;
                            if cwd & 1 != 0 {
                                new_sig |= (0xC8u32 << i) & inv_sig;
                            }
                            cwd >>= 1;
                            cnt += 1;
                        }
                    }
                    i += 4;
                    col_mask <<= 4;
                }

                if new_sig != 0 {
                    let mut dp = dpp + x as usize;
                    // a 64-bit shift, unlike the C++ (see the module docs)
                    let val = 3u64.wrapping_shl(p.wrapping_sub(2));
                    col_mask = 0xF;
                    for _ in 0..4 {
                        if (col_mask & new_sig) != 0 {
                            let mut sample_mask = 0x1111u32 & col_mask;
                            if new_sig & sample_mask != 0 {
                                put(dec, dp, (cwd << 63) | val);
                                cwd >>= 1;
                                cnt += 1;
                            }
                            sample_mask += sample_mask;
                            if new_sig & sample_mask != 0 {
                                put(dec, dp + stride, (cwd << 63) | val);
                                cwd >>= 1;
                                cnt += 1;
                            }
                            sample_mask += sample_mask;
                            if new_sig & sample_mask != 0 {
                                put(dec, dp + 2 * stride, (cwd << 63) | val);
                                cwd >>= 1;
                                cnt += 1;
                            }
                            sample_mask += sample_mask;
                            if new_sig & sample_mask != 0 {
                                put(dec, dp + 3 * stride, (cwd << 63) | val);
                                cwd >>= 1;
                                cnt += 1;
                            }
                        }
                        dp += 1;
                        col_mask <<= 4;
                    }
                }
                sigprop.advance(cnt);
            }

            new_sig |= cs;
            prev_row_sig[prev_sig] = new_sig as u16;

            let t = new_sig;
            new_sig |= (t & 0x7777) << 1;
            new_sig |= (t & 0xEEEE) >> 1;
            prev = new_sig | u;
            prev &= 0xF000;

            x += 4;
            cur_sig += 1;
            prev_sig += 1;
        }
        y += 4;
    }
}

/// Magnitude refinement pass.
#[allow(clippy::too_many_arguments)]
fn mrp64(
    sigma: &[u16; SCRATCH_LEN],
    data: &[u8],
    dec: &mut [u64],
    lengths1: u32,
    lengths2: u32,
    width: u32,
    height: u32,
    stride: u32,
    mstr: usize,
    p: u32,
) {
    let stride = stride as usize;
    let mut magref = Rev::new_mrp(data, lengths1 as i32, lengths2 as i32);
    let half = 1u64.wrapping_shl(p.wrapping_sub(2));
    let mut y = 0u32;
    while y < height {
        let mut cur_sig = (y >> 2) as usize * mstr;
        let dpp = y as usize * stride;
        let mut i = 0u32;
        while i < width {
            let mut cwd = magref.fetch();
            let sig = sigma[cur_sig] as u32 | ((sigma[cur_sig + 1] as u32) << 16);
            let mut col_mask = 0xFu32;
            if sig != 0 {
                for j in 0..8usize {
                    if sig & col_mask != 0 {
                        let mut dp = dpp + i as usize + j;
                        let mut sample_mask = 0x1111_1111u32 & col_mask;
                        for _ in 0..4 {
                            if sig & sample_mask != 0 {
                                let mut sym = (cwd & 1) as u64;
                                sym = (1 - sym).wrapping_shl(p.wrapping_sub(1));
                                sym |= half;
                                xor(dec, dp, sym);
                                cwd >>= 1;
                            }
                            sample_mask = sample_mask.wrapping_add(sample_mask);
                            dp += stride;
                        }
                    }
                    col_mask <<= 4;
                }
            }
            magref.advance(sig.count_ones());
            i += 8;
            cur_sig += 2;
        }
        y += 4;
    }
}
