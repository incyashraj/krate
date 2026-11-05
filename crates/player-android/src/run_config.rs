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

/// Whether an app is a command-line program (it prints and ends) rather than
/// one with a window. A phone has no terminal, so what such an app printed
/// is shown on Krate's own card once it ends (`output_args`).
pub fn prints_only(manifest: &Manifest) -> bool {
    let world = manifest.app.world.as_str();
    world.starts_with("krate:app/cli") || world.starts_with("krate:phase1")
}

/// The most characters of any one line of a program's output the card keeps.
pub const OUTPUT_LINE_CHARS: usize = 2_000;

/// The most arguments, and the most bytes of them, an app can be handed: the
/// runtime's `APP_ARGS_LIMITS`, held equal below wherever the runtime is
/// linked (the phones), and tested against its own rule on the host.
pub const APP_ARGS_LIMITS: (usize, usize) = (1024, 64 * 1024);
#[cfg(any(target_os = "android", target_os = "ios"))]
const _: () = assert!(
    APP_ARGS_LIMITS.0 == krate_runtime::APP_ARGS_LIMITS.0
        && APP_ARGS_LIMITS.1 == krate_runtime::APP_ARGS_LIMITS.1
);

/// The wall's `wall:output` arguments for a program that ended: its name, how
/// it ended in plain words, then each printed line -- the last ones, as a
/// terminal's scrollback keeps, as many as an app can be handed
/// (`APP_ARGS_LIMITS`: past them the wall could read none of
/// its arguments and the card would never show). Arguments may not be empty
/// or carry a newline or a NUL, so lines are split on the newline, cleaned of
/// NULs and carriage returns, and a blank line is one space.
pub fn output_args(name: &str, exit_code: Option<i32>, transcript: &[u8]) -> Vec<String> {
    let ended = match exit_code {
        Some(0) => "Finished".to_string(),
        Some(code) => format!("Ended with code {code}"),
        None => "Stopped: it used more than it was allowed".to_string(),
    };
    let text = String::from_utf8_lossy(transcript);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let kept: String = line
                .chars()
                .filter(|c| *c != '\r' && *c != '\0')
                .take(OUTPUT_LINE_CHARS)
                .collect();
            if kept.is_empty() {
                " ".to_string()
            } else {
                kept
            }
        })
        .collect();
    // The newline that ends the last line is not one more, empty, line.
    if text.ends_with('\n') || text.is_empty() {
        lines.pop();
    }
    let mut args = vec![
        "wall:output".to_string(),
        name.replace(['\n', '\0'], " "),
        ended,
    ];
    let (most_args, most_bytes) = APP_ARGS_LIMITS;
    // Room for the header, the "earlier lines" note, and each line's newline.
    let header: usize = args.iter().map(|a| a.len() + 1).sum::<usize>() + 64;
    let mut room = most_bytes.saturating_sub(header);
    let mut kept = 0;
    for line in lines.iter().rev() {
        if args.len() + 1 + kept + 1 > most_args || line.len() + 1 > room {
            break;
        }
        room -= line.len() + 1;
        kept += 1;
    }
    let skip = lines.len() - kept;
    if skip > 0 {
        args.push(format!("({skip} earlier lines not kept)"));
    }
    args.extend(lines.into_iter().skip(skip));
    args
}

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

/// The publisher an app's signature vouches for -- the signing root -- or
/// `None` for an unsigned app or a signature that does not hold. The same
/// rule the desktop keys its storage by (IC-736), so a phone and a computer
/// put one app's synced data in the same place (IC-981).
pub fn verified_publisher(open: &krate_bundle::OpenBundle) -> Option<String> {
    let verdict = open
        .full_verdict(&krate_bundle::delegation::RevocationState::Unknown {
            last_known: Vec::new(),
        })
        .ok()
        .flatten()?;
    if !verdict.is_trustworthy() {
        return None;
    }
    open.signing_authority().ok().flatten()
}

/// What a phone's run of one app needs to keep its store.kv the same on
/// every device the person signs in on (IC-981). The player turns it into
/// the runtime's `AccountSyncContext` with its device name and machine key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPlan {
    pub hub: &'static str,
    pub token: String,
    /// The phone's account-sync folder: device keys, the key it holds.
    pub device_dir: PathBuf,
    pub publisher: String,
    /// The toggles: `<store>.sync/off` for this app, `account-sync/off`
    /// for everything (S2 puts both on screen).
    pub enabled: bool,
}

