#![doc = include_str!("../README.md")]
#![warn(rustdoc::broken_intra_doc_links)]
// A Krate guest links only `krate:*`. `std` on wasm32-wasip1 carries latent
// `wasi:*` imports (its runtime, panicking, and `std::io`) that dead-code
// elimination only sometimes strips, so a `std` guest leaks them intermittently.
// Building the SDK `no_std` means there is no std runtime to leak: every app is
// `krate:*`-only by construction. `alloc` (Vec/String) is still available.
#![no_std]

extern crate alloc;

// When the `std` feature is on (host builds, docs, unit tests) link std so the
// generated `impl std::error::Error` blocks — which wit-bindgen gates behind
// `cfg(feature = "std")` — resolve. A guest never enables this feature, so it
// stays `no_std`.
#[cfg(feature = "std")]
extern crate std;

// The SDK owns the guest's runtime essentials so no app has to write them: a
// global allocator (for `alloc`'s Vec/String) and a panic handler. These are
// provided only for a `no_std` wasm guest — the shape every Krate app now uses.
// A consumer that links `std` (or enables this crate's `std` feature) already
// gets std's own allocator and panic handler, so gating on `not(feature =
// "std")` avoids a duplicate-lang-item collision. `not(test)` keeps host unit
// tests on std's. An app just writes `#![no_std]` + `extern crate alloc;`.
#[cfg(all(target_arch = "wasm32", not(feature = "std"), not(test)))]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[cfg(all(target_arch = "wasm32", not(feature = "std"), not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

// The compiler emits calls to the C mem* intrinsics (e.g. `memcmp` for a byte
// slice comparison). `std` supplies these on wasm; a `no_std` guest must, or
// they surface as unresolved `env::mem*` imports that break componentization.
// These are the standard, straightforward implementations, provided once here
// so no app has to. Guest-only (no_std wasm); a std consumer uses std's.
#[cfg(all(target_arch = "wasm32", not(feature = "std"), not(test)))]
mod mem_intrinsics {
    #[no_mangle]
    pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        let mut i = 0;
        while i < n {
            *dest.add(i) = *src.add(i);
            i += 1;
        }
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
        if (dest as usize) < (src as usize) {
            let mut i = 0;
            while i < n {
                *dest.add(i) = *src.add(i);
                i += 1;
            }
        } else {
            let mut i = n;
            while i > 0 {
                i -= 1;
                *dest.add(i) = *src.add(i);
            }
        }
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn memset(dest: *mut u8, value: i32, n: usize) -> *mut u8 {
        let byte = value as u8;
        let mut i = 0;
        while i < n {
            *dest.add(i) = byte;
            i += 1;
        }
        dest
    }

    #[no_mangle]
    pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
        let mut i = 0;
        while i < n {
            let av = *a.add(i);
            let bv = *b.add(i);
            if av != bv {
                return av as i32 - bv as i32;
            }
            i += 1;
        }
        0
    }
}

#[allow(warnings)]
#[doc(hidden)]
pub mod bindings;

pub use bindings::Guest;

/// Common imports for small Krate Rust components.
///
/// This keeps sample apps readable while the SDK is still thin:
///
/// ```no_run
/// use krate::prelude::*;
/// ```
pub mod prelude {
    pub use crate::export;
    pub use crate::fs::{self, FileExt, OpenMode};
    pub use crate::io::{self, streams::OutputStreamExt, Guest};
    pub use crate::locale;
    pub use crate::net;
    pub use crate::time;
}

#[macro_export]
macro_rules! export {
    ($ty:ident) => {
        const _: () = {
            #[unsafe(export_name = "run")]
            unsafe extern "C" fn export_run() -> i32 {
                unsafe { $crate::bindings::_export_run_cabi::<$ty>() }
            }
        };
    };
}

/// Standard input, output, arguments, and structured logs.
pub mod io {
    pub use crate::bindings::krate::io::types::IoError;
    pub use crate::Guest;

