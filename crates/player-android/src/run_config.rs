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

/// Where one app's data lives on the phone: its own folder (where it runs,
/// what it reads and writes as files) and, beside it and never in it, its
/// stores -- the app names keys and tables, never files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stores {
    pub kv: PathBuf,
    pub sql: PathBuf,
    pub secrets: PathBuf,
    pub shared: PathBuf,
    /// The app's own folder: `apps/<name>`.
    pub files: PathBuf,
    /// What its secrets are sealed under, mixed with the phone's key: the
    /// desktop's rule (IC-736), the store's own name for a signed app, so
    /// an impostor cannot derive the key; the bare id for an unsigned one,
    /// as it always was, so its secrets still open.
    pub secret_label: String,
}

/// Where the data of `app_id` lives under `data_root` (K-1172).
///
/// A signed app -- `publisher` is the root its signature verified to
/// ([`verified_publisher`]) -- is keyed by its id AND its publisher, the
/// desktop's rule and name (`<id>@<first 16 of the root>`,
/// krate_runtime::storage_key): another app that declares the same id
/// reaches a different place. An unsigned app keeps the id-keyed path it
/// always had, byte for byte: moving it would move people's data, and that
/// is the founder's call (K-1172's open half).
pub fn stores(data_root: &Path, app_id: &str, publisher: Option<&str>) -> Stores {
    let (name, secret_label) = match publisher {
        Some(publisher) => {
            let key = krate_runtime::storage_key::verified_storage_key(app_id, publisher);
            (key.clone(), key)
        }
        None => (sanitize(app_id), app_id.to_string()),
    };
    let store = |suffix: &str| store_file(data_root, &name, suffix);
    Stores {
        kv: store("kv"),
        sql: store("sqlite"),
        secrets: store("secrets"),
        // The desktop's own name for the mirror, so the two never differ.
        shared: store("shared.json"),
        files: data_root.join("apps").join(&name),
        secret_label,
    }
}

/// `stores/<name>.<suffix>`, built by name rather than by swapping a path's
/// extension, so a dotted id never loses a piece of itself.
fn store_file(data_root: &Path, name: &str, suffix: &str) -> PathBuf {
    data_root.join("stores").join(format!("{name}.{suffix}"))
}

/// The files beside a store that belong to it: SQLite's journal (the
/// database's newest writes can be in it) and sync's folder of what it
/// knows. What moves with a store moves all of these; what is cleared
/// clears the data ones.
const SQL_SIDECARS: [&str; 3] = ["sqlite-wal", "sqlite-shm", "sqlite-journal"];

impl Stores {
    /// Every data file of the app's stores, its journal included.
    fn data_files(&self) -> Vec<PathBuf> {
        let mut files = vec![
            self.kv.clone(),
            self.sql.clone(),
            self.secrets.clone(),
            self.shared.clone(),
        ];
        files.extend(SQL_SIDECARS.iter().map(|s| self.sql.with_extension(s)));
        files
    }

