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
    NSBackingStoreType, NSColor, NSFont, NSScreen, NSTextAlignment, NSTextField, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CALayer;

/// The overlay's size, in points.
const WIDTH: f64 = 200.0;
const HEIGHT: f64 = 82.0;

/// How round the corners are, matching the system overlay closely enough.
const CORNER: f64 = 18.0;

/// The segmented bar, as macOS draws it: sixteen cells, filled from the left.
const SEGMENTS: usize = 16;
/// The bar's inset from the panel's sides, its baseline, and its thickness.
const BAR_INSET_X: f64 = 20.0;
const BAR_Y: f64 = 20.0;
const BAR_HEIGHT: f64 = 12.0;
/// The gap between cells and how round each cell is.
const SEG_GAP: f64 = 3.0;
const SEG_CORNER: f64 = 2.0;
/// How bright an unfilled cell is — dim, but present, like the system bar.
const EMPTY_ALPHA: f64 = 0.30;

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

/// The overlay's window and the things drawn in it.
struct Hud {
    window: Retained<NSWindow>,
    percent: Retained<NSTextField>,
    /// The sixteen bar cells, kept so their fill can be set on each show.
    segments: Vec<Retained<CALayer>>,
    /// The colour of a filled cell and of an unfilled one, made once.
    filled: Retained<CGColor>,
    empty: Retained<CGColor>,
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

        let filled = NSColor::whiteColor().CGColor();
        let empty = NSColor::colorWithWhite_alpha(1.0, EMPTY_ALPHA).CGColor();

        let percent = Self::percent_label(mtm);
        let (bar, segments) = Self::segment_bar(mtm, &empty);
        effect.addSubview(&percent);
        effect.addSubview(&bar);
        window.setContentView(Some(&effect));

        Self {
            window,
            percent,
            segments,
            filled,
            empty,
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

    /// The segmented bar beneath it, sixteen cells, the way macOS draws it.
    ///
    /// A container view whose layer holds one sublayer per cell, so a change is
    /// just recolouring sixteen layers rather than redrawing anything. Starts all
    /// unfilled; the first show sets the fill.
    fn segment_bar(
        mtm: MainThreadMarker,
        empty: &CGColor,
    ) -> (Retained<NSView>, Vec<Retained<CALayer>>) {
        let bar_width = WIDTH - 2.0 * BAR_INSET_X;
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(BAR_INSET_X, BAR_Y),
                NSSize::new(bar_width, BAR_HEIGHT),
            ),
        );
        container.setWantsLayer(true);

        // The cells share the width evenly, with a gap between each.
        let seg_width = (bar_width - SEG_GAP * (SEGMENTS as f64 - 1.0)) / SEGMENTS as f64;
        let mut segments = Vec::with_capacity(SEGMENTS);
        for i in 0..SEGMENTS {
            let cell = CALayer::layer();
            cell.setFrame(NSRect::new(
                NSPoint::new(i as f64 * (seg_width + SEG_GAP), 0.0),
                NSSize::new(seg_width, BAR_HEIGHT),
            ));
            cell.setCornerRadius(SEG_CORNER);
            cell.setBackgroundColor(Some(empty));
            if let Some(layer) = container.layer() {
                layer.addSublayer(&cell);
            }
            segments.push(cell);
        }

        (container, segments)
    }

    fn show(&self, level: u8, mtm: MainThreadMarker) {
        self.percent
            .setStringValue(&NSString::from_str(&format!("{level}%")));
        self.paint(level);
        self.reposition(mtm);
        self.window.orderFrontRegardless();
        self.hide_at.set(Some(Instant::now() + SHOW_FOR));
    }

    /// Fills the cells up to the level, the rest dim, rounding to the nearest
    /// cell so a whole-number step of a sixteenth lands on exactly one more.
    fn paint(&self, level: u8) {
        let lit = (f64::from(level) / 100.0 * SEGMENTS as f64).round() as usize;
        for (i, cell) in self.segments.iter().enumerate() {
            let colour = if i < lit { &self.filled } else { &self.empty };
            cell.setBackgroundColor(Some(colour));
        }
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