    /// App arguments passed to `krate run app.wasm -- ...`.
    pub mod args {
        use alloc::string::{String, ToString};
        use alloc::vec::Vec;
        /// Return all app arguments as owned strings.
        ///
        /// This is the easiest helper for normal apps. It parses the current
        /// Phase 2 raw argument format and drops empty entries.
        #[inline]
        pub fn all() -> Vec<String> {
            split_raw(&raw()).map(str::to_string).collect()
        }

        /// Return the first app argument, if one was passed.
        #[inline]
        pub fn first() -> Option<String> {
            first_raw(&raw()).map(str::to_string)
        }

        /// Return raw app arguments passed after `--`.
        ///
        /// The current Phase 2 draft carries arguments as newline-separated
        /// text. `split_raw` and `first_raw` are the safer helpers for normal
        /// apps.
        #[inline]
        pub fn raw() -> String {
            crate::bindings::krate::io::args::raw()
        }

        /// Split a raw Phase 2 argument string into non-empty arguments.
        ///
        /// This accepts a borrowed raw string so tests and parsers can avoid an
        /// extra host call.
        #[inline]
        pub fn split_raw(raw: &str) -> impl Iterator<Item = &str> {
            raw.split('\n').filter(|arg| !arg.is_empty())
        }

        /// Return the first argument from a borrowed raw argument string.
        #[inline]
        pub fn first_raw(raw: &str) -> Option<&str> {
            split_raw(raw).next()
        }
    }

    /// Resource stream helpers for text and byte I/O.
    pub mod streams {
        pub use crate::bindings::krate::io::streams::{InputStream, OutputStream};
        pub use crate::bindings::krate::io::types::IoError;
        use alloc::string::String;
        use alloc::vec::Vec;

        /// Convenience methods for generated Krate input streams.
        pub trait InputStreamExt {
            /// Read until EOF and return every byte.
            fn read_to_end(&self) -> Result<Vec<u8>, IoError>;

            /// Read until EOF and decode the bytes as UTF-8 text.
            fn read_text(&self) -> Result<String, IoError>;
        }

        impl InputStreamExt for InputStream {
            fn read_to_end(&self) -> Result<Vec<u8>, IoError> {
                let mut out = Vec::new();

                loop {
                    let chunk = self.read(8192)?;
                    if chunk.is_empty() {
                        break;
                    }
                    out.extend_from_slice(&chunk);
                }

                Ok(out)
            }

            fn read_text(&self) -> Result<String, IoError> {
                String::from_utf8(self.read_to_end()?).map_err(|_| IoError::InvalidUtf8)
            }
        }

        /// Convenience methods for generated Krate output streams.
        pub trait OutputStreamExt {
            /// Write the complete byte slice.
            fn write_bytes(&self, bytes: &[u8]) -> Result<(), IoError>;

            /// Write text without adding a newline.
            fn write_text(&self, value: &str) -> Result<(), IoError>;

            /// Write text followed by `\n`.
            fn write_line(&self, value: &str) -> Result<(), IoError>;
        }

        impl OutputStreamExt for OutputStream {
            fn write_bytes(&self, bytes: &[u8]) -> Result<(), IoError> {
                self.write_all(bytes)
            }

            fn write_text(&self, value: &str) -> Result<(), IoError> {
                self.write_all(value.as_bytes())
            }

            fn write_line(&self, value: &str) -> Result<(), IoError> {
                self.write_all(value.as_bytes())?;
                self.write_all(b"\n")
            }
        }
    }

    /// Host standard streams.
    pub mod stdio {
        pub use crate::bindings::krate::io::stdio::{stderr, stdin, stdout};

        use super::streams::OutputStreamExt;
        use super::IoError;

        /// Write text to stdout.
        pub fn print(value: &str) -> Result<(), IoError> {
            stdout().write_text(value)
        }

        /// Write text plus a newline to stdout.
        pub fn println(value: &str) -> Result<(), IoError> {
            stdout().write_line(value)
        }

        /// Write text to stderr.
        pub fn eprint(value: &str) -> Result<(), IoError> {
            stderr().write_text(value)
        }

