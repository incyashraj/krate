//! An organisation's team data in a browser tab (`store.org`, G1): the
//! device's copy, the merge and the sealing, exactly as a computer runs them
//! -- `krate_orgdata`, the crate `krate-runtime`'s `org_store` is built on.
//!
//! The Player page (crates/adapter-web/player/web/org-data.js) keeps the
//! copy in IndexedDB and talks to the hub; everything that decides anything
//! happens here. The module holds each open copy in its own memory between
//! calls, keyed by the page's `store` name, so a read never re-parses the
//! whole copy. After a change the answer carries the copy as text
//! (`mirror`), for the page to keep -- what a computer writes to its file.
//!
//! One call, `{"op": ..., ...}`, answered `{"ok":true, ...}` or
//! `{"ok":false,"error":<the WIT org-error as JSON>}` (`{"tag":"invalid",
//! "val":"..."}`, `{"tag":"too-large"}`, `{"tag":"over-quota"}` or
//! `{"tag":"io","val":"..."}`). Sealing takes the page's randomness
//! (`entropy`) as every call in browser.rs does: 24 bytes per row.

use std::cell::RefCell;
use std::collections::BTreeMap;

use krate_orgdata::merge::{self, Stamp, Value};
use krate_orgdata::mirror::{self, DataError, Entry, Mirror, Page, Round};
use krate_orgkeys::Keyring;
use serde_json::{json, Value as Json};

use crate::browser::{device_of, org_of, text, with_entropy};

/// One open copy, and the round of a sync in flight for it.
struct Open {
    mirror: Mirror,
    round: Option<Round>,
}

thread_local! {
    static STORES: RefCell<BTreeMap<String, Open>> = const { RefCell::new(BTreeMap::new()) };
}

/// A call's failure, as the app would see it.
enum Fail {
    Data(DataError),
    Io(String),
}

impl From<DataError> for Fail {
    fn from(e: DataError) -> Self {
        Fail::Data(e)
    }
}

impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Io(e)
    }
}

impl From<&str> for Fail {
    fn from(e: &str) -> Self {
        Fail::Io(e.to_string())
    }
}

fn error_json(fail: Fail) -> Json {
    match fail {
        Fail::Data(DataError::Invalid(words)) => json!({ "tag": "invalid", "val": words }),
        Fail::Data(DataError::TooLarge) => json!({ "tag": "too-large" }),
        Fail::Data(DataError::OverQuota) => json!({ "tag": "over-quota" }),
        Fail::Io(words) => json!({ "tag": "io", "val": words }),
    }
}

/// One team-data call, answered as JSON.
pub fn org_data_to_json(bytes: &[u8]) -> String {
    let answer = serde_json::from_slice::<Json>(bytes)
        .map_err(|e| Fail::Io(format!("not JSON: {e}")))
        .and_then(|input| op(&input));
    match answer {
        Ok(mut value) => {
            value["ok"] = json!(true);
            value.to_string()
        }
        Err(fail) => json!({ "ok": false, "error": error_json(fail) }).to_string(),
    }
}

fn with_store<T>(input: &Json, f: impl FnOnce(&mut Open) -> Result<T, Fail>) -> Result<T, Fail> {
    let name = text(input, "store")?;
    STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        let open = stores
            .get_mut(name)
            .ok_or("this copy is not open: open it first")?;
        f(open)
    })
}

fn str_of<'a>(input: &'a Json, key: &str) -> Result<&'a str, Fail> {
    input[key]
        .as_str()
        .ok_or_else(|| Fail::Io(format!("{key} is text")))
}

fn ms_of(input: &Json, key: &str) -> Result<i64, Fail> {
    input[key]
        .as_f64()
        .filter(|n| n.is_finite())
        .map(|n| n as i64)
        .ok_or_else(|| Fail::Io(format!("{key} is milliseconds")))
}

