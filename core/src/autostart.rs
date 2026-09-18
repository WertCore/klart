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
    /// This build cannot ask the question.
    ///
    /// Either it is not running from an application bundle, so there is nothing
    /// for the system to launch, or the system is older than `SMAppService`.
    Unavailable,
}
