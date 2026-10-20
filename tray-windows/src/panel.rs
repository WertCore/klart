//! The panel that opens from the tray icon.
//!
//! Windows menus are not a place a slider can live. `muda` has no item that
//! holds one, and an owner-drawn `MENUITEMINFO` gives up its drag messages to
//! the menu's own modal loop. So this is a popup window instead: the shape of a
//! menu, drawn by hand, with a brightness bar for each display and the same
//! commands the macOS menu carries underneath.
//!
//! It is drawn rather than assembled from controls for two reasons. A trackbar
//! is a Windows control with its own idea of a theme, and on a hand-coloured
//! popup it looks like a trackbar: chunky, light in dark mode, and nothing like
//! the bar the menu bar agent draws. And the macOS menu redraws its slider
//! continuously while it is dragged; here the same thing is one `WM_MOUSEMOVE`
//! and a fill width.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DC_BRUSH, DC_PEN, DRAW_TEXT_FORMAT, DT_END_ELLIPSIS, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect,
    GetMonitorInfoW, GetStockObject, HDC, HFONT, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, NULL_PEN, PAINTSTRUCT, RoundRect, ScreenToClient, SelectObject,
    SetBkMode, SetDCBrushColor, SetDCPenColor, SetTextColor, TRANSPARENT, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_DROPSHADOW, CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, HWND_TOPMOST, IDC_ARROW, IDC_HAND, LoadCursorW, MA_ACTIVATE, PostMessageW,
    RegisterClassW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use klart_tray_common::request;

use crate::theme::{Fonts, Palette};
use crate::win::{contains, dpi, place, wide};
use crate::{App, WM_PANEL_CLOSE, WM_PANEL_COMMAND};

/// What a slider drives: one display, or all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    All,
    Display(usize),
}

/// One row the panel is built from.
pub struct SliderSpec {
    pub target: Target,
    pub caption: String,
    pub percent: u8,
    pub transient: bool,
}

/// One clickable row underneath the sliders.
pub struct CommandSpec {
    pub id: u32,
    pub text: String,
    pub checked: Option<bool>,
    pub enabled: bool,
}

/// The class name, registered once at startup.
const CLASS: &str = "KlartBrightnessPanel";

/// Logical measurements, in the 96-DPI units the layout is written in.
const WIDTH: i32 = 292;
const PAD: i32 = 12;
const LABEL_HEIGHT: i32 = 20;
const TRACK_HEIGHT: i32 = 26;
const NOTE_HEIGHT: i32 = 18;
const ROW_GAP: i32 = 4;
const SEPARATOR_HEIGHT: i32 = 13;
const COMMAND_HEIGHT: i32 = 30;
const KNOB: i32 = 7;
const BAR: i32 = 4;

struct Slider {
    target: Target,
    caption: String,
    percent: u8,
    transient: bool,
    label: RECT,
    track: RECT,
}

struct Command {
    id: u32,
    text: String,
    checked: Option<bool>,
    enabled: bool,
    rect: RECT,
}

/// A live popup. Owned by [`App`], which is also the stable address behind the
/// window's user data.
pub struct Panel {
    hwnd: HWND,
    owner: HWND,
    /// The agent, reached only from messages sent after the window exists.
    app: *mut App,
    generation: u32,
    dpi: u32,
    palette: Palette,
    fonts: Fonts,
    sliders: Vec<Slider>,
    commands: Vec<Command>,
    empty: bool,
    separator: Option<i32>,
    hover: Option<usize>,
    drag: Option<usize>,
    width: i32,
    height: i32,
    arrow: *mut c_void,
    hand: *mut c_void,
}

/// Registers the popup's class.
///
/// # Safety
///
/// Called once, on the thread that runs the message loop.
pub unsafe fn register(hinstance: HINSTANCE) -> bool {
    let name = wide(CLASS);
    // SAFETY: the cursor is a stock resource.
    let cursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };

    let class = WNDCLASSW {
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(panel_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: cursor,
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: name.as_ptr(),
    };

    // SAFETY: `class` is fully initialised and `name` is live for the call.
    unsafe { RegisterClassW(&class) != 0 }
}

