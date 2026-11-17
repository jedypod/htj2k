// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! One code block being encoded, after the write side of
//! `ojph_codeblock.cpp`.

use crate::codestream::Budget;
use crate::coding::{encode_codeblock32, encode_codeblock64};
use crate::error::ojph_error;
use crate::line::{LineBuf, LineData};
use crate::transform::codeblock_fun::{
    find_max_val32, find_max_val64, irv_tx_to_cb32, rev_tx_to_cb32, rev_tx_to_cb64,
};
use crate::types::Size;
use crate::Result;

/// What the packet header says about one code block, and where its coded
/// bytes are; `coded_cb_header` on the write side.
///
/// OpenJPH links the coded bytes of each block from the header
/// (`next_coded`). Here they are appended to one vector per subband, and
/// the header keeps their place in it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CodedCbHeader {
    pub pass_length: [u32; 2],
    /// Number of coding passes; 0 for a block with nothing coded.
    pub num_passes: u32,
    pub missing_msbs: u32,
    /// Where the coded bytes start in the subband's coded data; `None`
    /// for an empty block (`next_coded == NULL`).
    pub data: Option<usize>,
}

/// The sample buffer of a code block, 32 or 64 bits wide by the
/// precision the component needs.
#[derive(Debug)]
enum CbBuf {
    B32(Vec<u32>),
    B64(Vec<u64>),
}

#[derive(Debug)]
pub(crate) struct Codeblock {
    buf: CbBuf,
    cb_size: Size,
    /// The buffer's row pitch: the block's width. (OpenJPH rounds the
    /// nominal width up to its SIMD alignment; the encoder reads only the
    /// block's samples, so the coded bytes are the same.)
    stride: u32,
    line_offset: u32,
    cur_line: u32,
    delta_inv: f32,
    k_max: u32,
    reversible: bool,
    /// The OR of all magnitudes pushed so far; `max_val32/64[0]`.
    max_val: u64,
}

