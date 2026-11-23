//! A web app inside a Krate app on a Mac (IC-1004): a WKWebView filling
//! the window, its files served from the private `krate-app://app/` scheme
//! by a WKURLSchemeHandler that reads only inside the app's web folder and
//! sends every file with Krate's Content-Security-Policy, a bridge script
//! run before the page's own, and a script message handler that hands what
//! the page posts to the runtime. Nothing is kept between runs: the data
//! store is non-persistent, and what the page keeps it keeps through the
//! app.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;

use krate_adapter_common::appview;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol};
use objc2::{define_class, msg_send, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSObject, NSObjectProtocol, NSRect, NSString};

struct View {
    web: Retained<AnyObject>,
    folder: PathBuf,
    start: String,
    policy: String,
    _scheme: Retained<KrateAppScheme>,
    _messages: Retained<KrateAppMessages>,
    _nav: Retained<KrateAppNavigation>,
}

thread_local! {
    static VIEWS: RefCell<BTreeMap<u64, View>> = const { RefCell::new(BTreeMap::new()) };
}

fn ns(text: &str) -> Retained<NSString> {
    NSString::from_str(text)
}

fn url_of(object: &AnyObject, getter: &str) -> String {
    // SAFETY: `request` on a WKURLSchemeTask or a WKNavigationAction, its URL
    // and that URL's absoluteString.
    unsafe {
        let request: Option<Retained<AnyObject>> = if getter == "request" {
            msg_send![object, request]
        } else {
            None
        };
        let url: Option<Retained<AnyObject>> = request.and_then(|r| msg_send![&*r, URL]);
        url.and_then(|u| {
            let s: Option<Retained<NSString>> = msg_send![&*u, absoluteString];
            s.map(|s| s.to_string())
        })
        .unwrap_or_default()
    }
}

/// Answer one request of the page's: the file with its type and the policy,
/// or a 404 that says why.
fn answer(view: u64, task: &AnyObject) {
    let address = url_of(task, "request");
    let found = VIEWS.with(|views| {
        views
            .borrow()
            .get(&view)
            .map(|v| (v.folder.clone(), v.start.clone(), v.policy.clone()))
    });
    let Some((folder, start, policy)) = found else {
        return;
    };
    let (status, kind, body) = match appview::resolve(&address)
        .map_err(|r| format!("{r:?}"))
        .and_then(|rel| match appview::read_file(&folder, &rel) {
            Ok(bytes) => Ok((rel, bytes)),
            // One of the app's own routes, asked for by a reload: the start page.
            Err(_) if appview::is_route(&rel) => {
                appview::read_file(&folder, &start).map(|b| (start.clone(), b))
            }
            Err(why) => Err(why),
        }) {
        Ok((rel, bytes)) => (200isize, appview::content_type(&rel), bytes),
        Err(why) => {
            if std::env::var_os("KRATE_WEB_TRACE").is_some() {
                eprintln!("app-view: {address}: {why}");
            }
            (404, "text/plain; charset=utf-8", why.into_bytes())
        }
    };
    // SAFETY: an NSHTTPURLResponse for the task's own URL, handed to the
    // task with the bytes, on the main thread WebKit called us on.
    unsafe {
        let (Some(dict_class), Some(response_class), Some(data_class)) = (
            AnyClass::get(c"NSMutableDictionary"),
            AnyClass::get(c"NSHTTPURLResponse"),
            AnyClass::get(c"NSData"),
        ) else {
            return;
        };
        let request: Retained<AnyObject> = msg_send![task, request];
        let url: Retained<AnyObject> = msg_send![&*request, URL];
        let headers: Retained<AnyObject> = msg_send![dict_class, new];
        for (k, v) in [
            ("Content-Type", kind.to_string()),
            ("Content-Security-Policy", policy),
            ("Cache-Control", "no-store".to_string()),
            ("Content-Length", body.len().to_string()),
            ("X-Content-Type-Options", "nosniff".to_string()),
        ] {
            let _: () = msg_send![&*headers, setObject: &*ns(&v), forKey: &*ns(k)];
        }
        let made: objc2::rc::Allocated<AnyObject> = msg_send![response_class, alloc];
        let response: Option<Retained<AnyObject>> = msg_send![made,
            initWithURL: &*url,
            statusCode: status,
            HTTPVersion: &*ns("HTTP/1.1"),
            headerFields: &*headers];
        let Some(response) = response else {
            return;
        };
        let data: Retained<AnyObject> = msg_send![data_class, dataWithBytes: body.as_ptr().cast::<std::ffi::c_void>(), length: body.len()];
        let _: () = msg_send![task, didReceiveResponse: &*response];
        let _: () = msg_send![task, didReceiveData: &*data];
        let _: () = msg_send![task, didFinish];
    }
}