/// Builds and shows the panel next to `icon`.
///
/// `None` when the window could not be created, which leaves the tray icon
/// working and simply does not open anything.
pub fn show(
    app: *mut App,
    owner: HWND,
    generation: u32,
    icon: RECT,
    sliders: Vec<SliderSpec>,
    commands: Vec<CommandSpec>,
) -> Option<Box<Panel>> {
    // SAFETY: called from the message loop's thread, which is the one that
    // created the owner window.
    unsafe { show_inner(app, owner, generation, icon, sliders, commands) }
}

unsafe fn show_inner(
    app: *mut App,
    owner: HWND,
    generation: u32,
    icon: RECT,
    sliders: Vec<SliderSpec>,
    commands: Vec<CommandSpec>,
) -> Option<Box<Panel>> {
    let point = POINT {
        x: icon.left + (icon.right - icon.left) / 2,
        y: icon.top + (icon.bottom - icon.top) / 2,
    };

    // The panel is sized for the monitor the icon is on, not the primary one.
    // A taskbar on a 150% monitor otherwise opens a panel that is legible on
    // the other screen and tiny on this one.
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    // SAFETY: both out parameters are live. A failure leaves the defaults in
    // place, which is the right answer for a pre-8.1 system.
    unsafe {
        GetDpiForMonitor(
            monitor,
            MDT_EFFECTIVE_DPI,
            std::ptr::addr_of_mut!(dpi_x),
            std::ptr::addr_of_mut!(dpi_y),
        )
    };
    let scale = dpi_x.max(72);

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = u32::try_from(size_of::<MONITORINFO>()).unwrap_or(0);
    // SAFETY: `info` is live and says its own size, which is what the call
    // checks.
    let work = if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        info.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };

    let palette = Palette::current();
    // SAFETY: GDI object creation on the message-loop thread.
    let fonts = unsafe { Fonts::new(scale) };
    // SAFETY: stock cursors.
    let arrow = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    // SAFETY: as above.
    let hand = unsafe { LoadCursorW(null_mut(), IDC_HAND) };

    let empty = sliders.is_empty();
    let mut panel = Box::new(Panel {
        hwnd: null_mut(),
        owner,
        app,
        generation,
        dpi: scale,
        palette,
        fonts,
        sliders: sliders
            .into_iter()
            .map(|spec| Slider {
                target: spec.target,
                caption: spec.caption,
                percent: spec.percent,
                transient: spec.transient,
                label: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                track: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
            })
            .collect(),
        commands: commands
            .into_iter()
            .map(|spec| Command {
                id: spec.id,
                text: spec.text,
                checked: spec.checked,
                enabled: spec.enabled,
                rect: RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
            })
            .collect(),
        empty,
        separator: None,
        hover: None,
        drag: None,
        width: 0,
        height: 0,
        arrow,
        hand,
    });
    panel.layout();

    let class = wide(CLASS);
    let title = wide("klart");
    // SAFETY: the class is registered and the panel pointer outlives the
    // window: it is the `Box` this function returns.
    let hinstance = unsafe { GetModuleHandleW(null()) };
    let panel_ptr = (&*panel as *const Panel).cast_mut().cast::<c_void>();
    // SAFETY: all pointers are live for the duration of the call, and
    // `panel_ptr` stays valid because the panel is boxed.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            panel.width,
            panel.height,
            owner,
            null_mut(),
            hinstance,
            panel_ptr,
        )
    };
    if hwnd.is_null() {
        return None;
    }
    panel.hwnd = hwnd;

    // Windows 11 rounds popups itself; earlier versions return an error and the
    // square corners are what they have always looked like.
    let preference: i32 = DWMWCP_ROUND;
    // The frame DWM draws around the panel follows the app mode, not the
    // colours painted inside it; without this a dark panel has a light edge.
    let dark: i32 = i32::from(panel.palette.is_dark());
    // SAFETY: both attributes are `i32`s of the documented size.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            std::ptr::addr_of!(preference).cast(),
            u32::try_from(size_of::<i32>()).unwrap_or(4),
        );
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            std::ptr::addr_of!(dark).cast(),
            u32::try_from(size_of::<i32>()).unwrap_or(4),
        );
    }

    let (x, y) = place(icon, panel.width, panel.height, work, dpi(6, scale));
    // SAFETY: the window exists and the rectangle is in screen coordinates.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            panel.width,
            panel.height,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOWNORMAL);
        // A tray popup has to be foreground or clicking elsewhere will not
        // deactivate it, and it would stay on screen over everything.
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }

    Some(panel)
}

