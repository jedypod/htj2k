// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Packets: reading the packet header of one precinct, after
//! `ojph_precinct.cpp`.

use crate::codestream::bitbuffer_read::{BitReadBuf, ParseResult};
use crate::codestream::subband::Subband;
use crate::file::{MemInfile, SeekFrom};
use crate::types::{Point, Rect, Size};

#[derive(Debug, Clone, Default)]
pub(crate) struct Precinct {
    /// The precinct's top left corner projected to full resolution.
    pub img_point: Point,
    /// Which code blocks of each subband are in this precinct.
    pub cb_idxs: [Rect; 4],
    pub may_use_sop: bool,
    pub uses_eph: bool,
}

/// A tag tree over a scratch buffer.
///
/// Level 0 has one node per code block, each level above has half the
/// width and height rounded up, and one further level holds the single
/// value above the root. OpenJPH lays the levels out at offsets fixed per
/// resolution; here they are packed per precinct, which changes nothing
/// that can be observed.
struct TagTree {
    base: usize,
    width: u32,
    lev_off: [usize; 34],
}

impl TagTree {
    /// The number of bytes a tree for `s` code blocks needs.
    fn size(num_levels: u32, s: Size) -> usize {
        let mut total = 0usize;
        for lev in 0..=num_levels {
            total += Self::level_width(s.w, lev) as usize * Self::level_width(s.h, lev) as usize;
        }
        total
    }

    #[inline]
    fn level_width(w: u32, lev: u32) -> u32 {
        ((w as u64 + (1u64 << lev) - 1) >> lev) as u32
    }

    fn new(base: usize, num_levels: u32, s: Size) -> Self {
        let mut lev_off = [0usize; 34];
        let mut off = 0usize;
        for lev in 0..=num_levels {
            lev_off[lev as usize] = off;
            off += Self::level_width(s.w, lev) as usize * Self::level_width(s.h, lev) as usize;
        }
        TagTree {
            base,
            width: s.w,
            lev_off,
        }
    }

    #[inline]
    fn idx(&self, x: u32, y: u32, lev: u32) -> usize {
        self.base
            + self.lev_off[lev as usize]
            + x as usize
            + y as usize * Self::level_width(self.width, lev) as usize
    }
}

#[inline]
fn log2ceil(x: u32) -> u32 {
    let t = 31 - x.leading_zeros();
    t + u32::from(x & (x - 1) != 0)
}

