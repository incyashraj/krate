//! Real UIKit windowing for the iOS adapter.
//!
//! Two threads, on purpose. The main thread belongs to UIKit: it returns
//! from every delegate callout and UIApplicationMain's own run loop runs
//! free. The guest lives on a second thread, and every UIKit touch (create
//! the window, put a frame on screen) is marshaled to the main queue with
//! a synchronous dispatch. The first design ran the guest inside a
//! delegate callout and pumped a nested run loop -- but a nested run loop
//! never drains the main dispatch queue, iOS delivers parts of its own
//! touch pipeline through that queue, and the result was input that
//! arrived whenever something else happened to jostle the loop. A real
//! iPhone said "very late in response"; this split is the answer.
//!
//! Input flows the other way without blocking anyone: touch callouts on
//! the main thread push raw samples into shared state and signal a
//! condvar; the guest's park waits on that condvar and wakes the instant
//! a finger lands. Rasterization happens on the guest thread -- the main
//! thread only wraps finished pixels in a CGImage and hands them to the
//! view.
//!
//! iOS has one screen-sized window; `create_native_window` creates it on
//! first use and re-dresses it for each sequential guest (the wall sheet,
//! then the app).

#[cfg(target_os = "ios")]
pub use real::*;

#[cfg(not(target_os = "ios"))]
pub use stub::*;

use krate_adapter_common::ui::{
    RawKeySample, RawPointerSample, RawWheelSample, UiAdapterError, WidgetPlacement, WindowId,
    WindowSize, WinitWindowNativeEvent, WinitWindowSnapshot,
};

/// Native events paired with the Krate window they belong to.
pub type CollectedNativeEvents = Vec<(WindowId, WinitWindowNativeEvent)>;

