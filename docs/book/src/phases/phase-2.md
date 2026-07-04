# Phase 2: UAPI v0.1

**Status:** Active; exit evidence in progress
**Estimate:** est. 4 to 8 weeks
**Goal:** Make Krate useful for small command line apps.

Security baseline for this phase is now published:
[Phase 2 Threat Model v0.2](../phase2/threat-model-v0-2.md).

Phase 2 replaces the temporary Phase 1 host interface with real APIs:

- `io`
- `fs`
- `net`
- `time`
- `locale`

The first draft of those WIT contracts now lives at `wit/krate/phase2`.
It is not frozen yet, but it is real source code and CI parses it so syntax
mistakes are caught early.
Phase 2 also has a quick readiness command now:
`scripts/phase2-exit-readiness.sh`. It reads the exit ledger and prints the
current count of done, partial, pending, and blocked gates, so progress checks
do not depend on memory.
Hosted CI stability can now be recorded too:
`scripts/record-phase2-ci-stability-evidence.sh` writes recent CI and Pages run
history into one markdown report for exit review.
The same report can be included in the exit bundle with
`scripts/record-phase2-exit-bundle.sh --strict --include-ci-stability` when we
prepare a final review packet.
Self-hosted full-gate history has its own recorder too:
`scripts/record-phase2-self-hosted-evidence.sh`. This keeps the local runner
proof separate from hosted CI while still letting the exit bundle include it
when needed.
The UAPI freeze decision has its own packet now as well:
`scripts/check-phase2-freeze-decision.sh` checks that the packet names the
scope, required evidence, no-go conditions, and pending reviewer signoff before
we call the contract frozen.

The capability layer has also started. Krate can parse a sidecar
`manifest.toml`, check launch-time grants, and carry the session policy into the
runtime. The newest piece is a runtime UAPI guard: it translates calls like
`fs.read ./data/file.txt` or `net.connect api.example.com:443` into the exact
capability check that must pass before an adapter talks to the host OS.
Manifest capability parsing now also validates resource shapes earlier: unsafe
filesystem resource patterns and malformed `net.connect` endpoint forms fail
during manifest parsing instead of reaching runtime policy resolution.
Manifest `net.connect` host parsing is stricter now too: invalid dot placement,
invalid leading or trailing `-` in host labels, and malformed numeric IPv4
host forms are rejected at parse time.
Session policy matching now also normalizes `net.connect` host casing and
numeric ports before wildcard matching, so grant checks stay stable when users
type host names with mixed case or port values like `0443`.
Manifest capability parsing now stores that same canonical `net.connect`
resource shape too, so duplicate-capability checks and capability displays stay
deterministic across host case and numeric port formatting differences.
Manifest capability parsing now applies the same deterministic storage rule to
`fs.*` resources through shared logical-path normalization, so formatting
variants like `./notes/**` and `notes\\**` are stored as one canonical
`notes/**` shape for duplicate checks and capability output.
Interactive grant prompts now keep rationale text aligned with those canonical
capability shapes too, so prompts still show the right human reason lines even
when the manifest used non-canonical resource formatting.

There is also a first dispatcher scaffold now. In simple terms: we have the
place where generated UAPI calls will enter the runtime, get checked by policy,
and then move to the host adapter. The current tests prove denied file and
network calls stop before any adapter code runs.

The Phase 2 WIT also has a Rust host-binding checkpoint now. That means CI asks
Wasmtime to generate bindings for the new `cli` world and checks a few important
names. So far the shape is usable: `run` returns an `i32`, `OpenMode::Read`
exists, and `HttpMethod::Get` exists.

There is also a first Rust guest SDK crate now. It lives at
`crates/bindings-rust`, builds as package `krate`, and gives app code simple
module names such as `krate::io`, `krate::time`, and `krate::locale`.
The Rust sample apps now use that SDK facade, so normal app code talks to
`krate::fs::open`, `krate::net::get`, and `krate::time::now_millis`
instead of deep generated binding paths.