    /// What the app keeps on this phone, in bytes: its folder and its
    /// stores. The home's Storage line.
    pub fn size(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| match entry.metadata() {
                    Ok(m) if m.is_dir() => walk(&entry.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.files)
            + self
                .data_files()
                .iter()
                .filter_map(|f| std::fs::metadata(f).ok())
                .map(|m| m.len())
                .sum::<u64>()
    }

    /// Forget what the app keeps on this phone: its folder and its stores.
    /// Sync's switch for it stays, and so does the record of a signed app's
    /// one-time move ([`adopt_id_keyed_data`]): clearing never lets the
    /// next open take the id's data across again.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.files);
        for file in self.data_files() {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// What opening a signed app did with the data its id kept before phones
/// keyed signed apps by publisher (K-1172).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// An unsigned app: its data is where it always was.
    Unsigned,
    /// The id's old data was already taken across, by this publisher's app
    /// or another's, or there was none when the first signed app with this
    /// id opened: nothing moves now, or ever again for this id.
    Settled,
    /// This open was the first by a signed app with this id; the id kept
    /// nothing, so nothing moved.
    Nothing,
    /// This open took the id's data across: these pieces moved.
    Moved(Vec<String>),
}

/// The record that an id's old, id-keyed data has been decided:
/// `stores/<id>.moved`, the publisher that took it on its first line and
/// `done` on the second once every piece is across.
fn moved_record(data_root: &Path, app_id: &str) -> PathBuf {
    store_file(data_root, &sanitize(app_id), "moved")
}

/// The first time a signed app opens on this phone, take what its id kept
/// under the old id-keyed names across to its publisher's names, once
/// (K-1172).
///
/// Phones keyed every app's data by its id alone until now, so a signed
/// app's notes are under its bare id. The first signed app to open with
/// that id after this change takes them: each piece is renamed, never
/// copied, so nothing is left behind for another app declaring the id to
/// read. The id is then settled for good: a second publisher's app with
/// the same id, or this one opening again, moves nothing, and data an
/// unsigned app writes under the bare id later stays with it -- an
/// unsigned app with this id opens to a fresh, empty store, and what moved
/// stays with the signed publisher.
///
/// Before any of this the bare id's data was open to every app declaring
/// the id, so whichever signed app opens first takes nothing it could not
/// already read; what changes is that afterwards nobody else can.
///
/// The record is written before the first rename, naming the publisher,
/// and marked `done` after the last, so an open cut short (a crash, a
/// full disk) finishes the move the next time this same app opens, and
/// only then. `Err` is words for the person: the app should not open,
/// because a run now would start a second store beside a half-moved one.
pub fn adopt_id_keyed_data(
    data_root: &Path,
    app_id: &str,
    publisher: Option<&str>,
    machine_key: &[u8],
) -> Result<Adopted, String> {
    let Some(publisher) = publisher else {
        return Ok(Adopted::Unsigned);
    };
    let record = moved_record(data_root, app_id);
    let said =
        |e: std::io::Error| format!("its data could not be moved to its publisher's place ({e})");
    match std::fs::read_to_string(&record) {
        Ok(text) => {
            let mut lines = text.lines();
            let theirs = lines.next().unwrap_or("");
            if theirs != publisher || lines.next() == Some("done") {
                return Ok(Adopted::Settled);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(data_root.join("stores")).map_err(said)?;
            std::fs::write(&record, format!("{publisher}\n")).map_err(said)?;
        }
        Err(e) => return Err(said(e)),
    }

    let old = stores(data_root, app_id, None);
    let new = stores(data_root, app_id, Some(publisher));
    let old_name = sanitize(app_id);
    let new_name = krate_runtime::storage_key::verified_storage_key(app_id, publisher);
    let mut moved = Vec::new();
    // The journal before its database, so a cut-short move never leaves a
    // database at its new name without its newest writes beside it.
    let pieces =
        SQL_SIDECARS
            .into_iter()
            .chain(["sqlite", "kv.sync", "kv", "secrets", "shared.json"]);
    for suffix in pieces {
        let from = store_file(data_root, &old_name, suffix);
        let to = store_file(data_root, &new_name, suffix);
        if take_across(&from, &to).map_err(said)? {
            moved.push(suffix.to_string());
        }
    }
    if take_across(&old.files, &new.files).map_err(said)? {
        moved.push("files".to_string());
    }
    reseal_secrets(&new, &old.secret_label, machine_key)?;

    let done = record.with_extension("moved-tmp");
    std::fs::write(&done, format!("{publisher}\ndone\n")).map_err(said)?;
    std::fs::rename(&done, &record).map_err(said)?;
    Ok(if moved.is_empty() {
        Adopted::Nothing
    } else {
        Adopted::Moved(moved)
    })
}

/// Rename `from` to `to` when `from` is there and `to` is not. One rename
/// on one disk: the piece is in one place or the other, never both.
fn take_across(from: &Path, to: &Path) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(from).is_err() || std::fs::symlink_metadata(to).is_ok() {
        return Ok(false);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)?;
    Ok(true)
}

/// Secrets that moved were sealed under the bare id; a signed app's are
/// sealed under its store's name (the desktop's rule). Open them with the
/// old label and seal them again with the new, beside the file and then
/// over it, so the secrets are readable under one label or the other at
/// every moment.
///
/// A secrets file that does not decrypt reads as empty rather than as an
/// error (the runtime's rule: "not yours" must not say secrets exist), so
/// the test is what each label READS: something under the new label means
/// this is done; nothing under either means there is nothing to carry, and
/// the file is left exactly as it is.
fn reseal_secrets(new: &Stores, old_label: &str, machine_key: &[u8]) -> Result<(), String> {
    use krate_runtime::secret_host::AppSecrets;
    let read = |label: &str| AppSecrets::open(new.secrets.clone(), label, machine_key, true);
    if !new.secrets.exists()
        || !read(&new.secret_label)
            .names()
            .unwrap_or_default()
            .is_empty()
    {
        return Ok(());
    }
    let sealed = read(old_label);
    let names = sealed.names().unwrap_or_default();
    if names.is_empty() {
        return Ok(());
    }
    let said = |e: String| format!("its saved secrets could not be moved ({e})");
    let beside = new.secrets.with_extension("secrets-reseal");
    let _ = std::fs::remove_file(&beside);
    let mut resealed = AppSecrets::open(beside.clone(), &new.secret_label, machine_key, true);
    for name in names {
        if let Some(value) = sealed.get(&name).map_err(|e| said(format!("{e:?}")))? {
            resealed
                .set(&name, value)
                .map_err(|e| said(format!("{e:?}")))?;
        }
    }
    std::fs::rename(&beside, &new.secrets).map_err(|e| said(e.to_string()))
}

/// A kept app as the home needs it: its manifest, and the publisher its
/// signature verifies to (None when unsigned), so the home's Storage and
/// Clear and sync reach the store the app's runs use (K-1172).
pub fn kept_app(path: &Path) -> Option<(Manifest, Option<String>)> {
    let open = krate_bundle::open(path).ok()?;
    let text = std::fs::read_to_string(open.manifest_path()).ok()?;
    let manifest = Manifest::parse(&text).ok()?;
    Some((manifest, verified_publisher(&open)))
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

/// Whether an app keeps files of its own: it asks to write, make folders
/// in or remove from its folder. Its folder then follows the person to
/// their other devices with its store (IC-981 S3).
pub fn writes_files(manifest: &Manifest) -> bool {
    manifest.declared_capabilities().is_ok_and(|caps| {
        caps.iter()
            .any(|c| c.module() == "fs" && matches!(c.action(), "write" | "mkdir" | "remove"))
    })
}

/// The person signed in on this phone, as Krate's sign-in kept them
/// (`account.json` in the Player's private storage): their login and the
/// hub session. `None` when nobody is signed in.
pub fn signed_in(private: &Path) -> Option<krate_orgaccess::SignedIn> {
    let text = std::fs::read_to_string(private.join("account.json")).ok()?;
    let kept: serde_json::Value = serde_json::from_str(&text).ok()?;
    let field = |key: &str| kept[key].as_str().unwrap_or("").trim().to_string();
    let (login, token) = (field("login"), field("token"));
    (!token.is_empty() && !login.is_empty()).then_some(krate_orgaccess::SignedIn { login, token })
}

/// This phone's machine key: 32 random bytes made once and kept in the
/// Player's private storage, which no other app reads. It seals what Krate
/// keeps for apps (their secrets, this phone's organisation key), so a
/// copied file carries nothing usable. An app-private file, not the
/// Android Keystore or the iPhone's keychain: the phone's own sandbox is
/// what protects it.
pub fn machine_key(private: &Path) -> Vec<u8> {
    let path = private.join("machine.key");
    if let Ok(existing) = std::fs::read(&path) {
        if existing.len() >= 32 {
            return existing;
        }
    }
    let mut key = vec![0u8; 32];
    let made = std::fs::File::open("/dev/urandom").and_then(|mut f| {
        use std::io::Read;
        f.read_exact(&mut key)
    });
    if made.is_ok() {
        let _ = std::fs::create_dir_all(private);
        let _ = std::fs::write(&path, &key);
    }
    key
}

/// This phone, as an organisation's rules see it (K-1171): the same rules a
/// computer runs (crates/orgaccess), over the Player's private storage and
/// the person signed in on Krate's Profile tab. A new phone is a new device
/// of the person's: it registers and waits until a teammate's device lets
/// it in, exactly as a new computer does.
pub struct Phone {
    /// The Player's private storage: `leases/`, `org-data/` and `org-keys/`
    /// live under it, beside the sign-in.
    pub private: PathBuf,
    pub machine_key: Vec<u8>,
    pub signed_in: Option<krate_orgaccess::SignedIn>,
    /// The hub: Krate's own, or a local one in a test.
    pub hub: String,
    /// What an organisation's device list calls this phone ("Pixel 8").
    pub name: String,
    /// What the run should show the person before the app opens: the
    /// offline terms the first time, and that this phone is waiting.
    pub told: std::cell::RefCell<Vec<(String, String)>>,
}

impl Phone {
    /// This phone as it is: its private storage, its sign-in, Krate's hub.
    pub fn here(private: &Path, name: &str) -> Phone {
        Phone {
            private: private.to_path_buf(),
            machine_key: machine_key(private),
            signed_in: signed_in(private),
            hub: SHARED_HUB.to_string(),
            name: name.to_string(),
            told: Default::default(),
        }
    }
}

impl krate_orgaccess::Device for Phone {
    fn home(&self) -> PathBuf {
        self.private.clone()
    }
    fn machine_key(&self) -> Vec<u8> {
        self.machine_key.clone()
    }
    fn signed_in(&self) -> Option<krate_orgaccess::SignedIn> {
        self.signed_in.clone()
    }
    fn hub(&self) -> Result<String, krate_orgaccess::GateError> {
        Ok(self.hub.clone())
    }
    fn kind(&self) -> &str {
        "phone"
    }
    fn name(&self, _principal: Option<&str>) -> String {
        self.name.clone()
    }
    fn sign_in_words(&self) -> String {
        "sign in on Krate's Profile tab".to_string()
    }
    fn tell(&self, title: &str, words: &str) {
        self.told
            .borrow_mut()
            .push((title.to_string(), words.to_string()));
    }
}

/// The words a phone shows while it waits for its team's key.
pub const WAITING_TITLE: &str = "Waiting for someone on your team to let this phone in";

/// What one run of an organisation's app on a phone is given: its team
/// data and company connections, and what to tell the person first.
pub struct OrgRun {
    /// `store.org` for the run; `None` for an app that is not an
    /// organisation's private app.
    pub app_org: Option<krate_runtime::org_store::OrgAccess>,
    pub connections: krate_runtime::connections::Connections,
    /// Where this phone stands with the team's key, when it was asked.
    pub standing: Option<krate_orgaccess::KeyStanding>,
    /// (title, words) to show before the app opens, in order.
    pub notices: Vec<(String, String)>,
}

/// Decide a phone's run of the app at `path` (its kept bytes, opened as
/// `open`): an organisation's private app checks in with its hub as the
/// person signed in here and gets its team data and connections, as on a
/// computer; any other app gets neither and nothing is asked of a hub.
/// `Err` is the words of a refusal: the app does not open.
pub fn org_run(
    phone: &Phone,
    path: &Path,
    open: &krate_bundle::OpenBundle,
    manifest: &Manifest,
    policy: &SessionPolicy,
) -> Result<OrgRun, String> {
    let mark = krate_orgaccess::mark_of(open);
    if matches!(mark, krate_orgaccess::Mark::None) {
        return Ok(OrgRun {
            app_org: None,
            connections: Default::default(),
            standing: None,
            notices: Vec::new(),
        });
    }
    let opened = krate_orgaccess::gate_opened(phone, krate_orgaccess::Bytes::File(path), mark)
        .map_err(|err| err.to_string())?;
    let Some(opened) = opened else {
        return Ok(OrgRun {
            app_org: None,
            connections: Default::default(),
            standing: None,
            notices: phone.told.take(),
        });
    };
    let app_org = krate_orgaccess::org_access(phone, &opened, &manifest.app.id, 0)?;
    let digest = opened.digest.clone();
    let connections =
        krate_orgaccess::connections::for_run(phone, Some(manifest), policy, Some(open), || {
            Some(digest)
        });
    // A phone new to the team says so before the app opens, so its first
    // screen is not read as an empty team.
    let standing = opened
        .principal
        .is_some()
        .then(|| krate_orgaccess::key_standing(phone, &opened));
    let mut notices = phone.told.take();
    if standing == Some(krate_orgaccess::KeyStanding::Waiting) {
        notices.push((WAITING_TITLE.to_string(), waiting_words(&opened.org_name)));
    }
    Ok(OrgRun {
        app_org: Some(app_org),
        connections,
        standing,
        notices,
    })
}

/// What a phone waiting for its team's key tells the person.
pub fn waiting_words(org_name: &str) -> String {
    format!(
        "This phone is new to {org_name}. The team's data appears once someone on the team opens \
         one of {org_name}'s apps on a device that already has it, which lets this phone in by \
         itself. Anything you add now is kept here and shared then."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_keeps_files_when_it_asks_to_write_them() {
        let writes = manifest(
            "[[capabilities]]\ncap = \"fs.write:notes/**\"\nrationale = \"x\"\nrequired = true\n",
        );
        assert!(writes_files(&writes));
        let reads = manifest(
            "[[capabilities]]\ncap = \"fs.read:notes/**\"\nrationale = \"x\"\nrequired = true\n",
        );
        assert!(!writes_files(&reads), "reading writes nothing to sync");
        assert!(!writes_files(&manifest("")));
    }

    #[test]
    fn sync_follows_the_sign_in_and_the_toggles() {
        let dir = tempfile::tempdir().expect("dir");
        let store = stores(dir.path(), "dev.krate.reminders", None).kv;
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
        let got = stores(Path::new("/data/krate"), "../evil/app", None);
        assert_eq!(got.kv, Path::new("/data/krate/stores/.._evil_app.kv"));
        assert_eq!(got.sql, Path::new("/data/krate/stores/.._evil_app.sqlite"));
        assert_eq!(
            got.shared,
            Path::new("/data/krate/stores/.._evil_app.shared.json")
        );
        assert!(got.secrets.starts_with("/data/krate/stores"));
        assert_eq!(got.files, Path::new("/data/krate/apps/.._evil_app"));
        assert!(!sanitize("a/b\\c").contains(['/', '\\']));
    }

    const ROOT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    /// K-1172: a signed app's data is keyed by its id and its publisher, the
    /// desktop's name for it; an app declaring the same id with no signature
    /// (or another publisher's) reaches a different place, every piece.
    #[test]
    fn a_signed_app_and_an_impostor_declaring_its_id_get_different_stores() {
        let data = Path::new("/data/krate");
        let signed = stores(data, "dev.krate.notes", Some(ROOT));
        let impostor = stores(data, "dev.krate.notes", None);
        let other = stores(data, "dev.krate.notes", Some(OTHER));
        assert_eq!(
            signed.kv,
            Path::new("/data/krate/stores/dev.krate.notes@0123456789abcdef.kv")
        );
        assert_eq!(
            signed.files,
            Path::new("/data/krate/apps/dev.krate.notes@0123456789abcdef")
        );
        assert_eq!(signed.secret_label, "dev.krate.notes@0123456789abcdef");
        for theirs in [&impostor, &other] {
            assert_ne!(signed.kv, theirs.kv);
            assert_ne!(signed.sql, theirs.sql);
            assert_ne!(signed.secrets, theirs.secrets);
            assert_ne!(signed.shared, theirs.shared);
            assert_ne!(signed.files, theirs.files);
            assert_ne!(signed.secret_label, theirs.secret_label);
        }
        // An unsigned app is exactly where it always was.
        assert_eq!(
            impostor.kv,
            Path::new("/data/krate/stores/dev.krate.notes.kv")
        );
        assert_eq!(
            impostor.files,
            Path::new("/data/krate/apps/dev.krate.notes")
        );
        assert_eq!(impostor.secret_label, "dev.krate.notes");
        // No id can spell its way into a publisher's store: `@` never
        // survives the phone's sanitising.
        let spelled = stores(data, "dev.krate.notes@0123456789abcdef", None);
        assert_ne!(spelled.kv, signed.kv);
        assert_ne!(spelled.files, signed.files);
    }

    /// The pieces of an app's old, id-keyed data, each with something in it.
    fn old_data(root: &Path, key: &[u8]) {
        let old = stores(root, "dev.krate.notes", None);
        std::fs::create_dir_all(root.join("stores")).unwrap();
        std::fs::create_dir_all(old.files.join("trips")).unwrap();
        std::fs::write(old.files.join("trips").join("rome.txt"), b"pasta").unwrap();
        std::fs::write(&old.kv, b"kv").unwrap();
        std::fs::write(&old.sql, b"sql").unwrap();
        std::fs::write(old.sql.with_extension("sqlite-wal"), b"wal").unwrap();
        std::fs::write(&old.shared, b"{}").unwrap();
        krate_accountsync::toggles::set_app_sync(&old.kv, false).unwrap();
        let mut secrets = krate_runtime::secret_host::AppSecrets::open(
            old.secrets.clone(),
            &old.secret_label,
            key,
            true,
        );
        secrets.set("token", b"s3cret".to_vec()).unwrap();
    }

    fn secret(stores: &Stores, label: &str, key: &[u8]) -> Option<Vec<u8>> {
        krate_runtime::secret_host::AppSecrets::open(stores.secrets.clone(), label, key, true)
            .get("token")
            .ok()
            .flatten()
    }

    #[test]
    fn a_signed_app_takes_its_ids_old_data_across_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let key = [7u8; 32];
        old_data(root, &key);
        let old = stores(root, "dev.krate.notes", None);
        let new = stores(root, "dev.krate.notes", Some(ROOT));

        let moved = adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &key).unwrap();
        let Adopted::Moved(pieces) = moved else {
            panic!("expected a move, got {moved:?}")
        };
        for piece in [
            "sqlite-wal",
            "sqlite",
            "kv.sync",
            "kv",
            "secrets",
            "shared.json",
            "files",
        ] {
            assert!(pieces.iter().any(|p| p == piece), "{piece} in {pieces:?}");
        }
        // Everything is at the publisher's names, nothing is left at the id's.
        assert_eq!(std::fs::read(&new.kv).unwrap(), b"kv");
        assert_eq!(std::fs::read(&new.sql).unwrap(), b"sql");
        assert_eq!(
            std::fs::read(new.sql.with_extension("sqlite-wal")).unwrap(),
            b"wal"
        );
        assert_eq!(
            std::fs::read(new.files.join("trips").join("rome.txt")).unwrap(),
            b"pasta"
        );
        assert!(krate_accountsync::toggles::app_toggle_path(&new.kv).exists());
        for gone in [&old.kv, &old.sql, &old.secrets, &old.shared, &old.files] {
            assert!(!gone.exists(), "{} left behind", gone.display());
        }
        // Its secrets open under the signed app's label, and not the id's.
        assert_eq!(
            secret(&new, &new.secret_label, &key).as_deref(),
            Some(&b"s3cret"[..])
        );
        assert_eq!(secret(&new, &old.secret_label, &key), None);

        // Once: opening again moves nothing.
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &key),
            Ok(Adopted::Settled)
        );
        // An unsigned app with this id now opens to a fresh, empty store,
        // and what it writes there stays its own: the signed app does not
        // take it on a later open, and nor does another publisher's app.
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", None, &key),
            Ok(Adopted::Unsigned)
        );
        assert!(!old.kv.exists());
        std::fs::write(&old.kv, b"impostor").unwrap();
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &key),
            Ok(Adopted::Settled)
        );
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(OTHER), &key),
            Ok(Adopted::Settled)
        );
        assert_eq!(std::fs::read(&old.kv).unwrap(), b"impostor");
        assert_eq!(std::fs::read(&new.kv).unwrap(), b"kv");
        assert!(!stores(root, "dev.krate.notes", Some(OTHER)).kv.exists());
    }

    #[test]
    fn an_unsigned_app_keeps_its_old_path_and_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let key = [7u8; 32];
        old_data(root, &key);
        let old = stores(root, "dev.krate.notes", None);
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", None, &key),
            Ok(Adopted::Unsigned)
        );
        assert_eq!(std::fs::read(&old.kv).unwrap(), b"kv");
        assert!(old.files.join("trips").join("rome.txt").exists());
        assert_eq!(
            secret(&old, "dev.krate.notes", &key).as_deref(),
            Some(&b"s3cret"[..])
        );
        assert!(!moved_record(root, "dev.krate.notes").exists());
    }

    /// The first signed app with an id settles it even when the id kept
    /// nothing: data an unsigned app writes under the id later is never
    /// pulled into the publisher's store.
    #[test]
    fn a_first_open_with_nothing_to_take_still_settles_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]),
            Ok(Adopted::Nothing)
        );
        let old = stores(root, "dev.krate.notes", None);
        std::fs::write(&old.kv, b"planted").unwrap();
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]),
            Ok(Adopted::Settled)
        );
        assert_eq!(std::fs::read(&old.kv).unwrap(), b"planted");
        assert!(!stores(root, "dev.krate.notes", Some(ROOT)).kv.exists());
    }

    /// A move cut short finishes on the same app's next open, and only on
    /// its: another publisher's app with the id never finishes it.
    #[test]
    fn a_move_cut_short_finishes_for_the_same_publisher_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        old_data(root, &[]);
        let old = stores(root, "dev.krate.notes", None);
        let new = stores(root, "dev.krate.notes", Some(ROOT));
        // As a crash leaves it: the record names the publisher, the
        // database is across, the rest is not.
        std::fs::write(moved_record(root, "dev.krate.notes"), format!("{ROOT}\n")).unwrap();
        std::fs::rename(&old.sql, &new.sql).unwrap();
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(OTHER), &[]),
            Ok(Adopted::Settled)
        );
        assert!(old.kv.exists());
        let finished = adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]).unwrap();
        assert!(matches!(&finished, Adopted::Moved(p) if !p.iter().any(|p| p == "sqlite")));
        assert!(!old.kv.exists() && new.kv.exists() && new.sql.exists());
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]),
            Ok(Adopted::Settled)
        );
    }

    /// The home's Storage and Clear reach a signed app's store, and Clear
    /// never forgets that the id was settled.
    #[test]
    fn storage_and_clear_find_a_signed_apps_store_and_keep_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        old_data(root, &[]);
        adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]).unwrap();
        let new = stores(root, "dev.krate.notes", Some(ROOT));
        // rome.txt, kv, sql, its journal, shared.json and the secrets.
        let secrets = std::fs::metadata(&new.secrets).unwrap().len();
        assert_eq!(new.size(), 5 + 2 + 3 + 3 + 2 + secrets);
        assert_eq!(stores(root, "dev.krate.notes", None).size(), 0);
        new.clear();
        assert_eq!(new.size(), 0);
        assert!(!new.files.exists() && !new.kv.exists() && !new.secrets.exists());
        assert!(moved_record(root, "dev.krate.notes").exists());
        // So an unsigned app's later data is not taken across after a clear.
        std::fs::write(stores(root, "dev.krate.notes", None).kv, b"x").unwrap();
        assert_eq!(
            adopt_id_keyed_data(root, "dev.krate.notes", Some(ROOT), &[]),
            Ok(Adopted::Settled)
        );
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
