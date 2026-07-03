# Layer36 Status

Last updated: 2026-07-03
Repo: `incyashraj/layer6x6`
Branch: `main`
Latest checked completed push before this slice: `8bc9a15`
Working tree at this status update: P3-VS-01 complete through the WASM path.
`layer36 run` now executes Phase 3 `gui` world components: the hello-gui
sample creates a window, submits a widget tree, and polls events through real
`layer36:ui` host imports backed by the UCap-gated dispatcher. With
`--native-window` on macOS the same portable component opens a real AppKit
window with real native controls; a click on the native button reaches the
component as a portable pointer event and updates the native text field. The
component imports only `layer36:*` interfaces (the events contract moved to
single-event `option<event>` polling to keep guest bindings free of
list-of-variant lifting, which current guest toolchains cannot compile
without dragging WASI panic machinery into the component). Sub-slice 1
(native AppKit lowering) was verified earlier the same slice — the first real native AppKit widget lowering. A Layer36
widget tree now lowers to a real `NSButton` and `NSTextField` positioned by
the Taffy layout, a native click flows back through the delegate queue and
event-loop pump into the shared stream as a routed pointer event with the
correct widget id, and native text updates round-trip. Proven by the extended
`phase3_appkit_runtime_smoke` example on a real window. Earlier this same day
the July 2026 plan amendments were applied (`Plan/Plan-Amendments-2026-07.md`)
and the `layer36-local` runner was restored as a LaunchAgent service with a
green verification fuzz run.

## 1) Project size today

- Commits after this slice lands: about 350
- Tracked files after this slice lands: about 334
- Total tracked lines after this slice lands: about 98,012
- Rust lines (`.rs`) after this slice lands: about 50,020
- Docs lines (`.md`) after this slice lands: about 30,644

## 2) Latest CI and Pages state

Latest completed push (`02f28bf`) checks:

- CI: success (run `27895755338`)
- Deploy docs to GitHub Pages: success (run `27895755345`)

Manual hosted full CI run `26069665276` passed on commit `3f1a219`.
Linux, macOS, and Windows full-test lanes all passed. The language-variant,
UCap, adapter, and sample evidence compare jobs all passed too. This closes the
immediate hosted full CI blocker that was left after run `26064573902`.

The self-hosted `layer36-local` runner is back online as of 2026-07-02 and the
fuzz nightly schedule is restored. Root cause of the outage: the runner had
only ever run in a foreground terminal, died when that terminal closed
(2026-06-24), and GitHub then deleted the stale registration. It is now
re-registered and installed as a LaunchAgent service
(`~/runner/actions-runner/svc.sh`), so it survives reboots. If it ever drops
again: `cd ~/runner/actions-runner && ./svc.sh status && ./svc.sh start`.

## 3) What this version can do now

Layer36 already runs real Phase 2 CLI components through the runtime:

- `layer36-clock`
- `layer36-cat`
- `layer36-curl`

Current capability set includes:

- Manifest parsing and capability checks
- Launch grant flow (`--grant`, `--auto-grant`, prompt flow)
- Runtime UAPI policy checks before host calls
- Cross language fixture and parity coverage for Rust, TypeScript, and available Go paths
- Published docs on GitHub Pages with Phase tracking

## 4) Progress read for Phase 2

Practical engineering progress is strong and close to completion for the first useful slice.

- Core engineering slice: around 90% to 92%
- Formal Phase 2 exit gates: around 84% to 87%

Main reason formal completion is lower than engineering completion:
the remaining work is mostly evidence and gate closure, not missing base architecture.

## 4A) Phase 3 start

Phase 3 is now starting at the contract layer, while Phase 2 still waits for the
outside developer review before formal closeout.

Current Phase 3 slice:

- `layer36:app@0.2.0` with a `gui` world
- `layer36:ui@0.1.0` WIT draft for windows, widget trees, events, dialogs, clipboard, and menus
- `layer36:gfx@0.1.0` WIT draft for 2D canvas and a small future 3D surface
- `layer36:audio@0.1.0` WIT draft for playback and capture shape
- `scripts/check-phase3-uapi.sh` to keep the draft parseable and documented
- manifest tooling now accepts `layer36:app/gui@0.2.0`
- `layer36 run` recognizes GUI manifests and exits clearly until the window
  runtime exists