The SDK now has its first small helper layer too: argument helpers, stdout and
stderr text helpers, common file helpers, HTTP body helpers, and top-level time
and locale shortcuts. The important rule is still the same: guest apps should
import Krate UAPI, not host WASI APIs. The current sample components are
checked for that.
The Rust SDK also has a packaged-crate smoke now: CI creates a temporary app
outside the workspace and checks that it can compile a tiny Krate component
against the packaged SDK.

Go now has a clear Phase 2 decision too. The Go examples and TinyGo smoke build
stay in scope, but Go runtime parity is experimental until the compiled
components import only `krate:*` UAPI packages. We are not promoting Go
fixtures that still import `wasi:*` directly, because that would weaken the
runtime boundary Phase 2 is meant to prove.

The latest runtime piece is the generated type bridge. It maps WIT records and
errors into the dispatcher's Rust types, then back again. Put simply: the
runtime now understands the words that generated WIT code will use when it asks
for files, network, time, locale, and logs.

The generated host traits are also wired for the first useful slice. HTTP,
path-level filesystem operations, time, locale, logs, and stdio now call the
dispatcher, which means UCap sits in front of those calls. A small resource
table now owns opened file and stdio handles, so reads, writes, seeks, stats,
and flushes can route through the adapter without exposing raw host IDs. The
local runtime now also caps that open-handle table, so Phase 2 components
cannot grow stream/file handles without bound. Generated resource `drop`
callbacks now close underlying local adapter handles too, so released
resources return slots back to the same local runtime session. The generated
host-side resource table now has its own active-resource cap too, so both host
and adapter layers have bounded handle growth in this phase. Both tables now
also reuse released resource IDs before allocating fresh IDs, so long-running
sessions keep resource identity stable and avoid unbounded ID growth.

The runtime also has an initial Phase 2 execution path now. `krate run` keeps
supporting the Phase 1 proof world, then falls back to the Phase 2 `cli` world
and installs the generated UAPI imports. The local adapter currently covers
stdio, basic filesystem calls, time, locale, and a first plain HTTP request
path. Relative filesystem paths now resolve through an explicit runtime
sandbox root. The default is `.`, and `--sandbox-root <dir>` lets a run point
app-relative file access at a specific directory. Shared path cleanup also keeps
the filesystem adapter and UCap grant matcher using the same rules. It now also
rejects colon-based prefix forms up front, so Windows drive-style and alternate
data stream style paths do not cross host-specific parsing rules. It also
rejects reserved Windows device-style names such as `con`, `nul`, `com1`, and
`lpt1` before host I/O, and now rejects path segments ending in `.` or a
trailing space to avoid Windows filename normalization edge behavior. It now also
rejects oversized path segments and oversized normalized logical paths before
host I/O so cross-host path behavior remains predictable in this phase. Read
and list bounds are now explicit too: one file read call is capped at 8 MiB,
and one directory list call is capped at 4096 entries in this early adapter
slice. Write bounds now match that direction: one stream or file write call is
capped at 8 MiB in the same early adapter path. Absolute
logical paths are now sandbox-rooted too, so `/notes/file.txt` resolves under
the configured sandbox root instead of host root. For relative
paths, the local adapter now checks canonical existing targets, or the canonical
parent for new files, before host I/O. If a symlink would take the path outside
the sandbox root, the adapter denies the call. On Unix and Windows hosts, file
open also uses a no-follow final-symlink flag, so the final filename cannot be
a symlink at open time.
That no-follow open behavior now routes through the per-OS adapter crate path
instead of runtime-local cfg blocks.
Runtime path resolution now also denies symlinked path segments inside the
sandbox traversal path itself, for both existing-target and create-path
operations, so this phase has less directory traversal race exposure.
For Windows parity, the same traversal guard now treats reparse-point segments
as blocked link semantics too, so junction-style hops are denied in the same
sandbox traversal path checks.
This behavior is now captured as an architectural decision in
`docs/adr/0009-sandbox-link-semantics.md`.
Destructive filesystem operations now go through a shared operation-intent check
too. That means remove and rename cannot target root-like paths such as `.` or
`/` before the adapter reaches native host I/O.

