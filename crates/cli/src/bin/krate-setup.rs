//! The Windows Player installer: a receiver's whole setup, by double-click.
//!
//! Windows was the one platform with no artifact a non-developer could use.
//! macOS ships `Krate Player.app` -- double-click it and you are done. Windows
//! shipped a zip of loose binaries that registers nothing, so a person who was
//! sent a `.krate` had to unzip it somewhere permanent, find a terminal, and
//! run a PowerShell script by hand to make double-click work. The `/open` page
//! sent them to the Studio installer instead, which works but is the wrong
//! product and four times the size a receiver needs (K-214).
//!
//! This is the missing piece: one `.exe` that carries the runtime, puts it
//! somewhere stable, registers the file type, and says so. No terminal, no
//! admin rights, no choices to make.
//!
//! ## What it installs, and what it deliberately leaves out
//!
//! `krate.exe` and `krate-open.exe` only. NOT `cargo-component.exe`, which is
//! 12 MB of the 47 the full archive weighs and is a tool for BUILDING apps.
//! Someone opening a file they were sent never runs it. That is the same line
//! macOS draws -- `Krate Player.app` is receive-only too -- and it is what
//! makes the honest download 16 MB rather than 19.
//!
//! Someone who later wants to make apps installs the CLI or Studio, both of
//! which carry the build tooling.
//!
//! ## Why a Rust binary rather than NSIS
//!
//! It cross-compiles to both Windows targets with everything else in the
//! release, so there is no second toolchain in CI to keep working -- and no
//! makensis to find on a runner. It is also the same shape as
//! `krate-open.exe`: `windows_subsystem = "windows"` so no console flashes up,
//! and message boxes because with no console there is nowhere else to speak.
//!
//! ## Where the payload comes from
//!
//! `build.rs` writes the two binaries into `OUT_DIR` and this includes them.
//! When they are absent -- an ordinary `cargo build` of the workspace on any
//! platform -- the payload is empty and the installer says so rather than
//! silently producing a broken installer that ships nothing.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod installer {
    use std::path::{Path, PathBuf};

    /// The two binaries a receiver needs, staged by build.rs.
    ///
    /// Empty in a normal build; the release job stages them first. The check in
    /// `run` turns that into an explicit refusal instead of an installer that
    /// cheerfully writes zero bytes.
    const KRATE_EXE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload/krate.exe"));
    const KRATE_OPEN_EXE: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/payload/krate-open.exe"));
    const DOC_ICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload/KrateDoc.ico"));

    /// The association logic, embedded rather than reimplemented.
    ///
    /// This script is the single source of truth for what a `.krate`
    /// association looks like, and it already handles the case that matters:
    /// when Krate Studio owns the extension and is really installed, it leaves
    /// it alone instead of two installers fighting over one registry key
    /// (K-166). Rewriting that logic in Rust would mean two codepaths to keep
    /// in agreement, and the one that drifts is the one nobody runs.
    const DESKTOP_SCRIPT: &str = include_str!("../../../../scripts/install-krate-desktop.ps1");

    pub fn run() {
        if KRATE_EXE.is_empty() || KRATE_OPEN_EXE.is_empty() {
            say(
                "Krate Setup",
                "This installer was built without its payload, so there is \
                 nothing to install.\n\nDownload the installer from krate.tech \
                 rather than building it from source.",
            );
            return;
        }

        match install() {
            Ok(dir) => {
                say(
                    "Krate is ready",
                    &format!(
                        "Krate is installed.\n\nDouble-click any .krate file to \
                         open it. The app will ask what it may do before any of \
                         it runs.\n\nInstalled in:\n{}",
                        dir.display()
                    ),
                );
            }
            Err(err) => {
                say(
                    "Krate Setup",
                    &format!("Could not finish installing.\n\n{err}"),
                );
            }
        }
    }

    fn install() -> Result<PathBuf, String> {
        // %LOCALAPPDATA%\Krate: per-user, so no administrator prompt, and a
        // stable home the association can point at forever. A receiver who
        // unzipped to Downloads had the double-click break the moment they
        // tidied up; this cannot.
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("Windows did not say where %LOCALAPPDATA% is.")?;
        let dir = base.join("Krate");

        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;

        write_file(&dir.join("krate.exe"), KRATE_EXE)?;
        write_file(&dir.join("krate-open.exe"), KRATE_OPEN_EXE)?;
        if !DOC_ICON.is_empty() {
            // Best effort: a missing icon means Explorer draws a blank page for
            // a .krate, which is ugly but not broken.
            let _ = std::fs::write(dir.join("KrateDoc.ico"), DOC_ICON);
        }

        register(&dir)?;
        add_to_path(&dir);
        Ok(dir)
    }

    /// Replace a file that may be running.
    ///
    /// Re-running the installer to update is the ordinary case, and Windows
    /// refuses to overwrite a binary that is open. Renaming the old one out of
    /// the way is allowed even while it runs, so the write succeeds and the
    /// stale copy goes on the next reboot.
    fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
        if path.exists() {
            let stale = path.with_extension("exe.old");
            let _ = std::fs::remove_file(&stale);
            let _ = std::fs::rename(path, &stale);
        }
        std::fs::write(path, bytes).map_err(|e| {
            format!(
                "Could not write {}: {e}\n\nIf Krate is open, close it and run \
                 this again.",
                path.display()
            )
        })
    }

    /// Register the `.krate` association by running the shared script.
    fn register(dir: &Path) -> Result<(), String> {
        let script = dir.join("install-krate-desktop.ps1");
        std::fs::write(&script, DESKTOP_SCRIPT)
            .map_err(|e| format!("Could not stage the association script: {e}"))?;

        let binary = dir.join("krate.exe");
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg("-KrateBinary")
            .arg(&binary)
            .status()
            .map_err(|e| format!("Could not run PowerShell: {e}"))?;

        // Leave the script behind: the uninstall path (`-Uninstall`) needs it,
        // and it is 5 KB.
        if !status.success() {
            return Err(format!(
                "The file association step failed (exit code {}).\n\nKrate \
                 itself is installed in {} and works from a terminal.",
                status.code().unwrap_or(-1),
                dir.display()
            ));
        }
        Ok(())
    }

    /// Put the install directory on the user's PATH.
    ///
    /// Not required to open an app -- the association carries an absolute path
    /// -- so a failure here is not worth failing the install over. It is what
    /// makes `krate` work in a terminal afterwards, which is the difference
    /// between a receiver who stays a receiver and one who tries building
    /// something.
    fn add_to_path(dir: &Path) {
        let want = dir.display().to_string();
        // Compared entry by entry, the same way install.ps1 does it: a path is
        // matched as a whole segment, so C:\Krate does not count as present
        // because C:\Krate\extra is.
        let already = std::env::var("PATH").is_ok_and(|p| {
            p.split(';').any(|e| {
                e.trim_end_matches('\\')
                    .eq_ignore_ascii_case(want.trim_end_matches('\\'))
            })
        });
        if already {
            return;
        }
        // Through PowerShell rather than the registry directly: SetEnvironment
        // Variable broadcasts WM_SETTINGCHANGE, so new terminals see it without
        // a sign-out. A raw registry write does not, and the person is told
        // PATH is set while every new shell disagrees.
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(format!(
                "$p=[Environment]::GetEnvironmentVariable('Path','User'); \
                 if (-not ($p -split ';' | Where-Object {{ $_.TrimEnd('\\') -ieq '{}' }})) {{ \
                   [Environment]::SetEnvironmentVariable('Path', (($p -split ';' | \
                   Where-Object {{ $_ }}) + '{}' -join ';'), 'User') }}",
                want.trim_end_matches('\\').replace('\'', "''"),
                want.replace('\'', "''")
            ))
            .status();
    }

    /// Say something. With no console attached this is the only way to speak.
    fn say(title: &str, text: &str) {
        use std::os::windows::ffi::OsStrExt;

        fn wide(text: &str) -> Vec<u16> {
            std::ffi::OsStr::new(text)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }

        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }

        let text = wide(text);
        let title = wide(title);
        // MB_ICONINFORMATION.
        const KIND: u32 = 0x0000_0040;
        // SAFETY: both strings are null-terminated UTF-16 that outlive the call.
        unsafe {
            MessageBoxW(0, text.as_ptr(), title.as_ptr(), KIND);
        }
    }
}

#[cfg(windows)]
fn main() {
    installer::run();
}

/// Nothing to do off Windows: macOS has `Krate Player.app` and Linux has a
/// `.desktop` entry, both of which already open a bundle without a terminal.
#[cfg(not(windows))]
fn main() {
    eprintln!("krate-setup is the Windows Player installer; use install.sh here.");
    std::process::exit(1);
}
