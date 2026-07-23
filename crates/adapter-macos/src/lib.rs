//! macOS host adapter surface for Krate Phase 2.
//!
//! This crate is the macOS ownership boundary. Shared behavior still comes from
//! `krate-adapter-common`, while macOS-specific host wiring will land here.

mod appkit;

use krate_adapter_common::{
    locale::{DateStyle, HostLocale, LocaleId, NumberStyle},
    time::HostClock,
    ui::{
        DraftUiAdapter, KeyEvent, NativeWindowHandle, PointerEvent, TextChangedEvent,
        TextInputEvent, Theme, UiAdapter, UiAdapterError, UiAdapterInfo, UiEvent, UiEventLoopTick,
        WidgetId, WidgetNode, WidgetPlacement, WidgetTree, WindowAdapter, WindowBackendKind,
        WindowId, WindowOptions, WindowRecord, WindowSize,
    },
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::net::ToSocketAddrs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

pub use appkit::{
    AppKitColor, AppKitDrawFrame, AppKitDrawSurfaceState, AppKitDrawViewSurface,
    AppKitDrawViewSurfaceSnapshot, AppKitWidgetPlacement, AppKitWidgetSurface,
    AppKitWidgetSurfaceSnapshot, AppKitWindowBackend, AppKitWindowDelegateBridge,
    AppKitWindowDelegateCallback, AppKitWindowDelegateQueue, AppKitWindowEventLoopDriver,
    AppKitWindowEventLoopStep, AppKitWindowEventLoopStepReport, AppKitWindowEventState,
    AppKitWindowNativeDelegate, AppKitWindowNativeEvent, AppKitWindowPrototype,
    AppKitWindowSession, AppKitWindowSnapshot,
};

/// Host family handled by this adapter crate.
pub const HOST_FAMILY: &str = "macos";

thread_local! {
    static APPKIT_PROTOTYPE_SESSIONS: RefCell<BTreeMap<WindowId, AppKitWindowSession>> =
        const { RefCell::new(BTreeMap::new()) };
}

/// macOS Phase 3 UI adapter entry point.
///
/// This is still a headless draft adapter. It proves the macOS crate exposes
/// the same UI contract as Linux and Windows before the AppKit bridge lands.
#[derive(Debug, Default)]
pub struct MacosUiAdapter {
    draft: DraftUiAdapter,
}

impl MacosUiAdapter {
    /// Create the current macOS UI adapter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the backend used by this adapter build.
    pub fn backend_name(&self) -> &'static str {
        "macos-headless-draft"
    }

    /// Return whether this build creates real native windows.
    pub fn native_windows_enabled(&self) -> bool {
        false
    }

    /// Return the native backend planned for this host.
    pub fn planned_native_window_backend(&self) -> WindowBackendKind {
        WindowBackendKind::AppKit
    }

    /// Attach a real AppKit `NSWindow` pointer to a Krate window id.
    ///
    /// This is the handoff point for the first native macOS backend. The
    /// current default adapter still creates headless draft windows, but the
    /// AppKit bridge can now bind its native handle to the same event stream.
    pub fn attach_appkit_window_handle(
        &self,
        id: WindowId,
        raw_handle: u64,
    ) -> Result<NativeWindowHandle, UiAdapterError> {
        let handle = NativeWindowHandle::new(WindowBackendKind::AppKit, raw_handle)?;
        WindowAdapter::attach_native_window(self, id, handle)?;
        Ok(handle)
    }
}

/// Opt-in macOS adapter that uses the AppKit prototype path.
///
/// The default macOS adapter remains headless. This adapter is selected only by
/// runtime code that explicitly asks for the native prototype mode.
#[derive(Debug, Default)]
pub struct MacosAppKitPrototypeUiAdapter {
    headless: MacosUiAdapter,
    backend: AppKitWindowBackend,
    driver: AppKitWindowEventLoopDriver,
}

