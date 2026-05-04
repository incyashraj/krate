# Rust SDK

The Rust SDK lives in `crates/bindings-rust` and builds as the package
`layer36`.

It is still small. That is intentional. Phase 2 is where the UAPI shape is
settling, so the SDK gives Rust apps a clean front door without hiding the
actual platform contract.

The crate now has publish-facing metadata and a crate README. We still do not
publish it to crates.io while UAPI v0.1 is moving, but `cargo package -p
layer36` is part of CI so packaging problems show up early.

## What It Gives You

Instead of importing generated WIT paths directly, app code can use short
Layer36 modules:

```rust
use layer36::{
    fs::{self, OpenMode},
    io::{args, stdio, streams::OutputStreamExt},
    net, time,
    Guest,
};
```

The current modules are:

| Module | Use it for |
|---|---|
| `layer36::io` | app arguments, stdin, stdout, stderr, logging |
| `layer36::fs` | opening, reading, writing, and checking files |
| `layer36::net` | HTTP client calls |
| `layer36::time` | wall clock, monotonic clock, sleep |
| `layer36::locale` | locale, timezone, date and number formatting |

## A Minimal App

Every Phase 2 CLI app exports a `run` function through the `Guest` trait:

```rust
use layer36::{io::stdio, io::streams::OutputStreamExt, Guest};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        if out.write_line("Hello from Layer36").is_err() {
            return 20;
        }
        0
    }
}

layer36::export!(Component);
```

That `layer36::export!` line is the bridge between normal Rust code and the
WASM component export that the runtime calls.

## Reading Arguments

Layer36 app arguments are passed after `--`:

```bash
layer36 run app.wasm -- notes.txt
```

Inside the component:

```rust
let raw = layer36::io::args::raw();
let first = layer36::io::args::first_raw(&raw);
```

The current draft stores arguments as newline separated text. Use `raw` when
you need exact control, and use `first_raw` or `split_raw` for simple examples.

The small argument helpers are marked inline on purpose. That keeps guest
components from pulling in WASI environment imports. Layer36 apps should depend
on Layer36 UAPI, not host WASI argument APIs.

## Writing Output

The generated stream type already has `write_all`. The SDK adds text helpers:

```rust
use layer36::io::{stdio, streams::OutputStreamExt};

let out = stdio::stdout();
out.write_text("status: ")?;
out.write_line("ok")?;
out.flush()?;
```

The sample apps use this pattern for stdout and stderr.

## Files

For low level control:

```rust
use layer36::fs::{self, OpenMode};

let file = fs::open("input.txt", OpenMode::Read)?;
let bytes = file.read(8192)?;
```

For common cases:

```rust
let text = layer36::fs::read_to_string("input.txt")?;
layer36::fs::write("out.txt", text.as_bytes())?;
```

The runtime still checks capabilities before file access. The SDK does not
skip UCap.

## Network

The first network helper is deliberately narrow:

```rust
let body = layer36::net::get("http://127.0.0.1:8080/data.txt")?;
```

The runtime checks a `net.connect:HOST:PORT` grant before opening the socket.
HTTPS, redirects, streaming bodies, and response limits are still Phase 2 work.

## Current Limits

This is not a finished SDK yet.

- It is package-checked, but not published to crates.io.
- It wraps the generated Phase 2 guest bindings, which are still draft.
- It has enough helpers for the Rust samples, not a full developer experience.
- Go and TypeScript SDK work is still pending.

That is fine for now. The important part is that app code now points at
`layer36::fs`, `layer36::net`, and friends. That is the layer we can keep
improving without making app authors learn the generated binding layout.
