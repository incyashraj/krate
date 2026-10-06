//! Installing the player from a gift (IC-381, IC-383).
//!
//! A friend who was sent a Krate app has no Krate. The gift carries the
//! sender's own engine -- on a Mac the notarized universal binary that ships
//! inside Krate Studio -- and the gift's opener runs `krate player-install`
//! from that copy. This puts the engine somewhere the person owns and
//! registers `.krate` with the desktop, so the app opens now and every later
//! `.krate` opens on a double-click:
//!
//! - macOS: `~/Applications/Krate Player.app`, the same bundle the release
//!   builds (scripts/make-macos-app.sh), assembled here around a copy of this
//!   binary and registered with Launch Services. It is written on this
//!   machine, so it carries no quarantine and Gatekeeper has nothing to ask;
//!   the engine inside keeps its own Developer ID signature, which the opener
//!   checked before running it.
//! - Linux: `~/.local/share/krate/bin/krate`, a `krate` on `~/.local/bin`,
//!   and the per-user MIME type, launcher and icon that
//!   scripts/install-krate-desktop.sh writes.
//!
//! No administrator password on either, and nothing is downloaded: the bytes
//! that run are the bytes the opener verified (IC-381 forbids running what a
//! server sends). Windows waits for a code-signing certificate (K-212).

use anyhow::{Context, Result};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux"))]
const APP_ICON_PNG: &[u8] = include_bytes!("../../../docs/landing/krate-app-icon.png");
#[cfg(any(target_os = "macos", target_os = "linux"))]
const DOC_ICON_PNG: &[u8] = include_bytes!("../../../docs/landing/krate-document-icon.png");

/// `krate player-install [--home DIR]`: install this binary as the player.
///
/// `home` stands in for the person's home folder (tests, and nothing else).
/// Prints `Player: <path to the engine>` last, which the gift openers read.
pub(crate) fn install(home: Option<&Path>) -> Result<u8> {
    let home = match home {
        Some(h) => h.to_path_buf(),
        None => crate::home_dir().context("could not find your home folder")?,
    };
    let source = std::env::current_exe().context("could not find this program")?;
    let engine = install_into(&source, &home)?;
    println!("Player: {}", engine.display());
    Ok(0)
}

