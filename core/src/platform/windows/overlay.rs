//! A black, click-through window over a display in HDR.
//!
//! The gamma ramp is not defined while Windows is in HDR. Rather than write a
//! ramp whose effect Microsoft does not promise, this dims the desktop the way
//! a screen dimmer does: one topmost layered window per display, painted black,
//! with the alpha channel carrying the level.
//!
//! It is a fallback rather than a peer of the backlight. It can only darken
//! what is composited into the desktop, so an exclusive-fullscreen game is
//! outside it, and the alpha is applied after HDR tone mapping. What it buys is
//! a usable lower range on a display that otherwise has none, without touching
//! a colour pipeline that is already in a mode klart does not understand.
//!
//! The windows live on one thread with one message loop, not on the thread that
//! happens to open a control. The tray has two sets of controls — one for
//! reading on the message loop and one for writing off it — and a window per
//! control would be two overlays fighting over the same display. A process-wide
//! manager keyed by display collapses them into one.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HDC, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, MSG, PostMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, TranslateMessage, WM_APP,
    WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCCREATE, WM_NCHITTEST, WM_PAINT,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

use crate::Brightness;
use crate::backend::Backend;
use crate::display::Display;
use crate::error::{Error, Result};

use super::monitors::to_wide;

/// The name used in errors and in machine-readable output.
pub(crate) const NAME: &str = "overlay";

/// The same floor as the gamma ramp: the dimmest overlay still leaves a fifth
/// of the picture, so the panel can be found and the level raised again.
const FLOOR: f32 = 0.2;

/// The message that wakes the overlay thread for a queued command.
const WM_OVERLAY: u32 = WM_APP + 0x41;

/// The class every overlay window is an instance of.
const CLASS: &str = "KlartDimOverlay";

/// The class of the hidden window that receives display-change broadcasts.
///
/// Kept separate from [`CLASS`] because a broadcast goes to every top-level
/// window, and an overlay that posted a command to itself would not be heard by
/// the manager. Only this one has the procedure that forwards the broadcast.
const HOST_CLASS: &str = "KlartDimOverlayHost";

/// A command for the overlay thread.
enum Command {
    /// Make sure one display has a window at this rectangle.
    Ensure {
        key: String,
        rect: RECT,
        reply: Sender<std::result::Result<(), String>>,
    },
    /// Show one display at this level.
    Set {
        key: String,
        percent: u8,
        alpha: u8,
        reply: Sender<std::result::Result<(), String>>,
    },
    /// Stop holding one display.
    Clear {
        key: String,
        reply: Sender<std::result::Result<(), String>>,
    },
    /// Stop holding every display.
    ClearAll,
    /// Keep visible overlays below this window, usually the tray panel.
    ///
    /// Zero puts them back at the top of the topmost band. Without this the
    /// panel that is being dragged would be dimmed by the overlay it is
    /// controlling, which makes the slider hard to see at exactly the moment it
    /// matters.
    Anchor { hwnd: isize },
}

/// The overlay thread's state, reachable from any thread.
struct Manager {
    /// The hidden top-level window that receives commands.
    hwnd: isize,
    queue: Arc<Mutex<VecDeque<Command>>>,
    /// The level each display is currently showing, for `Backend::get`.
    levels: Arc<Mutex<HashMap<String, u8>>>,
}

/// Started on first use and never stopped; the process owns it.
static MANAGER: OnceLock<Option<Manager>> = OnceLock::new();

/// The panel the overlays should stay below, when one is open.
///
/// Kept outside the manager so that a panel opened before the first HDR overlay
/// still gets to set the order when that overlay is created.
static ANCHOR: AtomicIsize = AtomicIsize::new(0);

/// A display dimmed with a layered black window.
pub(crate) struct Overlay {
    key: String,
    levels: Arc<Mutex<HashMap<String, u8>>>,
}

