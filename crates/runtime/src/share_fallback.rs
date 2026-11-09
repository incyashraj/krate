//! Sharing on a computer with no share sheet (Windows and Linux; IC-919,
//! IC-982): the person still sends what the app offers, by the two ways
//! every desktop has.
//!
//! - Files are saved where the person picks, in the system's own save
//!   dialog (a folder, for more than one), and that folder is opened so they
//!   can attach or drag them on. The pick is the grant, as with any save.
//! - A text or a link is shown in a dialog that says what it is; OK puts it
//!   on the clipboard to paste where they want to send it. Nothing reaches
//!   the clipboard without that OK, so an app cannot use Share to write the
//!   clipboard behind the person's back.
//!
//! The answer is the one the WIT gives: true when the person went ahead
//! (saved or copied), false when they cancelled.
//!
//! A Mac has its own share picker, so this is only reached on Windows and
//! Linux; it compiles on every desktop so its tests run on each.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::path::{Path, PathBuf};

/// The most of an app's text the copy dialog shows; the rest is "...".
const SHOWN: usize = 600;

/// What goes on the clipboard: the text, then the link on its own line.
pub(crate) fn clipboard_text(text: Option<&str>, url: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = [text, url]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// The copy dialog's words, the app's text cut short if it is long.
pub(crate) fn copy_words(app: &str, offered: &str, files_saved: usize) -> String {
    let mut shown: String = offered.chars().take(SHOWN).collect();
    if offered.chars().count() > SHOWN {
        shown.push('\u{2026}');
    }
    let lead = match files_saved {
        0 => format!("{app} wants to share:"),
        1 => format!("The file is saved. {app} also offers:"),
        _ => format!("The files are saved. {app} also offers:"),
    };
    format!(
        "{lead}\n\n{shown}\n\nThis computer has no share menu: OK copies it, to paste where \
         you want to send it."
    )
}

/// A name in `dir` for `name` that is not taken: "photo.png", then
/// "photo (2).png", "photo (3).png".
pub(crate) fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|path| !path.exists())
        .expect("some number is free")
}

/// Write every file into `dir` under a name not already taken there.
pub(crate) fn save_into(dir: &Path, files: &[(String, Vec<u8>)]) -> Result<Vec<PathBuf>, String> {
    let mut saved = Vec::new();
    for (name, data) in files {
        let path = free_name(dir, name);
        std::fs::write(&path, data).map_err(|e| format!("could not save {name}: {e}"))?;
        saved.push(path);
    }
    Ok(saved)
}

/// The share, with the system's dialogs. `copy` puts text on the
/// clipboard once the person said OK.
pub(crate) fn share(
    app: &str,
    text: Option<&str>,
    url: Option<&str>,
    files: &[(String, Vec<u8>)],
    copy: impl FnOnce(&str) -> Result<(), String>,
) -> Result<bool, String> {
    let mut saved = Vec::new();
    if let [(name, data)] = files {
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Save {name} to share it"))
            .set_file_name(name)
            .save_file()
        else {
            return Ok(false);
        };
        std::fs::write(&path, data).map_err(|e| format!("could not save {name}: {e}"))?;
        saved.push(path);
    } else if !files.is_empty() {
        let Some(dir) = rfd::FileDialog::new()
            .set_title(format!(
                "Choose a folder for the {} files to share",
                files.len()
            ))
            .pick_folder()
        else {
            return Ok(false);
        };
        saved = save_into(&dir, files)?;
    }
    if let Some(first) = saved.first() {
        reveal(first);
    }
    let Some(offered) = clipboard_text(text, url) else {
        return Ok(!saved.is_empty());
    };
    let answer = rfd::MessageDialog::new()
        .set_title("Share")
        .set_description(copy_words(app, &offered, saved.len()))
        .set_buttons(rfd::MessageButtons::OkCancel)
        .show();
    if answer != rfd::MessageDialogResult::Ok {
        return Ok(!saved.is_empty());
    }
    copy(&offered)?;
    Ok(true)
}

/// Open the folder a saved file is in, the file picked out where the
/// file manager can.
fn reveal(path: &Path) {
    // Run directly, not through a shell: the path is one argument.
    let _ = if cfg!(target_os = "windows") {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path.as_os_str());
        std::process::Command::new("explorer.exe")
            .arg(select)
            .spawn()
    } else {
        std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_then_the_link_go_on_the_clipboard() {
        assert_eq!(
            clipboard_text(Some("My sketch"), Some("https://krate.tech/a/x")),
            Some("My sketch\nhttps://krate.tech/a/x".to_string())
        );
        assert_eq!(
            clipboard_text(None, Some(" https://x.test ")),
            Some("https://x.test".to_string())
        );
        assert_eq!(clipboard_text(Some("  "), None), None);
        assert_eq!(clipboard_text(None, None), None);
    }

    #[test]
    fn the_dialog_says_what_is_offered_and_what_ok_does() {
        let words = copy_words("Sketchpad", "My sketch", 0);
        assert!(words.starts_with("Sketchpad wants to share:"), "{words}");
        assert!(words.contains("My sketch"));
        assert!(words.contains("OK copies it"));
        let long = "x".repeat(SHOWN + 50);
        let words = copy_words("Notes", &long, 2);
        assert!(words.starts_with("The files are saved."), "{words}");
        assert!(words.contains('\u{2026}'));
        assert!(!words.contains(&long), "a long text is cut short");
    }

    #[test]
    fn files_saved_into_a_folder_never_overwrite_what_is_there() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("photo.png"), b"theirs").expect("existing");
        let saved = save_into(
            dir.path(),
            &[
                ("photo.png".to_string(), b"mine".to_vec()),
                ("photo.png".to_string(), b"mine too".to_vec()),
                ("notes".to_string(), b"plain".to_vec()),
            ],
        )
        .expect("saved");
        let names: Vec<String> = saved
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["photo (2).png", "photo (3).png", "notes"]);
        assert_eq!(
            std::fs::read(dir.path().join("photo.png")).unwrap(),
            b"theirs"
        );
        assert_eq!(std::fs::read(&saved[1]).unwrap(), b"mine too");
    }
}
