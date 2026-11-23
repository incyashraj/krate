//! A web app inside a Krate app on Windows (IC-1004): WebView2 filling the
//! app's window, its files served from the private `krate-app://app/`
//! scheme (registered with the environment, answered by WebResourceRequested
//! from inside the app's web folder only, each with Krate's
//! Content-Security-Policy), the bridge script added before any of the
//! page's own, and the page's `chrome.webview.postMessage` handed to the
//! runtime. The browser data lives in a folder of this run's own, removed
//! with it: the page keeps what it keeps through the app.
//!
//! WebView2 is part of Windows 11 and of every updated Windows 10; where it
//! is missing, `open` says so in words instead of failing silently.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

use krate_adapter_common::appview;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    take_pwstr, CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{DefSubclassProc, SHCreateMemStream, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_SIZE};

const SUBCLASS_ID: usize = 0x4B52_5756;

struct View {
    hwnd: isize,
    controller: ICoreWebView2Controller,
    web: ICoreWebView2,
}

thread_local! {
    static VIEWS: RefCell<BTreeMap<u64, View>> = const { RefCell::new(BTreeMap::new()) };
}

fn fail(what: &str) -> impl Fn(webview2_com::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

fn client_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: the app's own window and a RECT on the stack.
    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
    rect
}

unsafe extern "system" fn follow_size(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_SIZE {
        let rect = client_rect(hwnd);
        VIEWS.with(|views| {
            for v in views
                .borrow()
                .values()
                .filter(|v| v.hwnd == hwnd.0 as isize)
            {
                // SAFETY: the controller of a view in this window.
                let _ = unsafe { v.controller.SetBounds(rect) };
            }
        });
    }
    // SAFETY: winit's own procedure handles the message as before.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// The answer to one request of the page's.
fn respond(
    env: &ICoreWebView2Environment,
    folder: &std::path::Path,
    start: &str,
    policy: &str,
    uri: &str,
) -> windows::core::Result<ICoreWebView2WebResourceResponse> {
    let (status, reason, kind, body) = match appview::resolve(uri)
        .map_err(|r| format!("{r:?}"))
        .and_then(|rel| match appview::read_file(folder, &rel) {
            Ok(bytes) => Ok((rel, bytes)),
            Err(_) if appview::is_route(&rel) => {
                appview::read_file(folder, start).map(|b| (start.to_string(), b))
            }
            Err(why) => Err(why),
        }) {
        Ok((rel, bytes)) => (200, "OK", appview::content_type(&rel), bytes),
        Err(why) => (
            404,
            "Not Found",
            "text/plain; charset=utf-8",
            why.into_bytes(),
        ),
    };
    let headers = format!(
        "Content-Type: {kind}\r\nContent-Security-Policy: {policy}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff"
    );
    // SAFETY: an in-memory stream of the bytes and a response the
    // environment makes, both owned by COM reference counts.
    unsafe {
        let stream = SHCreateMemStream(Some(&body));
        env.CreateWebResourceResponse(
            stream.as_ref(),
            status,
            &HSTRING::from(reason),
            &HSTRING::from(headers),
        )
    }
}

/// Fill the window `hwnd` with the web app in `folder`.
pub(crate) fn open(
    hwnd: isize,
    view: u64,
    folder: PathBuf,
    start: &str,
    preload: &str,
    granted: &[(String, String)],
) -> Result<(), String> {
    let start = appview::resolve_path(start).map_err(|r| format!("{r:?}"))?;
    let policy = appview::csp(granted);
    let bridge = appview::bridge_script(appview::Channel::WebView2, preload);
    let window = HWND(hwnd as *mut _);
    // SAFETY: COM on the window's (main) thread; already initialised by
    // winit for drag and drop, in which case this is a harmless S_FALSE.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

    let data = std::env::temp_dir().join(format!("krate-webview-{}-{view}", std::process::id()));
    let data = HSTRING::from(data.as_os_str());
    let options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: the options object Krate made; one custom scheme, secure,
    // with an authority (`krate-app://app/`).
    unsafe {
        let scheme = CoreWebView2CustomSchemeRegistration::new(appview::SCHEME.to_string());
        scheme.set_treat_as_secure(true);
        scheme.set_has_authority_component(true);
        options.set_scheme_registrations(vec![Some(scheme.into())]);
    }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let (tx, rx) = std::sync::mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: WebView2's loader with Krate's options and data folder.
            unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    &data,
                    &options,
                    &handler,
                )
            }
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error, env| {
            error?;
            tx.send(env.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("the environment's receiver waits");
            Ok(())
        }),
    )
    .map_err(|e| {
        format!(
            "this computer has no WebView2 runtime ({e}); install it from Microsoft (\"WebView2 Runtime\") and open the app again"
        )
    })?;
    let env = rx
        .recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("WebView2: {e}"))?;

    let (tx, rx) = std::sync::mpsc::channel();
    let made = env.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: a controller hosted in the app's own window.
            unsafe { made.CreateCoreWebView2Controller(window, &handler) }
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |error, controller| {
            error?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("the controller's receiver waits");
            Ok(())
        }),
    )
    .map_err(fail("WebView2 could not start in the window"))?;
    let controller = rx
        .recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("WebView2: {e}"))?;

    // SAFETY: the controller and its web view, configured on the thread
    // that made them; every handler below runs on this thread too.
    unsafe {
        controller
            .SetBounds(client_rect(window))
            .map_err(|e| e.to_string())?;
        controller.SetIsVisible(true).map_err(|e| e.to_string())?;
        let web = controller.CoreWebView2().map_err(|e| e.to_string())?;
        if let Ok(settings) = web.Settings() {
            let _ = settings.SetAreDevToolsEnabled(std::env::var_os("KRATE_WEB_INSPECT").is_some());
            let _ = settings.SetIsStatusBarEnabled(false);
        }
        web.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(bridge), None)
            .map_err(|e| e.to_string())?;
        let filter = HSTRING::from(format!("{}://*", appview::SCHEME));
        web.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
            .map_err(|e| e.to_string())?;
        let mut token = Default::default();
        let (served_env, served_folder, served_start) =
            (env.clone(), folder.clone(), start.clone());
        web.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_web, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let request = args.Request()?;
                let mut uri = PWSTR::null();
                request.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                let response = respond(&served_env, &served_folder, &served_start, &policy, &uri)?;
                args.SetResponse(&response)
            })),
            &mut token,
        )
        .map_err(|e| e.to_string())?;
        web.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_web, args| {
                if let Some(args) = args {
                    let mut text = PWSTR::null();
                    if args.TryGetWebMessageAsString(&mut text).is_ok() {
                        appview::push_message(view, take_pwstr(text));
                    }
                }
                Ok(())
            })),
            &mut token,
        )
        .map_err(|e| e.to_string())?;
        web.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_web, args| {
                if let Some(args) = args {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    if !appview::navigation_allowed(&take_pwstr(uri)) {
                        args.SetCancel(true)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )
        .map_err(|e| e.to_string())?;
        // A link that asks for a new window opens nothing.
        web.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_web, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )
        .map_err(|e| e.to_string())?;
        let _ = SetWindowSubclass(window, Some(follow_size), SUBCLASS_ID, 0);
        web.Navigate(&HSTRING::from(appview::start_address(&start)))
            .map_err(|e| e.to_string())?;
        VIEWS.with(|views| {
            views.borrow_mut().insert(
                view,
                View {
                    hwnd,
                    controller,
                    web,
                },
            )
        });
    }
    Ok(())
}

/// Hand the page a message.
pub(crate) fn post(view: u64, message: &str) -> Result<(), String> {
    let script = HSTRING::from(appview::deliver_script(message));
    VIEWS.with(|views| {
        let views = views.borrow();
        let v = views.get(&view).ok_or("no such view")?;
        // SAFETY: the web view on its own thread; no completion wanted.
        unsafe { v.web.ExecuteScript(&script, None) }.map_err(|e| e.to_string())
    })
}

/// Take a view away.
pub(crate) fn close(view: u64) {
    if let Some(v) = VIEWS.with(|views| views.borrow_mut().remove(&view)) {
        // SAFETY: closing the controller this thread made.
        let _ = unsafe { v.controller.Close() };
    }
}
