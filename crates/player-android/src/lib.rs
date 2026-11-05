//! The Krate player for Android -- M1 of the mobile plan.
//!
//! One job: open a .krate, show the wall, run the app. Phones receive;
//! desktops make. Three ways in, one flow:
//!
//! - a tapped krate.tech/a/ link (VIEW intent, https) -- fetched from the
//!   hub over TLS;
//! - a tapped .krate file (VIEW intent, content:// or file://) -- read
//!   through the ContentResolver;
//! - a plain launch -- the embedded demo app.
//!
//! Whatever the source, the app never runs before the wall: a trusted
//! sheet (itself a Krate guest whose pixels come from the same renderer)
//! shows every capability line with the app's own rationale, required
//! rows locked on, the rest togglable. The player passes the lines in and
//! reads one decision line back from captured stdout; both sides of that
//! pipe belong to the player, so the sheet cannot be bypassed and cannot
//! be lied to. Deny still opens: whatever was toggled off is simply not
//! in the session policy, and the runtime enforces the difference.

#[cfg(target_os = "android")]
mod intent;
pub mod run_config;

#[cfg(target_os = "android")]
mod player {
    use crate::run_config::{output_args, prints_only, sanitize, use_in_words};
    use krate_adapter_android::winit_native::AndroidApp;
    use std::io::Cursor;

    const MAX_BUNDLE_BYTES: usize = crate::run_config::MAX_BUNDLE_BYTES as usize;

