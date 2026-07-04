# Migrating From Phase 1 To Phase 2

Phase 1 proved that Krate could load one WebAssembly component and call a tiny
host interface. Phase 2 starts the real app model.

This page is for anyone who tried the Phase 1 `hello-world` component and now
wants to understand what changed.

## The Short Version

Phase 1 was a runtime proof.

Phase 2 is the first app platform slice.

| Area | Phase 1 | Phase 2 |
|---|---|---|
| App shape | one proof component | CLI component world |
| Host API | `krate:phase1/host.print` and `exit` | `io`, `fs`, `net`, `time`, `locale` |
| Permissions | none | manifest capabilities plus run-session grants |
| App metadata | none | sidecar `manifest.toml` |
| Samples | `hello-world` | `krate-clock`, `krate-cat`, `krate-curl` |
| SDK | direct generated bindings | first Rust SDK facade |

## What Replaces `print`

Phase 1:

```rust
bindings::krate::phase1::host::print("Hello, Krate!");
```

Phase 2:

```rust
use krate::io::{stdio, streams::OutputStreamExt};

let out = stdio::stdout();
out.write_line("Hello, Krate")?;
```

The Phase 2 version is slightly longer because it is real I/O. `stdout` is a
host resource, and writing can fail.

## What Replaces `exit`

Phase 1 used a host import:

```rust
bindings::krate::phase1::host::exit(0);
```

Phase 2 apps return an integer from `run`:

```rust
impl krate::Guest for Component {
    fn run() -> i32 {
        0
    }
}
```

The runtime maps that return value to the process exit code.

## What Replaces "No Permissions"

Phase 1 had no file or network access, so it did not need permissions.

Phase 2 apps can ask for useful host access, so they must declare it:

```toml
[app]
id = "dev.krate.cat"
name = "krate-cat"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/krate_cat.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "io.stdout"
rationale = "Print file contents"
required = true

[[capabilities]]
cap = "fs.read:./fixtures/**"
rationale = "Read test fixture files"
required = true
```

You can generate this shape instead of writing it by hand:

```bash
cargo run -p krate-cli -- manifest init \
  --id dev.krate.cat \
  --name krate-cat \
  --entry target/wasm32-wasip1/release/krate_cat.wasm \
  --cap io.stdout \
  --cap 'fs.read:./fixtures/**'
```

And you can inspect what grants it will need:

```bash
cargo run -p krate-cli -- manifest explain apps/krate-cat/manifest.toml
```

For CI checks or editor tooling, use the JSON form:

```bash
cargo run -p krate-cli -- manifest check \
  --format json \
  apps/krate-cat/manifest.toml

cargo run -p krate-cli -- manifest explain \
  --format json \
  apps/krate-cat/manifest.toml
```

If you need a local audit trail while testing, run with `--log-grants`:

```bash
cargo run -p krate-cli -- run \
  --manifest apps/krate-cat/manifest.toml \
  --auto-grant \
  --log-grants krate-grants.log \
  apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
```

## What Replaces The Phase 1 World

Phase 1 component metadata pointed at:

```toml
[package.metadata.component.target]
path = "../../../wit/krate/phase1.wit"
world = "app"
```

Phase 2 samples point at the CLI world:

```toml
[package.metadata.component.target]
path = "../../wit/krate/phase2"
world = "cli"
```

That `cli` world imports Krate UAPI modules and exports `run`.

## What To Change In A Rust App

1. Replace Phase 1 generated host calls with the Rust SDK.
2. Return an exit code from `Guest::run`.
3. Add `manifest.toml`.
4. Declare every non-default capability.
5. Run with explicit grants, `--prompt`, or `--auto-grant`.

For a full walkthrough, read [Your First UAPI App In Rust](../uapi/first-rust-cli.md).

## Old Command, New Command

Phase 1:

```bash
cargo run -p krate-cli -- run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Phase 2:

```bash
cargo run -p krate-cli -- run \
  --manifest apps/krate-cat/manifest.toml \
  --auto-grant \
  apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
```

The `--` separates Krate runner arguments from app arguments.

## Keep In Mind

- Phase 1 is still supported so the original proof keeps working.
- Phase 2 is the path for new work.
- Phase 2 manifests are not signed yet.
- Grants are session-only.
- The UAPI is not frozen as a compatibility promise yet.

In simple words: Phase 1 proved the engine starts. Phase 2 teaches it useful
app behavior.
