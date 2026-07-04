# Build Log

Short public notes on what shipped, written as it happens. One entry per
milestone — the long-form detail always lives in `STATUS.md` and the phase
pages.

---

## 2026-07-04 — Layer36 becomes Krate

The project has a new name: **Krate**, by **Krate Labs**. A crate ships
goods anywhere unchanged; a Rust crate ships code. Both are exactly what
this runtime does to applications. The code, commands, and `layer36:*` API
namespaces keep the legacy name for a short transition — the code-level
rename is scheduled to land before the UAPI freeze, so early adopters never
face a breaking rename after stability is promised.

## 2026-07-04 — All three desktops, one file, three real windows

Hours after Linux, Windows followed — and the winit backend cloned from the
CI-proven Linux implementation compiled and worked on its first attempt. The
full test matrix is green with both proofs inside it: the portable hello-gui
component, the same bytes that opened the clicked native macOS window,
opened a real winit window on a Linux host and a real winit window on a
Windows host, exiting cleanly on both. The platform's founding claim —
write once, run on every desktop, natively — is now a machine-checked fact
on every full CI run. Next: drawing widgets inside those windows.

## 2026-07-04 — The same file opens a real window on Linux

Eight CI iterations after the slice began — driven entirely from a Mac that
can't run the code — the Linux winit backend went green: a thread-locally
owned, non-blockingly pumped X11 event loop feeding the same shared event
stream the macOS window uses. The proof is in the CI log itself: the
portable hello-gui component, byte-identical to the one that opened the
clicked AppKit window, opened a real winit window on a Linux host under
Xvfb and exited cleanly, with the adapter's window round-trip smoke passing
beside it. Two of three desktops now open real windows from one file;
Windows is next, and the component still will not change.

## 2026-07-03 — Layer36 speaks MCP

The agent-embedding track is complete. `layer36-mcp-server` is a small
binary any MCP-capable agent framework can attach to: one `run_component`
tool, executing portable components inside the capability sandbox and
returning the full machine-readable report. In the first end-to-end run an
agent asked to read a file without permission and was told exactly which
capability was missing; asked again with grants, it got the file. The
permission wall is now something agents can see and reason about, not just
hit.

## 2026-07-03 — Agents can now call Layer36

The embedding surface landed the same day as the native window. Any program
— including an AI-agent framework — can now execute a component inside
Layer36's capability sandbox in under 30 lines of Rust: grants supplied as
data, no prompts, stdout captured, exit classified. And `layer36 run --json`
turns every run into one machine-readable object: which app, which
capabilities were granted (with exact boundaries), what was denied, how it
exited, how long it took, what it printed. The wedge — safe execution of
generated software — now has a socket for the machines that need it.

## 2026-07-03 — One portable file opens a native window

The vertical slice is complete. `layer36 run --native-window hello-gui.wasm`
now takes a single portable WebAssembly component — the same bytes on any
OS — and opens a real native macOS window containing a real native button
and text field, laid out by our engine, permission-checked by our capability
layer. Click the native button and the component receives a portable event
and updates the native text. Headless, the same file runs everywhere in CI.
The component imports only `layer36:*` interfaces — no WASI, no host
specifics — which required teaching the events contract to deliver one event
at a time and the guest to allocate strings the way generated bindings do.
This is the platform's core promise, demonstrated end to end for the first
time.

## 2026-07-02 — A real native button, driven by a Layer36 widget tree

The first native widget lowering landed, hours after the amendments that
ordered it. A Layer36 widget tree now becomes a real AppKit `NSButton` and
`NSTextField`, positioned by our layout engine inside the prototype window. A
native click travels AppKit's own target-action path into Layer36's shared
event stream as a routed event carrying the widget id — the same shape drawn
widgets use. The smoke run proves the loop end to end without a human: lower
the widgets, click the real button programmatically, observe the routed
event, update the native text field. This is the core Phase 3 bet (native
lowering, ADR-0013) working for the first time. Also today: the self-hosted
fuzz runner is back online as a proper service, with a green verification
run.

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
