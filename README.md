# klart

Brightness control for every display attached to the machine — the built-in
panel and external monitors alike, from the menu bar or the command line.

[wertcore.github.io/klart](https://wertcore.github.io/klart/)

macOS dims the built-in display from the keyboard and leaves every other monitor
to its own on-screen buttons. `klart` puts all of them on one control.

And when a monitor will not take one, it tells you why.

## When a monitor will not answer

Some monitors cannot be dimmed over the wire, and the usual answer is a list of
things to try: another cable, another port, a setting in the on-screen menu, a
different application. `klart probe` measures instead.

```
$ klart probe
LS32AG55x (SAM-71e3-HNAW900001)
  Core Graphics id 2
  registry node    matched
  link             DP -> HDMI
  I2C channel      published
  ok               read 128 bytes at the EDID address (0x50): valid EDID header, version 1.3
  failed           read the same 128 bytes at the DDC/CI address (0x37): byte for byte identical to the EDID address — the chip address is being ignored
  failed           DDC/CI Get VCP 0x10 (chip 0x37, offset 0x51): 0xe0114102 — sub_iokit_audio_video(0x45), code 258

  EdidOnly
  This link serves a cached copy of the monitor's EDID and does no I2C:
  reads return the same bytes whichever address they ask for, and every
  write is refused. Nothing here ever reaches the monitor, so this is not
  the monitor's DDC/CI setting and no software can change it — the
  monitor is never asked. The display coprocessor behaves this way when it
  cannot run I2C over the link, which is what a DisplayPort-to-HDMI
  conversion inside a cable or adaptor causes. Use a link with no
  conversion in it: USB-C to DisplayPort, into the monitor's DisplayPort
  input.

Built-in Display (builtin)
  ...
  ok               read the same 128 bytes at the DDC/CI address (0x37): differs from the EDID address, so the address is honoured
  ...

  NotApplicable
  Nothing — the built-in panel has its own mechanism and does not use
  DDC/CI.

Corroborated on this machine: Built-in Display honoured the I2C chip
address and LS32AG55x ignored it, through the same calls in the same
process. The difference is the link, not the interface.
```

Every line above is something that was attempted, with what came back. The
verdict at the end is derived from those attempts and nothing else, and it
distinguishes the cases that otherwise all present as "brightness does not
work":

| Verdict | What it means | What to do |
| --- | --- | --- |
| `Answers` | DDC/CI is working on this display. | Nothing. |
| `MonitorDeclines` | Two addresses answered differently, so transactions are reaching the monitor — and it is refusing DDC/CI itself. Most ship with it off. | Turn on DDC/CI, Monitor Control, External Control or PC Control in the monitor's own menu. |
| `HdrInTheWay` | The display is in HDR, and brightness control did not answer. Those go together: an HDR picture mode commonly pins brightness or stops honouring the feature. Windows only. | Turn HDR off to regain the backlight, or set the level in the monitor's own menu. klart's HDR fallback is a black overlay below the hardware floor: it can darken the desktop, not raise a pinned brightness. |
| `EdidOnly` | The link serves a cached EDID and does no I2C. The monitor is never asked, so this is *not* its DDC/CI setting and no software can change it. | Use a link with no protocol conversion: USB-C to DisplayPort, into the monitor's DisplayPort input. |
| `NoI2c` | Nothing could be read over this link at all, not even the EDID. Something between the Mac and the monitor is not passing I2C. | A different cable, preferably with no conversion in it. |
| `NoChannel` | No I2C channel on this machine at all — what a virtual screen looks like: AirPlay, Sidecar, DisplayLink. None carry DDC/CI by design. | Software dimming is the only option, and `klart` falls back to it. |
| `NotApplicable` | The built-in panel, which has its own mechanism. | Nothing. |
| `Unclear` | The attempts do not fit a known pattern. | The attempts above are the evidence; the `IOReturn` codes are worth an issue. |

The last line of the output is the control, and it is the part that makes the
verdict a measurement rather than a guess. Two displays, the same calls, the
same process: if one honours the I2C chip address and the other ignores it, the
interface works and the difference is the link. Without that comparison,
`EdidOnly` would be an assertion.

## Ports

macOS runs and is tested on real hardware. Linux and Windows are written,
compiled and tested by CI on their own runners; the Windows agent has been run
on one machine, and its drag, below-minimum dimming and recovery were checked
there. Linux has not been run at all — the boxes that need a machine are
unticked in [`PLAN.md`](PLAN.md).

There are two tray agents, and they are separate programs rather than one with
`cfg` branches: `klart-tray` is AppKit, and `klart-tray-windows` is
`Shell_NotifyIcon` with a hand-drawn panel instead of a menu, because a Windows
menu cannot hold a slider that is dragged. What they do share is
`klart-tray-common` — the write rate limiting, the remembered levels and the
restore after a wake — so those cannot drift apart between the platforms. The
three crates live under `tray/`: `common`, `macos` and `windows`, with the
package and binary names unchanged. A tray for Linux is still StatusNotifierItem
over D-Bus or the legacy XEmbed protocol, and is a third crate rather than a
variant of either.

`DisplayKey` is portable by contract — its derivation is seven numbered rules,
each with a test, over EDID fields every operating system can read. A
configuration written on one is meant to be readable on another.

## Why macOS is three mechanisms rather than one

There is no single brightness API on macOS, so there is no single backend here.

| Display | Mechanism | Notes |
| --- | --- | --- |
| Built-in panel | `DisplayServices` private framework | The only path that works on Apple silicon. IOKit's `IODisplaySetFloatParameter` is documented for this and does nothing on these machines. |
| External, speaks DDC/CI | `IOAVServiceReadI2C` / `IOAVServiceWriteI2C`, VCP feature `0x10` | Real backlight control, at the pace the monitor's I2C link will take it. |
| Everything else | `CGSetDisplayTransferByFormula` | A gamma ramp. It darkens the picture rather than the backlight, so contrast suffers, and macOS reverts it the moment the process that set it exits — so it holds only while `klart` is running. A fallback for displays behind adaptors that do not carry DDC, not a peer of the other two. |

Both hardware paths are private, undocumented Apple interfaces. There is no
supported alternative that reaches an external monitor's backlight, and this is
what every tool in this space uses, but it is worth knowing before depending on
it: a macOS release can take either away.

Windows has the same shape with public APIs: the Monitor Configuration API for
external monitors, WMI for the machine's own panel, and a software half — a
gamma ramp in SDR, an overlay in HDR — for the range below the backlight. There
is no `DisplayServices` equivalent to worry about, and the driver does the
DDC/CI framing itself.

## Dimming below the backlight

A backlight has a minimum, and in a dark room that minimum can still be too
bright. Below it klart holds the backlight at that minimum and dims the picture
instead. The bottom quarter of every slider is that software range; above it the
backlight does the work and the picture is left alone. At exactly 25% the two
halves meet at the same display state, so crossing the seam does not jump.

The software half is a gamma ramp when Windows is definitely in SDR. It is a
copy of the ramp that was already there, scaled down, so an ICC calibration or a
Night Light curve is preserved rather than replaced. In HDR — and when HDR
cannot be read at all — Windows does not promise what a gamma ramp does, so
klart uses a click-through black window over the display instead. That overlay
can only darken what is composited into the desktop, so an exclusive-fullscreen
game can sit outside it, and it is not a way to make an HDR picture brighter.

A Windows gamma ramp outlives the command that set it. Before the first scaled
write, klart saves the original in `%APPDATA%\klart\gamma.conf`, keyed by
display; a crash can be recovered with `klart set 100`, and the file is removed
when the ramp is given back. The software floor is 20% of the picture, so even a
process that dies below the seam cannot leave a black screen. Overlay windows
vanish with the process that owns them.

## Using it

```
$ klart list
IDX  LEVEL  MECHANISM        DISPLAY                  KEY
  0    44%  DisplayServices  Built-in Display (main)  builtin
  1   100%  gamma*           LS32AG55x                SAM-71e3-HNAW900001
       DisplayServices cannot reach display SAM-71e3-HNAW900001
       DDC/CI cannot reach display SAM-71e3-HNAW900001

$ klart set 60
$ klart down 10 --display LS32
$ klart up --all
$ klart get --json
$ klart restore
$ klart probe        # why won't this monitor answer? (see above)
$ klart probe --json # the same, for pasting into an issue
$ klart autostart on # start the menu bar agent at login
$ klart rename 1 "Desk monitor"
```

Commands act on the main display — the one with the menu bar, on macOS — unless told otherwise.
`--display` takes an index, a key, or part of a name.

A `*` on the mechanism means the change does not outlive the command. On macOS
that is a gamma ramp, which the window server puts back; on Windows it is an HDR
overlay window, which goes away with the process.

Levels are remembered in `~/Library/Application Support/klart/levels.conf` on
macOS and `%APPDATA%\klart\levels.conf` on Windows, keyed by display rather than
by port, so they survive a reconnect. The file is meant to be edited by hand. A
key beginning with `:` is a setting rather than a display;
`:combined` is the only one, and it chooses what the menu's all-displays slider
does:

```
:combined = absolute   # the default: every display goes to the level shown
:combined = relative   # every display moves by the same amount from where it was
```

`relative` keeps a balance set between two displays. It measures from where they
were when the menu opened rather than accumulating, so a display that pins at one
end comes back to where it started instead of quietly losing the difference. The
menu has a tick for it.

## In the menu bar

`klart-tray` puts a sun in the menu bar with a slider per display. It is an
agent: no Dock tile, no window.

It is also the only way to hold a display dimmed that has no hardware brightness
control, because macOS reverts a gamma ramp when the process that set it exits —
which is why `klart autostart on` is worth setting if you have such a display.
Without it, that display is back at full brightness after every restart.

Scrolling over the icon changes brightness without opening anything. A wheel
notch is a step; a trackpad is continuous. It moves every display by the same
amount from wherever each of them is, rather than setting them all to one number
— so two displays balanced against each other stay balanced.

The agent puts levels back after a sleep. Plenty of monitors come back at full
brightness of their own accord, and a write sent the instant the machine wakes is
accepted and dropped — the link returns before the panel behind it does. So the
agent writes, reads back, and keeps trying for a few seconds until the display
agrees. A display that never agrees is reported once and left alone.

## In the notification area

`klart-tray.exe` is the Windows agent. It puts a sun in the notification area —
beside the sound icon on Windows 11 — in the same white or black as the system's
own icons, and redraws it when Windows mode changes. Left or right clicking it
opens a panel: a bar per display, one for all of them when there is more than
one, then the rows for keeping their relative levels, starting at login, looking
for displays again, resetting software dimming, and quitting. A wheel over a bar
moves it five percent — the wheel is handled by the panel rather than by the
icon, because Windows does not forward wheel messages to notification icons.

A bar with a hardware and software half has a tick at 25% and a note saying that
below it the picture is dimmed rather than the backlight. That lower range is
held by a gamma ramp in SDR or by an overlay in HDR; the overlay sits below this
panel while it is open, so opening the panel never hides the control under the
dimming. Quitting releases the software half, leaving the backlight at its
floor, and the next start restores the remembered level. “Reset software
dimming” does the same immediately without quitting, and remembers the resulting
25% level so the next start does not put the darker level straight back.

Windows puts a new notification icon in the overflow area by design, and no
documented API overrides that. On Windows 11 22H2 and later the choice is stored
per icon, so klart asks to be shown beside the clock on its first run — and only
then: if the icon has been hidden in Settings, that choice is left alone.
Windows 10 keeps the icon in the overflow until it is dragged out, which is what
that system asks of every application.

The panel is drawn rather than assembled from controls. A trackbar is a Windows
control with its own idea of a theme, and on a hand-coloured popup it reads as a
trackbar rather than as the bar the menu bar agent draws; drawing it also means
a drag is one mouse move and a new fill width. Windows 11 rounds its corners
through DWM, and Windows 10 draws it square, the way its own flyouts are.

A drag repaints before it asks for anything, and the write is made by a
background thread. A DDC/CI write takes about two hundred milliseconds on the
monitor this was written against: fine for a display, not fine for a pointer.
The thread keeps only the newest value for each display, so a fast drag skips
the positions already left behind instead of replaying them.

It is an agent in the same sense as the macOS one: no window, no taskbar button,
and it stays running because it has to hold the software half of a below-seam
level and put remembered levels back. An overlay disappears when the process
exits; a Windows gamma ramp would remain, so the agent deliberately gives it
back on the way out. The sleep restore above is the same code, from
`klart-tray-common`.

## Status

Early. [`PLAN.md`](PLAN.md) tracks what is built and what is not, one entry per
change; nothing is ticked there until it has run against real hardware.

## Installing

### Homebrew

```sh
brew install wertcore/tap/klart
```

macOS on Apple silicon and Linux on x86_64. That installs the `klart` command
line, and on macOS the `klart-tray` agent with it:

```sh
brew services start klart
```

Use that rather than `klart autostart` for a Homebrew install — that command
registers an application bundle, and there is no bundle in a Homebrew install.

### The archives

Every platform, including Windows, is on the
[releases page](https://github.com/WertCore/klart/releases).

#### macOS

Unzip and move `Klart.app` to `/Applications`.

It is not signed or notarised, so macOS refuses the first launch of anything that
arrived through a browser. Right-click it and choose Open, or:

```sh
xattr -dr com.apple.quarantine /Applications/Klart.app
```

The command line rides along inside the bundle:

```sh
ln -s /Applications/Klart.app/Contents/MacOS/klart /usr/local/bin/klart
```

Apple silicon only. An Intel Mac would enumerate its displays and reach neither
hardware mechanism, because the registry node both of them are found through is
one Intel Macs do not publish.

#### Linux

```sh
tar xzf klart-*-linux-x86_64.tar.gz
./klart list
```

Two permissions it will want, and neither is this program's to grant: external
monitors need read and write on `/dev/i2c-*`, usually through the `i2c` group or
a udev rule, and the laptop panel needs write on
`/sys/class/backlight/*/brightness`. `klart probe` says which mechanism reached
which display and why the others did not.

#### Windows

Unzip both files somewhere permanent.

```
klart.exe list      # the command line
klart-tray.exe      # the agent; "Start at login" keeps it there
```

The agent sits in the notification area and opens the panel described above.
Both binaries are unsigned, so SmartScreen warns on first run.

## Building

```sh
# macOS
cargo build --release -p klart-cli -p klart-tray
scripts/bundle.sh

# Windows
cargo build --release -p klart-cli -p klart-tray-windows
```

The toolchain is pinned in `rust-toolchain.toml`, so `rustup` will fetch the
right one on first build. Name the agent for the platform: `klart-tray` is
AppKit and `klart-tray-windows` is Win32. A bare `cargo build --release` tries
every workspace member and stops in the agent for the other platform, which says
so with a `compile_error!`.

## Licence

Apache-2.0.
