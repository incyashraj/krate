//! The embedded guest SDK, materialized on demand.
//!
//! `krate create` builds a generated app, and that app's `Cargo.toml` points at
//! the Krate WIT interfaces and the Rust bindings crate. Rather than require a
//! repo checkout, the binary carries the SDK inside it (see `build.rs`) and
//! writes it to a per-version cache directory the first time it is needed.
//! Later runs reuse the same directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// The build script embeds `EMBEDDED_SDK: &[(&str, &[u8])]`.
include!(concat!(env!("OUT_DIR"), "/embedded_sdk.rs"));

/// The SDK version tag the cache directory is keyed on, so a new binary lays
/// down a fresh SDK rather than reusing a stale one.
fn sdk_version() -> &'static str {
    // Distinct SDK contents get a distinct cache dir, including uncommitted WIT
    // changes during development.
    option_env!("KRATE_SDK_FINGERPRINT")
        .or(option_env!("KRATE_GIT_SHA"))
        .unwrap_or("dev")
}

/// The directory the SDK is materialized into for this binary's version.
fn sdk_cache_dir() -> Result<PathBuf> {
    let base = cache_root().context("find a cache directory")?;
    Ok(base.join("krate").join("sdk").join(sdk_version()))
}

/// The platform cache root: `XDG_CACHE_HOME` or `~/.cache` on Unix,
/// `LOCALAPPDATA` on Windows, falling back to a temp dir.
fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return Some(PathBuf::from(dir));
    }
    if let Some(home) = crate::home_dir() {
        return Some(home.join(".cache"));
    }
    Some(std::env::temp_dir())
}

/// Ensure the embedded SDK is present on disk and return its root directory.
///
/// This is idempotent: if the version's cache directory already holds the
/// files, it is returned as-is. Writing goes through a temp directory that is
/// renamed into place, so a concurrent or interrupted run never leaves a
/// half-written SDK behind.
pub fn ensure_materialized() -> Result<PathBuf> {
    let root = sdk_cache_dir()?;
    // A marker file records that the full SDK was written; its presence means
    // the directory is complete.
    let marker = root.join(".complete");
    if marker.is_file() {
        return Ok(root);
    }

    let parent = root.parent().context("sdk cache has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    // Write into a sibling temp dir, then rename into place.
    let staging = parent.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    for (rel, bytes) in EMBEDDED_SDK {
        write_file(&staging.join(rel), bytes)?;
    }
    write_file(&staging.join(".complete"), b"")?;

    // If another process won the race, keep theirs and drop ours.
    if marker.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(root);
    }
    let _ = std::fs::remove_dir_all(&root);
    match std::fs::rename(&staging, &root) {
        Ok(()) => Ok(root),
        Err(_) if marker.is_file() => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok(root)
        }
        Err(err) => Err(err).with_context(|| format!("install SDK into {}", root.display())),
    }
}

