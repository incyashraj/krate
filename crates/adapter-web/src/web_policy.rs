//! What an app may do in a tab, decided by the same code the desktop uses
//! (IC-956 Phase 1).
//!
//! - [`consent_rows`]: the consent screen's lines, in `Capability::plain`'s
//!   words (the CLI, the hub's approval screen and the Player say the same
//!   thing), each marked with whether a tab can do it at all.
//! - [`WebGrants`]: the session's grants, as a `krate_policy::SessionPolicy`,
//!   and the `net.connect` judgement a request gets before the page is
//!   asked to fetch: the URL parsed by `parse_url_endpoint`, the
//!   `net.connect:<host>:<port>` capability checked, every app header held
//!   to `check_app_http_header`. Unchanged from the desktop.

use krate_adapter_common::net::{
    check_app_http_header, parse_url_endpoint, PlainHttpError, MAX_APP_HTTP_BODY_BYTES,
    MAX_APP_HTTP_HEADERS,
};
use krate_manifest::Capability;
use krate_policy::SessionPolicy;
use serde::Deserialize;
use serde_json::{json, Value};

/// Whether a browser tab has an equivalent for this capability: the list
/// lives with the capability itself (`Capability::works_in_a_tab`), so
/// check-app warns from the same one.
pub fn works_in_a_tab(cap: &Capability) -> bool {
    cap.works_in_a_tab()
}

#[derive(Debug, Deserialize)]
struct Requested {
    cap: String,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    required: bool,
}

/// The consent screen for a manifest's `capabilities` (the judgement's
/// `manifest.capabilities`, as the validator reports them): one row per
/// capability a person is asked about, in plain words, with the app's own
/// reason, whether it is required, and whether a tab can do it.
///
/// Capabilities every app has without asking (`is_default_granted`) are not
/// rows, as on desktop. A capability the manifest crate refuses to parse
/// is a row with `known: false`, so it is shown rather than dropped.
pub fn consent_rows(capabilities_json: &str) -> Value {
    let requested: Vec<Requested> = match serde_json::from_str(capabilities_json) {
        Ok(list) => list,
        Err(err) => return json!({ "error": format!("not a capability list: {err}") }),
    };
    let rows: Vec<Value> = requested
        .into_iter()
        .filter_map(|req| match req.cap.parse::<Capability>() {
            Ok(cap) if cap.is_default_granted() => None,
            Ok(cap) => Some(json!({
                "cap": cap.to_string(),
                "plain": cap.plain(),
                "rationale": req.rationale,
                "required": req.required,
                "in_tab": works_in_a_tab(&cap),
                "known": true,
            })),
            Err(_) => Some(json!({
                "cap": req.cap,
                "plain": req.cap,
                "rationale": req.rationale,
                "required": req.required,
                "in_tab": false,
                "known": false,
            })),
        })
        .collect();
    json!({ "rows": rows })
}

/// The grants one run in a tab holds.
#[derive(Debug, Clone)]
pub struct WebGrants {
    policy: SessionPolicy,
}

/// A `net-error` in the WIT shape jco expects.
fn net_error(tag: &str, words: Option<&str>) -> Value {
    match words {
        Some(words) => json!({ "tag": tag, "val": words }),
        None => json!({ "tag": tag }),
    }
}

impl WebGrants {
    /// The person allowed `granted` (capability strings) on the consent
    /// screen; every app also holds the defaults, as on desktop. Anything
    /// that does not parse is dropped, never widened.
    pub fn new(granted: &[String]) -> Self {
        let caps = granted
            .iter()
            .filter_map(|cap| cap.parse::<Capability>().ok())
            .chain(krate_manifest::default_granted_capabilities());
        Self {
            policy: SessionPolicy::from_grants(caps),
        }
    }

    /// Whether the run may use `cap` (a capability string).
    pub fn allows(&self, cap: &str) -> bool {
        cap.parse::<Capability>()
            .map(|cap| self.policy.allows(&cap))
            .unwrap_or(false)
    }

    /// Whether a file named `name` the person dropped on the app may reach
    /// it, as the desktop decides (`allows_dropzone` in the runtime): the
    /// `ui.dropzone:<mime>` capability, the media type from the file's name
    /// by the one table both use. An empty name is a hover's question.
    pub fn allows_drop(&self, name: &str) -> bool {
        let mime = krate_adapter_common::mime::mime_for_file_name(name);
        Capability::new("ui", "dropzone", Some(mime))
            .map(|cap| self.policy.allows(&cap))
            .unwrap_or(false)
    }

    /// Judge one HTTP request before the page fetches it: `Ok` with the
    /// host and port it reaches, or the `net-error` the app gets, exactly
    /// as `UapiDispatcher::check_net_request` and the plain adapter's header
    /// rule answer on desktop.
    pub fn check_http(
        &self,
        url: &str,
        headers: &[(String, String)],
        body_len: usize,
    ) -> Result<(String, u16), Value> {
        let endpoint = parse_url_endpoint(url).map_err(|_| net_error("invalid-url", None))?;
        let cap = Capability::new(
            "net",
            "connect",
            Some(&format!("{}:{}", endpoint.host, endpoint.port)),
        )
        .map_err(|_| net_error("invalid-url", None))?;
        if !self.policy.allows(&cap) {
            return Err(net_error("permission-denied", None));
        }
        if headers.len() > MAX_APP_HTTP_HEADERS {
            return Err(net_error("protocol", Some("invalid HTTP header")));
        }
        for (name, value) in headers {
            check_app_http_header(name, value).map_err(|err| match err {
                PlainHttpError::HostControlledHeader => {
                    net_error("protocol", Some("host-controlled HTTP header"))
                }
                _ => net_error("protocol", Some("invalid HTTP header")),
            })?;
        }
        if body_len > MAX_APP_HTTP_BODY_BYTES {
            return Err(net_error("body-too-large", None));
        }
        Ok((endpoint.host, endpoint.port))
    }
}

