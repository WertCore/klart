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

## 21. Put the levels back after a wake

- [x] Watch for the machine waking and for the screens waking
- [x] Restore every display, not only the ones on the gamma ramp
- [x] Retry until the display answers, or until a deadline

A monitor does not reliably keep its brightness across a sleep. Plenty come back
at full and have to be dimmed again by hand, every morning. The agent is already
running and already knows what the level was, so it is in a position to put it
back, and until now it did not: it restored on launch and when a display
appeared, and a wake is neither.

Two notifications, because there are two sleeps. `NSWorkspaceDidWake` is the
machine; `NSWorkspaceScreensDidWake` is the displays coming back while the
machine stayed up, which is what a display sleep timeout produces and is just as
likely to have reset a monitor. Both set one flag, and the flag is read once a
pass, so both firing together costs one restore.

**This restore covers every display, where the existing one does not.** That is
a deliberate split rather than an inconsistency. The restore on launch skips a
monitor whose backlight persists, because such a monitor keeps its own setting
and overruling it would undo whatever its buttons had been used for since. Across
a sleep that argument does not hold — the monitor resetting itself is the whole
problem, and nobody pressed anything while the machine was asleep. The cost is
that a monitor adjusted by its own buttons and then slept comes back where klart
last left it. That is a real regression for somebody and it is the lesser one:
what klart restores is also a level the person chose, and it is the most recent
one it can see.

The part that is easy to get wrong, and that makes this feature usually get
reported as broken rather than missing: **a display does not answer the moment
the machine wakes.** The link comes back before the scaler behind it, and in that
window a write is accepted and dropped with no error — indistinguishable from
success at the point of writing. So a restore is not a write. It is a write
followed by a read-back, repeated every half second until the display agrees or
fifteen seconds pass. A display that will never answer stops being asked, and
says so once rather than thirty times.

The read-back allows two percent of slack, because a monitor whose range is not
a hundred quantises: 40% of a range of 64 is stored as 26 and reads back as 41%.
That is the display agreeing.

The clock lives on the restore rather than in the loop driving it, so the rules
about when to try again and when to give up are tested against synthetic instants
rather than against a monitor that has to be asleep to be interesting.

Verified on this machine as far as it can be without sleeping it: the observer
registers, and posting both notifications by hand reaches the selector and raises
the flag. The behaviour across a real suspend is not verified here.

One thing the selector arrangement cannot check at compile time is that the name
`define_class!` registers and the name handed to the notification centre are the
same string. They are checked at registration instead, because the alternative
symptom is an unrecognized selector hours later that reads as a crash on resume.

## 22. Lead with the probe

- [x] `klart probe` in the README, with its own output and what the verdicts mean
- [x] Say that the agent puts levels back after a sleep

No code. The README described `klart` as a brightness control with a debugging
aid attached, and that is backwards. Plenty of programs move a brightness
slider. The thing this one does that is hard is say *why a monitor will not move*
— and it says it from measurement, not from a list of things to try.

So `probe` is now the second thing in the README, with its real output pasted in
rather than described, and a table of what each verdict means and what to do
about it. The table is taken from `Verdict::advice`, so the two say the same
thing.

The transcript is verbatim as far as the elision, which is marked. The built-in
panel's block is cut down to the one line the paragraph underneath refers to: it
is the control. Two displays, the same calls, the same process, one honouring
the I2C chip address and one ignoring it — without that comparison `EdidOnly` is
an assertion rather than a measurement, and it is worth the reader seeing both
halves of it.

Two things were wrong in the first draft of the section and are worth recording,
because both would have been believed:

- it offered `klart probe --json`, which does not exist. `--json` is on the
  reading commands, not on this one. Left out rather than invented; a probe
  transcript is for pasting into an issue, and adding the flag is its own change
  with its own tests.
