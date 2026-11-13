# The web Player: a Krate app in a browser tab

IC-956. Phase 1 (this section) builds on the Phase 0 spike (below). Contract:
`Plan/org-pilot/ORG-FIRST-PLAN.md`, "IC-956 contract" and "IC-956 Phase 0
result". Measured 2026-10-15 on this Mac (M-series, macOS 27.0 beta),
headless Playwright 1.63.0 browsers only, against the real hub Worker code
run in Node (`crates/adapter-web/player/hub-local.mjs`). Raw:
`phase1.json`; screenshots: `phase1/`.

## Phase 1: what runs

A link `https://hub.krate.tech/run/<hash>` opens the app in the tab, nothing
installed. In order:

1. **The page** (`/run/<hash>`, served by the hub Worker with COOP
   same-origin + COEP require-corp + a CSP that allows scripts only from the
   hub and the Blob the glue becomes) fetches `/a/<hash>/web`: the bundle and
   its web pack in one answer, behind exactly `/a/<hash>`'s checks.
2. **The checks, in the tab**: the bytes hash to the link's address; the
   bundle passes `crates/validator-wasm` (the hub's own validator, now with
   `krate_judge_for_tab`), signature included -- a bundle changed after
   signing is refused; every core module about to run is byte-for-byte one
   of the judged component's own modules (the validator lists their
   SHA-256), so whoever served the files, what runs is what was judged.
3. **Consent** in the site's design, with the desktop's own words
   (`Capability::plain`) and grants (`krate-policy`). A capability a tab
   cannot give is greyed with "works in Krate on your computer"; a required
   one turns the button into "Open in Krate". An app that asks for nothing
   beyond a window opens with no screen, as on desktop.
4. **The app** runs in a Web Worker on the browser's own engine (jco glue +
   its core modules). `run()` blocks for real: `events.wait` on
   `Atomics.wait` over an input ring; async host work (fetch, IndexedDB,
   the clipboard, open-url) goes to the page over a **bridge**
   (`web/bridge.js`): the worker posts the call and blocks, the page answers
   into shared memory, in chunks of 1 MiB so an answer is never bounded by
   the buffer. Frames go to the page through a shared frame buffer; the
   page wakes on a shared counter with `Atomics.waitAsync` (the worker's
   `frame` message arrives ~9 ms late in Chromium, measured: the Phase 0
   p95 note).

**The `ui` host is the desktop's.** The runtime's tree-to-widgets step moved
out of `crates/runtime/src/phase3_gui_host.rs` into a new crate,
`crates/lowering` (layout with text measurement, placements, scroll
offsets and their clamp, press-to-focus, Enter/Space activation, the node
rules, and the `canvas2d` rasterizer), which the runtime now calls and the
Player compiles to wasm32 (`crates/adapter-web`, 3.29 MB / 1.18 MB gzip).
Windows, trees, focus and the event queue are the `DraftUiAdapter` the
desktop runs headless; the pixels are the painter with vector text (parley +
vello_cpu) in Geist Regular, bundled (OFL 1.1). Light and dark follow the
browser; a window takes the tab's room up to what the app asked for, and an
app that never asked its canvas size draws in the size it asked for.

**Hosts in a tab**: io (stdout, stderr, args from `?arg=`, log; stdin
closed), random, time (clock, monotonic, sleep), locale (Intl),
`net/http-client` (get, fetch, begin/poll/cancel) through the bridge with
the desktop's `net.connect` judgement unchanged (`parse_url_endpoint`, the
capability check, `check_app_http_header`, the 1 MiB response bound),
`redirect: 'manual'` and `credentials: 'omit'`; `store/kv` under the
desktop store's rules (`krate_adapter_common::kv`, now shared) over
IndexedDB, written through before the app reads its own write back; `ui`
window, tree, image, events, pointer, text-entry, screen, appearance,
clipboard (navigator.clipboard), launcher (open-url); `gfx/canvas2d` and
`canvas-images` (the desktop rasterizer). **Everything else** answers from a
refusal table the hub made at publish (each imported function's own error
case, e.g. `unsupported("... Open the app in Krate to use it.")`, or a
neutral answer where the function has no error channel), and the page shows
the person one line: "<App> wanted to use <thing>, which works in Krate on
your computer, not in a tab yet. [Open in Krate]". `store.org` is one of
these: the browser keyring (crates/orgkeys as a browser device) had not
landed on this base.