#[cfg(target_os = "ios")]
mod real {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Condvar, Mutex, OnceLock};

    use dispatch2::DispatchQueue;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpaceCreateDeviceRGB,
        CGImageAlphaInfo, CGImageByteOrderInfo,
    };
    use objc2_foundation::NSObjectProtocol;
    use objc2_ui_kit::{
        UIApplication, UIImage, UIImageView, UIScreen, UITouch, UITouchPhase, UIView,
        UIViewController, UIWindow, UIWindowScene,
    };

    /// One raw touch, in logical points, straight from the main-thread
    /// callouts. The gesture logic runs on the guest thread.
    #[derive(Clone, Copy)]
    struct RawTouch {
        phase: UITouchPhase,
        x: f32,
        y: f32,
        finger: usize,
    }

    /// State both threads touch, under one lock. Kept deliberately small:
    /// raw input in, screen geometry out.
    struct Shared {
        touches: Vec<RawTouch>,
        /// Set once the main thread has built the window.
        screen: Option<(WindowSize, f32)>,
        /// The CAMetalLayer's raw pointer plus the FULL native scale --
        /// the GPU renders at true density, so the CPU path's 2x quality
        /// cap dies here.
        metal_layer: Option<(usize, f32)>,
    }

    static SHARED: Mutex<Shared> = Mutex::new(Shared {
        touches: Vec::new(),
        screen: None,
        metal_layer: None,
    });

    /// Signaled by the touch callouts so a parked guest wakes instantly.
    static INPUT_ARRIVED: Condvar = Condvar::new();

    /// Guest-thread state: gesture synthesis, samples, and the raster
    /// buffer. Only the guest thread touches it.
    struct GuestSide {
        krate: Option<WindowId>,
        logical: WindowSize,
        scale: f32,
        placements: Vec<WidgetPlacement>,
        events: CollectedNativeEvents,
        pointer_samples: Vec<RawPointerSample>,
        key_samples: Vec<RawKeySample>,
        wheel_samples: Vec<RawWheelSample>,
        /// Scroll deltas accumulate here between drains: an iPhone reports
        /// touches at 120 Hz, and queueing one wheel event per report made
        /// the app replay a two-second backlog after the finger stopped
        /// (one frame per event, 16 ms floor). One coalesced delta per
        /// drain is what a finger actually means.
        pending_scroll: Option<(f32, f32, f32, f32)>,
        gesture: Option<TouchGesture>,
        hovered: Option<krate_adapter_common::ui::WidgetId>,
        pressed_widget: Option<krate_adapter_common::ui::WidgetId>,
        paint_buffer: Vec<u32>,
        dirty: bool,
    }

    struct TouchGesture {
        finger: usize,
        start: (f32, f32),
        last: (f32, f32),
        scrolling: bool,
    }

    thread_local! {
        static GUEST: RefCell<Option<GuestSide>> = const { RefCell::new(None) };
    }

    /// Main-thread-only UIKit objects, created on first use via dispatch.
    struct MainSide {
        _window: Retained<UIWindow>,
        _controller: Retained<UIViewController>,
        _metal: Retained<objc2_quartz_core::CAMetalLayer>,
        view: Retained<KrateSurfaceView>,
    }

    struct MainSideCell(OnceLock<MainSide>);
    // SAFETY: the cell is written and read only on the main thread, inside
    // main-queue closures; it only crosses threads as an opaque pointer.
    unsafe impl Send for MainSideCell {}
    unsafe impl Sync for MainSideCell {}
    static MAIN_SIDE: MainSideCell = MainSideCell(OnceLock::new());

    define_class!(
        // SAFETY:
        // - UIImageView has no extra subclassing requirements for touch
        //   overrides.
        // - UIKit delivers touches on the main thread.
        #[unsafe(super = UIImageView)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ()]
        struct KrateSurfaceView;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateSurfaceView {}

        impl KrateSurfaceView {
            // SAFETY: signatures match UIResponder's touch methods.
            #[unsafe(method(touchesBegan:withEvent:))]
            fn touches_began(
                &self,
                touches: &objc2_foundation::NSSet<UITouch>,
                _event: Option<&objc2_ui_kit::UIEvent>,
            ) {
                record_touches(self, touches);
            }

            #[unsafe(method(touchesMoved:withEvent:))]
            fn touches_moved(
                &self,
                touches: &objc2_foundation::NSSet<UITouch>,
                _event: Option<&objc2_ui_kit::UIEvent>,
            ) {
                record_touches(self, touches);
            }

            #[unsafe(method(touchesEnded:withEvent:))]
            fn touches_ended(
                &self,
                touches: &objc2_foundation::NSSet<UITouch>,
                _event: Option<&objc2_ui_kit::UIEvent>,
            ) {
                record_touches(self, touches);
            }

            #[unsafe(method(touchesCancelled:withEvent:))]
            fn touches_cancelled(
                &self,
                touches: &objc2_foundation::NSSet<UITouch>,
                _event: Option<&objc2_ui_kit::UIEvent>,
            ) {
                record_touches(self, touches);
            }
        }
    );

    fn record_touches(view: &KrateSurfaceView, touches: &objc2_foundation::NSSet<UITouch>) {
        let uiview: &UIView = view;
        let mut shared = SHARED.lock().expect("shared state lock");
        for touch in touches.allObjects() {
            let location: CGPoint = unsafe { touch.locationInView(Some(uiview)) };
            let finger = Retained::as_ptr(&touch) as usize;
            shared.touches.push(RawTouch {
                phase: unsafe { touch.phase() },
                x: location.x as f32,
                y: location.y as f32,
                finger,
            });
        }
        drop(shared);
        INPUT_ARRIVED.notify_all();
    }

    /// Build the UIKit side, on the main queue, once.
    fn ensure_main_side() -> Result<(WindowSize, f32), UiAdapterError> {
        if let Some(screen) = SHARED.lock().expect("shared state lock").screen {
            return Ok(screen);
        }
        let mut result: Result<(WindowSize, f32), UiAdapterError> =
            Err(UiAdapterError::Internal(
                "main-queue closure never ran".to_string(),
            ));
        DispatchQueue::main().exec_sync(|| {
            result = (|| {
                let mtm = MainThreadMarker::new().ok_or_else(|| {
                    UiAdapterError::Internal(
                        "main-queue closure ran off the main thread".to_string(),
                    )
                })?;
                let screen = UIScreen::mainScreen(mtm);
                let bounds: CGRect = screen.bounds();
                // Capped at 2x deliberately: CPU-rasterizing full 3x spent
                // the whole frame budget; the view's GPU compositor does
                // the final stretch free (K-089).
                let scale = (screen.scale() as f32).min(2.0);

                let window = unsafe { UIWindow::initWithFrame(UIWindow::alloc(mtm), bounds) };
                // The window MUST join the connected UIWindowScene: since
                // iOS 13 an unattached window still renders but rides a
                // legacy event path that starves continuous touch delivery
                // -- a real 120 Hz phone produced 116 touch callouts in an
                // entire scrolling session before this line existed.
                unsafe {
                    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();
                    for scene in scenes.allObjects() {
                        if let Ok(window_scene) = scene.downcast::<UIWindowScene>() {
                            window.setWindowScene(Some(&window_scene));
                            break;
                        }
                    }
                }
                let controller = unsafe { UIViewController::new(mtm) };
                let view: Retained<KrateSurfaceView> = {
                    let this = KrateSurfaceView::alloc(mtm).set_ivars(());
                    // SAFETY: UIView's initWithFrame is the designated
                    // initializer.
                    unsafe { msg_send![super(this), initWithFrame: bounds] }
                };
                unsafe {
                    view.setUserInteractionEnabled(true);
                    let as_uiview: &UIView = &view;
                    controller.setView(Some(as_uiview));
                    window.setRootViewController(Some(&controller));
                    window.makeKeyAndVisible();
                }
                let logical = WindowSize::new(
                    (bounds.size.width as u32).max(1),
                    (bounds.size.height as u32).max(1),
                )
                .map_err(|err| UiAdapterError::Unsupported(err.to_string()))?;

                // The GPU canvas draws into a CAMetalLayer sublayer sized
                // to the full view at FULL native density -- the 2x cap is
                // a CPU-raster economy the GPU does not need.
                let native_scale = screen.scale() as f32;
                let metal: objc2::rc::Retained<objc2_quartz_core::CAMetalLayer> =
                    unsafe { objc2_quartz_core::CAMetalLayer::new() };
                unsafe {
                    metal.setFrame(bounds);
                    metal.setContentsScale(native_scale as f64);
                    let as_uiview: &UIView = &view;
                    as_uiview.layer().addSublayer(&metal);
                }
                let metal_ptr =
                    objc2::rc::Retained::as_ptr(&metal) as usize;
                SHARED.lock().expect("shared state lock").metal_layer =
                    Some((metal_ptr, native_scale));

                let _ = MAIN_SIDE.0.set(MainSide {
                    _metal: metal,
                    _window: window,
                    _controller: controller,
                    view,
                });
                Ok((logical, scale))
            })();
        });
        let screen = result?;
        SHARED.lock().expect("shared state lock").screen = Some(screen);
        Ok(screen)
    }

    fn with_guest<T>(
        f: impl FnOnce(&mut GuestSide) -> Result<T, UiAdapterError>,
    ) -> Result<T, UiAdapterError> {
        GUEST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let (logical, scale) = ensure_main_side()?;
                *slot = Some(GuestSide {
                    krate: None,
                    logical,
                    scale,
                    placements: Vec::new(),
                    events: Vec::new(),
                    pointer_samples: Vec::new(),
                    key_samples: Vec::new(),
                    wheel_samples: Vec::new(),
                    pending_scroll: None,
                    gesture: None,
                    hovered: None,
                    pressed_widget: None,
                    paint_buffer: Vec::new(),
                    dirty: false,
                });
            }
            f(slot.as_mut().expect("guest side initialized"))
        })
    }

    fn guest_initialized() -> bool {
        GUEST.with(|slot| slot.borrow().is_some())
    }

    /// The tap-or-scroll synthesis: within the slop a touch is a tap
    /// (pointer press + release), past it every move is a wheel delta --
    /// the contract the Android adapter proved on-device.
    fn digest_touches(guest: &mut GuestSide) {
        const SLOP: f32 = 8.0;
        let raw: Vec<RawTouch> = {
            let mut shared = SHARED.lock().expect("shared state lock");
            std::mem::take(&mut shared.touches)
        };
        let Some(krate) = guest.krate else {
            return;
        };
        for touch in raw {
            match touch.phase {
                UITouchPhase::Began => {
                    if guest.gesture.is_none() {
                        guest.gesture = Some(TouchGesture {
                            finger: touch.finger,
                            start: (touch.x, touch.y),
                            last: (touch.x, touch.y),
                            scrolling: false,
                        });
                        guest.hovered = krate_adapter_common::painter::topmost_interactive_at(
                            &guest.placements,
                            touch.x,
                            touch.y,
                        );
                    }
                }
                UITouchPhase::Moved => {
                    let same = guest
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        let gesture = guest.gesture.as_mut().expect("gesture checked");
                        if !gesture.scrolling {
                            let sx = touch.x - gesture.start.0;
                            let sy = touch.y - gesture.start.1;
                            if sx * sx + sy * sy > SLOP * SLOP {
                                gesture.scrolling = true;
                            }
                        }
                        let dx = touch.x - gesture.last.0;
                        let dy = touch.y - gesture.last.1;
                        gesture.last = (touch.x, touch.y);
                        if gesture.scrolling
                            && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON)
                        {
                            let pending =
                                guest.pending_scroll.get_or_insert((0.0, 0.0, touch.x, touch.y));
                            pending.0 -= dx;
                            pending.1 -= dy;
                            pending.2 = touch.x;
                            pending.3 = touch.y;
                        }
                    }
                }
                UITouchPhase::Ended => {
                    let same = guest
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        let gesture = guest.gesture.take().expect("gesture checked");
                        if !gesture.scrolling {
                            guest.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: true,
                            });
                            guest.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: false,
                            });
                        }
                    }
                }
                UITouchPhase::Cancelled => {
                    let same = guest
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        guest.gesture = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Rasterize on the guest thread, then hand finished pixels to the
    /// main queue, which only wraps them in a CGImage and sets the view.
    /// The autoreleasepool per frame is the macOS adapter's 46 GB lesson.
    fn blit(guest: &mut GuestSide) {
        let phys_w = (guest.logical.width as f32 * guest.scale) as usize;
        let phys_h = (guest.logical.height as f32 * guest.scale) as usize;
        if phys_w == 0 || phys_h == 0 {
            return;
        }
        guest.paint_buffer.clear();
        guest.paint_buffer.resize(phys_w * phys_h, 0);
        krate_adapter_common::painter::paint_placements(
            &mut guest.paint_buffer,
            phys_w as u32,
            phys_h as u32,
            guest.scale,
            &guest.placements,
            krate_adapter_common::painter::PaintInteraction {
                hovered: guest.hovered,
                pressed: guest.pressed_widget,
            },
        );

        let buffer = &mut guest.paint_buffer;
        let scale = guest.scale;
        DispatchQueue::main().exec_sync(|| {
            objc2::rc::autoreleasepool(|_| unsafe {
                let Some(main_side) = MAIN_SIDE.0.get() else {
                    return;
                };
                let color_space = CGColorSpaceCreateDeviceRGB();
                // 0xAARRGGBB u32s are BGRA in little-endian memory:
                // premultiplied-first + 32-little describes them exactly,
                // and alpha is always 0xFF.
                let bitmap_info = CGImageAlphaInfo::PremultipliedFirst.0
                    | CGImageByteOrderInfo::Order32Little.0;
                let context = CGBitmapContextCreate(
                    buffer.as_mut_ptr().cast(),
                    phys_w,
                    phys_h,
                    8,
                    phys_w * 4,
                    color_space.as_deref(),
                    bitmap_info,
                );
                let Some(context) = context else {
                    return;
                };
                let Some(image) = CGBitmapContextCreateImage(Some(&context)) else {
                    return;
                };
                let ui_image = UIImage::imageWithCGImage_scale_orientation(
                    &image,
                    scale as f64,
                    objc2_ui_kit::UIImageOrientation::Up,
                );
                main_side.view.setImage(Some(&ui_image));
            });
        });
        guest.dirty = false;
    }

    // ------------------------------------------------------------------
    // The adapter surface. Every function runs on the guest thread.

    pub fn create_native_window(
        krate: WindowId,
        _title: &str,
        size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        with_guest(|guest| {
            guest.krate = Some(krate);
            guest.placements.clear();
            guest.gesture = None;
            guest.pointer_samples.clear();
            guest.wheel_samples.clear();
            // The screen is the law: the next pump tells the guest its
            // real size, exactly as a resize would. The initial snapshot
            // repeats the requested size so the truth registers as a
            // change (the session diffs snapshots).
            guest
                .events
                .push((krate, WinitWindowNativeEvent::Resized(guest.logical)));
            let snapshot = WinitWindowSnapshot::new(krate, size, true, true, guest.scale)?;
            // 0 reads as a null handle to the shared validation; any fixed
            // nonzero token works -- never dereferenced.
            Ok((1, snapshot))
        })
    }

    pub fn set_drawn_placements(
        krate: WindowId,
        placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        if !guest_initialized() {
            return Ok(0);
        }
        with_guest(|guest| {
            if guest.krate != Some(krate) {
                return Ok(0);
            }
            guest.placements = placements
                .iter()
                .filter(|placement| krate_adapter_common::painter::drawn_kind(placement.kind))
                .cloned()
                .collect();
            let drawn = guest.placements.len();
            guest.dirty = true;
            blit(guest);
            Ok(drawn)
        })
    }

    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        if !guest_initialized() {
            return Ok(Vec::new());
        }
        with_guest(|guest| {
            digest_touches(guest);
            Ok(std::mem::take(&mut guest.events))
        })
    }

    /// Park the guest thread until input arrives or the deadline passes.
    /// The condvar is signaled by the main thread's touch callouts, so a
    /// finger wakes the guest immediately -- no run loop involved.
    pub fn park_for_events(max: std::time::Duration) -> bool {
        if !guest_initialized() {
            return false;
        }
        let shared = SHARED.lock().expect("shared state lock");
        if !shared.touches.is_empty() {
            return true;
        }
        let _ = INPUT_ARRIVED
            .wait_timeout(shared, max)
            .expect("shared state lock");
        true
    }

    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| Ok(std::mem::take(&mut guest.pointer_samples))).unwrap_or_default()
    }

    pub fn drain_key_samples() -> Vec<RawKeySample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| Ok(std::mem::take(&mut guest.key_samples))).unwrap_or_default()
    }

    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| {
            let mut samples = std::mem::take(&mut guest.wheel_samples);
            if let (Some((dx, dy, x, y)), Some(krate)) =
                (guest.pending_scroll.take(), guest.krate)
            {
                samples.push(RawWheelSample {
                    window: krate,
                    x,
                    y,
                    dx,
                    dy,
                    modifiers: Default::default(),
                });
            }
            Ok(samples)
        })
        .unwrap_or_default()
    }

    pub fn show_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !guest_initialized() {
            return Ok(false);
        }
        with_guest(|guest| Ok(guest.krate == Some(krate)))
    }

    pub fn set_native_window_title(krate: WindowId, _title: &str) -> Result<bool, UiAdapterError> {
        // iOS windows have no title bars; accepting the call is the honest
        // no-op.
        if !guest_initialized() {
            return Ok(false);
        }
        with_guest(|guest| Ok(guest.krate == Some(krate)))
    }

    pub fn close_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !guest_initialized() {
            return Ok(false);
        }
        with_guest(|guest| {
            if guest.krate == Some(krate) {
                guest.krate = None;
                guest.placements.clear();
                guest.dirty = true;
                blit(guest);
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    pub fn has_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !guest_initialized() {
            return Ok(false);
        }
        with_guest(|guest| Ok(guest.krate == Some(krate)))
    }

    pub fn redraw_all() -> Result<(), UiAdapterError> {
        if !guest_initialized() {
            return Ok(());
        }
        with_guest(|guest| {
            if guest.dirty {
                blit(guest);
            }
            Ok(())
        })
    }

    thread_local! {
        static GPU: RefCell<Option<crate::vello_canvas::GpuCanvas>> =
            const { RefCell::new(None) };
    }

    /// Render one recorded canvas frame on the GPU. True when claimed;
    /// false hands the frame back to the CPU path (init failed, wrong
    /// window, not a list this consumer knows).
    pub fn present_canvas_list(
        krate: WindowId,
        _widget: krate_adapter_common::ui::WidgetId,
        list: &krate_adapter_common::ui::CanvasListHandle,
    ) -> bool {
        if !guest_initialized() {
            return false;
        }
        let Some(list) = list.downcast_ref::<krate_adapter_common::canvas_list::CanvasList>()
        else {
            return false;
        };
        let owns = with_guest(|guest| Ok(guest.krate == Some(krate))).unwrap_or(false);
        if !owns {
            return false;
        }
        GPU.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let Some(((layer, native_scale), logical)) = ({
                    let shared = SHARED.lock().expect("shared state lock");
                    shared.metal_layer.zip(shared.screen.map(|(l, _)| l))
                }) else {
                    return false;
                };
                let width = (logical.width as f32 * native_scale) as u32;
                let height = (logical.height as f32 * native_scale) as u32;
                // wgpu panics on validation gaps (the simulator's Metal
                // lacks indirect execution, for one); a missing GPU must
                // mean CPU fallback, never a dead app.
                let created = std::panic::catch_unwind(|| {
                    crate::vello_canvas::GpuCanvas::new(
                        layer as *mut std::ffi::c_void,
                        width,
                        height,
                        native_scale,
                    )
                })
                .unwrap_or_else(|_| Err("gpu init panicked (unsupported device)".into()));
                match created {
                    Ok(gpu) => {
                        // The image view would cover the metal layer with
                        // its last CPU frame; hide it once the GPU owns
                        // the pixels.
                        DispatchQueue::main().exec_sync(|| {
                            if let Some(main_side) = MAIN_SIDE.0.get() {
                                main_side.view.setHidden(false);
                                unsafe {
                                    let v: &UIView = &main_side.view;
                                    v.setOpaque(false);
                                }
                                main_side.view.setImage(None);
                            }
                        });
                        *slot = Some(gpu);
                    }
                    Err(why) => {
                        eprintln!("krate-ios: gpu canvas unavailable: {why}");
                        return false;
                    }
                }
            }
            let gpu = slot.as_mut().expect("gpu initialized");
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                gpu.present(list)
            }))
            .unwrap_or_else(|_| Err("gpu present panicked".into()));
            match outcome {
                Ok(()) => true,
                Err(why) => {
                    eprintln!("krate-ios: gpu present failed: {why}");
                    false
                }
            }
        })
    }

    pub fn window_scale(krate: WindowId) -> f32 {
        if !guest_initialized() {
            return 1.0;
        }
        with_guest(|guest| {
            Ok(if guest.krate == Some(krate) {
                guest.scale
            } else {
                1.0
            })
        })
        .unwrap_or(1.0)
    }
}

