# UAPI Reference

> Generated from `wit/layer36/phase2`. Do not edit this page by hand.

Layer36 Phase 2 exposes the `cli` world from `layer36:app@0.1.0`.

The current world imports these interfaces:

- `layer36:io/types@0.1.0`
- `layer36:io/streams@0.1.0`
- `layer36:io/stdio@0.1.0`
- `layer36:io/args@0.1.0`
- `layer36:io/log@0.1.0`
- `layer36:fs/types@0.1.0`
- `layer36:fs/files@0.1.0`
- `layer36:net/types@0.1.0`
- `layer36:net/http-client@0.1.0`
- `layer36:time/clock@0.1.0`
- `layer36:time/sleep@0.1.0`
- `layer36:locale/types@0.1.0`
- `layer36:locale/info@0.1.0`
- `layer36:locale/format@0.1.0`

The app exports:

- `run() -> s32`

## `layer36:fs/files@0.1.0`

Filesystem entry points. All host file access should pass through these functions and resource methods.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `fs.read:<path-glob>` - manifest or session grant
- `fs.write:<path-glob>` - manifest or session grant
- `fs.list:<path-glob>` - manifest or session grant
- `fs.remove:<path-glob>` - manifest or session grant
- `fs.mkdir:<path-glob>` - manifest or session grant

- `open`, `stat`, and `list` require a matching `fs.read:PATH` grant for read-style access.
- Write, mkdir, remove, and rename operations are part of the Phase 2 shape, but the first runtime slice focuses on read grants.

### Rust SDK Example

```rust
let text = layer36::fs::read_to_string("notes.txt")?;
layer36::io::stdio::println(&text)?;
```

### Functions

- `open(path: string, mode: open-mode) -> result<own<file>, fs-error>`
- `stat(path: string) -> result<file-stat, fs-error>`
- `list(path: string) -> result<list<string>, fs-error>`
- `remove-file(path: string) -> result<_, fs-error>`
- `remove-dir(path: string) -> result<_, fs-error>`
- `mkdir(path: string) -> result<_, fs-error>`
- `rename(from: string, to: string) -> result<_, fs-error>`

### Types

#### `file` resource

#### `file` methods

- `read(n: u32) -> result<list<u8>, fs-error>`
- `write(bytes: list<u8>) -> result<u32, fs-error>`
- `seek-set(pos: u64) -> result<u64, fs-error>`
- `seek-end() -> result<u64, fs-error>`
- `stat() -> result<file-stat, fs-error>`


## `layer36:fs/types@0.1.0`

Shared filesystem records, modes, and error shapes.

### Types

#### `file-stat` record

- `size`: `u64`
- `modified-millis`: `u64`
- `is-dir`: `bool`

#### `open-mode` variant

- `read`
- `write`
- `read-write`
- `append`

#### `fs-error` variant

- `not-found`
- `permission-denied`
- `already-exists`
- `invalid-path`
- `not-a-directory`
- `is-a-directory`
- `io`: `string`


## `layer36:io/args@0.1.0`

Raw Layer36 app arguments. These are the arguments passed after `--` in `layer36 run`.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `io.stdin` - default grant
- `io.stdout` - default grant
- `io.stderr` - default grant
- `io.args` - default grant
- `io.log` - default grant

- `io.args` is granted by default for CLI apps.
- The current draft encodes args as newline-separated text.

### Rust SDK Example

```rust
let raw = layer36::io::args::raw();
let first = layer36::io::args::first_raw(&raw);
```

### Functions

- `raw() -> string`


## `layer36:io/log@0.1.0`

Structured app logs. Hosts can route these to native logs, developer consoles, or test captures.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `io.stdin` - default grant
- `io.stdout` - default grant
- `io.stderr` - default grant
- `io.args` - default grant
- `io.log` - default grant

- `io.log` is a low-risk default grant.

### Functions

- `emit(level: log-level, message: string, fields: list<field>)`

### Types

#### `field` record

- `key`: `string`
- `value`: `string`


## `layer36:io/stdio@0.1.0`

Standard input, output, and error streams for CLI-style apps.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `io.stdin` - default grant
- `io.stdout` - default grant
- `io.stderr` - default grant
- `io.args` - default grant
- `io.log` - default grant

- `io.stdin`, `io.stdout`, and `io.stderr` are low-risk default grants for CLI apps.

### Rust SDK Example

```rust
layer36::io::stdio::println("Hello from Layer36")?;
layer36::io::stdio::eprintln("debug line")?;
```

### Functions

- `stdin() -> own<input-stream>`
- `stdout() -> own<output-stream>`
- `stderr() -> own<output-stream>`


## `layer36:io/streams@0.1.0`

Byte streams used by stdio and other UAPI modules.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `io.stdin` - default grant
- `io.stdout` - default grant
- `io.stderr` - default grant
- `io.args` - default grant
- `io.log` - default grant

