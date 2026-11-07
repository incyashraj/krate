//! An app's own files on two devices, one account, the real hub (IC-981
//! S3).
//!
//! The hub is cloud/worker/src/index.js itself, run by Node on this machine
//! over an in-memory KV and R2 (cloud/worker/test/account-sync-hub.mjs);
//! the devices are two of the runtime's own synced folders -- the very
//! type a run starts beside the app -- with two device identities, signed
//! in as one GitHub user through the hub's test stub.
//!
//! What it proves, on the path a person's files take:
//!
//! - a file written on the Mac is read on the phone, and the reverse; an
//!   edit and a delete travel too;
//! - a sync with nothing new costs the hub no write, and one with
//!   something new one KV write (the bodies go to R2);
//! - both offline, both editing one file: one conflict, each device
//!   reading its own; each of the choices -- this device's, the other's,
//!   both ("photo (Galaxy S22 Ultra).jpg"), merge for JSON, and delete
//!   against edit answered with the app closed -- settles both devices;
//! - a file over 16 MB stays where it is, and says so;
//! - a second account sees none of it;
//! - the hub's KV and R2 hold no file name, no content and no app id, in
//!   the clear or in base64.
//!
//! It needs Node and the hub's built wasm (`scripts/build-hub-validator.sh`,
//! `scripts/build-web-player.sh`) and `npm ci` in cloud/worker. Without
//! them it says so and passes; with `KRATE_REQUIRE_HUB_TESTS=1` it fails.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use krate_accountsync::device::{account_of, Account};
use krate_accountsync::hub::Hub;
use krate_accountsync::Resolution;
use krate_runtime::account_sync_files::{
    file_conflicts_at, resolve_file_at, sync_files_closed, SyncedFiles,
};
use krate_runtime::account_sync_host::{AccountSyncContext, Step};

const APP: &str = "dev.krate.journal";

struct HubProcess {
    child: Child,
    base: String,
}

impl Drop for HubProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn start_hub() -> Option<HubProcess> {
    let worker = repo().join("cloud/worker");
    let missing: Vec<&str> = ["src/validator.wasm", "src/transpiler.wasm", "node_modules"]
        .into_iter()
        .filter(|p| !worker.join(p).exists())
        .collect();
    let node = Command::new("node").arg("--version").output().is_ok();
    if !missing.is_empty() || !node {
        let why = format!(
            "the hub cannot run here (node: {node}, missing: {missing:?}) -- build it as \
             CLAUDE.md's \"Deploying the hub worker\" says"
        );
        if std::env::var_os("KRATE_REQUIRE_HUB_TESTS").is_some() {
            panic!("{why}");
        }
        eprintln!("SKIPPED: {why}");
        return None;
    }
    let mut child = Command::new("node")
        .arg("--experimental-wasm-modules")
        .arg(worker.join("test/account-sync-hub.mjs"))
        .arg("0")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("node starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout"))
        .read_line(&mut line)
        .expect("the hub says where it listens");
    let said: serde_json::Value = serde_json::from_str(&line).expect("one JSON line");
    Some(HubProcess {
        base: said["base"].as_str().expect("base").to_string(),
        child,
    })
}

fn context(hub: &str, who: &str, dir: &Path, name: &str) -> AccountSyncContext {
    AccountSyncContext {
        hub: hub.to_string(),
        token: format!("gho_{who}"),
        device_dir: dir.join("account-sync"),
        device_name: name.to_string(),
        machine_key: name.as_bytes().repeat(8),
        app_id: APP.to_string(),
        publisher: "unsigned".to_string(),
        enabled: true,
        background: false,
        files: Some(dir.join("folder")),
    }
}

fn store(dir: &Path) -> PathBuf {
    dir.join("journal.kv")
}

fn device(hub: &str, who: &str, dir: &Path, name: &str) -> SyncedFiles {
    std::fs::create_dir_all(dir.join("folder")).unwrap();
    SyncedFiles::open(dir.join("folder"), store(dir), context(hub, who, dir, name))
}

