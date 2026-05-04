# Changelog

All notable changes to Layer36 will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Layer36 uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0: breaking changes may occur in any minor release.

---

## [Unreleased]

### Added
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
