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
    /// The mechanism itself is not on this machine.
    ///
    /// Every mechanism `klart` uses is private, undocumented API. This is what a
    /// macOS release that withdraws one looks like from here.
    MechanismUnavailable {
        /// The mechanism that is missing.
        mechanism: &'static str,
    },
    /// The mechanism exists but does not drive this display.
    ///
    /// The ordinary answer rather than an exceptional one: `DisplayServices`
    /// says this about every external monitor, and DDC/CI says it about the
    /// built-in panel. It is how the caller knows to try the next mechanism.
    CannotReach {
        /// The mechanism that declined.
        mechanism: &'static str,
        /// The display it declined, by key.
        display: String,
    },
    /// The mechanism accepted the display and then failed the call.
    MechanismFailed {
        /// The mechanism that failed.
        mechanism: &'static str,
        /// The function that returned the code.
        call: &'static str,
        /// The code it returned, which is all the explanation there is.
        code: i32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoreGraphics { call, code } => {
                write!(f, "{call} failed with CGError {code}")
            }
            Self::MechanismUnavailable { mechanism } => {
                write!(f, "{mechanism} is not available on this machine")
            }
            Self::CannotReach { mechanism, display } => {
                write!(f, "{mechanism} cannot reach display {display}")
            }
            Self::MechanismFailed {
                mechanism,
                call,
                code,
            } => write!(f, "{mechanism}: {call} failed with status {code}"),
        }
    }
}

impl std::error::Error for Error {}
