//! What every tray agent does, whichever toolkit draws it.
//!
//! A tray is a platform's own thing: an `NSStatusItem` and an `NSMenu` on
//! macOS, a `Shell_NotifyIcon` icon and an owner-drawn panel on Windows. What
//! is not the platform's is the part underneath — writing levels at a rate a
//! DDC/CI link can take, remembering what each display was left at, and putting
//! them back after a wake. That is [`driver`], and it is shared rather than
//! copied so that two agents cannot drift apart on it.
//!
//! [`request`] is here for the same reason: it is the small queue a menu uses to
//! ask the agent for something that can only happen once the menu has closed.

pub mod driver;
pub mod request;