**The hub** (`cloud/worker/src/web-player.js`): at publish and at an
organisation's candidate upload, the judged component goes through
`crates/web-transpile` -- js-component-bindgen 2.12.0, the engine of jco
1.35.0, compiled to wasm with no imports like the validator -- and the pack
is stored at `web/jcb-2.12.0/<hash>` in R2, once (idempotent; a release
published earlier gets its pack on its first open). `/a/<hash>/web` serves
it behind `/a/`'s own takedown and organisation checks (refactored into one
`privateAnswer` both use, so an open in a tab is audited as the download it
is). A private app's tab signs in on the hub (`from=player:<hash>`) and
comes back to `/run/<hash>#token...` with its own nonce, as
`krate.tech/login/done` does. **The door**: `krate.tech/open/?a=<hash>`
offers "Open in this tab" beside the download (the primary button on a
phone).

## Phase 1: what was proved (`phase1.mjs`, 58 of 59 checks; Firefox did not start)

In Chromium 153 and WebKit 26.6 each:

- krate-curl (CLI): asks in the desktop words; fetches through the bridge
  and prints the body; a 302 is not followed (the echo server saw
  `/redirect` and never `/elsewhere`); a host it was not granted is refused
  before anything leaves the tab (`permission denied`, nothing reached the
  server).
- krate-hello-gui: the button, the checkbox, typing into the field and
  Backspace, a list row chosen, the scroll area scrolled by the wheel,
  dark mode followed, Close asked the app (which reported `typed:hi krat`),
  no page errors; at 375 wide the window fits with no sideways scroll.
- krate-bigscroll: 500 rows scrolled to the last one, and one more turn
  moves nothing (needed a layout fix, K-1070, below).
- krate-convert: every preset button answers.
- krate-checklist (canvas2d, store.kv, text input): typed "Buy bread" +
  Enter, saved through the store to IndexedDB; reopened in a new tab with
  the same storage, it has the four items.
- Guards: a bundle changed after signing refused ("This app was changed
  after it was signed"); bytes that are not the ones the link names refused
  ("changed on the way"); a required camera capability refused in words
  before running, the button "Open in Krate"; store.org refused at the call
  in words, the app told `unsupported`, the page says so; a signed app says
  "Signed by acme".
- A private organisation release: signed out, the tab asks for sign-in that
  starts on the hub and comes back to this app; a member (dan) opens it; a
  stranger is refused "This app was not shared with you" and learns no
  organisation.

Sabotage, each run and watched failing, then restored: tampered bundle
accepted (signature check removed), redirect followed (`redirect:
'follow'`), refused capability allowed (blocked list emptied), swapped bytes
accepted (hash check removed) -- each failed exactly its check; in Rust, the
`net.connect` check removed and camera marked tab-capable failed both
`web_policy` tests; the hub's private check skipped failed the
private-release hub test; the scroll minimum removed failed the layout test.

## Phase 1 numbers

Median of 10 cold runs each (new browser context, no HTTP cache), from
navigation start. Click-to-frame: pointerdown to the putImageData that
showed the change, 20 checkbox clicks; "painted" is the worker's half (the
frame written to shared memory). The machine was shared with other agents'
builds (load average 11 at the start, 9 at the end), so absolute numbers carry noise.

| | Chromium 153 | WebKit 26.6 |
|---|---|---|
| hello-gui link to first frame (no consent) | 130 (119-192) ms | 237 (194-290) ms |
| krate-curl link to consent shown | 81 (76-104) ms | 202 (163-241) ms |
| krate-curl Allow to first output (fetch included) | 55 (48-58) ms | 21 (18-25) ms |
| checklist Allow to first canvas frame | 105 (95-124) ms | 55 (46-62) ms |
| click to frame, median / p95 | 1.66 / 2.14 ms | 2.26 / 4.38 ms |
| click to painted (worker), median / p95 | 1.64 / 2.11 ms | 2.16 / 4.28 ms |

