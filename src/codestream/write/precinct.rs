// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Packets: one precinct being written, after the write side of
//! `ojph_precinct.cpp` and `ojph_bitbuffer_write.h`.

use super::subband::Subband;
use crate::error::ojph_error;
use crate::types::{Point, Rect};
use crate::Result;

#[derive(Debug, Clone, Default)]
pub(crate) struct Precinct {
    /// The precinct's top left corner projected to full resolution.
    pub img_point: Point,
    /// Which code blocks of each subband are in this precinct.
    pub cb_idxs: [Rect; 4],
    /// The packet header, once `prepare_precinct` has found something to
    /// code (OpenJPH's `coded` list). The vector keeps its capacity from
    /// one codestream to the next.
    header: Vec<u8>,
    /// False for an empty packet, which has no header.
    has_header: bool,
}

/// `bit_write_buf` and its `bb_*` functions: packet header bits, with a
/// zero bit stuffed after every 0xFF byte. OpenJPH writes them into 512
/// byte buffers from its elastic allocator and counts the bytes; one
/// vector holds them here.
struct BitWriteBuf {
    buf: Vec<u8>,
    avail_bits: i32,
    tmp: u64,
}

impl BitWriteBuf {
    /// `bb_init`, writing into `buf`, which must be empty.
    fn new(buf: Vec<u8>) -> Self {
        BitWriteBuf {
            buf,
            avail_bits: 8,
            tmp: 0,
        }
    }

    fn put_bit(&mut self, bit: u32) {
        self.avail_bits -= 1;
        self.tmp |= ((bit & 1) as u64) << self.avail_bits;
        if self.avail_bits <= 0 {
            self.avail_bits = 8 - i32::from(self.tmp == 0xFF);
            self.buf.push((self.tmp & 0xFF) as u8);
            self.tmp = 0;
        }
    }

    fn put_zeros(&mut self, num_zeros: i32) {
        for _ in 0..num_zeros.max(0) {
            self.put_bit(0);
        }
    }

    /// The low `num_bits` bits of `data`, most significant first.
    fn put_bits(&mut self, data: u32, num_bits: u32) {
        for i in (0..num_bits).rev() {
            self.put_bit(data.checked_shr(i).unwrap_or(0));
        }
    }

    /// `bb_terminate`: writes the last, partial byte. After a 0xFF byte
    /// this writes a 0x00 even with no bits pending, as OpenJPH does.
    fn terminate(mut self) -> Vec<u8> {
        if self.avail_bits < 8 {
            self.buf.push((self.tmp & 0xFF) as u8);
        }
        self.buf
    }
}

/// The layout of a tag tree with `num_levels` levels over `w` by `h`
/// code blocks, plus the extra level above the root.
///
/// OpenJPH gives level `i` of a resolution's trees `4^(L-1-i)` bytes for
/// the largest number of levels `L` there, and fills the first
/// `4^(n-1-i)` of them for a tree of `n` levels. Building a level reads
/// the four children of every node without checking them against the
/// edge, so for an odd width the "right" child is the first node of the
/// next row, and past the last row it is a byte of the fill (255 for the
/// value trees). The coded bits depend on those reads. Each level here
/// is a rectangle of its nodes plus one row and one byte, filled the
/// same way, which is exactly the part of OpenJPH's level that such
/// reads reach, so the values read are the same.
///
/// A tree over up to 2^32 blocks a side has at most 33 levels plus the
/// one above the root.
struct TagTree {
    off: [usize; 34],
    width: [usize; 34],
}

impl TagTree {
    /// The layout, and the number of bytes one tree takes.
    fn new(num_levels: u32, w: u32, h: u32) -> (Self, usize) {
        let mut off = [0; 34];
        let mut width = [0; 34];
        let mut total = 0usize;
        for lev in 0..=num_levels {
            let lw = level_size(w, lev);
            let lh = level_size(h, lev);
            off[lev as usize] = total;
            width[lev as usize] = lw;
            total += if lev < num_levels {
                lw * (lh + 1) + 1
            } else {
                1
            };
        }
        (TagTree { off, width }, total)
    }

    #[inline]
    fn idx(&self, x: u32, y: u32, lev: u32) -> usize {
        self.off[lev as usize] + x as usize + y as usize * self.width[lev as usize]
    }
}

/// `ceil(v / 2^lev)`
#[inline]
fn level_size(v: u32, lev: u32) -> usize {
    ((v as u64 + (1u64 << lev) - 1) >> lev) as usize
}

#[inline]
fn log2ceil(x: u32) -> u32 {
    let t = 31 - x.leading_zeros();
    t + u32::from(x & (x - 1) != 0)
}

/// The four trees of one subband in one scratch buffer: inclusion,
/// inclusion flags, missing MSBs, missing-MSB flags.
struct Trees<'a> {
    t: TagTree,
    size: usize,
    buf: &'a mut [u8],
}

