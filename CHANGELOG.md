# Changelog

All notable changes to Layer36 will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Layer36 uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0: breaking changes may occur in any minor release.

---

## [Unreleased]

### Added
- Shared Phase 2 host-clock overflow guards in `adapter-common`: monotonic nanoseconds now saturate at `u64::MAX`, and out-of-range Unix-millis conversion is rejected with explicit error handling.
- Phase 2 plain HTTP URL validation now rejects request-line whitespace/control characters and empty or zero ports before socket access.
- Shared Phase 2 filesystem operation intents in `adapter-common`, with runtime guards that reject destructive root-like remove and rename targets before native host I/O.
- Shared Phase 2 locale helpers in `adapter-common`, covering `LC_ALL`/`LANG` locale detection, `TZ` fallback, BCP 47-ish normalization, and the current deterministic formatting placeholder.
- Shared Phase 2 host clock helpers in `adapter-common`, including fixed test time, Unix-epoch millisecond conversion, monotonic elapsed time, and sleep.
- Unix host file opens now use a no-follow final-symlink flag for Phase 2 sandbox paths, reducing open-time symlink race risk.
- Symlink escape checks for relative Phase 2 filesystem paths resolved through the runtime sandbox root.
- `layer36 run --sandbox-root <dir>` and runtime sandbox-root resolution for relative Phase 2 filesystem paths.
- Shared Phase 2 filesystem path normalization in `adapter-common`, with runtime filesystem calls and policy resource matching using the same rules.
- `crates/adapter-common` as the first shared host-adapter crate, starting with plain HTTP URL parsing and request framing used by the runtime.
- Phase 2 plain HTTP `fetch(req)` now sends the selected method, app headers, and buffered body while keeping transport headers under host control.
- `scripts/smoke-rust-sdk.sh` to package the Rust SDK and check a fresh outside-workspace component against the packaged crate.
- Generated UAPI reference pages now include function-level behavior notes for the Phase 2 interfaces and resource methods.
- `layer36-curl` now prints clearer messages for oversized, timed-out, and malformed HTTP responses.
- Phase 2 HTTP timeout and protocol failures now map to WIT `net-error.timeout` and `net-error.protocol`.
- Phase 2 HTTP oversized-response errors now map to the WIT `net-error.body-too-large` variant instead of a generic connection failure.
- Phase 2 policy coverage tests now prove every supported capability name has a UAPI call mapping and every current dispatcher adapter method is reached through the policy gate.
- ADR-0006, ADR-0007, and ADR-0008 for Phase 2 WIT versioning, UCap soft enforcement, and host async runtime direction.
- `layer36 run --max-http-response-bytes` to tune the Phase 2 plain HTTP adapter response-size guard per run.
- `layer36 run --log-grants-format jsonl` to append one structured grant audit record per line for scripts and local tooling.
- `layer36 run --dump-caps-format json` to print effective run grants, app identity, and component path as structured data before starting a component.
- A 1 MiB response-size guard in the Phase 2 plain HTTP adapter so early network tests have deterministic host-side bounds.
- `layer36 manifest check --format json` and `layer36 manifest capabilities --format json`, making all manifest inspection commands script-friendly.
- `layer36 manifest explain --format json` for tools that need a structured view of app identity, requested capabilities, default grants, resources, and launch-grant needs.
- `layer36 run --log-grants <file>` to append app identity and effective session capabilities to a local grant audit log.
- Phase 1 to Phase 2 migration note explaining how the temporary `print`/`exit` proof path maps to the real UAPI, manifest, and grant model.
- Phase 2 Rust walkthrough showing the current SDK, component build, manifest generation, manifest explanation, granted run, and denial path.
- `layer36 manifest explain` to show app identity, requested capabilities, default grants, launch-grant needs, resources, and rationales in human-readable form.
- `layer36 manifest init` to generate starter Phase 2 `manifest.toml` files with validated app metadata and capability strings.
- Initial `packages/sdk-go` Go/TinyGo SDK scaffold with Phase 2 UAPI helper packages, clock/curl examples, and a dependency-free package shape check in CI.
- Initial `@layer36/sdk` TypeScript package scaffold with Phase 2 UAPI import declarations, helper modules, examples, and a dependency-free package shape check in CI.
- `layer36 doctor` now reports Phase 2 language-binding tool readiness for TinyGo, Go, Node, npm, and jco.
- Fixture-backed sample manifest tests proving `layer36-clock`, `layer36-cat`, and `layer36-curl` run through their sample `manifest.toml` files with `--auto-grant`.
- Phase 2 component import checker that rejects non-`layer36:*` imports in built sample components, wired into full hosted CI and self-hosted CI.
- Phase 2 component startup benchmarks for the smoke UAPI app and `layer36-clock`, including a first local runtime-path read in the mdBook benchmark notes.
- Phase 2 UAPI dispatch benchmark target and docs, with first local sub-microsecond results for default IO, filesystem grants, denial path, and network grant checks.
- Phase 2 UCap denial coverage for filesystem `stat`, `list`, `remove`, `mkdir`, and `rename`, proving they stop before adapter calls when grants are missing.
- Phase 2 stdio stream UCap hardening: stream handles now remember whether they came from stdin, stdout, or stderr, and stream read/write/flush methods re-check that capability before adapter work.
- Phase 2 file-handle UCap hardening: read-write opens now require both read and write grants, and file resource read/write/stat/seek methods re-check path capabilities before adapter calls.
- Generated UAPI reference capability tables now come from the same manifest crate table used by validation and `layer36 manifest capabilities`.
- `layer36 manifest capabilities` to print the canonical Phase 2 capability strings and default-grant status from the manifest crate.
- Phase 2 UAPI contract checker for the current WIT package shape, wired into hosted and self-hosted CI.
- Full WIT style guide for Layer36 UAPI naming, resource design, typed errors, capability mapping, comments, versioning, and review checks.
- Rust SDK API polish: crate-level docs, rustdoc comments for the helper layer, owned argument helpers, sample app usage of those helpers, and a self-hosted SDK doc build check.
- Human-facing generated UAPI reference context: interface summaries, capability notes, Rust SDK examples, and WIT doc comments.
- Generated Phase 2 UAPI reference seed from `wit/layer36/phase2`, linked in mdBook and checked in hosted and self-hosted CI.
- Rust guest SDK package preparation: crate README, crates.io-facing metadata, package include list, local package proof, and CI/self-hosted package dry-runs.
- Clear exit-code-5 permission-denied behavior for the Rust `layer36-cat` and `layer36-curl` samples, including an outside-granted-glob test for `layer36-cat`.
- `layer36 run --dump-caps` to print the effective Phase 2 session capabilities without starting the component.
- Manifest entry hardening for `layer36 run`: when a sidecar manifest is present, its `app.entry` must match the `.wasm` being run.
- First terminal grant prompt for Phase 2 manifest capabilities via `layer36 run --prompt`, while non-interactive runs still fail cleanly when required grants are missing.
- Rust guest SDK helper layer for Phase 2 apps, including argument helpers, stream text helpers, file read/write helpers, HTTP text helpers, time/locale shortcuts, and a public Rust SDK guide.
- First Rust guest SDK crate at `crates/bindings-rust`, published locally as package `layer36`, plus the Rust sample apps migrated onto the SDK facade.
- First Phase 2 HTTP adapter slice and `apps/layer36-curl` sample for granted localhost GETs.
- Phase 2 `io.args` import and `layer36 run ... -- <args>` forwarding, used by the new `apps/layer36-cat` sample.
- First named Phase 2 sample app, `apps/layer36-clock`, plus a hidden `layer36 run --test-time` flag for deterministic clock tests.
- Phase 2 smoke missing-grant test proving UCap maps an ungranted filesystem read to a visible permission-denied path.
- Phase 2 smoke component and CI fixture path proving that `layer36 run` can execute a Phase 2 `cli` app using UAPI stdio, filesystem, time, and locale calls.
- Initial Phase 2 `layer36 run` linker path that falls back to the generated `cli` world and installs UAPI imports with a local stdio/fs/time/locale adapter.
- Phase 2 host resource table for generated file and stdio resources, with file read/write/seek/stat and stream read/write/flush routed through adapter traits.
- Phase 2 generated import host trait wiring over the UAPI dispatcher for HTTP, path-level fs, time, locale, logging, and stdio handles.
- Phase 2 generated WIT type/error bridge for runtime dispatcher wiring.
- Phase 2 runtime UAPI dispatcher scaffold with host-adapter traits and policy-before-adapter tests.
- Phase 2 Rust host binding checkpoint behind the `phase2-bindings` runtime feature and CI coverage.
- Phase 2 runtime UAPI guard that maps `io`, `fs`, `net`, `time`, and `locale` calls to UCap checks.
- Phase 2 session policy crate and `layer36 run --grant/--auto-grant` checks for manifest-required capabilities.
- Phase 2 sidecar manifest parser crate with capability string validation and `layer36 manifest check`.
- Phase 2 UAPI v0.1 WIT package draft for CLI apps, covering `io`, `fs`, `net`, `time`, and `locale`.
- Initial repository scaffold: licenses, README, CI, ADR-0001.
- Phase Plans (0-7) and Build Plan in `Plan/`.
- `docs/adr/` with ADR template and ADR-0001.
- `docs/book/` mdBook scaffold.
- Phase 0 workspace sentinel so baseline Cargo commands succeed before runtime crates exist.
- Code of Conduct, first-PR guide, legal search record, launch drafts, and setup script scaffolding.
- mdBook CI coverage on pull requests.
- Phase 0 status tracker, repository label definitions, and good-first-issue drafts.
- Preliminary naming/trademark risk record and external setup checklists.
- Project rename from OneOS to Layer36 across plans, docs, CLI placeholders, WIT examples, and bundle naming.
- Phase 1 runtime and CLI crate scaffolds with `layer36 run`, `layer36 version`, and `layer36 doctor`.
- ADR-0002 and ADR-0003 for Wasmtime and Component Model decisions.
- Phase 1 WIT interface and Rust hello-world component fixture that prints through the Layer36 host import.
- CI-backed Phase 1 hello fixture test with SHA-256 logging and `layer36 run` output assertion.
- Phase 1 fuel and memory limit enforcement with clear `limit exceeded` CLI exits.
- Phase 1 release packaging workflow and local package script for tar.gz/zip artifacts.
- Phase 1 quickstart for building and running the hello-world component.
- Phase 1 Threat Model v0.1 and updated security scope.
- Phase 1 Criterion benchmark suite, warning-only regression check, and published local baseline.
- Phase 1 test harness script that builds the hello component before running workspace tests.
- Shared Phase 1 CI hello fixture artifact so Linux, macOS, and Windows run the same `.wasm` bytes and assert the same SHA-256.
- Release workflow tag matching for both final and RC-style version tags.
- Release workflow prerelease marking for RC-style tags.
- Visible shared-fixture artifact path for GitHub Actions upload/download.
- `v0.1.0-rc1` prerelease with Linux x64, Linux ARM64, macOS Intel, macOS Apple Silicon, Windows x64, and `SHA256SUMS` assets.

### Changed
- `layer36-cat` and `layer36-curl` now parse Layer36 raw app args directly so their built components import only Layer36 UAPI, with no accidental WASI Preview 2 host imports.
- CI is temporarily manual-only while the GitHub account's Actions billing/spending limit is blocked; local checks remain the required development gate.
- Reduced normal GitHub Actions usage by keeping push CI on cheap Linux checks and moving the full Linux/macOS/Windows matrix, benchmarks, and cargo-deny audit behind manual full CI or `[full-ci]`.
- Render Mermaid flowcharts as diagrams on the published mdBook site.
- Reworked the public mdBook docs into clearer human language, added system flow diagrams, and changed the roadmap from fixed fixed-month language to estimates.

---

<!-- Releases appear here as they are cut. Example format:

## [0.1.0] : 2026-XX-XX

### Added
- …

### Changed
- …

### Fixed
- …

[0.1.0]: https://github.com/incyashraj/layer6x6/releases/tag/v0.1.0
-->

[Unreleased]: https://github.com/incyashraj/layer6x6/compare/HEAD...HEAD
