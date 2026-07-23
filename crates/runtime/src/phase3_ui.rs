//! Phase 3 UI dispatcher scaffold.
//!
//! This module is the first runtime-facing Phase 3 UI boundary. It still uses
//! the shared UI adapter trait from `adapter-common`; native AppKit, Win32, and
//! GTK windows come later.

use krate_adapter_common::ui::{
    KeyEvent, Modifiers, PointerButton, PointerEvent, TextInputEvent, Theme, UiAdapter,
    UiAdapterError, UiAdapterInfo, UiEvent, UiEventLoopTick, WidgetId, WidgetNode, WidgetTree,
    WindowId, WindowOptions, WindowRecord, WindowSize,
};
use krate_layout::{
    compute_layout, hit_test, LayoutPoint, LayoutSnapshot, LayoutViewport, PreparedLayoutTree,
};
use thiserror::Error;

use crate::uapi::{UapiCall, UapiError, UapiGuard, UiCall};

pub type UiDispatchResult<T> = std::result::Result<T, UiDispatchError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiDispatchError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("UI adapter error: {0}")]
    Adapter(#[from] UiAdapterError),
    #[error("layout error: {0}")]
    Layout(String),
    #[error("policy error: {0}")]
    Policy(String),
    #[error("operation is not implemented in the Phase 3 draft UI dispatcher yet")]
    Unsupported,
}

pub struct Phase3UiDispatcher<'a> {
    guard: &'a UapiGuard,
    adapter: &'a dyn UiAdapter,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerRouteRequest {
    pub window: WindowId,
    pub viewport: LayoutViewport,
    pub x: f32,
    pub y: f32,
    pub button: Option<PointerButton>,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteRequest {
    pub window: WindowId,
    pub key: String,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputRouteRequest {
    pub window: WindowId,
    pub text: String,
}

/// Host UI backend mode requested by the Phase 3 runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase3HostUiMode {
    /// Use the current safe host adapter. Today this is the headless draft path.
    HeadlessDraft,
    /// Use the first opt-in native prototype path when the host supports it.
    NativePrototype,
}

/// Owns the Phase 3 UI guard and host adapter for one runtime session.
pub struct Phase3UiRuntime {
    guard: UapiGuard,
    adapter: Box<dyn UiAdapter>,
    host_mode: Phase3HostUiMode,
}

impl Phase3UiRuntime {
    /// Build a UI runtime with an explicit adapter, mainly for tests.
    pub fn new(guard: UapiGuard, adapter: Box<dyn UiAdapter>) -> Self {
        Self {
            guard,
            adapter,
            host_mode: Phase3HostUiMode::HeadlessDraft,
        }
    }

    /// Build a UI runtime with an explicit adapter and recorded host mode.
    pub fn with_adapter_mode(
        guard: UapiGuard,
        adapter: Box<dyn UiAdapter>,
        host_mode: Phase3HostUiMode,
    ) -> Self {
        Self {
            guard,
            adapter,
            host_mode,
        }
    }

    /// Build a UI runtime using the current host adapter entry point.
    pub fn with_host_adapter(guard: UapiGuard) -> Self {
        Self::with_adapter_mode(
            guard,
            discover_host_ui_adapter(),
            Phase3HostUiMode::HeadlessDraft,
        )
    }

    /// Build a UI runtime using an explicitly requested host adapter mode.
    pub fn try_with_host_adapter_mode(
        guard: UapiGuard,
        host_mode: Phase3HostUiMode,
    ) -> UiDispatchResult<Self> {
        Ok(Self::with_adapter_mode(
            guard,
            discover_host_ui_adapter_for_mode(host_mode)?,
            host_mode,
        ))
    }

    /// Return a dispatcher that checks policy before each adapter call.
    pub fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        Phase3UiDispatcher::new(&self.guard, self.adapter.as_ref())
    }

    /// Return the selected adapter information.
    pub fn adapter_info(&self) -> UiAdapterInfo {
        self.adapter.info()
    }

    /// Return the host UI mode requested for this runtime.
    pub fn host_mode(&self) -> Phase3HostUiMode {
        self.host_mode
    }
}

