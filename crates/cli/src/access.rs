//! Opening an organisation's private app, online or off (IC-948 G and H,
//! Addendum 5).
//!
//! Every door that runs an app -- `krate run`, `krate launch`, a double-click
//! through the Player, Studio's Open, the MCP tools -- reaches [`gate`] once
//! the bytes are in hand. Only a bundle that carries an organisation's mark
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
//! Leases live in `~/.krate/leases/<digest>.json`.
//!
//! The honest limit, said in the docs too: this is enforced by Krate on the
//! device. Somebody who strips the mark and the signature has an unsigned
//! copy of the code, not the organisation's app, and Krate cannot stop a
//! copy.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::{github_auth, org};

/// How a door is being used: from a terminal, or with nobody at one (a
/// double-click), where the words also go in a native alert.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Terminal,
    Window,
}

/// Where the bytes being opened came from.
pub enum Bytes<'a> {
    /// A file on this machine; hashed only if it is signed.
    File(&'a Path),
    /// A download whose sha256 is already known, from this link.
    Fetched { sha256: &'a str, link: &'a str },
}

fn leases_dir() -> PathBuf {
    crate::krate_home().join("leases")
}

fn record_path(digest: &str) -> PathBuf {
    leases_dir().join(format!("{digest}.json"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What is kept about one private app on this machine.
fn load_record(digest: &str) -> Option<Value> {
    let text = std::fs::read_to_string(record_path(digest)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_record(digest: &str, record: &Value) {
    let path = record_path(digest);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let staged = path.with_extension(format!("json.{}", std::process::id()));
    if std::fs::write(&staged, record.to_string()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600));
        }
        if std::fs::rename(&staged, &path).is_err() {
            let _ = std::fs::remove_file(&staged);
        }
    }
}

/// A refused check-in: the lease goes, and with it offline access. Only the
/// lease -- the app's own data is the person's and stays (R10). What is kept
/// is that these bytes are private, so the next open checks in again rather
/// than opening as an ordinary app.
fn forget(digest: &str) {
    if let Some(mut record) = load_record(digest) {
        record["answer"] = Value::Null;
        record["class"] = serde_json::json!("online");
        record["refused_at"] = serde_json::json!(now());
        save_record(digest, &record);
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
) -> std::result::Result<Option<Value>, String> {
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
    org::when(&serde_json::json!(secs))
}

/// Show `words` where the person is looking: always on stderr, and in a
/// native alert when nobody is at a terminal to read it.
pub fn tell(door: Door, title: &str, words: &str) {
    eprintln!("{words}");
    alert(door, title, words);
}

/// The native alert alone, for a window door nobody is watching from a
/// terminal.
fn alert(door: Door, title: &str, words: &str) {
    alert_where(door, title, words, true);
}

/// `on_windows` is false for a refusal: on Windows `main` already puts any
/// refusal in a dialog when nobody can read stderr, and two would be one
/// too many.
fn alert_where(door: Door, title: &str, words: &str, on_windows: bool) {
    if door != Door::Window {
        return;
    }
    #[cfg(target_os = "macos")]
    if !stderr_is_watched() {
        krate_adapter_macos::present_message(title, words);
    }
    #[cfg(windows)]
    if on_windows {
        // The same test as `main`'s dialog: no console and nobody reading
        // stderr is a double-click (K-178, K-240).
        let unwatched =
            unsafe { windows_sys::Win32::System::Console::GetConsoleWindow().is_null() }
                && !crate::stderr_has_a_reader();
        if unwatched {
            let _ = rfd::MessageDialog::new()
                .set_title(title)
                .set_description(words)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
    }
    let _ = (title, words, on_windows);
}

/// Is somebody reading stderr -- a terminal, a pipe, a file? Then a modal
/// alert would only block a process nobody can see (K-240). A double-click
/// sends stderr to /dev/null, which is none of them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn stderr_is_watched() -> bool {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(std::io::stderr().as_raw_fd(), &mut stat) } == 0 {
            let kind = stat.st_mode & libc::S_IFMT;
            return kind == libc::S_IFIFO || kind == libc::S_IFREG || kind == libc::S_IFSOCK;
        }
        true
    }
    #[cfg(not(unix))]
    {
        true
    }
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
pub fn org_session(digest: &str) -> Option<OrgSession> {
    let record = load_record(digest)?;
    let hub = record["hub"].as_str()?.to_string();
    let slug = record["org"]["slug"].as_str()?.to_string();
    let authorization = github_auth::current()
        .filter(|_| org::is_hub_link(&format!("{hub}/")))
        .map(|identity| format!("Bearer {}", identity.token));
    Some(OrgSession {
        hub,
        slug,
        authorization,
    })
}

/// A company connection's address and allow, as the hub last gave them for
/// these bytes: no credential, ever. Kept so that offline, or after the hub
/// refused, a request to the address is refused in words rather than sent.
pub fn known_connection(digest: &str, name: &str) -> Option<Value> {
    load_record(digest)?
        .get("connections")?
        .get(name)
        .filter(|c| c["base"].is_string())
        .cloned()
}

/// Keep a connection's address and allow (never its credential) beside
/// these bytes' lease record.
pub fn remember_connection(digest: &str, name: &str, base: &str, allow: &Value) {
    if let Some(mut record) = load_record(digest) {
        if !record["connections"].is_object() {
            record["connections"] = serde_json::json!({});
        }
        record["connections"][name] = serde_json::json!({ "base": base, "allow": allow });
        save_record(digest, &record);
    }
}

/// Offline access for `digest`, made ready ahead of the first open (IC-953
/// background readiness): the lease is fetched now, while there is a
/// connection, so a company app listed on this computer opens offline
/// within its lease. A lease already held for this sign-in and not past
/// half its life is kept, so listing every few minutes issues nothing new.
/// Answers when the lease runs out, `None` when there is none to hold (an
/// online-only app, or no answer from the hub); `Err` carries the hub's
/// refusal reason, and the lease for these bytes is gone.
///
/// The offline terms are not shown here: nobody is opening anything. They
/// are shown at the first open, online or not.
pub fn prefetch_lease(
    hub: &str,
    org: (&str, &str),
    digest: &str,
    authorization: &str,
    me: &str,
) -> std::result::Result<Option<u64>, String> {
    let (slug, org_name) = org;
    let key = krate_lease::lease_key();
    let mut record = load_record(digest).unwrap_or_else(|| serde_json::json!({}));
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
            if load_record(digest).is_some() {
                forget(digest);
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
    save_record(digest, &record);
    Ok(Some(lease.expires_at))
}

/// The offline terms, shown before the first private open (Addendum 3 G).
fn notice(org_name: &str, class: &str, expires_at: Option<u64>) -> String {
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
/// `~/.krate/org-data/<slug>/<digest of app id and principal>.json`. The
/// file name says neither who nor which app.
pub fn org_data_path(slug: &str, app_id: &str, principal: Option<&str>) -> PathBuf {
    let key = format!("{app_id}\n{}", principal.unwrap_or("signed-out"));
    let name = krate_bundle::sha256_hex(key.as_bytes());
    org_data_dir(slug).join(format!("{}.json", &name[..32]))
}

fn org_data_dir(slug: &str) -> PathBuf {
    crate::krate_home().join("org-data").join(slug)
}

/// This device's key for organisations' keyrings (F-316): an X25519 key
/// pair made once, its private half kept in `~/.krate/org-keys/device.json`
/// sealed with this computer's machine key (the one store.secret uses), so
/// a copied file does not carry it to another computer. Only the public
/// half is ever sent.
pub fn org_device_key() -> Result<krate_orgkeys::KeyPair> {
    const CONTEXT: &str = "krate org device key v1";
    let path = crate::krate_home().join("org-keys").join("device.json");
    let machine = crate::machine_key();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let stored: Value = serde_json::from_str(&text).unwrap_or_default();
        if let Some(sealed) = stored["sealed"].as_str() {
            match krate_orgkeys::open_local(&machine, CONTEXT, sealed) {
                Ok(secret) => {
                    return krate_orgkeys::KeyPair::from_secret(&secret)
                        .map_err(|err| anyhow::anyhow!("this computer's organisation key: {err}"))
                }
                Err(_) => anyhow::bail!(
                    "this computer's organisation key at {} does not open with this computer's machine key (was it copied from another computer?): move it aside and open the app again to make a new one, which another member's device then passes the key to",
                    path.display()
                ),
            }
        }
    }
    let pair = krate_orgkeys::KeyPair::generate();
    let sealed = krate_orgkeys::seal_local(&machine, CONTEXT, pair.secret())
        .map_err(|err| anyhow::anyhow!("could not keep this computer's organisation key: {err}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staged = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(
        &staged,
        serde_json::json!({ "v": 1, "public_key": pair.public_hex(), "sealed": sealed })
            .to_string(),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&staged, &path)?;
    Ok(pair)
}

/// What an organisation's device list calls this computer.
pub fn device_name(principal: Option<&str>) -> String {
    let who = principal
        .map(|p| p.split_once(':').map_or(p, |(_, v)| v))
        .unwrap_or("someone");
    format!("{who}'s {} computer", std::env::consts::OS)
}

/// May these bytes open? `Ok` at once, with no request anywhere, for any
/// bundle without an organisation's mark; for a marked one, `Ok` only when
/// this person may open it now, with what is known about the private app
/// it let through (`None` for an unmarked app).
pub fn gate_opened(bytes: Bytes, mark: Mark, door: Door) -> Result<Option<Opened>> {
    let slug = match mark {
        Mark::None => return Ok(None),
        Mark::Broken(slug) => {
            return refuse_opened(
                door,
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
            Err(err) => anyhow::bail!("could not read {}: {err}", path.display()),
        },
    };
    let record = load_record(&digest);
    let identity = github_auth::current();
    // The hub to ask: the one that served the link, the one this app came
    // from before, or the one this machine uses.
    let hub = link
        .filter(|l| org::is_hub_link(l))
        .and_then(org::origin)
        .or_else(|| {
            record
                .as_ref()
                .and_then(|r| r["hub"].as_str().map(str::to_string))
        })
        .map(Ok)
        .unwrap_or_else(|| org::hub_url(None))?;
    // A pinned hub (IC-949 [hub]) is the only one asked, whatever served
    // the link or answered before.
    let hub = crate::fleet::pinned_hub()
        .map(str::to_string)
        .unwrap_or(hub);
    // The session goes only to a Krate hub, as on every other path.
    let authorization = identity
        .as_ref()
        .filter(|_| org::is_hub_link(&format!("{hub}/")))
        .map(|identity| format!("Bearer {}", identity.token));
    let me = identity
        .as_ref()
        .map(|identity| org::principal_of(&identity.login));
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
            save_record(&digest, &known);
            forget(&digest);
            refuse(
                door,
                "not-released",
                &format!(
                    "this is {slug}'s private app, and {hub} has no release of it, so it does \
                     not open: ask {slug} for the link its reviewer's approval gave"
                ),
            )
        }
        CheckIn::Offline => offline(&digest, &known, me.as_deref(), door),
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
                save_record(&digest, &known);
                forget(&digest);
            }
            // Someone no longer in the organisation keeps none of its apps'
            // data (store.org, IC-951): it is the organisation's, not theirs.
            if matches!(reason.as_str(), "removed" | "not-a-member") {
                let _ = std::fs::remove_dir_all(org_data_dir(&slug_of_mark));
            }
            let org = org.or(Some(slug));
            let words = org::download_refusal(
                &format!("{hub}/a/{digest}"),
                &reason,
                &message,
                org.as_deref(),
            );
            refuse(door, &reason, &words)
        }
        CheckIn::Refused(other) => refuse(door, "hub-refused", &other.to_string()),
        CheckIn::Allowed(meta) => online(
            &digest,
            &hub,
            &meta,
            record,
            authorization.as_deref(),
            me.as_deref(),
            door,
        ),
    };
    result.map(|()| {
        if let Some(name) = load_record(&digest)
            .as_ref()
            .and_then(|r| r["org"]["name"].as_str())
            .filter(|n| !n.is_empty())
        {
            opened.org_name = name.to_string();
        }
        Some(opened)
    })
}

fn refuse_opened(door: Door, reason: &str, words: &str) -> Result<Option<Opened>> {
    refuse(door, reason, words).map(|()| None)
}

/// Refuse at the door with a stable reason for the open record. The error
/// reads as the words alone.
fn refuse(door: Door, reason: &str, words: &str) -> Result<()> {
    alert_refusal(door, words);
    Err(anyhow::Error::new(crate::audit::Refused::new(
        reason,
        words.to_string(),
    )))
}

/// The refusal alert alone, for the machine policy's gate (IC-949), which
/// refuses through the same door machinery as this one.
pub fn alert_refusal(door: Door, words: &str) {
    alert_where(door, "This app did not open", words, false);
}

/// Checked in: the person may open it. Keep the lease fresh and show the
/// terms the first time.
fn online(
    digest: &str,
    hub: &str,
    meta: &Value,
    record: Option<Value>,
    authorization: Option<&str>,
    me: Option<&str>,
    door: Door,
) -> Result<()> {
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
                    forget(digest);
                    let words = org::download_refusal(
                        &format!("{hub}/a/{digest}"),
                        &reason,
                        "",
                        Some(&slug),
                    );
                    return refuse(door, &reason, &words);
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
        tell(door, "Offline access", &notice(&org_name, &class, expires));
        record["noticed_class"] = serde_json::json!(class);
    }
    record["digest"] = serde_json::json!(digest);
    record["hub"] = serde_json::json!(hub);
    record["org"] = serde_json::json!({ "slug": slug, "name": org_name });
    record["principal"] = serde_json::json!(me);
    record["class"] = serde_json::json!(class);
    record["checked_at"] = serde_json::json!(now());
    save_record(digest, &record);
    Ok(())
}