impl MacosAppKitPrototypeUiAdapter {
    /// Create the opt-in AppKit prototype adapter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the backend used by this adapter build.
    pub fn backend_name(&self) -> &'static str {
        "macos-appkit-prototype"
    }

    /// Return whether this build can create AppKit windows.
    pub fn native_windows_enabled(&self) -> bool {
        self.backend.is_available()
    }

    /// Return whether this build has the first native event-loop driver.
    pub fn native_event_loop_enabled(&self) -> bool {
        self.backend.is_available()
    }

    /// Pump one native AppKit event-loop tick for a tracked prototype window.
    pub fn pump_event_loop_once(
        &self,
        id: WindowId,
    ) -> Result<Option<AppKitWindowEventLoopStepReport>, UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let mut sessions = sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&id) else {
                return Ok(None);
            };
            session
                .pump_event_loop_once(&self.driver, &self.backend, &self.headless)
                .map(Some)
        })
    }

    /// Lower widget placements into native AppKit controls for a tracked window.
    ///
    /// Returns `Ok(None)` when the window has no tracked native session.
    pub fn lower_widget_placements(
        &self,
        id: WindowId,
        placements: &[AppKitWidgetPlacement],
    ) -> Result<Option<AppKitWidgetSurfaceSnapshot>, UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let mut sessions = sessions.borrow_mut();
            let Some(session) = sessions.get_mut(&id) else {
                return Ok(None);
            };
            session.lower_widget_placements(placements).map(Some)
        })
    }

    /// Trigger a lowered native button exactly as a physical click would.
    pub fn perform_widget_click(
        &self,
        id: WindowId,
        widget: WidgetId,
    ) -> Result<bool, UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            let Some(session) = sessions.get(&id) else {
                return Ok(false);
            };
            session.perform_widget_click(widget)?;
            Ok(true)
        })
    }

    /// Set the text of a lowered native text field or label.
    pub fn set_widget_text(
        &self,
        id: WindowId,
        widget: WidgetId,
        text: &str,
    ) -> Result<bool, UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            let Some(session) = sessions.get(&id) else {
                return Ok(false);
            };
            session.set_widget_text(widget, text)?;
            Ok(true)
        })
    }

    /// Read the text of a lowered native control.
    pub fn widget_text(
        &self,
        id: WindowId,
        widget: WidgetId,
    ) -> Result<Option<String>, UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            let Some(session) = sessions.get(&id) else {
                return Ok(None);
            };
            session.widget_text(widget).map(Some)
        })
    }

    /// Widgets lowered to controls a person can type into.
    pub fn editable_widgets(&self, id: WindowId) -> Vec<WidgetId> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            sessions
                .get(&id)
                .map(|session| session.editable_widgets())
                .unwrap_or_default()
        })
    }

    fn remove_session(&self, id: WindowId) {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            sessions.borrow_mut().remove(&id);
        });
    }
}

impl WindowAdapter for MacosUiAdapter {
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

    fn attach_native_window(
        &self,
        id: WindowId,
        handle: NativeWindowHandle,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::attach_native_window(&self.draft, id, handle)
    }

