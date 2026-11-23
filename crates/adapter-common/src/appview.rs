//! A web app inside a Krate app (IC-1004): the parts every system shares.
//!
//! An app made of web files (a built Vite or React folder) ships them in its
//! assets and shows them full-window in the system's web view. Every host
//! serves the files from a private address -- `krate-app://app/<path>` --
//! through the same path rules here: inside the app's folder only, no `..`,
//! no absolute path, no hidden trick through percent-encoding. Every
//! response carries the same Content-Security-Policy, so the page's own
//! fetches, websockets, images and scripts reach only its own files and the
//! hosts the person granted the app `net.connect` to.
//!
//! The page and the app talk through one small bridge, the same script on
//! every system: the page calls `window.krate.post(text)` and the app's
//! code gets the text; the app posts back and the page's `krate.onmessage`
//! runs. Messages are text, at most [`MAX_MESSAGE`] bytes, and a page that
//! sends faster than the app reads loses the oldest past [`MAX_QUEUED`].

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

/// The private scheme the page is served from.
pub const SCHEME: &str = "krate-app";
/// The host part of the private address.
pub const HOST: &str = "app";
/// The longest message either way, in bytes.
pub const MAX_MESSAGE: usize = 1024 * 1024;
/// How many page messages wait for the app before the oldest are dropped.
pub const MAX_QUEUED: usize = 1024;
/// The largest file the scheme serves.
pub const MAX_FILE: u64 = 64 * 1024 * 1024;
/// How many web views one app may have open.
pub const MAX_VIEWS: usize = 4;
/// The first view id: high, so a view's `action` event cannot be mistaken
/// for a menu item's or a widget's.
pub const FIRST_VIEW_ID: u64 = 0x4B52_4156_0000_0001;

/// Why a request for a file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not the app's private address at all.
    NotOurs,
    /// A path that would leave the app's folder, or cannot be a file name.
    Outside(String),
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The file a request asks for, as a path relative to the app's web
/// folder (`assets/<root>/`), using `/` between parts. `address` is the
/// whole URL (`krate-app://app/assets/index.js?v=2#top`); a path ending in
/// `/` or naming nothing is its `index.html`.
pub fn resolve(address: &str) -> Result<String, Refusal> {
    let prefix = format!("{SCHEME}://{HOST}");
    let rest = address
        .get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(&prefix))
        .map(|_| &address[prefix.len()..])
        .ok_or(Refusal::NotOurs)?;
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#'))
    {
        // `krate-app://app.evil/...` or `krate-app://app:80/...`.
        return Err(Refusal::NotOurs);
    }
    let path = rest.split(['?', '#']).next().unwrap_or("");
    resolve_path(path)
}

/// [`resolve`] for the path part alone (`/assets/index.js`).
pub fn resolve_path(path: &str) -> Result<String, Refusal> {
    let decoded = percent_decode(path)
        .ok_or_else(|| Refusal::Outside(format!("{path} is not a file name")))?;
    if decoded.contains('\\') || decoded.contains('\0') {
        return Err(Refusal::Outside(format!("{path} is not a file name")));
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(Refusal::Outside(format!("{path} leaves the app's folder"))),
            p if p.contains(':') => {
                return Err(Refusal::Outside(format!("{path} is not a file name")))
            }
            p => parts.push(p),
        }
    }
    if decoded.is_empty() || decoded.ends_with('/') {
        parts.push("index.html");
    }
    Ok(parts.join("/"))
}

/// The address a view opens first. `index.html` opens as the folder
/// itself (`krate-app://app/`), so an app with a router that reads the
/// address (React Router's BrowserRouter, Vue Router's history mode) starts
/// on its home route rather than on a route called `/index.html`.
pub fn start_address(start: &str) -> String {
    if start == "index.html" {
        format!("{SCHEME}://{HOST}/")
    } else {
        format!("{SCHEME}://{HOST}/{start}")
    }
}

/// Whether a request that found no file is one of the app's own routes
/// (`/settings`, `/task/12`): its last part names no file type, so the
/// start page answers it, as every host of a single-page app does.
pub fn is_route(relative: &str) -> bool {
    !relative.rsplit('/').next().unwrap_or("").contains('.')
}

/// The media type a web view needs to run a file: scripts as JavaScript
/// (module scripts are refused otherwise), wasm as wasm.
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("ico") => "image/x-icon",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => crate::mime::mime_for_file_name(path),
    }
}