- first Phase 3 capability names now parse through the existing manifest and
  policy layer: `ui`, `gfx`, and `audio`
- `adapter-common::ui` now has an in-memory draft window registry, an explicit
  `WindowAdapter` trait for window lifecycle and host-window events, and a
  shared `UiAdapter` trait for widget trees, input, and draft clipboard
  behavior.
- `runtime::phase3_ui` now exposes the first runtime-facing UI dispatcher path:
  window create/show/resize/redraw/close goes through UCap before touching the
  shared adapter trait, and clipboard checks fail at the permission boundary
  before unsupported host code is reached
- `adapter-macos`, `adapter-linux`, and `adapter-windows` now expose Phase 3 UI
  adapter entry points. These are headless draft adapters today, with
  blank-window smoke tests, not native OS windows yet.
- `adapter-linux` and `adapter-windows` now expose guarded Winit prototype
  adapter boundaries, native-handle handoff helpers, and discovery entry
  points. They are wired into the runtime selector but still report unsupported
  until real OS windows exist.
- `adapter-common::ui` now exposes shared Winit session owner scaffolding:
  snapshots, native event shapes, event-loop steps, step reports, and
  `WinitWindowSession`.
- `adapter-linux` and `adapter-windows` now track Winit prototype sessions,
  attach the handle token, pump prepared native events through the shared
  queue, and remove the tracked session on close.
- `adapter-common::ui` now has `WinitWindowEventCollector`, a shared FIFO
  callback bridge for future Linux and Windows Winit event handlers.
- `adapter-linux` and `adapter-windows` can now record Winit-shaped callbacks,
  count pending callbacks, drain them through the normal event-loop pump, and
  keep the callback collector tied to the tracked session.
- `runtime::phase3_ui` now has `Phase3UiRuntime::with_host_adapter`, which
  selects the current OS adapter entry point and reports adapter capability info
  such as host family, backend name, and whether native windows are enabled.
- `docs/adr/0013-widget-lowering-strategy.md`,
  `docs/rfc/0003-widget-protocol.md`, and the mdBook widget protocol page now
  record the native widget plus drawn fallback rule before native backend work
  depends on it.
- `adapter-common::ui` now has the first host-neutral widget tree model:
  stable widget IDs, the first widget kind set, labels, role hints, small style
  hints, and parent-link validation.
- the shared `UiAdapter`, headless host adapters, and `Phase3UiDispatcher` now
  support draft widget-tree calls: set root, upsert child nodes, remove nodes,
  focus nodes, inspect the tree, and inspect focused widget state.
- `crates/layout` now has the first Taffy-backed layout wrapper. It maps the
  shared `WidgetTree` into Taffy, validates the logical viewport, and returns
  `LayoutSnapshot` rectangles keyed by stable `WidgetId`.
- `Phase3UiDispatcher` can now compute a layout snapshot for the draft widget
  tree stored on a window, after the same UI capability check used by the rest
  of the Phase 3 UI path.
- `layer36-layout` now has generated coverage for 100 different layout tree
  shapes, a compile-checked Criterion benchmark target for 1,000-node and
  10,000-node trees, absolute rectangle helpers, and a first hit-test helper
  that can find the deepest widget under a point.
- The first local 10,000-node layout benchmark run is still above the Phase 3
  exit budget. That means the benchmark path exists, but optimization and
  recorded cross-host numbers are still pending.
- `PreparedLayoutTree` now lets the runtime prepare a Taffy tree once and
  recompute layout for repeated viewport changes. The local prepared 10k
  benchmark is under the Phase 3 budget, while cold rebuild remains above it.
- The runtime now has the first draft pointer route. It computes layout for the
  window, hit-tests the logical pointer point to the deepest widget, then queues
  a portable `UiEvent::Pointer` with a window ID and optional widget ID. Native
  mouse, touch, hover, wheel, and keyboard event sources are still pending.
- The runtime now has draft key and committed-text routes too. It looks up the
  focused widget for a window, validates the portable key or text payload, and
  queues `UiEvent::Key` or `UiEvent::TextInput` through the shared adapter
  boundary. Full native keyboard capture, shortcut normalization, and IME
  composition events are still pending.
- The adapter and runtime now have FIFO event polling beside batch draining.
  This matches the planned `events.poll()` shape, so the future app-facing
  event loop can consume one queued UI event at a time.
