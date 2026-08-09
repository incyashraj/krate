//! The Krate player for Android -- M1 of the mobile plan.
//!
//! One job: open a .krate, show the wall, run the app. Phones receive;
//! desktops make. The player is the receiving half on the device where
//! shared links actually land.
//!
//! This is the first-light build: it runs one embedded app (krate-gram, the
//! modern-UI acceptance test) to prove the whole stack -- wasmtime, the
//! capability wall, the CPU painter, the winit surface -- on a real Android
//! surface. Intent handling (.krate files, krate.tech/a/ links) and the
//! wall-as-bottom-sheet land next; nothing ships to strangers before the
//! wall does.

#[cfg(target_os = "android")]
mod player {
    use krate_adapter_android::winit_native::AndroidApp;

    // The embedded first-light app. krate-gram is deliberate: it exercises
    // scrolling, springs, shadows, styled text and the whole painter -- if
    // it feels right on a phone, M1's rendering half is proven. Build
    // apps/krate-gram before building the player.
    const GRAM_WASM: &[u8] =
        include_bytes!("../../../apps/krate-gram/target/wasm32-wasip1/release/krate_gram.wasm");
    const GRAM_MANIFEST: &str = include_str!("../../../apps/krate-gram/manifest.toml");

    #[no_mangle]
    fn android_main(app: AndroidApp) {
        android_logger::init_once(
            android_logger::Config::default().with_max_level(log::LevelFilter::Info),
        );
        log::info!("krate player: first light");

        // The adapter can only build its event loop from the AndroidApp the
        // platform hands us, and only on this thread.
        krate_adapter_android::winit_native::set_android_app(app.clone());

        // The app's private files dir is the sandbox root -- the same
        // boundary the desktop runtime enforces, in the place Android
        // already isolates per app.
        let sandbox = app
            .internal_data_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&sandbox);

        let manifest = match krate_manifest::Manifest::parse(GRAM_MANIFEST) {
            Ok(manifest) => manifest,
            Err(err) => {
                log::error!("embedded manifest failed to parse: {err:#}");
                return;
            }
        };
        // First light grants everything the manifest declares. The wall --
        // the person seeing and answering those lines -- is the ship gate
        // for M1; this build exists to prove the pipeline underneath it.
        let policy = match krate_policy::SessionPolicy::allow_all_declared(&manifest) {
            Ok(policy) => policy,
            Err(err) => {
                log::error!("policy from manifest failed: {err:#}");
                return;
            }
        };

        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox,
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
        match runtime.run_bytes_for_world(GRAM_WASM, &config, krate_runtime::RuntimeWorld::Gui) {
            Ok(outcome) => log::info!("app finished: {outcome:?}"),
            Err(err) => log::error!("app failed: {err:#}"),
        }
        // The app is the process. When it exits -- close button, Android
        // back, or its own choice -- a blank NativeActivity shell must not
        // linger behind it.
        std::process::exit(0);
    }
}
