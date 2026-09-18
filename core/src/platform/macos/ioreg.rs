//! What the IORegistry knows about an attached display.
//!
//! Two things, and they have to be read together.
//!
//! The first is the name. Core Graphics can say a display's manufacturer is
//! `0x4c2d` but not that it calls itself `LS32AG55x`; that lives on the
//! `AppleCLCD2` node the display server publishes per panel, under
//! `DisplayAttributes` → `ProductAttributes`.
//!
//! The second is the service that can talk I2C to it, which is a
//! `DCPAVServiceProxy` somewhere else entirely — a descendant of the
//! `AppleDCPExpert` that sits *beside* the panel's node rather than above or
//! below it. Nothing on either node names the other. What relates them is
//! position: a depth-first walk of the IOService plane visits a panel's node and
//! then, before it reaches the next panel, that panel's proxy.
//!
//! So the walk is ordered and recursive rather than a class match, and the
//! pairing is by traversal order. That is what every tool in this space does,
//! for want of anything better published.
//!
//! The IOKitLib calls used here are public API with no binding in the objc2
//! family — `objc2-io-kit` covers USB, HID and power management and stops short
//! of the registry — so they are declared below.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::{NonNull, null};

use objc2_core_foundation::{CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CFType};

/// A `mach_port_t` naming a registry object.
pub(crate) type IoObject = u32;

/// `KERN_SUCCESS`.
const KERN_SUCCESS: i32 = 0;

/// `kIOMainPortDefault`, which has been a null port name since it was
/// `kIOMasterPortDefault`.
const MAIN_PORT_DEFAULT: IoObject = 0;

/// `kIORegistryIterateRecursively`.
const ITERATE_RECURSIVELY: u32 = 0x1;

/// The length of an `io_name_t`.
const IO_NAME_LEN: usize = 128;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegistryGetRootEntry(main_port: IoObject) -> IoObject;
    fn IORegistryEntryCreateIterator(
        entry: IoObject,
        plane: *const c_char,
        options: u32,
        iterator: *mut IoObject,
    ) -> i32;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> i32;
    fn IOObjectGetClass(object: IoObject, class_name: *mut c_char) -> i32;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObject,
        key: &CFString,
        allocator: *const c_void,
        options: u32,
    ) -> *mut CFType;
}

/// Releases a registry object on the way out.
///
/// Every `IOIteratorNext` hands back a retained object and every iterator is
/// itself one, so each of these is a leak of a kernel port if it is dropped on
/// an early return instead.
pub(crate) struct IoRef(IoObject);

impl IoRef {
    /// The underlying port, for handing to a call that wants one.
    pub(crate) fn raw(&self) -> IoObject {
        self.0
    }
}

impl Drop for IoRef {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from the registry and is released exactly once,
        // here.
        unsafe { IOObjectRelease(self.0) };
    }
}

impl std::fmt::Debug for IoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IoRef({:#x})", self.0)
    }
}

/// The class the display server publishes one node of per panel.
///
/// Apple silicon only. Intel Macs publish `IODisplayConnect` under a different
/// property layout, so nothing is found there and callers fall back to a
/// generated name rather than failing.
const PANEL_CLASS: &str = "AppleCLCD2";

/// The class that owns the I2C link to a panel.
const AV_SERVICE_CLASS: &str = "DCPAVServiceProxy";

/// What one display's registry node says about itself.
///
/// Every field is optional because the built-in panel publishes almost none of
/// them: it carries a manufacturer and a product identifier and no name, no
/// serial number and no year.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ProductAttributes {
    /// The name the display reports, such as `LS32AG55x`.
    pub name: Option<String>,
    /// The EDID manufacturer code as an integer, comparable with
    /// `CGDisplayVendorNumber`.
    pub legacy_manufacturer_id: Option<u64>,
    /// The EDID product code, comparable with `CGDisplayModelNumber`.
    pub product_id: Option<u64>,
    /// The EDID serial number, comparable with `CGDisplaySerialNumber`.
    pub serial_number: Option<u64>,
    /// The printed serial, such as `HNAW900001`. Far likelier to be unique
    /// across two of the same model than the numeric one.
    pub alphanumeric_serial: Option<String>,
}

/// One display, as the registry describes it.
#[derive(Debug)]
pub(crate) struct DisplayNode {
    /// What the panel says about itself.
    pub attributes: ProductAttributes,
    /// The service that can carry I2C to it, when it has one. The built-in panel
    /// has one and it answers nothing, and a display behind an adaptor that does
    /// not carry DDC may have none at all.
    pub av_service: Option<IoRef>,
}

