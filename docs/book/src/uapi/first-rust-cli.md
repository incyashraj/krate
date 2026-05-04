# Your First UAPI App In Rust

This walkthrough is the shortest current path from "I know Rust" to "I ran a
Layer36 Phase 2 app." It uses the real Rust SDK, the real manifest commands, and
the current `layer36 run` path.

Phase 2 is still pre-alpha, so this guide uses the repo workspace directly. The
future version will start with `cargo add layer36`.

## What You Build

A tiny CLI app that:

- reads one file path from app arguments
- reads that file through Layer36 `fs`
- writes the text through Layer36 `stdout`
- declares its permissions in `manifest.toml`

That is enough to touch the core Phase 2 loop:

```text
Rust app -> Layer36 SDK -> UAPI import -> UCap check -> host adapter
```

## 1. Check Your Tools

From the repo root:

```bash
cargo run -p layer36-cli -- doctor
```

For this walkthrough, you need:

- Rust installed
- `wasm32-wasip1` installed
- `cargo-component` installed

If `cargo-component` is missing:

```bash
cargo install cargo-component --locked --version 0.21.1
```

## 2. Start From The Cat Sample

The current Rust SDK is local, so the easiest first app is the existing sample:

```bash
apps/layer36-cat/
```

The core code is intentionally plain:

```rust
use layer36::{
    fs,
    io::{args, stdio, streams::OutputStreamExt},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let Some(path) = args::first() else {
            let _ = stdio::stderr().write_line("usage: layer36-cat <file>");
            return 64;
        };

        match fs::read_to_string(&path) {
            Ok(text) => {
                let _ = stdio::stdout().write_text(&text);
                0
            }
            Err(err) => {
                let _ = stdio::stderr().write_line(&format!("{err:?}"));
                5
            }
        }
    }
}

layer36::export!(Component);
```

The important part is what it does not do. It does not call `std::fs`, `std::env`,
or direct WASI imports. It asks Layer36 for arguments, file access, stdout, and
stderr.

## 3. Build The Component

From the repo root:

```bash
cargo build -p layer36-cli
scripts/build-layer36-cat-component.sh
```

The script prints a component path like:

```text
apps/layer36-cat/target/wasm32-wasip1/release/layer36_cat.wasm
```

## 4. Generate A Manifest

Use the CLI to create a starter manifest:

```bash
cargo run -p layer36-cli -- manifest init \
  --id dev.layer36.cat \
  --name layer36-cat \
  --entry target/wasm32-wasip1/release/layer36_cat.wasm \
  --cap io.args \
  --cap io.stdout \
  --cap io.stderr \
  --cap 'fs.read:./fixtures/**' \
  --output apps/layer36-cat/manifest.toml \
  --force
```

This writes valid TOML and checks every capability string before it writes.

## 5. Explain The Manifest

Before running the app, inspect what it asks for:

```bash
cargo run -p layer36-cli -- manifest explain apps/layer36-cat/manifest.toml
```

You should see that `io.args`, `io.stdout`, and `io.stderr` are default grants.
You should also see that `fs.read:./fixtures/**` needs a launch grant.

If you want the same explanation as structured data, use JSON:

```bash
cargo run -p layer36-cli -- manifest explain \
  --format json \
  apps/layer36-cat/manifest.toml
```

The check and capability table commands can print JSON too, which is useful
when you start wiring your own CI scripts.

That is the Phase 2 permission model in simple form:

```text
low-risk app plumbing -> default grant
host file/network access -> explicit launch grant
```

If you want a local record of the grants used for a run, add:

```bash
--log-grants layer36-grants.log
```

For scripts, use JSON Lines. Layer36 writes one audit record per line:

```bash
--log-grants layer36-grants.jsonl --log-grants-format jsonl
```

If you want a script-readable preview before the component starts, use:

```bash
--dump-caps --dump-caps-format json
```

## 6. Run It

Create a test file:

```bash
mkdir -p apps/layer36-cat/fixtures
printf 'hello from Layer36\n' > apps/layer36-cat/fixtures/hello.txt
```

Run with the sample manifest:

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

## 7. See The Denial Path

Run without granting the file capability:

```bash
cd apps/layer36-cat
printf '' | ../../target/debug/layer36 run \
  --manifest manifest.toml \
  target/wasm32-wasip1/release/layer36_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

In a non-interactive shell, Layer36 exits before starting the component and
prints the missing required capability.

That is the behavior we want. The runtime should refuse the app before native
file access happens.

## 8. What To Change For Your Own App

Start by changing:

- `apps/layer36-cat/src/lib.rs`
- `apps/layer36-cat/Cargo.toml`
- `apps/layer36-cat/manifest.toml`

Then rebuild and rerun. As the SDK gets published, this flow will move from a
repo-local sample to a fresh project outside the Layer36 tree.

## Current Limits

- The SDK is local to the repo and still draft.
- The manifest is unsigned.
- Grants are session-only.
- Cross-host sample proof is still running through CI, not through a released
  installer.

Even with those limits, this is now the core shape of a Layer36 CLI app.
