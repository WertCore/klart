//! What Core Graphics knows about an attached display.

use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayIsBuiltin, CGDisplayIsMain, CGDisplayModelNumber,
    CGDisplaySerialNumber, CGDisplayVendorNumber, CGError, CGGetActiveDisplayList,
};

use crate::display::Bounds;
use crate::error::{Error, Result};

/// Core Graphics wants the buffer sized up front. The largest Mac ever sold
/// drives eight displays, so this has room for four of those.
const MAX_DISPLAYS: u32 = 32;

/// Everything Core Graphics will say about one display.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CgDisplay {
    pub id: CGDirectDisplayID,
    /// The EDID manufacturer code, three letters packed into fifteen bits.
    pub vendor: u32,
    /// The EDID product code.
    pub model: u32,
    /// The EDID serial number, which is zero on plenty of real monitors.
    pub serial: u32,
    pub is_builtin: bool,
    pub is_main: bool,
    pub bounds: Bounds,
}

/// The displays that are on and drawing right now.
///
/// Active rather than online: a display that is asleep or mirrored to another is
/// online but has no desktop of its own, and nothing here can usefully address
/// one.
pub(crate) fn active_displays() -> Result<Vec<CgDisplay>> {
    let mut ids = vec![0; MAX_DISPLAYS as usize];
    let mut count: u32 = 0;

    // SAFETY: `ids` has `MAX_DISPLAYS` elements and `count` is a live `u32`,
    // which is what the two out pointers are documented to require.
    let status = unsafe { CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &raw mut count) };
    if status != CGError::Success {
        return Err(Error::CoreGraphics {
            call: "CGGetActiveDisplayList",
            code: status.0,
        });
    }

    ids.truncate(count as usize);
    Ok(ids.into_iter().map(describe).collect())
}

/// Reads the rest of the Core Graphics properties for one display.
///
/// None of these calls can fail: an identifier that is no longer valid — a
/// monitor unplugged between the list call and this one — reads back as zeroes
/// rather than an error, which is why the caller cannot distinguish that case.
fn describe(id: CGDirectDisplayID) -> CgDisplay {
    let frame = CGDisplayBounds(id);
    CgDisplay {
        id,
        vendor: CGDisplayVendorNumber(id),
        model: CGDisplayModelNumber(id),
        serial: CGDisplaySerialNumber(id),
        is_builtin: CGDisplayIsBuiltin(id),
        is_main: CGDisplayIsMain(id),
        bounds: Bounds {
            x: frame.origin.x.round() as i32,
            y: frame.origin.y.round() as i32,
            width: frame.size.width.round().max(0.0) as u32,
            height: frame.size.height.round().max(0.0) as u32,
        },
    }
}