Sizes: Player module 3.29 MB (1.18 MB gzip, includes the 58 KB font and
vello_cpu); validator 0.90 MB (0.34 MB); page scripts 70 KB (22 KB);
hello-gui's web pack 0.30 MB. The hub Worker grows from about 2.6 MB to
6.97 MB uploaded (1.80 MB gzip) with the 4.37 MB transpiler (1.13 MB gzip),
under the 3 MB free-plan and 10 MB paid limits.

## Phase 1: what does not work yet

- **Firefox**: Playwright's Firefox still does not launch on this macOS 27
  beta ("Failed to launch the browser process"); run `phase1.mjs` on a
  released OS.
- Interfaces answered "open it in Krate" in a tab: the microphone (sound
  out plays since K-1134, `player/audio-proof.mjs`), camera, speech,
  gpu, scene3d, video, files (fs), store.sql/secret/shared/group/org,
  net/ws and sockets, resources/assets, dialogs, notifications, reminders,
  print, menus, location, bluetooth, photos, share, sign-in, web-page.
- Text entry is keyboard events plus a hidden field for phone keyboards;
  no IME composition, no selection or caret drawing by the host.
- No accessibility tree yet (`on_screen` is the start of one).
- A private app asks for sign-in again on the hub: the krate.tech session
  is on another origin.
- Non-Latin text falls back to missing glyphs (one bundled Latin font).
- A page on https cannot fetch a plain http address (mixed content); the
  app is told so in words.
- A redirect's target is hidden from a tab (`opaqueredirect`), so the app
  gets an error in words, not the 3xx the desktop gives it.

## Rebuild and re-measure (Phase 1)

```
sh crates/adapter-web/player/build.sh     # apps, validator, transpiler, Player, bundles, browsers
cd crates/adapter-web/player
node phase1.mjs ../../../evidence/web-player/phase1.json 10
node open-door.mjs                        # the krate.tech /open/ button
```

