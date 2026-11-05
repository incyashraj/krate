//! A person's choices about their synced data, made the way Krate's home
//! on a phone and `krate account sync` on a computer make them (IC-981 S2),
//! against the real hub.
//!
//! The hub is cloud/worker/src/index.js run by Node over an in-memory KV
//! (cloud/worker/test/account-sync-hub.mjs), as in account_sync_two_devices.
//! The devices are the runtime's own synced stores. Every choice goes
//! through the calls the Players make while no app is running:
//! crates/accountsync `overview::{look, approve, recover, recovery_code}`
//! and the runtime's `conflicts_at` / `resolve_at`.
//!
//! What it proves:
//!
//! - a phone that signs in waits, the Mac sees it waiting by name, says yes,
//!   and the phone then reads the data;
//! - a third device with no other at hand gets in with the recovery code the
//!   Mac shows (and only with it), and the code is gone once put away;
//! - a change made on two devices, answered on the phone with each of the
//!   four choices -- this device's, the other's, both, merge -- ends the
//!   same on both devices with nothing left to decide;
//! - a delete on one against an edit on the other is a choice too, and
//!   keeping the delete deletes it everywhere.
//!
//! Without Node and the hub's built wasm it says so and passes; with
//! `KRATE_REQUIRE_HUB_TESTS=1` it fails.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use krate_accountsync::overview::{self, Me, Overview};
use krate_accountsync::Resolution;
use krate_runtime::account_sync_host::{
    conflicts_at, last_sync_at, resolve_at, AccountSyncContext, Step, SyncedKv,
};

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

/// One of the person's devices: its folder, its name, and its machine key.
struct Device {
    dir: tempfile::TempDir,
    name: &'static str,
    hub: String,
    key: Vec<u8>,
}

impl Device {
    fn new(hub: &str, name: &'static str) -> Device {
        Device {
            dir: tempfile::tempdir().unwrap(),
            name,
            hub: hub.to_string(),
            key: name.as_bytes().repeat(8),
        }
    }

    fn device_dir(&self) -> PathBuf {
        self.dir.path().join("account-sync")
    }

    fn store(&self) -> PathBuf {
        self.dir.path().join("reminders.kv")
    }

    /// The app, running.
    fn open(&self) -> SyncedKv {
        SyncedKv::open(
            self.store(),
            AccountSyncContext {
                hub: self.hub.clone(),
                token: "gho_ada".to_string(),
                device_dir: self.device_dir(),
                device_name: self.name.to_string(),
                machine_key: self.key.clone(),
                app_id: APP.to_string(),
                publisher: "unsigned".to_string(),
                enabled: true,
                background: false,
            },
        )
    }

    /// Krate's home on this device, the app closed.
    fn me(&self) -> (PathBuf, String) {
        (self.device_dir(), self.hub.clone())
    }

    fn look(&self) -> Overview {
        let (dir, hub) = self.me();
        overview::look(&me(&dir, &hub, &self.key, self.name), 1)
    }
}

fn me<'a>(dir: &'a Path, hub: &'a str, key: &'a [u8], name: &'a str) -> Me<'a> {
    Me {
        device_dir: dir,
        hub,
        token: "gho_ada",
        machine_key: key,
        name,
    }
}

fn text(kv: &SyncedKv, key: &str) -> Option<String> {
    kv.get(key)
        .expect("get")
        .map(|v| String::from_utf8(v).expect("text"))
}

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

/// Mac and phone, the phone approved by the Mac through Krate's home.
fn two_devices(base: &str) -> (Device, Device) {
    let mac = Device::new(base, "Mac");
    let phone = Device::new(base, "Galaxy S22 Ultra");
    // The Mac is the account's first device: the app's first sync makes
    // the key there.
    assert_eq!(mac.open().sync_now(), Ok(Step::Done));
    // The phone signs in and opens Settings -> Sync: it is registered and
    // waits.
    let seen = phone.look();
    assert_eq!(seen.reach, overview::Reach::Answered);
    assert!(!seen.holds_key, "the phone waits for a yes");
    // On the Mac, the phone waits by name.
    let on_mac = mac.look();
    assert!(on_mac.holds_key);
    let waiting: Vec<_> = on_mac.waiting().cloned().collect();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].name, "Galaxy S22 Ultra");
    // "Let Galaxy S22 Ultra read your apps' data?" -- yes.
    let (dir, hub) = mac.me();
    overview::approve(&me(&dir, &hub, &mac.key, mac.name), &waiting[0].id)
        .expect("the Mac says yes");
    assert!(phone.look().holds_key, "the phone holds the key now");
    assert!(
        mac.look().waiting().next().is_none(),
        "nobody waits on the Mac's list any more"
    );
    (mac, phone)
}