The HTTP path is still small on purpose: it is for localhost and
test-server proofs while HTTPS, redirects, streaming, and production hardening
stay open. It now forwards lower-level `fetch` methods, app headers, and
buffered bodies, while keeping host-controlled transport headers owned by the
adapter. It also has a response-size guard and typed errors for oversized
responses, timeouts, and malformed HTTP responses, so apps can react to the real
problem instead of receiving one generic network failure. The shared URL parser
now also rejects whitespace, control characters, empty ports, and port `0`
before anything reaches the request line or socket layer. It also rejects
unsupported authority forms in this early plain-HTTP slice and rejects control
characters in app-provided header values. `Transfer-Encoding` is now treated as
host-controlled with `Host`, `Connection`, and `Content-Length`.
Response parsing now also goes through shared adapter-common code, with strict
validation for HTTP version, status range, malformed header lines, header count
limits, and unsafe header values before data reaches runtime-facing response
types.
That shared response parser now also enforces a strict header-block size cap
(16 KiB), so oversized header sections fail early in a deterministic way.
Request framing now also enforces a strict total request-frame size cap, so
oversized combinations of valid headers and body fail before socket writes.
The response read loop is shared too, so timeout mapping and full-response size
limits use one helper across adapters.
Response integrity checks now reject unsupported response `Transfer-Encoding`
and conflicting or mismatched `Content-Length` shapes in this early plain-HTTP
slice.
Request framing now also enforces a shared buffered body size limit so this
early plain-HTTP path cannot consume unbounded request payloads.
Shared host parsing now also rejects invalid domain-label forms and invalid
numeric IPv4 literals so URL and capability-path validation stay aligned.
Numeric IPv4 textual parsing is now strict dotted-decimal as well, so ambiguous
leading-zero forms like `001.2.3.4` are rejected in both URL endpoint parsing
and manifest `net.connect` validation. Runtime dispatcher tests now also prove
these URLs fail as invalid before adapter network calls.
Host length is now bounded in the same way for both runtime URL endpoint
parsing and manifest `net.connect` parsing (maximum 253-byte host text), so
oversized hostnames fail early with consistent behavior.
It now also rejects wildcard or non-unicast numeric IPv4 endpoint forms
(`0.0.0.0`, `255.255.255.255`, and multicast ranges) during endpoint parsing,
so runtime URL checks and manifest `net.connect` validation stay aligned on
connectable target shapes. Dispatcher tests now also prove these values are
rejected before any adapter network call runs.
For manifest host patterns, wildcard is now constrained to explicit shapes only:
a full left-most `*` label (`*` or `*.example.com`). Partial-label wildcard
forms (for example `exa*mple.com`) and multiple wildcard labels are rejected.
Policy matching now also treats `*.example.com` as a single-label wildcard
scope only (for example `api.example.com`, but not `deep.api.example.com`),
while `*` host grants remain the broad opt-in form. Runtime dispatcher tests
now cover this grant behavior directly.
Host names are now normalized to lowercase in shared URL parsing, so capability
checks stay stable across input case differences like `EXAMPLE.com` and
`example.com`. URL scheme checks are now case-insensitive as well, so
`HTTP://` and `HTTPS://` forms follow the same grant matching path.
Resolved socket-address lists now also go through shared normalization:
duplicates are removed, IPv4 addresses are preferred before IPv6 while
first-seen order is still preserved inside each family, unspecified wildcard
targets (`0.0.0.0` and `::`) are dropped, unusable targets (`port=0` and
scope-less IPv6 link-local addresses) are filtered, non-unicast targets
(IPv4 broadcast, IPv4 multicast, and IPv6 multicast) are filtered, and
connect-attempt lists are capped to a fixed maximum so DNS result variance
cannot create unbounded retry loops.
When all resolved targets are filtered out by these guards, runtime resolution
now maps that case to a deterministic not-found path instead of leaving
host-specific behavior to later connect loops.
In this early plain-HTTP slice, URL parsing is also ASCII-only. Non-ASCII URLs
are rejected early so request framing and capability endpoint checks stay
deterministic until broader URL handling lands in a later hardening pass.
Request targets now also have a shared size limit before request framing so
this early plain-HTTP path rejects oversized path/query payloads up front.
The runtime's network capability gate now also uses a shared endpoint parser for
`http://` and `https://` URLs, so policy checks and adapter-side URL validation
no longer drift as separate parsers evolve. The plain `http://` URL parser now
reuses that same authority parsing path to keep host/port validation in one
place.
For helper-style `net.http-client.get` calls, the default request timeout is now
runtime-configurable through `krate run --http-timeout-millis`. The default is
5000 milliseconds, and `--http-timeout-millis 0` disables that default timeout
for the helper `get` path. For multi-address connect attempts, timed requests
now spend one shared connect-time budget across all resolved addresses instead
of applying the full timeout per address.

