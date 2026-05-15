# Your First UAPI App In Go

This is the current shortest path to start building Layer36 apps in Go.
Today, the Go track is still in scaffold mode. So this walkthrough focuses on
the parts that are already real and testable right now.

## What You Build Today

- a small Go app shape that uses Layer36 SDK imports
- a checked package layout
- a clear view of what is already wired and what is still pending

## 1. Check Tooling

From repo root:

```bash
cargo run -p layer36-cli -- doctor
```

Look for:

- `go`
- `tinygo`

If either is missing, you can still follow the SDK structure work, but runtime
component proof will stay blocked.

## 2. Start From The Go Cat Example

Use the sample at:

```text
packages/sdk-go/examples/layer36-cat/main.go
```

That sample already shows the right direction:

- read app args through Layer36 SDK
- read files through Layer36 SDK
- print through Layer36 SDK

This keeps host access behind UAPI and capability checks.

## 3. Run The Shape Check

From repo root:

```bash
node packages/sdk-go/scripts/check-shape.mjs
```

This check is cheap and fast. It confirms that Go SDK package structure and
public helper names still match our current Phase 2 contract.

## 4. Fixture Build Step (Current State)

From repo root:

```bash
scripts/build-phase2-language-variant-fixtures.sh
```

Today this script can auto-build the TypeScript fixture trio when `jco` is
available and reports Go status clearly. Go fixtures still need either manual
provisioning for runtime tests.

There is now also a Go TinyGo build-smoke lane:

```bash
scripts/build-phase2-go-variant-smoke.sh
scripts/record-phase2-go-readiness-evidence.sh
```

The first command builds Go clock/cat/curl WASI Preview 2 artifacts and checks
their component export shape (`wasi:cli/run`). The second command writes a small
evidence report with tool versions, artifact hashes, and the import-purity log.

The current TinyGo artifacts do build, but they are not Layer36-import pure yet.
The promotion step checks all three artifacts together and reports every
non-Layer36 import it sees. Current blockers are:

- clock and cat still import WASI stdout, IO stream/error, random, and the
  TinyGo run import helper
- curl imports a wider WASI surface because it currently pulls in environment,
  exit, stdio, clocks, filesystem, IO, random, and the TinyGo run import helper

That is useful progress: the build shape works, and the remaining Go task is to
replace those WASI host calls with real `layer36:*` imports before runtime
fixtures can be promoted.

## 5. Optional Runtime Variant Test Hook

If you already have compiled Go WASM fixtures at:

```text
test/integration/language-variants/
```

run:

```bash
scripts/test-phase2-language-variants.sh
```

If those fixtures are not present, the script exits cleanly and explains that
tests were skipped. If you provide Go fixture env vars, provide all three
(`clock`, `cat`, and `curl`) so the runtime lane runs as one complete set.
When all three are present, the script also runs the component import-purity
check before runtime assertions.
The Go curl runtime assertions now include missing-grant and unresolved-host
paths that do not require localhost fixture sockets.
You can force stricter CI behavior with `LAYER36_LANGUAGE_VARIANTS_MODE`.
Useful values are `optional` (default), `go`, `ts`, `any`, and `both`.

## 6. Where This Fits In Phase 2

Go is now at "SDK and test harness ready" stage.

Still pending:

- Layer36-import pure runtime fixture proof for Go variants
- always-on runtime fixture gate for Go variants

That means we are not blocked. We can keep improving UAPI and policy hardening
while finishing the Go build lane in parallel.
