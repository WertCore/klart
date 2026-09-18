//! Finding displays through the kernel's DRM interface.
//!
//! `/sys/class/drm` has a directory per connector — `card0-eDP-1`,
//! `card1-DP-3` — carrying whether something is plugged into it and, when
//! something is, that display's raw EDID. No X11, no Wayland, no display server
//! at all: this works over ssh on a machine with no session.
//!
//! What it does not carry is geometry. Where a monitor sits on the desktop is a
//! compositor's idea rather than the kernel's, and reaching a compositor means
//! linking either X11 or one of several Wayland protocols. Bounds are reported
//! as zero here and the consequence is contained: [`crate::identity`] only uses
//! them to order two displays that are otherwise indistinguishable.

use std::fs;
use std::path::{Path, PathBuf};

/// Where the kernel publishes connectors.
const DRM: &str = "/sys/class/drm";

/// Connector names that mean the panel is part of the machine.
///
/// `eDP` is a laptop panel, `LVDS` its predecessor, `DSI` is common on small
/// boards. Everything else — `DP`, `HDMI`, `DVI`, `VGA` — is something plugged
/// in.
const INTERNAL: [&str; 3] = ["eDP", "LVDS", "DSI"];

/// One connector with something attached to it.
pub(crate) struct Connector {
    /// The directory, such as `/sys/class/drm/card1-DP-1`.
    pub path: PathBuf,
    /// Its name, such as `card1-DP-1`.
    pub name: String,
    /// Whether this is a panel built into the machine.
    pub built_in: bool,
    /// The raw EDID, when the kernel has one.
    pub edid: Vec<u8>,
}

impl Connector {
    /// The I2C bus this connector's DDC lines are on.
    ///
    /// The kernel links it at `ddc/i2c-dev/i2c-N`, which is how the right bus is
    /// found without opening every `/dev/i2c-*` on the machine and talking to
    /// whatever answers. Not every driver publishes it.
    pub(crate) fn i2c_bus(&self) -> Option<u32> {
        let ddc = self.path.join("ddc/i2c-dev");
        for entry in fs::read_dir(ddc).ok()? {
            let name = entry.ok()?.file_name();
            if let Some(number) = name.to_str()?.strip_prefix("i2c-")
                && let Ok(number) = number.parse()
            {
                return Some(number);
            }
        }
        None
    }
}

/// Every connector with a display attached.
///
/// Returns an empty vector rather than an error when `/sys/class/drm` cannot be
/// read, which is what a container without the host's sysfs looks like.
pub(crate) fn connected() -> Vec<Connector> {
    let Ok(entries) = fs::read_dir(DRM) else {
        return Vec::new();
    };

    let mut found: Vec<Connector> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| describe(&entry.path()))
        .collect();

    // `read_dir` is in whatever order the filesystem feels like. Sorting makes
    // the display order the same from one run to the next, which matters because
    // the command line addresses displays by index.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn describe(path: &Path) -> Option<Connector> {
    let name = path.file_name()?.to_str()?.to_owned();

    // `card0-eDP-1` and not `card0` itself, nor `renderD128`.
    if !name.contains('-') {
        return None;
    }

    if fs::read_to_string(path.join("status")).ok()?.trim() != "connected" {
        return None;
    }

    let edid = fs::read(path.join("edid")).unwrap_or_default();

    Some(Connector {
        built_in: INTERNAL
            .iter()
            .any(|kind| name.contains(&format!("-{kind}-")) || name.ends_with(kind)),
        name,
        path: path.to_owned(),
        edid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Connector {
        Connector {
            path: PathBuf::from(DRM).join(name),
            built_in: INTERNAL
                .iter()
                .any(|kind| name.contains(&format!("-{kind}-")) || name.ends_with(kind)),
            name: name.to_owned(),
            edid: Vec::new(),
        }
    }

    #[test]
    fn a_laptop_panel_is_recognised_as_built_in() {
        assert!(named("card0-eDP-1").built_in);
        assert!(named("card0-LVDS-1").built_in);
        assert!(named("card0-DSI-1").built_in);
    }

    #[test]
    fn anything_plugged_in_is_not() {
        assert!(!named("card1-DP-1").built_in);
        assert!(!named("card1-HDMI-A-2").built_in);
        assert!(!named("card0-DVI-D-1").built_in);
        assert!(!named("card0-VGA-1").built_in);
    }
}
