//! Studio replacing itself with a newer release, in place, on every OS
//! (IC-1001, K-1369).
//!
//! The person presses Update, sees "Installing Krate 0.6.2", and Studio opens
//! again as the new version. No disk image window, no "drag Krate to
//! Applications", no installer wizard, nothing left mounted.
//!
//! Two halves:
//! - `hand_off` runs inside Studio before it quits: it unpacks the verified
//!   download next to the app it will replace and checks the copy (on a Mac:
//!   signature, team, Gatekeeper). Anything wrong is an error Studio shows
//!   while it is still open, and nothing has changed.
//! - A small helper script runs after Studio quits: it swaps the new copy in
//!   with two renames, opens it, waits for it to say it started
//!   (`note_started`), and only then throws the old copy away. If the new one
//!   does not start, the old one goes back and opens, and `failed.txt` says
//!   why on its next start.
//!
//! The helper scripts are plain constants compiled on every OS, so the tests
//! on one machine run the other systems' swap logic too.

// Each system uses its own helper; the others stay compiled so the tests on
// any one machine run all three, which leaves them unused outside tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A test-only update feed: a base URL serving `latest.json` (shaped like
/// GitHub's latest-release answer: `tag_name`, `body`) and
/// `v<version>/<file>` for each release file including `SHA256SUMS`.
pub const FEED_ENV: &str = "KRATE_UPDATE_FEED";
/// Run the update headless and log to this file instead of opening a window
/// (for proving an update on a machine someone is using).
pub const SELFTEST_ENV: &str = "KRATE_UPDATE_SELFTEST";

const RELEASES: &str = "https://github.com/incyashraj/krate/releases";

fn feed() -> Option<String> {
    std::env::var(FEED_ENV)
        .ok()
        .map(|f| f.trim().trim_end_matches('/').to_string())
        .filter(|f| !f.is_empty())
}

/// Where the newest release is announced.
pub fn latest_url() -> String {
    match feed() {
        Some(f) => format!("{f}/latest.json"),
        None => "https://api.github.com/repos/incyashraj/krate/releases/latest".to_string(),
    }
}

/// Where one release's files are downloaded from.
pub fn download_base(version: &str) -> String {
    match feed() {
        Some(f) => format!("{f}/v{version}"),
        None => format!("{RELEASES}/download/v{version}"),
    }
}

/// The release file this system installs from.
pub fn asset_name(version: &str) -> String {
    if cfg!(windows) {
        format!("krate-studio-{version}-windows-x64-setup.exe")
    } else if cfg!(target_os = "macos") {
        format!("krate-studio-{version}-universal.dmg")
    } else {
        format!("krate-studio-{version}-linux-x86_64.AppImage")
    }
}

/// "krate-studio-0.6.1-universal.dmg" -> "0.6.1". Any of our installer
/// names, on any system; None for anything else.
pub fn installer_version(file: &str) -> Option<&str> {
    let rest = file.strip_prefix("krate-studio-")?;
    for tail in [
        "-universal.dmg",
        "-windows-x64-setup.exe",
        "-linux-x86_64.AppImage",
    ] {
        if let Some(v) = rest.strip_suffix(tail) {
            return (!v.is_empty()).then_some(v);
        }
    }
    None
}

/// Semver order with pre-releases: 0.6.2-test1 < 0.6.2-test2 < 0.6.2.
/// Unparseable versions sort first.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(v: &str) -> (Option<(u64, u64, u64)>, Option<&str>) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        let triple = match (it.next(), it.next(), it.next()) {
            (Some(Some(a)), Some(Some(b)), Some(Some(c))) => Some((a, b, c)),
            _ => None,
        };
        (triple, pre)
    }
    let (ta, pa) = split(a);
    let (tb, pb) = split(b);
    ta.cmp(&tb).then_with(|| match (pa, pb) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => {
            // Numeric tails compare as numbers: test9 < test10.
            let num = |s: &str| {
                let digits: String = s.chars().rev().take_while(char::is_ascii_digit).collect();
                let n = digits.chars().rev().collect::<String>();
                (s[..s.len() - n.len()].to_string(), n.parse::<u64>().ok())
            };
            num(x).cmp(&num(y))
        }
    })
}

/// The new Studio says it is up: `<version>\n<pid>\n` in `updates/started`.
/// The helper waits for this before it deletes the old copy.
pub fn note_started(updates: &Path, version: &str) {
    let _ = std::fs::create_dir_all(updates);
    let tmp = updates.join(format!(".started.{}", std::process::id()));
    if std::fs::write(&tmp, format!("{version}\n{}\n", std::process::id())).is_ok() {
        let _ = std::fs::rename(&tmp, updates.join("started"));
    }
}

/// An update that did not take, once: `(version, why)`.
pub fn take_failed(updates: &Path) -> Option<(String, String)> {
    let path = updates.join("failed.txt");
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let version = lines.next()?.trim().to_string();
    let why = lines.next().unwrap_or("").trim().to_string();
    (!version.is_empty()).then_some((version, why))
}

/// Tidy what earlier updates left, at every start: our own update images
/// still mounted (Studio 0.6.0 and older opened them in Finder and never
/// closed them), downloads of this version or older, half downloads, and
/// staging folders.
pub fn cleanup_on_launch(updates: &Path, running: &str) {
    #[cfg(target_os = "macos")]
    let busy = mac::detach_ours(updates);
    #[cfg(not(target_os = "macos"))]
    let busy: Vec<PathBuf> = Vec::new();

    let Ok(entries) = std::fs::read_dir(updates) else {
        return;
    };
    let hour = std::time::Duration::from_secs(3600);
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if busy.iter().any(|b| same_file(b, &path)) {
            continue;
        }
        if let Some(v) = installer_version(&name) {
            if compare_versions(v, running) != std::cmp::Ordering::Greater {
                let _ = std::fs::remove_file(&path);
            }
        } else if name.starts_with('.') && name.ends_with(".partial") {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > hour);
            if old {
                let _ = std::fs::remove_file(&path);
            }
        } else if name == "stage" {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
    // Staging left beside the app by an installer step that was killed.
    // Never a fresh one: the step that is waiting for this very start keeps
    // the old version in it until it hears from us.
    if let Some(dir) = install_dir() {
        remove_stale_staging(&dir);
    }
    // Private mount points of an update step that was stopped halfway.
    // remove_dir only takes an empty folder, so a live mount is left alone.
    if let Ok(temp) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in temp.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("krate-update-mnt-")
            {
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
    // A receipt for a file that is gone is a receipt for nothing.
    if let Ok(text) = std::fs::read_to_string(updates.join("verified.json")) {
        let named = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["file"].as_str().map(str::to_string));
        if named.is_none_or(|f| !updates.join(f).exists()) {
            let _ = std::fs::remove_file(updates.join("verified.json"));
        }
    }
}

