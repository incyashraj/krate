//! Opening an organisation's private app on one device, online or off
//! (IC-948 G and H, Addendum 5), and what its run is given: its team data
//! (`store.org`, IC-951) and its company connections (`net.connection`,
//! IC-952).
//!
//! The same code on a computer (crates/cli) and a phone (crates/player-android
//! and crates/player-ios, K-1171): what differs between them -- where Krate
//! keeps its things, who is signed in, how the device is named, how a person
//! is told something -- is a [`Device`].
//!
//! Every door that runs an app reaches [`gate_opened`] once the bytes are in
//! hand. Only a bundle that carries an organisation's mark
//! (`ext/krate/org/private.json`) under a signature that holds is an
//! organisation's private app, and only such a bundle ever talks to a hub
//! here. An unmarked bundle, signed or not, opens without a single request.
//! A marked bundle whose signature does not hold is refused in words: the
//! mark says it is private, and nothing proves it is the organisation's.
//!
//! - **Online**, the hub's answer is the check-in. A member opens it, and the
//!   offline lease is fetched, or refreshed once it is past half its life. A
//!   refused check-in (removed, never shared) deletes the lease and nothing
//!   runs; the person's app data is not touched.
//! - **Offline**, a private app opens only on a stored lease for this
//!   organisation, digest and sign-in that verifies under the pinned hub key
//!   and has not run out ([`krate_lease::check`]).
//!
//! Before the first private open the person is shown the offline terms with
//! the exact date, and again whenever a shorter class replaces a longer one.
//! Leases live in `<home>/leases/<digest>.json`.
//!
//! The honest limit, said in the docs too: this is enforced by Krate on the
//! device. Somebody who strips the mark and the signature has an unsigned
//! copy of the code, not the organisation's app, and Krate cannot stop a
//! copy.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub mod connections;

/// The public hub, which every Krate sends its sign-in to.
pub const PUBLIC_HUB: &str = "https://hub.krate.tech";

/// The person signed in on a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    /// As the hub named them at sign-in: a GitHub login or an address.
    pub login: String,
    /// The hub session, sent only to a Krate hub as `Bearer <token>`.
    pub token: String,
}

/// What differs between the devices that open an organisation's app: a
/// computer and a phone run the same rules over these.
pub trait Device {
    /// Krate's own folder on this device: `leases/`, `org-data/` and
    /// `org-keys/` live under it. `~/.krate` on a computer; the Player's
    /// private storage, which no other app reads, on a phone.
    fn home(&self) -> PathBuf;
    /// This device's machine key: it seals the organisation key at rest, so
    /// a copied file does not carry it to another device.
    fn machine_key(&self) -> Vec<u8>;
    /// Who is signed in here, if anyone.
    fn signed_in(&self) -> Option<SignedIn>;
    /// The hub this device uses when neither a link nor an earlier open
    /// names one. A refusal here (a policy that pins another hub) is the
    /// open's refusal.
    fn hub(&self) -> Result<String, GateError>;
    /// The hub a company policy pins (IC-949 [hub]): the only one asked.
    fn pinned_hub(&self) -> Option<String> {
        None
    }
    /// What this device is, in a sentence: "computer", "phone".
    fn kind(&self) -> &str;
    /// What an organisation's device list calls this device.
    fn name(&self, principal: Option<&str>) -> String;
    /// How a person signs in here, as an instruction ("sign in with `krate
    /// account login`").
    fn sign_in_words(&self) -> String;
    /// Show `words` to the person: the offline terms before a first open.
    fn tell(&self, title: &str, words: &str);
    /// A line for whoever watches the run (a computer's stderr).
    fn note(&self, words: &str) {
        let _ = words;
    }
    /// Is `url` on a Krate hub, the only kind of place a sign-in is sent?
    fn is_hub_link(&self, url: &str) -> bool {
        let Some(link) = origin(url) else {
            return false;
        };
        if let Some(pinned) = self.pinned_hub() {
            return link == pinned;
        }
        let configured = self.hub().ok().and_then(|hub| origin(&hub));
        configured.as_deref() == Some(link.as_str())
            || link == PUBLIC_HUB
            || link == "https://krate.tech"
    }
    /// The words for a refused download or check-in of a private release.
    fn refusal_words(&self, url: &str, reason: &str, message: &str, org: Option<&str>) -> String {
        refusal_words(self, url, reason, message, org)
    }
}

/// A refusal at the door: a stable reason for the open record, and the
/// words the person reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub reason: String,
    pub words: String,
}

/// Why an open did not go ahead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateError {
    /// Refused: this person may not open these bytes now.
    Refused(Refusal),
    /// Something went wrong that is not a refusal (the bytes could not be
    /// read, no hub is configured).
    Failed(String),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateError::Refused(refusal) => f.write_str(&refusal.words),
            GateError::Failed(words) => f.write_str(words),
        }
    }
}

impl std::error::Error for GateError {}

fn refused<T>(reason: &str, words: &str) -> Result<T, GateError> {
    Err(GateError::Refused(Refusal {
        reason: reason.to_string(),
        words: words.to_string(),
    }))
}

/// Where the bytes being opened came from.
pub enum Bytes<'a> {
    /// A file on this device; hashed only if it is signed.
    File(&'a Path),
    /// A download whose sha256 is already known, from this link.
    Fetched { sha256: &'a str, link: &'a str },
}

