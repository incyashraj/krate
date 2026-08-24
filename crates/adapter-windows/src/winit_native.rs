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

#[cfg(any(target_os = "windows", feature = "dev-anyos"))]
pub use real::*;

#[cfg(not(any(target_os = "windows", feature = "dev-anyos")))]
pub use stub::*;

#[cfg(any(target_os = "windows", feature = "dev-anyos"))]
use krate_adapter_common::ui::Modifiers;
use krate_adapter_common::ui::{
    RawKeySample, RawPointerSample, RawWheelSample, UiAdapterError, WidgetPlacement, WindowId,
    WindowSize, WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Krate window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

// `dev-anyos` exists because this module was invisible to every build and
// test on the machine that develops it: cfg(windows) meant macOS compiled
// the stub, "cargo build succeeded" proved nothing, and Windows-only compile
// errors shipped twice. With the feature on, the REAL windowing and GPU code
// compiles and runs on the dev Mac (winit and wgpu are portable), so it is
// judged before CI rather than by users.
#[cfg(any(target_os = "windows", feature = "dev-anyos"))]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use std::num::NonZeroU32;
    use std::sync::Arc;

    use winit::application::ApplicationHandler;
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
        /// GPU init or a present failed once: stay on the CPU path for the
        /// life of the window rather than retrying into the same wall every
        /// frame. The fallback is the contract; flapping is not.
        gpu_dead: bool,
        /// When the most recent user input arrived, for S3's
        /// input-to-present latency measurement.
        last_input: Option<std::time::Instant>,
        placements: Vec<WidgetPlacement>,
        hovered: Option<krate_adapter_common::ui::WidgetId>,
        pressed_widget: Option<krate_adapter_common::ui::WidgetId>,
        /// Full-bleed took the title bar away; the adapter owes this window
        /// its own close and minimize controls (K-168).
        undecorated: bool,
        /// The overlay sprite, cached per pixel density (key: density x100).
        overlay_cache: Option<(u32, Vec<u8>, u32, u32)>,
    }

    impl TrackedWindow {
        /// The overlay sprite at a pixel density, rasterized on first use.
        fn overlay_sprite(&mut self, px_per_logical: f32) -> (&[u8], u32, u32) {
            let key = (px_per_logical * 100.0).round() as u32;
            if self.overlay_cache.as_ref().map(|(k, ..)| *k) != Some(key) {
                let (rgba, w, h) = krate_adapter_common::overlay::sprite(px_per_logical);
                self.overlay_cache = Some((key, rgba, w, h));
            }
            let (_, rgba, w, h) = self.overlay_cache.as_ref().expect("cached above");
            (rgba, *w, *h)
        }
    }

    /// The shared windowed presenter, wrapped with what only the adapter
    /// knows: the winit window's size, scale, and the moment of the last
    /// input (for S3's input-to-present latency).
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
            overlay_sprite: Option<(&[u8], u32, u32)>,
        ) -> Result<(), String> {
            let size = window.inner_size();
            window.pre_present_notify();
            // The game case first: one canvas covering the window. The full
            // scene pipeline re-uploaded that canvas as a fresh vello image
            // resource every frame, which cost ~18ms a frame on an Iris Xe
            // and held a 60fps-capable game at 30. A persistent texture,
            // one write and one scaling blit is the whole job -- the same
            // path the macOS Metal canvas takes.
            if let [only] = placements {
                if only.kind == krate_adapter_common::ui::WidgetKind::Canvas {
                    if let Some(pixels) = &only.pixels {
                        let scale = window.scale_factor() as f32;
                        let covers = only.x.abs() <= 1.0
                            && only.y.abs() <= 1.0
                            && (only.width * scale - size.width as f32).abs() <= 2.0 * scale
                            && (only.height * scale - size.height as f32).abs() <= 2.0 * scale;
                        let canvas_aspect = pixels.width as f32 / pixels.height as f32;
                        let window_aspect = size.width as f32 / size.height as f32;
                        let aspect_ok =
                            (canvas_aspect - window_aspect).abs() / window_aspect < 0.02;
                        if covers && aspect_ok {
                            return self.inner.present_pixels_into(
                                &pixels.rgba,
                                pixels.width,
                                pixels.height,
                                size.width,
                                size.height,
                                overlay_sprite,
                            );
                        }
                    }
                }
            }
            self.inner.render(
                size.width,
                size.height,
                window.scale_factor() as f32,
                placements,
                interaction,
                input_at,
                overlay_sprite.is_some(),
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
            // NamedKey::Space, so a bare to_string() names the key " " and no
            // app ever matches key_held("Space") -- which is why Space did
            // nothing on Windows while it worked on macOS, whose AppKit path
            // names keycode 49 "Space" outright. Normalise it here so every
            // platform hands the app the same name.
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
                    // LOGICAL, and this is the load-bearing unit decision.
                    //
                    // The app speaks logical units everywhere: it asks for a
                    // 420x652 window, lays out in those units, and the shared
                    // painter multiplies every coordinate by the scale factor
                    // when it rasterizes. Creating the window with that size
                    // as PHYSICAL pixels (the previous fix) made the buffer
                    // 420x652 while the painter drew 630x978 on a 150%
                    // display: the bottom third of every app was painted
                    // outside the buffer. That is the game whose rocket was
                    // simply not on screen until the person dragged the
                    // window bigger. Invisible at 100% scaling and on the
                    // Mac's native adapter, which is why it shipped twice --
                    // once in each direction.
                    //
                    // Logical creation means winit sizes the physical buffer
                    // to scale x request, which is exactly the area the
                    // painter fills: complete, and sharp at native density.
                    .with_inner_size(winit::dpi::LogicalSize::new(
                        f64::from(pending.size.width),
                        f64::from(pending.size.height),
                    ))
                    // Visible on creation, matching macOS. Hidden-until-shown
                    // assumed every app calls `window.show`, and none of them
                    // do: the samples create a window and start drawing. On
                    // macOS that displays; here it produced a running app with
                    // nothing on screen, which is what a first Windows user
                    // saw when they ran a 3D game and got a frame count.
                    //
                    // `show` still works and is still the way to reveal a
                    // window deliberately; it is simply no longer required to
                    // see anything at all.
                    .with_visible(true);
                if let Ok(window) = event_loop.create_window(attributes) {
                    // Windows will happily create a window BIGGER THAN THE
                    // SCREEN. A chess app asked for 1280x840 logical on a
                    // 1080p display at 150% scaling -- 1920x1260 physical --
                    // and got exactly that: a window whose bottom 240 pixels
                    // hung below the monitor, board cut off mid-rank, lower
                    // pieces unreachable. macOS constrains windows to the
                    // screen's visible frame on its own, which is why the
                    // same app was fine there and the founder met this only
                    // on Windows (K-167).
                    //
                    // So constrain it ourselves: clamp to the current
                    // monitor, less a conservative allowance for the title
                    // bar and taskbar (winit exposes the monitor's full
                    // size, not the work area), and pull the window to the
                    // top-left so the clamped size is actually on screen.
                    let scale = window.scale_factor();
                    if let Some(monitor) = window.current_monitor() {
                        let monitor_size = monitor.size();
                        let (max_w, max_h) = (
                            ((f64::from(monitor_size.width) / scale) - 16.0).max(320.0),
                            ((f64::from(monitor_size.height) / scale) - 96.0).max(240.0),
                        );
                        let inner = window.inner_size();
                        let (cur_w, cur_h) = (
                            f64::from(inner.width) / scale,
                            f64::from(inner.height) / scale,
                        );
                        if cur_w > max_w || cur_h > max_h {
                            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(
                                cur_w.min(max_w),
                                cur_h.min(max_h),
                            ));
                            window.set_outer_position(winit::dpi::LogicalPosition::new(16.0, 16.0));
                        }
                    }
                    // And read what actually exists now. When it differs
                    // from the request, say so through the same Resized
                    // event a drag produces -- the app already knows how to
                    // relayout; it was only ever missing the truth.
                    let actual = window.inner_size();
                    let (lw, lh) = (
                        (f64::from(actual.width) / scale).round() as u32,
                        (f64::from(actual.height) / scale).round() as u32,
                    );
                    if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                        eprintln!(
                            "krate-window: created physical={}x{} scale={scale} logical={lw}x{lh} requested={}x{}",
                            actual.width, actual.height, pending.size.width, pending.size.height
                        );
                    }
                    if (lw, lh) != (pending.size.width, pending.size.height) {
                        if let Ok(size) = WindowSize::new(lw.max(1), lh.max(1)) {
                            self.events
                                .push((pending.krate, WinitWindowNativeEvent::Resized(size)));
                        }
                    }
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
                            undecorated: false,
                            overlay_cache: None,
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
            // input-to-present from this. Redraws and focus shuffles are not
            // input.
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
                    // Logical to the app, like every other size and every
                    // pointer coordinate. Reporting physical here sent a
                    // resize-aware app into a spiral on scaled displays: it
                    // laid out to the physical number, the painter multiplied
                    // by scale again, and the content outgrew the window by
                    // the scale factor at every step.
                    let scale = self
                        .windows
                        .get(&native)
                        .map(|tracked| tracked.window.scale_factor())
                        .unwrap_or(1.0);
                    let (w, h) = (
                        (f64::from(size.width) / scale).round() as u32,
                        (f64::from(size.height) / scale).round() as u32,
                    );
                    if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                        eprintln!(
                            "krate-window: resized physical={}x{} logical={w}x{h}",
                            size.width, size.height
                        );
                    }
                    WindowSize::new(w.max(1), h.max(1))
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
                        // The overlay controls eat their clicks before the
                        // app can see them (K-168): a press in the cluster is
                        // ours, and the release is the action.
                        if let Some(tracked) = self.windows.get(&native) {
                            if tracked.undecorated {
                                let scale = tracked.window.scale_factor();
                                let lw =
                                    (f64::from(tracked.window.inner_size().width) / scale) as f32;
                                if let Some(control) = krate_adapter_common::overlay::hit(lw, x, y)
                                {
                                    if !pressed {
                                        match control {
                                            krate_adapter_common::overlay::ControlHit::Close => {
                                                self.events.push((
                                                    krate,
                                                    WinitWindowNativeEvent::CloseRequested,
                                                ));
                                            }
                                            krate_adapter_common::overlay::ControlHit::Minimize => {
                                                tracked.window.set_minimized(true);
                                            }
                                        }
                                    }
                                    return;
                                }
                            }
                        }
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
        // The GPU path first: vello on the window's own surface, vsync-paced.
        // KRATE_CPU_PRESENT=1 forces the CPU painter for A/B runs and
        // support; any GPU failure retires the path for this window and the
        // CPU painter takes over -- the fallback is the contract, flapping
        // between the two is not.
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
            if tracked.gpu.is_some() {
                let interaction = krate_adapter_common::painter::PaintInteraction {
                    hovered: tracked.hovered,
                    pressed: tracked.pressed_widget,
                };
                let input_at = tracked.last_input.take();
                // The overlay sprite's density: for the canvas fast path the
                // sprite lands in CANVAS pixels (later scaled to the window),
                // so its density is canvas-pixels-per-logical; the vector
                // scene path ignores the sprite and draws at window scale.
                let overlay = if tracked.undecorated {
                    let scale = tracked.window.scale_factor();
                    let logical_w =
                        (f64::from(tracked.window.inner_size().width) / scale).max(1.0) as f32;
                    let density = match &tracked.placements[..] {
                        [only] if only.pixels.is_some() => only
                            .pixels
                            .as_ref()
                            .map(|px| px.width as f32 / logical_w)
                            .unwrap_or(scale as f32),
                        _ => scale as f32,
                    };
                    let (rgba, w, h) = tracked.overlay_sprite(density);
                    // Borrow dance: the sprite borrows tracked, and so does
                    // gpu.render. Clone the small sprite instead of fighting
                    // the borrow checker with unsafe.
                    Some((rgba.to_vec(), w, h))
                } else {
                    None
                };
                let gpu = tracked.gpu.as_mut().expect("checked above");
                let overlay_ref = overlay.as_ref().map(|(rgba, w, h)| (&rgba[..], *w, *h));
                match gpu.render(
                    &tracked.window,
                    &tracked.placements,
                    interaction,
                    input_at,
                    overlay_ref,
                ) {
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
        // Rasterized before the surface borrow starts; the buffer below holds
        // a mutable borrow of the window for its whole scope.
        let cpu_scale = tracked.window.scale_factor() as f32;
        let cpu_overlay = if tracked.undecorated {
            let (rgba, w, h) = tracked.overlay_sprite(cpu_scale);
            Some((rgba.to_vec(), w, h))
        } else {
            None
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
            cpu_scale,
            &tracked.placements,
            krate_adapter_common::painter::PaintInteraction {
                hovered: tracked.hovered,
                pressed: tracked.pressed_widget,
            },
        );
        // The full-bleed window controls, blended over the frame's top-right
        // corner. softbuffer pixels are 0RGB u32s.
        if let Some((sprite, sw, sh)) = cpu_overlay {
            let (bw, bh) = (width.get(), height.get());
            let ox = bw.saturating_sub(sw);
            for y in 0..sh.min(bh) {
                for x in 0..sw.min(bw) {
                    let si = ((y * sw + x) * 4) as usize;
                    let a = u32::from(sprite[si + 3]);
                    if a == 0 {
                        continue;
                    }
                    let di = (y * bw + ox + x) as usize;
                    let under = buffer[di];
                    let blend = |over: u32, under: u32| (over * a + under * (255 - a)) / 255;
                    let r = blend(u32::from(sprite[si]), (under >> 16) & 0xff);
                    let g = blend(u32::from(sprite[si + 1]), (under >> 8) & 0xff);
                    let b = blend(u32::from(sprite[si + 2]), under & 0xff);
                    buffer[di] = (r << 16) | (g << 8) | b;
                }
            }
        }
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

    /// Repaint only the named windows.
    ///
    /// The pump calls this with the windows the OS actually asked to redraw
    /// this drain. It used to call `redraw_all` on EVERY pump, and a frame
    /// loop pumps dozens of times per frame -- on the GPU path each repaint
    /// blocks a full vsync, so one guest frame cost ~62 vsyncs and a game
    /// ran at under 2 fps while pinning three cores (measured on an Iris
    /// Xe desktop). Painting belongs to publishes and to real OS redraw
    /// requests, never to the act of checking for events.
    pub fn redraw_windows(targets: &[WindowId]) -> Result<(), UiAdapterError> {
        if targets.is_empty() || !host_initialized() {
            return Ok(());
        }
        with_host(|host| {
            for tracked in host.app.windows.values_mut() {
                if targets.contains(&tracked.krate) {
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
                let mut builder = EventLoop::builder();
                // Tests run on worker threads, where winit refuses to build
                // an event loop by default. The opt-in keeps production on
                // the safe main-thread default.
                #[cfg(target_os = "windows")]
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
            // Logical, matching creation and resize; the scale rides beside
            // it for anything that needs the physical density.
            let scale = tracked.window.scale_factor();
            let (lw, lh) = (
                (f64::from(inner.width) / scale).round() as u32,
                (f64::from(inner.height) / scale).round() as u32,
            );
            let snapshot = WinitWindowSnapshot::new(
                krate,
                WindowSize::new(lw.max(1), lh.max(1))?,
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

    /// Turn the window's title bar and border off (or back on).
    ///
    /// This is what full-bleed means on Windows: an undecorated window whose
    /// client area is the whole window, so the app paints to every edge. macOS
    /// keeps its traffic lights and overlays them on the app's own drawing;
    /// Windows has no equivalent overlay, so the buttons go away with the
    /// frame. That is a real difference, and the honest one -- the alternative
    /// is extending the client area into the frame with DwmExtendFrameIntoClientArea
    /// and hit-testing the caption by hand, which is a lot of surface to get
    /// wrong for a cosmetic gain.
    ///
    /// Returns the window's size afterwards so the caller can report a resize:
    /// losing the title bar changes the client area, and a canvas that does not
    /// refit leaves a band of stale pixels where the frame used to be.
    pub fn set_native_window_full_bleed(
        krate: WindowId,
        enabled: bool,
    ) -> Result<Option<WindowSize>, UiAdapterError> {
        if !host_initialized() {
            return Ok(None);
        }
        with_host(|host| {
            Ok(host
                .app
                .windows
                .values_mut()
                .find(|tracked| tracked.krate == krate)
                .map(|tracked| {
                    tracked.window.set_decorations(!enabled);
                    // No title bar means no close or minimize button; the
                    // draw paths overlay our own and the input path routes
                    // their clicks (K-168).
                    tracked.undecorated = enabled;
                    let size = tracked.window.inner_size();
                    let scale = tracked.window.scale_factor();
                    let logical = size.to_logical::<f64>(scale);
                    WindowSize {
                        width: logical.width.round().max(1.0) as u32,
                        height: logical.height.round().max(1.0) as u32,
                    }
                }))
        })
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
            // nothing on Windows: launch a game, type a space, both dead.
            assert_eq!(
                key_name(&Key::Character(SmolStr::new(" "))).as_deref(),
                Some("Space")
            );
            // The named variant, if winit ever sends it, must land the same.
            assert_eq!(
                key_name(&Key::Named(NamedKey::Space)).as_deref(),
                Some("Space")
            );
            // Ordinary characters still pass through as themselves.
            assert_eq!(
                key_name(&Key::Character(SmolStr::new("a"))).as_deref(),
                Some("a")
            );
            // Arrows keep their portable names.
            assert_eq!(
                key_name(&Key::Named(NamedKey::ArrowLeft)).as_deref(),
                Some("ArrowLeft")
            );
        }
    }
}

#[cfg(not(any(target_os = "windows", feature = "dev-anyos")))]
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
    pub fn set_native_window_full_bleed(
        _krate: WindowId,
        _enabled: bool,
    ) -> Result<Option<WindowSize>, UiAdapterError> {
        unsupported()
    }

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
    pub fn redraw_windows(_targets: &[WindowId]) -> Result<(), UiAdapterError> {
        unsupported()
    }

    /// Winit windows are only available in Windows builds.
    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        Vec::new()
    }

    /// Winit windows are only available in Windows builds.
    pub fn drain_key_samples() -> Vec<RawKeySample> {
        Vec::new()
    }

    /// Winit windows are only available in Windows builds.
    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        Vec::new()
    }
}
