# Hello GUI: Testing the Vertical Slice

`layer36-hello-gui` is the first GUI component: one portable `.wasm` file
that opens a real native window with a real native button and text field.
This page is the test manual for it.

## The one-command demo

```bash
sh scripts/demo-hello-gui.sh
```

On **macOS** this opens a native window front and center. Click the
**"Click me"** button within 30 seconds: the text field flips to
**"clicked!"**, the window closes itself a second later, and the script
reports the result. On **Linux and Windows** the same portable file runs
headless for about two seconds (those window backends are the next
milestone), which still exercises the full component → runtime → permission
→ adapter pipeline.

## Exit codes are the test assertions

The app reports what it observed, so scripts and CI can assert behavior:

| Exit | Meaning |
|---|---|
| `0` | A click on the native button reached the component (the full round trip) |
| `1` | Clean bounded run with no click — the normal headless outcome |
| `2` | The user closed the window before clicking |
| `30`–`32` | Window creation, show, or widget-tree calls failed |

## Manual commands

Interactive native window (macOS):

```bash
target/debug/layer36 run --auto-grant --native-window \
  --manifest apps/layer36-hello-gui/manifest.toml \
  apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm
```

Headless, fast, any OS (the `quick` app arg shortens the wait loop):

```bash
target/debug/layer36 run --auto-grant \
  --manifest apps/layer36-hello-gui/manifest.toml \
  apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm \
  -- quick
```

Machine-readable report of the same run (schema `layer36.run.v1`):

```bash
target/debug/layer36 run --json --auto-grant \
  --manifest apps/layer36-hello-gui/manifest.toml \
  apps/layer36-hello-gui/target/wasm32-wasip1/release/layer36_hello_gui.wasm \
  -- quick
```

Automated regression smoke (builds, checks import purity, asserts the
headless exit):

```bash
sh scripts/smoke-phase3-gui-app.sh
```

## How the three-OS claim is proven today

Honestly, in two layers:

1. **The artifact is proven portable on all three OSes on every full CI
   run.** The hosted full-test matrix builds the hello-gui fixture once,
   then runs the byte-identical file headless on Linux, macOS, and Windows
   and asserts the clean bounded exit. Same bytes, three operating systems,
   machine-checked.
2. **The visible window is proven on macOS only**, by a human clicking the
   native button (exit `0`), because macOS (AppKit) is the only native
   window backend so far. The Linux and Windows winit backends are the next
   Phase 3 milestone; when they land, layer 1's runs graduate into visible
   windows there too — the component itself will not change at all.

That last sentence is the point of the whole design: the app is already
finished for all three OSes. Only host adapters remain.

## What to look for when it misbehaves

- **No window appears on macOS**: the process must promote itself to a
  regular app and pump the NSApplication event queue — both were real bugs
  caught by a human eye and fixed in the adapter; if a regression appears,
  check `show_window` (activation policy + activate) and
  `pump_app_events` in `crates/adapter-macos/src/appkit.rs`.
- **Window appears but clicks do nothing**: the event-queue pump is not
  running — same place.
- **Exit `32` headless**: a widget-tree call failed; run with `--json` to
  see the classified error.