        /// Write text plus a newline to stderr.
        pub fn eprintln(value: &str) -> Result<(), IoError> {
            stderr().write_line(value)
        }

        /// Write raw bytes to stdout, with no newline and no UTF-8 requirement.
        ///
        /// Not every program's output is text. A hex viewer, an image filter, or
        /// anything piping binary through stdout needs this; the text helpers
        /// above cannot express it, because `&str` must be valid UTF-8.
        pub fn write(bytes: &[u8]) -> Result<(), IoError> {
            stdout().write_bytes(bytes)
        }

        /// Write raw bytes to stderr.
        pub fn ewrite(bytes: &[u8]) -> Result<(), IoError> {
            stderr().write_bytes(bytes)
        }
    }

    /// Structured log records emitted through the runtime.
    pub mod log {
        pub use crate::bindings::krate::io::log::{emit, Field};
        pub use crate::bindings::krate::io::types::LogLevel;
    }
}

/// The app's own durable key-value store.
///
/// Keys, not paths: an app cannot name a location, so this can never widen into
/// reading the person's files. Requires the `store.kv` capability, and every
/// call refuses with `Denied` without it.
pub mod store {
    pub use crate::bindings::krate::store::kv::StoreError;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Read one value. A key that was never set reads as `None`, because
    /// "nothing saved yet" is the normal first run rather than a failure.
    pub fn get(key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        crate::bindings::krate::store::kv::get(key)
    }

    /// Read one value as UTF-8 text, which is what most settings are.
    pub fn get_text(key: &str) -> Result<Option<String>, StoreError> {
        match get(key)? {
            Some(bytes) => Ok(String::from_utf8(bytes).ok()),
            None => Ok(None),
        }
    }

    /// Write one value, replacing whatever was there.
    pub fn set(key: &str, value: &[u8]) -> Result<(), StoreError> {
        crate::bindings::krate::store::kv::set(key, value)
    }

    /// Write one value as UTF-8 text.
    pub fn set_text(key: &str, value: &str) -> Result<(), StoreError> {
        set(key, value.as_bytes())
    }

    /// Remove one key. Removing a key that is not there succeeds.
    pub fn delete(key: &str) -> Result<(), StoreError> {
        crate::bindings::krate::store::kv::delete(key)
    }

    /// Every key currently set, sorted, so a listing is stable between runs.
    pub fn keys() -> Result<Vec<String>, StoreError> {
        crate::bindings::krate::store::kv::keys()
    }

    /// Remove everything this app has saved.
    pub fn clear() -> Result<(), StoreError> {
        crate::bindings::krate::store::kv::clear()
    }
}

/// Secrets the app keeps for itself, such as a sign-in token.
///
/// Encrypted at rest with a key derived per machine and per app, so a copied
/// file does not carry usable secrets elsewhere and one app cannot read
/// another's. Requires the `store.secret` capability.
pub mod secret {
    pub use crate::bindings::krate::store::secret::SecretError;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Read one secret. A name never set reads as `None`.
    pub fn get(name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        crate::bindings::krate::store::secret::get(name)
    }

    /// Read one secret as text, which is what a token usually is.
    pub fn get_text(name: &str) -> Result<Option<String>, SecretError> {
        match get(name)? {
            Some(bytes) => Ok(String::from_utf8(bytes).ok()),
            None => Ok(None),
        }
    }

    /// Store one secret, replacing whatever was there.
    pub fn set(name: &str, secret: &[u8]) -> Result<(), SecretError> {
        crate::bindings::krate::store::secret::set(name, secret)
    }

    /// Store one secret given as text.
    pub fn set_text(name: &str, secret: &str) -> Result<(), SecretError> {
        set(name, secret.as_bytes())
    }

    /// Remove one secret. Removing one that is absent succeeds.
    pub fn delete(name: &str) -> Result<(), SecretError> {
        crate::bindings::krate::store::secret::delete(name)
    }

    /// The names of stored secrets, never their values.
    pub fn names() -> Result<Vec<String>, SecretError> {
        crate::bindings::krate::store::secret::names()
    }
}

