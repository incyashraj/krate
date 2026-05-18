# Layer36 Status

Last updated: 2026-05-18
Repo: `incyashraj/layer6x6`
Branch: `main`
Latest checked completed push before this slice: `5043394`
Working tree at this status update: hosted full CI Windows localhost fixture fix in progress

## 1) Project size today

- Commits after this slice lands: about 306
- Tracked files after this slice lands: about 305
- Total tracked lines after this slice lands: about 85,198
- Rust lines (`.rs`) after this slice lands: about 40,232
- Docs lines (`.md`) after this slice lands: about 28,723

## 2) Latest CI and Pages state

Latest completed push (`5043394`) checks:

- CI: success (run `26045159687`)
- Deploy docs to GitHub Pages: success (run `26045159122`)

Manual hosted full CI run `26045334281` confirmed the shared fixture-path fix
and the Windows raw-args host-limit fix. It then found a Windows-only localhost
fixture issue in `configured_layer36_curl_component_rejects_response_above_cli_limit`.
The test intentionally makes Layer36 stop reading an over-limit HTTP response,
and Windows can report that expected early close as `ConnectionAborted` while
the fixture thread writes the response. The fix in progress treats that as an
accepted connection so the test can assert Layer36's exit code and stderr.

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

## 5) What remains to close Phase 2 fully

Top pending items:

1. Final UAPI v0.1 freeze review for WIT contracts
2. Broader cross host evidence for language variant behavior
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
- Expanded UCap evidence with a named dispatcher deny-before-adapter matrix that covers every non-default filesystem and network boundary
- Hosted workflows moved to Node 24 ready action versions
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
- Phase docs page: `docs/book/src/phases/phase-2.md`
- Progress page for non technical readers: `docs/book/src/progress-for-everyone.md`

## 8) Resume prompt for a new GPT session

Use this exact prompt in a new session:

`Continue Layer36 on main. Start with STATUS.md and Plan/Phase-2-Plan.md. Keep pushing Phase 2 closure, update plan/docs after each chunk, keep GitHub Pages in sync, and check CI after every push.`