/// The folder the running Krate sits in: the one holding the .app on a
/// Mac, the AppImage's folder on Linux.
fn install_dir() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        return std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .and_then(|p| p.parent().map(Path::to_path_buf));
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
        .and_then(|app| app.parent().map(Path::to_path_buf))
}

/// Remove `.krate-update-*` / `.krate-previous-*` entries in `dir` that are
/// more than ten minutes old (an installer step takes about a minute at
/// most).
pub fn remove_stale_staging(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name.starts_with(".krate-update-") || name.starts_with(".krate-previous-")) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(600));
        if stale {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        }
}

/// Can this person create files in `dir`? Asked by doing it.
pub fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".krate-write-check-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// One argument for /bin/sh, quoted.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The test-only settings the relaunched Studio must keep: they do not pass
/// through `open` or a new session by themselves.
fn forwarded() -> (String, String, String) {
    let get = |k: &str| std::env::var(k).unwrap_or_default();
    (get(FEED_ENV), get(SELFTEST_ENV), get("HOME"))
}

/// Unpack and check the verified download, then start the helper that
/// swaps it in once Studio has quit. `Ok` means Studio must now quit;
/// `Err` says, in plain words, why nothing was changed.
pub fn hand_off(verified: &Path, version: &str, updates: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return mac::hand_off(verified, version, updates);
    #[cfg(windows)]
    return windows::hand_off(verified, version, updates);
    #[cfg(target_os = "linux")]
    return linux::hand_off(verified, version, updates);
    #[allow(unreachable_code)]
    {
        let _ = (verified, version, updates);
        Err("Krate cannot update itself on this system.".to_string())
    }
}

/* ---- macOS ------------------------------------------------------------- */

