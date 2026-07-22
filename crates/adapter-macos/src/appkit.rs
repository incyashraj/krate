//! Opt-in AppKit window prototype for the macOS adapter.
//!
//! The default macOS adapter still uses the headless draft backend. This module
//! is the first native path behind the checked handle handoff: it creates an
//! AppKit `NSWindow`, keeps that object alive, and binds its raw pointer to a
//! stable Krate `WindowId`.

use krate_adapter_common::ui::{
    Modifiers, NativeWindowHandle, PointerButton, PointerEvent, UiAdapter, UiAdapterError,
    WidgetId, WidgetKind, WindowAdapter, WindowBackendKind, WindowId, WindowOptions, WindowSize,
};

use crate::MacosUiAdapter;

/// Placement of one widget inside an AppKit content view.
///
/// Coordinates are logical Krate layout units with a top-left origin, exactly
/// as `LayoutSnapshot` reports them. AppKit uses a bottom-left origin, so the
/// lowering path flips Y with [`AppKitWidgetPlacement::appkit_origin_y`].
#[derive(Debug, Clone, PartialEq)]
pub struct AppKitWidgetPlacement {
    widget: WidgetId,
    kind: WidgetKind,
    label: Option<String>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl AppKitWidgetPlacement {
    /// Widget kinds the first AppKit lowering pass supports natively.
    pub fn kind_supported(kind: WidgetKind) -> bool {
        matches!(
            kind,
            WidgetKind::Button
                | WidgetKind::TextField
                | WidgetKind::Text
                | WidgetKind::TextArea
                | WidgetKind::ListView
        )
    }

    /// Create a validated widget placement for native AppKit lowering.
    pub fn new(
        widget: WidgetId,
        kind: WidgetKind,
        label: Option<String>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Result<Self, UiAdapterError> {
        if !Self::kind_supported(kind) {
            return Err(UiAdapterError::Unsupported(format!(
                "AppKit lowering does not support {kind:?} yet"
            )));
        }
        for (name, value) in [("x", x), ("y", y), ("width", width), ("height", height)] {
            if !value.is_finite() || value < 0.0 {
                return Err(UiAdapterError::Unsupported(format!(
                    "AppKit widget placement {name} must be finite and non-negative, got {value}"
                )));
            }
        }
        if width == 0.0 || height == 0.0 {
            return Err(UiAdapterError::Unsupported(
                "AppKit widget placement needs a non-zero size".to_string(),
            ));
        }

        Ok(Self {
            widget,
            kind,
            label,
            x,
            y,
            width,
            height,
        })
    }

    /// Return the stable widget id this placement lowers.
    pub fn widget(&self) -> WidgetId {
        self.widget
    }

    /// Return the widget kind this placement lowers.
    pub fn kind(&self) -> WidgetKind {
        self.kind
    }

    /// Return the label or text content for the native control.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Replace the label, used to carry a person's in-progress typing across
    /// a re-lower rather than reverting the control to the guest's copy.
    pub fn with_label(mut self, label: String) -> Self {
        self.label = Some(label);
        self
    }

    /// Return the logical top-left rectangle as `(x, y, width, height)`.
    pub fn rect(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.width, self.height)
    }

    /// Convert the top-left logical Y into an AppKit bottom-left frame origin.
    pub fn appkit_origin_y(&self, content_height: f32) -> f32 {
        content_height - (self.y + self.height)
    }
}

/// Result of one native AppKit widget lowering pass.
#[derive(Debug, Clone, PartialEq)]
pub struct AppKitWidgetSurfaceSnapshot {
    /// Krate window that owns the lowered widgets.
    pub window: WindowId,
    /// Widgets lowered to native AppKit controls, in placement order.
    pub lowered: Vec<WidgetId>,
}

/// FIFO callback queue used by the real AppKit delegate object.
///
/// AppKit owns the timing of native callbacks. Krate drains this queue from
/// the Rust session object and then feeds the existing delegate bridge.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AppKitWindowDelegateQueue {
    callbacks: Vec<AppKitWindowDelegateCallback>,
}

impl AppKitWindowDelegateQueue {
    /// Create an empty delegate callback queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one callback reported by AppKit.
    pub fn push(&mut self, callback: AppKitWindowDelegateCallback) {
        self.callbacks.push(callback);
    }

    /// Return the number of callbacks waiting to be drained.
    pub fn len(&self) -> usize {
        self.callbacks.len()
    }

    /// Return whether the queue has no pending callbacks.
    pub fn is_empty(&self) -> bool {
        self.callbacks.is_empty()
    }

    /// Drain queued callbacks in AppKit delivery order.
    pub fn drain(&mut self) -> Vec<AppKitWindowDelegateCallback> {
        self.callbacks.drain(..).collect()
    }
}

/// Small native AppKit backend used by the first Phase 3 macOS prototype.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AppKitWindowBackend;

/// Current state read from an owned AppKit window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitWindowSnapshot {
    pub visible: bool,
    pub focused: bool,
    pub size: WindowSize,
    pub scale: f32,
}

/// RGBA color used by the first AppKit draw-surface scaffold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitColor {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl AppKitColor {
    /// Default Krate blue used by early drawing-surface tests.
    pub const KRATE_BLUE: Self = Self {
        red: 0.075,
        green: 0.263,
        blue: 0.859,
        alpha: 1.0,
    };

    /// Create a validated color with channels from 0.0 to 1.0.
    pub fn new(red: f32, green: f32, blue: f32, alpha: f32) -> Result<Self, UiAdapterError> {
        validate_color_channel("red", red)?;
        validate_color_channel("green", green)?;
        validate_color_channel("blue", blue)?;
        validate_color_channel("alpha", alpha)?;
        Ok(Self {
            red,
            green,
            blue,
            alpha,
        })
    }
}

/// One frame described by the early AppKit drawing-surface state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitDrawFrame {
    pub window: WindowId,
    pub size: WindowSize,
    pub scale: f32,
    pub clear_color: AppKitColor,
    pub frame_index: u64,
}

/// State returned after an AppKit draw view is attached to a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitDrawViewSurfaceSnapshot {
    pub window: WindowId,
    pub native_view_handle: u64,
    pub frame: AppKitDrawFrame,
    pub needs_display: bool,
}

/// Small AppKit drawing-surface state used before real pixels are painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitDrawSurfaceState {
    window: WindowId,
    size: WindowSize,
    scale: f32,
    clear_color: AppKitColor,
    frame_index: u64,
    redraws_requested: u64,
}

impl AppKitDrawSurfaceState {
    /// Create drawing-surface state for one AppKit-backed Krate window.
    pub fn new(
        window: WindowId,
        size: WindowSize,
        scale: f32,
        clear_color: AppKitColor,
    ) -> Result<Self, UiAdapterError> {
        validate_scale_factor(scale)?;
        Ok(Self {
            window,
            size,
            scale,
            clear_color,
            frame_index: 0,
            redraws_requested: 0,
        })
    }

    /// Create drawing-surface state from a native AppKit snapshot.
    pub fn from_snapshot(
        window: WindowId,
        snapshot: AppKitWindowSnapshot,
        clear_color: AppKitColor,
    ) -> Result<Self, UiAdapterError> {
        Self::new(window, snapshot.size, snapshot.scale, clear_color)
    }

    /// Return the Krate window this surface belongs to.
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// Return the logical surface size.
    pub fn size(&self) -> WindowSize {
        self.size
    }

    /// Return the current display scale for this surface.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Return the clear color recorded for the next frame.
    pub fn clear_color(&self) -> AppKitColor {
        self.clear_color
    }

    /// Return the last recorded frame index.
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Return how many redraw requests this surface has queued.
    pub fn redraws_requested(&self) -> u64 {
        self.redraws_requested
    }

    /// Update the logical surface size after a native resize event.
    pub fn resize(&mut self, size: WindowSize) {
        self.size = size;
    }

    /// Update the display scale after a native scale-change event.
    pub fn scale_changed(&mut self, scale: f32) -> Result<(), UiAdapterError> {
        validate_scale_factor(scale)?;
        self.scale = scale;
        Ok(())
    }

    /// Queue a redraw through the same AppKit delegate bridge used by native views.
    pub fn request_redraw(
        &mut self,
        bridge: &AppKitWindowDelegateBridge,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        event_state: &mut AppKitWindowEventState,
    ) -> Result<(), UiAdapterError> {
        bridge
            .handle_callback(
                backend,
                adapter,
                event_state,
                AppKitWindowDelegateCallback::ViewNeedsDisplay,
            )
            .map(|_| ())?;
        self.redraws_requested = self.redraws_requested.saturating_add(1);
        Ok(())
    }

    /// Record one frame description for the future AppKit view painter.
    pub fn record_frame(&mut self) -> AppKitDrawFrame {
        self.frame_index = self.frame_index.saturating_add(1);
        AppKitDrawFrame {
            window: self.window,
            size: self.size,
            scale: self.scale,
            clear_color: self.clear_color,
            frame_index: self.frame_index,
        }
    }
}

/// Native AppKit event shape accepted by the first callback bridge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppKitWindowNativeEvent {
    CloseRequested,
    Resized(WindowSize),
    Focused(bool),
    ScaleChanged(f32),
    RedrawRequested,
    Snapshot(AppKitWindowSnapshot),
    WidgetActivated(WidgetId),
}

