//! What the Android player decides about an app's run before it starts: how
//! big an app it will open, how much memory the app may use, where its data
//! lives, and how Krate's menu words what it was given. Kept apart from the
//! phone-only code so every rule here is tested on any machine.

use krate_manifest::Manifest;
use krate_policy::SessionPolicy;
use std::path::{Path, PathBuf};

/// The largest app the player opens: the bundle format's own ceiling for
/// anything a stranger can hand Krate -- a link, a phone -- the same the hub
/// publishes under and the iPhone opens. It was 6 MB here alone, so a game
/// with its sounds and pictures, published and opening everywhere else,
/// said "larger than any Krate app" on Android.
pub const MAX_BUNDLE_BYTES: u64 = krate_bundle::HUB_LIMITS.bundle_bytes;

/// The hub a shared store (store.shared) syncs through: the one the desktop
/// and the browser tab use, so a phone and a computer holding the same
/// invite code see the same data.
pub const SHARED_HUB: &str = "https://hub.krate.tech";

/// Where one app's data lives on the phone, beside its sandbox and never in
/// it: the app names keys and tables, never files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stores {
    pub kv: PathBuf,
    pub sql: PathBuf,
    pub secrets: PathBuf,
    pub shared: PathBuf,
}

/// The data paths for `app_id` under Krate's own `stores` folder.
pub fn stores(data_root: &Path, app_id: &str) -> Stores {
    let kv = data_root
        .join("stores")
        .join(format!("{}.kv", sanitize(app_id)));
    Stores {
        sql: kv.with_extension("sqlite"),
        secrets: kv.with_extension("secrets"),
        // The desktop's own name for the mirror, so the two never differ.
        shared: kv.with_extension("shared.json"),
        kv,
    }
}

/// How much memory the app may use, in bytes, when it asked for more than
/// the default with `memory.budget:<MiB>` and the person left that on at
/// the wall; None for the default. The desktop's rule (K-416): a budget
/// nobody granted changes nothing, and a malformed one is no budget. Before
/// this every app on Android ran in the default 256 MiB whatever the person
/// allowed, so a photo editor or a big game that asked for 1 GB ran out of
/// memory on a phone with eight.
pub fn memory_budget_bytes(manifest: &Manifest, policy: &SessionPolicy) -> Option<u64> {
    manifest
        .declared_capabilities()
        .ok()?
        .into_iter()
        .filter(|cap| cap.module() == "memory" && cap.action() == "budget")
        .filter(|cap| policy.allows(cap))
        .filter_map(|cap| {
            cap.resource()
                .and_then(|mib| krate_manifest::validate_memory_budget(mib).ok())
        })
        .max()
        .and_then(|mib| mib.checked_mul(1024 * 1024))
}

/// A granted capability as the permission sheet words it (apps/krate-wall
/// `describe`), or None for what every app has: its window, its output,
/// the clock and randomness. The iPhone player's list, for Krate's menu.
pub fn use_in_words(cap: &str) -> Option<String> {
    let mut parts = cap.splitn(2, ':');
    let kind = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let host = target.split(':').next().unwrap_or("");
    let own = target.is_empty() || target.starts_with('.');
    Some(match kind {
        "ui.window" | "io.stdout" | "io.stderr" | "io.stdin" | "io.args" | "io.env" | "io.log"
        | "time.clock" | "time.monotonic" | "time.sleep" | "random.bytes" | "locale.info"
        | "locale.format" | "memory.budget" => return None,
        "net.fetch" | "net.connect" if !host.is_empty() && host != "*" => {
            format!("The internet, at {host}")
        }
        "net.fetch" | "net.connect" => "The internet".to_string(),
        "net.listen" => "The local network".to_string(),
        "net.ws" => "A live connection".to_string(),
        "net.connection" => "Your organisation's connection".to_string(),
        "hub" | "hub.rooms" => "Krate online".to_string(),
        "fs.read" | "fs.list" if own => "Read its own files".to_string(),
        "fs.write" | "fs.remove" | "fs.mkdir" if own => "Save its own files".to_string(),
        "fs.read" | "fs.list" => "Read files".to_string(),
        "fs.write" | "fs.remove" | "fs.mkdir" => "Change files".to_string(),
        "ui.dialog" => "Files you pick".to_string(),
        "ui.dropzone" => "Files you drop on it".to_string(),
        "audio.playback" => "Sound".to_string(),
        "audio.capture" => "The microphone".to_string(),
        "speech" | "speech.say" | "speech.listen" => "Speech".to_string(),
        "camera.capture" | "camera" => "The camera".to_string(),
        "location.read" => "Where you are".to_string(),
        "bluetooth.connect" => "Bluetooth devices you pick".to_string(),
        "photos.add" => "Adding to your Photos".to_string(),
        "store.kv" | "store.sql" => "Keeping its data".to_string(),
        "store.shared" => "Sharing data with people you invite".to_string(),
        "store.secret" => "Keeping secrets safe".to_string(),
        "store.group" => "Sharing data with its maker's other apps".to_string(),
        "store.org" => "Your organisation's data".to_string(),
        // Refused on a phone (IC-977), named so the menu is not a raw id.
        "process.run" => "Running programs".to_string(),
        "process.terminal" => "A terminal".to_string(),
        "ui.clipboard" => "The clipboard".to_string(),
        "ui.notify" => "Notifications".to_string(),
        "ui.open-url" => "Opening links".to_string(),
        "ui.menu" => "A menu".to_string(),
        "gfx.gpu" => "The graphics chip".to_string(),
        _ => cap.to_string(),
    })
}