/// The app's own database.
///
/// Tables, not files: the app writes SQL against a database the runtime keeps
/// for it, and never learns a path. Requires the `store.sql` capability.
/// Statements that would reach outside that database -- attaching another one,
/// pragmas, the file-reading functions -- are refused by the host.
pub mod sql {
    pub use crate::bindings::krate::store::sql::{QueryResult, Row, SqlError, Value};
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Run a statement that returns rows.
    ///
    /// Pass values as `params` rather than building them into the text; they
    /// are bound all the way to the database, so user input cannot become SQL.
    pub fn query(statement: &str, params: &[Value]) -> Result<QueryResult, SqlError> {
        crate::bindings::krate::store::sql::query(statement, params)
    }

    /// Run a statement that changes data, returning the rows affected.
    pub fn execute(statement: &str, params: &[Value]) -> Result<u64, SqlError> {
        crate::bindings::krate::store::sql::execute(statement, params)
    }

    /// Run several statements as one unit. Any failure rolls the batch back, so
    /// a half-applied migration cannot survive a crash.
    pub fn transaction(statements: &[String]) -> Result<(), SqlError> {
        crate::bindings::krate::store::sql::transaction(statements)
    }

    /// The text of the first column of the first row, which is the shape of
    /// most one-answer queries ("what is the current schema version?").
    pub fn query_one_text(statement: &str, params: &[Value]) -> Result<Option<String>, SqlError> {
        let result = query(statement, params)?;
        Ok(result
            .rows
            .first()
            .and_then(|row| row.values.first())
            .and_then(|value| match value {
                Value::Text(text) => Some(text.clone()),
                _ => None,
            }))
    }

    /// Every row's first column as text, the shape of "list the things".
    pub fn query_texts(statement: &str, params: &[Value]) -> Result<Vec<String>, SqlError> {
        let result = query(statement, params)?;
        let mut out = Vec::new();
        for row in &result.rows {
            if let Some(Value::Text(text)) = row.values.first() {
                out.push(text.clone());
            }
        }
        Ok(out)
    }
}

/// Capability-checked file access.
pub mod fs {
    pub use crate::bindings::krate::fs::files::File;
    pub use crate::bindings::krate::fs::types::{FileStat, FsError, OpenMode};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// Open a file through the Krate filesystem UAPI.
    ///
    /// The runtime checks the active UCap session before the host filesystem is
    /// touched.
    pub fn open(path: &str, mode: OpenMode) -> Result<File, FsError> {
        crate::bindings::krate::fs::files::open(path, mode)
    }

    /// Read a whole file as bytes.
    pub fn read(path: &str) -> Result<Vec<u8>, FsError> {
        open(path, OpenMode::Read)?.read_to_end()
    }

    /// Read a whole file as UTF-8 text.
    pub fn read_to_string(path: &str) -> Result<String, FsError> {
        open(path, OpenMode::Read)?.read_text()
    }

    /// Replace or create a file with the supplied bytes.
    pub fn write(path: &str, bytes: &[u8]) -> Result<(), FsError> {
        open(path, OpenMode::Write)?.write_all(bytes)
    }

    /// Return metadata for a path.
    pub fn stat(path: &str) -> Result<FileStat, FsError> {
        crate::bindings::krate::fs::files::stat(path)
    }

    /// List a directory.
    pub fn list(path: &str) -> Result<Vec<String>, FsError> {
        crate::bindings::krate::fs::files::list(path)
    }

    /// Remove a file.
    pub fn remove_file(path: &str) -> Result<(), FsError> {
        crate::bindings::krate::fs::files::remove_file(path)
    }

    /// Remove a directory.
    pub fn remove_dir(path: &str) -> Result<(), FsError> {
        crate::bindings::krate::fs::files::remove_dir(path)
    }

    /// Create a directory.
    pub fn mkdir(path: &str) -> Result<(), FsError> {
        crate::bindings::krate::fs::files::mkdir(path)
    }

