//! The Windows double-click entry point: open a `.krate` without a console.
//!
//! `krate.exe` is a console application, which is right for a terminal and
//! wrong for Explorer: double-clicking a `.krate` popped a black console window
//! next to the app's own window and left it there for the whole session. macOS
//! solves this with Krate.app; this is the same idea, one binary rather than a
//! bundle.
//!
//! `windows_subsystem = "windows"` is the whole trick -- it tells the loader
//! not to allocate a console. Everything else is handing the path to the same
//! `krate run` that the terminal uses, so there is one runner and not two that
//! can drift apart.
//!
//! Failures have to be shown in a message box rather than printed: with no
//! console there is nowhere for a message to go, and an app that fails
//! silently on double-click is indistinguishable from one that does nothing.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    let Some(bundle) = std::env::args_os().nth(1) else {
        message_box(
            "Krate",
            "Open a .krate file with this, or run `krate` in a terminal.",
        );
        return;
    };

    // The console `krate.exe` sits beside this one; a release places them
    // together and so does a build. Resolving by neighbour rather than by PATH
    // is deliberate: a PATH lookup can find an older installed Krate, which is
    // the trap that has cost real time before.
    let Ok(here) = std::env::current_exe() else {
        message_box("Krate", "Could not work out where Krate is installed.");
        return;
    };
    let runner = here.with_file_name("krate.exe");
    if !runner.exists() {
        message_box(
            "Krate",
            &format!(
                "krate.exe is missing from {}.\n\nReinstall Krate from krate.tech.",
                here.parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
        );
        return;
    }

    match std::process::Command::new(&runner)
        .arg("run")
        .arg(&bundle)
        .arg("--consent")
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            // A non-zero exit is the app refusing to run -- a missing
            // capability, a bad bundle. Without a console this is the only
            // place that can say so.
            message_box(
                "Krate",
                &format!(
                    "{} could not be opened (exit code {}).\n\nRun it from a \
                     terminal with `krate run` to see why.",
                    std::path::Path::new(&bundle)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    status.code().unwrap_or(-1)
                ),
            );
        }
        Err(err) => {
            message_box("Krate", &format!("Could not start Krate: {err}"));
        }
    }
}

/// Show a message. The only way to say anything with no console attached.
#[cfg(windows)]
fn message_box(title: &str, text: &str) {
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
    // MB_ICONINFORMATION. MB_OK is 0x0, so naming it in the OR added
    // nothing but a lint; the comment carries the meaning instead.
    const KIND: u32 = 0x0000_0040;
    // SAFETY: both strings are null-terminated UTF-16 that outlive the call.
    unsafe {
        MessageBoxW(0, text.as_ptr(), title.as_ptr(), KIND);
    }
}

/// Nothing to do off Windows: macOS has Krate.app and Linux has a .desktop
/// entry, both of which already open a bundle without a terminal.
#[cfg(not(windows))]
fn main() {
    eprintln!("krate-open is the Windows double-click helper; use `krate run` here.");
    std::process::exit(1);
}
