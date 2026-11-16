//! The newest Krate on the computer brings the others along (IC-984).
//!
//! Every app `krate install` puts in ~/Applications on a Mac is a copy of
//! the engine that installed it, and so is every launcher: macOS takes the
//! Dock name from the executable really running, so a shim cannot do it.
//! Nothing used to refresh an installed app's copy, so a double-clicked app
//! kept the runtime it was installed with -- every fix since, and every
//! change to where Krate keeps state, missed it (K-1278).
//!
//! Here:
//!
//! - Each engine's fingerprint carries its commit time, so any two engines
//!   order. One without it predates this and is older than any with it --
//!   which also means it knows nothing of the secrets key record
//!   (machine_key.rs), the reason a file key cannot move while one is about.
//! - The newest engine -- a release build, not dirty, not run from a cargo
//!   build tree, not an installed app's own copy -- refreshes every
//!   Krate-made bundle carrying an older engine, staged beside and swapped
//!   in, keeping the folder name the Dock and Launchpad hold. Once per
//!   engine build, in a detached helper, so no command waits on it.
//! - Then, if every engine it can find knows the record and none older is
//!   still running, a secrets key kept as a file moves to the keychain or
//!   Windows. Otherwise the record says which engine holds it back, and the
//!   check is tried again at most once a day.
//!
//! The Krate Player app and Studio are never rewritten: the Player is
//! notarized and Studio updates itself. They are only asked what they are.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// One engine build, as its fingerprint describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Build {
    version: [u64; 3],
    dirty: bool,
    known_source: bool,
    /// The commit's time; `None` for every engine from before IC-984.
    commit_time: Option<u64>,
}

impl Build {
    /// What a bundle with no fingerprint at all carries: older than all.
    fn unknown() -> Self {
        Build {
            version: [0, 0, 0],
            dirty: false,
            known_source: false,
            commit_time: None,
        }
    }

    /// Whether this engine knows the secrets key record (machine_key.rs):
    /// every engine with a commit time was built after it.
    pub(crate) fn knows_the_key_record(&self) -> bool {
        self.commit_time.is_some()
    }
}

/// This engine's fingerprint: `<version>-<sha>-<length>-<mtime>-t<commit
/// time>`. The length and mtime tell dirty rebuilds of one commit apart for
/// the launchers' equality check; the commit time orders engines.
pub(crate) fn fingerprint() -> String {
    let (len, mtime) = std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| {
            (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    format!(
        "{}-{}-{len}-{mtime}-t{}",
        env!("CARGO_PKG_VERSION"),
        env!("KRATE_GIT_SHA"),
        env!("KRATE_COMMIT_TIME"),
    )
}

/// Read a fingerprint, this format or any older one.
pub(crate) fn parse(fingerprint: &str) -> Build {
    let text = fingerprint.trim();
    let mut parts = text.split('-');
    let version = parts
        .next()
        .map(|v| {
            let mut out = [0u64; 3];
            for (slot, n) in out.iter_mut().zip(v.split('.')) {
                *slot = n.parse().unwrap_or(0);
            }
            out
        })
        .unwrap_or([0, 0, 0]);
    let sha = parts.next().unwrap_or("");
    let commit_time = text
        .rsplit('-')
        .next()
        .and_then(|last| last.strip_prefix('t'))
        .and_then(|t| t.parse::<u64>().ok())
        .filter(|&t| t > 0);
    Build {
        version,
        dirty: text.contains("-dirty-"),
        known_source: !sha.is_empty() && sha != "unknown",
        commit_time,
    }
}

/// Is `ours` newer than `theirs`? Only an engine that knows its own commit
/// time can say so. Against one from before commit times, it is newer
/// unless that one names a later version.
pub(crate) fn newer(ours: &Build, theirs: &Build) -> bool {
    let Some(mine) = ours.commit_time else {
        return false;
    };
    match theirs.commit_time {
        Some(their) => mine > their,
        None => ours.version >= theirs.version,
    }
}

/// Why this engine must not rewrite other apps, if it must not.
fn held_from_refreshing(ours: &Build) -> Option<&'static str> {
    let testing = cfg!(debug_assertions) && std::env::var_os(TEST_ENV).is_some();
    if testing {
        return None;
    }
    if cfg!(debug_assertions) {
        return Some("a development build does not rewrite installed apps");
    }
    if ours.dirty || !ours.known_source || ours.commit_time.is_none() {
        return Some("a build of changed or unknown source does not rewrite installed apps");
    }
    if run_from_a_build_tree() {
        return Some("an engine run from a build folder does not rewrite installed apps");
    }
    None
}

/// Lets a debug build's tests run the sweep; ignored in a release build.
const TEST_ENV: &str = "KRATE_TEST_ENGINE_SWEEP";

/// Run from `<project>/target/...`, where a person builds Krate.
fn run_from_a_build_tree() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe.ancestors().any(|dir| {
        dir.file_name().is_some_and(|n| n == "target")
            && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
    })
}

