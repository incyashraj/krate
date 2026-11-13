//! An app's `store.sql` on two devices, one account, the real hub (IC-981
//! S4).
//!
//! The hub is cloud/worker/src/index.js itself, run by Node on this machine
//! (cloud/worker/test/account-sync-hub.mjs: the real SyncAccount object
//! over SQLite, R2 in memory); the devices are two of the runtime's own
//! synced databases -- the very type a run starts beside the app -- each
//! with the app's own connection beside it, as the app has, signed in as
//! one GitHub user through the hub's test stub.
//!
//! What it proves, on the path a person's rows take:
//!
//! - a row added on the Mac is read on the phone, and the reverse; an edit
//!   and a delete travel too; a row larger than an entry travels as a
//!   body; a sync with nothing new costs the hub no write;
//! - both editing one row apart: one conflict per row, each device reading
//!   its own; each choice -- this device's, the other's, both (a new row
//!   where the table holds two, refused in words where it does not),
//!   merge of JSON columns, delete against edit answered with the app
//!   closed -- settles both devices to the same database;
//! - a newer version of the app on the Mac (a column added) never has its
//!   rows written into the phone's older table: they wait, with words,
//!   while every row that fits keeps flowing both ways, and land the moment
//!   the phone updates;
//! - a second account sees none of it; the hub holds no table, row or app
//!   id, in the clear or in base64.
//!
//! It needs Node and the hub's built wasm (`scripts/build-hub-validator.sh`,
//! `scripts/build-web-player.sh`) and `npm ci` in cloud/worker. Without
//! them it says so and passes; with `KRATE_REQUIRE_HUB_TESTS=1` it fails.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use krate_accountsync::device::{account_of, Account};
use krate_accountsync::hub::Hub;
use krate_accountsync::{Resolution, ResolveError};
use krate_appsql::rusqlite::Connection;
use krate_runtime::account_sync_host::{AccountSyncContext, Step};
use krate_runtime::account_sync_sql::{
    resolve_row_at, row_conflicts_at, sync_sql_closed, RowConflict, SyncedSql,
};

const APP: &str = "dev.krate.lists";

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

fn store(dir: &Path) -> PathBuf {
    dir.join("lists.kv")
}

fn database(dir: &Path) -> PathBuf {
    dir.join("lists.sqlite")
}

/// The app's own connection, as sql_host opens it, with its tables made
/// the way the app makes them at start.
fn app(dir: &Path) -> Connection {
    let connection = Connection::open(database(dir)).unwrap();
    krate_appsql::prepare_connection(
        &connection,
        "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
    )
    .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, title TEXT, done INTEGER, items TEXT, photo BLOB);
             CREATE TABLE IF NOT EXISTS tags (name TEXT PRIMARY KEY, color TEXT);",
        )
        .unwrap();
    connection
}

fn device(hub: &str, who: &str, dir: &Path, name: &str) -> SyncedSql {
    SyncedSql::open(database(dir), store(dir), context(hub, who, dir, name))
}

fn account(hub: &str, who: &str, dir: &Path, name: &str) -> (Hub, Account) {
    let client = Hub::new(hub, &format!("gho_{who}"));
    let id = account_of(&client).expect("the hub names the account");
    let ctx = context(hub, who, dir, name);
    let account = Account::load(&ctx.device_dir, &id, &ctx.machine_key, name).expect("device");
    (client, account)
}

fn title(db: &Connection, id: i64) -> Option<String> {
    db.query_row("SELECT title FROM notes WHERE id = ?1", [id], |r| r.get(0))
        .ok()
}

/// Everything the app keeps, as text, to compare two devices.
fn everything(db: &Connection) -> String {
    let mut out = String::new();
    for (table, order) in [("notes", "id"), ("tags", "name")] {
        let mut statement = db
            .prepare(&format!("SELECT * FROM {table} ORDER BY {order}"))
            .unwrap();
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |r| {
                let mut line = Vec::new();
                for i in 0..columns {
                    line.push(format!("{:?}", r.get_ref(i).unwrap()));
                }
                Ok(line.join("|"))
            })
            .unwrap();
        for row in rows {
            out.push_str(&format!("{table}: {}\n", row.unwrap()));
        }
    }
    out
}

