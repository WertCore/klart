//! The menu behind the status item.
//!
//! Built with AppKit directly rather than through `muda`. A brightness menu
//! wants a slider, a slider in a menu is an `NSView` inside an `NSMenuItem`, and
//! `muda` has no item of that shape — so the choice was between mixing its menu
//! with raw items poked into the `NSMenu` underneath it, or building the whole
//! thing here. One mechanism is easier to follow than one and a half.
//!
//! `tray-icon` still owns the status item itself, which is the fiddly part it
//! does well: an `NSStatusItem`, and an RGBA buffer turned into a template
//! `NSImage`.

use std::rc::Rc;

use klart_core::LoginItem;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem, NSSlider, NSView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::driver::Driver;

use crate::request::{self, Request};

/// Wide enough for a slider that can be aimed, narrow enough not to make the
/// menu look like a window.
const ROW: NSSize = NSSize {
    width: 232.0,
    height: 26.0,
};

/// The tag the combined slider carries. It drives every display, so it names
/// none of them.
const ALL_DISPLAYS: usize = 0;

/// Indented to sit under the heading's text rather than its left margin.
const TRACK: NSRect = NSRect {
    origin: NSPoint { x: 20.0, y: 3.0 },
    size: NSSize {
        width: 196.0,
        height: 20.0,
    },
};

/// What the menu's controller needs to remember.
///
/// Only the headings. Everything else a click needs — which display, what level
/// — is carried on the slider itself.
pub struct MenuState {
    /// The heading above the combined slider, when there is one.
    all: Option<Retained<NSMenuItem>>,
    items: Vec<Retained<NSMenuItem>>,
    names: Vec<String>,
    transient: Vec<bool>,
    /// Shared with the agent, because a drag has to reach the display while the
    /// menu is open and the agent is not running then.
    driver: Rc<Driver>,
}

define_class!(
    // SAFETY:
    // - `NSObject` imposes no subclassing requirements.
    // - `Controller` implements no `Drop`.
    #[unsafe(super(NSObject))]
    // It is a target for menu items and sliders, which AppKit only ever sends
    // to on the main thread.
    #[thread_kind = MainThreadOnly]
    #[name = "KlartMenuController"]
    #[ivars = MenuState]
    pub struct Controller;

    impl Controller {
        /// Sent continuously while a slider is dragged.
        #[unsafe(method(brightnessChanged:))]
        fn brightness_changed(&self, sender: &NSSlider) {
            // Which display is carried on the slider's tag rather than in a
            // controller per display: one controller for the whole menu, and
            // the sender says which row it came from.
            let Ok(display) = usize::try_from(sender.tag()) else {
                return;
            };
            let percent = request::percent_from(sender.doubleValue());

            // Applied here rather than queued for the pump. AppKit tracks an
            // open menu in a loop of its own, so the pump does not run again
            // until the menu closes — a queued value would land after the drag
            // was over, which is not what someone dragging a brightness slider
            // is watching for.
            self.ivars().driver.request(display, percent);

            // Rebuilding the menu would end the drag, so the heading is
            // relabelled in place.
            self.relabel(display, percent);
        }

        /// Sent continuously while the combined slider is dragged.
        #[unsafe(method(allDisplaysChanged:))]
        fn all_displays_changed(&self, sender: &NSSlider) {
            let percent = request::percent_from(sender.doubleValue());
            let state = self.ivars();

            state.driver.request_all(percent);

            if let Some(heading) = state.all.as_ref() {
                heading.setTitle(&NSString::from_str(&request::heading(
                    "All displays",
                    percent,
                    false,
                )));
            }
            // The per-display headings are stale the moment this moves, and the
            // menu cannot be rebuilt under a drag without ending it.
            for display in 0..state.items.len() {
                self.relabel(display, percent);
            }
        }

        #[unsafe(method(toggleLoginItem:))]
        fn toggle_login_item(&self, sender: &NSMenuItem) {
            // The item's own tick is the current state, so the request is its
            // opposite. Set here as well so the tick moves with the click; if
            // the system disagrees the next rebuild corrects it.
            let wanted = sender.state() != NSControlStateValueOn;
            sender.setState(if wanted {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            request::push(Request::SetLoginItem(wanted));
        }

        #[unsafe(method(lookAgain:))]
        fn look_again(&self, _sender: Option<&AnyObject>) {
            request::push(Request::Refresh);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            request::push(Request::Quit);
        }
    }

    unsafe impl NSObjectProtocol for Controller {}
);

impl Controller {
    fn new(mtm: MainThreadMarker, state: MenuState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }

    fn relabel(&self, display: usize, percent: u8) {
        let headings = self.ivars();
        let (Some(item), Some(name), Some(&transient)) = (
            headings.items.get(display),
            headings.names.get(display),
            headings.transient.get(display),
        ) else {
            return;
        };
        item.setTitle(&NSString::from_str(&request::heading(
            name, percent, transient,
        )));
    }
}

/// The menu, and the controller that must outlive it.
///
/// The controller is returned rather than dropped because `NSControl` holds its
/// target weakly — AppKit will not keep it alive, and a released one is a menu
/// whose sliders do nothing.
pub struct Built {
    pub menu: Retained<NSMenu>,

    /// Never read, and load-bearing anyway: `NSControl` stores its target as an
    /// unowned reference, so this retain is the only thing keeping the sliders'
    /// target alive. Dropping it leaves a menu whose sliders move and do
    /// nothing.
    #[expect(dead_code, reason = "held for its lifetime, not its value")]
    pub controller: Retained<Controller>,
}

/// Builds the whole menu from the displays the agent currently holds.
pub fn build(mtm: MainThreadMarker, driver: &Rc<Driver>) -> Built {
    let menu = NSMenu::new(mtm);
    let controls = driver.controls();

    // Only worth the space when there is more than one thing to combine.
    let combined = (controls.len() > 1).then(|| {
        label(
            mtm,
            &request::heading("All displays", driver.average(), false),
        )
    });

    let mut state = MenuState {
        all: combined.clone(),
        items: Vec::with_capacity(controls.len()),
        names: Vec::with_capacity(controls.len()),
        transient: Vec::with_capacity(controls.len()),
        driver: Rc::clone(driver),
    };

    for control in controls.iter() {
        let transient = !control.persists();
        let percent = control.get().map_or(0, |level| level.percent_rounded());

        let heading = label(
            mtm,
            &request::heading(control.display().name(), percent, transient),
        );
        state.items.push(heading);
        state.names.push(control.display().name().to_owned());
        state.transient.push(transient);
    }

    let controller = Controller::new(mtm, state);

    if controls.is_empty() {
        menu.addItem(&label(mtm, "No displays found"));
    }

    if let Some(heading) = combined {
        menu.addItem(&heading);
        menu.addItem(&slider_row(
            mtm,
            &controller,
            ALL_DISPLAYS,
            driver.average(),
            sel!(allDisplaysChanged:),
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }

    for (display, control) in controls.iter().enumerate() {
        if display > 0 {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        let heading = controller.ivars().items[display].clone();
        menu.addItem(&heading);

        let percent = control.get().map_or(0, |level| level.percent_rounded());
        menu.addItem(&slider_row(
            mtm,
            &controller,
            display,
            percent,
            sel!(brightnessChanged:),
        ));

        if !control.persists() {
            menu.addItem(&label(mtm, "· held only while klart runs"));
        }
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&login_item(mtm, &controller));
    menu.addItem(&command(
        mtm,
        &controller,
        "Look for displays again",
        sel!(lookAgain:),
    ));
    menu.addItem(&command(mtm, &controller, "Quit klart", sel!(quit:)));

    Built { menu, controller }
}

/// A line that says something and does nothing.
fn label(mtm: MainThreadMarker, text: &str) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(text));
    item.setEnabled(false);
    item
}

/// A line that does something.
fn command(
    mtm: MainThreadMarker,
    controller: &Controller,
    text: &str,
    action: Sel,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(text));
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(controller));
    }
    item
}