/// Both devices change `key` apart; both apps closed afterwards.
fn changed_on_both(mac: &Device, phone: &Device, key: &str, on_mac: Option<&str>, on_phone: &str) {
    let (m, p) = (mac.open(), phone.open());
    // One shared starting point first, so this is a real edit on each.
    let mut m = m;
    let mut p = p;
    m.set(key, b"start".to_vec()).unwrap();
    settle(&[&m, &p]);
    assert_eq!(text(&p, key).as_deref(), Some("start"));
    match on_mac {
        Some(value) => m.set(key, value.as_bytes().to_vec()).unwrap(),
        None => m.delete(key).unwrap(),
    }
    p.set(key, on_phone.as_bytes().to_vec()).unwrap();
    settle(&[&m, &p]);
    settle(&[&m, &p]);
    assert_eq!(m.conflicts().len(), 1, "a conflict on the Mac for {key}");
    assert_eq!(p.conflicts().len(), 1, "a conflict on the phone for {key}");
}

/// Answer on the phone through the Players' calls, then reopen both and
/// sync: answers what each device's app reads at `key`.
fn answer_on_phone(
    mac: &Device,
    phone: &Device,
    key: &str,
    choice: Resolution,
) -> (Option<String>, Option<String>, SyncedKv, SyncedKv) {
    let seen = phone.look();
    let account = seen.account.clone().expect("the account");
    let names: BTreeMap<String, String> = seen.names();
    let waiting = conflicts_at(&phone.store(), &account, &names);
    let conflict = waiting
        .iter()
        .find(|c| c.key == key)
        .expect("the conflict, read from disk with the app closed");
    assert_eq!(
        conflict.others[0].1, "Mac",
        "named for the device it came from"
    );
    resolve_at(&phone.store(), &account, &names, key, &choice).expect("answered");
    assert!(
        conflicts_at(&phone.store(), &account, &names)
            .iter()
            .all(|c| c.key != key),
        "nothing left to decide on the phone"
    );
    let (m, p) = (mac.open(), phone.open());
    settle(&[&p, &m]);
    settle(&[&p, &m]);
    assert!(
        m.conflicts().iter().all(|c| c.key != key),
        "settled on the Mac"
    );
    assert!(
        p.conflicts().iter().all(|c| c.key != key),
        "settled on the phone"
    );
    (text(&m, key), text(&p, key), m, p)
}

