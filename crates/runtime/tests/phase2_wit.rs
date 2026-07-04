use std::path::PathBuf;

use wit_parser::Resolve;

#[test]
fn phase_2_uapi_wit_package_parses() {
    let wit_dir = workspace_root().join("wit/krate/phase2");
    let mut resolve = Resolve::default();
    let (package, _) = resolve.push_dir(&wit_dir).expect("parse Phase 2 WIT");

    let world = resolve
        .select_world(&[package], Some("cli"))
        .expect("select krate:app/cli world");

    let cli = &resolve.worlds[world];
    assert!(
        cli.imports.len() >= 8,
        "cli world should expose the Phase 2 UAPI imports"
    );
    assert_eq!(cli.exports.len(), 1);
    assert_eq!(resolve.packages.len(), 6);
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