/// The scheme and host of `url`, lower-case; `None` for anything that is
/// not one (userinfo@host is not a hub: refused rather than guessed).
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.contains('@') || host.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    ))
}

/// The principal a person is named by: an address with `@` is
/// `email:<address>`, anything else a GitHub login, `github:<login>`. Both
/// lower-case. An explicit `github:` or `email:` prefix is kept.
pub fn principal_of(name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    if name.starts_with("github:") || name.starts_with("email:") {
        name
    } else if name.contains('@') {
        format!("email:{name}")
    } else {
        format!("github:{name}")
    }
}

/// A time the hub gave: an RFC 3339 string is shown as it is; unix seconds
/// (or milliseconds) become a UTC date.
pub fn when(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let Some(mut secs) = value.as_u64() else {
        return String::new();
    };
    if secs > 100_000_000_000 {
        secs /= 1000;
    }
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        (rest % 3600) / 60
    )
}

/// `Bearer <token>` for the person signed in on `device`, only when `hub`
/// is a Krate hub: the session goes nowhere else.
pub fn authorization_for<D: Device + ?Sized>(device: &D, hub: &str) -> Option<String> {
    device
        .signed_in()
        .filter(|_| device.is_hub_link(&format!("{hub}/")))
        .map(|identity| format!("Bearer {}", identity.token))
}

/// The signed-in person as a principal (`github:<login>`).
pub fn me<D: Device + ?Sized>(device: &D) -> Option<String> {
    device
        .signed_in()
        .map(|identity| principal_of(&identity.login))
}

/// The words for a refused download of a private release, for a device
/// whose sign-in is not the command line's. A computer words them itself.
pub fn refusal_words<D: Device + ?Sized>(
    device: &D,
    url: &str,
    reason: &str,
    message: &str,
    org: Option<&str>,
) -> String {
    let org_name = org.unwrap_or("the organisation that sent it");
    let me = device
        .signed_in()
        .map(|identity| identity.login.to_ascii_lowercase());
    let kind = device.kind();
    match reason {
        "not-signed-in" => match (&me, device.is_hub_link(url)) {
            (Some(_), false) => "this app is private, and the link is not on Krate's hub, so \
                 Krate did not send your sign-in and nothing ran: ask for a link from Krate's hub"
                .to_string(),
            (Some(login), true) => format!(
                "the hub did not accept your sign-in as {login} (it may have run out), so \
                 nothing ran: {} again, then open the same link again",
                device.sign_in_words()
            ),
            (None, _) => format!(
                "this app is private to an organisation and you are not signed in, so nothing \
                 was downloaded or run: {} as the account it was shared with, then open it again",
                device.sign_in_words()
            ),
        },
        "not-a-member" => {
            let me = me.unwrap_or_else(|| "this account".to_string());
            format!(
                "this app is private to {org_name}, and {me} is not one of its members or \
                 recipients, so nothing ran: ask an owner or admin of {org_name} to invite {me}"
            )
        }
        "removed" => format!(
            "you were removed from {org_name}, so this app no longer opens for you: ask an \
             owner of {org_name} if that is a mistake"
        ),
        "role-cannot" => format!(
            "your role in {org_name} cannot download its releases: ask an owner or admin of \
             {org_name}"
        ),
        "no-such-org" => {
            "the organisation that released this app is gone, so it no longer opens".to_string()
        }
        "digest-mismatch" => format!(
            "this is an app {org_name} has not approved (or no longer has), so nothing ran on \
             this {kind}: ask whoever sent it for the link its approval gave"
        ),
        "orgs-off" => "organisations are switched off on this hub, so its private apps do \
             not open: ask whoever runs it"
            .to_string(),
        "release-withdrawn" => match Some(message.trim()).filter(|m| !m.is_empty()) {
            Some(said) => {
                format!("this app no longer opens, here or anywhere it next connects: {said}")
            }
            None => format!(
                "{org_name} withdrew this version, so it no longer opens: ask an admin of \
                 {org_name} which version to use now"
            ),
        },
        "not-in-audience" => match Some(message.trim()).filter(|m| !m.is_empty()) {
            Some(said) => format!("this app does not open for you: {said}"),
            None => format!(
                "this app is only for some groups in {org_name}, and you are in none of them: \
                 ask an admin of {org_name} to add you"
            ),
        },
        _ => format!("the hub would not hand this app over: {message}"),
    }
}

/// Whether `slug` is shaped like an organisation's slug: 2 to 39 lower-case
/// letters, digits and hyphens, starting with a letter.
pub fn is_org_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (2..=39).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// The address that hands over an app's bytes, for a link someone copied
/// from a browser (K-1166). The hub's tab Player lives at `/run/...` and
/// answers with a web page; the same app's bytes are at `/a/<hash>`,
/// `/a/v<line>` (an app's current version) and, for an organisation's app,
/// `/orgs/<slug>/apps/<app-id>/current`. Only on a Krate hub, and only for
/// those exact shapes; anything else is returned as it was given.
pub fn bundle_address<'a, D: Device + ?Sized>(device: &D, url: &'a str) -> Cow<'a, str> {
    if !device.is_hub_link(url) {
        return Cow::Borrowed(url);
    }
    let Some(origin) = origin(url) else {
        return Cow::Borrowed(url);
    };
    let path = url[url.find("://").map_or(0, |i| i + 3)..]
        .split_once('/')
        .map_or("", |(_, rest)| rest)
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    let path = path.strip_suffix('/').unwrap_or(path);
    let Some(rest) = path.strip_prefix("run/") else {
        return Cow::Borrowed(url);
    };
    let hex = |s: &str, n: usize| {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [hash] if hex(hash, 64) => Cow::Owned(format!("{origin}/a/{hash}")),
        [line] if line.len() == 12 && line.starts_with('v') && hex(&line[1..], 11) => {
            Cow::Owned(format!("{origin}/a/{line}"))
        }
        ["org", slug, app]
            if is_org_slug(slug)
                && !app.is_empty()
                && app.len() <= 128
                && app
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                && app.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)
                }) =>
        {
            Cow::Owned(format!("{origin}/orgs/{slug}/apps/{app}/current"))
        }
        _ => Cow::Borrowed(url),
    }
}