- it had `NoI2c` and `NoChannel` the wrong way round. `NoChannel` is no I2C
  channel published at all, which is what a virtual screen looks like — AirPlay,
  Sidecar, DisplayLink. `NoI2c` is a channel that reads nothing, not even the
  EDID.

## 23. Name HDR when it is the reason

- [x] Ask Windows whether a display is in HDR, and say so on every probe
- [x] Turn that into a verdict when brightness control also refused
- [ ] Run it on a Windows machine with an HDR monitor

HDR takes brightness control away and does not mention it. A monitor in an HDR
picture mode commonly pins its brightness to a preset or stops honouring the
brightness feature altogether, and the gamma ramp is no refuge either — Windows
does not guarantee ramp behaviour while HDR is on. From outside, all of that
looks like a brightness control that has broken.

Until now the probe said `Unclear` for it, which is true and useless: none of the
things `Unclear` sends someone off to try are the thing that works.

The query is a different family from the rest of this platform module — not
`EnumDisplayMonitors` and an `HMONITOR` but the display configuration API, which
addresses a monitor by adapter LUID and target id. Nothing converts one into the
other, so the join is the monitor's device path, which is the identifier
`monitors` already joins the geometry and the registry on. `instance_path` is
reused rather than reimplemented, so the two halves cannot drift.

Two decisions worth stating, because both are the sort of thing that gets
quietly reversed:

- **HDR is reported on every probe, not only on failures.** Someone reading a
  probe wants to know the display is in HDR whether or not it turned out to be
  the cause.
- **Only a definite yes becomes the verdict.** The query returns "on", "off", or
  "could not be read", and the last of those is not evidence of anything.
  Blaming HDR on the strength of a failed query sends someone to turn off a mode
  they are not in while the real cause goes unnamed — the same error as ruling it
  out on the strength of silence. That rule is a function of its own with four
  tests, because it is the part that would be got wrong.

`DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO` is marked deprecated in
recent SDKs in favour of a `_2` form that separates HDR from a wide colour gamut.
It is still answered, and the distinction does not matter for this question.

**Unverified.** This compiles and is tested for the decision rule, on Windows CI.
Nothing here has been run against a Windows machine, let alone one with an HDR
monitor, and the box above stays unticked until it has.

## 24. Scroll the menu bar icon

- [x] A wheel or a trackpad over the icon changes brightness
- [ ] Confirm a real scroll over the icon reaches the agent

Opening a menu to move a slider is three actions for a change that is usually one
step, and the volume and brightness items already in the menu bar have trained
everyone to expect a wheel to work there.

**Read in the pump rather than through a monitor or a subclass.** The two usual
ways to get this are a local event monitor, which means taking on `block2`, or
swizzling the status item button's class to override `scrollWheel:`. Neither is
needed: the agent's pump already takes every event the application is sent and
hands it on, so the one place that sees every scroll already exists. A scroll
over the icon is recognised there and absorbed; everything else passes through
untouched. Intercepting before `sendEvent:` also means this does not depend on
the button doing anything with a scroll, which it does not.

Which window, not which coordinates. A status item's button has a window of its
own, so its number answers "was this over the icon" exactly, and keeps answering
it when the item moves — which it does whenever anything to its right is added or
removed.

A wheel reports notches and a trackpad reports points, and the two differ by two
orders of magnitude; `hasPreciseScrollingDeltas` tells them apart, and treating
them alike would make one of the two useless. Sub-percent movement is accumulated
rather than rounded, because rounding each trackpad event on its own rounds
almost all of them to nothing — which would leave the icon dead for trackpad
users and working for everyone else.

**This is relative where the menu's combined slider is absolute,** and that is
not an inconsistency. A slider is a position, and dragging one to 40% means every
display goes to 40%. A scroll is a nudge from wherever each display already is. A
wheel that flattened two displays someone had balanced by eye onto the same
number would be a surprising thing for a wheel to do. Entry 15's argument for
absolute still holds for the slider and is unchanged.

