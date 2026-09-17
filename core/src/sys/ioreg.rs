//! What the IORegistry knows about an attached display.
//!
//! Core Graphics can say a display's manufacturer is `0x4c2d` but not that it
//! calls itself `LS32AG55x`. The name lives in the IORegistry, on the node the
//! display server publishes for each panel, under `DisplayAttributes` →
//! `ProductAttributes`.
//!
//! The IOKitLib calls used here are public API with no binding in the objc2
//! family — `objc2-io-kit` covers USB, HID and power management and stops short
//! of the registry — so they are declared below.

use std::ffi::{CString, c_char, c_void};
use std::ptr::{NonNull, null};

use objc2_core_foundation::{CFDictionary, CFNumber, CFNumberType, CFString, CFType};

/// A `mach_port_t` naming a registry object.
type IoObject = u32;

/// `KERN_SUCCESS`.
const KERN_SUCCESS: i32 = 0;

/// `kIOMainPortDefault`, which has been a null port name since it was
/// `kIOMasterPortDefault`.
const MAIN_PORT_DEFAULT: IoObject = 0;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: IoObject,
        matching: *mut c_void,
        existing: *mut IoObject,
    ) -> i32;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> i32;
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
struct IoRef(IoObject);

impl Drop for IoRef {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `IOServiceGetMatchingServices` or
        // `IOIteratorNext` and is released exactly once, here.
        unsafe { IOObjectRelease(self.0) };
    }
}

/// The class the display server publishes one node of per panel.
///
/// Apple silicon only. Intel Macs publish `IODisplayConnect` under a different
/// property layout, and this returns nothing there — callers fall back to a
/// generated name rather than failing.
const DISPLAY_CLASS: &str = "AppleCLCD2";

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

/// Reads the product attributes of every display node in the registry.
///
/// Returns an empty vector rather than an error when the class does not exist
/// or nothing matches: a missing name is a cosmetic loss, and failing display
/// discovery over it would turn a cosmetic loss into an outage.
pub(crate) fn product_attributes() -> Vec<ProductAttributes> {
    let Ok(class) = CString::new(DISPLAY_CLASS) else {
        return Vec::new();
    };

    // SAFETY: `class` is a live NUL-terminated string for the duration of the
    // call. The returned dictionary is a +1 reference that
    // `IOServiceGetMatchingServices` consumes, so it must not be released here.
    let matching = unsafe { IOServiceMatching(class.as_ptr()) };
    if matching.is_null() {
        return Vec::new();
    }

    let mut iterator: IoObject = 0;
    // SAFETY: `matching` is a valid dictionary and `iterator` is a live out
    // parameter.
    let status =
        unsafe { IOServiceGetMatchingServices(MAIN_PORT_DEFAULT, matching, &raw mut iterator) };
    if status != KERN_SUCCESS {
        return Vec::new();
    }
    let iterator = IoRef(iterator);

    let mut found = Vec::new();
    loop {
        // SAFETY: `iterator.0` is live until the `IoRef` is dropped below.
        let entry = unsafe { IOIteratorNext(iterator.0) };
        if entry == 0 {
            break;
        }
        let entry = IoRef(entry);
        if let Some(attributes) = read_entry(&entry) {
            found.push(attributes);
        }
    }
    found
}

/// Pulls `DisplayAttributes` → `ProductAttributes` off one registry node.
fn read_entry(entry: &IoRef) -> Option<ProductAttributes> {
    let key = CFString::from_static_str("DisplayAttributes");

    // SAFETY: `entry.0` is a live registry entry and `key` outlives the call.
    // The result is a +1 reference, which `CFRetained::from_raw` takes over.
    let raw = unsafe { IORegistryEntryCreateCFProperty(entry.0, &key, null(), 0) };
    let attributes = NonNull::new(raw)?;
    // SAFETY: the pointer is non-null and owns a reference, exactly what
    // `from_raw` documents.
    let attributes = unsafe { objc2_core_foundation::CFRetained::from_raw(attributes) };
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

    // SAFETY: `out` is an `i64`, which is what `SInt64Type` asks the value to
    // be written as.
    let read = unsafe { number.value(CFNumberType::SInt64Type, (&raw mut out).cast()) };

    read.then(|| u64::try_from(out).ok()).flatten()
}