fn record_file() -> PathBuf {
    crate::krate_home().join("engine-sweep.json")
}

fn read_record() -> Value {
    std::fs::read_to_string(record_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Which engine holds the secrets key back as a file, as the last sweep
/// found it -- what `krate identity` says.
pub(crate) fn key_held_by() -> Option<String> {
    read_record()["held_by"].as_str().map(str::to_string)
}

/// Called at the start of every command: a file read, and once per engine
/// build a detached helper. Never from an installed app's own engine, a
/// helper, or an engine that may not refresh others.
pub(crate) fn notice() {
    if !cfg!(any(target_os = "macos", windows))
        || std::env::var_os(HELPER_ENV).is_some()
        || crate::installed_app_payload().is_some()
    {
        return;
    }
    let ours_text = fingerprint();
    let ours = parse(&ours_text);
    if held_from_refreshing(&ours).is_some() {
        return;
    }
    let record = read_record();
    let same = record["fingerprint"].as_str() == Some(ours_text.as_str());
    let settled = record["done"].as_bool() == Some(true);
    let recent = record["at"]
        .as_u64()
        .is_some_and(|at| now_secs().saturating_sub(at) < 24 * 3600);
    if same && (settled || recent) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg("engine-sweep")
        .env(HELPER_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Set on the helper so it never spawns a helper.
const HELPER_ENV: &str = "KRATE_ENGINE_SWEEP_HELPER";

/// `krate engine-build`: this engine's fingerprint, for a newer engine
/// deciding whether this one knows the key record.
pub(crate) fn print_build() -> u8 {
    println!("{}", fingerprint());
    0
}

/// `krate engine-sweep`: refresh older bundles, then move a file key if
/// nothing can be hurt by it. Prints what it did.
pub(crate) fn sweep_now() -> anyhow::Result<u8> {
    let ours_text = fingerprint();
    let ours = parse(&ours_text);
    if let Some(why) = held_from_refreshing(&ours) {
        println!("Left installed apps as they are: {why}.");
        return Ok(0);
    }
    let Some(_lock) = Lock::take() else {
        println!("Another Krate is bringing the installed apps up to date.");
        return Ok(0);
    };
    let mut done = true;
    let mut refreshed = Vec::new();
    for bundle in krate_bundles() {
        let theirs = std::fs::read_to_string(bundle.join("Contents/Resources/engine.fingerprint"))
            .map(|t| parse(&t))
            .unwrap_or_else(|_| Build::unknown());
        if !newer(&ours, &theirs) {
            continue;
        }
        match refresh(&bundle) {
            Ok(()) => refreshed.push(bundle),
            Err(err) => {
                done = false;
                eprintln!(
                    "krate: could not bring {} up to date: {err:#}",
                    bundle.display()
                );
            }
        }
    }
    for bundle in &refreshed {
        println!("Brought up to date: {}", bundle.display());
    }
    let held_by = match move_the_key() {
        Ok(Some(moved)) => {
            println!("Your secrets key is kept {} now.", moved.describe());
            None
        }
        Ok(None) => None,
        Err(held) => {
            done = false;
            println!("Your secrets key stays a file for now: {held}.");
            Some(held)
        }
    };
    let record = json!({
        "fingerprint": ours_text,
        "at": now_secs(),
        "done": done,
        "held_by": held_by,
    });
    let _ = std::fs::create_dir_all(crate::krate_home());
    let _ = std::fs::write(record_file(), record.to_string());
    Ok(0)
}

/// One sweep at a time. A lock older than a quarter of an hour is a sweep
/// that died, and is taken over.
struct Lock(PathBuf);

impl Lock {
    fn take() -> Option<Lock> {
        let path = crate::krate_home().join("engine-sweep.lock");
        let _ = std::fs::create_dir_all(crate::krate_home());
        let stale = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(15 * 60));
        if stale {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()
            .map(|_| Lock(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/* ---- the bundles ---------------------------------------------------------- */

/// Every app bundle Krate made on this computer: installed apps and
/// launchers. Only a folder holding Krate's payload counts.
fn krate_bundles() -> Vec<PathBuf> {
    // Only a Mac's bundles carry an engine; elsewhere this stays empty.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let mut roots = Vec::new();
        if let Ok(root) = crate::install_root(None) {
            roots.push(root);
        }
        roots.push(crate::krate_home().join("launchers"));
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let named_app = path.extension().is_some_and(|e| e == "app");
                let hidden = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
                if named_app && !hidden && path.join("Contents/Resources/app.krate").is_file() {
                    out.push(path);
                }
            }
        }
        out.sort();
    }
    out
}

/// Rebuild one bundle around this engine, under the same folder name.
#[cfg(target_os = "macos")]
fn refresh(bundle: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let parent = bundle.parent().context("an app with no folder")?;
    let folder = bundle.file_name().context("an app with no name")?;
    let stage = parent.join(format!(".krate-refresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let cleanup = crate::RemoveOnDrop(stage.clone());
    // The payload is read before anything moves; install_bundle copies it.
    let built = crate::install_bundle(&bundle.join("Contents/Resources/app.krate"), &stage)?;
    let previous = parent.join(format!(".{}.replaced", folder.to_string_lossy()));
    crate::publish_staged(&built, bundle, &previous)?;
    drop(cleanup);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn refresh(_bundle: &Path) -> anyhow::Result<()> {
    anyhow::bail!("only a Mac's apps carry an engine")
}

/* ---- every engine on this computer -------------------------------------- */

/// What an engine is, for the key move: a bundle's recorded fingerprint, or
/// what a binary answers to `engine-build`. `None` is an engine too old to
/// answer -- or one that would not, which counts the same.
fn engines_here() -> Vec<(String, Option<Build>)> {
    let mut out = Vec::new();
    for bundle in krate_bundles() {
        let build = std::fs::read_to_string(bundle.join("Contents/Resources/engine.fingerprint"))
            .ok()
            .map(|t| parse(&t));
        out.push((bundle.display().to_string(), build));
    }
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());
    let mut seen = std::collections::BTreeSet::new();
    for binary in engine_binaries() {
        let Ok(real) = std::fs::canonicalize(&binary) else {
            continue;
        };
        if Some(&real) == me.as_ref() || !seen.insert(real.clone()) {
            continue;
        }
        out.push((binary.display().to_string(), ask(&real)));
    }
    out
}

/// Every place a Krate engine binary is known to live.
fn engine_binaries() -> Vec<PathBuf> {
    let name = if cfg!(windows) { "krate.exe" } else { "krate" };
    let mut out: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).map(|d| d.join(name)).collect())
        .unwrap_or_default();
    if let Some(home) = crate::home_dir() {
        out.push(home.join(".local/bin").join(name));
        for apps in [home.join("Applications"), PathBuf::from("/Applications")] {
            out.push(apps.join("Krate.app/Contents/Resources/bin/krate"));
            out.push(apps.join("Krate Player.app/Contents/MacOS/krate-cli"));
        }
    }
    out.push(PathBuf::from("/usr/local/bin").join(name));
    out.push(PathBuf::from("/opt/homebrew/bin").join(name));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(local).join("Krate").join("bin").join(name));
    }
    out.retain(|p| p.is_file());
    out
}

/// Ask a binary what it is, for at most ten seconds.
fn ask(binary: &Path) -> Option<Build> {
    let mut child = std::process::Command::new(binary)
        .arg("engine-build")
        .env(HELPER_ENV, "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut text = String::new();
                use std::io::Read;
                child.stdout.take()?.read_to_string(&mut text).ok()?;
                let line = text.lines().next()?.trim();
                return Some(parse(line)).filter(|b| b.known_source || b.commit_time.is_some());
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// The first engine that would not follow a key out of its file, if any.
pub(crate) fn first_that_cannot_follow(engines: &[(String, Option<Build>)]) -> Option<String> {
    engines
        .iter()
        .find(|(_, build)| !build.as_ref().is_some_and(Build::knows_the_key_record))
        .map(|(what, _)| format!("{what} is an older Krate, and it knows only the file"))
}

/// Move a file key to the keychain or Windows when nothing can be hurt by
/// it. `Ok(None)`: nothing to move, or no stronger keeper here.
fn move_the_key() -> Result<Option<crate::machine_key::Kept>, String> {
    if crate::machine_key::kept() != Some(crate::machine_key::Kept::File)
        || !crate::machine_key::stronger_keeper_here()
    {
        return Ok(None);
    }
    let engines = engines_here();
    if let Some(held) = first_that_cannot_follow(&engines) {
        return Err(held);
    }
    if let Some(running) = an_older_engine_running(&engines) {
        return Err(format!(
            "{running} is still running an older engine; it moves after"
        ));
    }
    crate::machine_key::move_to_keeper().map(Some)
}

/// A process still running an engine binary that has been replaced since
/// it started -- it would read the file as an older engine does.
fn an_older_engine_running(engines: &[(String, Option<Build>)]) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/bin/ps")
            .args(["-axww", "-o", "etime=,comm="])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let watched: Vec<PathBuf> = engines
            .iter()
            .map(|(what, _)| PathBuf::from(what))
            .collect();
        for line in text.lines() {
            let line = line.trim();
            let Some((elapsed, command)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let command = Path::new(command.trim());
            let ours = watched
                .iter()
                .any(|w| command.starts_with(w) || command == w);
            if !ours {
                continue;
            }
            let Some(age) = parse_elapsed(elapsed) else {
                continue;
            };
            let replaced_ago = std::fs::metadata(command)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok());
            if let Some(replaced_ago) = replaced_ago {
                if age > replaced_ago + Duration::from_secs(2) {
                    return Some(command.display().to_string());
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows will not replace a running executable, so an engine on
        // disk that knows the record is the engine every process of it runs.
        let _ = engines;
        None
    }
}

/// `ps`'s elapsed time: `[[dd-]hh:]mm:ss`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_elapsed(text: &str) -> Option<Duration> {
    let (days, rest) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(days * 86400 + secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(text: &str) -> Build {
        parse(text)
    }

    #[test]
    fn every_fingerprint_reads_old_and_new() {
        let old = build("0.5.5-e33246a8cbf1-dirty-229444616-1794831365");
        assert_eq!(old.version, [0, 5, 5]);
        assert!(old.dirty && old.known_source);
        assert_eq!(old.commit_time, None, "an engine from before commit times");
        let new = build("0.5.6-abc123def456-1000-1794831365-t1794800000");
        assert_eq!(new.commit_time, Some(1794800000));
        assert!(!new.dirty);
        assert!(new.knows_the_key_record() && !old.knows_the_key_record());
        assert_eq!(build("").commit_time, None);
        assert_eq!(
            build("0.1.0-unknown-1-1-t0").commit_time,
            None,
            "zero is unknown"
        );
        assert_eq!(parse(&fingerprint()), parse(&fingerprint()));
    }

    #[test]
    fn engines_order_by_commit_time_and_never_ping_pong() {
        let a = build("0.5.5-aaa-1-1-t100");
        let b = build("0.5.5-bbb-1-1-t200");
        assert!(
            newer(&b, &a) && !newer(&a, &b),
            "the later commit wins, one way"
        );
        assert!(!newer(&a, &a), "an engine never refreshes its own build");
        let before = build("0.5.5-e33246a8cbf1-229444616-1794831365");
        assert!(
            newer(&a, &before),
            "any engine with a time is after any without"
        );
        assert!(
            !newer(&before, &a),
            "an engine without a time never claims newer"
        );
        let later_version_before = build("0.9.0-fff-1-1");
        assert!(
            !newer(&a, &later_version_before),
            "a later version is not overtaken"
        );
        assert!(!newer(
            &before,
            &build("0.1.28-a2c95fbf5be1-56262736-1787629953")
        ));
    }

    #[test]
    fn a_key_moves_only_when_every_engine_can_follow_it() {
        let new = Some(build("0.5.6-abc-1-1-t200"));
        assert_eq!(
            first_that_cannot_follow(&[("a".into(), new.clone()), ("b".into(), new.clone())]),
            None
        );
        let held = first_that_cannot_follow(&[
            ("a".into(), new.clone()),
            (
                "/Users/x/.local/bin/krate".into(),
                Some(build("0.1.58-abc-1-1")),
            ),
        ])
        .unwrap();
        assert!(held.starts_with("/Users/x/.local/bin/krate"), "{held}");
        assert!(
            first_that_cannot_follow(&[("silent".into(), None)]).is_some(),
            "an engine that will not answer counts as older"
        );
    }

    #[test]
    fn a_development_build_does_not_rewrite_installed_apps() {
        if std::env::var_os(TEST_ENV).is_none() {
            let ours = parse(&fingerprint());
            assert!(held_from_refreshing(&ours).is_some());
        }
    }

    #[test]
    fn ps_elapsed_times_read() {
        assert_eq!(parse_elapsed("00:05"), Some(Duration::from_secs(5)));
        assert_eq!(parse_elapsed("02:00:05"), Some(Duration::from_secs(7205)));
        assert_eq!(
            parse_elapsed("3-00:00:01"),
            Some(Duration::from_secs(3 * 86400 + 1))
        );
        assert_eq!(parse_elapsed("x"), None);
    }
}