/// The Content-Security-Policy every served file carries: the page's own
/// files, inline scripts and styles (a built app uses them), and the
/// granted hosts for its connections, pictures, fonts and media -- nothing
/// else. `granted` is each `net.connect` grant's (host, port).
pub fn csp(granted: &[(String, String)]) -> String {
    let mut hosts = String::new();
    for (host, port) in granted {
        // A host pattern a CSP source can say; anything else is left out.
        if host.is_empty()
            || !host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'))
        {
            continue;
        }
        let port = match port.as_str() {
            "443" => String::new(),
            "*" => ":*".to_string(),
            p if p.chars().all(|c| c.is_ascii_digit()) => format!(":{p}"),
            _ => continue,
        };
        hosts.push_str(&format!(" https://{host}{port} wss://{host}{port}"));
    }
    let own = format!("{SCHEME}:");
    format!(
        "default-src {own}{hosts}; \
         script-src {own} 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval'; \
         style-src {own} 'unsafe-inline'{hosts}; \
         img-src {own} data: blob:{hosts}; \
         font-src {own} data:{hosts}; \
         media-src {own} data: blob:{hosts}; \
         connect-src {own}{hosts}; \
         worker-src {own} blob:; \
         frame-src 'none'; object-src 'none'; base-uri {own}; form-action 'none'"
    )
}

/// Whether the page may go to an address as a whole page (a link, a
/// redirect): its own files only. A link to a granted host is opened in the
/// person's browser by the app if it wants, never inside the app.
pub fn navigation_allowed(address: &str) -> bool {
    resolve(address).is_ok() || address == "about:blank"
}

/// How a host hands the page a message from the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// WKWebView (Mac, iPhone) and WebKitGTK: `webkit.messageHandlers.krate`.
    Webkit,
    /// WebView2 on Windows: `chrome.webview.postMessage`.
    WebView2,
    /// An iframe in the web Player: `parent.postMessage`.
    Frame,
}

/// The bridge script, run before any of the page's own: `window.krate`
/// with `post`, `onmessage`, the app's `preload` (JSON, or null), and --
/// when the preload carries a `localStorage` object -- a localStorage the
/// app keeps in its own store, so an unmodified web app's data is saved
/// through Krate.
pub fn bridge_script(channel: Channel, preload: &str) -> String {
    let send = match channel {
        Channel::Webkit => "window.webkit.messageHandlers.krate.postMessage(t)",
        Channel::WebView2 => "window.chrome.webview.postMessage(t)",
        Channel::Frame => "window.parent.postMessage({krate: t}, '*')",
    };
    // The preload goes in as a JSON string literal, parsed in the page, so no
    // text the app chose is ever read as script.
    let preload_literal = json_string(preload);
    format!(
        r#"(function () {{
  if (window.krate) return;
  var MAX = {MAX_MESSAGE};
  var preload = null;
  try {{ preload = JSON.parse({preload_literal}); }} catch (e) {{ preload = null; }}
  var listeners = [];
  var krate = {{
    preload: preload,
    onmessage: null,
    post: function (message) {{
      var t = typeof message === 'string' ? message : JSON.stringify(message);
      if (t.length > MAX) throw new Error('a message to the app is at most ' + MAX + ' bytes');
      {send};
    }},
    addEventListener: function (kind, f) {{ if (kind === 'message') listeners.push(f); }},
    removeEventListener: function (kind, f) {{ listeners = listeners.filter(function (g) {{ return g !== f; }}); }}
  }};
  Object.defineProperty(window, 'krate', {{ value: krate, writable: false, configurable: false }});
  window.__krateDeliver = function (t) {{
    var e = new MessageEvent('message', {{ data: t }});
    try {{ if (typeof krate.onmessage === 'function') krate.onmessage(e); }} catch (err) {{ console.error(err); }}
    listeners.slice().forEach(function (f) {{ try {{ f(e); }} catch (err) {{ console.error(err); }} }});
  }};
  if (preload && preload.localStorage && typeof preload.localStorage === 'object') {{
    var data = {{}};
    Object.keys(preload.localStorage).forEach(function (k) {{ data[k] = String(preload.localStorage[k]); }});
    var send = function (o) {{ o.krate = 'ls'; krate.post(JSON.stringify(o)); }};
    var store = {{
      getItem: function (k) {{ k = String(k); return Object.prototype.hasOwnProperty.call(data, k) ? data[k] : null; }},
      setItem: function (k, v) {{ k = String(k); v = String(v); data[k] = v; send({{ op: 'set', key: k, value: v }}); }},
      removeItem: function (k) {{ k = String(k); delete data[k]; send({{ op: 'remove', key: k }}); }},
      clear: function () {{ data = {{}}; send({{ op: 'clear' }}); }},
      key: function (i) {{ var ks = Object.keys(data); return i < ks.length ? ks[i] : null; }}
    }};
    Object.defineProperty(store, 'length', {{ get: function () {{ return Object.keys(data).length; }} }});
    try {{ Object.defineProperty(window, 'localStorage', {{ get: function () {{ return store; }}, configurable: true }}); }} catch (e) {{}}
  }}
}})();"#
    )
}

