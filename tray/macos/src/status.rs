//! The item in the menu bar itself.
//!
//! An `NSStatusItem` with a menu attached, which is all AppKit needs to do the
//! rest: it handles the click, the highlight and the popping of the menu, and
//! it does all three the way every other menu bar item does.
//!
//! This was `tray-icon` at first. That crate lays a target view over the status
//! item's button to turn clicks into events of its own, and that view only pops
//! a menu the crate itself was given — a `muda` menu, which has no item that can
//! hold a slider. The overlay swallowed the click and the menu never appeared.
//! There is no menu bar work left that the crate would be doing, so it is gone.

use objc2::rc::Retained;
use objc2_app_kit::{NSImage, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{MainThreadMarker, NSString};

/// The symbol on the menu bar.
///
/// A system symbol rather than a drawn glyph: it is the one Control Center uses
/// for the same thing, it is already a template image so it inverts with the
/// menu bar, and it tracks whatever the system decides that icon should look
/// like next.
const SYMBOL: &str = "sun.max";

/// Puts the item in the menu bar and keeps it there.
///
/// The returned value owns the item: dropping it does not remove the item from
/// the bar, but nothing else holds a reference, so it has to be kept.
pub fn install(mtm: MainThreadMarker) -> Retained<NSStatusItem> {
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

    if let Some(button) = item.button(mtm) {
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(SYMBOL),
            Some(&NSString::from_str("klart brightness")),
        );

        match symbol {
            Some(image) => button.setImage(Some(&image)),
            // Not expected on any macOS that has this symbol, which is every
            // one this crate builds for. A word beats an empty slot in the menu
            // bar that cannot be clicked because it cannot be seen.
            None => button.setTitle(&NSString::from_str("klart")),
        }
    }

    item
}