fn account(hub: &str, who: &str, dir: &Path, name: &str) -> (Hub, Account) {
    let client = Hub::new(hub, &format!("gho_{who}"));
    let id = account_of(&client).expect("the hub names the account");
    let ctx = context(hub, who, dir, name);
    let account = Account::load(&ctx.device_dir, &id, &ctx.machine_key, name).expect("device");
    (client, account)
}

fn write(dir: &Path, path: &str, bytes: &[u8]) {
    let target = dir.join("folder").join(path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, bytes).unwrap();
}

fn read(dir: &Path, path: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join("folder").join(path)).ok()
}

fn text(dir: &Path, path: &str) -> Option<String> {
    read(dir, path).map(|b| String::from_utf8(b).expect("text"))
}

/// Exchange until neither has anything left to send.
fn settle(devices: &[&SyncedFiles]) {
    for _ in 0..4 {
        let mut more = false;
        for d in devices {
            more |= d.sync_now().expect("sync") == Step::More;
        }
        if !more {
            return;
        }
    }
}

fn dump(base: &str) -> serde_json::Value {
    serde_json::from_str(
        &ureq::get(&format!("{base}/__kv"))
            .call()
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap()
}

/// Bytes a picture might hold, with a run no sealing would leave in place.
fn photo(seed: u8, len: usize) -> Vec<u8> {
    let mut out = b"PHOTO-PLAINTEXT-MARKER".to_vec();
    let mut x = seed as u32 | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        out.push(x as u8);
    }
    out
}