/// Walks the IOService plane and pairs every panel with its I2C service.
///
/// Returns an empty vector rather than an error when nothing matches: a missing
/// name is a cosmetic loss and a missing service means one backend is
/// unavailable, and failing display discovery over either would turn a partial
/// loss into a total one.
pub(crate) fn display_nodes() -> Vec<DisplayNode> {
    let Ok(plane) = CString::new("IOService") else {
        return Vec::new();
    };

    // SAFETY: a null main port is the documented default.
    let root = unsafe { IORegistryGetRootEntry(MAIN_PORT_DEFAULT) };
    if root == 0 {
        return Vec::new();
    }
    let root = IoRef(root);

    let mut iterator: IoObject = 0;
    // SAFETY: `root` is live, `plane` is a live NUL-terminated string, and
    // `iterator` is a live out parameter.
    let status = unsafe {
        IORegistryEntryCreateIterator(
            root.raw(),
            plane.as_ptr(),
            ITERATE_RECURSIVELY,
            &raw mut iterator,
        )
    };
    if status != KERN_SUCCESS {
        return Vec::new();
    }
    let iterator = IoRef(iterator);

    let mut found: Vec<DisplayNode> = Vec::new();
    loop {
        // SAFETY: the iterator is live until its `IoRef` is dropped.
        let entry = unsafe { IOIteratorNext(iterator.raw()) };
        if entry == 0 {
            break;
        }
        let entry = IoRef(entry);

        match class_of(&entry).as_deref() {
            Some(PANEL_CLASS) => {
                if let Some(attributes) = read_attributes(&entry) {
                    found.push(DisplayNode {
                        attributes,
                        av_service: None,
                    });
                }
            }
            Some(AV_SERVICE_CLASS) => {
                // To the panel most recently walked past, and only if it has not
                // already been given one: a panel publishes several proxies on
                // some machines and the first is the one that answers.
                if let Some(panel) = found.last_mut()
                    && panel.av_service.is_none()
                {
                    panel.av_service = Some(entry);
                }
            }
            _ => {}
        }
    }
    found
}

/// The node describing the display with these EDID numbers, if the registry has
/// one.
///
/// The two namespaces share no identifier, so this is the join: Core Graphics
/// and the registry both carry the EDID numbers, and nothing else is common to
/// them.
pub(crate) fn node_for(
    nodes: &[DisplayNode],
    vendor: u32,
    model: u32,
    serial: u32,
) -> Option<&DisplayNode> {
    nodes.iter().find(|candidate| {
        candidate.attributes.legacy_manufacturer_id == Some(u64::from(vendor))
            && candidate.attributes.product_id == Some(u64::from(model))
            && serial_agrees(&candidate.attributes, serial)
    })
}

/// Whether a candidate's serial number rules it out.
///
/// A node that publishes no serial is not evidence against a match — the
/// built-in panel publishes none at all, and neither do plenty of monitors — so
/// only a serial that is present and different disqualifies.
fn serial_agrees(candidate: &ProductAttributes, serial: u32) -> bool {
    match candidate.serial_number {
        Some(published) => published == u64::from(serial),
        None => true,
    }
}

/// The IOKit class name of a registry entry.
fn class_of(entry: &IoRef) -> Option<String> {
    let mut name: [c_char; IO_NAME_LEN] = [0; IO_NAME_LEN];

    // SAFETY: `name` is an `io_name_t`, which is exactly the buffer this writes.
    let status = unsafe { IOObjectGetClass(entry.raw(), name.as_mut_ptr()) };
    if status != KERN_SUCCESS {
        return None;
    }

    // SAFETY: `IOObjectGetClass` NUL-terminates within the buffer on success.
    let name = unsafe { CStr::from_ptr(name.as_ptr()) };
    name.to_str().ok().map(str::to_owned)
}