/// After Studio quits: swap the checked copy in, open it, wait for it to say
/// it started, and roll back if it does not.
///
/// `$1` Studio's pid, `$2` the new app (already in `.krate-update-<pid>`
/// next to the target, or anywhere when run as root), `$3` the app to
/// replace, `$4` Studio's updates folder, `$5` the version, `$6` the Team ID
/// the new app must carry ("-" for none), `$7` "1" when the new app must pass
/// Gatekeeper, `$8` the person's uid when this runs as root (the
/// administrator path) else empty, `$9`.. the test-only feed, self-test log
/// and HOME to pass on.
///
/// It never opens the disk image and never asks the person to do anything:
/// every failure puts the old Krate back, opens it, and leaves the reason in
/// `failed.txt` for it to say.
pub const MAC_SWAP_SCRIPT: &str = r##"
set -u
pid="$1"; src="$2"; target="$3"; updates="$4"; version="$5"; team="$6"; gk="$7"; asuid="$8"
feed="$9"; shift 9; selftest="$1"; home="$2"
case "$target" in /*.app) ;; *) exit 2 ;; esac
name=$(basename "$target"); parent=$(dirname "$target")
work="$parent/.krate-update-$pid"; new="$work/$name"; prev="$work/previous.app"
marker="$updates/started"; failed="$updates/failed.txt"; log="$updates/update.log"
ticks="${KRATE_UPDATE_WAIT_TICKS:-300}"; settle="${KRATE_UPDATE_SETTLE:-3}"; opener="${KRATE_UPDATE_OPEN:-/usr/bin/open}"
say() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*" >> "$log"; }
mine() { [ -n "$asuid" ] && chown "$asuid" "$@" 2>/dev/null; return 0; }
launch() {
  if [ -n "$feed$selftest" ]; then
    set -- --env "HOME=$home" --env "KRATE_UPDATE_FEED=$feed" --env "KRATE_UPDATE_SELFTEST=$selftest" "$1"
  fi
  if [ -n "$asuid" ]; then
    launchctl asuser "$asuid" sudo -u "#$asuid" "$opener" "$@"
  else
    "$opener" "$@"
  fi
}
give_up() {
  printf '%s\n%s\n' "$version" "$1" > "$failed"; mine "$failed" "$log"
  say "not installed: $1"
  rm -rf "$work"
  [ -e "$target" ] && launch "$target"
  exit 1
}
verify() {
  codesign --verify --deep --strict "$1" >/dev/null 2>&1 || return 1
  if [ "$team" != "-" ]; then
    got=$(codesign -dv --verbose=2 "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p')
    [ "$got" = "$team" ] || return 1
  fi
  if [ "$gk" = 1 ]; then spctl --assess --type execute "$1" >/dev/null 2>&1 || return 1; fi
  return 0
}
lock="$updates/.installing"
find "$lock" -maxdepth 0 -mmin +10 -exec rmdir {} \; 2>/dev/null
mkdir "$lock" 2>/dev/null || { rm -rf "$work"; exit 1; }
trap 'rmdir "$lock" 2>/dev/null' EXIT
say "installing $version over $target"; mine "$log"
i=0
while kill -0 "$pid" 2>/dev/null; do
  [ $i -ge 150 ] && { rm -rf "$work"; printf '%s\n%s\n' "$version" "Krate did not close, so the update was not installed." > "$failed"; mine "$failed"; exit 1; }
  sleep 0.2; i=$((i+1))
done
mkdir -p "$work" || give_up "Krate could not write next to $target."
if [ "$src" != "$new" ]; then
  rm -rf "$new"
  ditto "$src" "$new" || give_up "Krate could not copy the new version into place."
  xattr -dr com.apple.quarantine "$new" 2>/dev/null
fi
verify "$new" || give_up "The new version did not pass the Mac's signature check, so it was not installed."
rm -rf "$prev"
if [ -e "$target" ]; then
  mv "$target" "$prev" || give_up "Krate could not move the old version aside."
fi
if ! mv "$new" "$target"; then
  [ -e "$prev" ] && mv "$prev" "$target"
  give_up "Krate could not move the new version into place."
fi
say "swapped in $version"
rm -f "$marker"
launch "$target"
ok=""; i=0
while [ $i -lt "$ticks" ]; do
  if [ -f "$marker" ] && [ "$(sed -n 1p "$marker")" = "$version" ]; then ok=1; break; fi
  sleep 0.2; i=$((i+1))
done
if [ -n "$ok" ]; then
  npid=$(sed -n 2p "$marker"); sleep "$settle"
  kill -0 "$npid" 2>/dev/null || ok=""
fi
if [ -n "$ok" ]; then
  rm -rf "$work" "$updates/stage"
  rm -f "$updates"/krate-studio-* "$updates/verified.json"
  say "done: $version started"
  exit 0
fi
say "$version did not start; putting the old version back"
pkill -f "$target/Contents/MacOS/" 2>/dev/null; sleep 1
if [ -e "$prev" ]; then rm -rf "$target"; mv "$prev" "$target"; fi
give_up "Krate $version did not start, so Krate went back to the version you had."
"##;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    /// One of our update images, attached privately and detached on drop,
    /// whatever happens in between.
    struct Mounted {
        dev: String,
        point: PathBuf,
        made: Option<PathBuf>,
    }

    impl Mounted {
        fn attach(image: &Path) -> Result<Self, String> {
            let made =
                std::env::temp_dir().join(format!("krate-update-mnt-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&made);
            // -nobrowse: never in Finder's sidebar or on the desktop.
            // -noautoopen: never a Finder window, even for an image made to
            // open one. A private mount point instead of /Volumes.
            let out = Command::new("/usr/bin/hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-plist"])
                .arg("-mountpoint")
                .arg(&made)
                .arg(image)
                .output()
                .map_err(|e| format!("Krate could not open the download: {e}"))?;
            let text = String::from_utf8_lossy(&out.stdout);
            let dev = plist_strings(&text, "dev-entry").into_iter().next();
            let point = plist_strings(&text, "mount-point").into_iter().next();
            match (out.status.success(), dev, point) {
                (true, Some(dev), Some(point)) => Ok(Mounted {
                    dev,
                    point: PathBuf::from(point),
                    made: Some(made),
                }),
                _ => {
                    let _ = std::fs::remove_dir(&made);
                    Err(format!(
                        "Krate could not open the download ({}).",
                        String::from_utf8_lossy(&out.stderr)
                            .lines()
                            .rfind(|l| !l.contains("deprecated"))
                            .unwrap_or("hdiutil failed")
                            .trim()
                    ))
                }
            }
        }

        fn single_app(&self) -> Result<PathBuf, String> {
            let apps: Vec<PathBuf> = std::fs::read_dir(&self.point)
                .map_err(|e| format!("Krate could not read the download: {e}"))?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
                .collect();
            match apps.as_slice() {
                [one] => Ok(one.clone()),
                _ => Err("The download does not hold exactly one app.".to_string()),
            }
        }
    }

    impl Drop for Mounted {
        fn drop(&mut self) {
            detach(&self.dev);
            if let Some(made) = &self.made {
                let _ = std::fs::remove_dir(made);
            }
        }
    }

    fn detach(dev: &str) -> bool {
        let quiet = |force: bool| {
            let mut c = Command::new("/usr/bin/hdiutil");
            c.args(["detach", "-quiet"]);
            if force {
                c.arg("-force");
            }
            c.arg(dev)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        quiet(false) || quiet(true)
    }

    /// Every `<string>` that follows `<key>name</key>` in a plist.
    pub(super) fn plist_strings(text: &str, name: &str) -> Vec<String> {
        let key = format!("<key>{name}</key>");
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find(&key) {
            rest = &rest[at + key.len()..];
            let trimmed = rest.trim_start();
            if let Some(body) = trimmed.strip_prefix("<string>") {
                if let Some(end) = body.find("</string>") {
                    out.push(body[..end].to_string());
                }
            }
        }
        out
    }

    /// `hdiutil info`, reduced to (image file, whole-disk device, mount
    /// points) per attached image.
    pub(super) fn attached_images(info: &str) -> Vec<(PathBuf, String, Vec<PathBuf>)> {
        let mut out = Vec::new();
        for block in info.split("================================================") {
            let mut image = None;
            let mut dev = None;
            let mut mounts = Vec::new();
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("image-path") {
                    image = p.split_once(':').map(|(_, v)| PathBuf::from(v.trim()));
                } else if line.starts_with("/dev/disk") {
                    let mut cols = line.split('\t');
                    let d = cols.next().unwrap_or("").trim().to_string();
                    if dev.is_none() {
                        dev = Some(d);
                    }
                    if let Some(m) = cols.nth(1).map(str::trim).filter(|m| !m.is_empty()) {
                        mounts.push(PathBuf::from(m));
                    }
                }
            }
            if let (Some(image), Some(dev)) = (image, dev) {
                out.push((image, dev, mounts));
            }
        }
        out
    }

    /// Detach every attached image whose file sits in our updates folder,
    /// except one this Krate is running from. Returns the images that would
    /// not let go, so their files are kept.
    pub(super) fn detach_ours(updates: &Path) -> Vec<PathBuf> {
        let Ok(out) = Command::new("/usr/bin/hdiutil").arg("info").output() else {
            return Vec::new();
        };
        let ours = updates
            .canonicalize()
            .unwrap_or_else(|_| updates.to_path_buf());
        let exe = std::env::current_exe().unwrap_or_default();
        let mut busy = Vec::new();
        for (image, dev, mounts) in attached_images(&String::from_utf8_lossy(&out.stdout)) {
            if !(image.starts_with(&ours) || image.starts_with(updates)) {
                continue;
            }
            if mounts.iter().any(|m| exe.starts_with(m)) || !detach(&dev) {
                busy.push(image);
            }
        }
        busy
    }

    fn bundle_of(exe: &Path) -> Option<PathBuf> {
        exe.ancestors()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
            .map(Path::to_path_buf)
    }

    /// Where the update goes. Normally the running app itself. When macOS
    /// runs Krate from a hidden copy (App Translocation: opened straight from
    /// Downloads) or from the disk image, the update goes where apps live
    /// instead: /Applications, or ~/Applications when Krate is only there.
    pub(super) fn install_target(running: &Path, home: &Path) -> PathBuf {
        let s = running.to_string_lossy();
        let read_only_volume =
            s.starts_with("/Volumes/") && !running.parent().is_some_and(dir_writable);
        if !s.contains("/AppTranslocation/") && !read_only_volume {
            return running.to_path_buf();
        }
        let name = running
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "Krate.app".into());
        let system = Path::new("/Applications").join(&name);
        let user = home.join("Applications").join(&name);
        if !system.exists() && user.exists() {
            user
        } else {
            system
        }
    }

    fn info_value(app: &Path, key: &str) -> Option<String> {
        let text = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
        plist_strings(&text, key).into_iter().next()
    }

    fn team_of(app: &Path) -> Option<String> {
        let out = Command::new("/usr/bin/codesign")
            .args(["-dv", "--verbose=2"])
            .arg(app)
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stderr)
            .lines()
            .find_map(|l| l.strip_prefix("TeamIdentifier="))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && t != "not set")
    }

    fn gatekeeper_accepts(app: &Path) -> bool {
        Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute"])
            .arg(app)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// The trust the new app must meet: never less than the running one.
    /// A notarized Krate only takes a notarized Krate from the same team; a
    /// signed one only a signed one from the same team; an unsigned build
    /// (a fork, a local build) a copy with an intact signature.
    pub(super) struct Trust {
        pub team: Option<String>,
        pub gatekeeper: bool,
    }

    fn check(new: &Path, running: &Path, version: &str) -> Result<Trust, String> {
        let not_genuine = "The download is not a correctly signed Krate, so it was not installed.";
        let intact = Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(new)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !intact {
            return Err(not_genuine.to_string());
        }
        let id = info_value(new, "CFBundleIdentifier");
        if id.is_none() || id != info_value(running, "CFBundleIdentifier") {
            return Err("The download is a different app, so it was not installed.".to_string());
        }
        if let Some(v) = info_value(new, "CFBundleShortVersionString") {
            if compare_versions(&v, version) != std::cmp::Ordering::Equal
                && compare_versions(&v, version.split('-').next().unwrap_or(version))
                    != std::cmp::Ordering::Equal
            {
                return Err(format!(
                    "The download says it is Krate {v}, not {version}, so it was not installed."
                ));
            }
        }
        let team = team_of(running);
        if let Some(t) = &team {
            if team_of(new).as_deref() != Some(t.as_str()) {
                return Err(not_genuine.to_string());
            }
        }
        let gatekeeper = gatekeeper_accepts(running);
        if gatekeeper && !gatekeeper_accepts(new) {
            return Err(
                "The download is not notarized by Apple, so it was not installed.".to_string(),
            );
        }
        Ok(Trust { team, gatekeeper })
    }

    pub(super) fn hand_off(verified: &Path, version: &str, updates: &Path) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let running = bundle_of(&exe).ok_or_else(|| {
            format!(
                "This Krate is not an installed app (it runs from {}), so it cannot replace itself.",
                exe.display()
            )
        })?;
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let target = install_target(&running, &home);
        let parent = target
            .parent()
            .ok_or("Krate could not tell where it is installed.")?
            .to_path_buf();
        let writable = dir_writable(&parent);
        let pid = std::process::id();

        // An earlier Studio may have left this very image open in Finder; a
        // second attach of an attached image fails ("Resource busy").
        detach_ours(updates);

        let stage_root = if writable {
            parent.join(format!(".krate-update-{pid}"))
        } else {
            updates.join("stage")
        };
        let _ = std::fs::remove_dir_all(&stage_root);
        std::fs::create_dir_all(&stage_root).map_err(|e| {
            format!(
                "Krate could not prepare the update next to {}: {e}",
                target.display()
            )
        })?;
        let name = target.file_name().unwrap_or_else(|| "Krate.app".as_ref());
        let staged = stage_root.join(name);
        let prepared = (|| {
            {
                let image = Mounted::attach(verified)?;
                let app = image.single_app()?;
                let copied = Command::new("/usr/bin/ditto")
                    .arg(&app)
                    .arg(&staged)
                    .status()
                    .is_ok_and(|s| s.success());
                if !copied {
                    return Err(
                        "Krate could not copy the new version out of the download.".to_string()
                    );
                }
            } // detached here, whatever came before
            let _ = Command::new("/usr/bin/xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(&staged)
                .status();
            check(&staged, &running, version)
        })();
        let trust = match prepared {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&stage_root);
                return Err(e);
            }
        };

        let (feed, selftest, home_s) = forwarded();
        let args: Vec<String> = vec![
            pid.to_string(),
            staged.display().to_string(),
            target.display().to_string(),
            updates.display().to_string(),
            version.to_string(),
            trust.team.clone().unwrap_or_else(|| "-".to_string()),
            if trust.gatekeeper { "1" } else { "0" }.to_string(),
            String::new(),
            feed,
            selftest,
            home_s,
        ];

        if writable {
            use std::os::unix::process::CommandExt;
            Command::new("/bin/sh")
                .arg("-c")
                .arg(MAC_SWAP_SCRIPT)
                .arg("krate-update")
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                // Its own process group: nothing that ends Studio ends it.
                .process_group(0)
                .spawn()
                .map(|_| ())
                .map_err(|e| {
                    let _ = std::fs::remove_dir_all(&stage_root);
                    format!("Krate could not start the installer step: {e}")
                })
        } else {
            // /Applications belongs to an administrator: ask the way every
            // Mac app does, once, while Krate is still open to say why. The
            // helper then runs as root, copies the checked app into place
            // itself, checks that copy again, and opens the new Krate as
            // this person.
            let mut root_args = args;
            root_args[7] = unsafe { libc::getuid() }.to_string();
            let line = format!(
                "/bin/sh -c {} krate-update {} >/dev/null 2>&1 &",
                sh_quote(MAC_SWAP_SCRIPT),
                root_args
                    .iter()
                    .map(|a| sh_quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let apple = format!(
                "do shell script \"{}\" with prompt \"Krate needs an administrator to install version {} in {}.\" with administrator privileges",
                applescript_escape(&line),
                applescript_escape(version),
                applescript_escape(&parent.display().to_string()),
            );
            let out = Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(&apple)
                .output()
                .map_err(|e| format!("Krate could not ask for permission: {e}"))?;
            if out.status.success() {
                return Ok(());
            }
            let _ = std::fs::remove_dir_all(&stage_root);
            let err = String::from_utf8_lossy(&out.stderr);
            if err.contains("-128") {
                Err(format!(
                    "Cancelled. Krate stays as it is; installing into {} needs an administrator.",
                    parent.display()
                ))
            } else {
                Err(format!(
                    "Krate could not get permission to install into {}.",
                    parent.display()
                ))
            }
        }
    }

    fn applescript_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
}

#[cfg(all(test, target_os = "macos"))]
use mac::{attached_images, install_target, plist_strings};

/* ---- Linux ------------------------------------------------------------- */

