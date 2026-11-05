//! Android host adapter surface for Krate Phase 2.
//!
//! This crate is the Android ownership boundary. Shared behavior still comes from
//! `krate-adapter-common`, while Android-specific host wiring lands here.

use krate_adapter_common::{
    locale::{DateStyle, HostLocale, LocaleId, NumberStyle},
    time::HostClock,
    ui::{
        DraftUiAdapter, KeyEvent, NativeWindowHandle, PointerEvent, RawPointerSample,
        TextChangedEvent, TextInputEvent, Theme, UiAdapter, UiAdapterError, UiAdapterInfo, UiEvent,
        UiEventLoopTick, WheelEvent, WidgetId, WidgetNode, WidgetPlacement, WidgetTree,
        WindowAdapter, WindowBackendKind, WindowId, WindowOptions, WindowRecord, WindowSize,
        WinitWindowEventCollector, WinitWindowEventLoopStep, WinitWindowEventLoopStepReport,
        WinitWindowNativeEvent, WinitWindowSession, WinitWindowSnapshot,
    },
};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::net::ToSocketAddrs;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// Host family handled by this adapter crate.
pub const HOST_FAMILY: &str = "android";

/// Linux Phase 3 UI adapter entry point.
///
/// This is still a headless draft adapter. It proves the Linux crate exposes
/// the same UI contract as macOS and Windows before the winit bridge lands.
pub mod winit_native;

mod appearance;
pub mod behind;
mod bluetooth;
pub mod camera;
mod clipboard;
pub mod dialog;
mod documents;
mod haptics;
#[cfg(target_os = "android")]
mod java;
mod location;
pub mod menu;
mod motion;
pub mod phone;
mod photos;
pub mod reminders;
mod share;
mod sign_in;
mod typing;
pub mod unlock;
pub mod video;
mod web_page;

#[derive(Debug, Default)]
pub struct AndroidUiAdapter {
    draft: DraftUiAdapter,
}

impl AndroidUiAdapter {
    /// Create the current Linux UI adapter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the backend used by this adapter build.
    pub fn backend_name(&self) -> &'static str {
        "android-headless-draft"
    }

    /// Return whether this build creates real native windows.
    pub fn native_windows_enabled(&self) -> bool {
        false
    }

    /// Return the native backend planned for this host.
    pub fn planned_native_window_backend(&self) -> WindowBackendKind {
        WindowBackendKind::Winit
    }

    /// Attach a future winit window handle to a Krate window id.
    ///
    /// Linux still uses the headless draft path by default. This method is the
    /// stable handoff point the first real winit backend will use once it owns
    /// an OS window.
    pub fn attach_winit_window_handle(
        &self,
        id: WindowId,
        raw_handle: u64,
    ) -> Result<NativeWindowHandle, UiAdapterError> {
        let handle = NativeWindowHandle::new(WindowBackendKind::Winit, raw_handle)?;
        WindowAdapter::attach_native_window(self, id, handle)?;
        Ok(handle)
    }
}

/// Opt-in Linux adapter boundary for the coming winit prototype.
///
/// It is separate from the default adapter on purpose. Until the real winit
/// window owner lands, it reports the planned backend without claiming native
/// window support.
#[derive(Debug, Default)]
pub struct AndroidWinitPrototypeUiAdapter {
    headless: AndroidUiAdapter,
    sessions: Mutex<BTreeMap<WindowId, WinitWindowSession>>,
    collectors: Mutex<BTreeMap<WindowId, WinitWindowEventCollector>>,
    /// The covered edges last seen, to notice the keyboard coming and going.
    edges: Mutex<typing::EdgeWatch>,
}