#[test]
fn an_apps_files_follow_the_person_and_nobody_else() {
    let Some(hub) = start_hub() else {
        return;
    };
    let base = hub.base.as_str();
    let (mac_dir, phone_dir, bob_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (mac_d, phone_d) = (mac_dir.path(), phone_dir.path());

    // The Mac makes the account's key; the phone waits for its yes.
    write(
        mac_d,
        "notes/today.txt",
        b"Dear diary, the files learned to travel.",
    );
    let mac = device(base, "ada", mac_d, "Mac");
    assert_eq!(mac.sync_now(), Ok(Step::Done));
    let phone = device(base, "ada", phone_d, "Galaxy S22 Ultra");
    assert_eq!(phone.sync_now(), Ok(Step::Waiting));
    assert_eq!(
        read(phone_d, "notes/today.txt"),
        None,
        "nothing before the yes"
    );
    let (client, mac_account) = account(base, "ada", mac_d, "Mac");
    let pending = mac_account.pending(&client).unwrap();
    let ring = mac_account.held().expect("the Mac holds the key");
    mac_account
        .approve(&client, &ring, &pending[0].device_id)
        .expect("the Mac says yes");

    // Mac to phone.
    assert_eq!(phone.sync_now(), Ok(Step::Done));
    assert_eq!(
        text(phone_d, "notes/today.txt").as_deref(),
        Some("Dear diary, the files learned to travel.")
    );

    // Phone to Mac: a picture.
    let picture = photo(7, 300_000);
    write(phone_d, "photo.jpg", &picture);
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert_eq!(
        read(mac_d, "photo.jpg"),
        Some(picture.clone()),
        "phone to Mac"
    );

    // Nothing new: no hub write. Something new: one KV write.
    let before = dump(base)["writes"].as_u64().unwrap();
    mac.sync_now().unwrap();
    phone.sync_now().unwrap();
    assert_eq!(
        dump(base)["writes"].as_u64().unwrap(),
        before,
        "a sync with nothing new writes nothing"
    );
    write(mac_d, "notes/today.txt", b"Dear diary, edited on the Mac.");
    mac.sync_now().unwrap();
    assert_eq!(
        dump(base)["writes"].as_u64().unwrap(),
        before + 1,
        "one KV write for a sync that sends"
    );

    // An edit and a delete travel.
    phone.sync_now().unwrap();
    assert_eq!(
        text(phone_d, "notes/today.txt").as_deref(),
        Some("Dear diary, edited on the Mac.")
    );
    write(phone_d, "notes/old.txt", b"to be deleted");
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert!(read(mac_d, "notes/old.txt").is_some());
    std::fs::remove_file(phone_d.join("folder/notes/old.txt")).unwrap();
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert_eq!(read(mac_d, "notes/old.txt"), None, "a delete crosses too");

    // Both offline, both edit the picture: one conflict, each its own.
    let mac_picture = photo(11, 200_000);
    let phone_picture = photo(13, 250_000);
    write(mac_d, "photo.jpg", &mac_picture);
    write(phone_d, "photo.jpg", &phone_picture);
    // And three more, for the other choices.
    write(mac_d, "a.txt", b"a from the Mac");
    write(mac_d, "b.txt", b"b from the Mac");
    write(mac_d, "settings.json", br#"{"theme":"dark","size":12}"#);
    write(mac_d, "c.txt", b"c, kept on both");
    settle(&[&mac, &phone]);
    write(phone_d, "a.txt", b"a from the phone");
    write(phone_d, "b.txt", b"b from the phone");
    write(
        phone_d,
        "settings.json",
        br#"{"theme":"light","font":"serif"}"#,
    );
    write(phone_d, "c.txt", b"c, changed on the phone");
    write(mac_d, "a.txt", b"a, the Mac again");
    write(mac_d, "b.txt", b"b, the Mac again");
    write(mac_d, "settings.json", br#"{"theme":"dark","size":14}"#);
    std::fs::remove_file(mac_d.join("folder/c.txt")).unwrap();
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);
    assert_eq!(
        read(mac_d, "photo.jpg"),
        Some(mac_picture.clone()),
        "the Mac reads its own"
    );
    assert_eq!(
        read(phone_d, "photo.jpg"),
        Some(phone_picture.clone()),
        "the phone its own"
    );
    let on_mac = mac.conflicts();
    assert_eq!(on_mac.len(), 5, "one conflict a file: {on_mac:?}");
    let pic = on_mac
        .iter()
        .find(|c| c.path == "photo.jpg")
        .expect("photo.jpg");
    assert_eq!(pic.mine.as_ref().map(|s| s.size), Some(200_000));
    assert_eq!(pic.others.len(), 1);
    assert_eq!(pic.others[0].0.as_ref().map(|s| s.size), Some(250_000));
    assert!(pic.others[0].0.as_ref().unwrap().at.is_some(), "when, too");
    assert_eq!(pic.others[0].1, "Galaxy S22 Ultra");
    assert!(!pic.mergeable, "a picture is not merged");
    let settings = on_mac.iter().find(|c| c.path == "settings.json").unwrap();
    assert!(settings.mergeable, "two JSON records merge");
    let c = on_mac.iter().find(|c| c.path == "c.txt").unwrap();
    assert!(c.mine.is_none(), "deleted here, changed there");
    assert_eq!(phone.conflicts().len(), 5, "and the same on the phone");
    assert_eq!(phone.status().conflicts, 5);

    // Keep both, on the Mac.
    mac.resolve("photo.jpg", &Resolution::KeepBoth).unwrap();
    // The other device's, on the Mac.
    mac.resolve("a.txt", &Resolution::KeepOther(0)).unwrap();
    // This device's, on the phone.
    phone.resolve("b.txt", &Resolution::KeepMine).unwrap();
    // Merge, on the phone.
    phone.resolve("settings.json", &Resolution::Merge).unwrap();
    assert_eq!(
        phone.resolve("photo.jpg", &Resolution::Merge),
        Err(krate_accountsync::ResolveError::NotMergeable),
        "a refused merge decides nothing"
    );
    // Answers on both: each takes the other's on its next exchange.
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);

    // Delete against edit, answered on the Mac with the app closed: keep
    // it deleted, sent at once.
    drop(mac);
    let (_, mac_account) = account(base, "ada", mac_d, "Mac");
    let names = krate_accountsync::device::device_names(&client.view(None).unwrap());
    let closed = file_conflicts_at(&store(mac_d), &mac_account.id, &names);
    assert_eq!(closed.len(), 1, "only c.txt waits: {closed:?}");
    resolve_file_at(
        &store(mac_d),
        &mac_account.id,
        &names,
        "c.txt",
        &Resolution::KeepMine,
    )
    .unwrap();
    assert_eq!(
        sync_files_closed(
            &store(mac_d),
            &mac_account.id,
            context(base, "ada", mac_d, "Mac")
        ),
        Ok(Step::Done)
    );
    let mac = device(base, "ada", mac_d, "Mac");
    settle(&[&mac, &phone]);

    for (who, d, files) in [("Mac", mac_d, &mac), ("phone", phone_d, &phone)] {
        assert!(files.conflicts().is_empty(), "settled on the {who}");
        assert_eq!(read(d, "photo.jpg"), Some(mac_picture.clone()), "{who}");
        assert_eq!(
            read(d, "photo (Galaxy S22 Ultra).jpg"),
            Some(phone_picture.clone()),
            "keep both, on the {who}"
        );
        assert_eq!(
            text(d, "a.txt").as_deref(),
            Some("a from the phone"),
            "{who}"
        );
        assert_eq!(
            text(d, "b.txt").as_deref(),
            Some("b from the phone"),
            "{who}"
        );
        let merged: serde_json::Value =
            serde_json::from_slice(&read(d, "settings.json").unwrap()).unwrap();
        assert_eq!(
            merged,
            serde_json::json!({"theme": "dark", "size": 14, "font": "serif"}),
            "merge, on the {who}"
        );
        assert_eq!(read(d, "c.txt"), None, "kept deleted, on the {who}");
    }

    // A file over 16 MB stays where it is, and says so.
    write(mac_d, "film.mov", &vec![1u8; 16 * 1024 * 1024 + 1]);
    settle(&[&mac, &phone]);
    assert_eq!(mac.status().kept_here, vec!["film.mov".to_string()]);
    assert_eq!(read(phone_d, "film.mov"), None);
    assert!(read(mac_d, "film.mov").is_some(), "and is not touched here");

    // Another person, the same app: their own key, nothing of ada's.
    let bob = device(base, "bob", bob_dir.path(), "Bob's Mac");
    assert_eq!(bob.sync_now(), Ok(Step::Done));
    assert!(
        std::fs::read_dir(bob_dir.path().join("folder"))
            .unwrap()
            .next()
            .is_none(),
        "bob's folder stays empty"
    );

    // What the hub keeps: no path, no contents, no app id -- not in the
    // clear and not in base64, in KV or in R2.
    let dump = dump(base);
    let kv = dump["kv"].to_string();
    let r2 = dump["r2"].to_string();
    assert!(
        dump["r2"].as_object().unwrap().len() >= 5,
        "the bodies are in R2"
    );
    let mut r2_bytes = Vec::new();
    for value in dump["r2"].as_object().unwrap().values() {
        r2_bytes.extend(krate_orgkeys::b64_decode(value.as_str().unwrap()).unwrap());
    }
    // Long enough that a chance match in random ciphertext is not a worry.
    for plain in [
        "notes/today.txt",
        "today.txt",
        "Dear diary",
        "photo.jpg",
        "photo (Galaxy",
        "PHOTO-PLAINTEXT-MARKER",
        "settings.json",
        "\"font\":\"serif\"",
        "a from the phone",
        "film.mov",
        APP,
        "unsigned",
    ] {
        for (place, bytes) in [("KV", &kv), ("R2", &r2)] {
            assert!(
                !bytes.contains(plain),
                "{place} holds {plain:?} in the clear"
            );
            let b64 = krate_orgkeys::b64_encode(plain.as_bytes());
            let stem = &b64[..b64.len().saturating_sub(4)];
            assert!(!bytes.contains(stem), "{place} holds {plain:?} in base64");
        }
        assert!(
            !r2_bytes.windows(plain.len()).any(|w| w == plain.as_bytes()),
            "an R2 body holds {plain:?}"
        );
    }
    eprintln!(
        "MEASURE the hub took {} KV writes and {} R2 writes for this whole walk",
        dump["writes"], dump["r2_writes"]
    );
}