Verified as far as it goes without a hand on the mouse: the accumulator has three
tests, and the status item's button resolves to a real window at runtime, which
is what the match is against. **What is not verified is that a real scroll over
the icon arrives in the agent's queue carrying that window number** — that needs
somebody to scroll, and the box stays unticked until they have. The failure mode
if it does not is that nothing happens, not that something wrong happens.

## 25. Keep the displays' relative brightness, if that is wanted

- [x] `:combined` in the configuration file, and a tick in the menu
- [x] Relative measured from a baseline rather than accumulated

Entry 15 gave the all-displays slider absolute semantics: every display goes to
the level shown. That argument still holds and the default is unchanged. But it
is a preference rather than a fact, and the other preference is a real one —
somebody who has balanced two panels by eye wants that balance to survive the
slider, and absolute destroys it on the first touch.

So it is a setting, not a change.

**Measured from a baseline, not accumulated.** This is the part that decides
whether relative is any good. Applying each slider step as a delta means a
display that pins at 100 silently swallows the overshoot, so bringing the slider
back where it started does not bring the displays back where they started — the
balance the mode exists to protect is lost by the mode itself. Every display's
level is read when the menu is built, and every target is computed from there, so
saturation is reversible. Six tests, and the one that matters pushes two displays
thirty apart until the brighter pins and then brings them back.

**The configuration file grew a third line shape.** `key = percent` is a level,
`key:name = ...` is a name, and now a key beginning with `:` is a setting. The
colon is what makes all three safe: `DisplayKey`'s contract confines a key to
`A-Z a-z 0-9 . _ -`, so a key can neither contain a colon nor begin with one, and
no escaping is needed anywhere. The default is not written out, so a file
belonging to somebody who has never touched the setting does not grow a line
saying they have not.

The menu row is called "Keep displays' relative brightness" rather than anything
with `absolute` or `relative` in it. Most people have never thought about the
distinction and do not need to in order to know whether they want their displays
to stay as they set them. It is only shown when there is a combined slider to
configure, because a setting whose effect is not on screen is one nobody can
connect to anything.

Verified against the real configuration file: the setting round-trips, an
unreadable value and an unknown setting each produce one complaint and change
nothing.

## 26. v0.3.0

- [x] Tag and release all three platforms

Minor rather than patch: four new behaviours, and the public surface grew. Minor
rather than major even though `Verdict` gained a variant and would break an
exhaustive match downstream — before 1.0 that is what a minor bump is for, and
nothing outside this repository matches on it yet.

What is in it, and what is known about each:

| | verified |
| --- | --- |
| Levels go back after a sleep | plumbing, on this machine; not across a real suspend |
| Scrolling the menu bar icon | the accumulator, and that the status item resolves to a window; not a real scroll |
| `:combined = relative` | round-trips through the real configuration file |
| An HDR verdict in `probe` | the decision rule, on the Windows runner; no HDR hardware |

The release notes say which of those have been run against hardware and which
have not, in the same words, because a release that overstates what has been
tried is worse than one that ships less.

## 27. A page, and an install that needs no undoing

