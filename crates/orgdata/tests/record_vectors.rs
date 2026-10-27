//! Sealed team-data rows, across a computer and a browser tab (G1).
//!
//! `tests/vectors/org-records.json` holds rows sealed by this crate's own
//! sealing (`krate_orgkeys::AppDataKey`, through `mirror::payload`), each
//! with the nonce it was sealed with. This test proves the file is what the
//! code makes, byte for byte, and opens; the hub's Node test
//! (cloud/worker/test/player-team-data.test.mjs) proves the browser path --
//! the same crate compiled into validator.wasm, called from the Player's
//! page -- makes and opens exactly the same bytes. Remake the file with
//!
//!   KRATE_WRITE_ORG_RECORD_VECTORS=1 cargo test -p krate-orgdata --test record_vectors

use krate_orgdata::mirror::{open_rows, payload};
use krate_orgkeys::{hex, unhex, AppDataKey, Keyring, RowKind};
use serde_json::{json, Value};

const PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/org-records.json"
);

/// The inputs: an organisation key from a fixed secret, and rows of each
/// kind, each with its own fixed nonce.
fn inputs() -> Value {
    json!({
        "about": "Team data rows (store.org, F-316) sealed by crates/orgdata on a computer and by the same crate in validator.wasm in a browser tab. The organisation key is Keyring::from_secret(org, org_secret) at version 1; the app's data key is AppDataKey::derive(keyring, 1, app_id). r and k are the opaque record and field names, payload is the sealed plaintext, ct is base64 of nonce || XChaCha20-Poly1305(payload) bound to version, r, k and kind. Both suites must make exactly ct from the inputs and open it. Remake with KRATE_WRITE_ORG_RECORD_VECTORS=1 cargo test -p krate-orgdata --test record_vectors.",
        "org": "acme",
        "org_secret": "7e985170f9e4f25fe43daf8603d692ce6aaba1a9c2b25b681d5713ca45ba9dd1",
        "app_id": "dev.acme.inspections",
        "version": 1,
        "cases": [
            { "c": "sites", "i": "s1", "f": "name", "v": { "t": "North depot" }, "w": "dana", "nonce": "654e2c87d7820cbeb1b5b550f43166549582b5b353e00e27" },
            { "c": "sites", "i": "s1", "f": "score", "v": { "n": 7.5 }, "w": "dana", "nonce": "e889d30b85421e8cc830d15f3b5008c1896600e9a9129e09" },
            { "c": "sites", "i": "s1", "f": "passed", "v": { "b": true }, "w": "dana@acme.example", "nonce": "801125136d32a6287ce42b2630f06d53dcb3a0c8de7d1ace" },
            { "c": "sites", "i": "s2", "f": "photo", "v": { "x": "AAEC/f7/" }, "w": "lee", "nonce": "f5429debab559ab33bd308824a4ac7482e018ff3b3218d32" },
            { "c": "sites", "i": "s2", "f": "note", "v": null, "w": "lee", "nonce": "84227633ddd095ca62bcee3b4ecbe695a2c23911cfc6ef68" },
            { "c": "sites", "i": "s3", "f": "", "v": null, "w": "lee", "nonce": "1a48cc77f42ba0a7770bb1e93f88d3cd33e169c10e29d7f9" },
            { "c": "notes", "i": "n-é", "f": "body", "v": { "t": "Façade crack ✓ 2 m" }, "w": "dana", "nonce": "59672ff3c8d9065317cda543752b5ac278832b2ba0dbb345" }
        ]
    })
}

fn kind_of(f: &str, v: &Value) -> RowKind {
    if f.is_empty() {
        RowKind::Tombstone
    } else if v.is_null() {
        RowKind::Null
    } else {
        RowKind::Value
    }
}

fn made() -> Value {
    let mut out = inputs();
    let secret = unhex(out["org_secret"].as_str().unwrap()).unwrap();
    let keyring = Keyring::from_secret("acme", &secret).unwrap();
    let dk = AppDataKey::derive(&keyring, 1, out["app_id"].as_str().unwrap()).unwrap();
    out["org_public"] = json!(keyring.current().1.public_hex());
    for case in out["cases"].as_array_mut().unwrap() {
        let (c, i, f, w) = (
            case["c"].as_str().unwrap().to_string(),
            case["i"].as_str().unwrap().to_string(),
            case["f"].as_str().unwrap().to_string(),
            case["w"].as_str().unwrap().to_string(),
        );
        let kind = kind_of(&f, &case["v"]);
        let (r, k) = (dk.record_key(&c, &i), dk.field_key(&c, &i, &f));
        let plain = payload(&c, &i, &f, &case["v"], &w);
        let nonce: [u8; 24] = unhex(case["nonce"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let ct = dk
            .seal_with_nonce(&r, &k, kind, plain.as_bytes(), &nonce)
            .unwrap();
        case["r"] = json!(r);
        case["k"] = json!(k);
        case["kind"] = json!(kind.as_str());
        case["payload"] = json!(plain);
        case["ct"] = json!(ct);
    }
    out
}

#[test]
fn the_record_vectors_are_what_this_crate_seals_and_they_open() {
    let made = made();
    if std::env::var_os("KRATE_WRITE_ORG_RECORD_VECTORS").is_some() {
        std::fs::write(PATH, serde_json::to_string_pretty(&made).unwrap() + "\n").unwrap();
    }
    let file: Value = serde_json::from_str(&std::fs::read_to_string(PATH).unwrap()).unwrap();
    assert_eq!(
        file, made,
        "the vectors are not what crates/orgdata seals now"
    );

    // They open, through the same function a sync opens rows with, to the
    // names and values they were sealed under.
    let secret = unhex(file["org_secret"].as_str().unwrap()).unwrap();
    let keyring = Keyring::from_secret("acme", &secret).unwrap();
    let rows: Vec<Value> = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({ "r": c["r"], "k": c["k"], "kind": c["kind"], "ver": 1, "ct": c["ct"], "s": "0001760000000000-00000000-0123456789abcdef", "by": "hub-says" }))
        .collect();
    let (plain, rejected) = open_rows(
        &json!({ "rows": rows.clone() }),
        &keyring,
        file["app_id"].as_str().unwrap(),
    );
    assert_eq!(rejected, 0);
    for (case, row) in file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(plain["rows"].as_array().unwrap())
    {
        assert_eq!(
            (&row["c"], &row["i"], &row["f"], &row["v"]),
            (&case["c"], &case["i"], &case["f"], &case["v"])
        );
        // The writer is the sealed one, not what the hub says.
        assert_eq!(row["by"], case["w"]);
    }
    // One flipped bit anywhere in a ciphertext, and it is refused.
    let mut tampered = rows[0].clone();
    let mut bytes = krate_orgkeys::b64_decode(tampered["ct"].as_str().unwrap()).unwrap();
    let at = bytes.len() - 1;
    bytes[at] ^= 0x01;
    tampered["ct"] = json!(krate_orgkeys::b64_encode(&bytes));
    let (_, rejected) = open_rows(
        &json!({ "rows": [tampered] }),
        &keyring,
        "dev.acme.inspections",
    );
    assert_eq!(rejected, 1);
    assert_eq!(
        hex(&keyring.current().1.public()),
        file["org_public"].as_str().unwrap()
    );
}