/// The largest response body a tab hands an app, the desktop's limit.
pub const MAX_RESPONSE_BYTES: usize = MAX_APP_HTTP_BODY_BYTES;

#[cfg(test)]
mod tests {
    use super::*;

    /// The consent rows say what the desktop says, skip what every app has,
    /// and mark what a tab cannot do.
    #[test]
    fn consent_rows_use_the_desktop_words() {
        let rows = consent_rows(
            r#"[{"cap":"ui.window:create","rationale":"Open the window","required":true},
                {"cap":"store.kv","rationale":"Save your list","required":true},
                {"cap":"store.org","rationale":"The team's inspections","required":true},
                {"cap":"camera.capture","rationale":"Scan","required":false},
                {"cap":"net.connect:api.example.com:443","rationale":"Weather","required":true}]"#,
        );
        let rows = rows["rows"].as_array().unwrap().clone();
        let kv = rows.iter().find(|r| r["cap"] == "store.kv").unwrap();
        assert_eq!(kv["plain"], "save its own settings and data");
        assert_eq!(kv["in_tab"], true);
        // An organisation's team data works in a tab too (G1).
        let org = rows.iter().find(|r| r["cap"] == "store.org").unwrap();
        assert_eq!(org["in_tab"], true);
        // Sound plays in a tab (K-1134); listening does not.
        let sound: Capability = "audio.playback".parse().unwrap();
        assert!(works_in_a_tab(&sound));
        let mic: Capability = "audio.capture".parse().unwrap();
        assert!(!works_in_a_tab(&mic));
        let cam = rows.iter().find(|r| r["cap"] == "camera.capture").unwrap();
        assert_eq!(cam["in_tab"], false);
        assert_eq!(cam["plain"], "use your camera");
        let net = rows
            .iter()
            .find(|r| r["cap"] == "net.connect:api.example.com:443")
            .unwrap();
        assert_eq!(
            net["plain"],
            "connect to api.example.com:443 over the network"
        );
    }

    /// The `net.connect` judgement: only the granted host and port, any
    /// other refused before the page fetches; transport headers refused;
    /// a bad URL refused.
    #[test]
    fn a_request_reaches_only_a_granted_host() {
        let grants = WebGrants::new(&["net.connect:127.0.0.1:8912".to_string()]);
        assert_eq!(
            grants.check_http("http://127.0.0.1:8912/hello", &[], 0),
            Ok(("127.0.0.1".to_string(), 8912))
        );
        assert_eq!(
            grants.check_http("http://127.0.0.1:8913/hello", &[], 0),
            Err(json!({ "tag": "permission-denied" }))
        );
        assert_eq!(
            grants.check_http("https://evil.example/", &[], 0),
            Err(json!({ "tag": "permission-denied" }))
        );
        assert_eq!(
            grants.check_http(
                "http://127.0.0.1:8912/",
                &[("Host".to_string(), "evil".to_string())],
                0
            ),
            Err(json!({ "tag": "protocol", "val": "host-controlled HTTP header" }))
        );
        assert_eq!(
            grants.check_http("ftp://127.0.0.1:8912/", &[], 0),
            Err(json!({ "tag": "invalid-url" }))
        );
        assert_eq!(
            grants.check_http("http://user@127.0.0.1:8912/", &[], 0),
            Err(json!({ "tag": "invalid-url" }))
        );
        assert!(!grants.allows("store.kv"));
        assert!(WebGrants::new(&["store.kv".to_string()]).allows("store.kv"));
    }

    /// A dropped file reaches the app only under its `ui.dropzone` grant,
    /// judged by the file's name as the desktop judges it; a hover (no
    /// name) only when the app takes anything. The file dialogs work in a
    /// tab; the folder dialog does not.
    #[test]
    fn a_drop_reaches_only_an_app_that_takes_that_kind_of_file() {
        let text = WebGrants::new(&["ui.dropzone:text/plain".to_string()]);
        assert!(text.allows_drop("notes.TXT"));
        assert!(!text.allows_drop("photo.png"));
        assert!(!text.allows_drop(""));
        let images = WebGrants::new(&["ui.dropzone:image/*".to_string()]);
        assert!(images.allows_drop("photo.png"));
        assert!(!images.allows_drop("notes.txt"));
        assert!(!WebGrants::new(&[]).allows_drop("notes.txt"));
        let open: Capability = "ui.dialog:file-open".parse().unwrap();
        let save: Capability = "ui.dialog:file-save".parse().unwrap();
        assert!(works_in_a_tab(&open) && works_in_a_tab(&save));
        // The folder dialog is not promised in a tab.
        let folder: Capability = "ui.dialog:open-folder".parse().unwrap();
        assert!(!works_in_a_tab(&folder));
    }
}
