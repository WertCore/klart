//! The displays attached to the machine.

use std::fmt;

use crate::error::Result;
use crate::sys::graphics::{self, CgDisplay};
use crate::sys::ioreg::{self, ProductAttributes};

/// Whether a display is the machine's own panel or something plugged into it.
///
/// This decides which mechanism can reach its brightness, so it is part of the
/// public shape rather than an implementation detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    /// The panel built into a laptop or an iMac.
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

/// A display's identity, as stable as the display will allow.
///
/// Not the `CGDirectDisplayID`, which macOS reassigns freely: unplug a monitor,
/// plug it back in, and the identifier is very often a different number. This is
/// built from what the display says about itself, so that a level stored against
/// it can be found again tomorrow.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DisplayKey(String);

impl DisplayKey {
    /// The key in the form it is written to configuration.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
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
}

impl Display {
    /// The `CGDirectDisplayID`, for handing back to the system.
    ///
    /// Valid only for as long as this display stays attached; see [`DisplayKey`]
    /// for the identity that outlives a reconnect.
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

    /// Whether this display holds the menu bar.
    #[must_use]
    pub fn is_main(&self) -> bool {
        self.is_main
    }

    /// Where this display sits on the desktop.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }
}

/// Every display that is on and drawing, in the order Core Graphics returns.
///
/// Displays that are asleep or mirrored onto another are left out: they are
/// online, but they have no desktop of their own and nothing here can usefully
/// address one.
///
/// # Errors
///
/// Fails only if Core Graphics refuses to enumerate at all. A display whose name
/// cannot be found is still returned, under a generated one.
pub fn displays() -> Result<Vec<Display>> {
    let attached = graphics::active_displays()?;
    let published = ioreg::product_attributes();

    let described: Vec<(&CgDisplay, Option<&ProductAttributes>)> = attached
        .iter()
        .map(|display| (display, attributes_for(display, &published)))
        .collect();

    // Keys are settled for the whole set at once, because whether one needs a
    // suffix is a question about the others.
    let mut identities: Vec<(DisplayKey, Bounds)> = described
        .iter()
        .map(|(display, attributes)| (key_for(display, *attributes), display.bounds))
        .collect();
    disambiguate(&mut identities);

    Ok(described
        .into_iter()
        .zip(identities)
        .map(|((display, attributes), (key, _))| Display {
            id: display.id,
            key,
            name: name_for(display, attributes),
            kind: if display.is_builtin {
                DisplayKind::BuiltIn
            } else {
                DisplayKind::External
            },
            is_main: display.is_main,
            bounds: display.bounds,
        })
        .collect())
}

/// Finds the registry node that belongs to a Core Graphics display.
///
/// The two namespaces share no identifier, so the join is on the EDID numbers
/// both of them carry.
fn attributes_for<'a>(
    display: &CgDisplay,
    published: &'a [ProductAttributes],
) -> Option<&'a ProductAttributes> {
    published.iter().find(|candidate| {
        candidate.legacy_manufacturer_id == Some(u64::from(display.vendor))
            && candidate.product_id == Some(u64::from(display.model))
            && serial_agrees(candidate, display)
    })
}

/// Whether a candidate's serial number rules it out.
///
/// A node that publishes no serial is not evidence against a match — the
/// built-in panel publishes none at all, and neither do plenty of monitors — so
/// only a serial that is present and different disqualifies.
fn serial_agrees(candidate: &ProductAttributes, display: &CgDisplay) -> bool {
    match candidate.serial_number {
        Some(serial) => serial == u64::from(display.serial),
        None => true,
    }
}

/// The name to show for a display.
fn name_for(display: &CgDisplay, attributes: Option<&ProductAttributes>) -> String {
    if let Some(name) = attributes.and_then(|found| found.name.as_deref()) {
        return name.to_owned();
    }
    if display.is_builtin {
        // The built-in panel publishes no name anywhere in the registry. macOS
        // itself falls back to "Color LCD", which says no more than this does.
        return "Built-in Display".to_owned();
    }
    match pnp_code(display.vendor) {
        Some(vendor) => format!("{vendor} Display"),
        None => format!("Display {}", display.id),
    }
}

