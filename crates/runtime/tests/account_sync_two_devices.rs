//! Two devices, one account, the real hub (IC-981 S1).
//!
//! The hub is cloud/worker/src/index.js itself, run by Node on this machine
//! over an in-memory KV (cloud/worker/test/account-sync-hub.mjs); the
//! devices are two of the runtime's own synced stores -- the very type a
//! run's `store.kv` calls go to -- with two device identities in two
//! folders, signed in as one GitHub user through the hub's test stub.
//!
//! What it proves, on the path a person's data takes:
//!
//! - a reminder written on the Mac is read on the phone, and the reverse;
//! - the phone gets the account's key only when the Mac approves it, and a
//!   third device gets it from the recovery code with no other device;
//! - both offline, both editing one reminder: one conflict, both values,
//!   each device reading its own; answered "keep both" on one device, every
//!   device ends with both and no conflict;
//! - a second account sees none of it;
//! - the hub's bytes hold no key name, value or app id.
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
use krate_runtime::account_sync_host::{AccountSyncContext, Step, SyncedKv};

const APP: &str = "dev.krate.reminders";

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
        files: None,
    }
}

fn device(hub: &str, who: &str, dir: &Path, name: &str) -> SyncedKv {
    SyncedKv::open(dir.join("reminders.kv"), context(hub, who, dir, name))
}

fn account(hub: &str, who: &str, dir: &Path, name: &str) -> (Hub, Account) {
    let client = Hub::new(hub, &format!("gho_{who}"));
    let id = account_of(&client).expect("the hub names the account");
    let ctx = context(hub, who, dir, name);
    let account = Account::load(&ctx.device_dir, &id, &ctx.machine_key, name).expect("device");
    (client, account)
}

fn text(kv: &SyncedKv, key: &str) -> Option<String> {
    kv.get(key)
        .expect("get")
        .map(|v| String::from_utf8(v).expect("text"))
}