- [x] Lead the macOS release notes with Homebrew
- [x] A page at [wertcore.github.io/klart](https://wertcore.github.io/klart/)

The release notes told people to unzip a bundle and then clear its quarantine
attribute by hand, and did not mention Homebrew at all — even though the tap has
existed since entry 20 and installs through it need no such step.

**No `--no-quarantine` flag, because there is nothing to pass it to.** That
option belongs to casks; klart is a formula, and Homebrew's formula downloader
does not set the attribute. Checked rather than assumed: nothing in
`/opt/homebrew/bin` on this machine carries `com.apple.quarantine`. The attribute
comes from browsers and LaunchServices, which is why a zip fetched from the
releases page needs clearing and a `brew install` does not. The notes now say
that outright, because "why is there no quarantine step here" is a fair question
and a reader should not have to wonder whether it was forgotten.

The page is one static file with no build step, no JavaScript and no fonts to
fetch, served from `docs/` on `main`. It leads with the probe for the reason
entry 22 gave, and it carries the same platform table the release notes do —
including the line saying Linux and Windows have never been run. A landing page
that quietly drops the caveats the rest of the project states is a landing page
that lies.

One thing the page deliberately does not say: `klart autostart on` is offered
only under macOS. On Linux the entry it writes has `Exec` pointing at the command
line rather than an agent, because there is no agent there — so it starts a
process that prints its help and exits. That is a real defect in the Linux
autostart, found while writing this and not fixed here; it has its own box below.

- [ ] Linux `autostart` writes a `.desktop` that runs the command line, which
      exits immediately. It should refuse on a platform with no agent, or say
      what it is for.

## 28. Tell Homebrew users the command that works

- [x] `brew services start klart`, not `klart autostart on`

Entry 27's page told anyone installing through Homebrew to run `klart autostart
on`, and that does not work there. Autostart registers an *application bundle*
with `SMAppService`, and a Homebrew install has no bundle — only the two
binaries out of it. Run that way it answers `unavailable — this is not running
from the app bundle`, which reads as a broken program rather than the wrong
command.

The formula has said so in its own `caveats` since entry 20 and has carried a
launchd `service` block for exactly this. The page contradicted the formula it
was recommending.

Checked rather than assumed, because the obvious fix is wrong too: installing the
bundle into the Homebrew prefix and pointing `bin/klart` at the binary inside it
does **not** help. `NSBundle` resolves the main bundle from the path the
executable was invoked by, not from where the symlink lands, so a
`bin/klart -> …/Klart.app/Contents/MacOS/klart` symlink still reports no bundle.
Built and run both ways to confirm: through the symlink, `unavailable`; from
inside the bundle, `off`.

### On quarantine, once, so it stops coming up

- A **formula** is never quarantined. Nothing in the formula install path applies
  the attribute, and nothing in `/opt/homebrew/bin` on this machine carries it.
- A **cask** still is. `Quarantine.cask!` sets it with the agent name "Homebrew
  Cask", and `--no-quarantine` was deprecated in October 2025 and removed in July
  2026. Moving the bundle to a cask to avoid `xattr` would do the opposite.
- The **archive** is quarantined because a browser put it there, not because of
  anything in this project.

So `xattr` is needed on exactly one path, and the way to spare people is to make
Homebrew the path they take — not to add a flag. Removing it from the archive
path as well needs the bundle signed and notarised, which needs an Apple
Developer account.

- [ ] Sign and notarise the macOS bundle, if an Apple Developer account is ever
      worth its fee here. That, and only that, removes the `xattr` step for
      someone who downloads the zip.

## 29. Right-click and Open has not worked since macOS 15

- [x] Say what actually clears Gatekeeper for the archive

The release notes told anyone using the zip to "right-click it and choose Open".
Apple removed that bypass for unsigned applications in macOS 15, so on any Mac
new enough to be reading this it does nothing. The route now is System Settings →
Privacy & Security, where an **Open Anyway** button appears after the first
refused launch — or `xattr -dr`, which was already there and still works.

Worth stating alongside it, and now stated: quarantine is a tag rather than a
ban. Gatekeeper reads it and then decides based on the signature. A notarised
application is quarantined too and simply opens; klart's bundle is refused
because it is unsigned, not because of the attribute's presence. That distinction
is what makes `--no-quarantine`'s removal from Homebrew a non-event for everyone
whose casks are notarised, which is nearly everyone — and Homebrew replaced its
one real use, re-approving on every upgrade, with inheriting the approval when
the signing identity has not changed.

None of it reaches klart, which is a formula: that path never sets the attribute,
and the tap's CI installs and runs it on a clean macOS runner with no approval
step anywhere.