- Draft host window events now have shared routes too: close requested,
  host resized, and window focus changed. These routes queue events without
  closing the window early, so future native backends can report user intent
  while the app still decides what happens next.
- Draft theme and scale events now have shared routes too. A future native
  backend can report dark mode changes and per-window DPI scale changes through
  the same event queue before real drawing code depends on those values.
- Added the selectable AppKit runtime smoke command, which creates, shows,
  pumps, inspects, and closes the macOS native prototype path through
  `Phase3UiRuntime` and `Phase3UiDispatcher`.
- Added Linux and Windows Winit prototype boundaries with tested handle
  handoff, guarded discovery functions, runtime selector wiring, and adapter
  boundary checks.
- Added the shared Winit session owner scaffold. Linux and Windows can now hold
  a tracked Winit session, route prepared resize/focus/scale/redraw/close
  events through the shared queue, and clean up the session on close. Real
  Winit OS window creation and event collection are still the next step.
- Added the Winit callback collector bridge. Linux and Windows can now record
  Winit-shaped callbacks in FIFO order and let the shared event-loop pump drain
  them into the same UI event stream. Real Winit OS window creation is still
  the next step.
- The host adapter info now records both the active window backend and the
  planned native window backend. Today the active backend is still headless
  draft. The planned targets are AppKit for macOS and winit for Linux and
  Windows.
- `WindowAdapter` now has a native handle handoff path. A future native backend
  can attach, inspect, and detach an opaque AppKit, winit, or Win32 host handle
  for a stable Layer36 `WindowId`. macOS has the first AppKit handoff method,
  while the default backend still stays headless draft.
- macOS now has an opt-in AppKit window prototype. It can create an owned
  `NSWindow` on the main thread, attach the native pointer to a Layer36 window
  id, and show it through the shared window path. This is not the default UI
  runtime yet because native event capture and drawing still need to land.
- The AppKit prototype now has explicit bridge methods for native close,
  resize, focus, and display-scale events. It also has a snapshot helper that
  reads content size, focus, visibility, and backing scale from the real
  `NSWindow`. This gives the coming AppKit delegate code a small, checked place
  to feed native state into the shared event queue.
- `AppKitWindowSession` now owns the native window prototype and remembers the
  last native snapshot. Its refresh path queues only changed native state into
  the shared event queue. That gives the coming AppKit delegate and event-loop
  code one clear state object to call into.
- The macOS adapter now exports `AppKitWindowNativeEvent` and
  `AppKitWindowEventState`. This gives AppKit delegates a tested Rust callback
  shape for close, resize, focus, display-scale, and full-snapshot events before
  we add the Objective-C delegate object itself.
- AppKit redraw requests now use that same callback path and the shared
  `WindowAdapter::request_redraw` queue. This gives the first AppKit drawing
  surface a tested paint-request route before we wire the real view.
- The macOS adapter now exports `AppKitWindowDelegateCallback` and
  `AppKitWindowDelegateBridge`. These use AppKit-style callback names and
  translate them into the tested native event state, so the coming Objective-C
  delegate can stay thin.
- AppKit now has the first draw-surface state scaffold. It tracks the Layer36
  window id, logical size, display scale, clear color, redraw count, and frame
  number. Redraw requests use the same delegate bridge as the future `NSView`
  painter. This does not paint pixels yet.
- AppKit now has an opt-in draw view surface. It can attach an owned `NSView`
  to the native prototype window, set a visible clear color through AppKit, mark
  the view as needing display, and record a first frame snapshot. The default
  runtime still uses the headless draft adapter.
- AppKit now has a first real native window delegate object. It implements
  `NSWindowDelegate`, stays retained by the AppKit session, records close,
  resize, focus, and backing-scale callbacks into a FIFO queue, and lets
  `AppKitWindowSession` drain those callbacks through the existing Rust bridge.
  The default runtime still uses the headless draft adapter.
- AppKit now has a first non-blocking event-loop step driver. It refreshes the
  native snapshot, drains delegate callbacks, and can queue redraw through the
  same shared event stream. This is still an opt-in prototype path; the default
  runtime still uses the headless draft adapter.
- The runtime can now select the AppKit prototype path explicitly through
  `Phase3HostUiMode::NativePrototype`. The default host adapter path still
  stays headless, so CI and non-native tests remain stable.
