# Quickstart: Run A Phase 2 Layer36 Component

This walkthrough builds the Layer36 CLI, builds the current Phase 2 sample
components, and runs a file-reading WebAssembly component through the Layer36
UAPI and capability path.

At the end your terminal should print:

```text
hello from Layer36
```

Layer36 is still pre-alpha. This quickstart is for local developer proof, not
for running untrusted third-party components.

## Prerequisites

Install:

- Git
- Rust via `rustup`
- `cargo-component`

Layer36 pins its Rust toolchain in `rust-toolchain.toml`, so entering the repo
lets `rustup` install the right compiler and WASM targets.

Install the component tooling:

```bash
cargo install cargo-component --locked --version 0.21.1
```

## Get The Source

```bash
git clone https://github.com/incyashraj/layer6x6.git
cd layer6x6
```

## Build Layer36

```bash
cargo build -p layer36-cli
```

Check the local environment:

```bash
target/debug/layer36 doctor
```

`doctor` reports core Rust tools first, then Phase 2 language tools such as
`wasm-tools`, `tinygo`, `go`, `node`, `npm`, and `jco` when they are available.

## Build The Phase 2 Samples

```bash
scripts/build-layer36-clock-component.sh
scripts/build-layer36-cat-component.sh
scripts/build-layer36-curl-component.sh
```

The scripts print component paths like:

```text
apps/layer36-cat/target/wasm32-wasip1/release/layer36_cat.wasm
```

## Inspect A Manifest

Phase 2 apps carry a `manifest.toml` for app identity and capability requests.
Before running the file sample, inspect what it asks for:

```bash
target/debug/layer36 manifest explain apps/layer36-cat/manifest.toml
```

You should see:

- `io.args`, `io.stdout`, and `io.stderr` as default-granted app plumbing
- `fs.read:fixtures/**` as a non-default launch grant

That is the current permission model in simple form:

```text
low-risk app plumbing -> default grant
host file/network access -> explicit launch grant
```

## Run The Clock Sample

Run a deterministic clock sample through the Phase 2 UAPI path:

```bash
target/debug/layer36 run \
  --auto-grant \
  --manifest apps/layer36-clock/manifest.toml \
  --test-time 1234567890 \
  --test-locale en-US \
  --test-timezone UTC \
  apps/layer36-clock/target/wasm32-wasip1/release/layer36_clock.wasm
```

This exercises time, locale, timezone, and stdout. The test flags make output
stable enough for evidence runs.

## Run The File Sample

Create a test file:

```bash
mkdir -p apps/layer36-cat/fixtures
printf 'hello from Layer36\n' > apps/layer36-cat/fixtures/hello.txt
```

Run with the sample manifest and grant approval:

```bash
cd apps/layer36-cat
../../target/debug/layer36 run \
  --manifest manifest.toml \
  --auto-grant \
  target/wasm32-wasip1/release/layer36_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

Expected output:

```text
hello from Layer36
```

## See The Denial Path

Run the same app without granting the file capability:

```bash
cd apps/layer36-cat
printf '' | ../../target/debug/layer36 run \
  --manifest manifest.toml \
  target/wasm32-wasip1/release/layer36_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

In a non-interactive shell, Layer36 exits before starting the component and
prints the missing required capability. That is intentional: host file access
should be explicit.

## Optional Network Sample

Start a local HTTP server in one terminal:

```bash
mkdir -p /tmp/layer36-demo-http
printf 'portable runtime response\n' > /tmp/layer36-demo-http/demo.txt
cd /tmp/layer36-demo-http
python3 -m http.server 8765
```

In another terminal, run the curl sample with an explicit network grant:

```bash
cd /path/to/layer6x6
target/debug/layer36 run \
  --grant net.connect:127.0.0.1:8765 \
  apps/layer36-curl/target/wasm32-wasip1/release/layer36_curl.wasm \
  -- http://127.0.0.1:8765/demo.txt
```

If localhost sockets are restricted in your environment, skip this sample and
use the file and clock samples first.

## Check Phase 2 Readiness

```bash
scripts/phase2-exit-readiness.sh
```

This command reads the Phase 2 exit ledger and prints how many gates are done,
partial, pending, or blocked. It does not declare Phase 2 complete. It gives a
repeatable status snapshot.

## Run Evidence Helpers

For a local evidence packet:

```bash
scripts/record-phase2-exit-bundle.sh --strict
scripts/check-phase2-exit-evidence.sh
scripts/phase2-exit-readiness.sh
```

For sample output evidence:

```bash
scripts/record-phase2-sample-evidence.sh
```

For permission enforcement evidence:

```bash
scripts/record-phase2-ucap-evidence.sh --strict
```

## Historical Phase 1 Proof

The original hello-world proof still exists and is useful for understanding the
runtime base:

```bash
scripts/build-hello-component.sh
target/debug/layer36 run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Expected output:

```text
Hello, Layer36!
```

For new app work, use the Phase 2 UAPI path instead:

- [Your First UAPI App In Rust](uapi/first-rust-cli.md)
- [Migrating From Phase 1 To Phase 2](phase2/migrating-from-phase1.md)
