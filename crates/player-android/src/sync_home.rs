//! Sync on Krate's home, for both phones (IC-981 S2): what `sync.txt` says
//! -- read from this phone's files and the hub's last answer, never waiting
//! on the network -- and what each sync answer does. The iPhone player
//! shares this file, so both phones behave the same.
//!
//! The hub is asked on a thread of its own ([`check_in_background`]); the
//! home screen reads `sync.txt` again while it says a look is under way.
//! Conflicts are read and answered on disk with the app closed, through
//! the runtime's `conflicts_at` / `resolve_at` (store.kv) and
//! `file_conflicts_at` / `resolve_file_at` (the app's own files, S3), the
//! same calls `krate account sync` makes on a computer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use krate_accountsync::overview::{self, Me, Overview, Reach};
use krate_accountsync::readable::{ago, preview, size_in_words};
use krate_accountsync::{toggles, Resolution};

use crate::home::{AppSync, Choice, ConflictCard, SyncAnswer, SyncState, SyncView};

/// How long a value on a conflict card may be, in characters.
const PREVIEW: usize = 140;
/// The hub is asked again when the home shows and its last answer is older.
const RECHECK: Duration = Duration::from_secs(60);

/// This phone, as sync needs it.
#[derive(Clone, Debug)]
pub struct Phone {
    /// Where apps' stores live (`stores/<id>.kv`).
    pub data_root: PathBuf,
    /// The account-sync folder: device keys, the key it holds, the toggle
    /// for every app, the hub's last answer.
    pub device_dir: PathBuf,
    /// The sign-in, when someone is signed in.
    pub token: Option<String>,
    pub machine_key: Vec<u8>,
    pub name: String,
    pub hub: String,
}

impl Phone {
    fn me(&self) -> Option<Me<'_>> {
        Some(Me {
            device_dir: &self.device_dir,
            hub: &self.hub,
            token: self.token.as_deref().filter(|t| !t.trim().is_empty())?,
            machine_key: &self.machine_key,
            name: &self.name,
        })
    }

    /// A run's context for this phone, the app left to the replica.
    fn context(&self) -> Option<krate_runtime::account_sync_host::AccountSyncContext> {
        Some(krate_runtime::account_sync_host::AccountSyncContext {
            hub: self.hub.clone(),
            token: self.token.clone().filter(|t| !t.trim().is_empty())?,
            device_dir: self.device_dir.clone(),
            device_name: self.name.clone(),
            machine_key: self.machine_key.clone(),
            app_id: String::new(),
            publisher: String::new(),
            enabled: true,
            background: false,
            files: None,
        })
    }

    fn all_on(&self) -> bool {
        !toggles::global_toggle_path(&self.device_dir).exists()
    }

    /// The app's store.kv: by its publisher when it is signed (K-1172), the
    /// same place its runs keep it.
    fn store(&self, app: &LibApp) -> PathBuf {
        crate::run_config::stores(&self.data_root, &app.id, app.publisher.as_deref()).kv
    }
}

/// One Library app, by its place on the home screen.
#[derive(Clone, Debug)]
pub struct LibApp {
    pub n: usize,
    pub id: String,
    /// The publisher its signature verifies to; None when unsigned. Its
    /// data is kept by it (K-1172).
    pub publisher: Option<String>,
    /// It asks for store.kv: it has data that syncs.
    pub uses_kv: bool,
    /// It writes files of its own: its folder syncs too (S3).
    pub uses_files: bool,
}

/// The Library apps as sync sees them, from their manifests and verified
/// publishers in the order the home screen numbers them.
pub fn lib_apps(
    manifests: &[Option<krate_manifest::Manifest>],
    publishers: &[Option<String>],
) -> Vec<LibApp> {
    manifests
        .iter()
        .enumerate()
        .filter_map(|(n, m)| {
            let m = m.as_ref()?;
            let uses_kv = m.declared_capabilities().is_ok_and(|caps| {
                caps.iter()
                    .any(|c| c.module() == "store" && c.action() == "kv")
            });
            Some(LibApp {
                n,
                id: m.app.id.clone(),
                publisher: publishers.get(n).cloned().flatten(),
                uses_kv,
                uses_files: crate::run_config::writes_files(m),
            })
        })
        .collect()
}

