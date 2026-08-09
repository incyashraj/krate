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

#[cfg(target_os = "android")]
mod player {
    use krate_adapter_android::winit_native::AndroidApp;
    use std::io::Cursor;

    const RECORD_SEP: char = '\u{1e}';
    const FIELD_SEP: char = '\u{1c}';
    const MAX_BUNDLE_BYTES: usize = 6 * 1024 * 1024;

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
        run_player(&data_root, handles);
        // The app is the process: when the flow ends -- back button, close,
        // cancel at the wall -- no blank activity shell may linger.
        std::process::exit(0);
    }

    fn run_player(data_root: &std::path::Path, handles: crate::intent::ActivityHandles) {
        // What was tapped decides what runs.
        let target = crate::intent::view_target(handles);
        log::info!("launch target: {target:?}");

        let manifest_text: String;
        let component: Vec<u8>;
        let assets_root: Option<std::path::PathBuf>;
        // The extracted bundle dir is removed on drop, and the runtime
        // reads assets from it during the run -- it must live to the end.
        let mut _opened_bundle = None;

        match target {
            Some(uri) => {
                let bytes = if uri.starts_with("https://") || uri.starts_with("http://") {
                    log::info!("fetching {uri}");
                    match fetch(&uri) {
                        Ok(bytes) => bytes,
                        Err(why) => {
                            log::error!("could not fetch the app: {why}");
                            return;
                        }
                    }
                } else {
                    log::info!("opening {uri}");
                    match crate::intent::read_uri(handles, &uri, MAX_BUNDLE_BYTES) {
                        Ok(bytes) => bytes,
                        Err(why) => {
                            log::error!("could not read the file: {why}");
                            return;
                        }
                    }
                };
                let open = match krate_bundle::open_reader(Cursor::new(bytes)) {
                    Ok(open) => open,
                    Err(err) => {
                        log::error!("that is not a Krate app: {err:#}");
                        return;
                    }
                };
                manifest_text =
                    std::fs::read_to_string(open.manifest_path()).unwrap_or_default();
                component = match std::fs::read(open.component_path()) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        log::error!("bundle component unreadable: {err}");
                        return;
                    }
                };
                assets_root = open.assets_path().map(|p| p.to_path_buf());
                _opened_bundle = Some(open);
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

        let sandbox = data_root.join("apps").join(sanitize(&manifest.app.id));
        let _ = std::fs::create_dir_all(&sandbox);
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox,
            bundle_assets_root: assets_root,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        let runtime = match krate_runtime::Runtime::new(&config) {
            Ok(runtime) => runtime,
            Err(err) => {
                log::error!("engine failed to start: {err:#}");
                return;
            }
        };
        match runtime.run_bytes_for_world(&component, &config, krate_runtime::RuntimeWorld::Auto) {
            Ok(outcome) => log::info!("app finished: {outcome:?}"),
            Err(err) => log::error!("app failed: {err:#}"),
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

        let mut input = manifest.app.name.clone();
        for request in &manifest.capabilities {
            input.push(RECORD_SEP);
            input.push_str(&request.cap);
            input.push(FIELD_SEP);
            input.push_str(&request.rationale);
            input.push(FIELD_SEP);
            input.push(if request.required { '1' } else { '0' });
        }

        let wall_manifest = krate_manifest::Manifest::parse(WALL_MANIFEST).ok()?;
        let policy = krate_policy::SessionPolicy::allow_all_declared(&wall_manifest).ok()?;
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: data_root.join("wall"),
            app_args: vec![input],
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        let runtime = krate_runtime::Runtime::new(&config).ok()?;
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

    /// App ids become directory names; nothing an id says may escape the
    /// apps folder.
    fn sanitize(id: &str) -> String {
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
}