/// After Studio quits: swap the new AppImage in, start it, wait for it, and
/// roll back if it does not start. `$1` Studio's pid, `$2` the new AppImage
/// (already beside the old one), `$3` the AppImage to replace, `$4` the
/// updates folder, `$5` the version.
pub const LINUX_SWAP_SCRIPT: &str = r##"
set -u
pid="$1"; new="$2"; target="$3"; updates="$4"; version="$5"
prev="$(dirname "$target")/.krate-previous-$pid.AppImage"
marker="$updates/started"; failed="$updates/failed.txt"; log="$updates/update.log"
ticks="${KRATE_UPDATE_WAIT_TICKS:-300}"; settle="${KRATE_UPDATE_SETTLE:-3}"
say() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*" >> "$log"; }
launch() {
  if command -v setsid >/dev/null 2>&1; then setsid "$1" >/dev/null 2>&1 </dev/null &
  else nohup "$1" >/dev/null 2>&1 </dev/null & fi
  lpid=$!
}
give_up() {
  printf '%s\n%s\n' "$version" "$1" > "$failed"; say "not installed: $1"
  rm -f "$new"
  [ -e "$target" ] && launch "$target"
  exit 1
}
lock="$updates/.installing"
find "$lock" -maxdepth 0 -mmin +10 -exec rmdir {} \; 2>/dev/null
mkdir "$lock" 2>/dev/null || { rm -f "$new"; exit 1; }
trap 'rmdir "$lock" 2>/dev/null' EXIT
say "installing $version over $target"
i=0
while kill -0 "$pid" 2>/dev/null; do
  [ $i -ge 150 ] && { rm -f "$new"; printf '%s\n%s\n' "$version" "Krate did not close, so the update was not installed." > "$failed"; exit 1; }
  sleep 0.2; i=$((i+1))
