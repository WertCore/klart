# Plan

One entry per pull request. A box is ticked only once the change has run against
real hardware — a green test suite on a headless runner is not evidence that a
monitor dimmed, because a headless runner has no monitor.

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

The reconnect half of that last box was argued structurally at first — the key
contains no operating system handle — and has since been observed. The external
display was unplugged, stayed away long enough to disappear from
`system_profiler`, and came back as `SAM-71e3-HNAW900001`, byte for byte the key
it had before.

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

What the write is refused with has since been decoded: `0xe0114102` is
`sub_iokit_audio_video` (IOKit's `err_sub(0x45)`) with the family's own code 258.
So it is the display coprocessor's own refusal rather than a generic
`kIOReturnUnsupported` — the request reached the AV family and that family said no.

Independent reverse engineering corroborates the framing. Asahi Linux implements
DDC/CI through the same DCP firmware service macOS uses, as commands 9 and 10 on
`dcpav-service-epic`, with the first byte of a write carried as the firmware's
data-address parameter and settle delays of 10 ms after a write and 40 ms before
a read. That is byte for byte what `crate::ddc` does. It also settles that there
is no lower-level route: "the DCP firmware owns the DisplayPort AUX channel, so
the AP cannot run I2C-over-AUX itself". Nothing in user space gets underneath it.

**The cause is now known, and it is neither of the two things it looked like.**
Entry 13's probe found it. On this display's link the I2C chip address is
*ignored*: a read at the DDC/CI address returns, byte for byte, what a read at
the EDID address returns, and so does a read at any other address. Every write is
refused, including to an address nothing lives at. The link serves a cached copy
of the EDID and does no I2C at all, so nothing ever reaches the monitor — which
rules out the monitor's own DDC/CI setting, because the monitor is never asked.

The control that makes that conclusive is the built-in panel, on the same
machine, through the same calls in the same process: it *honours* the chip
address — `0x50` returns the EDID header and `0x37` returns zeros — and it
accepts writes. So the difference is the link, not the interface and not the
machine.

Everything else that could have been a way round it has been ruled out by
measurement rather than by assumption:

- **`CoreDisplay`** publishes `CoreDisplay_Display_SetUserBrightness` and
  friends. Its signature does not validate: asked for the built-in panel's level,
  which `DisplayServices` reports as 0.835, it answers 1.000. An unvalidated
  signature on an unpublished symbol is undefined behaviour rather than a failed
  call, so it is not used. `DisplayServices` has in any case already said it
  cannot reach this display.
- **A second I2C path.** `DCPAVServiceProxy` is the only display-facing I2C class
  in the registry; the other two matches are the SoC's own controllers.
- **A capability to switch on.** `IOAVServiceCopyProperties` returns identical
  dictionaries for the working link and the failing one, differing only in
  `Location`. There is no flag, nothing to enable; the firmware degrades quietly.

The link is `DP -> HDMI`, which is to say the DisplayPort-to-HDMI conversion
happens inside the cable. Ticking these two boxes needs a link with no conversion
in it: USB-C to DisplayPort, into the monitor's DisplayPort input. The probe will
say so either way — if the chip address starts being honoured on such a link,
the cable was the cause.

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

The seam is `platform::displays`, `platform::open` and `platform::config_directory`.
The order mechanisms are tried in sits inside the platform rather than above it,
because it is not the same list everywhere — Windows has two hardware paths and
neither is `DisplayServices`.

It was a claim rather than a fact until the dependencies were made to follow the
platform. They sat in a plain `[dependencies]` table, so a build for another
target failed inside a binding crate that refuses to compile off Apple hardware,
several layers below anything a porter could act on. They are under a target
table now, and a build for Windows or Linux stops at exactly one error — this
module's own, naming the three functions to implement. CI checks that, because a
seam nothing exercises stops being one the first time an Apple-only crate creeps
back into the wrong table.

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

## 12. Dim past where the backlight stops

- [x] Compose the backlight with the gamma ramp instead of choosing between them
- [x] Hold the seam continuous, so nothing jumps as it is crossed

A backlight has a minimum and on a laptop panel in a dark room that minimum is
still too bright. Below it the ramp is the only thing left. So the bottom quarter
of the range holds the backlight at its floor and dims with the ramp, and
everything above it drives the backlight and leaves the ramp alone.

Both mature macOS tools treat this as a headline feature, which is what put it
first on the list in the parity work.

The composition is platform-free: it takes two `Backend`s and does not care which,
so a Windows port gets it for nothing.

One consequence worth knowing: below the seam part of the level lives in a ramp
macOS discards when the process exits, so `Backend::persists` became a question
about *where the level currently is* rather than about the mechanism. It is
answered from a cached value, because a menu asks it on every draw and a DDC/CI
read costs the better part of a tenth of a second.

## 13. Say why a display will not answer

- [x] `klart probe`: what was tried, what came back, and what it means
- [x] Decode `IOReturn` into its subsystem, which is where the meaning is
- [x] Run it against a display that refuses DDC/CI

Every tool in this space reports the same thing when DDC/CI fails: that it
failed. That is the least useful true statement available, because the ordinary
causes want different responses — turn a setting on, change a cable, or accept
the software fallback.

The first attempt at a control was reading the EDID, on the grounds that it sits
on the same two wires at a different address and is always present. That is
wrong, and running it proved it wrong: a successful EDID read does not mean an
I2C transaction happened. The coprocessor caches the EDID when the link comes up
and will serve that cache while ignoring the address it was asked for.

The control that works is to read the *same offset at two different chip
addresses*. On a link doing real I2C they differ, because they are two different
devices. Identical bytes mean the address was ignored and both came from one
cache. That distinguishes:

- addresses differ, DDC/CI refused → transactions reach the monitor and it is
  declining. Look for DDC/CI in its on-screen menu; most ship with it off.
- addresses identical → the link does no I2C. Nothing reaches the monitor, the
  monitor's own settings are irrelevant, and no software can change it.

Where a machine has one link of each kind, the probe says so, because one link
failing proves only that something is wrong — another link succeeding through the
same calls is what proves the difference is the link.

## 14. Start with the session

- [x] Register the bundle as a login item, from the menu and from the command line
- [x] Show when macOS is waiting to be told it may

Not a convenience. On a display with no hardware brightness control the level
lasts exactly as long as this process does, because macOS reverts a gamma ramp
when the process that set it exits. An agent that does not start at login means
such a display is back at full brightness after every restart, whatever was asked
for before it. That is the whole reason this is entry 14 rather than a footnote.

`SMAppService` arrived in macOS 13 and the rest of this works further back, so the
class is looked up before it is used rather than raising the crate's floor for one
feature. It also registers a *bundle*: run as a bare binary out of `target/` there
is nothing for the system to launch, and saying so is more use than passing on a
framework's complaint about a path.

Three states rather than two. macOS puts a newly registered login item in front of
the person before it will honour it, and until they say yes in System Settings it
is registered and not running. Reporting that as "on" would be a lie, so it is its
own state in the menu and on the command line.

It lives in `core` rather than in the agent, even though the agent is what gets
launched, because `klart` has no settings window and the command line is where its
settings live. Both binaries sit in the same bundle, so either can register it.

## 15. One slider for all of them

- [x] A combined row above the per-display ones, when there is more than one

Every comparator has this and it is the commonest thing anyone wants from such a
menu: dim everything, now.

Absolute rather than relative. Moving each display by the same delta would
preserve whatever balance had been set between them, which is the nicer property
right up until one saturates and the balance is silently lost anyway — and it
needs a drag origin to be captured and held, which a continuous action does not
hand you. Setting them all to the level asked for is predictable at every point
in the range, and predictable wins in a control someone drags.

It starts at the mean of the displays' levels. Any single display's level would
be an arbitrary choice and a fixed position would jump the moment it was touched.

The held-back value is one value rather than one per display, because the last
position of a drag is what was chosen and it is the same for all of them.

Worth noting for review: both places that write to every display use an explicit
loop rather than `all`, which short circuits — it would have landed the level on
the first display and dropped it for the rest. Clippy suggested exactly that
change and the note attached to its own lint is what caught it.

## 16. Call a display what you call it

- [x] `klart rename`, stored beside the levels
- [x] Use it everywhere a display is shown

Last of the polish items from the comparison. It matters for one case in
particular: two monitors of the same model publish the same name, and telling
them apart in a menu by their EDID serials is not something anyone should have to
do.

The name lives in the same file as the levels, marked by `:name` after the key.
The colon is the point — entry 8's contract confines keys to
`A-Z a-z 0-9 . _ -`, so a colon cannot occur inside one and the two kinds of line
can never be confused. The value is free text and may contain anything, including
an equals sign, because the line is split on the *first* one.

Applied in `Control` rather than in `Display`. A `Display` is what the hardware
says it is; a `Control` is how a person deals with it, and a chosen name is the
person's.

## 17. Linux

- [x] Connectors and EDID from `/sys/class/drm`
- [x] The panel's backlight through `/sys/class/backlight`
- [x] DDC/CI over `/dev/i2c-*`, reusing the protocol from entry 8
- [x] Autostart as a freedesktop desktop entry
- [x] CI that builds and tests it
- [ ] Run it on a Linux machine

**Written, compiled and tested by CI. Never run.** The last box is the only one
that matters and it needs a machine this does not have.

Entry 8 paid for itself here. The DDC/CI protocol needed no changes at all: Linux
implements `Link`, which is two methods over an `I2C_RDWR` ioctl, and inherits
the framing, the checksums, the reply validation and the retries along with their
nine tests. One asymmetry was real and is documented where it is handled —
`IOAVServiceWriteI2C` takes the host's source address as an argument and puts it
on the wire itself, and a raw I2C write has nowhere to carry it, so Linux
prepends it.

The EDID parser is new and shared. macOS gets those fields pre-parsed from the
IORegistry; Linux and Windows get 128 raw bytes. Writing a second parser against
the same specification would have been a second chance to disagree about a
display key that is supposed to be identical everywhere, so there is one, and its
test asserts that a real block from the monitor on this desk produces
`SAM-71e3-HNAW900001` — the key macOS arrived at through the IORegistry, by a
completely different route.

Two honest gaps. There is no geometry: where a display sits is a compositor's
idea rather than the kernel's, and reaching a compositor means linking X11 or one
of several Wayland protocols. Bounds are zero, and the only thing that costs is
ordering two otherwise identical monitors. And there is no software fallback,
because a gamma ramp needs a display server too — so a display that answers
neither mechanism reports why rather than being dimmed badly.

`klart-tray` is macOS only and says so. A tray on Linux is StatusNotifierItem
over D-Bus or legacy XEmbed depending on the desktop; that is a new crate rather
than a `cfg`.

## 18. Windows

- [x] Monitors and their stored EDID, joined on the device instance path
- [x] External monitors through the Monitor Configuration API
- [x] The gamma ramp as the fallback
- [x] Autostart under the `Run` key
- [x] CI that builds and tests it
- [x] The laptop panel, through WMI
- [ ] Run it on a Windows machine

**Written, compiled and tested by CI. Never run.**

The pleasant surprise is that this is the easiest of the three. Every mechanism
is documented, supported public API: `GetMonitorBrightness` and
`SetMonitorBrightness` are DDC/CI with the driver doing the framing, the
checksums and the retries — so `crate::ddc`, which macOS and Linux hand-roll, is
not used on this platform at all. It also reports the monitor's own *minimum* as
well as its maximum, which nothing else does, and a monitor whose minimum is not
zero is the case that would otherwise be got quietly wrong.

The join is the same shape of problem macOS has. `EnumDisplayMonitors` gives the
handle every brightness call takes; the EDID is in the registry under the device
instance path; nothing connects them directly. `EnumDisplayDevices` bridges the
two, and the path rewriting it needs is tested.

Two things left out on purpose rather than guessed at.

The **laptop panel** wants the WMI class `WmiMonitorBrightnessMethods`, which
means COM, which means several hundred lines that cannot be checked from here.
An internal panel therefore falls through to the gamma ramp: a usable control,
and not the right one.

**Whether a panel is internal** is not something Windows says outright, so every
display is reported as external and the mechanism order sorts it out — a panel
refuses DDC/CI and lands on the ramp, which is where it would have landed anyway.

Unlike macOS, Windows leaves a gamma ramp in place when the process that set it
exits, so `persists` is true here where it is false there. That is the better
behaviour for a command line and it falls out of the platform rather than being
arranged.

The seam guard moved to `aarch64-apple-ios`, because all three desktop platforms
now have a module and the check needs a target that does not.

## 19. Ship the other two

- [x] Build and archive the command line on Linux and Windows, from their own
      runners
- [x] One release carrying all three

Cross-compiling would have been fewer jobs and the wrong answer: a binary built
on a Mac for Linux is one nothing has run the tests against, and the entire point
of shipping those two is that somebody can run them. So each platform builds its
own artefact, behind its own gates, and a last job gathers them.

`tar` on Linux and zip on Windows, for the same reason in opposite directions:
tar keeps the executable bit that a zip drops, and Explorer opens a zip without
anything installed.

The release notes say plainly which platforms have been run on real hardware and
which have not, and what permissions Linux will want that this program cannot
grant itself.

## 20. Homebrew

- [x] A tap at [WertCore/homebrew-tap](https://github.com/WertCore/homebrew-tap)
- [x] `brew install wertcore/tap/klart`

A tap rather than homebrew-core, which has notability requirements a new project
does not meet. The formula is the one it would be there.

It installs the released binary rather than building. The first version built
from source, which pulls the whole Rust toolchain in as a build dependency and
leaves it on the machine — `brew autoremove` clears it, but few people run that,
and asking for a gigabyte to produce a four-hundred-kilobyte binary is a poor
trade. The other half of the argument for building was avoiding macOS quarantine,
and that was simply wrong: quarantine is set by browsers and LaunchServices, not
by Homebrew's downloader. Source builds remain available through `--HEAD`, which
is where the Rust dependency belongs.

Two things the tap's own CI caught, both of which `brew audit` on this machine
could not:

- the service block pointed at `klart-tray` on Linux, where there is no agent and
  never will be one in that crate. `service` is not accepted inside `on_macos`,
  so it is a plain conditional.
- an archive with a single top-level directory leaves Homebrew already inside it,
  so `Klart.app/Contents/MacOS/klart` had the bundle in the path twice.

One consequence worth remembering: the formula pins a version and a checksum, so
every release needs the tap updated. Automating that from the release workflow is
a reasonable follow-up and is not done.

## 21. What people actually ask for

- [x] Read the open issues, discussions and threads of the three comparators
- [ ] Restore on wake, not only on launch and reconnect
- [ ] Say so when HDR has taken the brightness control away
- [ ] Scroll the tray icon
- [ ] Decide whether the all-displays slider should keep relative offsets

Everything below is counted from the comparators' own trackers on 18 September
2026, written down so the numbers can be checked rather than believed. Reaction
and comment counts are the signal — an issue nobody comments on is one person's
preference, and a seven-year-old issue with forty comments is a product
requirement.

### The one complaint everybody has

Brightness comes back wrong after sleep. It is the single most repeated
grievance across all three projects, and it arrives in two shapes: the monitor
forgets, and the program forgets.

- Monitorian, "Restore brightness of external display which is reset to 100%
  after restart/resume" — 4 reactions, 26 comments
- MonitorControl discussion 615, "resets to 100% on Lock Screen or after Sleep"
- MonitorControl discussion 850, "Always increases brightness of external
  display after sleep"
- MonitorControl, "App doesn't read brightness after waking up" — 4 reactions,
  16 comments
- MonitorControl, "App may crash on wake from sleep or display connect or
  disconnect" — 4 reactions, 42 comments, the highest comment count found
  anywhere in this survey

klart already remembers a level and restores it, but only on two events: the
agent starting, and a display appearing. Sleep and wake is neither, so today it
has the same bug as everybody else. Closing that is the highest-value thing on
this list and it is not large — the event exists on all three platforms
(`NSWorkspace.didWakeNotification`, `WM_POWERBROADCAST`, logind's
`PrepareForSleep`).

The part that will be got wrong if it is rushed: a monitor does not answer DDC
the instant the Mac wakes. The link comes back before the scaler does, so a
single write on the wake notification lands in a void and reports success. It
needs a bounded retry — read back and compare, not write and hope — which is
also, conveniently, the thing that stops the write from racing whatever the
monitor is doing to itself on resume.

The crash reports clustering at the same moment are worth noticing separately.
Wake, connect and disconnect are where display handles go stale, and a program
that keeps one across the event gets a use-after-free. The comparators are
Swift and C#, where an invalid `CGDirectDisplayID` is an integer nothing checks.
klart's handles are behind `IoRef` and `CFRetained` and the mechanism is chosen
per open rather than cached, so this is a class of bug it structurally does not
have — but only as long as nothing starts caching a `Control` across a display
change to save a few milliseconds. Worth stating here before somebody does.

### Nobody can tell you why it failed

The second theme is not a missing feature, it is a missing answer.

MonitorControl discussion 1299 is the clearest case: DDC stopped working on M2
Pro and M2 Max machines, eleven people reported it across LG, Dell, BenQ and
Gigabyte monitors, and the resolution was that neither the users nor the
maintainer could determine the cause — only that something in the architecture
had changed. The advice given was to use a different application. The project
also maintains a hand-written troubleshooting wiki page and a list of ports that
do not work, which is what a project ends up with when the program cannot
explain itself.

This is what `klart probe` is for, and it is the one thing in this survey that
no comparator has in any form. It is already built and it already answers this
exact question — it distinguishes "the monitor declines" from "there is no
channel" from "the link serves a cache and does no I2C", by measurement rather
than by asking somebody to try another cable. The work remaining is not code:
it is that the README treats it as a debugging aid rather than as the reason to
choose this program.

### Two things already shipped that others are still asking for

- ddcutil, "Feature request: JSON output" — 15 reactions, the most-reacted
  feature request found in this survey
- ddcutil, "Parseable output" — 4 reactions
- Monitorian, "Command-line options to get, set brightness, contrast or input" —
  4 comments

klart has `--json` and a command line on all three platforms, because the
command line was the first thing built rather than an afterthought bolted to a
GUI. Nothing to do here except know that it is a differentiator and say so.

### HDR

- Monitorian, "HDR monitors don't adjust brightness independently" — 8 comments
- Monitorian, SDR content brightness under HDR

Under HDR on Windows a monitor may switch picture mode, pin brightness to a
preset, weaken the effect of a DDC write, or refuse hardware brightness control
outright. The gamma ramp is not a reliable fallback either: Windows does not
guarantee ramp behaviour in HDR mode, so the software path can be reduced or
ignored as well.

The realistic goal is not to fix this — it is largely the monitor's firmware —
but to stop it looking like a bug in the program. A write that is accepted and
changes nothing is exactly the failure `probe` exists to name, and HDR is a
verdict it should learn: detect that the display is in HDR, report it as the
reason, and suggest the one thing that does work. That is a small addition to
`Verdict` and fits the existing shape.

### Smaller, cheap, asked for

- Scrolling the wheel over the tray icon to change brightness — Monitorian, 11
  comments. No klart equivalent; the menu has to be opened first.
- Preserving relative offsets so perceived brightness stays consistent when
  several displays move together — MonitorControl, 2 comments.
- Skipping launch-at-login when no external display is connected —
  MonitorControl.
- Reordering displays in the menu — MonitorControl.

The offsets one is a genuine disagreement with a decision already made. Entry 15
gave the all-displays slider absolute semantics: every display goes to the value
shown. Relative semantics would move each display by the same delta and keep a
monitor that was set dimmer dimmer. Absolute is easier to reason about and is
the right default for "make everything 40%"; relative is what somebody wants
when they have already balanced two panels by eye. This is a preference, not a
bug, and the honest resolution is that it becomes one — not that the default
flips. Recording it here so the decision is not quietly reversed by whoever
reads the issue next.

### On price

Lunar runs a 14-day Pro trial and then falls back to a free tier, licences five
Macs, and has a documented failure mode where the licence stops staying
activated if the bundle is modified or Paddle's domains are blocked. klart is
MIT or Apache-2.0 with nothing gated, which is worth stating plainly in the
README rather than leaving someone to infer it.

### What this changes

Ranked by evidence behind them, not by how interesting they are to build:

1. Restore on wake, with read-back retry. Fixes the most-complained-about
   behaviour in the category and klart has it too.
2. Lead with `probe`. Already built, genuinely unique, answers the question the
   comparators close as unanswerable.
3. An HDR verdict in `probe`. Small, and turns a "this app is broken" report
   into an explanation.
4. Scroll on the tray icon. Small, and asked for.
5. Relative offsets as an option. Not a default change.

Items 3 and 4 are worth doing whenever; item 1 is the one that should be next.