/// The identity to store settings against.
fn key_for(display: &CgDisplay, attributes: Option<&ProductAttributes>) -> DisplayKey {
    if display.is_builtin {
        // A Mac has exactly one built-in panel and it is not swappable, so
        // anything more specific would distinguish nothing.
        return DisplayKey("builtin".to_owned());
    }

    let vendor = pnp_code(display.vendor).unwrap_or_else(|| format!("{:04x}", display.vendor));

    // The printed serial is preferred over the EDID one because two units of the
    // same model are far likelier to differ in it.
    let serial = attributes
        .and_then(|found| found.alphanumeric_serial.clone())
        .unwrap_or_else(|| format!("{:08x}", display.serial));

    DisplayKey(format!("{vendor}-{:04x}-{serial}", display.model))
}

/// Decodes the three-letter manufacturer code EDID packs into fifteen bits.
///
/// Five bits a letter, `A` at 1, most significant letter first. Anything that
/// decodes outside `A..=Z` is not a code at all: a dock that invents an EDID for
/// a display behind it often gets this wrong, and a garbled string is worse in a
/// menu than an honest fallback.
fn pnp_code(vendor: u32) -> Option<String> {
    let packed = u16::try_from(vendor).ok()?;
    [(packed >> 10) & 0x1f, (packed >> 5) & 0x1f, packed & 0x1f]
        .into_iter()
        .map(|slot| {
            let letter = u8::try_from(slot).ok()?.checked_add(b'A' - 1)?;
            letter.is_ascii_uppercase().then_some(char::from(letter))
        })
        .collect()
}

