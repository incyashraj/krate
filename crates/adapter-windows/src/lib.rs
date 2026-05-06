//! Windows host adapter surface for Layer36 Phase 2.
//!
//! This crate is the Windows ownership boundary. Shared behavior still comes
//! from `layer36-adapter-common`, while Windows-specific host wiring will land
//! here.

use layer36_adapter_common::{
    locale::{DateStyle, HostLocale, LocaleId, NumberStyle},
    time::HostClock,
};
use std::fs::OpenOptions;
use std::net::ToSocketAddrs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Host family handled by this adapter crate.
pub const HOST_FAMILY: &str = "windows";

/// Resolve locale and timezone for Windows host runs.
pub fn discover_locale(
    locale_override: Option<&str>,
    timezone_override: Option<&str>,
) -> HostLocale {
    HostLocale::from_env_with_overrides(locale_override, timezone_override)
}

/// Build the Windows host clock surface.
pub fn discover_clock(test_time_millis: Option<u64>) -> HostClock {
    HostClock::new(test_time_millis)
}

/// Sleep through the Windows host adapter path.
pub fn sleep_millis(millis: u32) {
    HostClock::sleep_millis(millis);
}

/// Open a TCP stream through the Windows adapter path.
pub fn connect_tcp(addr: SocketAddr, timeout: Option<Duration>) -> std::io::Result<TcpStream> {
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&addr, timeout),
        None => TcpStream::connect(addr),
    }
}

/// Apply read/write timeouts through the Windows adapter path.
pub fn apply_tcp_timeouts(stream: &TcpStream, timeout: Duration) -> std::io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

/// Resolve socket addresses through the Windows adapter path.
pub fn resolve_socket_addrs(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    (host, port).to_socket_addrs().map(Iterator::collect)
}

/// Check blocked-link metadata semantics through the Windows adapter path.
pub fn is_blocked_link_metadata(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }

    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// Read filesystem metadata through the Windows adapter path.
