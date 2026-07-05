# Build Log

Short public notes on what shipped, written as it happens. One entry per
milestone — the long-form detail always lives in `STATUS.md` and the phase
pages.

---

## 2026-07-05 — The windows learn to listen to the keyboard

Keyboard input flows end to end on the host side: real key presses in
the Linux and Windows windows become portable key and text events
attached to the focused widget, and clicking a text field now moves
focus there. The contract needed zero changes — key, text-input, and
focus-changed events were designed into the WIT months ago and were
waiting for a backend to feed them. Raw keys travel the same
loop-proof drain channel the pointer uses. And the component that
visibly types landed the same evening: hello-gui renders every
keystroke into its text field live and echoes the final text on exit,
so CI now runs the complete loop on Linux — click the field, type
"hi krate" with a synthetic keyboard, photograph the window with the
words in it, click the button, and check the app repeated the text
back. A keyboard round trip through nine layers, machine-verified on
every full run.

## 2026-07-05 — The drawn widgets learn manners

Styling pass: buttons and fields now have rounded corners, and buttons
respond to the pointer — a lighter fill on hover, a deeper one while
pressed — repainted the moment the state changes. The interaction state
is a tiny value both painters accept, the cursor hit-testing is one
shared, unit-tested helper, and the pixel tests assert the corner
rounding and the state colors directly. Small slice, but it is the
difference between "a drawing of a button" and "a button."

## 2026-07-05 — The drawn windows get real typography

The renderer slice's first pass: widget frames on Linux and Windows now
render as full vector scenes — antialiased labels laid out by parley
from the host's real fonts, rasterized by vello_cpu on the CPU, so CI
needs no GPU and the pipeline stays byte-inspectable. The two platform
painters collapsed into one shared implementation first, so this swap
(and the GPU vello swap later) touches exactly one module. The 5x7
bitmap font stays as the zero-dependency fallback for hosts with no
usable fonts. A five-minute compile spike against the released crates
de-risked the dependency before a single line entered the tree.

## 2026-07-04 — The Linux button gets clicked by a robot, then learns to speak

Two proofs in one day. First, input routing: the full CI matrix now runs a
synthetic click — `xdotool` moves the pointer to the drawn button inside a
real winit window under Xvfb and presses it, and the portable component
observes the press and exits clean. The same click round trip a human hand
proved on macOS is now machine-proved on Linux, on every full CI run.
Second, drawn text: a small 5x7 bitmap font (a deliberate placeholder until
the vello renderer brings real typography) now paints actual labels into
the drawn windows on Linux and Windows — the button says "Click me",
fields show their text, and `Text` widgets are words instead of gray
blocks. The click proof also captures a screenshot of the drawn window and
publishes it as a CI artifact: visual evidence of the Linux UI produced
entirely by hosted CI.

## 2026-07-04 — The rename lands: the system is Krate everywhere

Phase B of the rename executed: 272 files of content, 25 renamed paths
with history preserved, regenerated bindings and contract-lock hashes
under the `krate:*` namespace, all four sample components rebuilt
import-pure, the CLI reborn as `krate`, the JSON schema as
`krate.run.v1`, the repository moved to `incyashraj/krate` (old links
redirect), and the self-hosted runner re-badged `krate-local`. Verified
before pushing: full workspace tests, lints, and a live
`krate run --json` answering with its new name. The future bundle
format becomes `.krate` — the brand you can attach to an email.

## 2026-07-04 — The Linux window shows its first pixels

The drawn-widget pass landed the same day: the Krate window on Linux now
paints its UI — background, a filled button block, a bordered text field —
from the same lowered placements macOS turns into native controls. It is
deliberately humble rendering (solid rectangles through a CPU framebuffer,
every system library loaded at runtime) because the pipeline was the point;
the vello GPU renderer replaces the painter behind the same contract. All
three OS lanes green with it aboard.

## 2026-07-04 — Layer36 becomes Krate

The project has a new name: **Krate**, by **Krate Labs**. A crate ships
goods anywhere unchanged; a Rust crate ships code. Both are exactly what
this runtime does to applications. The code, commands, and `krate:*` API
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

## 2026-07-03 — Krate speaks MCP

The agent-embedding track is complete. `krate-mcp-server` is a small
binary any MCP-capable agent framework can attach to: one `run_component`
tool, executing portable components inside the capability sandbox and
returning the full machine-readable report. In the first end-to-end run an
agent asked to read a file without permission and was told exactly which
capability was missing; asked again with grants, it got the file. The
permission wall is now something agents can see and reason about, not just
hit.

## 2026-07-03 — Agents can now call Krate

The embedding surface landed the same day as the native window. Any program
— including an AI-agent framework — can now execute a component inside
Krate's capability sandbox in under 30 lines of Rust: grants supplied as
data, no prompts, stdout captured, exit classified. And `krate run --json`
turns every run into one machine-readable object: which app, which
capabilities were granted (with exact boundaries), what was denied, how it
exited, how long it took, what it printed. The wedge — safe execution of
generated software — now has a socket for the machines that need it.

## 2026-07-03 — One portable file opens a native window

The vertical slice is complete. `krate run --native-window hello-gui.wasm`
now takes a single portable WebAssembly component — the same bytes on any
OS — and opens a real native macOS window containing a real native button
and text field, laid out by our engine, permission-checked by our capability
layer. Click the native button and the component receives a portable event
and updates the native text. Headless, the same file runs everywhere in CI.
The component imports only `krate:*` interfaces — no WASI, no host
specifics — which required teaching the events contract to deliver one event
at a time and the guest to allocate strings the way generated bindings do.
This is the platform's core promise, demonstrated end to end for the first
time.

## 2026-07-02 — A real native button, driven by a Krate widget tree

The first native widget lowering landed, hours after the amendments that
ordered it. A Krate widget tree now becomes a real AppKit `NSButton` and
`NSTextField`, positioned by our layout engine inside the prototype window. A
native click travels AppKit's own target-action path into Krate's shared
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
   `krate run --json`, and an MCP server wrapper, so AI-agent frameworks can
   execute generated components inside Krate's capability sandbox.

Also: Phase 2 closeout is timeboxed (the engineering has been done for a
while; what remains is evidence paperwork), and the self-hosted fuzz nightly
is paused while its runner is offline.

## 2026-06-23 — AppKit prototype complete through the event loop

The opt-in macOS native path now covers the full prototype chain: an owned
`NSWindow` bound to a Krate window id, a real retained `NSWindowDelegate`
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

The runtime runs `krate-clock`, `krate-cat`, and `krate-curl` from a
single `.wasm` per app on Linux, macOS, and Windows. Apps declare
capabilities in a manifest; the runtime refuses undeclared or ungranted
access before any host call happens. Cross-host CI records evidence for
samples, permission enforcement, adapters, and benchmarks on every change.

## 2026-05-03 — One binary, three operating systems

`krate run hello.wasm` produced identical output on Linux, macOS, and
Windows in hosted CI from one shared artifact. The portability bet works.
