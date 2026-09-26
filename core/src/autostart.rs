//! Whether the agent starts with the session.
//!
//! The mechanism differs — macOS registers a bundle with `SMAppService`, Linux
//! writes a desktop entry into the autostart directory — but the states a person
//! cares about are the same everywhere, so the type lives here and each platform
//! answers with it.
//!
//! It matters more than a convenience. On a display with no hardware brightness
//! control the level lasts exactly as long as the agent does, so an agent that
//! does not start at login means such a display is back at full brightness after
//! every restart.

/// Whether the agent is set to start with the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItem {
    /// It will start at login.
    Enabled,
    /// It will not.
    Disabled,
    /// It has been registered and is waiting to be allowed.
    ///
    /// macOS puts a newly registered login item in front of the person before it
    /// will honour it. Until they say yes in System Settings, this is where it
    /// stays — which is why it is worth showing rather than reporting as enabled.
    AwaitingApproval,
    /// The question does not apply to this build, with the reason why.
    ///
    /// The reason differs by platform and is carried rather than assumed: on
    /// macOS the agent is not running from an application bundle (or the system
    /// predates `SMAppService`); on Linux there is no agent to keep running at
    /// all, because every mechanism there changes brightness in hardware and it
    /// persists on its own; on Windows the registry could not be opened. A single
    /// message baked into the caller was wrong for two of the three.
    Unavailable(&'static str),
}
