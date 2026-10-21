//! Small things the Win32 calls all need and none of them provide.

use windows_sys::Win32::Foundation::RECT;

/// A NUL-terminated UTF-16 copy of `text`.
///
/// The Win32 wide-string APIs want a length they can walk to a terminator, and
/// every call here has to keep the buffer alive for the duration of the call,
/// so this returns an owned vector rather than something borrowed.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Copies `text` into a fixed-size field such as `szTip`, truncating it.
///
/// Win32 structures carry their strings as arrays, and overrunning one writes
/// into the next field rather than failing. The field always keeps its
/// terminator.
pub fn copy_into<const N: usize>(field: &mut [u16; N], text: &str) {
    let mut written = 0;
    for unit in text.encode_utf16() {
        if written + 1 >= N {
            break;
        }
        field[written] = unit;
        written += 1;
    }
    field[written] = 0;
}

/// A logical pixel measurement at a monitor's DPI.
///
/// The panel is laid out in the same units macOS uses for its menu — a 292 by
/// whatever at 100% — and scaled for the monitor it lands on, so it is the same
/// physical size at 125% and 200% rather than shrinking.
pub fn dpi(value: i32, dpi: u32) -> i32 {
    let scaled = i64::from(value) * i64::from(dpi) / 96;
    i32::try_from(scaled).unwrap_or(value)
}

/// Whether a point is inside a rectangle.
///
/// Half-open on the far edges, matching the display geometry in `klart-core`, so
/// two adjacent rows cannot both claim their shared boundary.
pub fn contains(rect: RECT, x: i32, y: i32) -> bool {
    x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
}

/// Where a popup of this size goes, next to the tray icon and inside the work
/// area.
///
/// Above the icon by preference, which is where the notification area lives on
/// a taskbar at the bottom of the screen, and below it when the taskbar is at
/// the top. The result is clamped into the work area in both directions, because
/// a panel that runs off the screen cannot be clicked and there is no way for
/// the person to move it.
pub fn place(icon: RECT, width: i32, height: i32, work: RECT, gap: i32) -> (i32, i32) {
    let mut x = icon.right - width;
    let mut y = icon.top - height - gap;

    // A taskbar at the top has no room above it, so the panel goes underneath.
    if y < work.top {
        y = icon.bottom + gap;
    }

    if x + width > work.right {
        x = work.right - width;
    }
    if x < work.left {
        x = work.left;
    }
    if y + height > work.bottom {
        y = work.bottom - height;
    }
    if y < work.top {
        y = work.top;
    }

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_panel_sits_above_a_bottom_taskbar_and_aligned_to_the_icon() {
        // A 1920x1080 screen, taskbar at the bottom, icon at its far right.
        let work = rect(0, 0, 1920, 1040);
        let icon = rect(1840, 1030, 1864, 1054);
        let (x, y) = place(icon, 300, 400, work, 8);

        assert_eq!(x, 1864 - 300, "the panel's right edge follows the icon's");
        assert_eq!(y, 1030 - 400 - 8);
    }

    #[test]
    fn a_taskbar_at_the_top_puts_the_panel_below_it() {
        let work = rect(0, 40, 1920, 1080);
        let icon = rect(1840, 10, 1864, 34);
        let (_, y) = place(icon, 300, 400, work, 8);

        assert_eq!(y, 34 + 8);
    }

    #[test]
    fn a_panel_that_would_run_off_the_left_is_moved_inside() {
        let work = rect(0, 0, 1920, 1040);
        let icon = rect(10, 1030, 34, 1054);
        let (x, _) = place(icon, 300, 400, work, 8);

        assert_eq!(x, 0);
    }

    #[test]
    fn a_panel_taller_than_the_work_area_is_pinned_to_its_top() {
        let work = rect(0, 0, 1920, 300);
        let icon = rect(1840, 1030, 1864, 1054);
        let (_, y) = place(icon, 300, 400, work, 8);

        assert_eq!(y, 0, "there is nowhere else for it to go");
    }

    #[test]
    fn a_point_on_the_far_edge_is_not_in_the_row() {
        let row = rect(0, 10, 100, 30);
        assert!(contains(row, 0, 10));
        assert!(contains(row, 99, 29));
        assert!(!contains(row, 100, 20));
        assert!(!contains(row, 50, 30));
    }

    #[test]
    fn a_truncated_string_still_ends_in_a_terminator() {
        let mut field = [0xFFFFu16; 5];
        copy_into(&mut field, "brightness");

        assert_eq!(
            &field[..5],
            &[b'b' as u16, b'r' as u16, b'i' as u16, b'g' as u16, 0],
            "four characters and a terminator are all a field of five holds"
        );
    }

    #[test]
    fn scaling_is_relative_to_ninety_six_dpi() {
        assert_eq!(dpi(100, 96), 100);
        assert_eq!(dpi(100, 144), 150);
        assert_eq!(dpi(100, 192), 200);
    }
}