done
chmod 755 "$new" || give_up "Krate could not prepare the new version."
rm -f "$prev"
mv "$target" "$prev" || give_up "Krate could not move the old version aside."
if ! mv "$new" "$target"; then mv "$prev" "$target"; give_up "Krate could not move the new version into place."; fi
say "swapped in $version"
rm -f "$marker"
launch "$target"
ok=""; i=0
while [ $i -lt "$ticks" ]; do
  if [ -f "$marker" ] && [ "$(sed -n 1p "$marker")" = "$version" ]; then ok=1; break; fi
  sleep 0.2; i=$((i+1))
done
if [ -n "$ok" ]; then
  npid=$(sed -n 2p "$marker"); sleep "$settle"
  kill -0 "$npid" 2>/dev/null || ok=""
fi
if [ -n "$ok" ]; then
  rm -f "$prev" "$updates"/krate-studio-* "$updates/verified.json"
  say "done: $version started"
  exit 0
fi
say "$version did not start; putting the old version back"
kill -TERM -"$lpid" 2>/dev/null || kill -TERM "$lpid" 2>/dev/null; sleep 1
rm -f "$target"; mv "$prev" "$target"
give_up "Krate $version did not start, so Krate went back to the version you had."
"##;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub(super) fn hand_off(verified: &Path, version: &str, updates: &Path) -> Result<(), String> {
        // The AppImage runtime names the real file; current_exe is inside
        // its temporary mount.
        let Some(target) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
            let exe = std::env::current_exe().unwrap_or_default();
            return Err(if exe.starts_with("/usr") || exe.starts_with("/opt") {
                "This Krate was installed by your system's package manager, so it is updated there, not from inside Krate.".to_string()
            } else {
                "This Krate is not the Krate AppImage, so it cannot replace itself. Download the new one from krate.tech.".to_string()
            });
        };
        let dir = target.parent().ok_or("Krate could not tell where it is.")?;
        if !dir_writable(dir) {
            return Err(format!(
                "Krate cannot write to {}, where its AppImage is. Move Krate's AppImage into your home folder and update again.",
                dir.display()
            ));
        }
        let pid = std::process::id();
        let new = dir.join(format!(".krate-update-{pid}.AppImage"));
        std::fs::copy(verified, &new)
            .map_err(|e| format!("Krate could not copy the new version beside the old one: {e}"))?;
        use std::os::unix::process::CommandExt;
        Command::new("/bin/sh")
            .arg("-c")
            .arg(LINUX_SWAP_SCRIPT)
            .arg("krate-update")
            .arg(pid.to_string())
            .arg(&new)
            .arg(&target)
            .arg(updates)
            .arg(version)
            // The old AppImage's runtime settings must not reach the new one.
            .env_remove("APPDIR")
            .env_remove("APPIMAGE")
            .env_remove("ARGV0")
            .env_remove("OWD")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .map(|_| ())
            .map_err(|e| {
                let _ = std::fs::remove_file(&new);
                format!("Krate could not start the installer step: {e}")
            })
    }
}

/* ---- Windows ----------------------------------------------------------- */