impl Panel {
    fn layout(&mut self) {
        let pad = dpi(PAD, self.dpi);
        let width = dpi(WIDTH, self.dpi);
        let mut y = pad;

        self.empty = self.sliders.is_empty();
        if self.empty {
            y += dpi(LABEL_HEIGHT, self.dpi) + dpi(ROW_GAP, self.dpi);
        }

        for slider in &mut self.sliders {
            let left = pad;
            let right = width - pad;
            slider.label = RECT {
                left,
                top: y,
                right,
                bottom: y + dpi(LABEL_HEIGHT, self.dpi),
            };
            y += dpi(LABEL_HEIGHT, self.dpi);

            slider.track = RECT {
                left,
                top: y,
                right,
                bottom: y + dpi(TRACK_HEIGHT, self.dpi),
            };
            y += dpi(TRACK_HEIGHT, self.dpi);

            if slider.transient {
                y += dpi(NOTE_HEIGHT, self.dpi);
            }
            y += dpi(ROW_GAP, self.dpi);
        }

        if !self.commands.is_empty() {
            let separator = dpi(SEPARATOR_HEIGHT, self.dpi);
            self.separator = Some(y + separator / 2);
            y += separator;

            let inset = pad / 2;
            for command in &mut self.commands {
                command.rect = RECT {
                    left: inset,
                    top: y,
                    right: width - inset,
                    bottom: y + dpi(COMMAND_HEIGHT, self.dpi),
                };
                y += dpi(COMMAND_HEIGHT, self.dpi);
            }
        } else {
            self.separator = None;
        }

        self.width = width;
        self.height = y + pad;
    }

    fn slider_at(&self, x: i32, y: i32) -> Option<usize> {
        self.sliders
            .iter()
            .position(|slider| contains(slider.track, x, y))
    }

    fn command_at(&self, x: i32, y: i32) -> Option<usize> {
        self.commands
            .iter()
            .position(|command| contains(command.rect, x, y))
    }

    fn move_to(&mut self, slider: usize, x: i32) {
        let Some(row) = self.sliders.get_mut(slider) else {
            return;
        };

        let knob = dpi(KNOB, self.dpi);
        let left = row.track.left + knob;
        let right = row.track.right - knob;
        if right <= left {
            return;
        }

        let fraction = f32::from(i16::try_from((x - left).clamp(0, right - left)).unwrap_or(0))
            / f32::from(i16::try_from(right - left).unwrap_or(1));
        let percent = (fraction * 100.0).round().clamp(0.0, 100.0) as u8;
        self.commit(slider, percent);
    }

    /// Moves one slider by a number of percent, for a wheel over it.
    fn step(&mut self, slider: usize, percent: i32) {
        let Some(row) = self.sliders.get(slider) else {
            return;
        };
        let moved = (i16::from(row.percent) + i16::try_from(percent).unwrap_or(0)).clamp(0, 100);
        self.commit(slider, u8::try_from(moved).unwrap_or(row.percent));
    }