/// The row that decides whether the agent comes back after a restart.
///
/// Worth a place in the menu rather than leaving it to System Settings: on a
/// display with no hardware brightness control the level lasts only as long as
/// this process, so an agent that does not start at login means that display is
/// back at full brightness after every restart.
fn login_item(mtm: MainThreadMarker, controller: &Controller) -> Retained<NSMenuItem> {
    match klart_core::login_item() {
        LoginItem::Unavailable => label(mtm, "Start at login — needs the app bundle"),

        LoginItem::AwaitingApproval => {
            // Registered, and macOS is waiting for the person to allow it in
            // System Settings. Saying "on" here would be a lie.
            label(mtm, "Start at login — allow it in System Settings")
        }

        state => {
            let item = command(mtm, controller, "Start at login", sel!(toggleLoginItem:));
            item.setState(if state == LoginItem::Enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            item
        }
    }
}

/// A menu row that is a slider.
fn slider_row(
    mtm: MainThreadMarker,
    controller: &Controller,
    display: usize,
    percent: u8,
    action: Sel,
) -> Retained<NSMenuItem> {
    let row = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: ROW,
    };

    let container = NSView::initWithFrame(NSView::alloc(mtm), row);
    let slider = NSSlider::initWithFrame(NSSlider::alloc(mtm), TRACK);

    slider.setMinValue(0.0);
    slider.setMaxValue(100.0);
    slider.setDoubleValue(f64::from(percent));
    // Continuous, so that the display follows the drag rather than jumping when
    // it is let go. Everything downstream is cheap enough to keep up except
    // DDC/CI, which the agent rate limits rather than this.
    slider.setContinuous(true);

    // SAFETY: the tag is an arbitrary integer AppKit carries for the caller, and
    // the target is kept alive by `Built`.
    unsafe {
        slider.setTag(isize::try_from(display).unwrap_or(0));
        slider.setTarget(Some(controller));
        slider.setAction(Some(action));
    }

    container.addSubview(&slider);

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&container));
    item
}
