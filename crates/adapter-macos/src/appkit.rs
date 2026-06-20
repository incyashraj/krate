//! Opt-in AppKit window prototype for the macOS adapter.
//!
//! The default macOS adapter still uses the headless draft backend. This module
//! is the first native path behind the checked handle handoff: it creates an
//! AppKit `NSWindow`, keeps that object alive, and binds its raw pointer to a
//! stable Layer36 `WindowId`.

use layer36_adapter_common::ui::{
    NativeWindowHandle, UiAdapterError, WindowAdapter, WindowBackendKind, WindowId, WindowOptions,
    WindowSize,
};

use crate::MacosUiAdapter;

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
    /// Default Layer36 blue used by early drawing-surface tests.
    pub const LAYER36_BLUE: Self = Self {
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
    /// Create drawing-surface state for one AppKit-backed Layer36 window.
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

    /// Return the Layer36 window this surface belongs to.
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
        };

        state.handle_native_event(backend, adapter, event)
    }
}

/// Mutable native event-loop state for one AppKit-backed Layer36 window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppKitWindowEventState {
    id: WindowId,
    last_snapshot: Option<AppKitWindowSnapshot>,
}

impl AppKitWindowEventState {
    /// Create event-loop state for an AppKit-backed Layer36 window.
    pub fn new(id: WindowId) -> Self {
        Self {
            id,
            last_snapshot: None,
        }
    }

    /// Return the Layer36 window id this state belongs to.
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
        }
    }
}

/// Owned AppKit window plus the last native state seen by Layer36.
pub struct AppKitWindowSession {
    window: AppKitWindowPrototype,
    event_state: AppKitWindowEventState,
}

impl AppKitWindowSession {
    /// Return the stable Layer36 window id for this native window session.
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

    /// Show the native window through AppKit and the shared Layer36 adapter.
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

    /// Queue a native AppKit event into the shared Layer36 event stream.
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
        })
    }

    /// Queue a native close request for a Layer36 window id.
    pub fn report_close_requested_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_close_requested(adapter, id)
    }

    /// Queue a native resize for a Layer36 window id.
    pub fn report_resized_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        size: WindowSize,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_host_resize(adapter, id, size)
    }

    /// Queue a native focus change for a Layer36 window id.
    pub fn report_focused_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        focused: bool,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_window_focused(adapter, id, focused)
    }

    /// Queue a native display scale change for a Layer36 window id.
    pub fn report_scale_changed_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
        scale: f32,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::queue_scale_changed(adapter, id, scale)
    }

    /// Queue a native redraw request for a Layer36 window id.
    pub fn report_redraw_requested_for_id(
        &self,
        adapter: &MacosUiAdapter,
        id: WindowId,
    ) -> Result<(), UiAdapterError> {
        WindowAdapter::request_redraw(adapter, id)
    }

    /// Queue changed native state for a Layer36 window id.
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
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSColor, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    /// Owned AppKit window bound to one Layer36 window id.
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

    impl AppKitWindowPrototype {
        /// Return the stable Layer36 window id.
        pub fn id(&self) -> WindowId {
            self.id
        }

        /// Return the opaque AppKit handle attached to the shared window registry.
        pub fn native_handle(&self) -> NativeWindowHandle {
            self.native_handle
        }

        /// Read current AppKit window state without draining the Layer36 queue.
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
    }

    impl AppKitDrawViewSurface {
        /// Return the stable Layer36 window id this view belongs to.
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
        /// Create a real AppKit `NSWindow` and attach it to a Layer36 window id.
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

        /// Show the AppKit window and mark the Layer36 window visible.
        pub fn show_window(
            &self,
            adapter: &MacosUiAdapter,
            window: &AppKitWindowPrototype,
        ) -> Result<(), UiAdapterError> {
            let _mtm = main_thread_marker()?;
            window.window.makeKeyAndOrderFront(None);
            WindowAdapter::show_window(adapter, window.id)
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
        use layer36_adapter_common::ui::WindowSize;

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
            let options = WindowOptions::new(
                "Layer36 AppKit prototype",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap();
            assert_eq!(options.title, "Layer36 AppKit prototype");
        }

        #[test]
        #[ignore = "opens a real AppKit window on the local macOS desktop"]
        fn ignored_smoke_can_create_and_show_appkit_window() {
            let adapter = MacosUiAdapter::new();
            let backend = AppKitWindowBackend;
            let options = WindowOptions::new(
                "Layer36 AppKit prototype",
                WindowSize::new(640, 480).unwrap(),
            )
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
                "Layer36 AppKit event bridge",
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
                WindowOptions::new("Layer36 AppKit session", WindowSize::new(640, 480).unwrap())
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
            let options = WindowOptions::new(
                "Layer36 AppKit draw view",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap();
            let mut session = backend
                .create_session(&adapter, options)
                .expect("create appkit session");
            session
                .refresh(&backend, &adapter)
                .expect("refresh appkit session");
            let mut state = session
                .create_draw_surface_state(AppKitColor::LAYER36_BLUE)
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

    impl AppKitWindowPrototype {
        /// Return the stable Layer36 window id.
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
    }

    impl AppKitDrawViewSurface {
        /// Return the stable Layer36 window id this view belongs to.
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
    use layer36_adapter_common::ui::UiEvent;

    #[test]
    fn appkit_event_bridge_queues_shared_window_events_by_id() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Layer36 AppKit events", WindowSize::new(640, 480).unwrap())
                .unwrap(),
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
            WindowOptions::new("Layer36 AppKit scale", WindowSize::new(640, 480).unwrap()).unwrap(),
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
            WindowOptions::new(
                "Layer36 AppKit snapshot",
                WindowSize::new(640, 480).unwrap(),
            )
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
            WindowOptions::new(
                "Layer36 AppKit delegate",
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
            WindowOptions::new(
                "Layer36 AppKit bad scale",
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
                "Layer36 AppKit delegate bridge",
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
                "Layer36 AppKit bad delegate scale",
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
    fn appkit_redraw_bridge_queues_shared_redraw_event() {
        let adapter = MacosUiAdapter::new();
        let backend = AppKitWindowBackend;
        let id = WindowAdapter::create_window(
            &adapter,
            WindowOptions::new("Layer36 AppKit redraw", WindowSize::new(640, 480).unwrap())
                .unwrap(),
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
                "Layer36 AppKit draw surface",
                WindowSize::new(640, 480).unwrap(),
            )
            .unwrap(),
        )
        .expect("create window");
        let mut surface = AppKitDrawSurfaceState::new(
            id,
            WindowSize::new(640, 480).unwrap(),
            2.0,
            AppKitColor::LAYER36_BLUE,
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
                clear_color: AppKitColor::LAYER36_BLUE,
                frame_index: 1,
            }
        );
        assert_eq!(
            second,
            AppKitDrawFrame {
                window: id,
                size: resized,
                scale: 1.5,
                clear_color: AppKitColor::LAYER36_BLUE,
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
                "Layer36 AppKit draw redraw",
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
            AppKitColor::LAYER36_BLUE,
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
                "Layer36 AppKit draw validation",
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
                AppKitColor::LAYER36_BLUE,
            ),
            Err(UiAdapterError::InvalidScaleFactor(_))
        ));
    }
}