/// An app's bytes from a link, at most `limit` of them: the address that
/// hands them over ([`bundle_address`]), with the person's session when --
/// and only when -- the link is on a Krate hub, so an organisation's private
/// release downloads for a member as it does on a computer (IC-946). A
/// refusal comes back in the device's words, and one that ends access ends
/// the lease for the bytes the link names.
pub fn download<D: Device + ?Sized>(
    device: &D,
    url: &str,
    limit: u64,
    user_agent: &str,
) -> Result<Vec<u8>, GateError> {
    use std::io::Read;
    let address = bundle_address(device, url).into_owned();
    let authorization = device
        .signed_in()
        .filter(|_| device.is_hub_link(&address))
        .map(|identity| format!("Bearer {}", identity.token));
    let mut request = agent(&address).get(&address).set("User-Agent", user_agent);
    if let Some(authorization) = &authorization {
        request = request.set("Authorization", authorization);
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let answer: Value = response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            return Err(match answer["error"].as_str() {
                Some(reason) if matches!(status, 401 | 403 | 404 | 410) => {
                    refused_link(device, &address, reason);
                    GateError::Refused(Refusal {
                        reason: reason.to_string(),
                        words: device.refusal_words(
                            &address,
                            reason,
                            answer["message"].as_str().unwrap_or(""),
                            answer["org"].as_str(),
                        ),
                    })
                }
                _ => GateError::Failed(format!("the hub answered {status}")),
            });
        }
        Err(err) => {
            return Err(GateError::Failed(format!(
                "the hub could not be reached ({err})"
            )))
        }
    };
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| GateError::Failed(format!("the download broke off ({err})")))?;
    if bytes.len() as u64 > limit {
        return Err(GateError::Failed(
            "that download is larger than any Krate app".to_string(),
        ));
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// What is kept about one private app on this device.
// ---------------------------------------------------------------------------

fn leases_dir<D: Device + ?Sized>(device: &D) -> PathBuf {
    device.home().join("leases")
}

fn record_path<D: Device + ?Sized>(device: &D, digest: &str) -> PathBuf {
    leases_dir(device).join(format!("{digest}.json"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_record<D: Device + ?Sized>(device: &D, digest: &str) -> Option<Value> {
    let text = std::fs::read_to_string(record_path(device, digest)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write `text` to `path` whole, readable only by this user.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staged = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&staged, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&staged, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })
}

fn save_record<D: Device + ?Sized>(device: &D, digest: &str, record: &Value) {
    let _ = write_private(&record_path(device, digest), &record.to_string());
}

/// A refused check-in: the lease goes, and with it offline access. Only the
/// lease -- the app's own data is the person's and stays (R10). What is kept
/// is that these bytes are private, so the next open checks in again rather
/// than opening as an ordinary app.
fn forget<D: Device + ?Sized>(device: &D, digest: &str) {
    if let Some(mut record) = load_record(device, digest) {
        record["answer"] = Value::Null;
        record["class"] = serde_json::json!("online");
        record["refused_at"] = serde_json::json!(now());
        save_record(device, digest, &record);
    }
}

/// The hub's answer to a marked bundle's check-in.
enum CheckIn {
    /// This person may open it: the hub's small JSON about the release.
    Allowed(Value),
    /// Refused, with the hub's stable reason.
    Refused(krate_bundle::BundleError),
    /// The hub holds no such release.
    NotReleased,
    /// The hub could not be reached, or gave no answer that means anything.
    Offline,
}

fn agent(url: &str) -> ureq::Agent {
    krate_http::builder(url)
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(15))
        .build()
}

/// Check in: `GET /a/<digest>` asking for the small JSON answer (Addendum
/// 2, item 6), which names the organisation or the reason. No bytes.
fn check_in(hub: &str, digest: &str, authorization: Option<&str>) -> CheckIn {
    let url = format!("{hub}/a/{digest}");
    let mut request = agent(&url).get(&url).set("Accept", "application/json");
    if let Some(value) = authorization {
        request = request.set("Authorization", value);
    }
    match request.call() {
        Ok(response) => {
            if !response.content_type().contains("json") {
                // Served as ordinary bytes: not a private release here.
                return CheckIn::NotReleased;
            }
            match response.into_string() {
                Ok(text) => CheckIn::Allowed(serde_json::from_str(&text).unwrap_or_default()),
                Err(_) => CheckIn::Offline,
            }
        }
        Err(ureq::Error::Status(404, _)) => CheckIn::NotReleased,
        // 410 is a release its organisation withdrew (IC-949 R): refused,
        // never "offline", or a stored lease would open it.
        Err(ureq::Error::Status(status @ (401 | 403 | 410), response)) => {
            let body = response.into_string().unwrap_or_default();
            let answer: Value = serde_json::from_str(&body).unwrap_or_default();
            CheckIn::Refused(krate_bundle::BundleError::HubRefused {
                url: url.clone(),
                status,
                reason: answer["error"]
                    .as_str()
                    .unwrap_or("not-a-member")
                    .to_string(),
                message: answer["message"].as_str().unwrap_or("").to_string(),
                org: answer["org"].as_str().map(str::to_string),
            })
        }
        // Busy, rate limited, broken: no answer, and never permission.
        Err(ureq::Error::Status(_, _)) | Err(ureq::Error::Transport(_)) => CheckIn::Offline,
    }
}

/// What a bundle's mark says, and whether its signature binds it.
pub enum Mark {
    /// No organisation mark: an ordinary app, never checked with a hub.
    None,
    /// `slug`'s mark, under a signature that holds.
    Signed(String),
    /// A mark the signature does not hold, or one that cannot be read.
    Broken(String),
}

/// The mark a bundle carries, and whether its signature covers it. The
/// signed statement covers every entry, so a signature that holds is what
/// makes the mark the organisation's.
pub fn mark_of(bundle: &krate_bundle::OpenBundle) -> Mark {
    match bundle.org_mark() {
        Ok(None) => Mark::None,
        Err(_) => Mark::Broken("an organisation".to_string()),
        Ok(Some(slug)) => {
            let holds = bundle
                .full_verdict(&krate_bundle::delegation::RevocationState::Known(Vec::new()))
                .ok()
                .flatten()
                .is_some_and(|verdict| verdict.is_trustworthy());
            if holds {
                Mark::Signed(slug)
            } else {
                Mark::Broken(slug)
            }
        }
    }
}

/// `GET /orgs/<slug>/leases/<digest>`: the hub's lease answer, `None` when
/// the hub has none to give (offline, no such route yet).
fn fetch_lease(
    hub: &str,
    slug: &str,
    digest: &str,
    authorization: &str,
) -> Result<Option<Value>, String> {
    let url = format!("{hub}/orgs/{slug}/leases/{digest}");
    match agent(&url)
        .get(&url)
        .set("Authorization", authorization)
        .set("Accept", "application/json")
        .call()
    {
        Ok(response) => Ok(response
            .into_string()
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())),
        Err(ureq::Error::Status(_, response)) => {
            let answer: Value = serde_json::from_str(&response.into_string().unwrap_or_default())
                .unwrap_or_default();
            match answer["error"].as_str() {
                Some(
                    reason @ ("removed" | "not-a-member" | "release-withdrawn" | "not-in-audience"),
                ) => Err(reason.to_string()),
                _ => Ok(None),
            }
        }
        Err(ureq::Error::Transport(_)) => Ok(None),
    }
}

/// The date a lease runs out, as a person reads it.
fn date(secs: u64) -> String {
    when(&serde_json::json!(secs))
}

/// Who is opening a private app that the gate let open, and through which
/// hub: what a company connection (IC-952) asks that hub with.
pub struct OrgSession {
    pub hub: String,
    pub slug: String,
    /// `Bearer <token>`, only ever for a Krate hub.
    pub authorization: Option<String>,
}

/// The session for `digest`, from what the gate kept when it last checked
/// these bytes in. `None` when the gate never saw them as a private app.
pub fn org_session<D: Device + ?Sized>(device: &D, digest: &str) -> Option<OrgSession> {
    let record = load_record(device, digest)?;
    let hub = record["hub"].as_str()?.to_string();
    let slug = record["org"]["slug"].as_str()?.to_string();
    let authorization = authorization_for(device, &hub);
    Some(OrgSession {
        hub,
        slug,
        authorization,
    })
}

/// A company connection's address and allow, as the hub last gave them for
/// these bytes: no credential, ever. Kept so that offline, or after the hub
/// refused, a request to the address is refused in words rather than sent.
pub fn known_connection<D: Device + ?Sized>(device: &D, digest: &str, name: &str) -> Option<Value> {
    load_record(device, digest)?
        .get("connections")?
        .get(name)
        .filter(|c| c["base"].is_string())
        .cloned()
}

/// Keep a connection's address and allow (never its credential) beside
/// these bytes' lease record.
pub fn remember_connection<D: Device + ?Sized>(
    device: &D,
    digest: &str,
    name: &str,
    base: &str,
    allow: &Value,
) {
    if let Some(mut record) = load_record(device, digest) {
        if !record["connections"].is_object() {
            record["connections"] = serde_json::json!({});
        }
        record["connections"][name] = serde_json::json!({ "base": base, "allow": allow });
        save_record(device, digest, &record);
    }
}

/// Offline access for `digest`, made ready ahead of the first open (IC-953
/// background readiness): the lease is fetched now, while there is a
/// connection, so a company app listed on this device opens offline within
/// its lease. A lease already held for this sign-in and not past half its
/// life is kept, so listing every few minutes issues nothing new. Answers
/// when the lease runs out, `None` when there is none to hold (an
/// online-only app, or no answer from the hub); `Err` carries the hub's
/// refusal reason, and the lease for these bytes is gone.
///
/// The offline terms are not shown here: nobody is opening anything. They
/// are shown at the first open, online or not.
pub fn prefetch_lease<D: Device + ?Sized>(
    device: &D,
    hub: &str,
    org: (&str, &str),
    digest: &str,
    authorization: &str,
    me: &str,
) -> Result<Option<u64>, String> {
    let (slug, org_name) = org;
    let key = krate_lease::lease_key();
    let mut record = load_record(device, digest).unwrap_or_else(|| serde_json::json!({}));
    let held = record
        .get("answer")
        .filter(|a| !a.is_null())
        .zip(key.as_ref())
        .and_then(|(answer, key)| krate_lease::verify(answer, key).ok())
        .filter(|lease| lease.principal == me && lease.digest == digest && lease.org == slug);
    if let Some(lease) = held.as_ref().filter(|l| !l.past_half_life(now())) {
        return Ok(Some(lease.expires_at));
    }
    let answer = match fetch_lease(hub, slug, digest, authorization) {
        Err(reason) => {
            if load_record(device, digest).is_some() {
                forget(device, digest);
            }
            return Err(reason);
        }
        Ok(None) => return Ok(held.map(|l| l.expires_at)),
        Ok(Some(answer)) => answer,
    };
    let lease = key
        .as_ref()
        .and_then(|key| krate_lease::verify(&answer, key).ok())
        .filter(|lease| lease.principal == me);
    let Some(lease) = lease else {
        // Online-only, or a lease this Krate cannot check: nothing to keep.
        return Ok(None);
    };
    record["digest"] = serde_json::json!(digest);
    record["hub"] = serde_json::json!(hub);
    record["org"] = serde_json::json!({ "slug": slug, "name": org_name });
    record["principal"] = serde_json::json!(me);
    record["class"] = serde_json::json!(lease.class);
    record["answer"] = answer;
    record["checked_at"] = serde_json::json!(now());
    save_record(device, digest, &record);
    Ok(Some(lease.expires_at))
}

/// The offline terms, shown before the first private open (Addendum 3 G).
pub fn notice(org_name: &str, class: &str, expires_at: Option<u64>) -> String {
    match expires_at {
        Some(at) if class != "online" => format!(
            "This is {org_name}'s private app. Works offline until {}, then needs a \
             connection. If you are removed, it stops when it next connects.",
            date(at)
        ),
        _ => format!(
            "This is {org_name}'s private app. It needs a connection every time it opens. If \
             you are removed, it stops at once."
        ),
    }
}

/// What an open that passed knows about an organisation's private app: the
/// organisation, the hub it checked in with, the bytes, and who is opening
/// it. The run gives these to the app's team data (`store.org`, IC-951).
#[derive(Debug, Clone)]
pub struct Opened {
    pub slug: String,
    pub hub: String,
    pub digest: String,
    /// `Bearer <token>`, only when the hub is a Krate hub.
    pub authorization: Option<String>,
    /// The signed-in person as a principal (`github:<login>`).
    pub principal: Option<String>,
    /// The organisation's name, as its hub last gave it; the slug before
    /// the hub has said.
    pub org_name: String,
}

/// Where this person's copy of one app's data in one organisation lives:
/// `<home>/org-data/<slug>/<digest of app id and principal>.json`. The file
/// name says neither who nor which app.
pub fn org_data_path<D: Device + ?Sized>(
    device: &D,
    slug: &str,
    app_id: &str,
    principal: Option<&str>,
) -> PathBuf {
    let key = format!("{app_id}\n{}", principal.unwrap_or("signed-out"));
    let name = krate_bundle::sha256_hex(key.as_bytes());
    org_data_dir(device, slug).join(format!("{}.json", &name[..32]))
}

/// Delete this app's team data in `slug` on this device, for the person
/// signed in and for nobody signed in: what a machine policy's
/// `wipe_when_offline_ends` asks once offline access runs out (G17).
/// Answers how many files went.
pub fn wipe_team_data<D: Device + ?Sized>(device: &D, slug: &str, app_id: &str) -> usize {
    let app_id = app_id.to_ascii_lowercase();
    let me = me(device);
    [me.as_deref(), None]
        .into_iter()
        .map(|principal| org_data_path(device, slug, &app_id, principal))
        .filter(|path| std::fs::remove_file(path).is_ok())
        .count()
}

fn org_data_dir<D: Device + ?Sized>(device: &D, slug: &str) -> PathBuf {
    device.home().join("org-data").join(slug)
}

/// Where this device's organisation key is kept.
pub fn org_device_key_path<D: Device + ?Sized>(device: &D) -> PathBuf {
    device.home().join("org-keys").join("device.json")
}

/// This device's key for organisations' keyrings (F-316): an X25519 key
/// pair made once, its private half kept in `<home>/org-keys/device.json`
/// sealed with this device's machine key (the one store.secret uses), so a
/// copied file does not carry it to another device. Only the public half
/// is ever sent.
pub fn org_device_key<D: Device + ?Sized>(device: &D) -> Result<krate_orgkeys::KeyPair, String> {
    const CONTEXT: &str = "krate org device key v1";
    let kind = device.kind();
    let path = org_device_key_path(device);
    let machine = device.machine_key();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let stored: Value = serde_json::from_str(&text).unwrap_or_default();
        if let Some(sealed) = stored["sealed"].as_str() {
            match krate_orgkeys::open_local(&machine, CONTEXT, sealed) {
                Ok(secret) => {
                    return krate_orgkeys::KeyPair::from_secret(&secret)
                        .map_err(|err| format!("this {kind}'s organisation key: {err}"))
                }
                Err(_) => {
                    return Err(format!(
                        "this {kind}'s organisation key at {} does not open with this {kind}'s machine key (was it copied from another {kind}?): move it aside and open the app again to make a new one, which another member's device then passes the key to",
                        path.display()
                    ))
                }
            }
        }
    }
    let pair = krate_orgkeys::KeyPair::generate();
    let sealed = krate_orgkeys::seal_local(&machine, CONTEXT, pair.secret())
        .map_err(|err| format!("could not keep this {kind}'s organisation key: {err}"))?;
    write_private(
        &path,
        &serde_json::json!({ "v": 1, "public_key": pair.public_hex(), "sealed": sealed })
            .to_string(),
    )
    .map_err(|err| format!("could not keep this {kind}'s organisation key: {err}"))?;
    Ok(pair)
}

