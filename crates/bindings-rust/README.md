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
