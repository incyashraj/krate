# krate

Rust guest SDK for Krate UAPI components.

Krate apps are WebAssembly components that call Krate APIs instead of
talking directly to one operating system. This crate gives Rust apps a small,
stable front door for the current Phase 2 UAPI draft:

- `krate::io` for args, stdout, stderr, stdin, and logs
- `krate::fs` for granted file access
- `krate::net` for granted HTTP client access
- `krate::time` for clock and sleep calls
- `krate::locale` for locale, timezone, and formatting calls

## Minimal app

```rust,ignore
use krate::{io::stdio, Guest};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        if stdio::println("Hello from Krate").is_err() {
            return 20;
        }

        0
    }
}

krate::export!(Component);
```

## Common helpers

```rust,ignore
let args = krate::io::args::all();
let text = krate::fs::read_to_string("input.txt")?;
let body = krate::net::get_text("http://127.0.0.1:8080/data.txt")?;
let response = krate::net::fetch(krate::net::Request {
    method: krate::net::HttpMethod::Post,
    url: "http://127.0.0.1:8080/submit".to_string(),
    headers: Vec::new(),
    body: b"hello".to_vec(),
    timeout_millis: Some(1000),
})?;
let now = krate::time::now_millis();
let locale = krate::locale::current();
```

## Status

This crate is still pre-release. It is useful for the Rust sample apps in this
repository, but UAPI v0.1 is not frozen yet and the crate is not published to
crates.io yet.

The SDK does not bypass Krate permissions. File and network access still go
through the runtime's UCap checks.

## A windowed app: the `gui` feature

The same crate carries both of Krate's worlds. A CLI app depends on it plain
and gets the `cli` world; a windowed app turns on the `gui` feature and gets
the `gui` world -- every Phase 2 helper unchanged, plus `krate::ui`,
`krate::gfx`, `krate::audio`, `krate::camera` and `krate::speech`, one thin
layer per interface with the exact WIT signatures and errors:

```toml
[dependencies]
krate = { path = "<sdk>/crates/bindings-rust", features = ["gui"] }
```

```rust,ignore
use krate::ui::{events, tree, types, window};

let win = window::create("Hello", types::WindowSize { width: 480, height: 320 })?;
tree::set_root(win, &root)?;
while let Some(event) = events::wait(None) { /* ... */ }
```

The feature decides which world the component declares, so a windowed app
must have it and a CLI app must not. The raw bindings for either world stay
reachable as `krate::bindings::krate::<package>::...` when a shape the
helpers do not cover is needed.
