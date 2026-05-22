//! Windows host adapter surface for Layer36 Phase 2.
//!
//! This crate is the Windows ownership boundary. Shared behavior still comes
//! from `layer36-adapter-common`, while Windows-specific host wiring will land
//! here.

use layer36_adapter_common::{
    locale::{DateStyle, HostLocale, LocaleId, NumberStyle},
    time::HostClock,
    ui::{
        DraftUiAdapter, KeyEvent, PointerEvent, TextInputEvent, Theme, UiAdapter, UiAdapterError,
        UiAdapterInfo, UiEvent, WidgetId, WidgetNode, WidgetTree, WindowAdapter, WindowBackendKind,
        WindowId, WindowOptions, WindowRecord, WindowSize,
    },
};
use std::fs::OpenOptions;
use std::net::ToSocketAddrs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

/// Host family handled by this adapter crate.
pub const HOST_FAMILY: &str = "windows";

/// Windows Phase 3 UI adapter entry point.
///
/// This is still a headless draft adapter. It proves the Windows crate exposes
/// the same UI contract as macOS and Linux before the winit or Win32 bridge lands.
#[derive(Debug, Default)]
pub struct WindowsUiAdapter {
    draft: DraftUiAdapter,
}

impl WindowsUiAdapter {
    /// Create the current Windows UI adapter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the backend used by this adapter build.
    pub fn backend_name(&self) -> &'static str {
        "windows-headless-draft"
    }

    /// Return whether this build creates real native windows.
    pub fn native_windows_enabled(&self) -> bool {
        false
    }

    /// Return the native backend planned for this host.
    pub fn planned_native_window_backend(&self) -> WindowBackendKind {
        WindowBackendKind::Winit
    }
}

impl WindowAdapter for WindowsUiAdapter {
    fn info(&self) -> UiAdapterInfo {
        UiAdapterInfo::headless_draft(
            HOST_FAMILY,
            self.backend_name(),
            self.planned_native_window_backend(),
        )
    }

    fn create_window(&self, options: WindowOptions) -> Result<WindowId, UiAdapterError> {
        WindowAdapter::create_window(&self.draft, options)
    }

    fn show_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::show_window(&self.draft, id)
    }

    fn close_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::close_window(&self.draft, id)
    }

    fn set_title(&self, id: WindowId, title: String) -> Result<(), UiAdapterError> {
        WindowAdapter::set_title(&self.draft, id, title)
    }

    fn set_size(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        WindowAdapter::set_size(&self.draft, id, size)
    }

    fn request_redraw(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::request_redraw(&self.draft, id)
    }

    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError> {
        WindowAdapter::window(&self.draft, id)
    }

    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError> {
        WindowAdapter::drain_events(&self.draft)
    }

    fn poll_event(&self) -> Result<Option<UiEvent>, UiAdapterError> {
        WindowAdapter::poll_event(&self.draft)
    }

    fn queue_close_requested(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_close_requested(&self.draft, id)
    }

    fn queue_host_resize(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_host_resize(&self.draft, id, size)
    }

    fn queue_window_focused(&self, id: WindowId, focused: bool) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_window_focused(&self.draft, id, focused)
    }

    fn queue_theme_changed(&self, theme: Theme) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_theme_changed(&self.draft, theme)
    }

    fn queue_scale_changed(&self, id: WindowId, scale: f32) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_scale_changed(&self.draft, id, scale)
    }
}

impl UiAdapter for WindowsUiAdapter {
    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.draft.set_root(window, root)
    }

    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError> {
        self.draft.upsert_node(window, node)
    }

    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.draft.remove_node(window, widget)
    }

    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.draft.focus_node(window, widget)
    }

    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError> {
        self.draft.widget_tree(window)
    }

    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError> {
        self.draft.focused_widget(window)
    }

    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.draft.queue_pointer_event(event)
    }

    fn queue_key_event(&self, event: KeyEvent) -> Result<(), UiAdapterError> {
        self.draft.queue_key_event(event)
    }

    fn queue_text_input(&self, event: TextInputEvent) -> Result<(), UiAdapterError> {
        self.draft.queue_text_input(event)
    }
}

