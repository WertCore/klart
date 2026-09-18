//! The displays attached to the machine.

use crate::error::Result;
use crate::identity::{self, DisplayKey, Identity};
use crate::platform;

/// Whether a display is the machine's own panel or something plugged into it.
///
/// This decides which mechanism can reach its brightness, so it is part of the
/// public shape rather than an implementation detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    /// The panel built into a laptop or an all-in-one.
    BuiltIn,
    /// Anything plugged in.
    External,
}

/// Where a display sits on the desktop, in points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Distance from the main display's left edge, negative to the left of it.
    pub x: i32,
    /// Distance from the main display's top edge.
    pub y: i32,
    /// Width in points.
    pub width: u32,
    /// Height in points.
    pub height: u32,
}

/// One display attached to the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    id: u32,
    key: DisplayKey,
    name: String,
    kind: DisplayKind,
    is_main: bool,
    bounds: Bounds,
    identity: Identity,
}

impl Display {
    /// The operating system's handle for this display.
    ///
    /// Valid only while the display stays attached, and not stable across a
    /// reconnect; see [`DisplayKey`] for the identity that is.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The identity to store a setting against.
    #[must_use]
    pub fn key(&self) -> &DisplayKey {
        &self.key
    }

    /// The name to show a person.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this is the built-in panel.
    #[must_use]
    pub fn kind(&self) -> DisplayKind {
        self.kind
    }

    /// Whether this display holds the menu bar, or whatever the platform calls
    /// its primary display.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.is_main
    }

    /// Where this display sits on the desktop.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// What the display published about itself.
    ///
    /// How a mechanism finds the display again in whatever registry its platform
    /// keeps, without the display having to own a handle into one.
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }
}

/// One display, as a platform found it.
///
/// The boundary between [`crate::platform`] and everything above it. A platform
/// reports what it can read; naming, keying and disambiguation happen here,
/// once, so that two platforms cannot drift apart on any of the three.
pub(crate) struct Found {
    /// The operating system's handle.
    pub id: u32,
    /// What the display publishes about itself.
    pub identity: Identity,
    /// The name the display publishes, if the platform could read one.
    pub name: Option<String>,
    /// Whether this is the platform's primary display.
    pub is_main: bool,
    /// Where it sits on the desktop.
    pub bounds: Bounds,
}

/// Every display that is on and drawing.
///
/// Displays that are asleep or mirrored onto another are left out: they exist,
/// but they have no desktop of their own and nothing here can usefully address
/// one.
///
/// # Errors
///
/// Fails only if the platform refuses to enumerate at all. A display whose name
/// could not be read is still returned, under a generated one.
pub fn displays() -> Result<Vec<Display>> {
    Ok(assemble(platform::displays()?))
}

/// Turns what a platform found into displays.
///
/// Deliberately free of anything platform-specific, and separated from the
/// reading so that it can be tested against display sets no one has to own.
fn assemble(found: Vec<Found>) -> Vec<Display> {
    // Keys are settled for the whole set at once, because whether one needs a
    // suffix is a question about the others.
    let mut keys: Vec<(DisplayKey, (i32, i32))> = found
        .iter()
        .map(|display| {
            (
                DisplayKey::of(&display.identity),
                (display.bounds.x, display.bounds.y),
            )
        })
        .collect();
    identity::disambiguate(&mut keys);

    found
        .into_iter()
        .zip(keys)
        .map(|(display, (key, _))| Display {
            name: name_for(&display),
            kind: if display.identity.built_in {
                DisplayKind::BuiltIn
            } else {
                DisplayKind::External
            },
            id: display.id,
            key,
            is_main: display.is_main,
            bounds: display.bounds,
            identity: display.identity,
        })
        .collect()
}

/// The name to show for a display.
fn name_for(found: &Found) -> String {
    if let Some(name) = found
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }

    if found.identity.built_in {
        // A soldered panel usually publishes no name. macOS falls back to
        // "Color LCD", which says no more than this does.
        return "Built-in Display".to_owned();
    }

    match identity::pnp_code(found.identity.manufacturer) {
        Some(maker) => format!("{maker} Display"),
        None => format!("Display {}", found.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: Option<&str>, built_in: bool, x: i32) -> Found {
        Found {
            id: 1,
            identity: Identity {
                built_in,
                manufacturer: 0x4c2d,
                product: 0x71e3,
                serial: 0,
                printed_serial: None,
            },
            name: name.map(str::to_owned),
            is_main: false,
            bounds: Bounds {
                x,
                y: 0,
                width: 2560,
                height: 1440,
            },
        }
    }

    #[test]
    fn a_published_name_wins_over_any_fallback() {
        assert_eq!(name_for(&found(Some("LS32AG55x"), false, 0)), "LS32AG55x");
    }

    #[test]
    fn a_name_that_is_only_padding_is_not_a_name() {
        assert_eq!(name_for(&found(Some("   "), false, 0)), "SAM Display");
    }

    #[test]
    fn a_nameless_external_is_named_after_its_manufacturer() {
        assert_eq!(name_for(&found(None, false, 0)), "SAM Display");
    }

    #[test]
    fn a_nameless_panel_is_named_for_being_one() {
        assert_eq!(name_for(&found(None, true, 0)), "Built-in Display");
    }

    #[test]
    fn a_nameless_external_with_no_usable_code_is_named_after_its_handle() {
        let mut odd = found(None, false, 0);
        odd.identity.manufacturer = 0;
        assert_eq!(name_for(&odd), "Display 1");
    }

    #[test]
    fn assembling_two_identical_displays_still_gives_them_distinct_keys() {
        // Neither publishes a serial, so they key the same until position
        // separates them. Right-hand one listed first.
        let assembled = assemble(vec![found(None, false, 2560), found(None, false, 0)]);

        assert_eq!(assembled[1].key().as_str(), "SAM-71e3-00000000#1");
        assert_eq!(assembled[0].key().as_str(), "SAM-71e3-00000000#2");
    }

    #[test]
    fn assembling_reads_the_kind_off_the_identity() {
        let assembled = assemble(vec![found(None, true, 0), found(Some("x"), false, 100)]);

        assert_eq!(assembled[0].kind(), DisplayKind::BuiltIn);
        assert_eq!(assembled[1].kind(), DisplayKind::External);
    }
}
