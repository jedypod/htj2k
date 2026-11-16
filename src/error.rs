// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

use std::borrow::Cow;
use std::fmt;

/// An error raised while reading or writing a codestream.
///
/// `code` is the message code OpenJPH uses for the same condition, where
/// there is one (for example `0x00030011`), so that a failure can be traced
/// back to the reference implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    code: u32,
    message: Cow<'static, str>,
}

/// The result type used throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(code: u32, message: impl Into<Cow<'static, str>>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }

    /// The OpenJPH message code for this error.
    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "htj2k error 0x{:08X}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// Builds an `Err(Error)` from an OpenJPH message code and a format string.
macro_rules! ojph_error {
    ($code:expr, $msg:literal) => {
        Err($crate::Error::new($code, $msg))
    };
    ($code:expr, $fmt:literal, $($arg:tt)*) => {
        Err($crate::Error::new($code, format!($fmt, $($arg)*)))
    };
}
pub(crate) use ojph_error;