define_class!(
    // SAFETY: NSObject subclass; WebKit calls it on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<u64>]
    struct KrateAppScheme;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateAppScheme {}

    // WKURLSchemeHandler, by selector.
    impl KrateAppScheme {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web: &AnyObject, task: &AnyObject) {
            answer(self.ivars().get(), task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web: &AnyObject, _task: &AnyObject) {}
    }
);

define_class!(
    // SAFETY: NSObject subclass; WebKit calls it on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<u64>]
    struct KrateAppMessages;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateAppMessages {}

    // WKScriptMessageHandler, by selector.
    impl KrateAppMessages {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &AnyObject, message: &AnyObject) {
            // SAFETY: a WKScriptMessage's body; the bridge always sends a
            // string, anything else is ignored.
            let text: Option<String> = unsafe {
                let body: Option<Retained<AnyObject>> = msg_send![message, body];
                body.and_then(|b| {
                    let is_string: bool = msg_send![&*b, isKindOfClass: NSString::class()];
                    is_string.then(|| {
                        let s: &NSString = &*(Retained::as_ptr(&b) as *const NSString);
                        s.to_string()
                    })
                })
            };
            if let Some(text) = text {
                appview::push_message(self.ivars().get(), text);
            }
        }
    }
);

define_class!(
    // SAFETY: NSObject subclass; WebKit calls it on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Cell<u64>]
    struct KrateAppNavigation;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateAppNavigation {}

    // WKNavigationDelegate, by selector: the page goes only to its own files.
    impl KrateAppNavigation {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(&self, _web: &AnyObject, action: &AnyObject, decide: &block2::DynBlock<dyn Fn(isize)>) {
            let address = url_of(action, "request");
            decide.call((if appview::navigation_allowed(&address) { 1 } else { 0 },));
        }
    }
);

fn conform(class: &AnyClass, protocol: &std::ffi::CStr) {
    if let Some(p) = AnyProtocol::get(protocol) {
        // SAFETY: adding a protocol whose methods the class defines.
        unsafe {
            objc2::ffi::class_addProtocol(class as *const AnyClass as *mut AnyClass, p);
        }
    }
}