/// Point an app's Cargo.toml at `current` (this binary's SDK copy) where it
/// names another version's copy in the same SDK cache (K-1311).
///
/// `krate create` writes the SDK's absolute cache path into the app, keyed
/// on the version that made it. A newer Krate then built the app against
/// the OLD copy: an SDK fix never reached an existing app, and a cleared
/// cache broke its build. Only paths under `current`'s own cache directory
/// are touched; a path into a checkout or anywhere else is left alone.
/// Answers the rewritten text and the old versions it replaced, or `None`
/// when nothing pointed elsewhere.
pub fn repoint_cached_sdk(text: &str, current: &Path) -> Option<(String, Vec<String>)> {
    let version = current.file_name()?.to_string_lossy().into_owned();
    let base = current.parent()?.to_string_lossy().into_owned();
    let mut out = String::from(text);
    let mut replaced: Vec<String> = Vec::new();
    // The same cache directory as Cargo.toml may spell it: as written, with
    // the other separator, or with TOML-escaped backslashes on Windows.
    let mut spellings: Vec<(String, &str)> = vec![
        (base.clone(), "/"),
        (base.clone(), "\\"),
        (base.replace('\\', "/"), "/"),
        (base.replace('\\', "\\\\"), "\\\\"),
    ];
    spellings.dedup();
    for (prefix, sep) in spellings {
        let lead = format!("{prefix}{sep}");
        let mut from = 0;
        while let Some(at) = out[from..].find(&lead) {
            let start = from + at + lead.len();
            let end = out[start..]
                .find(['/', '\\', '"', '\''])
                .map_or(out.len(), |e| start + e);
            let pinned = out[start..end].to_string();
            if !pinned.is_empty() && pinned != version {
                out.replace_range(start..end, &version);
                if !replaced.contains(&pinned) {
                    replaced.push(pinned);
                }
                from = start + version.len();
            } else {
                from = end;
            }
        }
    }
    (!replaced.is_empty()).then_some((out, replaced))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod repoint_tests {
    use super::repoint_cached_sdk;
    use std::path::Path;

    const OLD: &str = r#"[dependencies]
krate = { path = "/home/a/.cache/krate/sdk/22e4ac2beb5f1dbd/crates/bindings-rust", features = ["gui"] }

[package.metadata.component.target]
path = "/home/a/.cache/krate/sdk/22e4ac2beb5f1dbd/wit/krate/phase4"
"krate:io" = { path = "/home/a/.cache/krate/sdk/22e4ac2beb5f1dbd/wit/krate/phase4/deps/io" }
"#;

    /// The oven board Codex made with one Krate, rebuilt by a newer one:
    /// every pinned path moves to the running binary's copy (K-1311).
    #[test]
    fn an_app_pinned_to_an_older_sdk_builds_against_this_one() {
        let current = Path::new("/home/a/.cache/krate/sdk/29a3131cf89b1e4a");
        let (text, old) = repoint_cached_sdk(OLD, current).expect("it pointed elsewhere");
        assert_eq!(old, vec![String::from("22e4ac2beb5f1dbd")]);
        assert!(!text.contains("22e4ac2beb5f1dbd"), "{text}");
        assert_eq!(
            text.matches("/krate/sdk/29a3131cf89b1e4a/").count(),
            3,
            "{text}"
        );
        assert!(
            text.contains("features = [\"gui\"]"),
            "nothing else changes"
        );
    }

    #[test]
    fn an_app_already_on_this_sdk_or_on_a_checkout_is_left_alone() {
        let current = Path::new("/home/a/.cache/krate/sdk/22e4ac2beb5f1dbd");
        assert!(repoint_cached_sdk(OLD, current).is_none());
        let checkout = "krate = { path = \"../../crates/bindings-rust\" }\n";
        assert!(repoint_cached_sdk(checkout, Path::new("/home/a/.cache/krate/sdk/x")).is_none());
        let elsewhere =
            "krate = { path = \"/opt/krate/sdk/22e4ac2beb5f1dbd/crates/bindings-rust\" }\n";
        assert!(repoint_cached_sdk(elsewhere, Path::new("/home/a/.cache/krate/sdk/x")).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_path_with_escaped_backslashes_is_repointed() {
        let current = Path::new("C:\\Users\\a\\AppData\\Local\\krate\\sdk\\new1");
        let text = "krate = { path = \"C:\\\\Users\\\\a\\\\AppData\\\\Local\\\\krate\\\\sdk\\\\old1\\\\crates\\\\bindings-rust\" }\n";
        let (out, old) = repoint_cached_sdk(text, current).expect("it pointed elsewhere");
        assert_eq!(old, vec![String::from("old1")]);
        assert!(out.contains("new1") && !out.contains("old1"), "{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_every_wit_phase_the_checkout_has() {
        // The SDK must carry EVERY phase, not a list somebody remembered to
        // extend. This test named phase2 and phase3 by hand; phase 4 was
        // created, `build.rs` kept its two-element list, and every bundle
        // packed after that carried an SDK that could not rebuild a phase 4
        // app -- "failed to create a target world" (K-419). The test could
        // not see it, because it was asking about the two phases that were
        // there when it was written.
        //
        // So ask the CHECKOUT what phases exist and require all of them. A
        // phase 5 that nobody adds to the SDK fails here.
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("repo root");
        let wit_root = repo.join("wit/krate");
        let phases: Vec<String> = std::fs::read_dir(&wit_root)
            .expect("read wit/krate")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.join("world.wit").is_file())
            .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(
            !phases.is_empty(),
            "no WIT phase found under {}",
            wit_root.display()
        );

        let paths: Vec<&str> = EMBEDDED_SDK.iter().map(|(p, _)| *p).collect();
        for phase in &phases {
            let world = format!("wit/krate/{phase}/world.wit");
            assert!(
                paths.iter().any(|p| p.ends_with(&world)),
                "the SDK must carry {world}, or an app on that phase cannot \
                 rebuild from a bundle. Phases in the checkout: {phases:?}"
            );
        }

        assert!(paths.contains(&"crates/bindings-rust/Cargo.toml"));
        assert!(paths.contains(&"crates/bindings-rust/src/lib.rs"));
        // `features = ["sheets"]` needs the module's own directory (IC-993);
        // its tests read fixtures the SDK does not carry, so they stay out.
        for file in ["mod.rs", "zip.rs", "xml.rs", "xlsx.rs", "ods.rs", "csv.rs"] {
            let path = format!("crates/bindings-rust/src/sheets/{file}");
            assert!(paths.contains(&path.as_str()), "{path} is not shipped");
        }
        assert!(!paths.contains(&"crates/bindings-rust/src/sheets/tests.rs"));
        // `features = ["modbus"]` and `["mqtt"]` (IC-994): the shared link
        // and each module's own directory, without its tests.
        assert!(paths.contains(&"crates/bindings-rust/src/link.rs"));
        for file in [
            "modbus/mod.rs",
            "modbus/frame.rs",
            "mqtt/mod.rs",
            "mqtt/packet.rs",
        ] {
            let path = format!("crates/bindings-rust/src/{file}");
            assert!(paths.contains(&path.as_str()), "{path} is not shipped");
        }
        assert!(!paths.contains(&"crates/bindings-rust/src/modbus/tests.rs"));
        assert!(!paths.contains(&"crates/bindings-rust/src/mqtt/tests.rs"));
    }

    #[test]
    fn materialized_bindings_cargo_is_standalone() {
        // The embedded bindings Cargo.toml must not inherit from a workspace.
        let (_, bytes) = EMBEDDED_SDK
            .iter()
            .find(|(p, _)| *p == "crates/bindings-rust/Cargo.toml")
            .expect("bindings Cargo.toml embedded");
        let text = std::str::from_utf8(bytes).expect("utf8");
        assert!(!text.contains(".workspace = true"));
        // And it is the SAME crate the checkout has, version above all: an
        // app built in the checkout locks `krate` at the workspace version,
        // and a rebuild against the shipped SDK must resolve to it (K-340).
        // This crate inherits its version from the same workspace.
        assert!(
            text.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        // The SDK keeps its own, lower floor: the workspace's rust-version is
        // the runtime's, and declaring it for the guest SDK made `krate
        // create` refuse on a default 1.91 toolchain.
        assert!(text.contains("rust-version = \"1.91\""), "{text}");
    }

    #[test]
    fn materialize_is_idempotent_and_complete() {
        let root = ensure_materialized().expect("materialize");
        assert!(root.join("wit/krate/phase2/world.wit").is_file());
        assert!(root.join("crates/bindings-rust/src/bindings.rs").is_file());
        // A second call returns the same directory without error.
        let again = ensure_materialized().expect("materialize again");
        assert_eq!(root, again);
    }
}
