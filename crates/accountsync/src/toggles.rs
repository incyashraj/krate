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

/// Turn sync on or off for every app on this device.
pub fn set_all_sync(device_dir: &Path, on: bool) -> std::io::Result<()> {
    let path = global_toggle_path(device_dir);
    if on {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    } else {
        std::fs::create_dir_all(device_dir)?;
        std::fs::write(path, b"sync is off for every app on this device\n")
    }
}

/// The app's data was cleared on this device: what sync remembered about it
/// goes too, so the next sync reads the account's copy back instead of
/// sending the emptied store on as this device deleting everything. Its
/// toggle stays as the person set it.
pub fn forget_replicas(store: &Path) {
    let dir = sync_dir(store);
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let _ = std::fs::remove_file(path);
        } else if path.extension().is_some_and(|e| e == "held") {
            // The other devices' copies of files held for a choice (S3).
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Where the replica for `store` and one account lives: named by a hash of
/// the account id, so an id is never a path.
pub fn replica_path(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.json", slot(account)))
}

/// Where sync keeps what it knows of an app's own files (S3) for one
/// account, beside the replica: `<store>.sync/<slot>.files.json`.
pub fn files_state_path(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.files.json", slot(account)))
}

/// Where the other devices' copies of files wait while the person chooses
/// between them: `<store>.sync/<slot>.held/<sha256>`. Never in the app's
/// folder, so the app never reads a copy it did not choose.
pub fn held_dir(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.held", slot(account)))
}

/// Where sync keeps what it knows of an app's `store.sql` (S4) for one
/// account: `<store>.sync/<slot>.sql.json`.
pub fn sql_state_path(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.sql.json", slot(account)))
}

/// Where the other devices' rows wait while the person chooses between
/// them, and while they are put in place: `<store>.sync/<slot>.sql.held/`.
pub fn sql_held_dir(store: &Path, account: &str) -> PathBuf {
    sync_dir(store).join(format!("{}.sql.held", slot(account)))
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
        set_all_sync(&devices, true).unwrap();
        assert!(sync_wanted(&store, &devices));
        set_all_sync(&devices, false).unwrap();
        assert!(!sync_wanted(&store, &devices));
        // Clearing an app's data forgets the replica, not the toggle.
        set_app_sync(&store, false).unwrap();
        std::fs::write(replica_path(&store, "u1"), b"{}").unwrap();
        std::fs::write(files_state_path(&store, "u1"), b"{}").unwrap();
        std::fs::write(sql_state_path(&store, "u1"), b"{}").unwrap();
        std::fs::create_dir_all(sql_held_dir(&store, "u1")).unwrap();
        std::fs::create_dir_all(held_dir(&store, "u1")).unwrap();
        std::fs::write(held_dir(&store, "u1").join("x"), b"theirs").unwrap();
        forget_replicas(&store);
        assert!(!replica_path(&store, "u1").exists());
        assert!(!files_state_path(&store, "u1").exists(), "and the files'");
        assert!(!held_dir(&store, "u1").exists());
        assert!(!sql_state_path(&store, "u1").exists(), "and the database's");
        assert!(!sql_held_dir(&store, "u1").exists());
        assert!(app_toggle_path(&store).exists());
        assert!(slot("../../etc").chars().all(|c| c.is_ascii_hexdigit()));
    }
}