/// Delegate callback shape the real AppKit object will translate into Rust.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppKitWindowDelegateCallback {
    WindowShouldClose,
    WindowDidResize(WindowSize),
    WindowDidBecomeKey,
    WindowDidResignKey,
    WindowDidChangeBackingScale(f32),
    ViewNeedsDisplay,
    Snapshot(AppKitWindowSnapshot),
    WidgetActivated(WidgetId),
}

/// Small Rust bridge that keeps Objective-C delegate methods thin.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AppKitWindowDelegateBridge;

impl AppKitWindowDelegateBridge {
    /// Queue one delegate callback into the tested native event state.
    pub fn handle_callback(
        &self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        state: &mut AppKitWindowEventState,
        callback: AppKitWindowDelegateCallback,
    ) -> Result<Option<AppKitWindowSnapshot>, UiAdapterError> {
        let event = match callback {
            AppKitWindowDelegateCallback::WindowShouldClose => {
                AppKitWindowNativeEvent::CloseRequested
            }
            AppKitWindowDelegateCallback::WindowDidResize(size) => {
                AppKitWindowNativeEvent::Resized(size)
            }
            AppKitWindowDelegateCallback::WindowDidBecomeKey => {
                AppKitWindowNativeEvent::Focused(true)
            }
            AppKitWindowDelegateCallback::WindowDidResignKey => {
                AppKitWindowNativeEvent::Focused(false)
            }
            AppKitWindowDelegateCallback::WindowDidChangeBackingScale(scale) => {
                AppKitWindowNativeEvent::ScaleChanged(scale)
            }
            AppKitWindowDelegateCallback::ViewNeedsDisplay => {
                AppKitWindowNativeEvent::RedrawRequested
            }
            AppKitWindowDelegateCallback::Snapshot(snapshot) => {
                AppKitWindowNativeEvent::Snapshot(snapshot)
            }
            AppKitWindowDelegateCallback::WidgetActivated(widget) => {
                AppKitWindowNativeEvent::WidgetActivated(widget)
            }
        };

        state.handle_native_event(backend, adapter, event)
    }
}

/// One non-blocking unit of AppKit event-loop work.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AppKitWindowEventLoopStep {
    snapshot: Option<AppKitWindowSnapshot>,
    callbacks: Vec<AppKitWindowDelegateCallback>,
    request_redraw: bool,
}

impl AppKitWindowEventLoopStep {
    /// Create an empty AppKit event-loop step.
    pub fn new() -> Self {
        Self::default()
    }

    /// Include a fresh native window snapshot in this event-loop step.
    pub fn with_snapshot(mut self, snapshot: AppKitWindowSnapshot) -> Self {
        self.snapshot = Some(snapshot);
        self
    }

    /// Include delegate callbacks collected by AppKit in this event-loop step.
    pub fn with_callbacks(
        mut self,
        callbacks: impl IntoIterator<Item = AppKitWindowDelegateCallback>,
    ) -> Self {
        self.callbacks.extend(callbacks);
        self
    }

    /// Ask the shared event queue for a redraw during this event-loop step.
    pub fn with_redraw_request(mut self) -> Self {
        self.request_redraw = true;
        self
    }

    /// Apply this step to one AppKit event-state object.
    pub fn apply_to_event_state(
        &self,
        bridge: &AppKitWindowDelegateBridge,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        state: &mut AppKitWindowEventState,
    ) -> Result<AppKitWindowEventLoopStepReport, UiAdapterError> {
        if let Some(snapshot) = self.snapshot {
            state.sync_snapshot(backend, adapter, snapshot)?;
        }

        let mut callbacks_handled = 0;
        for callback in self.callbacks.iter().copied() {
            bridge.handle_callback(backend, adapter, state, callback)?;
            callbacks_handled += 1;
        }

        if self.request_redraw {
            state.handle_native_event(
                backend,
                adapter,
                AppKitWindowNativeEvent::RedrawRequested,
            )?;
        }

        Ok(AppKitWindowEventLoopStepReport {
            callbacks_handled,
            snapshot: state.last_snapshot(),
            redraw_requested: self.request_redraw,
        })
    }
}

/// Result from one non-blocking AppKit event-loop step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitWindowEventLoopStepReport {
    pub callbacks_handled: usize,
    pub snapshot: Option<AppKitWindowSnapshot>,
    pub redraw_requested: bool,
}

/// Small driver for the opt-in AppKit event-loop prototype.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AppKitWindowEventLoopDriver {
    bridge: AppKitWindowDelegateBridge,
}

impl AppKitWindowEventLoopDriver {
    /// Create a driver that translates AppKit callbacks through the Rust bridge.
    pub fn new() -> Self {
        Self {
            bridge: AppKitWindowDelegateBridge,
        }
    }

    /// Run one explicit event-loop step against a testable event-state object.
    pub fn run_step_for_state(
        &self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        state: &mut AppKitWindowEventState,
        step: &AppKitWindowEventLoopStep,
    ) -> Result<AppKitWindowEventLoopStepReport, UiAdapterError> {
        step.apply_to_event_state(&self.bridge, backend, adapter, state)
    }

    /// Poll one owned AppKit session without blocking.
    pub fn pump_session_once(
        &self,
        session: &mut AppKitWindowSession,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<AppKitWindowEventLoopStepReport, UiAdapterError> {
        // Dispatch pending NSApplication events first so real mouse and
        // keyboard input reaches the native views before state is read.
        session.window().pump_app_events()?;
        session.refresh(backend, adapter)?;
        let callbacks_handled =
            session.drain_native_delegate_callbacks(&self.bridge, backend, adapter)?;

        Ok(AppKitWindowEventLoopStepReport {
            callbacks_handled,
            snapshot: session.last_snapshot(),
            redraw_requested: false,
        })
    }
}

/// Mutable native event-loop state for one AppKit-backed Krate window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitWindowEventState {
    id: WindowId,
    last_snapshot: Option<AppKitWindowSnapshot>,
}

impl AppKitWindowEventState {
    /// Create event-loop state for an AppKit-backed Krate window.
    pub fn new(id: WindowId) -> Self {
        Self {
            id,
            last_snapshot: None,
        }
    }

    /// Return the Krate window id this state belongs to.
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// Return the last native snapshot observed by this state object.
    pub fn last_snapshot(&self) -> Option<AppKitWindowSnapshot> {
        self.last_snapshot
    }

    /// Refresh this state from a full AppKit snapshot.
    pub fn sync_snapshot(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        snapshot: AppKitWindowSnapshot,
    ) -> Result<AppKitWindowSnapshot, UiAdapterError> {
        let snapshot =
            backend.sync_snapshot_for_id(adapter, self.id, snapshot, self.last_snapshot)?;
        self.last_snapshot = Some(snapshot);
        Ok(snapshot)
    }

    /// Queue a native event reported by an AppKit callback.
    pub fn handle_native_event(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        event: AppKitWindowNativeEvent,
    ) -> Result<Option<AppKitWindowSnapshot>, UiAdapterError> {
        match event {
            AppKitWindowNativeEvent::CloseRequested => {
                backend.report_close_requested_for_id(adapter, self.id)?;
                Ok(self.last_snapshot)
            }
            AppKitWindowNativeEvent::Resized(size) => {
                backend.report_resized_for_id(adapter, self.id, size)?;
                self.last_snapshot = self.last_snapshot.map(|mut snapshot| {
                    snapshot.size = size;
                    snapshot
                });
                Ok(self.last_snapshot)
            }
            AppKitWindowNativeEvent::Focused(focused) => {
                backend.report_focused_for_id(adapter, self.id, focused)?;
                self.last_snapshot = self.last_snapshot.map(|mut snapshot| {
                    snapshot.focused = focused;
                    snapshot
                });
                Ok(self.last_snapshot)
            }
            AppKitWindowNativeEvent::ScaleChanged(scale) => {
                backend.report_scale_changed_for_id(adapter, self.id, scale)?;
                self.last_snapshot = self.last_snapshot.map(|mut snapshot| {
                    snapshot.scale = scale;
                    snapshot
                });
                Ok(self.last_snapshot)
            }
            AppKitWindowNativeEvent::RedrawRequested => {
                backend.report_redraw_requested_for_id(adapter, self.id)?;
                Ok(self.last_snapshot)
            }
            AppKitWindowNativeEvent::Snapshot(snapshot) => {
                self.sync_snapshot(backend, adapter, snapshot).map(Some)
            }
            AppKitWindowNativeEvent::WidgetActivated(widget) => {
                backend.report_widget_activated_for_id(adapter, self.id, widget)?;
                Ok(self.last_snapshot)
            }
        }
    }
}

/// Owned AppKit window plus the last native state seen by Krate.
pub struct AppKitWindowSession {
    window: AppKitWindowPrototype,
    event_state: AppKitWindowEventState,
    native_delegate: Option<AppKitWindowNativeDelegate>,
    widget_surface: Option<AppKitWidgetSurface>,
}

impl AppKitWindowSession {
    /// Return the stable Krate window id for this native window session.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Return the opaque native AppKit handle attached to this session.
    pub fn native_handle(&self) -> NativeWindowHandle {
        self.window.native_handle()
    }

    /// Return the most recent native state snapshot observed by this session.
    pub fn last_snapshot(&self) -> Option<AppKitWindowSnapshot> {
        self.event_state.last_snapshot()
    }

    /// Return the owned AppKit window prototype.
    pub fn window(&self) -> &AppKitWindowPrototype {
        &self.window
    }

    /// Return whether this session has a real AppKit delegate installed.
    pub fn has_native_delegate(&self) -> bool {
        self.native_delegate.is_some()
    }