fn mirror_text(mirror: &Mirror) -> Result<String, Fail> {
    mirror.to_json().map_err(Fail::Io)
}

fn entry_json(entry: &Entry) -> Json {
    json!({
        "id": entry.id,
        "fields": entry.fields.iter().map(|(name, value)| json!([name, value.to_wire()])).collect::<Vec<_>>(),
        "changed_by": entry.changed_by,
        "changed_at": entry.changed_at,
    })
}

fn page_json(page: &Page) -> Json {
    json!({
        "entries": page.entries.iter().map(entry_json).collect::<Vec<_>>(),
        "removed": page.removed,
        "next": page.next,
        "more": page.more,
    })
}

/// The fields of a put, `[[name, wire value], ...]`.
fn fields_of(input: &Json) -> Result<Vec<(String, Value)>, Fail> {
    let list = input["fields"]
        .as_array()
        .ok_or("fields is a list of [name, value]")?;
    list.iter()
        .map(|pair| {
            let name = pair[0].as_str().ok_or("a field's name is text")?;
            let value = Value::from_wire(&pair[1]).ok_or_else(|| {
                Fail::Data(DataError::Invalid(format!(
                    "the field {name} has a value that is not text, a finite number, true or false, bytes or null"
                )))
            })?;
            Ok((name.to_string(), value))
        })
        .collect()
}

/// The keyring this tab holds: its envelope opened with the device's key.
fn keyring_of(input: &Json) -> Result<Keyring, Fail> {
    let org = org_of(input)?;
    let envelope = text(input, "envelope")?;
    Keyring::open(envelope, &device_of(input)?, org)
        .map_err(|e| Fail::Io(format!("the organisation key: {e}")))
}

