//! A brief on-screen brightness level.
//!
//! macOS shows its own overlay when the brightness keys are pressed. `keys.rs`
//! swallows that along with the key, so without this the only sign a key did
//! anything is the screen changing — fine on a panel that visibly dims, thin on
//! one near the top of its range. This puts klart's own overlay back: a level
//! that appears on a press and fades away a beat later.
//!
//! It is a single borderless window, built once and reused. Rather than a timer,
//! it hides itself cooperatively: a press books a deadline, and the pump — which
//! is already looping — shortens its wait while the overlay is up and calls
//! [`tick`] to take it down when the time comes. That keeps all the timing on the
//! one thread the rest of the crate runs on, with nothing to synchronise.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSFont, NSLevelIndicator, NSLevelIndicatorStyle, NSScreen,
    NSTextAlignment, NSTextField, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

/// The overlay's size, in points.
const WIDTH: f64 = 200.0;
const HEIGHT: f64 = 78.0;

/// How round the corners are, matching the system overlay closely enough.
const CORNER: f64 = 18.0;

/// How long the overlay stays up after the last press.
const SHOW_FOR: Duration = Duration::from_millis(1200);

/// How far up from the bottom of the screen it sits, as a fraction of height.
/// A little above the bottom edge, where the system overlay also sits.
const FROM_BOTTOM: f64 = 0.12;

/// The window level that floats above ordinary windows and the menu bar.
///
/// `NSScreenSaverWindowLevel`'s value. Spelled as the number because the helper
/// that computes it from a key is not among the bindings, and the overlay needs
/// to sit above full-screen apps as the system one does.
const FLOATING_LEVEL: isize = 1000;

// The one overlay, built on first use. Main-thread only, like the rest of the
// tray, so a thread-local is all the sharing it needs.
thread_local! {
    static HUD: RefCell<Option<Hud>> = const { RefCell::new(None) };
}

/// Shows the level, or refreshes it if already up.
///
/// A no-op off the main thread, which cannot happen from where this is called
/// but is cheaper to check than to prove at every call site.
pub fn show(level: u8) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    HUD.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.get_or_insert_with(|| Hud::new(mtm)).show(level, mtm);
    });
}

/// Takes the overlay down if its time is up. Called by the pump each pass.
pub fn tick() {
    if MainThreadMarker::new().is_none() {
        return;
    }
    HUD.with(|slot| {
        if let Some(hud) = slot.borrow().as_ref() {
            hud.tick();
        }
    });
}

/// Seconds until the overlay should hide, so the pump can wait exactly that long
/// rather than its full idle spell. [`None`] when nothing is showing.
pub fn seconds_until_hide() -> Option<f64> {
    MainThreadMarker::new()?;
    HUD.with(|slot| slot.borrow().as_ref().and_then(Hud::remaining))
}

/// The overlay's window and the two things drawn in it.
struct Hud {
    window: Retained<NSWindow>,
    percent: Retained<NSTextField>,
    bar: Retained<NSLevelIndicator>,
    /// When to hide, or [`None`] when already hidden.
    hide_at: Cell<Option<Instant>>,
}

impl Hud {
    fn new(mtm: MainThreadMarker) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));

        // SAFETY: standard window construction on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setLevel(FLOATING_LEVEL);
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(true);
        // SAFETY: the window is owned by this struct, not by AppKit's close
        // machinery, so it must not be freed when ordered out.
        unsafe { window.setReleasedWhenClosed(false) };
        // On every space and every full-screen app, so a brightness change shows
        // wherever the person is looking, and stationary so it does not slide
        // with a space switch.
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setWantsLayer(true);
        if let Some(layer) = effect.layer() {
            layer.setCornerRadius(CORNER);
            layer.setMasksToBounds(true);
        }

        let percent = Self::percent_label(mtm);
        let bar = Self::level_bar(mtm);
        effect.addSubview(&percent);
        effect.addSubview(&bar);
        window.setContentView(Some(&effect));

        Self {
            window,
            percent,
            bar,
            hide_at: Cell::new(None),
        }
    }

    /// The percentage, large and centred near the top.
    fn percent_label(mtm: MainThreadMarker) -> Retained<NSTextField> {
        let label = NSTextField::labelWithString(&NSString::from_str("0%"), mtm);
        label.setAlignment(NSTextAlignment::Center);
        label.setFont(Some(&NSFont::boldSystemFontOfSize(30.0)));
        label.setTextColor(Some(&NSColor::labelColor()));
        label.setFrame(NSRect::new(
            NSPoint::new(0.0, 34.0),
            NSSize::new(WIDTH, 38.0),
        ));
        label
    }

    /// The bar beneath it, a continuous capacity gauge from 0 to 100.
    fn level_bar(mtm: MainThreadMarker) -> Retained<NSLevelIndicator> {
        let bar = NSLevelIndicator::initWithFrame(
            NSLevelIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(24.0, 18.0), NSSize::new(WIDTH - 48.0, 14.0)),
        );
        bar.setLevelIndicatorStyle(NSLevelIndicatorStyle::ContinuousCapacity);
        bar.setMinValue(0.0);
        bar.setMaxValue(100.0);
        bar.setDoubleValue(0.0);
        bar
    }

    fn show(&self, level: u8, mtm: MainThreadMarker) {
        self.percent
            .setStringValue(&NSString::from_str(&format!("{level}%")));
        self.bar.setDoubleValue(f64::from(level));
        self.reposition(mtm);
        self.window.orderFrontRegardless();
        self.hide_at.set(Some(Instant::now() + SHOW_FOR));
    }

    /// Centres the overlay near the bottom of the main screen.
    ///
    /// Done on each show rather than once, so it follows the main display when
    /// the arrangement changes underneath it.
    fn reposition(&self, mtm: MainThreadMarker) {
        let Some(screen) = NSScreen::mainScreen(mtm) else {
            return;
        };
        let f = screen.frame();
        let x = f.origin.x + (f.size.width - WIDTH) / 2.0;
        let y = f.origin.y + f.size.height * FROM_BOTTOM;
        self.window.setFrameOrigin(NSPoint::new(x, y));
    }

    fn tick(&self) {
        if let Some(at) = self.hide_at.get()
            && Instant::now() >= at
        {
            self.window.orderOut(None);
            self.hide_at.set(None);
        }
    }

    fn remaining(&self) -> Option<f64> {
        self.hide_at.get().map(|at| {
            let now = Instant::now();
            if at > now {
                (at - now).as_secs_f64()
            } else {
                0.0
            }
        })
    }
}