/// Build the current Windows UI adapter.
pub fn discover_ui_adapter() -> WindowsUiAdapter {
    WindowsUiAdapter::new()
}

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

/// Read from host stdin through the Windows adapter path.
pub fn read_stdin(buf: &mut [u8]) -> std::io::Result<usize> {
    let mut stdin = std::io::stdin();
    std::io::Read::read(&mut stdin, buf)
}

/// Write bytes to host stderr through the Windows adapter path.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::write_all(&mut stderr, bytes)
}

/// Flush host stderr through the Windows adapter path.
pub fn flush_stderr() -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::flush(&mut stderr)
}

/// Print a line to host stdout through the Windows adapter path.
pub fn print_stdout_line(msg: &str) {
    println!("{msg}");
}

/// Write bytes to host stdout through the Windows adapter path.
pub fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::write_all(&mut stdout, bytes)
}

/// Flush host stdout through the Windows adapter path.
pub fn flush_stdout() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::flush(&mut stdout)
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

/// Write a full request buffer through the Windows adapter TCP path.
pub fn write_all_tcp(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    std::io::Write::write_all(stream, bytes)
}

/// Read bytes through the Windows adapter TCP path.
pub fn read_tcp(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(stream, buf)
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
    fn ui_adapter_smoke_creates_blank_draft_window() {
        let adapter = discover_ui_adapter();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Layer36 blank window", size).expect("options"))
            .expect("create window");

        adapter.show_window(id).expect("show");
        adapter.request_redraw(id).expect("redraw");

        let window = adapter.window(id).expect("lookup").expect("window");
        let info = adapter.info();
        let window_adapter: &dyn WindowAdapter = &adapter;
        let window_info = window_adapter.info();
        assert_eq!(info.host_family, HOST_FAMILY);
        assert_eq!(info.backend, "windows-headless-draft");
        assert_eq!(info.window_backend, WindowBackendKind::HeadlessDraft);
        assert_eq!(info.planned_window_backend, WindowBackendKind::Winit);
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);
        assert_eq!(window_info, info);
        assert_eq!(adapter.backend_name(), "windows-headless-draft");
        assert!(!adapter.native_windows_enabled());
        assert_eq!(
            adapter.planned_native_window_backend(),
            WindowBackendKind::Winit
        );
        assert_eq!(window.title, "Layer36 blank window");
        assert_eq!(window.size, size);
        assert!(window.visible);
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::RedrawRequested(id),
            ]
        );
    }

    #[test]
    fn read_stdin_hook_is_available() {
        let hook: fn(&mut [u8]) -> std::io::Result<usize> = read_stdin;
        let _ = hook;
    }

    #[test]
    fn write_stderr_hook_is_available() {
        let hook: fn(&[u8]) -> std::io::Result<()> = write_stderr;
        let _ = hook;
    }

    #[test]
    fn flush_stderr_hook_is_available() {
        let hook: fn() -> std::io::Result<()> = flush_stderr;
        let _ = hook;
    }

    #[test]
    fn print_stdout_line_hook_is_available() {
        let hook: fn(&str) = print_stdout_line;
        let _ = hook;
    }

    #[test]
    fn write_stdout_hook_is_available() {
        let hook: fn(&[u8]) -> std::io::Result<()> = write_stdout;
        let _ = hook;
    }

    #[test]
    fn flush_stdout_hook_is_available() {
        let hook: fn() -> std::io::Result<()> = flush_stdout;
        let _ = hook;
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
    fn write_all_tcp_hook_is_available() {
        let hook: fn(&mut TcpStream, &[u8]) -> std::io::Result<()> = write_all_tcp;
        let _ = hook;
    }

    #[test]
    fn read_tcp_hook_is_available() {
        let hook: fn(&mut TcpStream, &mut [u8]) -> std::io::Result<usize> = read_tcp;
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
