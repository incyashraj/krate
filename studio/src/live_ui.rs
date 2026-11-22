//! Studio's interface, updated without a release (IC-998).
//!
//! Studio's screens are compiled into it, so every interface fix used to
//! wait for a release. Now Studio can take a newer interface from the hub:
//! one bundle (every file of studio/ui), signed with Krate's interface key.
//!
//! What keeps that safe:
//!   * The signature is checked here, against the public key compiled in
//!     below, every time a bundle is read -- the hub, the network and the
//!     disk are never trusted. The private key lives on one machine.
//!   * A bundle only replaces files the webview loads, through Tauri's own
//!     asset seam (`Context::set_assets`): the same `tauri://` origin, the
//!     same content security policy, the same commands and permission
//!     walls. A bundle can do nothing the built-in interface cannot.
//!   * A bundle older than this build's own interface is not used, nor one
//!     that calls a command this Studio does not answer.
//!   * Anything wrong -- no bundle, a bad signature, a broken file, no
//!     network -- leaves the built-in interface, exactly as before.
//!
//! The bundle: `{"format":1, "serial":<commit time>, "commit":"...",
//! "commands":[...], "files":{"index.html":"<base64>", ...}}`, and beside it
//! the Ed25519 signature of those exact bytes, hex.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use base64::Engine as _;
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::{Assets, Wry};

include!(concat!(env!("OUT_DIR"), "/studio_commands.rs"));

/// Krate's interface key, the public half (raw Ed25519). The private half
/// signs bundles in scripts/ship-studio-ui.sh and never leaves that machine.
const UI_PUBLIC_KEY: [u8; 32] = [
    0x6b, 0xcf, 0xcc, 0x26, 0xc6, 0x85, 0x86, 0xc9, 0x85, 0x9f, 0xe0, 0x72, 0x74, 0xbd, 0xbb, 0xb7,
    0x6c, 0x0b, 0xa7, 0xd0, 0x0b, 0x67, 0x0b, 0x9f, 0x9c, 0x07, 0x7b, 0x2d, 0xcb, 0x78, 0x31, 0xc6,
];

/// When this build's own interface last changed (build.rs, git commit time).
pub fn built_serial() -> u64 {
    env!("KRATE_UI_SERIAL").parse().unwrap_or(0)
}

/// A checked bundle, ready to serve.
#[derive(Debug)]
pub struct Bundle {
    pub serial: u64,
    pub commit: String,
    files: HashMap<String, Vec<u8>>,
}

#[derive(serde::Deserialize)]
struct Raw {
    format: u32,
    serial: u64,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    commands: Vec<String>,
    files: HashMap<String, String>,
}

/// Check a bundle against the key, this build and itself. `Err` says why
/// it is not used, in words a log can carry.
pub fn verify(
    bytes: &[u8],
    sig_hex: &str,
    key: &[u8; 32],
    built: u64,
    commands: &[&str],
) -> Result<Bundle, String> {
    let sig = hex_bytes(sig_hex.trim()).ok_or("the signature is not hex")?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(bytes, &sig)
        .map_err(|_| "the signature does not match Krate's interface key".to_string())?;
    let raw: Raw = serde_json::from_slice(bytes).map_err(|e| format!("not a bundle: {e}"))?;
    if raw.format != 1 {
        return Err(format!(
            "bundle format {} is newer than this Studio reads",
            raw.format
        ));
    }
    if raw.serial <= built {
        return Err("it is not newer than the interface this Studio was built with".into());
    }
    if let Some(missing) = raw
        .commands
        .iter()
        .find(|c| !commands.contains(&c.as_str()))
    {
        return Err(format!(
            "it calls {missing}, which this Studio does not have -- it needs a newer Studio"
        ));
    }
    if !raw.files.contains_key("index.html") {
        return Err("it has no index.html".into());
    }
    let mut files = HashMap::new();
    for (path, b64) in raw.files {
        if path.contains("..") || path.starts_with('/') {
            return Err(format!("{path} is not a path inside the interface"));
        }
        let body = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| format!("{path} is not base64"))?;
        files.insert(path, body);
    }
    Ok(Bundle {
        serial: raw.serial,
        commit: raw.commit,
        files,
    })
}