impl Overlay {
    /// Binds to one display and makes sure its window exists.
    ///
    /// # Errors
    ///
    /// When the overlay thread cannot be started or a window for this display
    /// cannot be created. Failing here is better than pretending to dim.
    pub(crate) fn open(display: &Display) -> Result<Self> {
        let Some(manager) = manager() else {
            return Err(Error::MechanismUnavailable { mechanism: NAME });
        };

        let bounds = display.bounds();
        let rect = RECT {
            left: bounds.x,
            top: bounds.y,
            right: bounds
                .x
                .saturating_add(i32::try_from(bounds.width).unwrap_or(i32::MAX)),
            bottom: bounds
                .y
                .saturating_add(i32::try_from(bounds.height).unwrap_or(i32::MAX)),
        };

        let key = display.key().as_str().to_owned();
        let (reply, answer) = mpsc::channel();
        if !post(Command::Ensure {
            key: key.clone(),
            rect,
            reply,
        }) {
            return Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "overlay thread",
                code: -1,
            });
        }

        match answer.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                key,
                levels: Arc::clone(&manager.levels),
            }),
            Ok(Err(_)) | Err(_) => Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "CreateWindowExW",
                code: -1,
            }),
        }
    }

    /// Sends a command and waits for the overlay thread to finish it.
    fn ask(
        &self,
        command: Command,
        answer: mpsc::Receiver<std::result::Result<(), String>>,
    ) -> Result<()> {
        if !post(command) {
            return Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "overlay thread",
                code: -1,
            });
        }

        match answer.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(Error::MechanismFailed {
                mechanism: NAME,
                call: "layered window",
                code: -1,
            }),
        }
    }
}

impl Backend for Overlay {
    fn name(&self) -> &'static str {
        NAME
    }

    fn persists(&self) -> bool {
        // The window belongs to this process. Quitting, or crashing, removes it
        // rather than leaving the desktop dim.
        false
    }

    fn get(&self) -> Result<Brightness> {
        let levels = self
            .levels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(Brightness::from_percent(f32::from(
            levels.get(&self.key).copied().unwrap_or(100),
        )))
    }

    fn set(&self, level: Brightness) -> Result<()> {
        let percent = level.percent_rounded();
        let alpha = alpha_for(level);
        let (reply, answer) = mpsc::channel();

        self.ask(
            Command::Set {
                key: self.key.clone(),
                percent,
                alpha,
                reply,
            },
            answer,
        )
    }

    fn release(&self) -> Result<()> {
        let (reply, answer) = mpsc::channel();
        self.ask(
            Command::Clear {
                key: self.key.clone(),
                reply,
            },
            answer,
        )
    }
}

/// The overlay thread, started if this is the first call.
fn manager() -> Option<&'static Manager> {
    MANAGER.get_or_init(start).as_ref()
}

/// The overlay thread, only if something has already started it.
fn existing_manager() -> Option<&'static Manager> {
    MANAGER.get().and_then(Option::as_ref)
}

/// Starts the thread and waits until it has a window to receive commands on.
fn start() -> Option<Manager> {
    let queue = Arc::new(Mutex::new(VecDeque::new()));
    let levels = Arc::new(Mutex::new(HashMap::new()));
    let (ready, started) = mpsc::channel();

    let thread_queue = Arc::clone(&queue);
    let thread_levels = Arc::clone(&levels);
    std::thread::Builder::new()
        .name("klart-overlay".to_owned())
        .spawn(move || run(thread_queue, thread_levels, ready))
        .ok()?;

    let hwnd = started.recv_timeout(Duration::from_secs(2)).ok()??;
    Some(Manager {
        hwnd,
        queue,
        levels,
    })
}

/// Queues a command and wakes the thread that owns the windows.
fn post(command: Command) -> bool {
    let Some(manager) = manager() else {
        return false;
    };
    post_to(manager, command)
}

/// Queues a command on a manager that already exists.
fn post_to(manager: &Manager, command: Command) -> bool {
    manager
        .queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push_back(command);

    // SAFETY: the handle is the hidden window this manager created on its own
    // thread, which outlives the command.
    unsafe { PostMessageW(manager.hwnd as HWND, WM_OVERLAY, 0, 0) != 0 }
}

/// Hides every overlay, for an agent that is about to exit.
///
/// Does not start the overlay thread: an agent that never used an overlay has
/// nothing to hide, and quitting should not create a thread just to tell it so.
pub(crate) fn clear_all() {
    if let Some(manager) = existing_manager() {
        let _ = post_to(manager, Command::ClearAll);
    }
}

/// Keeps visible overlays below a window, or back at the top for zero.
///
/// The anchor is recorded even before the overlay thread exists, so a panel
/// opened first is still above the first overlay that appears.
pub(crate) fn anchor(hwnd: isize) {
    ANCHOR.store(hwnd, Ordering::Relaxed);
    if let Some(manager) = existing_manager() {
        let _ = post_to(manager, Command::Anchor { hwnd });
    }
}