    /// Records a level, paints it, and asks for it to be written.
    fn commit(&mut self, slider: usize, percent: u8) {
        let Some(row) = self.sliders.get_mut(slider) else {
            return;
        };
        if row.percent == percent {
            return;
        }

        row.percent = percent;
        let target = row.target;
        let label = row.label;
        let track = row.track;

        // Only the heading's number and this row's fill changed; repainting the
        // whole panel would flicker every other slider.
        let mut dirty = label;
        dirty.bottom = track.bottom;
        // The paint is forced here rather than left until the message queue is
        // empty. A write can take a fifth of a second on a DDC/CI display, and
        // if it went first the knob would not move until the monitor answered.
        // SAFETY: `dirty` is a live rectangle in client coordinates and the
        // window is alive for this message.
        unsafe {
            InvalidateRect(self.hwnd, &dirty, 0);
            UpdateWindow(self.hwnd);
        }

        // Ask, do not write. The app hands this to a thread that is allowed to
        // wait for the display; the message loop goes straight back to the
        // pointer.
        // SAFETY: the panel exists only while the app does, and the app is not
        // borrowed by the message that reached here.
        unsafe {
            (*self.app).level(target, percent);
        }
    }

    fn paint(&self) {
        // SAFETY: `paint` is only reached from `WM_PAINT`, and the window is
        // live for the duration of the message.
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, std::ptr::addr_of_mut!(ps));
            if hdc.is_null() {
                return;
            }

