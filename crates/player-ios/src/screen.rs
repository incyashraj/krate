//! The iPhone player's own rules for Krate's home screen (apps/krate-home),
//! kept apart from the UIKit code so they are tested on any machine: where
//! an app's data lives, the row for the app this build carries, and the
//! mail "Report a problem" opens. The rows, the answers and the account file come
//! from the Android player's `home.rs`, shared, so both phones read one
//! contract.

use crate::home::{field, human_size};
use crate::run_config::stores;
use std::path::Path;

/// The mail "Report a problem" opens: to the founder, with this Krate's
/// version and the phone already said, as Krate's menu does for an app.
pub fn report_link(version: &str, device: &str) -> String {
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!(
        "mailto:yashraj@krate.tech?subject={}&body={}",
        encode("A problem with Krate on iPhone"),
        encode(&format!("Krate {version} on {device}\n\nWhat happened:\n"))
    )
}

/// Everything in a folder, however deep.
fn walk(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(m) if m.is_dir() => walk(&entry.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Everything this phone keeps for Krate's apps, for the profile tab: the
/// library, the apps' own files and their stores.
pub fn storage_used(data_root: &Path) -> u64 {
    ["Library", "apps", "stores"]
        .iter()
        .map(|d| walk(&data_root.join(d)))
        .sum()
}

/// What an app keeps on this phone: its folder (`apps/<name>`, where the
/// player runs it) and its stores (`stores/<name>.kv` and the rest), where
/// its runs keep them -- by its publisher when it is signed (K-1172).
pub fn app_data_size(data_root: &Path, app_id: &str, publisher: Option<&str>) -> u64 {
    stores(data_root, app_id, publisher).size()
}

/// Forget what an app keeps on this phone: its files and its stores.
pub fn clear_app_data(data_root: &Path, app_id: &str, publisher: Option<&str>) {
    let stores = stores(data_root, app_id, publisher);
    stores.clear();
    crate::sync_home::forget_after_clear(&stores);
}

/// The Library row for the app this build carries as native code: the
/// same five fields as a received app's (`library_row_full`). Unused in a
/// build that carries no app.
#[allow(dead_code)]
pub fn carried_row(name: &str, art: Option<&str>, data: u64, version: &str) -> String {
    format!(
        "{}\tBuilt in \u{00b7} Runs at full speed\t{}\t{}\t{}",
        field(name),
        art.map(field).unwrap_or_else(|| "-".to_string()),
        if data == 0 {
            "None".to_string()
        } else {
            human_size(data)
        },
        field(version)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::home::{home_answer, HomeAnswer};

    #[test]
    fn only_closed_leaves() {
        // The answer that closed Krate on an iPhone.
        assert_eq!(home_answer("home:signin\n"), HomeAnswer::SignIn);
        assert_eq!(home_answer("home:refresh"), HomeAnswer::Refresh);
        assert_eq!(home_answer("home:report"), HomeAnswer::Report);
        // Anything new, or malformed, stays on the home screen.
        assert_eq!(home_answer("home:something-new:3"), HomeAnswer::Stay);
        assert_eq!(home_answer("home:open:x"), HomeAnswer::Stay);
        // Only these leave.
        assert_eq!(home_answer("home:closed\n"), HomeAnswer::Closed);
        assert_eq!(home_answer(""), HomeAnswer::Closed);
    }

    #[test]
    fn the_report_mail_is_one_safe_link() {
        let link = report_link("0.5.6", "Yash's iPhone & co");
        assert!(link.starts_with("mailto:yashraj@krate.tech?subject=A%20problem"));
        assert_eq!(link.matches('&').count(), 1, "{link}");
        assert!(link.contains("Krate%200.5.6%20on%20Yash%27s%20iPhone%20%26%20co%0A"));
        assert!(!link.contains(' '));
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("krate-player-ios-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_apps_data_is_its_sandbox_and_stores_and_clears_alone() {
        let root = scratch("data");
        let sandbox = root.join("apps").join("dev.krate.notes");
        std::fs::create_dir_all(sandbox.join("deep")).unwrap();
        std::fs::write(sandbox.join("deep").join("a.txt"), [0u8; 100]).unwrap();
        std::fs::create_dir_all(root.join("stores")).unwrap();
        std::fs::write(root.join("stores").join("dev.krate.notes.kv"), [0u8; 30]).unwrap();
        std::fs::write(
            root.join("stores").join("dev.krate.notes.sqlite"),
            [0u8; 20],
        )
        .unwrap();
        // Another app's data, which clearing this one must not touch.
        std::fs::write(root.join("stores").join("dev.krate.chess.kv"), [0u8; 7]).unwrap();
        std::fs::create_dir_all(root.join("Library")).unwrap();
        std::fs::write(root.join("Library").join("notes.krate"), [0u8; 1000]).unwrap();

        assert_eq!(app_data_size(&root, "dev.krate.notes", None), 150);
        assert_eq!(storage_used(&root), 1157);
        clear_app_data(&root, "dev.krate.notes", None);
        assert_eq!(app_data_size(&root, "dev.krate.notes", None), 0);
        assert!(root.join("stores").join("dev.krate.chess.kv").exists());
        assert!(root.join("Library").join("notes.krate").exists());
        assert_eq!(storage_used(&root), 1007);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A signed app's Storage and Clear reach the store its runs use, kept
    /// by its publisher (K-1172), and leave an unsigned app that declares
    /// the same id alone.
    #[test]
    fn a_signed_apps_storage_and_clear_find_its_publishers_store() {
        let root = scratch("signed");
        let publisher = "c0ffee".repeat(11);
        let signed = stores(&root, "dev.krate.notes", Some(&publisher));
        assert_eq!(
            signed.kv,
            root.join("stores")
                .join("dev.krate.notes@c0ffeec0ffeec0ff.kv")
        );
        std::fs::create_dir_all(signed.files.join("deep")).unwrap();
        std::fs::write(signed.files.join("deep").join("a.txt"), [0u8; 100]).unwrap();
        std::fs::create_dir_all(root.join("stores")).unwrap();
        std::fs::write(&signed.kv, [0u8; 30]).unwrap();
        std::fs::write(&signed.sql, [0u8; 20]).unwrap();
        // An unsigned app declaring the same id: its own store.
        std::fs::write(root.join("stores").join("dev.krate.notes.kv"), [0u8; 7]).unwrap();

        assert_eq!(
            app_data_size(&root, "dev.krate.notes", Some(&publisher)),
            150
        );
        assert_eq!(app_data_size(&root, "dev.krate.notes", None), 7);
        clear_app_data(&root, "dev.krate.notes", Some(&publisher));
        assert_eq!(app_data_size(&root, "dev.krate.notes", Some(&publisher)), 0);
        assert!(!signed.files.exists());
        assert_eq!(app_data_size(&root, "dev.krate.notes", None), 7);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_carried_row_has_the_five_fields() {
        let row = carried_row("Nova\tx", None, 0, "1.0.0");
        assert_eq!(
            row,
            "Nova x\tBuilt in \u{00b7} Runs at full speed\t-\tNone\t1.0.0"
        );
        let row = carried_row("Nova", Some("screen-carried.rgba"), 2048, "2");
        assert_eq!(row.split('\t').nth(3), Some("2 KB"));
    }
}