/// What each `conflict` line of the last `sync.txt` was about: the store,
/// the key (or the file's path), and whether it is a file, so an answer by
/// number is never misread.
pub type Asked = Vec<(PathBuf, String, bool)>;

/// One side of a file conflict in words: "2.4 MB, changed 5 minutes ago".
fn file_words(side: &krate_runtime::account_sync_files::FileSide, at: u64) -> String {
    match side.at {
        Some(then) => format!("{}, changed {}", size_in_words(side.size), ago(at, then)),
        None => size_in_words(side.size),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static CHECKING: AtomicBool = AtomicBool::new(false);
static CHECKED: Mutex<Option<Instant>> = Mutex::new(None);
/// How the last look this run went. The file keeps what the hub said, not
/// whether it said it this minute; before any look this run, the last
/// answer is shown as it was.
static REACH: Mutex<Option<Reach>> = Mutex::new(None);

/// Ask the hub now, on this thread, and remember how it went.
fn look_now(me: &Me<'_>) -> Overview {
    let seen = overview::look(me, now());
    if let Ok(mut reach) = REACH.lock() {
        *reach = Some(seen.reach.clone());
    }
    if let Ok(mut at) = CHECKED.lock() {
        *at = Some(Instant::now());
    }
    seen
}

/// What the home shows, from this phone's files and the hub's last answer.
pub fn view(phone: &Phone, apps: &[LibApp]) -> (SyncView, Asked) {
    let checking = CHECKING.load(Ordering::SeqCst);
    let at = now();
    let mut view = SyncView {
        state: SyncState::SignedOut,
        words: "Sign in to keep your apps the same on every device you use.".to_string(),
        checking: false,
        device: phone.name.clone(),
        last: String::new(),
        devices: Vec::new(),
        can_approve: false,
        apps: Vec::new(),
        conflicts: Vec::new(),
        kept: Vec::new(),
    };
    if phone.me().is_none() {
        return (view, Vec::new());
    }
    let seen = overview::cached(&phone.device_dir).map(|mut seen| {
        if let Some(reach) = REACH.lock().ok().and_then(|r| r.clone()) {
            seen.reach = reach;
        }
        seen
    });
    let account = seen.as_ref().and_then(|o| o.account.clone());
    let names = seen.as_ref().map(Overview::names).unwrap_or_default();

    // Each app's own switch and last sync; the conflicts on disk.
    let mut asked = Vec::new();
    let mut newest: Option<u64> = None;
    for app in apps {
        let store = phone.store(app);
        let state = if !app.uses_kv && !app.uses_files {
            AppSync::Nothing
        } else if toggles::app_toggle_path(&store).exists() {
            AppSync::Off
        } else {
            let last = account.as_deref().and_then(|a| {
                krate_runtime::account_sync_host::last_sync_at(&store, a).max(
                    krate_runtime::account_sync_files::files_last_sync_at(&store, a),
                )
            });
            newest = newest.max(last);
            AppSync::On(last.map(|t| ago(at, t)).unwrap_or_default())
        };
        view.apps.push((app.n, state));
        let Some(account) = account.as_deref().filter(|_| app.uses_kv || app.uses_files) else {
            continue;
        };
        let kept = krate_runtime::account_sync_files::files_kept_here_at(&store, account);
        if !kept.is_empty() {
            let names: Vec<&str> = kept
                .iter()
                .map(|p| p.rsplit('/').next().unwrap_or(p))
                .collect();
            let list = if names.len() > 3 {
                format!("{} and {} more", names[..3].join(", "), names.len() - 3)
            } else {
                names.join(", ")
            };
            view.kept.push((
                app.n,
                format!(
                    "{} Kept here: {list}.",
                    krate_accountsync::files::too_big_words().replace("this device", "this phone"),
                ),
            ));
        }
        for c in krate_runtime::account_sync_files::file_conflicts_at(&store, account, &names) {
            let (theirs, device) = c
                .others
                .first()
                .cloned()
                .unwrap_or((None, "another device".to_string()));
            view.conflicts.push(ConflictCard {
                app: app.n,
                key: c.path.clone(),
                mine: c.mine.as_ref().map(|s| file_words(s, at)),
                theirs: theirs.as_ref().map(|s| file_words(s, at)),
                device,
                more: c.others.len().saturating_sub(1),
                merge: c.mergeable,
                file: true,
            });
            asked.push((store.clone(), c.path, true));
        }
        for c in krate_runtime::account_sync_host::conflicts_at(&store, account, &names) {
            let (theirs, device) = c
                .others
                .first()
                .cloned()
                .unwrap_or((None, "another device".to_string()));
            view.conflicts.push(ConflictCard {
                app: app.n,
                key: c.key.clone(),
                mine: c.mine.as_deref().map(|v| preview(v, PREVIEW)),
                theirs: theirs.as_deref().map(|v| preview(v, PREVIEW)),
                device,
                more: c.others.len().saturating_sub(1),
                merge: c.mergeable,
                file: false,
            });
            asked.push((store.clone(), c.key, false));
        }
    }
    view.last = newest.map(|t| ago(at, t)).unwrap_or_default();
    if let Some(seen) = &seen {
        if !seen.device_name.is_empty() {
            view.device = seen.device_name.clone();
        }
        view.devices = seen
            .devices
            .iter()
            .map(|d| (d.id.clone(), d.name.clone(), d.holds))
            .collect();
        view.can_approve = seen.holds_key;
    }

    let (state, words) = if !phone.all_on() {
        (
            SyncState::Off,
            "Your apps keep their data on this phone only. Turn this on to have the same data on every device you sign in on.".to_string(),
        )
    } else {
        match &seen {
            None if checking => (
                SyncState::Checking,
                "Checking with your other devices\u{2026}".to_string(),
            ),
            None => (
                SyncState::Offline,
                "Krate can't reach the internet right now. Your apps keep working here, and sync when it's back.".to_string(),
            ),
            Some(seen) => match (&seen.reach, seen.holds_key) {
                (Reach::Failed(words), _) => (SyncState::Error, words.clone()),
                (_, false) => (
                    SyncState::Waiting,
                    format!(
                        "This phone needs a yes before it can read your apps' data. Open Krate on a device that already syncs and approve {} in Settings, Sync across your devices. No other device at hand? Use your recovery code.",
                        view.device
                    ),
                ),
                (Reach::Offline, true) => (
                    SyncState::Offline,
                    "Krate can't reach the internet right now. Your apps keep working here, and sync when it's back.".to_string(),
                ),
                (Reach::Answered, true) => (
                    SyncState::On,
                    "Your apps' data is the same on every device you sign in on, encrypted so only your devices can read it.".to_string(),
                ),
            },
        }
    };
    view.state = state;
    view.words = words;
    view.checking = checking;
    (view, asked)
}

/// Write `sync.txt` whole, so the screen never reads half of one.
pub fn write(ui: &Path, view: &SyncView) {
    let part = ui.join("sync.txt.part");
    if std::fs::write(&part, crate::home::sync_file(view)).is_ok() {
        let _ = std::fs::rename(&part, ui.join("sync.txt"));
    }
}

/// The recovery code to show once, if this phone made the account's key and
/// the person has not put it away. Handed to the screen as an argument,
/// never written to a file.
pub fn recovery_code(phone: &Phone) -> Option<String> {
    if !phone.all_on() {
        return None;
    }
    overview::recovery_code(&phone.me()?)
}

/// Ask the hub how this phone stands, on a thread of its own, unless a
/// look is under way or one answered lately (`force` asks anyway). When
/// it ends, `sync.txt` is written again for the screen to read.
pub fn check_in_background(phone: &Phone, apps: &[LibApp], ui: &Path, force: bool) {
    if phone.me().is_none() || !phone.all_on() {
        return;
    }
    let fresh = CHECKED
        .lock()
        .ok()
        .and_then(|at| *at)
        .is_some_and(|at| at.elapsed() < RECHECK);
    if (fresh && !force) || CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (phone, apps, ui) = (phone.clone(), apps.to_vec(), ui.to_path_buf());
    let spawned = std::thread::Builder::new()
        .name("krate-sync-look".into())
        .spawn(move || {
            if let Some(me) = phone.me() {
                look_now(&me);
            }
            CHECKING.store(false, Ordering::SeqCst);
            write(&ui, &view(&phone, &apps).0);
        });
    if spawned.is_err() {
        CHECKING.store(false, Ordering::SeqCst);
    }
}

/// What an answer did: a card to show first, if any, and where the home
/// opens next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acted {
    pub notice: Option<(String, String)>,
    pub start: String,
    /// Ask the hub again now.
    pub recheck: bool,
}

fn acted(start: &str) -> Acted {
    Acted {
        notice: None,
        start: start.to_string(),
        recheck: false,
    }
}

/// Do what the person chose. `asked` is what the last `sync.txt` listed.
pub fn act(phone: &Phone, apps: &[LibApp], asked: &Asked, answer: &SyncAnswer) -> Acted {
    let say = |start: &str, title: &str, words: &str| Acted {
        notice: Some((title.to_string(), words.to_string())),
        start: start.to_string(),
        recheck: false,
    };
    match answer {
        SyncAnswer::All(on) => match toggles::set_all_sync(&phone.device_dir, *on) {
            Ok(()) => Acted {
                recheck: *on,
                ..acted("sync")
            },
            Err(e) => say(
                "sync",
                "Could not change sync",
                &format!("This phone could not save the change ({e})."),
            ),
        },
        SyncAnswer::App(n, on) => {
            let Some(app) = apps.iter().find(|a| a.n == *n) else {
                return acted("library");
            };
            match toggles::set_app_sync(&phone.store(app), *on) {
                Ok(()) => acted(&format!("app:{n}")),
                Err(e) => say(
                    &format!("app:{n}"),
                    "Could not change sync",
                    &format!("This phone could not save the change ({e})."),
                ),
            }
        }
        SyncAnswer::Approve(device) => {
            let Some(me) = phone.me() else {
                return acted("you");
            };
            let name = overview::cached(&phone.device_dir)
                .and_then(|o| o.devices.into_iter().find(|d| d.id == *device))
                .map(|d| d.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "That device".to_string());
            match overview::approve(&me, device) {
                Ok(()) => {
                    look_now(&me);
                    say(
                        "sync",
                        "Done",
                        &format!("{name} can read your apps' data now. It catches up the next time it syncs."),
                    )
                }
                Err(words) => say("sync", &format!("{name} was not approved"), &words),
            }
        }
        SyncAnswer::Recover(code) => {
            let Some(me) = phone.me() else {
                return acted("you");
            };
            match overview::recover(&me, code) {
                Ok(()) => {
                    look_now(&me);
                    say(
                        "sync",
                        "This phone is in",
                        "It can read your apps' data now. Each app catches up the next time you open it.",
                    )
                }
                Err(words) => say("recover", "That did not work", &words),
            }
        }
        SyncAnswer::CodeKept => {
            if let Some(me) = phone.me() {
                overview::forget_recovery_code(&me);
            }
            acted("sync")
        }
        SyncAnswer::Resolve(c, choice) => {
            let Some((store, key, file)) = asked.get(*c) else {
                return acted("sync");
            };
            let Some(seen) = overview::cached(&phone.device_dir) else {
                return acted("sync");
            };
            let Some(account) = seen.account.clone() else {
                return acted("sync");
            };
            let resolution = match choice {
                Choice::This => Resolution::KeepMine,
                Choice::Other => Resolution::KeepOther(0),
                Choice::Both => Resolution::KeepBoth,
                Choice::Merge => Resolution::Merge,
            };
            let answered = if *file {
                krate_runtime::account_sync_files::resolve_file_at(
                    store,
                    &account,
                    &seen.names(),
                    key,
                    &resolution,
                )
            } else {
                krate_runtime::account_sync_host::resolve_at(
                    store,
                    &account,
                    &seen.names(),
                    key,
                    &resolution,
                )
            };
            match answered {
                Ok(()) => {
                    // Sent now, so the other devices settle without this
                    // app opening again -- unless the hub was out of reach
                    // a moment ago: then it goes with the app's next run,
                    // and the home does not sit waiting on the network.
                    let away = REACH
                        .lock()
                        .ok()
                        .and_then(|r| r.clone())
                        .is_some_and(|r| r == Reach::Offline);
                    if let Some(context) = phone.context().filter(|_| !away) {
                        let _ = if *file {
                            krate_runtime::account_sync_files::sync_files_closed(
                                store, &account, context,
                            )
                        } else {
                            krate_runtime::account_sync_host::sync_closed(store, &account, context)
                        };
                    }
                    acted("sync")
                }
                // Settled elsewhere meanwhile, or a merge that cannot work:
                // nothing changed, both are still kept.
                Err(why) => say(
                    "sync",
                    "Nothing changed",
                    &format!(
                        "{}. Both versions are still kept.",
                        capital(&why.to_string())
                    ),
                ),
            }
        }
        SyncAnswer::Check => Acted {
            recheck: true,
            ..acted("sync")
        },
    }
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Clearing an app's data on this phone: what sync remembered about it goes
/// too, so its next sync brings the account's copy back here rather than
/// sending the emptied store on as a delete on every device.
pub fn forget_after_clear(stores: &crate::run_config::Stores) {
    toggles::forget_replicas(&stores.kv);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(dir: &Path, token: Option<&str>) -> Phone {
        Phone {
            data_root: dir.to_path_buf(),
            device_dir: dir.join("account-sync"),
            token: token.map(str::to_string),
            machine_key: vec![4; 32],
            name: "Galaxy S22 Ultra".into(),
            hub: "http://127.0.0.1:9".into(),
        }
    }

    fn apps() -> Vec<LibApp> {
        vec![
            LibApp {
                n: 0,
                id: "dev.krate.reminders".into(),
                publisher: None,
                uses_kv: true,
                uses_files: false,
            },
            LibApp {
                n: 1,
                id: "dev.krate.clock".into(),
                publisher: None,
                uses_kv: false,
                uses_files: false,
            },
        ]
    }

    #[test]
    fn signed_out_says_sign_in_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let (view, asked) = view(&phone(dir.path(), None), &apps());
        assert_eq!(view.state, SyncState::SignedOut);
        assert!(view.words.contains("Sign in"));
        assert!(view.apps.is_empty() && asked.is_empty());
    }

    #[test]
    fn every_state_has_words_and_the_toggles_are_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = phone(dir.path(), Some("krs_x"));
        // Never answered, nothing under way: out of reach, and says so.
        let (v, _) = view(&p, &apps());
        assert_eq!(v.state, SyncState::Offline);
        assert!(v.words.contains("keep working"));
        assert_eq!(
            v.apps[1].1,
            AppSync::Nothing,
            "no store.kv, nothing to sync"
        );
        assert_eq!(v.apps[0].1, AppSync::On(String::new()));

        // The switch for every app.
        let acted = act(&p, &apps(), &Vec::new(), &SyncAnswer::All(false));
        assert_eq!(acted.start, "sync");
        assert_eq!(view(&p, &apps()).0.state, SyncState::Off);
        assert!(recovery_code(&p).is_none(), "off: no code shown");
        act(&p, &apps(), &Vec::new(), &SyncAnswer::All(true));
        assert_ne!(view(&p, &apps()).0.state, SyncState::Off);

        // One app's switch.
        let acted = act(&p, &apps(), &Vec::new(), &SyncAnswer::App(0, false));
        assert_eq!(acted.start, "app:0");
        assert_eq!(view(&p, &apps()).0.apps[0].1, AppSync::Off);
        act(&p, &apps(), &Vec::new(), &SyncAnswer::App(0, true));
        assert!(matches!(view(&p, &apps()).0.apps[0].1, AppSync::On(_)));
        assert_eq!(
            act(&p, &apps(), &Vec::new(), &SyncAnswer::App(9, true)).start,
            "library",
            "an app that is not there changes nothing"
        );

        // The hub's last answer: waiting for a yes, then on.
        let mut seen = Overview {
            account: Some("u1".into()),
            device_id: "aaaaaaaaaaaaaaaa".into(),
            device_name: "Galaxy S22 Ultra".into(),
            holds_key: false,
            devices: vec![krate_accountsync::overview::Known {
                id: "bbbbbbbbbbbbbbbb".into(),
                name: "Mac".into(),
                holds: true,
            }],
            checked_at: 1,
            reach: Reach::Answered,
        };
        let keep = |o: &Overview| {
            std::fs::create_dir_all(&p.device_dir).unwrap();
            std::fs::write(
                overview::cache_path(&p.device_dir),
                serde_json::to_string(o).unwrap(),
            )
            .unwrap();
        };
        keep(&seen);
        let (v, _) = view(&p, &apps());
        assert_eq!(v.state, SyncState::Waiting);
        assert!(v.words.contains("approve Galaxy S22 Ultra"), "{}", v.words);
        assert!(v.words.contains("recovery code"));
        assert!(!v.can_approve);
        seen.holds_key = true;
        keep(&seen);
        let (v, _) = view(&p, &apps());
        assert_eq!(v.state, SyncState::On);
        assert!(v.words.contains("only your devices can read it"));
        assert!(v.can_approve);
        assert_eq!(
            v.devices,
            vec![("bbbbbbbbbbbbbbbb".into(), "Mac".into(), true)]
        );

        // An answer about a conflict that is no longer listed does nothing.
        let acted = act(
            &p,
            &apps(),
            &Vec::new(),
            &SyncAnswer::Resolve(4, Choice::Both),
        );
        assert_eq!(acted, super::acted("sync"));
        // Nobody to ask: approving says so in words.
        let acted = act(
            &p,
            &apps(),
            &Vec::new(),
            &SyncAnswer::Approve("bbbbbbbbbbbbbbbb".into()),
        );
        let (title, words) = acted.notice.expect("a card");
        assert_eq!(title, "Mac was not approved");
        assert!(words.contains("can't reach"), "{words}");
    }

    #[test]
    fn an_app_has_data_to_sync_only_when_it_asks_for_store_kv() {
        let manifest = |id: &str, caps: &str| {
            krate_manifest::Manifest::parse(&format!(
                "[app]\nid = \"{id}\"\nname = \"N\"\nversion = \"1.0.0\"\n\
                 entry = \"app.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n{caps}"
            ))
            .ok()
        };
        let kv = "[[capabilities]]\ncap = \"store.kv\"\nrationale = \"x\"\nrequired = true\n";
        let root = "ab".repeat(32);
        let apps = lib_apps(
            &[manifest("dev.a", kv), None, manifest("dev.c", "")],
            &[None, None, Some(root.clone())],
        );
        assert_eq!(
            apps.len(),
            2,
            "an app whose manifest did not read is left out"
        );
        assert!(apps[0].uses_kv && apps[0].n == 0);
        assert!(
            !apps[1].uses_kv && apps[1].n == 2,
            "numbered as the home numbers it"
        );
        assert_eq!(apps[1].publisher.as_deref(), Some(root.as_str()));

        // Sync reads the store the app's runs use: a signed app's is keyed
        // by its publisher (K-1172), an unsigned app's by its id as before.
        let dir = tempfile::tempdir().unwrap();
        let p = phone(dir.path(), None);
        assert_eq!(
            p.store(&apps[1]),
            crate::run_config::stores(dir.path(), "dev.c", Some(&root)).kv
        );
        assert_eq!(
            p.store(&apps[1]),
            dir.path().join("stores").join("dev.c@abababababababab.kv")
        );
        assert_eq!(
            p.store(&apps[0]),
            dir.path().join("stores").join("dev.a.kv")
        );
    }

    /// A file changed here and on the Mac, as the files' sync leaves it on
    /// disk: the card says so in words, and an answer puts the file in place.
    #[test]
    fn a_file_changed_on_two_devices_is_a_card_with_sizes_and_its_answer_lands() {
        use krate_accountsync::files::FileValue;
        use krate_accountsync::{Replica, Version, VersionVector};
        let dir = tempfile::tempdir().unwrap();
        let p = phone(dir.path(), Some("krs_x"));
        let app = LibApp {
            n: 2,
            id: "dev.krate.journal".into(),
            publisher: None,
            uses_kv: false,
            uses_files: true,
        };
        let store = p.store(&app);
        let folder = dir.path().join("apps").join("dev.krate.journal");
        std::fs::create_dir_all(&folder).unwrap();
        let mine = vec![7u8; 3000];
        let theirs = vec![9u8; 2_500_000];
        std::fs::write(folder.join("photo.jpg"), &mine).unwrap();
        let (me, mac) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        let mut replica = Replica::new(me);
        replica.local_write("photo.jpg", Some(&FileValue::of(&mine).encode()));
        let mut vv = VersionVector::new();
        vv.bump(mac);
        replica.merge_remote(
            "photo.jpg",
            &[Version {
                value: Some(FileValue::of(&theirs).encode()),
                vv,
                by: mac.into(),
            }],
        );
        let theirs_hash = FileValue::of(&theirs).hash;
        let state = serde_json::json!({
            "replica": replica,
            "folder": folder,
            "bodies": { "photo.jpg": { &theirs_hash: { "blob": "c".repeat(64), "k": 1, "at": now() - 3600 } } },
            "kept_here": ["trips/film.mov"],
        });
        std::fs::create_dir_all(toggles::sync_dir(&store)).unwrap();
        std::fs::write(toggles::files_state_path(&store, "u1"), state.to_string()).unwrap();
        let held = toggles::held_dir(&store, "u1");
        std::fs::create_dir_all(&held).unwrap();
        std::fs::write(held.join(&theirs_hash), &theirs).unwrap();
        let seen = Overview {
            account: Some("u1".into()),
            device_id: me.into(),
            device_name: "Galaxy S22 Ultra".into(),
            holds_key: true,
            devices: vec![krate_accountsync::overview::Known {
                id: mac.into(),
                name: "Mac".into(),
                holds: true,
            }],
            checked_at: 1,
            reach: Reach::Answered,
        };
        std::fs::create_dir_all(&p.device_dir).unwrap();
        std::fs::write(
            overview::cache_path(&p.device_dir),
            serde_json::to_string(&seen).unwrap(),
        )
        .unwrap();

        let (v, asked) = view(&p, std::slice::from_ref(&app));
        assert!(matches!(v.apps[0].1, AppSync::On(_)), "files alone sync");
        assert_eq!(
            v.kept,
            vec![(
                2,
                "Files over 16 MB stay on this phone. Kept here: film.mov.".to_string()
            )]
        );
        assert_eq!(v.conflicts.len(), 1, "{:?}", v.conflicts);
        let card = &v.conflicts[0];
        assert!(card.file && card.key == "photo.jpg" && card.device == "Mac");
        assert!(card.mine.as_deref().unwrap().starts_with("3 KB, changed "));
        assert_eq!(card.theirs.as_deref(), Some("2.4 MB, changed 1 hour ago"));
        assert!(!card.merge, "a picture is not merged");
        assert_eq!(asked, vec![(store.clone(), "photo.jpg".to_string(), true)]);

        // Keep both: the Mac's copy lands beside it under the Mac's name.
        let acted = act(
            &p,
            std::slice::from_ref(&app),
            &asked,
            &SyncAnswer::Resolve(0, Choice::Both),
        );
        assert_eq!(acted, super::acted("sync"));
        assert_eq!(std::fs::read(folder.join("photo.jpg")).unwrap(), mine);
        assert_eq!(
            std::fs::read(folder.join("photo (Mac).jpg")).unwrap(),
            theirs
        );
        assert!(view(&p, &[app]).0.conflicts.is_empty(), "answered");
    }

    #[test]
    fn clearing_an_apps_data_forgets_only_what_sync_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let stores = crate::run_config::stores(dir.path(), "dev.krate.reminders", None);
        let store = stores.kv.clone();
        toggles::set_app_sync(&store, false).unwrap();
        let replica = toggles::replica_path(&store, "u1");
        std::fs::write(&replica, b"{}").unwrap();
        forget_after_clear(&stores);
        assert!(!replica.exists());
        assert!(
            toggles::app_toggle_path(&store).exists(),
            "the switch stays"
        );
    }
}