/// [`SyncPlan`] for a run, or `None` when nobody is signed in on this phone
/// -- then the store is exactly what it always was.
pub fn sync_plan(
    data_root: &Path,
    verified_root: Option<&str>,
    store: &Path,
    token: Option<String>,
) -> Option<SyncPlan> {
    let token = token.filter(|t| !t.trim().is_empty())?;
    let device_dir = data_root.join("account-sync");
    Some(SyncPlan {
        hub: SHARED_HUB,
        token,
        enabled: krate_accountsync::toggles::sync_wanted(store, &device_dir),
        device_dir,
        publisher: krate_accountsync::publisher_label(verified_root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_follows_the_sign_in_and_the_toggles() {
        let dir = tempfile::tempdir().expect("dir");
        let store = stores(dir.path(), "dev.krate.reminders").kv;
        assert_eq!(
            sync_plan(dir.path(), None, &store, None),
            None,
            "nobody signed in: the store as it always was"
        );
        assert_eq!(sync_plan(dir.path(), None, &store, Some(" ".into())), None);
        let on = sync_plan(dir.path(), None, &store, Some("krs_x".into())).expect("signed in");
        assert!(on.enabled);
        assert_eq!(on.publisher, "unsigned");
        assert_eq!(on.hub, SHARED_HUB);
        assert_eq!(on.device_dir, dir.path().join("account-sync"));
        krate_accountsync::toggles::set_app_sync(&store, false).expect("off");
        let off =
            sync_plan(dir.path(), Some("AB12"), &store, Some("krs_x".into())).expect("signed in");
        assert!(!off.enabled, "the toggle stops it");
        assert_eq!(
            off.publisher, "ab12",
            "the publisher the bundle vouches for"
        );
    }

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

    #[test]
    fn a_program_s_output_becomes_one_argument_per_line() {
        let args = output_args(
            "grex",
            Some(2),
            b"error: missing <INPUT>\r\n\nUsage: grex\0 [OPTIONS]\n",
        );
        assert_eq!(
            args,
            vec![
                "wall:output",
                "grex",
                "Ended with code 2",
                "error: missing <INPUT>",
                " ",
                "Usage: grex [OPTIONS]",
            ]
        );
        krate_runtime::app_args_fit(&args).expect("the wall can read a blank line");
        assert_eq!(output_args("x", Some(0), b"")[2], "Finished");
        assert_eq!(
            output_args("x", Some(0), b"").len(),
            3,
            "nothing printed, no lines"
        );
        assert_eq!(output_args("x", Some(0), b"\n").len(), 4, "one blank line");
        assert!(output_args("x", None, b"")[2].starts_with("Stopped"));
    }

    #[test]
    fn a_long_output_keeps_the_last_lines_the_wall_can_read() {
        let (most_args, most_bytes) = APP_ARGS_LIMITS;
        // Many short lines: the count is the limit.
        let many: String = (0..5_000).map(|i| format!("line {i}\n")).collect();
        let args = output_args("x", Some(0), many.as_bytes());
        krate_runtime::app_args_fit(&args).expect("many lines still open");
        assert!(args.len() > most_args - 8, "kept as many as fit");
        assert_eq!(args.last().map(String::as_str), Some("line 4999"));
        let skipped = 5_000 - (args.len() - 4);
        assert_eq!(args[3], format!("({skipped} earlier lines not kept)"));
        // A few huge lines: the bytes are the limit.
        let wide = format!("{}\n", "x".repeat(OUTPUT_LINE_CHARS * 2)).repeat(100);
        let args = output_args("x", Some(0), wide.as_bytes());
        krate_runtime::app_args_fit(&args).expect("wide lines still open");
        assert_eq!(args[4].len(), OUTPUT_LINE_CHARS);
        let bytes: usize = args.iter().map(|a| a.len() + 1).sum();
        assert!(
            bytes > most_bytes - OUTPUT_LINE_CHARS * 2,
            "kept as many as fit"
        );
        // Multi-byte text counts in bytes, not characters.
        let wide_chars = format!("{}\n", "\u{1F600}".repeat(OUTPUT_LINE_CHARS)).repeat(40);
        let args = output_args("x", Some(0), wide_chars.as_bytes());
        krate_runtime::app_args_fit(&args).expect("emoji lines still open");
    }

    #[test]
    fn only_a_command_line_world_prints_only() {
        let with = |world: &str| {
            Manifest::parse(&format!(
                "[app]\nid = \"com.acme.x\"\nname = \"x\"\nversion = \"0.1.0\"\nentry = \"x.wasm\"\nworld = \"{world}\"\n"
            ))
            .expect("manifest")
        };
        assert!(prints_only(&with("krate:app/cli@0.1.0")));
        assert!(!prints_only(&with("krate:app/gui@0.2.0")));
    }
}