    /// Install the real AppKit window delegate for this session.
    pub fn install_native_delegate(&mut self) -> Result<(), UiAdapterError> {
        self.native_delegate = Some(self.window.install_native_delegate()?);
        Ok(())
    }

    /// Lower validated widget placements into native AppKit controls.
    ///
    /// Requires the native delegate first, because button activations reuse
    /// the same callback queue the delegate writes to.
    pub fn lower_widget_placements(
        &mut self,
        placements: &[AppKitWidgetPlacement],
    ) -> Result<AppKitWidgetSurfaceSnapshot, UiAdapterError> {
        let delegate = self.native_delegate.as_ref().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "AppKit widget lowering needs the native delegate installed first".to_string(),
            )
        })?;

        // Every tree change replaces the whole native control set, which
        // would destroy the control a person is typing into and reset it to
        // whatever the guest last knew. For an editable control the person is
        // the source of truth, not the guest, so carry the live text across
        // the rebuild. Without this, each keystroke races the re-lower and
        // characters are dropped.
        let live_text: std::collections::BTreeMap<WidgetId, String> = self
            .widget_surface
            .as_ref()
            .map(|surface| {
                surface
                    .editable_widgets()
                    .into_iter()
                    .filter_map(|widget| surface.text(widget).ok().map(|text| (widget, text)))
                    .collect()
            })
            .unwrap_or_default();

        let placements: Vec<AppKitWidgetPlacement> = placements
            .iter()
            .map(|placement| match live_text.get(&placement.widget()) {
                Some(text) if placement.kind() == WidgetKind::TextArea => {
                    placement.clone().with_label(text.clone())
                }
                _ => placement.clone(),
            })
            .collect();

        let surface = self.window.lower_widget_placements(&placements, delegate)?;
        let snapshot = surface.snapshot();
        self.widget_surface = Some(surface);
        Ok(snapshot)
    }

    /// Return the last widget lowering result, if any.
    pub fn widget_surface_snapshot(&self) -> Option<AppKitWidgetSurfaceSnapshot> {
        self.widget_surface
            .as_ref()
            .map(AppKitWidgetSurface::snapshot)
    }

    fn widget_surface(&self) -> Result<&AppKitWidgetSurface, UiAdapterError> {
        self.widget_surface.as_ref().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "this AppKit session has no lowered widget surface".to_string(),
            )
        })
    }

    /// Trigger a lowered native button exactly as a physical click would.
    pub fn perform_widget_click(&self, widget: WidgetId) -> Result<(), UiAdapterError> {
        self.widget_surface()?.perform_click(widget)
    }

    /// Set the text content of a lowered native text field or label.
    pub fn set_widget_text(&self, widget: WidgetId, text: &str) -> Result<(), UiAdapterError> {
        self.widget_surface()?.set_text(widget, text)
    }

    /// Read the current text content of a lowered native control.
    pub fn widget_text(&self, widget: WidgetId) -> Result<String, UiAdapterError> {
        self.widget_surface()?.text(widget)
    }

    /// Widgets lowered to controls a person can type into.
    pub fn editable_widgets(&self) -> Vec<WidgetId> {
        self.widget_surface()
            .map(|surface| surface.editable_widgets())
            .unwrap_or_default()
    }

    /// Show the native window through AppKit and the shared Krate adapter.
    pub fn show(
        &self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<(), UiAdapterError> {
        backend.show_window(adapter, &self.window)
    }

    /// Refresh native window state into the shared event queue.
    pub fn refresh(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<AppKitWindowSnapshot, UiAdapterError> {
        let snapshot = self.window.snapshot()?;
        self.event_state.sync_snapshot(backend, adapter, snapshot)
    }

    /// Queue a native AppKit event into the shared Krate event stream.
    pub fn handle_native_event(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        event: AppKitWindowNativeEvent,
    ) -> Result<Option<AppKitWindowSnapshot>, UiAdapterError> {
        self.event_state
            .handle_native_event(backend, adapter, event)
    }

    /// Queue a native AppKit delegate callback into the shared event stream.
    pub fn handle_delegate_callback(
        &mut self,
        bridge: &AppKitWindowDelegateBridge,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        callback: AppKitWindowDelegateCallback,
    ) -> Result<Option<AppKitWindowSnapshot>, UiAdapterError> {
        bridge.handle_callback(backend, adapter, &mut self.event_state, callback)
    }

    /// Queue callbacks already collected from a native AppKit delegate.
    pub fn handle_delegate_callbacks(
        &mut self,
        bridge: &AppKitWindowDelegateBridge,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
        callbacks: impl IntoIterator<Item = AppKitWindowDelegateCallback>,
    ) -> Result<usize, UiAdapterError> {
        let mut handled = 0;
        for callback in callbacks {
            self.handle_delegate_callback(bridge, backend, adapter, callback)?;
            handled += 1;
        }
        Ok(handled)
    }

    /// Drain callbacks from the installed native AppKit delegate.
    pub fn drain_native_delegate_callbacks(
        &mut self,
        bridge: &AppKitWindowDelegateBridge,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<usize, UiAdapterError> {
        let Some(delegate) = &self.native_delegate else {
            return Ok(0);
        };
        let callbacks = delegate.drain_callbacks();
        self.handle_delegate_callbacks(bridge, backend, adapter, callbacks)
    }

    /// Poll this AppKit session once without blocking.
    pub fn pump_event_loop_once(
        &mut self,
        driver: &AppKitWindowEventLoopDriver,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<AppKitWindowEventLoopStepReport, UiAdapterError> {
        driver.pump_session_once(self, backend, adapter)
    }

    /// Report a close request from native AppKit into the shared queue.
    pub fn report_close_requested(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<(), UiAdapterError> {
        self.handle_native_event(backend, adapter, AppKitWindowNativeEvent::CloseRequested)
            .map(|_| ())
    }

    /// Request a redraw for the native AppKit window.
    pub fn request_redraw(
        &mut self,
        backend: &AppKitWindowBackend,
        adapter: &MacosUiAdapter,
    ) -> Result<(), UiAdapterError> {
        self.handle_native_event(backend, adapter, AppKitWindowNativeEvent::RedrawRequested)
            .map(|_| ())
    }

    /// Create early draw-surface state from the last native snapshot.
    pub fn create_draw_surface_state(
        &self,
        clear_color: AppKitColor,
    ) -> Result<AppKitDrawSurfaceState, UiAdapterError> {
        let snapshot = self.last_snapshot().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "AppKit draw surface state needs a native snapshot first".to_string(),
            )
        })?;
        AppKitDrawSurfaceState::from_snapshot(self.id(), snapshot, clear_color)
    }

    /// Attach a first AppKit draw view to this native window session.
    pub fn attach_draw_view_surface(
        &self,
        state: &mut AppKitDrawSurfaceState,
    ) -> Result<AppKitDrawViewSurface, UiAdapterError> {
        self.window.attach_draw_view_surface(state)
    }
}

impl AppKitWindowBackend {
    /// Return the native backend kind created by this prototype.
    pub fn backend_kind(&self) -> WindowBackendKind {
        WindowBackendKind::AppKit
    }

    /// Return whether this crate build can create AppKit windows.
    pub fn is_available(&self) -> bool {
        cfg!(target_os = "macos")
    }

    /// Create an owned native window session for the first AppKit event-loop work.
    pub fn create_session(
        &self,
        adapter: &MacosUiAdapter,
        options: WindowOptions,
    ) -> Result<AppKitWindowSession, UiAdapterError> {
        let window = self.create_window(adapter, options)?;
        let event_state = AppKitWindowEventState::new(window.id());
        Ok(AppKitWindowSession {
            window,
            event_state,
            native_delegate: None,
            widget_surface: None,
        })
    }

    /// Queue a native close request for a Krate window id.
    pub fn report_close_requested_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_close_requested(adapter, id)
    }

    /// Queue a native resize for a Krate window id.
    pub fn report_resized_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        size: WindowSize,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_host_resize(adapter, id, size)
    }

    /// Queue a native focus change for a Krate window id.
    pub fn report_focused_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        focused: bool,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_window_focused(adapter, id, focused)
    }

    /// Queue a native display scale change for a Krate window id.
    pub fn report_scale_changed_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        scale: f32,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_scale_changed(adapter, id, scale)
    }

    /// Queue a native redraw request for a Krate window id.
    pub fn report_redraw_requested_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::request_redraw(adapter, id)
    }

    /// Queue a native widget activation (for example an `NSButton` click).
    ///
    /// The activation enters the shared stream as a routed pointer event that
    /// already carries the target widget id, matching the shape the runtime's
    /// hit-test route produces for drawn widgets.
    pub fn report_widget_activated_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        widget: WidgetId,
    ) -> Result<(), UiAdapterError> {
        UiAdapter::queue_pointer_event(
            adapter,
            PointerEvent {
                window: id,
                widget: Some(widget),
                x: 0.0,
                y: 0.0,
                button: Some(PointerButton::Primary),
                pressed: true,
                modifiers: Modifiers::default(),
            },
        )
    }

    /// Queue changed native state for a Krate window id.
    pub fn sync_snapshot_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        snapshot: AppKitWindowSnapshot,
        previous: Option<AppKitWindowSnapshot>,
    ) -> Result<AppKitWindowSnapshot, UiAdapterError> {
        if previous.is_none_or(|previous| previous.size != snapshot.size) {
            self.report_resized_for_id(adapter, id, snapshot.size)?;
        }
        if previous.is_none_or(|previous| previous.focused != snapshot.focused) {
            self.report_focused_for_id(adapter, id, snapshot.focused)?;
        }
        if previous.is_none_or(|previous| previous.scale != snapshot.scale) {
            self.report_scale_changed_for_id(adapter, id, snapshot.scale)?;
        }

        Ok(snapshot)
    }

    /// Queue a native close request for an owned AppKit window.
    pub fn report_close_requested(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
    ) -> Result<(), UiAdapterError> {
        self.report_close_requested_for_id(adapter, window.id())
    }

    /// Queue a native resize for an owned AppKit window.
    pub fn report_resized(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
        size: WindowSize,
    ) -> Result<(), UiAdapterError> {
        self.report_resized_for_id(adapter, window.id(), size)
    }

    /// Queue a native focus change for an owned AppKit window.
    pub fn report_focused(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
        focused: bool,
    ) -> Result<(), UiAdapterError> {
        self.report_focused_for_id(adapter, window.id(), focused)
    }

    /// Queue a native display scale change for an owned AppKit window.
    pub fn report_scale_changed(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
        scale: f32,
    ) -> Result<(), UiAdapterError> {
        self.report_scale_changed_for_id(adapter, window.id(), scale)
    }

    /// Queue a native redraw request for an owned AppKit window.
    pub fn report_redraw_requested(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
    ) -> Result<(), UiAdapterError> {
        self.report_redraw_requested_for_id(adapter, window.id())
    }

    /// Read an AppKit snapshot and queue changed state into the shared event stream.
    pub fn sync_window_state(
        &self,
        adapter: &MacosUiAdapter,
        window: &AppKitWindowPrototype,
        previous: Option<AppKitWindowSnapshot>,
    ) -> Result<AppKitWindowSnapshot, UiAdapterError> {
        let snapshot = window.snapshot()?;
        self.sync_snapshot_for_id(adapter, window.id(), snapshot, previous)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSButton, NSColor,
        NSControl, NSEventMask, NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSDefaultRunLoopMode, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
        NSString,
    };
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    type SharedDelegateQueue = Rc<RefCell<AppKitWindowDelegateQueue>>;

    #[derive(Debug, Default)]
    struct KrateWidgetTargetIvars {
        callbacks: SharedDelegateQueue,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no extra subclassing requirements for a passive
        //   target-action receiver.
        // - The target is main-thread-only because AppKit delivers control
        //   actions on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = KrateWidgetTargetIvars]
        struct KrateWidgetTarget;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for KrateWidgetTarget {}

        impl KrateWidgetTarget {
            // SAFETY: The action signature matches AppKit's target-action
            // convention (`-(void)action:(id)sender`), and every control that
            // uses this selector is an NSControl created by the lowering path.
            #[unsafe(method(krateWidgetActivated:))]
            fn krate_widget_activated(&self, sender: &NSControl) {
                let tag = sender.tag();
                if tag <= 0 {
                    return;
                }
                if let Ok(widget) = WidgetId::new(tag as u64) {
                    self.ivars()
                        .callbacks
                        .borrow_mut()
                        .push(AppKitWindowDelegateCallback::WidgetActivated(widget));
                }
            }
        }
    );

    impl KrateWidgetTarget {
        fn new(mtm: MainThreadMarker, callbacks: SharedDelegateQueue) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(KrateWidgetTargetIvars { callbacks });
            // SAFETY: NSObject's `init` signature is correct for a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Native AppKit controls lowered from Krate widget placements.
    pub struct AppKitWidgetSurface {
        window: WindowId,
        _target: Retained<KrateWidgetTarget>,
        controls: BTreeMap<WidgetId, Retained<NSControl>>,
        kinds: BTreeMap<WidgetId, WidgetKind>,
        lowered: Vec<WidgetId>,
    }

    impl AppKitWidgetSurface {
        /// Return the lowering result snapshot for this surface.
        pub fn snapshot(&self) -> AppKitWidgetSurfaceSnapshot {
            AppKitWidgetSurfaceSnapshot {
                window: self.window,
                lowered: self.lowered.clone(),
            }
        }

        /// Return how many native controls this surface owns.
        pub fn widget_count(&self) -> usize {
            self.controls.len()
        }

        /// Return the opaque native control pointer for diagnostics.
        pub fn native_control_handle(&self, widget: WidgetId) -> Option<u64> {
            self.controls
                .get(&widget)
                .map(|control| Retained::as_ptr(control) as usize as u64)
        }

        fn control(&self, widget: WidgetId) -> Result<&Retained<NSControl>, UiAdapterError> {
            self.controls.get(&widget).ok_or_else(|| {
                UiAdapterError::Unsupported(format!(
                    "widget {} has no lowered AppKit control",
                    widget.get()
                ))
            })
        }

        /// Set the text content of a lowered text field or label.
        pub fn set_text(&self, widget: WidgetId, text: &str) -> Result<(), UiAdapterError> {
            match self.kinds.get(&widget) {
                Some(WidgetKind::TextField) | Some(WidgetKind::Text) => {
                    let control = self.control(widget)?;
                    control.setStringValue(&NSString::from_str(text));
                    Ok(())
                }
                _ => Err(UiAdapterError::Unsupported(format!(
                    "widget {} does not accept text updates",
                    widget.get()
                ))),
            }
        }

        /// Read the current text content of a lowered control.
        pub fn text(&self, widget: WidgetId) -> Result<String, UiAdapterError> {
            let control = self.control(widget)?;
            Ok(control.stringValue().to_string())
        }

        /// Widgets lowered to controls a person can type into.
        ///
        /// AppKit keeps typed text inside the control, so the gui host polls
        /// exactly these after each pump to notice what was typed.
        pub fn editable_widgets(&self) -> Vec<WidgetId> {
            self.kinds
                .iter()
                .filter(|(_, kind)| matches!(kind, WidgetKind::TextField | WidgetKind::TextArea))
                .map(|(widget, _)| *widget)
                .collect()
        }

        /// Trigger a lowered button exactly as a physical click would.
        ///
        /// AppKit's `performClick:` drives the same target-action path a real
        /// mouse click uses, so this proves the native round trip end to end
        /// without needing a human in the loop.
        pub fn perform_click(&self, widget: WidgetId) -> Result<(), UiAdapterError> {
            match self.kinds.get(&widget) {
                Some(WidgetKind::Button) => {
                    let control = self.control(widget)?;
                    // SAFETY: The control is a retained NSButton created by the
                    // lowering path on the main thread, and performClick takes
                    // an optional sender.
                    unsafe { control.performClick(None) };
                    Ok(())
                }
                _ => Err(UiAdapterError::Unsupported(format!(
                    "widget {} is not a clickable AppKit control",
                    widget.get()
                ))),
            }
        }
    }

    #[derive(Debug, Default)]
    struct KrateWindowDelegateIvars {
        callbacks: SharedDelegateQueue,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no extra subclassing requirements for a passive delegate.
        // - The delegate is main-thread-only because AppKit delivers these callbacks
        //   on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = KrateWindowDelegateIvars]
        struct KrateWindowDelegate;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for KrateWindowDelegate {}

        // SAFETY: Method signatures match the generated NSWindowDelegate protocol.
        unsafe impl NSWindowDelegate for KrateWindowDelegate {
            #[unsafe(method(windowShouldClose:))]
            fn window_should_close(&self, _sender: &NSWindow) -> bool {
                self.push_callback(AppKitWindowDelegateCallback::WindowShouldClose);
                false
            }

            #[unsafe(method(windowDidResize:))]
            fn window_did_resize(&self, notification: &NSNotification) {
                if let Some(window) = notification_window(notification) {
                    if let Ok(size) = size_from_rect(window.contentLayoutRect()) {
                        self.push_callback(AppKitWindowDelegateCallback::WindowDidResize(size));
                    }
                }
            }

            #[unsafe(method(windowDidBecomeKey:))]
            fn window_did_become_key(&self, _notification: &NSNotification) {
                self.push_callback(AppKitWindowDelegateCallback::WindowDidBecomeKey);
            }

            #[unsafe(method(windowDidResignKey:))]
            fn window_did_resign_key(&self, _notification: &NSNotification) {
                self.push_callback(AppKitWindowDelegateCallback::WindowDidResignKey);
            }

            #[unsafe(method(windowDidChangeBackingProperties:))]
            fn window_did_change_backing_properties(&self, notification: &NSNotification) {
                if let Some(window) = notification_window(notification) {
                    self.push_callback(AppKitWindowDelegateCallback::WindowDidChangeBackingScale(
                        window.backingScaleFactor() as f32,
                    ));
                }
            }
        }
    );

    impl KrateWindowDelegate {
        fn new(mtm: MainThreadMarker, callbacks: SharedDelegateQueue) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(KrateWindowDelegateIvars { callbacks });
            // SAFETY: NSObject's `init` signature is correct for a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }

        fn push_callback(&self, callback: AppKitWindowDelegateCallback) {
            self.ivars().callbacks.borrow_mut().push(callback);
        }
    }

    /// Owned AppKit window bound to one Krate window id.
    pub struct AppKitWindowPrototype {
        id: WindowId,
        native_handle: NativeWindowHandle,
        window: Retained<NSWindow>,
    }

    /// Owned AppKit view used by the first visible draw-surface smoke path.
    pub struct AppKitDrawViewSurface {
        view: Retained<NSView>,
        snapshot: AppKitDrawViewSurfaceSnapshot,
    }

    /// Retained AppKit window delegate plus the queue it writes into.
    pub struct AppKitWindowNativeDelegate {
        delegate: Retained<KrateWindowDelegate>,
        callbacks: SharedDelegateQueue,
    }

    impl AppKitWindowPrototype {
        /// Return the stable Krate window id.
        pub fn id(&self) -> WindowId {
            self.id
        }

        /// Return the opaque AppKit handle attached to the shared window registry.
        pub fn native_handle(&self) -> NativeWindowHandle {
            self.native_handle
        }

        /// Read current AppKit window state without draining the Krate queue.
        pub fn snapshot(&self) -> Result<AppKitWindowSnapshot, UiAdapterError> {
            let _mtm = main_thread_marker()?;
            let content_rect = self.window.contentLayoutRect();
            Ok(AppKitWindowSnapshot {
                visible: self.window.isVisible(),
                focused: self.window.isKeyWindow(),
                size: size_from_rect(content_rect)?,
                scale: self.window.backingScaleFactor() as f32,
            })
        }

        /// Attach a simple AppKit content view for the first visible frame path.
        pub fn attach_draw_view_surface(
            &self,
            state: &mut AppKitDrawSurfaceState,
        ) -> Result<AppKitDrawViewSurface, UiAdapterError> {
            let mtm = main_thread_marker()?;
            let content_rect = self.window.contentLayoutRect();
            let size = size_from_rect(content_rect)?;
            state.resize(size);
            state.scale_changed(self.window.backingScaleFactor() as f32)?;

            let view = create_draw_view(mtm, content_rect);
            let color = ns_color_from_appkit_color(state.clear_color());
            self.window.setBackgroundColor(Some(&color));
            self.window.setContentView(Some(&view));
            view.setNeedsDisplay(true);

            let frame = state.record_frame();
            let snapshot = AppKitDrawViewSurfaceSnapshot {
                window: self.id,
                native_view_handle: Retained::as_ptr(&view) as usize as u64,
                frame,
                needs_display: true,
            };

            Ok(AppKitDrawViewSurface { view, snapshot })
        }

        /// Install a real AppKit delegate on this native window.
        pub fn install_native_delegate(
            &self,
        ) -> Result<AppKitWindowNativeDelegate, UiAdapterError> {
            let mtm = main_thread_marker()?;
            let native_delegate = AppKitWindowNativeDelegate::new(mtm);
            self.window
                .setDelegate(Some(ProtocolObject::from_ref(&*native_delegate.delegate)));
            Ok(native_delegate)
        }

        /// Lower validated widget placements into real AppKit controls.
        ///
        /// Buttons become `NSButton`, text fields become editable
        /// `NSTextField`, and text labels become non-editable `NSTextField`
        /// labels. Button activations flow into the same callback queue the
        /// installed window delegate writes to, so the normal event-loop pump
        /// drains them into the shared Krate event stream.
        pub fn lower_widget_placements(
            &self,
            placements: &[AppKitWidgetPlacement],
            delegate: &AppKitWindowNativeDelegate,
        ) -> Result<AppKitWidgetSurface, UiAdapterError> {
            let mtm = main_thread_marker()?;
            let content_view = self.window.contentView().ok_or_else(|| {
                UiAdapterError::Unsupported(
                    "AppKit widget lowering needs a window content view".to_string(),
                )
            })?;
            let content_height = self.window.contentLayoutRect().size.height as f32;

            let target = KrateWidgetTarget::new(mtm, Rc::clone(&delegate.callbacks));
            let mut controls = BTreeMap::new();
            let mut kinds = BTreeMap::new();
            let mut lowered = Vec::with_capacity(placements.len());

            for placement in placements {
                let (x, _, width, height) = placement.rect();
                let frame = NSRect::new(
                    NSPoint::new(
                        f64::from(x),
                        f64::from(placement.appkit_origin_y(content_height)),
                    ),
                    NSSize::new(f64::from(width), f64::from(height)),
                );

                let control: Retained<NSControl> = match placement.kind() {
                    WidgetKind::Button => {
                        let title = NSString::from_str(placement.label().unwrap_or("Button"));
                        let target_object: &AnyObject = &target;
                        // SAFETY: The target outlives the button (both are
                        // owned by the returned surface), and the selector is
                        // implemented by KrateWidgetTarget above.
                        let button = unsafe {
                            NSButton::buttonWithTitle_target_action(
                                &title,
                                Some(target_object),
                                Some(sel!(krateWidgetActivated:)),
                                mtm,
                            )
                        };
                        button.setFrame(frame);
                        button.setTag(placement.widget().get() as isize);
                        Retained::into_super(button)
                    }
                    WidgetKind::TextField => {
                        let value = NSString::from_str(placement.label().unwrap_or(""));
                        let field = NSTextField::textFieldWithString(&value, mtm);
                        field.setFrame(frame);
                        field.setTag(placement.widget().get() as isize);
                        Retained::into_super(field)
                    }
                    WidgetKind::Text => {
                        let value = NSString::from_str(placement.label().unwrap_or(""));
                        let label = NSTextField::labelWithString(&value, mtm);
                        label.setFrame(frame);
                        label.setTag(placement.widget().get() as isize);
                        Retained::into_super(label)
                    }
                    WidgetKind::TextArea => {
                        // A multi-line editable NSTextField rather than an
                        // NSTextView: it is an NSControl like every other
                        // lowered widget, so it fits the existing surface
                        // without a second storage path, and it is genuinely
                        // editable, which is what a note editor needs.
                        let value = NSString::from_str(placement.label().unwrap_or(""));
                        let field = NSTextField::textFieldWithString(&value, mtm);
                        field.setFrame(frame);
                        field.setTag(placement.widget().get() as isize);
                        field.setEditable(true);
                        field.setSelectable(true);
                        Retained::into_super(field)
                    }
                    WidgetKind::ListView => {
                        // The container itself paints nothing on macOS: its
                        // rows are separate Text placements that lower to
                        // labels. Lowering it as a non-editable, empty label
                        // keeps ids and hit testing consistent without drawing
                        // a box over the rows.
                        let empty = NSString::from_str("");
                        let label = NSTextField::labelWithString(&empty, mtm);
                        label.setFrame(frame);
                        label.setTag(placement.widget().get() as isize);
                        Retained::into_super(label)
                    }
                    other => {
                        return Err(UiAdapterError::Unsupported(format!(
                            "AppKit lowering does not support {other:?} yet"
                        )));
                    }
                };

                content_view.addSubview(&control);
                controls.insert(placement.widget(), control);
                kinds.insert(placement.widget(), placement.kind());
                lowered.push(placement.widget());
            }

            Ok(AppKitWidgetSurface {
                window: self.id,
                _target: target,
                controls,
                kinds,
                lowered,
            })
        }
    }

    impl AppKitWindowNativeDelegate {
        fn new(mtm: MainThreadMarker) -> Self {
            let callbacks = Rc::new(RefCell::new(AppKitWindowDelegateQueue::new()));
            let delegate = KrateWindowDelegate::new(mtm, Rc::clone(&callbacks));
            Self {
                delegate,
                callbacks,
            }
        }

        /// Return the number of queued AppKit callbacks.
        pub fn pending_callbacks(&self) -> usize {
            self.callbacks.borrow().len()
        }

        /// Return whether no AppKit callbacks are waiting.
        pub fn is_empty(&self) -> bool {
            self.callbacks.borrow().is_empty()
        }

        /// Drain queued AppKit callbacks in delivery order.
        pub fn drain_callbacks(&self) -> Vec<AppKitWindowDelegateCallback> {
            self.callbacks.borrow_mut().drain()
        }
    }

    impl AppKitDrawViewSurface {
        /// Return the stable Krate window id this view belongs to.
        pub fn window(&self) -> WindowId {
            self.snapshot.window
        }

        /// Return the opaque AppKit `NSView` pointer for diagnostics.
        pub fn native_view_handle(&self) -> u64 {
            self.snapshot.native_view_handle
        }

        /// Return the first frame recorded when the view was attached.
        pub fn frame(&self) -> AppKitDrawFrame {
            self.snapshot.frame
        }

        /// Return the latest view-surface snapshot.
        pub fn snapshot(&self) -> AppKitDrawViewSurfaceSnapshot {
            self.snapshot
        }

        /// Mark the AppKit view dirty and record another draw frame.
        pub fn request_display(
            &mut self,
            state: &mut AppKitDrawSurfaceState,
        ) -> AppKitDrawViewSurfaceSnapshot {
            self.view.setNeedsDisplay(true);
            self.snapshot = AppKitDrawViewSurfaceSnapshot {
                window: state.window(),
                native_view_handle: self.native_view_handle(),
                frame: state.record_frame(),
                needs_display: true,
            };
            self.snapshot
        }
    }

    impl AppKitWindowBackend {
        /// Create a real AppKit `NSWindow` and attach it to a Krate window id.
        pub fn create_window(
            &self,
            adapter: &MacosUiAdapter,
            options: WindowOptions,
        ) -> Result<AppKitWindowPrototype, UiAdapterError> {
            let mtm = main_thread_marker()?;
            let app = NSApplication::sharedApplication(mtm);
            let native_window = create_native_window(mtm, &options);
            let raw_handle = Retained::as_ptr(&native_window) as usize as u64;
            let id = WindowAdapter::create_window(adapter, options)?;
            let native_handle = adapter.attach_appkit_window_handle(id, raw_handle)?;

            drop(app);

            Ok(AppKitWindowPrototype {
                id,
                native_handle,
                window: native_window,
            })
        }

        /// Show the AppKit window and mark the Krate window visible.
        ///
        /// An unbundled process defaults to a background activation policy,
        /// under which the WindowServer never displays its windows. Promote
        /// the process to a regular app and activate it so the window really
        /// appears in front of the user.
        pub fn show_window(
            &self,
            adapter: &MacosUiAdapter,
            window: &AppKitWindowPrototype,
        ) -> Result<(), UiAdapterError> {
            let mtm = main_thread_marker()?;
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            window.window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            WindowAdapter::show_window(adapter, window.id)
        }
    }

    impl AppKitWindowPrototype {
        /// Drain and dispatch pending NSApplication events without blocking.
        ///
        /// Real mouse and keyboard input only reaches AppKit views when the
        /// process pumps the application event queue. This is the manual,
        /// non-blocking equivalent of one `NSApplication::run` turn, called
        /// from the shared event-loop pump each tick.
        pub fn pump_app_events(&self) -> Result<usize, UiAdapterError> {
            let mtm = main_thread_marker()?;
            let app = NSApplication::sharedApplication(mtm);
            let mut dispatched = 0usize;

            loop {
                // SAFETY: Called on the main thread; a nil date means "do not
                // wait", so this never blocks the runtime.
                let event = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        None,
                        NSDefaultRunLoopMode,
                        true,
                    )
                };
                let Some(event) = event else {
                    break;
                };
                app.sendEvent(&event);
                dispatched += 1;
            }
            app.updateWindows();

            Ok(dispatched)
        }
    }

    fn main_thread_marker() -> Result<MainThreadMarker, UiAdapterError> {
        MainThreadMarker::new().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "AppKit windows must be created on the macOS main thread".to_string(),
            )
        })
    }

    fn create_native_window(mtm: MainThreadMarker, options: &WindowOptions) -> Retained<NSWindow> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(options.size.width as f64, options.size.height as f64),
        );

        // SAFETY: AppKit requires NSWindow allocation and initialization on the
        // main thread. The caller holds MainThreadMarker, and objc2 keeps the
        // returned NSWindow retained while AppKitWindowPrototype is alive.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        let title = NSString::from_str(&options.title);
        window.setTitle(&title);
        window.center();
        window
    }

    fn create_draw_view(mtm: MainThreadMarker, rect: NSRect) -> Retained<NSView> {
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect);
        view.setWantsLayer(true);
        view
    }

    fn ns_color_from_appkit_color(color: AppKitColor) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            color.red as f64,
            color.green as f64,
            color.blue as f64,
            color.alpha as f64,
        )
    }

    fn notification_window(notification: &NSNotification) -> Option<Retained<NSWindow>> {
        notification.object()?.downcast::<NSWindow>().ok()
    }

    fn size_from_rect(rect: NSRect) -> Result<WindowSize, UiAdapterError> {
        let width = logical_edge_to_u32(rect.size.width);
        let height = logical_edge_to_u32(rect.size.height);
        WindowSize::new(width, height)
    }

    fn logical_edge_to_u32(value: f64) -> u32 {
        if !value.is_finite() || value < 1.0 || value > u32::MAX as f64 {
            return 0;
        }

        value.round() as u32
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use krate_adapter_common::ui::WindowSize;

        #[test]
        fn appkit_backend_reports_native_target() {
            let backend = AppKitWindowBackend;
            assert_eq!(backend.backend_kind(), WindowBackendKind::AppKit);
            assert!(backend.is_available());
        }

        #[test]
        fn appkit_main_thread_gate_is_explicit() {
            let backend = AppKitWindowBackend;
            assert!(backend.is_available());
            let options =
                WindowOptions::new("Krate AppKit prototype", WindowSize::new(640, 480).unwrap())
                    .unwrap();
            assert_eq!(options.title, "Krate AppKit prototype");
        }

        #[test]
        #[ignore = "opens a real AppKit window on the local macOS desktop"]
        fn ignored_smoke_can_create_and_show_appkit_window() {
            let adapter = MacosUiAdapter::new();
            let backend = AppKitWindowBackend;
            let options =
                WindowOptions::new("Krate AppKit prototype", WindowSize::new(640, 480).unwrap())
                    .unwrap();
            let window = backend
                .create_window(&adapter, options)
                .expect("create appkit window");
            backend
                .show_window(&adapter, &window)
                .expect("show appkit window");
            assert_eq!(window.native_handle().backend, WindowBackendKind::AppKit);
        }

        #[test]
        #[ignore = "opens a real AppKit window on the local macOS desktop"]
        fn ignored_smoke_can_snapshot_and_sync_appkit_window_state() {
            let adapter = MacosUiAdapter::new();
            let backend = AppKitWindowBackend;
            let options = WindowOptions::new(
                "Krate AppKit event bridge",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap();
            let window = backend
                .create_window(&adapter, options)
                .expect("create appkit window");
            let snapshot = backend
                .sync_window_state(&adapter, &window, None)
                .expect("sync appkit state");

            assert_eq!(snapshot.size, WindowSize::new(640, 480).unwrap());
            assert!(snapshot.scale > 0.0);
        }

        #[test]
        #[ignore = "opens a real AppKit window on the local macOS desktop"]
        fn ignored_smoke_can_refresh_appkit_window_session() {
            let adapter = MacosUiAdapter::new();
            let backend = AppKitWindowBackend;
            let options =
                WindowOptions::new("Krate AppKit session", WindowSize::new(640, 480).unwrap())
                    .unwrap();
            let mut session = backend
                .create_session(&adapter, options)
                .expect("create appkit session");

            session
                .show(&backend, &adapter)
                .expect("show appkit session");
            let snapshot = session
                .refresh(&backend, &adapter)
                .expect("refresh appkit session");

            assert_eq!(session.id(), session.window().id());
            assert_eq!(session.native_handle().backend, WindowBackendKind::AppKit);
            assert_eq!(session.last_snapshot(), Some(snapshot));
            assert!(snapshot.scale > 0.0);
        }

        #[test]
        #[ignore = "opens a real AppKit window on the local macOS desktop"]
        fn ignored_smoke_can_attach_appkit_draw_view_surface() {
            let adapter = MacosUiAdapter::new();
            let backend = AppKitWindowBackend;
            let options =
                WindowOptions::new("Krate AppKit draw view", WindowSize::new(640, 480).unwrap())
                    .unwrap();
            let mut session = backend
                .create_session(&adapter, options)
                .expect("create appkit session");
            session
                .refresh(&backend, &adapter)
                .expect("refresh appkit session");
            let mut state = session
                .create_draw_surface_state(AppKitColor::KRATE_BLUE)
                .expect("draw surface state");
            let mut view = session
                .attach_draw_view_surface(&mut state)
                .expect("attach draw view");

            assert_eq!(view.window(), session.id());
            assert_ne!(view.native_view_handle(), 0);
            assert_eq!(view.frame().frame_index, 1);

            let updated = view.request_display(&mut state);
            assert_eq!(updated.frame.frame_index, 2);
            assert!(updated.needs_display);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    /// Placeholder returned only on macOS builds.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AppKitWindowPrototype {
        id: WindowId,
        native_handle: NativeWindowHandle,
    }

    /// Placeholder returned only on macOS builds.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct AppKitDrawViewSurface {
        snapshot: AppKitDrawViewSurfaceSnapshot,
    }

    /// Placeholder returned only on macOS builds.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AppKitWindowNativeDelegate;

    /// Placeholder returned only on macOS builds.
    #[derive(Debug, Clone, PartialEq)]
    pub struct AppKitWidgetSurface {
        snapshot: AppKitWidgetSurfaceSnapshot,
    }

    impl AppKitWidgetSurface {
        /// Return the lowering result snapshot for this surface.
        pub fn snapshot(&self) -> AppKitWidgetSurfaceSnapshot {
            self.snapshot.clone()
        }

        /// Return how many native controls this surface owns.
        pub fn widget_count(&self) -> usize {
            0
        }

        /// Native control handles are only available on macOS.
        pub fn native_control_handle(&self, _widget: WidgetId) -> Option<u64> {
            None
        }

        /// AppKit text updates are only available on macOS.
        pub fn set_text(&self, _widget: WidgetId, _text: &str) -> Result<(), UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit widget text is only available on macOS".to_string(),
            ))
        }

        /// AppKit text reads are only available on macOS.
        pub fn text(&self, _widget: WidgetId) -> Result<String, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit widget text is only available on macOS".to_string(),
            ))
        }

        /// AppKit clicks are only available on macOS.
        pub fn perform_click(&self, _widget: WidgetId) -> Result<(), UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit widget clicks are only available on macOS".to_string(),
            ))
        }
    }

    impl AppKitWindowPrototype {
        /// AppKit widget lowering is only available in macOS builds.
        pub fn lower_widget_placements(
            &self,
            _placements: &[AppKitWidgetPlacement],
            _delegate: &AppKitWindowNativeDelegate,
        ) -> Result<AppKitWidgetSurface, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit widget lowering is only available on macOS".to_string(),
            ))
        }
        /// Return the stable Krate window id.
        pub fn id(&self) -> WindowId {
            self.id
        }

        /// Return the opaque AppKit handle attached to the shared window registry.
        pub fn native_handle(&self) -> NativeWindowHandle {
            self.native_handle
        }

        /// AppKit snapshots are only available in macOS builds.
        pub fn snapshot(&self) -> Result<AppKitWindowSnapshot, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit window snapshots are only available on macOS".to_string(),
            ))
        }

        /// AppKit draw views are only available in macOS builds.
        pub fn attach_draw_view_surface(
            &self,
            _state: &mut AppKitDrawSurfaceState,
        ) -> Result<AppKitDrawViewSurface, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit draw views are only available on macOS".to_string(),
            ))
        }

        /// AppKit delegates are only available in macOS builds.
        pub fn install_native_delegate(
            &self,
        ) -> Result<AppKitWindowNativeDelegate, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit window delegates are only available on macOS".to_string(),
            ))
        }

        /// AppKit event pumping is only available in macOS builds.
        pub fn pump_app_events(&self) -> Result<usize, UiAdapterError> {
            Ok(0)
        }
    }

    impl AppKitDrawViewSurface {
        /// Return the stable Krate window id this view belongs to.
        pub fn window(&self) -> WindowId {
            self.snapshot.window
        }

        /// Return the opaque native view handle.
        pub fn native_view_handle(&self) -> u64 {
            self.snapshot.native_view_handle
        }

        /// Return the last frame snapshot.
        pub fn frame(&self) -> AppKitDrawFrame {
            self.snapshot.frame
        }

        /// Return the latest view-surface snapshot.
        pub fn snapshot(&self) -> AppKitDrawViewSurfaceSnapshot {
            self.snapshot
        }

        /// Marking an AppKit view dirty is only available in macOS builds.
        pub fn request_display(
            &mut self,
            _state: &mut AppKitDrawSurfaceState,
        ) -> AppKitDrawViewSurfaceSnapshot {
            self.snapshot
        }
    }

    impl AppKitWindowNativeDelegate {
        /// Return the number of queued AppKit callbacks.
        pub fn pending_callbacks(&self) -> usize {
            0
        }

        /// Return whether no AppKit callbacks are waiting.
        pub fn is_empty(&self) -> bool {
            true
        }

        /// Drain queued AppKit callbacks in delivery order.
        pub fn drain_callbacks(&self) -> Vec<AppKitWindowDelegateCallback> {
            Vec::new()
        }
    }

    impl AppKitWindowBackend {
        /// AppKit is only available in macOS builds.
        pub fn create_window(
            &self,
            _adapter: &MacosUiAdapter,
            _options: WindowOptions,
        ) -> Result<AppKitWindowPrototype, UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit windows are only available on macOS".to_string(),
            ))
        }

        /// AppKit is only available in macOS builds.
        pub fn show_window(
            &self,
            _adapter: &MacosUiAdapter,
            _window: &AppKitWindowPrototype,
        ) -> Result<(), UiAdapterError> {
            Err(UiAdapterError::Unsupported(
                "AppKit windows are only available on macOS".to_string(),
            ))
        }
    }
}

