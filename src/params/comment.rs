// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! `ojph::comment_exchange`: a COM marker segment payload handed to the
//! codestream writer.

use crate::error::ojph_error;
use crate::Result;

/// `comment_exchange`. C++ keeps a pointer to the caller's bytes; this
/// owns a copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentExchange {
    pub(crate) data: Vec<u8>,
    pub(crate) len: u16,
    /// 1 for a Latin-1 string, 0 for binary data
    #[allow(non_snake_case)]
    pub(crate) Rcom: u16,
}

impl CommentExchange {
    pub fn new() -> Self {
        Self::default()
    }

    /// C++ checks the old `len` member, not the new string's length (a
    /// bug), so a long string is accepted and its length truncated to 16
    /// bits; this does the same.
    pub fn set_string(&mut self, s: &str) -> Result<()> {
        let t = s.len();
        if self.len > 65531 {
            return ojph_error!(
                0x000500C1,
                "COM marker string length cannot be larger than 65531"
            );
        }
        self.data = s.as_bytes().to_vec();
        self.len = t as u16;
        self.Rcom = 1;
        Ok(())
    }

    /// C++ `set_data(const char* data, ui16 len)`; `data.len()` must fit in
    /// 16 bits like the C++ `len`, else this is the 0x000500C2 error.
    pub fn set_data(&mut self, data: &[u8]) -> Result<()> {
        let len = data.len();
        if len > 65531 {
            return ojph_error!(
                0x000500C2,
                "COM marker string length cannot be larger than 65531"
            );
        }
        self.data = data.to_vec();
        self.len = len as u16;
        self.Rcom = 0;
        Ok(())
    }

    /// The bytes (`len` of them).
    pub fn data(&self) -> &[u8] {
        &self.data[..(self.len as usize).min(self.data.len())]
    }

    pub fn len(&self) -> u16 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[allow(non_snake_case)]
    pub fn get_Rcom(&self) -> u16 {
        self.Rcom
    }
}
