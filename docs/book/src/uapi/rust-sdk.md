# Rust SDK

The Rust SDK lives in `crates/bindings-rust` and builds as the package
`krate`.

It is still small. That is intentional. Phase 2 is where the UAPI shape is
settling, so the SDK gives Rust apps a clean front door without hiding the
actual platform contract.

The crate now has publish-facing metadata and a crate README. We still do not
publish it to crates.io while UAPI v0.1 is moving, but `cargo package -p
krate` is part of CI so packaging problems show up early. CI also runs
`scripts/smoke-rust-sdk.sh`, which creates a temporary app outside the
workspace and checks that the packaged SDK can compile a tiny Krate component.
The smoke path now has a matching Phase 2 evidence recorder:
[Rust SDK Evidence](../phase2/rust-sdk-evidence.md). Normal hosted CI uploads
that report as the `rust-sdk-evidence` artifact.

## What It Gives You

Instead of importing generated WIT paths directly, app code can use short
Krate modules:

```rust
use krate::{
    fs::{self, OpenMode},
    io::{args, stdio, streams::OutputStreamExt},
    net, time,
    Guest,
};
```

The current modules are:

| Module | Use it for |
|---|---|
| `krate::io` | app arguments, stdin, stdout, stderr, logging |
| `krate::fs` | opening, reading, writing, and checking files |
| `krate::net` | HTTP client calls |
| `krate::time` | wall clock, monotonic clock, sleep |
| `krate::locale` | locale, timezone, date and number formatting |

## A Minimal App

Every Phase 2 CLI app exports a `run` function through the `Guest` trait:

```rust
use krate::{io::stdio, io::streams::OutputStreamExt, Guest};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        if out.write_line("Hello from Krate").is_err() {
            return 20;
        }
        0
    }
}

krate::export!(Component);
```

That `krate::export!` line is the bridge between normal Rust code and the
WASM component export that the runtime calls.

## Reading Arguments

Krate app arguments are passed after `--`:

```bash
krate run app.wasm -- notes.txt
```

Inside the component:

```rust
let raw = krate::io::args::raw();
let first = krate::io::args::first_raw(&raw);
```

For normal apps, use the owned helpers:

```rust
let args = krate::io::args::all();
let first = krate::io::args::first();
```

The current draft stores arguments as newline separated text under the hood.
Use `raw` when you need exact control, `split_raw` when you already have the raw
string, and `all` or `first` for app code.

The small argument helpers are marked inline on purpose. That keeps guest
components from pulling in WASI environment imports. Krate apps should depend
on Krate UAPI, not host WASI argument APIs.

## Writing Output

The generated stream type already has `write_all`. The SDK adds text helpers:

```rust
use krate::io::{stdio, streams::OutputStreamExt};

let out = stdio::stdout();
out.write_text("status: ")?;
out.write_line("ok")?;
out.flush()?;
```

The sample apps use this pattern for stdout and stderr.

## Files

For low level control:

```rust
use krate::fs::{self, OpenMode};

let file = fs::open("input.txt", OpenMode::Read)?;
let bytes = file.read(8192)?;
```

For common cases:

```rust
let text = krate::fs::read_to_string("input.txt")?;
krate::fs::write("out.txt", text.as_bytes())?;
```

The runtime still checks capabilities before file access. The SDK does not
skip UCap.

## Network

The first network helper is deliberately narrow:

```rust
let body = krate::net::get("http://127.0.0.1:8080/data.txt")?;
```

The runtime checks a `net.connect:HOST:PORT` grant before opening the socket.
For lower-level work, `krate::net::fetch(req)` sends the request method, app
headers, and a buffered body, then returns status, headers, and body. The host
still owns transport headers such as `Host`, `Connection`, and
`Content-Length`. The current plain HTTP adapter caps the full response at 1
MiB by default, with `krate run --max-http-response-bytes` available for test
runs that need a different limit. Helper `krate::net::get` calls use a default
timeout of 5000 ms, configurable with `krate run --http-timeout-millis` (`0`
disables the default timeout). HTTPS, redirects, and streaming bodies are still
Phase 2 work.

## Current Limits

This is not a finished SDK yet.

- It is package-checked, but not published to crates.io.
- A fresh outside-workspace smoke app now checks the packaged crate.
- Its public helper layer now has rustdoc comments and a local doc build check.
- It wraps the generated Phase 2 guest bindings, which are still draft.
- It has enough helpers for the Rust samples, not a full developer experience.
- Go and TypeScript SDK work is still pending.

That is fine for now. The important part is that app code now points at
`krate::fs`, `krate::net`, and friends. That is the layer we can keep
improving without making app authors learn the generated binding layout.

For the end-to-end app flow, read [Your First UAPI App In Rust](first-rust-cli.md).
