# Layer36 Status

Last updated: 2026-05-21
Repo: `incyashraj/layer6x6`
Branch: `main`
Latest checked completed push before this slice: `9cd3e55`
Working tree at this status update: Phase 3 runtime host UI adapter discovery slice in progress

## 1) Project size today

- Commits after this slice lands: about 321
- Tracked files after this slice lands: about 320
- Total tracked lines after this slice lands: about 88,000
- Rust lines (`.rs`) after this slice lands: about 41,700
- Docs lines (`.md`) after this slice lands: about 29,100

## 2) Latest CI and Pages state

Latest completed push (`9cd3e55`) checks:

- CI: success (run `26177194810`)
- Deploy docs to GitHub Pages: success (run `26177194868`)

Manual hosted full CI run `26069665276` passed on commit `3f1a219`.
Linux, macOS, and Windows full-test lanes all passed. The language-variant,
UCap, adapter, and sample evidence compare jobs all passed too. This closes the
immediate hosted full CI blocker that was left after run `26064573902`.

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
- `adapter-common::ui` now has an in-memory draft window registry and a shared
  `UiAdapter` trait for window IDs, title and size validation, show, redraw,
  close, events, and draft clipboard behavior
- `runtime::phase3_ui` now exposes the first runtime-facing UI dispatcher path:
  window create/show/resize/redraw/close goes through UCap before touching the
  shared adapter trait, and clipboard checks fail at the permission boundary
  before unsupported host code is reached
- `adapter-macos`, `adapter-linux`, and `adapter-windows` now expose Phase 3 UI
  adapter entry points. These are headless draft adapters today, with
  blank-window smoke tests, not native OS windows yet.
- `runtime::phase3_ui` now has `Phase3UiRuntime::with_host_adapter`, which
  selects the current OS adapter entry point and reports adapter capability info
  such as host family, backend name, and whether native windows are enabled.

This does not mean desktop UI is implemented yet. It means the first public
contract for desktop UI work is now in the repo and checked locally. The new UI
adapter trait, registry, dispatcher, host entry points, and runtime discovery
path are shared models for host adapters to follow before we wire real AppKit,
Win32, or GTK windows.

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
- Progress page for non technical readers: `docs/book/src/progress-for-everyone.md`

## 8) Resume prompt for a new GPT session

Use this exact prompt in a new session:

`Continue Layer36 on main. Start with STATUS.md, Plan/Phase-2-Plan.md, and Plan/Phase-3-Plan.md. Phase 3 has started with WIT, GUI manifest recognition, Phase 3 capability names, an adapter-common draft window registry, a shared UiAdapter trait, runtime::phase3_ui dispatcher scaffolding, headless UI adapter entry points in the macOS, Linux, and Windows crates, and runtime host UI adapter discovery. Keep Phase 2 closeout evidence separate, keep Phase 3 narrow, update plan/docs after each chunk, keep GitHub Pages in sync, and check CI after every push.`