Time is also starting to move into shared adapter code. The local runtime now
uses a common host clock helper for fixed test time, Unix-epoch milliseconds,
monotonic elapsed time, and sleep. That keeps future desktop adapters from each
making slightly different clock choices. It now also guards edge cases:
monotonic nanoseconds saturate instead of wrapping, and out-of-range
Unix-millisecond values fail with a clear conversion error.

Locale has the same first shared path now. The runtime uses common helper code
for `LC_ALL`/`LANG` locale detection, `TZ` fallback, basic locale normalization,
deterministic baseline date/number formatting, and locale-tag canonicalization
to stable language/script/region casing with a safe fallback for malformed
values, including stricter primary locale-subtag and bounded-subtag checks so
invalid locale-tag shapes fall back to `en-US`. Timezone normalization is now
conservative too, accepting only simple timezone-name shapes for this phase and
falling back to `UTC` on invalid input. It now also accepts normalized UTC
offset forms such as `UTC+05:30`, `GMT-02:00`, `+0530`, and `-07`, then stores
them as canonical `UTC±HH:MM`. Locale discovery now also has practical
fallbacks for `LC_TIME`, `LC_NUMERIC`, `LC_MONETARY`, `LC_CTYPE`,
`LC_COLLATE`, `LC_MESSAGES`, `LANGUAGE` (first preferred token), and
`AppleLocale` when `LC_ALL`/`LANG` are absent. Timezone discovery now has a
Unix fallback too: when `TZ` is not set and `/etc/localtime` is a zoneinfo
symlink, Krate derives a normalized timezone from that link target. It now
also falls back to `/etc/timezone` parsing if the localtime symlink path is not
usable, with strict parsing rules (ignore comments/blank lines, accept first
valid candidate, reject malformed timezone shapes). Real ICU4X formatting and
broader host-native per-OS locale/timezone discovery are still open, but the
early behavior now has one home instead of being copied in the runtime. This
fallback order is captured in
`docs/adr/0010-locale-timezone-discovery-fallbacks.md`.
Date formatting now also applies normalized UTC offsets to the rendered time
value, so `UTC+05:30` and `UTC-01:00` shift timestamps instead of only changing
the timezone label text.
As part of the per-OS adapter split, runtime locale and clock construction now
route through dedicated Linux, macOS, and Windows adapter crates, with fallback
logic kept for unsupported hosts.
The same is now true for `time.sleep-millis`, and locale read/format paths
(`current`, `timezone`, `format-date`, `format-number`), which now route
through the per-OS adapter path instead of runtime-local direct calls.
Filesystem no-follow open-flag setup and plain HTTP TCP connect calls now also
route through the same per-OS adapter crate path.
Socket-address resolution for plain HTTP fetch now follows that same per-OS
adapter route too.
Socket timeout setup for that fetch path now also routes through per-OS
adapters.
Sandbox link-metadata checks now also route through per-OS adapters, so both
symlink/reparse-point detection and network host calls are less runtime-local.
Filesystem `stat` and directory listing host calls now route through per-OS
adapters too.
Filesystem mutation host calls (`remove-file`, `remove-dir`, `mkdir`, `rename`)
now route through per-OS adapters as well.
Sandbox symlink metadata reads now also route through per-OS adapters.
Sandbox-root and parent-path canonicalization now also route through per-OS
adapters.
Filesystem `open` host calls now also route through per-OS adapters.
Runtime component-file reads and sandbox-root directory creation now also route
through per-OS adapters.
File-handle read, write, seek, and metadata host calls now also route through
per-OS adapters.
Stdin reads and stderr write or flush host calls now also route through per-OS
adapters.
Plain HTTP request writes and response reads now also route through per-OS
adapters.
Shared stdout print, write, and flush paths now also route through per-OS
adapters.

