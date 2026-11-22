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
    println!("cargo:rerun-if-changed=src/main.rs");
    live_ui_facts();
    tauri_build::build();
}

/// What a signed interface bundle is judged against (IC-998, live_ui.rs):
/// when this build's own interface was last changed, and every command this
/// build answers. A bundle older than the first is not newer; a bundle
/// calling a command outside the second would break on this Studio.
fn live_ui_facts() {
    // The commit time of the last change to ui/ -- the same clock the
    // bundles are stamped with. 0 (no git, a tarball build) lets any
    // correctly signed, compatible bundle through, which is still safe.
    let serial = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ct", "--", "ui"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    println!("cargo:rustc-env=KRATE_UI_SERIAL={serial}");

    let main = fs::read_to_string("src/main.rs").expect("read src/main.rs");
    let start = main
        .find("tauri::generate_handler![")
        .expect("main.rs registers its commands with generate_handler!");
    let body = &main[start + "tauri::generate_handler![".len()..];
    // `])` closes the macro; an attribute inside it ends `)]`, not `])`.
    let end = body.find("])").expect("generate_handler! closes");
    let mut names: Vec<String> = body[..end]
        .split(',')
        // `#[cfg(...)] name`: the name after the attribute.
        .map(|n| n.rsplit(']').next().unwrap_or(n).trim())
        // A command from a module is registered as path::name; JS calls it by name.
        .map(|n| n.rsplit("::").next().unwrap_or(n).to_string())
        .filter(|n| !n.is_empty() && !n.starts_with("//"))
        .collect();
    names.sort();
    names.dedup();
    let out = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("studio_commands.rs");
    fs::write(
        out,
        format!("pub const STUDIO_COMMANDS: &[&str] = &{names:?};\n"),
    )
    .expect("write studio_commands.rs");
}
