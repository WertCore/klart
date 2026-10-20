//! `klart` in the notification area.
//!
//! An agent rather than an application: no window, no taskbar button, no
//! console. It holds a `Shell_NotifyIcon` icon and, because the gamma ramp a
//! display is dimmed with is reverted the moment the process that set it exits,
//! it stays running so that a display with no hardware brightness control can
//! stay dim.
//!
//! The icon opens a hand-drawn popup rather than a menu, for the reason
//! [`panel`] gives: a Windows menu cannot hold a slider that is dragged.
//!
//! What is shared with the macOS agent is [`klart_tray_common::driver`] — the
//! rate limiting, the remembered levels, the restore after a wake. This file is
//! everything that is the platform's own: a hidden message window, the icon's
//! lifetime, and the timer that drives the shared driver.

// A tray agent has nowhere to print to in release: there is no console attached
// to a `windows` subsystem binary, and a window would defeat the point of one.
// Debug builds keep the console so that `eprintln!` is visible.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

// Like `klart-tray` on the other side: a tray is a platform's own program, not
// a build flag. This one is `Shell_NotifyIcon` and a hand-drawn popup, and none
// of it has an equivalent to `cfg` its way onto another platform.
#[cfg(not(target_os = "windows"))]
compile_error!(
    "klart-tray-windows is Win32 and Windows only. `klart-core`, `klart-cli` \
     and `klart-tray-common` build everywhere; the menu bar agent is \
     `klart-tray`, and a tray for another platform is a new crate, not a cfg"
);

mod icon;
mod panel;
mod theme;
mod win;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use klart_core::controls;
use klart_core::{Combined, LoginItem};
use klart_tray_common::driver::Driver;
use panel::{CommandSpec, SliderSpec, Target};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DBT_DEVNODES_CHANGED, DefWindowProcW, DestroyIcon,
    DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    IDC_ARROW, IDI_APPLICATION, KillTimer, LoadCursorW, LoadIconW, MSG, PBT_APMRESUMEAUTOMATIC,
    PBT_APMRESUMESUSPEND, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetTimer,
    SetWindowLongPtrW, TranslateMessage, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCCREATE,
    WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW,
};

use crate::win::{copy_into, wide};

/// The message the shell sends when the pointer is used on the icon.
const WM_TRAY: u32 = WM_APP + 1;

/// Posted by the panel when it has lost focus and should go away.
const WM_PANEL_CLOSE: u32 = WM_APP + 2;

/// Posted by the panel when a command row was clicked.
const WM_PANEL_COMMAND: u32 = WM_APP + 3;

/// The panel's commands, carried in the `wparam` of [`WM_PANEL_COMMAND`].
const CMD_KEEP_OFFSETS: u32 = 1;
const CMD_LOGIN_ITEM: u32 = 2;
const CMD_REFRESH: u32 = 3;
const CMD_QUIT: u32 = 4;

/// The icon's identifier within the shell, and the timer's within the window.
const ICON_ID: u32 = 1;
const TIMER_ID: usize = 1;

/// How often the shared driver is given a turn.
///
/// The slowest thing it asks for is the retry gap of a wake restore, which is
/// 500 ms, and a value held back by the write rate limiter should not wait much
/// longer than that either. Any faster is waking the process for nothing.
const TICK: u32 = 500;

/// How long a click is allowed to be the second half of the same gesture.
///
/// Version 4 of the notification protocol can report one click as more than one
/// message — `NIN_SELECT` and `WM_LBUTTONUP` — and a double click as two ups.
/// Without this, a double click would open the panel and immediately close it.
const CLICK_GAP: Duration = Duration::from_millis(250);

/// How long to wait after the displays have changed before enumerating them.
///
/// The message arrives while the change is still settling: a monitor that has
/// just been plugged in is not in WMI's list for a moment, and enumerating then
/// reads the old set and never hears about the new one. Long enough for the
/// driver stack to finish, short enough that the panel is not visibly stale.
const DISPLAY_SETTLE: Duration = Duration::from_millis(700);

/// How long to wait before restoring levels after a wake.
///
/// The monitor's link comes back before the scaler behind it does, so an
/// enumeration at the instant of the wake can miss a display that is about to
/// answer. Slightly later than [`DISPLAY_SETTLE`] because a wake is the slower
/// of the two.
const WAKE_SETTLE: Duration = Duration::from_millis(1200);