/// May these bytes open? `Ok` at once, with no request anywhere, for any
/// bundle without an organisation's mark; for a marked one, `Ok` only when
/// this person may open it now, with what is known about the private app
/// it let through (`None` for an unmarked app).
pub fn gate_opened<D: Device + ?Sized>(
    device: &D,
    bytes: Bytes,
    mark: Mark,
) -> Result<Option<Opened>, GateError> {
    let slug = match mark {
        Mark::None => return Ok(None),
        Mark::Broken(slug) => {
            return refused(
                "broken-mark",
                &format!(
                    "this app is marked as {slug}'s private app, but its signature does not \
                     hold, so it does not open: it was changed after it was signed, or the \
                     copy is damaged. Ask {slug} for a fresh copy"
                ),
            )
        }
        Mark::Signed(slug) => slug,
    };
    let slug_of_mark = slug.clone();
    let (digest, link) = match bytes {
        Bytes::Fetched { sha256, link } => (sha256.to_string(), Some(link)),
        Bytes::File(path) => match std::fs::read(path) {
            Ok(bytes) => (krate_bundle::sha256_hex(&bytes), None),
            Err(err) => {
                return Err(GateError::Failed(format!(
                    "could not read {}: {err}",
                    path.display()
                )))
            }
        },
    };
    let record = load_record(device, &digest);
    // The hub to ask: the one that served the link, the one this app came
    // from before, or the one this device uses.
    let hub = link
        .filter(|l| device.is_hub_link(l))
        .and_then(origin)
        .or_else(|| {
            record
                .as_ref()
                .and_then(|r| r["hub"].as_str().map(str::to_string))
        })
        .map(Ok)
        .unwrap_or_else(|| device.hub())?;
    // A pinned hub (IC-949 [hub]) is the only one asked, whatever served
    // the link or answered before.
    let hub = device.pinned_hub().unwrap_or(hub);
    // The session goes only to a Krate hub, as on every other path.
    let authorization = authorization_for(device, &hub);
    let me = me(device);
    // What is known before the hub answers: the mark's organisation.
    let known = record.clone().unwrap_or_else(|| {
        serde_json::json!({
            "digest": digest, "hub": hub, "org": { "slug": slug, "name": slug },
            "class": "online", "answer": null,
        })
    });

    let mut opened = Opened {
        slug: slug.clone(),
        hub: hub.clone(),
        digest: digest.clone(),
        authorization: authorization.clone(),
        principal: me.clone(),
        org_name: slug.clone(),
    };
    let result = match check_in(&hub, &digest, authorization.as_deref()) {
        CheckIn::NotReleased => {
            save_record(device, &digest, &known);
            forget(device, &digest);
            refused(
                "not-released",
                &format!(
                    "this is {slug}'s private app, and {hub} has no release of it, so it does \
                     not open: ask {slug} for the link its reviewer's approval gave"
                ),
            )
        }
        CheckIn::Offline => offline(device, &digest, &known, me.as_deref()),
        CheckIn::Refused(krate_bundle::BundleError::HubRefused {
            reason,
            message,
            org,
            ..
        }) => {
            if matches!(
                reason.as_str(),
                "removed"
                    | "not-a-member"
                    | "digest-mismatch"
                    | "no-such-org"
                    | "release-withdrawn"
                    | "not-in-audience"
            ) {
                save_record(device, &digest, &known);
                forget(device, &digest);
            }
            // Someone no longer in the organisation keeps none of its apps'
            // data (store.org, IC-951): it is the organisation's, not theirs.
            if matches!(reason.as_str(), "removed" | "not-a-member") {
                let _ = std::fs::remove_dir_all(org_data_dir(device, &slug_of_mark));
            }
            let org = org.or(Some(slug));
            let words = device.refusal_words(
                &format!("{hub}/a/{digest}"),
                &reason,
                &message,
                org.as_deref(),
            );
            refused(&reason, &words)
        }
        CheckIn::Refused(other) => refused("hub-refused", &other.to_string()),
        CheckIn::Allowed(meta) => online(
            device,
            &digest,
            &hub,
            &meta,
            record,
            authorization.as_deref(),
            me.as_deref(),
        ),
    };
    result.map(|()| {
        if let Some(name) = load_record(device, &digest)
            .as_ref()
            .and_then(|r| r["org"]["name"].as_str())
            .filter(|n| !n.is_empty())
        {
            opened.org_name = name.to_string();
        }
        Some(opened)
    })
}

