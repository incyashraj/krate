//! The Krate player for iOS -- M2 of the mobile plan.
//!
//! The same product as the Android player: open an app, show the wall, run
//! it. The engine underneath is Pulley -- wasmtime's interpreter -- because
//! iOS forbids executable pages, and the measurement said the interpreter
//! renders krate-gram pixel-identical to the JIT.
//!
//! Startup is the one genuinely iOS-shaped part. UIApplicationMain owns
//! the process and never returns, so the player's real main runs from the
//! app delegate's first `applicationDidBecomeActive:` and never returns
//! either: the guest's own frame pacing pumps the run loop through the
//! adapter, which is how touches arrive. When the flow ends the process
//! exits -- the app is the process, same as Android.
//!
//! This build runs the embedded demo through the wall. Link and file
//! opening (universal links, UTI document types) arrive with the Apple
//! developer account, which the plan tracks.

#[cfg(target_os = "ios")]
mod player {
    use objc2::{define_class, ClassType, DefinedClass, MainThreadOnly};
    use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
    use objc2_ui_kit::{UIApplication, UIApplicationDelegate};
    use std::cell::Cell;

    const RECORD_SEP: char = '\u{1e}';
    const FIELD_SEP: char = '\u{1c}';

    const GRAM_WASM: &[u8] =
        include_bytes!("../../../apps/krate-gram/target/wasm32-wasip1/release/krate_gram.wasm");
    const GRAM_MANIFEST: &str = include_str!("../../../apps/krate-gram/manifest.toml");
    const WALL_WASM: &[u8] =
        include_bytes!("../../../apps/krate-wall/target/wasm32-wasip1/release/krate_wall.wasm");
    const WALL_MANIFEST: &str = include_str!("../../../apps/krate-wall/manifest.toml");

    pub struct DelegateState {
        started: Cell<bool>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements for an app delegate.
        // - UIKit calls the delegate on the main thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = DelegateState]
        struct KrateAppDelegate;

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for KrateAppDelegate {}

        impl KrateAppDelegate {
            // SAFETY: init matches NSObject's designated initializer, and
            // UIKit instantiates the delegate through exactly this path --
            // the ivars must be planted here or they never exist.
            #[unsafe(method_id(init))]
            fn init(this: objc2::rc::Allocated<Self>) -> objc2::rc::Retained<Self> {
                let this = this.set_ivars(DelegateState {
                    started: Cell::new(false),
                });
                unsafe { objc2::msg_send![super(this), init] }
            }
        }

        // SAFETY: the implemented methods match UIApplicationDelegate's
        // signatures.
        unsafe impl UIApplicationDelegate for KrateAppDelegate {
            #[unsafe(method(applicationDidBecomeActive:))]
            fn did_become_active(&self, _application: &UIApplication) {
                // Launch has completed by the first activation, so a main
                // that never returns cannot trip the launch watchdog; the
                // adapter pumps the run loop from inside the guest's own
                // frame pacing, so the app stays responsive.
                if self.ivars().started.replace(true) {
                    return;
                }
                run_player();
                std::process::exit(0);
            }
        }
    );

    pub fn main() {
        let mtm = objc2::MainThreadMarker::new()
            .expect("the iOS player's main runs on the main thread");
        let delegate_class = NSString::from_class(KrateAppDelegate::class());
        UIApplication::main(None, Some(&delegate_class), mtm);
    }

    fn run_player() {
        let data_root = std::env::var("HOME")
            .map(|home| std::path::PathBuf::from(home).join("Documents"))
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&data_root);

        let manifest = match krate_manifest::Manifest::parse(GRAM_MANIFEST) {
            Ok(manifest) => manifest,
            Err(err) => {
                eprintln!("manifest failed to parse: {err:#}");
                return;
            }
        };

        let granted = match show_wall(&manifest, &data_root) {
            Some(granted) => granted,
            None => {
                eprintln!("the person said no; nothing runs");
                return;
            }
        };

        let declared = match manifest.declared_capabilities() {
            Ok(declared) => declared,
            Err(err) => {
                eprintln!("capabilities failed to parse: {err:#}");
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

        let sandbox = data_root.join("apps").join("demo");
        let _ = std::fs::create_dir_all(&sandbox);
        let config = krate_runtime::Config {
            session_policy: policy,
            sandbox_root: sandbox,
            phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
            ..Default::default()
        };
        let runtime = match krate_runtime::Runtime::new(&config) {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("engine failed to start: {err:#}");
                return;
            }
        };
        match runtime.run_bytes_for_world(GRAM_WASM, &config, krate_runtime::RuntimeWorld::Gui) {
            Ok(outcome) => eprintln!("app finished: {outcome:?}"),
            Err(err) => eprintln!("app failed: {err:#}"),
        }
    }

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
            .map_err(|err| eprintln!("the wall failed: {err:#}"))
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
        // A crashed sheet grants nothing.
        None
    }
}

#[cfg(target_os = "ios")]
fn main() {
    player::main();
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("the Krate iOS player only runs on iOS");
}