            let mut client: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, std::ptr::addr_of_mut!(client));
            fill(hdc, client, self.palette.background);

            if self.empty {
                let mut rect = RECT {
                    left: dpi(PAD, self.dpi),
                    top: dpi(PAD, self.dpi),
                    right: client.right - dpi(PAD, self.dpi),
                    bottom: dpi(PAD + LABEL_HEIGHT, self.dpi),
                };
                draw(
                    hdc,
                    "No displays found",
                    &mut rect,
                    self.palette.dim,
                    self.fonts.regular(),
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
                );
            }

            for slider in &self.sliders {
                self.paint_slider(hdc, slider);
            }

            if let Some(y) = self.separator {
                let line = RECT {
                    left: dpi(PAD, self.dpi),
                    top: y,
                    right: client.right - dpi(PAD, self.dpi),
                    bottom: y + 1,
                };
                fill(hdc, line, self.palette.separator);
            }

            for (index, command) in self.commands.iter().enumerate() {
                if self.hover == Some(index) && command.enabled {
                    rounded(hdc, command.rect, self.palette.hover, dpi(6, self.dpi));
                }

                if command.checked == Some(true) {
                    let mut mark = command.rect;
                    mark.left += dpi(10, self.dpi);
                    mark.right = mark.left + dpi(16, self.dpi);
                    draw(
                        hdc,
                        "\u{2713}",
                        &mut mark,
                        self.palette.text,
                        self.fonts.regular(),
                        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
                    );
                }

                let mut text = command.rect;
                text.left += dpi(30, self.dpi);
                text.right -= dpi(10, self.dpi);
                let colour = if command.enabled {
                    self.palette.text
                } else {
                    self.palette.dim
                };
                draw(
                    hdc,
                    &command.text,
                    &mut text,
                    colour,
                    self.fonts.regular(),
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
                );
            }

            EndPaint(self.hwnd, &ps);
        }
    }

    fn paint_slider(&self, hdc: HDC, slider: &Slider) {
        // SAFETY: `hdc` is the paint DC handed to `paint`, and every GDI object
        // is selected back out before it is released.
        unsafe {
            let mut label = slider.label;
            let caption = request::heading(&slider.caption, slider.percent, slider.transient);
            draw(
                hdc,
                &caption,
                &mut label,
                self.palette.text,
                self.fonts.semibold(),
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS,
            );

            let knob = dpi(KNOB, self.dpi);
            let bar = dpi(BAR, self.dpi).max(2);
            let left = slider.track.left + knob;
            let right = slider.track.right - knob;
            let centre = slider.track.top + (slider.track.bottom - slider.track.top) / 2;

            let track = RECT {
                left,
                top: centre - bar / 2,
                right,
                bottom: centre - bar / 2 + bar,
            };
            rounded(hdc, track, self.palette.track, bar / 2);

            let filled_right =
                left + ((right - left) as f32 * f32::from(slider.percent) / 100.0).round() as i32;
            if filled_right > left {
                let filled = RECT {
                    left,
                    top: track.top,
                    right: filled_right,
                    bottom: track.bottom,
                };
                rounded(hdc, filled, self.palette.fill, bar / 2);
            }

            dot(
                hdc,
                filled_right,
                centre,
                knob,
                self.palette.knob,
                self.palette.knob_edge,
            );

            if slider.transient {
                let mut note = RECT {
                    left: slider.track.left + dpi(10, self.dpi),
                    top: slider.track.bottom,
                    right: slider.track.right,
                    bottom: slider.track.bottom + dpi(NOTE_HEIGHT, self.dpi),
                };
                draw(
                    hdc,
                    "\u{00b7} held only while klart runs",
                    &mut note,
                    self.palette.dim,
                    self.fonts.regular(),
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
                );
            }
        }
    }

    fn on_message(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_PAINT => {
                self.paint();
                0
            }
            WM_ERASEBKGND => 1,
            WM_MOUSEMOVE => {
                let (x, y) = point(lparam);
                if let Some(slider) = self.drag {
                    self.move_to(slider, x);
                } else {
                    let hover = self
                        .command_at(x, y)
                        .filter(|&index| self.commands[index].enabled);
                    if hover != self.hover {
                        self.hover = hover;
                        // SAFETY: the window is live for this message.
                        unsafe { InvalidateRect(hwnd, null(), 0) };
                    }
                    // SAFETY: stock cursors, and the window is live.
                    unsafe {
                        SetCursor(if hover.is_some() {
                            self.hand
                        } else {
                            self.arrow
                        });
                    }
                }
                0
            }
            WM_MOUSEWHEEL => {
                // A notch is 120 units, and the wheel is handled here rather
                // than over the icon because the shell does not forward wheel
                // messages to notification icons. `lparam` is in screen
                // coordinates for this message.
                let (x, y) = point(lparam);
                let mut client = POINT { x, y };
                // SAFETY: the window is live and `client` is a live point.
                unsafe { ScreenToClient(hwnd, &mut client) };

                if let Some(slider) = self.slider_at(client.x, client.y) {
                    let delta = i32::from((wparam >> 16) as u16 as i16);
                    let notches = delta / 120;
                    if notches != 0 {
                        self.step(slider, notches * 5);
                    }
                }
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lparam);
                // Focus, so Escape works even though nothing here is a control.
                // SAFETY: the window is live.
                unsafe { SetFocus(hwnd) };

                if let Some(slider) = self.slider_at(x, y) {
                    self.drag = Some(slider);
                    // SAFETY: the window is live, and the capture is released
                    // either on the button coming up or on losing capture.
                    unsafe { SetCapture(hwnd) };
                    self.move_to(slider, x);
                }
                0
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lparam);
                if let Some(slider) = self.drag.take() {
                    self.move_to(slider, x);
                    // SAFETY: this panel holds the capture.
                    unsafe { ReleaseCapture() };
                    // The drag's last value is the one the person chose; the
                    // rate limiter may have held it back.
                    // SAFETY: the app outlives the panel.
                    unsafe { (*self.app).flush() };
                } else if let Some(index) = self.command_at(x, y) {
                    let command = &self.commands[index];
                    if command.enabled {
                        // Posted rather than run here: the command may close
                        // this panel, and destroying the window from inside its
                        // own message is how a use-after-free starts.
                        // SAFETY: the owner window is live and outlives the
                        // panel.
                        unsafe {
                            PostMessageW(
                                self.owner,
                                WM_PANEL_COMMAND,
                                command.id as usize,
                                self.generation as LPARAM,
                            );
                        }
                    }
                }
                0
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                0
            }
            WM_ACTIVATE => {
                if (wparam as u32) & 0xFFFF == WA_INACTIVE {
                    // Closing is the owner's job, for the same reason commands
                    // are posted: this message is dispatched to a window that
                    // is about to be destroyed.
                    // SAFETY: the owner window is live and outlives the panel.
                    unsafe {
                        PostMessageW(self.owner, WM_PANEL_CLOSE, self.generation as usize, 0);
                    }
                }
                0
            }
            WM_KEYDOWN => {
                if wparam as u32 == u32::from(VK_ESCAPE) {
                    // SAFETY: as above.
                    unsafe {
                        PostMessageW(self.owner, WM_PANEL_CLOSE, self.generation as usize, 0);
                    }
                }
                0
            }
            WM_MOUSEACTIVATE => MA_ACTIVATE as LRESULT,
            WM_DESTROY | WM_NCDESTROY => 0,
            _ => {
                // SAFETY: forwarding the message this window was sent.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this panel, and the user data is
        // cleared first so the destroy messages cannot reach the `Panel` that
        // is halfway through being dropped.
        unsafe {
            if !self.hwnd.is_null() {
                SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
                // SAFETY: the handle is live and is destroyed exactly once.
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
            }
        }
    }
}

