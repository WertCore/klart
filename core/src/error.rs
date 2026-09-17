use std::fmt;

/// The result of an operation that has to reach a display to succeed.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong on the way to a display.
///
/// The variants are deliberately about *where* a call failed rather than what a
/// caller should do about it: a display that has been unplugged mid-call and one
/// behind a dock that swallows I2C fail in the same place, and only the message
/// can tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A Core Graphics call returned a failure code.
    CoreGraphics {
        /// The function that failed.
        call: &'static str,
        /// The `CGError` it returned.
        code: i32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoreGraphics { call, code } => {
                write!(f, "{call} failed with CGError {code}")
            }
        }
    }
}

impl std::error::Error for Error {}
