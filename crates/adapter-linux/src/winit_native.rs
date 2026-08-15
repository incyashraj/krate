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

#[cfg(any(target_os = "linux", feature = "dev-anyos"))]
pub use real::*;

#[cfg(not(any(target_os = "linux", feature = "dev-anyos")))]
pub use stub::*;

#[cfg(any(target_os = "linux", feature = "dev-anyos"))]
use krate_adapter_common::ui::Modifiers;
use krate_adapter_common::ui::{
    RawKeySample, RawPointerSample, RawWheelSample, UiAdapterError, WidgetPlacement, WindowId,
    WindowSize, WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Krate window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

// dev-anyos: the real path compiles on the dev Mac too, so this module is
// judged before CI rather than by users. Same reasoning, same shape as the
// Windows twin.
#[cfg(any(target_os = "linux", feature = "dev-anyos"))]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use std::num::NonZeroU32;
    use std::sync::Arc;

    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::window::{Window, WindowAttributes, WindowId as NativeWindowId};

    type DrawSurface = softbuffer::Surface<Arc<Window>, Arc<Window>>;

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
        key_samples: Vec<RawKeySample>,
        wheel_samples: Vec<RawWheelSample>,
        modifiers: Modifiers,
    }

    struct PendingCreate {
        krate: WindowId,
        title: String,
        size: WindowSize,
    }

    struct TrackedWindow {
        krate: WindowId,
        window: Arc<Window>,
        surface: Option<DrawSurface>,
        /// The GPU presenter for this window, once it initialized.
        gpu: Option<GpuPresent>,
        /// GPU init or a present failed once: CPU for the life of the
        /// window. The fallback is the contract; flapping is not.
        gpu_dead: bool,
        /// When the most recent user input arrived (S3 latency).
        last_input: Option<std::time::Instant>,
        placements: Vec<WidgetPlacement>,
        hovered: Option<krate_adapter_common::ui::WidgetId>,
        pressed_widget: Option<krate_adapter_common::ui::WidgetId>,
    }

    /// The shared windowed presenter, wrapped with what only the adapter
    /// knows: the winit window's size, scale, and last-input moment. One
    /// implementation lives in presenter-gpu; this and the Windows twin are
    /// wrappers that cannot drift from it.
    pub struct GpuPresent {
        inner: krate_presenter_gpu::WindowPresenter,
    }

    impl GpuPresent {
        pub fn new(window: Arc<Window>) -> Result<Self, String> {
            Ok(Self {
                inner: krate_presenter_gpu::WindowPresenter::new(window)?,
            })
        }

        pub fn render(
            &mut self,
            window: &Window,
            placements: &[WidgetPlacement],
            interaction: krate_adapter_common::painter::PaintInteraction,
            input_at: Option<std::time::Instant>,
        ) -> Result<(), String> {
            let size = window.inner_size();
            window.pre_present_notify();
            self.inner.render(
                size.width,
                size.height,
                window.scale_factor() as f32,
                placements,
                interaction,
                input_at,
            )
        }
    }

    /// Normalize a winit logical key into the portable key-name shape.
    /// Characters map to themselves; a curated set of named keys map to
    /// stable names; everything else (bare modifiers, media keys) is
    /// dropped for now.
    fn key_name(key: &winit::keyboard::Key) -> Option<String> {
        use winit::keyboard::{Key, NamedKey};
        match key {
            // winit reports the spacebar as a literal " " character, not
            // NamedKey::Space; without this an app never matches
            // key_held("Space"). Same fix as the Windows adapter.
            Key::Character(text) if text.as_str() == " " => Some("Space".to_string()),
            Key::Character(text) => Some(text.to_string()),
            Key::Named(named) => {
                let name = match named {
                    NamedKey::Enter => "Enter",
                    NamedKey::Space => "Space",
                    NamedKey::Backspace => "Backspace",
                    NamedKey::Delete => "Delete",
                    NamedKey::Tab => "Tab",
                    NamedKey::Escape => "Escape",
                    NamedKey::ArrowLeft => "ArrowLeft",
                    NamedKey::ArrowRight => "ArrowRight",
                    NamedKey::ArrowUp => "ArrowUp",
                    NamedKey::ArrowDown => "ArrowDown",
                    NamedKey::Home => "Home",
                    NamedKey::End => "End",
                    NamedKey::PageUp => "PageUp",
                    NamedKey::PageDown => "PageDown",
                    _ => return None,
                };
                Some(name.to_string())
            }
            _ => None,
        }
    }

    impl PumpApp {
        fn drain_pending_creates(&mut self, event_loop: &ActiveEventLoop) {
            for pending in self.pending_creates.drain(..) {
                let attributes = WindowAttributes::default()
                    .with_title(pending.title)
                    // Physical, not logical -- see the same note in the Windows
                    // adapter. Every other size here comes from `inner_size()`,
                    // which is physical, so a fractional-scaling desktop got a
                    // window bigger than the app painted.
                    .with_inner_size(PhysicalSize::new(pending.size.width, pending.size.height))
                    // Visible on creation, matching macOS. See the same note in
                    // the Windows adapter: hidden-until-shown assumed every app
                    // calls `window.show`, and none of the samples do. A user
                    // ran a 3D app on Windows and saw a frame count print with
                    // no window; Linux had the identical bug.
                    .with_visible(true);
                if let Ok(window) = event_loop.create_window(attributes) {
                    self.windows.insert(
                        window.id(),
                        TrackedWindow {
                            krate: pending.krate,
                            window: Arc::new(window),
                            surface: None,
                            gpu: None,
                            gpu_dead: false,
                            last_input: None,
                            placements: Vec::new(),
                            hovered: None,
                            pressed_widget: None,
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
            // Stamp real user input on arrival; the presenter reports
            // input-to-present from this.
            if matches!(
                event,
                WindowEvent::CursorMoved { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::KeyboardInput { .. }
            ) {
                if let Some(tracked) = self.windows.get_mut(&native) {
                    tracked.last_input = Some(std::time::Instant::now());
                }
            }
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
                    let (x, y) = ((position.x / scale) as f32, (position.y / scale) as f32);
                    self.cursor.insert(native, (x, y));
                    if let Some(tracked) = self.windows.get_mut(&native) {
                        let hovered = krate_adapter_common::painter::topmost_interactive_at(
                            &tracked.placements,
                            x,
                            y,
                        );
                        if hovered != tracked.hovered {
                            tracked.hovered = hovered;
                            draw_placements(tracked);
                        }
                    }
                    None
                }
                WindowEvent::MouseInput {
                    state,
                    button: winit::event::MouseButton::Left,
                    ..
                } => {
                    if let Some((x, y)) = self.cursor.get(&native).copied() {
                        let pressed = state == winit::event::ElementState::Pressed;
                        self.pointer_samples.push(RawPointerSample {
                            window: krate,
                            x,
                            y,
                            pressed,
                        });
                        if let Some(tracked) = self.windows.get_mut(&native) {
                            let pressed_widget = if pressed { tracked.hovered } else { None };
                            if pressed_widget != tracked.pressed_widget {
                                tracked.pressed_widget = pressed_widget;
                                draw_placements(tracked);
                            }
                        }
                    }
                    None
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if let Some((x, y)) = self.cursor.get(&native).copied() {
                        // Line deltas scale to ~20 logical px per notch;
                        // pixel deltas divide by the window scale factor.
                        // Winit's positive y scrolls content up and positive x
                        // scrolls it right; ours are positive-down and
                        // positive-right, so both are negated.
                        let (dx, dy) = match delta {
                            winit::event::MouseScrollDelta::LineDelta(columns, lines) => {
                                (-columns * 20.0, -lines * 20.0)
                            }
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                let scale = self
                                    .windows
                                    .get(&native)
                                    .map(|tracked| tracked.window.scale_factor())
                                    .unwrap_or(1.0);
                                (-(pos.x / scale) as f32, -(pos.y / scale) as f32)
                            }
                        };
                        if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                            self.wheel_samples.push(RawWheelSample {
                                window: krate,
                                x,
                                y,
                                dx,
                                dy,
                                modifiers: self.modifiers,
                            });
                        }
                    }
                    None
                }
                WindowEvent::ModifiersChanged(state) => {
                    let state = state.state();
                    self.modifiers = Modifiers {
                        shift: state.shift_key(),
                        control: state.control_key(),
                        alt: state.alt_key(),
                        meta: state.super_key(),
                    };
                    None
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let Some(key) = key_name(&event.logical_key) {
                        let pressed = event.state == winit::event::ElementState::Pressed;
                        // Text comes from the platform's layout processing;
                        // only presses produce it, and control characters
                        // (Enter, Backspace) travel as key names instead.
                        let text = if pressed {
                            event
                                .text
                                .as_ref()
                                .map(|text| text.to_string())
                                .filter(|text| {
                                    !text.is_empty() && !text.chars().any(char::is_control)
                                })
                        } else {
                            None
                        };
                        self.key_samples.push(RawKeySample {
                            window: krate,
                            key,
                            pressed,
                            modifiers: self.modifiers,
                            text,
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

    fn draw_placements(tracked: &mut TrackedWindow) {
        // GPU first, vsync-paced; KRATE_CPU_PRESENT=1 forces the CPU painter;
        // any GPU failure retires the path for this window.
        if !tracked.gpu_dead && std::env::var_os("KRATE_CPU_PRESENT").is_none() {
            if tracked.gpu.is_none() {
                match GpuPresent::new(tracked.window.clone()) {
                    Ok(gpu) => tracked.gpu = Some(gpu),
                    Err(why) => {
                        tracked.gpu_dead = true;
                        eprintln!("krate: GPU presenter unavailable ({why}); drawing on the CPU");
                    }
                }
            }
            if let Some(gpu) = tracked.gpu.as_mut() {
                let interaction = krate_adapter_common::painter::PaintInteraction {
                    hovered: tracked.hovered,
                    pressed: tracked.pressed_widget,
                };
                let input_at = tracked.last_input.take();
                match gpu.render(&tracked.window, &tracked.placements, interaction, input_at) {
                    Ok(()) => return,
                    Err(why) => {
                        tracked.gpu_dead = true;
                        tracked.gpu = None;
                        eprintln!("krate: GPU present failed ({why}); drawing on the CPU");
                    }
                }
            }
        }

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
        krate_adapter_common::painter::paint_placements(
            &mut buffer,
            width.get(),
            height.get(),
            tracked.window.scale_factor() as f32,
            &tracked.placements,
            krate_adapter_common::painter::PaintInteraction {
                hovered: tracked.hovered,
                pressed: tracked.pressed_widget,
            },
        );
        let _ = buffer.present();
    }

    /// Store drawn-widget placements for a window and repaint it.
    ///
    /// Pixels come from the shared CPU painter in `adapter-common`
    /// (rectangles plus bitmap-font labels); the vello renderer replaces
    /// that painter behind the same placement contract.
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
                            krate_adapter_common::painter::drawn_kind(placement.kind)
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

    /// The sonames `xkbcommon-dl` tries, in its order.
    ///
    /// Kept identical to that crate's list so this check answers the same
    /// question it will ask. If it ever loads a different name, this probe
    /// would pass and the panic would come back.
    const XKB_X11_SONAMES: [&str; 2] = ["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"];

    /// `Some(message)` when this is an X11 session and the X11 keyboard
    /// bridge cannot be loaded.
    ///
    /// Ubuntu splits the two libraries: `libxkbcommon0` carries
    /// libxkbcommon.so.0 and is installed by Ubuntu Desktop, while
    /// `libxkbcommon-x11-0` carries the X11 bridge and is not. So a stock
    /// desktop has the first and not the second, and every GUI app panicked.
    ///
    /// Wayland sessions never load it, so the check is skipped there rather
    /// than refusing a machine that would have worked.
    fn missing_x11_keyboard_library() -> Option<String> {
        // Wayland-only session: winit takes the Wayland path and this
        // library is never opened.
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return None;
        }
        for soname in XKB_X11_SONAMES {
            let c_name = match std::ffi::CString::new(soname) {
                Ok(name) => name,
                Err(_) => continue,
            };
            // SAFETY: a NUL-terminated name and RTLD_LAZY|RTLD_LOCAL, the
            // same call dlib makes. The handle is closed straight away; this
            // only asks whether the loader can find it.
            let handle =
                unsafe { libc::dlopen(c_name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
            if !handle.is_null() {
                unsafe { libc::dlclose(handle) };
                return None;
            }
        }
        Some(
            "this system is missing the X11 keyboard library that windows need. \
             On Ubuntu or Debian install it with:\n\
             \n    sudo apt install libxkbcommon-x11-0\n\n\
             On Fedora the package is libxkbcommon-x11, and on Arch it is part of \
             libxkbcommon."
                .to_string(),
        )
    }

    fn with_host<T>(
        f: impl FnOnce(&mut Host) -> Result<T, UiAdapterError>,
    ) -> Result<T, UiAdapterError> {
        WINIT_HOST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                // winit's Linux backend panics (rather than erring) when no
                // display server exists, so guard on the environment first.
                if std::env::var_os("DISPLAY").is_none()
                    && std::env::var_os("WAYLAND_DISPLAY").is_none()
                {
                    return Err(UiAdapterError::Unsupported(
                        "no display server: DISPLAY and WAYLAND_DISPLAY are unset".to_string(),
                    ));
                }
                // Same class of problem, one layer down. On X11 winit reaches
                // for libxkbcommon-x11, and the crate that loads it ends in
                // `.expect(...)` -- so a missing library is a Rust panic with
                // a crate path and a line number, which is precisely what a
                // person opening an app must never see (K-036).
                if let Some(missing) = missing_x11_keyboard_library() {
                    return Err(UiAdapterError::Unsupported(missing));
                }
                let mut builder = EventLoop::builder();
                // Tests run on worker threads, where winit refuses to build
                // an event loop by default. The opt-in keeps production on
                // the safe main-thread default.
                if std::env::var("KRATE_WINIT_ANY_THREAD").as_deref() == Ok("1") {
                    #[cfg(target_os = "linux")]
                    {
                        use winit::platform::x11::EventLoopBuilderExtX11;
                        builder.with_any_thread(true);
                    }
                }
                let event_loop = builder.build().map_err(|err| {
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

    /// Drain raw keyboard samples captured since the last call.
    pub fn drain_key_samples() -> Vec<RawKeySample> {
        if !host_initialized() {
            return Vec::new();
        }
        WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|host| std::mem::take(&mut host.app.key_samples))
                .unwrap_or_default()
        })
    }

    /// Drain raw mouse-wheel samples captured since the last call.
    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        if !host_initialized() {
            return Vec::new();
        }
        WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|host| std::mem::take(&mut host.app.wheel_samples))
                .unwrap_or_default()
        })
    }

    /// Whether a native window is currently tracked for the id.
    pub fn has_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        with_tracked(krate, |_| ()).map(|found| found.is_some())
    }

    #[cfg(test)]
    mod tests {
        use super::key_name;
        use super::{missing_x11_keyboard_library, XKB_X11_SONAMES};
        use winit::keyboard::{Key, NamedKey, SmolStr};

        #[test]
        fn the_probe_asks_for_the_same_libraries_the_loader_will() {
            // The whole check rests on asking the question xkbcommon-dl asks.
            // If it ever loads a different name, this probe would succeed,
            // winit would still panic, and K-036 would be back with the
            // check apparently passing.
            assert_eq!(
                XKB_X11_SONAMES,
                ["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"],
                "these must stay identical to xkbcommon-dl's own soname list"
            );
        }

        #[test]
        fn the_message_names_the_package_to_install() {
            // A person who sees this has to be able to act on it without
            // knowing what xkbcommon is. If the library happens to be
            // present on the test machine there is nothing to assert, and
            // that is the correct outcome there.
            if let Some(message) = missing_x11_keyboard_library() {
                assert!(
                    message.contains("libxkbcommon-x11-0"),
                    "must name the apt package: {message}"
                );
                assert!(
                    message.contains("sudo apt install"),
                    "must give the command to run: {message}"
                );
                assert!(
                    !message.contains("panicked") && !message.contains(".rs:"),
                    "must not read like a crash: {message}"
                );
            }
        }

        #[test]
        fn a_wayland_only_session_is_not_asked_about_x11() {
            // Wayland never loads the X11 bridge, so refusing a Wayland
            // machine for a library it does not need would be a new bug.
            let display = std::env::var_os("DISPLAY");
            let wayland = std::env::var_os("WAYLAND_DISPLAY");
            // SAFETY: single-threaded test, restored before returning.
            unsafe {
                std::env::remove_var("DISPLAY");
                std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            }
            let verdict = missing_x11_keyboard_library();
            unsafe {
                std::env::remove_var("WAYLAND_DISPLAY");
                if let Some(value) = display {
                    std::env::set_var("DISPLAY", value);
                }
                if let Some(value) = wayland {
                    std::env::set_var("WAYLAND_DISPLAY", value);
                }
            }
            assert!(
                verdict.is_none(),
                "a Wayland session must not be refused over an X11 library"
            );
        }

        #[test]
        fn the_spacebar_is_named_space_not_a_blank() {
            // winit hands the spacebar over as a " " character, and a bare
            // to_string() named the key " ", which no app matches against
            // key_held("Space"). This is the regression that made Space do
            // nothing on Linux and Windows while it worked on macOS.
            assert_eq!(
                key_name(&Key::Character(SmolStr::new(" "))).as_deref(),
                Some("Space")
            );
            assert_eq!(
                key_name(&Key::Named(NamedKey::Space)).as_deref(),
                Some("Space")
            );
            assert_eq!(
                key_name(&Key::Character(SmolStr::new("a"))).as_deref(),
                Some("a")
            );
            assert_eq!(
                key_name(&Key::Named(NamedKey::ArrowLeft)).as_deref(),
                Some("ArrowLeft")
            );
        }
    }
}

#[cfg(not(any(target_os = "linux", feature = "dev-anyos")))]
mod stub {
    use super::*;

    fn unsupported<T>() -> Result<T, UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "winit native windows are only available in Linux builds of this crate".to_string(),
        ))
    }

    /// Winit windows are only available in Linux builds.
    pub fn create_native_window(
        _krate: WindowId,
        _title: &str,
        _size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn show_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn set_native_window_title(_krate: WindowId, _title: &str) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn request_native_redraw(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn close_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn has_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn set_drawn_placements(
        _krate: WindowId,
        _placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn redraw_all() -> Result<(), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Linux builds.
    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        Vec::new()
    }

    /// Winit windows are only available in Linux builds.
    pub fn drain_key_samples() -> Vec<RawKeySample> {
        Vec::new()
    }

    /// Winit windows are only available in Linux builds.
    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        Vec::new()
    }
}
