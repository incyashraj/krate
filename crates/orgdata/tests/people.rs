//! The people an organisation app may name (IC-979): what a sync says is
//! kept with the copy, read offline after a restart, and left alone by an
//! answer from a hub that says nothing of people.

use krate_orgdata::mirror::Mirror;
use serde_json::json;

fn answer(people: Option<serde_json::Value>) -> serde_json::Value {
    let mut a = json!({
        "v": 1, "now": 1760400000000u64, "accepted": 0, "key_version": 1,
        "rows": [], "cursor": "0", "more": false, "full": false,
        "me": { "name": "carol", "role": "developer" },
        "org": { "slug": "acme", "name": "Acme" },
    });
    if let Some(people) = people {
        a["people"] = people;
    }
    a
}

#[test]
fn a_sync_names_the_people_and_the_copy_keeps_them() {
    let mut m = Mirror::fresh("aaaaaaaaaaaaaaaa".into());
    assert!(m.members().is_empty(), "nobody before a hub has said");
    let round = m.prepare(1760400000000);
    let changed = m.apply(
        &round,
        &answer(Some(json!([
            { "name": "carol", "role": "developer" },
            { "name": "dan@acme.example", "role": "recipient" },
            { "name": "", "role": "recipient" },
        ]))),
    );
    assert!(!changed, "people are not a change to the team's data");
    let names: Vec<(String, String, String)> = m
        .members()
        .into_iter()
        .map(|p| (p.name, p.role, p.org))
        .collect();
    assert_eq!(
        names,
        vec![
            ("carol".into(), "developer".into(), "Acme".into()),
            ("dan@acme.example".into(), "recipient".into(), "Acme".into()),
        ],
        "an empty name is dropped"
    );

    // Offline after a restart: read from the saved copy.
    let back = Mirror::from_json(&m.to_json().expect("save")).expect("load");
    assert_eq!(back.members(), m.members());

    // A hub that says nothing of people leaves them as they were.
    let mut later = back.clone();
    let round = later.prepare(1760400001000);
    later.apply(&round, &answer(None));
    assert_eq!(later.members(), m.members());
    let round = later.prepare(1760400002000);
    let same = json!([
        { "name": "carol", "role": "developer" },
        { "name": "dan@acme.example", "role": "recipient" },
    ]);
    later.apply(&round, &answer(Some(same)));
    assert_eq!(later.members(), m.members());
    // Dan taken out of the audience: gone.
    let round = later.prepare(1760400003000);
    later.apply(
        &round,
        &answer(Some(json!([{ "name": "carol", "role": "developer" }]))),
    );
    assert_eq!(later.members().len(), 1);

    // A copy saved before people existed still loads.
    let mut old: serde_json::Value = serde_json::from_str(&m.to_json().unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("people");
    assert!(Mirror::from_json(&old.to_string())
        .expect("old copy")
        .members()
        .is_empty());
}
