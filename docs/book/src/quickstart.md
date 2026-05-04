# Quickstart: Run Your First Layer36 Component

This walkthrough builds the Phase 1 hello-world WebAssembly component and runs
it through the `layer36` CLI. At the end your terminal should print:

```text
Hello, Layer36!
```

## Prerequisites

Install:

- Git
- Rust via `rustup`

Layer36 pins its Rust toolchain in `rust-toolchain.toml`, so entering the repo
lets `rustup` install the right compiler and WASM targets.

## Get the Source

```bash
git clone https://github.com/incyashraj/layer6x6.git
cd layer6x6
```

## Install Component Tooling

```bash
cargo install cargo-component --locked --version 0.21.1
```

## Build Layer36

```bash
cargo build --workspace
```

## Build the Hello Component

```bash
scripts/build-hello-component.sh
```

The script prints the generated component path:

```text
test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

`cargo-component` currently writes this adapted component under
`wasm32-wasip1`; this is expected for the Phase 1 toolchain.

## Run It

```bash
cargo run -p layer36-cli -- run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Expected output:

```text
Hello, Layer36!
```

## Try the Limit Checks

Fuel and memory limits are set very low here so you can see the failure path:

```bash
cargo run -p layer36-cli -- run --fuel 1 test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
cargo run -p layer36-cli -- run --mem-limit 0 test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Both commands exit with code `4` and print a `limit exceeded` message.

## Run the Phase 1 Test Harness

```bash
scripts/test-phase1.sh
```

This builds the hello-world component and runs the workspace tests with the
fixture path wired into `LAYER36_HELLO_WASM`.

## What You Just Ran

The hello component uses this Phase 1 WIT interface:

```wit
package layer36:phase1@0.0.1;

interface host {
  print: func(msg: string);
  exit: func(code: s32);
}

world app {
  import host;
  export run: func();
}
```

The component source is tiny by design:

```rust
#[allow(warnings)]
mod bindings;

use bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        bindings::layer36::phase1::host::print("Hello, Layer36!");
        bindings::layer36::phase1::host::exit(0);
    }
}

bindings::export!(Component with_types_in bindings);
```

Its `Cargo.toml` points `cargo-component` at the Layer36 WIT world:

```toml
[package]
name = "hello-world"
version = "0.1.0-dev"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/incyashraj/layer6x6"
rust-version = "1.91"

[dependencies]
wit-bindgen-rt = { version = "0.44.0", features = ["bitflags"] }

[lib]
crate-type = ["cdylib"]

[package.metadata.component]
package = "layer36:hello-world"

[package.metadata.component.target]
path = "../../../wit/layer36/phase1.wit"
world = "app"
```

Layer36 loads that WebAssembly component, registers the temporary
`layer36:phase1/host` imports, calls the exported `run` function, and routes the
component's `print` call to your terminal.

## Verify Your Environment

```bash
cargo run -p layer36-cli -- doctor
```

`doctor` reports your `cargo-component` version, `wasm32-wasip1` and
`wasm32-wasip2` target status, and Layer36 state directory. It is basic in
Phase 1 because the runtime is still a proof.

## Check a Phase 2 Manifest

Phase 2 adds a sidecar `manifest.toml` for app identity and capability
requests. You can validate one before the app runtime path is complete:

```bash
cargo run -p layer36-cli -- manifest check path/to/manifest.toml
```

For now this checks the TOML schema, app world, app id, duplicate capabilities,
and capability string format.

To understand what a manifest will ask for:

```bash
cargo run -p layer36-cli -- manifest explain path/to/manifest.toml
```

You can also generate a starter manifest:

```bash
cargo run -p layer36-cli -- manifest init \
  --id com.example.hello \
  --name Hello \
  --entry hello.wasm \
  --cap io.stdout \
  --output manifest.toml
```

To see the Phase 2 capability strings accepted by this runtime:

```bash
cargo run -p layer36-cli -- manifest capabilities
```

When a manifest sits next to an app component, `layer36 run` also checks
required capabilities before the component starts:

```bash
cargo run -p layer36-cli -- run path/to/app.wasm --grant fs.read:./data/**
cargo run -p layer36-cli -- run path/to/app.wasm --auto-grant
```

This is session-only. The runtime also checks granted file and network
capabilities at UAPI call sites. Persistent permission storage comes later.

For the Phase 2 Rust app flow, see
[Your First UAPI App In Rust](uapi/first-rust-cli.md).