    // The demo app, for a plain launch with no intent: krate-gram, the
    // modern-UI acceptance test. It goes through the same wall as any
    // downloaded app -- the demo demonstrates the product, and the wall is
    // the product.
    const GRAM_WASM: &[u8] =
        include_bytes!("../../../apps/krate-gram/target/wasm32-wasip1/release/krate_gram.wasm");
    const GRAM_MANIFEST: &str = include_str!("../../../apps/krate-gram/manifest.toml");

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
        if app
            .external_data_path()
            .is_some_and(|dir| dir.join(".frame-stats").exists())
        {
            std::env::set_var("KRATE_FRAME_STATS", "1");
        }
        run_player(&data_root, handles);
        // The app is the process: when the flow ends -- back button, close,
        // cancel at the wall -- no blank activity shell may linger. Finish
        // the activity first: Android restarts a top activity whose process
        // died moments after launch, with the same link, so a file that
        // could not be read flickered open and shut forever (K-1015).
        crate::intent::finish(handles);
        std::process::exit(0);
    }

    fn run_player(data_root: &std::path::Path, handles: crate::intent::ActivityHandles) {
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
        // What was tapped decides what runs.
        let target = crate::intent::view_target(handles);
        log::info!("launch target: {target:?}");
        // A sign-in coming back to a Krate that is no longer running: the
        // app that asked is gone, so there is nothing to hand it to.
        if target
            .as_deref()
            .is_some_and(|uri| uri.starts_with("krate-signin:"))
        {
            log::info!("a sign-in came back after its app closed; nothing to do");
            return;
        }

        let manifest_text: String;
        let component: Vec<u8>;
        let assets_root: Option<std::path::PathBuf>;
        // The extracted bundle dir is removed on drop, and the runtime
        // reads assets from it during the run -- it must live to the end.
        let mut _opened_bundle = None;
        // The bundle as it came, kept so a reminder tapped later can open
        // this app again (IC-921): a link may be offline by then, and a
        // file's grant from the app that sent it does not last.
        let mut kept_bytes: Option<Vec<u8>> = None;

        match target {
            Some(uri) => {
                let bytes = if uri.starts_with("https://") || uri.starts_with("http://") {
                    log::info!("fetching {uri}");
                    match fetch(&uri) {
                        Ok(bytes) => bytes,
                        Err(why) => {
                            log::error!("could not fetch the app: {why}");
                            let why =
                                format!("{why}. Check the connection and open the link again.");
                            return notice(data_root, "Could not download this app", &why);
                        }
                    }
                } else {
                    log::info!("opening {uri}");
                    match crate::intent::read_uri(handles, &uri, MAX_BUNDLE_BYTES) {
                        Ok(bytes) => bytes,
                        Err(why) => {
                            log::error!("could not read the file: {why}");
                            return notice(data_root, "Could not open this file", &why);
                        }
                    }
                };
                // Read in place, then kept: a big app is not held twice.
                let open = match krate_bundle::open_reader(Cursor::new(bytes.as_slice())) {
                    Ok(open) => open,
                    Err(err) => {
                        log::error!("that is not a Krate app: {err:#}");
                        // A Krate too old for the app says so in words a person
                        // can act on; anything else is a damaged or wrong file.
                        let why = match err {
                            krate_bundle::BundleError::NeedsNewerKrate { .. } => format!("{err}"),
                            _ => "This file is not a Krate app, or it was damaged on the way. Ask for it again.".to_string(),
                        };
                        return notice(data_root, "This app did not open", &why);
                    }
                };
                manifest_text = std::fs::read_to_string(open.manifest_path()).unwrap_or_default();
                component = match std::fs::read(open.component_path()) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        log::error!("bundle component unreadable: {err}");
                        return;
                    }
                };
                assets_root = open.assets_path().map(|p| p.to_path_buf());
                _opened_bundle = Some(open);
                kept_bytes = Some(bytes);
            }
            None => {
                manifest_text = GRAM_MANIFEST.to_string();
                component = GRAM_WASM.to_vec();
                assets_root = None;
            }
        }

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

        let sandbox = data_root.join("apps").join(sanitize(&manifest.app.id));
        let _ = std::fs::create_dir_all(&sandbox);
        // The app's key-value store and database: the runtime opens them only
        // when told where, and the phone players never said, so every
        // store.kv and store.sql call was refused and a recipe book made in
        // Studio quit at start. Kept beside the sandbox, not in it, as on a
        // desktop: the app names keys, never files.
        let stores = crate::run_config::stores(data_root, &manifest.app.id);
        let _ = std::fs::create_dir_all(data_root.join("stores"));
        let mut config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox,
            app_store_path: Some(stores.kv.clone()),
            app_database_path: Some(stores.sql.clone()),
            // Its secrets (store.secret), encrypted with a key of this
            // phone's: a Vault's entries had nowhere to live on Android.
            app_secrets: Some((
                stores.secrets.clone(),
                manifest.app.id.clone(),
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
        if let Some(bytes) = memory_bytes {
            config.memory_bytes = bytes;
        }
        // Which app runs, for its reminders: their ids carry its id, and a
        // tapped one opens the copy kept here.
        let opens = kept_bytes
            .and_then(|bytes| {
                let kept = data_root.join("kept");
                let _ = std::fs::create_dir_all(&kept);
                let path = kept.join(format!("{}.krate", sanitize(&manifest.app.id)));
                std::fs::write(&path, bytes).ok()?;
                Some(format!("file://{}", path.display()))
            })
            .unwrap_or_default();
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
    fn show_wall(
        manifest: &krate_manifest::Manifest,
        data_root: &std::path::Path,
    ) -> Option<Vec<String>> {
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
    fn notice(data_root: &std::path::Path, title: &str, message: &str) {
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
    fn show_output(data_root: &std::path::Path, args: Vec<String>) {
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

    fn fetch(url: &str) -> Result<Vec<u8>, String> {
        let response = ureq::get(url).call().map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        use std::io::Read;
        response
            .into_reader()
            .take(MAX_BUNDLE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err("that download is larger than any Krate app".to_string());
        }
        Ok(bytes)
    }

    /// This phone's key for apps' secrets: 32 random bytes made once and
    /// kept in Krate's own data, which no other app can read. Copying an
    /// app's secrets file elsewhere carries nothing usable.
    fn machine_key(data_root: &std::path::Path) -> Vec<u8> {
        let path = data_root.join("machine.key");
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
            let _ = std::fs::write(&path, &key);
        }
        key
    }
}