/// The script a host runs to hand the page one message from the app.
pub fn deliver_script(message: &str) -> String {
    format!(
        "window.__krateDeliver && window.__krateDeliver({});",
        json_string(message)
    )
}

/// Text as a JSON string literal (also a valid JavaScript one, with the two
/// line separators JavaScript once refused escaped too).
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Check a message's size either way.
pub fn check_message(text: &str) -> Result<(), String> {
    if text.len() > MAX_MESSAGE {
        Err(format!(
            "a message is at most {MAX_MESSAGE} bytes; this one is {}",
            text.len()
        ))
    } else {
        Ok(())
    }
}

static INBOX: Mutex<BTreeMap<u64, VecDeque<String>>> = Mutex::new(BTreeMap::new());

/// A page sent its app a message. Any thread. Oversized messages are
/// dropped; past [`MAX_QUEUED`] the oldest goes.
pub fn push_message(view: u64, text: String) {
    if check_message(&text).is_err() {
        return;
    }
    if let Ok(mut inbox) = INBOX.lock() {
        let queue = inbox.entry(view).or_default();
        if queue.len() >= MAX_QUEUED {
            queue.pop_front();
        }
        queue.push_back(text);
    }
}

/// Everything a page sent since the last call, oldest first.
pub fn take_messages(view: u64) -> Vec<String> {
    INBOX
        .lock()
        .ok()
        .and_then(|mut inbox| inbox.remove(&view))
        .map(Vec::from)
        .unwrap_or_default()
}

/// The views with messages waiting.
pub fn views_waiting() -> Vec<u64> {
    INBOX
        .lock()
        .map(|inbox| {
            inbox
                .iter()
                .filter(|(_, q)| !q.is_empty())
                .map(|(v, _)| *v)
                .collect()
        })
        .unwrap_or_default()
}

