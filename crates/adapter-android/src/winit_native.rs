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

/// Whether the app about to open lays itself out for any screen (IC-912).
static ADAPTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Say, before the app's first window, whether it adapts to any screen.
pub fn set_adapts(adapts: bool) {
    ADAPTS.store(adapts, std::sync::atomic::Ordering::Relaxed);
}

/// The orientation Android is asked to hold for a window the app asked for
/// at `width` x `height`: an app that lays itself out for any screen turns
/// with the phone (`-1`, SCREEN_ORIENTATION_UNSPECIFIED); a fixed design
/// wider than tall holds landscape either way up (`6`,
/// SCREEN_ORIENTATION_SENSOR_LANDSCAPE), so a 16:10 game is not shown in a
/// strip across an upright phone; anything else is the phone's choice.
///
/// Every app was held to the shape of the window it asked for: a notes app
/// that asked for 1024x768 and reads krate:ui/screen could never be used
/// upright, though it lays out for a phone held either way.
pub fn orientation_for(width: u32, height: u32, adapts: bool) -> i32 {
    if !adapts && width > height {
        6
    } else {
        -1
    }
}

/// The picture of a placement that covers the whole window and draws
/// nothing else -- no label, box, clip or selection -- so the picture alone
/// is the frame. A canvas laid out in whole points is up to a few pixels
/// short of the screen (411 points of 411.43 is 1079 pixels of 1080), so a
/// picture that much smaller still counts: `copy_opaque` repeats its last
/// column and row into the gap.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn full_window_pixels(
    placement: &WidgetPlacement,
    width: u32,
    height: u32,
    scale: f32,
) -> Option<&std::sync::Arc<krate_adapter_common::ui::ImagePixels>> {
    let pixels = placement.pixels.as_ref()?;
    let slack = scale.ceil().max(1.0) as u32;
    let near = |have: u32, want: u32| have > 0 && have <= want && want - have <= slack;
    let fits = near(pixels.width, width)
        && near(pixels.height, height)
        && placement.x == 0.0
        && placement.y == 0.0
        && near((placement.width * scale).floor() as u32, width)
        && near((placement.height * scale).floor() as u32, height)
        && placement.clip.is_none()
        && placement.r#box.is_none()
        && placement.label.is_none()
        && placement.selection.is_none()
        && pixels.rgba.len() == pixels.width as usize * pixels.height as usize * 4;
    fits.then_some(pixels)
}

/// Write a straight-RGBA picture `width` pixels wide into a screen buffer
/// `out_width` wide (0x00RRGGBB), in one pass, repeating the picture's last
/// column and row into any pixels it falls short. False, leaving the rest
/// for the full painter to overwrite, at the first pixel that is not
/// opaque: it would need blending over the background.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn copy_opaque(rgba: &[u8], width: usize, out: &mut [u32], out_width: usize) -> bool {
    if width == 0 || out_width < width || !rgba.len().is_multiple_of(width * 4) {
        return false;
    }
    let rows = rgba.len() / (width * 4);
    let out_rows = out.len() / out_width;
    if rows == 0 || out_rows < rows || out.len() != out_rows * out_width {
        return false;
    }
    for (src, dst) in rgba
        .chunks_exact(width * 4)
        .zip(out.chunks_exact_mut(out_width))
    {
        for (px, slot) in src.chunks_exact(4).zip(dst.iter_mut()) {
            if px[3] != 255 {
                return false;
            }
            *slot = (u32::from(px[0]) << 16) | (u32::from(px[1]) << 8) | u32::from(px[2]);
        }
        let last = dst[width - 1];
        dst[width..].fill(last);
    }
    let (filled, rest) = out.split_at_mut(rows * out_width);
    let last_row = &filled[(rows - 1) * out_width..];
    for row in rest.chunks_exact_mut(out_width) {
        row.copy_from_slice(last_row);
    }
    true
}

