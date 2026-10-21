//! The panel's colours and type, which follow the system's light or dark mode.

use std::mem::size_of;

use windows_sys::Win32::Foundation::COLORREF;
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, DEFAULT_CHARSET, DeleteObject, FW_NORMAL, FW_SEMIBOLD, HFONT,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

use crate::win::{dpi, wide};

/// Where Windows keeps the setting that decides it.
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// What the panel is drawn in.
///
/// Windows has no single "menu colour" that a hand-drawn popup can use: the
/// system colours describe the classic 3D widgets, and a Windows 11 menu is a
/// flat surface in whichever mode the person chose. These are the values those
/// modes use, near enough to sit next to the system's own flyouts without
/// looking like a different program.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: COLORREF,
    pub text: COLORREF,
    pub dim: COLORREF,
    pub track: COLORREF,
    pub fill: COLORREF,
    pub knob: COLORREF,
    pub knob_edge: COLORREF,
    pub separator: COLORREF,
    pub hover: COLORREF,
}

impl Palette {
    /// The colours for the mode the system is in.
    #[must_use]
    pub fn current() -> Self {
        if apps_use_light_theme() {
            Self::light()
        } else {
            Self::dark()
        }
    }

    /// Whether this is the dark palette.
    ///
    /// The popup is painted itself, but the window's border and shadow are
    /// DWM's, and they are light unless it is told otherwise.
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.background == Self::dark().background
    }

    fn light() -> Self {
        Self {
            background: rgb(249, 249, 249),
            text: rgb(26, 26, 26),
            dim: rgb(100, 100, 100),
            track: rgb(214, 214, 214),
            fill: rgb(0, 120, 212),
            knob: rgb(255, 255, 255),
            knob_edge: rgb(0, 95, 184),
            separator: rgb(226, 226, 226),
            hover: rgb(237, 237, 237),
        }
    }

    fn dark() -> Self {
        Self {
            background: rgb(43, 43, 43),
            text: rgb(255, 255, 255),
            dim: rgb(174, 174, 174),
            track: rgb(92, 92, 92),
            fill: rgb(76, 194, 255),
            knob: rgb(255, 255, 255),
            knob_edge: rgb(76, 194, 255),
            separator: rgb(68, 68, 68),
            hover: rgb(60, 60, 60),
        }
    }
}

/// The two weights the panel uses.
///
/// A display's heading is semibold and everything else is not, which is the
/// difference the macOS menu makes with its two system font weights.
pub struct Fonts {
    regular: HFONT,
    semibold: HFONT,
}

impl Fonts {
    /// Creates the fonts at a monitor's DPI.
    ///
    /// # Safety
    ///
    /// Must be called on a thread that may create GDI objects; the handles are
    /// released by [`Drop`] on the same thread.
    pub unsafe fn new(scale: u32) -> Self {
        // Nine-point Segoe UI is twelve pixels at 96 DPI, and scales from there
        // the way every other menu on the system does.
        let height = -dpi(12, scale);
        let face = wide("Segoe UI");

        // SAFETY: `face` is live and NUL-terminated for both calls.
        let regular = unsafe {
            windows_sys::Win32::Graphics::Gdi::CreateFontW(
                height,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                u32::from(DEFAULT_CHARSET),
                0,
                0,
                u32::from(CLEARTYPE_QUALITY),
                0,
                face.as_ptr(),
            )
        };
        // SAFETY: as above.
        let semibold = unsafe {
            windows_sys::Win32::Graphics::Gdi::CreateFontW(
                height,
                0,
                0,
                0,
                FW_SEMIBOLD as i32,
                0,
                0,
                0,
                u32::from(DEFAULT_CHARSET),
                0,
                0,
                u32::from(CLEARTYPE_QUALITY),
                0,
                face.as_ptr(),
            )
        };

        Self { regular, semibold }
    }

    #[must_use]
    pub fn regular(&self) -> HFONT {
        self.regular
    }

    #[must_use]
    pub fn semibold(&self) -> HFONT {
        self.semibold
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: both handles came from `CreateFontW` and are released once.
        unsafe {
            DeleteObject(self.regular);
            DeleteObject(self.semibold);
        }
    }
}

/// Whether the system's taskbar and menus are in light mode.
///
/// The registry is the only public answer. `SystemUsesLightTheme` is the
/// taskbar; `AppsUseLightTheme` is menus and flyouts, which is what this panel
/// is imitating. A missing value means a fresh profile, whose Windows default is
/// light.
#[must_use]
pub fn apps_use_light_theme() -> bool {
    read_dword("AppsUseLightTheme").unwrap_or(1) != 0
}

/// Whether the taskbar and its notification icons are light.
///
/// This is deliberately not [`apps_use_light_theme`]: Windows lets the taskbar
/// and the apps disagree, and the system's own tray icons follow the taskbar.
/// The panel follows the app setting; the sun follows this one.
#[must_use]
pub fn taskbar_uses_light_theme() -> bool {
    read_dword("SystemUsesLightTheme")
        .or_else(|| read_dword("AppsUseLightTheme"))
        .unwrap_or(1)
        != 0
}

fn read_dword(value: &str) -> Option<u32> {
    let path = wide(PERSONALIZE);
    let name = wide(value);
    let mut data: u32 = 0;
    let mut size = u32::try_from(size_of::<u32>()).ok()?;

    // SAFETY: both strings are live and NUL-terminated, and `data`/`size` are
    // live out parameters of the size this call is told to expect.
    let found = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(data).cast(),
            std::ptr::addr_of_mut!(size),
        )
    };

    (found == 0).then_some(data)
}

const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    (red as u32) | ((green as u32) << 8) | ((blue as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `COLORREF` is 0x00BBGGRR, not the 0x00RRGGBB the rest of the world
    /// writes. Getting that backwards swaps red and blue in every colour here.
    #[test]
    fn a_colour_is_packed_the_way_gdi_expects() {
        assert_eq!(rgb(1, 2, 3), 0x0003_0201);
        assert_eq!(rgb(255, 0, 0), 0x0000_00ff);
        assert_eq!(rgb(0, 0, 255), 0x00ff_0000);
    }
}
