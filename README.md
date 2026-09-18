# klart

Brightness control for every display attached to a Mac — the built-in panel and
external monitors alike, from the menu bar or the command line.

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
| `HdrInTheWay` | The display is in HDR, and brightness control did not answer. Those go together: an HDR picture mode commonly pins brightness or stops honouring the feature. Windows only. | Turn HDR off, or set the level in the monitor's own menu. Software dimming is not a way round it — Windows does not guarantee gamma ramp behaviour under HDR either. |
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
compiled and tested by CI on their own runners, and have never been run — the
boxes that need a machine are unticked in [`PLAN.md`](PLAN.md).

The menu bar agent is macOS only. A tray elsewhere is StatusNotifierItem over
D-Bus or `Shell_NotifyIcon`, which is a new crate rather than a `cfg`.

`DisplayKey` is portable by contract — its derivation is seven numbered rules,
each with a test, over EDID fields every operating system can read. A
configuration written on one is meant to be readable on another.

## Why this is three mechanisms rather than one

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
$ klart autostart on # start the menu bar agent at login
$ klart rename 1 "Desk monitor"
```

Commands act on the display holding the menu bar unless told otherwise.
`--display` takes an index, a key, or part of a name.

A `*` on the mechanism means the change does not outlive the command — see the
gamma row of the table above.

Levels are remembered in `~/Library/Application Support/klart/levels.conf`, keyed
by display rather than by port, so they survive a reconnect. The file is meant to
be edited by hand.

## In the menu bar

`klart-tray` puts a sun in the menu bar with a slider per display. It is an
agent: no Dock tile, no window.

It is also the only way to hold a display dimmed that has no hardware brightness
control, because macOS reverts a gamma ramp when the process that set it exits —
which is why `klart autostart on` is worth setting if you have such a display.
Without it, that display is back at full brightness after every restart.

The agent puts levels back after a sleep. Plenty of monitors come back at full
brightness of their own accord, and a write sent the instant the machine wakes is
accepted and dropped — the link returns before the panel behind it does. So the
agent writes, reads back, and keeps trying for a few seconds until the display
agrees. A display that never agrees is reported once and left alone.

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

```
klart.exe list
```

Unsigned, so SmartScreen warns on first run.

## Building

```sh
cargo build --release      # the two binaries
scripts/bundle.sh          # dist/Klart.app
```

The toolchain is pinned in `rust-toolchain.toml`, so `rustup` will fetch the
right one on first build.

## Licence

MIT or Apache-2.0, at your option.
