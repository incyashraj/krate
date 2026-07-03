//! Real winit windowing for the Linux prototype adapter.
//!
//! The event loop is owned thread-locally and pumped non-blockingly with
//! `pump_app_events`, mirroring how the macOS crate owns its AppKit sessions.
//! Window creation is queued and performed inside the pump (winit only hands
//! out `ActiveEventLoop` inside callbacks), and native `WindowEvent`s are
//! mapped into the shared [`WinitWindowNativeEvent`] shape that the existing
//! collector and event-loop pump already drain.
//!
//! Creating the event loop requires a display server (X11 or Wayland).
//! Headless hosts — CI without `xvfb-run` — get a clean `Unsupported` error
//! at first use; everything stays compiled and unit-testable everywhere.

#[cfg(target_os = "linux")]
pub use real::*;

#[cfg(not(target_os = "linux"))]
pub use stub::*;

use layer36_adapter_common::ui::{
    UiAdapterError, WindowId, WindowSize, WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Layer36 window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

#[cfg(target_os = "linux")]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowAttributes, WindowId as NativeWindowId};

    thread_local! {
        static WINIT_HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    }

    struct Host {
        event_loop: EventLoop<()>,
        app: PumpApp,
    }

    #[derive(Default)]
    struct PumpApp {
        pending_creates: Vec<PendingCreate>,
        windows: BTreeMap<NativeWindowId, TrackedWindow>,
        events: CollectedNativeEvents,
    }

    struct PendingCreate {
        layer36: WindowId,
        title: String,
        size: WindowSize,
    }

    struct TrackedWindow {
        layer36: WindowId,
        window: Window,
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
                            layer36: pending.layer36,
                            window,
                        },
                    );
                }
            }
        }

        fn layer36_id(&self, native: NativeWindowId) -> Option<WindowId> {
            self.windows.get(&native).map(|tracked| tracked.layer36)
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
            let Some(layer36) = self.layer36_id(native) else {
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
                _ => None,
            };
            if let Some(event) = mapped {
                self.events.push((layer36, event));
            }
        }
    }

    fn with_host<T>(
        f: impl FnOnce(&mut Host) -> Result<T, UiAdapterError>,
    ) -> Result<T, UiAdapterError> {
        WINIT_HOST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let event_loop = EventLoop::new().map_err(|err| {
                    UiAdapterError::Unsupported(format!(
                        "winit event loop unavailable (no display server?): {err}"
                    ))
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

    /// Create a real (initially hidden) winit window for a Layer36 window id.
    ///
    /// Returns the opaque native handle value and the first native snapshot.
    pub fn create_native_window(
        layer36: WindowId,
        title: &str,
        size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        with_host(|host| {
            host.app.pending_creates.push(PendingCreate {
                layer36,
                title: title.to_string(),
                size,
            });
            pump(host);

            let tracked = host
                .app
                .windows
                .values()
                .find(|tracked| tracked.layer36 == layer36)
                .ok_or_else(|| {
                    UiAdapterError::Unsupported(
                        "winit did not create the requested window".to_string(),
                    )
                })?;

            let raw_handle = u64::from(tracked.window.id());
            let inner = tracked.window.inner_size();
            let snapshot = WinitWindowSnapshot::new(
                layer36,
                WindowSize::new(inner.width.max(1), inner.height.max(1))?,
                false,
                tracked.window.has_focus(),
                tracked.window.scale_factor() as f32,
            )?;
            Ok((raw_handle, snapshot))
        })
    }

    fn with_tracked<T>(
        layer36: WindowId,
        f: impl FnOnce(&TrackedWindow) -> T,
    ) -> Result<Option<T>, UiAdapterError> {
        with_host(|host| {
            Ok(host
                .app
                .windows
                .values()
                .find(|tracked| tracked.layer36 == layer36)
                .map(f))
        })
    }

    /// Make a created native window visible.
    pub fn show_native_window(layer36: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(layer36, |tracked| tracked.window.set_visible(true))
            .map(|shown| shown.is_some())
    }

    /// Update the native window title.
    pub fn set_native_window_title(layer36: WindowId, title: &str) -> Result<bool, UiAdapterError> {
        with_tracked(layer36, |tracked| tracked.window.set_title(title)).map(|set| set.is_some())
    }

    /// Ask the native window for a redraw.
    pub fn request_native_redraw(layer36: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(layer36, |tracked| tracked.window.request_redraw())
            .map(|requested| requested.is_some())
    }

    /// Drop the native window for a Layer36 window id.
    pub fn close_native_window(layer36: WindowId) -> Result<bool, UiAdapterError> {
        with_host(|host| {
            let native: Vec<NativeWindowId> = host
                .app
                .windows
                .iter()
                .filter(|(_, tracked)| tracked.layer36 == layer36)
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
        with_host(|host| {
            pump(host);
            Ok(std::mem::take(&mut host.app.events))
        })
    }

    /// Whether a native window is currently tracked for the id.
    pub fn has_native_window(layer36: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(layer36, |_| ()).map(|found| found.is_some())
    }
}

#[cfg(not(target_os = "linux"))]
mod stub {
    use super::*;

    fn unsupported<T>() -> Result<T, UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "winit native windows are only available in Linux builds of this crate".to_string(),
        ))
    }

    /// Winit windows are only available in Linux builds.
    pub fn create_native_window(
        _layer36: WindowId,
        _title: &str,
        _size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn show_native_window(_layer36: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn set_native_window_title(
        _layer36: WindowId,
        _title: &str,
    ) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn request_native_redraw(_layer36: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn close_native_window(_layer36: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn has_native_window(_layer36: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }
}
