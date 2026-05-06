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

> Open a path and return a file resource.

- `open(path: string, mode: open-mode) -> result<own<file>, fs-error>`
  - Opens a host file through Layer36 and returns a `file` handle.
  - `read` needs `fs.read:PATH`; `write`, `append`, and `read-write` also need the matching write grant.
> Read metadata for a path without opening it as a file resource.

- `stat(path: string) -> result<file-stat, fs-error>`
  - Reads file metadata without opening the file body.
  - Requires `fs.read:PATH` for the path being inspected.
> List directory entry names for a path.

- `list(path: string) -> result<list<string>, fs-error>`
  - Returns directory entry names for a granted directory.
  - Requires `fs.list:PATH` before the adapter reads the directory.
> Remove one file.

- `remove-file(path: string) -> result<_, fs-error>`
  - Deletes one file.
  - Requires `fs.remove:PATH`; missing grants fail before host deletion is attempted.
> Remove one directory.

- `remove-dir(path: string) -> result<_, fs-error>`
  - Deletes one directory.
  - Requires `fs.remove:PATH`; hosts can still reject non-empty directories.
> Create one directory.

- `mkdir(path: string) -> result<_, fs-error>`
  - Creates one directory.
  - Requires `fs.mkdir:PATH` for the directory being created.
> Rename or move a path.

- `rename(from: string, to: string) -> result<_, fs-error>`
  - Moves or renames a file or directory.
  - Requires grants for both sides: remove/write style access to the source and write style access to the destination.

### Types

#### `file` resource

> Open file resource.

#### `file` methods

> Read up to `n` bytes from the current file cursor.

- `read(n: u32) -> result<list<u8>, fs-error>`
  - Reads up to `n` bytes from an opened file handle.
  - The runtime rechecks the handle path before each adapter read.
> Write bytes at the current file cursor.

- `write(bytes: list<u8>) -> result<u32, fs-error>`
  - Writes bytes to an opened file handle and returns the number written.
  - The runtime rechecks write permission before each adapter write.
> Seek to an absolute byte position.

- `seek-set(pos: u64) -> result<u64, fs-error>`
  - Moves the file cursor to an absolute byte position.
  - The handle must still be valid and backed by a granted file.
> Seek to the end of the file.

- `seek-end() -> result<u64, fs-error>`
  - Moves the file cursor to the end and returns the new position.
  - Useful before append-style writes or size checks.
> Read metadata for this open file handle.

- `stat() -> result<file-stat, fs-error>`
  - Reads metadata for the opened file handle.
  - The runtime rechecks the handle path before the adapter stat call.


## `layer36:fs/types@0.1.0`

Shared filesystem records, modes, and error shapes.

### Types

#### `file-stat` record

> Metadata returned for files and directories.

> Size in bytes for files. Directory size is host-defined.

- `size`: `u64`
> Last modified time in Unix epoch milliseconds.

- `modified-millis`: `u64`
> True when the path is a directory.

- `is-dir`: `bool`

#### `open-mode` variant

> How a file should be opened.

> Open for reads.

- `read`
> Open for writes, creating or truncating according to host policy.

- `write`
> Open for both reads and writes.

- `read-write`
> Open for appending writes.

- `append`

#### `fs-error` variant

> Filesystem error shape used by path and file-handle calls.

> Path does not exist.

- `not-found`
> Capability policy or sandbox rules denied the operation.

- `permission-denied`
> The target already exists.

- `already-exists`
> Path text is not accepted by the Phase 2 path rules.

- `invalid-path`
> Operation needed a directory but found something else.

- `not-a-directory`
> Operation needed a file but found a directory.

- `is-a-directory`
> Host-specific filesystem error text.

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

> Raw argument payload for the current CLI slice.
> 
> The Phase 2 host encodes arguments as newline-separated text. SDKs should
> expose friendlier argument helpers over this raw transport.

- `raw() -> string`
  - Returns the app arguments passed after `--` in `layer36 run`.
  - Current encoding is newline-separated text, so SDK helpers should parse it for app code.


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

