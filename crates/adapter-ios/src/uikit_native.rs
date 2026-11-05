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
    use objc2_ui_kit::UIDocumentPickerDelegate;
    use objc2_ui_kit::{
        UIApplication, UIImage, UIImageView, UIScreen, UITouch, UITouchPhase, UIView,
        UIViewController, UIWindow, UIWindowScene,
    };
    use objc2_ui_kit::{UIKeyInput, UITextInputTraits};

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
    /// What the on-screen keyboard typed, in order.
    enum Typed {
        Text(String),
        Backspace,
        Enter,
    }

    struct Shared {
        touches: Vec<RawTouch>,
        /// Typing from the on-screen keyboard, waiting for the guest.
        typed: Vec<Typed>,
        /// The close capsule was tapped: the app is asked to close.
        close_tapped: bool,
        /// Set once the main thread has built the window.
        screen: Option<(WindowSize, f32)>,
        /// The CAMetalLayer's raw pointer plus the FULL native scale --
        /// the GPU renders at true density, so the CPU path's 2x quality
        /// cap dies here.
        metal_layer: Option<(usize, f32)>,
        /// The panel's own scale (3 on a current iPhone): whole-frame
        /// pictures are drawn at it, one pass, with no second stretch by the
        /// compositor.
        panel_scale: f32,
    }

    static SHARED: Mutex<Shared> = Mutex::new(Shared {
        touches: Vec::new(),
        typed: Vec::new(),
        close_tapped: false,
        screen: None,
        metal_layer: None,
        panel_scale: 2.0,
    });

    /// Signaled by the touch callouts AND by the display link, so a parked
    /// guest wakes on input or on vsync, whichever comes first.
    static INPUT_ARRIVED: Condvar = Condvar::new();

    /// Set by the display link each time the panel is about to refresh, and
    /// cleared by the guest when it wakes. This is what replaces sleeping a
    /// fixed 16 ms and hoping it lines up with the display: on a 120 Hz
    /// iPhone the panel decides when a frame is due, so frames land on the
    /// refresh instead of drifting against it (K-090's last structural
    /// cause).
    static VSYNC_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// TEMP on-device diagnostics for K-090: a timestamped log in the app
    /// container, pulled over USB. Nothing else escapes a real iPhone.
    pub fn diag(line: &str) {
        use std::io::Write;
        // Opt-in: only with Documents/krate-diag.on present. Writing a line
        // per frame, forever, cost every frame a file write and grew the
        // log without end.
        static ON: OnceLock<bool> = OnceLock::new();
        let on = *ON.get_or_init(|| {
            std::env::var("HOME")
                .map(|h| {
                    std::path::Path::new(&h)
                        .join("Documents/krate-diag.on")
                        .exists()
                })
                .unwrap_or(false)
        });
        if !on {
            return;
        }
        static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
        static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        let epoch = EPOCH.get_or_init(std::time::Instant::now);
        let file = FILE.get_or_init(|| {
            let path = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Documents/krate-diag.log"))
                .unwrap_or_else(|_| "/tmp/krate-diag.log".into());
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .expect("diag log"),
            )
        });
        let ms = epoch.elapsed().as_secs_f64() * 1000.0;
        let mut file = file.lock().expect("diag lock");
        let _ = writeln!(file, "{ms:.1} {line}");
        let _ = file.flush();
    }

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
        /// Every finger, for apps that read them all (krate:ui/touch).
        fingers: krate_adapter_common::ui::FingerTracker,
        /// Where the pointer is and whether it is held, for apps that ask
        /// (krate:ui/pointer): fed from the presses as they leave for the
        /// runtime. Never fed, a game steering by `button-held` could not
        /// be steered by finger.
        pointer: krate_adapter_common::ui::PointerTracker,
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
        /// When the finger landed, and whether its press has gone out yet.
        began: std::time::Instant,
        pressed: bool,
        /// It began on something that drags -- a slider, or what the app
        /// described as adjustable (a scrubber, a volume) -- so moving it
        /// moves that, never scrolls (K-1059).
        drags: bool,
    }

    /// The canvases a finger presses rather than scrolls, by Krate window
    /// and widget: games drawn at a design size (K-1202).
    static PRESSING: Mutex<
        std::collections::BTreeSet<(WindowId, krate_adapter_common::ui::WidgetId)>,
    > = Mutex::new(std::collections::BTreeSet::new());

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
    /// to the screen reader as adjustable. A canvas that lays itself out
    /// for the screen is a page, and scrolls.
    fn drags_at(
        window: WindowId,
        placements: &[krate_adapter_common::ui::WidgetPlacement],
        x: f32,
        y: f32,
    ) -> bool {
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
        }) || ACCESS_DESCRIBED
            .lock()
            .expect("access lock")
            .iter()
            .any(|e| {
                e.role == krate_adapter_common::ui::AccessRole::Adjustable
                    && inside(e.x, e.y - 8.0, e.width, e.height + 16.0)
            })
    }

    /// How long a finger rests before its press goes out, unless it starts
    /// to scroll first -- iOS's own rule for touches inside scrolling
    /// content. A tap used to press only when the finger lifted, a beat
    /// late on every button; a drag that scrolls still presses nothing.
    const PRESS_AFTER: std::time::Duration = std::time::Duration::from_millis(50);

    thread_local! {
        static GUEST: RefCell<Option<GuestSide>> = const { RefCell::new(None) };
    }

    /// Main-thread-only UIKit objects, created on first use via dispatch.
    struct MainSide {
        window: Retained<UIWindow>,
        /// Krate's close capsule, over every app (see `set_app_chrome`).
        close: Retained<objc2_ui_kit::UIButton>,
        /// Krate's loading cover (see `show_loading`).
        loading: Loading,
        _close_target: Retained<KrateCloseTarget>,
        controller: Retained<KrateViewController>,
        metal: Retained<objc2_quartz_core::CAMetalLayer>,
        /// Held so the link's target outlives the link that calls it.
        _link_target: Retained<KrateDisplayLinkTarget>,
        _display_link: Retained<objc2_quartz_core::CADisplayLink>,
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
        // - NSObject has no subclassing requirements for a target-action
        //   receiver.
        // - CADisplayLink calls this on the main thread, which is where it
        //   was added to the run loop.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ()]
        struct KrateDisplayLinkTarget;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateDisplayLinkTarget {}

        impl KrateDisplayLinkTarget {
            // SAFETY: matches CADisplayLink's target-action signature.
            #[unsafe(method(krateDisplayLinkFired:))]
            fn fired(&self, _link: &objc2_quartz_core::CADisplayLink) {
                VSYNC_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
                diag("vsync");
                // Waking the guest costs nothing when it is already awake,
                // and is the whole point when it is parked.
                INPUT_ARRIVED.notify_all();
            }
        }
    );

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

        // The view takes the on-screen keyboard's typing when an app asks
        // for text entry (krate:ui/text-entry). No autocorrection: a name in
        // a game, a command in an emulator, is typed as meant.
        unsafe impl UITextInputTraits for KrateSurfaceView {
            #[unsafe(method(autocorrectionType))]
            fn autocorrection_type(&self) -> objc2_ui_kit::UITextAutocorrectionType {
                objc2_ui_kit::UITextAutocorrectionType::No
            }

            #[unsafe(method(autocapitalizationType))]
            fn autocapitalization_type(&self) -> objc2_ui_kit::UITextAutocapitalizationType {
                objc2_ui_kit::UITextAutocapitalizationType::None
            }
        }

        unsafe impl UIKeyInput for KrateSurfaceView {
            #[unsafe(method(hasText))]
            fn has_text(&self) -> bool {
                true
            }

            #[unsafe(method(insertText:))]
            fn insert_text(&self, text: &objc2_foundation::NSString) {
                let text = text.to_string();
                let mut shared = SHARED.lock().expect("shared state lock");
                for part in text.split_inclusive('\n') {
                    match part.strip_suffix('\n') {
                        Some(before) => {
                            if !before.is_empty() {
                                shared.typed.push(Typed::Text(before.to_string()));
                            }
                            shared.typed.push(Typed::Enter);
                        }
                        None => shared.typed.push(Typed::Text(part.to_string())),
                    }
                }
                drop(shared);
                INPUT_ARRIVED.notify_all();
            }

            #[unsafe(method(deleteBackward))]
            fn delete_backward(&self) {
                SHARED.lock().expect("shared state lock").typed.push(Typed::Backspace);
                INPUT_ARRIVED.notify_all();
            }
        }

        impl KrateSurfaceView {
            // VoiceOver reads the elements the app described (IC-926), not
            // the view as one picture.
            #[unsafe(method(isAccessibilityElement))]
            fn is_accessibility_element(&self) -> bool {
                false
            }

            #[unsafe(method_id(accessibilityElements))]
            fn accessibility_elements(&self) -> Option<Retained<objc2::runtime::AnyObject>> {
                access_elements_for(self)
            }

            // Only a first responder gets the keyboard's typing.
            #[unsafe(method(canBecomeFirstResponder))]
            fn can_become_first_responder(&self) -> bool {
                true
            }

            // SAFETY: signature matches UIView's.
            #[unsafe(method(layoutSubviews))]
            fn layout_subviews(&self) {
                let _: () = unsafe { msg_send![super(self), layoutSubviews] };
                view_laid_out(self);
            }

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

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a target-action
        //   receiver.
        // - UIKit calls it on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ()]
        struct KrateCloseTarget;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateCloseTarget {}

        impl KrateCloseTarget {
            #[unsafe(method(krateClose:))]
            fn krate_close(&self, _sender: &objc2_foundation::NSObject) {
                request_close();
            }
        }
    );

    /// Ask the running app to close, from Krate's own controls: the cover at
    /// once, so the tap answers before the app has finished closing and
    /// Krate's home is drawn. Main thread.
    fn request_close() {
        LOADING.store(true, std::sync::atomic::Ordering::Relaxed);
        *LOADING_SINCE.lock().expect("loading lock") = Some(std::time::Instant::now());
        if let Some(main_side) = MAIN_SIDE.0.get() {
            main_side.close.setHidden(true);
            cover_shown(main_side, "Krate", "");
        }
        SHARED.lock().expect("shared state lock").close_tapped = true;
        INPUT_ARRIVED.notify_all();
        end_if_unanswered();
    }

    /// Which app is running, counted: a watchdog started for one app must
    /// not end the next.
    static APP_RUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Krate's Close or Restart is the person's, not a request the app may
    /// sit on: an app still running two seconds later -- stuck in a loop,
    /// never calling the host -- is ended by the engine at its next
    /// safepoint (IC-934).
    fn end_if_unanswered() {
        if !APP_CHROME.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let run = APP_RUN.load(std::sync::atomic::Ordering::Relaxed);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if APP_CHROME.load(std::sync::atomic::Ordering::Relaxed)
                && APP_RUN.load(std::sync::atomic::Ordering::Relaxed) == run
            {
                eprintln!("krate-menu: the app did not answer Close; ending it");
                krate_adapter_common::ui::end_running_app();
                INPUT_ARRIVED.notify_all();
            }
        });
    }

    // ------------------------------------------------------------------
    // A web page inside an app (IC-927): WKWebView over Krate's view.

    #[link(name = "WebKit", kind = "framework")]
    unsafe extern "C" {}

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a navigation
        //   delegate.
        // - WebKit calls it on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = std::cell::Cell<u64>]
        struct KrateWebDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateWebDelegate {}

        // WKNavigationDelegate, by selector.
        impl KrateWebDelegate {
            /// Every navigation -- the first address, a link, a redirect, a
            /// frame -- goes only to a granted https host.
            #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
            fn decide(
                &self,
                _web: &objc2::runtime::AnyObject,
                action: &objc2::runtime::AnyObject,
                decide: &block2::DynBlock<dyn Fn(isize)>,
            ) {
                let page = self.ivars().get();
                // SAFETY: the action's request and its URL's string.
                let address: String = unsafe {
                    let request: Option<Retained<objc2::runtime::AnyObject>> = msg_send![action, request];
                    let url: Option<Retained<objc2::runtime::AnyObject>> =
                        request.and_then(|r| msg_send![&*r, URL]);
                    url.and_then(|u| {
                        let text: Option<Retained<objc2_foundation::NSString>> = msg_send![&*u, absoluteString];
                        text.map(|t| t.to_string())
                    })
                    .unwrap_or_default()
                };
                let verdict = WEB_PAGES.with(|pages| {
                    pages.borrow().get(&page).map(|p| {
                        // about:blank and the page's own fragments are not
                        // somewhere new.
                        if address == "about:blank" {
                            Ok(())
                        } else {
                            krate_adapter_common::net::web_page_allowed(&address, &p.granted)
                        }
                    })
                });
                match verdict {
                    Some(Ok(())) => decide.call((1,)),
                    Some(Err(why)) => {
                        WEB_PAGES.with(|pages| {
                            if let Some(p) = pages.borrow_mut().get_mut(&page) {
                                p.problem = why;
                            }
                        });
                        decide.call((0,));
                    }
                    None => decide.call((0,)),
                }
            }

            #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
            fn failed_early(
                &self,
                _web: &objc2::runtime::AnyObject,
                _navigation: &objc2::runtime::AnyObject,
                error: &objc2::runtime::AnyObject,
            ) {
                web_page_failed(self.ivars().get(), error);
            }

            #[unsafe(method(webView:didFailNavigation:withError:))]
            fn failed(
                &self,
                _web: &objc2::runtime::AnyObject,
                _navigation: &objc2::runtime::AnyObject,
                error: &objc2::runtime::AnyObject,
            ) {
                web_page_failed(self.ivars().get(), error);
            }

            #[unsafe(method(webView:didCommitNavigation:))]
            fn committed(&self, _web: &objc2::runtime::AnyObject, _navigation: &objc2::runtime::AnyObject) {
                WEB_PAGES.with(|pages| {
                    if let Some(p) = pages.borrow_mut().get_mut(&self.ivars().get()) {
                        p.problem.clear();
                    }
                });
            }
        }
    );

    /// One web page on screen: its view, its delegate, where it may go, and
    /// what went wrong last.
    struct WebPage {
        view: Retained<objc2::runtime::AnyObject>,
        _delegate: Retained<KrateWebDelegate>,
        granted: Vec<(String, String)>,
        problem: String,
    }

    thread_local! {
        /// The web pages on screen, by handle. Main thread.
        static WEB_PAGES: RefCell<std::collections::BTreeMap<u64, WebPage>> =
            const { RefCell::new(std::collections::BTreeMap::new()) };
    }

    static NEXT_WEB_PAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    /// A cancelled load (the delegate refused it) is not a failure to say.
    fn web_page_failed(page: u64, error: &objc2::runtime::AnyObject) {
        // SAFETY: an NSError's code and localizedDescription.
        let (code, why): (isize, String) = unsafe {
            let code: isize = msg_send![error, code];
            let why: Retained<objc2_foundation::NSString> = msg_send![error, localizedDescription];
            (code, why.to_string())
        };
        // NSURLErrorCancelled, and WebKit's "frame load interrupted".
        if code == -999 || code == 102 {
            return;
        }
        WEB_PAGES.with(|pages| {
            if let Some(p) = pages.borrow_mut().get_mut(&page) {
                p.problem = why;
            }
        });
    }

    fn rect_of(frame: (f32, f32, f32, f32)) -> CGRect {
        CGRect::new(
            CGPoint::new(frame.0 as f64, frame.1 as f64),
            objc2_core_foundation::CGSize::new(frame.2 as f64, frame.3 as f64),
        )
    }

    fn load(view: &objc2::runtime::AnyObject, address: &str) {
        // SAFETY: an NSURLRequest for the address, loaded by the web view.
        unsafe {
            let (Some(url_class), Some(request_class)) = (
                objc2::runtime::AnyClass::get(c"NSURL"),
                objc2::runtime::AnyClass::get(c"NSURLRequest"),
            ) else {
                return;
            };
            let url: Option<Retained<objc2::runtime::AnyObject>> = msg_send![url_class, URLWithString: &*objc2_foundation::NSString::from_str(address)];
            let Some(url) = url else {
                return;
            };
            let request: Retained<objc2::runtime::AnyObject> =
                msg_send![request_class, requestWithURL: &*url];
            let _: Option<Retained<objc2::runtime::AnyObject>> =
                msg_send![view, loadRequest: &*request];
        }
    }

    /// Show a web page at `frame` over Krate's view: no stored data between
    /// openings, nothing shared with the app, only the granted hosts.
    pub fn open_web_page(
        address: &str,
        frame: (f32, f32, f32, f32),
        granted: Vec<(String, String)>,
    ) -> Result<u64, String> {
        let page = NEXT_WEB_PAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let address = address.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let made = (|| -> Result<(), String> {
                let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
                let main_side = MAIN_SIDE.0.get().ok_or("no screen")?;
                let (Some(config_class), Some(store_class), Some(web_class)) = (
                    objc2::runtime::AnyClass::get(c"WKWebViewConfiguration"),
                    objc2::runtime::AnyClass::get(c"WKWebsiteDataStore"),
                    objc2::runtime::AnyClass::get(c"WKWebView"),
                ) else {
                    return Err("this iOS has no web view".to_string());
                };
                // SAFETY: a WKWebView with a non-persistent data store and
                // Krate's delegate, added to Krate's view, on the main thread.
                unsafe {
                    let config: Retained<objc2::runtime::AnyObject> = msg_send![config_class, new];
                    let store: Retained<objc2::runtime::AnyObject> =
                        msg_send![store_class, nonPersistentDataStore];
                    let _: () = msg_send![&*config, setWebsiteDataStore: &*store];
                    let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                        msg_send![web_class, alloc];
                    let view: Retained<objc2::runtime::AnyObject> =
                        msg_send![made, initWithFrame: rect_of(frame), configuration: &*config];
                    let delegate =
                        KrateWebDelegate::alloc(mtm).set_ivars(std::cell::Cell::new(page));
                    let delegate: Retained<KrateWebDelegate> = msg_send![super(delegate), init];
                    if let Some(protocol) =
                        objc2::runtime::AnyProtocol::get(c"WKNavigationDelegate")
                    {
                        objc2::ffi::class_addProtocol(
                            <KrateWebDelegate as objc2::ClassType>::class()
                                as *const objc2::runtime::AnyClass
                                as *mut objc2::runtime::AnyClass,
                            protocol,
                        );
                    }
                    let _: () = msg_send![&*view, setNavigationDelegate: &*delegate];
                    let as_view: &UIView = &*(Retained::as_ptr(&view) as *const UIView);
                    as_view.layer().setCornerRadius(12.0);
                    as_view.setClipsToBounds(true);
                    let host: &UIView = &main_side.view;
                    host.addSubview(as_view);
                    load(&view, &address);
                    WEB_PAGES.with(|pages| {
                        pages.borrow_mut().insert(
                            page,
                            WebPage {
                                view,
                                _delegate: delegate,
                                granted,
                                problem: String::new(),
                            },
                        )
                    });
                }
                Ok(())
            })();
            let _ = tx.send(made);
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "the web view did not answer".to_string())??;
        Ok(page)
    }

    /// Run `f` on the page's web view, on the main thread, without waiting.
    fn with_web_page(page: u64, f: impl FnOnce(&WebPage) + Send + 'static) {
        DispatchQueue::main().exec_async(move || {
            WEB_PAGES.with(|pages| {
                if let Some(p) = pages.borrow().get(&page) {
                    f(p);
                }
            });
        });
    }

    pub fn place_web_page(page: u64, frame: (f32, f32, f32, f32)) {
        with_web_page(page, move |p| {
            // SAFETY: a WKWebView's frame, on the main thread.
            unsafe {
                let _: () = msg_send![&*p.view, setFrame: rect_of(frame)];
            }
        });
    }

    pub fn web_page_go(page: u64, address: &str) {
        let address = address.to_string();
        with_web_page(page, move |p| load(&p.view, &address));
    }

    pub fn web_page_back(page: u64) {
        with_web_page(page, |p| {
            // SAFETY: WKWebView's goBack, on the main thread.
            unsafe {
                let _: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*p.view, goBack];
            }
        });
    }

    pub fn web_page_state(page: u64) -> Option<krate_adapter_common::ui::WebPageState> {
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let state = WEB_PAGES.with(|pages| {
                pages.borrow().get(&page).map(|p| {
                    // SAFETY: WKWebView's URL, title and isLoading, on the
                    // main thread.
                    unsafe {
                        let url: Option<Retained<objc2::runtime::AnyObject>> =
                            msg_send![&*p.view, URL];
                        let address = url
                            .and_then(|u| {
                                let t: Option<Retained<objc2_foundation::NSString>> =
                                    msg_send![&*u, absoluteString];
                                t.map(|t| t.to_string())
                            })
                            .unwrap_or_default();
                        let title: Option<Retained<objc2_foundation::NSString>> =
                            msg_send![&*p.view, title];
                        let loading: bool = msg_send![&*p.view, isLoading];
                        krate_adapter_common::ui::WebPageState {
                            address,
                            title: title.map(|t| t.to_string()).unwrap_or_default(),
                            loading,
                            problem: p.problem.clone(),
                        }
                    }
                })
            });
            let _ = tx.send(state);
        });
        rx.recv_timeout(std::time::Duration::from_millis(500))
            .ok()
            .flatten()
    }

    pub fn close_web_page(page: u64) {
        DispatchQueue::main().exec_async(move || {
            if let Some(p) = WEB_PAGES.with(|pages| pages.borrow_mut().remove(&page)) {
                // SAFETY: removing a WKWebView from its superview, main thread.
                unsafe {
                    let _: () = msg_send![&*p.view, removeFromSuperview];
                }
            }
        });
    }

    /// The app is gone: every page it opened goes with it. Main thread.
    fn close_all_web_pages() {
        let pages = WEB_PAGES.with(|pages| std::mem::take(&mut *pages.borrow_mut()));
        for (_, p) in pages {
            // SAFETY: removing a WKWebView from its superview, main thread.
            unsafe {
                let _: () = msg_send![&*p.view, removeFromSuperview];
            }
        }
    }

    // ------------------------------------------------------------------
    // The screen reader (IC-926).

    /// What the running app told VoiceOver it shows, and what Krate drew
    /// for it as widgets; the app's own description wins when it has one.
    static ACCESS_DESCRIBED: Mutex<Vec<krate_adapter_common::ui::AccessElement>> =
        Mutex::new(Vec::new());
    static ACCESS_WIDGETS: Mutex<Vec<krate_adapter_common::ui::AccessElement>> =
        Mutex::new(Vec::new());
    /// Bumped whenever either changes, so the built elements are rebuilt.
    static ACCESS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    thread_local! {
        /// The UIAccessibilityElements last built, and for which generation.
        static ACCESS_BUILT: RefCell<Option<(u64, Retained<objc2::runtime::AnyObject>)>> = const { RefCell::new(None) };
    }

    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        fn UIAccessibilityIsVoiceOverRunning() -> bool;
        fn UIAccessibilityPostNotification(
            notification: u32,
            argument: *mut objc2::runtime::AnyObject,
        );
    }

    /// Whether VoiceOver is on.
    pub fn voice_over_on() -> bool {
        // SAFETY: a C query UIKit allows from any thread.
        unsafe { UIAccessibilityIsVoiceOverRunning() }
    }

    /// Tell VoiceOver the screen changed, so it reads the new elements.
    fn access_changed(widgets: bool, elements: Vec<krate_adapter_common::ui::AccessElement>) {
        let slot = if widgets {
            &ACCESS_WIDGETS
        } else {
            &ACCESS_DESCRIBED
        };
        {
            let mut held = slot.lock().expect("access lock");
            if *held == elements {
                return;
            }
            *held = elements;
        }
        ACCESS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        DispatchQueue::main().exec_async(|| {
            // SAFETY: UIAccessibilityLayoutChangedNotification (1001), on the
            // main thread, with no element to move to.
            unsafe { UIAccessibilityPostNotification(1001, std::ptr::null_mut()) };
        });
    }

    /// The app described its screen (IC-926).
    pub fn describe_accessibility(elements: &[krate_adapter_common::ui::AccessElement]) {
        access_changed(false, elements.to_vec());
    }

    /// The app is gone: nothing of it stays for VoiceOver.
    fn access_clear() {
        access_changed(false, Vec::new());
        access_changed(true, Vec::new());
    }

    /// VoiceOver's elements for Krate's view, built on the main thread.
    fn access_elements_for(view: &KrateSurfaceView) -> Option<Retained<objc2::runtime::AnyObject>> {
        use krate_adapter_common::ui::AccessRole;
        let generation = ACCESS_GENERATION.load(std::sync::atomic::Ordering::Relaxed);
        if let Some(built) = ACCESS_BUILT.with(|b| {
            b.borrow()
                .as_ref()
                .filter(|(g, _)| *g == generation)
                .map(|(_, array)| array.clone())
        }) {
            return Some(built);
        }
        let described = ACCESS_DESCRIBED.lock().expect("access lock").clone();
        let elements = if described.is_empty() {
            ACCESS_WIDGETS.lock().expect("access lock").clone()
        } else {
            described
        };
        let class = objc2::runtime::AnyClass::get(c"UIAccessibilityElement")?;
        let mut built: Vec<Retained<objc2::runtime::AnyObject>> =
            Vec::with_capacity(elements.len());
        // SAFETY: UIAccessibilityElements made for Krate's view as their
        // container, with documented properties, on the main thread.
        unsafe {
            for e in &elements {
                let made: objc2::rc::Allocated<objc2::runtime::AnyObject> = msg_send![class, alloc];
                let element: Retained<objc2::runtime::AnyObject> =
                    msg_send![made, initWithAccessibilityContainer: view];
                let frame = CGRect::new(
                    CGPoint::new(e.x as f64, e.y as f64),
                    objc2_core_foundation::CGSize::new(e.width as f64, e.height as f64),
                );
                let _: () = msg_send![&*element, setAccessibilityFrameInContainerSpace: frame];
                let _: () = msg_send![&*element, setIsAccessibilityElement: true];
                let _: () = msg_send![
                    &*element,
                    setAccessibilityLabel: &*objc2_foundation::NSString::from_str(&e.label)
                ];
                let value = match e.checked {
                    Some(true) => "On".to_string(),
                    Some(false) => "Off".to_string(),
                    None => e.value.clone(),
                };
                if !value.is_empty() {
                    let _: () = msg_send![
                        &*element,
                        setAccessibilityValue: &*objc2_foundation::NSString::from_str(&value)
                    ];
                }
                // UIAccessibilityTraits: button 1, link 2, image 4, static
                // text 64, not enabled 256, adjustable 4096, header 65536.
                let mut traits: u64 = match e.role {
                    AccessRole::Button | AccessRole::Toggle => 1,
                    AccessRole::Link => 2,
                    AccessRole::Image => 4,
                    AccessRole::Text => 64,
                    AccessRole::Adjustable => 4096,
                    AccessRole::Heading => 65536,
                    AccessRole::Field => 0,
                };
                if !e.enabled {
                    traits |= 256;
                }
                let _: () = msg_send![&*element, setAccessibilityTraits: traits];
                built.push(element);
            }
        }
        let array: Retained<objc2::runtime::AnyObject> = unsafe {
            Retained::cast_unchecked(objc2_foundation::NSArray::from_retained_slice(&built))
        };
        ACCESS_BUILT.with(|b| *b.borrow_mut() = Some((generation, array.clone())));
        Some(array)
    }

    // ------------------------------------------------------------------
    // Krate's menu (IC-934): touch and hold the strip under the clock.
    //
    // No button over an app: an X made every app look like a mini app in
    // Krate. The strip is the band Krate keeps apps out of; a short tap
    // there still reaches the app, a pull from the very top edge is the
    // phone's own (Notification Centre), and a half-second hold is nothing
    // else's. The menu is Krate's, drawn over everything -- the opening
    // cover too -- so it works whatever the app is doing.

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a target-action
        //   receiver and gesture delegate.
        // - UIKit calls it on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ()]
        struct KrateMenuTarget;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateMenuTarget {}

        impl KrateMenuTarget {
            #[unsafe(method(krateHold:))]
            fn hold(&self, recognizer: &objc2::runtime::AnyObject) {
                // SAFETY: UIGestureRecognizer's state; 1 is Began.
                let state: isize = unsafe { msg_send![recognizer, state] };
                if state == 1 {
                    eprintln!("krate-menu: held");
                    show_menu(self.mtm(), self);
                }
            }

            /// UIGestureRecognizerDelegate: only a touch that starts in the
            /// strip under the clock, only while an app runs.
            #[unsafe(method(gestureRecognizer:shouldReceiveTouch:))]
            fn should_receive(
                &self,
                _recognizer: &objc2::runtime::AnyObject,
                touch: &objc2::runtime::AnyObject,
            ) -> objc2::runtime::Bool {
                let Some(main_side) = MAIN_SIDE.0.get() else {
                    return objc2::runtime::Bool::NO;
                };
                let host: &UIView = &main_side.view;
                // SAFETY: the touch's location in Krate's own view.
                let at: CGPoint = unsafe { msg_send![touch, locationInView: host] };
                if (at.y as f32) >= hold_band() {
                    return objc2::runtime::Bool::NO;
                }
                let chrome = APP_CHROME.load(std::sync::atomic::Ordering::Relaxed);
                let open = MENU.with(|m| m.borrow().is_some());
                if !chrome || open {
                    eprintln!("krate-menu: touch in the strip refused (app running: {chrome}, menu open: {open})");
                    return objc2::runtime::Bool::NO;
                }
                objc2::runtime::Bool::YES
            }

            /// Let the hold be seen beside the app's own touch handling.
            #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
            fn together(
                &self,
                _a: &objc2::runtime::AnyObject,
                _b: &objc2::runtime::AnyObject,
            ) -> objc2::runtime::Bool {
                objc2::runtime::Bool::YES
            }

            #[unsafe(method(krateMenuDismiss:))]
            fn dismiss(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
            }

            #[unsafe(method(krateMenuClose:))]
            fn menu_close(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
                request_close();
            }

            #[unsafe(method(krateMenuRestart:))]
            fn menu_restart(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
                restart_app();
            }

            #[unsafe(method(krateMenuShare:))]
            fn menu_share(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
                share_app();
            }

            #[unsafe(method(krateMenuUses:))]
            fn menu_uses(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
                show_uses();
            }

            #[unsafe(method(krateMenuReport:))]
            fn menu_report(&self, _sender: &objc2::runtime::AnyObject) {
                hide_menu();
                report_problem();
            }
        }
    );

    thread_local! {
        /// The menu that is up: its shade and panel. Main thread.
        static MENU: RefCell<Option<(Retained<UIView>, Retained<UIView>)>> = const { RefCell::new(None) };
        /// The menu's target, kept for as long as the gesture lives.
        static MENU_TARGET: RefCell<Option<Retained<KrateMenuTarget>>> = const { RefCell::new(None) };
        /// The grab bar and the first-apps hint over a newly opened app.
        static GRAB: RefCell<Option<(Retained<UIView>, Retained<objc2_ui_kit::UILabel>)>> = const { RefCell::new(None) };
    }

    /// What the running app was granted this run, in plain words, for "What
    /// it can use". Set by the player.
    static CURRENT_USES: Mutex<Option<Vec<String>>> = Mutex::new(None);

    /// The player says what the running app may use this run (IC-934);
    /// None while the person has not been asked yet.
    pub fn set_current_uses(uses: Option<Vec<String>>) {
        *CURRENT_USES.lock().expect("uses lock") = uses;
    }

    /// The running app's name, for the menu's title.
    static CURRENT_NAME: Mutex<Option<String>> = Mutex::new(None);

    /// The player names the running app (IC-934).
    pub fn set_current_name(name: Option<String>) {
        *CURRENT_NAME.lock().expect("name lock") = name;
    }

    /// The band under the system's top inset that Krate keeps apps out of,
    /// for the grab bar and the hold.
    const MENU_BAND: f32 = 20.0;

    /// The strip under the clock that holds Krate's menu: the system's top
    /// inset and the band Krate keeps clear below it.
    fn hold_band() -> f32 {
        SAFE.lock().expect("safe lock").0.max(20.0) + MENU_BAND
    }

    /// Put the hold on Krate's view. Main thread, once.
    fn install_menu_gesture(mtm: MainThreadMarker, host: &UIView) {
        let target = KrateMenuTarget::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init on a fresh allocation.
        let target: Retained<KrateMenuTarget> = unsafe { msg_send![super(target), init] };
        if let Some(protocol) = objc2::runtime::AnyProtocol::get(c"UIGestureRecognizerDelegate") {
            // SAFETY: declaring a protocol the class implements by selector.
            unsafe {
                objc2::ffi::class_addProtocol(
                    <KrateMenuTarget as objc2::ClassType>::class()
                        as *const objc2::runtime::AnyClass
                        as *mut objc2::runtime::AnyClass,
                    protocol,
                );
            }
        }
        let Some(class) = objc2::runtime::AnyClass::get(c"UILongPressGestureRecognizer") else {
            return;
        };
        // SAFETY: a long-press recognizer with Krate's target and delegate:
        // half a second, a finger allowed to drift 14 points, the app's own
        // touches never cancelled by it.
        unsafe {
            let made: objc2::rc::Allocated<objc2::runtime::AnyObject> = msg_send![class, alloc];
            let hold: Retained<objc2::runtime::AnyObject> =
                msg_send![made, initWithTarget: &*target, action: objc2::sel!(krateHold:)];
            let _: () = msg_send![&*hold, setMinimumPressDuration: 0.5f64];
            let _: () = msg_send![&*hold, setAllowableMovement: 14.0f64];
            let _: () = msg_send![&*hold, setCancelsTouchesInView: false];
            let _: () = msg_send![&*hold, setDelegate: &*target];
            let _: () = msg_send![host, addGestureRecognizer: &*hold];
        }
        MENU_TARGET.with(|slot| *slot.borrow_mut() = Some(target));
    }

    /// One of UIKit's semantic colours, which follow light and dark mode.
    fn ui_color(name: &str) -> Retained<objc2_ui_kit::UIColor> {
        let class = <objc2_ui_kit::UIColor as objc2::ClassType>::class();
        // SAFETY: UIColor class properties, each returning a UIColor.
        unsafe {
            match name {
                "label" => msg_send![class, labelColor],
                "secondaryLabel" => msg_send![class, secondaryLabelColor],
                "separator" => msg_send![class, separatorColor],
                _ => msg_send![class, systemBackgroundColor],
            }
        }
    }

    fn size(w: f64, h: f64) -> objc2_core_foundation::CGSize {
        objc2_core_foundation::CGSize::new(w, h)
    }

    /// Drop Krate's menu from the top. Main thread.
    ///
    /// One glass panel, one row: an icon over a short word for each way
    /// about, the way the phone's own menus put Copy, Move and Share
    /// (F-314). Liquid Glass on iOS 26 and later, the system's material
    /// before it. It slides in rather than fading: glass whose view fades
    /// from nothing is not drawn at all.
    fn show_menu(mtm: MainThreadMarker, target: &KrateMenuTarget) {
        if MENU.with(|m| m.borrow().is_some()) {
            return;
        }
        let Some(main_side) = MAIN_SIDE.0.get() else {
            return;
        };
        let host: &UIView = &main_side.view;
        let bounds = host.bounds();
        let top = SAFE.lock().expect("safe lock").0.max(20.0) as f64;
        let items: [(&str, &str, &str, objc2::runtime::Sel); 5] = [
            ("xmark", "Close", "Close", objc2::sel!(krateMenuClose:)),
            (
                "arrow.clockwise",
                "Restart",
                "Restart",
                objc2::sel!(krateMenuRestart:),
            ),
            (
                "square.and.arrow.up",
                "Share",
                "Share this app",
                objc2::sel!(krateMenuShare:),
            ),
            (
                "checkmark.shield",
                "Access",
                "What it can use",
                objc2::sel!(krateMenuUses:),
            ),
            (
                "exclamationmark.bubble",
                "Report",
                "Report a problem",
                objc2::sel!(krateMenuReport:),
            ),
        ];
        let width = (bounds.size.width - 32.0).min(380.0);
        let height = 80.0f64;
        let x = (bounds.size.width - width) / 2.0;
        let y = top + 6.0;
        let cell = width / items.len() as f64;
        // SAFETY: UIKit views built and added on the main thread; selectors
        // name methods KrateMenuTarget implements.
        unsafe {
            // Catches a tap anywhere else, to lift the menu; nearly clear, so
            // the app stays in view behind it.
            let shade = UIView::new(mtm);
            shade.setFrame(bounds);
            shade.setBackgroundColor(Some(&objc2_ui_kit::UIColor::colorWithWhite_alpha(
                0.0, 0.12,
            )));
            if let Some(tap_class) = objc2::runtime::AnyClass::get(c"UITapGestureRecognizer") {
                let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                    msg_send![tap_class, alloc];
                let tap: Retained<objc2::runtime::AnyObject> =
                    msg_send![made, initWithTarget: target, action: objc2::sel!(krateMenuDismiss:)];
                let _: () = msg_send![&*shade, addGestureRecognizer: &*tap];
            }
            // Liquid Glass where the phone has it, the thick material where
            // it does not.
            let glass: Option<Retained<objc2::runtime::AnyObject>> =
                objc2::runtime::AnyClass::get(c"UIGlassEffect").and_then(|c| msg_send![c, new]);
            let effect: Option<Retained<objc2::runtime::AnyObject>> = glass.or_else(|| {
                objc2::runtime::AnyClass::get(c"UIBlurEffect")
                    .and_then(|c| msg_send![c, effectWithStyle: 9isize])
            });
            let panel: Retained<UIView> =
                match (objc2::runtime::AnyClass::get(c"UIVisualEffectView"), effect) {
                    (Some(class), Some(effect)) => {
                        let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                            msg_send![class, alloc];
                        let view: Retained<objc2::runtime::AnyObject> =
                            msg_send![made, initWithEffect: &*effect];
                        Retained::cast_unchecked(view)
                    }
                    _ => {
                        let view = UIView::new(mtm);
                        view.setBackgroundColor(Some(&ui_color("systemBackground")));
                        view
                    }
                };
            panel.setFrame(CGRect::new(CGPoint::new(x, y - 14.0), size(width, height)));
            panel.layer().setCornerRadius(26.0);
            panel.setClipsToBounds(true);
            let content: Retained<UIView> = {
                let maybe: Option<Retained<UIView>> = msg_send![&*panel, contentView];
                maybe.unwrap_or_else(|| panel.clone())
            };
            for (i, (symbol, word, said, action)) in items.iter().enumerate() {
                let item =
                    objc2_ui_kit::UIButton::buttonWithType(objc2_ui_kit::UIButtonType::System, mtm);
                let as_view: &UIView = &item;
                as_view.setFrame(CGRect::new(
                    CGPoint::new(i as f64 * cell, 0.0),
                    size(cell, height),
                ));
                let configuration: Option<Retained<objc2::runtime::AnyObject>> =
                    objc2::runtime::AnyClass::get(c"UIImageSymbolConfiguration").and_then(
                        |c| msg_send![c, configurationWithPointSize: 19.0f64, weight: 5isize],
                    );
                let image =
                    UIImage::systemImageNamed(&objc2_foundation::NSString::from_str(symbol));
                let image: Option<Retained<UIImage>> = match (image, configuration) {
                    (Some(image), Some(configuration)) => {
                        msg_send![&*image, imageWithConfiguration: &*configuration]
                    }
                    (image, _) => image,
                };
                let icon = UIImageView::initWithImage(UIImageView::alloc(mtm), image.as_deref());
                icon.setTintColor(Some(&ui_color("label")));
                let _: () = msg_send![&*icon, setContentMode: 1isize];
                let icon_view: &UIView = &icon;
                icon_view.setFrame(CGRect::new(CGPoint::new(0.0, 16.0), size(cell, 26.0)));
                icon_view.setUserInteractionEnabled(false);
                as_view.addSubview(icon_view);
                let label = objc2_ui_kit::UILabel::new(mtm);
                label.setText(Some(&objc2_foundation::NSString::from_str(word)));
                label.setFont(Some(&objc2_ui_kit::UIFont::systemFontOfSize_weight(
                    12.0,
                    objc2_ui_kit::UIFontWeightMedium,
                )));
                label.setTextColor(Some(&ui_color("label")));
                label.setTextAlignment(objc2_ui_kit::NSTextAlignment::Center);
                let label_view: &UIView = &label;
                label_view.setFrame(CGRect::new(CGPoint::new(0.0, 48.0), size(cell, 16.0)));
                label_view.setUserInteractionEnabled(false);
                as_view.addSubview(label_view);
                let _: () = msg_send![
                    as_view,
                    setAccessibilityLabel: &*objc2_foundation::NSString::from_str(said)
                ];
                item.addTarget_action_forControlEvents(
                    Some(target),
                    *action,
                    objc2_ui_kit::UIControlEvents::TouchUpInside,
                );
                content.addSubview(as_view);
            }
            host.addSubview(&shade);
            host.addSubview(&panel);
            let panel_a = panel.clone();
            let settle = block2::RcBlock::new(move || {
                panel_a.setFrame(CGRect::new(CGPoint::new(x, y), size(width, height)));
            });
            if let Some(view_class) = objc2::runtime::AnyClass::get(c"UIView") {
                let _: () = msg_send![
                    view_class,
                    animateWithDuration: 0.26f64,
                    delay: 0.0f64,
                    usingSpringWithDamping: 0.78f64,
                    initialSpringVelocity: 0.0f64,
                    options: 0usize,
                    animations: &*settle,
                    completion: Option::<&objc2::runtime::AnyObject>::None
                ];
            }
            // A light tap under the finger: the hold was heard.
            haptic(Some(krate_adapter_common::ui::HapticFeel::Light), 1.0);
            MENU.with(|m| *m.borrow_mut() = Some((shade, panel)));
        }
    }

    /// Keep a small, blurred picture of the screen as it is now at `path`
    /// (an 8-byte header of width and height, then RGBA), for Krate's
    /// permission sheet to sit over: the screen the person came from,
    /// softened, as the phone's own sheets do. Waits for the main thread.
    pub fn capture_behind(path: &std::path::Path) {
        let path = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let _ = tx.send(snapshot_small());
        });
        let Ok(Some((w, h, mut rgba))) = rx.recv_timeout(std::time::Duration::from_millis(800))
        else {
            let _ = std::fs::remove_file(&path);
            return;
        };
        for _ in 0..3 {
            box_blur(&mut rgba, w as usize, h as usize, 4);
        }
        let mut out = Vec::with_capacity(8 + rgba.len());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&rgba);
        let part = path.with_extension("part");
        if std::fs::write(&part, &out).is_ok() {
            let _ = std::fs::rename(&part, &path);
        }
    }

    #[link(name = "UIKit", kind = "framework")]
    unsafe extern "C" {
        fn UIGraphicsBeginImageContextWithOptions(
            size: objc2_core_foundation::CGSize,
            opaque: bool,
            scale: f64,
        );
        fn UIGraphicsGetCurrentContext() -> *mut std::ffi::c_void;
        fn UIGraphicsEndImageContext();
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGBitmapContextGetData(context: *mut std::ffi::c_void) -> *mut u8;
        fn CGBitmapContextGetBytesPerRow(context: *mut std::ffi::c_void) -> usize;
    }

    /// Krate's view drawn at a third of its size, as RGBA. Main thread.
    fn snapshot_small() -> Option<(u32, u32, Vec<u8>)> {
        let main_side = MAIN_SIDE.0.get()?;
        let host: &UIView = &main_side.view;
        let bounds = host.bounds();
        let (w, h) = (
            (bounds.size.width / 3.0).round(),
            (bounds.size.height / 3.0).round(),
        );
        if w < 1.0 || h < 1.0 {
            return None;
        }
        // SAFETY: a UIKit image context on the main thread, the view's
        // hierarchy drawn into it, its bytes read while it is current, then
        // ended. The context is 32-bit premultiplied-first little-endian:
        // BGRA in memory, opaque here.
        unsafe {
            UIGraphicsBeginImageContextWithOptions(size(w, h), true, 1.0);
            let drawn: bool = msg_send![
                host,
                drawViewHierarchyInRect: CGRect::new(CGPoint::new(0.0, 0.0), size(w, h)),
                afterScreenUpdates: false
            ];
            let context = UIGraphicsGetCurrentContext();
            let data = if context.is_null() {
                std::ptr::null_mut()
            } else {
                CGBitmapContextGetData(context)
            };
            let result = if drawn && !data.is_null() {
                let stride = CGBitmapContextGetBytesPerRow(context);
                let (w, h) = (w as usize, h as usize);
                let mut rgba = Vec::with_capacity(w * h * 4);
                for row in 0..h {
                    let line = std::slice::from_raw_parts(data.add(row * stride), w * 4);
                    for px in line.chunks_exact(4) {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
                Some((w as u32, h as u32, rgba))
            } else {
                None
            };
            UIGraphicsEndImageContext();
            result
        }
    }

    /// One pass of a box blur of `radius` over RGBA pixels, across then down.
    fn box_blur(rgba: &mut [u8], w: usize, h: usize, radius: usize) {
        let mut tmp = vec![0u8; rgba.len()];
        let pass =
            |src: &[u8], dst: &mut [u8], along: usize, across: usize, step: usize, line: usize| {
                for a in 0..across {
                    for i in 0..along {
                        let lo = i.saturating_sub(radius);
                        let hi = (i + radius).min(along - 1);
                        let mut sum = [0u32; 3];
                        for j in lo..=hi {
                            let at = (a * line + j * step) * 4;
                            for c in 0..3 {
                                sum[c] += u32::from(src[at + c]);
                            }
                        }
                        let n = (hi - lo + 1) as u32;
                        let at = (a * line + i * step) * 4;
                        for c in 0..3 {
                            dst[at + c] = (sum[c] / n) as u8;
                        }
                        dst[at + 3] = 255;
                    }
                }
            };
        pass(rgba, &mut tmp, w, h, 1, w);
        pass(&tmp, rgba, h, w, w, 1);
    }

    /// Lift Krate's menu. Main thread.
    fn hide_menu() {
        if let Some((shade, panel)) = MENU.with(|m| m.borrow_mut().take()) {
            shade.removeFromSuperview();
            panel.removeFromSuperview();
        }
    }

    /// Close the app and open it again: the player opens what was asked for
    /// once the app has closed, as a tapped reminder does. Main thread.
    fn restart_app() {
        let opens = CURRENT_APP
            .lock()
            .expect("app lock")
            .as_ref()
            .map(|(_, opens)| opens.clone());
        if let Some(opens) = opens {
            *NOTICE_OPEN.lock().expect("notice lock") = Some(opens);
            close_for_opened_file();
        }
    }

    /// The share sheet with the app's own .krate, so it can be sent to
    /// someone; the app built into Krate shares Krate's address. Main thread.
    fn share_app() {
        let opens = CURRENT_APP
            .lock()
            .expect("app lock")
            .as_ref()
            .map(|(_, opens)| opens.clone());
        let (Some(controller), Some(sheet_class), Some(url_class)) = (
            main_controller(),
            objc2::runtime::AnyClass::get(c"UIActivityViewController"),
            objc2::runtime::AnyClass::get(c"NSURL"),
        ) else {
            return;
        };
        // SAFETY: an NSURL item in an NSArray, a UIActivityViewController over
        // it anchored to Krate's view, presented from Krate's controller.
        unsafe {
            let item: Option<Retained<objc2::runtime::AnyObject>> = match opens.as_deref() {
                Some(path) if path.ends_with(".krate") => msg_send![
                    url_class,
                    fileURLWithPath: &*objc2_foundation::NSString::from_str(path)
                ],
                _ => msg_send![
                    url_class,
                    URLWithString: &*objc2_foundation::NSString::from_str("https://krate.tech")
                ],
            };
            let Some(item) = item else {
                return;
            };
            let items = objc2_foundation::NSArray::from_retained_slice(&[item]);
            let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                msg_send![sheet_class, alloc];
            let sheet: Option<Retained<objc2::runtime::AnyObject>> = msg_send![
                made,
                initWithActivityItems: &*items,
                applicationActivities: Option::<&objc2::runtime::AnyObject>::None
            ];
            let Some(sheet) = sheet else {
                return;
            };
            let popover: Option<Retained<objc2::runtime::AnyObject>> =
                msg_send![&*sheet, popoverPresentationController];
            if let (Some(popover), Some(main_side)) = (popover, MAIN_SIDE.0.get()) {
                let host: &UIView = &main_side.view;
                let _: () = msg_send![&*popover, setSourceView: host];
            }
            let _: () = msg_send![
                &*controller,
                presentViewController: &*sheet,
                animated: true,
                completion: Option::<&objc2::runtime::AnyObject>::None
            ];
        }
    }

    /// Show a message with one button (`question` false) or a yes-or-no
    /// question as the iPhone's own alert, and wait until the person closes
    /// it (krate:ui/dialog). Yes is the only yes. The alert runs on the
    /// main thread; the app's thread waits, as it does for the picker.
    pub fn ask(title: &str, body: &str, question: bool) -> bool {
        let (title, body) = (title.to_string(), body.to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || present_ask(&title, &body, question, tx));
        rx.recv().unwrap_or(false)
    }

    fn present_ask(title: &str, body: &str, question: bool, tx: std::sync::mpsc::Sender<bool>) {
        let (Some(controller), Some(alert_class), Some(action_class)) = (
            main_controller(),
            objc2::runtime::AnyClass::get(c"UIAlertController"),
            objc2::runtime::AnyClass::get(c"UIAlertAction"),
        ) else {
            let _ = tx.send(false);
            return;
        };
        // SAFETY: a UIAlertController with one or two UIAlertActions,
        // presented from the topmost controller (a controller already
        // presenting something shows nothing new, and the app would wait
        // forever); the handler blocks run on the main thread.
        unsafe {
            let mut top: Retained<objc2::runtime::AnyObject> = Retained::cast_unchecked(controller);
            loop {
                let next: Option<Retained<objc2::runtime::AnyObject>> =
                    msg_send![&*top, presentedViewController];
                match next {
                    Some(next) => top = next,
                    None => break,
                }
            }
            let alert: Retained<objc2::runtime::AnyObject> = msg_send![
                alert_class,
                alertControllerWithTitle: &*objc2_foundation::NSString::from_str(title),
                message: &*objc2_foundation::NSString::from_str(body),
                preferredStyle: 1isize
            ];
            let button = |label: &str, style: isize, answer: bool| {
                let tx = tx.clone();
                let handler =
                    block2::RcBlock::new(move |_action: *mut objc2::runtime::AnyObject| {
                        let _ = tx.send(answer);
                    });
                let action: Retained<objc2::runtime::AnyObject> = msg_send![
                    action_class,
                    actionWithTitle: &*objc2_foundation::NSString::from_str(label),
                    style: style,
                    handler: &*handler
                ];
                let _: () = msg_send![&*alert, addAction: &*action];
            };
            if question {
                button("No", 1, false);
                button("Yes", 0, true);
            } else {
                button("OK", 0, false);
            }
            let _: () = msg_send![
                &*top,
                presentViewController: &*alert,
                animated: true,
                completion: Option::<&objc2::runtime::AnyObject>::None
            ];
        }
    }

    /// What the app may use this run, in plain words, with the way to
    /// change it: Krate asks on every open, so "Ask again" opens it again.
    fn show_uses() {
        let name = CURRENT_NAME
            .lock()
            .expect("name lock")
            .clone()
            .unwrap_or_else(|| "This app".to_string());
        let uses = CURRENT_USES.lock().expect("uses lock").clone();
        let message = match &uses {
            None => "Nothing yet: it has not been allowed anything.".to_string(),
            Some(uses) if uses.is_empty() => {
                "Only its own window: nothing on this phone beyond it.".to_string()
            }
            Some(uses) => uses
                .iter()
                .map(|u| format!("\u{2022} {u}"))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let (Some(controller), Some(alert_class), Some(action_class)) = (
            main_controller(),
            objc2::runtime::AnyClass::get(c"UIAlertController"),
            objc2::runtime::AnyClass::get(c"UIAlertAction"),
        ) else {
            return;
        };
        // SAFETY: a UIAlertController with two UIAlertActions, presented from
        // Krate's controller; the handler block runs on the main thread.
        unsafe {
            let alert: Retained<objc2::runtime::AnyObject> = msg_send![
                alert_class,
                alertControllerWithTitle: &*objc2_foundation::NSString::from_str(&format!("{name} can use")),
                message: &*objc2_foundation::NSString::from_str(&message),
                preferredStyle: 1isize
            ];
            let again =
                block2::RcBlock::new(|_action: *mut objc2::runtime::AnyObject| restart_app());
            let ask: Retained<objc2::runtime::AnyObject> = msg_send![
                action_class,
                actionWithTitle: &*objc2_foundation::NSString::from_str("Ask again"),
                style: 0isize,
                handler: &*again
            ];
            let done: Retained<objc2::runtime::AnyObject> = msg_send![
                action_class,
                actionWithTitle: &*objc2_foundation::NSString::from_str("Done"),
                style: 1isize,
                handler: Option::<&objc2::runtime::AnyObject>::None
            ];
            let _: () = msg_send![&*alert, addAction: &*ask];
            let _: () = msg_send![&*alert, addAction: &*done];
            let _: () = msg_send![
                &*controller,
                presentViewController: &*alert,
                animated: true,
                completion: Option::<&objc2::runtime::AnyObject>::None
            ];
        }
    }

    /// A mail to Krate about the running app, its name and id filled in.
    /// Main thread.
    fn report_problem() {
        let name = CURRENT_NAME
            .lock()
            .expect("name lock")
            .clone()
            .unwrap_or_else(|| "an app".to_string());
        let id = CURRENT_APP
            .lock()
            .expect("app lock")
            .as_ref()
            .map(|(id, _)| id.clone())
            .unwrap_or_default();
        let encode = |text: &str| -> String {
            text.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect()
        };
        let link = format!(
            "mailto:yashraj@krate.tech?subject={}&body={}",
            encode(&format!("A problem with {name}")),
            encode(&format!("App: {name} ({id})\n\nWhat happened:\n"))
        );
        let (Some(url_class), Some(mtm)) = (
            objc2::runtime::AnyClass::get(c"NSURL"),
            MainThreadMarker::new(),
        ) else {
            return;
        };
        // SAFETY: +[NSURL URLWithString:], then UIApplication's
        // openURL:options:completionHandler:, on the main thread.
        unsafe {
            let url: Option<Retained<objc2::runtime::AnyObject>> =
                msg_send![url_class, URLWithString: &*objc2_foundation::NSString::from_str(&link)];
            if let Some(url) = url {
                let app = UIApplication::sharedApplication(mtm);
                let options = objc2_foundation::NSDictionary::<
                    objc2::runtime::AnyObject,
                    objc2::runtime::AnyObject,
                >::new();
                let _: () = msg_send![
                    &*app,
                    openURL: &*url,
                    options: &*options,
                    completionHandler: Option::<&objc2::runtime::AnyObject>::None
                ];
            }
        }
    }

    /// Show the grab bar at the top centre over a newly opened app, fading
    /// out after two seconds, and on the first three apps a person opens a
    /// line saying how to reach the menu. Main thread.
    fn show_grab(mtm: MainThreadMarker) {
        let Some(main_side) = MAIN_SIDE.0.get() else {
            return;
        };
        let host: &UIView = &main_side.view;
        let bounds = host.bounds();
        let top = SAFE.lock().expect("safe lock").0.max(20.0) as f64;
        // SAFETY: UIKit views made and styled on the main thread.
        let (bar, hint) = GRAB.with(|slot| unsafe {
            slot.borrow_mut()
                .get_or_insert_with(|| {
                    let bar = UIView::new(mtm);
                    bar.setBackgroundColor(Some(&objc2_ui_kit::UIColor::colorWithWhite_alpha(
                        0.55, 0.85,
                    )));
                    bar.layer().setCornerRadius(2.5);
                    bar.setUserInteractionEnabled(false);
                    let hint = objc2_ui_kit::UILabel::new(mtm);
                    hint.setText(Some(&objc2_foundation::NSString::from_str(
                        "Touch and hold the top for Krate's menu",
                    )));
                    hint.setFont(Some(&objc2_ui_kit::UIFont::systemFontOfSize_weight(
                        13.0,
                        objc2_ui_kit::UIFontWeightMedium,
                    )));
                    hint.setTextColor(Some(&objc2_ui_kit::UIColor::whiteColor()));
                    hint.setTextAlignment(objc2_ui_kit::NSTextAlignment::Center);
                    let as_view: &UIView = &hint;
                    as_view.setBackgroundColor(Some(&objc2_ui_kit::UIColor::colorWithWhite_alpha(
                        0.0, 0.72,
                    )));
                    as_view.layer().setCornerRadius(15.0);
                    as_view.setClipsToBounds(true);
                    as_view.setUserInteractionEnabled(false);
                    (bar, hint)
                })
                .clone()
        });
        bar.setFrame(CGRect::new(
            CGPoint::new(bounds.size.width / 2.0 - 18.0, top + 6.0),
            size(36.0, 5.0),
        ));
        host.addSubview(&bar);
        bar.setAlpha(1.0);
        // SAFETY: NSUserDefaults read and written on the main thread, and
        // UIView's class animation with a block run on it.
        unsafe {
            let defaults: Option<Retained<objc2::runtime::AnyObject>> =
                objc2::runtime::AnyClass::get(c"NSUserDefaults")
                    .and_then(|c| msg_send![c, standardUserDefaults]);
            let key = objc2_foundation::NSString::from_str("krate.menuHints");
            let shown: isize = defaults
                .as_ref()
                .map_or(3, |d| msg_send![&**d, integerForKey: &*key]);
            let hint_view: &UIView = &hint;
            if shown < 3 {
                if let Some(d) = &defaults {
                    let _: () = msg_send![&**d, setInteger: shown + 1, forKey: &*key];
                }
                let w = 300.0f64.min(bounds.size.width - 32.0);
                hint_view.setFrame(CGRect::new(
                    CGPoint::new((bounds.size.width - w) / 2.0, top + 18.0),
                    size(w, 30.0),
                ));
                host.addSubview(hint_view);
                hint_view.setAlpha(1.0);
            } else {
                hint_view.setAlpha(0.0);
            }
            let (bar_a, hint_a) = (bar.clone(), hint.clone());
            let fade = block2::RcBlock::new(move || {
                bar_a.setAlpha(0.0);
                let h: &UIView = &hint_a;
                h.setAlpha(0.0);
            });
            if let Some(view_class) = objc2::runtime::AnyClass::get(c"UIView") {
                let _: () = msg_send![
                    view_class,
                    animateWithDuration: 0.6f64,
                    delay: if shown < 3 { 3.5f64 } else { 2.0f64 },
                    options: 0usize,
                    animations: &*fade,
                    completion: Option::<&objc2::runtime::AnyObject>::None
                ];
            }
        }
    }

    /// Krate's loading cover: the mark, a title, a line of status and the
    /// system spinner, over everything while an app opens or Krate gets
    /// ready -- so a person never watches a blank screen, or an app's first
    /// frame drawn for the wrong size.
    struct Loading {
        cover: Retained<UIView>,
        mark: Retained<UIImageView>,
        title: Retained<objc2_ui_kit::UILabel>,
        status: Retained<objc2_ui_kit::UILabel>,
        spinner: Retained<objc2_ui_kit::UIActivityIndicatorView>,
    }

    static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static LOADING_SINCE: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    /// The cover lifts on its own after this, whatever the app does.
    const LOADING_MOST: std::time::Duration = std::time::Duration::from_secs(5);

    fn cover_shown(main_side: &MainSide, title: &str, status: &str) {
        let l = &main_side.loading;
        l.title
            .setText(Some(&objc2_foundation::NSString::from_str(title)));
        l.status
            .setText(Some(&objc2_foundation::NSString::from_str(status)));
        l.cover.setHidden(false);
        let host: &UIView = &main_side.view;
        host.bringSubviewToFront(&l.cover);
        raise_menu(host);
        l.spinner.startAnimating();
    }

    /// Krate's menu stays above everything, the opening cover included:
    /// the cover is raised on every layout, and adding the menu is itself a
    /// layout, so a menu opened over the cover was put straight back under
    /// it -- open, untouchable, and refusing a second hold. Main thread.
    fn raise_menu(host: &UIView) {
        MENU.with(|m| {
            if let Some((shade, panel)) = m.borrow().as_ref() {
                host.bringSubviewToFront(shade);
                host.bringSubviewToFront(panel);
            }
        });
    }

    /// Cover the screen with Krate's loading cover. It lifts by itself on
    /// the first frame an app draws at the screen's real size (or after five
    /// seconds), or with `hide_loading`.
    pub fn show_loading(title: &str, status: &str) {
        if ensure_main_side().is_err() {
            return;
        }
        LOADING.store(true, std::sync::atomic::Ordering::Relaxed);
        *LOADING_SINCE.lock().expect("loading lock") = Some(std::time::Instant::now());
        let (title, status) = (title.to_string(), status.to_string());
        DispatchQueue::main().exec_async(move || {
            if let Some(main_side) = MAIN_SIDE.0.get() {
                cover_shown(main_side, &title, &status);
            }
        });
    }

    /// Lift the loading cover.
    pub fn hide_loading() {
        if !LOADING.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        DispatchQueue::main().exec_async(|| {
            if let Some(main_side) = MAIN_SIDE.0.get() {
                main_side.loading.cover.setHidden(true);
                main_side.loading.spinner.stopAnimating();
            }
        });
    }

    /// A frame of `size` reached the screen: the cover lifts when the frame
    /// is the screen's real size, or when it has been up too long.
    fn frame_shown(size: Option<WindowSize>) {
        if !LOADING.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let screen = SHARED
            .lock()
            .expect("shared state lock")
            .screen
            .map(|(l, _)| l);
        let right_size = match (size, screen) {
            (Some(frame), Some(screen)) => {
                frame.width.abs_diff(screen.width) <= 1 && frame.height.abs_diff(screen.height) <= 1
            }
            _ => true,
        };
        let too_long = LOADING_SINCE
            .lock()
            .expect("loading lock")
            .is_some_and(|since| since.elapsed() > LOADING_MOST);
        if right_size || too_long {
            hide_loading();
        }
    }

    /// Show (true) or hide Krate's close capsule. The player shows it over
    /// every app it runs and hides it on Krate's own screens.
    ///
    /// A phone has no window close button, so an app had no way back. A
    /// gesture would collide with apps: a pinch is a map's zoom, an edge
    /// swipe a game's thumb stick, and neither can be discovered. A capsule
    /// drawn by Krate -- not by the app, so no app can hide or imitate it --
    /// is always there and means the same thing everywhere. Tapping it asks
    /// the app to close, as a desktop close button does; a second tap closes
    /// an app that does not answer.
    pub fn set_app_chrome(visible: bool) {
        if !visible {
            access_clear();
            stop_motion();
            stop_location();
            crate::bluetooth::disconnect_all();
        }
        if visible {
            // A close tapped before this app began (on the screen before
            // it) is not meant for it: it must not close the app at once.
            SHARED.lock().expect("shared state lock").close_tapped = false;
            APP_RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        APP_CHROME.store(visible, std::sync::atomic::Ordering::Relaxed);
        DispatchQueue::main().exec_async(move || {
            if let Some(main_side) = MAIN_SIDE.0.get() {
                // Krate's menu replaces the capsule (IC-934): no button over
                // an app; a grab bar that fades and a hold under the clock.
                main_side.close.setHidden(true);
                if let Some(mtm) = MainThreadMarker::new() {
                    if visible {
                        show_grab(mtm);
                    } else {
                        hide_menu();
                        close_all_web_pages();
                    }
                }
            }
        });
    }

    static APP_CHROME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The system's safe-area insets (top, right, bottom, left), as last
    /// laid out: the notch, the status bar, the home bar.
    static SAFE: Mutex<(f32, f32, f32, f32)> = Mutex::new((0.0, 0.0, 0.0, 0.0));

    /// How much of the screen's bottom the on-screen keyboard covers, in
    /// points; 0 while it is away (IC-915).
    static KEYBOARD: Mutex<f32> = Mutex::new(0.0);
    /// The covered edges changed without the screen changing size (the
    /// keyboard came or went): the app is sent a resize so it asks again.
    static INSETS_CHANGED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// Follow the on-screen keyboard: while it shows, its height counts as a
    /// covered bottom edge in `krate:ui/screen`, so an app keeps the field
    /// being typed into above it. Without this a field near the bottom of a
    /// phone was hidden by the keyboard typing into it.
    fn observe_keyboard() {
        use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue};
        let center = NSNotificationCenter::defaultCenter();
        let queue = NSOperationQueue::mainQueue();
        let changed = block2::RcBlock::new(|note: std::ptr::NonNull<NSNotification>| {
            // SAFETY: UIKit hands a live notification to its observer.
            let note = unsafe { note.as_ref() };
            let frame = note.userInfo().and_then(|info| {
                // SAFETY: the key is UIKit's own constant; the value under it
                // is an NSValue holding a CGRect, per UIKit's documentation.
                let key = unsafe { objc2_ui_kit::UIKeyboardFrameEndUserInfoKey };
                let value = info.objectForKey(key)?;
                let value = value.downcast::<objc2_foundation::NSValue>().ok()?;
                use objc2_ui_kit::NSValueUIGeometryExtensions;
                Some(unsafe { value.CGRectValue() })
            });
            let Some(frame) = frame else {
                return;
            };
            let Some(main_side) = MAIN_SIDE.0.get() else {
                return;
            };
            let bounds = main_side.window.bounds();
            let covered =
                (bounds.size.height - frame.origin.y).clamp(0.0, bounds.size.height) as f32;
            set_keyboard(covered);
        });
        let hidden = block2::RcBlock::new(|_: std::ptr::NonNull<NSNotification>| {
            set_keyboard(0.0);
        });
        // SAFETY: names are UIKit's constants; the blocks are 'static and run
        // on the main queue, where they read main-thread state.
        unsafe {
            let _ = center.addObserverForName_object_queue_usingBlock(
                Some(objc2_ui_kit::UIKeyboardWillChangeFrameNotification),
                None,
                Some(&queue),
                &changed,
            );
            let _ = center.addObserverForName_object_queue_usingBlock(
                Some(objc2_ui_kit::UIKeyboardWillHideNotification),
                None,
                Some(&queue),
                &hidden,
            );
        }
    }

    /// The person's pick, waiting to be sent back to the app's thread.
    pub struct PickState {
        tx: std::cell::RefCell<Option<std::sync::mpsc::Sender<Option<std::path::PathBuf>>>>,
        /// A folder is opened in place and needs its access started.
        folder: bool,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a delegate.
        // - UIKit calls a document picker's delegate on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = PickState]
        struct KratePickDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KratePickDelegate {}

        // SAFETY: the methods match UIDocumentPickerDelegate's signatures.
        unsafe impl UIDocumentPickerDelegate for KratePickDelegate {
            #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
            fn picked(
                &self,
                _controller: &objc2_ui_kit::UIDocumentPickerViewController,
                urls: &objc2_foundation::NSArray<objc2_foundation::NSURL>,
            ) {
                let path = urls.firstObject().and_then(|url| {
                    if self.ivars().folder {
                        // A folder outside Krate is reachable only while its
                        // security scope is open; it stays open for the run.
                        // SAFETY: an ordinary call on a URL UIKit handed us.
                        unsafe { url.startAccessingSecurityScopedResource() };
                    }
                    url.path().map(|p| std::path::PathBuf::from(p.to_string()))
                });
                self.finish(path);
            }

            #[unsafe(method(documentPickerWasCancelled:))]
            fn cancelled(&self, _controller: &objc2_ui_kit::UIDocumentPickerViewController) {
                self.finish(None);
            }
        }
    );

    impl KratePickDelegate {
        fn finish(&self, path: Option<std::path::PathBuf>) {
            if let Some(tx) = self.ivars().tx.borrow_mut().take() {
                let _ = tx.send(path);
            }
            PICK_DELEGATE.with(|slot| slot.borrow_mut().take());
        }
    }

    thread_local! {
        /// The live picker delegate: UIKit holds delegates weakly.
        static PICK_DELEGATE: std::cell::RefCell<Option<objc2::rc::Retained<KratePickDelegate>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Show the Files picker for a file or a folder and wait for the
    /// person's pick (IC-915). A file comes back as a copy iOS makes for
    /// Krate; a folder is opened in place. Runs the picker on the main
    /// thread; the app's thread waits for the answer.
    pub fn choose_path(folder: bool, filter: &str) -> Option<std::path::PathBuf> {
        let filter = filter.to_ascii_lowercase();
        let images = ["png", "jpg", "jpeg", "gif", "heic", "image", "photo"]
            .iter()
            .any(|word| filter.contains(word));
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || present_pick(folder, images, tx));
        rx.recv().ok().flatten()
    }

    #[allow(deprecated)]
    fn present_pick(
        folder: bool,
        images: bool,
        tx: std::sync::mpsc::Sender<Option<std::path::PathBuf>>,
    ) {
        let (Some(mtm), Some(main_side)) = (MainThreadMarker::new(), MAIN_SIDE.0.get()) else {
            let _ = tx.send(None);
            return;
        };
        let kind = if folder {
            "public.folder"
        } else if images {
            "public.image"
        } else {
            "public.item"
        };
        let types = objc2_foundation::NSArray::from_retained_slice(&[
            objc2_foundation::NSString::from_str(kind),
        ]);
        let mode = if folder {
            objc2_ui_kit::UIDocumentPickerMode::Open
        } else {
            objc2_ui_kit::UIDocumentPickerMode::Import
        };
        let picker = objc2_ui_kit::UIDocumentPickerViewController::initWithDocumentTypes_inMode(
            objc2_ui_kit::UIDocumentPickerViewController::alloc(mtm),
            &types,
            mode,
        );
        let delegate: objc2::rc::Retained<KratePickDelegate> = {
            let this = KratePickDelegate::alloc(mtm).set_ivars(PickState {
                tx: std::cell::RefCell::new(Some(tx)),
                folder,
            });
            unsafe { objc2::msg_send![super(this), init] }
        };
        picker.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));
        PICK_DELEGATE.with(|slot| *slot.borrow_mut() = Some(delegate));
        main_side
            .controller
            .presentViewController_animated_completion(&picker, true, None);
    }

    // CoreMotion, through the Objective-C runtime: one manager, made on the
    // first ask, read without a queue (the app pulls at its own frame rate).
    #[link(name = "CoreMotion", kind = "framework")]
    unsafe extern "C" {}

    /// CMAcceleration and CMRotationRate: three doubles.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CmVector {
        x: f64,
        y: f64,
        z: f64,
    }

    // SAFETY: the C layout of CoreMotion's {double x, y, z} structs.
    unsafe impl objc2::Encode for CmVector {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "?",
            &[
                objc2::Encoding::Double,
                objc2::Encoding::Double,
                objc2::Encoding::Double,
            ],
        );
    }

    struct MotionManager(Retained<objc2::runtime::AnyObject>);
    // SAFETY: CMMotionManager's pull interface (deviceMotion, start, stop)
    // may be used from any thread; it is only ever touched under the lock.
    unsafe impl Send for MotionManager {}

    static MOTION: Mutex<Option<MotionManager>> = Mutex::new(None);

    /// The way the interface is turned, as UIKit numbers it (1 upright,
    /// 2 upside down, 3 home button right, 4 home button left), kept from
    /// the last layout so a reading can be turned into the screen's frame.
    static INTERFACE_TURN: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

    /// The way the interface is turned now, as UIKit numbers it (1 upright,
    /// 2 upside down, 3 home button right, 4 home button left).
    pub fn interface_turn() -> u8 {
        INTERFACE_TURN.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn motion_class() -> Option<&'static objc2::runtime::AnyClass> {
        objc2::runtime::AnyClass::get(c"CMMotionManager")
    }

    pub fn motion_available() -> bool {
        if let Some(manager) = MOTION.lock().expect("motion lock").as_ref() {
            return unsafe { msg_send![&*manager.0, isDeviceMotionAvailable] };
        }
        let Some(class) = motion_class() else {
            return false;
        };
        // SAFETY: a fresh CMMotionManager, asked one property, then dropped.
        unsafe {
            let fresh: Option<Retained<objc2::runtime::AnyObject>> = msg_send![class, new];
            fresh.is_some_and(|m| msg_send![&*m, isDeviceMotionAvailable])
        }
    }

    /// The latest reading in the screen's frame (x right, y down, z toward
    /// the person), starting the sensors on the first ask (IC-917).
    pub fn motion_read() -> Option<krate_adapter_common::ui::MotionReading> {
        let mut slot = MOTION.lock().expect("motion lock");
        if slot.is_none() {
            let class = motion_class()?;
            // SAFETY: CMMotionManager's documented setup: make, check, set
            // the interval, start; the north-referenced frame only when the
            // device offers it (it carries the compass heading).
            unsafe {
                let manager: Option<Retained<objc2::runtime::AnyObject>> = msg_send![class, new];
                let manager = manager?;
                let available: bool = msg_send![&*manager, isDeviceMotionAvailable];
                if !available {
                    return None;
                }
                let _: () = msg_send![&*manager, setDeviceMotionUpdateInterval: 1.0f64 / 60.0];
                let frames: usize = msg_send![class, availableAttitudeReferenceFrames];
                const NORTH_Z_VERTICAL: usize = 1 << 2;
                if frames & NORTH_Z_VERTICAL != 0 {
                    let _: () = msg_send![&*manager, startDeviceMotionUpdatesUsingReferenceFrame: NORTH_Z_VERTICAL];
                } else {
                    let _: () = msg_send![&*manager, startDeviceMotionUpdates];
                }
                *slot = Some(MotionManager(manager));
            }
        }
        let manager = &slot.as_ref()?.0;
        // SAFETY: deviceMotion is nil until the first sample; its struct
        // properties are plain doubles.
        let (g, a, r, heading) = unsafe {
            let motion: Option<Retained<objc2::runtime::AnyObject>> =
                msg_send![&**manager, deviceMotion];
            let motion = motion?;
            let g: CmVector = msg_send![&*motion, gravity];
            let a: CmVector = msg_send![&*motion, userAcceleration];
            let r: CmVector = msg_send![&*motion, rotationRate];
            let heading: f64 = msg_send![&*motion, heading];
            (g, a, r, heading)
        };
        let turn = INTERFACE_TURN.load(std::sync::atomic::Ordering::Relaxed);
        Some(motion_in_screen_frame(turn, g, a, r, heading))
    }

    /// The device's readings in the screen's frame for the way the
    /// interface is turned (UIKit's numbering).
    fn motion_in_screen_frame(
        turn: u8,
        g: CmVector,
        a: CmVector,
        r: CmVector,
        heading: f64,
    ) -> krate_adapter_common::ui::MotionReading {
        use krate_adapter_common::ui::{device_to_screen, heading_to_screen, ScreenTurn};
        let turn = match turn {
            2 => ScreenTurn::UpsideDown,
            // UIInterfaceOrientationLandscapeRight: home button on the
            // right, so the phone's top points left.
            3 => ScreenTurn::TopLeft,
            4 => ScreenTurn::TopRight,
            _ => ScreenTurn::Upright,
        };
        let degrees = 180.0 / std::f64::consts::PI;
        krate_adapter_common::ui::MotionReading {
            gravity: device_to_screen(turn, (g.x, g.y, g.z)),
            acceleration: device_to_screen(turn, (a.x, a.y, a.z)),
            rotation: device_to_screen(turn, (r.x * degrees, r.y * degrees, r.z * degrees)),
            heading: (heading >= 0.0).then(|| heading_to_screen(turn, heading)),
        }
    }

    /// The app closed: stop the sensors so they cost nothing.
    fn stop_motion() {
        if let Some(manager) = MOTION.lock().expect("motion lock").take() {
            // SAFETY: stopping updates on the manager that started them.
            let _: () = unsafe { msg_send![&*manager.0, stopDeviceMotionUpdates] };
        }
    }

    // CoreLocation, through the Objective-C runtime, read by polling: no
    // delegate, the manager's latest fix is asked for each time.
    #[link(name = "CoreLocation", kind = "framework")]
    unsafe extern "C" {}

    /// CLLocationCoordinate2D.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CoordinateC {
        latitude: f64,
        longitude: f64,
    }

    // SAFETY: the C layout of CLLocationCoordinate2D, two doubles.
    unsafe impl objc2::Encode for CoordinateC {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CLLocationCoordinate2D",
            &[objc2::Encoding::Double, objc2::Encoding::Double],
        );
    }

    struct LocationManager(Retained<objc2::runtime::AnyObject>);
    // SAFETY: made and stopped on the main thread; its properties are read
    // from the guest thread under the lock, which CLLocationManager allows.
    unsafe impl Send for LocationManager {}

    static LOCATION: Mutex<Option<LocationManager>> = Mutex::new(None);
    static LOCATION_ASKED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn location_class() -> Option<&'static objc2::runtime::AnyClass> {
        objc2::runtime::AnyClass::get(c"CLLocationManager")
    }

    pub fn location_available() -> bool {
        // SAFETY: a class method answering a BOOL.
        location_class().is_some_and(|class| unsafe { msg_send![class, locationServicesEnabled] })
    }

    /// The latest fix (IC-917). The first ask makes the manager on the main
    /// thread -- where the phone's prompt can show -- asks to use location
    /// while the app is open, and starts it; until a fix comes it answers
    /// Finding.
    pub fn location_read() -> krate_adapter_common::ui::LocationAnswer {
        use krate_adapter_common::ui::{LocationAnswer, Position};
        let Some(class) = location_class() else {
            return LocationAnswer::Unavailable;
        };
        if !location_available() {
            return LocationAnswer::Unavailable;
        }
        if !LOCATION_ASKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            DispatchQueue::main().exec_async(move || {
                // SAFETY: CLLocationManager's documented setup on the main
                // thread: make it, ask for "while using", start.
                unsafe {
                    let manager: Option<Retained<objc2::runtime::AnyObject>> =
                        msg_send![class, new];
                    let Some(manager) = manager else {
                        return;
                    };
                    let best: f64 = -1.0; // kCLLocationAccuracyBest
                    let _: () = msg_send![&*manager, setDesiredAccuracy: best];
                    let _: () = msg_send![&*manager, requestWhenInUseAuthorization];
                    let _: () = msg_send![&*manager, startUpdatingLocation];
                    *LOCATION.lock().expect("location lock") = Some(LocationManager(manager));
                }
            });
            return LocationAnswer::Finding;
        }
        let slot = LOCATION.lock().expect("location lock");
        let Some(manager) = slot.as_ref() else {
            return LocationAnswer::Finding;
        };
        // SAFETY: properties of a live manager and of the CLLocation it
        // holds; nil until the first fix.
        unsafe {
            let status: i32 = msg_send![&*manager.0, authorizationStatus];
            match status {
                // Restricted or denied.
                1 | 2 => return LocationAnswer::NotAllowed,
                // Not asked yet: the prompt is up.
                0 => return LocationAnswer::Finding,
                _ => {}
            }
            let fix: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*manager.0, location];
            let Some(fix) = fix else {
                return LocationAnswer::Finding;
            };
            let accuracy: f64 = msg_send![&*fix, horizontalAccuracy];
            if accuracy < 0.0 {
                return LocationAnswer::Finding;
            }
            let at: CoordinateC = msg_send![&*fix, coordinate];
            let altitude: f64 = msg_send![&*fix, altitude];
            let vertical: f64 = msg_send![&*fix, verticalAccuracy];
            let speed: f64 = msg_send![&*fix, speed];
            let course: f64 = msg_send![&*fix, course];
            let stamp: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*fix, timestamp];
            let age = stamp.map_or(0.0, |date| {
                let since: f64 = msg_send![&*date, timeIntervalSinceNow];
                -since
            });
            LocationAnswer::Here(Position {
                latitude: at.latitude,
                longitude: at.longitude,
                accuracy: accuracy as f32,
                altitude: (vertical > 0.0).then_some(altitude as f32),
                speed: (speed >= 0.0).then_some(speed as f32),
                course: (course >= 0.0).then_some(course as f32),
                age_ms: (age.max(0.0) * 1000.0) as u64,
            })
        }
    }

    // PhotosUI and Photos, through the Objective-C runtime.
    #[link(name = "PhotosUI", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Photos", kind = "framework")]
    unsafe extern "C" {}

    /// Where the photos picked are copied, and who waits for them.
    struct PhotoPick {
        tx: std::cell::RefCell<
            Option<std::sync::mpsc::Sender<Result<Vec<std::path::PathBuf>, String>>>,
        >,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a delegate.
        // - PhotosUI calls a picker's delegate on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = PhotoPick]
        struct KratePhotoDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KratePhotoDelegate {}

        // PHPickerViewControllerDelegate, by selector.
        impl KratePhotoDelegate {
            #[unsafe(method(picker:didFinishPicking:))]
            fn finished(&self, picker: &objc2::runtime::AnyObject, results: &objc2::runtime::AnyObject) {
                // SAFETY: dismiss the picker PhotosUI handed us.
                let _: () = unsafe { msg_send![picker, dismissViewControllerAnimated: true, completion: Option::<&objc2::runtime::AnyObject>::None] };
                let Some(tx) = self.ivars().tx.borrow_mut().take() else {
                    return;
                };
                PHOTO_DELEGATE.with(|slot| slot.borrow_mut().take());
                // SAFETY: an NSArray of PHPickerResult.
                let count: usize = unsafe { msg_send![results, count] };
                if count == 0 {
                    let _ = tx.send(Ok(Vec::new()));
                    return;
                }
                static PICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let dir = std::env::temp_dir().join("krate-photos").join(format!(
                    "{}-{}",
                    std::process::id(),
                    PICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                let _ = std::fs::create_dir_all(&dir);
                let gathered = std::sync::Arc::new(std::sync::Mutex::new((count, vec![None; count], Some(tx))));
                for index in 0..count {
                    // SAFETY: each result's itemProvider.
                    let provider: Retained<objc2::runtime::AnyObject> = unsafe {
                        let result: Retained<objc2::runtime::AnyObject> = msg_send![results, objectAtIndex: index];
                        msg_send![&*result, itemProvider]
                    };
                    load_photo(provider, index, dir.clone(), gathered.clone(), true);
                }
            }
        }
    );

    /// What a pick gathers: photos still to come, each one's copy, and who
    /// waits for the lot.
    type Gathered = std::sync::Arc<
        std::sync::Mutex<(
            usize,
            Vec<Option<std::path::PathBuf>>,
            Option<std::sync::mpsc::Sender<Result<Vec<std::path::PathBuf>, String>>>,
        )>,
    >;

    /// Copy one picked photo out: first as a JPEG (iOS converts a HEIC),
    /// and if that is refused, in its own format. The file iOS lends is
    /// gone once the block returns, so it is copied inside it. When the
    /// last one is in, the pick is answered: an error if none could be
    /// read -- never an empty list, which reads as a cancel.
    fn load_photo(
        provider: Retained<objc2::runtime::AnyObject>,
        index: usize,
        dir: std::path::PathBuf,
        gathered: Gathered,
        jpeg_first: bool,
    ) {
        let kind = if jpeg_first {
            "public.jpeg"
        } else {
            "public.image"
        };
        let retry = provider.clone();
        let got = block2::RcBlock::new(
            move |file: *mut objc2::runtime::AnyObject, _error: *mut objc2::runtime::AnyObject| {
                // SAFETY: the NSURL iOS lends for the duration of this block.
                let source: Option<String> = (!file.is_null())
                    .then(|| unsafe {
                        let path: Option<Retained<objc2_foundation::NSString>> =
                            msg_send![&*file, path];
                        path.map(|p| p.to_string())
                    })
                    .flatten();
                let Some(source) = source else {
                    if jpeg_first {
                        load_photo(retry.clone(), index, dir.clone(), gathered.clone(), false);
                        return;
                    }
                    finish_photo(&gathered, index, None);
                    return;
                };
                let extension = std::path::Path::new(&source)
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_else(|| "jpg".to_string());
                let to = dir.join(format!("photo-{}.{extension}", index + 1));
                let copied = std::fs::copy(&source, &to).ok().map(|_| to);
                finish_photo(&gathered, index, copied);
            },
        );
        let kind = objc2_foundation::NSString::from_str(kind);
        // SAFETY: NSItemProvider's loadFileRepresentationForTypeIdentifier:.
        let _: Option<Retained<objc2::runtime::AnyObject>> = unsafe {
            msg_send![&*provider, loadFileRepresentationForTypeIdentifier: &*kind, completionHandler: &*got]
        };
    }

    fn finish_photo(gathered: &Gathered, index: usize, copied: Option<std::path::PathBuf>) {
        let mut g = gathered.lock().expect("photos lock");
        g.1[index] = copied;
        g.0 -= 1;
        if g.0 == 0 {
            let picked: Vec<std::path::PathBuf> = g.1.iter().flatten().cloned().collect();
            if let Some(tx) = g.2.take() {
                let _ = tx.send(if picked.is_empty() {
                    Err("iOS would not hand over the photos picked".to_string())
                } else {
                    Ok(picked)
                });
            }
        }
    }

    thread_local! {
        static PHOTO_DELEGATE: std::cell::RefCell<Option<Retained<KratePhotoDelegate>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// The system's photo picker for up to `limit` photos (IC-920): the
    /// picks copied out as JPEGs, empty when cancelled. No permission: the
    /// picker runs outside Krate, and only what is picked comes back.
    pub fn pick_photos(limit: u32) -> Result<Vec<std::path::PathBuf>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let (Some(mtm), Some(controller)) = (MainThreadMarker::new(), main_controller()) else {
                let _ = tx.send(Err("no screen to pick from".to_string()));
                return;
            };
            let (Some(config_class), Some(picker_class), Some(filter_class)) = (
                objc2::runtime::AnyClass::get(c"PHPickerConfiguration"),
                objc2::runtime::AnyClass::get(c"PHPickerViewController"),
                objc2::runtime::AnyClass::get(c"PHPickerFilter"),
            ) else {
                let _ = tx.send(Err("this iOS has no photo picker".to_string()));
                return;
            };
            // SAFETY: a PHPickerConfiguration limited to images and to
            // `limit`, a picker over it, Krate's delegate held while it shows.
            unsafe {
                let config: Retained<objc2::runtime::AnyObject> = msg_send![config_class, new];
                let _: () = msg_send![&*config, setSelectionLimit: limit as isize];
                let images: Option<Retained<objc2::runtime::AnyObject>> = msg_send![filter_class, imagesFilter];
                if let Some(images) = images {
                    let _: () = msg_send![&*config, setFilter: &*images];
                }
                let made: objc2::rc::Allocated<objc2::runtime::AnyObject> = msg_send![picker_class, alloc];
                let picker: Option<Retained<objc2::runtime::AnyObject>> = msg_send![made, initWithConfiguration: &*config];
                let Some(picker) = picker else {
                    let _ = tx.send(Err("the photo picker would not open".to_string()));
                    return;
                };
                let delegate: Retained<KratePhotoDelegate> = {
                    let this = KratePhotoDelegate::alloc(mtm).set_ivars(PhotoPick {
                        tx: std::cell::RefCell::new(Some(tx)),
                    });
                    msg_send![super(this), init]
                };
                let _: () = msg_send![&*picker, setDelegate: &*delegate];
                PHOTO_DELEGATE.with(|slot| *slot.borrow_mut() = Some(delegate));
                let _: () = msg_send![&*controller, presentViewController: &*picker, animated: true, completion: Option::<&objc2::runtime::AnyObject>::None];
            }
        });
        rx.recv_timeout(std::time::Duration::from_secs(900))
            .unwrap_or_else(|_| Ok(Vec::new()))
    }

    /// Add a PNG or JPEG to Photos (IC-920): add-only access asked first
    /// (the phone's own prompt), then one asset created from the picture.
    pub fn save_photo(data: &[u8]) -> Result<(), Option<String>> {
        let (Some(library), Some(image_class), Some(request_class)) = (
            objc2::runtime::AnyClass::get(c"PHPhotoLibrary"),
            objc2::runtime::AnyClass::get(c"UIImage"),
            objc2::runtime::AnyClass::get(c"PHAssetChangeRequest"),
        ) else {
            return Err(Some("this iOS has no photo library".to_string()));
        };
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), Option<String>>>();
        let bytes = data.to_vec();
        // SAFETY: requestAuthorizationForAccessLevel: with add-only (1); its
        // handler, on success, decodes the picture as a UIImage and creates
        // an asset from it in performChanges:, whose completion answers.
        unsafe {
            let tx_auth = tx.clone();
            let asked = block2::RcBlock::new(move |status: isize| {
                // Authorized (3) or limited (4).
                if status != 3 && status != 4 {
                    let _ = tx_auth.send(Err(None));
                    return;
                }
                let Some(data_class) = objc2::runtime::AnyClass::get(c"NSData") else {
                    let _ = tx_auth.send(Err(Some("no NSData".to_string())));
                    return;
                };
                let ns_data: Option<Retained<objc2::runtime::AnyObject>> = msg_send![data_class, dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void, length: bytes.len()];
                let image: Option<Retained<objc2::runtime::AnyObject>> =
                    ns_data.and_then(|d| msg_send![image_class, imageWithData: &*d]);
                let Some(image) = image else {
                    let _ = tx_auth.send(Err(Some("iOS could not read that picture".to_string())));
                    return;
                };
                let change = block2::RcBlock::new(move || {
                    let _: Option<Retained<objc2::runtime::AnyObject>> =
                        msg_send![request_class, creationRequestForAssetFromImage: &*image];
                });
                let tx_done = tx_auth.clone();
                let done = block2::RcBlock::new(
                    move |ok: objc2::runtime::Bool, error: *mut objc2::runtime::AnyObject| {
                        let _ = tx_done.send(if ok.as_bool() {
                            Ok(())
                        } else if error.is_null() {
                            Err(Some("Photos refused the picture".to_string()))
                        } else {
                            let text: Option<Retained<objc2_foundation::NSString>> =
                                msg_send![&*error, localizedDescription];
                            Err(Some(text.map(|t| t.to_string()).unwrap_or_default()))
                        });
                    },
                );
                let shared: Retained<objc2::runtime::AnyObject> =
                    msg_send![library, sharedPhotoLibrary];
                let _: () =
                    msg_send![&*shared, performChanges: &*change, completionHandler: &*done];
            });
            let _: () =
                msg_send![library, requestAuthorizationForAccessLevel: 1isize, handler: &*asked];
        }
        rx.recv_timeout(std::time::Duration::from_secs(120))
            .unwrap_or(Err(Some("Photos did not answer".to_string())))
    }

    /// The system's share sheet with a text, a link and files (IC-919). Files
    /// are written to a fresh folder in the temporary space first: the sheet
    /// shares URLs, not bytes. Waits for the person: true when they sent it.
    pub fn share(
        text: Option<&str>,
        url: Option<&str>,
        files: &[(String, Vec<u8>)],
    ) -> Result<bool, String> {
        static SHARES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join("krate-share").join(format!(
            "{}-{}",
            std::process::id(),
            SHARES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut paths = Vec::new();
        if !files.is_empty() {
            std::fs::create_dir_all(&dir).map_err(|e| format!("could not stage the files: {e}"))?;
            for (name, data) in files {
                let path = dir.join(name);
                std::fs::write(&path, data).map_err(|e| format!("could not stage {name}: {e}"))?;
                paths.push(path);
            }
        }
        let (text, url) = (text.map(str::to_string), url.map(str::to_string));
        let staged = !paths.is_empty();
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let (Some(_mtm), Some(controller)) = (MainThreadMarker::new(), main_controller()) else {
                let _ = tx.send(Err("no screen to share from".to_string()));
                return;
            };
            let (Some(sheet_class), Some(url_class)) = (
                objc2::runtime::AnyClass::get(c"UIActivityViewController"),
                objc2::runtime::AnyClass::get(c"NSURL"),
            ) else {
                let _ = tx.send(Err("this iOS has no share sheet".to_string()));
                return;
            };
            // SAFETY: the items as NSString and NSURL objects in an NSArray,
            // a UIActivityViewController over them, its completion block
            // answering on the channel, presented from Krate's controller.
            unsafe {
                let mut items: Vec<Retained<objc2::runtime::AnyObject>> = Vec::new();
                if let Some(text) = text.filter(|t| !t.is_empty()) {
                    items.push(Retained::cast_unchecked(objc2_foundation::NSString::from_str(&text)));
                }
                if let Some(url) = url.filter(|u| !u.is_empty()) {
                    let link: Option<Retained<objc2::runtime::AnyObject>> =
                        msg_send![url_class, URLWithString: &*objc2_foundation::NSString::from_str(&url)];
                    if let Some(link) = link {
                        items.push(link);
                    }
                }
                for path in &paths {
                    let file: Option<Retained<objc2::runtime::AnyObject>> = msg_send![
                        url_class,
                        fileURLWithPath: &*objc2_foundation::NSString::from_str(&path.to_string_lossy())
                    ];
                    if let Some(file) = file {
                        items.push(file);
                    }
                }
                let items = objc2_foundation::NSArray::from_retained_slice(&items);
                let made: objc2::rc::Allocated<objc2::runtime::AnyObject> = msg_send![sheet_class, alloc];
                let sheet: Option<Retained<objc2::runtime::AnyObject>> = msg_send![
                    made,
                    initWithActivityItems: &*items,
                    applicationActivities: Option::<&objc2::runtime::AnyObject>::None
                ];
                let Some(sheet) = sheet else {
                    let _ = tx.send(Err("the share sheet would not open".to_string()));
                    return;
                };
                let tx = std::sync::Mutex::new(Some(tx));
                let done = block2::RcBlock::new(
                    move |_kind: *mut objc2::runtime::AnyObject,
                          completed: objc2::runtime::Bool,
                          _returned: *mut objc2::runtime::AnyObject,
                          _error: *mut objc2::runtime::AnyObject| {
                        if let Some(tx) = tx.lock().expect("share lock").take() {
                            let _ = tx.send(Ok(completed.as_bool()));
                        }
                    },
                );
                let _: () = msg_send![&*sheet, setCompletionWithItemsHandler: &*done];
                let _: () = msg_send![&*controller, presentViewController: &*sheet, animated: true, completion: Option::<&objc2::runtime::AnyObject>::None];
            }
        });
        let answer = rx
            .recv_timeout(std::time::Duration::from_secs(600))
            .unwrap_or(Ok(false));
        if staged {
            let _ = std::fs::remove_dir_all(&dir);
        }
        answer
    }

    /// The person's text size against the default (Large, 17 points), from
    /// UIKit's content size category: Apple's own body sizes, 14 to 53.
    pub(crate) fn text_scale_for(category: &str) -> f32 {
        let body = match category.trim_start_matches("UICTContentSizeCategory") {
            "XS" => 14.0,
            "S" => 15.0,
            "M" => 16.0,
            "XL" => 19.0,
            "XXL" => 21.0,
            "XXXL" => 23.0,
            "AccessibilityM" => 28.0,
            "AccessibilityL" => 33.0,
            "AccessibilityXL" => 40.0,
            "AccessibilityXXL" => 47.0,
            "AccessibilityXXXL" => 53.0,
            _ => 17.0,
        };
        body / 17.0
    }

    /// The look the main thread read last, for the app's thread to take
    /// without waiting.
    static LOOK: std::sync::Mutex<Option<krate_adapter_common::ui::Appearance>> =
        std::sync::Mutex::new(None);

    /// How the person asked things to look (IC-925): dark mode, more
    /// contrast and text size, from Krate's own view controller's traits.
    ///
    /// Never waits. The traits are read on the main thread, which may itself
    /// be waiting for this app's frame; an app thread that waited for it
    /// stalled half a second on every read, and a widget app never drew its
    /// first frame (Timer, 2026-10-11). So this answers the last look the
    /// main thread saw and asks it to look again.
    pub fn appearance() -> krate_adapter_common::ui::Appearance {
        DispatchQueue::main().exec_async(|| {
            let look = main_controller().map(|controller| {
                // SAFETY: the controller's UITraitCollection and three of its
                // documented properties, on the main thread.
                unsafe {
                    let traits: Retained<objc2::runtime::AnyObject> =
                        msg_send![&*controller, traitCollection];
                    let style: isize = msg_send![&*traits, userInterfaceStyle];
                    let contrast: isize = msg_send![&*traits, accessibilityContrast];
                    let category: Option<Retained<objc2_foundation::NSString>> =
                        msg_send![&*traits, preferredContentSizeCategory];
                    krate_adapter_common::ui::Appearance {
                        dark: style == 2,
                        high_contrast: contrast == 1,
                        text_scale: category.map_or(1.0, |c| text_scale_for(&c.to_string())),
                    }
                }
            });
            if let Some(look) = look {
                *LOOK.lock().expect("look lock") = Some(look);
            }
        });
        LOOK.lock().expect("look lock").unwrap_or_default()
    }

    // Signing in through the system's sheet (IC-923).
    #[link(name = "AuthenticationServices", kind = "framework")]
    unsafe extern "C" {}

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a presentation
        //   context provider.
        // - AuthenticationServices asks it on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        struct KrateSignInAnchor;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateSignInAnchor {}

        // ASWebAuthenticationPresentationContextProviding, by selector.
        impl KrateSignInAnchor {
            /// The window the sheet rises over: Krate's own.
            #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
            fn anchor(&self, _session: &objc2::runtime::AnyObject) -> Option<Retained<objc2::runtime::AnyObject>> {
                let controller = main_controller();
                // SAFETY: the controller's view and that view's window.
                controller.and_then(|controller| unsafe {
                    let view: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*controller, view];
                    view.and_then(|view| msg_send![&*view, window])
                })
            }
        }
    );

    thread_local! {
        /// The sheet that is up and its anchor, kept alive until it answers.
        static SIGN_IN: RefCell<Option<(Retained<objc2::runtime::AnyObject>, Retained<KrateSignInAnchor>)>> =
            const { RefCell::new(None) };
    }

    /// Open the sign-in sheet at `url` and wait for the service to send the
    /// person back to `scheme:`; that address, or None when they closed it.
    pub fn sign_in(url: &str, scheme: &str) -> Result<Option<String>, String> {
        let (url, scheme) = (url.to_string(), scheme.to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = tx.send(Err("not on the main thread".to_string()));
                return;
            };
            let (Some(class), Some(url_class)) = (
                objc2::runtime::AnyClass::get(c"ASWebAuthenticationSession"),
                objc2::runtime::AnyClass::get(c"NSURL"),
            ) else {
                let _ = tx.send(Err("this iOS has no sign-in sheet".to_string()));
                return;
            };
            if SIGN_IN.with(|up| up.borrow().is_some()) {
                let _ = tx.send(Err("a sign-in sheet is already open".to_string()));
                return;
            }
            // SAFETY: an ASWebAuthenticationSession over an NSURL with a
            // completion block answering on the channel, given Krate's
            // window as its anchor, started on the main thread, and kept
            // alive in SIGN_IN until the block runs.
            unsafe {
                let link: Option<Retained<objc2::runtime::AnyObject>> =
                    msg_send![url_class, URLWithString: &*objc2_foundation::NSString::from_str(&url)];
                let Some(link) = link else {
                    let _ = tx.send(Err("iOS could not read the sign-in address".to_string()));
                    return;
                };
                let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
                let fail = {
                    let tx = tx.clone();
                    move |why: &str| {
                        if let Some(tx) = tx.lock().expect("sign-in lock").take() {
                            let _ = tx.send(Err(why.to_string()));
                        }
                    }
                };
                let done = block2::RcBlock::new(
                    move |back: *mut objc2::runtime::AnyObject, error: *mut objc2::runtime::AnyObject| {
                        let answer = if let Some(back) = back.as_ref() {
                            let text: Retained<objc2_foundation::NSString> = msg_send![back, absoluteString];
                            Ok(Some(text.to_string()))
                        } else if let Some(error) = error.as_ref() {
                            // ASWebAuthenticationSessionErrorCodeCanceledLogin.
                            let code: isize = msg_send![error, code];
                            if code == 1 {
                                Ok(None)
                            } else {
                                let why: Retained<objc2_foundation::NSString> = msg_send![error, localizedDescription];
                                Err(why.to_string())
                            }
                        } else {
                            Ok(None)
                        };
                        if let Some(tx) = tx.lock().expect("sign-in lock").take() {
                            let _ = tx.send(answer);
                        }
                        SIGN_IN.with(|up| up.borrow_mut().take());
                    },
                );
                let made: objc2::rc::Allocated<objc2::runtime::AnyObject> = msg_send![class, alloc];
                let session: Option<Retained<objc2::runtime::AnyObject>> = msg_send![
                    made,
                    initWithURL: &*link,
                    callbackURLScheme: &*objc2_foundation::NSString::from_str(&scheme),
                    completionHandler: &*done
                ];
                let Some(session) = session else {
                    fail("the sign-in sheet would not open");
                    return;
                };
                let anchor = KrateSignInAnchor::alloc(mtm).set_ivars(());
                let anchor: Retained<KrateSignInAnchor> = msg_send![super(anchor), init];
                if let Some(protocol) = objc2::runtime::AnyProtocol::get(c"ASWebAuthenticationPresentationContextProviding") {
                    objc2::ffi::class_addProtocol(
                        <KrateSignInAnchor as objc2::ClassType>::class() as *const objc2::runtime::AnyClass as *mut objc2::runtime::AnyClass,
                        protocol,
                    );
                }
                let _: () = msg_send![&*session, setPresentationContextProvider: &*anchor];
                SIGN_IN.with(|up| *up.borrow_mut() = Some((session.clone(), anchor)));
                let started: bool = msg_send![&*session, start];
                if !started {
                    SIGN_IN.with(|up| up.borrow_mut().take());
                    fail("the sign-in sheet would not open");
                }
            }
        });
        // The person may take minutes: a password manager, a two-factor
        // code. Fifteen is the most the sheet waits.
        rx.recv_timeout(std::time::Duration::from_secs(900))
            .unwrap_or(Ok(None))
    }

    /// Open a link that has passed the runtime's checks (IC-918): iOS hands
    /// it to Safari, Mail or the app that owns it.
    pub fn open_url(url: &str) -> Result<(), String> {
        let text = url.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        DispatchQueue::main().exec_async(move || {
            let answer = (|| {
                let class = objc2::runtime::AnyClass::get(c"NSURL").ok_or("no NSURL")?;
                // SAFETY: +[NSURL URLWithString:] (nil for a malformed one),
                // then UIApplication's openURL:options:completionHandler:.
                unsafe {
                    let link: Option<Retained<objc2::runtime::AnyObject>> =
                        msg_send![class, URLWithString: &*objc2_foundation::NSString::from_str(&text)];
                    let link = link.ok_or("iOS could not read that link")?;
                    let app = UIApplication::sharedApplication(MainThreadMarker::new().ok_or("not on the main thread")?);
                    let options = objc2_foundation::NSDictionary::<objc2::runtime::AnyObject, objc2::runtime::AnyObject>::new();
                    let _: () = msg_send![&*app, openURL: &*link, options: &*options, completionHandler: Option::<&objc2::runtime::AnyObject>::None];
                }
                Ok::<(), &'static str>(())
            })();
            let _ = tx.send(answer.map_err(str::to_string));
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| Err("iOS did not answer".to_string()))
    }

    // UserNotifications, through the Objective-C runtime.
    #[link(name = "UserNotifications", kind = "framework")]
    unsafe extern "C" {}

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for a delegate.
        // - UserNotifications calls it on the main thread.
        #[unsafe(super = objc2_foundation::NSObject)]
        #[thread_kind = MainThreadOnly]
        struct KrateNoticeDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateNoticeDelegate {}

        // UNUserNotificationCenterDelegate, by selector.
        impl KrateNoticeDelegate {
            /// A reminder tapped: open the app that set it (IC-921). Its
            /// userInfo names the app; Krate's player opens it next.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &objc2::runtime::AnyObject,
                response: &objc2::runtime::AnyObject,
                done: &block2::DynBlock<dyn Fn()>,
            ) {
                // SAFETY: the response's notification, request, content and
                // userInfo, read for one string Krate itself wrote.
                let opens: Option<String> = unsafe {
                    let notification: Retained<objc2::runtime::AnyObject> = msg_send![response, notification];
                    let request: Retained<objc2::runtime::AnyObject> = msg_send![&*notification, request];
                    let content: Retained<objc2::runtime::AnyObject> = msg_send![&*request, content];
                    let info: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*content, userInfo];
                    info.and_then(|info| {
                        let value: Option<Retained<objc2_foundation::NSString>> =
                            msg_send![&*info, objectForKey: &*objc2_foundation::NSString::from_str("krate-open")];
                        value.map(|v| v.to_string())
                    })
                };
                if let Some(opens) = opens {
                    *NOTICE_OPEN.lock().expect("notice lock") = Some(opens);
                    if guest_initialized() {
                        close_for_opened_file();
                    }
                }
                done.call(());
            }

            /// Show an app's notification while Krate is in front: by
            /// default iOS shows nothing for the app on screen, and the
            /// app on screen is the one posting.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &objc2::runtime::AnyObject,
                _notification: &objc2::runtime::AnyObject,
                done: &block2::DynBlock<dyn Fn(usize)>,
            ) {
                // Banner (16) | list (8) | sound (2).
                done.call((16 | 8 | 2,));
            }
        }
    );

    thread_local! {
        static NOTICE_DELEGATE: std::cell::OnceCell<Retained<KrateNoticeDelegate>> =
            const { std::cell::OnceCell::new() };
    }

    /// The app a tapped reminder asks to open, waiting for the player.
    static NOTICE_OPEN: Mutex<Option<String>> = Mutex::new(None);
    /// The running app: its id (reminders are namespaced by it) and what
    /// the player calls it, to open it again from a reminder.
    static CURRENT_APP: Mutex<Option<(String, String)>> = Mutex::new(None);

    /// The player says which app is running, or none (IC-921).
    pub fn set_current_app(app: Option<(String, String)>) {
        *CURRENT_APP.lock().expect("app lock") = app;
    }

    /// The app a tapped reminder asked to open, once.
    pub fn take_notice_open() -> Option<String> {
        NOTICE_OPEN.lock().expect("notice lock").take()
    }

    /// Be the notification center's delegate from launch, so a reminder
    /// tapped while Krate was closed still reaches it. Main thread.
    pub fn install_notice_delegate() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(center_class) = objc2::runtime::AnyClass::get(c"UNUserNotificationCenter") else {
            return;
        };
        // SAFETY: the current center and Krate's delegate, held for the life
        // of the process.
        unsafe {
            let center: Retained<objc2::runtime::AnyObject> =
                msg_send![center_class, currentNotificationCenter];
            NOTICE_DELEGATE.with(|cell| {
                let delegate = cell.get_or_init(|| {
                    let this = KrateNoticeDelegate::alloc(mtm).set_ivars(());
                    msg_send![super(this), init]
                });
                let _: () = msg_send![&*center, setDelegate: &**delegate];
            });
        }
    }

    fn reminder_prefix() -> Option<(String, String)> {
        CURRENT_APP
            .lock()
            .expect("app lock")
            .clone()
            .map(|(id, opens)| (format!("krate.{id}."), opens))
    }

    /// Schedule a reminder for the running app (IC-921): identifier
    /// `krate.<app id>.<id>`, so apps never meet; delivered by iOS with
    /// the app closed; `daily` repeats at that time of day.
    pub fn reminder_schedule(
        sender: &str,
        id: &str,
        title: &str,
        body: &str,
        after: u64,
        daily: bool,
    ) -> Result<(), String> {
        let (prefix, opens) = reminder_prefix().ok_or("no app is running")?;
        let (sender, title, body) = (sender.to_string(), title.to_string(), body.to_string());
        let full = format!("{prefix}{id}");
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        DispatchQueue::main().exec_async(move || {
            install_notice_delegate();
            let classes = (
                objc2::runtime::AnyClass::get(c"UNUserNotificationCenter"),
                objc2::runtime::AnyClass::get(c"UNMutableNotificationContent"),
                objc2::runtime::AnyClass::get(c"UNNotificationRequest"),
                objc2::runtime::AnyClass::get(c"UNTimeIntervalNotificationTrigger"),
                objc2::runtime::AnyClass::get(c"UNCalendarNotificationTrigger"),
                objc2::runtime::AnyClass::get(c"NSDateComponents"),
                objc2::runtime::AnyClass::get(c"NSDictionary"),
            );
            let (Some(center_class), Some(content_class), Some(request_class), Some(interval_class), Some(calendar_class), Some(components_class), Some(dict_class)) = classes else {
                let _ = tx.send(Err("this iOS has no reminders".to_string()));
                return;
            };
            // SAFETY: authorization, then a content, a trigger (a time
            // interval, or the time of day for a daily one) and a request
            // added to the current center; the completion answers.
            unsafe {
                let center: Retained<objc2::runtime::AnyObject> = msg_send![center_class, currentNotificationCenter];
                let center_for_add = center.clone();
                let asked = block2::RcBlock::new(move |ok: objc2::runtime::Bool, _error: *mut objc2::runtime::AnyObject| {
                    if !ok.as_bool() {
                        let _ = tx.send(Err("notifications are switched off for Krate: Settings > Krate > Notifications".to_string()));
                        return;
                    }
                    let ns = |t: &str| objc2_foundation::NSString::from_str(t);
                    let content: Retained<objc2::runtime::AnyObject> = msg_send![content_class, new];
                    let _: () = msg_send![&*content, setTitle: &*ns(&sender)];
                    let _: () = msg_send![&*content, setSubtitle: &*ns(&title)];
                    let _: () = msg_send![&*content, setBody: &*ns(&body)];
                    let info: Retained<objc2::runtime::AnyObject> =
                        msg_send![dict_class, dictionaryWithObject: &*ns(&opens), forKey: &*ns("krate-open")];
                    let _: () = msg_send![&*content, setUserInfo: &*info];
                    let sound: Option<Retained<objc2::runtime::AnyObject>> = objc2::runtime::AnyClass::get(c"UNNotificationSound")
                        .and_then(|class| msg_send![class, defaultSound]);
                    if let Some(sound) = sound {
                        let _: () = msg_send![&*content, setSound: &*sound];
                    }
                    let trigger: Retained<objc2::runtime::AnyObject> = if daily {
                        // The time of day it first comes, every day.
                        let at = std::time::SystemTime::now() + std::time::Duration::from_secs(after);
                        let date_class = objc2::runtime::AnyClass::get(c"NSDate").expect("NSDate");
                        let calendar_cls = objc2::runtime::AnyClass::get(c"NSCalendar").expect("NSCalendar");
                        let secs = at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                        let date: Retained<objc2::runtime::AnyObject> = msg_send![date_class, dateWithTimeIntervalSince1970: secs];
                        let calendar: Retained<objc2::runtime::AnyObject> = msg_send![calendar_cls, currentCalendar];
                        // Hour (32) | minute (64) | second (128).
                        let parts: Retained<objc2::runtime::AnyObject> =
                            msg_send![&*calendar, components: (32usize | 64 | 128), fromDate: &*date];
                        let _ = components_class;
                        msg_send![calendar_class, triggerWithDateMatchingComponents: &*parts, repeats: true]
                    } else {
                        msg_send![interval_class, triggerWithTimeInterval: after as f64, repeats: false]
                    };
                    let request: Retained<objc2::runtime::AnyObject> = msg_send![
                        request_class,
                        requestWithIdentifier: &*ns(&full),
                        content: &*content,
                        trigger: &*trigger
                    ];
                    let tx_add = tx.clone();
                    let added = block2::RcBlock::new(move |error: *mut objc2::runtime::AnyObject| {
                        let _ = tx_add.send(if error.is_null() {
                            Ok(())
                        } else {
                            Err("iOS refused the reminder".to_string())
                        });
                    });
                    let _: () = msg_send![&*center_for_add, addNotificationRequest: &*request, withCompletionHandler: &*added];
                });
                let _: () = msg_send![&*center, requestAuthorizationWithOptions: 7usize, completionHandler: &*asked];
            }
        });
        rx.recv_timeout(std::time::Duration::from_secs(120))
            .unwrap_or(Err("iOS did not answer".to_string()))
    }

    /// Take back one of the running app's reminders.
    pub fn reminder_cancel(id: &str) {
        let Some((prefix, _)) = reminder_prefix() else {
            return;
        };
        let full = format!("{prefix}{id}");
        DispatchQueue::main().exec_async(move || {
            let Some(center_class) = objc2::runtime::AnyClass::get(c"UNUserNotificationCenter")
            else {
                return;
            };
            // SAFETY: remove one pending request by its identifier.
            unsafe {
                let center: Retained<objc2::runtime::AnyObject> =
                    msg_send![center_class, currentNotificationCenter];
                let ids = objc2_foundation::NSArray::from_retained_slice(&[
                    objc2_foundation::NSString::from_str(&full),
                ]);
                let _: () =
                    msg_send![&*center, removePendingNotificationRequestsWithIdentifiers: &*ids];
            }
        });
    }

    /// The running app's reminders still to come, by the app's own ids.
    pub fn reminder_pending() -> Vec<String> {
        let Some((prefix, _)) = reminder_prefix() else {
            return Vec::new();
        };
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        DispatchQueue::main().exec_async(move || {
            let Some(center_class) = objc2::runtime::AnyClass::get(c"UNUserNotificationCenter")
            else {
                let _ = tx.send(Vec::new());
                return;
            };
            // SAFETY: the pending requests, read for their identifiers.
            unsafe {
                let center: Retained<objc2::runtime::AnyObject> =
                    msg_send![center_class, currentNotificationCenter];
                let got = block2::RcBlock::new(move |requests: *mut objc2::runtime::AnyObject| {
                    let mut ids = Vec::new();
                    if !requests.is_null() {
                        let count: usize = msg_send![&*requests, count];
                        for i in 0..count {
                            let request: Retained<objc2::runtime::AnyObject> =
                                msg_send![&*requests, objectAtIndex: i];
                            let id: Option<Retained<objc2_foundation::NSString>> =
                                msg_send![&*request, identifier];
                            if let Some(rest) = id
                                .map(|s| s.to_string())
                                .and_then(|s| s.strip_prefix(&prefix).map(str::to_string))
                            {
                                ids.push(rest);
                            }
                        }
                    }
                    let _ = tx.send(ids);
                });
                let _: () =
                    msg_send![&*center, getPendingNotificationRequestsWithCompletionHandler: &*got];
            }
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_default()
    }

    /// Post a notification now (IC-918): `sender` -- the app's consented
    /// name, never its own words -- on the title line, the app's title
    /// under it. iOS asks the person once whether Krate may notify.
    pub fn notify(sender: &str, title: &str, body: &str) -> Result<(), String> {
        let (sender, title, body) = (sender.to_string(), title.to_string(), body.to_string());
        DispatchQueue::main().exec_async(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some(center_class) = objc2::runtime::AnyClass::get(c"UNUserNotificationCenter") else {
                return;
            };
            // SAFETY: the current notification center, its delegate held by
            // Krate (it holds delegates weakly), and an authorization request
            // whose completion posts the notification.
            unsafe {
                let center: Retained<objc2::runtime::AnyObject> = msg_send![center_class, currentNotificationCenter];
                NOTICE_DELEGATE.with(|cell| {
                    let delegate = cell.get_or_init(|| {
                        let this = KrateNoticeDelegate::alloc(mtm).set_ivars(());
                        msg_send![super(this), init]
                    });
                    let _: () = msg_send![&*center, setDelegate: &**delegate];
                });
                let center_for_post = center.clone();
                let granted = block2::RcBlock::new(move |ok: objc2::runtime::Bool, _error: *mut objc2::runtime::AnyObject| {
                    if !ok.as_bool() {
                        return;
                    }
                    let (Some(content_class), Some(request_class)) = (
                        objc2::runtime::AnyClass::get(c"UNMutableNotificationContent"),
                        objc2::runtime::AnyClass::get(c"UNNotificationRequest"),
                    ) else {
                        return;
                    };
                    let ns = |t: &str| objc2_foundation::NSString::from_str(t);
                    let content: Retained<objc2::runtime::AnyObject> = msg_send![content_class, new];
                    let _: () = msg_send![&*content, setTitle: &*ns(&sender)];
                    let _: () = msg_send![&*content, setSubtitle: &*ns(&title)];
                    let _: () = msg_send![&*content, setBody: &*ns(&body)];
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let id = format!("krate-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                    let request: Retained<objc2::runtime::AnyObject> = msg_send![
                        request_class,
                        requestWithIdentifier: &*ns(&id),
                        content: &*content,
                        trigger: Option::<&objc2::runtime::AnyObject>::None
                    ];
                    let _: () = msg_send![
                        &*center_for_post,
                        addNotificationRequest: &*request,
                        withCompletionHandler: Option::<&objc2::runtime::AnyObject>::None
                    ];
                });
                // Alert (4) | sound (2) | badge (1).
                let _: () = msg_send![&*center, requestAuthorizationWithOptions: 7usize, completionHandler: &*granted];
            }
        });
        Ok(())
    }

    // Speech, through the Objective-C runtime.
    #[link(name = "Speech", kind = "framework")]
    unsafe extern "C" {}

    /// Speech to text on the phone itself (IC-918): SFSpeechRecognizer over
    /// the app's mono 16-bit PCM, on the device when it can, asking the
    /// person once. `language` is a BCP 47 tag ("en", "en-US"); none is
    /// the phone's own language.
    pub fn transcribe(pcm: &[u8], rate: u32, language: Option<&str>) -> Result<String, String> {
        let get = |name: &std::ffi::CStr| {
            objc2::runtime::AnyClass::get(name)
                .ok_or_else(|| format!("no {}", name.to_string_lossy()))
        };
        let recognizer_class = get(c"SFSpeechRecognizer")?;
        // Asked once; the answer is waited for.
        let (tx, rx) = std::sync::mpsc::channel::<isize>();
        let asked = block2::RcBlock::new(move |status: isize| {
            let _ = tx.send(status);
        });
        // SAFETY: +[SFSpeechRecognizer requestAuthorization:] with a block
        // that reports the status.
        unsafe {
            let _: () = msg_send![recognizer_class, requestAuthorization: &*asked];
        }
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            // Authorized.
            Ok(3) => {}
            Ok(_) => {
                return Err(
                    "speech recognition is not allowed for Krate: Settings > Krate".to_string(),
                )
            }
            Err(_) => return Err("the phone did not answer".to_string()),
        }
        let samples: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let (format_class, buffer_class, request_class, locale_class) = (
            get(c"AVAudioFormat")?,
            get(c"AVAudioPCMBuffer")?,
            get(c"SFSpeechAudioBufferRecognitionRequest")?,
            get(c"NSLocale")?,
        );
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        // SAFETY: a recognizer for the language, one PCM buffer of the
        // app's samples (int16, mono, interleaved), a buffer request ended
        // at once, and a task whose handler answers on the final result.
        unsafe {
            let recognizer: Option<Retained<objc2::runtime::AnyObject>> = match language {
                Some(tag) => {
                    let locale: Retained<objc2::runtime::AnyObject> = msg_send![locale_class, localeWithLocaleIdentifier: &*objc2_foundation::NSString::from_str(tag)];
                    let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                        msg_send![recognizer_class, alloc];
                    msg_send![made, initWithLocale: &*locale]
                }
                None => msg_send![recognizer_class, new],
            };
            let recognizer = recognizer.ok_or("no recognizer for that language")?;
            let available: bool = msg_send![&*recognizer, isAvailable];
            if !available {
                return Err("speech recognition is not available right now".to_string());
            }
            let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                msg_send![format_class, alloc];
            // AVAudioPCMFormatInt16 = 3.
            let format: Option<Retained<objc2::runtime::AnyObject>> = msg_send![made, initWithCommonFormat: 3usize, sampleRate: rate as f64, channels: 1u32, interleaved: true];
            let format = format.ok_or("that sample rate cannot be read")?;
            let made: objc2::rc::Allocated<objc2::runtime::AnyObject> =
                msg_send![buffer_class, alloc];
            let buffer: Option<Retained<objc2::runtime::AnyObject>> =
                msg_send![made, initWithPCMFormat: &*format, frameCapacity: samples.len() as u32];
            let buffer = buffer.ok_or("the audio would not fit")?;
            let channels: *const *mut i16 = msg_send![&*buffer, int16ChannelData];
            if channels.is_null() || (*channels).is_null() {
                return Err("the audio buffer has no room".to_string());
            }
            std::ptr::copy_nonoverlapping(samples.as_ptr(), *channels, samples.len());
            let _: () = msg_send![&*buffer, setFrameLength: samples.len() as u32];
            let request: Retained<objc2::runtime::AnyObject> = msg_send![request_class, new];
            let _: () = msg_send![&*request, setShouldReportPartialResults: false];
            let on_device: bool = msg_send![&*recognizer, supportsOnDeviceRecognition];
            if on_device {
                let _: () = msg_send![&*request, setRequiresOnDeviceRecognition: true];
            }
            let _: () = msg_send![&*request, appendAudioPCMBuffer: &*buffer];
            let _: () = msg_send![&*request, endAudio];
            let tx = std::sync::Mutex::new(Some(tx));
            let heard = block2::RcBlock::new(
                move |result: *mut objc2::runtime::AnyObject,
                      error: *mut objc2::runtime::AnyObject| {
                    let answer = if !result.is_null() {
                        let done: bool = msg_send![&*result, isFinal];
                        if !done {
                            return;
                        }
                        let best: Retained<objc2::runtime::AnyObject> =
                            msg_send![&*result, bestTranscription];
                        let text: Retained<objc2_foundation::NSString> =
                            msg_send![&*best, formattedString];
                        Ok(text.to_string())
                    } else if !error.is_null() {
                        let why: Option<Retained<objc2_foundation::NSString>> =
                            msg_send![&*error, localizedDescription];
                        let why = why.map(|w| w.to_string()).unwrap_or_default();
                        // No speech in the audio is an empty transcript, not a fault.
                        if why.to_ascii_lowercase().contains("no speech") {
                            Ok(String::new())
                        } else {
                            Err(why)
                        }
                    } else {
                        return;
                    };
                    if let Some(tx) = tx.lock().expect("speech lock").take() {
                        let _ = tx.send(answer);
                    }
                },
            );
            let task: Option<Retained<objc2::runtime::AnyObject>> = msg_send![&*recognizer, recognitionTaskWithRequest: &*request, resultHandler: &*heard];
            let _keep = (task, recognizer);
            rx.recv_timeout(std::time::Duration::from_secs(60))
                .unwrap_or_else(|_| Err("speech recognition took too long".to_string()))
        }
    }

    /// The controller Krate's own sheets are shown from.
    pub(crate) fn main_controller() -> Option<Retained<objc2_ui_kit::UIViewController>> {
        MAIN_SIDE
            .0
            .get()
            .map(|main_side| Retained::into_super(main_side.controller.clone()))
    }

    /// The app closed: stop looking, so the location arrow goes away.
    fn stop_location() {
        LOCATION_ASKED.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(manager) = LOCATION.lock().expect("location lock").take() {
            DispatchQueue::main().exec_async(move || {
                // SAFETY: stopping the manager that started, on its thread.
                let _: () = unsafe { msg_send![&*manager.0, stopUpdatingLocation] };
                drop(manager);
            });
        }
    }

    /// The phone's own feedback generators, made once and kept warm on the
    /// main thread, as Apple asks: a fresh one per tap starts late.
    struct Haptics {
        light: Retained<objc2_ui_kit::UIImpactFeedbackGenerator>,
        medium: Retained<objc2_ui_kit::UIImpactFeedbackGenerator>,
        heavy: Retained<objc2_ui_kit::UIImpactFeedbackGenerator>,
        selection: Retained<objc2_ui_kit::UISelectionFeedbackGenerator>,
        notice: Retained<objc2_ui_kit::UINotificationFeedbackGenerator>,
    }

    thread_local! {
        static HAPTICS: std::cell::OnceCell<Haptics> = const { std::cell::OnceCell::new() };
    }

    /// When the last tap went out: taps closer than the Taptic Engine can
    /// play are dropped, so a game may ask every frame.
    static LAST_TAP: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    /// Tap back (IC-917): `feel`, or an impact at `strength` (0 to 1) when
    /// `feel` is None.
    pub fn haptic(feel: Option<krate_adapter_common::ui::HapticFeel>, strength: f32) -> bool {
        {
            let mut last = LAST_TAP.lock().expect("tap lock");
            let now = std::time::Instant::now();
            if last.is_some_and(|at| now.duration_since(at) < std::time::Duration::from_millis(30))
            {
                return true;
            }
            *last = Some(now);
        }
        DispatchQueue::main().exec_async(move || play_haptic(feel, strength));
        true
    }

    fn play_haptic(feel: Option<krate_adapter_common::ui::HapticFeel>, strength: f32) {
        use krate_adapter_common::ui::HapticFeel;
        use objc2_ui_kit::{
            UIImpactFeedbackGenerator, UIImpactFeedbackStyle, UINotificationFeedbackGenerator,
            UINotificationFeedbackType, UISelectionFeedbackGenerator,
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        HAPTICS.with(|cell| {
            let h = cell.get_or_init(|| {
                let impact = |style| {
                    UIImpactFeedbackGenerator::initWithStyle(
                        UIImpactFeedbackGenerator::alloc(mtm),
                        style,
                    )
                };
                Haptics {
                    light: impact(UIImpactFeedbackStyle::Light),
                    medium: impact(UIImpactFeedbackStyle::Medium),
                    heavy: impact(UIImpactFeedbackStyle::Heavy),
                    selection: UISelectionFeedbackGenerator::new(mtm),
                    notice: UINotificationFeedbackGenerator::new(mtm),
                }
            });
            match feel {
                Some(HapticFeel::Light) => h.light.impactOccurred(),
                Some(HapticFeel::Medium) => h.medium.impactOccurred(),
                Some(HapticFeel::Heavy) => h.heavy.impactOccurred(),
                Some(HapticFeel::Selection) => h.selection.selectionChanged(),
                Some(HapticFeel::Success) => h
                    .notice
                    .notificationOccurred(UINotificationFeedbackType::Success),
                Some(HapticFeel::Warning) => h
                    .notice
                    .notificationOccurred(UINotificationFeedbackType::Warning),
                Some(HapticFeel::Error) => h
                    .notice
                    .notificationOccurred(UINotificationFeedbackType::Error),
                None => {
                    if strength > 0.0 {
                        h.heavy.impactOccurredWithIntensity(strength as f64);
                    }
                }
            }
        });
    }

    /// Hand a file the app wrote to iOS's own "Save to Files" sheet: the
    /// person picks where it goes (iCloud Drive, On My iPhone, a folder of
    /// theirs) and the file is moved there. Saving on a phone is this two
    /// step: an app cannot write straight into a place it has not been
    /// given, and the sheet is how a person gives one (K-1032).
    pub fn export_file(path: &std::path::Path) -> bool {
        if !path.is_file() {
            return false;
        }
        let path = path.to_path_buf();
        DispatchQueue::main().exec_async(move || present_export(&path));
        true
    }

    fn present_export(path: &std::path::Path) {
        let (Some(mtm), Some(main_side)) = (MainThreadMarker::new(), MAIN_SIDE.0.get()) else {
            return;
        };
        let url = objc2_foundation::NSURL::fileURLWithPath(&objc2_foundation::NSString::from_str(
            &path.to_string_lossy(),
        ));
        let urls = objc2_foundation::NSArray::from_retained_slice(&[url]);
        // Moved, not copied: the file was written only to be placed, and a
        // copy would leave it behind in Krate.
        let picker = objc2_ui_kit::UIDocumentPickerViewController::initForExportingURLs_asCopy(
            objc2_ui_kit::UIDocumentPickerViewController::alloc(mtm),
            &urls,
            false,
        );
        main_side
            .controller
            .presentViewController_animated_completion(&picker, true, None);
    }

    /// Where the running app's preview goes, and when to take it: a picture
    /// of the app as it really looks, for its card on Krate's home screen,
    /// taken once a couple of seconds in so the app has drawn its content.
    static PREVIEW: Mutex<Option<(std::path::PathBuf, std::time::Instant)>> = Mutex::new(None);

    /// Take the running app's preview, once, `after` from now, into `path`
    /// (the home screen's picture format: width and height as little-endian
    /// u32, then RGBA).
    pub fn capture_preview(path: std::path::PathBuf, after: std::time::Duration) {
        *PREVIEW.lock().expect("preview lock") = Some((path, std::time::Instant::now() + after));
    }

    /// The app ended before its preview was due: take none.
    pub fn cancel_preview() {
        PREVIEW.lock().expect("preview lock").take();
    }

    /// The preview's path when it is due now, taken so it is written once.
    pub(crate) fn preview_due() -> Option<std::path::PathBuf> {
        let mut preview = PREVIEW.lock().expect("preview lock");
        match preview.as_ref() {
            Some((_, at)) if std::time::Instant::now() >= *at => {
                preview.take().map(|(path, _)| path)
            }
            _ => None,
        }
    }

    /// Write a preview: RGBA pixels `w` by `h`, shrunk by a whole factor to
    /// at most 400 pixels wide.
    pub(crate) fn write_preview(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
        let factor = w.div_ceil(400).max(1);
        let (ow, oh) = (w / factor, h / factor);
        if ow == 0 || oh == 0 {
            return;
        }
        let mut out = Vec::with_capacity(8 + (ow * oh * 4) as usize);
        out.extend_from_slice(&ow.to_le_bytes());
        out.extend_from_slice(&oh.to_le_bytes());
        for y in 0..oh {
            for x in 0..ow {
                let mut sum = [0u32; 4];
                for dy in 0..factor {
                    for dx in 0..factor {
                        let i = (((y * factor + dy) * w + x * factor + dx) * 4) as usize;
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u32::from(rgba[i + c]);
                        }
                    }
                }
                let n = factor * factor;
                // Opaque: a preview is a picture of the screen.
                out.extend_from_slice(&[
                    (sum[0] / n) as u8,
                    (sum[1] / n) as u8,
                    (sum[2] / n) as u8,
                    255,
                ]);
            }
        }
        let part = path.with_extension("part");
        if std::fs::write(&part, &out).is_ok() {
            let _ = std::fs::rename(&part, path);
        }
    }

    /// A file was opened in Krate from elsewhere (Messages, Mail, Files)
    /// while Krate was running: cover the screen with "Opening" and ask
    /// whatever is on it -- Krate's home screen or an app -- to close, so
    /// the player can open the file next. Main thread only.
    pub fn close_for_opened_file() {
        LOADING.store(true, std::sync::atomic::Ordering::Relaxed);
        *LOADING_SINCE.lock().expect("loading lock") = Some(std::time::Instant::now());
        if let Some(main_side) = MAIN_SIDE.0.get() {
            main_side.close.setHidden(true);
            cover_shown(main_side, "Krate", "Opening");
        }
        SHARED.lock().expect("shared state lock").close_tapped = true;
        INPUT_ARRIVED.notify_all();
        end_if_unanswered();
    }

    /// Krate is in the background: nothing is drawn (iOS ends an app that
    /// touches the GPU while it is not on screen), and the app keeps its
    /// state until the person comes back (IC-915).
    static BACKGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Krate went to the background, or came back. Coming back sends the app
    /// a resize of its own size, so it draws its frame again.
    pub fn set_background(on: bool) {
        BACKGROUND.store(on, std::sync::atomic::Ordering::Relaxed);
        if !on {
            INSETS_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
            INPUT_ARRIVED.notify_all();
        }
    }

    fn in_background() -> bool {
        BACKGROUND.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_keyboard(covered: f32) {
        let mut keyboard = KEYBOARD.lock().expect("keyboard lock");
        if (*keyboard - covered).abs() > 0.5 {
            *keyboard = covered;
            INSETS_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
            INPUT_ARRIVED.notify_all();
        }
    }

    /// The edges an app should keep text and controls out of (IC-912): the
    /// system's safe area, the on-screen keyboard while it shows, and, while
    /// an app runs, the close button Krate draws at the top right (placed in
    /// `view_laid_out`).
    pub fn screen_safe() -> (f32, f32, f32, f32) {
        let (top, right, bottom, left) = *SAFE.lock().expect("safe lock");
        let bottom = bottom.max(*KEYBOARD.lock().expect("keyboard lock"));
        if APP_CHROME.load(std::sync::atomic::Ordering::Relaxed) {
            // The strip under the clock that holds Krate's menu (IC-934).
            (top.max(20.0) + MENU_BAND, right, bottom, left)
        } else {
            (top, right, bottom, left)
        }
    }

    /// Which way up the app's window wants the screen: landscape when the
    /// app asked for a window wider than tall (a game), portrait otherwise.
    static LANDSCAPE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The running app lays itself out for any screen (IC-912): the screen
    /// follows the way the phone is held, as the phone's own apps do,
    /// instead of turning to the shape of the window the app asked for.
    static ADAPTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn set_adapts(adapts: bool) {
        ADAPTS.store(adapts, std::sync::atomic::Ordering::Relaxed);
    }

    fn adapts() -> bool {
        ADAPTS.load(std::sync::atomic::Ordering::Relaxed)
    }

    define_class!(
        // SAFETY:
        // - UIViewController has no extra subclassing requirements for
        //   overriding its orientation query.
        // - UIKit calls it on the main thread.
        #[unsafe(super = UIViewController)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ()]
        struct KrateViewController;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateViewController {}

        impl KrateViewController {
            // SAFETY: signature matches UIViewController's.
            #[unsafe(method(supportedInterfaceOrientations))]
            fn supported_interface_orientations(&self) -> objc2_ui_kit::UIInterfaceOrientationMask {
                if adapts() {
                    objc2_ui_kit::UIInterfaceOrientationMask::AllButUpsideDown
                } else if LANDSCAPE.load(std::sync::atomic::Ordering::Relaxed) {
                    objc2_ui_kit::UIInterfaceOrientationMask::Landscape
                } else {
                    objc2_ui_kit::UIInterfaceOrientationMask::Portrait
                }
            }

            #[unsafe(method(prefersStatusBarHidden))]
            fn prefers_status_bar_hidden(&self) -> bool {
                !adapts() && LANDSCAPE.load(std::sync::atomic::Ordering::Relaxed)
            }

            #[unsafe(method(prefersHomeIndicatorAutoHidden))]
            fn prefers_home_indicator_auto_hidden(&self) -> bool {
                !adapts() && LANDSCAPE.load(std::sync::atomic::Ordering::Relaxed)
            }
        }
    );

    /// Turn the screen to suit a window of `size`: sideways for a window
    /// wider than tall. iOS animates the turn; this waits for it (up to
    /// 1.5 s), so the window an app gets already has the shape it asked
    /// for -- Counter-Strike read 390x844 for its first frame otherwise and
    /// sized its picture for a portrait screen.
    fn request_orientation(size: WindowSize) {
        if adapts() {
            // Any way up: let iOS turn to how the phone is held, and leave
            // the window whatever shape that gives. Nothing to wait for.
            LANDSCAPE.store(false, std::sync::atomic::Ordering::Relaxed);
            DispatchQueue::main().exec_async(move || {
                let Some(main_side) = MAIN_SIDE.0.get() else {
                    return;
                };
                main_side.controller.setNeedsUpdateOfSupportedInterfaceOrientations();
                main_side.controller.setNeedsStatusBarAppearanceUpdate();
                if let Some(scene) = main_side.window.windowScene() {
                    let prefs =
                        objc2_ui_kit::UIWindowSceneGeometryPreferencesIOS::initWithInterfaceOrientations(
                            <objc2_ui_kit::UIWindowSceneGeometryPreferencesIOS as objc2::AnyThread>::alloc(),
                            objc2_ui_kit::UIInterfaceOrientationMask::AllButUpsideDown,
                        );
                    scene.requestGeometryUpdateWithPreferences_errorHandler(&prefs, None);
                }
            });
            return;
        }
        let landscape = size.width > size.height;
        if LANDSCAPE.swap(landscape, std::sync::atomic::Ordering::Relaxed) == landscape {
            return;
        }
        let turned = |shared: &Shared| {
            shared
                .screen
                .is_some_and(|(l, _)| (l.width > l.height) == landscape)
        };
        DispatchQueue::main().exec_async(move || {
            let Some(main_side) = MAIN_SIDE.0.get() else {
                return;
            };
            let mask = if landscape {
                objc2_ui_kit::UIInterfaceOrientationMask::Landscape
            } else {
                objc2_ui_kit::UIInterfaceOrientationMask::Portrait
            };
            main_side.controller.setNeedsUpdateOfSupportedInterfaceOrientations();
            main_side.controller.setNeedsStatusBarAppearanceUpdate();
            if let Some(scene) = main_side.window.windowScene() {
                let prefs =
                    objc2_ui_kit::UIWindowSceneGeometryPreferencesIOS::initWithInterfaceOrientations(
                        <objc2_ui_kit::UIWindowSceneGeometryPreferencesIOS as objc2::AnyThread>::alloc(),
                        mask,
                    );
                scene.requestGeometryUpdateWithPreferences_errorHandler(&prefs, None);
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let mut shared = SHARED.lock().expect("shared state lock");
        while !turned(&shared) {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            shared = INPUT_ARRIVED
                .wait_timeout(shared, deadline - now)
                .expect("shared state lock")
                .0;
        }
    }

    /// The view's bounds changed (the screen turned): move the GPU layer
    /// with it and publish the new size for the guest.
    fn view_laid_out(view: &KrateSurfaceView) {
        let uiview: &UIView = view;
        let bounds: CGRect = uiview.bounds();
        if let Some(main_side) = MAIN_SIDE.0.get() {
            objc2_quartz_core::CATransaction::begin();
            objc2_quartz_core::CATransaction::setDisableActions(true);
            main_side.metal.setFrame(bounds);
            objc2_quartz_core::CATransaction::commit();
            // The capsule: top right, inside the safe area (clear of the
            // notch and the status bar), above the app.
            if let Some(scene) = main_side.window.windowScene() {
                let turn = scene.interfaceOrientation().0;
                INTERFACE_TURN.store(turn.clamp(0, 4) as u8, std::sync::atomic::Ordering::Relaxed);
            }
            let safe = uiview.safeAreaInsets();
            *SAFE.lock().expect("safe lock") = (
                safe.top as f32,
                safe.right as f32,
                safe.bottom as f32,
                safe.left as f32,
            );
            let (cw, ch) = (44.0, 30.0);
            let close: &UIView = &main_side.close;
            close.setFrame(CGRect::new(
                CGPoint::new(
                    bounds.size.width - safe.right - 12.0 - cw,
                    safe.top.max(8.0) + 6.0,
                ),
                objc2_core_foundation::CGSize::new(cw, ch),
            ));
            uiview.bringSubviewToFront(close);
            // The cover fills the view, its contents centred.
            let l = &main_side.loading;
            let (w, h) = (bounds.size.width, bounds.size.height);
            let frame = |x: f64, y: f64, fw: f64, fh: f64| {
                CGRect::new(
                    CGPoint::new(x, y),
                    objc2_core_foundation::CGSize::new(fw, fh),
                )
            };
            l.cover.setFrame(bounds);
            let mark: &UIView = &l.mark;
            mark.setFrame(frame(w / 2.0 - 36.0, h / 2.0 - 96.0, 72.0, 72.0));
            let title: &UIView = &l.title;
            title.setFrame(frame(16.0, h / 2.0 - 8.0, w - 32.0, 28.0));
            let status: &UIView = &l.status;
            status.setFrame(frame(16.0, h / 2.0 + 22.0, w - 32.0, 22.0));
            let spinner: &UIView = &l.spinner;
            spinner.setFrame(frame(w / 2.0 - 15.0, h / 2.0 + 58.0, 30.0, 30.0));
            if !l.cover.isHidden() {
                uiview.bringSubviewToFront(&l.cover);
            }
            raise_menu(uiview);
        }
        let Ok(logical) = WindowSize::new(
            (bounds.size.width as u32).max(1),
            (bounds.size.height as u32).max(1),
        ) else {
            return;
        };
        let mut shared = SHARED.lock().expect("shared state lock");
        if let Some((current, scale)) = shared.screen {
            if current != logical {
                shared.screen = Some((logical, scale));
                drop(shared);
                INPUT_ARRIVED.notify_all();
            }
        }
    }

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
        diag("touch");
        INPUT_ARRIVED.notify_all();
        // Wake the guest again when a resting finger's press falls due.
        std::thread::spawn(|| {
            std::thread::sleep(PRESS_AFTER + std::time::Duration::from_millis(2));
            INPUT_ARRIVED.notify_all();
        });
    }

    /// Build the UIKit side, on the main queue, once.
    fn ensure_main_side() -> Result<(WindowSize, f32), UiAdapterError> {
        if let Some(screen) = SHARED.lock().expect("shared state lock").screen {
            return Ok(screen);
        }
        let mut result: Result<(WindowSize, f32), UiAdapterError> = Err(UiAdapterError::Internal(
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
                let controller: Retained<KrateViewController> = {
                    let this = KrateViewController::alloc(mtm).set_ivars(());
                    unsafe { msg_send![super(this), init] }
                };
                let view: Retained<KrateSurfaceView> = {
                    let this = KrateSurfaceView::alloc(mtm).set_ivars(());
                    // SAFETY: UIView's initWithFrame is the designated
                    // initializer.
                    unsafe { msg_send![super(this), initWithFrame: bounds] }
                };
                unsafe {
                    view.setUserInteractionEnabled(true);
                    // Every finger, not only the first: a game moves with one
                    // thumb and aims with the other (krate:ui/touch). UIKit
                    // delivers one touch per view unless told otherwise.
                    view.setMultipleTouchEnabled(true);
                    let as_uiview: &UIView = &view;
                    controller.setView(Some(as_uiview));
                    let as_controller: &UIViewController = &controller;
                    window.setRootViewController(Some(as_controller));
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
                // Cap the GPU raster at 2x too. Full 3x is 3.0 million
                // pixels per frame; vello's compute passes could not finish
                // one refresh in time, so get_current_texture waited for the
                // NEXT vsync -- measured at 26 ms of acquire and a hard 30
                // fps. At 2x the pixel work drops 2.25x and the layer's own
                // contentsScale still presents it sharp on a 3x panel.
                let native_scale = (screen.scale() as f32).min(2.0);
                let metal: objc2::rc::Retained<objc2_quartz_core::CAMetalLayer> =
                    unsafe { objc2_quartz_core::CAMetalLayer::new() };
                unsafe {
                    metal.setFrame(bounds);
                    metal.setContentsScale(native_scale as f64);
                    let as_uiview: &UIView = &view;
                    as_uiview.layer().addSublayer(&metal);
                }
                // The display link: the panel tells us when a frame is due.
                // Scheduled on the main run loop in the common modes so it
                // keeps firing during tracking (a finger on the screen is
                // exactly when frames matter most).
                let link_target = {
                    let this = KrateDisplayLinkTarget::alloc(mtm).set_ivars(());
                    let this: objc2::rc::Retained<KrateDisplayLinkTarget> =
                        unsafe { objc2::msg_send![super(this), init] };
                    this
                };
                let display_link = unsafe {
                    objc2_quartz_core::CADisplayLink::displayLinkWithTarget_selector(
                        &link_target,
                        objc2::sel!(krateDisplayLinkFired:),
                    )
                };
                // The panel's full rate: 120 Hz on a ProMotion iPhone (the
                // Info.plist opts in), so a list follows the finger the way
                // the phone's own apps do. iOS still lowers it for battery
                // when nothing moves.
                display_link.setPreferredFrameRateRange(objc2_quartz_core::CAFrameRateRange {
                    minimum: 60.0,
                    maximum: 120.0,
                    preferred: 120.0,
                });
                unsafe {
                    display_link.addToRunLoop_forMode(
                        &objc2_foundation::NSRunLoop::mainRunLoop(),
                        objc2_foundation::NSRunLoopCommonModes,
                    );
                }

                observe_keyboard();

                let metal_ptr = objc2::rc::Retained::as_ptr(&metal) as usize;
                SHARED.lock().expect("shared state lock").panel_scale = screen.scale() as f32;
                SHARED.lock().expect("shared state lock").metal_layer =
                    Some((metal_ptr, native_scale));

                // Krate's close capsule: dark glass, the system's own xmark,
                // above the app's drawing, hidden until the player runs an app.
                let close_target: Retained<KrateCloseTarget> = {
                    let this = KrateCloseTarget::alloc(mtm).set_ivars(());
                    unsafe { msg_send![super(this), init] }
                };
                let close =
                    objc2_ui_kit::UIButton::buttonWithType(objc2_ui_kit::UIButtonType::System, mtm);
                unsafe {
                    close.setImage_forState(
                        UIImage::systemImageNamed(&objc2_foundation::NSString::from_str("xmark"))
                            .as_deref(),
                        objc2_ui_kit::UIControlState::Normal,
                    );
                    close.setTintColor(Some(&objc2_ui_kit::UIColor::whiteColor()));
                    let as_view: &UIView = &close;
                    as_view.setBackgroundColor(Some(&objc2_ui_kit::UIColor::colorWithWhite_alpha(
                        0.0, 0.38,
                    )));
                    as_view.layer().setCornerRadius(15.0);
                    as_view.setHidden(!APP_CHROME.load(std::sync::atomic::Ordering::Relaxed));
                    close.addTarget_action_forControlEvents(
                        Some(&close_target),
                        objc2::sel!(krateClose:),
                        objc2_ui_kit::UIControlEvents::TouchUpInside,
                    );
                    let host: &UIView = &view;
                    host.addSubview(as_view);
                    install_menu_gesture(mtm, host);
                }
                // The loading cover: light, the mark, a title, a status line
                // and the spinner; hidden until something is loading.
                let loading = unsafe {
                    let cover = UIView::new(mtm);
                    cover.setBackgroundColor(Some(
                        &objc2_ui_kit::UIColor::colorWithRed_green_blue_alpha(
                            0.949, 0.953, 0.969, 1.0,
                        ),
                    ));
                    cover.setHidden(true);
                    let mark_image =
                        UIImage::imageNamed(&objc2_foundation::NSString::from_str("KrateMark"));
                    let mark =
                        UIImageView::initWithImage(UIImageView::alloc(mtm), mark_image.as_deref());
                    let label = |size: f64, weight: f64, grey: f64| {
                        let label = objc2_ui_kit::UILabel::new(mtm);
                        label.setFont(Some(&objc2_ui_kit::UIFont::systemFontOfSize_weight(
                            size, weight,
                        )));
                        label.setTextColor(Some(&objc2_ui_kit::UIColor::colorWithWhite_alpha(
                            grey, 1.0,
                        )));
                        label.setTextAlignment(objc2_ui_kit::NSTextAlignment::Center);
                        label
                    };
                    let title = label(20.0, objc2_ui_kit::UIFontWeightSemibold, 0.05);
                    let status = label(15.0, objc2_ui_kit::UIFontWeightRegular, 0.45);
                    let spinner =
                        objc2_ui_kit::UIActivityIndicatorView::initWithActivityIndicatorStyle(
                            objc2_ui_kit::UIActivityIndicatorView::alloc(mtm),
                            objc2_ui_kit::UIActivityIndicatorViewStyle::Medium,
                        );
                    for part in [
                        &*mark as &UIView,
                        &*title as &UIView,
                        &*status as &UIView,
                        &*spinner as &UIView,
                    ] {
                        cover.addSubview(part);
                    }
                    let host: &UIView = &view;
                    host.addSubview(&cover);
                    Loading {
                        cover,
                        mark,
                        title,
                        status,
                        spinner,
                    }
                };
                let _ = MAIN_SIDE.0.set(MainSide {
                    loading,
                    close,
                    _close_target: close_target,
                    metal,
                    _link_target: link_target,
                    _display_link: display_link,
                    window,
                    controller,
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
                    fingers: Default::default(),
                    pointer: Default::default(),
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
                UITouchPhase::Ended | UITouchPhase::Cancelled => {
                    guest.fingers.lift(touch.finger as u64)
                }
                _ => guest.fingers.touch(touch.finger as u64, touch.x, touch.y),
            }
            match touch.phase {
                UITouchPhase::Began => {
                    if guest.gesture.is_none() {
                        let drags = drags_at(krate, &guest.placements, touch.x, touch.y);
                        guest.gesture = Some(TouchGesture {
                            finger: touch.finger,
                            start: (touch.x, touch.y),
                            last: (touch.x, touch.y),
                            scrolling: false,
                            began: std::time::Instant::now(),
                            pressed: drags,
                            drags,
                        });
                        // Something that drags is pressed at once, so the
                        // first movement already moves it.
                        if drags {
                            guest.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: true,
                            });
                        }
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
                        if gesture.drags {
                            // The finger moves what it is on: the press
                            // follows it, held, until it lifts.
                            gesture.last = (touch.x, touch.y);
                            guest.pointer_samples.push(RawPointerSample {
                                window: krate,
                                x: touch.x,
                                y: touch.y,
                                pressed: true,
                            });
                            continue;
                        }
                        if !gesture.scrolling {
                            let sx = touch.x - gesture.start.0;
                            let sy = touch.y - gesture.start.1;
                            if sx * sx + sy * sy > SLOP * SLOP {
                                gesture.scrolling = true;
                                // A press already sent is let go far outside
                                // the window, so nothing that acts on a
                                // release under the finger fires: the touch
                                // became a scroll.
                                if gesture.pressed {
                                    guest.pointer_samples.push(RawPointerSample {
                                        window: krate,
                                        x: -10_000.0,
                                        y: -10_000.0,
                                        pressed: false,
                                    });
                                }
                            }
                        }
                        let dx = touch.x - gesture.last.0;
                        let dy = touch.y - gesture.last.1;
                        gesture.last = (touch.x, touch.y);
                        if gesture.scrolling && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON)
                        {
                            let pending = guest
                                .pending_scroll
                                .get_or_insert((0.0, 0.0, touch.x, touch.y));
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
                            // A quick tap lifts before its press went out:
                            // both now, at the lift.
                            if !gesture.pressed {
                                guest.pointer_samples.push(RawPointerSample {
                                    window: krate,
                                    x: touch.x,
                                    y: touch.y,
                                    pressed: true,
                                });
                            }
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

    /// A finger that has rested past PRESS_AFTER without scrolling presses
    /// now, where it landed.
    fn deliver_due_press(guest: &mut GuestSide) {
        let Some(krate) = guest.krate else {
            return;
        };
        if let Some(gesture) = guest.gesture.as_mut() {
            if !gesture.pressed && !gesture.scrolling && gesture.began.elapsed() >= PRESS_AFTER {
                gesture.pressed = true;
                guest.pointer_samples.push(RawPointerSample {
                    window: krate,
                    x: gesture.start.0,
                    y: gesture.start.1,
                    pressed: true,
                });
            }
        }
    }

    /// Rasterize on the guest thread, then hand finished pixels to the
    /// main queue, which only wraps them in a CGImage and sets the view.
    /// The autoreleasepool per frame is the macOS adapter's 46 GB lesson.
    fn blit(guest: &mut GuestSide) {
        // Off screen: keep the frame owed (dirty) for when Krate comes back.
        if in_background() {
            return;
        }
        diag("cpu-blit-start");
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
                let bitmap_info =
                    CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
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
        // A CPU frame lifts the loading cover only while it is what the
        // screen shows. Once the GPU draws the canvases, the CPU blit is
        // the empty window a new app has before its first canvas frame --
        // lifting the cover on it showed the last app's frame, stretched,
        // through the turn of the screen and the app's start.
        if !GPU_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            frame_shown(Some(guest.logical));
        }
    }

    // ------------------------------------------------------------------
    // The adapter surface. Every function runs on the guest thread.

    pub fn create_native_window(
        krate: WindowId,
        _title: &str,
        size: WindowSize,
    ) -> Result<(u64, WinitWindowSnapshot), UiAdapterError> {
        // The window exists first, then the screen turns to suit it, then
        // the guest learns the size it got.
        ensure_main_side()?;
        request_orientation(size);
        with_guest(|guest| {
            if let Some((logical, _)) = SHARED.lock().expect("shared state lock").screen {
                guest.logical = logical;
            }
            guest.krate = Some(krate);
            guest.placements.clear();
            // A new app starts on the CPU path until it draws on the GPU
            // itself. The flag used to stay set for the life of Krate, so a
            // widget app opened after any canvas app -- Krate's own home is
            // one -- painted nothing and the opening cover never lifted
            // (Timer, 2026-10-11).
            GPU_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
            // The metal layer is a sublayer of the image view, so it draws
            // OVER the CPU frame: its last GPU frame (the opening animation,
            // the last app) hid every widget frame until it was hidden.
            DispatchQueue::main().exec_async(|| {
                if let Some(main_side) = MAIN_SIDE.0.get() {
                    main_side.metal.setHidden(true);
                }
            });
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
            access_changed(
                true,
                guest
                    .placements
                    .iter()
                    .filter_map(krate_adapter_common::ui::AccessElement::for_placement)
                    .collect(),
            );
            guest.dirty = true;
            if !GPU_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                blit(guest);
            }
            Ok(drawn)
        })
    }

    pub fn pump_native_events() -> Result<CollectedNativeEvents, UiAdapterError> {
        if !guest_initialized() {
            return Ok(Vec::new());
        }
        // The cover lifts after LOADING_MOST whether or not a frame came:
        // the rule was only asked when one did, so an app that never drew
        // kept it up for ever, with no way to close the app beneath it.
        if LOADING.load(std::sync::atomic::Ordering::Relaxed)
            && LOADING_SINCE
                .lock()
                .expect("loading lock")
                .is_some_and(|since| since.elapsed() > LOADING_MOST)
        {
            hide_loading();
        }
        with_guest(|guest| {
            digest_touches(guest);
            let close = std::mem::take(&mut SHARED.lock().expect("shared state lock").close_tapped);
            if let (true, Some(krate)) = (close, guest.krate) {
                guest
                    .events
                    .push((krate, WinitWindowNativeEvent::CloseRequested));
            }
            let screen = SHARED.lock().expect("shared state lock").screen;
            if let (Some((logical, _)), Some(krate)) = (screen, guest.krate) {
                // The keyboard came or went: same size, new covered edge.
                if INSETS_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed)
                    && logical == guest.logical
                {
                    guest.dirty = true;
                    guest
                        .events
                        .push((krate, WinitWindowNativeEvent::Resized(logical)));
                }
                if logical != guest.logical {
                    guest.logical = logical;
                    guest.dirty = true;
                    guest
                        .events
                        .push((krate, WinitWindowNativeEvent::Resized(logical)));
                }
            }
            Ok(std::mem::take(&mut guest.events))
        })
    }

    /// Park the guest thread until input arrives or the deadline passes.
    /// The condvar is signaled by the main thread's touch callouts, so a
    /// finger wakes the guest immediately -- no run loop involved.
    /// Park until the panel's next refresh, or `max`. Touches do NOT wake
    /// this: pacing that wakes on input lets a fast finger present faster
    /// than the display consumes and stalls the next acquire (K-090).
    pub fn park_for_frame(max: std::time::Duration) -> bool {
        if !guest_initialized() {
            return false;
        }
        if VSYNC_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        let deadline = std::time::Instant::now() + max;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return true;
            }
            let shared = SHARED.lock().expect("shared state lock");
            let (_guard, _timeout) = INPUT_ARRIVED
                .wait_timeout(shared, deadline - now)
                .expect("shared state lock");
            if VSYNC_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            // A touch woke us, not the panel. Keep waiting for the frame.
        }
    }

    pub fn park_for_events(max: std::time::Duration) -> bool {
        if !guest_initialized() {
            return false;
        }
        // A frame is already due, or input is waiting: do not park at all.
        if VSYNC_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        let shared = SHARED.lock().expect("shared state lock");
        if !shared.touches.is_empty() {
            return true;
        }
        // Wake on whichever comes first, a finger or the panel's refresh.
        let (_guard, _timeout) = INPUT_ARRIVED
            .wait_timeout(shared, max)
            .expect("shared state lock");
        VSYNC_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
        true
    }

    /// Every finger on the window now, in logical pixels.
    pub fn touches(_id: WindowId) -> Vec<(u64, f32, f32)> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| {
            digest_touches(guest);
            Ok(guest.fingers.read())
        })
        .unwrap_or_default()
    }

    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| {
            digest_touches(guest);
            deliver_due_press(guest);
            let samples = std::mem::take(&mut guest.pointer_samples);
            for sample in &samples {
                guest.pointer.moved_to(sample.x, sample.y);
                guest.pointer.button(
                    krate_adapter_common::ui::PointerButton::Primary,
                    sample.pressed,
                );
            }
            Ok(samples)
        })
        .unwrap_or_default()
    }

    /// The pointer as the presses so far leave it (krate:ui/pointer).
    pub fn pointer_state(id: WindowId) -> krate_adapter_common::ui::PointerTracker {
        if !guest_initialized() {
            return Default::default();
        }
        with_guest(|guest| {
            Ok(if guest.krate == Some(id) {
                guest.pointer
            } else {
                Default::default()
            })
        })
        .unwrap_or_default()
    }

    /// Hand over the pointer's motion since the last call.
    pub fn take_pointer_motion(id: WindowId) -> (f32, f32) {
        if !guest_initialized() {
            return (0.0, 0.0);
        }
        with_guest(|guest| {
            Ok(if guest.krate == Some(id) {
                guest.pointer.take_motion()
            } else {
                (0.0, 0.0)
            })
        })
        .unwrap_or_default()
    }

    pub fn drain_key_samples() -> Vec<RawKeySample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| {
            let typed = std::mem::take(&mut SHARED.lock().expect("shared state lock").typed);
            if let Some(window) = guest.krate {
                for t in typed {
                    let (key, text) = match t {
                        Typed::Text(text) => {
                            // One press per character, each carrying its text,
                            // the way a physical keyboard delivers them.
                            for c in text.chars() {
                                let key = c.to_string();
                                for pressed in [true, false] {
                                    guest.key_samples.push(RawKeySample {
                                        window,
                                        key: key.clone(),
                                        pressed,
                                        modifiers: Default::default(),
                                        text: pressed.then(|| key.clone()),
                                    });
                                }
                            }
                            continue;
                        }
                        Typed::Backspace => ("Backspace", None),
                        Typed::Enter => ("Enter", None::<String>),
                    };
                    for pressed in [true, false] {
                        guest.key_samples.push(RawKeySample {
                            window,
                            key: key.to_string(),
                            pressed,
                            modifiers: Default::default(),
                            text: text.clone(),
                        });
                    }
                }
            }
            Ok(std::mem::take(&mut guest.key_samples))
        })
        .unwrap_or_default()
    }

    /// Show or put away the on-screen keyboard. True when it is showing.
    pub fn set_text_entry(_id: WindowId, on: bool) -> bool {
        if !guest_initialized() {
            return false;
        }
        let mut showing = false;
        DispatchQueue::main().exec_sync(|| {
            if let Some(main_side) = MAIN_SIDE.0.get() {
                let view: &UIView = &main_side.view;
                showing = if on {
                    view.becomeFirstResponder()
                } else {
                    view.resignFirstResponder();
                    false
                };
            }
        });
        showing
    }

    pub fn drain_wheel_samples() -> Vec<RawWheelSample> {
        if !guest_initialized() {
            return Vec::new();
        }
        with_guest(|guest| {
            let mut samples = std::mem::take(&mut guest.wheel_samples);
            if let (Some((dx, dy, x, y)), Some(krate)) = (guest.pending_scroll.take(), guest.krate)
            {
                diag(&format!("wheel dy={dy:.0}"));
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

    /// True once the GPU renders this app's frames: the CPU blit then has
    /// nothing to show and painting 1.4 million invisible pixels per frame
    /// is pure heat.
    static GPU_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The GPU would not start on this device (the simulator's Metal lacks
    /// indirect execution; any device can fail): from then on Krate says it
    /// has no GPU, so the runtime paints frames on the CPU instead of
    /// recording them for a GPU that will not draw them (K-1214).
    static GPU_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// Whether frames should be recorded for the GPU: on iOS, until it fails.
    pub fn gpu_usable() -> bool {
        !GPU_FAILED.load(std::sync::atomic::Ordering::Relaxed)
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
        // Off screen: no GPU work (iOS ends an app that does it in the
        // background). Claimed as shown so nothing falls back to pixels;
        // the app draws again when Krate comes back.
        if in_background() {
            return true;
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
                        // GPU_ACTIVE is set, and the metal layer shown, by
                        // the first frame this app presents (below).
                        // The image view would cover the metal layer with
                        // its last CPU frame; hide it once the GPU owns
                        // the pixels.
                        DispatchQueue::main().exec_sync(|| {
                            if let Some(main_side) = MAIN_SIDE.0.get() {
                                // The stale CPU frame must not fight the
                                // metal layer -- clearing the image is
                                // what stops the flashing.
                                main_side.view.setImage(None);
                            }
                        });
                        *slot = Some(gpu);
                    }
                    Err(why) => {
                        eprintln!("krate-ios: gpu canvas unavailable: {why}");
                        GPU_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                        return false;
                    }
                }
            }
            diag("gpu-present-start");
            let gpu = slot.as_mut().expect("gpu initialized");
            // The drawable follows the view's size (the screen turned) and
            // the kind of frame: a whole-frame picture (a game, an emulator)
            // at the panel's full scale, so it is stretched once, by us,
            // crisply when the ratio is whole; interface frames at the 2x
            // cap the vector renderer can keep up with.
            let wanted = {
                let shared = SHARED.lock().expect("shared state lock");
                shared
                    .metal_layer
                    .zip(shared.screen.map(|(l, _)| l))
                    .map(|(layer, logical)| (layer, logical, shared.panel_scale))
            };
            if let Some(((_, native_scale), logical, panel_scale)) = wanted {
                let picture = crate::vello_canvas::whole_frame_picture(list).is_some();
                let scale = if picture { panel_scale } else { native_scale };
                if gpu.layer_scale() != scale {
                    DispatchQueue::main().exec_sync(|| {
                        if let Some(main_side) = MAIN_SIDE.0.get() {
                            main_side.metal.setContentsScale(scale as f64);
                        }
                    });
                    gpu.set_layer_scale(scale);
                }
                gpu.resize(
                    (logical.width as f32 * scale) as u32,
                    (logical.height as f32 * scale) as u32,
                );
            }
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.present(list)))
                    .unwrap_or_else(|_| Err("gpu present panicked".into()));
            match outcome {
                Ok(()) => {
                    diag("gpu-present-done");
                    // This app draws on the GPU: the CPU image would cover
                    // the metal layer with a stale frame, so clear it the
                    // first time.
                    if !GPU_ACTIVE.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        DispatchQueue::main().exec_async(|| {
                            if let Some(main_side) = MAIN_SIDE.0.get() {
                                main_side.view.setImage(None);
                                main_side.metal.setHidden(false);
                            }
                        });
                    }
                    frame_shown(WindowSize::new(list.logical_width, list.logical_height).ok());
                    true
                }
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

    pub fn park_for_frame(_max: std::time::Duration) -> bool {
        false
    }

    pub fn park_for_events(_max: std::time::Duration) -> bool {
        false
    }

    pub fn touches(_id: WindowId) -> Vec<(u64, f32, f32)> {
        Vec::new()
    }

    pub fn drain_pointer_samples() -> Vec<RawPointerSample> {
        Vec::new()
    }

    pub fn pointer_state(_id: WindowId) -> krate_adapter_common::ui::PointerTracker {
        Default::default()
    }

    pub fn take_pointer_motion(_id: WindowId) -> (f32, f32) {
        (0.0, 0.0)
    }

    pub fn drain_key_samples() -> Vec<RawKeySample> {
        Vec::new()
    }

    pub fn set_text_entry(_id: WindowId, _on: bool) -> bool {
        false
    }

    pub fn set_app_chrome(_visible: bool) {}

    pub fn screen_safe() -> (f32, f32, f32, f32) {
        (0.0, 0.0, 0.0, 0.0)
    }

    pub fn set_adapts(_adapts: bool) {}

    pub fn gpu_usable() -> bool {
        false
    }

    pub fn set_canvas_presses(
        _window: WindowId,
        _widget: krate_adapter_common::ui::WidgetId,
        _presses: bool,
    ) {
    }

    pub fn ask(_title: &str, _body: &str, _question: bool) -> bool {
        false
    }

    pub fn set_background(_on: bool) {}

    pub fn close_for_opened_file() {}

    pub fn capture_preview(_path: std::path::PathBuf, _after: std::time::Duration) {}

    pub fn cancel_preview() {}

    pub fn choose_path(_folder: bool, _filter: &str) -> Option<std::path::PathBuf> {
        None
    }

    pub fn export_file(_path: &std::path::Path) -> bool {
        false
    }

    pub fn open_url(_url: &str) -> Result<(), String> {
        Err("links open from the iOS player only".to_string())
    }

    pub fn share(
        _text: Option<&str>,
        _url: Option<&str>,
        _files: &[(String, Vec<u8>)],
    ) -> Result<bool, String> {
        Err("the share sheet is on the iOS player only".to_string())
    }

    pub fn pick_photos(_limit: u32) -> Result<Vec<std::path::PathBuf>, String> {
        Err("the photo picker is on the iOS player only".to_string())
    }

    pub fn sign_in(_url: &str, _scheme: &str) -> Result<Option<String>, String> {
        Err("the sign-in sheet is on the iOS player only".to_string())
    }

    pub fn appearance() -> krate_adapter_common::ui::Appearance {
        krate_adapter_common::ui::Appearance::default()
    }

    pub fn save_photo(_data: &[u8]) -> Result<(), Option<String>> {
        Err(Some("Photos is on the iOS player only".to_string()))
    }

    pub fn notify(_sender: &str, _title: &str, _body: &str) -> Result<(), String> {
        Err("notifications post from the iOS player only".to_string())
    }

    pub fn set_current_app(_app: Option<(String, String)>) {}

    pub fn set_current_name(_name: Option<String>) {}

    pub fn capture_behind(_path: &std::path::Path) {}

    pub fn open_web_page(
        _address: &str,
        _frame: (f32, f32, f32, f32),
        _granted: Vec<(String, String)>,
    ) -> Result<u64, String> {
        Err("web pages show on the iOS player only".to_string())
    }

    pub fn place_web_page(_page: u64, _frame: (f32, f32, f32, f32)) {}

    pub fn web_page_go(_page: u64, _address: &str) {}

    pub fn web_page_back(_page: u64) {}

    pub fn web_page_state(_page: u64) -> Option<krate_adapter_common::ui::WebPageState> {
        None
    }

    pub fn close_web_page(_page: u64) {}

    pub fn describe_accessibility(_elements: &[krate_adapter_common::ui::AccessElement]) {}

    pub fn voice_over_on() -> bool {
        false
    }

    pub fn set_current_uses(_uses: Option<Vec<String>>) {}

    pub fn transcribe(_pcm: &[u8], _rate: u32, _language: Option<&str>) -> Result<String, String> {
        Err("speech recognition is on the iOS player only".to_string())
    }

    pub fn take_notice_open() -> Option<String> {
        None
    }

    pub fn install_notice_delegate() {}

    pub fn reminder_schedule(
        _sender: &str,
        _id: &str,
        _title: &str,
        _body: &str,
        _after: u64,
        _daily: bool,
    ) -> Result<(), String> {
        Err("reminders are on the iOS player only".to_string())
    }

    pub fn reminder_cancel(_id: &str) {}

    pub fn reminder_pending() -> Vec<String> {
        Vec::new()
    }

    pub fn haptic(_feel: Option<krate_adapter_common::ui::HapticFeel>, _strength: f32) -> bool {
        false
    }

    pub fn motion_available() -> bool {
        false
    }

    pub fn motion_read() -> Option<krate_adapter_common::ui::MotionReading> {
        None
    }

    pub fn location_available() -> bool {
        false
    }

    pub fn location_read() -> krate_adapter_common::ui::LocationAnswer {
        krate_adapter_common::ui::LocationAnswer::Unavailable
    }

    pub fn show_loading(_title: &str, _status: &str) {}

    pub fn hide_loading() {}

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
