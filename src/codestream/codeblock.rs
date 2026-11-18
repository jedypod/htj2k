// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One code block of a subband, after `ojph_codeblock.cpp`.

use crate::codestream::Budget;
use crate::coding::{decode_codeblock32, decode_codeblock64};
use crate::error::ojph_error;
use crate::line::{LineBuf, LineData};
use crate::transform::codeblock_fun::{irv_tx_from_cb32, rev_tx_from_cb32, rev_tx_from_cb64};
use crate::types::Size;
use crate::Result;

/// What the packet headers say about one code block, and where its coded
/// bytes are.
///
/// OpenJPH copies the coded bytes into a list of buffers as it parses the
/// packets. The whole codestream is in memory here, so the header keeps the
/// offset of the bytes in the codestream instead.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CodedCbHeader {
    pub pass_length: [u32; 2],
    /// Number of passes to be decoded.
    pub num_passes: u32,
    pub k_max: u32,
    pub missing_msbs: u32,
    /// Offset in the codestream of the coded bytes, once a packet carrying
    /// them has been parsed.
    pub data: Option<usize>,
    /// How many of the `pass_length[0] + pass_length[1]` coded bytes are
    /// present at `data`. It is less only when the packet was cut short,
    /// and then the missing bytes read as zero.
    pub data_len: u32,
}

/// The sample buffer of a code block; 32 or 64 bits wide, by the precision
/// the component needs.
#[derive(Debug)]
enum CbBuf {
    B32(Vec<u32>),
    B64(Vec<u64>),
}

#[derive(Debug)]
pub(crate) struct Codeblock {
    buf: CbBuf,
    cb_size: Size,
    stride: u32,
    buf_size: u32,
    line_offset: u32,
    cur_line: u32,
    delta: f32,
    k_max: u32,
    reversible: bool,
    resilient: bool,
    stripe_causal: bool,
    /// True when the decoded block is all zero.
    zero_block: bool,
}

