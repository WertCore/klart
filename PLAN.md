# Plan

One entry per pull request. A box is ticked only once the change has run against
real hardware — a green test suite on a headless runner is not evidence that a
monitor dimmed, because a headless runner has no monitor.

[`PARITY.md`](PARITY.md) is where the entries past this list come from: it
measures `klart` against the established tools and orders the differences.

## 1. Scaffold the workspace

- [x] Cargo workspace, pinned toolchain, `rustfmt` settings, dual licence
- [x] CI on macOS: format, clippy at `-D warnings`, tests, docs at `-D warnings`
- [x] `Brightness`, the fraction every backend converts to and from

## 2. Find the displays

- [x] Enumerate active displays through `CGGetActiveDisplayList`
- [x] Per display: vendor, model, serial, built-in flag, bounds, main flag
- [x] Readable names out of the IORegistry, since Core Graphics has none
- [x] A stable key per display, so configuration survives a reconnect and a
      `CGDirectDisplayID` that is not stable across one

## 3. The built-in panel

- [x] `BrightnessBackend`, the trait the other two entries implement
- [x] `DisplayServices` bound at run time through `dlopen`, so a macOS release
      that drops the symbol is a clear error rather than a failure to launch
- [x] Get and set on the built-in display

## 4. External monitors over DDC/CI

- [x] `IOAVService` bound the same way
- [x] Match each `CGDirectDisplayID` to its IORegistry node — the two namespaces
      have no common identifier, so this goes through the product attributes
- [ ] VCP `0x10` get and set, with the checksums, the reply validation and the
      inter-message delays that DDC needs to be reliable
- [ ] Report the monitor's own maximum rather than assuming one

The last two stay unticked. The framing and the reply validation are written and
unit tested against hand-computed frames, but no monitor here has answered one.

The only external display available is attached by a USB-C cable that terminates
in HDMI, so the link converts DisplayPort to HDMI inside the cable. On that path
the Mac's display coprocessor refuses every DDC write with `0xe0114102` — a
DCPAV-family rejection rather than a generic "unsupported" — while a read on the
same channel succeeds. Five framings were tried (chip `0x37` and `0x6e`, offsets
`0x51`, `0x00` and `0x6e`, with and without the host address in the buffer) and
all five failed identically, which is what rules the encoding out as the cause.

Verifying these needs a display on DisplayPort or USB-C alt mode without an HDMI
conversion in the path.

## 5. The gamma fallback, and choosing between the three

- [x] Software dimming through `CGSetDisplayTransferByFormula`, floored so that
      the dimmest setting is still a screen this can be undone from
- [x] Establish what a ramp's lifetime actually is
- [x] Per-display resolution: built-in, else DDC/CI, else gamma

The second box replaces "restore the ramp on exit, including on a signal — a
process that dies holding a dark ramp leaves the display dark until the user logs
out". That premise is wrong. macOS reverts a display's ramp when the process that
set it exits, on a clean exit and on `SIGKILL` alike: both read back at 100% from
a fresh process, while a second process reads 50% for as long as the setter is
alive. So there is no signal handler, and `restore_everything` is a panic button
rather than a shutdown path.

The consequence lands on entries 6 and 7 instead. Gamma dimming holds only while
something holds it, so a command that dims a gamma-backed display and exits has
done nothing at all. The menu bar agent is resident and can hold it. The command
line cannot, and has to say so rather than appear to work.

## 6. The command line

- [x] `list`, `get`, `set`, `up`, `down`
- [x] `--display` by index, name or key; `--all`
- [x] `--json`, so it composes with something else
- [x] Say so when a change will not outlive the command, per entry 5

## 7. The menu bar

- [x] An agent under `NSApplicationActivationPolicyAccessory`, so it has no Dock
      tile and no window
- [x] One slider per display, applied live as it is dragged
- [x] Re-enumerate when a display is plugged or unplugged

The plan said "one submenu per display: presets, a step up and a step down",
built on `tray-icon`. Both changed, for one reason: a brightness menu wants a
slider, and a slider in a menu is an `NSView` inside an `NSMenuItem`.

`muda`, which `tray-icon` uses for its menus, has no item of that shape. And the
crate lays a target view over the status item's button which only pops a menu it
was handed itself, so an `NSMenu` built here was never shown — the icon appeared
and clicking it did nothing. With the menu native there was no menu bar work
left for the crate to do, so it is gone, and `muda`, `crossbeam-channel`,
`once_cell`, `serde` and `thiserror` went with it.

The icon is the `sun.max` system symbol rather than a drawn glyph: it is what
Control Center uses for the same thing, and it is already a template image.