To deploy (the founder's step): `scripts/build-hub-validator.sh`, then
`scripts/build-web-player.sh` (writes `cloud/worker/src/transpiler.wasm`,
which the Worker now imports, and `cloud/worker/player-dist/`, its
`[assets]`), then `wrangler deploy`. The transpiler crate pins Rust 1.96
(its `rust-toolchain.toml`; wasmtime-environ 49 needs it).

---

## Phase 0: the measured spike

IC-956 Phase 0, the measured spike (contract: `Plan/org-pilot/ORG-FIRST-PLAN.md`,
"IC-956 contract"). Question: can a Krate app open in a tab in about a second
while the same component still runs natively? Measured 2026-10-14 on this Mac
(M-series, macOS 27.0 beta 26A428), headless Playwright browsers only.

Short answer: yes for the two apps tried. The real components of
`apps/krate-diceroll` (cli world) and `apps/krate-hello-gui` (gui world) run
unmodified on the browser's own WebAssembly engine after `jco transpile`, and
reach first output / first frame in 40-65 ms in Chromium and 170-245 ms in
WebKit (where an empty page alone took 300 ms in the same run). A click on
hello-gui's checkbox reaches the canvas in 1-3 ms median. Firefox could not be
measured on this machine (see "What did not work").

## What runs

- **The app**: the same `.wasm` component a `.krate` carries, transpiled by
  jco 1.35.0 to core wasm (20-24 KB) plus JS glue (155-272 KB, 31-39 KB
  gzipped). Nothing in the app changed.
- **Hosts** (`crates/adapter-web/player/web/host.js`): only what the two
  apps import -- `krate:io` args/stdout/stderr, `krate:random/bytes`
  (crypto.getRandomValues), `krate:ui` window/tree/events. A grant set
  stands in for the session's capabilities; a refused call answers `denied`.
- **Drawing**: `krate-layout` + `krate-adapter-common` compiled to wasm32
  (`crates/adapter-web/src/web_ui.rs`, 405 KB, 138 KB gzipped), painting the
  app's widget tree into RGBA that `putImageData` takes, and hit-testing
  presses with `krate_layout::hit_test` so `pointer.widget` is right. Same
  geometry as the native `--shoot` frame; the text is the 5x7 bitmap painter
  (the native shot uses vello vector text).
- **The blocking loop, both ways**:
  1. *JSPI* (`web/gui-jspi.html`): jco `--async-mode jspi`, `events.wait`
     returns a Promise, run() suspends on the main thread.
  2. *Worker + Atomics* (`web/gui-worker.html` + `gui-worker.js`): run()
     blocks in a Web Worker on `Atomics.wait` over a SharedArrayBuffer ring
     the page writes input into; the worker paints into a shared frame
     buffer and the page puts it on the canvas.
- **The comparison**: wasmtime 46.0.3 with pulley, itself compiled to wasm32
  (`crates/adapter-web/pulley-probe`), interpreting diceroll precompiled for
  pulley32 (`web/cli-pulley.html`). It runs.

## Numbers

Median of 10 cold runs per row: each run is a new browser context (empty
cache), the server sends `Cache-Control: no-store`, localhost (no network
time). Cold start = `performance.now()` from navigation start to the first
stdout text (CLI) or the first frame on the page's canvas (GUI). Latency =
pointerdown event timestamp to the frame showing the app's change on the
page's canvas, 10 checkbox clicks per run (100 per row). Memory = RSS of all
Playwright browser processes on the app page minus the same context on
about:blank (one sample, noisy), plus the wasm linear memories. The machine
was shared with other agents' builds (load average 7-10 during the run),
which is why the min-max spread is wide. Raw: `browser-runs.json`.

| Path | Browser | Cold start median (min-max) | Click to frame median / p95 | RSS delta | wasm memory | Works |
|---|---|---|---|---|---|---|
| empty page (the floor) | Chromium 153 | 34.1 ms (31.4-40.8) | -- | 17 MB | 0 | yes |
| CLI, jco, main thread | Chromium 153 | 39.3 ms (38.5-49.4) | -- | 20 MB | 1.1 MB | yes |
| CLI, pulley in wasm | Chromium 153 | 45.0 ms (43.1-61.5) | -- | 45 MB | 12.3 MB | yes |
| GUI, JSPI, main thread | Chromium 153 | 59.9 ms (51.7-152.0) | 1.02 / 2.24 ms | 54 MB | 4.6 MB | yes |
| GUI, Worker + Atomics | Chromium 153 | 65.1 ms (54.9-168.1) | 1.18 / 14.89 ms | 27 MB | 2.1 MB | yes |
| empty page (the floor) | WebKit 26.6 | 298.4 ms (213.7-363.3) | -- | 6 MB | 0 | yes |
| CLI, jco, main thread | WebKit 26.6 | 242.5 ms (176.9-287.0) | -- | 14 MB | 1.1 MB | yes |
| CLI, pulley in wasm | WebKit 26.6 | 258.3 ms (145.7-291.1) | -- | 36 MB | 12.3 MB | yes |
| GUI, JSPI, main thread | WebKit 26.6 | 230.4 ms (177.6-403.7) | 1.88 / 5.84 ms | 58 MB | 4.6 MB | yes |
| GUI, Worker + Atomics | WebKit 26.6 | 168.7 ms (156.6-200.0) | 2.66 / 8.76 ms | 61 MB | 2.1 MB | yes |
| all paths | Firefox | not measured | -- | -- | -- | browser did not start |

WebKit's floor is the new context's first page load itself: an empty module
page took longer than the Krate pages in the same run, so in WebKit the Krate
part is lost in the browser's own start-up noise.

**Native, same components**, `krate 0.5.5` release build (the redesign
worktree's `target/release/krate`, built 2026-10-12), 10 runs each, two
sessions under different load (`native-runs.json` is the second):

| App | What | Median | Peak RSS |
|---|---|---|---|
| diceroll | `krate run --headless` spawn to first stdout | 8.0 ms / 21.9 ms | 22 MB |
| hello-gui | `krate run --shoot x.png --shoot-scale 1` spawn to PNG on disk | 154 ms / 201 ms | 46 MB |
| hello-gui | click to frame | not measured: needs a visible window, not allowed during this spike | -- |

The native GUI figure includes vello vector-text painting and PNG encoding,
so it is not a like-for-like frame time; the CLI figure uses Krate's
compiled-code cache (warm).

**Sizes a link would download** (raw / gzip -9): diceroll glue 155 KB / 31 KB
+ core 24 KB / 11 KB; hello-gui glue 272 KB / 38 KB + core 20 KB / 8 KB;
painter 405 KB / 138 KB; host.js 7 KB / 3 KB. Pulley instead: engine 1,086 KB
/ 330 KB + precompiled app 149 KB / 35 KB.

**jco transpile cost** (in-process, warm, node, median of 10):
diceroll 12.4 ms, hello-gui 26.4 ms (`transpile-time.mjs`). That is the cost
of producing the glue once per app version, wherever it is done.

## Cross-origin isolation (COOP/COEP), per browser

The Worker path needs `SharedArrayBuffer`, which needs the page to be
cross-origin isolated. Asked four ways (`coi-probe.mjs`, raw:
`isolation-probe.json`):

| Headers | Chromium 153 | WebKit 26.6 | Firefox |
|---|---|---|---|
| COOP same-origin + COEP require-corp | isolated | isolated | not measured |
| COOP same-origin + COEP credentialless | isolated | NOT isolated | not measured |
| none (what GitHub Pages sends) | not isolated | not isolated | not measured |
| none + a service worker adding the headers (`web/coi-sw.js`, applies from the second load) | isolated | isolated | not measured |

So `require-corp` is the one that works on both measured engines;
`credentialless` does not work in WebKit.

Where the headers can come from:

- **The hub Worker** (`hub.krate.tech`, a Cloudflare Worker custom domain):
  it writes its own responses, so a Player page served from it can carry
  COOP/COEP with no founder step. This is the straightforward door.
- **krate.tech itself**: measured today, it is served straight from GitHub
  (`server: GitHub.com`, no `cf-ray` header), i.e. the apex record is not
  proxied through Cloudflare, so a Cloudflare header rule would not apply
  until the record is switched to proxied (a founder step, and it changes
  how the whole site is served).
- **A service worker** on GitHub Pages: works in both measured engines, but
  only from the second load (the first load registers it, then the page must
  reload), which costs the "opens in about a second" first visit.

## What did not work, and why

- **Firefox: not measured.** Playwright 1.63.0's Firefox 155 build exits at
  once on this macOS 27 beta: `Could not find profile folder.` (also with an
  explicit `-profile` directory and `XRE_PROFILE_PATH`). Playwright 1.62.1's
  Firefox 153 starts but never connects (`sandbox_extension_issue_file_to_process
  failed ... plugin-container.app: Operation not permitted`, then the launch
  times out), also with its content sandbox turned off in the throwaway
  profile. Class: environment (a beta OS). Docker was not running, so no
  Linux run either. Phase 0's exit asks for Firefox; it needs a run on a
  released macOS, Linux or Windows: `node measure.mjs` and
  `node coi-probe.mjs` there, nothing else changes.
- **OffscreenCanvas from a blocked worker never reaches the page.** Proved
  with a control (`web/offscreen-control.html`): a worker that fills its
  OffscreenCanvas red and returns shows red; the same worker that then sits
  in `Atomics.wait` shows white, in Chromium and WebKit. A Krate app's run()
  never returns to the worker's event loop, so the worker cannot present
  through OffscreenCanvas. The Worker path must hand frames to the page (the
  shared frame buffer used here works in both).
- **The Worker path's p95 in Chromium is ~15 ms** against 2 ms for JSPI; the
  median is the same (1.2 ms vs 1.0 ms). Likely the page receiving the
  worker's "frame ready" message at its next rendering opportunity; not
  investigated further.
- **jco's glue is large for a tiny app** (155-272 KB raw), mostly runtime
  boilerplate; 31-39 KB gzipped.
- **Native click-to-frame**: not measured (above).

## Recommendation (the contract's decision rule)

Rule: Worker + Atomics if it runs on all three; JSPI as the fast path where
present only if it is measurably better.

- Worker + Atomics runs in Chromium and WebKit; Firefox is unverified here
  (environment), not failed.
- JSPI also ran in Playwright's WebKit 26.6 build (not only Chromium), so
  this spike cannot claim which shipping Safari has it.
- JSPI is not measurably better at the median: cold start is within noise of
  the Worker path in both engines (Chromium 59.9 vs 65.1 ms; WebKit 230.4 vs
  168.7 ms), click-to-frame medians are 1-3 ms either way. Its one edge is
  Chromium's p95 (2.2 vs 14.9 ms), under one 60 Hz frame. Against that, JSPI
  runs the app on the page's main thread, so an app that computes without
  calling `wait` freezes the tab.

**Choose Worker + Atomics with the shared frame buffer, served from the hub
Worker with COOP same-origin + COEP require-corp; do not build JSPI as a
second path now.** Confirm Firefox on another machine before Phase 1 starts,
and treat the Chromium p95 as a Phase 1 item.

The pulley-in-wasm comparison runs and is close on this tiny CLI app (45 vs
39 ms in Chromium) but carries a 1.1 MB engine, 12 MB of wasm memory and 2x
the RSS, and interprets every instruction; diceroll does too little to show
the interpreter's cost. Running on the browser's engine is the better road.

## What Phase 1 has to build (estimate, not measured)

- **The hosts for the Worker**: every `krate:*` interface the phase-2 and
  phase-4 worlds import, in the worker. Async browser work (fetch for
  `net.http`, IndexedDB, the hub for `store.org`) cannot complete inside a
  worker that is blocked in `Atomics.wait`, so it must run on the page or a
  helper worker that writes the answer into shared memory -- this bridge is
  the core of Phase 1. OPFS sync access handles work inside the worker for
  `store.kv` and files. Anything with no browser equivalent refused with
  words that say "open it in Krate".
- **The real lowering**: replace `web_ui.rs` (the plain case only: no
  scroll offsets, focus, text measurement or vector text) with the runtime's
  own lowering from `phase3_gui_host.rs`, extracted so it builds for wasm32
  without `krate-runtime`'s GPU and OS dependencies, plus a bundled font for
  vector text.
- **Opening a `.krate` in the tab**: unzip, check the signature with
  `crates/validator-wasm` (the hub's validator), read the manifest for the
  consent screen and the grants, and get the jco glue -- produced once per
  version at publish (12-26 ms here) and stored beside the bundle, or the
  3.5 MB transpiler loaded into the tab.
- **The doors**: `/open/?a=<hash>` and the private org link on
  `hub.krate.tech` serving the Player with COOP/COEP, the consent screen,
  installed Krate offered beside it.
- **Size**: big. For the phase-2 hosts plus `ui` at the depth hello-gui
  uses, my estimate is several thousand lines of JS and Rust and a few weeks
  of agent work; the phase-4 media and GPU interfaces (canvas, audio,
  camera, gpu) are each further work on top.

## Two findings outside this spike

- `crates/runtime/src/lib.rs` `engine_settings` forces `pulley64` when built
  for wasm32, and `precompile_for_web` emits pulley64. A wasm32 host is
  pulley32: the probe got `target 'pulley64-unknown-unknown-elf' specified in
  the configuration does not match the host` until both ends said pulley32.
  The old `runtime_probe` never reached this check (it panicked on the clock
  first).
- `crates/adapter-web`'s wasm32 build had stopped compiling (two
  `WidgetStyle` literals missing new fields, one clippy lint) because nothing
  builds it for wasm32 in CI. Fixed here; the engine probe moved behind an
  `engine-probe` feature so the painter builds without wasmtime.

## Rebuild and re-measure (Phase 0)

Phase 1 replaced the Phase 0 pages that drove the spike's painter
(`host.js`, `gui-worker.*`, `gui-jspi.html`, `cli.html`, `smoke.mjs`,
`measure.mjs`); they are in git at 5f8920291. The isolation probe and the
pulley comparison below still run as they were.

```
sh crates/adapter-web/player/build.sh          # apps, jco glue, painter, pulley probe, browsers
cd crates/adapter-web/player
node server.mjs 8861 &                          # /coi /cred /plain header modes
node smoke.mjs   http://127.0.0.1:8861          # does each path run; screenshots in build/
node measure.mjs http://127.0.0.1:8861 chromium,webkit,firefox 10 > browser-runs.json
node coi-probe.mjs http://127.0.0.1:8861        # isolation + OffscreenCanvas control
node native.mjs  <absolute path to krate> 10    # the native numbers
node transpile-time.mjs <component.wasm>...
```

Pinned: `@bytecodealliance/jco` 1.35.0 and `playwright` 1.63.0
(`package.json`, `package-lock.json`); wasm-bindgen 0.2.120 (matches
Cargo.lock); wasmtime =46.0.3 in the probe. Nothing under `build/`,
`node_modules/` or `target/` is committed.
