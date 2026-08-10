//! Real winit windowing for the Android adapter.
//!
//! The event loop is owned thread-locally and pumped non-blockingly with
//! `pump_app_events`, mirroring how the macOS crate owns its AppKit sessions.
//! Window creation is queued and performed inside the pump (winit only hands
//! out `ActiveEventLoop` inside callbacks), and native `WindowEvent`s are
//! mapped into the shared [`WinitWindowNativeEvent`] shape that the existing
//! collector and event-loop pump already drain.
//!
//! Creating the event loop requires the `AndroidApp` handle that the
//! platform hands to `android_main`; the player entry point deposits it with
//! [`set_android_app`] before the runtime first touches the adapter. Off
//! Android everything compiles as stubs, so the crate stays unit-testable
//! on desktop machines.

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(not(target_os = "android"))]
pub use stub::*;

#[cfg(target_os = "android")]
use krate_adapter_common::ui::Modifiers;
use krate_adapter_common::ui::{
    RawKeySample, RawPointerSample, RawWheelSample, UiAdapterError, WidgetPlacement, WindowId,
    WindowSize, WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Krate window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

#[cfg(target_os = "android")]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use std::num::NonZeroU32;
    use std::rc::Rc;

    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::platform::pump_events::EventLoopExtPumpEvents;
    use winit::window::{Window, WindowAttributes, WindowId as NativeWindowId};

    type DrawSurface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

    thread_local! {
        static WINIT_HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
        static ANDROID_APP: RefCell<Option<winit::platform::android::activity::AndroidApp>> =
            const { RefCell::new(None) };
    }

    /// Display scale of a tracked window, 1.0 when unknown.
    pub fn window_scale(krate: WindowId) -> f32 {
        if !host_initialized() {
            return 1.0;
        }
        with_tracked(krate, |tracked| tracked.window.scale_factor() as f32)
            .ok()
            .flatten()
            .unwrap_or(1.0)
    }

    /// The player takes AndroidApp from here so both crates share one
    /// winit by construction.
    pub use winit::platform::android::activity::AndroidApp;

    /// Deposit the `AndroidApp` handle from `android_main`.
    ///
    /// Must run on the thread that will pump the event loop, before the
    /// runtime first touches the adapter.
    pub fn set_android_app(app: winit::platform::android::activity::AndroidApp) {
        ANDROID_APP.with(|slot| *slot.borrow_mut() = Some(app));
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

    /// One in-flight touch gesture. Krate's event model is pointer + wheel;
    /// a phone's finger becomes both, decided by movement: stay within the
    /// slop and it was a tap (pointer press + release), move past it and
    /// every further move is a wheel delta -- the same conversion the wheel
    /// contract already promises ("the host converts"). Only the first
    /// finger drives; extra fingers are ignored rather than corrupting the
    /// gesture.
    struct TouchGesture {
        finger: u64,
        native: NativeWindowId,
        start: (f32, f32),
        last: (f32, f32),
        scrolling: bool,
    }

    #[derive(Default)]
    struct PumpApp {
        pending_creates: Vec<PendingCreate>,
        touch: Option<TouchGesture>,
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
        window: Rc<Window>,
        surface: Option<DrawSurface>,
        placements: Vec<WidgetPlacement>,
        hovered: Option<krate_adapter_common::ui::WidgetId>,
        pressed_widget: Option<krate_adapter_common::ui::WidgetId>,
        /// Painted into by the CPU painter, then copied to the window
        /// buffer in one pass. The window buffer is uncached
        /// write-combined memory, and the painter's scattered row writes
        /// there cost 40 ms a frame on the emulator; sequential bulk copy
        /// is what that memory is for (K-089's Android leg).
        staging: Vec<u32>,
        /// Repaint only when something actually changed: the pump used to
        /// repaint every tick, spending whole frames re-drawing a still
        /// image.
        dirty: bool,
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
                            window: Rc::new(window),
                            surface: None,
                            placements: Vec::new(),
                            hovered: None,
                            pressed_widget: None,
                            staging: Vec::new(),
                            dirty: true,
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

        fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
            // Android tears the native window down when the app leaves the
            // foreground. Drop every softbuffer surface now; the draw path
            // lazily recreates one against the fresh native window on the
            // first paint after resume. Keeping the old surface means
            // presenting into a dead window -- black screen at best.
            for tracked in self.windows.values_mut() {
                tracked.surface = None;
                tracked.dirty = true;
            }
            self.touch = None;
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
                    // Logical, like every other size the app sees.
                    let scale = self
                        .windows
                        .get(&native)
                        .map(|tracked| tracked.window.scale_factor())
                        .unwrap_or(1.0)
                        .max(0.25);
                    let logical_w = ((size.width as f64 / scale).round() as u32).max(1);
                    let logical_h = ((size.height as f64 / scale).round() as u32).max(1);
                    WindowSize::new(logical_w, logical_h)
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
                WindowEvent::Touch(touch) => {
                    let scale = self
                        .windows
                        .get(&native)
                        .map(|tracked| tracked.window.scale_factor())
                        .unwrap_or(1.0)
                        .max(0.25);
                    let (x, y) = (
                        (touch.location.x / scale) as f32,
                        (touch.location.y / scale) as f32,
                    );
                    // Movement past this many logical pixels turns a touch
                    // from a tap into a scroll -- the standard slop every
                    // mobile toolkit uses so a wobbly finger still taps.
                    const SLOP: f32 = 8.0;
                    match touch.phase {
                        winit::event::TouchPhase::Started => {
                            if self.touch.is_none() {
                                self.touch = Some(TouchGesture {
                                    finger: touch.id,
                                    native,
                                    start: (x, y),
                                    last: (x, y),
                                    scrolling: false,
                                });
                                // The finger is the cursor: hit tests and
                                // wheel positions read from here.
                                self.cursor.insert(native, (x, y));
                                if let Some(tracked) = self.windows.get_mut(&native) {
                                    let hovered =
                                        krate_adapter_common::painter::topmost_interactive_at(
                                            &tracked.placements,
                                            x,
                                            y,
                                        );
                                    if hovered != tracked.hovered {
                                        tracked.hovered = hovered;
                                    }
                                }
                            }
                        }
                        winit::event::TouchPhase::Moved => {
                            let same = self
                                .touch
                                .as_ref()
                                .is_some_and(|g| g.finger == touch.id && g.native == native);
                            if same {
                                let gesture = self.touch.as_mut().expect("gesture checked");
                                if !gesture.scrolling {
                                    let sx = x - gesture.start.0;
                                    let sy = y - gesture.start.1;
                                    if sx * sx + sy * sy > SLOP * SLOP {
                                        gesture.scrolling = true;
                                    }
                                }
                                let dx = x - gesture.last.0;
                                let dy = y - gesture.last.1;
                                gesture.last = (x, y);
                                let scrolling = gesture.scrolling;
                                if scrolling && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON)
                                {
                                    self.cursor.insert(native, (x, y));
                                    // Content follows the finger: a finger
                                    // moving up drags the list deeper, which
                                    // is positive dy in the wheel contract.
                                    self.wheel_samples.push(RawWheelSample {
                                        window: krate,
                                        x,
                                        y,
                                        dx: -dx,
                                        dy: -dy,
                                        modifiers: self.modifiers,
                                    });
                                }
                            }
                        }
                        winit::event::TouchPhase::Ended => {
                            let same = self
                                .touch
                                .as_ref()
                                .is_some_and(|g| g.finger == touch.id && g.native == native);
                            if same {
                                let gesture = self.touch.take().expect("gesture checked");
                                if !gesture.scrolling {
                                    // A tap: one press and one release at
                                    // the touch point, exactly what a click
                                    // delivers. Apps' double-tap detection
                                    // sees two of these.
                                    self.cursor.insert(native, (x, y));
                                    self.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x,
                                        y,
                                        pressed: true,
                                    });
                                    self.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x,
                                        y,
                                        pressed: false,
                                    });
                                    if let Some(tracked) = self.windows.get_mut(&native) {
                                        let hovered =
                                            krate_adapter_common::painter::topmost_interactive_at(
                                                &tracked.placements,
                                                x,
                                                y,
                                            );
                                        // Press feedback for drawn widgets:
                                        // flash the pressed state through one
                                        // repaint so a tapped button reads
                                        // as tapped.
                                        if hovered.is_some() {
                                            tracked.pressed_widget = hovered;
                                            draw_placements(tracked);
                                            tracked.pressed_widget = None;
                                            draw_placements(tracked);
                                        }
                                    }
                                }
                            }
                        }
                        winit::event::TouchPhase::Cancelled => {
                            let same = self
                                .touch
                                .as_ref()
                                .is_some_and(|g| g.finger == touch.id && g.native == native);
                            if same {
                                self.touch = None;
                            }
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
                WindowEvent::KeyboardInput { event, .. }
                    if matches!(
                        event.logical_key,
                        // winit spells the Android back button BrowserBack;
                        // GoBack is matched too in case that ever changes.
                        winit::keyboard::Key::Named(
                            winit::keyboard::NamedKey::BrowserBack
                                | winit::keyboard::NamedKey::GoBack
                        )
                    ) && event.state == winit::event::ElementState::Pressed =>
                {
                    // The system back gesture/button. Apps already handle
                    // close-requested on every platform; back is Android's
                    // spelling of the same intent.
                    Some(WinitWindowNativeEvent::CloseRequested)
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
        let pixel_count = width.get() as usize * height.get() as usize;
        tracked.staging.clear();
        tracked.staging.resize(pixel_count, 0);
        krate_adapter_common::painter::paint_placements(
            &mut tracked.staging,
            width.get(),
            height.get(),
            tracked.window.scale_factor() as f32,
            &tracked.placements,
            krate_adapter_common::painter::PaintInteraction {
                hovered: tracked.hovered,
                pressed: tracked.pressed_widget,
            },
        );
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        buffer.copy_from_slice(&tracked.staging);
        let _ = buffer.present();
        tracked.dirty = false;
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
                    tracked.dirty = true;
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
                // A clean window with a live surface has nothing to show
                // that it is not already showing; repainting it anyway was
                // a whole extra frame of work per pump.
                if tracked.dirty || tracked.surface.is_none() {
                    draw_placements(tracked);
                }
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
                // Android's event loop can only be built with the AndroidApp
                // the platform hands to android_main. The player deposits it
                // before the runtime runs; anything else is a wiring bug.
                let Some(android_app) = ANDROID_APP.with(|app| app.borrow().clone()) else {
                    return Err(UiAdapterError::Unsupported(
                        "no AndroidApp deposited: call set_android_app from android_main first"
                            .to_string(),
                    ));
                };
                let mut builder = EventLoop::builder();
                {
                    use winit::platform::android::EventLoopBuilderExtAndroid;
                    builder.with_android_app(android_app);
                }
                let event_loop = builder.build().map_err(|err| {
                    eprintln!("krate-adapter: event loop build failed: {err}");
                    UiAdapterError::Unsupported(format!(
                        "winit event loop unavailable: {err}"
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
            // Android delivers Resumed on its own schedule, and a window can
            // only be created once it has -- one pump is not enough right
            // after launch. Pump until the window exists, bounded so a
            // platform that never resumes fails loudly instead of hanging.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                pump(host);
                let created = host
                    .app
                    .windows
                    .values()
                    .any(|tracked| tracked.krate == krate);
                if created || std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            let tracked = host
                .app
                .windows
                .values()
                .find(|tracked| tracked.krate == krate)
                .ok_or_else(|| {
                    eprintln!("krate-adapter: window never appeared after pumping");
                    UiAdapterError::Unsupported(
                        "winit did not create the requested window".to_string(),
                    )
                })?;

            // Android numbers its first (only) window 0, and the shared
            // handle validation reads 0 as "null handle" -- a real bug class
            // on desktops, kept there. This value is an opaque token minted
            // and consumed only by this adapter, never dereferenced, so a
            // fixed offset keeps both truths.
            let raw_handle = u64::from(tracked.window.id()) + 1;
            let inner = tracked.window.inner_size();
            // Everything the app sees is logical pixels (the K-067 rule):
            // divide winit's physical size by the display scale here, once,
            // at the boundary. Input events already do the same.
            let scale = tracked.window.scale_factor().max(0.25);
            let logical_w = (inner.width.max(1) as f64 / scale).round() as u32;
            let logical_h = (inner.height.max(1) as f64 / scale).round() as u32;
            let snapshot = WinitWindowSnapshot::new(
                krate,
                WindowSize::new(logical_w.max(1), logical_h.max(1))?,
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
        use winit::keyboard::{Key, NamedKey, SmolStr};

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

#[cfg(not(target_os = "android"))]
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

    pub fn window_scale(_krate: WindowId) -> f32 {
        1.0
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
