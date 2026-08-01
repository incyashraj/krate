# UAPI Reference

> Generated from `wit/krate/phase2`. Do not edit this page by hand.

Krate Phase 2 exposes the `cli` world from `krate:app@0.1.0`.

The current world imports these interfaces:

- `krate:io/types@0.1.0`
- `krate:io/streams@0.1.0`
- `krate:io/stdio@0.1.0`
- `krate:io/args@0.1.0`
- `krate:io/log@0.1.0`
- `krate:fs/types@0.1.0`
- `krate:fs/files@0.1.0`
- `krate:net/types@0.1.0`
- `krate:net/http-client@0.1.0`
- `krate:time/clock@0.1.0`
- `krate:time/sleep@0.1.0`
- `krate:locale/types@0.1.0`
- `krate:locale/info@0.1.0`
- `krate:locale/format@0.1.0`
- `krate:resources/assets@0.1.0`
- `krate:store/kv@0.1.0`
- `krate:store/sql@0.1.0`
- `krate:store/secret@0.1.0`
- `krate:random/bytes@0.1.0`

The app exports:

- `run() -> s32`

## `krate:fs/files@0.1.0`

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
let text = krate::fs::read_to_string("notes.txt")?;
krate::io::stdio::println(&text)?;
```

### Functions

> Open a path and return a file resource.

- `open(path: string, mode: open-mode) -> result<own<file>, fs-error>`
  - Opens a host file through Krate and returns a `file` handle.
  - `read` needs `fs.read:PATH`; `write`, `append`, and `read-write` also need the matching write grant.
> Open a file the person chose in a dialog, by its token.
> 
> The counterpart to `ui.dialog.open-file`. It takes a token rather than a
> path because the app never learns the path: the person's click granted
> this one file, not the folder it happens to sit in, and handing over a
> path would let the app walk to its siblings.
> 
> A token belongs to one run. It is refused after the run that produced it,
> so an app cannot store one and come back later for a file nobody offered
> again.

- `open-chosen(token: string, mode: open-mode) -> result<own<file>, fs-error>`
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


## `krate:fs/types@0.1.0`

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


## `krate:io/args@0.1.0`

Raw Krate app arguments. These are the arguments passed after `--` in `krate run`.

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
let raw = krate::io::args::raw();
let first = krate::io::args::first_raw(&raw);
```

### Functions

> Raw argument payload for the current CLI slice.
> 
> The Phase 2 host encodes arguments as newline-separated text. SDKs should
> expose friendlier argument helpers over this raw transport.

- `raw() -> string`
  - Returns the app arguments passed after `--` in `krate run`.
  - Current encoding is newline-separated text, so SDK helpers should parse it for app code.


## `krate:io/log@0.1.0`

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


## `krate:io/stdio@0.1.0`

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
krate::io::stdio::println("Hello from Krate")?;
krate::io::stdio::eprintln("debug line")?;
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


## `krate:io/streams@0.1.0`

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
use krate::io::streams::OutputStreamExt;

let out = krate::io::stdio::stdout();
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


## `krate:io/types@0.1.0`

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


## `krate:locale/format@0.1.0`

Host-backed date and number formatting.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `locale.info` - default grant
- `locale.format` - default grant

- Locale reads and formatting are default grants for CLI apps.

### Rust SDK Example

```rust
let locale = krate::locale::current();
let text = krate::locale::format_number(42.0, krate::locale::NumberStyle::Decimal, &locale);
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


## `krate:locale/info@0.1.0`

The host user's current locale and timezone.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `locale.info` - default grant
- `locale.format` - default grant

- Locale reads and formatting are default grants for CLI apps.

### Rust SDK Example

```rust
let locale = krate::locale::current();
let timezone = krate::locale::timezone();
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


## `krate:locale/types@0.1.0`

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


## `krate:net/http-client@0.1.0`

HTTP client calls. Phase 2 starts with simple request and response bodies.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `net.connect:<host>:<port>` - manifest or session grant

- `get` and `fetch` require a matching `net.connect:HOST:PORT` grant before the adapter opens a socket.
- The current host adapter supports plain HTTP request framing first, with a 1 MiB full-response cap; HTTPS, redirects, streaming, and richer network behavior are still Phase 2 work.

### Rust SDK Example