/// Checked in: the person may open it. Keep the lease fresh and show the
/// terms the first time.
fn online<D: Device + ?Sized>(
    device: &D,
    digest: &str,
    hub: &str,
    meta: &Value,
    record: Option<Value>,
    authorization: Option<&str>,
    me: Option<&str>,
) -> Result<(), GateError> {
    let slug = meta["org"]["slug"].as_str().unwrap_or_default().to_string();
    let org_name = meta["org"]["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or(&slug)
        .to_string();
    let mut record = record.unwrap_or_else(|| serde_json::json!({}));
    let key = krate_lease::lease_key();
    let held = record.get("answer").filter(|a| !a.is_null()).cloned();
    let fresh = held
        .as_ref()
        .zip(key.as_ref())
        .and_then(|(answer, key)| krate_lease::verify(answer, key).ok())
        .filter(|lease| Some(lease.principal.as_str()) == me && !lease.past_half_life(now()));
    let mut class = record["class"].as_str().unwrap_or("online").to_string();
    let mut expires = fresh.as_ref().map(|lease| lease.expires_at);
    if fresh.is_none() {
        if let (Some(authorization), false) = (authorization, slug.is_empty()) {
            match fetch_lease(hub, &slug, digest, authorization) {
                Err(reason) => {
                    forget(device, digest);
                    let words = device.refusal_words(
                        &format!("{hub}/a/{digest}"),
                        &reason,
                        "",
                        Some(&slug),
                    );
                    return refused(&reason, &words);
                }
                Ok(Some(answer)) => {
                    if answer["class"].as_str() == Some("online") && answer["lease"].is_null() {
                        class = "online".into();
                        record["answer"] = Value::Null;
                        expires = None;
                    } else {
                        class = answer["lease"]["class"]
                            .as_str()
                            .unwrap_or("online")
                            .to_string();
                        // Kept as it came. Whether it verifies is asked when
                        // it is needed -- offline -- against the pinned key.
                        expires = key
                            .as_ref()
                            .and_then(|key| krate_lease::verify(&answer, key).ok())
                            .map(|lease| lease.expires_at);
                        if expires.is_none() {
                            class = "online".into();
                        }
                        record["answer"] = answer;
                    }
                }
                // No lease to be had right now: this open stays online-only.
                Ok(None) => {}
            }
        }
    }
    let noticed = record["noticed_class"].as_str().map(str::to_string);
    let shown_again = noticed
        .as_deref()
        .is_none_or(|was| krate_lease::class_rank(&class) < krate_lease::class_rank(was));
    if shown_again {
        device.tell("Offline access", &notice(&org_name, &class, expires));
        record["noticed_class"] = serde_json::json!(class);
    }
    record["digest"] = serde_json::json!(digest);
    record["hub"] = serde_json::json!(hub);
    record["org"] = serde_json::json!({ "slug": slug, "name": org_name });
    record["principal"] = serde_json::json!(me);
    record["class"] = serde_json::json!(class);
    record["checked_at"] = serde_json::json!(now());
    save_record(device, digest, &record);
    Ok(())
}

/// No connection: open only on a lease that holds.
fn offline<D: Device + ?Sized>(
    device: &D,
    digest: &str,
    record: &Value,
    me: Option<&str>,
) -> Result<(), GateError> {
    let org_name = record["org"]["name"]
        .as_str()
        .unwrap_or("its organisation")
        .to_string();
    let Some(me) = me else {
        return refused(
            "offline-not-signed-in",
            &format!(
                "this is {org_name}'s private app and you are not signed in, so it does not \
                 open offline: {} while connected",
                device.sign_in_words()
            ),
        );
    };
    let Some(answer) = record.get("answer").filter(|a| !a.is_null()) else {
        return refused(
            "offline-online-only",
            &format!(
                "this is {org_name}'s private app, and it opens only with a connection: \
                 connect to the internet and open it again"
            ),
        );
    };
    // The organisation the hub named when it checked these bytes in.
    let slug = record["org"]["slug"].as_str().unwrap_or_default();
    match krate_lease::check(
        answer,
        krate_lease::lease_key().as_ref(),
        slug,
        digest,
        me,
        now(),
    ) {
        Ok(lease) => {
            // A lease made ready ahead of time (IC-953) reaches its first
            // open here, offline: the terms are shown before it opens, as
            // they would be online.
            let noticed = record["noticed_class"].as_str();
            if noticed.is_none_or(|was| {
                krate_lease::class_rank(&lease.class) < krate_lease::class_rank(was)
            }) {
                device.tell(
                    "Offline access",
                    &notice(&org_name, &lease.class, Some(lease.expires_at)),
                );
                let mut kept = record.clone();
                kept["noticed_class"] = serde_json::json!(lease.class);
                save_record(device, digest, &kept);
            }
            device.note(&format!(
                "offline: opening {org_name}'s app on its offline access, which runs out {}",
                date(lease.expires_at)
            ));
            Ok(())
        }
        Err(krate_lease::Refusal::Expired { expires_at }) => refused(
            "offline-expired",
            &format!(
                "this is {org_name}'s private app, and its offline access ran out on {}: \
                 connect to the internet and open it again",
                date(expires_at)
            ),
        ),
        Err(krate_lease::Refusal::OtherPerson) => refused(
            "offline-other-person",
            &format!(
                "this is {org_name}'s private app, and its offline access belongs to another \
                 sign-in: connect to the internet and open it again as yourself"
            ),
        ),
        Err(why) => refused(
            "offline-unchecked",
            &format!(
                "this is {org_name}'s private app, and its offline access could not be checked \
                 ({}), so it needs a connection: connect to the internet and open it again",
                match why {
                    krate_lease::Refusal::NoKey =>
                        "this Krate has no hub key to check it with".to_string(),
                    krate_lease::Refusal::BadSignature => "its signature does not hold".into(),
                    krate_lease::Refusal::OtherApp => "it is for other bytes".into(),
                    krate_lease::Refusal::Malformed(what) => what,
                    _ => "it does not hold".into(),
                }
            ),
        ),
    }
}

/// A link's download was refused: when the refusal ends access, the lease
/// for the bytes it names goes too, exactly as a refused check-in does.
pub fn refused_link<D: Device + ?Sized>(device: &D, url: &str, reason: &str) {
    if !matches!(
        reason,
        "removed"
            | "not-a-member"
            | "digest-mismatch"
            | "no-such-org"
            | "release-withdrawn"
            | "not-in-audience"
    ) {
        return;
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if let Some(segment) = path.split("/a/").nth(1).and_then(|s| s.split('/').next()) {
        if segment.len() == 64 && segment.bytes().all(|b| b.is_ascii_hexdigit()) {
            forget(device, &segment.to_ascii_lowercase());
        }
    }
}

/// The organisation's team data (store.org, IC-951) for a run the gate let
/// through: only the private app it opened, as that release, for the person
/// signed in. `app_id` is the id the signed manifest names; `clock_skew_ms`
/// moves this device's clock for a test of a wrong one, zero in use.
pub fn org_access<D: Device + ?Sized>(
    device: &D,
    opened: &Opened,
    app_id: &str,
    clock_skew_ms: i64,
) -> Result<krate_runtime::org_store::OrgAccess, String> {
    let app_id = app_id.to_ascii_lowercase();
    Ok(krate_runtime::org_store::OrgAccess::Granted(Box::new(
        krate_runtime::org_store::OrgGrant {
            slug: opened.slug.clone(),
            hub: opened.hub.clone(),
            digest: opened.digest.clone(),
            authorization: opened.authorization.clone(),
            me: opened
                .principal
                .as_deref()
                .map(|p| p.split_once(':').map_or(p, |(_, v)| v).to_string()),
            mirror: org_data_path(device, &opened.slug, &app_id, opened.principal.as_deref()),
            clock_skew_ms,
            app_id,
            device_secret: krate_runtime::org_store::DeviceSecret(
                *org_device_key(device)?.secret(),
            ),
            device_name: device.name(opened.principal.as_deref()),
            org_name: opened.org_name.clone(),
        },
    )))
}

/// Where this device stands with an organisation's key, for words before a
/// run: a new device registers and waits until another member's device is
/// online and passes the key on, which happens by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStanding {
    /// This device holds the organisation's key.
    Holds,
    /// Registered and waiting for a teammate's device to let it in.
    Waiting,
    /// The organisation has not turned on encryption yet.
    NoKey,
    /// The hub could not be reached: nothing is known.
    Offline,
    /// The hub refused, or a key did not open: the words.
    Problem(String),
}

/// Ask the hub where this device stands with `opened`'s organisation, and
/// do this device's share (registering it; passing the key on when it holds
/// it), exactly as the run's own sync does.
pub fn key_standing<D: Device + ?Sized>(device: &D, opened: &Opened) -> KeyStanding {
    use krate_orgkeys::agent::{Agent, AgentError, Status};
    let pair = match org_device_key(device) {
        Ok(pair) => pair,
        Err(words) => return KeyStanding::Problem(words),
    };
    let agent = Agent::new(
        &opened.hub,
        &opened.slug,
        opened.authorization.clone(),
        pair,
        &device.name(opened.principal.as_deref()),
    );
    match agent.refresh(None) {
        Ok(Status::Holds(_)) => KeyStanding::Holds,
        Ok(Status::Waiting) => KeyStanding::Waiting,
        Ok(Status::NoKey) => KeyStanding::NoKey,
        Err(AgentError::Offline) => KeyStanding::Offline,
        Err(err) => KeyStanding::Problem(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_names_the_date_or_says_it_needs_a_connection() {
        let days = notice("Acme", "days-7", Some(1_760_400_000));
        assert!(days.contains("Works offline until 2025-10-14 00:00 UTC, then needs a connection."));
        assert!(days.contains("If you are removed, it stops when it next connects."));
        let online = notice("Acme", "online", None);
        assert!(online.contains("needs a connection every time it opens"));
    }

    #[test]
    fn principals_and_origins_are_read_one_way() {
        assert_eq!(principal_of("Alice"), "github:alice");
        assert_eq!(principal_of("dan@Acme.example"), "email:dan@acme.example");
        assert_eq!(principal_of("email:x@y"), "email:x@y");
        assert_eq!(
            origin("HTTPS://Hub.Krate.Tech/a/x?y").as_deref(),
            Some("https://hub.krate.tech")
        );
        assert_eq!(origin("https://me@hub.krate.tech/a"), None);
        assert_eq!(origin("hub.krate.tech"), None);
    }
}
