//! The Krate player for Android -- M1 of the mobile plan.
//!
//! One job: open a .krate, show the wall, run the app. Phones receive;
//! desktops make. Three ways in, one flow:
//!
//! - a tapped krate.tech/a/ link (VIEW intent, https) -- fetched from the
//!   hub over TLS;
//! - a tapped .krate file (VIEW intent, content:// or file://) -- read
//!   through the ContentResolver;
//! - a plain launch -- Krate's home screen (apps/krate-home): Library, the
//!   apps kept on this phone, and Discover, the apps on the hub.
//!
//! Every app opened is kept in the library, and when it ends Krate goes
//! back to its home screen; leaving the home screen ends Krate.
//!
//! Whatever the source, the app never runs before the wall: a trusted
//! sheet (itself a Krate guest whose pixels come from the same renderer)
//! shows every capability line with the app's own rationale, required
//! rows locked on, the rest togglable. The player passes the lines in and
//! reads one decision line back from captured stdout; both sides of that
//! pipe belong to the player, so the sheet cannot be bypassed and cannot
//! be lied to. Deny still opens: whatever was toggled off is simply not
//! in the session policy, and the runtime enforces the difference.

pub mod home;
#[cfg(target_os = "android")]
mod intent;
pub mod run_config;
// Sync on Krate's home (IC-981 S2), shared with the iPhone player.
#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod sync_home;

// The hub's catalogue, screenshots and downloads: the iPhone player's own
// code, included rather than copied, so both phones show one Discover.
#[cfg(target_os = "android")]
#[path = "../../player-ios/src/hub.rs"]
mod hub;

// Signing in to Krate, and the account the profile tab shows.
#[cfg(target_os = "android")]
mod account;

#[cfg(target_os = "android")]
mod player {
    use crate::home::{HomeAnswer, Indexed};
    use crate::run_config::{output_args, prints_only, use_in_words};
    use krate_adapter_android::winit_native::AndroidApp;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    const MAX_BUNDLE_BYTES: usize = crate::run_config::MAX_BUNDLE_BYTES as usize;

    // Krate's home screen, a trusted guest: Library and Discover. Build
    // apps/krate-home (krate check-app) before the player.
    const HOME_WASM: &[u8] =
        include_bytes!("../../../apps/krate-home/target/wasm32-wasip1/release/krate_home.wasm");
    const HOME_MANIFEST: &str = include_str!("../../../apps/krate-home/manifest.toml");

    // The wall sheet, a trusted guest. Build apps/krate-wall before the
    // player.
    const WALL_WASM: &[u8] =
        include_bytes!("../../../apps/krate-wall/target/wasm32-wasip1/release/krate_wall.wasm");
    const WALL_MANIFEST: &str = include_str!("../../../apps/krate-wall/manifest.toml");

    #[no_mangle]
    fn android_main(app: AndroidApp) {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );

        krate_adapter_android::winit_native::set_android_app(app.clone());
        let data_root = app
            .internal_data_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let handles = crate::intent::ActivityHandles {
            vm: app.vm_as_ptr(),
            activity: app.activity_as_ptr(),
        };
        // An app's temporary space is the activity's cache: Rust's
        // temp_dir() is /data/local/tmp on Android, which no app may write,
        // and a save on a phone is written there before it is placed
        // (K-1032). Picks and saves a run left behind go at the next start.
        let cache = crate::intent::cache_dir(handles).unwrap_or_else(|| data_root.join("tmp"));
        for left in ["krate-picked", "krate-saves", "krate-share"] {
            let _ = std::fs::remove_dir_all(cache.join(left));
        }
        let _ = std::fs::create_dir_all(&cache);
        std::env::set_var("TMPDIR", &cache);
        // Measuring on a phone, where there is no shell to set a variable: a
        // file named .frame-stats in the Player's own folder (adb can put one
        // there) turns on the runtime's frame timings in logcat.
        // A number in the file is how many frames each line sums.
        // The file being there is the switch: one adb writes may not be
        // readable by the Player.
        if let Some(stats) = app
            .external_data_path()
            .map(|dir| dir.join(".frame-stats"))
            .filter(|file| file.exists())
        {
            std::env::set_var("KRATE_FRAME_STATS", "1");
            if let Some(every) = std::fs::read_to_string(stats)
                .ok()
                .and_then(|text| text.trim().parse::<usize>().ok())
            {
                std::env::set_var("KRATE_FRAME_STATS_EVERY", every.to_string());
            }
        }
        run_player(&data_root, handles);
        // Krate's home screen closed: no blank activity shell may linger.
        // Finish the activity first: Android restarts a top activity whose
        // process died moments after launch, with the same link, so a file
        // that could not be read flickered open and shut forever (K-1015).
        crate::intent::finish(handles);
        std::process::exit(0);
    }

    fn run_player(data_root: &Path, handles: crate::intent::ActivityHandles) {
        // The picture the permission sheet sits over is the app the person
        // just restarted from Krate's menu, written moments ago; any older
        // one belongs to another visit, and the sheet sits on its wash.
        let behind = data_root.join("wall").join("behind.rgba");
        let fresh = std::fs::metadata(&behind)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(20));
        if !fresh {
            let _ = std::fs::remove_file(&behind);
        }
        // The library: every app opened on this phone, one .krate per app.
        // It is the folder the copies kept for reminders always lived in, so
        // an app opened before the home screen existed is listed, and a
        // reminder set then still opens.
        let library = data_root.join("kept");
        let _ = std::fs::create_dir_all(&library);
        loop {
            // A link or file tapped -- at launch, or while Krate's own
            // screen showed -- comes first.
            let opened = crate::intent::take_open(handles);
            log::info!("launch target: {opened:?}");
            let app = match opened {
                // A sign-in coming back to a Krate that is no longer
                // running: the app that asked is gone, so there is nothing
                // to hand it to.
                Some(uri) if uri.starts_with("krate-signin:") => {
                    log::info!("a sign-in came back after its app closed; nothing to do");
                    None
                }
                Some(uri) => receive(&uri, &library, data_root, handles),
                None => None,
            };
            let app = match app {
                Some(app) => app,
                None => match home(&library, data_root) {
                    Some(app) => app,
                    // The home screen closed for a link or file tapped while
                    // it showed: go round and open that.
                    None if crate::intent::open_waiting(handles) => continue,
                    None => return,
                },
            };
            launch(&app, data_root);
        }
    }

    /// Fetch or read what was tapped and keep it in the library; None, after
    /// telling the person why, when it is not an app that opens.
    fn receive(
        uri: &str,
        library: &Path,
        data_root: &Path,
        handles: crate::intent::ActivityHandles,
    ) -> Option<PathBuf> {
        let bytes = if uri.starts_with("https://") || uri.starts_with("http://") {
            log::info!("fetching {uri}");
            // With the sign-in when the link is on Krate's hub, so an
            // organisation's private app downloads for its members (K-1171).
            let phone = crate::run_config::Phone::here(data_root, &phone_name());
            match krate_orgaccess::download(&phone, uri, MAX_BUNDLE_BYTES as u64, USER_AGENT) {
                Ok(bytes) => bytes,
                Err(krate_orgaccess::GateError::Refused(refusal)) => {
                    log::info!("the hub refused the app: {}", refusal.reason);
                    notice(
                        data_root,
                        "This app did not open",
                        &capitalised(&refusal.words),
                    );
                    return None;
                }
                Err(why) => {
                    log::error!("could not fetch the app: {why}");
                    let why = format!(
                        "{}. Check the connection and open the link again.",
                        capitalised(&why.to_string())
                    );
                    notice(data_root, "Could not download this app", &why);
                    return None;
                }
            }
        } else {
            log::info!("opening {uri}");
            match crate::intent::read_uri(handles, uri, MAX_BUNDLE_BYTES) {
                Ok(bytes) => bytes,
                Err(why) => {
                    log::error!("could not read the file: {why}");
                    notice(data_root, "Could not open this file", &why);
                    return None;
                }
            }
        };
        keep(&bytes, library, data_root)
    }

    /// Keep an app's bytes in the library, named for its app, so the home
    /// screen lists it, a newer copy replaces an older one, and a reminder
    /// tapped later opens it again (IC-921): a link may be offline by then,
    /// and a file's grant from the app that sent it does not last.
    fn keep(bytes: &[u8], library: &Path, data_root: &Path) -> Option<PathBuf> {
        let open = match krate_bundle::open_reader(Cursor::new(bytes)) {
            Ok(open) => open,
            Err(err) => {
                log::error!("that is not a Krate app: {err:#}");
                // A Krate too old for the app says so in words a person
                // can act on; anything else is a damaged or wrong file.
                let why = match err {
                    krate_bundle::BundleError::NeedsNewerKrate { .. } => format!("{err}"),
                    _ => "This file is not a Krate app, or it was damaged on the way. Ask for it again.".to_string(),
                };
                notice(data_root, "This app did not open", &why);
                return None;
            }
        };
        let manifest = std::fs::read_to_string(open.manifest_path()).unwrap_or_default();
        let Ok(manifest) = krate_manifest::Manifest::parse(&manifest) else {
            notice(
                data_root,
                "This app did not open",
                "Its description could not be read.",
            );
            return None;
        };
        let file = crate::home::library_file(&manifest.app.id);
        let into = library.join(&file);
        if let Err(err) = std::fs::write(&into, bytes) {
            notice(
                data_root,
                "This app did not open",
                &format!("Could not keep the app: {err}"),
            );
            return None;
        }
        remember(
            library,
            &file,
            &Indexed {
                name: manifest.app.name,
                sha: krate_bundle::sha256_hex(bytes),
                size: bytes.len() as u64,
            },
        );
        Some(into)
    }

    fn remember(library: &Path, file: &str, kept: &Indexed) {
        let index = library.join("index.tsv");
        let text = std::fs::read_to_string(&index).unwrap_or_default();
        let _ = std::fs::write(index, crate::home::index_with(&text, file, kept));
    }

    /// The library's apps, newest first: (file, what the index knows). Apps
    /// kept before the index existed are read once and added to it.
    fn library_apps(library: &Path) -> Vec<(PathBuf, Indexed)> {
        let mut known = crate::home::read_index(
            &std::fs::read_to_string(library.join("index.tsv")).unwrap_or_default(),
        );
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(library)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "krate"))
            .map(|path| {
                let at = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                (at, path)
            })
            .collect();
        files.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let mut out = Vec::new();
        for (_, path) in files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if known.get(&name).is_none_or(|k| k.size != size) {
                let app_name = krate_bundle::open(&path)
                    .ok()
                    .and_then(|open| std::fs::read_to_string(open.manifest_path()).ok())
                    .and_then(|text| krate_manifest::Manifest::parse(&text).ok())
                    .map(|manifest| manifest.app.name);
                let Some(app_name) = app_name else {
                    continue;
                };
                let sha = std::fs::read(&path)
                    .map(|b| krate_bundle::sha256_hex(&b))
                    .unwrap_or_default();
                let kept = Indexed {
                    name: app_name,
                    sha,
                    size,
                };
                remember(library, &name, &kept);
                known.insert(name.clone(), kept);
            }
            if let Some(kept) = known.get(&name).cloned() {
                out.push((path, kept));
            }
        }
        out
    }

    /// Krate's own screens: Library and Discover (apps/krate-home). The
    /// player prepares what they show -- the library, the hub catalogue,
    /// decoded screenshots -- and acts on the answer: open an app, get one
    /// from the hub, or pick a `.krate` in the phone's files. None when the
    /// person leaves.
    /// The app id a bundle's manifest declares, read from its bytes.
    fn bundle_app_id(bytes: &[u8]) -> Option<String> {
        let open = krate_bundle::open_reader(Cursor::new(bytes)).ok()?;
        let text = std::fs::read_to_string(open.manifest_path()).ok()?;
        Some(krate_manifest::Manifest::parse(&text).ok()?.app.id)
    }

    /// A kept app's manifest, read from its bundle.
    fn manifest_of(path: &Path) -> Option<krate_manifest::Manifest> {
        let open = krate_bundle::open(path).ok()?;
        let text = std::fs::read_to_string(open.manifest_path()).ok()?;
        krate_manifest::Manifest::parse(&text).ok()
    }

    /// The account the profile tab shows, as last fetched: refreshed from
    /// the hub off the main thread, so the home screen never waits on it.
    static ACCOUNT: std::sync::Mutex<Option<crate::home::Account>> = std::sync::Mutex::new(None);

    fn refresh_account(data_root: &Path, art_dir: &Path) {
        let found = crate::account::account(data_root, art_dir);
        *ACCOUNT.lock().expect("account lock") = found;
    }

    /// Everything a phone keeps for Krate's apps, for the profile tab.
    fn storage_used(data_root: &Path) -> u64 {
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
        ["kept", "apps", "stores"]
            .iter()
            .map(|d| walk(&data_root.join(d)))
            .sum()
    }

    /// What an app keeps on this phone: its files and its stores, where its
    /// runs keep them -- by its publisher when it is signed (K-1172).
    fn app_data_size(data_root: &Path, app_id: &str, publisher: Option<&str>) -> u64 {
        crate::run_config::stores(data_root, app_id, publisher).size()
    }

    /// Forget what an app keeps on this phone: its files and its stores.
    fn clear_app_data(data_root: &Path, app_id: &str, publisher: Option<&str>) {
        let stores = crate::run_config::stores(data_root, app_id, publisher);
        stores.clear();
        crate::sync_home::forget_after_clear(&stores);
    }

    /// This phone, as sync on the home screen needs it (IC-981 S2): the same
    /// folder, name and key a run's sync uses.
    fn sync_phone(data_root: &Path) -> crate::sync_home::Phone {
        crate::sync_home::Phone {
            data_root: data_root.to_path_buf(),
            device_dir: data_root.join("account-sync"),
            token: crate::account::token(data_root),
            machine_key: machine_key(data_root),
            name: phone_name(),
            hub: crate::run_config::SHARED_HUB.to_string(),
        }
    }

    /// Sign in to Krate without leaving it and without a code: krate.tech's
    /// sign-in page (GitHub, Google or email) opens in the phone's in-app
    /// sign-in tab over Krate, and the moment the person approves, the page
    /// hands the sign-in back as krate://signed-in, the tab closes itself and
    /// Krate is signed in. The hand-back counts only with the nonce this
    /// phone sent (K-909). Closing the page is a cancel, and says nothing.
    fn sign_in(data_root: &Path, art_dir: &Path) {
        let Some(nonce) = crate::account::new_nonce() else {
            notice(
                data_root,
                "Could not start signing in",
                "This phone gave no randomness to sign in with.",
            );
            return;
        };
        let url = crate::account::web_sign_in_url(&nonce);
        match krate_adapter_android::sign_in::sign_in(&url, "krate") {
            Ok(Some(back)) => match crate::account::keep_web_sign_in(&back, &nonce, data_root) {
                Ok(()) => refresh_account(data_root, art_dir),
                Err(why) => notice(data_root, "Not signed in", &why),
            },
            Ok(None) => {}
            Err(why) => notice(
                data_root,
                "Could not start signing in",
                &format!("{why}. Check the connection and try again."),
            ),
        }
    }

    /// Discover's rows from the catalogue as it stands.
    fn write_discover(ui: &Path, art_dir: &Path) {
        let rows: Vec<String> = crate::hub::catalogue(ui)
            .iter()
            .map(|app| {
                crate::home::discover_row(
                    &app.id,
                    &app.name,
                    &app.author,
                    &app.category,
                    &app.description,
                    crate::hub::cached_art(app, art_dir).as_deref(),
                    app.size,
                )
            })
            .collect();
        let part = ui.join("discover.txt.part");
        if std::fs::write(&part, rows.join("\n") + "\n").is_ok() {
            let _ = std::fs::rename(&part, ui.join("discover.txt"));
        }
    }

    /// Fetch the hub's catalogue off the main thread, telling Discover how
    /// it stands (hub-state.txt): loading, then ready, or offline when there
    /// is nothing to show.
    fn refresh_hub(ui: &Path, art_dir: &Path) {
        let state = ui.join("hub-state.txt");
        if crate::hub::catalogue(ui).is_empty() {
            let _ = std::fs::write(&state, "loading\n");
        }
        let (ui, art_dir) = (ui.to_path_buf(), art_dir.to_path_buf());
        std::thread::spawn(move || {
            let at = std::time::Instant::now();
            crate::hub::refresh(&ui, &art_dir);
            log::info!(
                "hub catalogue fetched in {:.1} s",
                at.elapsed().as_secs_f32()
            );
            // discover.txt first: the screen reloads it when the state turns.
            write_discover(&ui, &art_dir);
            let now = if crate::hub::catalogue(&ui).is_empty() {
                "offline"
            } else {
                "ready"
            };
            let _ = std::fs::write(ui.join("hub-state.txt"), format!("{now}\n"));
        });
    }

    fn home(library: &Path, data_root: &Path) -> Option<PathBuf> {
        let ui = data_root.join("home-ui");
        let art_dir = ui.join("art");
        let _ = std::fs::create_dir_all(&art_dir);
        let mut start: Option<String> = None;
        // The hub's catalogue as last fetched, refreshed in the background.
        // Nothing waits on the network: with no catalogue yet, Discover shows
        // its loading rows (hub-state.txt) and fills in when it arrives -- the
        // first open sat on a blank screen for 5.6 s.
        static REFRESHED: std::sync::Once = std::sync::Once::new();
        REFRESHED.call_once(|| refresh_hub(&ui, &art_dir));
        if crate::account::token(data_root).is_some() {
            static ACCOUNT_REFRESHED: std::sync::Once = std::sync::Once::new();
            ACCOUNT_REFRESHED.call_once(|| {
                let (data_root, art_dir) = (data_root.to_path_buf(), art_dir.clone());
                std::thread::spawn(move || refresh_account(&data_root, &art_dir));
            });
        }
        loop {
            let hub_apps = crate::hub::catalogue(&ui);
            let hub_art: std::collections::HashMap<String, String> = hub_apps
                .iter()
                .filter_map(|app| Some((app.id.clone(), crate::hub::cached_art(app, &art_dir)?)))
                .collect();

            let kept_apps = library_apps(library);
            let mut apps: Vec<PathBuf> = kept_apps.iter().map(|(path, _)| path.clone()).collect();
            // Each app's manifest and the publisher its signature verifies
            // to: a signed app's data is kept by its publisher (K-1172).
            let (manifests, publishers): (Vec<Option<krate_manifest::Manifest>>, Vec<_>) = apps
                .iter()
                .map(|path| match crate::run_config::kept_app(path) {
                    Some((manifest, publisher)) => (Some(manifest), publisher),
                    None => (None, None),
                })
                .unzip();
            let rows: Vec<String> = kept_apps
                .iter()
                .zip(manifests.iter().zip(&publishers))
                .map(|((_, kept), (manifest, publisher))| {
                    // An app from the hub wears its hub picture: the hub
                    // names apps by the SHA-256 of their bytes.
                    let art = hub_art.get(&kept.sha).map(String::as_str);
                    let from_hub = hub_apps.iter().any(|app| app.id == kept.sha);
                    let data = manifest
                        .as_ref()
                        .map(|m| app_data_size(data_root, &m.app.id, publisher.as_deref()))
                        .unwrap_or(0);
                    let version = manifest.as_ref().map(|m| m.app.version.as_str());
                    crate::home::library_row_full(
                        &kept.name, kept.size, art, from_hub, data, version,
                    )
                })
                .collect();
            let _ = std::fs::write(ui.join("library.txt"), rows.join("\n") + "\n");
            // What each Library app can use, for its detail screen.
            let perms = ui.join("perms");
            let _ = std::fs::remove_dir_all(&perms);
            let _ = std::fs::create_dir_all(&perms);
            for (n, manifest) in manifests.iter().enumerate() {
                let uses: Vec<(String, bool)> = manifest
                    .iter()
                    .flat_map(|m| m.capabilities.iter())
                    .filter_map(|request| Some((use_in_words(&request.cap)?, request.required)))
                    .collect();
                let _ = std::fs::write(
                    perms.join(format!("{n}.txt")),
                    crate::home::perms_file(&uses),
                );
            }
            // Sync across the person's devices: what sync.txt says, from
            // this phone's files; the hub asked on a thread of its own.
            let phone = sync_phone(data_root);
            let sync_apps = crate::sync_home::lib_apps(&manifests, &publishers);
            let (sync_view, asked) = crate::sync_home::view(&phone, &sync_apps);
            crate::sync_home::write(&ui, &sync_view);
            crate::sync_home::check_in_background(&phone, &sync_apps, &ui, false);
            let args = crate::sync_home::recovery_code(&phone)
                .map(|code| vec!["sync-code".to_string(), code])
                .unwrap_or_default();
            // Who is signed in, for the profile tab.
            let signed_in = crate::account::token(data_root).is_some();
            let account = if signed_in {
                ACCOUNT.lock().expect("account lock").clone().or_else(|| {
                    Some(crate::home::Account {
                        login: "you".to_string(),
                        ..Default::default()
                    })
                })
            } else {
                None
            };
            let _ = std::fs::write(
                ui.join("account.txt"),
                crate::home::account_file(
                    account.as_ref(),
                    &krate_adapter_android::phone::device_name(),
                    storage_used(data_root),
                    env!("CARGO_PKG_VERSION"),
                ),
            );
            write_discover(&ui, &art_dir);
            match start.take() {
                Some(tab) => {
                    let _ = std::fs::write(ui.join("start.txt"), tab);
                }
                None => {
                    let _ = std::fs::remove_file(ui.join("start.txt"));
                }
            }

            let answer = run_trusted(HOME_WASM, HOME_MANIFEST, &ui, args)?;
            match crate::home::home_answer(&answer) {
                HomeAnswer::Open(n) => {
                    return (n < apps.len()).then(|| apps.swap_remove(n));
                }
                HomeAnswer::Get(id) => {
                    log::info!("getting {id} from the hub");
                    match crate::hub::download(
                        &id,
                        MAX_BUNDLE_BYTES as u64,
                        data_root,
                        &phone_name(),
                    ) {
                        Ok(bytes) => {
                            if let Some(app) = keep(&bytes, library, data_root) {
                                return Some(app);
                            }
                        }
                        Err(why) => notice(
                            data_root,
                            "Could not download this app",
                            &format!("{why}. Check the connection and try again."),
                        ),
                    }
                    start = Some("discover".to_string());
                }
                HomeAnswer::Files => {
                    // The phone's own document picker, the one an app's
                    // file dialog uses: the pick is copied into Krate.
                    if let Some(path) = krate_adapter_android::documents::choose(false, "krate") {
                        let read = std::fs::metadata(&path)
                            .map(|m| m.len() <= MAX_BUNDLE_BYTES as u64)
                            .unwrap_or(false)
                            .then(|| std::fs::read(&path).ok())
                            .flatten();
                        let _ = std::fs::remove_file(&path);
                        match read {
                            Some(bytes) => {
                                if let Some(app) = keep(&bytes, library, data_root) {
                                    return Some(app);
                                }
                            }
                            None => notice(
                                data_root,
                                "Could not open this file",
                                "It is larger than any Krate app, or it could not be read.",
                            ),
                        }
                    }
                }
                HomeAnswer::SignIn => {
                    sign_in(data_root, &art_dir);
                    start = Some("you".to_string());
                }
                HomeAnswer::SignOut => {
                    crate::account::sign_out(data_root);
                    *ACCOUNT.lock().expect("account lock") = None;
                    let _ = std::fs::remove_file(art_dir.join("avatar.rgba"));
                    start = Some("you".to_string());
                }
                HomeAnswer::Remove(n) => {
                    if let Some(path) = apps.get(n) {
                        if let Some(Some(manifest)) = manifests.get(n) {
                            let publisher = publishers.get(n).cloned().flatten();
                            clear_app_data(data_root, &manifest.app.id, publisher.as_deref());
                        }
                        let _ = std::fs::remove_file(path);
                    }
                }
                HomeAnswer::Clear(n) => {
                    if let Some(Some(manifest)) = manifests.get(n) {
                        let publisher = publishers.get(n).cloned().flatten();
                        clear_app_data(data_root, &manifest.app.id, publisher.as_deref());
                    }
                    start = Some(format!("app:{n}"));
                }
                HomeAnswer::Share(n) => {
                    if let (Some(path), Some(Some(manifest))) = (apps.get(n), manifests.get(n)) {
                        if let Ok(bytes) = std::fs::read(path) {
                            let name =
                                format!("{}.krate", manifest.app.name.replace(['/', '\\'], " "));
                            let _ =
                                krate_adapter_android::share::share(None, None, &[(name, bytes)]);
                        }
                    }
                    start = Some(format!("app:{n}"));
                }
                HomeAnswer::Update(n) => {
                    let current = manifests.get(n).and_then(|m| m.clone());
                    let sha = kept_apps
                        .get(n)
                        .map(|(_, kept)| kept.sha.clone())
                        .unwrap_or_default();
                    // The hub's list as it is now: the screen may have opened
                    // while it was still loading, and "Up to date" from an
                    // empty list was wrong.
                    let catalogue = crate::hub::catalogue(&ui);
                    match current {
                        _ if catalogue.is_empty() => notice(
                            data_root,
                            "Could not check for updates",
                            "The hub's list has not loaded. Check the connection and try again.",
                        ),
                        None => notice(
                            data_root,
                            "Could not check for updates",
                            "This app's description could not be read.",
                        ),
                        Some(manifest) => {
                            // A name is only a lead: the hub's copy replaces
                            // this one only when it is the same app (its own
                            // id), never a stranger's app that shares a name.
                            let mut found = None;
                            let mut failed = None;
                            for app in catalogue
                                .iter()
                                .filter(|app| {
                                    app.name.eq_ignore_ascii_case(&manifest.app.name)
                                        && app.id != sha
                                })
                                .take(3)
                            {
                                match crate::hub::download(
                                    &app.id,
                                    MAX_BUNDLE_BYTES as u64,
                                    data_root,
                                    &phone_name(),
                                ) {
                                    Ok(bytes)
                                        if bundle_app_id(&bytes).as_deref()
                                            == Some(manifest.app.id.as_str()) =>
                                    {
                                        found = Some(bytes);
                                        break;
                                    }
                                    Ok(_) => {}
                                    Err(why) => failed = Some(why),
                                }
                            }
                            match (found, failed) {
                                (Some(bytes), _) => {
                                    let _ = keep(&bytes, library, data_root);
                                    notice(
                                        data_root,
                                        "Updated",
                                        "This app is now the newest version on the hub.",
                                    );
                                }
                                (None, Some(why)) => notice(
                                    data_root,
                                    "Could not check for updates",
                                    &format!("{why}. Check the connection and try again."),
                                ),
                                (None, None) => notice(
                                    data_root,
                                    "Up to date",
                                    "This is the newest version on the hub.",
                                ),
                            }
                            // Back on its page: a kept copy moves to the top of
                            // the Library, so find it there by its own id.
                            let now = library_apps(library)
                                .iter()
                                .position(|(path, _)| {
                                    manifest_of(path).is_some_and(|m| m.app.id == manifest.app.id)
                                })
                                .unwrap_or(n);
                            start = Some(format!("app:{now}"));
                        }
                    }
                    if start.is_none() {
                        start = Some(format!("app:{n}"));
                    }
                }
                HomeAnswer::Refresh => {
                    refresh_hub(&ui, &art_dir);
                    start = Some("discover".to_string());
                }
                HomeAnswer::Report => {
                    let link = format!(
                        "mailto:yashraj@krate.tech?subject=A%20problem%20with%20Krate%20on%20Android&body=Krate%20{}%0A%0AWhat%20happened%3A%0A",
                        env!("CARGO_PKG_VERSION")
                    );
                    let _ = krate_adapter_android::phone::open_url(&link);
                    start = Some("settings".to_string());
                }
                HomeAnswer::Sync(answer) => {
                    let acted = crate::sync_home::act(&phone, &sync_apps, &asked, &answer);
                    if let Some((title, words)) = &acted.notice {
                        notice(data_root, title, words);
                    }
                    if acted.recheck {
                        crate::sync_home::check_in_background(&phone, &sync_apps, &ui, true);
                    }
                    start = Some(acted.start);
                }
                HomeAnswer::Stay => {}
                HomeAnswer::Closed => return None,
            }
        }
    }

    /// Ask on the wall, then run one app kept in the library. Every open
    /// asks: the answer is never remembered.
    fn launch(path: &Path, data_root: &Path) {
        // The extracted bundle dir is removed on drop, and the runtime
        // reads assets from it during the run -- it must live to the end.
        let opened = match krate_bundle::open(path) {
            Ok(open) => open,
            Err(err) => {
                log::error!("a kept app did not open: {err:#}");
                return notice(data_root, "This app did not open", &format!("{err}"));
            }
        };
        let manifest_text = std::fs::read_to_string(opened.manifest_path()).unwrap_or_default();
        let component = match std::fs::read(opened.component_path()) {
            Ok(bytes) => bytes,
            Err(err) => {
                log::error!("bundle component unreadable: {err}");
                return;
            }
        };
        let assets_root = opened.assets_path().map(Path::to_path_buf);

        let manifest = match krate_manifest::Manifest::parse(&manifest_text) {
            Ok(manifest) => manifest,
            Err(err) => {
                log::error!("manifest failed to parse: {err:#}");
                return;
            }
        };

        // The wall, before anything else happens.
        let granted = match show_wall(&manifest, data_root) {
            Some(granted) => granted,
            None => {
                log::info!("the person said no; nothing runs");
                return;
            }
        };

        // The policy is exactly what survived the wall. declared_capabilities
        // parses the same request list the wall displayed, in the same
        // order, so filtering by the displayed cap string cannot drift.
        let declared = match manifest.declared_capabilities() {
            Ok(declared) => declared,
            Err(err) => {
                log::error!("capabilities failed to parse: {err:#}");
                return;
            }
        };
        let kept: Vec<_> = manifest
            .capabilities
            .iter()
            .zip(declared)
            .filter(|(request, _)| granted.iter().any(|g| g == &request.cap))
            .map(|(_, capability)| capability)
            .collect();
        let policy = krate_policy::SessionPolicy::from_grants(kept);
        // More memory than the default, when the app asked and the person
        // left it on (K-416).
        let memory_bytes = crate::run_config::memory_budget_bytes(&manifest, &policy);

        // Where its data lives: a signed app's by its id and its verified
        // publisher, so another app declaring the same id reaches a
        // different place (K-1172, the desktop's IC-736); an unsigned app's
        // by its id, as always. A signed app opening for the first time
        // takes what its id kept here before across, once.
        let verified = crate::run_config::verified_publisher(&opened);
        match crate::run_config::adopt_id_keyed_data(
            data_root,
            &manifest.app.id,
            verified.as_deref(),
            &machine_key(data_root),
        ) {
            Ok(adopted) => log::info!("its data: {adopted:?}"),
            Err(words) => {
                log::error!("a signed app's data did not move: {words}");
                return notice(
                    data_root,
                    "This app did not open",
                    &format!(
                        "{}. Nothing was lost; open it again to finish.",
                        capitalised(&words)
                    ),
                );
            }
        }
        let stores = crate::run_config::stores(data_root, &manifest.app.id, verified.as_deref());
        let sandbox = stores.files.clone();
        let _ = std::fs::create_dir_all(&sandbox);
        // The app's key-value store and database: the runtime opens them only
        // when told where, and the phone players never said, so every
        // store.kv and store.sql call was refused and a recipe book made in
        // Studio quit at start. Kept beside the sandbox, not in it, as on a
        // desktop: the app names keys, never files.
        let _ = std::fs::create_dir_all(data_root.join("stores"));
        let files = crate::run_config::writes_files(&manifest).then(|| sandbox.clone());
        let mut config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox,
            app_store_path: Some(stores.kv.clone()),
            app_database_path: Some(stores.sql.clone()),
            // Its secrets (store.secret), encrypted with a key of this
            // phone's: a Vault's entries had nowhere to live on Android.
            app_secrets: Some((
                stores.secrets.clone(),
                stores.secret_label.clone(),
                machine_key(data_root),
            )),
            // Its shared store (store.shared): a phone never said where, so
            // every shared call was refused and a list shared from a
            // computer could not be joined on a phone.
            app_shared: Some((
                stores.shared.clone(),
                crate::run_config::SHARED_HUB.to_string(),
            )),
            bundle_assets_root: assets_root,
            // The name the person agreed to on the wall: a notification's
            // sender line, never text the app picks per call.
            app_name: Some(manifest.app.name.clone()),
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        // The same data on every device the person signs in on (IC-981):
        // signed in on this phone, the app's store.kv syncs by their account.
        config.account_sync = crate::run_config::sync_plan(
            data_root,
            verified.as_deref(),
            &stores.kv,
            crate::account::token(data_root),
        )
        .map(
            |plan| krate_runtime::account_sync_host::AccountSyncContext {
                hub: plan.hub.to_string(),
                token: plan.token,
                device_dir: plan.device_dir,
                device_name: phone_name(),
                machine_key: machine_key(data_root),
                app_id: manifest.app.id.clone(),
                publisher: plan.publisher,
                enabled: plan.enabled,
                background: true,
                // Its own folder: its files follow the person too (S3).
                files,
            },
        );
        if let Some(bytes) = memory_bytes {
            config.memory_bytes = bytes;
        }
        // An organisation's private app (K-1171): it checks in as the
        // person signed in on this phone and gets its team data and company
        // connections, as on a computer; refused, it does not open.
        let phone = crate::run_config::Phone::here(data_root, &phone_name());
        match crate::run_config::org_run(&phone, path, &opened, &manifest, &config.session_policy) {
            Ok(run) => {
                for (title, words) in &run.notices {
                    notice(data_root, title, words);
                }
                config.app_org = run.app_org;
                config.connections = run.connections;
            }
            Err(words) => {
                log::info!("an organisation's app was refused: {words}");
                return notice(data_root, "This app did not open", &capitalised(&words));
            }
        }
        // Which app runs, for its reminders: their ids carry its id, and a
        // tapped one opens the copy kept in the library.
        let opens = format!("file://{}", path.display());
        krate_adapter_android::reminders::set_current_app(Some((
            manifest.app.id.clone(),
            opens.clone(),
        )));
        // For Krate's menu (IC-934): the app's name, and what it may use this
        // run in the permission sheet's own words.
        let mut uses: Vec<String> = manifest
            .capabilities
            .iter()
            .filter(|request| request.required || granted.contains(&request.cap))
            .filter_map(|request| use_in_words(&request.cap))
            .collect();
        uses.dedup();
        krate_adapter_android::menu::set_app(Some((
            &manifest.app.name,
            &manifest.app.id,
            &uses,
            &opens,
        )));
        let runtime = match krate_runtime::Runtime::new(&config) {
            Ok(runtime) => runtime,
            Err(err) => {
                log::error!("engine failed to start: {err:#}");
                return;
            }
        };
        // A command-line program prints and ends, and a phone has no
        // terminal: what it printed is kept, and shown once it ends, where
        // before the app flashed and was gone.
        let program = prints_only(&manifest);
        let ran = if program {
            runtime
                .run_bytes_transcribed_for_world(
                    &component,
                    &config,
                    krate_runtime::RuntimeWorld::Cli,
                )
                .map(|(outcome, transcript)| {
                    let code = match &outcome {
                        krate_runtime::RunOutcome::Exited(code) => Some(*code),
                        krate_runtime::RunOutcome::LimitExceeded(_) => None,
                    };
                    show_output(
                        data_root,
                        output_args(&manifest.app.name, code, &transcript),
                    );
                    outcome
                })
        } else {
            runtime.run_bytes_for_world(&component, &config, krate_runtime::RuntimeWorld::Auto)
        };
        krate_adapter_android::menu::set_app(None);
        match ran {
            Ok(outcome) => log::info!("app finished: {outcome:?}"),
            Err(err) => {
                log::error!("app failed: {err:#}");
                notice(
                    data_root,
                    &format!("{} stopped", manifest.app.name),
                    &format!("{err:#}"),
                );
            }
        }
        krate_adapter_android::reminders::set_current_app(None);
        drop(opened);
        // Restart (or Ask again) in Krate's menu: the app closed, so open it
        // again, from the wall, in a fresh process.
        if krate_adapter_android::menu::restart_asked() {
            // The app as it was, blurred, for the sheet to sit over.
            krate_adapter_android::behind::write(&data_root.join("wall").join("behind.rgba"));
            krate_adapter_android::menu::relaunch();
        }
    }

    /// Run the wall sheet and return the granted capability names, or None
    /// for cancel. An app that declares nothing privileged shows no wall.
    fn show_wall(manifest: &krate_manifest::Manifest, data_root: &Path) -> Option<Vec<String>> {
        if manifest.capabilities.is_empty() {
            return Some(Vec::new());
        }

        // One argument per field, never one string the wall has to cut up.
        //
        // This used to join the name, capability, rationale and required flag
        // with U+001E and U+001C and hand the wall the result. Nothing checked
        // the manifest strings for those characters, so an app name could
        // carry them and add rows to the wall: an app declaring only ui.window
        // displayed a net.connect request with a convincing reason, and a
        // crafted rationale could flip its own capability from required to
        // optional -- required rows being the ones a person cannot switch off.
        //
        // The runtime rejects any argument containing a newline or a NUL
        // (encode_args_raw), so argument boundaries cannot be forged the way
        // an in-band separator can. Manifest parsing now refuses these
        // characters in displayed fields as well; both locks stay.
        // `wall:ask` first: the line under the name (its version) is its own
        // argument, so a name cannot forge it.
        let mut input = vec![
            "wall:ask".to_string(),
            manifest.app.name.clone(),
            format!("Version {}", manifest.app.version),
        ];
        for request in &manifest.capabilities {
            input.push(request.cap.clone());
            input.push(request.rationale.clone());
            input.push(if request.required { "1" } else { "0" }.to_string());
        }

        let wall_manifest = krate_manifest::Manifest::parse(WALL_MANIFEST).ok()?;
        let policy = krate_policy::SessionPolicy::allow_all_declared(&wall_manifest).ok()?;
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: data_root.join("wall"),
            app_args: input,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        // Every way the wall cannot run is logged: one that only returned
        // None read in logcat as the person saying no (K-1207).
        let runtime = krate_runtime::Runtime::new(&config)
            .map_err(|err| log::error!("the wall could not start: {err:#}"))
            .ok()?;
        let (_, stdout) = runtime
            .run_bytes_captured_for_world(WALL_WASM, &config, krate_runtime::RuntimeWorld::Gui)
            .map_err(|err| log::error!("the wall failed: {err:#}"))
            .ok()?;

        let text = String::from_utf8_lossy(&stdout);
        for line in text.lines() {
            if let Some(list) = line.strip_prefix("wall:open:") {
                return Some(
                    list.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
            }
            if line.trim() == "wall:cancel" {
                return None;
            }
        }
        // No decision line at all -- a crashed or killed wall. Fail closed.
        None
    }

    /// Tell the person why nothing opened, on the wall's trusted card, and
    /// wait for Close. A failure that only reached the log looked, on a
    /// phone, like the app flashing and vanishing.
    fn notice(data_root: &Path, title: &str, message: &str) {
        // The runtime refuses arguments with newlines; a reason never needs one.
        let message = message.replace(['\n', '\0'], " ");
        let Some(wall_manifest) = krate_manifest::Manifest::parse(WALL_MANIFEST).ok() else {
            return;
        };
        let Ok(policy) = krate_policy::SessionPolicy::allow_all_declared(&wall_manifest) else {
            return;
        };
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: data_root.join("wall"),
            app_args: vec![
                "wall:notice".to_string(),
                title.replace(['\n', '\0'], " "),
                message,
            ],
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        if let Ok(runtime) = krate_runtime::Runtime::new(&config) {
            let _ = runtime.run_bytes_captured_for_world(
                WALL_WASM,
                &config,
                krate_runtime::RuntimeWorld::Gui,
            );
        }
    }

    /// Show what a command-line program printed, on the wall's trusted
    /// card, until Close.
    fn show_output(data_root: &Path, args: Vec<String>) {
        let Some(wall_manifest) = krate_manifest::Manifest::parse(WALL_MANIFEST).ok() else {
            return;
        };
        let Ok(policy) = krate_policy::SessionPolicy::allow_all_declared(&wall_manifest) else {
            return;
        };
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: data_root.join("wall"),
            app_args: args,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        match krate_runtime::Runtime::new(&config) {
            Ok(runtime) => {
                if let Err(err) = runtime.run_bytes_captured_for_world(
                    WALL_WASM,
                    &config,
                    krate_runtime::RuntimeWorld::Gui,
                ) {
                    log::error!("the output card failed: {err:#}");
                }
            }
            Err(err) => log::error!("the output card could not start: {err:#}"),
        }
    }

    /// Run one of Krate's own screens (the home screen) with everything its
    /// manifest declares, in `sandbox`, and return what it printed. Every way
    /// it cannot run is logged, as the wall's are (K-1207).
    fn run_trusted(
        wasm: &[u8],
        manifest: &str,
        sandbox: &Path,
        args: Vec<String>,
    ) -> Option<String> {
        let manifest = krate_manifest::Manifest::parse(manifest)
            .map_err(|err| log::error!("a trusted screen's manifest: {err:#}"))
            .ok()?;
        let policy = krate_policy::SessionPolicy::allow_all_declared(&manifest)
            .map_err(|err| log::error!("a trusted screen's policy: {err:#}"))
            .ok()?;
        let _ = std::fs::create_dir_all(sandbox);
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox.to_path_buf(),
            app_args: args,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        let runtime = krate_runtime::Runtime::new(&config)
            .map_err(|err| log::error!("{} could not start: {err:#}", manifest.app.name))
            .ok()?;
        let (_, stdout) = runtime
            .run_bytes_captured_for_world(wasm, &config, krate_runtime::RuntimeWorld::Gui)
            .map_err(|err| log::error!("{} failed: {err:#}", manifest.app.name))
            .ok()?;
        Some(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// What Krate calls itself to the hub, whose edge refuses requests
    /// without a real user agent.
    const USER_AGENT: &str = concat!("Krate/", env!("CARGO_PKG_VERSION"), " (Android)");

    /// This phone's key for apps' secrets and its organisation key: 32
    /// random bytes made once and kept in Krate's own data, which no other
    /// app can read. Copying an app's secrets file elsewhere carries nothing
    /// usable.
    fn machine_key(data_root: &Path) -> Vec<u8> {
        crate::run_config::machine_key(data_root)
    }

    /// A sentence's first letter in capitals, for a notice's words.
    fn capitalised(words: &str) -> String {
        let mut chars = words.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }

    /// What the person's device list calls this phone: its model, as the
    /// system says it ("SM-S908B"), else "Android phone".
    fn phone_name() -> String {
        static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        NAME.get_or_init(|| {
            let ask = |prop: &str| {
                std::process::Command::new("getprop")
                    .arg(prop)
                    .output()
                    .ok()
                    .and_then(|out| String::from_utf8(out.stdout).ok())
                    .map(|text| text.trim().to_string())
                    .filter(|text| !text.is_empty())
            };
            let maker = ask("ro.product.manufacturer");
            match (maker, ask("ro.product.model")) {
                (Some(maker), Some(model)) if !model.starts_with(&maker) => {
                    format!("{maker} {model}")
                }
                (_, Some(model)) => model,
                _ => "Android phone".to_string(),
            }
        })
        .clone()
    }
}
