// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Reading packet headers bit by bit, after `ojph_bitbuffer_read.h`.

use crate::file::{MemInfile, SeekFrom};
use crate::params::marker;

/// The error type of packet parsing. OpenJPH throws string literals here,
/// and the tile turns them into an error or, in resilient mode, ignores
/// them.
pub(crate) type ParseResult<T> = std::result::Result<T, &'static str>;

pub(crate) struct BitReadBuf<'f, 'a> {
    pub file: &'f mut MemInfile<'a>,
    tmp: u32,
    avail_bits: u32,
    unstuff: bool,
    pub bytes_left: u32,
}

impl<'f, 'a> BitReadBuf<'f, 'a> {
    pub(crate) fn new(bytes_left: u32, file: &'f mut MemInfile<'a>) -> Self {
        BitReadBuf {
            file,
            tmp: 0,
            avail_bits: 0,
            unstuff: false,
            bytes_left,
        }
    }

    /// Loads the next byte. Returns false, and supplies zero bits, when the
    /// packet has no bytes left.
    fn read(&mut self) -> ParseResult<bool> {
        if self.bytes_left > 0 {
            let t = self.file.read_u8().ok_or("error reading from file")?;
            self.tmp = t as u32;
            self.avail_bits = 8 - u32::from(self.unstuff);
            self.unstuff = t == 0xFF;
            self.bytes_left -= 1;
            Ok(true)
        } else {
            self.tmp = 0;
            self.avail_bits = 8 - u32::from(self.unstuff);
            self.unstuff = false;
            Ok(false)
        }
    }

    /// Reads one bit; the flag is false if it came from beyond the packet.
    #[inline]
    pub(crate) fn read_bit(&mut self) -> ParseResult<(u32, bool)> {
        let mut result = true;
        if self.avail_bits == 0 {
            result = self.read()?;
        }
        self.avail_bits -= 1;
        Ok(((self.tmp >> self.avail_bits) & 1, result))
    }

    /// Reads up to 32 bits; the flag is as for [`Self::read_bit`].
    pub(crate) fn read_bits(&mut self, mut num_bits: u32) -> ParseResult<(u32, bool)> {
        debug_assert!(num_bits <= 32);
        let mut bits = 0u32;
        let mut result = true;
        while num_bits > 0 {
            if self.avail_bits == 0 {
                result = self.read()?;
            }
            let tx_bits = self.avail_bits.min(num_bits);
            // tx_bits is at most 8, but bits may already hold 32 - 8 bits
            bits = bits.wrapping_shl(tx_bits);
            self.avail_bits -= tx_bits;
            num_bits -= tx_bits;
            bits |= (self.tmp >> self.avail_bits) & ((1 << tx_bits) - 1);
        }
        Ok((bits, result))
    }

    /// Claims `num_bytes` of code block data at the cursor. Returns their
    /// offset in the codestream, how many of them are really there, and
    /// false if the codestream ended before the packet did.
    ///
    /// When the packet itself has fewer than `num_bytes` left, OpenJPH
    /// still accepts the block and pads it with zeros, so that is not
    /// reported as a failure here either.
    pub(crate) fn read_chunk(&mut self, num_bytes: u32) -> (usize, u32, bool) {
        // OpenJPH asserts that no bits are pending here. A damaged packet
        // header can leave some, and a release build carries on.
        let offset = self.file.tell();
        let bytes = num_bytes.min(self.bytes_left);
        let bytes_read = self.file.remaining().len().min(bytes as usize) as u32;
        self.file.seek(bytes_read as i64, SeekFrom::Current);
        self.bytes_left -= bytes_read;
        (offset, bytes_read, bytes_read == bytes)
    }

    fn skip_eph(&mut self) -> ParseResult<()> {
        if self.bytes_left >= 2 {
            let m = self.file.read_u16().ok_or("error reading from file")?;
            self.bytes_left -= 2;
            if m != marker::EPH {
                return Err("should find EPH, but found something else");
            }
        }
        Ok(())
    }

    pub(crate) fn terminate(&mut self, uses_eph: bool) -> ParseResult<bool> {
        let mut result = true;
        if self.unstuff {
            result = self.read()?;
        }
        // OpenJPH asserts that no stuffing is pending here; it still is
        // when the byte just read is 0xFF, which a release build ignores.
        if uses_eph {
            self.skip_eph()?;
        }
        self.tmp = 0;
        self.avail_bits = 0;
        Ok(result)
    }

    pub(crate) fn skip_sop(&mut self) -> ParseResult<bool> {
        if self.bytes_left >= 2 {
            let m = self.file.read_u16().ok_or("error reading from file")?;
            if m == marker::SOP {
                self.bytes_left -= 2;
                if self.bytes_left >= 4 {
                    let com_len = self.file.read_u16().ok_or("error reading from file")?;
                    if com_len != 4 {
                        return Err("something is wrong with SOP length");
                    }
                    if !self.file.seek(com_len as i64 - 2, SeekFrom::Current) {
                        return Err("error seeking file");
                    }
                    self.bytes_left -= com_len as u32;
                } else {
                    return Err("precinct truncated early");
                }
                return Ok(true);
            } else if !self.file.seek(-2, SeekFrom::Current) {
                return Err("error seeking file");
            }
        }
        Ok(false)
    }
}