- `io.stdin`, `io.stdout`, and `io.stderr` are low-risk default grants for CLI apps.

### Rust SDK Example

```rust
use layer36::io::streams::OutputStreamExt;

let out = layer36::io::stdio::stdout();
out.write_line("ok")?;
out.flush()?;
```

### Types

#### `input-stream` resource

#### `output-stream` resource

#### `input-stream` methods

- `read(n: u32) -> result<list<u8>, io-error>`
- `read-to-string() -> result<string, io-error>`

#### `output-stream` methods

- `write(bytes: list<u8>) -> result<u32, io-error>`
- `write-all(bytes: list<u8>) -> result<_, io-error>`
- `flush() -> result<_, io-error>`


## `layer36:io/types@0.1.0`

Shared IO log and error types.

### Types

#### `log-level` enum

- `trace`
- `debug`
- `info`
- `warn`
- `error`

#### `io-error` variant

- `closed`
- `interrupted`
- `unexpected-eof`
- `invalid-utf8`
- `other`: `string`


## `layer36:locale/format@0.1.0`

Host-backed date and number formatting.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `locale.info` - default grant
- `locale.format` - default grant

- Locale reads and formatting are default grants for CLI apps.

### Rust SDK Example

```rust
let locale = layer36::locale::current();
let text = layer36::locale::format_number(42.0, layer36::locale::NumberStyle::Decimal, &locale);
```

### Functions

- `format-date(millis: u64, tz: string, style: date-style, loc: locale-id) -> string`
- `format-number(value: f64, style: number-style, loc: locale-id) -> string`


## `layer36:locale/info@0.1.0`

The host user's current locale and timezone.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `locale.info` - default grant
- `locale.format` - default grant

- Locale reads and formatting are default grants for CLI apps.

### Rust SDK Example

```rust
let locale = layer36::locale::current();
let timezone = layer36::locale::timezone();
```

### Functions

> The user's preferred locale as reported by the host.

- `current() -> locale-id`
> IANA timezone name, for example "Asia/Singapore".

- `timezone() -> string`


## `layer36:locale/types@0.1.0`

Locale and formatting type definitions.

### Types

#### `locale-id` record

- `bcp47`: `string`

#### `date-style` enum

- `short`
- `medium`
- `long`
- `full`

#### `number-style` enum

- `decimal`
- `percent`
- `currency`


## `layer36:net/http-client@0.1.0`

HTTP client calls. Phase 2 starts with simple request and response bodies.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `net.connect:<host>:<port>` - manifest or session grant

- `get` and `fetch` require a matching `net.connect:HOST:PORT` grant before the adapter opens a socket.
- The current host adapter supports the plain HTTP test path first with a 1 MiB full-response cap; HTTPS and richer network behavior are still Phase 2 work.

### Rust SDK Example

```rust
let body = layer36::net::get_text("http://127.0.0.1:8080/data.txt")?;
layer36::io::stdio::println(&body)?;
```

### Functions

- `get(url: string) -> result<list<u8>, net-error>`
- `fetch(req: request) -> result<response, net-error>`


## `layer36:net/types@0.1.0`

Shared network request, response, and error types.

### Types

#### `http-method` enum

- `get`
- `post`
- `put`
- `delete`
- `patch`
- `head`
- `options`

#### `header` record

- `name`: `string`
- `value`: `string`

#### `request` record

- `method`: `http-method`
- `url`: `string`
- `headers`: `list<header>`
- `body`: `list<u8>`
- `timeout-millis`: `option<u32>`

#### `response` record

- `status`: `u16`
- `headers`: `list<header>`
- `body`: `list<u8>`

#### `net-error` variant

- `invalid-url`
- `dns-failure`: `string`
- `connect-failure`: `string`
- `tls-failure`: `string`
- `timeout`
- `body-too-large`
- `permission-denied`
- `protocol`: `string`
- `other`: `string`


## `layer36:time/clock@0.1.0`

Wall-clock and monotonic clock reads.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `time.clock` - default grant
- `time.monotonic` - default grant
- `time.sleep` - default grant

- `time.clock` and `time.monotonic` are default grants.

### Rust SDK Example

```rust
let now = layer36::time::now_millis();
let tick = layer36::time::monotonic_nanos();
```

### Functions

> Milliseconds since Unix epoch. Wall-clock; can jump.

- `now-millis() -> u64`
> Monotonic nanoseconds since an arbitrary origin.
> Guaranteed non-decreasing; suitable for measuring intervals.

- `monotonic-nanos() -> u64`


## `layer36:time/sleep@0.1.0`

Blocking sleep for CLI-style components.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `time.clock` - default grant
- `time.monotonic` - default grant
- `time.sleep` - default grant

- `sleep-millis` requires `time.sleep`.

### Rust SDK Example

```rust
layer36::time::sleep_millis(100);
```

### Functions

> Block the calling task for at least `millis` milliseconds.

- `sleep-millis(millis: u32)`