pub fn stat_path(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

/// Read a directory iterator through the Windows adapter path.
pub fn read_dir(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(path)
}

/// Read symlink metadata through the Windows adapter path.
pub fn symlink_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

/// Canonicalize a filesystem path through the Windows adapter path.
pub fn canonicalize_path(path: &Path) -> std::io::Result<std::path::PathBuf> {
    path.canonicalize()
}

/// Open a filesystem path through the Windows adapter path.
pub fn open_path(path: &Path, opts: &mut OpenOptions) -> std::io::Result<std::fs::File> {
    opts.open(path)
}

/// Read from an open file through the Windows adapter path.
pub fn read_file(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(file, buf)
}

/// Write to an open file through the Windows adapter path.
pub fn write_file(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<usize> {
    std::io::Write::write(file, bytes)
}

/// Seek an open file through the Windows adapter path.
pub fn seek_file(file: &mut std::fs::File, pos: std::io::SeekFrom) -> std::io::Result<u64> {
    std::io::Seek::seek(file, pos)
}

/// Read metadata for an open file through the Windows adapter path.
pub fn file_metadata(file: &std::fs::File) -> std::io::Result<std::fs::Metadata> {
    file.metadata()
}

/// Read a full filesystem path through the Windows adapter path.
pub fn read_path(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Ensure a directory tree exists through the Windows adapter path.
pub fn create_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Remove a file through the Windows adapter path.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Remove an empty directory through the Windows adapter path.
pub fn remove_dir(path: &Path) -> std::io::Result<()> {
    std::fs::remove_dir(path)
}

/// Create a directory through the Windows adapter path.
pub fn create_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// Rename a filesystem path through the Windows adapter path.
pub fn rename_path(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Read the current locale through the Windows adapter path.
pub fn current_locale(locale: &HostLocale) -> LocaleId {
    locale.current()
}

/// Read the current timezone through the Windows adapter path.
pub fn timezone(locale: &HostLocale) -> String {
    locale.timezone()
}

/// Format a date through the Windows adapter path.
pub fn format_date(millis: u64, timezone: &str, style: DateStyle, locale: &LocaleId) -> String {
    HostLocale::format_date(millis, timezone, style, locale)
}

/// Format a number through the Windows adapter path.
pub fn format_number(value: f64, style: NumberStyle, locale: &LocaleId) -> String {
    HostLocale::format_number(value, style, locale)
}

/// Apply Windows no-follow-final-symlink open behavior.
pub fn apply_no_follow_final_symlink(opts: &mut OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        // Ask CreateFile to open the reparse point itself so final symlinks are not followed.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    #[cfg(not(windows))]
    {
        let _ = opts;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_family_constant_matches_windows() {
        assert_eq!(HOST_FAMILY, "windows");
    }

    #[test]
    fn locale_discovery_applies_overrides() {
        let locale = discover_locale(Some("en-US"), Some("UTC"));
        assert_eq!(locale.current().bcp47, "en-US");
        assert_eq!(locale.timezone(), "UTC");
    }

    #[test]
    fn clock_discovery_applies_fixed_time_override() {
        let clock = discover_clock(Some(1_777));
        assert_eq!(clock.now_millis().expect("fixed clock"), 1_777);
    }

    #[test]
    fn sleep_hook_accepts_zero_millis() {
        sleep_millis(0);
    }

    #[test]
    fn connect_tcp_hook_is_available() {
        let hook: fn(SocketAddr, Option<Duration>) -> std::io::Result<TcpStream> = connect_tcp;
        let _ = hook;
    }

    #[test]
    fn apply_tcp_timeouts_hook_is_available() {
        let hook: fn(&TcpStream, Duration) -> std::io::Result<()> = apply_tcp_timeouts;
        let _ = hook;
    }

    #[test]
    fn resolve_socket_addrs_hook_is_available() {
        let hook: fn(&str, u16) -> std::io::Result<Vec<SocketAddr>> = resolve_socket_addrs;
        let _ = hook;
    }

    #[test]
    fn blocked_link_metadata_hook_is_available() {
        let hook: fn(&std::fs::Metadata) -> bool = is_blocked_link_metadata;
        let _ = hook;
    }

    #[test]
    fn stat_path_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<std::fs::Metadata> = stat_path;
        let _ = hook;
    }

    #[test]
    fn read_dir_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<std::fs::ReadDir> = read_dir;
        let _ = hook;
    }

    #[test]
    fn symlink_metadata_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<std::fs::Metadata> = symlink_metadata;
        let _ = hook;
    }

    #[test]
    fn canonicalize_path_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<std::path::PathBuf> = canonicalize_path;
        let _ = hook;
    }

    #[test]
    fn open_path_hook_is_available() {
        let hook: fn(&Path, &mut OpenOptions) -> std::io::Result<std::fs::File> = open_path;
        let _ = hook;
    }

    #[test]
    fn read_path_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<Vec<u8>> = read_path;
        let _ = hook;
    }

    #[test]
    fn read_file_hook_is_available() {
        let hook: fn(&mut std::fs::File, &mut [u8]) -> std::io::Result<usize> = read_file;
        let _ = hook;
    }

    #[test]
    fn write_file_hook_is_available() {
        let hook: fn(&mut std::fs::File, &[u8]) -> std::io::Result<usize> = write_file;
        let _ = hook;
    }

    #[test]
    fn seek_file_hook_is_available() {
        let hook: fn(&mut std::fs::File, std::io::SeekFrom) -> std::io::Result<u64> = seek_file;
        let _ = hook;
    }

    #[test]
    fn file_metadata_hook_is_available() {
        let hook: fn(&std::fs::File) -> std::io::Result<std::fs::Metadata> = file_metadata;
        let _ = hook;
    }

    #[test]
    fn create_dir_all_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<()> = create_dir_all;
        let _ = hook;
    }

    #[test]
    fn remove_file_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<()> = remove_file;
        let _ = hook;
    }

    #[test]
    fn remove_dir_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<()> = remove_dir;
        let _ = hook;
    }

    #[test]
    fn create_dir_hook_is_available() {
        let hook: fn(&Path) -> std::io::Result<()> = create_dir;
        let _ = hook;
    }

    #[test]
    fn rename_path_hook_is_available() {
        let hook: fn(&Path, &Path) -> std::io::Result<()> = rename_path;
        let _ = hook;
    }

    #[test]
    fn locale_format_helpers_return_stable_values() {
        let locale = discover_locale(Some("en-US"), Some("UTC"));
        let current = current_locale(&locale);
        let tz = timezone(&locale);
        let date = format_date(1_234, &tz, DateStyle::Medium, &current);
        let number = format_number(42.5, NumberStyle::Decimal, &current);

        assert_eq!(current.bcp47, "en-US");
        assert_eq!(tz, "UTC");
        assert_eq!(date, "1970-01-01 00:00");
        assert_eq!(number, "42.5");
    }

    #[test]
    fn no_follow_hook_accepts_open_options() {
        let mut opts = OpenOptions::new();
        apply_no_follow_final_symlink(&mut opts);
    }
}