#[cfg(not(target_os = "ios"))]
mod stub {
    use super::*;

    fn unsupported<T>() -> Result<T, UiAdapterError> {
        Err(UiAdapterError::Unsupported(
            "the UIKit backend only exists in iOS builds".to_string(),
        ))
    }

    pub fn create_native_window(
        _krate: WindowId,
        _title: &str,
        _size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        unsupported()
    }

    pub fn set_drawn_placements(
        _krate: WindowId,
        _placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        Ok(0)
    }

    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        Ok(Vec::new())
    }

    pub fn park_for_events(_max: std::time::Duration) -> bool {
        false
    }

    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        Vec::new()
    }

    pub fn drain_key_samples() -> Vec<RawKeySample> {
        Vec::new()
    }

    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        Vec::new()
    }

    pub fn show_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        Ok(false)
    }

    pub fn set_native_window_title(_krate: WindowId, _title: &str) -> Result<bool, UiAdapterError> {
        Ok(false)
    }

    pub fn close_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        Ok(false)
    }

    pub fn has_native_window(_krate: WindowId) -> Result<bool, UiAdapterError> {
        Ok(false)
    }

    pub fn redraw_all() -> Result<(), UiAdapterError> {
        Ok(())
    }

    pub fn window_scale(_krate: WindowId) -> f32 {
        1.0
    }

    pub fn present_canvas_list(
        _krate: WindowId,
        _widget: krate_adapter_common::ui::WidgetId,
        _list: &krate_adapter_common::ui::CanvasListHandle,
    ) -> bool {
        false
    }
}
