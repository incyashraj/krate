//! Signing in to Krate on the phone: krate.tech's own sign-in page (GitHub,
//! Google or email) in the phone's in-app sign-in tab, handed back as
//! krate://signed-in, then the account's profile (`/me`), organisations
//! (`/orgs`) and published apps (`/my/apps`) for the home's profile tab.
//!
//! The sign-in is kept in Krate's own private storage (no other app can read
//! it) and is only ever sent to the hub, as a bearer token.

#![cfg(any(target_os = "android", target_os = "ios"))]

use crate::home::Account;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HUB: &str = "https://hub.krate.tech";

// The iPhone player includes this same file (crates/player-ios).
#[cfg(target_os = "android")]
const USER_AGENT: &str = concat!("Krate/", env!("CARGO_PKG_VERSION"), " (Android)");
#[cfg(target_os = "ios")]
const USER_AGENT: &str = concat!("Krate/", env!("CARGO_PKG_VERSION"), " (iPhone)");

fn agent() -> ureq::Agent {
    // The hub's edge refuses requests without a real user agent.
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
}

fn body(response: ureq::Response) -> Result<serde_json::Value, String> {
    let mut text = String::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn text_of(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string()
}

fn kept(data_root: &Path) -> PathBuf {
    data_root.join("account.json")
}

/// The token of the person signed in on this phone, if anyone is.
pub fn token(data_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(kept(data_root)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(text_of(&v, "token")).filter(|t| !t.is_empty())
}

/// Forget the sign-in on this phone.
pub fn sign_out(data_root: &Path) {
    let _ = std::fs::remove_file(kept(data_root));
}

/// krate.tech's sign-in page for Krate on this phone: GitHub, Google or
/// email, as the person likes, handed back as krate://signed-in with this
/// `nonce` -- the proof Krate asked for it (K-909).
pub fn web_sign_in_url(nonce: &str) -> String {
    format!("https://krate.tech/login/?app_nonce={nonce}")
}

/// A fresh nonce: 32 hex digits from the system's randomness.
pub fn new_nonce() -> Option<String> {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Keep a krate://signed-in hand-back, once its nonce is the one this phone
/// sent: anyone can open such an address, and only the sign-in Krate started
/// counts.
pub fn keep_web_sign_in(back: &str, nonce: &str, data_root: &Path) -> Result<(), String> {
    let fields = crate::home::signed_in_fields(back).ok_or("That was not a Krate sign-in.")?;
    if nonce.is_empty() || fields.app_nonce != nonce {
        return Err("That sign-in was not the one Krate started. Try again.".to_string());
    }
    if fields.token.is_empty() {
        return Err("The sign-in came back without an account. Try again.".to_string());
    }
    let kept_json = serde_json::json!({
        "login": fields.login,
        "name": fields.name,
        "avatar_url": fields.avatar_url,
        "token": fields.token,
    });
    let path = kept(data_root);
    let part = path.with_extension("part");
    std::fs::write(&part, kept_json.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())
}

fn get(path: &str, token: &str) -> Result<serde_json::Value, String> {
    let response = agent()
        .get(&format!("{HUB}{path}"))
        .set("authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| e.to_string())?;
    body(response)
}

/// The signed-in person's profile, organisations and published apps, as last
/// known: fetched from the hub when it answers, else what was kept. None when
/// nobody is signed in. `art_dir` receives their avatar.
pub fn account(data_root: &Path, art_dir: &Path) -> Option<Account> {
    let token = token(data_root)?;
    let text = std::fs::read_to_string(kept(data_root)).ok()?;
    let saved: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut account = Account {
        login: text_of(&saved, "login"),
        name: text_of(&saved, "name"),
        ..Default::default()
    };
    if let Ok(me) = get("/me", &token) {
        if let Some(user) = me.get("user") {
            account.name = text_of(user, "name");
            account.login = text_of(user, "login");
        }
    }
    if let Ok(serde_json::Value::Array(orgs)) = get("/orgs", &token) {
        account.orgs = orgs
            .iter()
            .map(|o| (text_of(o, "name"), text_of(o, "role")))
            .filter(|(name, _)| !name.is_empty())
            .collect();
    }
    if let Ok(mine) = get("/my/apps", &token) {
        account.published = mine
            .get("apps")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
            .map(|a| {
                let line = text_of(a, "description");
                (text_of(a, "hash"), text_of(a, "name"), line)
            })
            .filter(|(id, name, _)| !id.is_empty() && !name.is_empty())
            .collect();
    }
    account.avatar = avatar(&text_of(&saved, "avatar_url"), art_dir);
    Some(account)
}

/// The avatar as a ready-to-draw picture in `art_dir` (fetched once).
fn avatar(url: &str, art_dir: &Path) -> Option<String> {
    const NAME: &str = "avatar.rgba";
    if art_dir.join(NAME).exists() {
        return Some(NAME.to_string());
    }
    if !url.starts_with("https://") {
        return None;
    }
    let response = agent().get(url).call().ok()?;
    let mut png = Vec::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_end(&mut png)
        .ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()]
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()]
            .iter()
            .flat_map(|&g| [g, g, g, 255])
            .collect(),
        _ => return None,
    };
    let mut file = Vec::with_capacity(8 + rgba.len());
    file.extend_from_slice(&info.width.to_le_bytes());
    file.extend_from_slice(&info.height.to_le_bytes());
    file.extend_from_slice(&rgba);
    std::fs::write(art_dir.join(NAME), file).ok()?;
    Some(NAME.to_string())
}
