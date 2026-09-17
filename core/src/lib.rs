//! Display discovery and brightness control for macOS.
//!
//! macOS has no single API that dims every attached panel. The built-in display,
//! an external monitor on DDC/CI, and a monitor behind a dock that answers
//! neither are three unrelated mechanisms, and this crate's job is to hide that
//! behind one vocabulary. [`Brightness`] is the unit that vocabulary is written
//! in: every backend converts to and from it at its own edge.

#![deny(missing_docs)]

// The two mechanisms this crate is built on — the `DisplayServices` framework and
// IOKit's `IOAVService` — are macOS-only and have no equivalent elsewhere, so a
// build for another target would be a silent no-op rather than a port.
#[cfg(not(target_os = "macos"))]
compile_error!(
    "klart drives macOS display services directly and has no backend for other platforms"
);

mod brightness;

pub use brightness::Brightness;