> Emit one structured log event to the host.

- `emit(level: log-level, message: string, fields: list<field>)`
  - Sends one structured log event to the host.
  - Fields are plain key/value strings so native hosts can map them to their own log systems.

### Types

#### `field` record

> One key/value pair attached to a log event.

> Field name.

- `key`: `string`
> Field value rendered as text.

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

> Host standard input.

- `stdin() -> own<input-stream>`
  - Returns an input stream connected to the host standard input.
  - Granted by default for CLI apps.
> Host standard output for normal app output.

- `stdout() -> own<output-stream>`
  - Returns an output stream connected to host standard output.
  - Use this for normal command output that other tools may read.
> Host standard error for diagnostics.

- `stderr() -> own<output-stream>`
  - Returns an output stream connected to host standard error.
  - Use this for diagnostics and permission errors.


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

> Readable byte stream owned by the runtime.

#### `output-stream` resource

> Writable byte stream owned by the runtime.

#### `input-stream` methods

> Read up to `n` bytes from the stream.

- `read(n: u32) -> result<list<u8>, io-error>`
  - Reads up to `n` bytes from an input stream.
  - A short read is valid; an empty read means the stream has no more bytes right now or is closed.
> Read the stream as UTF-8 text.

- `read-to-string() -> result<string, io-error>`
  - Reads the stream as UTF-8 text.
  - Invalid UTF-8 returns `io-error.invalid-utf8` instead of lossy text.

#### `output-stream` methods

> Write some bytes and return the number accepted by the host.

- `write(bytes: list<u8>) -> result<u32, io-error>`
  - Writes bytes to an output stream and returns the number accepted.
  - Apps that need all bytes written should use `write-all` or an SDK helper.
> Write the whole byte buffer or return an error.

- `write-all(bytes: list<u8>) -> result<_, io-error>`
  - Writes the full byte buffer or returns an IO error.
  - This is the right primitive for line-oriented CLI output.
> Flush host-side output buffers.

- `flush() -> result<_, io-error>`
  - Asks the host to push buffered output through.
  - Use it before exiting after important diagnostics or prompts.


## `layer36:io/types@0.1.0`

Shared IO log and error types.

### Types

#### `log-level` enum

> Severity level for app log events.

> Very detailed diagnostic data.

- `trace`
> Developer-focused diagnostic data.

- `debug`
> Normal informational event.

- `info`
> Something unexpected happened, but the app can continue.

- `warn`
> The app hit an error condition.

- `error`

#### `io-error` variant

> Error shape for byte streams and text stream helpers.

> The stream was already closed.

- `closed`
> The host interrupted the operation.

- `interrupted`
> The stream ended before enough bytes were read.

- `unexpected-eof`
> Bytes could not be decoded as UTF-8 text.

- `invalid-utf8`
> Host-specific IO error text.

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

> Format Unix epoch milliseconds using a timezone, style, and locale.

- `format-date(millis: u64, tz: string, style: date-style, loc: locale-id) -> string`
  - Formats a timestamp using a requested timezone, date style, and locale.
  - The host owns the native formatting behavior so output can match the platform.
> Format a number using a style and locale.

- `format-number(value: f64, style: number-style, loc: locale-id) -> string`
  - Formats a number using a requested style and locale.
  - Currency style is present in the shape, but richer currency-code handling remains future work.


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
  - Returns the host user's preferred locale as a BCP 47 string.
  - Good for display choices, not for security decisions.
> IANA timezone name, for example "Asia/Singapore".

- `timezone() -> string`
  - Returns the host timezone name.
  - Expected form is an IANA name such as `Asia/Singapore` when the host can provide one.


## `layer36:locale/types@0.1.0`

Locale and formatting type definitions.

### Types

#### `locale-id` record

> Host locale identifier using a BCP 47 language tag.

> Canonicalized BCP 47 locale tag, for example `en-US`.

- `bcp47`: `string`

#### `date-style` enum

> Date rendering style requested from the host.

> Compact numeric date form.