impl Trees<'_> {
    /// A byte of tree `tree` (0 to 3).
    fn get(&self, tree: usize, x: u32, y: u32, lev: u32) -> Result<u8> {
        match self.buf.get(tree * self.size + self.t.idx(x, y, lev)) {
            Some(&v) => Ok(v),
            None => ojph_error!(0x00080050, "internal error: tag tree index out of range"),
        }
    }

    fn set(&mut self, tree: usize, x: u32, y: u32, lev: u32, v: u8) -> Result<()> {
        match self.buf.get_mut(tree * self.size + self.t.idx(x, y, lev)) {
            Some(p) => {
                *p = v;
                Ok(())
            }
            None => ojph_error!(0x00080050, "internal error: tag tree index out of range"),
        }
    }
}

const INC: usize = 0;
const INC_FLAGS: usize = 1;
const MMSB: usize = 2;
const MMSB_FLAGS: usize = 3;

impl Precinct {
    /// Forgets the packet header, as `new` leaves the precinct.
    pub(crate) fn reset(&mut self) {
        self.header.clear();
        self.has_header = false;
    }

    /// Codes the packet header, and returns the size of the packet: the
    /// header and the code blocks' bytes, or 1 for an empty packet.
    /// `scratch` holds the tag trees.
    pub(crate) fn prepare_precinct(
        &mut self,
        bands: &[Subband; 4],
        scratch: &mut Vec<u8>,
    ) -> Result<u64> {
        let mut header = std::mem::take(&mut self.header);
        header.clear();
        self.has_header = false;
        let mut bb: Option<BitWriteBuf> = None;
        let mut cb_bytes = 0u64;
        let mut num_skipped_subbands = 0u32;
        for (band, cb) in bands.iter().zip(self.cb_idxs.iter()) {
            if band.empty || cb.siz.w == 0 || cb.siz.h == 0 {
                continue;
            }
            let (w, h) = (cb.siz.w, cb.siz.h);
            let num_levels = 1 + log2ceil(w).max(log2ceil(h));

            // create quad trees for inclusion and missing msbs
            let (t, size) = TagTree::new(num_levels, w, h);
            scratch.clear();
            scratch.resize(4 * size, 0);
            for lev in 0..num_levels as usize {
                let end = t.off[lev + 1];
                scratch[INC * size + t.off[lev]..INC * size + end].fill(255);
                scratch[MMSB * size + t.off[lev]..MMSB * size + end].fill(255);
            }
            let mut tr = Trees {
                t,
                size,
                buf: scratch,
            };

            let band_width = band.num_blocks.w as usize;
            let cbs = |x: u32, y: u32| {
                let i = cb.org.x as usize + x as usize + (cb.org.y + y) as usize * band_width;
                match band.coded_cbs.get(i) {
                    Some(c) => Ok(c),
                    None => {
                        ojph_error!(0x00080051, "internal error: code block index out of range")
                    }
                }
            };
            for y in 0..h {
                for x in 0..w {
                    let p = cbs(x, y)?;
                    tr.set(INC, x, y, 0, u8::from(p.data.is_none()))?; // 1 if true
                    tr.set(MMSB, x, y, 0, p.missing_msbs as u8)?;
                }
            }
            for lev in 1..num_levels {
                let height = level_size(h, lev) as u32;
                let width = level_size(w, lev) as u32;
                for y in 0..height {
                    for x in 0..width {
                        for tree in [INC, MMSB] {
                            let t1 = tr.get(tree, x << 1, y << 1, lev - 1)?.min(tr.get(
                                tree,
                                (x << 1) + 1,
                                y << 1,
                                lev - 1,
                            )?);
                            let t2 = tr.get(tree, x << 1, (y << 1) + 1, lev - 1)?.min(tr.get(
                                tree,
                                (x << 1) + 1,
                                (y << 1) + 1,
                                lev - 1,
                            )?);
                            tr.set(tree, x, y, lev, t1.min(t2))?;
                            tr.set(tree + 1, x, y, lev, 0)?;
                        }
                    }
                }
            }
            for tree in [INC, INC_FLAGS, MMSB, MMSB_FLAGS] {
                tr.set(tree, 0, 0, num_levels, 0)?;
            }
            if tr.get(INC, 0, 0, num_levels - 1)? != 0 {
                // empty subband
                match bb.as_mut() {
                    // non empty precinct, tag tree top is 0
                    Some(bb) => bb.put_bits(0, 1),
                    None => num_skipped_subbands += 1,
                }
                continue;
            }
            // now we are in a position to code
            let bb = bb.get_or_insert_with(|| {
                let mut bb = BitWriteBuf::new(std::mem::take(&mut header));
                // store non empty packet
                bb.put_bit(1);
                // if the first one or two subbands are empty (has
                // codeblocks but no data in them), we need to code them
                // here.
                bb.put_bits(0, num_skipped_subbands);
                bb
            });

            for y in 0..h {
                for x in 0..w {
                    let cp = cbs(x, y)?;
                    // inclusion bits
                    for cur_lev in (1..=num_levels).rev() {
                        let levm1 = cur_lev - 1;
                        let (xl, yl) = (x >> levm1, y >> levm1);
                        // check sent
                        if tr.get(INC_FLAGS, xl, yl, levm1)? == 0 {
                            let skipped = tr.get(INC, xl, yl, levm1)? as i32
                                - tr.get(INC, x >> cur_lev, y >> cur_lev, cur_lev)? as i32;
                            // for HTJ2K, this should 0 or 1
                            if !(0..=1).contains(&skipped) {
                                return ojph_error!(
                                    0x00080052,
                                    "internal error: inclusion tag tree is inconsistent"
                                );
                            }
                            bb.put_bits(1 - skipped as u32, 1);
                            tr.set(INC_FLAGS, xl, yl, levm1, 1)?;
                        }
                        if tr.get(INC, xl, yl, levm1)? > 0 {
                            break;
                        }
                    }

                    if cp.num_passes == 0 {
                        // empty codeblock
                        continue;
                    }

                    // missing msbs
                    for cur_lev in (1..=num_levels).rev() {
                        let levm1 = cur_lev - 1;
                        let (xl, yl) = (x >> levm1, y >> levm1);
                        // check sent
                        if tr.get(MMSB_FLAGS, xl, yl, levm1)? == 0 {
                            let num_zeros = tr.get(MMSB, xl, yl, levm1)? as i32
                                - tr.get(MMSB, x >> cur_lev, y >> cur_lev, cur_lev)? as i32;
                            bb.put_zeros(num_zeros);
                            bb.put_bits(1, 1);
                            tr.set(MMSB_FLAGS, xl, yl, levm1, 1)?;
                        }
                    }

                    // number of coding passes
                    match cp.num_passes {
                        3 => bb.put_bits(12, 4),
                        2 => bb.put_bits(2, 2),
                        1 => bb.put_bits(0, 1),
                        n => {
                            return ojph_error!(
                                0x00080053,
                                "internal error: a code block has {} coding passes",
                                n
                            )
                        }
                    }

                    // pass lengths
                    // either one, two, or three passes, but only one or two
                    // lengths
                    let bits1 = 32 - cp.pass_length[0].leading_zeros() as i32;
                    let extra_bit = i32::from(cp.num_passes > 2); // for 2nd length
                    let bits2 = if cp.num_passes > 1 {
                        32 - cp.pass_length[1].leading_zeros() as i32
                    } else {
                        0
                    };
                    let bits = (bits1.max(bits2 - extra_bit) - 3).max(0) as u32;
                    bb.put_bits(0xFFFF_FFFE, bits + 1);
                    bb.put_bits(cp.pass_length[0], bits + 3);
                    if cp.num_passes > 1 {
                        bb.put_bits(cp.pass_length[1], bits + 3 + extra_bit as u32);
                    }
                    cb_bytes += cp.pass_length[0] as u64 + cp.pass_length[1] as u64;
                }
            }
        }

        match bb {
            Some(bb) => {
                self.header = bb.terminate();
                self.has_header = true;
                Ok(cb_bytes + self.header.len() as u64)
            }
            None => {
                self.header = header;
                Ok(1) // 1 for empty packet
            }
        }
    }

    /// Appends the packet: the header, then the code blocks' bytes.
    pub(crate) fn write(&self, bands: &[Subband; 4], out: &mut Vec<u8>) -> Result<()> {
        if !self.has_header {
            // empty packet
            out.push(0x00);
            return Ok(());
        }
        out.extend_from_slice(&self.header);
        for (band, cb) in bands.iter().zip(self.cb_idxs.iter()) {
            if band.empty {
                continue;
            }
            let band_width = band.num_blocks.w as usize;
            for y in 0..cb.siz.h as usize {
                let row = cb.org.x as usize + (y + cb.org.y as usize) * band_width;
                for x in 0..cb.siz.w as usize {
                    let Some(cp) = band.coded_cbs.get(row + x) else {
                        return ojph_error!(
                            0x00080051,
                            "internal error: code block index out of range"
                        );
                    };
                    if let Some(start) = cp.data {
                        let len = cp.pass_length[0] as usize + cp.pass_length[1] as usize;
                        match band.coded.get(start..start + len) {
                            Some(bytes) => out.extend_from_slice(bytes),
                            None => {
                                return ojph_error!(
                                    0x00080054,
                                    "internal error: coded bytes out of range"
                                )
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
