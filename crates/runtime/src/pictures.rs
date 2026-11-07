//! Adding a picture to the photo library on a computer whose only library
//! is a folder (Windows and Linux; IC-920, IC-982): the person's Pictures
//! folder, which the Windows Photos app and the Linux photo apps show. The
//! app's `photos.add` grant on Krate's card is the person's yes; the app
//! can add a picture, never read the folder.
//!
//! A Mac and a phone add to their Photos library itself instead.
#![cfg_attr(target_os = "macos", allow(dead_code))]

use std::path::{Path, PathBuf};

/// A name for an added picture: the app's name and the time, as a phone's
/// screenshots are named, with only characters every file system takes.
/// `stamp` is "2026-11-05 at 14.03.22".
pub(crate) fn picture_name(app: &str, stamp: &str, png: bool) -> String {
    let clean: String = app
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let app = if clean.is_empty() {
        "Krate app"
    } else {
        &clean
    };
    format!("{app} {stamp}.{}", if png { "png" } else { "jpg" })
}

/// "2026-11-05 at 14.03.22" for `unix_secs`, in UTC (the name only has
/// to sort and not collide).
pub(crate) fn stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rest = unix_secs % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} at {:02}.{:02}.{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Write `data` into `dir` under `name`, never over a picture already
/// there.
pub(crate) fn add_into(dir: &Path, name: &str, data: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not reach your Pictures: {e}"))?;
    let path = crate::share_fallback::free_name(dir, name);
    std::fs::write(&path, data).map_err(|e| format!("could not add the picture: {e}"))?;
    Ok(path)
}

/// The person's Pictures folder, as the system names it.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn pictures_dir() -> Option<PathBuf> {
    directories_next::UserDirs::new()
        .and_then(|dirs| dirs.picture_dir().map(Path::to_path_buf))
        .or_else(|| directories_next::UserDirs::new().map(|d| d.home_dir().join("Pictures")))
}

/// Add a checked PNG or JPEG to the person's Pictures.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn add(app: &str, data: &[u8]) -> Result<(), String> {
    let dir = pictures_dir().ok_or("this computer has no Pictures folder Krate can find")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = stamp(now);
    let png = data.starts_with(&[0x89, b'P', b'N', b'G']);
    add_into(&dir, &picture_name(app, &stamp, png), data).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_named_by_its_app_and_time_in_safe_characters() {
        assert_eq!(
            picture_name("Sketch Pad", "2026-11-05 at 14.03.22", true),
            "Sketch Pad 2026-11-05 at 14.03.22.png"
        );
        assert_eq!(
            picture_name("a/b\\c:d*e", "t", false),
            "a b c d e t.jpg",
            "no path or reserved characters"
        );
        assert_eq!(picture_name("../", "t", true), "Krate app t.png");
    }

    #[test]
    fn the_time_in_a_name_is_the_calendar_s() {
        assert_eq!(stamp(0), "1970-01-01 at 00.00.00");
        // 2026-11-05T14:03:22Z.
        assert_eq!(stamp(1_793_887_402), "2026-11-05 at 14.03.22");
        // A leap day.
        assert_eq!(stamp(1_709_164_800), "2024-02-29 at 00.00.00");
    }

    #[test]
    fn adding_never_replaces_a_picture_already_there() {
        let dir = tempfile::tempdir().expect("temp dir");
        let first = add_into(dir.path(), "Pad t.png", b"one").expect("first");
        let second = add_into(dir.path(), "Pad t.png", b"two").expect("second");
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), b"one");
        assert_eq!(second.file_name().unwrap(), "Pad t (2).png");
    }
}