/// Read a file the page asked for: `folder` is the app's web folder on
/// disk, `relative` what [`resolve`] answered. The file must be inside the
/// folder after links are followed, and not too large.
pub fn read_file(folder: &std::path::Path, relative: &str) -> Result<Vec<u8>, String> {
    let root = folder
        .canonicalize()
        .map_err(|e| format!("the app's web folder is missing: {e}"))?;
    let path = root.join(relative);
    let real = path
        .canonicalize()
        .map_err(|_| format!("{relative} is not in the app"))?;
    if !real.starts_with(&root) {
        return Err(format!("{relative} leaves the app's folder"));
    }
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("{relative} is not a file"));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("{relative} is larger than {MAX_FILE} bytes"));
    }
    std::fs::read(&real).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_private_address_names_a_file_in_the_folder() {
        assert_eq!(resolve("krate-app://app/").unwrap(), "index.html");
        assert_eq!(resolve("krate-app://app").unwrap(), "index.html");
        assert_eq!(
            resolve("krate-app://app/index.html?x=1#top").unwrap(),
            "index.html"
        );
        assert_eq!(
            resolve("krate-app://app/assets/index-B2x.js").unwrap(),
            "assets/index-B2x.js"
        );
        assert_eq!(resolve("KRATE-APP://app/a%20b.css").unwrap(), "a b.css");
        assert_eq!(resolve("krate-app://app/docs/").unwrap(), "docs/index.html");
        assert_eq!(resolve("krate-app://app/./a//b.js").unwrap(), "a/b.js");
    }

    #[test]
    fn a_path_out_of_the_folder_is_refused_however_it_is_spelled() {
        for bad in [
            "krate-app://app/../secret",
            "krate-app://app/a/../../b",
            "krate-app://app/%2e%2e/secret",
            "krate-app://app/%2E%2E%2Fsecret",
            "krate-app://app/..%5csecret",
            "krate-app://app/a%00.js",
            "krate-app://app/C:/x",
            "krate-app://app/%zz",
        ] {
            assert!(
                matches!(resolve(bad), Err(Refusal::Outside(_))),
                "{bad} should be refused"
            );
        }
        for not_ours in [
            "https://app/index.html",
            "krate-app://app.evil/x",
            "krate-app://app:80/x",
            "krate-app://other/x",
            "file:///etc/passwd",
        ] {
            assert_eq!(resolve(not_ours), Err(Refusal::NotOurs), "{not_ours}");
        }
        assert!(navigation_allowed("krate-app://app/about.html"));
        assert!(!navigation_allowed("https://example.com/"));
    }

    #[test]
    fn a_router_starts_home_and_its_routes_reach_the_start_page() {
        assert_eq!(start_address("index.html"), "krate-app://app/");
        assert_eq!(
            start_address("app/main.html"),
            "krate-app://app/app/main.html"
        );
        assert!(is_route("settings"));
        assert!(is_route("task/12"));
        assert!(!is_route("assets/index-abc.js"));
        assert!(!is_route("favicon.ico"));
    }

    #[test]
    fn scripts_are_served_as_javascript() {
        assert!(content_type("assets/index.js").starts_with("text/javascript"));
        assert!(content_type("a.MJS").starts_with("text/javascript"));
        assert_eq!(content_type("x.wasm"), "application/wasm");
        assert!(content_type("index.html").starts_with("text/html"));
    }

    #[test]
    fn the_policy_names_only_granted_hosts() {
        let policy = csp(&[
            ("api.example.com".into(), "443".into()),
            ("*.cdn.net".into(), "8443".into()),
            ("bad host; script-src *".into(), "443".into()),
        ]);
        assert!(
            policy.contains("connect-src krate-app: https://api.example.com wss://api.example.com")
        );
        assert!(policy.contains("https://*.cdn.net:8443"));
        assert!(!policy.contains("bad host"));
        assert!(!policy.contains("script-src *"));
        assert!(csp(&[]).contains("connect-src krate-app:;"));
    }

    #[test]
    fn app_text_in_a_script_stays_a_string() {
        let evil = "\");alert(1);//\u{2028}</script>";
        let lit = json_string(evil);
        assert!(!lit.contains("</script>"));
        assert!(lit.starts_with('"') && lit.ends_with('"'));
        assert!(lit.contains("\\\")"));
        assert!(lit.contains("\\u2028"));
        let bridge = bridge_script(Channel::Webkit, evil);
        assert!(bridge.contains("JSON.parse(\"\\\");alert(1);"));
    }

    #[test]
    fn messages_are_bounded_and_queued_per_view() {
        let view = FIRST_VIEW_ID + 900;
        assert!(check_message(&"x".repeat(MAX_MESSAGE)).is_ok());
        assert!(check_message(&"x".repeat(MAX_MESSAGE + 1)).is_err());
        push_message(view, "x".repeat(MAX_MESSAGE + 1));
        assert!(
            take_messages(view).is_empty(),
            "an oversized message is dropped"
        );
        for i in 0..(MAX_QUEUED + 5) {
            push_message(view, i.to_string());
        }
        let got = take_messages(view);
        assert_eq!(got.len(), MAX_QUEUED);
        assert_eq!(got[0], "5");
        assert!(take_messages(view).is_empty());
    }

    #[test]
    fn a_file_is_read_only_from_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("krate-appview-{}", std::process::id()));
        let web = dir.join("web");
        std::fs::create_dir_all(web.join("assets")).unwrap();
        std::fs::write(web.join("index.html"), "<p>hi</p>").unwrap();
        std::fs::write(dir.join("secret.txt"), "no").unwrap();
        assert_eq!(read_file(&web, "index.html").unwrap(), b"<p>hi</p>");
        assert!(read_file(&web, "../secret.txt").is_err());
        assert!(read_file(&web, "assets").is_err(), "a folder is not a file");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("secret.txt"), web.join("link.txt")).unwrap();
            assert!(
                read_file(&web, "link.txt").is_err(),
                "a link out of the folder"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