There is also a first smoke app under `test/integration/phase2-smoke`. It is not
one of the final sample apps yet. Its job is smaller: prove that a real Phase 2
component can read a file, call time and locale, and print through the UAPI
path. CI builds that component and runs it through `krate run` on the host
test matrix. The same smoke app now has a missing-grant test too: without
`fs.read`, the host returns permission denied and the component exits with a
clear stderr message.

The first named sample app has started too. `apps/krate-clock` is a Rust
component that reads time and locale through UAPI, then prints through UAPI
stdout. The CLI now has a hidden `--test-time` flag, so tests can freeze the
clock and check stable output. It now also has hidden `--test-locale` and
`--test-timezone` flags, so fixture tests can pin all clock output fields and
assert one exact snapshot across hosts.

The second sample path has started as well. `apps/krate-cat` reads app
arguments through `krate:io/args.raw`, opens files through `krate:fs/files`,
and writes bytes to UAPI stdout. The tests prove both sides: it reads files with
the right `fs.read` grant, and gets permission denied without that grant. It
also denies a file outside the granted glob with exit code `5`, matching the
CLI's permission-denied convention. In this phase, raw app-argument transport
is intentionally conservative: empty arguments, newline/NUL delimiter
characters, too many argument entries, and oversized raw payloads are rejected
before they reach guest argument parsing. Krate CLI now also does the same
check as a preflight step, so these invalid argument shapes fail before runtime
startup.

The third sample path has started now too. `apps/krate-curl` reads a URL from
Krate app args, calls `krate:net/http-client.get`, and writes the response
body through UAPI stdout. Its first tests use a local HTTP server: with
`net.connect:127.0.0.1:PORT` it fetches, without that grant it exits before the
runtime opens a socket. Permission denial also exits with code `5`.
Oversized responses, timeouts, and malformed HTTP responses now print specific
curl messages too.

The generated UAPI reference has also grown past a raw signature list. It now
pulls capability strings from the manifest crate and adds short behavior notes
under each function and resource method, so the docs explain both the call shape
and the permission model in one place.
The WIT files now carry first-pass contract comments for the Phase 2 world,
interfaces, records, variants, enum cases, resource methods, and functions.
Those comments flow into the generated reference, so the published API page is
closer to a real freeze candidate instead of only showing type signatures.

The UAPI gate is stricter now too. `scripts/check-uapi.sh` first runs
`wasm-tools component wit` across the Phase 2 world and all dependency
packages (`io`, `fs`, `net`, `time`, `locale`), then runs the contract-shape
checks in `krate-tools --bin check-uapi`. That checker now also fails if
public Phase 2 WIT items are missing contract docs. Hosted and self-hosted CI
both run this before UAPI reference regeneration.

The Rust SDK publish-readiness smoke is stricter too. It still packages the
`krate` crate and compiles a tiny outside-workspace component against that
packaged crate, and now it also checks that the packaged crate contains the
public README, SDK root, and generated bindings files before passing.
There is now a [Rust SDK Evidence](../phase2/rust-sdk-evidence.md) page too.
`scripts/record-phase2-rust-sdk-evidence.sh` records package smoke results,
SDK doc-build results, and packaged-file presence in one report. Normal hosted
CI uploads that report as a `rust-sdk-evidence` artifact, which gives `P2E-03`
a concrete proof source while crates.io publishing waits for UAPI freeze.

