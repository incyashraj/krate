# Widget Protocol

Layer36 does not want every desktop app to look like the same painted surface.
It also does not want every app developer to write three different UIs for
Windows, macOS, and Linux.

The Phase 3 answer is a small widget tree. The app says what it wants. The host
adapter decides how to make it feel right on that machine.

## The Basic Idea

For common controls, Layer36 should use real host widgets. A button should be a
real button. A text field should use the host text system where possible. A menu
should follow host menu rules.

For surfaces that do not have a good native match, Layer36 draws them itself.
That gives apps room for canvas, custom lists, charts, and later richer
graphics.

```mermaid
flowchart LR
    A["App asks for widgets"] --> B["Layer36 runtime"]
    B --> C["Layout and permission checks"]
    C --> D{"Host has a real match?"}
    D -->|yes| E["Native widget"]
    D -->|no| F["Drawn fallback"]
    E --> G["User sees the app"]
    F --> G
```

## Why This Direction

There are three common ways to build cross platform desktop UI:

| Approach | What it means | Why Layer36 is not using it as the main path |
|---|---|---|
| Draw everything | The framework paints every control itself | Easier to match pixels, but often feels less native |
| Use only native controls | Every widget is a host widget | Good feel, but too rigid for custom app surfaces |
| Embed a browser | The app is a web UI in a desktop shell | Useful elsewhere, but not the Layer36 desktop goal |

Layer36 uses a mixed path. Native where the host has the right control. Drawn
where the app needs its own surface.

## The Native Three Of Five Rule

A widget belongs in the core protocol only when at least three of these hosts
have a native control with the same meaning:

- Windows
- macOS
- Linux
- iOS
- Android

This keeps the core set small. It also keeps the API close to what real
platforms already know how to do.

## First Widget Set

The first set is intentionally small:

| Widget | First use |
|---|---|
| Window root | Put the app in a host window |
| Stack | Arrange controls vertically or horizontally |
| Text | Show labels and short text |
| Button | Trigger actions |
| Text field | Edit one line |
| Text area | Edit note content |
| List | Show notes |
| Scroll | Move through long content |
| Checkbox | Basic on or off state |
| Menu | App and window commands |
| Canvas | Custom drawing later |

That is enough to build the first `layer36-notes` app without turning Phase 3
into a full design system.

## How Events Move

The host creates raw events. Layer36 turns those into stable events that the app
can understand.

```mermaid
sequenceDiagram
    participant Host
    participant Adapter
    participant Runtime
    participant App

    Host->>Adapter: click, key, text, pointer, close
    Adapter->>Runtime: Layer36 UI event
    Runtime->>App: event with widget or window id
    App->>Runtime: next widget tree
```

The first code path already handles draft window lifecycle events. It also has
a routed pointer event path now: the runtime can take a logical pointer
position, run layout hit testing, and queue an event with the target widget ID.
It also has key and text input routing through the focused widget. The next
steps are a real native window, a host event loop, then a tiny widget tree with
text and a button.

## Current Status

Done now:

- Phase 3 UI, graphics, and audio WIT drafts exist.
- GUI manifests are recognized.
- Phase 3 permission names exist.
- `adapter-common::ui` has the first host-neutral widget tree types:
  `WidgetId`, `WidgetKind`, `WidgetNode`, `WidgetStyle`, and `WidgetTree`.
- The shared UI adapter and runtime dispatcher can now set a root widget,
  update child nodes, remove nodes, move focus, and inspect draft widget state.
- `layer36-layout` can compute Taffy-backed rectangles for the shared widget
  tree and return them by stable widget ID.
- The runtime dispatcher can ask for a layout snapshot for the widget tree
  stored on a draft window.
- The runtime dispatcher can also prepare a layout tree for repeated passes,
  which is the path future event loops should use between widget mutations.
- The layout crate has a first hit-test helper. It can use the layout snapshot
  to find the deepest widget under a point.
