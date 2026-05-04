# layer36

Rust guest SDK for Layer36 UAPI components.

Layer36 apps are WebAssembly components that call Layer36 APIs instead of
talking directly to one operating system. This crate gives Rust apps a small,
stable front door for the current Phase 2 UAPI draft:

- `layer36::io` for args, stdout, stderr, stdin, and logs
- `layer36::fs` for granted file access
- `layer36::net` for granted HTTP client access
- `layer36::time` for clock and sleep calls
- `layer36::locale` for locale, timezone, and formatting calls

## Minimal app

```rust,ignore
use layer36::{io::stdio, Guest};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        if stdio::println("Hello from Layer36").is_err() {
            return 20;
        }

        0
    }
}

layer36::export!(Component);
```

## Common helpers

```rust,ignore
let args = layer36::io::args::all();
let text = layer36::fs::read_to_string("input.txt")?;
let body = layer36::net::get_text("http://127.0.0.1:8080/data.txt")?;
let now = layer36::time::now_millis();
let locale = layer36::locale::current();
```

## Status

This crate is still pre-release. It is useful for the Rust sample apps in this
repository, but UAPI v0.1 is not frozen yet and the crate is not published to
crates.io yet.

The SDK does not bypass Layer36 permissions. File and network access still go
through the runtime's UCap checks.
