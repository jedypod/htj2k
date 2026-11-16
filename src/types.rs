// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Small geometry types, after `ojph_base.h`.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    pub const fn new(x: u32, y: u32) -> Self {
        Point { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Size { w, h }
    }

    pub const fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub org: Point,
    pub siz: Size,
}

/// `ceil(a / b)` for unsigned integers, as OpenJPH's `ojph_div_ceil`.
#[inline]
pub const fn div_ceil(a: u32, b: u32) -> u32 {
    ((a as u64 + b as u64 - 1) / b as u64) as u32
}