/// The overlay thread's loop.
fn run(
    queue: Arc<Mutex<VecDeque<Command>>>,
    levels: Arc<Mutex<HashMap<String, u8>>>,
    ready: Sender<Option<isize>>,
) {
    // SAFETY: no arguments; the module handle is this process's own.
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    if hinstance.is_null() {
        let _ = ready.send(None);
        return;
    }

    let name = to_wide(CLASS);
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(overlay_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: name.as_ptr(),
    };

    // SAFETY: `class` is fully initialised and `name` is live for the call.
    let registered = unsafe { RegisterClassW(&class) };
    if registered == 0 {
        // A second start after a failed first one sees the class already there.
        // SAFETY: no arguments; the error is read immediately.
        let error = unsafe { GetLastError() };
        if error != 1410 {
            let _ = ready.send(None);
            return;
        }
    }

    let host_name = to_wide(HOST_CLASS);
    let host_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(host_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: host_name.as_ptr(),
    };

    // SAFETY: as above; the host class uses its own name and procedure.
    let registered = unsafe { RegisterClassW(&host_class) };
    if registered == 0 {
        // SAFETY: no arguments; the error is read immediately.
        let error = unsafe { GetLastError() };
        if error != 1410 {
            let _ = ready.send(None);
            return;
        }
    }

    let title = to_wide("klart");
    // SAFETY: both classes are registered, the strings are live, and a hidden
    // top-level window is wanted so that display-change broadcasts reach it.
    let hidden = unsafe {
        CreateWindowExW(
            0,
            host_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hidden.is_null() {
        let _ = ready.send(None);
        return;
    }

    let _ = ready.send(Some(hidden as isize));

    let mut windows: HashMap<String, OverlayWindow> = HashMap::new();
    let mut anchor = ANCHOR.load(Ordering::Relaxed);

    // SAFETY: a standard message loop on the thread that created every window
    // it dispatches to.
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        loop {
            let got = GetMessageW(&mut message, std::ptr::null_mut(), 0, 0);
            if got <= 0 {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);

            if message.message == WM_OVERLAY {
                if message.wParam == WM_DISPLAYCHANGE as WPARAM {
                    // A monitor moved, changed resolution, or was plugged in.
                    // The rectangles the windows were built against are no
                    // longer trustworthy, so take them down; the agent
                    // re-enumerates and sets the held levels again a moment
                    // later.
                    for window in windows.values_mut() {
                        hide(window.hwnd);
                    }
                } else {
                    drain(&queue, &levels, &mut windows, &mut anchor);
                }
            }
        }

        for window in windows.values() {
            destroy_window(window.hwnd);
        }
        destroy_window(hidden);
    }
}

/// Applies every command that has arrived.
fn drain(
    queue: &Arc<Mutex<VecDeque<Command>>>,
    levels: &Arc<Mutex<HashMap<String, u8>>>,
    windows: &mut HashMap<String, OverlayWindow>,
    anchor: &mut isize,
) {
    loop {
        let command = queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front();
        let Some(command) = command else {
            return;
        };

        match command {
            Command::Ensure { key, rect, reply } => {
                let outcome = ensure(&key, rect, windows);
                let _ = reply.send(outcome);
            }
            Command::Set {
                key,
                percent,
                alpha,
                reply,
            } => {
                let outcome = match windows.get(&key) {
                    Some(window) => show(window.hwnd, alpha, *anchor),
                    None => Err("no window for this display".to_owned()),
                };

                // Only remember a level that is actually on the screen. A
                // failed `show` must not make `get` report a dimming that never
                // happened.
                if outcome.is_ok() {
                    levels
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(key, percent);
                }
                let _ = reply.send(outcome);
            }
            Command::Clear { key, reply } => {
                levels
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&key);
                let outcome = match windows.get(&key) {
                    Some(window) => {
                        hide(window.hwnd);
                        Ok(())
                    }
                    None => Ok(()),
                };
                let _ = reply.send(outcome);
            }
            Command::ClearAll => {
                levels
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clear();
                for window in windows.values() {
                    hide(window.hwnd);
                }
            }
            Command::Anchor { hwnd } => {
                *anchor = hwnd;
                for window in windows.values() {
                    // SAFETY: each handle is this thread's window.
                    unsafe {
                        place(window.hwnd, *anchor);
                    }
                }
            }
        }
    }
}

