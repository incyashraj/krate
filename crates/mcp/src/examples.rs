//! Complete shipped apps, embedded as source.
//!
//! The highest-value thing we can hand a model is a whole app that works. Prose
//! about `#![no_std]` is a rule the model may or may not follow; a 640-line
//! dashboard that ships, passes CI, and imports only `krate:*` is a pattern it
//! can copy. That is why these are full files and not excerpts.
//!
//! They are `include_str!`ed rather than read from disk for the same reason the
//! authoring pack is: a released binary has no `apps/` tree beside it, and an
//! example that is missing in the field is worse than no example at all. It
//! also means they cannot drift -- if a shipped app stops compiling, CI catches
//! it, and this file is carrying that same known-good source.

/// One shipped app, as a model would need to recreate it.
pub struct Example {
    /// The app's directory name, e.g. `krate-pulse`.
    pub name: &'static str,
    /// `cli` or `gui`.
    pub kind: &'static str,
    /// One line on what this app is worth reading for.
    pub teaches: &'static str,
    pub lib_rs: &'static str,
    pub cargo_toml: &'static str,
    pub manifest_toml: &'static str,
}

/// A tiny CLI app. The smallest complete thing that works: it proves the whole
/// no_std shape in seventy lines, and it proves an ordinary ecosystem crate
/// (`rand`) runs on Krate through the SDK's getrandom backend.
const DICEROLL: Example = Example {
    name: "krate-diceroll",
    kind: "cli",
    teaches: "the smallest complete app: no_std, reading args, writing stdout, \
              and using an ordinary crate (rand) through the SDK's getrandom backend",
    lib_rs: include_str!("../../../apps/krate-diceroll/src/lib.rs"),
    cargo_toml: include_str!("../../../apps/krate-diceroll/Cargo.toml"),
    manifest_toml: include_str!("../../../apps/krate-diceroll/manifest.toml"),
};

/// A real GUI app that stores data. The shape most requests actually want:
/// a window, a widget tree, input, and state that survives a restart.
const CHECKLIST: Example = Example {
    name: "krate-checklist",
    kind: "gui",
    teaches: "a windowed app with a widget tree, keyboard and mouse input, and \
              state saved to the key-value store so it survives a restart",
    lib_rs: include_str!("../../../apps/krate-checklist/src/lib.rs"),
    cargo_toml: include_str!("../../../apps/krate-checklist/Cargo.toml"),
    manifest_toml: include_str!("../../../apps/krate-checklist/manifest.toml"),
};

/// The one to read when the request wants something that looks designed:
/// everything drawn by hand on a canvas, with no `format!` anywhere.
const PULSE: Example = Example {
    name: "krate-pulse",
    kind: "gui",
    teaches: "a polished dashboard drawn pixel by pixel on a canvas -- charts, \
              typography, hit-testing and click handling, and number formatting \
              done by hand because format! can pull in a panic path",
    lib_rs: include_str!("../../../apps/krate-pulse/src/lib.rs"),
    cargo_toml: include_str!("../../../apps/krate-pulse/Cargo.toml"),
    manifest_toml: include_str!("../../../apps/krate-pulse/manifest.toml"),
};

/// Every embedded example.
pub const ALL: &[Example] = &[DICEROLL, CHECKLIST, PULSE];

/// Pick the examples worth sending for a `kind` filter.
///
/// An unknown filter returns everything rather than nothing: a model that
/// guessed the wrong word should get useful source back, not an empty list it
/// has to debug.
pub fn select(kind: Option<&str>) -> Vec<&'static Example> {
    let wanted = kind.map(str::trim).filter(|k| !k.is_empty());
    match wanted {
        Some(kind) if kind.eq_ignore_ascii_case("cli") => {
            ALL.iter().filter(|e| e.kind == "cli").collect()
        }
        Some(kind) if kind.eq_ignore_ascii_case("gui") => {
            ALL.iter().filter(|e| e.kind == "gui").collect()
        }
        _ => ALL.iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_example_carries_all_three_files_with_real_content() {
        for example in ALL {
            assert!(
                example.lib_rs.len() > 200,
                "{} has no real source",
                example.name
            );
            assert!(
                example.cargo_toml.contains("[package]"),
                "{} has no package table",
                example.name
            );
            assert!(
                example.manifest_toml.contains("[app]"),
                "{} has no app table",
                example.name
            );
        }
    }

    #[test]
    fn every_example_demonstrates_the_no_std_rule_it_is_teaching() {
        // An example that linked std would teach the exact mistake check-app
        // rejects. This is the one property that must hold for all of them.
        for example in ALL {
            assert!(
                example.lib_rs.contains("#![no_std]"),
                "{} must be no_std to be worth copying",
                example.name
            );
            assert!(
                example.cargo_toml.contains("krate = { path ="),
                "{} must depend on the SDK",
                example.name
            );
        }
    }

    #[test]
    fn filtering_by_kind_selects_and_never_returns_nothing() {
        assert!(select(Some("cli")).iter().all(|e| e.kind == "cli"));
        assert!(select(Some("gui")).iter().all(|e| e.kind == "gui"));
        assert!(!select(Some("cli")).is_empty());
        assert!(!select(Some("gui")).is_empty());
        assert_eq!(select(None).len(), ALL.len());
        // A model that guesses "desktop" should get everything, not an empty
        // list it then has to work out how to fix.
        assert_eq!(select(Some("desktop")).len(), ALL.len());
        assert_eq!(select(Some("")).len(), ALL.len());
    }
}