fn hex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The bundle in use, if any. Swapped when a newer one arrives.
fn live() -> &'static RwLock<Option<Arc<Bundle>>> {
    static LIVE: OnceLock<RwLock<Option<Arc<Bundle>>>> = OnceLock::new();
    LIVE.get_or_init(|| RwLock::new(None))
}

/// The serial being served: the bundle's, or this build's own.
pub fn serving_serial() -> u64 {
    live()
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|b| b.serial))
        .unwrap_or_else(built_serial)
}

fn install(bundle: Bundle) {
    eprintln!(
        "studio: using the interface from {} (newer than the built-in one)",
        if bundle.commit.is_empty() {
            "a signed update"
        } else {
            &bundle.commit
        }
    );
    if let Ok(mut g) = live().write() {
        *g = Some(Arc::new(bundle));
    }
}

/// Where the last good bundle is kept.
pub fn cache_dir(studio_dir: &Path) -> PathBuf {
    studio_dir.join("ui-live")
}

/// At start: use the bundle on disk when it still checks out.
pub fn load_cached(studio_dir: &Path) {
    let dir = cache_dir(studio_dir);
    let (Ok(bytes), Ok(sig)) = (
        std::fs::read(dir.join("bundle.json")),
        std::fs::read_to_string(dir.join("bundle.sig")),
    ) else {
        return;
    };
    match verify(
        &bytes,
        &sig,
        &UI_PUBLIC_KEY,
        built_serial(),
        STUDIO_COMMANDS,
    ) {
        Ok(bundle) => install(bundle),
        Err(why) => {
            eprintln!("studio: the kept interface update is not used: {why}");
            // Never keep reading a bundle that will not be used again.
            if why.contains("not newer") || why.contains("signature") {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

/// Ask the hub for a newer interface; keep and use it when it checks out.
/// True when a new one was installed (the window may reload to show it).
pub fn fetch_newer(hub: &str, studio_dir: &Path) -> bool {
    let latest: serde_json::Value = match krate_http::get(&format!("{hub}/studio-ui/latest"))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .ok()
        .and_then(|r| r.into_json().ok())
    {
        Some(v) => v,
        None => return false,
    };
    let serial = latest["serial"].as_u64().unwrap_or(0);
    if serial <= serving_serial() {
        return false;
    }
    let get = |path: &str| -> Option<Vec<u8>> {
        let resp = krate_http::get(&format!("{hub}{path}"))
            .timeout(std::time::Duration::from_secs(60))
            .call()
            .ok()?;
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut body).ok()?;
        Some(body)
    };
    let (Some(bytes), Some(sig)) = (get("/studio-ui/bundle"), get("/studio-ui/bundle.sig")) else {
        return false;
    };
    let sig = String::from_utf8_lossy(&sig).to_string();
    let bundle = match verify(
        &bytes,
        &sig,
        &UI_PUBLIC_KEY,
        built_serial(),
        STUDIO_COMMANDS,
    ) {
        Ok(b) => b,
        Err(why) => {
            eprintln!("studio: the hub's interface update is not used: {why}");
            return false;
        }
    };
    let dir = cache_dir(studio_dir);
    let tmp = dir.with_extension("new");
    let _ = std::fs::remove_dir_all(&tmp);
    if std::fs::create_dir_all(&tmp).is_ok()
        && std::fs::write(tmp.join("bundle.json"), &bytes).is_ok()
        && std::fs::write(tmp.join("bundle.sig"), sig.as_bytes()).is_ok()
    {
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::rename(&tmp, &dir);
    }
    install(bundle);
    true
}

/// What the webview loads: the live bundle's file when it has one, else
/// the built-in file. The same assets seam, so the same origin and policy.
pub struct LiveAssets {
    builtin: Box<dyn Assets<Wry>>,
}

impl LiveAssets {
    pub fn new(builtin: Box<dyn Assets<Wry>>) -> Self {
        Self { builtin }
    }

    /// A stand-in for the moment the built-in assets are taken out of the
    /// context to be wrapped. Serves nothing; never left in place.
    pub fn empty() -> Self {
        Self {
            builtin: Box::new(NoAssets),
        }
    }
}

struct NoAssets;

impl Assets<Wry> for NoAssets {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

impl Assets<Wry> for LiveAssets {
    fn get(&self, key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        let path = key.as_ref().trim_start_matches('/');
        if let Ok(g) = live().read() {
            if let Some(bundle) = g.as_ref() {
                if let Some(body) = bundle.files.get(path) {
                    return Some(Cow::Owned(body.clone()));
                }
            }
        }
        self.builtin.get(key)
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        self.builtin.iter()
    }

    fn csp_hashes(&self, html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        // script-src 'self' with no inline scripts: no hashes either way.
        self.builtin.csp_hashes(html_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn keypair() -> Ed25519KeyPair {
        let rng = ring::rand::SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap()
    }

    fn bundle(serial: u64, commands: &[&str]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": 1, "serial": serial, "commit": "abc",
            "commands": commands,
            "files": { "index.html": base64::engine::general_purpose::STANDARD.encode("<p>new</p>") }
        }))
        .unwrap()
    }

    fn sign(kp: &Ed25519KeyPair, bytes: &[u8]) -> String {
        kp.sign(bytes)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn key(kp: &Ed25519KeyPair) -> [u8; 32] {
        kp.public_key().as_ref().try_into().unwrap()
    }

    #[test]
    fn a_signed_newer_bundle_is_used() {
        let kp = keypair();
        let b = bundle(200, &["me_info"]);
        let got = verify(&b, &sign(&kp, &b), &key(&kp), 100, &["me_info", "sign_ins"]).unwrap();
        assert_eq!(got.serial, 200);
        assert_eq!(got.files["index.html"], b"<p>new</p>");
    }

    #[test]
    fn a_changed_byte_or_another_key_is_refused() {
        let kp = keypair();
        let b = bundle(200, &[]);
        let sig = sign(&kp, &b);
        let mut tampered = b.clone();
        let at = tampered.len() - 5;
        tampered[at] ^= 1;
        assert!(verify(&tampered, &sig, &key(&kp), 100, &[])
            .unwrap_err()
            .contains("signature"));
        let other = keypair();
        assert!(verify(&b, &sig, &key(&other), 100, &[])
            .unwrap_err()
            .contains("signature"));
        assert!(verify(&b, "zz", &key(&kp), 100, &[]).is_err());
    }

    #[test]
    fn an_older_bundle_or_one_for_a_newer_studio_is_refused() {
        let kp = keypair();
        let old = bundle(100, &[]);
        assert!(verify(&old, &sign(&kp, &old), &key(&kp), 100, &[])
            .unwrap_err()
            .contains("not newer"));
        let ahead = bundle(200, &["a_command_from_next_year"]);
        let why = verify(&ahead, &sign(&kp, &ahead), &key(&kp), 100, &["me_info"]).unwrap_err();
        assert!(
            why.contains("a_command_from_next_year") && why.contains("newer Studio"),
            "{why}"
        );
    }

    #[test]
    fn a_path_outside_the_interface_is_refused() {
        let kp = keypair();
        let b = serde_json::to_vec(&serde_json::json!({
            "format": 1, "serial": 200, "commands": [],
            "files": { "index.html": "", "../../etc/x": "" }
        }))
        .unwrap();
        assert!(verify(&b, &sign(&kp, &b), &key(&kp), 100, &[])
            .unwrap_err()
            .contains("not a path"));
    }

    #[test]
    fn this_build_knows_its_own_commands() {
        assert!(STUDIO_COMMANDS.contains(&"me_info"));
        assert!(
            STUDIO_COMMANDS.contains(&"ide_devices"),
            "module commands are listed by name"
        );
        assert!(STUDIO_COMMANDS
            .iter()
            .all(|c| !c.contains('#') && !c.contains(':')));
    }

    #[test]
    fn the_compiled_key_is_krates() {
        // The public half of ~/.krate-studio-ui/ui-signing-key.pem.
        assert_eq!(UI_PUBLIC_KEY[0], 0x6b);
        assert_eq!(UI_PUBLIC_KEY[31], 0xc6);
    }
}