There is now a dedicated [UAPI Freeze Review](../phase2/uapi-freeze-review.md)
page. It turns the remaining freeze work into a clear checklist for contract
shape, runtime behavior, samples, language tracks, and evidence.
There is also a generated [UAPI Freeze Evidence](../phase2/uapi-freeze-evidence.md)
page now. It records the current package set, imported interface set, world
shape, and contract checks in one place, so freeze review has a concrete
snapshot instead of only a checklist. Hosted CI and self-hosted CI now
regenerate that page and fail if it is stale.
There is also a generated [UAPI Freeze Lock](../phase2/uapi-freeze-lock.md).
It records SHA-256 hashes for the exact Phase 2 WIT files under review. CI
regenerates it and fails if the committed lock is stale, so UAPI contract drift
has to be intentional and visible.
The [UAPI Freeze Review Evidence](../phase2/uapi-freeze-review-evidence.md)
page now explains the local review recorder. That recorder checks the WIT
contract, generated reference, freeze evidence, freeze lock, adapter-boundary
guard, and exit ledger in one report. It gives the freeze review a concrete
artifact without pretending the final decision is already made.
There is now a [Phase 2 Exit Evidence](../phase2/exit-evidence.md) ledger too.
It tracks all 15 exit gates with a status, proof source, and next step, and CI
checks the page shape so the list cannot silently drift from the plan.
The quick local review path is now [Exit Bundle](../phase2/exit-bundle.md).
`scripts/record-phase2-exit-bundle.sh` runs the core local exit checks and
writes one report with command results, the current gate snapshot, working tree
state, and log tails. This is not a completion stamp. It is a cleaner handoff
file for review before the final Phase 2 decision.
For the one human proof gate, there is now a
[Timed Walkthrough Evidence](../phase2/walkthrough-evidence.md) page and a
template generator. This keeps the outside Rust walkthrough tied to a commit,
with one place for timing, step results, and notes.

The adapter split now has a guard too. The
[Adapter Boundary](../phase2/adapter-boundary.md) page explains the rule in
plain terms: the runtime checks policy, then the OS adapter touches the host.
`scripts/check-adapter-boundary.sh` checks 34 runtime wrappers and verifies that
the Linux, macOS, and Windows adapter crates expose the matching functions.
Hosted CI and the self-hosted full gate run that check so direct host-call
backsliding is easier to catch.
There is now an [Adapter Evidence](../phase2/adapter-evidence.md) flow too:
`scripts/record-phase2-adapter-evidence.sh` records a host report and
`scripts/compare-phase2-adapter-evidence.sh` compares Linux/macOS/Windows
reports for one commit. Hosted full CI now publishes per-OS adapter evidence
artifacts and runs the compare gate automatically.
The adapter report now includes more than boundary shape: it also runs shared
adapter behavior tests and the native adapter crate tests for that host. This
keeps P2E-02 pointed at real behavior proof instead of only checking that
function names exist.

Sample evidence has a repeatable recorder now too. The
[Sample Evidence](../phase2/sample-evidence.md) page explains how to run
`scripts/record-phase2-sample-evidence.sh` on each desktop host. The recorder
builds the CLI and Rust clock/cat/curl components, runs fixed fixtures, and
writes a markdown report with host metadata, commands, exit codes, stdout
hashes, and output snapshots. That gives the Phase 2 exit review a practical
way to compare Linux, macOS, and Windows sample behavior.
There is also a companion comparator now:
`scripts/compare-phase2-sample-evidence.sh` wraps
`krate-tools --bin compare-phase2-sample-evidence` so three host reports can
be checked in one run. It fails fast when clock/cat/curl stdout hashes drift
across hosts and supports a temporary `--allow-blocked-curl` exception for
restricted localhost environments. That exception is now narrow: if two hosts
run curl successfully and one host is blocked, the successful curl hashes must
still match.
Hosted full CI now also records per-OS sample evidence artifacts and runs a
`sample-evidence-compare` job automatically. That compare step currently uses
the curl blocked exception, while still enforcing strict cross-host hash parity
for clock and cat outputs plus any successful curl outputs.
Language-variant evidence now has a recorder as well:
[Language Variant Evidence](../phase2/language-variant-evidence.md) explains
how to run `scripts/record-phase2-language-variant-evidence.sh`. That report
captures fixture readiness plus test outcomes for Rust, Go, and TypeScript in
one markdown artifact, with fixture hashes and log tails for review.
There is now a companion comparator too:
`scripts/compare-phase2-language-variant-evidence.sh` checks three host reports
in one run, verifies same commit metadata, enforces passed build/test steps,
and fails when fixture availability is not aligned or a present fixture is
missing its hash. It does not require TypeScript fixtures generated by jco on
different operating systems to be byte-identical. This lane proves portable
behavior through Krate, not reproducible compiler output.
Hosted full CI now runs that comparator automatically after Linux, macOS, and
Windows full-test lanes upload their evidence artifacts.
UCap deny evidence now follows the same pattern:
[UCap Enforcement Evidence](../phase2/ucap-enforcement-evidence.md) shows how
to record one report per host with `scripts/record-phase2-ucap-evidence.sh` and
compare those reports with `scripts/compare-phase2-ucap-evidence.sh`.
The report now includes a named dispatcher matrix that proves every non-default
filesystem and network boundary returns permission denied before an adapter can
touch the host.
Hosted full CI now uploads per-host UCap evidence artifacts and runs a compare
job so deny-path regressions are caught as a cross-host gate, not only in local
checks.
Performance evidence now has the same repeatable flow:
[Benchmark Evidence](../phase2/benchmark-evidence.md) shows how to record
startup and dispatch benchmark results, run baseline regression checks, and
compare Linux/macOS/Windows benchmark reports for commit and gate consistency.
That report now also includes full external CLI startup evidence for
`krate run krate-clock`, so `P2E-10` is tracking the command users will
actually run, not only the in-process runtime path.
The benchmark comparator also enforces per-host threshold bounds from the
metric table, so a host can no longer pass this lane with an over-threshold
metric hidden behind warning-only regression mode.
This gives `P2E-10` and `P2E-11` clearer cross-host proof tracking even when
raw timing numbers differ by hardware.
Dependency evidence is tracked beside those performance checks:
[Dependency Evidence](../phase2/dependency-evidence.md) records the
`cargo-deny` wrapper output, tool versions, and any advisory-database warning
so dependency signoff is explicit instead of buried in CI logs.

