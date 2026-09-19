# Krate Founder OS

The visual Founder OS packaged as one `.krate` application.

## Open it

Double-click:

```text
Plan/founder-os/Krate-Founder-OS.krate
```

Or run it from the repository:

```sh
target/debug/krate run Plan/founder-os/Krate-Founder-OS.krate --prompt
```

Grant `ui.window:create` and `store.kv`. The store grant keeps status changes
and tasks added inside the application under the stable app id
`dev.krate.founder-os`.

## What works

- The current 41 tasks are generated from `Plan/founder-os/tasks.json` when the
  app is built.
- Board view shows Locked, Ready, and Waiting tasks. Wheel inside one column to
  move through it.
- Click a card to inspect its outcome, lane, status, and direct dependencies.
- Flow view shows the selected task's prerequisites and the work it directly
  unlocks.
- Lock respects the one-Build-task and one-Company-task focus limits.
- Complete recalculates dependent tasks; blocked work becomes ready only when
  every recorded prerequisite is complete.
- Park removes work from the active board without deleting its record.
- New tasks can be independent or depend on the selected task. Choose Build or
  Company before adding them.

## Current boundary

This `.krate` keeps its working state in Krate's per-app key-value store. It
does not write changes back into `Plan/founder-os/tasks.json`; the current
runtime has no wired user-selected-file picker for giving a portable app an
ongoing external JSON document safely. Rebuilding the app refreshes its seed
from that JSON file while retaining compatible saved status changes.

Krate's current pointer event reports presses and releases but not pointer
movement. That makes click-to-inspect and explicit Lock, Complete, and Park
buttons real; hover details and freeform card dragging would be fake, so they
remain future runtime work.

## Build and package

```sh
cd apps/krate-founder-os
rustup run 1.94.1 cargo component build --release --target wasm32-wasip1
../../target/debug/krate pack \
  target/wasm32-wasip1/release/krate_founder_os.wasm \
  --manifest manifest.toml \
  --output ../../Plan/founder-os/Krate-Founder-OS.krate
```

The package includes its source so the `.krate` remains editable.
