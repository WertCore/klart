//! The sun in the notification area.
//!
//! macOS uses the `sun.max` system symbol, which is a template image and
//! therefore has no colour of its own. Windows has no template images for tray
//! icons, so the glyph is drawn here into an alpha bitmap: the same sun, at
//! whatever size the notification area currently asks for, and in the ink the
//! system's own tray icons use — white on a dark taskbar, black on a light one.
//! The shell does not recolour a third-party icon, so the agent redraws and
//! replaces this one when Windows mode changes.
//!
//! It is drawn rather than shipped as an `.ico` so that it is sharp at 16, 20,
//! 24 and 32 pixels — the sizes Windows picks between as the DPI changes — and
//! so both inks come from one description of the shape.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

/// How many samples per pixel each edge of the glyph is anti-aliased with.
const SAMPLES: i32 = 4;

/// The radius of the disc, as a fraction of the icon's width.
const DISC: f32 = 0.20;

/// How far the rays begin and end, in the same units.
const RAY_INNER: f32 = 0.29;
const RAY_OUTER: f32 = 0.47;

/// Half the width of a ray.
const RAY_HALF: f32 = 0.045;

/// Makes an icon sized for the notification area.
///
/// `on_light_taskbar` selects the ink: the system's own icons are black on a
/// light taskbar and white on a dark one, and this follows them.
///
/// The caller owns it and must hand it to `DestroyIcon`.
#[must_use]
pub fn sun(on_light_taskbar: bool) -> HICON {
    // SAFETY: no arguments, and the call has no side effects.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    // SAFETY: `size` is positive and GDI object creation is on the main thread.
    unsafe { render(size, on_light_taskbar) }
}

/// Draws the sun at `size` pixels square.
///
/// # Safety
///
/// GDI object creation and `CreateIconIndirect` must be called from a thread
/// that is allowed to own them, which in this program is the main thread.
unsafe fn render(size: i32, on_light_taskbar: bool) -> HICON {
    let side = usize::try_from(size).unwrap_or(16);
    let mut pixels = vec![0u32; side * side];

    for y in 0..size {
        for x in 0..size {
            let coverage = coverage(size, x, y);
            if coverage <= 0.0 {
                continue;
            }

            // `0xAARRGGBB` in a little-endian word is the BGRA byte order a
            // 32-bit DIB holds.
            let alpha = (coverage * 255.0).round().clamp(0.0, 255.0) as u32;
            let index = usize::try_from(y * size + x).unwrap_or(0);
            pixels[index] = (alpha << 24) | ink(on_light_taskbar);
        }
    }

    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or(0);
    info.bmiHeader.biWidth = size;
    // Negative, so row zero is the top one and the loop above can write in
    // reading order.
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;

    let mut bits: *mut c_void = null_mut();
    // SAFETY: the header describes a 32-bit top-down bitmap, and `bits` is a
    // live out parameter that receives `side * side` words.
    let colour = unsafe {
        CreateDIBSection(
            null_mut(),
            &info,
            DIB_RGB_COLORS,
            std::ptr::addr_of_mut!(bits),
            null_mut(),
            0,
        )
    };
    if colour.is_null() || bits.is_null() {
        return null_mut();
    }

    // SAFETY: the section is at least `pixels.len()` 32-bit words, which is what
    // is copied into it.
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
    }

    // A 32-bit icon's alpha channel does the transparency. The mask still has to
    // exist, and zeroes mean "ask the colour bitmap", which is what is wanted.
    let stride = side.div_ceil(16) * 2;
    let mask_bits = vec![0u8; stride * side];
    // SAFETY: `mask_bits` is exactly the size a 1-bit bitmap of this shape
    // needs.
    let mask = unsafe { CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast::<c_void>()) };
    if mask.is_null() {
        // SAFETY: created above and not used again.
        unsafe { DeleteObject(colour) };
        return null_mut();
    }

    let info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: colour,
    };

    // SAFETY: both bitmaps are live and of the shape the structure says.
    let icon = unsafe { CreateIconIndirect(&info) };

    // SAFETY: `CreateIconIndirect` copied what it needed, and both bitmaps are
    // released exactly once here.
    unsafe {
        DeleteObject(colour);
        DeleteObject(mask);
    }

    icon
}

/// The ink the system's own tray icons use on each taskbar.
fn ink(on_light_taskbar: bool) -> u32 {
    if on_light_taskbar { 0x000000 } else { 0xFFFFFF }
}

/// How much of one pixel the sun covers, from 0 to 1.
fn coverage(size: i32, x: i32, y: i32) -> f32 {
    let step = 1.0 / f32::from(i16::try_from(SAMPLES).unwrap_or(4));
    let mut hits = 0;

    for sample_y in 0..SAMPLES {
        for sample_x in 0..SAMPLES {
            let px = x as f32 + (sample_x as f32 + 0.5) * step;
            let py = y as f32 + (sample_y as f32 + 0.5) * step;
            if is_sun(px, py, size as f32) {
                hits += 1;
            }
        }
    }

    hits as f32 / (SAMPLES * SAMPLES) as f32
}

/// Whether a point in pixel coordinates falls inside the sun.
fn is_sun(x: f32, y: f32, size: f32) -> bool {
    let centre = size / 2.0;
    let u = (x - centre) / size;
    let v = (y - centre) / size;

    if u * u + v * v <= DISC * DISC {
        return true;
    }

    // Eight rays, the same count `sun.max` uses.
    for ray in 0..8 {
        let angle = f32::from(i16::try_from(ray).unwrap_or(0)) * std::f32::consts::FRAC_PI_4;
        let (dx, dy) = (angle.cos(), angle.sin());

        let along = u * dx + v * dy;
        let clamped = along.clamp(RAY_INNER, RAY_OUTER);
        let ox = u - dx * clamped;
        let oy = v - dy * clamped;

        if ox * ox + oy * oy <= RAY_HALF * RAY_HALF {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_centre_of_the_disc_is_solid() {
        assert!(is_sun(50.0, 50.0, 100.0));
    }

    #[test]
    fn the_corners_are_empty() {
        assert!(!is_sun(0.0, 0.0, 100.0));
        assert!(!is_sun(99.9, 99.9, 100.0));
    }

    #[test]
    fn a_ray_reaches_out_from_the_disc() {
        // Straight up from the centre: a ray begins outside the disc and ends
        // before the edge of the icon.
        assert!(is_sun(50.0, 50.0 - 38.0, 100.0));
        // The cap is rounded, so the last covered point is just past
        // `RAY_OUTER` rather than at it.
        assert!(
            !is_sun(50.0, -2.0, 100.0),
            "a ray must not reach the edge of the icon"
        );
    }

    #[test]
    fn a_small_icon_still_has_covered_pixels() {
        let mut covered = 0;
        for y in 0..16 {
            for x in 0..16 {
                if coverage(16, x, y) > 0.0 {
                    covered += 1;
                }
            }
        }
        assert!(covered > 20, "a 16px sun has to be visible: {covered}");
    }

    #[test]
    fn the_ink_follows_the_taskbar() {
        assert_eq!(ink(true), 0x000000, "a light taskbar needs a dark glyph");
        assert_eq!(ink(false), 0xFFFFFF, "a dark taskbar needs a light glyph");
    }
}