impl Codeblock {
    /// A block `cb_size.w` samples wide, whose buffer holds up to
    /// `max_height` rows: the tallest the block is in any row of blocks
    /// of its subband.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        budget: &mut Budget,
        cb_size: Size,
        max_height: u32,
        k_max: u32,
        line_offset: u32,
        precision: u32,
        delta_inv: f32,
        reversible: bool,
    ) -> Result<Self> {
        let n = cb_size.w as usize * max_height as usize;
        let buf = if precision <= 32 {
            CbBuf::B32(budget.alloc(n)?)
        } else {
            if !reversible {
                // OpenJPH has no 64-bit irreversible conversion, and
                // would call a null function pointer.
                return ojph_error!(
                    0x00080010,
                    "irreversible coding needs a precision of at most 32 bits, not {}",
                    precision
                );
            }
            CbBuf::B64(budget.alloc(n)?)
        };
        Ok(Codeblock {
            buf,
            cb_size,
            stride: cb_size.w,
            line_offset,
            cur_line: 0,
            delta_inv,
            k_max,
            reversible,
            max_val: 0,
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

    /// Starts the next row of blocks.
    pub(crate) fn recreate(&mut self, cb_size: Size) {
        self.cb_size = cb_size;
        self.cur_line = 0;
        self.max_val = 0;
    }

    /// Converts this block's part of the next line of its subband to
    /// sign and magnitude, and keeps the largest magnitude.
    pub(crate) fn push(&mut self, line: &LineBuf) -> Result<()> {
        let w = self.cb_size.w as usize;
        let src = line.pre_size + self.line_offset as usize;
        let dst = self.cur_line as usize * self.stride as usize;
        let mismatch = || {
            ojph_error!(
                0x00080011,
                "internal error: line and code block types do not match"
            )
        };
        match (&mut self.buf, &line.data) {
            (CbBuf::B32(buf), LineData::I32(sp)) if self.reversible => {
                let (Some(sp), Some(dp)) = (sp.get(src..src + w), buf.get_mut(dst..dst + w)) else {
                    return mismatch();
                };
                let mut mv = self.max_val as u32;
                rev_tx_to_cb32(sp, dp, self.k_max, self.delta_inv, w as u32, &mut mv);
                self.max_val = mv as u64;
            }
            (CbBuf::B32(buf), LineData::F32(sp)) if !self.reversible => {
                let (Some(sp), Some(dp)) = (sp.get(src..src + w), buf.get_mut(dst..dst + w)) else {
                    return mismatch();
                };
                let mut mv = self.max_val as u32;
                irv_tx_to_cb32(sp, dp, self.k_max, self.delta_inv, w as u32, &mut mv);
                self.max_val = mv as u64;
            }
            (CbBuf::B64(buf), LineData::I64(sp)) => {
                let (Some(sp), Some(dp)) = (sp.get(src..src + w), buf.get_mut(dst..dst + w)) else {
                    return mismatch();
                };
                let mut mv = self.max_val;
                rev_tx_to_cb64(sp, dp, self.k_max, self.delta_inv, w as u32, &mut mv);
                self.max_val = mv;
            }
            _ => return mismatch(),
        }
        self.cur_line += 1;
        Ok(())
    }

    /// Encodes the block if any of its samples is significant, appending
    /// the coded bytes to `coded` and filling in `coded_cb`.
    pub(crate) fn encode(
        &mut self,
        coded_cb: &mut CodedCbHeader,
        coded: &mut Vec<u8>,
        budget: &mut Budget,
    ) -> Result<()> {
        let (w, h, stride) = (self.cb_size.w, self.cb_size.h, self.stride);
        let significant = match &self.buf {
            CbBuf::B32(_) => {
                let mv = find_max_val32(&[self.max_val as u32]);
                // K_max is at most 30 for 32-bit blocks (the precision is
                // K_max + 2); the C++ shift is undefined above 31
                self.k_max <= 31 && mv >= 1u32 << (31 - self.k_max)
            }
            CbBuf::B64(_) => {
                let mv = find_max_val64(&[self.max_val]);
                self.k_max <= 63 && mv >= 1u64 << (63 - self.k_max)
            }
        };
        if !significant {
            return Ok(());
        }
        // OpenJPH asserts that missing_msbs = K_max - 1 is above 0
        if self.k_max < 2 {
            return ojph_error!(
                0x00080012,
                "a code block with K_max {} cannot be coded; the quantization leaves no magnitude bits",
                self.k_max
            );
        }
        coded_cb.missing_msbs = self.k_max - 1;
        coded_cb.num_passes = 1;
        let start = coded.len();
        let len = match &self.buf {
            CbBuf::B32(buf) => {
                #[cfg(test)]
                record::push32(buf, self.k_max - 1, w, h, stride);
                encode_codeblock32(buf, self.k_max - 1, 1, w, h, stride, coded)?
            }
            CbBuf::B64(buf) => {
                #[cfg(test)]
                record::push64(buf, self.k_max - 1, w, h, stride);
                encode_codeblock64(buf, self.k_max - 1, 1, w, h, stride, coded)?
            }
        };
        budget.take(len as u64)?;
        coded_cb.pass_length = [len, 0];
        coded_cb.data = Some(start);
        Ok(())
    }
}

/// A log of the block encoder's inputs, for the tests that compare them
/// with the records of OpenJPH's calls in `tests/data/encoder`.
#[cfg(test)]
pub(crate) mod record {
    use std::cell::RefCell;

    /// One call: the encoder's width (32 or 64), `missing_msbs`, the
    /// block size, and the samples packed row by row.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Call {
        pub bits: u32,
        pub missing_msbs: u32,
        pub width: u32,
        pub height: u32,
        pub samples: Vec<u64>,
    }

    thread_local! {
        static LOG: RefCell<Option<Vec<Call>>> = const { RefCell::new(None) };
    }

    /// Starts logging on this thread.
    pub(crate) fn start() {
        LOG.with(|l| *l.borrow_mut() = Some(Vec::new()));
    }

    /// Stops logging and returns the calls.
    pub(crate) fn take() -> Vec<Call> {
        LOG.with(|l| l.borrow_mut().take().unwrap_or_default())
    }

    fn push<T: Copy + Into<u64>>(bits: u32, buf: &[T], mm: u32, w: u32, h: u32, stride: u32) {
        LOG.with(|l| {
            if let Some(log) = l.borrow_mut().as_mut() {
                let mut samples = Vec::with_capacity((w * h) as usize);
                for y in 0..h as usize {
                    let row = y * stride as usize;
                    samples.extend(buf[row..row + w as usize].iter().map(|&v| v.into()));
                }
                log.push(Call {
                    bits,
                    missing_msbs: mm,
                    width: w,
                    height: h,
                    samples,
                });
            }
        });
    }

    pub(crate) fn push32(buf: &[u32], mm: u32, w: u32, h: u32, stride: u32) {
        push(32, buf, mm, w, h, stride)
    }

    pub(crate) fn push64(buf: &[u64], mm: u32, w: u32, h: u32, stride: u32) {
        push(64, buf, mm, w, h, stride)
    }
}