#[cfg(target_os = "macos")]
fn install_into(source: &Path, home: &Path) -> Result<PathBuf> {
    let apps = home.join("Applications");
    let app = apps.join("Krate Player.app");
    let engine = app.join("Contents/MacOS/krate-cli");
    // Already there and at least as new: leave it, and use it. A gift made
    // with an older Krate must never downgrade a player someone updated.
    if let Some(have) = installed_version(&engine) {
        if !newer(env!("CARGO_PKG_VERSION"), &have) {
            return Ok(engine);
        }
    }
    fs::create_dir_all(&apps).with_context(|| format!("could not create {}", apps.display()))?;
    // Built beside the destination and swapped in whole, so there is never a
    // half-written player for Launch Services to find.
    let staging = apps.join(".Krate Player.app.installing");
    let _ = fs::remove_dir_all(&staging);
    let macos = staging.join("Contents/MacOS");
    let resources = staging.join("Contents/Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::copy(source, macos.join("krate-cli")).context("could not copy the player")?;
    make_executable(&macos.join("krate-cli"))?;
    // The opener already checked this binary's signature; the copy must not
    // carry the download's quarantine flag into a place it will run from.
    let _ = std::process::Command::new("xattr")
        .args(["-d", "com.apple.quarantine"])
        .arg(macos.join("krate-cli"))
        .stderr(std::process::Stdio::null())
        .status();
    // Launch Services starts CFBundleExecutable with no arguments, so a shim
    // starts the engine in open-app mode (scripts/make-macos-app.sh).
    fs::write(
        macos.join("Krate"),
        "#!/bin/sh\nexec \"$(dirname \"$0\")/krate-cli\" open-app\n",
    )?;
    make_executable(&macos.join("Krate"))?;
    let icons =
        icns(&resources, "Krate", APP_ICON_PNG) && icns(&resources, "KrateDoc", DOC_ICON_PNG);
    fs::write(staging.join("Contents/Info.plist"), player_plist(icons))?;
    if app.exists() {
        fs::remove_dir_all(&app).with_context(|| format!("could not replace {}", app.display()))?;
    }
    fs::rename(&staging, &app).with_context(|| format!("could not install {}", app.display()))?;
    // Only for the real home: a test's stand-in player must never become
    // the .krate handler of the machine the test runs on.
    if !home_is_real(home) {
        return Ok(engine);
    }
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/\
                      Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(lsregister)
        .arg("-f")
        .arg(&app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(engine)
}

/// An .icns built from one PNG with the tools every Mac has. False when it
/// could not be made, and the plist then names no icon rather than a missing
/// file (a missing icon draws every .krate as a broken page).
#[cfg(target_os = "macos")]
fn icns(resources: &Path, name: &str, png: &[u8]) -> bool {
    let work = resources.join(format!(".{name}.iconset"));
    let _ = fs::remove_dir_all(&work);
    if fs::create_dir_all(&work).is_err() {
        return false;
    }
    let src = work.join("src.png");
    if fs::write(&src, png).is_err() {
        return false;
    }
    let mut ok = true;
    for size in [16u32, 32, 128, 256, 512] {
        for (scale, px) in [("", size), ("@2x", size * 2)] {
            let out = work.join(format!("icon_{size}x{size}{scale}.png"));
            ok &= std::process::Command::new("sips")
                .args(["-z", &px.to_string(), &px.to_string()])
                .arg(&src)
                .arg("--out")
                .arg(&out)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }
    }
    let _ = fs::remove_file(&src);
    let set = resources.join(format!("{name}.iconset"));
    let _ = fs::remove_dir_all(&set);
    ok = ok && fs::rename(&work, &set).is_ok();
    ok = ok
        && std::process::Command::new("iconutil")
            .args(["-c", "icns"])
            .arg(&set)
            .arg("-o")
            .arg(resources.join(format!("{name}.icns")))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    let _ = fs::remove_dir_all(&set);
    let _ = fs::remove_dir_all(&work);
    ok
}

/// Krate Player's Info.plist: the one scripts/make-macos-app.sh writes, so
/// a player installed from a gift and one installed from the release are
/// the same app to Launch Services (identifier, document type, UTI).
#[cfg(target_os = "macos")]
fn player_plist(icons: bool) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let (app_icon, doc_icon) = if icons {
        (
            "    <key>CFBundleIconFile</key>\n    <string>Krate</string>\n",
            "            <key>CFBundleTypeIconFile</key>\n            <string>KrateDoc</string>\n",
        )
    } else {
        ("", "")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Krate Player</string>
    <key>CFBundleDisplayName</key>
    <string>Krate Player</string>
    <key>CFBundleIdentifier</key>
    <string>dev.krate.app</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>Krate</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>Krate uses the microphone only for apps that request audio capture and only after you allow it.</string>
    <key>NSCameraUsageDescription</key>
    <string>Krate uses the camera only for apps that request camera access and only after you allow it.</string>
{app_icon}    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Krate App Bundle</string>
{doc_icon}            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>dev.krate.bundle</string>
            </array>
            <key>LSHandlerRank</key>
            <string>Owner</string>
        </dict>
    </array>
    <key>UTExportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>dev.krate.bundle</string>
            <key>UTTypeDescription</key>
            <string>Krate App Bundle</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.data</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>krate</string>
                </array>
            </dict>
        </dict>
    </array>
</dict>
</plist>
"#
    )
}

#[cfg(target_os = "linux")]
fn install_into(source: &Path, home: &Path) -> Result<PathBuf> {
    let data = match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() && home_is_real(home) => PathBuf::from(d),
        _ => home.join(".local/share"),
    };
    let bin_dir = data.join("krate/bin");
    let engine = bin_dir.join("krate");
    let fresh = match installed_version(&engine) {
        Some(have) => newer(env!("CARGO_PKG_VERSION"), &have),
        None => true,
    };
    if fresh {
        fs::create_dir_all(&bin_dir)?;
        let staging = bin_dir.join(".krate.installing");
        fs::copy(source, &staging).context("could not copy the player")?;
        make_executable(&staging)?;
        fs::rename(&staging, &engine)?;
    }
    // `krate` in a terminal too, unless one is already there (a curl install
    // or a package manager's): theirs stays theirs.
    let local_bin = home.join(".local/bin");
    let link = local_bin.join("krate");
    if fs::symlink_metadata(&link).is_err() {
        let _ = fs::create_dir_all(&local_bin);
        let _ = std::os::unix::fs::symlink(&engine, &link);
    }
    register_linux(&data, &engine, home_is_real(home))?;
    Ok(engine)
}

