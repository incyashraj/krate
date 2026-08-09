//! Real UIKit windowing for the iOS adapter.
//!
//! Not winit: winit cannot pump on iOS, and this adapter's whole contract
//! is a non-blocking pump the guest drives from inside its own calls. So
//! this module is UIKit-direct in the macOS adapter's mold -- one
//! UIWindow with an image view the CPU painter blits into,
//! `NSRunLoop runMode:beforeDate:` as the per-frame pump, and UITouch
//! feeding the same tap-or-scroll synthesis the Android adapter proved:
//! within the slop a touch is a tap (pointer press + release), past it
//! every move is a wheel delta.
//!
//! iOS has exactly one screen-sized window, so `create_native_window`
//! creates it the first time and re-dresses it for every later guest
//! (the wall sheet, then the app). The player never returns to a blank
//! shell between them.

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

    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_core_foundation::{CGPoint, CGRect};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpaceCreateDeviceRGB,
        CGImageAlphaInfo, CGImageByteOrderInfo,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSObjectProtocol, NSRunLoop};
    use objc2_ui_kit::{
        UIImage, UIImageView, UIScreen, UITouch, UITouchPhase, UIView, UIViewController, UIWindow,
    };

    /// One raw touch, straight from the view's overrides, in logical
    /// points. The gesture logic runs at pump time, not in the callout.
    #[derive(Clone, Copy)]
    struct RawTouch {
        phase: UITouchPhase,
        x: f32,
        y: f32,
        finger: usize,
    }

    thread_local! {
        static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
        /// Touches land here from the view callouts; the pump drains them.
        static TOUCHES: RefCell<Vec<RawTouch>> = const { RefCell::new(Vec::new()) };
    }

    struct TouchGesture {
        finger: usize,
        start: (f32, f32),
        last: (f32, f32),
        scrolling: bool,
    }

    struct Host {
        window: Retained<UIWindow>,
        image_view: Retained<KrateSurfaceView>,
        krate: Option<WindowId>,
        logical: WindowSize,
        scale: f32,
        placements: Vec<WidgetPlacement>,
        events: CollectedNativeEvents,
        pointer_samples: Vec<RawPointerSample>,
        key_samples: Vec<RawKeySample>,
        wheel_samples: Vec<RawWheelSample>,
        gesture: Option<TouchGesture>,
        hovered: Option<krate_adapter_common::ui::WidgetId>,
        pressed_widget: Option<krate_adapter_common::ui::WidgetId>,
    }

    define_class!(
        // SAFETY:
        // - UIImageView has no extra subclassing requirements for touch
        //   overrides.
        // - UIKit delivers touches on the main thread, which is also the
        //   thread that owns every thread-local here.
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
        for touch in touches.allObjects() {
            let location: CGPoint = unsafe { touch.locationInView(Some(uiview)) };
            // The Retained pointer identifies the finger across its life.
            let finger = Retained::as_ptr(&touch) as usize;
            TOUCHES.with(|queue| {
                queue.borrow_mut().push(RawTouch {
                    phase: unsafe { touch.phase() },
                    x: location.x as f32,
                    y: location.y as f32,
                    finger,
                })
            });
        }
    }

    fn with_host<T>(
        f: impl FnOnce(&mut Host) -> Result<T, UiAdapterError>,
    ) -> Result<T, UiAdapterError> {
        HOST.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                *slot = Some(create_host()?);
            }
            f(slot.as_mut().expect("uikit host initialized"))
        })
    }

    fn host_initialized() -> bool {
        HOST.with(|slot| slot.borrow().is_some())
    }

    fn create_host() -> Result<Host, UiAdapterError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "the iOS adapter must run on the main thread".to_string(),
            )
        })?;
        let screen = UIScreen::mainScreen(mtm);
        let bounds: CGRect = screen.bounds();
        let scale = screen.scale() as f32;

        let window = unsafe { UIWindow::initWithFrame(UIWindow::alloc(mtm), bounds) };
        let controller = unsafe { UIViewController::new(mtm) };
        let view: Retained<KrateSurfaceView> = {
            let this = KrateSurfaceView::alloc(mtm).set_ivars(());
            // SAFETY: UIView's initWithFrame is the designated initializer.
            unsafe { msg_send![super(this), initWithFrame: bounds] }
        };
        unsafe {
            view.setUserInteractionEnabled(true);
            let as_uiview: &UIView = &view;
            controller.setView(Some(as_uiview));
            window.setRootViewController(Some(&controller));
            window.makeKeyAndVisible();
        }
        let image_view = view;

        let logical = WindowSize::new(
            (bounds.size.width as u32).max(1),
            (bounds.size.height as u32).max(1),
        )
        .map_err(|err| UiAdapterError::Unsupported(err.to_string()))?;

        Ok(Host {
            window,
            image_view,
            krate: None,
            logical,
            scale,
            placements: Vec::new(),
            events: Vec::new(),
            pointer_samples: Vec::new(),
            key_samples: Vec::new(),
            wheel_samples: Vec::new(),
            gesture: None,
            hovered: None,
            pressed_widget: None,
        })
    }

    /// Let UIKit deliver whatever is pending, without blocking: one spin of
    /// the main run loop up to now.
    fn spin_run_loop() {
        unsafe {
            let run_loop = NSRunLoop::mainRunLoop();
            let _ = run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &NSDate::now());
        }
    }

    /// The tap-or-scroll synthesis, identical in spirit to the Android
    /// adapter: within the slop a touch is a tap, past it a scroll.
    fn digest_touches(host: &mut Host) {
        const SLOP: f32 = 8.0;
        let Some(krate) = host.krate else {
            TOUCHES.with(|queue| queue.borrow_mut().clear());
            return;
        };
        let raw: Vec<RawTouch> = TOUCHES.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
        for touch in raw {
            match touch.phase {
                UITouchPhase::Began => {
                    if host.gesture.is_none() {
                        host.gesture = Some(TouchGesture {
                            finger: touch.finger,
                            start: (touch.x, touch.y),
                            last: (touch.x, touch.y),
                            scrolling: false,
                        });
                        host.hovered = krate_adapter_common::painter::topmost_interactive_at(
                            &host.placements,
                            touch.x,
                            touch.y,
                        );
                    }
                }
                UITouchPhase::Moved => {
                    let same = host
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        let gesture = host.gesture.as_mut().expect("gesture checked");
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
                            host.wheel_samples.push(RawWheelSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                dx: -dx,
                                dy: -dy,
                                modifiers: Default::default(),
                            });
                        }
                    }
                }
                UITouchPhase::Ended => {
                    let same = host
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        let gesture = host.gesture.take().expect("gesture checked");
                        if !gesture.scrolling {
                            host.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: true,
                            });
                            host.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: false,
                            });
                        }
                    }
                }
                UITouchPhase::Cancelled => {
                    let same = host
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.finger == touch.finger);
                    if same {
                        host.gesture = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Rasterize the placements and put them on screen. Everything inside
    /// one autoreleasepool: the macOS adapter's 46 GB lesson, honored here
    /// from day one.
    fn blit(host: &mut Host) {
        let phys_w = (host.logical.width as f32 * host.scale) as usize;
        let phys_h = (host.logical.height as f32 * host.scale) as usize;
        if phys_w == 0 || phys_h == 0 {
            return;
        }
        let mut buffer = vec![0u32; phys_w * phys_h];
        krate_adapter_common::painter::paint_placements(
            &mut buffer,
            phys_w as u32,
            phys_h as u32,
            host.scale,
            &host.placements,
            krate_adapter_common::painter::PaintInteraction {
                hovered: host.hovered,
                pressed: host.pressed_widget,
            },
        );

        objc2::rc::autoreleasepool(|_| unsafe {
            let color_space = CGColorSpaceCreateDeviceRGB();
            // The painter writes 0xAARRGGBB u32s; on this little-endian
            // machine that is BGRA in memory, which premultiplied-first +
            // 32-bit-little byte order describes exactly. Alpha is always
            // 0xFF, so "premultiplied" is trivially true.
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
                host.scale as f64,
                objc2_ui_kit::UIImageOrientation::Up,
            );
            host.image_view.setImage(Some(&ui_image));
        });
    }

    // ------------------------------------------------------------------
    // The adapter surface, mirroring the Android module's contract.

    pub fn create_native_window(
        krate: WindowId,
        _title: &str,
        size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        with_host(|host| {
            // One screen, one window: a new guest re-dresses it.
            host.krate = Some(krate);
            host.placements.clear();
            host.gesture = None;
            host.pointer_samples.clear();
            host.wheel_samples.clear();
            // The screen is the law: whatever size the guest asked for, the
            // next pump tells it the truth, the same way a resize would.
            // Without this the window record keeps the requested size and
            // the canvas letterboxes (K-087's lesson, host-side).
            host.events
                .push((krate, WinitWindowNativeEvent::Resized(host.logical)));
            // The initial snapshot deliberately repeats the size the guest
            // asked for: the session diffs snapshots to decide what to
            // queue, so the Resized truth above only lands if this baseline
            // differs from it.
            let snapshot = WinitWindowSnapshot::new(krate, size, true, true, host.scale)?;
            // 0 reads as a null handle to the shared validation; any fixed
            // nonzero token works -- this adapter never dereferences it.
            Ok((1, snapshot))
        })
    }

    pub fn set_drawn_placements(
        krate: WindowId,
        placements: &[WidgetPlacement],
    ) -> Result<usize, UiAdapterError> {
        if !host_initialized() {
            return Ok(0);
        }
        with_host(|host| {
            if host.krate != Some(krate) {
                return Ok(0);
            }
            host.placements = placements
                .iter()
                .filter(|placement| krate_adapter_common::painter::drawn_kind(placement.kind))
                .cloned()
                .collect();
            let drawn = host.placements.len();
            blit(host);
            Ok(drawn)
        })
    }

    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        if !host_initialized() {
            return Ok(Vec::new());
        }
        spin_run_loop();
        with_host(|host| {
            digest_touches(host);
            Ok(std::mem::take(&mut host.events))
        })
    }

    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        if !host_initialized() {
            return Vec::new();
        }
        with_host(|host| Ok(std::mem::take(&mut host.pointer_samples))).unwrap_or_default()
    }

    pub fn drain_key_samples() -> Vec<RawKeySample> {
        if !host_initialized() {
            return Vec::new();
        }
        with_host(|host| Ok(std::mem::take(&mut host.key_samples))).unwrap_or_default()
    }

    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        if !host_initialized() {
            return Vec::new();
        }
        with_host(|host| Ok(std::mem::take(&mut host.wheel_samples))).unwrap_or_default()
    }

    pub fn show_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !host_initialized() {
            return Ok(false);
        }
        with_host(|host| Ok(host.krate == Some(krate)))
    }

    pub fn set_native_window_title(krate: WindowId, _title: &str) -> Result<bool, UiAdapterError> {
        // iOS windows have no title bars; accepting the call is the honest
        // no-op, matching how the platform treats every app.
        if !host_initialized() {
            return Ok(false);
        }
        with_host(|host| Ok(host.krate == Some(krate)))
    }

    pub fn close_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !host_initialized() {
            return Ok(false);
        }
        with_host(|host| {
            if host.krate == Some(krate) {
                host.krate = None;
                host.placements.clear();
                blit(host);
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    pub fn has_native_window(krate: WindowId) -> Result<bool, UiAdapterError> {
        if !host_initialized() {
            return Ok(false);
        }
        with_host(|host| Ok(host.krate == Some(krate)))
    }

    pub fn redraw_all() -> Result<(), UiAdapterError> {
        if !host_initialized() {
            return Ok(());
        }
        with_host(|host| {
            blit(host);
            Ok(())
        })
    }

    pub fn window_scale(krate: WindowId) -> f32 {
        if !host_initialized() {
            return 1.0;
        }
        with_host(|host| {
            Ok(if host.krate == Some(krate) {
                host.scale
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
}