/// Exchange until neither has anything left to send.
fn settle(devices: &[&SyncedKv]) {
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

#[test]
fn a_reminder_follows_the_person_and_nobody_else() {
    let Some(hub) = start_hub() else {
        return;
    };
    let base = hub.base.as_str();
    let (mac_dir, phone_dir, ipad_dir, bob_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );

    // The Mac: the account's first device makes its key.
    let mut mac = device(base, "ada", mac_dir.path(), "Mac");
    mac.set("reminder:1", br#"{"title":"Buy milk"}"#.to_vec())
        .unwrap();
    assert_eq!(mac.sync_now(), Ok(Step::Done));
    let (client, mac_account) = account(base, "ada", mac_dir.path(), "Mac");
    let code = mac_account
        .take_recovery_code()
        .expect("the first device holds a recovery code to show once");

    // The phone signs in: registered, and waiting for a yes.
    let mut phone = device(base, "ada", phone_dir.path(), "Galaxy S22 Ultra");
    assert_eq!(phone.sync_now(), Ok(Step::Waiting));
    assert!(phone.status().waiting_for_approval);
    assert_eq!(text(&phone, "reminder:1"), None, "nothing before the yes");
    let pending = mac_account.pending(&client).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].name, "Galaxy S22 Ultra");
    let ring = mac_account.held().expect("the Mac holds the key");
    mac_account
        .approve(&client, &ring, &pending[0].device_id)
        .expect("the Mac says yes");

    // Mac to phone.
    assert_eq!(phone.sync_now(), Ok(Step::Done));
    assert_eq!(
        text(&phone, "reminder:1").as_deref(),
        Some(r#"{"title":"Buy milk"}"#)
    );
    assert!(!phone.status().waiting_for_approval);

    // Phone to Mac: a new one, an edit, a delete.
    phone
        .set("reminder:2", br#"{"title":"Call mum"}"#.to_vec())
        .unwrap();
    phone
        .set("reminder:1", br#"{"title":"Buy oat milk"}"#.to_vec())
        .unwrap();
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert_eq!(
        text(&mac, "reminder:2").as_deref(),
        Some(r#"{"title":"Call mum"}"#)
    );
    assert_eq!(
        text(&mac, "reminder:1").as_deref(),
        Some(r#"{"title":"Buy oat milk"}"#)
    );
    mac.delete("reminder:2").unwrap();
    mac.sync_now().unwrap();
    phone.sync_now().unwrap();
    assert_eq!(text(&phone, "reminder:2"), None, "a delete crosses too");

    // Both offline, both edit one reminder.
    mac.set(
        "reminder:1",
        br#"{"title":"Buy oat milk","done":true}"#.to_vec(),
    )
    .unwrap();
    phone
        .set("reminder:1", br#"{"title":"Buy almond milk"}"#.to_vec())
        .unwrap();
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);
    assert_eq!(
        text(&mac, "reminder:1").as_deref(),
        Some(r#"{"title":"Buy oat milk","done":true}"#),
        "the Mac reads its own"
    );
    assert_eq!(
        text(&phone, "reminder:1").as_deref(),
        Some(r#"{"title":"Buy almond milk"}"#),
        "the phone reads its own"
    );
    let on_mac = mac.conflicts();
    assert_eq!(on_mac.len(), 1, "one conflict: {on_mac:?}");
    assert_eq!(on_mac[0].key, "reminder:1");
    assert_eq!(on_mac[0].others.len(), 1);
    assert_eq!(
        on_mac[0].others[0].0.as_deref(),
        Some(&br#"{"title":"Buy almond milk"}"#[..])
    );
    assert_eq!(on_mac[0].others[0].1, "Galaxy S22 Ultra");
    assert!(on_mac[0].mergeable);
    assert_eq!(phone.conflicts().len(), 1, "and the same one on the phone");
    assert_eq!(phone.status().conflicts, 1);

    // Answered on the Mac: keep both.
    mac.resolve("reminder:1", &Resolution::KeepBoth).unwrap();
    settle(&[&mac, &phone]);
    for (who, kv) in [("Mac", &mac), ("phone", &phone)] {
        assert!(kv.conflicts().is_empty(), "settled on the {who}");
        assert_eq!(
            text(kv, "reminder:1").as_deref(),
            Some(r#"{"title":"Buy oat milk","done":true}"#),
            "{who}"
        );
        assert_eq!(
            text(kv, "reminder:1 (Galaxy S22 Ultra)").as_deref(),
            Some(r#"{"title":"Buy almond milk"}"#),
            "{who}"
        );
    }

    // A third device, no other at hand: the recovery code.
    let ipad = device(base, "ada", ipad_dir.path(), "iPad");
    assert_eq!(ipad.sync_now(), Ok(Step::Waiting));
    let (client, ipad_account) = account(base, "ada", ipad_dir.path(), "iPad");
    assert!(
        ipad_account.recover(&client, "not a code").is_err(),
        "a wrong code opens nothing"
    );
    ipad_account
        .recover(&client, &code)
        .expect("the recovery code opens the key");
    let ipad = device(base, "ada", ipad_dir.path(), "iPad");
    assert_eq!(ipad.sync_now(), Ok(Step::Done));
    assert_eq!(
        text(&ipad, "reminder:1 (Galaxy S22 Ultra)").as_deref(),
        Some(r#"{"title":"Buy almond milk"}"#)
    );

    // Another person, the same app: their own key, their own empty bucket.
    let bob = device(base, "bob", bob_dir.path(), "Bob's Mac");
    assert_eq!(bob.sync_now(), Ok(Step::Done));
    assert_eq!(
        bob.keys().unwrap(),
        Vec::<String>::new(),
        "bob sees nothing"
    );
    let (bob_client, _) = account(base, "bob", bob_dir.path(), "Bob's Mac");
    let view = bob_client.view(None).unwrap();
    assert!(
        view["devices"].as_array().unwrap().len() == 1,
        "bob's account lists only bob's device"
    );

    // What the hub keeps: no key name, no value, no app id -- not in the
    // clear and not in base64.
    let dump: serde_json::Value = serde_json::from_str(
        &ureq::get(&format!("{base}/__kv"))
            .call()
            .unwrap()
            .into_string()
            .unwrap(),
    )
    .unwrap();
    let bytes = dump["kv"].to_string();
    assert!(bytes.contains("acsync:"), "the dump holds the sync records");
    for plain in [
        "reminder:1",
        "reminder",
        "Buy milk",
        "oat milk",
        "almond",
        "Call mum",
        APP,
        "unsigned",
    ] {
        assert!(
            !bytes.contains(plain),
            "the hub holds {plain:?} in the clear"
        );
        let b64 = krate_orgkeys::b64_encode(plain.as_bytes());
        let stem = &b64[..b64.len().saturating_sub(4)];
        assert!(!bytes.contains(stem), "the hub holds {plain:?} in base64");
    }
    eprintln!(
        "MEASURE the hub took {} KV writes for this whole walk",
        dump["writes"]
    );
    drop(phone);
    drop(mac);
}