/// The window procedure for the popup.
///
/// # Safety
///
/// Called by Windows with a live window handle.
unsafe extern "system" fn panel_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: `lparam` is the `CREATESTRUCTW` for this creation, and
        // `lpCreateParams` is the pointer `CreateWindowExW` was given.
        unsafe {
            let create = &*(lparam as *const CREATESTRUCTW);
            let panel = create.lpCreateParams.cast::<Panel>();
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, panel as isize);
            if !panel.is_null() {
                (*panel).hwnd = hwnd;
            }
        }
        return 1;
    }

    // SAFETY: set in `WM_NCCREATE` above, and cleared before destruction.
    let panel = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Panel };
    if panel.is_null() {
        // SAFETY: the default handler is the right answer for a window with no
        // panel behind it.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    // SAFETY: the pointer is this window's own panel, which outlives the
    // message.
    unsafe { (*panel).on_message(hwnd, msg, wparam, lparam) }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam as i32;
    (i32::from(packed as i16), i32::from((packed >> 16) as i16))
}

/// Fills a rectangle with one colour.
///
/// # Safety
///
/// `hdc` must be a valid device context.
unsafe fn fill(hdc: HDC, rect: RECT, colour: COLORREF) {
    // SAFETY: the brush is created and released around the fill.
    unsafe {
        let brush = CreateSolidBrush(colour);
        FillRect(hdc, &rect, brush);
        DeleteObject(brush);
    }
}

/// Fills a rounded rectangle with one colour.
///
/// The stock `DC_BRUSH` and `NULL_PEN` avoid creating and deleting two objects
/// per call, of which a drag makes many.
///
/// # Safety
///
/// `hdc` must be a valid device context.
unsafe fn rounded(hdc: HDC, rect: RECT, colour: COLORREF, radius: i32) {
    // SAFETY: stock objects are selected and restored in pairs.
    unsafe {
        let old_brush = SelectObject(hdc, GetStockObject(DC_BRUSH));
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        SetDCBrushColor(hdc, colour);
        RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
    }
}

/// Draws the slider's knob.
///
/// # Safety
///
/// `hdc` must be a valid device context.
unsafe fn dot(hdc: HDC, x: i32, y: i32, radius: i32, fill: COLORREF, edge: COLORREF) {
    // SAFETY: stock objects are selected and restored in pairs.
    unsafe {
        let old_brush = SelectObject(hdc, GetStockObject(DC_BRUSH));
        let old_pen = SelectObject(hdc, GetStockObject(DC_PEN));
        SetDCBrushColor(hdc, fill);
        SetDCPenColor(hdc, edge);
        Ellipse(hdc, x - radius, y - radius, x + radius + 1, y + radius + 1);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
    }
}

/// Draws one line of text, with no background.
///
/// # Safety
///
/// `hdc` must be a valid device context and `rect` the area to draw in.
unsafe fn draw(
    hdc: HDC,
    text: &str,
    rect: &mut RECT,
    colour: COLORREF,
    font: HFONT,
    flags: DRAW_TEXT_FORMAT,
) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let length = i32::try_from(units.len()).unwrap_or(i32::MAX);

    // SAFETY: the string is live for the call, and the font is owned by the
    // panel that is painting.
    unsafe {
        let old = SelectObject(hdc, font);
        SetTextColor(hdc, colour);
        SetBkMode(hdc, TRANSPARENT as i32);
        DrawTextW(hdc, units.as_ptr(), length, rect, flags);
        SelectObject(hdc, old);
    }
}
