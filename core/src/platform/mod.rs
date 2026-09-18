//! Everything that differs between operating systems.
//!
//! One module per platform, selected by `cfg`, so exactly one of them compiles
//! into any given binary — a macOS build carries no Linux code and pays nothing
//! for its existence.
//!
//! The seam is two functions. [`displays`] reports what is attached, as
//! [`crate::display::Found`], which is EDID and geometry and nothing an
//! operating system invented. [`open`] picks a mechanism for one display and
//! says what the mechanisms ahead of it refused.
//!
//! The order the mechanisms are tried in is deliberately *inside* the platform
//! rather than above it, because it is not the same list everywhere: macOS has
//! one hardware path for the built-in panel and one for everything else, and
//! Windows has two entirely different ones. What is shared is the policy behind
//! the order — hardware before software, because the gamma ramp darkens a
//! picture rather than a backlight — and that belongs in prose, not in a
//! sequence some other platform would have to contort itself into.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::{displays, open};

// A build for another target would link and silently do nothing, which is worse
// than not building. The seam above is what a port plugs into.
#[cfg(not(target_os = "macos"))]
compile_error!(
    "klart has no platform module for this target yet; see `platform::displays` \
     and `platform::open`, and PLAN.md"
);