impl Codeblock {
    /// A block `cb_size.w` samples wide, whose buffer holds rows of up to
    /// `max_height` samples: the tallest the block is in any row of
    /// blocks of its subband.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        budget: &mut Budget,
        cb_size: Size,
        max_height: u32,
        k_max: u32,
        line_offset: u32,
        precision: u32,
        delta: f32,
        reversible: bool,
        resilient: bool,
        stripe_causal: bool,
    ) -> Result<Self> {
        // OpenJPH sizes the buffer for the nominal block, whatever the
        // block's actual size, with the stride rounded up to its SIMD
        // alignment of 64 bytes (16 32-bit samples) and the height to a
        // whole number of quad rows. A block never changes width, and is
        // never taller than `max_height`, so the buffer is sized for
        // those here instead; at subband edges and in small or short
        // images that is much less. The output is the same: the block
        // decoders write every sample of the block, and what they write
        // past its last row lands outside the buffer and is dropped
        // (see `coding::put`). The stride keeps its rounding, so writes
        // past the last column of a row (at most to the next multiple of
        // 8) stay in that row's padding.
        let f = 64 / 4 - 1;
        let stride = (cb_size.w + f) & !f;
        let buf_size = stride * max_height;
        let buf = if precision <= 32 {
            CbBuf::B32(budget.alloc(buf_size as usize)?)
        } else {
            CbBuf::B64(budget.alloc(buf_size as usize)?)
        };
        Ok(Codeblock {
            buf,
            cb_size,
            stride,
            buf_size,
            line_offset,
            cur_line: 0,
            delta,
            k_max,
            reversible,
            resilient,
            stripe_causal,
            zero_block: false,
        })
    }

    /// Puts the block back on the first row of blocks of its subband,
    /// whose height is `height`, as `new` leaves it.
    pub(crate) fn reset(&mut self, height: u32) {
        self.recreate(Size::new(self.cb_size.w, height));
    }

    /// Sets every sample of the buffer to the bit pattern `bits`.
    #[cfg(test)]
    pub(crate) fn fill_buffers(&mut self, bits: u32) {
        match &mut self.buf {
            CbBuf::B32(b) => b.fill(bits),
            CbBuf::B64(b) => b.fill((bits as u64) << 32 | bits as u64),
        }
    }

    pub(crate) fn recreate(&mut self, cb_size: Size) {
        debug_assert!(cb_size.h * self.stride <= self.buf_size && cb_size.w <= self.stride);
        self.cb_size = cb_size;
        self.cur_line = 0;
        self.zero_block = false;
    }

    // inlined into the subband's line loop: left to itself LLVM inlines
    // or outlines this and `pull_line` from one build to the next, which
    // moves every decode by up to 0.4 instructions per sample
    #[inline(always)]
    pub(crate) fn decode(&mut self, coded_cb: &CodedCbHeader, data: &[u8]) -> Result<()> {
        let padded;
        let coded = match coded_cb.data {
            Some(offset) if coded_cb.pass_length[0] > 0 && coded_cb.num_passes > 0 => {
                let len = coded_cb.pass_length[0] as usize + coded_cb.pass_length[1] as usize;
                let present = (coded_cb.data_len as usize).min(len);
                let bytes = data.get(offset..).and_then(|d| d.get(..present));
                if present == len {
                    bytes
                } else {
                    // a packet that was cut short: rare enough to copy
                    padded = bytes.map(|b| {
                        let mut v = vec![0u8; len];
                        v[..present].copy_from_slice(b);
                        v
                    });
                    padded.as_deref()
                }
            }
            _ => {
                self.zero_block = true;
                return Ok(());
            }
        };
        let result = match (coded, &mut self.buf) {
            (Some(coded), CbBuf::B32(buf)) => decode_codeblock32(
                coded,
                buf,
                coded_cb.missing_msbs,
                coded_cb.num_passes,
                coded_cb.pass_length[0],
                coded_cb.pass_length[1],
                self.cb_size.w,
                self.cb_size.h,
                self.stride,
                self.stripe_causal,
            ),
            (Some(coded), CbBuf::B64(buf)) => decode_codeblock64(
                coded,
                buf,
                coded_cb.missing_msbs,
                coded_cb.num_passes,
                coded_cb.pass_length[0],
                coded_cb.pass_length[1],
                self.cb_size.w,
                self.cb_size.h,
                self.stride,
                self.stripe_causal,
            ),
            (None, _) => false,
        };
        if !result {
            if self.resilient {
                // OJPH_INFO 0x000300A1
                self.zero_block = true;
            } else {
                return ojph_error!(0x000300A1, "Error decoding a codeblock.");
            }
        }
        Ok(())
    }

    /// Writes the next line of the block into its place in `line`.
    // inlined, see `decode`
    #[inline(always)]
    pub(crate) fn pull_line(&mut self, line: &mut LineBuf) -> Result<()> {
        let w = self.cb_size.w as usize;
        let off = line.pre_size + self.line_offset as usize;
        let start = (self.cur_line * self.stride) as usize;
        let mismatch = || {
            ojph_error!(
                0x000300A2,
                "internal error: line and code block types do not match"
            )
        };
        match (&self.buf, &mut line.data) {
            (CbBuf::B32(buf), LineData::I32(dst)) => {
                let dp = &mut dst[off..off + w];
                if self.zero_block {
                    dp.fill(0);
                } else {
                    rev_tx_from_cb32(&buf[start..start + w], dp, self.k_max, self.delta, w as u32);
                }
            }
            (CbBuf::B32(buf), LineData::F32(dst)) => {
                let dp = &mut dst[off..off + w];
                if self.zero_block {
                    dp.fill(0.0);
                } else {
                    irv_tx_from_cb32(&buf[start..start + w], dp, self.k_max, self.delta, w as u32);
                }
            }
            (CbBuf::B64(buf), LineData::I64(dst)) if self.reversible => {
                let dp = &mut dst[off..off + w];
                if self.zero_block {
                    dp.fill(0);
                } else {
                    rev_tx_from_cb64(&buf[start..start + w], dp, self.k_max, self.delta, w as u32);
                }
            }
            _ => return mismatch(),
        }
        self.cur_line += 1;
        debug_assert!(self.cur_line <= self.cb_size.h);
        Ok(())
    }
}