fn main() -> ExitCode {
    // Per-monitor v2 before any window exists, so the popup is laid out in the
    // real pixels of the monitor it lands on and Windows does not bitmap-stretch
    // it on a 150% screen.
    // SAFETY: no window has been created on this thread yet, which is the only
    // condition the call has.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let Some(mut app) = App::start() else {
        eprintln!("klart-tray: could not start");
        return ExitCode::FAILURE;
    };

    if !app.run() {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// The icon, the window behind it, the displays, and the popup if one is open.
struct App {
    hwnd: HWND,
    /// The window Windows turns into a taskbar button if it is ever shown, which
    /// it is not. It exists to receive the icon's callbacks and the broadcasts
    /// that say a display, a theme or the shell has changed.
    driver: Driver,
    icon: *mut core::ffi::c_void,
    /// Whether [`App::icon`] came from [`icon::sun`] and must be destroyed, or
    /// is a shared system icon that must not be.
    icon_owned: bool,
    /// Whether the icon is currently in the notification area. False after
    /// Explorer has restarted and before it has been put back.
    icon_added: bool,
    nid: NOTIFYICONDATAW,
    panel: Option<Box<panel::Panel>>,
    /// Distinguishes the open panel from messages an earlier one posted.
    generation: u32,
    /// The id `TaskbarCreated` was registered under. Zero if registration
    /// failed, which is only possible on a system that cannot happen on.
    taskbar_created: u32,
    /// When the last click was acted on, for [`CLICK_GAP`].
    last_click: Option<Instant>,
    /// Whether the shell has told us this click is the second of a double.
    double_clicked: bool,
    /// When the open panel closed itself because it lost focus.
    ///
    /// Clicking the icon while the panel is open can reach here in either
    /// order: the panel's deactivation is posted, and the shell's callback is
    /// sent. If the deactivation lands first, the panel is already gone and the
    /// same click would open it again; this is what stops that.
    closed_at: Option<Instant>,
    /// When to enumerate the displays again, if the set is about to change.
    refresh_at: Option<Instant>,
    /// Whether a wake is waiting for its enumeration before the levels go back.
    wake_pending: bool,
    timer: usize,
}

impl App {
    /// Sets up the window, the icon and the displays.
    ///
    /// Returns [`None`] when the hidden window could not be created, which is
    /// the one failure with nothing left to run.
    fn start() -> Option<Box<Self>> {
        // SAFETY: no arguments; the module handle is the executable's own.
        let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if hinstance.is_null() {
            return None;
        }

        // The class is registered before the window, and the panel's before
        // anything tries to open one. A failure to register is reported by
        // `CreateWindowExW` as well, with the same reason.
        // SAFETY: both classes are fully initialised and their names live for
        // the calls.
        unsafe {
            register_tray_class(hinstance);
            panel::register(hinstance);
        }

        let driver = Driver::new(controls().unwrap_or_default());
        // Before the first panel is opened, so its levels are the ones the
        // displays are sitting at, and before anything is drawn.
        driver.restore();

        // SAFETY: called on the thread that will run the message loop, which is
        // where GDI objects and `CreateIconIndirect` belong.
        let (icon, icon_owned) = unsafe {
            let drawn = icon::sun();
            if drawn.is_null() {
                (LoadIconW(std::ptr::null_mut(), IDI_APPLICATION), false)
            } else {
                (drawn, true)
            }
        };

        let mut app = Box::new(Self {
            hwnd: std::ptr::null_mut(),
            driver,
            icon,
            icon_owned,
            icon_added: false,
            nid: NOTIFYICONDATAW::default(),
            panel: None,
            generation: 0,
            taskbar_created: 0,
            last_click: None,
            double_clicked: false,
            closed_at: None,
            refresh_at: None,
            wake_pending: false,
            timer: 0,
        });

        let class = wide(CLASS);
        let title = wide("klart");
        // SAFETY: the class is registered, the strings are NUL-terminated and
        // live for the call, and the box is not moved after this.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                (&mut *app as *mut Self).cast(),
            )
        };
        if hwnd.is_null() {
            // SAFETY: the box is still alive, and its `Drop` would find no
            // window and remove no icon.
            let code = unsafe { GetLastError() };
            eprintln!("klart-tray: could not create the message window (error {code})");
            return None;
        }
        app.hwnd = hwnd;

        // SAFETY: the message id is a registered name, and the pointer is a
        // NUL-terminated string.
        app.taskbar_created = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };

        app.nid = NOTIFYICONDATAW {
            cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONDATAW>()).unwrap_or(0),
            hWnd: hwnd,
            uID: ICON_ID,
            ..NOTIFYICONDATAW::default()
        };
        app.add_icon();

        // SAFETY: the window exists and the timer id is this window's to use.
        app.timer = unsafe { SetTimer(hwnd, TIMER_ID, TICK, None) };
        if app.timer == 0 {
            eprintln!("klart-tray: could not start the tick; levels may lag a drag");
        }

        Some(app)
    }

    /// Runs until the window is destroyed.
    ///
    /// Returns false only when the message loop itself failed, which is not
    /// something the agent can carry on from.
    fn run(&mut self) -> bool {
        // SAFETY: a standard message loop on the thread that created every
        // window it dispatches to.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            loop {
                let got = GetMessageW(&mut message, std::ptr::null_mut(), 0, 0);
                match got {
                    0 => return true,
                    -1 => {
                        eprintln!("klart-tray: the message loop failed");
                        return false;
                    }
                    _ => {}
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Puts the icon back into the notification area.
    ///
    /// Called at startup and again after Explorer has restarted, which removes
    /// every icon that was registered with it. A failure is retried on the next
    /// tick rather than reported once and forgotten: the shell may simply not be
    /// ready yet.
    fn add_icon(&mut self) {
        let mut data = self.nid;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = self.icon;
        copy_into(&mut data.szTip, "klart — brightness");

        // SAFETY: the structure is live for the call and describes this
        // window's icon.
        if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
            self.icon_added = false;
            return;
        }

        // Version 4 puts the cursor position in `wparam` and the message in
        // `lparam`, which is what makes a click on the icon land where it was
        // clicked rather than where the pointer is a moment later.
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: as above; the version is set after the icon exists, which is
        // what the protocol requires.
        unsafe {
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }

        self.nid = data;
        self.icon_added = true;
    }

    /// Takes the icon out of the notification area.
    fn remove_icon(&mut self) {
        if self.icon_added {
            // SAFETY: removes the identifier this window registered.
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &self.nid);
            }
            self.icon_added = false;
        }
    }

    /// Acts on the level a slider asked for.
    ///
    /// Reached from the panel through the raw pointer it holds, which is why it
    /// takes `&self`: nothing here changes the agent's own state, only what the
    /// driver has been asked for.
    fn level(&self, target: Target, percent: u8) {
        match target {
            Target::All => self.driver.request_all(percent),
            Target::Display(display) => self.driver.request(display, percent),
        }
    }

    /// Lands the last value of a drag, which the rate limiter may have held.
    fn flush(&self) {
        self.driver.flush();
    }

    /// Lands whatever a drag could not and gives the driver a turn.
    fn tick(&mut self) {
        self.driver.flush();
        self.driver.persist();
        self.driver.drive_restore();

        if !self.icon_added {
            self.add_icon();
        }

        if self.refresh_at.is_some_and(|at| Instant::now() >= at) {
            self.refresh_at = None;
            // Before the restore, so that a wake onto a different set of
            // monitors restores the set that is actually there.
            self.look_again();
            if self.wake_pending {
                self.wake_pending = false;
                self.driver.restore_after_wake();
            }
        }
    }

    /// Whether the event in the icon's callback should be treated as a click.
    ///
    /// Both buttons open the panel. There is no separate right-click menu to
    /// put one in: the panel is the menu.
    fn on_tray(&mut self, event: u32) {
        match event {
            // A double click is not a second click: it selects the icon once.
            // The flag swallows the up that follows it, because the first up
            // already did the work.
            WM_LBUTTONDBLCLK => self.double_clicked = true,
            WM_LBUTTONUP | WM_RBUTTONUP | WM_CONTEXTMENU | NIN_SELECT => {
                if self.double_clicked {
                    self.double_clicked = false;
                    return;
                }
                self.click();
            }
            _ => {}
        }
    }

    fn click(&mut self) {
        let now = Instant::now();
        if self
            .last_click
            .is_some_and(|last| now.duration_since(last) < CLICK_GAP)
        {
            return;
        }
        self.last_click = Some(now);

        if self.panel.is_none()
            && self
                .closed_at
                .is_some_and(|closed| now.duration_since(closed) < CLICK_GAP)
        {
            // The click that is arriving closed the panel a moment ago by
            // taking its focus; it was aimed at the icon, not at reopening.
            return;
        }

        self.toggle_panel();
    }

    fn toggle_panel(&mut self) {
        if self.panel.is_some() {
            self.panel = None;
        } else {
            self.open_panel();
        }
    }

    fn open_panel(&mut self) {
        self.closed_at = None;

        let Some(anchor) = self.icon_rect() else {
            eprintln!("klart-tray: could not find the icon to open next to");
            return;
        };

        let sliders = self.sliders();
        let commands = self.commands();

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        // SAFETY: the agent is boxed in `main` and outlives every panel it
        // opens; the panel clears its own user data before it is destroyed.
        self.panel = panel::show(
            &mut *self as *mut Self,
            self.hwnd,
            generation,
            anchor,
            sliders,
            commands,
        );

        if self.panel.is_none() {
            eprintln!("klart-tray: the brightness panel could not be created");
        }
    }

    /// One slider per display, and a combined one when there is more than one
    /// display to combine.
    fn sliders(&self) -> Vec<SliderSpec> {
        let controls = self.driver.controls();
        let mut sliders = Vec::with_capacity(controls.len() + 1);

        if controls.len() > 1 {
            sliders.push(SliderSpec {
                target: Target::All,
                caption: "All displays".to_owned(),
                // Recorded here as well as displayed, so a relative drag has
                // the levels to measure from.
                percent: self.driver.open_combined(),
                transient: false,
            });
        }

        for (display, control) in controls.iter().enumerate() {
            sliders.push(SliderSpec {
                target: Target::Display(display),
                caption: control.name().to_owned(),
                percent: control.get().map_or(0, |level| level.percent_rounded()),
                transient: !control.persists(),
            });
        }

        sliders
    }

    /// The rows underneath the sliders.
    fn commands(&self) -> Vec<CommandSpec> {
        let mut commands = Vec::with_capacity(4);

        // Only when there is a combined slider for it to affect. A setting whose
        // effect is not on screen is a setting nobody can connect to anything.
        if self.driver.controls().len() > 1 {
            commands.push(CommandSpec {
                id: CMD_KEEP_OFFSETS,
                // Named for what it does rather than for which of the two modes
                // it is: most people have never thought about absolute against
                // relative and do not need to in order to know whether they
                // want their displays to stay as they set them.
                text: "Keep displays' relative brightness".to_owned(),
                checked: Some(self.driver.combined() == Combined::Relative),
                enabled: true,
            });
        }

        commands.push(login_item_command());

        commands.push(CommandSpec {
            id: CMD_REFRESH,
            text: "Look for displays again".to_owned(),
            checked: None,
            enabled: true,
        });
        commands.push(CommandSpec {
            id: CMD_QUIT,
            text: "Quit klart".to_owned(),
            checked: None,
            enabled: true,
        });

        commands
    }

    /// Runs a command the panel posted.
    fn command(&mut self, id: u32) {
        match id {
            CMD_KEEP_OFFSETS => {
                let wanted = self.driver.combined() != Combined::Relative;
                self.driver.set_combined(if wanted {
                    Combined::Relative
                } else {
                    Combined::Absolute
                });
            }
            CMD_LOGIN_ITEM => {
                // The next panel is built from what the system says, not from
                // what was asked for.
                let wanted = !matches!(klart_core::login_item(), LoginItem::Enabled);
                if let Err(problem) = klart_core::set_login_item(wanted) {
                    eprintln!("klart-tray: {problem}");
                }
            }
            CMD_REFRESH => self.look_again(),
            CMD_QUIT => {
                // SAFETY: ends the message loop; the agent's `Drop` removes the
                // icon and destroys the window on the way out.
                unsafe { PostQuitMessage(0) };
            }
            _ => {}
        }

        self.panel = None;
    }

    /// Enumerates the displays again.
    fn look_again(&mut self) {
        match controls() {
            Ok(found) => self.driver.replace(found),
            Err(problem) => eprintln!("klart-tray: {problem}"),
        }

        // A display that has just appeared has whatever level it powered on
        // with, which for a gamma-dimmed one is none of this process's doing.
        self.driver.restore();
    }

    /// Schedules an enumeration once the display stack has settled.
    fn schedule_refresh(&mut self, after: Duration) {
        let at = Instant::now() + after;
        self.refresh_at = Some(match self.refresh_at {
            Some(earlier) if earlier <= at => earlier,
            _ => at,
        });
    }

    /// The rectangle the icon occupies, for placing the panel beside it.
    ///
    /// The shell knows; the fallback is the pointer, which is where the click
    /// that opened the panel was. If neither answers, there is nowhere sensible
    /// to put a panel.
    fn icon_rect(&self) -> Option<RECT> {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: u32::try_from(std::mem::size_of::<NOTIFYICONIDENTIFIER>()).unwrap_or(0),
            hWnd: self.hwnd,
            uID: ICON_ID,
            guidItem: unsafe { std::mem::zeroed() },
        };
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        // SAFETY: both the identifier and the rectangle are live, and the
        // rectangle is written on success.
        if unsafe { Shell_NotifyIconGetRect(&identifier, &mut rect) } == 0 {
            return Some(rect);
        }

        let mut point: POINT = unsafe { std::mem::zeroed() };
        // SAFETY: `point` is a live out parameter.
        if unsafe { GetCursorPos(&mut point) } != 0 {
            // A small square around the pointer: enough for `place` to align
            // the panel against and to know which monitor it is on.
            return Some(RECT {
                left: point.x - 8,
                top: point.y - 8,
                right: point.x + 8,
                bottom: point.y + 8,
            });
        }

        None
    }

    fn on_message(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TRAY => {
                // Version 4: the low word of `lparam` is the event, and `wparam`
                // carries the cursor position rather than the icon's id.
                self.on_tray((lparam as usize & 0xFFFF) as u32);
                0
            }
            WM_PANEL_CLOSE => {
                if wparam as u32 == self.generation && self.panel.is_some() {
                    self.panel = None;
                    self.closed_at = Some(Instant::now());
                }
                0
            }
            WM_PANEL_COMMAND => {
                if lparam as u32 == self.generation {
                    self.command(wparam as u32);
                }
                0
            }
            WM_TIMER => {
                if wparam == TIMER_ID {
                    self.tick();
                }
                0
            }
            WM_POWERBROADCAST => {
                if matches!(wparam as u32, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) {
                    self.woke();
                }
                1
            }
            WM_DISPLAYCHANGE => {
                self.displays_changed();
                0
            }
            WM_DEVICECHANGE => {
                // Monitor arrival and removal already arrives as
                // `WM_DISPLAYCHANGE`; this catches a device tree change that
                // does not, and costs one enumeration.
                if wparam as u32 == DBT_DEVNODES_CHANGED {
                    self.displays_changed();
                }
                0
            }
            WM_SETTINGCHANGE => {
                if immersive_colour_changed(lparam) {
                    // The palette is read when the panel is built, so closing
                    // an open one is enough: the next opens in the new mode.
                    self.panel = None;
                }
                0
            }
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION => {
                if wparam != 0 {
                    // SAFETY: ends the message loop; `Drop` does the cleanup
                    // that a shutdown leaves time for.
                    unsafe { PostQuitMessage(0) };
                }
                0
            }
            WM_CLOSE => {
                // SAFETY: destroys the window this message was sent to; its
                // `WM_DESTROY` ends the loop.
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                self.hwnd = std::ptr::null_mut();
                // SAFETY: posted from the window that is being destroyed, which
                // is the documented place for it.
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => {
                // SAFETY: forwards the message this window was sent.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
    }

    /// Waking is two things that have to happen in order: the displays are
    /// enumerated once they are ready, and then the levels go back.
    ///
    /// The restore is not started here because a monitor that comes back a
    /// moment after the machine does would be restored against the set from
    /// before the sleep, and a display that has gone would be asked forever.
    fn woke(&mut self) {
        self.panel = None;
        self.wake_pending = true;
        self.schedule_refresh(WAKE_SETTLE);
    }

    fn displays_changed(&mut self) {
        self.panel = None;
        self.schedule_refresh(DISPLAY_SETTLE);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // First, so the popup's window is gone before the owner it posts to.
        self.panel = None;
        self.remove_icon();

        // SAFETY: the icon came from `CreateIconIndirect` exactly when
        // `icon_owned` says so, and is destroyed once.
        unsafe {
            if self.icon_owned && !self.icon.is_null() {
                DestroyIcon(self.icon);
            }
        }

        // SAFETY: the timer and window belong to this thread's message loop,
        // and both are released once.
        unsafe {
            if !self.hwnd.is_null() {
                if self.timer != 0 {
                    KillTimer(self.hwnd, TIMER_ID);
                }
                DestroyWindow(self.hwnd);
            }
        }
    }
}

/// The command that decides whether the agent comes back after a restart.
///
/// Worth a row rather than leaving it to Task Manager: a display dimmed with a
/// gamma ramp is back at full brightness once this process exits, so an agent
/// that does not start at login means that display is full brightness after
/// every restart.
fn login_item_command() -> CommandSpec {
    let state = klart_core::login_item();

    match state {
        LoginItem::Enabled | LoginItem::Disabled => CommandSpec {
            id: CMD_LOGIN_ITEM,
            text: "Start at login".to_owned(),
            checked: Some(state == LoginItem::Enabled),
            enabled: true,
        },
        LoginItem::AwaitingApproval => CommandSpec {
            id: CMD_LOGIN_ITEM,
            text: "Start at login — waiting for approval".to_owned(),
            checked: None,
            enabled: false,
        },
        LoginItem::Unavailable(reason) => CommandSpec {
            id: CMD_LOGIN_ITEM,
            text: format!("Start at login — {reason}"),
            checked: None,
            enabled: false,
        },
    }
}

/// Whether this `WM_SETTINGCHANGE` is the one that means light or dark mode.
///
/// The string is only valid when `lparam` is not zero, and comparing it is the
/// documented way to tell this setting from the many others that share the
/// message.
fn immersive_colour_changed(lparam: LPARAM) -> bool {
    if lparam == 0 {
        return false;
    }

    let expected = wide("ImmersiveColorSet");
    let actual = lparam as *const u16;

    for (index, wanted) in expected.iter().enumerate() {
        // SAFETY: the system hands a NUL-terminated string here, which is how
        // the message documents it, and reading stops at its terminator.
        let found = unsafe { *actual.add(index) };
        if found != *wanted {
            return false;
        }
        if found == 0 {
            break;
        }
    }

    true
}

/// Registers the hidden window's class.
///
/// # Safety
///
/// Called once, on the thread that runs the message loop.
unsafe fn register_tray_class(hinstance: *mut core::ffi::c_void) {
    let name = wide(CLASS);
    // SAFETY: the cursor is a stock resource.
    let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };

    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(tray_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: cursor,
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: name.as_ptr(),
    };

    // SAFETY: `class` is fully initialised and `name` is live for the call.
    unsafe {
        RegisterClassW(&class);
    }
}

/// The hidden window's procedure.
///
/// # Safety
///
/// Called by Windows with a live window handle.
unsafe extern "system" fn tray_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: `lparam` is this window's `CREATESTRUCTW`, and
        // `lpCreateParams` is the boxed agent handed to `CreateWindowExW`.
        unsafe {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return 1;
    }

    // SAFETY: set in `WM_NCCREATE` and cleared in `WM_NCDESTROY` below.
    let app = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App };

    if !app.is_null() {
        // SAFETY: the pointer is this window's own agent, which outlives the
        // window because `main` drops it after the loop ends.
        let this = unsafe { &mut *app };
        if this.taskbar_created != 0 && msg == this.taskbar_created {
            this.icon_added = false;
            this.add_icon();
            return 0;
        }
        return this.on_message(hwnd, msg, wparam, lparam);
    }

    // SAFETY: the default handler is the right answer for a window with no
    // agent behind it.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The hidden window's class name.
const CLASS: &str = "KlartTrayWindow";
