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
- `adapter-common::ui::WindowAdapter`, the named lower boundary for window lifecycle and host event-loop signals
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
- draft theme and scale event routing, so dark mode and DPI changes have a stable path before native windows land
- headless draft UI adapter entry points in the macOS, Linux, and Windows adapter crates, each with a blank-window smoke test
- active and planned window backend info for each host, with AppKit planned for macOS and winit planned for Linux and Windows
- native window handle handoff in `WindowAdapter`, so a real host window can be bound to a stable Layer36 `WindowId`
- the first macOS AppKit handoff method, ready for the native AppKit window prototype while the default adapter remains headless draft
- an opt-in macOS `AppKitWindowBackend` that creates an owned `NSWindow` on the main thread, attaches its native handle to a Layer36 window id, and shows it through the shared window path
- AppKit event bridge targets for close, resize, focus, and display scale, plus a native window snapshot helper for the coming delegate wiring
- `AppKitWindowSession`, a small state object that owns the native window, remembers the last snapshot, and refreshes changed native state into the shared event queue
- `AppKitWindowNativeEvent` and `AppKitWindowEventState`, a tested Rust callback surface for the real AppKit delegate to call next
- an AppKit redraw bridge, so the first native drawing surface can request paint through the shared window event queue
- `AppKitWindowDelegateCallback` and `AppKitWindowDelegateBridge`, so the coming Objective-C delegate can stay thin and hand event translation to tested Rust code
- `AppKitDrawSurfaceState`, which tracks size, scale, clear color, redraw requests, and frame metadata before the real AppKit view paints pixels
- `AppKitDrawViewSurface`, an opt-in AppKit `NSView` attachment path that sets a visible clear color, marks the view dirty, and records the first frame snapshot
- `AppKitWindowNativeDelegate`, a retained AppKit `NSWindowDelegate` object that records native close, resize, focus, and backing-scale callbacks for the Rust session to drain
- `Phase3UiRuntime::with_host_adapter`, which selects the current host UI adapter and reports whether it is still headless or native
- ADR-0013 and RFC-0003 now record the widget lowering strategy: native controls where the host has a semantic match, drawn fallback where it does not
- ADR-0014 records the layout engine choice: Taffy, with a small flexbox-style subset first

This is a draft contract, not a frozen API. The macOS side can now create and
show one AppKit window through an opt-in prototype, and it has checked bridge
methods plus session state for the main host-window events. The callback-shaped
Rust event state is now in place too, including redraw requests for the first
paint path. AppKit-style delegate callbacks now have a tested Rust translator
too. AppKit now also has draw-surface state and an opt-in AppKit draw view
surface. The view path can attach an `NSView`, set a visible clear color, mark
the view dirty, and record the first frame snapshot. The real AppKit delegate
object now exists too. It records native window callbacks into a small queue
that the Rust session drains through the same bridge. The next work is to wire
that native event path into the default runtime without breaking headless CI.

See [Widget Protocol](../phase3/widget-protocol.md) for the plain-language
version of this Phase 3 direction. See [Layout](../phase3/layout.md) for the
current geometry path.