    fn native_window(&self, id: WindowId) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        WindowAdapter::native_window(&self.draft, id)
    }

    fn detach_native_window(
        &self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        WindowAdapter::detach_native_window(&self.draft, id)
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

impl WindowAdapter for MacosAppKitPrototypeUiAdapter {
    fn info(&self) -> UiAdapterInfo {
        let native = self.native_windows_enabled();
        UiAdapterInfo::new(
            HOST_FAMILY,
            self.backend_name(),
            if native {
                WindowBackendKind::AppKit
            } else {
                WindowBackendKind::HeadlessDraft
            },
            WindowBackendKind::AppKit,
            native,
            self.native_event_loop_enabled(),
        )
    }

    fn create_window(&self, options: WindowOptions) -> Result<WindowId, UiAdapterError> {
        let mut session = self.backend.create_session(&self.headless, options)?;
        session.install_native_delegate()?;
        let id = session.id();
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            sessions.borrow_mut().insert(id, session);
        });
        Ok(id)
    }

    fn show_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        APPKIT_PROTOTYPE_SESSIONS.with(|sessions| {
            let sessions = sessions.borrow();
            if let Some(session) = sessions.get(&id) {
                session.show(&self.backend, &self.headless)
            } else {
                WindowAdapter::show_window(&self.headless, id)
            }
        })
    }

    fn close_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        self.remove_session(id);
        WindowAdapter::close_window(&self.headless, id)
    }

    fn set_title(&self, id: WindowId, title: String) -> Result<(), UiAdapterError> {
        WindowAdapter::set_title(&self.headless, id, title)
    }

    fn set_size(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        WindowAdapter::set_size(&self.headless, id, size)
    }

    fn request_redraw(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::request_redraw(&self.headless, id)
    }

    fn window(&self, id: WindowId) -> Result<Option<WindowRecord>, UiAdapterError> {
        WindowAdapter::window(&self.headless, id)
    }

    fn attach_native_window(
        &self,
        id: WindowId,
        handle: NativeWindowHandle,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::attach_native_window(&self.headless, id, handle)
    }

    fn native_window(&self, id: WindowId) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        WindowAdapter::native_window(&self.headless, id)
    }

    fn detach_native_window(
        &self,
        id: WindowId,
    ) -> Result<Option<NativeWindowHandle>, UiAdapterError> {
        WindowAdapter::detach_native_window(&self.headless, id)
    }

    fn drain_events(&self) -> Result<Vec<UiEvent>, UiAdapterError> {
        WindowAdapter::drain_events(&self.headless)
    }

    fn poll_event(&self) -> Result<Option<UiEvent>, UiAdapterError> {
        WindowAdapter::poll_event(&self.headless)
    }

    fn queue_close_requested(&self, id: WindowId) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_close_requested(&self.headless, id)
    }

    fn queue_host_resize(&self, id: WindowId, size: WindowSize) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_host_resize(&self.headless, id, size)
    }

    fn queue_window_focused(&self, id: WindowId, focused: bool) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_window_focused(&self.headless, id, focused)
    }

    fn queue_theme_changed(&self, theme: Theme) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_theme_changed(&self.headless, theme)
    }

    fn queue_scale_changed(&self, id: WindowId, scale: f32) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_scale_changed(&self.headless, id, scale)
    }
}

impl UiAdapter for MacosUiAdapter {
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

    fn queue_text_changed(&self, event: TextChangedEvent) -> Result<(), UiAdapterError> {
        self.draft.queue_text_changed(event)
    }
}