/// App ids become file and directory names; nothing an id says may escape
/// Krate's folders.
pub fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(caps: &str) -> Manifest {
        Manifest::parse(&format!(
            "[app]\nid = \"dev.krate.big\"\nname = \"Big\"\nversion = \"1.0.0\"\n\
             entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n{caps}"
        ))
        .expect("manifest")
    }

    fn granting(manifest: &Manifest, granted: &[&str]) -> SessionPolicy {
        let declared = manifest.declared_capabilities().expect("caps");
        SessionPolicy::from_grants(
            manifest
                .capabilities
                .iter()
                .zip(declared)
                .filter(|(request, _)| granted.contains(&request.cap.as_str()))
                .map(|(_, cap)| cap),
        )
    }

    const BIG: &str = "[[capabilities]]\ncap = \"memory.budget:1024\"\n\
                       rationale = \"Big photos\"\nrequired = false\n";

    #[test]
    fn a_granted_memory_budget_is_the_apps_and_an_ungranted_one_is_not() {
        let m = manifest(BIG);
        assert_eq!(
            memory_budget_bytes(&m, &granting(&m, &["memory.budget:1024"])),
            Some(1024 * 1024 * 1024)
        );
        // Switched off at the wall: the default, not the ask.
        assert_eq!(memory_budget_bytes(&m, &granting(&m, &[])), None);
        // No ask at all.
        let plain = manifest("");
        assert_eq!(memory_budget_bytes(&plain, &granting(&plain, &[])), None);
    }

    #[test]
    fn the_player_opens_what_the_hub_publishes() {
        // Not a private ceiling of its own: the format's door for a link or
        // a phone, which is far past a game with its assets.
        assert_eq!(MAX_BUNDLE_BYTES, krate_bundle::HUB_LIMITS.bundle_bytes);
        const { assert!(MAX_BUNDLE_BYTES >= 64 * 1024 * 1024) };
    }

    #[test]
    fn an_apps_data_lives_beside_its_sandbox_under_a_safe_name() {
        let got = stores(Path::new("/data/krate"), "../evil/app");
        assert_eq!(got.kv, Path::new("/data/krate/stores/.._evil_app.kv"));
        assert_eq!(got.sql, Path::new("/data/krate/stores/.._evil_app.sqlite"));
        assert_eq!(
            got.shared,
            Path::new("/data/krate/stores/.._evil_app.shared.json")
        );
        assert!(got.secrets.starts_with("/data/krate/stores"));
        assert!(!sanitize("a/b\\c").contains(['/', '\\']));
    }

    #[test]
    fn the_menu_words_what_the_app_was_given() {
        assert_eq!(use_in_words("ui.window:create"), None);
        assert_eq!(use_in_words("time.sleep"), None);
        // Every capability Krate knows is worded, never shown as its code.
        for spec in krate_manifest::supported_capability_specs() {
            let cap = format!("{}.{}", spec.module(), spec.action());
            if let Some(words) = use_in_words(&cap) {
                assert_ne!(words, cap, "{cap} has no words in Krate's menu");
            }
        }
        assert_eq!(
            use_in_words("net.connect:api.example.com:443").as_deref(),
            Some("The internet, at api.example.com")
        );
        assert_eq!(
            use_in_words("store.shared").as_deref(),
            Some("Sharing data with people you invite")
        );
        assert_eq!(
            use_in_words("fs.mkdir:./notes").as_deref(),
            Some("Save its own files")
        );
    }
}
