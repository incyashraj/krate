# CI and runners

Layer36 uses two CI paths.

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

There is also a `Self-hosted CI` workflow. It is manual-only and targets a
runner labeled `layer36-local`. Use it when you want GitHub to run the full
local gate on your own machine instead of a hosted runner.

The runtime tests now include an optional Phase 2 language-variant slice for Go
and TypeScript sample components. It runs through:

```bash
scripts/test-phase2-language-variants.sh
```

By default it skips unless any of these env vars are set to built component
paths:

- `LAYER36_GO_CLOCK_WASM`
- `LAYER36_GO_CAT_WASM`
- `LAYER36_GO_CURL_WASM`
- `LAYER36_TS_CLOCK_WASM`
- `LAYER36_TS_CAT_WASM`
- `LAYER36_TS_CURL_WASM`

## Setting up a local runner

In GitHub, open:

`Settings -> Actions -> Runners -> New self-hosted runner`

Choose macOS if you are using your Mac. GitHub will show the exact commands to
download and configure the runner. When it asks for labels, add:

```text
layer36-local
```

Start the runner with the command GitHub shows, usually:

```bash
./run.sh
```

After that, open `Actions -> Self-hosted CI -> Run workflow`.

The runner uses your machine, so close it when you do not want jobs to start.
For a personal project machine, the safest habit is to run it only while you are
actively working.