impl UiAdapter for MacosAppKitPrototypeUiAdapter {
    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.headless.set_root(window, root)
    }

    /// AppKit keeps typed text inside the native control, so the guest never
    /// sees it unless the host reads it back. These two make that possible.
    fn native_widget_text(&self, window: WindowId, widget: WidgetId) -> Option<String> {
        MacosAppKitPrototypeUiAdapter::widget_text(self, window, widget)
            .ok()
            .flatten()
    }

    fn native_editable_widgets(&self, window: WindowId) -> Vec<WidgetId> {
        MacosAppKitPrototypeUiAdapter::editable_widgets(self, window)
    }

    fn lower_widget_placements(
        &self,
        window: WindowId,
        placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        let mut native = Vec::with_capacity(placements.len());
        for placement in placements {
            // Kinds without native AppKit lowering yet (checkbox, slider,
            // progress, ...) are skipped, not errors: one drawn-only widget
            // in the tree must not break the window's native controls.
            if !AppKitWidgetPlacement::kind_supported(placement.kind) {
                continue;
            }
            native.push(
                AppKitWidgetPlacement::new(
                    placement.widget,
                    placement.kind,
                    placement.label.clone(),
                    placement.x,
                    placement.y,
                    placement.width,
                    placement.height,
                )?
                .with_clickable(placement.clickable)
                .with_checked(placement.checked),
            );
        }

        match self.lower_widget_placements(window, &native)? {
            Some(snapshot) => Ok(snapshot.lowered.len()),
            None => Ok(0),
        }
    }

    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError> {
        self.headless.upsert_node(window, node)
    }

    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.headless.remove_node(window, widget)
    }

    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.headless.focus_node(window, widget)
    }

    fn widget_tree(&self, window: WindowId) -> Result<Option<WidgetTree>, UiAdapterError> {
        self.headless.widget_tree(window)
    }

    fn focused_widget(&self, window: WindowId) -> Result<Option<WidgetId>, UiAdapterError> {
        self.headless.focused_widget(window)
    }

    fn queue_pointer_event(&self, event: PointerEvent) -> Result<(), UiAdapterError> {
        self.headless.queue_pointer_event(event)
    }

    fn queue_key_event(&self, event: KeyEvent) -> Result<(), UiAdapterError> {
        self.headless.queue_key_event(event)
    }

    fn queue_text_input(&self, event: TextInputEvent) -> Result<(), UiAdapterError> {
        self.headless.queue_text_input(event)
    }

    fn queue_text_changed(&self, event: TextChangedEvent) -> Result<(), UiAdapterError> {
        self.headless.queue_text_changed(event)
    }

    fn read_clipboard_text(&self) -> Result<String, UiAdapterError> {
        self.headless.read_clipboard_text()
    }

    fn write_clipboard_text(&self, text: &str) -> Result<(), UiAdapterError> {
        self.headless.write_clipboard_text(text)
    }

    fn pump_event_loop_once(
        &self,
        window: WindowId,
    ) -> Result<Option<UiEventLoopTick>, UiAdapterError> {
        Ok(
            MacosAppKitPrototypeUiAdapter::pump_event_loop_once(self, window)?.map(|report| {
                UiEventLoopTick {
                    window,
                    callbacks_handled: report.callbacks_handled,
                    snapshot_refreshed: report.snapshot.is_some(),
                    redraw_requested: report.redraw_requested,
                }
            }),
        )
    }
}

/// Build the current macOS UI adapter.
pub fn discover_ui_adapter() -> MacosUiAdapter {
    MacosUiAdapter::new()
}

/// Build the opt-in AppKit prototype UI adapter.
pub fn discover_appkit_prototype_ui_adapter(
) -> Result<MacosAppKitPrototypeUiAdapter, UiAdapterError> {
    let adapter = MacosAppKitPrototypeUiAdapter::new();
    if !adapter.native_windows_enabled() {
        return Err(UiAdapterError::Unsupported(
            "AppKit prototype UI adapter is only available on macOS".to_string(),
        ));
    }
    Ok(adapter)
}

/// Resolve locale and timezone for macOS host runs.
pub fn discover_locale(
    locale_override: Option<&str>,
    timezone_override: Option<&str>,
) -> HostLocale {
    HostLocale::from_env_with_overrides(locale_override, timezone_override)
}

/// Build the macOS host clock surface.
pub fn discover_clock(test_time_millis: Option<u64>) -> HostClock {
    HostClock::new(test_time_millis)
}

/// Sleep through the macOS host adapter path.
pub fn sleep_millis(millis: u32) {
    HostClock::sleep_millis(millis);
}

/// Read from host stdin through the macOS adapter path.
pub fn read_stdin(buf: &mut [u8]) -> std::io::Result<usize> {
    let mut stdin = std::io::stdin();
    std::io::Read::read(&mut stdin, buf)
}

/// Write bytes to host stderr through the macOS adapter path.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::write_all(&mut stderr, bytes)
}

/// Flush host stderr through the macOS adapter path.
pub fn flush_stderr() -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::flush(&mut stderr)
}

/// Print a line to host stdout through the macOS adapter path.
pub fn print_stdout_line(msg: &str) {
    println!("{msg}");
}

/// Write bytes to host stdout through the macOS adapter path.
pub fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::write_all(&mut stdout, bytes)
}

/// Flush host stdout through the macOS adapter path.
pub fn flush_stdout() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::flush(&mut stdout)
}

/// Open a TCP stream through the macOS adapter path.
pub fn connect_tcp(addr: SocketAddr, timeout: Option<Duration>) -> std::io::Result<TcpStream> {
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&addr, timeout),
        None => TcpStream::connect(addr),
    }
}

