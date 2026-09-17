# Plan

One entry per pull request. A box is ticked only once the change has run against
real hardware — a green test suite on a headless runner is not evidence that a
monitor dimmed, because a headless runner has no monitor.

## 1. Scaffold the workspace

- [x] Cargo workspace, pinned toolchain, `rustfmt` settings, dual licence
- [x] CI on macOS: format, clippy at `-D warnings`, tests, docs at `-D warnings`
- [x] `Brightness`, the fraction every backend converts to and from

## 2. Find the displays

- [ ] Enumerate active displays through `CGGetActiveDisplayList`
- [ ] Per display: vendor, model, serial, built-in flag, bounds, main flag
- [ ] Readable names out of the IORegistry, since Core Graphics has none
- [ ] A stable key per display, so configuration survives a reconnect and a
      `CGDirectDisplayID` that is not stable across one

## 3. The built-in panel

- [ ] `BrightnessBackend`, the trait the other two entries implement
- [ ] `DisplayServices` bound at run time through `dlopen`, so a macOS release
      that drops the symbol is a clear error rather than a failure to launch
- [ ] Get and set on the built-in display

## 4. External monitors over DDC/CI

- [ ] `IOAVService` bound the same way
- [ ] Match each `CGDirectDisplayID` to its IORegistry node — the two namespaces
      have no common identifier, so this goes through the product attributes
- [ ] VCP `0x10` get and set, with the checksums, the reply validation and the
      inter-message delays that DDC needs to be reliable
- [ ] Report the monitor's own maximum rather than assuming one

## 5. The gamma fallback, and choosing between the three

- [ ] Software dimming through `CGSetDisplayTransferByFormula`
- [ ] Restore the ramp on exit, including on a signal — a process that dies
      holding a dark ramp leaves the display dark until the user logs out
- [ ] Per-display backend resolution: built-in, else DDC, else gamma

## 6. The command line

- [ ] `list`, `get`, `set`, `up`, `down`
- [ ] `--display` by index, name or key; `--all`
- [ ] `--json`, so it composes with something else

## 7. The menu bar

- [ ] A `tray-icon` agent under `NSApplicationActivationPolicyAccessory`, so it
      has no Dock tile and no window
- [ ] One submenu per display: presets, a step up and a step down
- [ ] Re-enumerate when a display is plugged or unplugged

## 8. Remember the levels

- [ ] Per-display levels in `~/Library/Application Support/klart`
- [ ] Restore on launch and on reconnect, keyed by the stable key from entry 2
- [ ] Global hotkeys for brighter and dimmer across every display at once

## 9. Ship it

- [ ] A `Klart.app` bundle with `LSUIElement`, so the agent starts without a
      Dock tile
- [ ] A release workflow that builds and attaches it