The first terminal grant prompt exists too. `krate run --prompt app.wasm`
shows the app identity, lists missing manifest capabilities, accepts all or a
numbered subset, and stores the approved caps only for that run. In a normal
terminal the same prompt can appear automatically when required capabilities
are missing. In non-interactive runs, Krate keeps the safer behavior and
fails with a clear permission message.

There is now a small manifest trust check as well. If the sidecar manifest says
the app entry is `app.wasm`, then `krate run` must be pointed at that same
file. Running a different component with that manifest is rejected before any
grant prompt or runtime execution.

For debugging, `krate run --dump-caps app.wasm` now prints the effective
session capabilities and exits before the component starts. It is a simple way
to see what the current grant resolution actually produced.

The proof apps are:

- `krate-curl`
- `krate-cat`
- `krate-clock`

The Go and TypeScript SDK tracks now also include matching clock/cat/curl sample
sources with CI shape checks. The CLI test harness now also has optional
fixture assertions for Go and TypeScript variants behind `KRATE_GO_*` and
`KRATE_TS_*` WASM env vars. We now also run a pre-test fixture build step
(`scripts/build-phase2-language-variant-fixtures.sh`) in hosted full CI and
self-hosted CI. Hosted full CI now runs that step in `ts` mode by default and
allows `npx` install for jco (with a pinned package version). The full-test
matrix also pins Node 22 for this lane, so TypeScript runtime fixtures stay
active without manual runner setup. Go lane status remains explicit.
Hosted CI now also uses Node 24-ready core actions (`actions/checkout@v5` and
`actions/setup-node@v5`) to remove Node 20 deprecation warnings in daily runs.
The same fixture build step now also auto-attempts Go runtime fixture promotion
when `go`, `tinygo`, and `wasm-tools` are available, so both language tracks
use one orchestration entry point.
When fixture mode is set to `go` or `both`, that Go promotion path now runs in
strict required mode so missing or non-pure Go fixtures fail that run clearly.
The shared fixture build step now also prints per-language readiness reasons
and includes those reasons in strict mode failures, so CI triage is faster when
language fixture lanes fail on toolchain or fixture-state issues.
TypeScript fixture generation now enforces Krate-only imports and uses the
real WIT variant shape for filesystem open mode (`{ tag: "read" }`), which made
the TypeScript cat fixture runtime path stable in local tests.
TypeScript curl fixture coverage now also includes non-localhost denial and
unresolved-host error checks, so restricted runners still prove key curl failure
paths even when localhost fixture sockets are unavailable.
TypeScript curl error classification is now aligned with Rust for two critical
failure paths: missing net grant now exits with code `5` and invalid URL now
exits with code `20`. The CLI harness now includes explicit Rust versus
TypeScript parity checks for both of those paths.
Go curl fixture coverage now includes matching non-localhost denial and
unresolved-host checks with stable stderr markers.
The Go curl sample now also uses a stable, case-insensitive error classifier
with unit tests for key Krate net error classes, so common failure paths
map to predictable messages and exit codes (`5`, `20`, `21`).
The dedicated Go curl fixture tests now assert those same contracts directly,
instead of allowing broad fallback statuses, so parity regressions fail earlier.
The language-variant parity tests now check curl error paths across Rust, Go,
and TypeScript too: missing grant, invalid URL, and unresolved-host cases are
validated together without requiring localhost fixture sockets.
The Go fixture promotion diagnostics are also clearer now. When TinyGo outputs
are not Krate-import pure, the import checker reports every failing Go
artifact instead of stopping at the first one. Current local smoke artifacts
build successfully, but still import WASI host APIs, so they are correctly not
promoted into runtime fixtures yet.
There is now a Go readiness evidence recorder as well. It builds the TinyGo
smoke artifacts, records Go, TinyGo, and `wasm-tools` versions, hashes the
artifacts, and stores the import-purity log in one review file. This does not
make Go complete. It makes the remaining Go blocker visible enough for a clean
Phase 2 exit decision.
When both Go and TypeScript fixture sets are available, the CLI harness now
also runs cross-language parity checks for `krate-clock`, `krate-cat`, and
`krate-curl`. Those checks run Rust, Go, and TypeScript samples with the same
inputs and assert byte-identical stdout. The curl parity check uses a local
fixture server and skips cleanly on restricted runners that cannot use local
socket fixtures.
Self-hosted CI now runs a TinyGo WASI Preview 2 build-smoke lane for Go
clock/cat/curl samples and then tries to promote those outputs into
`test/integration/language-variants/krate_go_*.wasm`.
Promotion only happens when import-purity checks pass (`krate:*` imports
only). If current TinyGo outputs still import host `wasi:*`, the promotion step
prints a clear skip message and keeps Go runtime fixtures disabled.
Krate-runtime fixture proof for Go therefore remains pending, but it is now
guarded by an explicit readiness gate.
We now also have three language walkthroughs in the docs: Rust, Go, and
TypeScript, so contributors can onboard per language without guessing the
current phase boundaries.
We also now have the first fuzz-harness set for manifest parsing, shared path
normalization, and policy matching. Nightly multi-hour fuzz runs are still a
remaining exit item, but the run path is now configurable without editing
scripts. `scripts/run-phase2-fuzz-smoke.sh` accepts
`KRATE_FUZZ_MAX_TOTAL_TIME` (seconds per target), and self-hosted CI now
accepts a matching manual input. There is also a dedicated
`Self-hosted Fuzz Nightly` workflow that defaults to long runs on local
runners.
The first self-hosted fuzz smoke already paid off by finding and helping fix a
real non-ASCII path-parsing panic in shared path handling.
Timezone fallback coverage also now includes extra Unix timezone file paths
(`/etc/TIMEZONE` and `/var/db/timezone/timezone`) in addition to `/etc/timezone`,
which helps hosts that do not expose timezone only through `/etc/localtime`.
The closeout material is also drafted now. The Phase 2 retrospective draft and
Phase 3 kickoff issue draft exist, and a small checker keeps both in draft form
until the final exit review is actually done. That checker now runs in hosted
CI, self-hosted CI, and the exit bundle.

If Phase 2 works, those apps should produce the same output on Linux, macOS, and
Windows while running through the same Krate runtime model.

If you used the Phase 1 proof app, read
[Migrating From Phase 1 To Phase 2](../phase2/migrating-from-phase1.md).

See [`Plan/Phase-2-Plan.md`](https://github.com/incyashraj/krate/blob/main/Plan/Phase-2-Plan.md).