/// After Studio quits: run the Krate installer silently over this install
/// (`/S /UPDATE`: no wizard, keep the place and the settings), start the new
/// Krate, wait for it, and put the old files back if it does not start. The
/// old install folder is renamed aside first, so a failed install never
/// leaves half of each version. `__PID__` and the quoted `__...__` values are
/// filled in by `windows_script`.
pub const WINDOWS_SWAP_SCRIPT: &str = r##"
$ErrorActionPreference = 'Continue'
$studioPid = __PID__; $setup = __SETUP__; $exe = __EXE__; $updates = __UPDATES__; $version = __VERSION__
$dir = Split-Path -Parent $exe
$prev = "$dir.krate-old"
$marker = Join-Path $updates 'started'; $failed = Join-Path $updates 'failed.txt'; $log = Join-Path $updates 'update.log'
function Say($m) { Add-Content -Path $log -Value ("{0:yyyy-MM-dd HH:mm:ss} {1}" -f (Get-Date), $m) }
function GiveUp($m) { Set-Content -Path $failed -Value @($version, $m); Say "not installed: $m"; if (Test-Path $exe) { Start-Process -FilePath $exe }; exit 1 }
Say "installing $version over $dir"
try { Wait-Process -Id $studioPid -Timeout 30 -ErrorAction SilentlyContinue } catch {}
if (Get-Process -Id $studioPid -ErrorAction SilentlyContinue) { Set-Content -Path $failed -Value @($version, 'Krate did not close, so the update was not installed.'); exit 1 }
$mine = Get-AuthenticodeSignature -FilePath $exe
if ($mine.Status -eq 'Valid') {
  $theirs = Get-AuthenticodeSignature -FilePath $setup
  if ($theirs.Status -ne 'Valid' -or $theirs.SignerCertificate.Subject -ne $mine.SignerCertificate.Subject) {
    GiveUp 'The download is not signed by the publisher of this Krate, so it was not installed.'
  }
}
try {
  if (Test-Path $prev) { Remove-Item -Recurse -Force $prev -ErrorAction Stop }
  Rename-Item -Path $dir -NewName (Split-Path -Leaf $prev) -ErrorAction Stop
} catch { GiveUp 'Part of Krate is still running (an app or a build), so it could not be updated. Close it and update again.' }
$ok = $false
try {
  $p = Start-Process -FilePath $setup -ArgumentList '/S', '/UPDATE' -Wait -PassThru -ErrorAction Stop
  $ok = ($p.ExitCode -eq 0) -and (Test-Path $exe)
  if (-not $ok) { Say "installer exit $($p.ExitCode)" }
} catch { Say "installer did not run: $_" }
if ($ok) {
  Say "installed $version"
  Remove-Item -Force $marker -ErrorAction SilentlyContinue
  Start-Process -FilePath $exe
  $ok = $false
  for ($i = 0; $i -lt 300; $i++) {
    if (Test-Path $marker) { $l = @(Get-Content $marker); if ($l[0] -eq $version) { $ok = $true; break } }
    Start-Sleep -Milliseconds 200
  }
  if ($ok) { Start-Sleep -Seconds 3; $ok = [bool](Get-Process -Id ([int]$l[1]) -ErrorAction SilentlyContinue) }
}
if ($ok) {
  Remove-Item -Recurse -Force $prev -ErrorAction SilentlyContinue
  Get-ChildItem -Path $updates -Filter 'krate-studio-*' | Remove-Item -Force -ErrorAction SilentlyContinue
  Remove-Item -Force (Join-Path $updates 'verified.json') -ErrorAction SilentlyContinue
  Say "done: $version started"
  exit 0
}
Say "$version did not start; putting the old version back"
Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($dir + '\', 'OrdinalIgnoreCase') } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue }
Rename-Item -Path $prev -NewName (Split-Path -Leaf $dir)
GiveUp "Krate $version did not start, so Krate went back to the version you had."
"##;

/// The Windows helper with its values in place, each a single-quoted
/// PowerShell literal (a quote inside is doubled).
pub fn windows_script(pid: u32, setup: &Path, exe: &Path, updates: &Path, version: &str) -> String {
    let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
    WINDOWS_SWAP_SCRIPT
        .replace("__PID__", &pid.to_string())
        .replace("__SETUP__", &lit(&setup.display().to_string()))
        .replace("__EXE__", &lit(&exe.display().to_string()))
        .replace("__UPDATES__", &lit(&updates.display().to_string()))
        .replace("__VERSION__", &lit(version))
}

#[cfg(windows)]
mod windows {
    use super::*;

    pub(super) fn hand_off(verified: &Path, version: &str, updates: &Path) -> Result<(), String> {
        use base64::Engine as _;
        use std::os::windows::process::CommandExt;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = exe
            .parent()
            .ok_or("Krate could not tell where it is installed.")?;
        // The Krate installer leaves its uninstaller beside the app. Without
        // it this copy came from somewhere else and the installer would put
        // the new version in a different place.
        if !dir.join("uninstall.exe").exists() {
            return Err("This Krate was not installed with the Krate installer, so it cannot update itself. Download the new one from krate.tech.".to_string());
        }
        let script = windows_script(std::process::id(), verified, &exe, updates, version);
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encoded,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: no console, and
            // nothing that ends Studio ends it.
            .creation_flags(0x0800_0000 | 0x0000_0200)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Krate could not start the installer step: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn installer_names_give_their_version_and_nothing_else_does() {
        assert_eq!(
            installer_version("krate-studio-0.6.1-universal.dmg"),
            Some("0.6.1")
        );
        assert_eq!(
            installer_version("krate-studio-0.6.2-test1-windows-x64-setup.exe"),
            Some("0.6.2-test1")
        );
        assert_eq!(
            installer_version("krate-studio-1.0.0-linux-x86_64.AppImage"),
            Some("1.0.0")
        );
        for other in [
            "verified.json",
            "krate-studio--universal.dmg",
            "Krate.dmg",
            "krate-0.6.1.zip",
        ] {
            assert_eq!(installer_version(other), None, "{other}");
        }
        assert_eq!(installer_version(&asset_name("9.8.7")), Some("9.8.7"));
    }

    #[test]
    fn versions_order_like_releases_do() {
        assert_eq!(compare_versions("0.6.2-test2", "0.6.2-test1"), Greater);
        assert_eq!(compare_versions("0.6.2-test10", "0.6.2-test9"), Greater);
        assert_eq!(compare_versions("0.6.2", "0.6.2-test9"), Greater);
        assert_eq!(compare_versions("v0.6.1", "0.6.1"), Equal);
        assert_eq!(compare_versions("0.6.1", "0.10.0"), Less);
        assert_eq!(compare_versions("garbage", "0.0.1"), Less);
    }

    #[test]
    fn the_started_note_and_a_failure_are_read_back_once() {
        let dir = tempfile::tempdir().unwrap();
        note_started(dir.path(), "0.6.2");
        let text = std::fs::read_to_string(dir.path().join("started")).unwrap();
        assert_eq!(text, format!("0.6.2\n{}\n", std::process::id()));

        assert_eq!(take_failed(dir.path()), None);
        std::fs::write(
            dir.path().join("failed.txt"),
            "0.6.2\nKrate 0.6.2 did not start.\n",
        )
        .unwrap();
        assert_eq!(
            take_failed(dir.path()),
            Some(("0.6.2".into(), "Krate 0.6.2 did not start.".into()))
        );
        assert_eq!(take_failed(dir.path()), None, "said once");
    }

    /// The founder's folder after two updates: three old disk images and a
    /// receipt. Starting as 0.6.2 keeps only what is newer than 0.6.2.
    #[test]
    fn a_start_deletes_downloads_it_no_longer_needs() {
        let dir = tempfile::tempdir().unwrap();
        let u = dir.path();
        for f in [
            "krate-studio-0.5.0-universal.dmg",
            "krate-studio-0.6.1-universal.dmg",
            "krate-studio-0.6.2-universal.dmg",
            "krate-studio-0.6.3-universal.dmg",
            "notes.txt",
            ".krate-studio-0.6.3-universal.dmg.99.partial",
        ] {
            std::fs::write(u.join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(u.join("stage/Krate.app")).unwrap();
        std::fs::write(
            u.join("verified.json"),
            r#"{"file": "krate-studio-0.6.1-universal.dmg", "sha256": "00"}"#,
        )
        .unwrap();

        cleanup_on_launch(u, "0.6.2");

        let mut left: Vec<String> = std::fs::read_dir(u)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                ".krate-studio-0.6.3-universal.dmg.99.partial", // fresh: a download may be running
                "krate-studio-0.6.3-universal.dmg",
                "notes.txt",
            ]
        );
    }

    #[test]
    fn only_old_staging_beside_the_app_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join(".krate-update-1/previous.app")).unwrap();
        std::fs::create_dir_all(d.join(".krate-update-2/previous.app")).unwrap();
        std::fs::write(d.join(".krate-previous-3.AppImage"), b"x").unwrap();
        std::fs::create_dir_all(d.join("Krate.app")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for n in [".krate-update-1", ".krate-previous-3.AppImage"] {
            std::fs::File::options()
                .write(true)
                .open(d.join(n))
                .or_else(|_| std::fs::File::open(d.join(n)))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        remove_stale_staging(d);
        let mut left: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [".krate-update-2", "Krate.app"],
            "the fresh one may be in use"
        );
    }

    #[test]
    fn the_windows_helper_gets_every_value_quoted() {
        let s = windows_script(
            42,
            Path::new(
                r"C:\Users\o'brien\.krate\studio\updates\krate-studio-0.6.2-windows-x64-setup.exe",
            ),
            Path::new(r"C:\Users\o'brien\AppData\Local\Krate\krate-studio.exe"),
            Path::new(r"C:\Users\o'brien\.krate\studio\updates"),
            "0.6.2",
        );
        assert!(!s.contains("__"), "a placeholder was left: {s}");
        assert!(s.contains("$studioPid = 42;"));
        assert!(s.contains(r"$exe = 'C:\Users\o''brien\AppData\Local\Krate\krate-studio.exe'"));
        assert!(s.contains("'/S', '/UPDATE'"), "silent, in place");
        // Never a wizard, never the installer opened for the person.
        assert!(!s.contains("/P'") && !s.to_lowercase().contains("explorer"));
    }

    #[test]
    fn shell_quoting_survives_quotes() {
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "printf %s {}",
                sh_quote("it's a \"path\" $HOME `x`")
            ))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "it's a \"path\" $HOME `x`"
        );
    }

    /* ---- the helper scripts, run for real against stand-ins ------------ */

    #[cfg(unix)]
    fn dead_pid() -> String {
        let mut c = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid.to_string()
    }

    #[cfg(unix)]
    fn write_exec(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A program that does what a Studio of `version` does at start: write
    /// the started note, then stay up. `starts = false` is a broken build that
    /// dies before it gets that far.
    #[cfg(unix)]
    fn studio_body(updates: &Path, version: &str, starts: bool, ran: &Path) -> String {
        let up = updates.display();
        let ran = ran.display();
        if starts {
            format!("echo {version} >> '{ran}'\nprintf '%s\\n%s\\n' {version} $$ > '{up}/started'\nsleep 6")
        } else {
            format!("echo {version}-crashed >> '{ran}'\nexit 3")
        }
    }

    #[cfg(unix)]
    fn wait_for(path: &Path, needle: &str) -> String {
        for _ in 0..150 {
            if let Ok(t) = std::fs::read_to_string(path) {
                if t.contains(needle) {
                    return t;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "{} never said {needle:?}: {:?}",
            path.display(),
            std::fs::read_to_string(path)
        );
    }

    #[cfg(unix)]
    fn run_linux(dir: &Path, new_starts: bool) -> (PathBuf, PathBuf, PathBuf) {
        let updates = dir.join("updates");
        std::fs::create_dir_all(&updates).unwrap();
        std::fs::write(
            updates.join("krate-studio-0.6.2-linux-x86_64.AppImage"),
            b"dl",
        )
        .unwrap();
        std::fs::write(updates.join("verified.json"), b"{}").unwrap();
        let ran = dir.join("ran.txt");
        let target = dir.join("apps/Krate.AppImage");
        write_exec(&target, &studio_body(&updates, "0.6.1", true, &ran));
        let new = dir.join("apps/.krate-update-1.AppImage");
        write_exec(&new, &studio_body(&updates, "0.6.2", new_starts, &ran));
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(LINUX_SWAP_SCRIPT)
            .arg("krate-update")
            .args([dead_pid().as_str()])
            .arg(&new)
            .arg(&target)
            .arg(&updates)
            .arg("0.6.2")
            .env("KRATE_UPDATE_WAIT_TICKS", "25")
            .env("KRATE_UPDATE_SETTLE", "1")
            .status()
            .unwrap();
        assert_eq!(status.success(), new_starts);
        (updates, target, ran)
    }

    #[cfg(unix)]
    #[test]
    fn linux_swaps_the_appimage_and_cleans_up_once_the_new_one_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (updates, target, ran) = run_linux(dir.path(), true);
        assert!(std::fs::read_to_string(&target).unwrap().contains("0.6.2"));
        let left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("Krate.AppImage")],
            "no copies left beside it"
        );
        assert!(!updates
            .join("krate-studio-0.6.2-linux-x86_64.AppImage")
            .exists());
        assert!(!updates.join("verified.json").exists());
        assert!(!updates.join("failed.txt").exists());
        assert_eq!(std::fs::read_to_string(ran).unwrap(), "0.6.2\n");
    }

    #[cfg(unix)]
    #[test]
    fn linux_puts_the_old_appimage_back_when_the_new_one_does_not_start() {
        let dir = tempfile::tempdir().unwrap();
        let (updates, target, ran) = run_linux(dir.path(), false);
        assert!(
            std::fs::read_to_string(&target).unwrap().contains("0.6.1"),
            "old one back"
        );
        let failed = std::fs::read_to_string(updates.join("failed.txt")).unwrap();
        assert_eq!(
            failed,
            "0.6.2\nKrate 0.6.2 did not start, so Krate went back to the version you had.\n"
        );
        // The old one was opened again for the person.
        wait_for(&ran, "0.6.1");
        let left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("Krate.AppImage")]);
    }

    #[cfg(target_os = "macos")]
    mod mac_tests {
        use super::*;

        /// A real signed bundle (ad hoc) whose program is `body`.
        fn bundle(app: &Path, version: &str, body: &str) {
            write_exec(&app.join("Contents/MacOS/Krate"), body);
            std::fs::write(
                app.join("Contents/Info.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                     <key>CFBundleExecutable</key><string>Krate</string>\
                     <key>CFBundleIdentifier</key><string>dev.krate.studio.test</string>\
                     <key>CFBundleShortVersionString</key><string>{version}</string>\
                     <key>CFBundlePackageType</key><string>APPL</string></dict></plist>\n"
                ),
            )
            .unwrap();
            let ok = Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(app)
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "{}",
                String::from_utf8_lossy(&ok.stderr)
            );
        }

        struct Run {
            updates: PathBuf,
            target: PathBuf,
            ran: PathBuf,
            ok: bool,
        }

        fn run_mac(dir: &Path, starts: bool, tamper: bool) -> Run {
            let updates = dir.join("updates");
            std::fs::create_dir_all(&updates).unwrap();
            std::fs::write(updates.join("krate-studio-0.6.2-universal.dmg"), b"dl").unwrap();
            let ran = dir.join("ran.txt");
            let apps = dir.join("Applications");
            let target = apps.join("Krate.app");
            bundle(
                &target,
                "0.6.1",
                &studio_body(&updates, "0.6.1", true, &ran),
            );
            let pid = dead_pid();
            let new = apps.join(format!(".krate-update-{pid}/Krate.app"));
            bundle(&new, "0.6.2", &studio_body(&updates, "0.6.2", starts, &ran));
            if tamper {
                std::fs::write(new.join("Contents/MacOS/Krate"), "#!/bin/sh\necho evil\n").unwrap();
            }
            // `open` would go through Launch Services; the stand-in runs the
            // bundle's program directly, in the background, the same way.
            let opener = dir.join("open.sh");
            write_exec(
                &opener,
                "\"$1/Contents/MacOS/Krate\" >/dev/null 2>&1 </dev/null &",
            );
            let status = Command::new("/bin/sh")
                .arg("-c")
                .arg(MAC_SWAP_SCRIPT)
                .arg("krate-update")
                .args([pid.as_str()])
                .arg(&new)
                .arg(&target)
                .arg(&updates)
                .args(["0.6.2", "-", "0", "", "", "", ""])
                .env("KRATE_UPDATE_WAIT_TICKS", "25")
                .env("KRATE_UPDATE_SETTLE", "1")
                .env("KRATE_UPDATE_OPEN", &opener)
                .status()
                .unwrap();
            Run {
                updates,
                target,
                ran,
                ok: status.success(),
            }
        }

        fn version_of(app: &Path) -> String {
            let text = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
            plist_strings(&text, "CFBundleShortVersionString").remove(0)
        }

        fn only_the_app_left(apps: &Path) {
            let left: Vec<_> = std::fs::read_dir(apps)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(
                left,
                vec![std::ffi::OsString::from("Krate.app")],
                "nothing left beside it"
            );
        }

        #[test]
        fn a_mac_update_swaps_the_bundle_and_cleans_up_once_the_new_one_starts() {
            let dir = tempfile::tempdir().unwrap();
            let r = run_mac(dir.path(), true, false);
            assert!(r.ok);
            assert_eq!(version_of(&r.target), "0.6.2");
            only_the_app_left(r.target.parent().unwrap());
            assert!(
                !r.updates.join("krate-studio-0.6.2-universal.dmg").exists(),
                "download deleted"
            );
            assert_eq!(
                std::fs::read_to_string(&r.ran).unwrap(),
                "0.6.2\n",
                "only the new one ran"
            );
            let log = std::fs::read_to_string(r.updates.join("update.log")).unwrap();
            assert!(log.contains("done: 0.6.2 started"), "{log}");
        }

        #[test]
        fn a_mac_update_that_does_not_start_puts_the_old_bundle_back() {
            let dir = tempfile::tempdir().unwrap();
            let r = run_mac(dir.path(), false, false);
            assert!(!r.ok);
            assert_eq!(version_of(&r.target), "0.6.1");
            only_the_app_left(r.target.parent().unwrap());
            assert_eq!(
                std::fs::read_to_string(r.updates.join("failed.txt")).unwrap(),
                "0.6.2\nKrate 0.6.2 did not start, so Krate went back to the version you had.\n"
            );
            wait_for(&r.ran, "0.6.1");
        }

        #[test]
        fn a_mac_update_with_a_broken_signature_is_never_swapped_in() {
            let dir = tempfile::tempdir().unwrap();
            let r = run_mac(dir.path(), true, true);
            assert!(!r.ok);
            assert_eq!(version_of(&r.target), "0.6.1", "untouched");
            only_the_app_left(r.target.parent().unwrap());
            let failed = std::fs::read_to_string(r.updates.join("failed.txt")).unwrap();
            assert!(
                failed.contains("did not pass the Mac's signature check"),
                "{failed}"
            );
            // The person is not left with nothing open.
            wait_for(&r.ran, "0.6.1");
        }

        /// The block shapes of `hdiutil info` on macOS 27, with the
        /// founder's two update images still attached from Studio 0.6.0.
        #[test]
        fn attached_update_images_are_found_with_their_whole_disk() {
            let info = "framework       : 1.0\ndriver          : 1.0\n\
================================================\n\
image-path      : /Users/p/.krate/studio/updates/krate-studio-0.6.0-universal.dmg\n\
image-alias     : /Users/p/.krate/studio/updates/krate-studio-0.6.0-universal.dmg\n\
process ID      : 35715\n\
/dev/disk6\tGUID_partition_scheme\t\n\
/dev/disk6s1\t7C3457EF-0000-11AA-AA11-00306543ECAC\t\n\
/dev/disk7\tEF57347C-0000-11AA-AA11-00306543ECAC\t\n\
/dev/disk7s1\t41504653-0000-11AA-AA11-00306543ECAC\t/Volumes/Krate\n\
================================================\n\
image-path      : /Users/p/.krate/studio/updates/krate-studio-0.6.1-universal.dmg\n\
/dev/disk8\tGUID_partition_scheme\t\n\
/dev/disk9s1\t41504653-0000-11AA-AA11-00306543ECAC\t/Volumes/Krate 1\n";
            let found = attached_images(info);
            assert_eq!(found.len(), 2);
            assert_eq!(found[0].1, "/dev/disk6");
            assert_eq!(found[0].2, vec![PathBuf::from("/Volumes/Krate")]);
            assert_eq!(found[1].1, "/dev/disk8");
            assert_eq!(found[1].2, vec![PathBuf::from("/Volumes/Krate 1")]);
        }

        #[test]
        fn a_krate_run_from_downloads_or_the_disk_image_installs_into_applications() {
            let home = Path::new("/Users/nobody-here");
            let translocated =
                Path::new("/private/var/folders/xy/T/AppTranslocation/0A1B/d/Krate.app");
            assert_eq!(
                install_target(translocated, home),
                Path::new("/Applications/Krate.app")
            );
            let dmg = Path::new("/Volumes/Krate-not-mounted-here/Krate.app");
            assert_eq!(
                install_target(dmg, home),
                Path::new("/Applications/Krate.app")
            );
            let normal = Path::new("/Applications/Krate.app");
            assert_eq!(install_target(normal, home), normal);
            let mine = Path::new("/Users/nobody-here/Applications/Krate.app");
            assert_eq!(install_target(mine, home), mine);
        }

        #[test]
        fn plist_strings_reads_hdiutil_answers() {
            let t = "<dict><key>dev-entry</key>\n\t<string>/dev/disk10</string><key>mount-point</key>\t<string>/tmp/m</string></dict>\
                     <dict><key>dev-entry</key><string>/dev/disk10s1</string></dict>";
            assert_eq!(
                plist_strings(t, "dev-entry"),
                ["/dev/disk10", "/dev/disk10s1"]
            );
            assert_eq!(plist_strings(t, "mount-point"), ["/tmp/m"]);
        }
    }
}
