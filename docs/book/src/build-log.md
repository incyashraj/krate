# Build Log

Short public notes on what shipped, written as it happens. One entry per
milestone — the long-form detail always lives in `STATUS.md` and the phase
pages.

---

## 2026-07-02 — Direction check: plan amendments adopted

Nine weeks in, we stopped and audited the whole project against its own plans
before starting the next big slice. The result is a formal change order
(`Plan/Plan-Amendments-2026-07.md`). The three changes that matter:

1. **Linux widgets go drawn-first (ADR-0015).** The original plan paired winit
   windows with native GTK4 widgets — but GTK4 removed foreign-window
   embedding, so those two choices cannot compose. Caught on paper before any
   code was written against it. Linux v0.1 now draws every widget (vello)
   inside winit windows; macOS keeps native AppKit lowering and Windows keeps
   native Win32 controls.
2. **The next milestone is a vertical slice, not more scaffolding.** P3-VS-01:
   one WebAssembly component opens a real macOS window containing a real
   native `NSButton` and `NSTextField`, and receives the click back —
   end-to-end through the permission layer and the runtime dispatcher. It
   proves the riskiest architectural bet first.
3. **An agent-embedding track.** After the slice: a runtime embedding API,
   `layer36 run --json`, and an MCP server wrapper, so AI-agent frameworks can
   execute generated components inside Layer36's capability sandbox.

Also: Phase 2 closeout is timeboxed (the engineering has been done for a
while; what remains is evidence paperwork), and the self-hosted fuzz nightly
is paused while its runner is offline.

## 2026-06-23 — AppKit prototype complete through the event loop

The opt-in macOS native path now covers the full prototype chain: an owned
`NSWindow` bound to a Layer36 window id, a real retained `NSWindowDelegate`
recording close/resize/focus/scale callbacks, an attached `NSView` draw
surface with a visible clear color, a non-blocking event-loop step driver,
and a local smoke command that creates, shows, pumps, inspects, and closes
the window through the runtime dispatcher. Linux and Windows gained Winit
session scaffolding and a callback collector bridge, ready for real windows.

## 2026-06 — Phase 3 contract layer landed

WIT drafts for `ui`, `gfx`, and `audio`; a host-neutral widget tree with
stable IDs; a Taffy-backed layout crate with prepared-tree reuse, hit
testing, and 1k/10k-node benchmarks; and a UCap-gated runtime dispatcher
with pointer, key, text, host-window, theme, and scale event routes. All of
it headless and tested before any native backend depends on it.

## 2026-05 — Phase 2: real CLI apps with real permissions

The runtime runs `layer36-clock`, `layer36-cat`, and `layer36-curl` from a
single `.wasm` per app on Linux, macOS, and Windows. Apps declare
capabilities in a manifest; the runtime refuses undeclared or ungranted
access before any host call happens. Cross-host CI records evidence for
samples, permission enforcement, adapters, and benchmarks on every change.

## 2026-05-03 — One binary, three operating systems

`layer36 run hello.wasm` produced identical output on Linux, macOS, and
Windows in hosted CI from one shared artifact. The portability bet works.
