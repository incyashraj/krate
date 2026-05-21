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
- `adapter-common::ui`, the first host-neutral widget tree model for stable widget IDs, widget kinds, labels, roles, style hints, and parent links
- `adapter-common::ui::UiAdapter`, the shared trait that native UI adapters will implement
- `runtime::phase3_ui`, a runtime dispatcher scaffold that checks UCap before calling the shared UI adapter
- draft widget-tree dispatch for setting a root widget, upserting nodes, removing nodes, moving focus, and inspecting widget state
- `layer36-layout`, the first Taffy-backed layout wrapper, which turns the shared widget tree into stable rectangles by widget ID
- runtime layout dispatch, so the Phase 3 dispatcher can compute layout for a stored draft widget tree after the same UI capability check
- generated layout tests across 100 tree shapes, plus a 1k/10k-node Criterion benchmark target
- a prepared repeated-layout path, so future event loops can reuse the same layout tree instead of rebuilding it each frame
- first layout-based hit-test helper for finding the deepest widget under a point
- draft pointer event routing, so the runtime can turn a logical pointer position into a queued event with the hit widget ID
- draft key and text input routing, so focused widgets can receive portable key events and committed text before native IME work starts
- FIFO event polling, so future `events.poll()` calls can consume one queued UI event at a time
- draft host window event routing for close requests, resize, and focus changes
- headless draft UI adapter entry points in the macOS, Linux, and Windows adapter crates, each with a blank-window smoke test
- `Phase3UiRuntime::with_host_adapter`, which selects the current host UI adapter and reports whether it is still headless or native
- ADR-0013 and RFC-0003 now record the widget lowering strategy: native controls where the host has a semantic match, drawn fallback where it does not
- ADR-0014 records the layout engine choice: Taffy, with a small flexbox-style subset first

This is a draft contract, not a frozen API. The next work is to add a tiny host
side prototype that connects this shared model to one real native window,
feeds real host window and input events into the draft event queue, and draws a simple surface.

See [Widget Protocol](../phase3/widget-protocol.md) for the plain-language
version of this Phase 3 direction. See [Layout](../phase3/layout.md) for the
current geometry path.
