# Go Readiness Evidence

This page records the current Phase 2 state for Go.

The short version: the Go examples build with TinyGo, but they are not runtime
fixtures yet. They still import some WASI host APIs directly. Phase 2 keeps them
out of the runtime fixture set until those imports are replaced by `layer36:*`
UAPI imports.

## Why This Matters

Layer36 is trying to make apps portable by keeping host access behind one small
runtime boundary.

For Go, that means a compiled component should call Layer36 UAPI packages such
as `layer36:io`, `layer36:fs`, and `layer36:net`. It should not reach around
the runtime and call `wasi:filesystem`, `wasi:stdio`, or other host APIs
directly.

That rule keeps the security model simple:

```mermaid
flowchart LR
    GO["Go app"] --> L36["layer36:* UAPI"]
    L36 --> POLICY["Layer36 policy checks"]
    POLICY --> ADAPTER["Host adapter"]
    ADAPTER --> OS["Operating system"]

    GO -.blocked for runtime fixtures.-> WASI["wasi:* host API"]
    WASI -.bypasses Layer36 policy.-> OS
```

## Run The Recorder

From the repo root:

```bash
scripts/record-phase2-go-readiness-evidence.sh
```

Default output:

```text
target/phase2-go-readiness-evidence/go-readiness-evidence.md
```

For an exit-style check that fails when Go is not import-pure:

```bash
scripts/record-phase2-go-readiness-evidence.sh --strict
```

## What It Records

The report includes:

- git commit, host OS, host architecture, and timestamp
- Go, TinyGo, and `wasm-tools` versions
- TinyGo smoke build result for clock, cat, and curl
- import-purity result for the three compiled components
- SHA-256 hashes for the smoke artifacts
- the full import-purity log tail

## Current Decision

Go is still a Phase 2 binding track, but not a promoted runtime fixture track.

The current acceptable Phase 2 decision is:

- keep Go SDK source, examples, shape checks, and TinyGo build smoke
- keep Go runtime fixture promotion gated by import purity
- do not claim Go runtime parity until the compiled artifacts import only
  `layer36:*`
- if Phase 2 must exit before that work is complete, mark Go as experimental
  for this phase and carry the import-pure runtime proof into the next phase

That is not a failure of direction. It is the correct boundary doing its job.
