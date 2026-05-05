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

## 4. Optional Runtime Variant Test Hook

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

## 5. Where This Fits In Phase 2

Go is now at "SDK and test harness ready" stage.

Still pending:

- full TinyGo component build path
- routine fixture generation in CI
- always-on runtime fixture gate for Go variants

That means we are not blocked. We can keep improving UAPI and policy hardening
while finishing the Go build lane in parallel.