- The runtime and shared UI adapter now have one host-neutral event-loop pump.
  Headless adapters return no native tick, while the AppKit prototype maps its
  native event-loop step report into a common `UiEventLoopTick` shape.
- The selectable AppKit prototype runtime path now has a local smoke command.
  It asks for `Phase3HostUiMode::NativePrototype`, creates and shows a real
  AppKit window, pumps one shared event-loop tick, checks the native report,
  and closes the window through the runtime dispatcher on the main process
  thread.

- P3-VS-01 sub-slice 2 landed: the WASM path. `runtime` gained
  `phase3_gui_bindings` (the `gui` world generated against the Phase 3 WIT,
  reusing the Phase 2 generated modules via `with:` mappings) and
  `phase3_gui_host::Phase3GuiHost`, which serves `layer36:ui` window, tree,
  and events imports through the Phase 3 dispatcher, re-lowers supported
  widgets natively after every tree change (naive re-submit, per the slice
  spec), and returns honest `unsupported` errors for dialog, menu, gfx, and
  audio. `layer36 run` reaches the gui world through the existing fallback
  chain, and the new `--native-window` flag selects the AppKit prototype.
  `apps/layer36-hello-gui` is the first GUI component: import-pure
  (`layer36:*` only), runs headless everywhere (clean bounded exit 1), and
  opens a real native window on macOS. The `layer36:ui` events interface
  changed from `list<event>` to `option<event>` polling — it matches the
  dispatcher's FIFO `poll_event` design and keeps guest components pure; the
  guest-side lesson (std string constructors and list-of-variant lifting pull
  WASI panic machinery; use lifting-style raw allocation) is recorded in the
  hello-gui source. `scripts/smoke-phase3-gui-app.sh` builds the component,
  enforces import purity, and asserts the headless run.
- P3-VS-01 sub-slice 1 landed: `adapter-macos` now lowers `Button`,
  `TextField`, and `Text` widget placements to real AppKit controls
  (`AppKitWidgetPlacement`, `AppKitWidgetSurface`) inside the prototype
  window's content view, with top-left-to-AppKit Y-flip from layout rects. A
  new `Layer36WidgetTarget` Objective-C object receives NSButton
  target-action callbacks and pushes `WidgetActivated` into the same FIFO the
  window delegate uses, so the normal event-loop pump drains native clicks
  into the shared stream as routed pointer events carrying the widget id.
  `performClick` drives the identical path a physical click uses, so the
  round trip is provable without a human. Remaining for P3-VS-01: the WASM
  component wiring (minimal `layer36:ui` host imports for the `gui` world)
  and the demo component under `apps/`.
- The July 2026 plan amendments are now in effect
  (`Plan/Plan-Amendments-2026-07.md`). The three that change Phase 3 work:
  ADR-0015 moves Linux v0.1 widgets to the vello drawn fallback inside winit
  windows (GTK4 cannot embed in foreign windows, so the original gtk4-rs
  native-widget plan could not compose with winit); the next implementation
  milestone is re-sequenced to `P3-VS-01`, the macOS vertical slice that
  proves native widget lowering end-to-end before Winit broadening; and the
  agent-embedding tasks `P3-EMB-01..03` (runtime embedding API,
  `layer36 run --json`, MCP server wrapper) are added as a parallel,
  non-exit-blocking track after the slice.

This does not mean desktop UI is implemented yet. It means the first public
contract for desktop UI work is now in the repo and checked locally. The window
adapter trait, UI adapter trait, registry, dispatcher, host entry points, and
runtime discovery path are shared models for host adapters to follow before we
wire real AppKit, winit, or Win32 widgets (Linux widgets are drawn per
ADR-0015). The native handle handoff is now the checked bridge those real
windows can use.

## 4B) Hiring lane

Tracked per `Plan/Plan-Amendments-2026-07.md` A6 (Build Plan §14.3 marks this
risk Critical). Update this section with every status refresh.

- First-hire profile: systems engineer who owns the Windows and Linux adapter
  lanes (winit windowing, Win32 widget lowering, the Linux drawn-widget
  backend), freeing the founder for macOS, the runtime boundary, and the
  app-facing contract.