impl Precinct {
    /// Parses one packet: its header, then the location of each code
    /// block's bytes. `data_left` is the number of bytes left in the
    /// tile part, and is updated.
    pub(crate) fn parse(
        &self,
        bands: &mut [Subband; 4],
        scratch: &mut Vec<u8>,
        data_left: &mut u32,
        file: &mut MemInfile,
        skipped: bool,
    ) -> ParseResult<()> {
        debug_assert!(*data_left > 0);
        let mut bb = BitReadBuf::new(*data_left, file);
        if self.may_use_sop {
            bb.skip_sop()?;
        }

        let mut empty_packet = true;
        for (band, cb_idxs) in bands.iter_mut().zip(self.cb_idxs.iter()) {
            if band.empty {
                continue;
            }
            if cb_idxs.siz.w == 0 || cb_idxs.siz.h == 0 {
                continue;
            }

            if empty_packet {
                // one bit to check if the packet is empty
                let (bit, _) = bb.read_bit()?;
                if bit == 0 {
                    bb.terminate(self.uses_eph)?;
                    *data_left = bb.bytes_left;
                    return Ok(());
                }
                empty_packet = false;
            }

            let num_levels = 1 + log2ceil(cb_idxs.siz.w).max(log2ceil(cb_idxs.siz.h));
            let tree_size = TagTree::size(num_levels, cb_idxs.siz);
            scratch.clear();
            scratch.resize(4 * tree_size, 0);
            let inc_tag = TagTree::new(0, num_levels, cb_idxs.siz);
            let inc_tag_flags = TagTree::new(tree_size, num_levels, cb_idxs.siz);
            let mmsb_tag = TagTree::new(2 * tree_size, num_levels, cb_idxs.siz);
            let mmsb_tag_flags = TagTree::new(3 * tree_size, num_levels, cb_idxs.siz);

            let band_width = band.num_blocks.w as usize;
            let width = cb_idxs.siz.w;
            let height = cb_idxs.siz.h;
            for y in 0..height {
                let row = cb_idxs.org.x as usize + (y + cb_idxs.org.y) as usize * band_width;
                for x in 0..width {
                    let cp = band
                        .coded_cbs
                        .get_mut(row + x as usize)
                        .ok_or("internal error: code block index out of range")?;

                    // process inclusion
                    let mut empty_cb = false;
                    for cl in (1..=num_levels).rev() {
                        let cur_lev = cl - 1;
                        let i = inc_tag.idx(x >> cur_lev, y >> cur_lev, cur_lev);
                        empty_cb = scratch[i] == 1;
                        if empty_cb {
                            break;
                        }
                        let f = inc_tag_flags.idx(x >> cur_lev, y >> cur_lev, cur_lev);
                        if scratch[f] == 0 {
                            let (bit, ok) = bb.read_bit()?;
                            if !ok {
                                *data_left = 0;
                                return Err("error reading from file p1");
                            }
                            empty_cb = bit == 0;
                            scratch[i] = (1 - bit) as u8;
                            scratch[f] = 1;
                        }
                        if empty_cb {
                            break;
                        }
                    }
                    if empty_cb {
                        continue;
                    }

                    // process missing msbs
                    let mut mmsbs = 0u32;
                    for levp1 in (1..=num_levels).rev() {
                        let cur_lev = levp1 - 1;
                        mmsbs = scratch[mmsb_tag.idx(x >> levp1, y >> levp1, levp1)] as u32;
                        let f = mmsb_tag_flags.idx(x >> cur_lev, y >> cur_lev, cur_lev);
                        if scratch[f] == 0 {
                            let mut bit = 0;
                            while bit == 0 {
                                let (b, ok) = bb.read_bit()?;
                                if !ok {
                                    *data_left = 0;
                                    return Err("error reading from file p2");
                                }
                                bit = b;
                                mmsbs += 1 - bit;
                            }
                            // OpenJPH stores this in a byte as well; a value
                            // that does not fit is far above any K_max
                            scratch[mmsb_tag.idx(x >> cur_lev, y >> cur_lev, cur_lev)] =
                                mmsbs as u8;
                            scratch[f] = 1;
                        }
                    }

                    if mmsbs > cp.k_max {
                        return Err("error in parsing a tile header;                             missing msbs are larger or equal to Kmax. The most likely                             cause is a corruption in the bitstream.");
                    }
                    cp.missing_msbs = mmsbs;

                    // get number of passes
                    let mut num_passes = 1u32;
                    let (bit, ok) = bb.read_bit()?;
                    if !ok {
                        *data_left = 0;
                        return Err("error reading from file p3");
                    }
                    if bit != 0 {
                        num_passes = 2;
                        let (bit, ok) = bb.read_bit()?;
                        if !ok {
                            *data_left = 0;
                            return Err("error reading from file p4");
                        }
                        if bit != 0 {
                            let (bit, ok) = bb.read_bits(2)?;
                            if !ok {
                                *data_left = 0;
                                return Err("error reading from file p5");
                            }
                            num_passes = 3 + bit;
                            if bit == 3 {
                                let (bit, ok) = bb.read_bits(5)?;
                                if !ok {
                                    *data_left = 0;
                                    return Err("error reading from file p6");
                                }
                                num_passes = 6 + bit;
                                if bit == 31 {
                                    let (bit, ok) = bb.read_bits(7)?;
                                    if !ok {
                                        *data_left = 0;
                                        return Err("error reading from file p7");
                                    }
                                    num_passes = 37 + bit;
                                }
                            }
                        }
                    }
                    cp.num_passes = num_passes;

                    // parse pass lengths
                    // for one pass, one length, but for 2 or 3 passes, two lengths
                    let mut num_phld_passes = (num_passes - 1) / 3;
                    cp.missing_msbs += num_phld_passes;

                    num_phld_passes *= 3;
                    cp.num_passes = num_passes - num_phld_passes;
                    cp.pass_length = [0, 0];

                    let mut lblock = 3u32;
                    let mut bit = 1;
                    while bit != 0 {
                        let (b, ok) = bb.read_bit()?;
                        if !ok {
                            *data_left = 0;
                            return Err("error reading from file p8");
                        }
                        bit = b;
                        lblock += bit;
                    }

                    let bits = lblock + 31 - (num_phld_passes + 1).leading_zeros();
                    if bits > 32 {
                        // OpenJPH asserts here
                        return Err(
                            "the length of a codeblock segment is coded with too many bits",
                        );
                    }
                    let (bit, ok) = bb.read_bits(bits)?;
                    if !ok {
                        *data_left = 0;
                        return Err("error reading from file p9");
                    }
                    if bit < 2 {
                        return Err("The cleanup segment of an HT codeblock cannot contain                             less than 2 bytes");
                    }
                    if bit >= 65535 {
                        return Err("The cleanup segment of an HT codeblock must contain                             less than 65535 bytes");
                    }
                    cp.pass_length[0] = bit;
                    if cp.num_passes > 1 {
                        let bits = lblock + u32::from(cp.num_passes > 2);
                        if bits > 32 {
                            return Err(
                                "the length of a codeblock segment is coded with too many bits",
                            );
                        }
                        let (bit, ok) = bb.read_bits(bits)?;
                        if !ok {
                            *data_left = 0;
                            return Err("error reading from file p10");
                        }
                        if bit >= 2047 {
                            return Err("The refinement segment (SigProp and MagRep passes) of                                 an HT codeblock must contain less than 2047 bytes");
                        }
                        cp.pass_length[1] = bit;
                    }
                }
            }
        }
        if empty_packet {
            // all subbands are empty
            bb.read_bit()?;
        }
        bb.terminate(self.uses_eph)?;

        // read codeblock data
        for (band, cb_idxs) in bands.iter_mut().zip(self.cb_idxs.iter()) {
            if band.empty {
                continue;
            }
            let band_width = band.num_blocks.w as usize;
            let width = cb_idxs.siz.w as usize;
            let height = cb_idxs.siz.h as usize;
            for y in 0..height {
                let row = cb_idxs.org.x as usize + (y + cb_idxs.org.y as usize) * band_width;
                for x in 0..width {
                    let cp = band
                        .coded_cbs
                        .get_mut(row + x)
                        .ok_or("internal error: code block index out of range")?;
                    let num_bytes = cp.pass_length[0] + cp.pass_length[1];
                    if *data_left != 0 {
                        if num_bytes != 0 {
                            if skipped {
                                // no need to read
                                let t = num_bytes.min(bb.bytes_left);
                                let cur_loc = bb.file.tell();
                                // as in OpenJPH, a seek past the end does not move
                                bb.file.seek(t as i64, SeekFrom::Current);
                                let bytes_read = (bb.file.tell() - cur_loc) as u32;
                                cp.pass_length = [0, 0];
                                bb.bytes_left -= bytes_read;
                            } else {
                                let (offset, bytes_read, ok) = bb.read_chunk(num_bytes);
                                cp.data = Some(offset);
                                cp.data_len = bytes_read;
                                if !ok {
                                    // no need to decode a broken codeblock
                                    cp.pass_length = [0, 0];
                                    *data_left = 0;
                                }
                            }
                        }
                    } else {
                        cp.pass_length = [0, 0];
                    }
                }
            }
        }
        *data_left = bb.bytes_left;
        Ok(())
    }
}