```rust
let body = krate::net::get_text("http://127.0.0.1:8080/data.txt")?;
krate::io::stdio::println(&body)?;
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


## `krate:net/types@0.1.0`

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


## `krate:random/bytes@0.1.0`

### Functions

> Return exactly `count` random bytes.
> 
> A request for zero bytes succeeds and returns nothing, so a caller
> computing a length does not have to special-case the empty case.

- `get(count: u32) -> result<list<u8>, random-error>`
> A uniformly distributed 64-bit value.
> 
> Offered alongside `get` because drawing a number is the common case, and
> assembling one from bytes by hand is somewhere an app can get the byte
> order wrong without ever noticing.

- `next-u64() -> result<u64, random-error>`
> A uniform integer in `[0, bound)`, or an error when `bound` is zero.
> 
> Provided because the obvious way to write this -- take a random number
> modulo `bound` -- is subtly wrong whenever `bound` does not divide the
> range evenly: the low values come up more often. A shuffled deck would
> deal some cards more than others and the output would still look random.
> The host draws again instead of taking a remainder.

- `below(bound: u64) -> result<u64, random-error>`

### Types

#### `random-error` variant

> Error returned by a request for random bytes.

> The app did not receive the `random.bytes` capability.

- `denied`
> More bytes were asked for than one call may return.

- `too-large`
> `below` was given a bound of zero, which names an empty range.
> 
> Its own variant rather than reusing `too-large`, which would say the
> opposite of what happened, or returning zero, which is indistinguishable
> from a legitimate draw.

- `empty-range`
> The operating system had no entropy to give.
> 
> Reported rather than worked around. A caller that receives this knows
> it got nothing; a caller handed weak bytes it believes are strong has no
> way to find out.

- `unavailable`: `string`


## `krate:resources/assets@0.1.0`

### Functions

> Read one asset by its path relative to the bundle's `assets/` directory.

- `read(path: string) -> result<list<u8>, resource-error>`
> List direct children below a relative directory.

- `list(path: string) -> result<list<string>, resource-error>`

### Types

#### `resource-error` variant

> Error returned while resolving an application-bundled resource.

> No bundled asset exists at the requested path.

- `not-found`
> The path was absolute, escaped the asset root, or used unsupported syntax.

- `invalid-path`
> The asset is larger than the runtime's bounded read limit.

- `too-large`
> The host could not read the asset.

- `io`: `string`


## `krate:store/kv@0.1.0`

### Functions

> Read one value. A key that was never set reads as `none` rather than an
> error, because "nothing saved yet" is the normal first run.

- `get(key: string) -> result<option<list<u8>>, store-error>`
> Write one value, replacing whatever was there.

- `set(key: string, value: list<u8>) -> result<_, store-error>`
> Remove one key. Removing a key that is not present succeeds: the caller
> wanted it gone, and it is gone.

- `delete(key: string) -> result<_, store-error>`
> Every key currently set, in sorted order so a listing is stable across
> runs and across operating systems.

- `keys() -> result<list<string>, store-error>`
> Remove everything. Separate from `delete` because "forget all of it" is a
> deliberate action an app should have to name.

- `clear() -> result<_, store-error>`

### Types

#### `store-error` variant

> Error returned by a store operation.

> The app did not receive the `store.kv` capability.

- `denied`
> The key was empty, too long, or used unsupported syntax.

- `invalid-key`
> The value is larger than the runtime's bounded write limit.

- `too-large`
> The store could not be read or written.

- `io`: `string`


## `krate:store/secret@0.1.0`

### Functions

> Read one secret. A name that was never set reads as `none`.

- `get(name: string) -> result<option<list<u8>>, secret-error>`
> Store one secret, replacing whatever was there.

- `set(name: string, secret: list<u8>) -> result<_, secret-error>`
> Remove one secret. Removing one that is absent succeeds.

- `delete(name: string) -> result<_, secret-error>`
> The names of stored secrets, never their values, so a listing cannot
> become a way to read everything at once.

- `names() -> result<list<string>, secret-error>`

### Types

#### `secret-error` variant

> Error returned by a secret operation.

> The app did not receive the `store.secret` capability.

- `denied`
> The name was empty, too long, or used unsupported syntax.

- `invalid-name`
> The secret is larger than the runtime's bounded limit.

- `too-large`
> The secret store could not be read or written.

- `io`: `string`


## `krate:store/sql@0.1.0`

### Functions

> Run a statement that returns rows.
> 
> Parameters are bound, never substituted into the text, so an app cannot
> build an injection out of its own user's input by accident.

- `query(statement: string, params: list<value>) -> result<query-result, sql-error>`
> Run a statement that changes data, returning the number of rows affected.

- `execute(statement: string, params: list<value>) -> result<u64, sql-error>`
> Run several statements as one unit, so a half-applied change cannot
> survive a crash. Any failure rolls the whole batch back.

- `transaction(statements: list<string>) -> result<_, sql-error>`

### Types

#### `value` variant

> One value in a row or a query parameter.
> 
> A closed set rather than an open one: every value crossing the boundary
> has a known shape, so the host never has to interpret app-supplied text as
> a type declaration.

> SQL NULL.

- `null`
> A 64-bit signed integer.

- `integer`: `s64`
> A double-precision float.

- `real`: `f64`
> Text.

- `text`: `string`
> Arbitrary bytes.

- `blob`: `list<u8>`

#### `sql-error` variant

> Error returned by a database operation.

> The app did not receive the `store.sql` capability.

- `denied`
> The statement could not be parsed or refers to something missing.

- `invalid-statement`: `string`
> The statement is one this interface does not permit, such as attaching
> another database or reading a file from the host.

- `forbidden`: `string`
> The result, or the database as a whole, exceeded its bound.

- `too-large`
> The database could not be read or written.

- `io`: `string`

#### `row` record

> One returned row, in the column order of the query.

> The row's values, in the column order of the query.

- `values`: `list<value>`

#### `query-result` record

> The result of a query.

> Column names in the order the values appear, so a caller can address
> results by name without a second round trip.

- `columns`: `list<string>`
> The rows the query matched, in the order the database returned them.

- `rows`: `list<row>`


## `krate:time/clock@0.1.0`

Wall-clock and monotonic clock reads.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `time.clock` - default grant
- `time.monotonic` - default grant
- `time.sleep` - default grant

- `time.clock` and `time.monotonic` are default grants.

### Rust SDK Example

```rust
let now = krate::time::now_millis();
let tick = krate::time::monotonic_nanos();
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


## `krate:time/sleep@0.1.0`

Blocking sleep for CLI-style components.

### Capability Notes

Accepted capability strings for this module, generated from the runtime manifest table:

- `time.clock` - default grant
- `time.monotonic` - default grant
- `time.sleep` - default grant

- `sleep-millis` requires `time.sleep`.

### Rust SDK Example

```rust
krate::time::sleep_millis(100);
```

### Functions

> Block the calling task for at least `millis` milliseconds.

- `sleep-millis(millis: u32)`
  - Blocks the calling component task for at least the requested milliseconds.
  - Requires `time.sleep`; hosts may wake slightly later than requested.