A drag is applied while the menu is open rather than queued for the event pump.
AppKit tracks an open menu in a loop of its own, so the pump does not run again
until the menu closes. The first attempt queued the values, which meant they all
landed at once afterwards and the rate limiter kept the first — so the display
jumped to wherever the drag started and looked as though nothing had happened.

## 8. Make the core portable

- [x] `platform`, one module per operating system chosen by `cfg`, so exactly one
      compiles into any binary
- [x] Everything else platform-free: `Brightness`, `DisplayKey`, the DDC/CI
      protocol, the naming and the resolution bookkeeping
- [x] `DisplayKey` portable by contract rather than by coincidence

Ahead of entry 9 rather than after it, because a configuration file that stores
levels against a key fixes that key's format — and a format settled by whatever
macOS happened to publish is not one another operating system can meet.

So the key's derivation is written down as seven numbered rules on
`DisplayKey::of`, and each rule has a test named after it. Every input is an EDID
field, which is the display's own and not the operating system's: macOS reads it
through the IORegistry, Linux through `/sys/class/drm/*/edid`, Windows through
SetupAPI. Two of the rules exist only for portability — a printed serial is
trimmed and confined to a safe character set, because EDID strings are padded and
terminated differently by different readers and two platforms reading the same
panel must still arrive at the same bytes.

The DDC/CI protocol turned out to be portable in its entirety. Only the transport
is not, so the framing, the checksums, the reply validation and the retries moved
to `ddc`, behind a `Link` trait that is four lines wide. Its nine tests no longer
need a Mac to run.

The seam is two functions: `platform::displays` and `platform::open`. The order
mechanisms are tried in sits inside the platform rather than above it, because it
is not the same list everywhere — Windows has two hardware paths and neither is
`DisplayServices`.

This entry also deleted the gamma panic button. Entry 5 measured that macOS puts
a ramp back when the process that set it exits, `SIGKILL` included, so there was
nothing left for it to do; it survived only because it was still exported. The
restructure is what made that visible.

## 9. Remember the levels

- [x] Per-display levels in `~/Library/Application Support/klart`
- [x] Restore on launch and on reconnect, keyed by the stable key from entry 2
- [x] `klart restore` as well, so the feature is usable without the agent

The automatic restore is deliberately narrower than the explicit one. The agent
puts back only displays whose mechanism does not persist — which today means
gamma-dimmed ones, and they are the only displays that genuinely lost anything,
because macOS wiped the ramp when the last process exited. A monitor whose
backlight can be moved keeps its own setting through a reconnect and a reboot,
so there is nothing to restore, and restoring anyway would overrule whatever the
person did with the brightness keys since. `klart restore` restores everything,
because there it was asked for rather than assumed.

The file format is its own rather than a library's: a key, an equals sign and a
percentage, one display to a line. Entry 8's contract already confines keys to
characters that need no quoting, so a parser is thirty lines and a dependency
would have been a larger surface than the thing it parsed. Only a *leading* `#`
is a comment, because a key can contain one — that is how two identical monitors
are told apart, and treating it as a comment anywhere would silently drop exactly
those displays.

## 10. Global hotkeys

- [ ] Brighter and dimmer across every display at once

Split out of entry 9 because it is a decision rather than an implementation, and
the two available answers are not close together.

Registering a chord of `klart`'s own — `RegisterEventHotKey`, in Carbon — needs
no permission and works the moment it is installed, but the shortcut is invented
and nobody's fingers know it.

Taking over the Mac's own brightness keys is what someone actually wants, because
the reason to want this at all is that F1 and F2 do nothing to an external
monitor. It needs a `CGEventTap`, which needs Accessibility, which means a
permission dialog, a trip to System Settings, and an agent that silently does
nothing until that is done.

Not worth guessing at.

## 11. Ship it

- [x] A `Klart.app` bundle with `LSUIElement`, so the agent starts without a
      Dock tile
- [x] A release workflow that builds and attaches it

The agent sets its own activation policy, which covers being run from a shell.
`LSUIElement` covers the other case, and it can only live in a bundle — launched
from Finder without one, macOS gives the agent a Dock tile before any of its code
runs.

The bundle carries the command line too, so that one download is the whole thing.

Apple silicon only, and not universal. The IORegistry walk that finds a monitor's
name and its I2C channel matches `AppleCLCD2`, which Intel Macs do not publish:
an Intel build would enumerate displays and reach neither hardware mechanism.
Shipping half of a universal binary that is known to be degraded and has never
been run is worse than shipping one architecture and saying so.

Not signed and not notarised, which means the first launch is refused and the
release notes have to say how to get past it. Signing needs a paid Developer ID;
that is a decision about money rather than about code.