/// Pulls `DisplayAttributes` → `ProductAttributes` off one registry node.
fn read_attributes(entry: &IoRef) -> Option<ProductAttributes> {
    let key = CFString::from_static_str("DisplayAttributes");

    // SAFETY: `entry` is live and `key` outlives the call. The result is a +1
    // reference, which `CFRetained::from_raw` takes over.
    let raw = unsafe { IORegistryEntryCreateCFProperty(entry.raw(), &key, null(), 0) };
    let attributes = NonNull::new(raw)?;
    // SAFETY: the pointer is non-null and owns a reference, which is what
    // `from_raw` documents.
    let attributes = unsafe { CFRetained::from_raw(attributes) };
    let attributes = attributes.downcast_ref::<CFDictionary>()?;

    let product = dictionary(attributes, "ProductAttributes")?;

    Some(ProductAttributes {
        name: string(product, "ProductName"),
        legacy_manufacturer_id: number(product, "LegacyManufacturerID"),
        product_id: number(product, "ProductID"),
        serial_number: number(product, "SerialNumber"),
        alphanumeric_serial: string(product, "AlphanumericSerialNumber"),
    })
}

/// Looks one key up in a dictionary, without taking ownership of the result.
///
/// `CFDictionaryGetValue` follows the Get Rule, so the value is borrowed from
/// the dictionary and the lifetime here is the one that makes that sound.
fn value<'a>(dictionary: &'a CFDictionary, key: &str) -> Option<&'a CFType> {
    let key = CFString::from_str(key);
    let key_ptr: *const CFString = &*key;

    // SAFETY: both the dictionary and the key are live across the call.
    let found = unsafe { dictionary.value(key_ptr.cast()) };
    let found = NonNull::new(found.cast_mut())?;

    // SAFETY: the value is owned by `dictionary` and so outlives `'a`.
    Some(unsafe { found.cast::<CFType>().as_ref() })
}

fn dictionary<'a>(parent: &'a CFDictionary, key: &str) -> Option<&'a CFDictionary> {
    value(parent, key)?.downcast_ref::<CFDictionary>()
}

fn string(parent: &CFDictionary, key: &str) -> Option<String> {
    Some(value(parent, key)?.downcast_ref::<CFString>()?.to_string())
}

/// Reads an integer, whatever width it was stored at.
///
/// Signed, because `CFNumber` has no unsigned representation — the built-in
/// panel's product identifier needs all 64 bits, so a narrower read would
/// truncate it into a value that collides with a real EDID product code.
fn number(parent: &CFDictionary, key: &str) -> Option<u64> {
    let number = value(parent, key)?.downcast_ref::<CFNumber>()?;
    let mut out: i64 = 0;

    // SAFETY: `out` is an `i64`, which is what `SInt64Type` asks the value to be
    // written as.
    let read = unsafe { number.value(CFNumberType::SInt64Type, (&raw mut out).cast()) };

    read.then(|| u64::try_from(out).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(attributes: ProductAttributes) -> DisplayNode {
        DisplayNode {
            attributes,
            av_service: None,
        }
    }

    #[test]
    fn a_serial_that_is_present_and_different_rules_a_candidate_out() {
        let candidate = ProductAttributes {
            serial_number: Some(999),
            ..ProductAttributes::default()
        };
        assert!(!serial_agrees(&candidate, 1));
    }

    #[test]
    fn a_candidate_that_publishes_no_serial_is_still_a_candidate() {
        // The built-in panel publishes none, and neither do many monitors.
        assert!(serial_agrees(&ProductAttributes::default(), 1));
    }

    #[test]
    fn the_join_needs_the_manufacturer_and_the_product_to_agree() {
        let nodes = vec![node(ProductAttributes {
            legacy_manufacturer_id: Some(0x4c2d),
            product_id: Some(0x71e3),
            serial_number: Some(810_043_474),
            ..ProductAttributes::default()
        })];

        assert!(node_for(&nodes, 0x4c2d, 0x71e3, 810_043_474).is_some());
        assert!(
            node_for(&nodes, 0x4c2d, 0x71e3, 1).is_none(),
            "wrong serial"
        );
        assert!(
            node_for(&nodes, 0x4c2d, 1, 810_043_474).is_none(),
            "wrong product"
        );
        assert!(
            node_for(&nodes, 1, 0x71e3, 810_043_474).is_none(),
            "wrong maker"
        );
    }

    /// Runs against whatever is plugged in. A headless runner publishes no panel
    /// nodes at all, so it holds vacuously there.
    #[test]
    fn every_panel_the_registry_publishes_is_paired_with_at_most_one_service() {
        for found in display_nodes() {
            // Nothing to assert beyond the walk completing without pairing a
            // service twice, which `Option` already enforces — this exists to
            // exercise the walk itself under the test runner.
            let _ = found.av_service.is_some();
        }
    }
}
