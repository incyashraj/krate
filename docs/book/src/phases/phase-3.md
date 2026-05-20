# Phase 3: UI + Graphics

**Status:** Started
**Estimate:** est. 6 to 10 weeks
**Goal:** Run one windowed app on Windows, macOS, and Linux.

Phase 3 has started with the contract layer. That means we are defining what a
desktop app is allowed to ask Layer36 for before we build the host adapters and
sample app behind it.

Phase 2 is still waiting on the outside developer review before we call it
formally closed. The engineering proof is strong enough to begin the Phase 3
draft contracts, as long as Phase 2's existing API remains unchanged.

Phase 3 adds the first visual app surface:

- windows
- layout
- buttons and text
- keyboard and pointer input
- 2D drawing
- a small notes app

The target is not a web view hidden in a desktop shell. The target is a Layer36
app that feels close to native on each desktop host.

## Current Slice

The first Phase 3 slice is now in the repo:

- `layer36:app@0.2.0` with a `gui` world
- `layer36:ui@0.1.0` for windows, widget trees, events, dialogs, clipboard, and menus
- `layer36:gfx@0.1.0` for 2D canvas and a small future 3D surface
- `layer36:audio@0.1.0` for playback and capture shape
- `scripts/check-phase3-uapi.sh` so CI can reject broken WIT before runtime code depends on it
- manifest and policy support for the first Phase 3 permission names
- `adapter-common::ui`, an in-memory draft window registry for IDs, title and size validation, lifecycle state, redraw requests, and events
- `runtime::phase3_ui`, a runtime dispatcher scaffold that checks UCap before draft window work

This is a draft contract, not a frozen API. The next work is to add a tiny host
side prototype that connects this shared model to one real native window,
receives events, and draws a simple surface.