/// Fill the window whose NSWindow is `ns_window` with the web app in
/// `folder`, starting at `start`.
pub(crate) fn open(
    ns_window: u64,
    view: u64,
    folder: PathBuf,
    start: &str,
    preload: &str,
    granted: &[(String, String)],
) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("a web view is made on the main thread")?;
    let start = appview::resolve_path(start).map_err(|r| format!("{r:?}"))?;
    let policy = appview::csp(granted);
    let bridge = appview::bridge_script(appview::Channel::Webkit, preload);
    conform(KrateAppScheme::class(), c"WKURLSchemeHandler");
    conform(KrateAppMessages::class(), c"WKScriptMessageHandler");
    conform(KrateAppNavigation::class(), c"WKNavigationDelegate");
    // SAFETY: WebKit's configuration, scheme handler, user script and
    // message handler, and a WKWebView added to the app's own window's
    // content view, all on the main thread; the NSWindow pointer is the
    // native handle the adapter made.
    unsafe {
        let (
            Some(config_class),
            Some(store_class),
            Some(web_class),
            Some(script_class),
            Some(url_class),
            Some(req_class),
        ) = (
            AnyClass::get(c"WKWebViewConfiguration"),
            AnyClass::get(c"WKWebsiteDataStore"),
            AnyClass::get(c"WKWebView"),
            AnyClass::get(c"WKUserScript"),
            AnyClass::get(c"NSURL"),
            AnyClass::get(c"NSURLRequest"),
        )
        else {
            return Err("this Mac has no web view".to_string());
        };
        let window: Retained<AnyObject> = Retained::retain(ns_window as usize as *mut AnyObject)
            .ok_or("the app's window is gone")?;
        let content: Option<Retained<AnyObject>> = msg_send![&*window, contentView];
        let content = content.ok_or("the app's window has no content")?;
        let config: Retained<AnyObject> = msg_send![config_class, new];
        let store: Retained<AnyObject> = msg_send![store_class, nonPersistentDataStore];
        let _: () = msg_send![&*config, setWebsiteDataStore: &*store];

        let scheme = KrateAppScheme::alloc(mtm).set_ivars(Cell::new(view));
        let scheme: Retained<KrateAppScheme> = msg_send![super(scheme), init];
        let _: () =
            msg_send![&*config, setURLSchemeHandler: &*scheme, forURLScheme: &*ns(appview::SCHEME)];

        let messages = KrateAppMessages::alloc(mtm).set_ivars(Cell::new(view));
        let messages: Retained<KrateAppMessages> = msg_send![super(messages), init];
        let controller: Retained<AnyObject> = msg_send![&*config, userContentController];
        let _: () =
            msg_send![&*controller, addScriptMessageHandler: &*messages, name: &*ns("krate")];
        let made: objc2::rc::Allocated<AnyObject> = msg_send![script_class, alloc];
        // WKUserScriptInjectionTimeAtDocumentStart = 0, main frame only.
        let script: Retained<AnyObject> = msg_send![made,
            initWithSource: &*ns(&bridge),
            injectionTime: 0isize,
            forMainFrameOnly: true];
        let _: () = msg_send![&*controller, addUserScript: &*script];

        let bounds: NSRect = msg_send![&*content, bounds];
        let made: objc2::rc::Allocated<AnyObject> = msg_send![web_class, alloc];
        let web: Retained<AnyObject> =
            msg_send![made, initWithFrame: bounds, configuration: &*config];
        // NSViewWidthSizable | NSViewHeightSizable: it follows the window.
        let _: () = msg_send![&*web, setAutoresizingMask: 18usize];
        if std::env::var_os("KRATE_WEB_INSPECT").is_some() {
            let _: () = msg_send![&*web, setInspectable: true];
        }
        let nav = KrateAppNavigation::alloc(mtm).set_ivars(Cell::new(view));
        let nav: Retained<KrateAppNavigation> = msg_send![super(nav), init];
        let _: () = msg_send![&*web, setNavigationDelegate: &*nav];
        let _: () = msg_send![&*content, addSubview: &*web];

        VIEWS.with(|views| {
            views.borrow_mut().insert(
                view,
                View {
                    web: web.clone(),
                    folder,
                    start: start.clone(),
                    policy,
                    _scheme: scheme,
                    _messages: messages,
                    _nav: nav,
                },
            )
        });
        let address = appview::start_address(&start);
        let url: Option<Retained<AnyObject>> = msg_send![url_class, URLWithString: &*ns(&address)];
        let url = url.ok_or("the start page is not an address")?;
        let request: Retained<AnyObject> = msg_send![req_class, requestWithURL: &*url];
        let _: Option<Retained<AnyObject>> = msg_send![&*web, loadRequest: &*request];
    }
    Ok(())
}

/// Hand the page a message.
pub(crate) fn post(view: u64, message: &str) -> Result<(), String> {
    let script = appview::deliver_script(message);
    VIEWS.with(|views| {
        let views = views.borrow();
        let v = views.get(&view).ok_or("no such view")?;
        let none: *const AnyObject = std::ptr::null();
        // SAFETY: WKWebView's evaluateJavaScript with no completion, main thread.
        unsafe {
            let _: () =
                msg_send![&*v.web, evaluateJavaScript: &*ns(&script), completionHandler: none];
        }
        Ok(())
    })
}

/// Take a view away.
pub(crate) fn close(view: u64) {
    if let Some(v) = VIEWS.with(|views| views.borrow_mut().remove(&view)) {
        // SAFETY: removing a WKWebView from its window, main thread; the
        // message handler is taken off so the controller lets it go.
        unsafe {
            let config: Retained<AnyObject> = msg_send![&*v.web, configuration];
            let controller: Retained<AnyObject> = msg_send![&*config, userContentController];
            let _: () = msg_send![&*controller, removeScriptMessageHandlerForName: &*ns("krate")];
            let _: () = msg_send![&*v.web, removeFromSuperview];
        }
    }
}
