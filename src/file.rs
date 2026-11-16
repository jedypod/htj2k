// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Reading a codestream from memory, after `mem_infile` in `ojph_file.h`.
//!
//! Writing needs no counterpart: codestreams are written to a `Vec<u8>`.

/// Where a [`MemInfile::seek`] offset is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    Start,
    Current,
    End,
}

/// A cursor over a codestream held in memory.
#[derive(Debug, Clone)]
pub struct MemInfile<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> MemInfile<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        MemInfile { data, pos: 0 }
    }

    /// Copies up to `buf.len()` bytes and returns how many were copied,
    /// which is less than requested only at the end of the data.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n
    }

    /// Reads one byte, or `None` at the end of the data.
    pub fn read_u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// Reads a big-endian `u16`, or `None` if fewer than two bytes remain,
    /// in which case the position does not move.
    pub fn read_u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Reads a big-endian `u32`, or `None` if fewer than four bytes remain,
    /// in which case the position does not move.
    pub fn read_u32(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Moves the cursor. Returns false, without moving, if the target lies
    /// outside the data.
    pub fn seek(&mut self, offset: i64, origin: SeekFrom) -> bool {
        let base = match origin {
            SeekFrom::Start => 0,
            SeekFrom::Current => self.pos as i64,
            SeekFrom::End => self.data.len() as i64,
        };
        match base.checked_add(offset) {
            Some(p) if p >= 0 && p as u64 <= self.data.len() as u64 => {
                self.pos = p as usize;
                true
            }
            _ => false,
        }
    }

    pub fn tell(&self) -> usize {
        self.pos
    }

    pub fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The whole of the underlying data.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The data from the cursor to the end.
    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }
}