- Pipeline: not started — no candidates contacted yet.
- Outside-ready answer: "The first hire owns the Windows/Linux adapter lanes;
  the profile is defined and the search runs alongside Phase 3's vertical
  slice."

## 5) What remains to close Phase 2 fully

Top pending items:

1. Final UAPI v0.1 freeze review for WIT contracts
2. Final evidence bundle using the now-green hosted full CI run
3. Formal gate evidence:
   - multi day CI stability window
   - long fuzz soak pass
   - benchmark and dependency audit sign off
4. One timed outside Rust walkthrough
5. Phase 2 retrospective and Phase 3 kickoff issue

Timebox rule (2026-07-02, per `Plan/Plan-Amendments-2026-07.md` A4): closeout
gets one focused week; whatever is not closed stays tracked in the exit ledger
without blocking Phase 3. No new evidence recorder/comparator/checker scripts
until a second engineer joins — reuse existing harnesses. The long fuzz soak
additionally waits on the `layer36-local` runner returning (nightly schedule
paused 2026-07-02 while it is offline).

## 6) Recent completed development highlights

- TypeScript curl error behavior aligned with Rust:
  - permission denied returns exit code `5`
  - invalid URL returns exit code `20`
- Added parity tests for Rust vs TypeScript on curl denial and invalid URL paths
- Go curl classifier hardened with stable mapping and unit tests
- Expanded curl error-path parity checks to Rust, Go, and TypeScript for missing-grant, invalid-url, and unresolved-host paths
- Tightened dedicated Go curl fixture checks so missing-grant, invalid-url, and unresolved-host paths enforce expected exit codes and stderr markers
- Added a language-variant evidence recorder that writes one markdown report with fixture availability, SHA-256 hashes, command results, and log tails
- Extended the language-variant evidence recorder with `--mode`, `--output`, and `--strict` flags for CI-friendly and local evidence runs
- Added a language-variant evidence comparator that verifies commit/host metadata, step pass state, and fixture parity across Linux, macOS, and Windows reports
- Wired hosted full CI to publish per-OS language-variant evidence artifacts for easier cross-host comparison
- Added a hosted full CI compare gate that downloads Linux/macOS/Windows language-variant evidence artifacts and enforces cross-host parity
- Added a runtime deny-matrix test for non-default capabilities and an explicit net-connect deny test under default grants
- Added a shared Phase 3 `UiAdapter` trait and a draft in-memory implementation so the runtime no longer depends directly on draft window storage
- Added headless Phase 3 UI adapter entry points and blank-window smoke tests to the macOS, Linux, and Windows adapter crates
- Added runtime host UI adapter discovery so Phase 3 dispatch can use the current OS adapter entry point and report backend capability info
- Added ADR-0013, RFC-0003, a mdBook widget protocol page, and a Phase 3 design-doc checker so the native widget plus drawn fallback rule is recorded before host UI backend work grows
- Added the first shared widget tree model in `adapter-common::ui` so layout and native lowering can use stable widget IDs, kinds, labels, roles, style hints, and parent validation.
- Added a draft widget-tree dispatch path so runtime UI calls can set, update, remove, focus, and inspect widgets through the same UCap and adapter boundary before native widgets exist.
- Added the first Taffy-backed layout crate and runtime layout dispatch path so the shared widget tree can now produce stable widget rectangles before native widgets or drawn fallback rendering exist.
- Expanded Phase 3 layout proof with generated 100-shape tests, a 1k/10k-node benchmark target, absolute rectangle helpers, and a first layout hit-test helper for future input routing.
- Recorded that the first local 10k layout measurement is not an exit pass yet, so the next layout work should focus on optimization and formal benchmark evidence rather than claiming the 60 fps tree budget is done.
- Added `PreparedLayoutTree`, prepared 1k/10k benchmark lanes, and `Phase3UiDispatcher::prepare_layout` so future event loops can reuse the layout tree between widget mutations.
- Added the first runtime-side pointer event route so logical pointer coordinates can be hit-tested against layout and queued as portable UI events before real native event loops land.
- Added draft key and text input routing so focused widgets can receive portable key events and committed typed text before real native keyboard and IME event loops land.
- Added FIFO UI event polling through the shared adapter and runtime dispatcher so future app-facing `events.poll()` calls can consume one event at a time.
- Added draft host window event routes for close requests, resize, and focus changes so the first native event loop has stable queue targets.
- Added draft theme and scale event routes so dark mode and DPI changes have stable queue targets before real native windows land.
- Added an explicit `WindowAdapter` trait below `UiAdapter`, plus active/planned window backend reporting for macOS, Linux, and Windows.
- Added native window handle attach, lookup, and detach support, plus the first macOS AppKit handle handoff method.
- Added the first opt-in AppKit window prototype for macOS, with target-specific `objc2` dependencies, owned `NSWindow` lifetime, main-thread gating, shared handle attachment, and ignored local smoke coverage for opening the real native window.
- Added AppKit event bridge targets and a native snapshot helper so close, resize, focus, and scale changes can flow from the real macOS window into the shared Layer36 event queue.
- Added AppKit window session state so the native macOS prototype owns the window, caches the last snapshot, refreshes changed state, and has a clear place for real delegates to report close requests.
- Added AppKit native event state so future delegates can report close, resize, focus, scale, and snapshot events through one exported, tested Rust path.
- Added the AppKit redraw bridge so the future native drawing surface can request paint through the same shared event queue as other window events.
- Added the AppKit delegate callback bridge so Objective-C delegate methods can translate into one tested Rust event path.
- Added AppKit draw-surface state so the coming native `NSView` painter has a checked place for size, scale, clear color, redraw count, and frame metadata.
- Added AppKit draw view surface so the opt-in native prototype can attach an `NSView`, set a visible clear color, mark it dirty, and record a first frame snapshot.
- Added AppKit native window delegate object so native close, resize, focus,
  and backing-scale callbacks can be recorded in order and drained through the
  tested Rust event bridge.