/// One window the overlay thread owns.
struct OverlayWindow {
    hwnd: HWND,
}

/// Creates a window for a display, or moves the one it already has.
fn ensure(
    key: &str,
    rect: RECT,
    windows: &mut HashMap<String, OverlayWindow>,
) -> std::result::Result<(), String> {
    if let Some(window) = windows.get(key) {
        // SAFETY: the handle belongs to this thread. No z-order change here;
        // `place` is what decides whether the panel stays above it.
        let moved = unsafe {
            SetWindowPos(
                window.hwnd,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                rect.right.saturating_sub(rect.left),
                rect.bottom.saturating_sub(rect.top),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if moved == 0 {
            return Err("SetWindowPos failed".to_owned());
        }
        return Ok(());
    }

    // SAFETY: the class is registered on this thread, and the wide title is
    // live for the call.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            to_wide(CLASS).as_ptr(),
            to_wide("klart dim").as_ptr(),
            WS_POPUP,
            rect.left,
            rect.top,
            rect.right.saturating_sub(rect.left),
            rect.bottom.saturating_sub(rect.top),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("CreateWindowExW failed".to_owned());
    }

    // SAFETY: the handle is live and the window was created layered.
    let layered = unsafe { SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA) };
    if layered == 0 {
        // SAFETY: the window was just created and is not used anywhere else.
        unsafe { destroy_window(hwnd) };
        return Err("SetLayeredWindowAttributes failed".to_owned());
    }

    windows.insert(key.to_owned(), OverlayWindow { hwnd });
    Ok(())
}

/// Hides one overlay.
fn hide(hwnd: HWND) {
    // SAFETY: the handle is a window owned by the calling thread.
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
}

/// Shows or hides one overlay at an alpha.
fn show(hwnd: HWND, alpha: u8, anchor: isize) -> std::result::Result<(), String> {
    if alpha == 0 {
        // SAFETY: the handle belongs to this thread.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
        return Ok(());
    }

    // SAFETY: the handle is live and the window is layered.
    let layered = unsafe { SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA) };
    if layered == 0 {
        return Err("SetLayeredWindowAttributes failed".to_owned());
    }

    // SAFETY: the handle belongs to this thread.
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        place(hwnd, anchor);
    }
    Ok(())
}

