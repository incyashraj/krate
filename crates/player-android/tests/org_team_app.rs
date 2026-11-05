//! A team app on a phone, against the real hub (K-1171).
//!
//! The hub is cloud/worker/src/index.js itself with the real Org Durable
//! Object, run by Node on this machine (cloud/worker/test/org-phone-hub.mjs):
//! acme, its owner alice, its reviewer bob, and a starter app as the site
//! serves it, released to acme. The phone is the Player's own run decision
//! (`run_config::org_run`, the code both phone Players call before a run)
//! over a private folder of its own; the computer is the same organisation
//! rules through crates/orgaccess, as the CLI runs them. Both read and write
//! the team's data through the runtime's own `store.org` (AppOrg).
//!
//! What it proves, on the path a person takes:
//!
//! - bob's phone opens acme's app as bob: it checks in, gets its team data,
//!   and -- new to the team -- says it is waiting to be let in, rather than
//!   showing an empty team;
//! - alice's computer, doing what any member's device with an acme app open
//!   does, lets the phone in;
//! - the phone then reads what the computer wrote, and the computer reads
//!   what the phone wrote;
//! - someone outside acme, or nobody signed in, is refused at the door in
//!   words, and nothing runs.
//!
//! It needs Node and the hub's built wasm (`scripts/build-hub-validator.sh`,
//! `scripts/build-web-player.sh`) and `npm ci` in cloud/worker. Without them
//! it says so and passes; with `KRATE_REQUIRE_HUB_TESTS=1` it fails.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use krate_player::run_config::{org_run, Phone, WAITING_TITLE};
use krate_runtime::org_merge::Value;
use krate_runtime::org_store::{AppOrg, Link, OrgAccess};

struct HubProcess {
    child: Child,
    base: String,
    digest: String,
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

/// The hub with acme in it and `starter` released to it, written to `out`.
fn start_hub(starter: &str, out: &Path) -> Option<HubProcess> {
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
        .arg(worker.join("test/org-phone-hub.mjs"))
        .arg(starter)
        .arg(out)
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
        digest: said["digest"].as_str().expect("digest").to_string(),
        child,
    })
}

/// Alice's computer: the organisation rules the CLI runs, over a folder of
/// its own (its `~/.krate`).
struct Desk {
    home: PathBuf,
    hub: String,
}

impl krate_orgaccess::Device for Desk {
    fn home(&self) -> PathBuf {
        self.home.clone()
    }
    fn machine_key(&self) -> Vec<u8> {
        b"alice's computer".repeat(2)
    }
    fn signed_in(&self) -> Option<krate_orgaccess::SignedIn> {
        Some(krate_orgaccess::SignedIn {
            login: "alice".into(),
            token: "gho_alice".into(),
        })
    }
    fn hub(&self) -> Result<String, krate_orgaccess::GateError> {
        Ok(self.hub.clone())
    }
    fn kind(&self) -> &str {
        "computer"
    }
    fn name(&self, _principal: Option<&str>) -> String {
        "Alice's MacBook".into()
    }
    fn sign_in_words(&self) -> String {
        "sign in with `krate account login`".into()
    }
    fn tell(&self, _title: &str, _words: &str) {}
}

fn phone(dir: &Path, hub: &str, who: Option<&str>, name: &str) -> Phone {
    Phone {
        private: dir.to_path_buf(),
        machine_key: name.as_bytes().repeat(4),
        signed_in: who.map(|login| krate_orgaccess::SignedIn {
            login: login.to_string(),
            token: format!("gho_{login}"),
        }),
        hub: hub.to_string(),
        name: name.to_string(),
        told: Default::default(),
    }
}

/// Ask `done` every half second until it answers true or `secs` pass.
fn until(secs: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    done()
}

fn text(v: &str) -> Value {
    Value::Text(v.to_string())
}