fn op(input: &Json) -> Result<Json, Fail> {
    match text(input, "op")? {
        "open" => {
            let name = text(input, "store")?.to_string();
            let held = input["mirror"].as_str().and_then(Mirror::from_json);
            let fresh = held.is_none();
            let mirror = match held {
                Some(mirror) => mirror,
                None => {
                    let device = text(input, "device")?;
                    if !merge::valid_device(device) {
                        return Err("device is 16 lower-case hex digits".into());
                    }
                    Mirror::fresh(device.to_string())
                }
            };
            let answer = json!({
                "fresh": fresh,
                "device": mirror.device,
                "pending": mirror.pending(),
                "envelope": mirror.envelope,
                "mirror": if fresh { Some(mirror_text(&mirror)?) } else { None },
            });
            STORES.with(|s| {
                s.borrow_mut().insert(
                    name,
                    Open {
                        mirror,
                        round: None,
                    },
                )
            });
            Ok(answer)
        }
        "close" => {
            let name = text(input, "store")?;
            STORES.with(|s| s.borrow_mut().remove(name));
            Ok(json!({}))
        }
        "reset" => with_store(input, |open| {
            // The person is no longer in the organisation: its data leaves.
            let device = text(input, "device")?;
            if !merge::valid_device(device) {
                return Err("device is 16 lower-case hex digits".into());
            }
            open.mirror = Mirror::fresh(device.to_string());
            open.round = None;
            Ok(json!({ "mirror": mirror_text(&open.mirror)? }))
        }),
        "put" => with_store(input, |open| {
            let (collection, id) = (str_of(input, "collection")?, str_of(input, "id")?);
            let fields = fields_of(input)?;
            let phys = open.mirror.clock(ms_of(input, "now")?, 0);
            let by = open.mirror.me_name(input["me"].as_str());
            let written = open.mirror.put(collection, id, fields, phys, &by)?;
            let text = if written {
                Some(mirror_text(&open.mirror)?)
            } else {
                None
            };
            Ok(json!({ "written": written, "mirror": text, "pending": open.mirror.pending() }))
        }),
        "delete" => with_store(input, |open| {
            let (collection, id) = (str_of(input, "collection")?, str_of(input, "id")?);
            let phys = open.mirror.clock(ms_of(input, "now")?, 0);
            let by = open.mirror.me_name(input["me"].as_str());
            let written = open.mirror.delete(collection, id, phys, &by)?;
            let text = if written {
                Some(mirror_text(&open.mirror)?)
            } else {
                None
            };
            Ok(json!({ "written": written, "mirror": text, "pending": open.mirror.pending() }))
        }),
        "get" => with_store(input, |open| {
            let entry = open
                .mirror
                .get(str_of(input, "collection")?, str_of(input, "id")?)?;
            Ok(json!({ "entry": entry.as_ref().map(entry_json) }))
        }),
        "records" => with_store(input, |open| {
            let page = open
                .mirror
                .records(str_of(input, "collection")?, input["since"].as_str())?;
            Ok(json!({ "page": page_json(&page) }))
        }),
        "who" => with_store(input, |open| {
            Ok(
                json!({ "member": open.mirror.who().map(|m| json!({ "name": m.name, "role": m.role, "org": m.org })) }),
            )
        }),
        "status" => with_store(input, |open| {
            let m = &open.mirror;
            Ok(json!({
                "device": m.device, "pending": m.pending(), "last_synced": m.synced_at,
                "envelope": m.envelope, "rejected": m.rejected, "used": m.used(), "quota": m.quota(),
            }))
        }),
        "set_envelope" => with_store(input, |open| {
            let changed = open
                .mirror
                .set_envelope(input["envelope"].as_str().map(str::to_string));
            Ok(json!({ "changed": changed, "mirror": mirror_text(&open.mirror)? }))
        }),
        "sync_begin" => {
            // Only the keyring in the envelope, with the page's randomness for
            // the nonces: what a computer's sync sends, sealed the same way.
            let keyring = keyring_of(input)?;
            let app_id = text(input, "app_id")?.to_ascii_lowercase();
            let now = ms_of(input, "now")?;
            with_store(input, |open| {
                let round = open.mirror.prepare(open.mirror.clock(now, 0));
                let need = 24 * round.writes.len();
                let have = input["entropy"]
                    .as_str()
                    .and_then(krate_orgkeys::b64_decode)
                    .map_or(0, |b| b.len());
                if have < need {
                    return Err(Fail::Io(format!(
                        "sealing {} changes needs {need} bytes of entropy from crypto.getRandomValues",
                        round.writes.len()
                    )));
                }
                let sealed = with_entropy(input, || {
                    mirror::seal_writes(&round.writes, &keyring, &app_id)
                })?;
                let body = round.body(sealed);
                let writes = round.writes.len();
                let left_behind = round.left_behind;
                open.round = Some(round);
                Ok(json!({ "body": body, "writes": writes, "left_behind": left_behind }))
            })
        }
        "sync_end" => {
            let keyring = keyring_of(input)?;
            let app_id = text(input, "app_id")?.to_ascii_lowercase();
            with_store(input, |open| {
                let round = open
                    .round
                    .take()
                    .ok_or("no sync is under way for this copy")?;
                let answer = &input["answer"];
                let (plain, rejected) = mirror::open_rows(answer, &keyring, &app_id);
                open.mirror.rejected += rejected;
                let changed = open.mirror.apply(&round, &plain);
                let again = answer["more"].as_bool().unwrap_or(false) || round.left_behind;
                Ok(json!({
                    "changed": changed, "again": again, "rejected": rejected,
                    "pending": open.mirror.pending(), "last_synced": open.mirror.synced_at,
                    "mirror": mirror_text(&open.mirror)?,
                }))
            })
        }
        "seal_rows" => {
            // Rows a device sends, sealed as a sync seals them: for the
            // cross-language vectors and for a test playing another device.
            let keyring = keyring_of(input)?;
            let app_id = text(input, "app_id")?.to_ascii_lowercase();
            let rows = input["rows"].as_array().ok_or("rows is a list")?;
            let sealed = with_entropy(input, || mirror::seal_writes(rows, &keyring, &app_id))?;
            Ok(json!({ "rows": sealed }))
        }
        "open_rows" => {
            let keyring = keyring_of(input)?;
            let app_id = text(input, "app_id")?.to_ascii_lowercase();
            let (plain, rejected) = mirror::open_rows(&input["answer"], &keyring, &app_id);
            Ok(json!({ "rows": plain["rows"], "rejected": rejected }))
        }
        "sync_abandon" => with_store(input, |open| {
            // Offline or refused: the round's writes stay pending.
            open.round = None;
            Ok(json!({ "pending": open.mirror.pending() }))
        }),
        // The merge rule itself, for the shared vectors
        // (cloud/worker/test/fixtures/org-data-vectors.json).
        "stamp_parse" => Ok(json!({ "valid": Stamp::parse(str_of(input, "text")?).is_some() })),
        "stamp_compare" => {
            let a = Stamp::parse(str_of(input, "a")?).ok_or("a is not a stamp")?;
            let b = Stamp::parse(str_of(input, "b")?).ok_or("b is not a stamp")?;
            Ok(json!({ "order": a.cmp(&b) as i8 }))
        }
        "stamp_next" => {
            let last = match input["last"].as_str() {
                Some(t) => Some(Stamp::parse(t).ok_or("last is not a stamp")?),
                None => None,
            };
            let phys = input["phys"].as_u64().ok_or("phys is milliseconds")?;
            Ok(
                json!({ "stamp": Stamp::next(last.as_ref(), phys, str_of(input, "device")?).to_string() }),
            )
        }
        "rebase" => {
            let s = Stamp::parse(str_of(input, "stamp")?).ok_or("not a stamp")?;
            let (d, h) = (
                input["device_now"].as_u64().ok_or("device_now")?,
                input["hub_now"].as_u64().ok_or("hub_now")?,
            );
            Ok(json!({ "stamp": merge::rebase(&s, d, h).to_string() }))
        }
        other => Err(Fail::Io(format!("no such operation: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_orgkeys::{hex, KeyPair};

    fn call(input: Json) -> Json {
        serde_json::from_str(&org_data_to_json(input.to_string().as_bytes())).unwrap()
    }

    /// The tab's copy answers as a computer's does: limits, paging, deletes,
    /// and a sync whose writes a computer's keyring opens and whose rows
    /// are refused when they do not open.
    #[test]
    fn a_tab_copy_writes_reads_seals_and_refuses_like_a_computer() {
        let store = "t1";
        let opened = call(json!({ "op": "open", "store": store, "device": "0123456789abcdef" }));
        assert_eq!(opened["fresh"], true, "{opened}");
        let put = call(json!({
            "op": "put", "store": store, "collection": "tasks", "id": "t1", "now": 1_760_000_000_000u64,
            "me": "dana", "fields": [["title", { "t": "Order chairs" }], ["done", { "b": false }]],
        }));
        assert_eq!(
            (put["written"].as_bool(), put["pending"].as_u64()),
            (Some(true), Some(2)),
            "{put}"
        );
        let got = call(json!({ "op": "get", "store": store, "collection": "tasks", "id": "t1" }));
        assert_eq!(
            got["entry"]["fields"],
            json!([["done", { "b": false }], ["title", { "t": "Order chairs" }]])
        );
        assert_eq!(got["entry"]["changed_by"], "dana");
        let bad = call(
            json!({ "op": "put", "store": store, "collection": "", "id": "x", "now": 1, "fields": [["a", null]] }),
        );
        assert_eq!(bad["error"]["tag"], "invalid", "{bad}");
        let big = call(json!({
            "op": "put", "store": store, "collection": "tasks", "id": "big", "now": 1,
            "fields": [["blob", { "t": "x".repeat(70_000) }]],
        }));
        assert_eq!(big["error"], json!({ "tag": "too-large" }));
        let gone = call(
            json!({ "op": "delete", "store": store, "collection": "tasks", "id": "nope", "now": 1 }),
        );
        assert_eq!(gone["written"], false);
        let page = call(json!({ "op": "records", "store": store, "collection": "tasks" }));
        assert_eq!(page["page"]["entries"].as_array().unwrap().len(), 1);

        // A sync: the keyring sealed to this tab's device key.
        let device = KeyPair::generate();
        let keyring = Keyring::create("acme");
        let envelope = keyring.seal_to(&device.public()).unwrap();
        let begin = call(json!({
            "op": "sync_begin", "store": store, "org": "acme", "app_id": "dev.acme.Tasks",
            "envelope": envelope, "device_secret": hex(device.secret()), "now": 1_760_000_000_100u64,
        }));
        // Native tests draw nonces from the system; the entropy check still
        // holds the page to bringing enough.
        assert_eq!(begin["error"]["tag"], "io", "{begin}");
        let begin = call(json!({
            "op": "sync_begin", "store": store, "org": "acme", "app_id": "dev.acme.Tasks",
            "envelope": envelope, "device_secret": hex(device.secret()), "now": 1_760_000_000_100u64,
            "entropy": krate_orgkeys::b64_encode(&[7u8; 48]),
        }));
        assert_eq!(begin["writes"], 2, "{begin}");
        let writes = begin["body"]["writes"].as_array().unwrap().clone();
        assert!(!begin["body"].to_string().contains("Order chairs"));
        // What a computer holding the same keyring opens.
        let (plain, rejected) =
            mirror::open_rows(&json!({ "rows": writes }), &keyring, "dev.acme.tasks");
        assert_eq!(rejected, 0);
        assert_eq!(plain["rows"][1]["v"], json!({ "t": "Order chairs" }));
        // The hub's answer: the rows back, plus one tampered and one from a
        // stranger's key.
        let mut tampered = writes[0].clone();
        let mut ct = krate_orgkeys::b64_decode(tampered["ct"].as_str().unwrap()).unwrap();
        ct[30] ^= 1;
        tampered["ct"] = json!(krate_orgkeys::b64_encode(&ct));
        let mut rows: Vec<Json> = writes
            .iter()
            .map(|w| {
                let mut r = w.clone();
                r["by"] = json!("dana");
                r
            })
            .collect();
        rows.push(tampered);
        let end = call(json!({
            "op": "sync_end", "store": store, "org": "acme", "app_id": "dev.acme.tasks",
            "envelope": envelope, "device_secret": hex(device.secret()),
            "answer": { "now": 1_760_000_000_200u64, "rows": rows, "cursor": "3", "me": { "name": "dana", "role": "developer" }, "org": { "name": "Acme" } },
        }));
        assert_eq!(
            (end["rejected"].as_u64(), end["pending"].as_u64()),
            (Some(1), Some(0)),
            "{end}"
        );
        let who = call(json!({ "op": "who", "store": store }));
        assert_eq!(
            who["member"],
            json!({ "name": "dana", "role": "developer", "org": "Acme" })
        );
        let status = call(json!({ "op": "status", "store": store }));
        assert_eq!(status["rejected"], 1);
        // Reopened from the text it handed out: the same copy.
        let reopened = call(
            json!({ "op": "open", "store": "t2", "mirror": end["mirror"], "device": "ffffffffffffffff" }),
        );
        assert_eq!(
            (reopened["fresh"].as_bool(), reopened["device"].as_str()),
            (Some(false), Some("0123456789abcdef"))
        );
        // Another organisation's name does not open this keyring.
        let wrong = call(json!({
            "op": "sync_begin", "store": store, "org": "globex", "app_id": "dev.acme.tasks",
            "envelope": envelope, "device_secret": hex(device.secret()), "now": 1,
        }));
        assert_eq!(wrong["error"]["tag"], "io", "{wrong}");
    }
}