#[test]
fn every_choice_settles_on_both_devices() {
    let Some(hub) = start_hub() else {
        return;
    };
    let (mac, phone) = two_devices(&hub.base);

    // Keep this device's (the phone's).
    changed_on_both(
        &mac,
        &phone,
        "r1",
        Some("Milk from the Mac"),
        "Milk from the phone",
    );
    let (m, p, _, _) = answer_on_phone(&mac, &phone, "r1", Resolution::KeepMine);
    assert_eq!(m.as_deref(), Some("Milk from the phone"));
    assert_eq!(p.as_deref(), Some("Milk from the phone"));

    // Keep the other's (the Mac's).
    changed_on_both(
        &mac,
        &phone,
        "r2",
        Some("Eggs from the Mac"),
        "Eggs from the phone",
    );
    let (m, p, _, _) = answer_on_phone(&mac, &phone, "r2", Resolution::KeepOther(0));
    assert_eq!(m.as_deref(), Some("Eggs from the Mac"));
    assert_eq!(p.as_deref(), Some("Eggs from the Mac"));

    // Keep both: the Mac's beside the phone's, under the Mac's name.
    changed_on_both(
        &mac,
        &phone,
        "r3",
        Some("Bread from the Mac"),
        "Bread from the phone",
    );
    let (m, p, mk, pk) = answer_on_phone(&mac, &phone, "r3", Resolution::KeepBoth);
    assert_eq!(m.as_deref(), Some("Bread from the phone"));
    assert_eq!(p.as_deref(), Some("Bread from the phone"));
    for kv in [&mk, &pk] {
        assert_eq!(text(kv, "r3 (Mac)").as_deref(), Some("Bread from the Mac"));
    }
    drop((mk, pk));

    // Merge: two records put together.
    changed_on_both(
        &mac,
        &phone,
        "r4",
        Some(r#"{"title":"Jam","done":true}"#),
        r#"{"title":"Jam","due":"fri"}"#,
    );
    let (m, p, _, _) = answer_on_phone(&mac, &phone, "r4", Resolution::Merge);
    assert_eq!(m, p, "the same merge on both");
    let merged: serde_json::Value = serde_json::from_str(&m.expect("merged")).unwrap();
    assert_eq!(
        merged,
        serde_json::json!({ "title": "Jam", "done": true, "due": "fri" })
    );

    // Deleted on the Mac, changed on the phone: keep the delete.
    changed_on_both(&mac, &phone, "r5", None, "Tea, changed on the phone");
    {
        let seen = phone.look();
        let account = seen.account.clone().unwrap();
        let waiting = conflicts_at(&phone.store(), &account, &seen.names());
        let c = waiting.iter().find(|c| c.key == "r5").unwrap();
        assert_eq!(c.mine.as_deref(), Some(&b"Tea, changed on the phone"[..]));
        assert_eq!(c.others[0].0, None, "deleted on the Mac");
        assert!(!c.mergeable, "a delete does not merge");
    }
    let (m, p, mk, pk) = answer_on_phone(&mac, &phone, "r5", Resolution::KeepOther(0));
    assert_eq!((m, p), (None, None), "deleted everywhere");

    // The home's "last synced" for the app is there on both.
    let account = phone.look().account.unwrap();
    drop((mk, pk));
    assert!(last_sync_at(&phone.store(), &account).is_some());
    assert!(last_sync_at(&mac.store(), &account).is_some());

    // A merge offered where it cannot work decides nothing.
    changed_on_both(&mac, &phone, "r6", Some("plain"), "text");
    let seen = phone.look();
    let account = seen.account.clone().unwrap();
    assert!(resolve_at(
        &phone.store(),
        &account,
        &seen.names(),
        "r6",
        &Resolution::Merge
    )
    .is_err());
    assert_eq!(
        conflicts_at(&phone.store(), &account, &seen.names()).len(),
        1,
        "still waiting, both kept"
    );
}

#[test]
fn a_waiting_device_gets_in_with_a_yes_or_the_recovery_code() {
    let Some(hub) = start_hub() else {
        return;
    };
    let (mac, phone) = two_devices(&hub.base);
    {
        let mut m = mac.open();
        m.set("r1", b"Buy milk".to_vec()).unwrap();
        m.sync_now().unwrap();
        let p = phone.open();
        p.sync_now().unwrap();
        assert_eq!(text(&p, "r1").as_deref(), Some("Buy milk"), "after the yes");
    }

    // The Mac made the key: its recovery code waits to be shown, and stays
    // until it is put away.
    let (dir, base) = mac.me();
    let mac_me = me(&dir, &base, &mac.key, mac.name);
    let code = overview::recovery_code(&mac_me).expect("the code to show once");
    assert_eq!(
        overview::recovery_code(&mac_me).as_deref(),
        Some(code.as_str())
    );
    assert!(
        overview::recovery_code(&me(&phone.device_dir(), &base, &phone.key, phone.name)).is_none(),
        "only the device that made the key has one"
    );

    // A third device, nobody to approve it.
    let ipad = Device::new(&hub.base, "iPad");
    assert!(!ipad.look().holds_key);
    let (idir, ibase) = ipad.me();
    let ipad_me = me(&idir, &ibase, &ipad.key, ipad.name);
    let wrong = overview::recover(&ipad_me, "ABCDE-FGHJK-MNPQR-STVWX-YZ012-3").unwrap_err();
    assert!(wrong.contains("did not open"), "{wrong}");
    let short = overview::recover(&ipad_me, "abc").unwrap_err();
    assert!(short.contains("26 letters"), "{short}");
    overview::recover(&ipad_me, &code.to_lowercase()).expect("the code opens it");
    assert!(ipad.look().holds_key);
    assert_eq!(
        text(
            &{
                let kv = ipad.open();
                kv.sync_now().unwrap();
                kv
            },
            "r1"
        )
        .as_deref(),
        Some("Buy milk")
    );

    // Written down: gone from the Mac.
    overview::forget_recovery_code(&mac_me);
    assert_eq!(overview::recovery_code(&mac_me), None);

    // A yes from a device that does not hold the key is refused in words.
    let late = Device::new(&hub.base, "Old laptop");
    let waiting_id = late.look().device_id;
    let refused = overview::approve(
        &me(&late.device_dir(), &base, &late.key, late.name),
        &waiting_id,
    )
    .unwrap_err();
    assert!(refused.contains("waiting"), "{refused}");
}
