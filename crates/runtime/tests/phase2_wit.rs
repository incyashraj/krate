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
    // Name the packages rather than counting them: a bare number tells whoever
    // hits this failure that something changed, but not what, and adding a
    // capability is a normal reason for the count to move.
    let packages: std::collections::BTreeSet<String> = resolve
        .packages
        .iter()
        .map(|(_, package)| package.name.to_string())
        .collect();
    for expected in [
        "krate:app@0.1.0",
        "krate:io@0.1.0",
        "krate:fs@0.1.0",
        "krate:net@0.1.0",
        "krate:time@0.1.0",
        "krate:locale@0.1.0",
        "krate:resources@0.1.0",
        "krate:store@0.1.0",
    ] {
        assert!(
            packages.contains(expected),
            "the cli world lost {expected}; packages are {packages:?}"
        );
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
