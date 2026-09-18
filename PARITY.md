# Parity

What the established tools in this space do, what `klart` does, and which of the
differences are worth closing.

Compiled 18 September 2026 from each project's own documentation, linked at the
bottom. Where a claim could not be read off a primary source it is marked
`unknown` rather than guessed at.

| | reads |
| --- | --- |
| **yes** | has it |
| **part** | has it with a caveat, noted underneath |
| **—** | does not have it |
| **n/a** | does not apply to that tool |

The comparators:

- **MonitorControl** — macOS, open source, free. The closest thing to a direct
  peer.
- **Lunar** — macOS, closed source, freemium (Pro is a $23 lifetime licence).
  The most featureful of any of them.
- **Monitorian** — Windows, open source, free. The Windows peer, and the one
  `klart`'s planned Windows port would be measured against.
- **ddcutil** — Linux, open source, free. A DDC/CI toolkit rather than a
  brightness applet, and the reference for the planned Linux port.

## Reaching the display

| | klart | MonitorControl | Lunar | Monitorian | ddcutil |
| --- | --- | --- | --- | --- | --- |
| Built-in panel | yes | yes | yes | part ¹ | — ² |
| External over DDC/CI | part ³ | yes | yes | yes | yes |
| Gamma / software dimming | yes | yes | yes | — | n/a |
| Overlay "shade" for virtual screens ⁴ | — | yes | part | — | n/a |
| Combined hardware + software dimming ⁵ | — | yes | yes | — | n/a |
| Beyond 100% on XDR panels | — | — | yes (Pro) | n/a | n/a |
| Relay for links that refuse DDC ⁶ | — | — | yes | — | — |

¹ Monitorian's internal-display support "depends on hardware capability".
² ddcutil cannot drive laptop panels at all: they "use a special API, not I2C".
³ Written and unit tested; no monitor has ever answered one. See below.
⁴ AirPlay, Sidecar and DisplayLink screens have no gamma table to bend, so the
  only way to dim them is to lay a dark window over them.
⁵ Continuing to dim with the gamma ramp once the backlight is already at its
  minimum. Both macOS peers treat this as a headline feature, and it is the one
  most obviously missing here.
⁶ Lunar can route DDC through a Raspberry Pi on the same network when the Mac's
  own link will not carry it — the situation this repository is currently in.

## What else can be controlled

| | klart | MonitorControl | Lunar | Monitorian | ddcutil |
| --- | --- | --- | --- | --- | --- |
| Brightness | yes | yes | yes | yes | yes |
| Contrast | — | yes | yes | yes | yes |
| Volume / mute | — | yes | yes | — | yes |
| Input switching | — | — | yes | — | yes |
| Monitor power | — | — | yes (Pro) | — | yes |
| Arbitrary VCP features | — | — | yes | — | yes |

`klart`'s DDC layer already speaks arbitrary VCP codes — `ddc::read_feature` and
`set_request` both take the feature as a parameter — so contrast and volume are a
few lines each. They are absent because nothing here can verify them.

## Driving it

| | klart | MonitorControl | Lunar | Monitorian |
| --- | --- | --- | --- | --- |
| Menu bar / tray slider | yes | yes | yes | yes |
| One slider for every display | — | yes | yes | yes |
| The Mac's own brightness keys | — | yes ⁷ | yes ⁷ | n/a |
| Custom hotkeys | — | yes | yes | part ⁸ |
| Native OSD | — | yes | yes | n/a |
| Scroll wheel or trackpad on the slider | — | unknown | unknown | yes |
| Renaming a display | — | unknown | yes | yes |
| A settings window | — | yes | yes | yes |
| Launch at login | — | yes | yes | yes |
| Command line | yes | — | yes | yes |
| Machine-readable output | yes | n/a | unknown | unknown |

⁷ Requires Accessibility permission, which is what plan entry 10 is stuck on.
  That both macOS peers pay that cost is itself evidence about which way to go.
⁸ An optional add-on rather than part of the application.

## Deciding for itself

| | klart | MonitorControl | Lunar | Monitorian |
| --- | --- | --- | --- | --- |
| Remembers levels per display | yes | yes | yes | yes |
| Identity survives a reconnect | yes ⁹ | unknown | yes | part ¹⁰ |
| Mirrors the built-in panel's ambient adaptation | — | yes | yes (Pro) | — |
| Ambient light sensor | — | part | yes (Pro) | part ¹¹ |
| Time or sun based schedules | — | — | yes (Pro) | part ¹² |
| Per-application presets | — | — | yes (Pro) | — |
| Shortcuts / automation integration | — | — | yes (Pro) | part ¹² |

⁹ Derived from EDID and written down as seven numbered rules with a test each,
  so that the same display keys the same on another operating system.