- Added AppKit event-loop step driver so the opt-in native path can process one
  non-blocking tick through the shared event stream before default runtime
  wiring starts.
- Added selectable AppKit prototype runtime mode so the runtime can request the
  native prototype by name while preserving the default headless path.
- Added the shared event-loop pump boundary so any host adapter can expose one
  non-blocking native UI tick through the runtime dispatcher.
- Added the selectable AppKit runtime smoke command so the full opt-in macOS
  native prototype path can be checked locally without making normal CI open a
  desktop window.
- Added a UCap enforcement evidence recorder and cross-host comparator (`record-phase2-ucap-evidence` + `compare-phase2-ucap-evidence`)
- Wired hosted full CI to upload per-OS UCap evidence artifacts and run a dedicated cross-host compare gate
- Added a benchmark evidence recorder and comparator (`record-phase2-benchmark-evidence` + `compare-phase2-benchmark-evidence`) to track startup and dispatch performance evidence in one per-host report
- Tightened benchmark evidence comparison so each host report must also stay within per-metric baseline thresholds, not only match report shape and step pass state
- Added full external CLI startup evidence for `layer36 run layer36-clock`; the benchmark evidence report now checks the real command path, not only the in-process runtime path
- Added a dependency evidence recorder so Phase 2 `cargo-deny` signoff records tool versions, advisory status, license/bans/source status, and log tails
- Added a Go readiness evidence recorder so TinyGo smoke builds, artifact hashes, tool versions, and current import-purity blockers are recorded in one report
- Added an adapter evidence recorder and comparator (`record-phase2-adapter-evidence` + `compare-phase2-adapter-evidence`) to track adapter-boundary proof per host and compare Linux/macOS/Windows reports for one commit
- Expanded adapter evidence so each host report now records shared adapter behavior tests and the native adapter crate test for that host
- Wired hosted full CI to publish per-OS adapter evidence artifacts and run a dedicated cross-host adapter evidence compare gate
- Wired hosted full CI to publish per-OS sample evidence artifacts and run a cross-host sample evidence compare gate (with temporary curl-blocked fallback)
- Hardened self-hosted fuzz nightly concurrency so scheduled runs no longer cancel older queued runs when the local runner is offline
- Added a dedicated benchmark evidence docs page and linked it from Phase 2 and the exit ledger for P2E-10 and P2E-11 tracking
- Added a Rust SDK evidence recorder and hosted artifact path so P2E-03 has one proof file for package smoke, SDK doc build, and packaged-file presence
- Added a Phase 2 exit bundle recorder so local review can capture UAPI, adapter, exit-ledger, docs, gate snapshot, working tree state, and log tails in one report
- Added a UAPI freeze candidate lock with per-WIT SHA-256 hashes, a checker, and CI freshness wiring so contract drift is visible before and after the final freeze decision
- Added a Phase 2 UAPI freeze-review evidence recorder so the freeze candidate can be checked as one report before the final human decision
- Wired the self-hosted full gate to regenerate and check the UAPI freeze lock, then record a freeze-review evidence artifact
- Added a timed Rust walkthrough evidence template so the outside developer proof can be recorded against a specific commit
- Opted GitHub Actions workflows into the Node 24 JavaScript action runtime and moved cache/artifact/Pages upload steps to Node 24 action majors where available
- Added a Phase 2 exit readiness command so the current gate count and hard blockers can be checked from the ledger without reading the whole page by hand
- Recorded the Go Phase 2 decision: Go remains in the SDK and TinyGo smoke-build track, but runtime parity is experimental until artifacts import only `layer36:*`
- Added a hosted CI stability evidence recorder so recent CI and Pages run history can be attached to Phase 2 exit review
- Added a timed walkthrough evidence checker so the outside Rust walkthrough packet must have filled metadata, numeric timing, a pass/fail result, and reviewer results before `P2E-12` can be accepted
- Added the Phase 2 retrospective draft, Phase 3 kickoff issue draft, and a closeout-docs checker so handoff material exists without claiming Phase 2 is complete early
- Wired the closeout-docs checker into hosted CI, self-hosted CI, and the Phase 2 exit bundle
- Added a UAPI freeze decision packet and checker so the final freeze decision stays explicit until the remaining proof is ready
- Added an optional exit-bundle mode that records hosted CI and Pages stability evidence alongside the local Phase 2 proof
- Added ignore rules for generated demo outputs and local quickstart fixture files so source-status checks stay clean after manual proof runs
- Added a self-hosted full-gate evidence recorder and optional exit-bundle inclusion path for local runner proof
- Added strict self-hosted evidence checking so final bundles fail when no completed green self-hosted full gate is present in inspected history, with an optional date-window filter for final candidate proof
- Added strict hosted CI stability checking so final bundles fail when hosted CI or Pages has no completed green run in the selected review window
- Added a final-review exit bundle shortcut so the fuller Phase 2 review packet can be collected with one command
- Added a fuzz evidence recorder and exit-bundle inclusion path so smoke and longer soak runs can be reviewed as markdown proof
- Added a full Phase 2 readiness mode and wired it into the exit bundle so review packets list every unfinished gate with its next step
- Added a Rust walkthrough rehearsal script and exit-bundle row so the reviewer path can be checked locally before the outside walkthrough
- Added a hosted full CI evidence recorder so normal fast CI is not mistaken for Linux, macOS, Windows cross-host proof
- Fixed hosted full CI sample manifest fixture setup so downloaded shared Rust fixtures are copied into the app target paths used by the sample manifests
- Recorded the Windows command-line limit for the oversized raw-args guard test so full CI can keep proving reachable behavior on each host
- Fixed the hosted full CI evidence recorder so cancelled or failed full runs are shown accurately in the selected-run summary
- Hardened the local HTTP fixture used by curl response-limit tests so Windows early client close behavior does not hide the Layer36 assertion
- Fixed Windows sandbox resolution for absolute Layer36 logical paths by converting normalized logical strings into relative sandbox segments before host path joining
- Hardened sample evidence recording so hosted full CI reuses shared downloaded fixture bytes instead of rebuilding with lane-local `cargo-component`
- Fixed Windows sample evidence recording so the hosted full-test lane can use `target/debug/layer36.exe` explicitly under Git Bash while Linux and macOS continue using `target/debug/layer36`
- Fixed language-variant evidence comparison so it records Windows fixture hashes correctly and checks portable behavior without claiming byte-identical jco output across hosts
- Recorded hosted full CI run `26069665276` as green for the full Linux, macOS, Windows Phase 2 evidence matrix
- Expanded UCap evidence with a named dispatcher deny-before-adapter matrix that covers every non-default filesystem and network boundary
- Hosted workflows moved to Node 24 ready action versions
- Started Phase 3 at the contract layer with parseable WIT for the GUI world,
  `ui`, `gfx`, and `audio`, plus a checker and docs page