/// Appends a positional suffix to any key that more than one display produced.
///
/// Two monitors of the same model with the same printed serial do occur, because
/// some manufacturers ship every unit with the serial left at zero. Without this
/// they would share an identity, and whatever was stored against one would be
/// read back for the other.
///
/// The suffix follows screen order, left to right and then top to bottom. That
/// survives a reboot but not rearranging the displays in System Settings, which
/// is the best available: the displays are, by construction, publishing nothing
/// that tells them apart.
///
/// Quadratic, over a list that cannot exceed the eight displays a Mac Pro drives.
fn disambiguate(identities: &mut [(DisplayKey, Bounds)]) {
    let mut collisions: Vec<DisplayKey> = Vec::new();
    for (key, _) in identities.iter() {
        let shared = identities.iter().filter(|(other, _)| other == key).count() > 1;
        if shared && !collisions.contains(key) {
            collisions.push(key.clone());
        }
    }

    for key in collisions {
        let mut sharing: Vec<usize> = identities
            .iter()
            .enumerate()
            .filter(|(_, (other, _))| *other == key)
            .map(|(index, _)| index)
            .collect();
        sharing.sort_by_key(|&index| (identities[index].1.x, identities[index].1.y));

        for (ordinal, index) in sharing.into_iter().enumerate() {
            identities[index].0 = DisplayKey(format!("{key}#{}", ordinal + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external(vendor: u32, model: u32, serial: u32, x: i32) -> CgDisplay {
        CgDisplay {
            id: 1,
            vendor,
            model,
            serial,
            is_builtin: false,
            is_main: false,
            bounds: Bounds {
                x,
                y: 0,
                width: 2560,
                height: 1440,
            },
        }
    }

    fn at(x: i32, y: i32) -> Bounds {
        Bounds {
            x,
            y,
            width: 2560,
            height: 1440,
        }
    }

    #[test]
    fn it_decodes_the_packed_manufacturer_code() {
        // The two codes this machine reports: Samsung on the external monitor,
        // Apple on the built-in panel.
        assert_eq!(pnp_code(0x4c2d).as_deref(), Some("SAM"));
        assert_eq!(pnp_code(0x0610).as_deref(), Some("APP"));
    }

    #[test]
    fn it_rejects_bit_patterns_that_are_not_three_letters() {
        assert_eq!(pnp_code(0), None, "slot zero is not a letter");
        assert_eq!(pnp_code(0xffff), None, "31 is past Z");
        assert_eq!(
            pnp_code(0x1_0000),
            None,
            "wider than the field EDID gives it"
        );
    }

    #[test]
    fn the_built_in_panel_has_one_fixed_key() {
        let mut panel = external(0x0610, 0, 0, 0);
        panel.is_builtin = true;
        assert_eq!(key_for(&panel, None).as_str(), "builtin");
    }

    #[test]
    fn an_external_key_prefers_the_printed_serial() {
        let attributes = ProductAttributes {
            alphanumeric_serial: Some("HNAW900001".to_owned()),
            ..ProductAttributes::default()
        };
        assert_eq!(
            key_for(&external(0x4c2d, 0x71e3, 810_043_474, 0), Some(&attributes)).as_str(),
            "SAM-71e3-HNAW900001"
        );
    }

    #[test]
    fn an_external_key_falls_back_to_the_edid_serial() {
        assert_eq!(
            key_for(&external(0x4c2d, 0x71e3, 0x0c0f, 0), None).as_str(),
            "SAM-71e3-00000c0f"
        );
    }

    #[test]
    fn a_published_name_wins_over_any_fallback() {
        let attributes = ProductAttributes {
            name: Some("LS32AG55x".to_owned()),
            ..ProductAttributes::default()
        };
        assert_eq!(
            name_for(&external(0x4c2d, 0x71e3, 1, 0), Some(&attributes)),
            "LS32AG55x"
        );
    }

    #[test]
    fn a_nameless_external_is_named_after_its_manufacturer() {
        assert_eq!(name_for(&external(0x4c2d, 1, 1, 0), None), "SAM Display");
    }

    #[test]
    fn a_nameless_external_with_no_usable_code_is_named_after_its_id() {
        assert_eq!(name_for(&external(0, 1, 1, 0), None), "Display 1");
    }

    #[test]
    fn a_serial_that_is_present_and_different_rules_a_candidate_out() {
        let candidate = ProductAttributes {
            serial_number: Some(999),
            ..ProductAttributes::default()
        };
        assert!(!serial_agrees(&candidate, &external(0x4c2d, 1, 1, 0)));
    }

    #[test]
    fn a_candidate_that_publishes_no_serial_is_still_a_candidate() {
        // The built-in panel publishes none, and neither do many monitors.
        let candidate = ProductAttributes::default();
        assert!(serial_agrees(&candidate, &external(0x4c2d, 1, 1, 0)));
    }

    #[test]
    fn identical_displays_are_separated_by_screen_order() {
        let shared = DisplayKey("SAM-71e3-00000000".to_owned());
        // Listed right-hand first, to show the suffix follows position rather
        // than the order Core Graphics happened to enumerate in.
        let mut identities = vec![(shared.clone(), at(2560, 0)), (shared, at(0, 0))];

        disambiguate(&mut identities);

        assert_eq!(identities[1].0.as_str(), "SAM-71e3-00000000#1");
        assert_eq!(identities[0].0.as_str(), "SAM-71e3-00000000#2");
    }

    #[test]
    fn displays_that_already_differ_keep_their_keys() {
        let mut identities = vec![
            (DisplayKey("builtin".to_owned()), at(-1470, 0)),
            (DisplayKey("SAM-71e3-HNAW900001".to_owned()), at(0, 0)),
        ];

        disambiguate(&mut identities);

        assert_eq!(identities[0].0.as_str(), "builtin");
        assert_eq!(identities[1].0.as_str(), "SAM-71e3-HNAW900001");
    }

    // The three below run against whatever is plugged into the machine. A
    // headless runner reports no displays at all, so they hold vacuously there
    // and do their work on a developer's machine — which is the only place the
    // Core Graphics and IORegistry join can actually be exercised.

    #[test]
    fn every_attached_display_has_a_distinct_identity() {
        let attached = displays().expect("Core Graphics should enumerate");

        let mut keys: Vec<&DisplayKey> = attached.iter().map(Display::key).collect();
        let total = keys.len();
        keys.sort();
        keys.dedup();

        assert_eq!(keys.len(), total, "two displays share a key: {attached:#?}");
    }

    #[test]
    fn every_attached_display_has_a_distinct_identifier() {
        let attached = displays().expect("Core Graphics should enumerate");

        let mut ids: Vec<u32> = attached.iter().map(Display::id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();

        assert_eq!(ids.len(), total, "two displays share an id: {attached:#?}");
    }

    #[test]
    fn there_is_at_most_one_main_and_one_built_in_display() {
        let attached = displays().expect("Core Graphics should enumerate");

        let main = attached.iter().filter(|found| found.is_main()).count();
        assert!(main <= 1, "{main} displays claim to be main: {attached:#?}");

        let built_in = attached
            .iter()
            .filter(|found| found.kind() == DisplayKind::BuiltIn)
            .count();
        assert!(built_in <= 1, "{built_in} built-in panels: {attached:#?}");
    }
}