¹⁰ Monitorian notes its identification "relies on OS-assigned unique
  identifiers, which may vary by connection type" — the exact problem entry 2
  set out to avoid.
¹¹ Displays sensor readings; does not act on them.
¹² Through Windows Task Scheduler rather than internally.

## Where klart is not behind

Worth writing down, because everything above is a deficit and that is not the
whole picture.

- **It is the only one of the four designed to be cross-platform.** Each of the
  others is one operating system's tool. `klart`'s display key is specified so
  that a configuration file written on macOS names the same monitors on Windows,
  which is a thing none of the others can do because none of them has to.
- **It needs no permissions.** Nothing here asks for Accessibility, and nothing
  here silently does nothing until a dialog is answered. Both macOS peers require
  it for their best feature.
- **The agent is 405 KB and idles at nothing measurable.** Not compared, because
  the others' figures were not measured here.
- **The command line is a first-class surface**, not an add-on or a paid tier.
  MonitorControl has none; Monitorian's is a separate download; Lunar's is free
  but the application around it is not.

## The gaps, ordered

### Worth doing, and verifiable without an external monitor

1. **Combined hardware and software dimming.** Once the backlight is at its
   minimum, keep going with the gamma ramp. Both macOS peers lead with this, a
   MacBook panel at its dimmest is still too bright in a dark room, and the whole
   of it can be verified on the built-in display. `Backend` would grow a notion
   of a floor and `Control` would compose two mechanisms rather than choose one.
2. **One slider for every display at once.** Every comparator has it. Trivial
   against the existing menu.
3. **Launch at login.** `SMAppService` on macOS 13 and later; a bundle already
   exists for it to register.
4. **Renaming a display.** The remembered-levels file is already a per-display
   store keyed properly; a name is one more field.

### Worth doing, but not verifiable here

5. **Contrast and volume over DDC.** A few lines each on a protocol layer that
   already takes the feature code as a parameter — and untestable until a monitor
   answers a DDC request.
6. **Overlay dimming for AirPlay, Sidecar and DisplayLink screens.** A fourth
   mechanism below gamma, because a virtual screen has no ramp to bend. Needs one
   of those screens to develop against.

### Needs a decision rather than an implementation

7. **The Mac's own brightness keys.** Plan entry 10. Both macOS peers pay the
   Accessibility cost, which is the strongest evidence available that the
   permission is worth asking for.
8. **The native OSD.** Every macOS peer shows the system brightness overlay.
   Doing it means `OSDUIHelper`, which is private, undocumented and has moved
   between releases — a third private interface to maintain.

### Deliberately not

9. **Ambient, scheduled and per-application adaptation.** Lunar's territory, and
   most of it is why Lunar is worth paying for. A tool that dims a monitor when
   asked is a different product from one that decides when.
10. **A network relay for links that will not carry DDC.** Lunar solves this with
    a Raspberry Pi. It is the correct answer and it is an enormous amount of
    machinery for a case a different cable also fixes.
11. **Input switching and monitor power.** `ddcutil` already does the whole VCP
    surface on Linux, and doing a worse job of it here is not worth the menu
    space. A `klart vcp` escape hatch would be a better answer than menu items,
    if anyone asks.

## On the DDC gap specifically

Plan entry 4's two unticked boxes are still unticked, and the comparators explain
why more precisely than this repository could on its own.

The well-documented Apple silicon limitation is that the **built-in HDMI port** on
M1-generation Macs — the Mac mini, Mac Studio and the 14- and 16-inch MacBook Pro
— does not pass DDC/CI. MonitorControl and Lunar both document it, and both give
the same advice: connect over USB-C or DisplayPort instead.

That is adjacent to, but not the same as, what was measured here. This machine is
an M2 Air, which has no HDMI port at all, and the display is on a USB-C cable that
terminates in HDMI — so the DisplayPort-to-HDMI conversion happens inside the
cable. Every DDC write is refused locally by the display coprocessor while reads
on the same channel succeed, which points at the converter rather than at the
port.

Either way the remedy is the one both peers give: a link with no HDMI conversion
in it. USB-C to DisplayPort, into the monitor's DisplayPort input.

## Sources

- [MonitorControl](https://github.com/MonitorControl/MonitorControl)
- [Lunar](https://lunar.fyi/)
- [Monitorian](https://github.com/emoacht/Monitorian)
- [ddcutil](https://www.ddcutil.com/)
- [MonitorControl: monitor troubleshooting](https://github.com/MonitorControl/MonitorControl/wiki/Monitor-Troubleshooting)
- [The journey to controlling external monitors on M1 Macs](https://alinpanaitiu.com/blog/journey-to-ddc-on-m1-macs/)