/// Whether rows `y0..y1` of a screen buffer (0x00RRGGBB, `width` wide) are
/// light enough that the bar over them needs dark icons. Sampled sparsely:
/// it runs every frame.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn light_band(buffer: &[u32], width: usize, y0: usize, y1: usize) -> bool {
    let (mut sum, mut n) = (0.0f32, 0u32);
    let mut y = y0;
    while y < y1 {
        let row = y * width;
        let mut x = width / 64;
        while x < width {
            if let Some(px) = buffer.get(row + x) {
                let (r, g, b) = ((px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF);
                sum += 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
                n += 1;
            }
            x += (width / 32).max(1);
        }
        y += ((y1 - y0) / 6).max(1);
    }
    n > 0 && sum / n as f32 > 0.6 * 255.0
}

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

    /// Ask Android to hold landscape (either way up, by the sensor) for a
    /// window wider than tall, and give the choice back to the phone for any
    /// other. A game drawn 16:10 was otherwise shown in a strip across an
    /// upright phone whenever the person happened to hold it that way.
    fn request_orientation(orientation: i32) {
        ANDROID_APP.with(|slot| {
            let Some(app) = slot.borrow().clone() else {
                return;
            };
            let Ok(vm) = (unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }) else {
                return;
            };
            let Ok(mut env) = vm.attach_current_thread() else {
                return;
            };
            let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
            if env
                .call_method(
                    &activity,
                    "setRequestedOrientation",
                    "(I)V",
                    &[jni::objects::JValue::Int(orientation)],
                )
                .is_err()
            {
                let _ = env.exception_clear();
            }
        });
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
        crate::java::set_handles(app.vm_as_ptr(), app.activity_as_ptr());
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
        /// When a finger landed on the strip under the clock, for Krate's
        /// menu (IC-934); None elsewhere, or once it moved.
        hold_from: Option<std::time::Instant>,
        /// The hold opened the menu: the lift is no tap for the app.
        held: bool,
        /// It began on something that drags -- a slider, or what the app
        /// described as adjustable (a scrubber, a volume) -- so moving it
        /// moves that, never scrolls (K-1059).
        drags: bool,
    }

    /// What the app last described to the screen reader (IC-926), for
    /// knowing what a finger may drag.
    static DESCRIBED: std::sync::Mutex<Vec<krate_adapter_common::ui::AccessElement>> =
        std::sync::Mutex::new(Vec::new());

    /// Keep what the app described (krate:ui/accessibility).
    pub fn set_described(elements: &[krate_adapter_common::ui::AccessElement]) {
        *DESCRIBED.lock().expect("described lock") = elements.to_vec();
    }

    /// The canvases a finger presses rather than scrolls, by Krate window
    /// and widget: games drawn at a design size (K-1202).
    static PRESSING: std::sync::Mutex<
        std::collections::BTreeSet<(WindowId, krate_adapter_common::ui::WidgetId)>,
    > = std::sync::Mutex::new(std::collections::BTreeSet::new());

    /// Note whether a finger on this canvas presses it (the runtime says).
    pub fn set_canvas_presses(
        window: WindowId,
        widget: krate_adapter_common::ui::WidgetId,
        presses: bool,
    ) {
        let mut pressing = PRESSING.lock().expect("pressing lock");
        if presses {
            pressing.insert((window, widget));
        } else {
            pressing.remove(&(window, widget));
        }
    }

    /// Whether a finger landing at (x, y) is on something that drags: a
    /// slider Krate draws, a game's canvas, or an element the app described
    /// as adjustable, with 8 points to spare above and below -- the
    /// iPhone's rule. A canvas that lays itself out for the screen is a
    /// page, and scrolls.
    fn drags_at(window: WindowId, placements: &[WidgetPlacement], x: f32, y: f32) -> bool {
        let inside =
            |px: f32, py: f32, w: f32, h: f32| x >= px && x < px + w && y >= py && y < py + h;
        let pressing = PRESSING.lock().expect("pressing lock");
        placements.iter().any(|p| {
            let drags = match p.kind {
                krate_adapter_common::ui::WidgetKind::Slider => true,
                krate_adapter_common::ui::WidgetKind::Canvas => {
                    pressing.contains(&(window, p.widget))
                }
                _ => false,
            };
            drags && inside(p.x, p.y, p.width, p.height)
        }) || DESCRIBED.lock().expect("described lock").iter().any(|e| {
            e.role == krate_adapter_common::ui::AccessRole::Adjustable
                && inside(e.x, e.y - 8.0, e.width, e.height + 16.0)
        })
    }

    #[derive(Default)]
    struct PumpApp {
        pending_creates: Vec<PendingCreate>,
        touch: Option<TouchGesture>,
        /// Every finger on each window, by Krate window id then finger id,
        /// for apps that ask for all of them (krate:ui/touch).
        fingers: BTreeMap<u64, krate_adapter_common::ui::FingerTracker>,
        /// Where each window's pointer is and whether it is held, for apps
        /// that ask rather than wait for events (krate:ui/pointer): a game
        /// steering by `button-held` and `position`. Fed from the presses
        /// as they leave for the runtime; never fed, it said "not held"
        /// forever, and no game could be steered by finger.
        pointers: BTreeMap<u64, krate_adapter_common::ui::PointerTracker>,
        windows: BTreeMap<NativeWindowId, TrackedWindow>,
        events: CollectedNativeEvents,
        cursor: BTreeMap<NativeWindowId, (f32, f32)>,
        pointer_samples: Vec<RawPointerSample>,
        key_samples: Vec<RawKeySample>,
        wheel_samples: Vec<RawWheelSample>,
        /// Touch-scroll deltas accumulate here between drains: 120 Hz
        /// panels flood one event per report, and an app that draws one
        /// frame per event replays a seconds-long backlog after the finger
        /// stops. One coalesced delta per drain is the gesture's meaning.
        pending_scroll: Option<(WindowId, f32, f32, f32, f32)>,
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
        /// The physical size last reported to Krate. Android sends no resize
        /// when the phone turns (the activity handles the orientation change
        /// itself), so the pump compares and reports the difference.
        reported: (u32, u32),
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
                request_orientation(orientation_for(
                    pending.size.width,
                    pending.size.height,
                    ADAPTS.load(std::sync::atomic::Ordering::Relaxed),
                ));
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
                    // A phone window is the whole screen, whatever size the
                    // app asked for, and Android sends no resize to say so.
                    // Without this Krate kept the requested 1280x800 and the
                    // frame was stretched across a 20:9 screen; reporting the
                    // real size at once lets the app lay out (and render) for
                    // the screen it is actually on.
                    let scale = window.scale_factor().max(0.25);
                    let inner = window.inner_size();
                    if inner.width > 0 && inner.height > 0 {
                        let logical = WindowSize::new(
                            ((f64::from(inner.width) / scale).round() as u32).max(1),
                            ((f64::from(inner.height) / scale).round() as u32).max(1),
                        );
                        if let Ok(logical) = logical {
                            self.events
                                .push((pending.krate, WinitWindowNativeEvent::Resized(logical)));
                        }
                    }
                    self.windows.insert(
                        window.id(),
                        TrackedWindow {
                            krate: pending.krate,
                            reported: (inner.width, inner.height),
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
            BACKGROUNDED.store(true, std::sync::atomic::Ordering::Relaxed);
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
                    if let Some(tracked) = self.windows.get_mut(&native) {
                        tracked.reported = (size.width, size.height);
                    }
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
                WindowEvent::Focused(focused) => {
                    if !focused {
                        // Lifted fingers never arrive once focus is gone.
                        self.fingers.remove(&krate.get());
                        if let Some(pointer) = self.pointers.get_mut(&krate.get()) {
                            pointer.focus_lost();
                        }
                    }
                    Some(WinitWindowNativeEvent::Focused(focused))
                }
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
                    {
                        let fingers = self.fingers.entry(krate.get()).or_default();
                        match touch.phase {
                            winit::event::TouchPhase::Started | winit::event::TouchPhase::Moved => {
                                fingers.touch(touch.id, x, y);
                            }
                            winit::event::TouchPhase::Ended
                            | winit::event::TouchPhase::Cancelled => {
                                fingers.lift(touch.id);
                            }
                        }
                    }
                    match touch.phase {
                        winit::event::TouchPhase::Started => {
                            if self.touch.is_none() {
                                let top = crate::phone::covered_edges().0 / scale as f32;
                                let strip =
                                    crate::menu::chrome_on() && crate::menu::in_strip(y, top);
                                let drags = !strip
                                    && self
                                        .windows
                                        .get(&native)
                                        .is_some_and(|t| drags_at(krate, &t.placements, x, y));
                                self.touch = Some(TouchGesture {
                                    finger: touch.id,
                                    native,
                                    start: (x, y),
                                    last: (x, y),
                                    scrolling: false,
                                    hold_from: strip.then(std::time::Instant::now),
                                    held: false,
                                    drags,
                                });
                                // Something that drags is pressed at once, so
                                // the first movement already moves it.
                                if drags {
                                    self.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x,
                                        y,
                                        pressed: true,
                                    });
                                }
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
                                if gesture.drags {
                                    // The finger moves what it is on: the
                                    // press follows it, held, until it lifts.
                                    gesture.last = (x, y);
                                    self.cursor.insert(native, (x, y));
                                    self.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x,
                                        y,
                                        pressed: true,
                                    });
                                    return;
                                }
                                if !gesture.scrolling {
                                    let sx = x - gesture.start.0;
                                    let sy = y - gesture.start.1;
                                    if sx * sx + sy * sy > SLOP * SLOP {
                                        gesture.scrolling = true;
                                        gesture.hold_from = None;
                                    }
                                }
                                let dx = x - gesture.last.0;
                                let dy = y - gesture.last.1;
                                gesture.last = (x, y);
                                let scrolling = gesture.scrolling;
                                if scrolling && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON)
                                {
                                    self.cursor.insert(native, (x, y));
                                    // Content follows the finger (positive
                                    // dy scrolls deeper); deltas coalesce
                                    // so a 120 Hz flood cannot outrun the
                                    // app's frame rate.
                                    let pending =
                                        self.pending_scroll.get_or_insert((krate, 0.0, 0.0, x, y));
                                    pending.1 -= dx;
                                    pending.2 -= dy;
                                    pending.3 = x;
                                    pending.4 = y;
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
                                if gesture.drags {
                                    // Let go where the finger lifted.
                                    self.cursor.insert(native, (x, y));
                                    self.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x,
                                        y,
                                        pressed: false,
                                    });
                                } else if !gesture.scrolling && !gesture.held {
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
                                        // A tap on a text field brings the
                                        // keyboard up, the same as on any
                                        // phone -- a tap on the field it is
                                        // already in too (K-1024).
                                        if let Some(many_lines) =
                                            crate::typing::field_at(&tracked.placements, x, y)
                                        {
                                            crate::typing::set(krate, true, many_lines);
                                        }
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
                    // The system back gesture/button. Over an app it opens
                    // Krate's menu, where Close is (IC-934): back ended the
                    // app outright, a swipe away from losing it. Over
                    // Krate's own sheet it is still a cancel.
                    if crate::menu::chrome_on() {
                        remember_frame(&self.windows);
                        crate::menu::show();
                        None
                    } else {
                        Some(WinitWindowNativeEvent::CloseRequested)
                    }
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
        // A window that is one opaque canvas the size of the screen -- a
        // game, any app that draws itself -- needs no painting: its pixels
        // go to the screen buffer in one pass. Repainting it through the
        // vector painter as a full-screen picture, then copying that, cost
        // 17 ms of a 26 ms frame on a Galaxy S22 Ultra, holding Nova at 38
        // frames a second (K-1211).
        if let [only] = tracked.placements.as_slice() {
            if let Some(pixels) = super::full_window_pixels(
                only,
                width.get(),
                height.get(),
                tracked.window.scale_factor() as f32,
            ) {
                let Ok(mut buffer) = surface.buffer_mut() else {
                    return;
                };
                if super::copy_opaque(
                    &pixels.rgba,
                    pixels.width as usize,
                    &mut buffer,
                    width.get() as usize,
                ) {
                    colour_bars(&buffer, width.get() as usize, height.get() as usize);
                    let _ = buffer.present();
                    tracked.dirty = false;
                    return;
                }
            }
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
        colour_bars(&buffer, width.get() as usize, height.get() as usize);
        let _ = buffer.present();
        tracked.dirty = false;
    }

    /// Dark bar icons over a light top or bottom, light over dark: what the
    /// app drew under the status bar and the navigation bar decides, each
    /// frame, and Android is told only when it changes.
    fn colour_bars(buffer: &[u32], width: usize, height: usize) {
        static SAID: std::sync::Mutex<Option<(bool, bool)>> = std::sync::Mutex::new(None);
        let (top, _, bottom, _) = crate::phone::covered_edges();
        let band = |covered: f32| (covered.max(24.0) as usize).min(height / 4).max(1);
        let light_top = super::light_band(buffer, width, 0, band(top));
        let light_bottom =
            super::light_band(buffer, width, height.saturating_sub(band(bottom)), height);
        let mut said = SAID.lock().expect("bars lock");
        if *said != Some((light_top, light_bottom)) {
            *said = Some((light_top, light_bottom));
            crate::phone::light_bars(light_top, light_bottom);
        }
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

    /// Keep a picture of what the app shows now, for the permission sheet
    /// to sit over if the person restarts it from Krate's menu (F-314).
    fn remember_frame(windows: &BTreeMap<NativeWindowId, TrackedWindow>) {
        if let Some(tracked) = windows.values().next() {
            let size = tracked.window.inner_size();
            crate::behind::remember(
                &tracked.staging,
                size.width as usize,
                size.height as usize,
                tracked.window.scale_factor() as f32,
            );
        }
    }

    fn pump(host: &mut Host) {
        let Host { event_loop, app } = host;
        let _status = event_loop.pump_app_events(Some(Duration::ZERO), app);
        // A finger held still on the strip under the clock opens Krate's
        // menu, and its lift is then no tap (IC-934).
        if let Some(gesture) = app.touch.as_mut() {
            let due = gesture
                .hold_from
                .is_some_and(|at| at.elapsed().as_millis() >= crate::menu::HOLD_MS);
            if due && !gesture.held {
                gesture.held = true;
                gesture.hold_from = None;
                remember_frame(&app.windows);
                crate::menu::show();
            }
        }
        // Close (or Restart) in the menu: the app hears it as a desktop
        // close button, and Krate closes it anyway if it does not answer.
        // Krate's own screen closes the same way for a link or file tapped
        // while it shows, so the player can open that instead; a running
        // app is never closed for one.
        let close = if crate::menu::chrome_on() {
            crate::menu::take_close()
        } else {
            crate::menu::take_opened_meanwhile()
        };
        if close {
            for tracked in app.windows.values() {
                app.events
                    .push((tracked.krate, WinitWindowNativeEvent::CloseRequested));
            }
        }
        // Turning the phone resizes the window without a resize event; a
        // layout left at the old shape drew every frame cropped and zoomed
        // onto the new one. Whatever changed the size, report it.
        for tracked in app.windows.values_mut() {
            let inner = tracked.window.inner_size();
            if inner.width == 0
                || inner.height == 0
                || (inner.width, inner.height) == tracked.reported
            {
                continue;
            }
            tracked.reported = (inner.width, inner.height);
            tracked.dirty = true;
            let scale = tracked.window.scale_factor().max(0.25);
            if let Ok(logical) = WindowSize::new(
                ((f64::from(inner.width) / scale).round() as u32).max(1),
                ((f64::from(inner.height) / scale).round() as u32).max(1),
            ) {
                app.events
                    .push((tracked.krate, WinitWindowNativeEvent::Resized(logical)));
            }
        }
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

    /// Set when Krate leaves the screen: the person may be going to look at
    /// what an app did in a folder they gave it.
    static BACKGROUNDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Whether Krate left the screen since last asked.
    pub fn take_backgrounded() -> bool {
        BACKGROUNDED.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Pump the native event loop once and keep what it brings for the next
    /// [`pump_native_events`]: Android's lifecycle waits on this thread,
    /// so while Krate waits for the person (a picker, a prompt), it must
    /// still answer.
    pub fn keep_alive() {
        if !host_initialized() {
            return;
        }
        let _ = with_host(|host| {
            pump(host);
            Ok(())
        });
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

    /// Every finger on a window now.
    pub fn touches(id: WindowId) -> Vec<(u64, f32, f32)> {
        if !host_initialized() {
            return Vec::new();
        }
        WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .and_then(|host| host.app.fingers.get_mut(&id.get()))
                .map(|fingers| fingers.read())
                .unwrap_or_default()
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
                .map(|host| {
                    let samples = std::mem::take(&mut host.app.pointer_samples);
                    for sample in &samples {
                        let pointer = host.app.pointers.entry(sample.window.get()).or_default();
                        pointer.moved_to(sample.x, sample.y);
                        pointer.button(
                            krate_adapter_common::ui::PointerButton::Primary,
                            sample.pressed,
                        );
                    }
                    samples
                })
                .unwrap_or_default()
        })
    }

    /// A window's pointer as the presses so far leave it (krate:ui/pointer).
    pub fn pointer_state(id: WindowId) -> krate_adapter_common::ui::PointerTracker {
        if !host_initialized() {
            return Default::default();
        }
        WINIT_HOST.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|host| host.app.pointers.get(&id.get()).copied())
                .unwrap_or_default()
        })
    }

    /// Hand over a window's pointer motion since the last call.
    pub fn take_pointer_motion(id: WindowId) -> (f32, f32) {
        if !host_initialized() {
            return (0.0, 0.0);
        }
        WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .and_then(|host| host.app.pointers.get_mut(&id.get()))
                .map(|pointer| pointer.take_motion())
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
        let coalesced = with_host(|host| {
            Ok(host
                .app
                .pending_scroll
                .take()
                .map(|(krate, dx, dy, x, y)| RawWheelSample {
                    window: krate,
                    x,
                    y,
                    dx,
                    dy,
                    modifiers: host.app.modifiers,
                }))
        })
        .ok()
        .flatten();
        let mut samples = WINIT_HOST.with(|slot| {
            slot.borrow_mut()
                .as_mut()
                .map(|host| std::mem::take(&mut host.app.wheel_samples))
                .unwrap_or_default()
        });
        samples.extend(coalesced);
        samples
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

    /// No Android lifecycle off Android.
    pub fn take_backgrounded() -> bool {
        false
    }

    /// No touch screen to drag on off Android.
    pub fn set_described(_elements: &[krate_adapter_common::ui::AccessElement]) {}

    pub fn set_canvas_presses(
        _window: WindowId,
        _widget: krate_adapter_common::ui::WidgetId,
        _presses: bool,
    ) {
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

    pub fn pointer_state(_id: WindowId) -> krate_adapter_common::ui::PointerTracker {
        Default::default()
    }

    pub fn take_pointer_motion(_id: WindowId) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Winit windows are only available in Android builds.
    pub fn touches(_id: WindowId) -> Vec<(u64, f32, f32)> {
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

#[cfg(test)]
mod bar_tests {
    use super::light_band;

    #[test]
    fn a_light_top_wants_dark_icons_and_a_dark_one_light() {
        let (w, h) = (90usize, 40usize);
        let mut buf = vec![0x00F5_F5F7u32; w * h];
        assert!(light_band(&buf, w, 0, 10), "Krate's own light home");
        for px in &mut buf[..w * 10] {
            *px = 0x0008_080A;
        }
        assert!(!light_band(&buf, w, 0, 10), "a dark game");
        assert!(light_band(&buf, w, 30, 40), "the bottom is still light");
    }
}

#[cfg(test)]
mod full_window_tests {
    use super::*;
    use krate_adapter_common::ui::{ImagePixels, WidgetId, WidgetKind};

    fn canvas(w: u32, h: u32, scale: f32, rgba: Vec<u8>) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(2).expect("id"),
            kind: WidgetKind::Canvas,
            width: w as f32 / scale,
            height: h as f32 / scale,
            pixels: Some(std::sync::Arc::new(ImagePixels {
                width: w,
                height: h,
                rgba,
            })),
            ..Default::default()
        }
    }

    /// K-1211: a full-window opaque canvas is copied straight to the screen
    /// buffer, byte for byte as 0x00RRGGBB; anything else is painted.
    #[test]
    fn a_full_window_canvas_goes_straight_to_the_screen() {
        let rgba: Vec<u8> = (0..4 * 6)
            .map(|i| if i % 4 == 3 { 255 } else { i as u8 })
            .collect();
        let p = canvas(3, 2, 1.5, rgba.clone());
        let pixels = full_window_pixels(&p, 3, 2, 1.5).expect("covers the window");
        let mut out = vec![0u32; 6];
        assert!(copy_opaque(&pixels.rgba, 3, &mut out, 3));
        assert_eq!(out[1], (4 << 16) | (5 << 8) | 6);
        // A canvas a pixel short of the screen each way (whole points at a
        // fractional scale) still goes straight there, its edges repeated.
        assert!(
            full_window_pixels(&p, 4, 3, 1.5).is_some(),
            "one pixel short counts"
        );
        let mut wide = vec![0u32; 12];
        assert!(copy_opaque(&pixels.rgba, 3, &mut wide, 4));
        assert_eq!(wide[3], wide[2], "the last column is repeated");
        assert_eq!(wide[8..12], wide[4..8], "the last row is repeated");
        assert!(
            full_window_pixels(&p, 9, 2, 1.5).is_none(),
            "far smaller is painted"
        );
        let mut labelled = p.clone();
        labelled.label = Some("x".into());
        assert!(
            full_window_pixels(&labelled, 3, 2, 1.5).is_none(),
            "a label is painted"
        );
        let mut see_through = rgba;
        see_through[7] = 128;
        assert!(
            !copy_opaque(&see_through, 3, &mut out, 3),
            "a clear pixel needs blending"
        );
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::orientation_for;

    #[test]
    fn an_app_that_adapts_turns_with_the_phone_and_a_wide_design_holds_landscape() {
        // A fixed 16:10 game holds landscape.
        assert_eq!(orientation_for(1280, 800, false), 6);
        // The same window from an app that reads krate:ui/screen turns.
        assert_eq!(orientation_for(1280, 800, true), -1);
        // Taller than wide, or square: the phone decides.
        assert_eq!(orientation_for(390, 844, false), -1);
        assert_eq!(orientation_for(600, 600, false), -1);
    }
}
