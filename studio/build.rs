use std::{env, fs, path::Path};

fn ensure_resource_glob_matches() {
    let bin_dir = Path::new("bin");
    let placeholder = bin_dir.join("placeholder");

    // Keep the release safeguard intact: release packaging must fail when the
    // real engine has not been staged by the release workflow. Remove a
    // placeholder left by an earlier test build before Tauri checks `bin/*`.
    if matches!(env::var("PROFILE").as_deref(), Ok("release")) {
        match fs::remove_file(&placeholder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("failed to remove Studio resource placeholder: {error}"),
        }
        return;
    }

    let has_resource = fs::read_dir(bin_dir)
        .ok()
        .and_then(|mut entries| entries.next())
        .is_some();

    if has_resource {
        return;
    }

    fs::create_dir_all(bin_dir).expect("failed to create Studio resource directory");
    fs::write(placeholder, []).expect("failed to create Studio resource placeholder");
}

fn main() {
    // A fresh checkout has no staged engine, but Tauri still requires the
    // `bin/*` resource glob to match while compiling debug and test targets.
    ensure_resource_glob_matches();

    // Tauri embeds the frontend directory into the native binary. Track it
    // explicitly so a CSS or JavaScript-only change cannot leave `cargo build`
    // reporting success while the launched Studio still serves stale assets.
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=bin");
    tauri_build::build();
}