- `short`
> Medium-length date form.

- `medium`
> Long date form.

- `long`
> Full date form.

- `full`

#### `number-style` enum

> Number rendering style requested from the host.

> Decimal number formatting.

- `decimal`
> Percent formatting.

- `percent`
> Currency formatting. Currency code selection remains future work.

- `currency`


## `layer36:net/http-client@0.1.0`

HTTP client calls. Phase 2 starts with simple request and response bodies.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `net.connect:<host>:<port>` - manifest or session grant

- `get` and `fetch` require a matching `net.connect:HOST:PORT` grant before the adapter opens a socket.
- The current host adapter supports plain HTTP request framing first, with a 1 MiB full-response cap; HTTPS, redirects, streaming, and richer network behavior are still Phase 2 work.

### Rust SDK Example

```rust
let body = layer36::net::get_text("http://127.0.0.1:8080/data.txt")?;
layer36::io::stdio::println(&body)?;
```

### Functions

> Perform a simple GET request and return only the response body.

- `get(url: string) -> result<list<u8>, net-error>`
  - Performs a simple HTTP GET and returns only the response body.
  - Requires `net.connect:HOST:PORT`; Phase 2 currently supports the plain HTTP adapter path.
> Perform a buffered HTTP request and return status, headers, and body.

- `fetch(req: request) -> result<response, net-error>`
  - Performs a lower-level HTTP request and returns status, headers, and body.
  - The plain HTTP adapter now forwards the method, app headers, and buffered body while keeping `Host`, `Connection`, and `Content-Length` under host control.
  - Timeouts, oversized bodies, malformed responses, and missing grants are typed as `net-error` cases.


## `layer36:net/types@0.1.0`

Shared network request, response, and error types.

### Types

#### `http-method` enum

> HTTP method for Phase 2 client requests.

> HTTP GET.

- `get`
> HTTP POST.

- `post`
> HTTP PUT.

- `put`
> HTTP DELETE.

- `delete`
> HTTP PATCH.

- `patch`
> HTTP HEAD.

- `head`
> HTTP OPTIONS.

- `options`

#### `header` record

> One HTTP header field.

> Header name.

- `name`: `string`
> Header value.

- `value`: `string`

#### `request` record

> Buffered HTTP request shape.

> Request method.

- `method`: `http-method`
> Absolute request URL.

- `url`: `string`
> App-provided headers. Host-controlled transport headers are rejected.

- `headers`: `list<header>`
> Buffered request body.

- `body`: `list<u8>`
> Optional timeout in milliseconds for this request.

- `timeout-millis`: `option<u32>`

#### `response` record

> Buffered HTTP response shape.

> Numeric HTTP status code.

- `status`: `u16`
> Response headers accepted by the host adapter.

- `headers`: `list<header>`
> Buffered response body.

- `body`: `list<u8>`

#### `net-error` variant

> Network error shape returned by HTTP client calls.

> URL syntax or unsupported Phase 2 URL shape.

- `invalid-url`
> Hostname resolution failed.

- `dns-failure`: `string`
> Socket connection failed.

- `connect-failure`: `string`
> TLS setup failed. HTTPS is not yet implemented in the first Phase 2 adapter slice.

- `tls-failure`: `string`
> Request timed out.

- `timeout`
> Response exceeded the configured body-size limit.

- `body-too-large`
> Capability policy denied the request before socket access.

- `permission-denied`
> Response framing or protocol parsing failed.

- `protocol`: `string`
> Host-specific network error text.

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
  - Reads host wall-clock time in milliseconds since Unix epoch.
  - This value can move backward or forward if the host clock changes.
> Monotonic nanoseconds since an arbitrary origin.
> Guaranteed non-decreasing; suitable for measuring intervals.

- `monotonic-nanos() -> u64`
  - Reads a non-decreasing timer in nanoseconds.
  - Use this for durations instead of wall-clock time.


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
  - Blocks the calling component task for at least the requested milliseconds.
  - Requires `time.sleep`; hosts may wake slightly later than requested.

