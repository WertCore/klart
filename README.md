# klart

Brightness control for every display attached to a Mac — the built-in panel and
external monitors alike, from the menu bar or the command line.

macOS dims the built-in display from the keyboard and leaves every other monitor
to its own on-screen buttons. `klart` puts all of them on one control.

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
```

Commands act on the display holding the menu bar unless told otherwise.
`--display` takes an index, a key, or part of a name.

A `*` on the mechanism means the change does not outlive the command — see the
gamma row of the table above.

## In the menu bar

`klart-tray` puts a sun in the menu bar with a slider per display. It is an
agent: no Dock tile, no window.

It is also the only way to hold a display dimmed that has no hardware brightness
control, because macOS reverts a gamma ramp when the process that set it exits.

## Status

Early. [`PLAN.md`](PLAN.md) tracks what is built and what is not, one entry per
change; nothing is ticked there until it has run against real hardware.

## Building

```sh
cargo build --release
```

The toolchain is pinned in `rust-toolchain.toml`, so `rustup` will fetch the
right one on first build.

## Licence

MIT or Apache-2.0, at your option.