    /// Rename or move a path.
    pub fn rename(from: &str, to: &str) -> Result<(), FsError> {
        crate::bindings::krate::fs::files::rename(from, to)
    }

    /// Convenience methods for generated Krate file resources.
    pub trait FileExt {
        /// Read the file from the current cursor position until EOF.
        fn read_to_end(&self) -> Result<Vec<u8>, FsError>;

        /// Read the file from the current cursor position as UTF-8 text.
        fn read_text(&self) -> Result<String, FsError>;

        /// Keep writing until every byte has been accepted by the host.
        fn write_all(&self, bytes: &[u8]) -> Result<(), FsError>;

        /// Write text without adding a newline.
        fn write_text(&self, value: &str) -> Result<(), FsError>;
    }

    impl FileExt for File {
        fn read_to_end(&self) -> Result<Vec<u8>, FsError> {
            let mut out = Vec::new();

            loop {
                let chunk = self.read(8192)?;
                if chunk.is_empty() {
                    break;
                }
                out.extend_from_slice(&chunk);
            }

            Ok(out)
        }

        fn read_text(&self) -> Result<String, FsError> {
            String::from_utf8(self.read_to_end()?)
                .map_err(|_| FsError::Io("file is not valid UTF-8".to_string()))
        }

        fn write_all(&self, bytes: &[u8]) -> Result<(), FsError> {
            let mut written = 0;
            while written < bytes.len() {
                let count = self.write(&bytes[written..])? as usize;
                if count == 0 {
                    return Err(FsError::Io("file write made no progress".to_string()));
                }
                written += count;
            }

            Ok(())
        }

        fn write_text(&self, value: &str) -> Result<(), FsError> {
            self.write_all(value.as_bytes())
        }
    }
}

/// Capability-checked HTTP client access.
pub mod net {
    pub use crate::bindings::krate::net::types::{Header, HttpMethod, NetError, Request, Response};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    /// Fetch a URL with a simple HTTP GET and return the response body.
    pub fn get(url: &str) -> Result<Vec<u8>, NetError> {
        crate::bindings::krate::net::http_client::get(url)
    }

    /// Fetch a URL with HTTP GET and decode the response body as UTF-8 text.
    pub fn get_text(url: &str) -> Result<String, NetError> {
        String::from_utf8(get(url)?)
            .map_err(|_| NetError::Other("response body is not valid UTF-8".to_string()))
    }

    /// Send a lower-level HTTP request record.
    ///
    /// The current local adapter supports plain HTTP request framing. It sends
    /// the selected method, app headers, and buffered body while keeping
    /// transport headers such as `Host`, `Connection`, and `Content-Length`
    /// under host control.
    pub fn fetch(req: Request) -> Result<Response, NetError> {
        crate::bindings::krate::net::http_client::fetch(&req)
    }
}

/// Wall-clock, monotonic clock, and sleep helpers.
pub mod time {
    /// Return the current wall-clock time in milliseconds since Unix epoch.
    pub fn now_millis() -> u64 {
        clock::now_millis()
    }

    /// Return a monotonic timestamp in nanoseconds.
    pub fn monotonic_nanos() -> u64 {
        clock::monotonic_nanos()
    }

    /// Sleep the current component for at least the requested milliseconds.
    pub fn sleep_millis(millis: u32) {
        sleep::sleep_millis(millis)
    }

    /// Clock functions from `krate:time/clock`.
    pub mod clock {
        /// Return the current wall-clock time in milliseconds since Unix epoch.
        pub fn now_millis() -> u64 {
            crate::bindings::krate::time::clock::now_millis()
        }

        /// Return a monotonic timestamp in nanoseconds.
        pub fn monotonic_nanos() -> u64 {
            crate::bindings::krate::time::clock::monotonic_nanos()
        }
    }

    /// Sleep functions from `krate:time/sleep`.
    pub mod sleep {
        /// Sleep the current component for at least the requested milliseconds.
        pub fn sleep_millis(millis: u32) {
            crate::bindings::krate::time::sleep::sleep_millis(millis)
        }
    }
}