impl AndroidWinitPrototypeUiAdapter {
    /// Create the opt-in Linux winit prototype adapter boundary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Name the prototype backend path.
    pub fn backend_name(&self) -> &'static str {
        "android-winit-prototype"
    }

    /// Return whether this build can create native Linux windows.
    ///
    /// True in Linux builds: the winit backend is compiled in. Creating the
    /// event loop still needs a display server at call time; headless hosts
    /// get a clean `Unsupported` error from the first native call.
    pub fn native_windows_enabled(&self) -> bool {
        cfg!(target_os = "android")
    }

    /// Return whether this build has a native event-loop driver.
    pub fn native_event_loop_enabled(&self) -> bool {
        cfg!(target_os = "android")
    }

    /// Attach a tracked winit session to a Krate window id.
    ///
    /// This is still a prototype helper. Real winit code will call this after
    /// it creates the OS window and has a real raw handle to attach.
    pub fn attach_winit_session(
        &self,
        id: WindowId,
        raw_handle: u64,
        initial_snapshot: WinitWindowSnapshot,
    ) -> Result<NativeWindowHandle, UiAdapterError> {
        let handle = self.headless.attach_winit_window_handle(id, raw_handle)?;
        let session = WinitWindowSession::new(id, handle, initial_snapshot)?;
        self.sessions()?.insert(id, session);
        self.collectors()?
            .insert(id, WinitWindowEventCollector::new(id));
        Ok(handle)
    }

    /// Return a copy of a tracked winit session.
    pub fn winit_session(
        &self,
        id: WindowId,
    ) -> Result<Option<WinitWindowSession>, UiAdapterError> {
        Ok(self.sessions()?.get(&id).cloned())
    }

    /// Record one future winit callback for a tracked session.
    pub fn record_winit_native_event(
        &self,
        id: WindowId,
        event: WinitWindowNativeEvent,
    ) -> Result<bool, UiAdapterError> {
        if !self.sessions()?.contains_key(&id) {
            return Ok(false);
        }

        let mut collectors = self.collectors()?;
        let collector = collectors
            .entry(id)
            .or_insert_with(|| WinitWindowEventCollector::new(id));
        collector.push_event(event)?;
        Ok(true)
    }

    /// Return the number of collected callbacks waiting for this session.
    pub fn pending_winit_callbacks(&self, id: WindowId) -> Result<Option<usize>, UiAdapterError> {
        Ok(self
            .collectors()?
            .get(&id)
            .map(WinitWindowEventCollector::pending_callbacks))
    }

    /// Pump one prepared winit event-loop step through the shared event queue.
    pub fn pump_winit_event_loop_step(
        &self,
        id: WindowId,
        step: &WinitWindowEventLoopStep,
    ) -> Result<Option<WinitWindowEventLoopStepReport>, UiAdapterError> {
        let mut sessions = self.sessions()?;
        let Some(session) = sessions.get_mut(&id) else {
            return Ok(None);
        };

        session.pump_event_loop_once(&self.headless, step).map(Some)
    }

    /// Drain collected future winit callbacks and pump them through Krate.
    pub fn pump_collected_winit_events(
        &self,
        id: WindowId,
    ) -> Result<Option<WinitWindowEventLoopStepReport>, UiAdapterError> {
        let step = {
            let mut collectors = self.collectors()?;
            let Some(collector) = collectors.get_mut(&id) else {
                return Ok(None);
            };
            collector.drain_step()
        };

        self.pump_winit_event_loop_step(id, &step)
    }

    fn remove_session(&self, id: WindowId) {
        if let Ok(mut sessions) = self.sessions() {
            sessions.remove(&id);
        }
        if let Ok(mut collectors) = self.collectors() {
            collectors.remove(&id);
        }
    }

    fn sessions(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<WindowId, WinitWindowSession>>, UiAdapterError> {
        self.sessions.lock().map_err(|_| {
            UiAdapterError::Internal("android winit session lock is poisoned".to_string())
        })
    }

    fn collectors(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<WindowId, WinitWindowEventCollector>>, UiAdapterError> {
        self.collectors.lock().map_err(|_| {
            UiAdapterError::Internal("android winit callback lock is poisoned".to_string())
        })
    }
}

impl WindowAdapter for AndroidUiAdapter {
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

    fn queue_file_dropped(
        &self,
        id: WindowId,
        path: std::path::PathBuf,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_file_dropped(&self.draft, id, path)
    }

    fn queue_file_hovering(&self, id: WindowId, over: bool) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_file_hovering(&self.draft, id, over)
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

impl UiAdapter for AndroidUiAdapter {
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

    fn queue_wheel_event(&self, event: WheelEvent) -> Result<(), UiAdapterError> {
        self.draft.queue_wheel_event(event)
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

impl WindowAdapter for AndroidWinitPrototypeUiAdapter {
    fn info(&self) -> UiAdapterInfo {
        let native = self.native_windows_enabled();
        UiAdapterInfo::new(
            HOST_FAMILY,
            self.backend_name(),
            if native {
                WindowBackendKind::Winit
            } else {
                WindowBackendKind::HeadlessDraft
            },
            WindowBackendKind::Winit,
            native,
            self.native_event_loop_enabled(),
        )
    }

    fn create_window(&self, options: WindowOptions) -> Result<WindowId, UiAdapterError> {
        let id = WindowAdapter::create_window(&self.headless, options.clone()).map_err(|e| {
            eprintln!("krate-adapter: headless create failed: {e:?}");
            e
        })?;
        let (raw_handle, snapshot) =
            winit_native::create_native_window(id, &options.title, options.size).map_err(|e| {
                eprintln!("krate-adapter: native create failed: {e:?}");
                e
            })?;
        let actual = snapshot.size;
        self.attach_winit_session(id, raw_handle, snapshot)
            .map_err(|e| {
                eprintln!("krate-adapter: attach failed: {e:?}");
                e
            })?;
        // A phone window is the whole screen, whatever size was asked for.
        // The session starts from the real size, so a resize to it later
        // reads as "no change" and is skipped -- leaving the window record at
        // the requested size forever, and every frame laid out for a window
        // the phone does not have (stretched, then cropped and zoomed).
        // Bring the record to the real size here, once, at the source.
        if actual != options.size {
            WindowAdapter::queue_host_resize(&self.headless, id, actual)?;
        }
        Ok(id)
    }

    fn show_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        winit_native::show_native_window(id)?;
        WindowAdapter::show_window(&self.headless, id)
    }

    fn close_window(&self, id: WindowId) -> Result<(), UiAdapterError> {
        let _ = winit_native::close_native_window(id);
        // The app is done with any folder it was given: what it changed
        // goes back to the real folder (K-1032).
        documents::put_back_folders();
        // Sensors stop with the app, so they cost nothing after it.
        motion::stop();
        location::stop();
        // Nothing stays connected after the app.
        bluetooth::disconnect_all();
        // Its web pages go with it, and what they kept.
        web_page::close_all();
        #[cfg(target_os = "android")]
        camera::close();
        // The keyboard goes with the window it typed into.
        #[cfg(target_os = "android")]
        typing::set(id, false, false);
        self.remove_session(id);
        WindowAdapter::close_window(&self.headless, id)
    }

    fn set_title(&self, id: WindowId, title: String) -> Result<(), UiAdapterError> {
        if winit_native::has_native_window(id).unwrap_or(false) {
            winit_native::set_native_window_title(id, &title)?;
        }
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

    fn queue_file_dropped(
        &self,
        id: WindowId,
        path: std::path::PathBuf,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_file_dropped(&self.headless, id, path)
    }

    fn queue_file_hovering(&self, id: WindowId, over: bool) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_file_hovering(&self.headless, id, over)
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

impl UiAdapter for AndroidWinitPrototypeUiAdapter {
    fn window_scale(&self, id: WindowId) -> f32 {
        winit_native::window_scale(id)
    }

    fn drain_raw_pointer_input(&self) -> Vec<RawPointerSample> {
        winit_native::drain_pointer_samples()
    }

    /// Where the pointer is and whether it is held (krate:ui/pointer).
    fn pointer(&self, window: WindowId) -> krate_adapter_common::ui::PointerTracker {
        winit_native::pointer_state(window)
    }

    fn take_pointer_motion(&self, window: WindowId) -> (f32, f32) {
        winit_native::take_pointer_motion(window)
    }

    /// A phone or tablet is touched, and its status bar, gesture bar and
    /// camera cutout cover its edges (K-1039): reported in points, so an app
    /// keeps its text out from under them.
    fn screen_kind(&self, window: WindowId) -> krate_adapter_common::ui::ScreenKind {
        let scale = winit_native::window_scale(window).max(0.25);
        let (top, right, bottom, left) = phone::covered_edges();
        // Over an app, the strip under the clock is Krate's: the hold that
        // opens its menu lives there (IC-934).
        let top = if menu::chrome_on() {
            (top / scale).max(20.0) + menu::MENU_BAND
        } else {
            top / scale
        };
        krate_adapter_common::ui::ScreenKind {
            touch: true,
            safe: (top, right / scale, bottom / scale, left / scale),
        }
    }

    /// A widget-tree app is laid out below the status bar and Krate's menu
    /// strip and above the gesture bar: the same covered edges krate:ui/screen
    /// reports to apps that draw themselves (K-1060).
    fn widget_insets(&self, window: WindowId) -> (f32, f32, f32, f32) {
        if !cfg!(target_os = "android") {
            return (0.0, 0.0, 0.0, 0.0);
        }
        self.screen_kind(window).safe
    }

    /// What the app drew, as it described it to the screen reader (IC-926):
    /// kept so a finger on something adjustable drags it (K-1059). Android's
    /// TalkBack does not read these yet.
    fn describe_accessibility(
        &self,
        _window: WindowId,
        elements: &[krate_adapter_common::ui::AccessElement],
    ) -> Result<(), UiAdapterError> {
        winit_native::set_described(elements);
        Ok(())
    }

    /// A web page in a box of the app (IC-927): a WebView over it.
    fn open_web_page(
        &self,
        window: WindowId,
        address: &str,
        frame: (f32, f32, f32, f32),
        granted: Vec<(String, String)>,
    ) -> Result<u64, String> {
        if !winit_native::has_native_window(window).unwrap_or(false) {
            return Err("the app's window is not on screen".to_string());
        }
        web_page::open(address, frame, winit_native::window_scale(window), granted)
    }

    fn place_web_page(&self, page: u64, frame: (f32, f32, f32, f32)) {
        web_page::place(page, frame);
    }

    fn web_page_go(&self, page: u64, address: &str) {
        web_page::go(page, address);
    }

    fn web_page_back(&self, page: u64) {
        web_page::back(page);
    }

    fn web_page_state(&self, page: u64) -> Option<krate_adapter_common::ui::WebPageState> {
        web_page::state(page)
    }

    fn close_web_page(&self, page: u64) {
        web_page::close(page);
    }

    /// Every finger on the window (krate:ui/touch).
    fn touches(&self, window: WindowId) -> Vec<(u64, f32, f32)> {
        winit_native::touches(window)
    }

    fn drain_raw_key_input(&self) -> Vec<krate_adapter_common::ui::RawKeySample> {
        #[allow(unused_mut)]
        let mut samples = winit_native::drain_key_samples();
        // What the on-screen keyboard typed, after the hardware keys.
        #[cfg(target_os = "android")]
        samples.extend(typing::drain());
        samples
    }

    /// The on-screen keyboard, for an app that draws its own text field
    /// (krate:ui/text-entry, K-1024).
    fn set_text_entry(&self, window: WindowId, on: bool) -> bool {
        #[cfg(target_os = "android")]
        {
            typing::set(window, on, false)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (window, on);
            false
        }
    }

    /// Whether TalkBack (or another screen reader that explores by touch)
    /// is on (IC-926).
    fn screen_reader_on(&self) -> bool {
        phone::screen_reader_on()
    }

    /// An app that lays itself out for any screen follows the way the phone
    /// is held; any other keeps the shape of the window it asked for
    /// (IC-912).
    #[cfg(target_os = "android")]
    fn ask(&self, _window: WindowId, title: &str, body: &str, question: bool) -> Option<bool> {
        if question {
            Some(dialog::confirm(title, body))
        } else {
            dialog::message(title, body);
            Some(false)
        }
    }

    fn set_canvas_presses(&self, window: WindowId, widget: WidgetId, presses: bool) {
        winit_native::set_canvas_presses(window, widget, presses);
    }

    fn set_adapts(&self, adapts: bool) {
        winit_native::set_adapts(adapts);
    }

    fn drain_raw_wheel_input(&self) -> Vec<krate_adapter_common::ui::RawWheelSample> {
        winit_native::drain_wheel_samples()
    }

    fn lower_widget_placements(
        &self,
        window: WindowId,
        placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        if winit_native::has_native_window(window).unwrap_or(false) {
            return winit_native::set_drawn_placements(window, placements);
        }
        Ok(0)
    }

    fn set_root(&self, window: WindowId, root: WidgetNode) -> Result<(), UiAdapterError> {
        self.headless.set_root(window, root)
    }

    fn upsert_node(&self, window: WindowId, node: WidgetNode) -> Result<(), UiAdapterError> {
        self.headless.upsert_node(window, node)
    }

    fn remove_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.headless.remove_node(window, widget)
    }

    fn focus_node(&self, window: WindowId, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.headless.focus_node(window, widget)?;
        // Focus on a text field brings the phone's keyboard up; focus moving
        // to anything else puts it away (K-1024).
        #[cfg(target_os = "android")]
        if winit_native::has_native_window(window).unwrap_or(false) {
            let kind = self
                .headless
                .widget_tree(window)?
                .and_then(|tree| tree.node(widget).map(|node| node.kind));
            match kind {
                Some(kind) if typing::takes_typing(kind) => {
                    typing::set(
                        window,
                        true,
                        kind == krate_adapter_common::ui::WidgetKind::TextArea,
                    );
                }
                _ => {
                    typing::set(window, false, false);
                }
            }
        }
        Ok(())
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

    fn queue_wheel_event(&self, event: WheelEvent) -> Result<(), UiAdapterError> {
        self.headless.queue_wheel_event(event)
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

    // The drawn path paints its own text, so Cut/Copy/Paste can only work if
    // the guest reaches the real OS clipboard. macOS never gets here — its
    // NSTextField owns Cut/Copy/Paste natively — so this lives on the drawn
    // adapters alone. Capability gating (`ui.clipboard:read`/`write`) happens
    // in the runtime before either call is dispatched.
    fn read_clipboard_text(&self) -> Result<String, UiAdapterError> {
        clipboard::read_text()
    }

    fn write_clipboard_text(&self, text: &str) -> Result<(), UiAdapterError> {
        clipboard::write_text(text)
    }

    /// The system's document picker for a file or a folder, copied in as a
    /// path of Krate's own (K-1032).
    fn choose_path(
        &self,
        _window: WindowId,
        folder: bool,
        filter: &str,
    ) -> Option<std::path::PathBuf> {
        documents::choose(folder, filter)
    }

    /// A reminder that comes with the app closed (IC-921).
    #[cfg(target_os = "android")]
    fn reminder_schedule(
        &self,
        id: &str,
        sender: &str,
        title: &str,
        body: &str,
        after_seconds: u64,
        daily: bool,
    ) -> Result<(), String> {
        reminders::schedule(id, sender, title, body, after_seconds, daily)
    }

    #[cfg(target_os = "android")]
    fn reminder_cancel(&self, id: &str) {
        reminders::cancel(id)
    }

    #[cfg(target_os = "android")]
    fn reminder_pending(&self) -> Vec<String> {
        reminders::pending()
    }

    /// The system's photo picker; the photos picked, copied in (IC-920).
    #[cfg(target_os = "android")]
    fn pick_photos(
        &self,
        _window: WindowId,
        limit: u32,
    ) -> Option<Result<Vec<std::path::PathBuf>, String>> {
        Some(photos::pick(limit))
    }

    /// Add a PNG or JPEG to Pictures/Krate (IC-920).
    #[cfg(target_os = "android")]
    fn save_photo(&self, data: &[u8]) -> Result<(), Option<String>> {
        photos::save(data)
    }

    /// The system's share sheet with the app's text, link and files; true
    /// when the person sent them to an app (IC-919).
    #[cfg(target_os = "android")]
    fn share(
        &self,
        _window: WindowId,
        text: Option<&str>,
        url: Option<&str>,
        files: &[(String, Vec<u8>)],
    ) -> Result<bool, String> {
        share::share(text, url, files)
    }

    /// The service's sign-in page in the browser, and the address it sent
    /// the person back to (IC-923).
    #[cfg(target_os = "android")]
    fn sign_in(
        &self,
        _window: WindowId,
        authorize_url: &str,
        scheme: &str,
    ) -> Result<Option<String>, String> {
        sign_in::sign_in(authorize_url, scheme)
    }

    /// Dark mode, more contrast and text size, as the person set them
    /// (IC-925).
    #[cfg(target_os = "android")]
    fn appearance(&self) -> krate_adapter_common::ui::Appearance {
        appearance::read()
    }

    /// Whether the phone has Bluetooth (IC-917).
    #[cfg(target_os = "android")]
    fn bluetooth_available(&self) -> bool {
        bluetooth::available()
    }

    /// Krate's own list of nearby devices offering `service`, and the one
    /// the person picks, connected; the runtime has checked the grant.
    #[cfg(target_os = "android")]
    fn bluetooth_choose(
        &self,
        _window: WindowId,
        service: &str,
        title: &str,
    ) -> Result<Option<krate_adapter_common::ui::BtDevice>, krate_adapter_common::ui::BtError> {
        bluetooth::choose(service, title)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_read(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
    ) -> Result<Vec<u8>, krate_adapter_common::ui::BtError> {
        bluetooth::read(device, service, characteristic)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_write(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
        data: &[u8],
        with_response: bool,
    ) -> Result<(), krate_adapter_common::ui::BtError> {
        bluetooth::write(device, service, characteristic, data, with_response)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_subscribe(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
    ) -> Result<(), krate_adapter_common::ui::BtError> {
        bluetooth::subscribe(device, service, characteristic)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_notifications(
        &self,
        device: u64,
    ) -> Vec<krate_adapter_common::ui::BtNotification> {
        bluetooth::notifications(device)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_connected(&self, device: u64) -> bool {
        bluetooth::connected(device)
    }

    #[cfg(target_os = "android")]
    fn bluetooth_disconnect(&self, device: u64) {
        bluetooth::disconnect(device)
    }

    /// Whether the phone can tell where it is (IC-917).
    fn location_available(&self) -> bool {
        location::available()
    }

    /// The latest fix; the runtime has already checked `location.read`.
    fn location(&self) -> krate_adapter_common::ui::LocationAnswer {
        location::read()
    }

    /// Whether the phone has motion sensors (IC-917).
    fn motion_available(&self) -> bool {
        motion::available()
    }

    /// The latest reading in the screen's frame, sensors started on the
    /// first ask.
    fn motion(&self) -> Option<krate_adapter_common::ui::MotionReading> {
        motion::read()
    }

    /// Whether the phone has a vibrator to tap back with (IC-917).
    fn haptics_available(&self) -> bool {
        haptics::available()
    }

    /// Tap back: one of the seven feels, or an impact at `strength`.
    fn haptic(&self, feel: Option<krate_adapter_common::ui::HapticFeel>, strength: f32) -> bool {
        haptics::play(feel, strength)
    }

    /// A save the app wrote, to the place the person picks in the system's
    /// "save to" picker (K-1032).
    fn export_file(&self, _window: WindowId, path: &Path) -> bool {
        documents::export(path)
    }

    fn pump_event_loop_once(
        &self,
        window: WindowId,
    ) -> Result<Option<UiEventLoopTick>, UiAdapterError> {
        if winit_native::has_native_window(window).unwrap_or(false) {
            for (target, event) in winit_native::pump_native_events()? {
                self.record_winit_native_event(target, event)?;
            }
            // Leaving Krate, the person may go and look at a folder they
            // gave the app: what it changed there is put back now, not only
            // when it closes (K-1032).
            if winit_native::take_backgrounded() {
                documents::put_back_folders();
            }
            // The keyboard sliding up (or the phone's bars changing) moves
            // the edges an app must keep clear: lay the window out again
            // above them, as a phone's own toolkit does, even though its
            // size did not change.
            if self
                .edges
                .lock()
                .map(|mut watch| watch.changed(phone::covered_edges()))
                .unwrap_or(false)
            {
                if let Some(record) = WindowAdapter::window(&self.headless, window)? {
                    WindowAdapter::queue_host_resize(&self.headless, window, record.size)?;
                }
            }
            winit_native::redraw_all()?;
        }
        Ok(self
            .pump_collected_winit_events(window)?
            .map(|report| UiEventLoopTick {
                window,
                callbacks_handled: report.callbacks_handled,
                snapshot_refreshed: report.snapshot.is_some(),
                redraw_requested: report.redraw_requested,
            }))
    }
}

/// Build the current Linux UI adapter.
pub fn discover_ui_adapter() -> AndroidUiAdapter {
    AndroidUiAdapter::new()
}

/// Build the opt-in Linux winit prototype UI adapter when it is ready.
pub fn discover_winit_prototype_ui_adapter(
) -> Result<AndroidWinitPrototypeUiAdapter, UiAdapterError> {
    let adapter = AndroidWinitPrototypeUiAdapter::new();
    if !adapter.native_windows_enabled() {
        return Err(UiAdapterError::Unsupported(
            "Linux winit prototype UI adapter is not enabled yet".to_string(),
        ));
    }
    Ok(adapter)
}

/// Resolve locale and timezone for Linux host runs.
pub fn discover_locale(
    locale_override: Option<&str>,
    timezone_override: Option<&str>,
) -> HostLocale {
    HostLocale::from_env_with_overrides(locale_override, timezone_override)
}

/// Build the Linux host clock surface.
pub fn discover_clock(test_time_millis: Option<u64>) -> HostClock {
    HostClock::new(test_time_millis)
}

/// Sleep through the Linux host adapter path.
pub fn sleep_millis(millis: u32) {
    HostClock::sleep_millis(millis);
}

/// Read from host stdin through the Linux adapter path.
pub fn read_stdin(buf: &mut [u8]) -> std::io::Result<usize> {
    let mut stdin = std::io::stdin();
    std::io::Read::read(&mut stdin, buf)
}

/// Write bytes to host stderr through the Linux adapter path.
pub fn write_stderr(bytes: &[u8]) -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::write_all(&mut stderr, bytes)
}

/// Flush host stderr through the Linux adapter path.
pub fn flush_stderr() -> std::io::Result<()> {
    let mut stderr = std::io::stderr();
    std::io::Write::flush(&mut stderr)
}

/// Print a line to host stdout through the Linux adapter path.
pub fn print_stdout_line(msg: &str) {
    println!("{msg}");
}

/// Write bytes to host stdout through the Linux adapter path.
pub fn write_stdout(bytes: &[u8]) -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::write_all(&mut stdout, bytes)
}

/// Flush host stdout through the Linux adapter path.
pub fn flush_stdout() -> std::io::Result<()> {
    let mut stdout = std::io::stdout();
    std::io::Write::flush(&mut stdout)
}

/// Open a TCP stream through the Linux adapter path.
pub fn connect_tcp(addr: SocketAddr, timeout: Option<Duration>) -> std::io::Result<TcpStream> {
    match timeout {
        Some(timeout) => TcpStream::connect_timeout(&addr, timeout),
        None => TcpStream::connect(addr),
    }
}

/// Apply read/write timeouts through the Linux adapter path.
pub fn apply_tcp_timeouts(stream: &TcpStream, timeout: Duration) -> std::io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

/// Write a full request buffer through the Linux adapter TCP path.
pub fn write_all_tcp(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    std::io::Write::write_all(stream, bytes)
}

/// Read bytes through the Linux adapter TCP path.
pub fn read_tcp(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(stream, buf)
}

/// Resolve socket addresses through the Linux adapter path.
pub fn resolve_socket_addrs(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    (host, port).to_socket_addrs().map(Iterator::collect)
}

/// Check blocked-link metadata semantics through the Linux adapter path.
pub fn is_blocked_link_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Read filesystem metadata through the Linux adapter path.
pub fn stat_path(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

/// Read a directory iterator through the Linux adapter path.
pub fn read_dir(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(path)
}

/// Read symlink metadata through the Linux adapter path.
pub fn symlink_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

/// Canonicalize a filesystem path through the Linux adapter path.
pub fn canonicalize_path(path: &Path) -> std::io::Result<std::path::PathBuf> {
    path.canonicalize()
}

/// Open a filesystem path through the Linux adapter path.
pub fn open_path(path: &Path, opts: &mut OpenOptions) -> std::io::Result<std::fs::File> {
    opts.open(path)
}

/// Read from an open file through the Linux adapter path.
pub fn read_file(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    std::io::Read::read(file, buf)
}

/// Write to an open file through the Linux adapter path.
pub fn write_file(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<usize> {
    std::io::Write::write(file, bytes)
}

/// Seek an open file through the Linux adapter path.
pub fn seek_file(file: &mut std::fs::File, pos: std::io::SeekFrom) -> std::io::Result<u64> {
    std::io::Seek::seek(file, pos)
}

/// Read metadata for an open file through the Linux adapter path.
pub fn file_metadata(file: &std::fs::File) -> std::io::Result<std::fs::Metadata> {
    file.metadata()
}

/// Read a full filesystem path through the Linux adapter path.
pub fn read_path(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Ensure a directory tree exists through the Linux adapter path.
pub fn create_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Remove a file through the Linux adapter path.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Remove an empty directory through the Linux adapter path.
pub fn remove_dir(path: &Path) -> std::io::Result<()> {
    std::fs::remove_dir(path)
}

/// Create a directory through the Linux adapter path.
pub fn create_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

/// Rename a filesystem path through the Linux adapter path.
pub fn rename_path(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Read the current locale through the Linux adapter path.
pub fn current_locale(locale: &HostLocale) -> LocaleId {
    locale.current()
}

/// Read the current timezone through the Linux adapter path.
pub fn timezone(locale: &HostLocale) -> String {
    locale.timezone()
}

/// Format a date through the Linux adapter path.
pub fn format_date(millis: u64, timezone: &str, style: DateStyle, locale: &LocaleId) -> String {
    HostLocale::format_date(millis, timezone, style, locale)
}

/// Format a number through the Linux adapter path.
pub fn format_number(value: f64, style: NumberStyle, locale: &LocaleId) -> String {
    HostLocale::format_number(value, style, locale)
}

/// Apply Linux no-follow-final-symlink open behavior.
pub fn apply_no_follow_final_symlink(opts: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        opts.custom_flags(libc::O_NOFOLLOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_family_constant_matches_android() {
        assert_eq!(HOST_FAMILY, "android");
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
        assert_eq!(info.backend, "android-headless-draft");
        assert_eq!(info.window_backend, WindowBackendKind::HeadlessDraft);
        assert_eq!(info.planned_window_backend, WindowBackendKind::Winit);
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);
        assert_eq!(window_info, info);
        assert_eq!(adapter.backend_name(), "android-headless-draft");
        assert!(!adapter.native_windows_enabled());
        assert_eq!(
            adapter.planned_native_window_backend(),
            WindowBackendKind::Winit
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
    fn ui_adapter_can_bind_future_winit_handle_to_window_id() {
        let adapter = discover_ui_adapter();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate native host", size).expect("options"))
            .expect("create window");
        let handle = adapter
            .attach_winit_window_handle(id, 0xB17E)
            .expect("attach winit handle");

        assert_eq!(handle.backend, WindowBackendKind::Winit);
        assert_eq!(handle.raw_handle, 0xB17E);
        assert_eq!(adapter.native_window(id).expect("lookup"), Some(handle));
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
            ]
        );
    }

    #[test]
    fn winit_prototype_adapter_is_separate_from_default_adapter() {
        let default_adapter = discover_ui_adapter();
        let prototype_adapter = AndroidWinitPrototypeUiAdapter::new();
        let default_info = default_adapter.info();
        let prototype_info = prototype_adapter.info();

        assert_eq!(default_info.backend, "android-headless-draft");
        assert_eq!(
            default_info.window_backend,
            WindowBackendKind::HeadlessDraft
        );
        assert!(!default_info.native_windows);
        assert!(!default_info.native_event_loop);

        assert_eq!(prototype_info.backend, "android-winit-prototype");
        if cfg!(target_os = "android") {
            assert_eq!(prototype_info.window_backend, WindowBackendKind::Winit);
        } else {
            assert_eq!(
                prototype_info.window_backend,
                WindowBackendKind::HeadlessDraft
            );
        }
        assert_eq!(
            prototype_info.planned_window_backend,
            WindowBackendKind::Winit
        );
        assert_eq!(prototype_info.native_windows, cfg!(target_os = "android"));
        assert_eq!(
            prototype_info.native_event_loop,
            cfg!(target_os = "android")
        );
        if cfg!(target_os = "android") {
            assert!(discover_winit_prototype_ui_adapter().is_ok());
        } else {
            assert!(matches!(
                discover_winit_prototype_ui_adapter(),
                Err(UiAdapterError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn winit_prototype_tracks_session_and_pumps_step() {
        let adapter = AndroidWinitPrototypeUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        // Allocate the id headless: this test exercises the session and pump
        // plumbing with a fake handle, not real winit window creation (which
        // needs a display server and is covered by the ignored native smoke).
        let id = adapter
            .headless
            .create_window(WindowOptions::new("Krate winit host", size).expect("options"))
            .expect("create window");
        let snapshot =
            WinitWindowSnapshot::new(id, size, false, false, 1.0).expect("initial snapshot");
        let handle = adapter
            .attach_winit_session(id, 0xB17E, snapshot)
            .expect("attach session");
        let resized = WindowSize::new(900, 700).expect("resized");
        let step = WinitWindowEventLoopStep::new().with_callbacks([
            krate_adapter_common::ui::WinitWindowNativeEvent::Focused(true),
            krate_adapter_common::ui::WinitWindowNativeEvent::Resized(resized),
            krate_adapter_common::ui::WinitWindowNativeEvent::RedrawRequested,
        ]);

        let report = adapter
            .pump_winit_event_loop_step(id, &step)
            .expect("pump")
            .expect("session report");

        assert_eq!(handle.backend, WindowBackendKind::Winit);
        assert_eq!(report.callbacks_handled, 3);
        assert_eq!(
            report.snapshot,
            Some(WinitWindowSnapshot::new(id, resized, false, true, 1.0).expect("snapshot"))
        );
        assert!(report.redraw_requested);
        assert_eq!(
            adapter
                .winit_session(id)
                .expect("session")
                .expect("tracked session")
                .last_snapshot()
                .size,
            resized
        );
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
            ]
        );

        adapter.close_window(id).expect("close");
        assert!(adapter.winit_session(id).expect("session").is_none());
    }

    /// Real winit window round trip. Needs a display server; run with
    /// `KRATE_WINIT_NATIVE_SMOKE=1 cargo test -p krate-adapter-linux -- --ignored`
    /// (under `xvfb-run` on headless hosts).
    #[test]
    #[ignore = "needs a display server; opt-in native smoke"]
    fn winit_prototype_native_window_smoke() {
        if std::env::var("KRATE_WINIT_NATIVE_SMOKE").as_deref() != Ok("1") {
            eprintln!("skipping: KRATE_WINIT_NATIVE_SMOKE not set");
            return;
        }
        let adapter = AndroidWinitPrototypeUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .create_window(WindowOptions::new("Krate winit native smoke", size).expect("options"))
            .expect("create native window");
        assert!(winit_native::has_native_window(id).expect("native window tracked"));
        adapter.show_window(id).expect("show native window");
        let tick = adapter
            .pump_event_loop_once(id)
            .expect("pump native events");
        assert!(tick.is_some());
        adapter.close_window(id).expect("close native window");
        assert!(!winit_native::has_native_window(id).unwrap_or(true));
    }

    #[test]
    fn winit_prototype_collects_callbacks_for_event_loop_pump() {
        let adapter = AndroidWinitPrototypeUiAdapter::new();
        let size = WindowSize::new(640, 480).expect("size");
        let id = adapter
            .headless
            .create_window(WindowOptions::new("Krate winit collected", size).expect("options"))
            .expect("create window");
        let snapshot =
            WinitWindowSnapshot::new(id, size, false, false, 1.0).expect("initial snapshot");
        adapter
            .attach_winit_session(id, 0xC011EC7, snapshot)
            .expect("attach session");
        let resized = WindowSize::new(1000, 720).expect("resized");

        assert_eq!(
            adapter.pending_winit_callbacks(id).expect("pending"),
            Some(0)
        );
        assert!(adapter
            .record_winit_native_event(id, WinitWindowNativeEvent::Focused(true))
            .expect("record focus"));
        assert!(adapter
            .record_winit_native_event(id, WinitWindowNativeEvent::Resized(resized))
            .expect("record resize"));
        assert!(adapter
            .record_winit_native_event(id, WinitWindowNativeEvent::RedrawRequested)
            .expect("record redraw"));
        assert_eq!(
            adapter.pending_winit_callbacks(id).expect("pending"),
            Some(3)
        );

        let tick = adapter
            .pump_event_loop_once(id)
            .expect("pump")
            .expect("native tick");

        assert_eq!(tick.window, id);
        assert_eq!(tick.callbacks_handled, 3);
        assert!(tick.snapshot_refreshed);
        assert!(tick.redraw_requested);
        assert_eq!(
            adapter.pending_winit_callbacks(id).expect("pending"),
            Some(0)
        );
        assert_eq!(
            adapter.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::NativeWindowAttached {
                    id,
                    backend: WindowBackendKind::Winit,
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
            ]
        );

        adapter.close_window(id).expect("close");
        assert_eq!(adapter.pending_winit_callbacks(id).expect("pending"), None);
        assert!(!adapter
            .record_winit_native_event(id, WinitWindowNativeEvent::RedrawRequested)
            .expect("closed session"));
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
