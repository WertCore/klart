//! Starting with the session.
//!
//! Worth having here rather than leaving to the person, because on a display
//! with no hardware brightness control the level only lasts as long as this
//! process does — macOS reverts a gamma ramp when the process that set it exits.
//! An agent that is not started at login means such a display is back at full
//! brightness after every restart, whatever was asked for before it.
//!
//! `SMAppService` arrived in macOS 13 and the rest of this works further back,
//! so the class is looked up before it is used rather than raising the whole
//! crate's floor for one feature.
//!
//! It also registers *a bundle*. Run as a bare binary out of `target/`, there is
//! nothing for the system to launch, and saying so is more use than an error
//! from a framework about a path.

use objc2::runtime::AnyClass;
use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

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

/// Whether the agent will start with the session.
pub fn status() -> LoginItem {
    let Some(service) = service() else {
        return LoginItem::Unavailable;
    };

    // SAFETY: the class exists and this takes no arguments.
    match unsafe { service.status() } {
        SMAppServiceStatus::Enabled => LoginItem::Enabled,
        SMAppServiceStatus::RequiresApproval => LoginItem::AwaitingApproval,
        _ => LoginItem::Disabled,
    }
}

/// Asks for the agent to start with the session, or stops it doing so.
///
/// # Errors
///
/// The framework's own message, which is what distinguishes "not allowed" from
/// "no such bundle" and is not worth paraphrasing.
pub fn set(enabled: bool) -> Result<LoginItem, String> {
    let Some(service) = service() else {
        return Err(
            "this build cannot register a login item: it is not running from an application \
             bundle, or this macOS predates SMAppService. Build the bundle with \
             `scripts/bundle.sh` and run it from there."
                .to_owned(),
        );
    };

    // SAFETY: the class exists and neither takes arguments.
    let outcome = unsafe {
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
    };

    match outcome {
        Ok(()) => Ok(status()),
        Err(problem) => Err(problem.localizedDescription().to_string()),
    }
}

/// The service for this bundle, if there is one to speak of.
fn service() -> Option<objc2::rc::Retained<SMAppService>> {
    // Before `class!`, which panics on a class that is not there. This is the
    // whole of the macOS 13 check: on anything older the class is simply absent.
    AnyClass::get(c"SMAppService")?;

    // A bare binary has no bundle identifier, and registering one would be
    // registering nothing.
    NSBundle::mainBundle().bundleIdentifier()?;

    // SAFETY: the class exists, and this takes no arguments.
    Some(unsafe { SMAppService::mainAppService() })
}