/// Apply read/write timeouts through the macOS adapter path.
pub fn apply_tcp_timeouts(stream: &TcpStream, timeout: Duration) -> std::io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

/// Write a full request buffer through the macOS adapter TCP path.
pub fn write_all_tcp(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    std::io::Write::write_all(stream, bytes)
}

/// Read bytes through the macOS adapter TCP path.
pub fn read_tcp(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(stream, buf)
}

/// Resolve socket addresses through the macOS adapter path.
pub fn resolve_socket_addrs(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    (host, port).to_socket_addrs().map(Iterator::collect)
}

/// Check blocked-link metadata semantics through the macOS adapter path.
pub fn is_blocked_link_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Read filesystem metadata through the macOS adapter path.
pub fn stat_path(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

/// Read a directory iterator through the macOS adapter path.
pub fn read_dir(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(path)
}

/// Read symlink metadata through the macOS adapter path.
pub fn symlink_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

/// Canonicalize a filesystem path through the macOS adapter path.
pub fn canonicalize_path(path: &Path) -> std::io::Result<std::path::PathBuf> {
    path.canonicalize()
}

/// Open a filesystem path through the macOS adapter path.
pub fn open_path(path: &Path, opts: &mut OpenOptions) -> std::io::Result<std::fs::File> {
    opts.open(path)
}

/// Read from an open file through the macOS adapter path.
pub fn read_file(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(file, buf)
}

/// Write to an open file through the macOS adapter path.
pub fn write_file(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<usize> {
    std::io::Write::write(file, bytes)
}

/// Seek an open file through the macOS adapter path.
pub fn seek_file(file: &mut std::fs::File, pos: std::io::SeekFrom) -> std::io::Result<u64> {
    std::io::Seek::seek(file, pos)
}

/// Read metadata for an open file through the macOS adapter path.
pub fn file_metadata(file: &std::fs::File) -> std::io::Result<std::fs::Metadata> {
    file.metadata()
}

/// Read a full filesystem path through the macOS adapter path.
pub fn read_path(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Ensure a directory tree exists through the macOS adapter path.
pub fn create_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Remove a file through the macOS adapter path.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Remove an empty directory through the macOS adapter path.
pub fn remove_dir(path: &Path) -> std::io::Result<()> {
    std::fs::remove_dir(path)
}

/// Create a directory through the macOS adapter path.
pub fn create_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// Rename a filesystem path through the macOS adapter path.
pub fn rename_path(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Read the current locale through the macOS adapter path.
pub fn current_locale(locale: &HostLocale) -> LocaleId {
    locale.current()
}

/// Read the current timezone through the macOS adapter path.
pub fn timezone(locale: &HostLocale) -> String {
    locale.timezone()
}

/// Format a date through the macOS adapter path.
pub fn format_date(millis: u64, timezone: &str, style: DateStyle, locale: &LocaleId) -> String {
    HostLocale::format_date(millis, timezone, style, locale)
}

/// Format a number through the macOS adapter path.
pub fn format_number(value: f64, style: NumberStyle, locale: &LocaleId) -> String {
    HostLocale::format_number(value, style, locale)
}

/// Apply macOS no-follow-final-symlink open behavior.
pub fn apply_no_follow_final_symlink(opts: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        opts.custom_flags(libc::O_NOFOLLOW);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_lowering_skips_kinds_without_appkit_support() {
        use krate_adapter_common::ui::{
            UiAdapter, WidgetId, WidgetKind, WidgetPlacement, WindowAdapter, WindowOptions,
            WindowSize,
        };
        let adapter = MacosAppKitPrototypeUiAdapter::default();
        // A real AppKit window needs the main thread; the historical failure
        // happened while CONVERTING placements, before any session lookup,
        // so an id with no native session behind it exercises the fix.
        let size = WindowSize::new(640, 480).expect("size");
        let draft = krate_adapter_common::ui::DraftUiAdapter::default();
        let window = WindowAdapter::create_window(
            &draft,
            WindowOptions::new("Test", size).expect("options"),
        )
        .expect("create window");
        let checkbox = WidgetPlacement {
            widget: WidgetId::new(9).expect("id"),
            kind: WidgetKind::Checkbox,
            label: Some("robot was here".to_string()),
            checked: Some(false),
            value: None,
            selection: None,
            clip: None,
            clickable: false,
            x: 10.0,
            y: 10.0,
            width: 100.0,
            height: 20.0,
        };
        // No native session exists in tests; the call used to fail while
        // converting the unsupported kind, before even reaching a session.
        let lowered = UiAdapter::lower_widget_placements(&adapter, window, &[checkbox])
            .expect("unsupported kinds are skipped, not fatal");
        assert_eq!(lowered, 0);
    }

    use super::*;

    #[test]
    fn host_family_constant_matches_macos() {
        assert_eq!(HOST_FAMILY, "macos");
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
            .create_window(WindowOptions::new("Krate blank window", size).expect("options"))
            .expect("create window");

        adapter.show_window(id).expect("show");
        adapter.request_redraw(id).expect("redraw");

        let window = adapter.window(id).expect("lookup").expect("window");
        let info = adapter.info();
        let window_adapter: &dyn WindowAdapter = &adapter;
        let window_info = window_adapter.info();
        assert_eq!(info.host_family, HOST_FAMILY);
        assert_eq!(info.backend, "macos-headless-draft");
        assert_eq!(info.window_backend, WindowBackendKind::HeadlessDraft);
        assert_eq!(info.planned_window_backend, WindowBackendKind::AppKit);
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);
        assert_eq!(window_info, info);
        assert_eq!(adapter.backend_name(), "macos-headless-draft");
        assert!(!adapter.native_windows_enabled());
        assert_eq!(
            adapter.planned_native_window_backend(),
            WindowBackendKind::AppKit
        );
        assert_eq!(window.title, "Krate blank window");
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
    fn ui_adapter_can_bind_future_appkit_handle_to_window_id() {
        let adapter = discover_ui_adapter();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate native host", size).expect("options"))
            .expect("create window");
        let handle = adapter
            .attach_appkit_window_handle(id, 0xA11CE)
            .expect("attach appkit handle");

        assert_eq!(handle.backend, WindowBackendKind::AppKit);
        assert_eq!(handle.raw_handle, 0xA11CE);
        assert_eq!(adapter.native_window(id).expect("lookup"), Some(handle));
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::AppKit,
                },
            ]
        );
    }

    #[test]
    fn appkit_backend_is_exposed_but_not_default_yet() {
        let adapter = discover_ui_adapter();
        let backend = AppKitWindowBackend;
        assert_eq!(backend.backend_kind(), WindowBackendKind::AppKit);
        assert_eq!(
            adapter.planned_native_window_backend(),
            backend.backend_kind()
        );
        assert_eq!(backend.is_available(), cfg!(target_os = "macos"));
        assert!(!adapter.native_windows_enabled());
    }

    #[test]
    fn appkit_prototype_adapter_is_separate_from_default_adapter() {
        let default_adapter = discover_ui_adapter();
        let prototype_adapter = MacosAppKitPrototypeUiAdapter::new();
        let default_info = default_adapter.info();
        let prototype_info = prototype_adapter.info();

        assert_eq!(default_info.backend, "macos-headless-draft");
        assert_eq!(
            default_info.window_backend,
            WindowBackendKind::HeadlessDraft
        );
        assert!(!default_info.native_windows);
        assert!(!default_info.native_event_loop);

        assert_eq!(prototype_info.backend, "macos-appkit-prototype");
        assert_eq!(
            prototype_info.planned_window_backend,
            WindowBackendKind::AppKit
        );
        if cfg!(target_os = "macos") {
            assert_eq!(prototype_info.window_backend, WindowBackendKind::AppKit);
            assert!(prototype_info.native_windows);
            assert!(prototype_info.native_event_loop);
        } else {
            assert_eq!(
                prototype_info.window_backend,
                WindowBackendKind::HeadlessDraft
            );
            assert!(!prototype_info.native_windows);
            assert!(!prototype_info.native_event_loop);
        }
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