fn title_of(org: &AppOrg, id: &str) -> Option<String> {
    org.get("notes", id)
        .expect("get")
        .and_then(|entry| entry.fields.into_iter().find(|(k, _)| k == "title"))
        .and_then(|(_, v)| match v {
            Value::Text(t) => Some(t),
            _ => None,
        })
}

#[test]
fn a_team_app_on_a_phone_waits_to_be_let_in_then_reads_the_teams_records() {
    let dir = tempfile::tempdir().expect("dir");
    let bundle_path = dir.path().join("team-notes.krate");
    let Some(hub) = start_hub("team-notes.krate", &bundle_path) else {
        return;
    };
    let base = hub.base.as_str();
    let open = krate_bundle::open(&bundle_path).expect("the released bundle opens");
    let manifest = krate_manifest::Manifest::parse(
        &std::fs::read_to_string(open.manifest_path()).expect("manifest"),
    )
    .expect("manifest parses");
    let policy = krate_policy::SessionPolicy::allow_all_declared(&manifest).expect("policy");

    // Alice's computer: acme's first key, made on it, then a note written
    // and sent up through the runtime's own store.org.
    let desk = Desk {
        home: dir.path().join("alice-computer"),
        hub: base.to_string(),
    };
    let pair = krate_orgaccess::org_device_key(&desk).expect("the computer's key");
    krate_orgkeys::agent::Agent::new(
        base,
        "acme",
        Some("Bearer gho_alice".into()),
        pair,
        "Alice's MacBook",
    )
    .init()
    .expect("alice makes acme's key on her computer");
    let opened = krate_orgaccess::gate_opened(
        &desk,
        krate_orgaccess::Bytes::File(&bundle_path),
        krate_orgaccess::mark_of(&open),
    )
    .expect("alice may open it")
    .expect("it is acme's private app");
    assert_eq!(opened.digest, hub.digest);
    assert_eq!(opened.org_name, "Acme");
    let computer = AppOrg::open(
        krate_orgaccess::org_access(&desk, &opened, &manifest.app.id, 0).expect("team data"),
    );
    computer
        .put("notes", "n1", vec![("title".into(), text("Order chairs"))])
        .expect("alice writes");
    let sent = computer.sync().expect("alice syncs");
    assert!(sent.online && sent.pending == 0, "{sent:?}");

    // Bob's phone, new: it opens acme's app as bob, is given its team data,
    // and says it is waiting to be let in.
    let bobs = phone(&dir.path().join("bob-phone"), base, Some("bob"), "Pixel 8");
    let run = org_run(&bobs, &bundle_path, &open, &manifest, &policy).expect("bob may open it");
    assert_eq!(
        run.standing,
        Some(krate_orgaccess::KeyStanding::Waiting),
        "a new phone waits for its team's key"
    );
    let (title, words) = run
        .notices
        .iter()
        .find(|(title, _)| title == WAITING_TITLE)
        .expect("the phone says it is waiting");
    assert_eq!(
        title,
        "Waiting for someone on your team to let this phone in"
    );
    assert!(words.contains("This phone is new to Acme"), "{words}");
    assert!(
        run.notices
            .iter()
            .any(|(title, words)| title == "Offline access" && words.contains("Acme's private app")),
        "the offline terms are shown before the first open: {:?}",
        run.notices
    );
    let Some(OrgAccess::Granted(grant)) = run.app_org else {
        panic!("a member's phone is given the team's data");
    };
    assert_eq!(grant.slug, "acme");
    assert_eq!(grant.device_name, "Pixel 8");
    assert!(
        grant
            .mirror
            .starts_with(dir.path().join("bob-phone/org-data/acme")),
        "the team data lives in the Player's private storage: {}",
        grant.mirror.display()
    );
    assert!(
        dir.path().join("bob-phone/org-keys/device.json").exists(),
        "the phone's organisation key is kept in its private storage"
    );
    let on_phone = AppOrg::open(OrgAccess::Granted(grant));
    on_phone
        .put("notes", "n2", vec![("title".into(), text("Book the hall"))])
        .expect("a write while waiting is kept");
    let waiting = on_phone.sync().expect("the phone syncs");
    assert!(waiting.online, "{waiting:?}");
    assert_eq!(waiting.pending, 1, "its write waits for the key");
    assert_eq!(on_phone.status().expect("status").link, Link::WaitingForKey);
    assert_eq!(title_of(&on_phone, "n1"), None, "nothing of the team's yet");

    // The devices list shows the phone as bob's, waiting.
    let pair = krate_orgaccess::org_device_key(&desk).expect("the computer's key");
    let view = krate_orgkeys::agent::Agent::new(
        base,
        "acme",
        Some("Bearer gho_alice".into()),
        pair,
        "Alice's MacBook",
    )
    .view()
    .expect("the keyring's view");
    assert!(
        view["requests"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["principal"] == "github:bob"),
        "alice's computer sees bob's phone waiting: {view}"
    );

    // Alice's computer, in its next sync, lets the phone in. A sync asked
    // for sooner than the runtime's floor waits for the next turn, so both
    // devices sync as their apps would, until the phone has the team's
    // records (or half a minute has gone).
    let settled = until(30, || {
        let _ = computer.sync();
        let _ = on_phone.sync();
        title_of(&on_phone, "n1").is_some() && on_phone.status().expect("status").pending == 0
    });
    assert!(
        settled,
        "the phone was never let in: {:?}",
        on_phone.status()
    );
    assert_eq!(
        title_of(&on_phone, "n1").as_deref(),
        Some("Order chairs"),
        "the phone reads what the computer wrote"
    );
    assert_ne!(on_phone.status().expect("status").link, Link::WaitingForKey);
    assert!(
        until(30, || {
            let _ = computer.sync();
            title_of(&computer, "n2").is_some()
        }),
        "the computer never read the phone's note"
    );
    assert_eq!(
        title_of(&computer, "n2").as_deref(),
        Some("Book the hall"),
        "the computer reads what the phone wrote"
    );

    // Opened again, the phone holds the key: no waiting words.
    let again = org_run(&bobs, &bundle_path, &open, &manifest, &policy).expect("bob again");
    assert_eq!(again.standing, Some(krate_orgaccess::KeyStanding::Holds));
    assert!(
        again
            .notices
            .iter()
            .all(|(title, _)| title != WAITING_TITLE),
        "{:?}",
        again.notices
    );

    // Someone outside acme is refused at the door, in words; so is a phone
    // nobody is signed in on.
    let strangers = phone(
        &dir.path().join("stranger-phone"),
        base,
        Some("stranger"),
        "Galaxy",
    );
    let refused = org_run(&strangers, &bundle_path, &open, &manifest, &policy)
        .err()
        .expect("a stranger is refused");
    assert!(
        refused.contains("stranger is not one of its members"),
        "{refused}"
    );
    assert!(
        !dir.path().join("stranger-phone/org-data").exists(),
        "a refused phone keeps no team data"
    );
    let nobody = phone(&dir.path().join("signed-out-phone"), base, None, "Galaxy");
    let refused = org_run(&nobody, &bundle_path, &open, &manifest, &policy)
        .err()
        .expect("nobody signed in is refused");
    assert!(
        refused.contains("sign in on Krate's Profile tab"),
        "{refused}"
    );

    // The link a member is sent downloads with their sign-in, and not
    // without it.
    let link = format!("{base}/a/{}", hub.digest);
    let fetched = krate_orgaccess::download(&bobs, &link, 64 << 20, "Krate test")
        .expect("bob's phone downloads acme's app");
    assert_eq!(krate_bundle::sha256_hex(&fetched), hub.digest);
    let refused = krate_orgaccess::download(&strangers, &link, 64 << 20, "Krate test")
        .expect_err("a stranger's phone is refused the download");
    assert!(
        matches!(refused, krate_orgaccess::GateError::Refused(ref r) if r.reason == "not-a-member"),
        "{refused:?}"
    );
}