impl<'a> Phase3UiDispatcher<'a> {
    pub fn new(guard: &'a UapiGuard, adapter: &'a dyn UiAdapter) -> Self {
        Self { guard, adapter }
    }

    pub fn adapter_info(&self) -> UiAdapterInfo {
        self.adapter.info()
    }

    pub fn create_window(&self, options: WindowOptions) -> UiDispatchResult<WindowId> {
        self.check_window_access()?;
        Ok(self.adapter.create_window(options)?)
    }

    pub fn show_window(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.show_window(id)?;
        Ok(())
    }

    pub fn close_window(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.close_window(id)?;
        Ok(())
    }

    pub fn set_title(&self, id: WindowId, title: impl Into<String>) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_title(id, title.into())?;
        Ok(())
    }

    pub fn set_size(&self, id: WindowId, size: WindowSize) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_size(id, size)?;
        Ok(())
    }

    pub fn request_redraw(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.request_redraw(id)?;
        Ok(())
    }

    pub fn set_root(&self, window: WindowId, root: WidgetNode) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_root(window, root)?;
        Ok(())
    }

    /// Lower widget placements to native host controls after the UI check.
    ///
    /// Returns how many widgets the host lowered natively; headless and
    /// drawn-only adapters return zero, which is a valid state.
    pub fn lower_widget_placements(
        &self,
        window: WindowId,
        placements: &[krate_adapter_common::ui::WidgetPlacement],
    ) -> UiDispatchResult<usize> {
        self.check_window_access()?;
        Ok(self.adapter.lower_widget_placements(window, placements)?)
    }

    pub fn upsert_node(&self, window: WindowId, node: WidgetNode) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.upsert_node(window, node)?;
        Ok(())
    }

    pub fn remove_node(&self, window: WindowId, widget: WidgetId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.remove_node(window, widget)?;
        Ok(())
    }

    pub fn focus_node(&self, window: WindowId, widget: WidgetId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.focus_node(window, widget)?;
        Ok(())
    }

    pub fn read_clipboard_text(&self) -> UiDispatchResult<String> {
        self.check(&UapiCall::Ui(UiCall::ClipboardRead))?;
        self.adapter.read_clipboard_text().map_err(Into::into)
    }

    pub fn write_clipboard_text(&self, text: &str) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::ClipboardWrite))?;
        self.adapter.write_clipboard_text(text).map_err(Into::into)
    }

    pub fn window(&self, id: WindowId) -> UiDispatchResult<Option<WindowRecord>> {
        Ok(self.adapter.window(id)?)
    }

    pub fn widget_tree(&self, window: WindowId) -> UiDispatchResult<Option<WidgetTree>> {
        Ok(self.adapter.widget_tree(window)?)
    }

    pub fn focused_widget(&self, window: WindowId) -> UiDispatchResult<Option<WidgetId>> {
        Ok(self.adapter.focused_widget(window)?)
    }

    pub fn compute_layout(
        &self,
        window: WindowId,
        viewport: LayoutViewport,
    ) -> UiDispatchResult<LayoutSnapshot> {
        self.check_window_access()?;
        let tree = self
            .adapter
            .widget_tree(window)?
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;

        compute_layout(&tree, viewport).map_err(|err| UiDispatchError::Layout(err.to_string()))
    }

    pub fn prepare_layout(&self, window: WindowId) -> UiDispatchResult<PreparedLayoutTree> {
        self.check_window_access()?;
        let tree = self
            .adapter
            .widget_tree(window)?
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;

        PreparedLayoutTree::new(&tree).map_err(|err| UiDispatchError::Layout(err.to_string()))
    }

    pub fn route_pointer_event(
        &self,
        request: PointerRouteRequest,
    ) -> UiDispatchResult<Option<WidgetId>> {
        self.check_window_access()?;
        let point = LayoutPoint::new(request.x, request.y)
            .map_err(|err| UiDispatchError::Layout(err.to_string()))?;
        let tree =
            self.adapter
                .widget_tree(request.window)?
                .ok_or(UiAdapterError::MissingWidgetTree {
                    window: request.window.get(),
                })?;
        let layout = compute_layout(&tree, request.viewport)
            .map_err(|err| UiDispatchError::Layout(err.to_string()))?;
        let widget = hit_test(&tree, &layout, point).map(|hit| hit.widget);

        self.adapter.queue_pointer_event(PointerEvent {
            window: request.window,
            widget,
            x: request.x,
            y: request.y,
            button: request.button,
            pressed: request.pressed,
            modifiers: request.modifiers,
        })?;

        Ok(widget)
    }

    pub fn route_key_event(&self, request: KeyRouteRequest) -> UiDispatchResult<Option<WidgetId>> {
        self.check_window_access()?;
        let widget = self.adapter.focused_widget(request.window)?;
        let event = KeyEvent::new(
            request.window,
            widget,
            request.key,
            request.pressed,
            request.modifiers,
        )?;
        self.adapter.queue_key_event(event)?;
        Ok(widget)
    }

    pub fn route_text_input(
        &self,
        request: TextInputRouteRequest,
    ) -> UiDispatchResult<Option<WidgetId>> {
        self.check_window_access()?;
        let widget = self.adapter.focused_widget(request.window)?;
        let event = TextInputEvent::new(request.window, widget, request.text)?;
        self.adapter.queue_text_input(event)?;
        Ok(widget)
    }

    pub fn route_close_requested(&self, window: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_close_requested(window)?;
        Ok(())
    }

    pub fn route_host_resize(&self, window: WindowId, size: WindowSize) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_host_resize(window, size)?;
        Ok(())
    }

    pub fn route_window_focused(&self, window: WindowId, focused: bool) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_window_focused(window, focused)?;
        Ok(())
    }

    pub fn route_theme_changed(&self, theme: Theme) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_theme_changed(theme)?;
        Ok(())
    }

    pub fn route_scale_changed(&self, window: WindowId, scale: f32) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_scale_changed(window, scale)?;
        Ok(())
    }

    /// Drain raw native pointer input for runtime-side hit-test routing.
    /// Drain raw keyboard samples from the native backend.
    pub fn drain_raw_key_input(&self) -> Vec<krate_adapter_common::ui::RawKeySample> {
        if self.check_window_access().is_err() {
            return Vec::new();
        }
        self.adapter.drain_raw_key_input()
    }

    /// Queue a focus-routed key event for the app.
    pub fn queue_key_event(
        &self,
        event: krate_adapter_common::ui::KeyEvent,
    ) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_key_event(event).map_err(Into::into)
    }

    /// Queue a focus-routed text input event for the app.
    pub fn queue_text_input(
        &self,
        event: krate_adapter_common::ui::TextInputEvent,
    ) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.queue_text_input(event).map_err(Into::into)
    }

    /// Queue a native control's complete text after a person edited it.
    pub fn queue_text_changed(
        &self,
        window: WindowId,
        widget: WidgetId,
        text: String,
    ) -> UiDispatchResult<()> {
        self.check_window_access()?;
        let event = krate_adapter_common::ui::TextChangedEvent::new(window, widget, text)?;
        self.adapter.queue_text_changed(event).map_err(Into::into)
    }

    /// Drain raw mouse-wheel samples from the native backend.
    pub fn drain_raw_wheel_input(&self) -> Vec<krate_adapter_common::ui::RawWheelSample> {
        if self.check_window_access().is_err() {
            return Vec::new();
        }
        self.adapter.drain_raw_wheel_input()
    }

    pub fn drain_raw_pointer_input(&self) -> Vec<krate_adapter_common::ui::RawPointerSample> {
        self.adapter.drain_raw_pointer_input()
    }

    pub fn drain_events(&self) -> UiDispatchResult<Vec<UiEvent>> {
        Ok(self.adapter.drain_events()?)
    }

    pub fn poll_event(&self) -> UiDispatchResult<Option<UiEvent>> {
        Ok(self.adapter.poll_event()?)
    }

    pub fn pump_event_loop_once(
        &self,
        window: WindowId,
    ) -> UiDispatchResult<Option<UiEventLoopTick>> {
        self.check_window_access()?;
        Ok(self.adapter.pump_event_loop_once(window)?)
    }

    /// Widgets the host lowered to native controls a person can type into.
    pub fn native_editable_widgets(&self, window: WindowId) -> Vec<WidgetId> {
        self.adapter.native_editable_widgets(window)
    }

    /// Current text of a natively lowered editable control.
    pub fn native_widget_text(&self, window: WindowId, widget: WidgetId) -> Option<String> {
        self.adapter.native_widget_text(window, widget)
    }

    fn check_window_access(&self) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::WindowCreate))
    }

    fn check(&self, call: &UapiCall) -> UiDispatchResult<()> {
        self.guard.check(call).map(|_| ()).map_err(map_ui_policy)
    }
}