/// Puts an overlay below the panel when one is open, or at the top otherwise.
///
/// # Safety
///
/// `hwnd` must be a window owned by the calling thread, and `anchor` either
/// zero or a live topmost window.
unsafe fn place(hwnd: HWND, anchor: isize) {
    let insert_after = if anchor == 0 {
        HWND_TOPMOST
    } else {
        anchor as HWND
    };

    // SAFETY: both handles are live for the call; the flags leave position and
    // size alone and do not activate the overlay.
    unsafe {
        SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// The alpha that leaves a given fraction of the picture.
fn alpha_for(level: Brightness) -> u8 {
    let visible = FLOOR + (1.0 - FLOOR) * level.fraction();
    ((1.0 - visible) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The window procedure for the hidden manager window.
///
/// It has one job: turn a broadcast into a posted message, because the manager
/// only sees what it retrieves from its own queue.
///
/// # Safety
///
/// Called by Windows with a live window handle.
unsafe extern "system" fn host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_DISPLAYCHANGE {
        // SAFETY: the handle is this window, and the posted message carries no
        // borrowed data.
        unsafe {
            PostMessageW(hwnd, WM_OVERLAY, WM_DISPLAYCHANGE as WPARAM, 0);
        }
        return 0;
    }

    // SAFETY: forwards the message this window was sent.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The window procedure for an overlay.
///
/// # Safety
///
/// Called by Windows with a live window handle.
unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            // SAFETY: the create data is live for this message. Nothing needs
            // to be stored, but returning one lets creation continue.
            let _ = unsafe { &*(lparam as *const CREATESTRUCTW) };
            1
        }
        WM_PAINT => {
            // SAFETY: this is a paint message for this window, and the brush is
            // released after the fill.
            unsafe {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let hdc: HDC = BeginPaint(hwnd, &mut paint);
                if !hdc.is_null() {
                    let mut client: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut client);
                    let brush = CreateSolidBrush(0 as COLORREF);
                    FillRect(hdc, &client, brush);
                    DeleteObject(brush);
                    EndPaint(hwnd, &paint);
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => {
            // SAFETY: forwards the message this window was sent.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// Destroys a window without needing the `windows-sys` import at every call.
///
/// # Safety
///
/// `hwnd` must be a window this thread owns and has not destroyed.
unsafe fn destroy_window(hwnd: HWND) {
    // SAFETY: the caller promises the handle is live and owned here.
    unsafe { DestroyWindow(hwnd) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dimmest_overlay_still_leaves_a_fifth_of_the_picture() {
        assert_eq!(alpha_for(Brightness::MIN), 204);
        assert!(alpha_for(Brightness::MIN) < 255, "not a black screen");
    }

    #[test]
    fn the_brightest_overlay_is_invisible() {
        assert_eq!(alpha_for(Brightness::MAX), 0);
    }

    #[test]
    fn alpha_rises_as_the_level_falls() {
        let mut previous = 0;
        for whole in (0..=100).rev() {
            let alpha = alpha_for(Brightness::from_percent(whole as f32));
            assert!(alpha >= previous, "alpha at {whole}% was {alpha}");
            previous = alpha;
        }
    }

    /// Ignored because it covers a real display for a moment.
    ///
    /// This is the window path the arithmetic above cannot prove: a layered
    /// window created on the manager thread, shown through the command queue,
    /// and hidden again by `release`.
    #[test]
    #[ignore = "covers a real display"]
    fn a_real_overlay_can_be_shown_and_released() {
        let controls = crate::controls().expect("displays should open");
        let control = controls.first().expect("a display should be attached");
        let overlay =
            Overlay::open(control.display()).expect("an overlay window should be created");

        overlay
            .set(Brightness::from_percent(50.0))
            .expect("the overlay should show");
        assert_eq!(
            overlay
                .get()
                .expect("the level should read")
                .percent_rounded(),
            50
        );

        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, FindWindowW, GW_HWNDNEXT, GetWindow, IsWindowVisible,
            SendMessageW, WM_DISPLAYCHANGE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        };

        let overlay_name = to_wide(CLASS);
        let host_name = to_wide(HOST_CLASS);
        // SAFETY: both names are NUL terminated and live for the calls.
        let window = unsafe { FindWindowW(overlay_name.as_ptr(), std::ptr::null()) };
        let host = unsafe { FindWindowW(host_name.as_ptr(), std::ptr::null()) };
        assert!(!window.is_null() && !host.is_null());
        assert_ne!(unsafe { IsWindowVisible(window) }, 0);

        // A display change is a broadcast, and the manager has to hear it as a
        // posted command. This is what proves the host procedure is in the
        // path; the level map deliberately survives so the level can be
        // reapplied once the new rectangles are known.
        unsafe {
            SendMessageW(host, WM_DISPLAYCHANGE, 0, 0);
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(unsafe { IsWindowVisible(window) }, 0);

        // Show it again, then put a topmost stand-in for the tray panel above
        // it. The overlay has to be the very next window below that anchor.
        overlay
            .set(Brightness::from_percent(50.0))
            .expect("the overlay should show again");
        let anchor_class = to_wide("STATIC");
        let anchor_title = to_wide("klart overlay anchor test");
        // SAFETY: the predefined class is registered, the strings are live, and
        // the window is destroyed before the test returns.
        let anchor = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                anchor_class.as_ptr(),
                anchor_title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(
            !anchor.is_null(),
            "a topmost anchor window should be created"
        );

        super::anchor(anchor as isize);
        std::thread::sleep(Duration::from_millis(100));
        // SAFETY: both handles are live; `GetWindow` neither takes nor retains
        // ownership.
        assert_eq!(
            unsafe { GetWindow(anchor, GW_HWNDNEXT) },
            window,
            "the overlay should sit directly below the anchor"
        );

        super::anchor(0);
        // SAFETY: the anchor belongs to this test and is destroyed once.
        unsafe {
            DestroyWindow(anchor);
        }

        overlay.release().expect("the overlay should be released");
        assert_eq!(
            overlay
                .get()
                .expect("the level should read")
                .percent_rounded(),
            100
        );
    }
}