- The runtime can queue a routed pointer event after hit testing, so a future
  native mouse or touch event can already become a stable Layer36 event with a
  window ID and optional widget ID.
- The runtime can queue routed key events and committed text input for the
  focused widget, which gives native keyboard and IME commit events a stable
  place to land later.
- The adapter and runtime can poll one queued UI event at a time in FIFO order,
  which matches the planned `events.poll()` app-facing shape.
- Host window events for close request, resize, and focus change have draft
  routes through the same queue.
- Theme and scale change events have draft routes too, so dark mode and DPI
  changes can use the same queue once real native windows exist.
- `WindowAdapter` now names the lower window/event-loop boundary. `UiAdapter`
  builds on it for widget trees, input, and clipboard.
- Native window handles now have a shared handoff point. A host backend can
  attach an AppKit, winit, or Win32 handle to a stable Layer36 window id, then
  look it up or detach it later.
- macOS has the first opt-in AppKit window prototype. It creates an owned
  `NSWindow`, binds it to the Layer36 window id, and can show it through the
  shared window path. This starts the real native window backend work. Native
  events and drawing are still pending.
- The AppKit prototype now has bridge targets for close requests, resize,
  focus, and display-scale changes. It also has a snapshot helper that reads
  native size, focus, visibility, and scale from the real window.
- AppKit now has a window session object. It owns the native window, remembers
  the last snapshot, refreshes changed state into the queue, and gives future
  delegates a clear object to call.
- AppKit now has a native event state object. It accepts delegate-shaped events
  for close, resize, focus, display scale, and full snapshots, then queues them
  through the same shared path.
- AppKit redraw requests now use that same path, so the first native drawing
  surface has a tested way to ask Layer36 for another paint.
- AppKit delegate callbacks now have a Rust bridge. The future Objective-C
  delegate can translate native method calls into a small enum, then the Rust
  bridge handles resize, focus, scale, redraw, close, and snapshot routing.
- AppKit now has draw-surface state. It tracks the window size, display scale,
  clear color, redraw count, and frame number. It still does not paint pixels.
  It gives the next `NSView` step a small state object to use.
- AppKit now has a first draw view surface too. It can attach an owned
  `NSView` to the prototype window, set a visible clear color, mark the view
  dirty, and record the first frame snapshot. This is still opt-in and not the
  default runtime path.
- AppKit now has a first real window delegate object. It implements
  `NSWindowDelegate`, keeps the delegate retained for the window session, and
  records close, resize, focus, and backing-scale callbacks into a FIFO queue.
  The Rust session drains that queue through the same tested bridge.
- AppKit now has a first event-loop step driver. It can refresh native state,
  drain delegate callbacks, and queue a redraw request without blocking the
  process.
- The runtime can now select the AppKit prototype mode explicitly on macOS.
  The default runtime path still stays headless.
- The runtime and shared `UiAdapter` now have one common event-loop pump method.
  Headless adapters return no native tick, and the AppKit prototype maps its
  native step into the shared `UiEventLoopTick` report.
- The runtime has a UI dispatcher scaffold.
- macOS, Linux, and Windows adapters expose headless draft window and UI entry
  points, plus the planned native backend for each host. macOS also exposes the
  first AppKit handle handoff method.
- The runtime can choose the current host adapter.
- ADR-0013 and RFC-0003 now describe the widget lowering rule.

Pending:

- ignored local smoke for the selectable AppKit runtime path and shared pump
- Linux and Windows native window prototypes
- host event loop that feeds real close, resize, focus, theme, scale, pointer, key, and text events into the queue
- widget tree lowering
- larger layout style coverage and recorded large-tree benchmark results on all target hosts
- IME composition events
- accessibility tree
- `layer36-notes`

This is the right direction for the universal platform goal. We are building the
contract first, then the runtime boundary, then the host adapters. That keeps
the platform from becoming one app demo with no reusable core.
