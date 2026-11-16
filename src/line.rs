// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Line buffers, after `line_buf` in `ojph_mem.h`.

/// The samples of a [`LineBuf`].
#[derive(Debug, Clone, Default, PartialEq)]
pub enum LineData {
    /// Not allocated.
    #[default]
    None,
    /// 32-bit integers, used by the reversible path up to 32 bits of
    /// precision.
    I32(Vec<i32>),
    /// 64-bit integers, used by the reversible path above 32 bits of
    /// precision.
    I64(Vec<i64>),
    /// Floats, used by the irreversible path.
    F32(Vec<f32>),
}

/// One line of samples.
///
/// A line has `size` addressable samples, preceded by `pre_size` extra
/// samples that the wavelet kernels use for boundary extension. The backing
/// vector therefore holds `pre_size + size` elements, and logical sample
/// `i` (which may be as low as `-pre_size`) lives at vector index
/// `pre_size + i`. This mirrors OpenJPH, where `line_buf::i32` points
/// `pre_size` elements into its allocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineBuf {
    pub size: usize,
    pub pre_size: usize,
    pub data: LineData,
}

impl LineBuf {
    pub fn new_i32(size: usize, pre_size: usize) -> Self {
        LineBuf { size, pre_size, data: LineData::I32(vec![0; size + pre_size]) }
    }

    pub fn new_i64(size: usize, pre_size: usize) -> Self {
        LineBuf { size, pre_size, data: LineData::I64(vec![0; size + pre_size]) }
    }

    pub fn new_f32(size: usize, pre_size: usize) -> Self {
        LineBuf { size, pre_size, data: LineData::F32(vec![0.0; size + pre_size]) }
    }

    pub fn is_i32(&self) -> bool {
        matches!(self.data, LineData::I32(_))
    }

    pub fn is_i64(&self) -> bool {
        matches!(self.data, LineData::I64(_))
    }

    pub fn is_f32(&self) -> bool {
        matches!(self.data, LineData::F32(_))
    }

    /// The samples from logical index 0 on. Panics if the line is not i32.
    pub fn i32(&self) -> &[i32] {
        match &self.data {
            LineData::I32(v) => &v[self.pre_size..],
            _ => panic!("line buffer is not i32"),
        }
    }

    pub fn i32_mut(&mut self) -> &mut [i32] {
        match &mut self.data {
            LineData::I32(v) => &mut v[self.pre_size..],
            _ => panic!("line buffer is not i32"),
        }
    }

    pub fn i64(&self) -> &[i64] {
        match &self.data {
            LineData::I64(v) => &v[self.pre_size..],
            _ => panic!("line buffer is not i64"),
        }
    }

    pub fn i64_mut(&mut self) -> &mut [i64] {
        match &mut self.data {
            LineData::I64(v) => &mut v[self.pre_size..],
            _ => panic!("line buffer is not i64"),
        }
    }

    pub fn f32(&self) -> &[f32] {
        match &self.data {
            LineData::F32(v) => &v[self.pre_size..],
            _ => panic!("line buffer is not f32"),
        }
    }

    pub fn f32_mut(&mut self) -> &mut [f32] {
        match &mut self.data {
            LineData::F32(v) => &mut v[self.pre_size..],
            _ => panic!("line buffer is not f32"),
        }
    }

    /// The whole backing vector, including the `pre_size` leading samples.
    pub fn i32_full_mut(&mut self) -> &mut [i32] {
        match &mut self.data {
            LineData::I32(v) => v,
            _ => panic!("line buffer is not i32"),
        }
    }

    pub fn i64_full_mut(&mut self) -> &mut [i64] {
        match &mut self.data {
            LineData::I64(v) => v,
            _ => panic!("line buffer is not i64"),
        }
    }

    pub fn f32_full_mut(&mut self) -> &mut [f32] {
        match &mut self.data {
            LineData::F32(v) => v,
            _ => panic!("line buffer is not f32"),
        }
    }
}
