//! Real winit windowing and drawn-widget presentation for the Windows prototype adapter.
//!
//! The event loop is owned thread-locally and pumped non-blockingly with
//! `pump_app_events`, mirroring how the macOS crate owns its AppKit sessions.
//! Window creation is queued and performed inside the pump (winit only hands
//! out `ActiveEventLoop` inside callbacks), and native `WindowEvent`s are
//! mapped into the shared [`WinitWindowNativeEvent`] shape that the existing
//! collector and event-loop pump already drain.
//!
//! Windows always has a window station, so no display guard is needed; the
//! any-thread opt-in exists because cargo runs tests on worker threads and
//! winit refuses off-main-thread event loops by default.

#[cfg(target_os = "windows")]
pub use real::*;

#[cfg(not(target_os = "windows"))]
pub use stub::*;

#[cfg(target_os = "windows")]
use krate_adapter_common::ui::WidgetKind;
use krate_adapter_common::ui::{
    RawPointerSample, UiAdapterError, WidgetPlacement, WindowId, WindowSize,
    WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Krate window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

#[cfg(target_os = "windows")]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use std::num::NonZeroU32;
    use std::rc::Rc;

    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::window::{Window, WindowAttributes, WindowId as NativeWindowId};

    type DrawSurface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

    thread_local! {
        static WINIT_HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    }

    struct Host {
        event_loop: EventLoop<()>,
        app: PumpApp,
    }

    /// Whether the event loop has been created on this thread.
    ///
    /// Query and pump paths must never lazily create the loop: winit
    /// panics off the main thread, and an uninitialized host simply means
    /// no native windows exist yet.
    fn host_initialized() -> bool {
        WINIT_HOST.with(|slot| slot.borrow().is_some())
    }

    #[derive(Default)]
    struct PumpApp {
        pending_creates: Vec<PendingCreate>,
        windows: BTreeMap<NativeWindowId, TrackedWindow>,
        events: CollectedNativeEvents,
        cursor: BTreeMap<NativeWindowId, (f32, f32)>,
        pointer_samples: Vec<RawPointerSample>,
    }

    struct PendingCreate {
        krate: WindowId,
        title: String,
        size: WindowSize,
    }

    struct TrackedWindow {
        krate: WindowId,
        window: Rc<Window>,
        surface: Option<DrawSurface>,
        placements: Vec<WidgetPlacement>,
    }

    impl PumpApp {
        fn drain_pending_creates(&mut self, event_loop: &ActiveEventLoop) {
            for pending in self.pending_creates.drain(..) {
                let attributes = WindowAttributes::default()
                    .with_title(pending.title)
                    .with_inner_size(LogicalSize::new(
                        pending.size.width as f64,
                        pending.size.height as f64,
                    ))
                    .with_visible(false);
                if let Ok(window) = event_loop.create_window(attributes) {
                    self.windows.insert(
                        window.id(),
                        TrackedWindow {
                            krate: pending.krate,
                            window: Rc::new(window),
                            surface: None,
                            placements: Vec::new(),
                        },
                    );
                }
            }
        }

        fn krate_id(&self, native: NativeWindowId) -> Option<WindowId> {
            self.windows.get(&native).map(|tracked| tracked.krate)
        }
    }

    impl ApplicationHandler for PumpApp {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            self.drain_pending_creates(event_loop);
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            self.drain_pending_creates(event_loop);
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            native: NativeWindowId,
            event: WindowEvent,
        ) {
            let Some(krate) = self.krate_id(native) else {
                return;
            };
            let mapped = match event {
                WindowEvent::CloseRequested => Some(WinitWindowNativeEvent::CloseRequested),
                WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                    WindowSize::new(size.width, size.height)
                        .ok()
                        .map(WinitWindowNativeEvent::Resized)
                }
                WindowEvent::Focused(focused) => Some(WinitWindowNativeEvent::Focused(focused)),
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    Some(WinitWindowNativeEvent::ScaleChanged(scale_factor as f32))
                }
                WindowEvent::RedrawRequested => Some(WinitWindowNativeEvent::RedrawRequested),
                WindowEvent::CursorMoved { position, .. } => {
                    let scale = self
                        .windows
                        .get(&native)
                        .map(|tracked| tracked.window.scale_factor())
                        .unwrap_or(1.0);
                    self.cursor.insert(
                        native,
                        ((position.x / scale) as f32, (position.y / scale) as f32),
                    );
                    None
                }
                WindowEvent::MouseInput {
                    state,
                    button: winit::event::MouseButton::Left,
                    ..
                } => {
                    if let Some((x, y)) = self.cursor.get(&native).copied() {
                        self.pointer_samples.push(RawPointerSample {
                            window: krate,
                            x,
                            y,
                            pressed: state == winit::event::ElementState::Pressed,
                        });
                    }
                    None
                }
                _ => None,
            };
            if let Some(event) = mapped {
                self.events.push((krate, event));
            }
        }
    }

    /// Colors for the first drawn pass (0xAARRGGBB).
    const COLOR_BACKGROUND: u32 = 0xFFF2F2F2;
    const COLOR_BUTTON: u32 = 0xFF3B82F6;
    const COLOR_FIELD_FILL: u32 = 0xFFFFFFFF;
    const COLOR_FIELD_BORDER: u32 = 0xFF9CA3AF;

    fn fill_rect(
        buffer: &mut [u32],
        width: u32,
        height: u32,
        rect: (f32, f32, f32, f32),
        color: u32,
    ) {
        let (x, y, w, h) = rect;
        let x0 = x.max(0.0) as u32;
        let y0 = y.max(0.0) as u32;
        let x1 = ((x + w).max(0.0) as u32).min(width);
        let y1 = ((y + h).max(0.0) as u32).min(height);
        for row in y0..y1 {
            let start = (row * width + x0) as usize;
            let end = (row * width + x1) as usize;
            for pixel in &mut buffer[start..end] {
                *pixel = color;
            }
        }
    }

    fn draw_placements(tracked: &mut TrackedWindow) {
        let size = tracked.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        if tracked.surface.is_none() {
            let context = match softbuffer::Context::new(tracked.window.clone()) {
                Ok(context) => context,
                Err(_) => return,
            };
            tracked.surface = softbuffer::Surface::new(&context, tracked.window.clone()).ok();
        }
        let Some(surface) = tracked.surface.as_mut() else {
            return;
        };
        if surface.resize(width, height).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };

        let scale = tracked.window.scale_factor() as f32;
        let (w, h) = (width.get(), height.get());
        buffer.fill(COLOR_BACKGROUND);
        for placement in &tracked.placements {
            let (px, py) = (placement.x * scale, placement.y * scale);
            let (pw, ph) = (placement.width * scale, placement.height * scale);
            let text_scale = (scale.round() as u32).max(1);
            let label = placement.label.as_deref().unwrap_or("");
            match placement.kind {
                WidgetKind::Button => {
                    fill_rect(&mut buffer, w, h, (px, py, pw, ph), COLOR_BUTTON);
                    let tw = krate_adapter_common::drawtext::text_width(label, text_scale) as f32;
                    let th = krate_adapter_common::drawtext::text_height(text_scale) as f32;
                    krate_adapter_common::drawtext::draw_text(
                        &mut buffer,
                        w,
                        h,
                        ((px + (pw - tw) / 2.0) as i32, (py + (ph - th) / 2.0) as i32),
                        text_scale,
                        0xFFFFFFFF,
                        label,
                    );
                }
                WidgetKind::TextField | WidgetKind::TextArea => {
                    fill_rect(&mut buffer, w, h, (px, py, pw, ph), COLOR_FIELD_BORDER);
                    fill_rect(
                        &mut buffer,
                        w,
                        h,
                        (
                            px + 1.0 * scale,
                            py + 1.0 * scale,
                            (pw - 2.0 * scale).max(0.0),
                            (ph - 2.0 * scale).max(0.0),
                        ),
                        COLOR_FIELD_FILL,
                    );
                    let th = krate_adapter_common::drawtext::text_height(text_scale) as f32;
                    krate_adapter_common::drawtext::draw_text(
                        &mut buffer,
                        w,
                        h,
                        ((px + 4.0 * scale) as i32, (py + (ph - th) / 2.0) as i32),
                        text_scale,
                        0xFF1F2937,
                        label,
                    );
                }
                WidgetKind::Text => {
                    let th = krate_adapter_common::drawtext::text_height(text_scale) as f32;
                    krate_adapter_common::drawtext::draw_text(
                        &mut buffer,
                        w,
                        h,
                        (px as i32, (py + (ph - th) / 2.0) as i32),
                        text_scale,
                        0xFF111827,
                        label,
                    );
                }
                _ => {}
            }
        }
        let _ = buffer.present();
    }

    /// Store drawn-widget placements for a window and repaint it.
    ///
    /// This is the first drawn-fallback pass: solid rectangles presented
    /// through a CPU framebuffer. Labels and real styling arrive with the
    /// vello renderer; the placement contract stays the same.
    pub fn set_drawn_placements(
        krate: WindowId,
        placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        if !host_initialized() {
            return Ok(0);
        }
        with_host(|host| {
            let mut drawn = 0;
            for tracked in host.app.windows.values_mut() {
                if tracked.krate == krate {
                    tracked.placements = placements
                        .iter()
                        .filter(|placement| {
                            matches!(
                                placement.kind,
                                WidgetKind::Button
                                    | WidgetKind::TextField
                                    | WidgetKind::TextArea
                                    | WidgetKind::Text
                            )
                        })
                        .cloned()
                        .collect();
                    drawn = tracked.placements.len();
                    draw_placements(tracked);
                }
            }
            Ok(drawn)
        })
    }

    /// Repaint every tracked window from its stored placements.
    pub fn redraw_all() -> Result<(), UiAdapterError> {
        if !host_initialized() {
            return Ok(());
        }
        with_host(|host| {
            for tracked in host.app.windows.values_mut() {
                draw_placements(tracked);
            }
            Ok(())
        })
    }

    fn with_host<T>(
        f: impl FnOnce(&mut Host) -> Result<T, UiAdapterError>,
    ) -> Result<T, UiAdapterError> {
        WINIT_HOST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let mut builder = EventLoop::builder();
                // Tests run on worker threads, where winit refuses to build
                // an event loop by default. The opt-in keeps production on
                // the safe main-thread default.
                if std::env::var("KRATE_WINIT_ANY_THREAD").as_deref() == Ok("1") {
                    use winit::platform::windows::EventLoopBuilderExtWindows;
                    builder.with_any_thread(true);
                }
                let event_loop = builder.build().map_err(|err| {
                    UiAdapterError::Unsupported(format!("winit event loop unavailable: {err}"))
                })?;
                *slot = Some(Host {
                    event_loop,
                    app: PumpApp::default(),
                });
            }
            f(slot.as_mut().expect("winit host initialized"))
        })
    }

    fn pump(host: &mut Host) {
        let Host { event_loop, app } = host;
        let _status = event_loop.pump_app_events(Some(Duration::ZERO), app);
    }

    /// Create a real (initially hidden) winit window for a Krate window id.
    ///
    /// Returns the opaque native handle value and the first native snapshot.
    pub fn create_native_window(
        krate: WindowId,
        title: &str,
        size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        with_host(|host| {
            host.app.pending_creates.push(PendingCreate {
                krate,
                title: title.to_string(),
                size,
            });
            pump(host);

            let tracked = host
                .app
                .windows
                .values()
                .find(|tracked| tracked.krate == krate)
                .ok_or_else(|| {
                    UiAdapterError::Unsupported(
                        "winit did not create the requested window".to_string(),
                    )
                })?;

            let raw_handle = u64::from(tracked.window.id());
            let inner = tracked.window.inner_size();
            let snapshot = WinitWindowSnapshot::new(
                krate,
                WindowSize::new(inner.width.max(1), inner.height.max(1))?,
                false,
                tracked.window.has_focus(),
                tracked.window.scale_factor() as f32,
            )?;
            Ok((raw_handle, snapshot))
        })
    }

    fn with_tracked<T>(
        krate: WindowId,
        f: impl FnOnce(&TrackedWindow) -> T,
    ) -> Result<Option<T>, UiAdapterError> {
        if !host_initialized() {
            return Ok(None);
        }
        with_host(|host| {
            Ok(host
                .app
                .windows
                .values()
                .find(|tracked| tracked.krate == krate)
                .map(f))
        })
    }

    /// Make a created native window visible.
    pub fn show_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(krate, |tracked| tracked.window.set_visible(true)).map(|shown| shown.is_some())
    }

    /// Update the native window title.
    pub fn set_native_window_title(krate: WindowId, title: &str) -> Result<bool, UiAdapterError> {
        with_tracked(krate, |tracked| tracked.window.set_title(title)).map(|set| set.is_some())
    }

    /// Ask the native window for a redraw.
    pub fn request_native_redraw(krate: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(krate, |tracked| tracked.window.request_redraw())
            .map(|requested| requested.is_some())
    }

    /// Drop the native window for a Krate window id.
    pub fn close_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !host_initialized() {
            return Ok(false);
        }
        with_host(|host| {
            let native: Vec<NativeWindowId> = host
                .app
                .windows
                .iter()
                .filter(|(_, tracked)| tracked.krate == krate)
                .map(|(native, _)| *native)
                .collect();
            for id in &native {
                host.app.windows.remove(id);
            }
            pump(host);
            Ok(!native.is_empty())
        })
    }

    /// Pump the native event loop once and drain mapped window events.
    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        if !host_initialized() {
            return Ok(Vec::new());
        }
        with_host(|host| {
            pump(host);
            Ok(std::mem::take(&mut host.app.events))
        })
    }

    /// Drain raw pointer samples captured since the last call.
    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        if !host_initialized() {
            return Vec::new();
        }
        WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|host| std::mem::take(&mut host.app.pointer_samples))
                .unwrap_or_default()
        })
    }

    /// Whether a native window is currently tracked for the id.
    pub fn has_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(krate, |_| ()).map(|found| found.is_some())
    }
}

#[cfg(not(target_os = "windows"))]
mod stub {
    use super::*;

    fn unsupported<T>() -> Result<T, UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "winit native windows are only available in Windows builds of this crate".to_string(),
        ))
    }

    /// Winit windows are only available in Windows builds.
    pub fn create_native_window(
        _krate: WindowId,
        _title: &str,
        _size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn show_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn set_native_window_title(_krate: WindowId, _title: &str) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn request_native_redraw(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn close_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn has_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn set_drawn_placements(
        _krate: WindowId,
        _placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn redraw_all() -> Result<(), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        Vec::new()
    }
}
