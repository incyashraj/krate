# CI and runners

Krate uses two CI paths.

The normal `CI` workflow runs on GitHub-hosted runners. Because the repository
is public, this is the best default path for daily work. Pushes to `main` and
pull requests run the cheap checks:

- Rust formatting
- Clippy
- Linux workspace tests
- mdBook docs build

The expensive checks stay opt-in. Run the `CI` workflow manually with
`full = true`, or include `[full-ci]` in a push commit message, when you want:

- shared component fixture builds
- Linux, macOS, and Windows runtime checks
- Phase 2 host-binding checkpoint
- benchmarks
- `cargo-deny`

Hosted workflows use Node 24-ready core actions (`actions/checkout@v5` and
`actions/setup-node@v5`). Cache and artifact steps use the Node 24 action
families too: `actions/cache@v5`, `actions/upload-artifact@v7`, and
`actions/download-artifact@v8`. Pages uses `actions/configure-pages@v5` and
`actions/upload-pages-artifact@v4`.

All workflows also set `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true`, so any action
that has not moved its major tag yet still runs under the Node 24 action runtime.
This keeps CI evidence cleaner and avoids waiting for the June 2026 runtime
switch to surprise us.

In hosted full CI, the Phase 2 TypeScript language-variant lane now runs in
`ts` mode by default. The fixture build step can install jco through `npx`
when needed, and the full-test matrix pins Node 22 for this lane, so TypeScript
runtime fixtures stay active without manual runner tool preinstall.
Self-hosted CI now uses the same pinned `npx` fallback for jco in its fixture
build step.

Dependency audit now runs through `scripts/check-dependencies.sh`. This keeps
`licenses`, `bans`, and `sources` as hard failures, while known advisory-db
parser or lock-path failures in the current `cargo-deny` path are downgraded to
warnings until upstream compatibility catches up.

There is also a `Self-hosted CI` workflow. It is manual-only and targets a
runner labeled `krate-local`. Use it when you want GitHub to run the full
local gate on your own machine instead of a hosted runner.
Self-hosted workflows currently keep `actions/checkout@v4` to preserve
compatibility with older local runner installs.
They still opt JavaScript actions into Node 24, so a local runner must be new
enough to support that runtime. For the newer cache and artifact action
families, use runner `2.327.1` or newer.
The scheduled `Self-hosted Fuzz Nightly` workflow now keeps older queued runs
when the runner is offline. Automatic schedule runs do not cancel each other
anymore, while manual dispatch still cancels in-progress runs for the same
branch.
That local gate now also runs a short Phase 2 fuzz smoke over the first fuzz
targets.
The benchmark regression step is warning-only by default in this manual
workflow, and you can switch it to strict fail mode with the
`benchmark_regression_mode` input when you want to enforce performance gating.
It now runs a TinyGo WASI Preview 2 build-smoke lane for Go clock/cat/curl
samples and then tries to promote those artifacts into Krate runtime fixtures
through:

```bash
scripts/promote-phase2-go-runtime-fixtures.sh
```

The promotion script still builds with TinyGo and checks `wasi:cli/run` export
shape. It then runs import-purity checks. Only import-pure outputs are copied to
`test/integration/language-variants/krate_go_*.wasm` and picked up by runtime
variant tests. When imports are still host `wasi:*`, the script keeps the Go
runtime fixtures disabled and prints a clear skip message.

The TypeScript curl variant now has extra runtime assertions that do not depend
on localhost fixture sockets. Missing-grant and unresolved-host paths are
checked directly, so restricted runners still provide useful curl evidence.
The Go curl variant now has matching non-localhost denial and unresolved-host
assertions too, so we keep useful failure-path evidence across both language
lanes even when localhost fixture sockets are blocked.

The runtime tests now include an optional Phase 2 language-variant slice for Go
and TypeScript sample components. It runs through:

```bash
scripts/test-phase2-language-variants.sh
```

When both Go and TypeScript fixture sets are present, this script now also runs
cross-language parity checks for Rust, Go, and TypeScript `clock`, `cat`, and
`curl` samples.

By default it skips unless any of these env vars are set to built component
paths:

- `KRATE_GO_CLOCK_WASM`
- `KRATE_GO_CAT_WASM`
- `KRATE_GO_CURL_WASM`
- `KRATE_TS_CLOCK_WASM`
- `KRATE_TS_CAT_WASM`
- `KRATE_TS_CURL_WASM`

It can also auto-discover built variant fixtures at these paths when the env
vars are not set:

- `test/integration/language-variants/krate_go_clock.wasm`
- `test/integration/language-variants/krate_go_cat.wasm`
- `test/integration/language-variants/krate_go_curl.wasm`
- `test/integration/language-variants/krate_ts_clock.wasm`
- `test/integration/language-variants/krate_ts_cat.wasm`
- `test/integration/language-variants/krate_ts_curl.wasm`

## Setting up a local runner

In GitHub, open:

`Settings -> Actions -> Runners -> New self-hosted runner`

Choose macOS if you are using your Mac. GitHub will show the exact commands to
download and configure the runner. When it asks for labels, add:

```text
krate-local
```

Start the runner with the command GitHub shows, usually:

```bash
./run.sh
```

After that, open `Actions -> Self-hosted CI -> Run workflow`.

The runner uses your machine, so close it when you do not want jobs to start.
For a personal project machine, the safest habit is to run it only while you are
actively working.