fn settle(devices: &[&SyncedSql]) {
    for _ in 0..6 {
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

fn find<'a>(conflicts: &'a [RowConflict], place: &str) -> &'a RowConflict {
    conflicts
        .iter()
        .find(|c| c.place == place)
        .unwrap_or_else(|| panic!("{place} in {conflicts:?}"))
}

#[test]
fn an_apps_rows_follow_the_person_and_nobody_else() {
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
    let mac_db = app(mac_d);
    mac_db
        .execute(
            "INSERT INTO notes (title, done) VALUES ('Buy milk ROW-PLAINTEXT-MARKER', 0)",
            [],
        )
        .unwrap();
    let mac = device(base, "ada", mac_d, "Mac");
    assert_eq!(mac.sync_now(), Ok(Step::Done));
    let phone_db = app(phone_d);
    let phone = device(base, "ada", phone_d, "Galaxy S22 Ultra");
    assert_eq!(phone.sync_now(), Ok(Step::Waiting));
    assert_eq!(title(&phone_db, 1), None, "nothing before the yes");
    let (client, mac_account) = account(base, "ada", mac_d, "Mac");
    let pending = mac_account.pending(&client).unwrap();
    let ring = mac_account.held().expect("the Mac holds the key");
    mac_account
        .approve(&client, &ring, &pending[0].device_id)
        .expect("the Mac says yes");

    // Mac to phone.
    assert_eq!(phone.sync_now(), Ok(Step::Done));
    assert_eq!(
        title(&phone_db, 1).as_deref(),
        Some("Buy milk ROW-PLAINTEXT-MARKER"),
        "a row added on the Mac is on the phone"
    );

    // Phone to Mac, with a photo larger than an entry carries.
    let photo: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    phone_db
        .execute(
            "INSERT INTO notes (id, title, done, photo) VALUES (2, 'Call mum', 0, ?1)",
            [&photo],
        )
        .unwrap();
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert_eq!(title(&mac_db, 2).as_deref(), Some("Call mum"), "and back");
    let mac_photo: Vec<u8> = mac_db
        .query_row("SELECT photo FROM notes WHERE id = 2", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mac_photo, photo, "a large row, whole");
    let bodies = dump(base)["r2"].clone();
    assert_eq!(
        bodies.as_object().unwrap().len(),
        1,
        "the large row is a body in R2"
    );
    for body in bodies.as_object().unwrap().values() {
        let raw = krate_orgkeys::b64_decode(body.as_str().unwrap()).unwrap();
        assert!(
            !raw.windows(64).any(|w| w == &photo[1000..1064]),
            "and sealed"
        );
    }

    // Nothing new: no hub write.
    let before = dump(base)["writes"].as_u64().unwrap();
    mac.sync_now().unwrap();
    phone.sync_now().unwrap();
    assert_eq!(
        dump(base)["writes"].as_u64().unwrap(),
        before,
        "a sync with nothing new writes nothing"
    );

    // An edit and a delete travel.
    mac_db
        .execute("UPDATE notes SET done = 1 WHERE id = 1", [])
        .unwrap();
    mac.sync_now().unwrap();
    phone.sync_now().unwrap();
    let done: i64 = phone_db
        .query_row("SELECT done FROM notes WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(done, 1, "an edit travels");
    phone_db
        .execute("DELETE FROM notes WHERE id = 2", [])
        .unwrap();
    phone.sync_now().unwrap();
    mac.sync_now().unwrap();
    assert_eq!(title(&mac_db, 2), None, "a delete travels");

    // Rows both will change apart.
    mac_db
        .execute_batch(
            "INSERT INTO notes (id, title, done) VALUES (3, 'Plan trip', 0), (4, 'Pay rent', 0), (6, 'Old idea', 0);
             INSERT INTO notes (id, title, items) VALUES (5, 'Packing', '[\"passport\"]');
             INSERT INTO tags VALUES ('home', 'blue');",
        )
        .unwrap();
    settle(&[&mac, &phone]);
    assert_eq!(everything(&mac_db), everything(&phone_db));
    mac_db
        .execute_batch(
            "UPDATE notes SET title = 'Plan trip to Rome' WHERE id = 3;
             UPDATE notes SET title = 'Pay rent today' WHERE id = 4;
             UPDATE notes SET items = '[\"passport\",\"charger\"]' WHERE id = 5;
             UPDATE tags SET color = 'green' WHERE name = 'home';
             DELETE FROM notes WHERE id = 6;",
        )
        .unwrap();
    phone_db
        .execute_batch(
            "UPDATE notes SET title = 'Plan trip to Oslo' WHERE id = 3;
             UPDATE notes SET title = 'Pay rent on Friday' WHERE id = 4;
             UPDATE notes SET items = '[\"passport\",\"tickets\"]' WHERE id = 5;
             UPDATE tags SET color = 'red' WHERE name = 'home';
             UPDATE notes SET title = 'Old idea, still good' WHERE id = 6;",
        )
        .unwrap();
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);
    assert_eq!(
        title(&mac_db, 3).as_deref(),
        Some("Plan trip to Rome"),
        "the Mac reads its own"
    );
    assert_eq!(
        title(&phone_db, 3).as_deref(),
        Some("Plan trip to Oslo"),
        "the phone its own"
    );
    let on_mac = mac.conflicts();
    assert_eq!(on_mac.len(), 5, "one conflict a row: {on_mac:#?}");
    let trip = find(&on_mac, "notes, id 3");
    assert_eq!(
        trip.mine.as_deref(),
        Some("title: Plan trip to Rome, done: 0")
    );
    assert_eq!(
        trip.others[0].0.as_deref(),
        Some("title: Plan trip to Oslo, done: 0")
    );
    assert_eq!(trip.others[0].1, "Galaxy S22 Ultra");
    assert!(trip.both, "a table keyed by its rowid holds two");
    assert!(!trip.mergeable, "two titles do not merge");
    assert!(
        find(&on_mac, "notes, id 5").mergeable,
        "two JSON lists merge"
    );
    let tag = find(&on_mac, "tags, name home");
    assert!(!tag.both, "one row per tag name");
    assert!(find(&on_mac, "notes, id 6").mine.is_none(), "deleted here");
    assert_eq!(phone.conflicts().len(), 5, "and the same on the phone");

    // Keep both, on the Mac: the phone's trip as a new row.
    mac.resolve(&trip.key, &Resolution::KeepBoth).unwrap();
    // The other device's, on the Mac.
    mac.resolve(&find(&on_mac, "notes, id 4").key, &Resolution::KeepOther(0))
        .unwrap();
    // Keep both where the table holds one row per key: refused, in words.
    let on_phone = phone.conflicts();
    let refused = phone
        .resolve(
            &find(&on_phone, "tags, name home").key,
            &Resolution::KeepBoth,
        )
        .unwrap_err();
    assert_eq!(refused, ResolveError::OneRowOnly);
    assert!(refused.to_string().contains("one row per key"), "{refused}");
    // This device's, on the phone.
    phone
        .resolve(
            &find(&on_phone, "tags, name home").key,
            &Resolution::KeepMine,
        )
        .unwrap();
    // Merge, on the phone.
    phone
        .resolve(&find(&on_phone, "notes, id 5").key, &Resolution::Merge)
        .unwrap();
    assert_eq!(
        phone.resolve(&find(&on_phone, "notes, id 4").key, &Resolution::Merge),
        Err(ResolveError::NotMergeable),
        "a refused merge decides nothing"
    );
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);

    // Delete against edit, answered on the Mac with the app closed: keep it
    // deleted, sent at once.
    drop(mac);
    let (_, mac_account) = account(base, "ada", mac_d, "Mac");
    let names = krate_accountsync::device::device_names(&client.view(None).unwrap());
    let closed = row_conflicts_at(&store(mac_d), &mac_account.id, &names);
    assert_eq!(closed.len(), 1, "only the old idea waits: {closed:?}");
    resolve_row_at(
        &store(mac_d),
        &mac_account.id,
        &names,
        &closed[0].key,
        &Resolution::KeepMine,
    )
    .unwrap();
    assert_eq!(
        sync_sql_closed(
            &store(mac_d),
            &mac_account.id,
            context(base, "ada", mac_d, "Mac")
        ),
        Ok(Step::Done)
    );
    let mac = device(base, "ada", mac_d, "Mac");
    settle(&[&mac, &phone]);

    for (who, db, sync) in [("Mac", &mac_db, &mac), ("phone", &phone_db, &phone)] {
        assert!(sync.conflicts().is_empty(), "settled on the {who}");
        assert_eq!(title(db, 3).as_deref(), Some("Plan trip to Rome"), "{who}");
        let copy: String = db
            .query_row(
                "SELECT title FROM notes WHERE id > 6 AND title = 'Plan trip to Oslo'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| panic!("keep both, on the {who}"));
        assert_eq!(copy, "Plan trip to Oslo");
        assert_eq!(title(db, 4).as_deref(), Some("Pay rent on Friday"), "{who}");
        let color: String = db
            .query_row("SELECT color FROM tags WHERE name = 'home'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(color, "red", "this device's, on the {who}");
        let items: String = db
            .query_row("SELECT items FROM notes WHERE id = 5", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&items).unwrap(),
            serde_json::json!(["passport", "tickets", "charger"]),
            "merge, on the {who}"
        );
        assert_eq!(title(db, 6), None, "kept deleted, on the {who}");
    }
    assert_eq!(everything(&mac_db), everything(&phone_db), "one database");

    // A newer version of the app on the Mac: a column added, a row using it.
    mac_db
        .execute_batch(
            "ALTER TABLE notes ADD COLUMN color TEXT;
             UPDATE notes SET title = 'Buy oat milk', color = 'red' WHERE id = 1;
             UPDATE tags SET color = 'gold' WHERE name = 'home';",
        )
        .unwrap();
    // The phone, still on the older version, edits another note.
    phone_db
        .execute("UPDATE notes SET title = 'Pay rent, done' WHERE id = 4", [])
        .unwrap();
    settle(&[&mac, &phone]);
    settle(&[&mac, &phone]);
    assert_eq!(
        title(&phone_db, 1).as_deref(),
        Some("Buy milk ROW-PLAINTEXT-MARKER"),
        "the newer row never lands in the older table"
    );
    let waits = phone.status().waits;
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert_eq!(waits[0].table, "notes");
    let words = krate_accountsync::sql::wait_words("Lists", "Mac", &waits[0]);
    assert!(
        words
            .starts_with("Update Lists on this device to keep it in sync: Mac has a newer version"),
        "{words}"
    );
    let gold: String = phone_db
        .query_row("SELECT color FROM tags WHERE name = 'home'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(gold, "gold", "a row that fits still travels");
    assert_eq!(
        title(&mac_db, 4).as_deref(),
        Some("Pay rent, done"),
        "the older version's edit fits the newer table"
    );
    let (red, oat): (String, String) = mac_db
        .query_row("SELECT color, title FROM notes WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        (red.as_str(), oat.as_str()),
        ("red", "Buy oat milk"),
        "untouched"
    );
    assert!(mac.status().waits.is_empty());

    // The phone updates: the row waiting lands, and the wait is gone.
    phone_db
        .execute("ALTER TABLE notes ADD COLUMN color TEXT", [])
        .unwrap();
    settle(&[&mac, &phone]);
    assert_eq!(title(&phone_db, 1).as_deref(), Some("Buy oat milk"));
    assert!(phone.status().waits.is_empty());
    assert!(mac.conflicts().is_empty() && phone.conflicts().is_empty());
    assert_eq!(
        everything(&mac_db),
        everything(&phone_db),
        "one database again"
    );

    // Another person, the same app: their own key, nothing of ada's.
    let bob_db = app(bob_dir.path());
    let bob = device(base, "bob", bob_dir.path(), "Bob's Mac");
    assert_eq!(bob.sync_now(), Ok(Step::Done));
    let bobs: i64 = bob_db
        .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bobs, 0, "bob's database stays empty");

    // What the hub keeps: no table, row or app id, in the clear or base64.
    let dump = dump(base);
    let kv = dump["kv"].to_string();
    let r2 = dump["r2"].to_string();
    let mut r2_bytes = Vec::new();
    for value in dump["r2"].as_object().unwrap().values() {
        r2_bytes.extend(krate_orgkeys::b64_decode(value.as_str().unwrap()).unwrap());
    }
    for plain in [
        "ROW-PLAINTEXT-MARKER",
        "Plan trip to Oslo",
        "notes\n",
        "\"title\"",
        "passport",
        "tags\n",
        APP,
        "unsigned",
    ] {
        for (place, bytes) in [("the object", &kv), ("R2", &r2)] {
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
        "MEASURE the hub took {} writes and {} R2 writes for this whole walk",
        dump["writes"], dump["r2_writes"]
    );
}