- Added Phase 3 GUI manifest recognition so tooling can validate the `gui`
  world before the window runtime exists
- Added the first Phase 3 capability strings for window creation, dialogs,
  clipboard, drop zones, GPU access, and audio. Safe basics are default grants;
  clipboard, GPU compute, and audio capture stay explicit.
- Added a shared draft UI adapter model in `adapter-common` so Phase 3 window
  lifecycle behavior can be tested before native host adapters are wired.
- Added `runtime::phase3_ui`, the first runtime UI dispatcher scaffold. It
  gates draft window operations through UCap and keeps unsupported clipboard
  paths permission-checked.
- WIT contract comments added across Phase 2 UAPI and enforced by `check-uapi`
- Generated UAPI reference now includes those WIT contract comments
- Rust SDK package smoke now verifies packaged README, SDK root, and generated bindings files
- Go TinyGo smoke artifacts build locally, but promotion correctly blocks them because they still import WASI host APIs
- Component import checker now reports all failing artifacts in one run, which improves Go runtime-proof triage
- Added a Phase 2 UAPI freeze-review page with checklist and commands
- Added a repeatable UAPI freeze-evidence snapshot page generated from `check-uapi`
- Wired hosted and self-hosted CI to fail when that freeze-evidence page is stale
- Added a runtime adapter-boundary guard for 34 host wrappers across Linux, macOS, and Windows adapter crates
- Added a Phase 2 exit-evidence ledger that tracks all 15 exit gates with status, proof source, and next step
- Added a Phase 2 Rust sample evidence recorder for clock, cat, and curl stdout/hash proof across hosts
- Added a Phase 2 cross-host sample evidence comparator for Linux/macOS/Windows report parity checks
- Hardened sample-evidence comparison with host-label validation so `--linux`, `--macos`, and `--windows` cannot silently point to the wrong host report
- Hardened sample-evidence comparison with same-commit validation so cross-host reports must come from one code revision
- Tightened sample-evidence comparison so `--allow-blocked-curl` still compares curl stdout hashes across hosts where curl did run

