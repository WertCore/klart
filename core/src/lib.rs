//! Display discovery and brightness control.
//!
//! No operating system has a single API that dims every attached panel. A
//! built-in panel, an external monitor on DDC/CI, and a monitor behind an
//! adaptor that answers neither are three unrelated mechanisms, and this crate's
//! job is to hide that behind one vocabulary. [`Brightness`] is the unit that
//! vocabulary is written in: every backend converts to and from it at its own
//! edge.
//!
//! Start at [`controls`], which pairs every display with the mechanism that
//! reaches it, or [`displays`] if the displays are all that is wanted.
//!
//! # Layout
//!
//! Only `platform` knows what operating system this is. Everything else —
//! [`Brightness`], [`DisplayKey`], the DDC/CI protocol, the resolution order's
//! bookkeeping — is written once and shared, which is what makes a port a matter
//! of implementing two functions rather than a second copy of the crate.

#![deny(missing_docs)]

mod autostart;
mod backend;
mod brightness;
mod combined;
mod control;
mod ddc;
mod diagnose;
mod display;
// Compiled everywhere though macOS never calls it: the IORegistry hands that
// platform the same fields already parsed. It stays in the build so that its
// tests run here too, and those tests are the ones that check a raw EDID
// produces the display key macOS arrived at by a different route — which is
// least convincing on the platform that cannot run it.
#[cfg_attr(
    target_os = "macos",
    allow(dead_code, reason = "parsed by the IORegistry there")
)]
mod edid;
mod error;
mod identity;
mod platform;
mod remembered;

pub use autostart::LoginItem;
pub use backend::Backend;
pub use brightness::Brightness;
pub use control::{Control, controls};
pub use diagnose::{Attempt, Note, Report, Verdict, diagnose};
pub use display::{Bounds, Display, DisplayKind, displays};
pub use error::{Error, Result};
pub use identity::DisplayKey;
pub use platform::{login_item, set_login_item};
pub use remembered::{Remembered, path as remembered_path};
