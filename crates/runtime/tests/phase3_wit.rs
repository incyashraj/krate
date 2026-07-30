use std::path::PathBuf;

use wit_parser::{Resolve, WorldItem};

#[test]
fn phase_3_gui_wit_package_parses() {
    let wit_dir = workspace_root().join("wit/krate/phase3");
    let mut resolve = Resolve::default();
    let (package, _) = resolve.push_dir(&wit_dir).expect("parse Phase 3 WIT");

    let world = resolve
        .select_world(&[package], Some("gui"))
        .expect("select krate:app/gui world");

    let gui = &resolve.worlds[world];
    let imports = gui
        .imports
        .values()
        .filter(|item| matches!(item, WorldItem::Interface { .. }))
        .count();

    assert!(
        imports >= 20,
        "gui world should expose Phase 2 plus Phase 3 imports"
    );
    assert_eq!(gui.exports.len(), 1);
    // Named rather than counted: a bare number says something changed without
    // saying what, and adding a capability moves it for a good reason.
    let packages: std::collections::BTreeSet<String> = resolve
        .packages
        .iter()
        .map(|(_, package)| package.name.to_string())
        .collect();
    for expected in [
        "krate:app@0.2.0",
        "krate:io@0.1.0",
        "krate:fs@0.1.0",
        "krate:net@0.1.0",
        "krate:time@0.1.0",
        "krate:locale@0.1.0",
        "krate:resources@0.1.0",
        "krate:store@0.1.0",
        "krate:ui@0.1.0",
        "krate:gfx@0.1.0",
        "krate:audio@0.1.0",
        "krate:speech@0.1.0",
    ] {
        assert!(
            packages.contains(expected),
            "the gui world lost {expected}; packages are {packages:?}"
        );
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