## 7) Source of truth files

- Plan: `Plan/Phase-2-Plan.md`
- Current phase plan: `Plan/Phase-3-Plan.md`
- Phase docs page: `docs/book/src/phases/phase-2.md`
- Phase 3 docs page: `docs/book/src/phases/phase-3.md`
- Phase 3 widget protocol page: `docs/book/src/phase3/widget-protocol.md`
- Phase 3 layout page: `docs/book/src/phase3/layout.md`
- Progress page for non technical readers: `docs/book/src/progress-for-everyone.md`

## 8) Resume prompt for a new session

Use this exact prompt in a new session:

`Continue Layer36 on main. Read Plan/Plan-Amendments-2026-07.md FIRST — it supersedes conflicting instructions in older plan text — then STATUS.md and Plan/Phase-3-Plan.md. Phase 3 has the full contract-and-prototype layer landed: WIT drafts, GUI manifests, capability names, shared widget tree, Taffy-backed layer36-layout with PreparedLayoutTree and hit testing, runtime::phase3_ui dispatcher with UCap gating, draft pointer/key/text/host-window/theme/scale event routes with FIFO polling, headless adapters on all three OSes, an opt-in macOS AppKit window prototype (owned NSWindow, native delegate, draw view, event-loop step driver, selectable runtime mode, local smoke command), and guarded Linux/Windows Winit scaffolding (session owner, callback collector — no real Winit windows yet). Next Phase 3 work is P3-VS-01, the macOS vertical slice: one WASM component drives a real AppKit window containing a native NSButton and NSTextField and receives the click event back end-to-end through UCap and the Phase 3 dispatcher (task spec in Plan/Phase-3-Plan.md section 19). Linux widget lowering is drawn-fallback-only per docs/adr/0015-linux-widget-strategy.md; Winit window broadening resumes only after P3-VS-01 passes; evidence tooling is frozen per amendment A4 (no new recorder/comparator/checker scripts); the agent-embedding tasks P3-EMB-01..03 follow the slice. Prepared 10k layout is locally under budget, but cold rebuild and formal cross-host evidence remain pending. Keep Phase 2 closeout separate and timeboxed, keep Phase 3 narrow, update STATUS.md and plan docs after each chunk, update the STATUS hiring lane, keep GitHub Pages in sync, check CI after every push, and never add AI co-author credits to commits.`
