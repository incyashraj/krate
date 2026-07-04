#![doc = include_str!("../README.md")]
#![warn(rustdoc::broken_intra_doc_links)]

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
    }

    /// Structured log records emitted through the runtime.
    pub mod log {
        pub use crate::bindings::krate::io::log::{emit, Field};
        pub use crate::bindings::krate::io::types::LogLevel;
    }
}

/// Capability-checked file access.
pub mod fs {
    pub use crate::bindings::krate::fs::files::File;
    pub use crate::bindings::krate::fs::types::{FileStat, FsError, OpenMode};

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
pub mod locale {
    pub use crate::bindings::krate::locale::types::{DateStyle, LocaleId, NumberStyle};

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