/// No connection: open only on a lease that holds.
fn offline(digest: &str, record: &Value, me: Option<&str>, door: Door) -> Result<()> {
    let org_name = record["org"]["name"]
        .as_str()
        .unwrap_or("its organisation")
        .to_string();
    let Some(me) = me else {
        return refuse(
            door,
            "offline-not-signed-in",
            &format!(
                "this is {org_name}'s private app and you are not signed in, so it does not \
                 open offline: sign in with `krate account login` while connected"
            ),
        );
    };
    let Some(answer) = record.get("answer").filter(|a| !a.is_null()) else {
        return refuse(
            door,
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
                tell(
                    door,
                    "Offline access",
                    &notice(&org_name, &lease.class, Some(lease.expires_at)),
                );
                let mut kept = record.clone();
                kept["noticed_class"] = serde_json::json!(lease.class);
                save_record(digest, &kept);
            }
            eprintln!(
                "offline: opening {org_name}'s app on its offline access, which runs out {}",
                date(lease.expires_at)
            );
            Ok(())
        }
        Err(krate_lease::Refusal::Expired { expires_at }) => refuse(
            door,
            "offline-expired",
            &format!(
                "this is {org_name}'s private app, and its offline access ran out on {}: \
                 connect to the internet and open it again",
                date(expires_at)
            ),
        ),
        Err(krate_lease::Refusal::OtherPerson) => refuse(
            door,
            "offline-other-person",
            &format!(
                "this is {org_name}'s private app, and its offline access belongs to another \
                 sign-in: connect to the internet and open it again as yourself"
            ),
        ),
        Err(why) => refuse(
            door,
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

/// The words for a refused download: one place for every door that
/// fetches (launch, MCP), as `krate run` already does. A refusal that ends
/// access also ends the lease for the bytes the link names.
pub fn refused_words(err: &krate_bundle::BundleError, url: &str) -> Option<String> {
    match err {
        krate_bundle::BundleError::HubRefused {
            reason,
            message,
            org,
            ..
        } => {
            refused_link(url, reason);
            Some(org::download_refusal(url, reason, message, org.as_deref()))
        }
        _ => None,
    }
}

/// A link's download was refused: when the refusal ends access, the lease
/// for the bytes it names goes too, exactly as a refused check-in does.
pub fn refused_link(url: &str, reason: &str) {
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
            forget(&segment.to_ascii_lowercase());
        }
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
}