fn discover_host_ui_adapter() -> Box<dyn UiAdapter> {
    #[cfg(target_os = "linux")]
    {
        Box::new(krate_adapter_linux::discover_ui_adapter())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(krate_adapter_macos::discover_ui_adapter())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(krate_adapter_windows::discover_ui_adapter())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Box::new(krate_adapter_common::ui::DraftUiAdapter::new())
    }
}

fn discover_host_ui_adapter_for_mode(
    host_mode: Phase3HostUiMode,
) -> UiDispatchResult<Box<dyn UiAdapter>> {
    match host_mode {
        Phase3HostUiMode::HeadlessDraft => Ok(discover_host_ui_adapter()),
        Phase3HostUiMode::NativePrototype => discover_native_prototype_ui_adapter(),
    }
}

fn discover_native_prototype_ui_adapter() -> UiDispatchResult<Box<dyn UiAdapter>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(
            krate_adapter_linux::discover_winit_prototype_ui_adapter()?,
        ))
    }

    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(
            krate_adapter_macos::discover_appkit_prototype_ui_adapter()?,
        ))
    }

    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(
            krate_adapter_windows::discover_winit_prototype_ui_adapter()?,
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(UiDispatchError::Unsupported)
    }
}

fn map_ui_policy(err: UapiError) -> UiDispatchError {
    if matches!(
        err,
        UapiError::Policy(krate_policy::PolicyError::Denied { .. })
    ) {
        UiDispatchError::PermissionDenied
    } else {
        UiDispatchError::Policy(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use krate_adapter_common::ui::{
        DraftUiAdapter, UiEvent, WidgetId, WidgetKind, WidgetNode, WidgetStyle, WindowBackendKind,
        WindowOptions, WindowSize,
    };
    use krate_policy::SessionPolicy;

    use super::*;

    #[test]
    fn default_window_grant_creates_and_tracks_draft_window() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(800, 600).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Krate Notes", size).expect("options"))
            .expect("create window");
        dispatcher.show_window(id).expect("show window");
        let resized = WindowSize::new(1024, 768).expect("resized");
        dispatcher.set_size(id, resized).expect("resize window");
        dispatcher.request_redraw(id).expect("redraw");
        dispatcher.close_window(id).expect("close window");

        let window = dispatcher.window(id).expect("adapter").expect("window");
        assert_eq!(window.title, "Krate Notes");
        assert_eq!(window.size, resized);
        assert!(!window.visible);
        assert!(window.closed);
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowClosed(id),
            ]
        );
    }

    #[test]
    fn poll_event_returns_fifo_runtime_events() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(800, 600).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Krate Notes", size).expect("options"))
            .expect("create window");
        dispatcher.show_window(id).expect("show window");

        assert_eq!(
            dispatcher.poll_event().expect("poll"),
            Some(UiEvent::WindowCreated(id))
        );
        assert_eq!(
            dispatcher.poll_event().expect("poll"),
            Some(UiEvent::WindowShown(id))
        );
        assert_eq!(dispatcher.poll_event().expect("poll"), None);
    }

    #[test]
    fn headless_runtime_event_loop_pump_is_noop() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let id = dispatcher
            .create_window(
                WindowOptions::new(
                    "Krate headless pump",
                    WindowSize::new(640, 480).expect("size"),
                )
                .expect("options"),
            )
            .expect("create window");

        assert_eq!(dispatcher.pump_event_loop_once(id).expect("pump"), None);
    }

    #[test]
    fn routes_host_window_events_into_queue() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(800, 600).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Krate Notes", size).expect("options"))
            .expect("create window");
        let resized = WindowSize::new(900, 700).expect("resized");

        dispatcher
            .route_window_focused(id, true)
            .expect("window focus");
        dispatcher
            .route_host_resize(id, resized)
            .expect("host resize");
        dispatcher
            .route_close_requested(id)
            .expect("close requested");

        let window = dispatcher.window(id).expect("adapter").expect("window");
        assert_eq!(window.size, resized);
        assert!(window.focused);
        assert!(!window.closed);
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::WindowCloseRequested(id),
            ]
        );
    }

    #[test]
    fn routes_theme_and_scale_events_into_queue() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(800, 600).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Krate Notes", size).expect("options"))
            .expect("create window");

        dispatcher
            .route_theme_changed(Theme::Dark)
            .expect("theme changed");
        dispatcher
            .route_scale_changed(id, 2.0)
            .expect("scale changed");

        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::ThemeChanged { theme: Theme::Dark },
                UiEvent::ScaleChanged { id, scale: 2.0 },
            ]
        );
    }

    #[test]
    fn runtime_discovers_current_host_ui_adapter() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let runtime = Phase3UiRuntime::with_host_adapter(guard);
        let info = runtime.adapter_info();
        let dispatcher = runtime.dispatcher();
        let size = WindowSize::new(480, 320).expect("size");

        assert_eq!(runtime.host_mode(), Phase3HostUiMode::HeadlessDraft);
        assert!(info.backend.ends_with("headless-draft"));
        assert_eq!(info.window_backend, WindowBackendKind::HeadlessDraft);
        assert_ne!(info.planned_window_backend, WindowBackendKind::Unknown);
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);

        let id = dispatcher
            .create_window(WindowOptions::new("Krate host adapter", size).expect("options"))
            .expect("create window through discovered adapter");
        dispatcher.show_window(id).expect("show");

        let window = dispatcher.window(id).expect("adapter").expect("window");
        assert_eq!(window.title, "Krate host adapter");
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![UiEvent::WindowCreated(id), UiEvent::WindowShown(id)]
        );
    }

    #[test]
    fn runtime_native_prototype_mode_is_explicit() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let runtime =
            Phase3UiRuntime::try_with_host_adapter_mode(guard, Phase3HostUiMode::NativePrototype);

        #[cfg(target_os = "macos")]
        {
            let runtime = runtime.expect("macOS native prototype runtime");
            let info = runtime.adapter_info();

            assert_eq!(runtime.host_mode(), Phase3HostUiMode::NativePrototype);
            assert_eq!(info.host_family, "macos");
            assert_eq!(info.backend, "macos-appkit-prototype");
            assert_eq!(info.planned_window_backend, WindowBackendKind::AppKit);
            assert!(info.native_windows);
            assert!(info.native_event_loop);
        }

        #[cfg(target_os = "linux")]
        {
            let runtime = runtime.expect("Linux winit prototype runtime");
            let info = runtime.adapter_info();

            assert_eq!(runtime.host_mode(), Phase3HostUiMode::NativePrototype);
            assert_eq!(info.host_family, "linux");
            assert_eq!(info.backend, "linux-winit-prototype");
            assert_eq!(info.planned_window_backend, WindowBackendKind::Winit);
            assert!(info.native_windows);
            assert!(info.native_event_loop);
        }

        #[cfg(target_os = "windows")]
        {
            let runtime = runtime.expect("Windows winit prototype runtime");
            let info = runtime.adapter_info();

            assert_eq!(runtime.host_mode(), Phase3HostUiMode::NativePrototype);
            assert_eq!(info.host_family, "windows");
            assert_eq!(info.backend, "windows-winit-prototype");
            assert_eq!(info.planned_window_backend, WindowBackendKind::Winit);
            assert!(info.native_windows);
            assert!(info.native_event_loop);
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            assert!(matches!(runtime, Err(UiDispatchError::Unsupported)));
        }
    }

    #[test]
    fn window_operations_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let err = dispatcher
            .set_title(id, " ")
            .expect_err("empty title should fail");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::EmptyTitle)
        ));
    }

    #[test]
    fn key_and_text_events_route_through_the_dispatcher() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let field = WidgetNode::new(WidgetId::new(2).expect("field"), WidgetKind::TextField)
            .with_parent(root.id);
        dispatcher.set_root(window, root).expect("set root");
        dispatcher.upsert_node(window, field).expect("upsert field");
        let field_id = WidgetId::new(2).expect("field");
        dispatcher.focus_node(window, field_id).expect("focus");

        // No native backend: the draft adapter reports no raw samples.
        assert!(dispatcher.drain_raw_key_input().is_empty());

        let key = krate_adapter_common::ui::KeyEvent::new(
            window,
            Some(field_id),
            "a",
            true,
            krate_adapter_common::ui::Modifiers::default(),
        )
        .expect("key event");
        dispatcher.queue_key_event(key.clone()).expect("queue key");
        let text = krate_adapter_common::ui::TextInputEvent::new(window, Some(field_id), "a")
            .expect("text event");
        dispatcher
            .queue_text_input(text.clone())
            .expect("queue text");

        // Focus change from focus_node arrives first, then key, then text.
        let mut saw_key = false;
        let mut saw_text = false;
        while let Some(event) = dispatcher.poll_event().expect("poll") {
            match event {
                UiEvent::Key(event) => {
                    assert_eq!(event, key);
                    saw_key = true;
                }
                UiEvent::TextInput(event) => {
                    assert_eq!(event, text);
                    saw_text = true;
                }
                _ => {}
            }
        }
        assert!(saw_key && saw_text, "key and text events must round-trip");
    }

    #[test]
    fn widget_tree_operations_pass_through_window_policy() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(root.id)
            .with_label("Save")
            .expect("label");

        dispatcher.set_root(window, root).expect("set root");
        dispatcher
            .upsert_node(window, button)
            .expect("upsert button");
        dispatcher
            .focus_node(window, WidgetId::new(2).expect("button"))
            .expect("focus button");

        let tree = dispatcher
            .widget_tree(window)
            .expect("tree lookup")
            .expect("tree");
        assert_eq!(tree.nodes().len(), 2);
        assert_eq!(
            dispatcher.focused_widget(window).expect("focus"),
            Some(WidgetId::new(2).expect("button"))
        );
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(window),
                UiEvent::WidgetRootSet {
                    window,
                    root: WidgetId::new(1).expect("root"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
                UiEvent::FocusChanged {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
            ]
        );
    }

    #[test]
    fn widget_tree_operations_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let orphan = WidgetNode::new(WidgetId::new(3).expect("orphan"), WidgetKind::Text);

        let err = dispatcher
            .upsert_node(window, orphan)
            .expect_err("missing widget tree should fail");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::MissingWidgetTree { .. })
        ));
    }

    #[test]
    fn computes_layout_for_stored_widget_tree() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let row = WidgetNode::new(WidgetId::new(2).expect("row"), WidgetKind::Text)
            .with_parent(root.id)
            .with_style(WidgetStyle {
                width: Some(120.0),
                height: Some(32.0),
                ..WidgetStyle::default()
            })
            .expect("style");

        dispatcher.set_root(window, root).expect("root");
        dispatcher.upsert_node(window, row).expect("row");
        let layout = dispatcher
            .compute_layout(window, LayoutViewport::new(300.0, 200.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            layout.rect(WidgetId::new(1).expect("root")),
            Some(krate_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 200.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(2).expect("row")),
            Some(krate_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 32.0,
            })
        );
    }

    #[test]
    fn prepares_layout_for_repeated_window_layout_passes() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let row = WidgetNode::new(WidgetId::new(2).expect("row"), WidgetKind::Text)
            .with_parent(root.id)
            .with_style(WidgetStyle {
                grow: 1.0,
                ..WidgetStyle::default()
            })
            .expect("style");

        dispatcher.set_root(window, root).expect("root");
        dispatcher.upsert_node(window, row).expect("row");
        let mut prepared = dispatcher.prepare_layout(window).expect("prepared");
        let first = prepared
            .compute(LayoutViewport::new(300.0, 200.0).expect("viewport"))
            .expect("first layout");
        let second = prepared
            .compute(LayoutViewport::new(600.0, 400.0).expect("viewport"))
            .expect("second layout");

        assert_eq!(
            first.rect(WidgetId::new(1).expect("root")),
            Some(krate_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 200.0,
            })
        );
        assert_eq!(
            second.rect(WidgetId::new(1).expect("root")),
            Some(krate_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 600.0,
                height: 400.0,
            })
        );
    }

    #[test]
    fn routes_pointer_events_to_deepest_layout_hit() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(root.id)
            .with_style(WidgetStyle {
                width: Some(120.0),
                height: Some(40.0),
                ..WidgetStyle::default()
            })
            .expect("style");
        let label = WidgetNode::new(WidgetId::new(3).expect("label"), WidgetKind::Text)
            .with_parent(button.id)
            .with_style(WidgetStyle {
                width: Some(80.0),
                height: Some(20.0),
                ..WidgetStyle::default()
            })
            .expect("style");

        dispatcher.set_root(window, root).expect("root");
        dispatcher.upsert_node(window, button).expect("button");
        dispatcher.upsert_node(window, label).expect("label");
        let target = dispatcher
            .route_pointer_event(PointerRouteRequest {
                window,
                viewport: LayoutViewport::new(300.0, 200.0).expect("viewport"),
                x: 10.0,
                y: 10.0,
                button: Some(PointerButton::Primary),
                pressed: true,
                modifiers: Modifiers::default(),
            })
            .expect("pointer");

        assert_eq!(target, Some(WidgetId::new(3).expect("label")));
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(window),
                UiEvent::WidgetRootSet {
                    window,
                    root: WidgetId::new(1).expect("root"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(3).expect("label"),
                },
                UiEvent::Pointer(PointerEvent {
                    window,
                    widget: Some(WidgetId::new(3).expect("label")),
                    x: 10.0,
                    y: 10.0,
                    button: Some(PointerButton::Primary),
                    pressed: true,
                    modifiers: Modifiers::default(),
                }),
            ]
        );
    }

    #[test]
    fn routes_pointer_events_without_widget_when_outside_layout() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(100, 100).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);

        dispatcher.set_root(window, root).expect("root");
        let target = dispatcher
            .route_pointer_event(PointerRouteRequest {
                window,
                viewport: LayoutViewport::new(100.0, 100.0).expect("viewport"),
                x: 120.0,
                y: 10.0,
                button: None,
                pressed: false,
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            })
            .expect("pointer");

        assert_eq!(target, None);
        assert_eq!(
            dispatcher.drain_events().expect("events").last(),
            Some(&UiEvent::Pointer(PointerEvent {
                window,
                widget: None,
                x: 120.0,
                y: 10.0,
                button: None,
                pressed: false,
                modifiers: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
            }))
        );
    }

    #[test]
    fn routes_key_and_text_input_to_focused_widget() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let title = WidgetNode::new(WidgetId::new(2).expect("title"), WidgetKind::TextField)
            .with_parent(root.id)
            .with_label("Title")
            .expect("label");

        dispatcher.set_root(window, root).expect("root");
        dispatcher.upsert_node(window, title).expect("title");
        dispatcher
            .focus_node(window, WidgetId::new(2).expect("title"))
            .expect("focus");

        let key_target = dispatcher
            .route_key_event(KeyRouteRequest {
                window,
                key: "Enter".to_string(),
                pressed: true,
                modifiers: Modifiers {
                    meta: true,
                    ..Modifiers::default()
                },
            })
            .expect("key route");
        let text_target = dispatcher
            .route_text_input(TextInputRouteRequest {
                window,
                text: "hello".to_string(),
            })
            .expect("text route");

        assert_eq!(key_target, Some(WidgetId::new(2).expect("title")));
        assert_eq!(text_target, Some(WidgetId::new(2).expect("title")));
        assert_eq!(
            dispatcher.drain_events().expect("events").last_chunk::<2>(),
            Some(&[
                UiEvent::Key(
                    KeyEvent::new(
                        window,
                        Some(WidgetId::new(2).expect("title")),
                        "Enter",
                        true,
                        Modifiers {
                            meta: true,
                            ..Modifiers::default()
                        },
                    )
                    .expect("key")
                ),
                UiEvent::TextInput(
                    TextInputEvent::new(window, Some(WidgetId::new(2).expect("title")), "hello")
                        .expect("text"),
                ),
            ])
        );
    }

    #[test]
    fn routes_key_event_without_widget_when_no_focus_exists() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");

        let target = dispatcher
            .route_key_event(KeyRouteRequest {
                window,
                key: "Escape".to_string(),
                pressed: true,
                modifiers: Modifiers::default(),
            })
            .expect("key route");

        assert_eq!(target, None);
        assert_eq!(
            dispatcher.drain_events().expect("events").last(),
            Some(&UiEvent::Key(
                KeyEvent::new(window, None, "Escape", true, Modifiers::default()).expect("key")
            ))
        );
    }

    #[test]
    fn key_and_text_routes_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");

        let key_err = dispatcher
            .route_key_event(KeyRouteRequest {
                window,
                key: "\n".to_string(),
                pressed: true,
                modifiers: Modifiers::default(),
            })
            .expect_err("bad key");
        let text_err = dispatcher
            .route_text_input(TextInputRouteRequest {
                window,
                text: String::new(),
            })
            .expect_err("bad text");

        assert!(matches!(
            key_err,
            UiDispatchError::Adapter(UiAdapterError::InvalidInputEvent(_))
        ));
        assert!(matches!(
            text_err,
            UiDispatchError::Adapter(UiAdapterError::InvalidInputEvent(_))
        ));
    }

    #[test]
    fn clipboard_read_denies_before_unsupported_draft_path() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let err = dispatcher
            .read_clipboard_text()
            .expect_err("clipboard should need explicit grant");

        assert!(matches!(err, UiDispatchError::PermissionDenied));
    }

    #[test]
    fn clipboard_read_reaches_draft_unsupported_when_granted() {
        let policy =
            SessionPolicy::from_cli_grants(&["ui.clipboard:read".to_string()]).expect("policy");
        let guard = UapiGuard::new(policy);
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let err = dispatcher
            .read_clipboard_text()
            .expect_err("clipboard host adapter is not implemented yet");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::Unsupported(_))
        ));
    }
}
