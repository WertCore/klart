//! Starting with the session — except there is nothing here to start.
//!
//! An earlier version wrote a freedesktop `.desktop` entry into the autostart
//! directory. It pointed at [`std::env::current_exe`], which on Linux is the
//! `klart` command line: there is no agent on this platform, because the menu
//! bar agent is macOS AppKit and does not build here. So the entry launched the
//! command line with no arguments at login, which printed its help and exited.
//! An autostart entry that runs for a fraction of a second and does nothing is
//! worse than none.
//!
//! It is not that the agent is missing — it is that there is no work for one.
//! Both Linux mechanisms, the sysfs backlight and DDC/CI, change brightness in
//! hardware, and hardware keeps it. macOS needs an agent only because a gamma
//! ramp there is reverted the moment the process that set it exits; Linux has no
//! gamma ramp (see this platform's `mod.rs`), so nothing has to be held. A level
//! set by `klart` on Linux simply stays set.
//!
//! So this reports the feature as unavailable, with the reason, rather than
//! offering a switch that does nothing. The one thing it still does is remove a
//! stale entry a previous version may have written.

use std::fs;
use std::path::PathBuf;

use crate::autostart::LoginItem;

/// What the entry is called.
const ENTRY: &str = "klart.desktop";

/// Why there is no login item, and why that is not a gap.
const NOT_NEEDED: &str =
    "not needed on Linux; brightness there is set in hardware and persists on its own";

/// Whether the agent will start with the session.
///
/// Always [`LoginItem::Unavailable`]: there is no agent to start and no reason
/// to start one. See the module docs.
pub fn status() -> LoginItem {
    LoginItem::Unavailable(NOT_NEEDED)
}

/// Refuses to enable a login item, and removes a stale one on disable.
///
/// # Errors
///
/// On enable, always — with the reason it is not offered. On disable, only if a
/// stale entry exists and cannot be removed.
pub fn set(enabled: bool) -> Result<LoginItem, String> {
    if enabled {
        return Err(NOT_NEEDED.to_owned());
    }

    // Disable is honoured even though enable is not, so that anyone who has a
    // dead entry from the earlier version can be rid of it with `klart autostart
    // off`.
    if let Some(path) = entry() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {}
            Err(problem) => return Err(problem.to_string()),
        }
    }

    Ok(status())
}

/// Where a stale entry from the earlier version would be.
fn entry() -> Option<PathBuf> {
    let base = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var_os("HOME").filter(|x| !x.is_empty())?).join(".config")
    };
    Some(base.join("autostart").join(ENTRY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_unavailable_with_a_reason() {
        // The whole point: Linux never reports a login item as on or off, because
        // there is not one to have.
        let LoginItem::Unavailable(reason) = status() else {
            panic!("Linux autostart status should always be Unavailable");
        };
        assert!(!reason.trim().is_empty(), "the reason must say why");
    }

    #[test]
    fn enabling_is_refused_rather_than_writing_a_dead_entry() {
        // The bug this replaced: enable used to write a `.desktop` that launched
        // the command line, which exited at once. Refusing is the fix, so enable
        // must be an error and must never succeed.
        //
        // Disable is deliberately not tested here: it removes a file under the
        // real `$XDG_CONFIG_HOME`/`$HOME`, and a test has no business deleting a
        // developer's actual entry — nor is mutating those vars to sandbox it
        // safe while tests share a process.
        let refused = set(true);
        assert!(
            refused.is_err(),
            "enabling autostart on Linux must refuse, not write an entry"
        );
    }
}
