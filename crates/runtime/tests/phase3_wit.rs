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
    assert_eq!(resolve.packages.len(), 9);
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