/// Locale, timezone, date, and number formatting helpers.
/// Random bytes from the operating system.
///
/// Requires the `random.bytes` capability; every call refuses with `Denied`
/// without it. There is no seeded generator and no way to ask for one -- an app
/// handed a predictable stream while believing it is random has no way to tell.
pub mod random {
    pub use crate::bindings::krate::random::bytes::RandomError;
    use alloc::vec::Vec;

    /// Return exactly `count` random bytes.
    pub fn bytes(count: u32) -> Result<Vec<u8>, RandomError> {
        crate::bindings::krate::random::bytes::get(count)
    }

    /// A uniformly distributed 64-bit value.
    pub fn u64() -> Result<u64, RandomError> {
        crate::bindings::krate::random::bytes::next_u64()
    }

    /// A uniform integer in `[0, bound)`.
    ///
    /// Prefer this over `u64()? % bound`. Taking a remainder skews the result
    /// toward the low end whenever `bound` does not divide the range evenly:
    /// a shuffled deck deals some cards more often than others, and nothing in
    /// the output looks wrong. The host draws again instead.
    pub fn below(bound: u64) -> Result<u64, RandomError> {
        crate::bindings::krate::random::bytes::below(bound)
    }

    /// Fill a fixed-size buffer with random bytes.
    ///
    /// Takes a slice the caller already owns, so an app following the
    /// fixed-capacity discipline can draw entropy without allocating.
    pub fn fill(buf: &mut [u8]) -> Result<(), RandomError> {
        let drawn = bytes(buf.len() as u32)?;
        buf.copy_from_slice(&drawn);
        Ok(())
    }

    /// Shuffle a slice into a uniformly random order.
    ///
    /// Provided because writing this by hand is where shuffles go wrong: the
    /// obvious loop that swaps each item with any other position does not
    /// produce a uniform permutation. This is Fisher-Yates, which does.
    pub fn shuffle<T>(items: &mut [T]) -> Result<(), RandomError> {
        if items.len() < 2 {
            return Ok(());
        }
        let mut i = items.len() - 1;
        while i > 0 {
            let j = below(i as u64 + 1)? as usize;
            items.swap(i, j);
            i -= 1;
        }
        Ok(())
    }
}

pub mod locale {
    pub use crate::bindings::krate::locale::types::{DateStyle, LocaleId, NumberStyle};
    use alloc::string::String;

    /// Return the user's current locale.
    pub fn current() -> LocaleId {
        info::current()
    }

    /// Return the user's current timezone identifier.
    pub fn timezone() -> String {
        info::timezone()
    }

    /// Format a millisecond timestamp for a locale and timezone.
    pub fn format_date(millis: u64, tz: &str, style: DateStyle, loc: &LocaleId) -> String {
        format::format_date(millis, tz, style, loc)
    }

    /// Format a number for a locale.
    pub fn format_number(value: f64, style: NumberStyle, loc: &LocaleId) -> String {
        format::format_number(value, style, loc)
    }

    /// Locale and timezone discovery.
    pub mod info {
        pub use crate::bindings::krate::locale::info::{current, timezone};
    }

    /// Date and number formatting.
    pub mod format {
        pub use crate::bindings::krate::locale::format::{format_date, format_number};
        pub use crate::bindings::krate::locale::types::{DateStyle, LocaleId, NumberStyle};
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn sdk_reexports_core_phase2_types() {
        let mode = fs::OpenMode::Read;
        let method = net::HttpMethod::Get;
        let date_style = locale::DateStyle::Short;

        assert!(matches!(mode, fs::OpenMode::Read));
        assert!(matches!(method, net::HttpMethod::Get));
        assert!(matches!(date_style, locale::DateStyle::Short));
    }

    #[test]
    fn sdk_splits_raw_phase2_args() {
        assert_eq!(
            io::args::split_raw("one\n\nthree\n").collect::<Vec<_>>(),
            vec!["one", "three"]
        );

        assert_eq!(io::args::first_raw("one\nthree\n"), Some("one"));
    }
}
