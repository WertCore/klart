//! Starting with the session, the way a freedesktop desktop does it.
//!
//! A `.desktop` entry in the autostart directory, which every compositor and
//! desktop environment that follows the freedesktop specification reads at login
//! — GNOME, KDE, Xfce, sway and the rest. No D-Bus, no systemd unit, no
//! dependency.
//!
//! A systemd user unit would be the other candidate and is deliberately not
//! used: it would work on most distributions and not all of them, and it would
//! start the agent before a display server exists on some.

use std::fs;
use std::path::PathBuf;

use crate::autostart::LoginItem;

/// What the entry is called.
const ENTRY: &str = "klart.desktop";

/// Whether the agent will start with the session.
pub fn status() -> LoginItem {
    let Some(path) = entry() else {
        return LoginItem::Unavailable;
    };

    match fs::read_to_string(path) {
        // The specification's way of keeping an entry while switching it off,
        // and what some settings panels write rather than deleting the file.
        Ok(text) if text.lines().any(|line| line.trim() == "Hidden=true") => LoginItem::Disabled,
        Ok(_) => LoginItem::Enabled,
        Err(_) => LoginItem::Disabled,
    }
}

/// Writes or removes the entry.
///
/// # Errors
///
/// The underlying message, which is the one that distinguishes a read-only home
/// directory from a missing one.
pub fn set(enabled: bool) -> Result<LoginItem, String> {
    let Some(path) = entry() else {
        return Err("no autostart directory: neither XDG_CONFIG_HOME nor HOME is set".to_owned());
    };

    if !enabled {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(status()),
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(status()),
            Err(problem) => return Err(problem.to_string()),
        }
    }

    // The agent's own path, so an entry written from a build directory points at
    // that build rather than at wherever a packager might have put one.
    let executable = std::env::current_exe()
        .map_err(|problem| problem.to_string())?
        .display()
        .to_string();

    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).map_err(|problem| problem.to_string())?;
    }

    fs::write(
        &path,
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=klart\n\
             Comment=Brightness for every attached display\n\
             Exec={executable}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        ),
    )
    .map_err(|problem| problem.to_string())?;

    Ok(status())
}

/// Where the entry lives.
fn entry() -> Option<PathBuf> {
    let base = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        PathBuf::from(xdg)
    } else {
        PathBuf::from(std::env::var_os("HOME").filter(|x| !x.is_empty())?).join(".config")
    };
    Some(base.join("autostart").join(ENTRY))
}