pub use platform::AppKitDrawViewSurface;
pub use platform::AppKitWidgetSurface;
pub use platform::AppKitWindowNativeDelegate;
pub use platform::AppKitWindowPrototype;

fn validate_color_channel(name: &str, value: f32) -> Result<(), UiAdapterError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(UiAdapterError::InvalidWidgetStyle(format!(
            "AppKit color channel {name} must be finite and between 0 and 1"
        )));
    }

    Ok(())
}

fn validate_scale_factor(value: f32) -> Result<(), UiAdapterError> {
    if !value.is_finite() || value <= 0.0 || value > 8.0 {
        return Err(UiAdapterError::InvalidScaleFactor(format!(
            "{value} must be finite and between 0 and 8"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_adapter_common::ui::UiEvent;

    #[test]
    fn appkit_widget_placement_validates_kind_and_geometry() {
        let widget = WidgetId::new(7).unwrap();

        assert!(AppKitWidgetPlacement::new(
            widget,
            WidgetKind::Button,
            Some("Save".to_string()),
            10.0,
            20.0,
            120.0,
            32.0,
        )
        .is_ok());

        assert!(matches!(
            AppKitWidgetPlacement::new(widget, WidgetKind::Slider, None, 0.0, 0.0, 10.0, 10.0),
            Err(UiAdapterError::Unsupported(_))
        ));
        assert!(matches!(
            AppKitWidgetPlacement::new(widget, WidgetKind::Button, None, -1.0, 0.0, 10.0, 10.0),
            Err(UiAdapterError::Unsupported(_))
        ));
        assert!(matches!(
            AppKitWidgetPlacement::new(widget, WidgetKind::Button, None, 0.0, 0.0, 0.0, 10.0),
            Err(UiAdapterError::Unsupported(_))
        ));
        assert!(matches!(
            AppKitWidgetPlacement::new(widget, WidgetKind::Button, None, f32::NAN, 0.0, 10.0, 10.0),
            Err(UiAdapterError::Unsupported(_))
        ));
    }

    #[test]
    fn appkit_widget_placement_flips_y_for_appkit_origin() {
        let widget = WidgetId::new(3).unwrap();
        let placement = AppKitWidgetPlacement::new(
            widget,
            WidgetKind::TextField,
            None,
            16.0,
            24.0,
            200.0,
            28.0,
        )
        .unwrap();

        // Top-left logical y=24 with height 28 inside a 480-tall content view
        // must land at AppKit bottom-left origin y = 480 - (24 + 28) = 428.
        assert_eq!(placement.appkit_origin_y(480.0), 428.0);
        assert_eq!(placement.rect(), (16.0, 24.0, 200.0, 28.0));
    }

    #[test]
    fn appkit_widget_activation_queues_routed_pointer_event() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit activation",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let widget = WidgetId::new(42).unwrap();
        UiAdapter::set_root(
            &adapter,
            id,
            krate_adapter_common::ui::WidgetNode {
                id: widget,
                parent: None,
                kind: WidgetKind::Button,
                label: Some("Save".to_string()),
                role: None,
                style: krate_adapter_common::ui::WidgetStyle::default(),
                checked: None,
                value: None,
                selected: None,
            },
        )
        .expect("set root widget");
        let mut state = AppKitWindowEventState::new(id);
        let bridge = AppKitWindowDelegateBridge;

        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WidgetActivated(widget),
            )
            .expect("widget activation");

        let events = WindowAdapter::drain_events(&adapter).expect("events");
        assert_eq!(
            events.last(),
            Some(&UiEvent::Pointer(PointerEvent {
                window: id,
                widget: Some(widget),
                x: 0.0,
                y: 0.0,
                button: Some(PointerButton::Primary),
                pressed: true,
                modifiers: Modifiers::default(),
            }))
        );
    }

    #[test]
    fn appkit_event_bridge_queues_shared_window_events_by_id() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit events", WindowSize::new(640, 480).unwrap()).unwrap(),
        )
        .expect("create window");
        let resized = WindowSize::new(800, 600).unwrap();

        backend
            .report_resized_for_id(&adapter, id, resized)
            .expect("resize");
        backend
            .report_focused_for_id(&adapter, id, true)
            .expect("focus");
        backend
            .report_scale_changed_for_id(&adapter, id, 2.0)
            .expect("scale");
        backend
            .report_close_requested_for_id(&adapter, id)
            .expect("close request");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::Resized { id, size: resized },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::ScaleChanged { id, scale: 2.0 },
                UiEvent::WindowCloseRequested(id),
            ]
        );
    }

    #[test]
    fn appkit_event_bridge_reuses_shared_scale_validation() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit scale", WindowSize::new(640, 480).unwrap()).unwrap(),
        )
        .expect("create window");

        assert!(matches!(
            backend.report_scale_changed_for_id(&adapter, id, 0.0),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
    }

    #[test]
    fn appkit_snapshot_sync_queues_only_changed_window_state() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit snapshot", WindowSize::new(640, 480).unwrap())
                .unwrap(),
        )
        .expect("create window");
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };

        backend
            .sync_snapshot_for_id(&adapter, id, first, None)
            .expect("sync first snapshot");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::Resized {
                    id,
                    size: first.size
                },
                UiEvent::WindowFocused { id, focused: false },
                UiEvent::ScaleChanged { id, scale: 1.0 },
            ]
        );

        backend
            .sync_snapshot_for_id(&adapter, id, first, Some(first))
            .expect("sync unchanged snapshot");
        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![]
        );

        let changed = AppKitWindowSnapshot {
            visible: true,
            focused: true,
            size: WindowSize::new(800, 600).unwrap(),
            scale: 2.0,
        };

        backend
            .sync_snapshot_for_id(&adapter, id, changed, Some(first))
            .expect("sync changed snapshot");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::Resized {
                    id,
                    size: changed.size
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::ScaleChanged { id, scale: 2.0 },
            ]
        );
    }

    #[test]
    fn appkit_event_state_handles_delegate_shaped_events() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit delegate", WindowSize::new(640, 480).unwrap())
                .unwrap(),
        )
        .expect("create window");
        let mut state = AppKitWindowEventState::new(id);
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };

        state
            .handle_native_event(&backend, &adapter, AppKitWindowNativeEvent::Snapshot(first))
            .expect("initial snapshot");
        WindowAdapter::drain_events(&adapter).expect("drain initial snapshot");

        state
            .handle_native_event(
                &backend,
                &adapter,
                AppKitWindowNativeEvent::Resized(WindowSize::new(800, 600).unwrap()),
            )
            .expect("resize callback");
        state
            .handle_native_event(&backend, &adapter, AppKitWindowNativeEvent::Focused(true))
            .expect("focus callback");
        state
            .handle_native_event(
                &backend,
                &adapter,
                AppKitWindowNativeEvent::ScaleChanged(2.0),
            )
            .expect("scale callback");
        state
            .handle_native_event(&backend, &adapter, AppKitWindowNativeEvent::RedrawRequested)
            .expect("redraw callback");
        state
            .handle_native_event(&backend, &adapter, AppKitWindowNativeEvent::CloseRequested)
            .expect("close callback");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::Resized {
                    id,
                    size: WindowSize::new(800, 600).unwrap()
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::ScaleChanged { id, scale: 2.0 },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowCloseRequested(id),
            ]
        );
        assert_eq!(
            state.last_snapshot(),
            Some(AppKitWindowSnapshot {
                visible: true,
                focused: true,
                size: WindowSize::new(800, 600).unwrap(),
                scale: 2.0,
            })
        );
    }

    #[test]
    fn appkit_event_state_does_not_cache_failed_scale_event() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit bad scale", WindowSize::new(640, 480).unwrap())
                .unwrap(),
        )
        .expect("create window");
        let mut state = AppKitWindowEventState::new(id);
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };

        state
            .handle_native_event(&backend, &adapter, AppKitWindowNativeEvent::Snapshot(first))
            .expect("initial snapshot");
        WindowAdapter::drain_events(&adapter).expect("drain initial snapshot");

        assert!(matches!(
            state.handle_native_event(
                &backend,
                &adapter,
                AppKitWindowNativeEvent::ScaleChanged(0.0),
            ),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
        assert_eq!(state.last_snapshot(), Some(first));
    }

    #[test]
    fn appkit_delegate_bridge_translates_callbacks_to_native_event_state() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let bridge = AppKitWindowDelegateBridge;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit delegate bridge",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let mut state = AppKitWindowEventState::new(id);
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };

        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::Snapshot(first),
            )
            .expect("initial snapshot");
        WindowAdapter::drain_events(&adapter).expect("drain initial snapshot");

        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowDidResize(WindowSize::new(900, 700).unwrap()),
            )
            .expect("resize callback");
        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowDidBecomeKey,
            )
            .expect("become key callback");
        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowDidResignKey,
            )
            .expect("resign key callback");
        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowDidChangeBackingScale(2.0),
            )
            .expect("scale callback");
        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::ViewNeedsDisplay,
            )
            .expect("display callback");
        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowShouldClose,
            )
            .expect("close callback");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::Resized {
                    id,
                    size: WindowSize::new(900, 700).unwrap()
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::WindowFocused { id, focused: false },
                UiEvent::ScaleChanged { id, scale: 2.0 },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowCloseRequested(id),
            ]
        );
        assert_eq!(
            state.last_snapshot(),
            Some(AppKitWindowSnapshot {
                visible: true,
                focused: false,
                size: WindowSize::new(900, 700).unwrap(),
                scale: 2.0,
            })
        );
    }

    #[test]
    fn appkit_delegate_bridge_reuses_failed_scale_validation() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let bridge = AppKitWindowDelegateBridge;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit bad delegate scale",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let mut state = AppKitWindowEventState::new(id);
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };

        bridge
            .handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::Snapshot(first),
            )
            .expect("initial snapshot");
        WindowAdapter::drain_events(&adapter).expect("drain initial snapshot");

        assert!(matches!(
            bridge.handle_callback(
                &backend,
                &adapter,
                &mut state,
                AppKitWindowDelegateCallback::WindowDidChangeBackingScale(0.0),
            ),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
        assert_eq!(state.last_snapshot(), Some(first));
    }

    #[test]
    fn appkit_event_loop_step_feeds_snapshot_callbacks_and_redraw() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let driver = AppKitWindowEventLoopDriver::new();
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit event-loop step",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        WindowAdapter::drain_events(&adapter).expect("drain creation");
        let mut state = AppKitWindowEventState::new(id);
        let first = AppKitWindowSnapshot {
            visible: true,
            focused: false,
            size: WindowSize::new(640, 480).unwrap(),
            scale: 1.0,
        };
        let resized = WindowSize::new(900, 700).unwrap();
        let step = AppKitWindowEventLoopStep::new()
            .with_snapshot(first)
            .with_callbacks([
                AppKitWindowDelegateCallback::WindowDidBecomeKey,
                AppKitWindowDelegateCallback::WindowDidResize(resized),
                AppKitWindowDelegateCallback::WindowDidChangeBackingScale(2.0),
            ])
            .with_redraw_request();

        let report = driver
            .run_step_for_state(&backend, &adapter, &mut state, &step)
            .expect("event-loop step");

        assert_eq!(
            report,
            AppKitWindowEventLoopStepReport {
                callbacks_handled: 3,
                snapshot: Some(AppKitWindowSnapshot {
                    visible: true,
                    focused: true,
                    size: resized,
                    scale: 2.0,
                }),
                redraw_requested: true,
            }
        );
        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![
                UiEvent::Resized {
                    id,
                    size: first.size
                },
                UiEvent::WindowFocused { id, focused: false },
                UiEvent::ScaleChanged {
                    id,
                    scale: first.scale
                },
                UiEvent::WindowFocused { id, focused: true },
                UiEvent::Resized { id, size: resized },
                UiEvent::ScaleChanged { id, scale: 2.0 },
                UiEvent::RedrawRequested(id),
            ]
        );
    }

    #[test]
    fn appkit_event_loop_empty_step_is_noop() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let driver = AppKitWindowEventLoopDriver::new();
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit empty event-loop step",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        WindowAdapter::drain_events(&adapter).expect("drain creation");
        let mut state = AppKitWindowEventState::new(id);
        let step = AppKitWindowEventLoopStep::new();

        let report = driver
            .run_step_for_state(&backend, &adapter, &mut state, &step)
            .expect("event-loop step");

        assert_eq!(
            report,
            AppKitWindowEventLoopStepReport {
                callbacks_handled: 0,
                snapshot: None,
                redraw_requested: false,
            }
        );
        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![]
        );
    }

    #[test]
    fn appkit_delegate_queue_drains_callbacks_in_order() {
        let mut queue = AppKitWindowDelegateQueue::new();
        let size = WindowSize::new(900, 700).unwrap();

        queue.push(AppKitWindowDelegateCallback::WindowDidBecomeKey);
        queue.push(AppKitWindowDelegateCallback::WindowDidResize(size));
        queue.push(AppKitWindowDelegateCallback::WindowShouldClose);

        assert_eq!(queue.len(), 3);
        assert!(!queue.is_empty());
        assert_eq!(
            queue.drain(),
            vec![
                AppKitWindowDelegateCallback::WindowDidBecomeKey,
                AppKitWindowDelegateCallback::WindowDidResize(size),
                AppKitWindowDelegateCallback::WindowShouldClose,
            ]
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn appkit_redraw_bridge_queues_shared_redraw_event() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Krate AppKit redraw", WindowSize::new(640, 480).unwrap()).unwrap(),
        )
        .expect("create window");

        backend
            .report_redraw_requested_for_id(&adapter, id)
            .expect("redraw by id");

        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![UiEvent::WindowCreated(id), UiEvent::RedrawRequested(id)]
        );
    }

    #[test]
    fn appkit_draw_surface_records_frames() {
        let adapter = MacosUiAdapter::new();
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit draw surface",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let mut surface = AppKitDrawSurfaceState::new(
            id,
            WindowSize::new(640, 480).unwrap(),
            2.0,
            AppKitColor::KRATE_BLUE,
        )
        .expect("surface state");

        let first = surface.record_frame();
        let resized = WindowSize::new(800, 600).unwrap();
        surface.resize(resized);
        surface.scale_changed(1.5).expect("scale changed");
        let second = surface.record_frame();

        assert_eq!(
            first,
            AppKitDrawFrame {
                window: id,
                size: WindowSize::new(640, 480).unwrap(),
                scale: 2.0,
                clear_color: AppKitColor::KRATE_BLUE,
                frame_index: 1,
            }
        );
        assert_eq!(
            second,
            AppKitDrawFrame {
                window: id,
                size: resized,
                scale: 1.5,
                clear_color: AppKitColor::KRATE_BLUE,
                frame_index: 2,
            }
        );
        assert_eq!(surface.frame_index(), 2);
        assert_eq!(surface.size(), resized);
        assert_eq!(surface.scale(), 1.5);
    }

    #[test]
    fn appkit_draw_surface_requests_redraw_through_delegate_bridge() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let bridge = AppKitWindowDelegateBridge;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit draw redraw",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let mut event_state = AppKitWindowEventState::new(id);
        let mut surface = AppKitDrawSurfaceState::new(
            id,
            WindowSize::new(640, 480).unwrap(),
            1.0,
            AppKitColor::KRATE_BLUE,
        )
        .expect("surface state");

        surface
            .request_redraw(&bridge, &backend, &adapter, &mut event_state)
            .expect("request redraw");

        assert_eq!(surface.redraws_requested(), 1);
        assert_eq!(
            WindowAdapter::drain_events(&adapter).expect("events"),
            vec![UiEvent::WindowCreated(id), UiEvent::RedrawRequested(id)]
        );
    }

    #[test]
    fn appkit_draw_surface_validates_color_and_scale() {
        assert!(AppKitColor::new(0.0, 0.5, 1.0, 1.0).is_ok());
        assert!(matches!(
            AppKitColor::new(0.0, f32::NAN, 1.0, 1.0),
            Err(UiAdapterError::InvalidWidgetStyle(_))
        ));

        let adapter = MacosUiAdapter::new();
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new(
                "Krate AppKit draw validation",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        assert!(matches!(
            AppKitDrawSurfaceState::new(
                id,
                WindowSize::new(640, 480).unwrap(),
                0.0,
                AppKitColor::KRATE_BLUE,
            ),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
    }
}