/// The per-user `.krate` handler: what scripts/install-krate-desktop.sh does,
/// pointed at the installed engine.
#[cfg(target_os = "linux")]
fn register_linux(data: &Path, engine: &Path, real_home: bool) -> Result<()> {
    let mime = "application/x-krate";
    let desktop_name = "dev.krate.open.desktop";
    let icon_name = "application-x-krate";
    let apps = data.join("applications");
    let mime_dir = data.join("mime");
    let doc_icons = data.join("icons/hicolor/512x512/mimetypes");
    let app_icons = data.join("icons/hicolor/512x512/apps");
    for dir in [&apps, &mime_dir.join("packages"), &doc_icons, &app_icons] {
        fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    }
    fs::write(
        mime_dir.join("packages/krate.xml"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="{mime}">
    <comment>Krate app bundle</comment>
    <glob pattern="*.krate"/>
    <icon name="{icon_name}"/>
  </mime-type>
</mime-info>
"#
        ),
    )?;
    // A path with a space must be quoted in Exec, or the launcher runs the
    // first word of it.
    let exec = engine.display().to_string().replace('"', "\\\"");
    let desktop = apps.join(desktop_name);
    fs::write(
        &desktop,
        format!(
            "[Desktop Entry]\nType=Application\nName=Krate\n\
             Comment=Open a Krate app after reviewing what it can access\n\
             Exec=\"{exec}\" run %f --consent\nIcon=krate\nTerminal=false\nNoDisplay=true\n\
             MimeType={mime};\nCategories=Utility;\n"
        ),
    )?;
    make_executable(&desktop)?;
    fs::write(doc_icons.join(format!("{icon_name}.png")), DOC_ICON_PNG)?;
    fs::write(app_icons.join("krate.png"), APP_ICON_PNG)?;
    // A test's stand-in home stops here: xdg-mime writes the REAL user's
    // mimeapps.list whatever the data folder is.
    if !real_home {
        return Ok(());
    }
    // Best effort: a desktop without these tools still gets the files, and
    // the next login or file-manager start reads them.
    let quiet = |cmd: &str, args: &[&std::ffi::OsStr]| {
        let _ = std::process::Command::new(cmd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    quiet("update-mime-database", &[mime_dir.as_os_str()]);
    quiet("update-desktop-database", &[apps.as_os_str()]);
    quiet(
        "xdg-mime",
        &["default".as_ref(), desktop_name.as_ref(), mime.as_ref()],
    );
    quiet(
        "gtk-update-icon-cache",
        &["-f".as_ref(), data.join("icons/hicolor").as_os_str()],
    );
    Ok(())
}

/// XDG_DATA_HOME belongs to the real home; a `--home` stand-in (tests) must
/// not write into the real one.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home_is_real(home: &Path) -> bool {
    // KRATE_PLAYER_NO_REGISTER: a test that drives a whole gift with HOME
    // pointed at a scratch folder must not register that scratch player
    // with the desktop either.
    if std::env::var_os("KRATE_PLAYER_NO_REGISTER").is_some() {
        return false;
    }
    crate::home_dir().map(|h| h == home).unwrap_or(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_into(_source: &Path, _home: &Path) -> Result<PathBuf> {
    anyhow::bail!("installing the player from a gift is not available on this system yet")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

/// The version an installed engine reports, if it runs at all.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn installed_version(engine: &Path) -> Option<String> {
    if !engine.is_file() {
        return None;
    }
    let out = std::process::Command::new(engine)
        .arg("--version")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "krate v0.5.4" or "krate 0.5.4"
    let word = text.split_whitespace().nth(1)?;
    Some(word.trim_start_matches('v').to_string())
}

/// Whether version `a` is newer than `b` (numeric, dot by dot; anything
/// after a `-` is ignored, so an rc is as new as its release).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('-')
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_compares_numbers_not_text() {
        assert!(newer("0.5.10", "0.5.9"));
        assert!(!newer("0.5.9", "0.5.10"));
        assert!(!newer("0.5.4", "0.5.4"));
        assert!(!newer("0.5.4-rc1", "0.5.4"));
        assert!(newer("1.0.0", "0.9.99"));
    }
}
