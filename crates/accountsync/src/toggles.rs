//! Where sync keeps its files beside an app's store, and the two toggles
//! (F-321: "a toggle per app ... and one for everything"). Files, so every
//! player reads them the same way and S2 only has to put them on screen.

use std::path::{Path, PathBuf};

/// `<store>.sync`: the replica per account, and this app's toggle.
pub fn sync_dir(store: &Path) -> PathBuf {
    let name = store
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_string());
    store.with_file_name(format!("{name}.sync"))
}

/// The file whose presence turns sync off for one app: `<store>.sync/off`.
pub fn app_toggle_path(store: &Path) -> PathBuf {
    sync_dir(store).join("off")
}

/// The file whose presence turns sync off for every app on this device.
pub fn global_toggle_path(device_dir: &Path) -> PathBuf {
    device_dir.join("off")
}

/// Whether an app's data should sync here: neither its toggle nor the one
/// for everything is off.
pub fn sync_wanted(store: &Path, device_dir: &Path) -> bool {
    !app_toggle_path(store).exists() && !global_toggle_path(device_dir).exists()
}

/// Turn sync on or off for one app.
pub fn set_app_sync(store: &Path, on: bool) -> std::io::Result<()> {
    let path = app_toggle_path(store);
    if on {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    } else {
        std::fs::create_dir_all(sync_dir(store))?;
        std::fs::write(path, b"sync is off for this app on this device\n")
    }
}

/// Where the replica for `store` and one account lives: named by a hash of
/// the account id, so an id is never a path.
pub fn replica_path(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.json", slot(account)))
}

/// The folder (or file stem) one account's files go by.
pub fn slot(account: &str) -> String {
    crate::value_hash(format!("krate account {account}").as_bytes())[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_toggles_are_files_beside_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("notes@ab12.kv");
        let devices = dir.path().join("account-sync");
        assert!(sync_wanted(&store, &devices));
        set_app_sync(&store, false).unwrap();
        assert!(!sync_wanted(&store, &devices));
        set_app_sync(&store, true).unwrap();
        set_app_sync(&store, true).unwrap();
        assert!(sync_wanted(&store, &devices));
        std::fs::create_dir_all(&devices).unwrap();
        std::fs::write(global_toggle_path(&devices), b"").unwrap();
        assert!(!sync_wanted(&store, &devices), "the one for everything");
        assert_eq!(
            replica_path(&store, "u1").parent().unwrap(),
            dir.path().join("notes@ab12.kv.sync")
        );
        assert_ne!(slot("u1"), slot("u2"));
        assert!(slot("../../etc").chars().all(|c| c.is_ascii_hexdigit()));
    }
}
