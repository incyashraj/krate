# Quickstart: Run A Phase 2 Krate Component

This walkthrough builds the Krate CLI, builds the current Phase 2 sample
components, and runs a file-reading WebAssembly component through the Krate
UAPI and capability path.

At the end your terminal should print:

```text
hello from Krate
```

Krate is still pre-alpha. This quickstart is for local developer proof, not
for running untrusted third-party components.

## Prerequisites

Install:

- Git
- Rust via `rustup`
- `cargo-component`

Krate pins its Rust toolchain in `rust-toolchain.toml`, so entering the repo
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

## Build Krate

```bash
cargo build -p krate-cli
```

Check the local environment:

```bash
target/debug/krate doctor
```

`doctor` reports core Rust tools first, then Phase 2 language tools such as
`wasm-tools`, `tinygo`, `go`, `node`, `npm`, and `jco` when they are available.

## Build The Phase 2 Samples

```bash
scripts/build-krate-clock-component.sh
scripts/build-krate-cat-component.sh
scripts/build-krate-curl-component.sh
```

The scripts print component paths like:

```text
apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm
```

## Inspect A Manifest

Phase 2 apps carry a `manifest.toml` for app identity and capability requests.
Before running the file sample, inspect what it asks for:

```bash
target/debug/krate manifest explain apps/krate-cat/manifest.toml
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
target/debug/krate run \
  --auto-grant \
  --manifest apps/krate-clock/manifest.toml \
  --test-time 1234567890 \
  --test-locale en-US \
  --test-timezone UTC \
  apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm
```

This exercises time, locale, timezone, and stdout. The test flags make output
stable enough for evidence runs.

## Run The File Sample

Create a test file:

```bash
mkdir -p apps/krate-cat/fixtures
printf 'hello from Krate\n' > apps/krate-cat/fixtures/hello.txt
```

Run with the sample manifest and grant approval:

```bash
cd apps/krate-cat
../../target/debug/krate run \
  --manifest manifest.toml \
  --auto-grant \
  target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

Expected output:

```text
hello from Krate
```

## See The Denial Path

Run the same app without granting the file capability:

```bash
cd apps/krate-cat
printf '' | ../../target/debug/krate run \
  --manifest manifest.toml \
  target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

In a non-interactive shell, Krate exits before starting the component and
prints the missing required capability. That is intentional: host file access
should be explicit.

## Optional Network Sample

Start a local HTTP server in one terminal:

```bash
mkdir -p /tmp/krate-demo-http
printf 'portable runtime response\n' > /tmp/krate-demo-http/demo.txt
cd /tmp/krate-demo-http
python3 -m http.server 8765
```

In another terminal, run the curl sample with an explicit network grant:

```bash
cd /path/to/layer6x6
target/debug/krate run \
  --grant net.connect:127.0.0.1:8765 \
  apps/krate-curl/target/wasm32-wasip1/release/krate_curl.wasm \
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
target/debug/krate run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Expected output:

```text
Hello, Krate!
```

For new app work, use the Phase 2 UAPI path instead:

- [Your First UAPI App In Rust](uapi/first-rust-cli.md)
- [Migrating From Phase 1 To Phase 2](phase2/migrating-from-phase1.md)

## See It With a Window

The GUI vertical slice has a one-command demo. On macOS it opens a real
native window (click the button within 30 seconds and watch the text field
change); on Linux and Windows the same portable file runs headless — and the
full CI matrix proves the identical bytes open real windows there too.

```bash
sh scripts/demo-hello-gui.sh
```

Exit codes are the assertions: `0` = native click observed, `1` = clean
bounded run without a click, `2` = window closed early. The full test
manual, including manual commands and troubleshooting, is on the
[Hello GUI Demo & Testing](phase3/hello-gui-demo.md) page.

## Machine-Readable Runs

Add `--json` to any run to get one `krate.run.v1` object describing it —
app identity, granted capabilities with boundaries, denials, exit class,
duration, and captured output. This is the same report AI agents receive
through `krate-mcp-server`; see
[Embedding & JSON Runs](phase3/embedding.md).
