# Phase 1 — POC Runtime

**Status:** In Progress  
**Duration:** Months 2–3  
**Sentence:** *Prove one `.wasm` binary runs identically on three desktop hosts.*

## Current slice

Phase 1 has started with the runtime and CLI scaffold:

- `crates/runtime` initializes Wasmtime with Component Model support.
- `crates/cli` builds the `layer36` binary.
- `layer36 --help`, `layer36 version`, and `layer36 doctor` work locally.
- `layer36 run <file>` validates input paths and routes component execution
  through the runtime crate.
- `scripts/build-hello-component.sh` builds the Rust hello-world component.
- `layer36 run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm`
  prints `Hello, Layer36!` locally.
- `layer36 run --fuel 1 ...` and `layer36 run --mem-limit 0 ...` fail
  cleanly with exit code 4 and a limit-exceeded message.
- CI builds the hello-world component once on Ubuntu, uploads that exact
  `.wasm` as a workflow artifact, verifies its SHA-256 hash, and runs the same
  bytes through the real `layer36` binary on Linux, macOS, and Windows.
- `scripts/test-phase1.sh` can either build the hello-world fixture locally or
  consume a prebuilt `LAYER36_HELLO_WASM` fixture with an expected
  `LAYER36_HELLO_SHA256`.
- `.github/workflows/release.yml` packages the five planned Phase 1 release
  artifacts on `v*` tags and publishes a `SHA256SUMS` file.
- `v0.1.0-rc1` is published as a GitHub prerelease with Linux x64, Linux
  ARM64, macOS Intel, macOS Apple Silicon, Windows x64, and `SHA256SUMS`
  assets.
- The Phase 1 quickstart is published at `docs/book/src/quickstart.md` and
  walks from a fresh checkout to `Hello, Layer36!`.
- Threat Model v0.1 is published at `docs/book/src/phase1/threat-model.md`.
- Baseline runtime benchmarks are published at
  `docs/book/src/phase1/benchmarks.md` and checked in CI as warning-only
  regression signals.

The runtime now registers the temporary `layer36:phase1/host` WIT interface for
`print` and `exit`. The Linux/macOS/Windows CI matrix is green, the first RC
release artifact proof is green, and volunteer quickstart timing is still
pending.

See [`Plan/Phase-1-Plan.md`](https://github.com/incyashraj/layer6x6/blob/main/Plan/Phase-1-Plan.md).
