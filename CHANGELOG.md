# Changelog

All notable changes to Krate are recorded in this file. Downloads and
checksums for every version are on the
[releases page](https://github.com/incyashraj/krate/releases).

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Krate uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Pre-1.0: breaking changes may occur in any minor release.

---

## [Unreleased]

Changes merged since 0.5.5-rc9. Contributors: add a line here with your change.

### Fixed
- Web Player: in a browser tab, the haptics, motion and unlock interfaces now answer `available() = false` (and the calls answer as a phone without the feature does) instead of showing a "works in Krate on a computer" notice, which a tab cannot promise.

## [0.5.5] pre-releases (rc2 to rc9) - latest rc9 2026-11-04

### Added
- Open a Krate app from a link in any browser tab, with nothing installed. A `.krate` file you were sent opens at krate.tech/open without being uploaded.
- Apps opened from a link keep their own database, open and save files, use live connections, and can ask for the camera, microphone, location, notifications and speech.
- One app, many versions: publishing the same app again updates it, and a link you already sent opens the new version with its data.
- Apps lay themselves out for the screen they are on, and `check-app` runs every app as a phone, a tablet and a desktop and fails one that does not fit.
- C and C++ programs run as Krate apps, exceptions included. Doom ships as one 2.8 MB `.krate` that plays at 51 fps.
- Apps reach the GPU through `krate:gpu`, wgpu and Bevy programs draw in a Krate window, and `krate:process` lets an app run programs on the computer it opens on.
- Krate for organisations: private team apps with approvals, releases and groups, team data sealed end to end, company connections the hub cannot read, SAML, OIDC and SCIM sign-in, audit export, and a console for each role.
- A guide to publishing and opening big apps: the size limits, resuming an interrupted upload, and how a big app opens from a link.

### Changed
- Krate Studio and krate.tech are redesigned. Studio gains an IDE (Beta) to open, edit and build a Krate project with your AI, and a Port door (Beta) to bring an app you already have.
- A gift `.krate` carries Krate with it: friends on a Mac or Linux need nothing installed first.
- Apps are compiled once and cached, large bundles unpack once, and `krate publish` handles apps over 5 MiB.
- Releases carry SBOMs, build provenance and `.pkg`, `.deb` and `.rpm` installers, and Krate follows a company's machine policy and proxy.

### Fixed
- Security updates to Wasmtime and other dependencies for published advisories, plus clearer errors: a crash says what it was, and an app made with a newer Krate says to update Krate.

## [0.5.0] to [0.5.4] - 2026-09-19 to 2026-09-30

### Added
- Krate Studio in the browser: make, change, download and publish an app from krate.tech, with Plan mode and your own AI key.
- GPU-backed 3D with lighting, shadows, fog, transparency, normal mapping and tone mapping: 250,000 triangles at 72 fps on the GPU.
- Widget placement, backgrounds, borders and rounded corners, plus file drop and a working save-file dialog.
- Apps can speak aloud, watch a folder for changes, print their window, share a store with the same publisher's other apps, and ask the person for more memory.
- Every developer gets their own page at `name.krate.tech`.

### Changed
- New text draws about eight times faster.
- A refused capability says why, an optional one is asked for when the feature is used, and a grant the app never declared is refused.
- krate.tech and Studio work on a phone, and Krate apps on a Mac quit with Cmd+Q and close with Cmd+W.

### Fixed
- Linux Krate starts on Ubuntu 22.04 and Debian 12 again, Linux Studio opens, and arm64 Linux ships `cargo-component`.
- Claude builds no longer fail with "OAuth session expired", and Studio no longer loses sessions to ordinary input.

## [0.4.0] - 2026-09-14

### Added
- Signed apps: a publisher signs a bundle, delegates and revokes release keys, and an app changed after signing does not run.
- A Windows Player installer, so someone who only opens apps has a path that fits.
- Take an app's saved data out and put it back, and remove an installed app without losing what you wrote in it.
- New sample apps: Pad, Diff, Bigtable and Slowfetch.
- `check-app` drives every app with the keyboard too, and proves the app does what its author says it is for.

### Changed
- An app built against an older interface still opens, and the packer writes the same bytes for the same input on every machine.
- The hub checks a published app with the same validator the person opening it uses.
- One SDK for command-line and windowed apps, through a `gui` feature.

### Fixed
- Security hardening: a Wasmtime security patch, secrets sealed with real AEAD, SQLite deciding what a statement may do, and an updater that installs only what it verified.
- Windows opens links and `.krate` files without a shell in the way, and `krate pack` no longer overflows the stack there.

## [0.3.0] - 2026-09-02

### Added
- Make an app in the browser: describe it on the website, watch the build, and get the app. Studio also runs in a browser tab.
- Author with an Anthropic or OpenAI API key, with no coding CLI to install.
- After a build, a developer can reach the app's code and the command that made it.
- The Mac gift is notarized, and the receive page knows which app you were sent and hands you one download.

### Changed
- Studio has a new logo, sessions in a drawer you can drag, and rebuilt Settings.

### Fixed
- Studio no longer runs from the filesystem root, so macOS stops asking for Downloads in Krate's name; dictation works; the drawer closes on Windows and Linux.

## [0.2.0] to [0.2.5] - 2026-08-26 to 2026-08-31

### Added
- `krate card` and Studio's Send sheet: share an app as a card, and a hub link lands the person on a page about their app.
- `krate wrap`: a double-clickable file for one friend that installs Krate once, then opens the app.
- Every published app gets a page that shows it, and krate.tech has a privacy page written from the source code.

### Changed
- Every supported AI gets its own sign-in in Studio, and the AI agent is confined to its workspace.
- Setup finishes itself without a new terminal, and a new app in the same sitting starts from a warm session.
- A new design for krate.tech and Studio. Studio and the hub move to the Business Source License 1.1; the player, format, CLI and runtime stay MIT or Apache.

### Fixed
- The in-app updater verifies what it downloads, animating apps no longer grow memory without limit, and a late frame no longer burns a core.

## [0.1.1] to [0.1.58] - 2026-08-07 to 2026-08-25

### Added
- Krate Studio: describe an app, watch it get built, change it, and publish it. It ships as a signed, notarized Mac app, then for Windows and Linux.
- Studio plans with you before it builds, and `krate revise` changes an app you already have instead of remaking it.
- Modern UI for apps: rounded corners, soft shadows, gradients, styled text, arcs and spring motion.
- Apps can use the camera, keep live connections, share data between people, and join two-player game rooms on the hub.
- Krate Cloud in Studio: browse apps, publish your own with a listing, and open an app's page.

### Changed
- Apps open about ten times faster and fill the window they are given.
- Windows, macOS and Linux draw through the GPU, and macOS canvases present on Metal with vsync.
- Double-clicking a `.krate` opens the app under its own name and icon.

### Fixed
- A fresh Windows machine works, Windows archives unpack correctly everywhere, and the close button always ends an app.

## [0.1.0] - 2026-08-07

The first release where the whole path works: install Krate, describe an app, an AI writes it, and the `.krate` file it hands you opens on macOS, Windows and Linux.

Everything below was built for 0.1.0, from the first commit in April 2026.

### Added
- Manifest `net.connect` host-pattern hardening: capability parsing now rejects invalid dot placement, invalid label-edge `-` forms, and malformed numeric IPv4 host forms before runtime policy matching, with manifest tests for accepted wildcard/label forms and malformed host cases.
- Docs: added `First Go CLI` and `First TypeScript CLI` walkthroughs under `docs/book/src/uapi/`, and linked them in mdBook navigation so Phase 2 now has one onboarding tutorial per language track (Rust, Go, TypeScript).
- CI dependency audit hardening: hosted and self-hosted workflows now run `scripts/check-dependencies.sh`, which preserves hard gates for `licenses`, `bans`, and `sources` while downgrading known advisory-db parser/lock-path failures to warnings until `cargo-deny` catches up.
- Dependency audit config fix: `crates/manifest/Cargo.toml` now pins `krate-adapter-common` with `version = "0.1.0-dev"` on its path dependency, avoiding wildcard-dependency bans failures.
- Phase 2 fuzz scaffolding: added `fuzz/` with first `cargo-fuzz` harness targets for manifest parsing, shared logical-path parsing, and policy matching, plus `scripts/run-phase2-fuzz-smoke.sh` for short local and self-hosted smoke runs.
- Self-hosted CI now installs `cargo-fuzz` and runs the short Phase 2 fuzz smoke as part of `Local full gate`.
- Fuzz smoke runner now pins nightly toolchain binaries explicitly (`CARGO`, `RUSTC`, and `PATH`) before running `cargo fuzz`, so sub-commands that spawn nested cargo/rustc calls stay on nightly in runner environments where `cargo +nightly` and plain cargo proxies differ.
- Fuzz crash fix in shared path hardening: `is_reserved_windows_segment` no longer slices strings at fixed byte offsets for COM/LPT detection. It now uses safe ASCII-byte matching and includes a regression test from the fuzz-found non-ASCII crashing input.
- Policy `net.connect` matching parity hardening: session grant matching now normalizes host case and numeric ports before wildcard matching, so grants like `API.example.com:0443` match canonical runtime endpoint checks.
- Manifest capability resource-shape hardening: filesystem capability resources now use shared logical-path validation rules, and `net.connect` resources must match valid endpoint shapes (`<host>:<port>` with wildcard support) so malformed resources fail at manifest parse time.
- Docs: added a non technical progress page at `docs/book/src/progress-for-everyone.md` with plain language goals, system flow diagrams, current timeline state, and glossary style explanations.
- CLI and runtime Phase 2 app-argument count guardrail: `krate run` preflight and runtime `io.args.raw` now reject more than 1024 app arguments before guest parsing, with runtime and CLI tests covering this path.
- Runtime Phase 2 resource-id reuse hardening: local adapter and generated host resource tables now recycle released file/stream resource IDs before allocating new IDs, with runtime tests proving close/drop reuse and free-list-first allocation at ID-counter ceiling edge cases.
- Generated host Phase 2 resource-table guardrail: the host-side file/input/output resource table now enforces an active-resource cap and fails with a clear host-table limit error when exceeded, with runtime tests covering overflow rejection.
- Runtime Phase 2 resource lifecycle hardening: generated file/stream `drop` callbacks now close underlying local adapter handles before host-side resource-table removal, with tests covering close-on-drop and resource-slot reuse after close.
- Runtime Phase 2 resource-table guardrail: local adapter now caps the number of open file/stream handles in one run session, and returns a clear runtime I/O error when the cap is exceeded, with runtime tests covering overflow rejection.
- Phase 2 language-variant fixture auto-discovery: Go/TypeScript CLI integration tests and `scripts/test-phase2-language-variants.sh` now auto-detect `test/integration/language-variants/*.wasm` fixtures when `KRATE_GO_*` / `KRATE_TS_*` env vars are not set.
- CLI Phase 2 app-argument preflight: `krate run` now rejects empty args, newline/NUL delimiter characters, and oversized raw app-argument payloads above 64 KiB before runtime startup, with integration tests covering each rejection path.
- Runtime Phase 2 raw app-argument guardrails: `io.args.raw` now rejects empty args, newline/NUL delimiter characters, and oversized raw payloads above 64 KiB, with runtime tests covering valid and invalid argument shapes.
- Runtime Phase 2 write guardrails: local adapter stream writes and filesystem writes now reject per-call sizes above 8 MiB, with deterministic runtime errors and runtime tests covering oversized write requests.
- Runtime Phase 2 filesystem resource guardrails: local adapter reads now reject per-call sizes above 8 MiB, and directory listings now reject responses above 4096 entries, with deterministic runtime errors and test coverage for both bounds.
- Runtime Phase 2 sandbox hardening for absolute logical paths: local filesystem resolution now treats absolute logical paths as sandbox-rooted paths instead of host-root paths, so they run through the same sandbox checks as relative paths.
- Shared Phase 2 locale-subtag shape hardening in `adapter-common`: locale-tag normalization now enforces a stricter primary language-subtag shape and bounded locale subtag lengths, with malformed locale-tag inputs falling back to `en-US`.
- Shared Phase 2 path-length hardening in `adapter-common`: logical path parsing now rejects oversized path segments and oversized normalized logical paths before host I/O in the early sandboxed filesystem path.
- Shared Phase 2 timezone-shape normalization hardening in `adapter-common`: timezone normalization now accepts only conservative ASCII timezone-name characters with a length cap in this phase, and falls back to `UTC` for invalid input.
- Shared Phase 2 ASCII-only URL input hardening in `adapter-common`: plain-HTTP URL parsing and shared endpoint extraction now reject non-ASCII URL input so early request-line framing and capability checks stay deterministic.
- Shared Phase 2 locale-tag canonicalization hardening in `adapter-common`: locale normalization now canonicalizes subtag casing (language/script/region), rejects malformed locale-tag shapes, and falls back to `en-US`; timezone normalization now rejects control-character values and falls back to `UTC`.
- Shared Phase 2 URL scheme-case parsing parity in `adapter-common`: `http`/`https` scheme handling is now case-insensitive in endpoint parsing, and mixed-case `HTTP://` plain URL inputs are accepted by the shared plain HTTP parser.
- Shared Phase 2 filesystem path trailing-segment hardening in `adapter-common`: path parsing now rejects segments ending in `.` or a trailing space before sandbox resolution and host I/O, reducing Windows normalization edge cases.
- Shared Phase 2 plain HTTP request-target size guard in `adapter-common`: oversized path/query targets are now rejected during URL parsing before request framing and socket writes.
- Runtime network-policy parity test for host-case normalization: mixed-case URL hosts are now explicitly covered in dispatcher tests to confirm lowercase `net.connect` grants still match.
- Shared Phase 2 host-case normalization in `adapter-common`: accepted URL hosts are now normalized to lowercase during endpoint parsing so policy checks treat `EXAMPLE.com` and `example.com` as the same endpoint.
- Shared Phase 2 strict buffered `Content-Length` validation: shared plain HTTP response parsing now rejects any truncated body, including empty buffered bodies when a non-zero length is declared.
- Shared Phase 2 plain HTTP request-body guard in `adapter-common`: buffered request framing now rejects bodies above a fixed early-phase cap and maps them through runtime fetch as `body-too-large`.
- Shared Phase 2 plain HTTP response integrity checks in `adapter-common`: the shared response parser now rejects unsupported response `Transfer-Encoding`, conflicting `Content-Length` headers, and mismatched buffered body lengths in this early plain-HTTP slice.
- Shared Phase 2 HTTP response read-loop hardening in `adapter-common`: full-response reads now use one helper that enforces byte limits and maps timeout-style socket reads before response parsing.
- Shared Phase 2 host-name validation hardening in `adapter-common`: domain labels now reject leading/trailing `-`, label lengths are bounded, and numeric-dot hosts must parse as valid IPv4 literals.
- Shared Phase 2 plain HTTP response parser hardening in `adapter-common`: runtime fetch parsing now uses one shared parser that validates HTTP/1.0/1.1 version, status-code range, header count limits, header syntax, and safe header values before data is returned to app-facing response types.
- Shared Phase 2 deterministic locale formatting baseline in `adapter-common`: date styles now format Unix-millis timestamps into stable UTC date/time strings, and number styles now return deterministic decimal/percent/currency text from locale hints.
- Optional Phase 2 Go/TypeScript runtime fixture assertions in `crates/cli/tests/cli.rs` behind `KRATE_GO_*` and `KRATE_TS_*` WASM env vars, plus `scripts/test-phase2-language-variants.sh` and CI/self-hosted workflow hooks that auto-skip until variant components are supplied.
- Go and TypeScript Phase 2 `krate-cat` sample variants under `packages/sdk-go/examples` and `packages/sdk-ts/examples`, plus stricter SDK shape checks that assert cat/clock/curl sample behavior contracts.
- Hidden Phase 2 deterministic clock-fixture controls: `krate run --test-locale` and `--test-timezone`, runtime wiring for locale/timezone overrides, and a strict `krate-clock` snapshot test with fixed time + locale + timezone for cross-host output comparison.
- Shared Phase 2 Windows reserved-name path hardening in `adapter-common`: path segments that map to device-style names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`) are now rejected before sandbox resolution and host I/O.
- Shared Phase 2 authority parser reuse across network paths: plain `http://` URL parsing now reuses the same host/port parser used by runtime endpoint policy checks, with extra runtime tests for invalid-URL short-circuiting and default port grant behavior.
- Shared Phase 2 URL endpoint parsing for policy checks: `adapter-common` now provides `parse_url_endpoint` for `http://` and `https://`, and runtime network capability checks use this shared parser instead of separate endpoint parsing logic.
- Shared Phase 2 filesystem path prefix hardening in `adapter-common`: colon-based path forms are now rejected before host I/O, including Windows drive-style prefixes and alternate-data-stream style suffixes.
- Shared Phase 2 plain HTTP authority and header-value hardening in `adapter-common`: unsupported authority forms are rejected in this early slice, control characters in app header values are denied, and `Transfer-Encoding` is treated as host-controlled.
- Windows host file opens now apply a no-follow final-symlink flag (`FILE_FLAG_OPEN_REPARSE_POINT`) for Phase 2 sandbox paths.
- Shared Phase 2 host-clock overflow guards in `adapter-common`: monotonic nanoseconds now saturate at `u64::MAX`, and out-of-range Unix-millis conversion is rejected with explicit error handling.
- Phase 2 plain HTTP URL validation now rejects request-line whitespace/control characters and empty or zero ports before socket access.
- Shared Phase 2 filesystem operation intents in `adapter-common`, with runtime guards that reject destructive root-like remove and rename targets before native host I/O.
- Shared Phase 2 locale helpers in `adapter-common`, covering `LC_ALL`/`LANG` locale detection, `TZ` fallback, and BCP 47-ish normalization.
- Shared Phase 2 host clock helpers in `adapter-common`, including fixed test time, Unix-epoch millisecond conversion, monotonic elapsed time, and sleep.
- Unix host file opens now use a no-follow final-symlink flag for Phase 2 sandbox paths, reducing open-time symlink race risk.
- Symlink escape checks for relative Phase 2 filesystem paths resolved through the runtime sandbox root.
- `krate run --sandbox-root <dir>` and runtime sandbox-root resolution for relative Phase 2 filesystem paths.
- Shared Phase 2 filesystem path normalization in `adapter-common`, with runtime filesystem calls and policy resource matching using the same rules.
- `crates/adapter-common` as the first shared host-adapter crate, starting with plain HTTP URL parsing and request framing used by the runtime.
- Phase 2 plain HTTP `fetch(req)` now sends the selected method, app headers, and buffered body while keeping transport headers under host control.
- `scripts/smoke-rust-sdk.sh` to package the Rust SDK and check a fresh outside-workspace component against the packaged crate.
- Generated UAPI reference pages now include function-level behavior notes for the Phase 2 interfaces and resource methods.
- `krate-curl` now prints clearer messages for oversized, timed-out, and malformed HTTP responses.
- Phase 2 HTTP timeout and protocol failures now map to WIT `net-error.timeout` and `net-error.protocol`.
- Phase 2 HTTP oversized-response errors now map to the WIT `net-error.body-too-large` variant instead of a generic connection failure.
- Phase 2 policy coverage tests now prove every supported capability name has a UAPI call mapping and every current dispatcher adapter method is reached through the policy gate.
- ADR-0006, ADR-0007, and ADR-0008 for Phase 2 WIT versioning, UCap soft enforcement, and host async runtime direction.
- `krate run --max-http-response-bytes` to tune the Phase 2 plain HTTP adapter response-size guard per run.
- `krate run --log-grants-format jsonl` to append one structured grant audit record per line for scripts and local tooling.
- `krate run --dump-caps-format json` to print effective run grants, app identity, and component path as structured data before starting a component.
- A 1 MiB response-size guard in the Phase 2 plain HTTP adapter so early network tests have deterministic host-side bounds.
- `krate manifest check --format json` and `krate manifest capabilities --format json`, making all manifest inspection commands script-friendly.
- `krate manifest explain --format json` for tools that need a structured view of app identity, requested capabilities, default grants, resources, and launch-grant needs.
- `krate run --log-grants <file>` to append app identity and effective session capabilities to a local grant audit log.
- Phase 1 to Phase 2 migration note explaining how the temporary `print`/`exit` proof path maps to the real UAPI, manifest, and grant model.
- Phase 2 Rust walkthrough showing the current SDK, component build, manifest generation, manifest explanation, granted run, and denial path.
- `krate manifest explain` to show app identity, requested capabilities, default grants, launch-grant needs, resources, and rationales in human-readable form.
- `krate manifest init` to generate starter Phase 2 `manifest.toml` files with validated app metadata and capability strings.
- Initial `packages/sdk-go` Go/TinyGo SDK scaffold with Phase 2 UAPI helper packages, clock/curl examples, and a dependency-free package shape check in CI.
- Initial `@krate/sdk` TypeScript package scaffold with Phase 2 UAPI import declarations, helper modules, examples, and a dependency-free package shape check in CI.
- `krate doctor` now reports Phase 2 language-binding tool readiness for TinyGo, Go, Node, npm, and jco.
- Fixture-backed sample manifest tests proving `krate-clock`, `krate-cat`, and `krate-curl` run through their sample `manifest.toml` files with `--auto-grant`.
- Phase 2 component import checker that rejects non-`krate:*` imports in built sample components, wired into full hosted CI and self-hosted CI.
- Phase 2 component startup benchmarks for the smoke UAPI app and `krate-clock`, including a first local runtime-path read in the mdBook benchmark notes.
- Phase 2 UAPI dispatch benchmark target and docs, with first local sub-microsecond results for default IO, filesystem grants, denial path, and network grant checks.
- Phase 2 UCap denial coverage for filesystem `stat`, `list`, `remove`, `mkdir`, and `rename`, proving they stop before adapter calls when grants are missing.
- Phase 2 stdio stream UCap hardening: stream handles now remember whether they came from stdin, stdout, or stderr, and stream read/write/flush methods re-check that capability before adapter work.
- Phase 2 file-handle UCap hardening: read-write opens now require both read and write grants, and file resource read/write/stat/seek methods re-check path capabilities before adapter calls.
- Generated UAPI reference capability tables now come from the same manifest crate table used by validation and `krate manifest capabilities`.
- `krate manifest capabilities` to print the canonical Phase 2 capability strings and default-grant status from the manifest crate.
- Phase 2 UAPI contract checker for the current WIT package shape, wired into hosted and self-hosted CI.
- Full WIT style guide for Krate UAPI naming, resource design, typed errors, capability mapping, comments, versioning, and review checks.
- Rust SDK API polish: crate-level docs, rustdoc comments for the helper layer, owned argument helpers, sample app usage of those helpers, and a self-hosted SDK doc build check.
- Human-facing generated UAPI reference context: interface summaries, capability notes, Rust SDK examples, and WIT doc comments.
- Generated Phase 2 UAPI reference seed from `wit/krate/phase2`, linked in mdBook and checked in hosted and self-hosted CI.
- Rust guest SDK package preparation: crate README, crates.io-facing metadata, package include list, local package proof, and CI/self-hosted package dry-runs.
- Clear exit-code-5 permission-denied behavior for the Rust `krate-cat` and `krate-curl` samples, including an outside-granted-glob test for `krate-cat`.
- `krate run --dump-caps` to print the effective Phase 2 session capabilities without starting the component.
- Manifest entry hardening for `krate run`: when a sidecar manifest is present, its `app.entry` must match the `.wasm` being run.
- First terminal grant prompt for Phase 2 manifest capabilities via `krate run --prompt`, while non-interactive runs still fail cleanly when required grants are missing.
- Rust guest SDK helper layer for Phase 2 apps, including argument helpers, stream text helpers, file read/write helpers, HTTP text helpers, time/locale shortcuts, and a public Rust SDK guide.
- First Rust guest SDK crate at `crates/bindings-rust`, published locally as package `krate`, plus the Rust sample apps migrated onto the SDK facade.
- First Phase 2 HTTP adapter slice and `apps/krate-curl` sample for granted localhost GETs.
- Phase 2 `io.args` import and `krate run ... -- <args>` forwarding, used by the new `apps/krate-cat` sample.
- First named Phase 2 sample app, `apps/krate-clock`, plus a hidden `krate run --test-time` flag for deterministic clock tests.
- Phase 2 smoke missing-grant test proving UCap maps an ungranted filesystem read to a visible permission-denied path.
- Phase 2 smoke component and CI fixture path proving that `krate run` can execute a Phase 2 `cli` app using UAPI stdio, filesystem, time, and locale calls.
- Initial Phase 2 `krate run` linker path that falls back to the generated `cli` world and installs UAPI imports with a local stdio/fs/time/locale adapter.
- Phase 2 host resource table for generated file and stdio resources, with file read/write/seek/stat and stream read/write/flush routed through adapter traits.
- Phase 2 generated import host trait wiring over the UAPI dispatcher for HTTP, path-level fs, time, locale, logging, and stdio handles.
- Phase 2 generated WIT type/error bridge for runtime dispatcher wiring.
- Phase 2 runtime UAPI dispatcher scaffold with host-adapter traits and policy-before-adapter tests.
- Phase 2 Rust host binding checkpoint behind the `phase2-bindings` runtime feature and CI coverage.
- Phase 2 runtime UAPI guard that maps `io`, `fs`, `net`, `time`, and `locale` calls to UCap checks.
- Phase 2 session policy crate and `krate run --grant/--auto-grant` checks for manifest-required capabilities.
- Phase 2 sidecar manifest parser crate with capability string validation and `krate manifest check`.
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
- Project rename from OneOS to Krate across plans, docs, CLI placeholders, WIT examples, and bundle naming.
- Phase 1 runtime and CLI crate scaffolds with `krate run`, `krate version`, and `krate doctor`.
- ADR-0002 and ADR-0003 for Wasmtime and Component Model decisions.
- Phase 1 WIT interface and Rust hello-world component fixture that prints through the Krate host import.
- CI-backed Phase 1 hello fixture test with SHA-256 logging and `krate run` output assertion.
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
- `krate-cat` and `krate-curl` now parse Krate raw app args directly so their built components import only Krate UAPI, with no accidental WASI Preview 2 host imports.
- CI is temporarily manual-only while the GitHub account's Actions billing/spending limit is blocked; local checks remain the required development gate.
- Reduced normal GitHub Actions usage by keeping push CI on cheap Linux checks and moving the full Linux/macOS/Windows matrix, benchmarks, and cargo-deny audit behind manual full CI or `[full-ci]`.
- Render Mermaid flowcharts as diagrams on the published mdBook site.
- Reworked the public mdBook docs into clearer human language, added system flow diagrams, and changed the roadmap from fixed fixed-month language to estimates.

### Fixed
- Windowed-sync tests no longer use a fixed 100 ms sleep before asserting the stand-in hub saw the connection; they poll `served` with a 5 s deadline so the test stays reliable on a busy machine (#72).
- Krate Studio now creates an ignored placeholder resource for debug and test builds when `studio/bin/` is absent, allowing tests to compile on a fresh checkout while keeping the release engine requirement intact.

---

[Unreleased]: https://github.com/incyashraj/krate/compare/v0.5.5-rc9...HEAD
[0.5.5]: https://github.com/incyashraj/krate/releases/tag/v0.5.5-rc9
[0.5.4]: https://github.com/incyashraj/krate/releases/tag/v0.5.4
[0.5.0]: https://github.com/incyashraj/krate/releases/tag/v0.5.0
[0.4.0]: https://github.com/incyashraj/krate/releases/tag/v0.4.0
[0.3.0]: https://github.com/incyashraj/krate/releases/tag/v0.3.0
[0.2.5]: https://github.com/incyashraj/krate/releases/tag/v0.2.5
[0.2.0]: https://github.com/incyashraj/krate/releases/tag/v0.2.0
[0.1.58]: https://github.com/incyashraj/krate/releases/tag/v0.1.58
[0.1.1]: https://github.com/incyashraj/krate/releases/tag/v0.1.1
[0.1.0]: https://github.com/incyashraj/krate/releases/tag/v0.1.0
