//! Reading a component's imports and checking they are all Krate APIs.
//!
//! A Krate component may import only `krate:*` interfaces — anything else
//! (`wasi:*`, a host-specific package) means the component would reach for a
//! capability the Krate runtime does not provide, and it would fail to
//! instantiate. `krate create` uses this to reject a generated app before
//! packaging it, so a broken component never becomes a `.krate`.

use std::collections::BTreeSet;

use wasmparser::{Parser, Payload};

/// Whether these bytes are a component rather than a core module (K-272).
///
/// The two are told apart by their header, and only there: wasmparser parses
/// a core module perfectly well, so asking for its component imports returns
/// an empty set -- exactly what a component that imports nothing returns.
/// That is why `krate pack` accepted a module and the failure landed on
/// whoever was sent the file.
///
///   component  00 61 73 6d 0d 00 01 00
///   module     00 61 73 6d 01 00 00 00
///                          ^^^^^^^^^^^
///
/// The first four bytes are the same magic. The next four are a version and
/// a layer, and the layer is what says which of the two this is.
pub fn is_component(bytes: &[u8]) -> bool {
    // 8 bytes of header, then the layer field: 1 for a component, 0 for a
    // module. Anything shorter is neither.
    matches!(
        bytes.get(..8),
        Some([0x00, 0x61, 0x73, 0x6d, _, _, 0x01, 0x00])
    )
}

/// Every interface a component imports, in sorted order.
pub fn component_imports(bytes: &[u8]) -> Result<BTreeSet<String>, String> {
    let mut imports = BTreeSet::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|err| format!("parse component: {err}"))?;
        if let Payload::ComponentImportSection(section) = payload {
            for import in section {
                let import = import.map_err(|err| format!("read import: {err}"))?;
                imports.insert(import.name.0.to_string());
            }
        }
    }
    Ok(imports)
}

include!(concat!(env!("OUT_DIR"), "/known_interfaces.rs"));

/// Whether an import names a Krate interface.
///
/// Namespace AND existence (IC-210). `starts_with("krate:")` alone accepted
/// anything in our namespace, so an invented `krate:anything/not-real@999.0.0`
/// passed as a Krate API and the component failed later at instantiate with
/// wasmparser's words instead of ours. A name we never defined is not a
/// Krate API -- it is a component asking for something that does not exist,
/// and saying so at pack time is the whole point of this check.
///
/// The known list is generated from `wit/` at build time. When it is empty
/// -- a vendored build with no WIT tree -- this falls back to the namespace
/// test rather than rejecting everything: not knowing what is defined is a
/// reason to check less, never a reason to call every real interface
/// invented.
pub fn is_krate_import(import: &str) -> bool {
    if !import.starts_with("krate:") {
        return false;
    }
    if KNOWN_KRATE_INTERFACES.is_empty() {
        return true;
    }
    KNOWN_KRATE_INTERFACES.contains(&import)
}

/// Imports that sit in the `krate:` namespace but name nothing Krate defines.
///
/// Separated from [`non_krate_imports`] because the two are different
/// problems with different messages: a `wasi:*` import means the app reached
/// for the operating system, while `krate:evil/backdoor@0.1.0` means it
/// asked for a Krate API that has never existed.
pub fn unknown_krate_imports(bytes: &[u8]) -> Result<Vec<String>, String> {
    if KNOWN_KRATE_INTERFACES.is_empty() {
        return Ok(Vec::new());
    }
    Ok(component_imports(bytes)?
        .into_iter()
        .filter(|import| {
            import.starts_with("krate:") && !KNOWN_KRATE_INTERFACES.contains(&import.as_str())
        })
        .collect())
}

/// Every interface or function a component exports.
///
/// A Krate app must export `run` -- the runtime calls it and nothing else.
/// A component with clean imports and no `run` packs today and fails at the
/// recipient's machine, which is the same shape of defect as accepting a
/// name we never defined (IC-210).
pub fn component_exports(bytes: &[u8]) -> Result<BTreeSet<String>, String> {
    let mut exports = BTreeSet::new();
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|err| format!("parse component: {err}"))?;
        if let Payload::ComponentExportSection(section) = payload {
            for export in section {
                let export = export.map_err(|err| format!("read export: {err}"))?;
                exports.insert(export.name.0.to_string());
            }
        }
    }
    Ok(exports)
}

/// The imports that are not Krate APIs. Empty means the component is clean.
pub fn non_krate_imports(bytes: &[u8]) -> Result<Vec<String>, String> {
    Ok(component_imports(bytes)?
        .into_iter()
        .filter(|import| !is_krate_import(import))
        .collect())
}

/// The one export every Krate world declares, and the only one it does.
pub const RUN_EXPORT: &str = "run";

/// Which version of the rules below judged a component. Bump it when a rule
/// changes, so a report that says "validator 1" means the same thing next
/// year.
pub const VALIDATOR_VERSION: u32 = 1;

/// What the validator found in a component it accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentReport {
    pub imports: BTreeSet<String>,
    pub exports: BTreeSet<String>,
    /// The world the component was checked against, when a manifest named
    /// one this runtime hosts. `None` means only the namespace rules ran.
    pub world: Option<String>,
    pub validator: u32,
    /// SHA-256 of the WIT tree the rules came from; empty in a build with
    /// no WIT tree, where the world and existence checks cannot run.
    pub wit_digest: &'static str,
}

/// Why the validator refused a component. Each carries what a person needs
/// to fix it, because every one of these used to reach the recipient as an
/// instantiate error in wasmparser's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentDefect {
    /// Not a component at all: a core module, or bytes that are not wasm.
    NotAComponent { detail: String },
    /// A component header followed by sections wasmparser cannot read.
    Malformed { detail: String },
    /// Imports outside `krate:*` -- the app reached for the operating system.
    NonKrateImports(Vec<String>),
    /// Imports in `krate:*` that name nothing Krate has ever defined.
    UnknownKrateImports(Vec<String>),
    /// Imports Krate defines but the declared world does not provide.
    OutsideWorld { world: String, imports: Vec<String> },
    /// No `run` export: the runtime would have nothing to call.
    MissingRunExport,
    /// Exports beyond `run`: the world declares one, and a component that
    /// offers more was built against something else.
    ExtraExports(Vec<String>),
}

impl std::fmt::Display for ComponentDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAComponent { detail } => {
                write!(f, "it is not a WebAssembly component: {detail}")
            }
            Self::Malformed { detail } => write!(
                f,
                "it starts like a component but cannot be read past the header: {detail}"
            ),
            Self::NonKrateImports(list) => write!(
                f,
                "it imports host APIs Krate does not provide, so it cannot run under Krate: {}",
                list.join(", ")
            ),
            Self::UnknownKrateImports(list) => write!(
                f,
                "it imports Krate interfaces that do not exist: {}",
                list.join(", ")
            ),
            Self::OutsideWorld { world, imports } => write!(
                f,
                "its manifest declares the world {world}, which does not provide: {}. \
                 Declare the world the app was built for.",
                imports.join(", ")
            ),
            Self::MissingRunExport => write!(
                f,
                "it exports no `run` function, so Krate would have nothing to call. \
                 Every Krate app exports exactly `run`."
            ),
            Self::ExtraExports(list) => write!(
                f,
                "it exports more than `run`, which no Krate world declares: {}",
                list.join(", ")
            ),
        }
    }
}

/// The one validator every door runs (IC-210): pack, open -- and through
/// open, publish, the hub and run. One function, so the four cannot
/// disagree about what a Krate component is.
///
/// `world` is the manifest's declared world. When it is one this runtime
/// hosts, the component's imports are checked against what that world
/// provides; an older hosted name is checked against the world that hosts
/// it. A name the runtime does not host is the manifest's problem and is
/// refused there, so this function simply skips the world check for it.
pub fn validate_component(
    bytes: &[u8],
    world: Option<&str>,
) -> Result<ComponentReport, ComponentDefect> {
    if !is_component(bytes) {
        let detail = if bytes.get(..4) == Some(&[0x00, 0x61, 0x73, 0x6d]) {
            "it is a core WebAssembly module, not a component. Build it with \
             `cargo component build` rather than `cargo build`."
                .to_string()
        } else {
            "it does not start with the WebAssembly magic number".to_string()
        };
        return Err(ComponentDefect::NotAComponent { detail });
    }
    let imports = component_imports(bytes).map_err(|detail| ComponentDefect::Malformed {
        detail: first_line(&detail),
    })?;
    let exports = component_exports(bytes).map_err(|detail| ComponentDefect::Malformed {
        detail: first_line(&detail),
    })?;

    let foreign: Vec<String> = imports
        .iter()
        .filter(|import| !import.starts_with("krate:"))
        .cloned()
        .collect();
    if !foreign.is_empty() {
        return Err(ComponentDefect::NonKrateImports(foreign));
    }
    if !KNOWN_KRATE_INTERFACES.is_empty() {
        let unknown: Vec<String> = imports
            .iter()
            .filter(|import| !KNOWN_KRATE_INTERFACES.contains(&import.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(ComponentDefect::UnknownKrateImports(unknown));
        }
    }

    // The world check. An interface is provided by a world when the world
    // lists it, or when it belongs to a package the world lists something
    // from: every real app imports krate:io/types, which no world names
    // because it is a dependency of krate:io/stdio, which every world does.
    let hosted = world
        .and_then(|name| krate_manifest::AppWorld::from_world_name(name).ok())
        .map(|hosted| hosted.world_name());
    let checked_world = hosted.and_then(|name| {
        KNOWN_WORLDS
            .iter()
            .find(|(world, _)| *world == name)
            .map(|(world, provided)| (*world, *provided))
    });
    if let Some((world_name, provided)) = checked_world {
        let packages: BTreeSet<&str> = provided.iter().filter_map(|i| package_of(i)).collect();
        let outside: Vec<String> = imports
            .iter()
            .filter(|import| {
                !provided.contains(&import.as_str())
                    && !package_of(import).is_some_and(|p| packages.contains(p))
            })
            .cloned()
            .collect();
        if !outside.is_empty() {
            return Err(ComponentDefect::OutsideWorld {
                world: world_name.to_string(),
                imports: outside,
            });
        }
    }

    if !exports.contains(RUN_EXPORT) {
        return Err(ComponentDefect::MissingRunExport);
    }
    let extra: Vec<String> = exports
        .iter()
        .filter(|export| export.as_str() != RUN_EXPORT)
        .cloned()
        .collect();
    if !extra.is_empty() {
        return Err(ComponentDefect::ExtraExports(extra));
    }

    Ok(ComponentReport {
        imports,
        exports,
        world: checked_world.map(|(name, _)| name.to_string()),
        validator: VALIDATOR_VERSION,
        wit_digest: WIT_DIGEST,
    })
}

/// `krate:io/types@0.1.0` -> `krate:io`
fn package_of(interface: &str) -> Option<&str> {
    interface.split('/').next().filter(|p| p.contains(':'))
}

/// wasmparser's own words run to several lines of hex for the commonest
/// case; the first line carries the fact.
fn first_line(detail: &str) -> String {
    if detail.contains("magic header not detected") {
        return "it does not start with the WebAssembly magic number".to_string();
    }
    detail
        .trim()
        .lines()
        .next()
        .unwrap_or("unreadable")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn krate_imports_are_recognized() {
        assert!(is_krate_import("krate:io/stdio@0.1.0"));
        assert!(!is_krate_import("wasi:cli/environment@0.2.3"));
        assert!(!is_krate_import("example:host/api@0.1.0"));
    }

    /// A name in our namespace that we never defined is not a Krate API
    /// (IC-210).
    ///
    /// `starts_with("krate:")` accepted every one of these, so a component
    /// importing an interface that has never existed packed cleanly and
    /// failed later at instantiate.
    #[test]
    fn a_core_module_is_not_a_component() {
        // K-272. Only the header separates them: wasmparser parses a core
        // module happily, so asking for its component imports gives an empty
        // set -- the same answer a component that imports nothing gives.
        // That is how `krate pack` accepted a module.
        assert!(
            is_component(&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]),
            "a component header must be recognised"
        );
        assert!(
            !is_component(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]),
            "a core module must NOT pass as a component"
        );

        // The import check genuinely cannot tell them apart, which is the
        // reason this function exists. If this ever stops being true, the
        // header check could be simplified away.
        let module = &[0x00u8, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let component = &[0x00u8, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
        assert_eq!(
            component_imports(module).map(|i| i.len()),
            component_imports(component).map(|i| i.len()),
            "the import check sees these as the same, so the header is the \
             only thing that can separate them"
        );

        // Too short to have a header is not a component either.
        assert!(!is_component(b""), "empty bytes are not a component");
        assert!(
            !is_component(&[0x00, 0x61, 0x73, 0x6d]),
            "magic alone is not a component"
        );
    }

    #[test]
    fn an_invented_krate_name_is_not_a_krate_api() {
        // The empty-list fallback cannot be exercised here -- this build
        // HAS the WIT, so the branch is unreachable in tests. It is written
        // to fail open (accept the namespace) rather than closed, because
        // not knowing what is defined is a reason to check less, never a
        // reason to call every real interface invented.
        assert!(
            !KNOWN_KRATE_INTERFACES.is_empty(),
            "this build should have generated the list from wit/",
        );
        for invented in [
            "krate:anything/not-real@999.0.0",
            "krate:evil/backdoor@0.1.0",
            "krate:io/stdio@9.9.9",
            "krate:io/not-an-interface@0.1.0",
            "krate:",
        ] {
            assert!(
                !is_krate_import(invented),
                "{invented} is not an interface Krate defines",
            );
        }
    }

    /// Every interface a real app imports must still pass, or this check
    /// breaks the product to close a hole.
    /// A component written in the text format with exactly the imports and
    /// exports named. Every refusal below needs a component cargo-component
    /// would never produce -- the SDK would not compile an import of an
    /// interface that does not exist -- so they are written by hand.
    fn component_with(imports: &[&str], exports: &[&str]) -> Vec<u8> {
        let mut wat = String::from("(component\n");
        for import in imports {
            wat.push_str(&format!("  (import \"{import}\" (instance))\n"));
        }
        wat.push_str("  (core module $m\n");
        for export in exports {
            wat.push_str(&format!(
                "    (func (export \"{export}\") (result i32) i32.const 0)\n"
            ));
        }
        wat.push_str("  )\n  (core instance $i (instantiate $m))\n");
        for export in exports {
            wat.push_str(&format!(
                "  (func ${export} (result s32) (canon lift (core func $i \"{export}\")))\n\
                 \x20 (export \"{export}\" (func ${export}))\n"
            ));
        }
        wat.push_str(")\n");
        wat::parse_str(&wat).expect("the test's own component must parse")
    }

    const CLI: Option<&str> = Some("krate:app/cli@0.1.0");
    const GUI: Option<&str> = Some("krate:app/gui@0.2.0");

    /// The validator's matrix (IC-210). One function judges every door, so
    /// one table says what it accepts and what it refuses, and why.
    #[test]
    fn the_validator_accepts_exactly_what_the_worlds_declare() {
        // Valid CLI and GUI components.
        let cli_app = component_with(&["krate:io/types@0.1.0", "krate:io/stdio@0.1.0"], &["run"]);
        let report = validate_component(&cli_app, CLI).expect("a CLI app fits the CLI world");
        assert_eq!(report.world.as_deref(), Some("krate:app/cli@0.1.0"));
        assert_eq!(report.validator, VALIDATOR_VERSION);
        assert!(
            !report.wit_digest.is_empty(),
            "this build has the WIT, so it must say which"
        );
        assert_eq!(report.exports.iter().collect::<Vec<_>>(), vec!["run"]);

        let gui_app = component_with(
            &[
                "krate:io/stdio@0.1.0",
                "krate:ui/types@0.1.0",
                "krate:ui/window@0.1.0",
            ],
            &["run"],
        );
        validate_component(&gui_app, GUI).expect("a GUI app fits the GUI world");
        // A CLI app also fits the GUI world: the GUI world is a superset.
        validate_component(&cli_app, GUI).expect("the GUI world provides everything CLI does");

        // The older hosted GUI world is checked against the world that hosts it.
        validate_component(&gui_app, Some("krate:app/gui@0.1.0"))
            .expect("an older supported world is hosted by the current one");

        // Manifest-world mismatch: a GUI app declaring the CLI world.
        match validate_component(&gui_app, CLI) {
            Err(ComponentDefect::OutsideWorld { world, imports }) => {
                assert_eq!(world, "krate:app/cli@0.1.0");
                assert_eq!(
                    imports,
                    vec!["krate:ui/types@0.1.0", "krate:ui/window@0.1.0"]
                );
            }
            other => panic!("a GUI app under the CLI world must be refused: {other:?}"),
        }

        // A world the runtime does not host is the manifest's refusal, not
        // this one's: the namespace and export rules still run.
        validate_component(&cli_app, Some("krate:phase1/host@0.0.1"))
            .expect("an unhosted world name skips only the world check");
        validate_component(&cli_app, None).expect("no world: namespace rules only");
    }

    #[test]
    fn the_validator_refuses_each_kind_of_broken_component_by_name() {
        // Core module.
        let module = wat::parse_str("(module)").expect("module");
        assert!(matches!(
            validate_component(&module, CLI),
            Err(ComponentDefect::NotAComponent { ref detail }) if detail.contains("core WebAssembly module")
        ));
        // Arbitrary bytes.
        assert!(matches!(
            validate_component(b"hello, this is not wasm at all", CLI),
            Err(ComponentDefect::NotAComponent { ref detail }) if detail.contains("magic number")
        ));
        // Malformed section: a real header, then garbage where sections go.
        let mut malformed = vec![0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
        malformed.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x01, 0x02, 0x03]);
        assert!(
            matches!(
                validate_component(&malformed, CLI),
                Err(ComponentDefect::Malformed { .. })
            ),
            "{:?}",
            validate_component(&malformed, CLI)
        );

        // Unknown `krate:` package, interface and version.
        for invented in [
            "krate:anything/not-real@999.0.0",
            "krate:io/not-an-interface@0.1.0",
            "krate:io/stdio@9.9.9",
        ] {
            let app = component_with(&["krate:io/stdio@0.1.0", invented], &["run"]);
            match validate_component(&app, CLI) {
                Err(ComponentDefect::UnknownKrateImports(list)) => assert_eq!(list, vec![invented]),
                other => panic!("{invented} must be refused as unknown: {other:?}"),
            }
        }

        // Non-Krate import: the app reached for the operating system.
        let leaky = component_with(
            &["krate:io/stdio@0.1.0", "wasi:cli/environment@0.2.3"],
            &["run"],
        );
        match validate_component(&leaky, CLI) {
            Err(ComponentDefect::NonKrateImports(list)) => {
                assert_eq!(list, vec!["wasi:cli/environment@0.2.3"]);
            }
            other => panic!("a wasi import must be refused: {other:?}"),
        }

        // Missing export.
        let silent = component_with(&["krate:io/stdio@0.1.0"], &[]);
        assert_eq!(
            validate_component(&silent, CLI),
            Err(ComponentDefect::MissingRunExport)
        );
        let wrong_name = component_with(&[], &["main"]);
        assert_eq!(
            validate_component(&wrong_name, CLI),
            Err(ComponentDefect::MissingRunExport)
        );

        // Extra export.
        let chatty = component_with(&[], &["run", "debug-hook"]);
        match validate_component(&chatty, CLI) {
            Err(ComponentDefect::ExtraExports(list)) => assert_eq!(list, vec!["debug-hook"]),
            other => panic!("an extra export must be refused: {other:?}"),
        }

        // Every refusal reads as a sentence a person can act on.
        for defect in [
            ComponentDefect::MissingRunExport,
            ComponentDefect::ExtraExports(vec!["x".into()]),
            ComponentDefect::NonKrateImports(vec!["wasi:x/y@0.1.0".into()]),
            ComponentDefect::UnknownKrateImports(vec!["krate:x/y@0.1.0".into()]),
            ComponentDefect::OutsideWorld {
                world: "w".into(),
                imports: vec!["i".into()],
            },
        ] {
            let text = defect.to_string();
            assert!(text.len() > 20 && !text.contains("Err("), "{text}");
        }
    }

    /// The interface list, the world table and the digest all come from the
    /// same WIT tree, and they agree with each other.
    #[test]
    fn the_generated_world_table_matches_the_wit() {
        let cli = KNOWN_WORLDS
            .iter()
            .find(|(w, _)| *w == "krate:app/cli@0.1.0")
            .expect("the CLI world is generated");
        let gui = KNOWN_WORLDS
            .iter()
            .find(|(w, _)| *w == "krate:app/gui@0.2.0")
            .expect("the GUI world is generated");
        assert!(cli.1.contains(&"krate:io/stdio@0.1.0"));
        assert!(gui.1.contains(&"krate:ui/window@0.1.0"));
        assert!(
            !cli.1.contains(&"krate:ui/window@0.1.0"),
            "the CLI world has no UI"
        );
        for import in cli.1.iter().chain(gui.1.iter()) {
            assert!(
                KNOWN_KRATE_INTERFACES.contains(import),
                "{import} is in a world but not in the interface list"
            );
        }
        assert_eq!(WIT_DIGEST.len(), 64, "a SHA-256 in hex");
    }

    /// Every real bundle on this machine still opens under the validator.
    /// Run by hand: KRATE_CORPUS=<file with one .krate path per line>
    /// cargo test -p krate-bundle --lib every_bundle_in_the_corpus -- --ignored --nocapture
    #[test]
    #[ignore]
    fn every_bundle_in_the_corpus_still_opens() {
        let list = std::env::var("KRATE_CORPUS").expect("KRATE_CORPUS names the list");
        let mut failures = Vec::new();
        let mut opened = 0;
        for line in std::fs::read_to_string(&list).expect("read list").lines() {
            let path = std::path::Path::new(line.trim());
            if line.trim().is_empty() {
                continue;
            }
            match crate::open(path) {
                Ok(_) => opened += 1,
                Err(err) => failures.push(format!("{}: {err}", path.display())),
            }
        }
        eprintln!("{opened} bundles opened");
        assert!(
            failures.is_empty(),
            "bundles refused:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn every_interface_the_wit_defines_is_accepted() {
        for known in KNOWN_KRATE_INTERFACES {
            assert!(is_krate_import(known), "{known} comes from our own WIT");
        }
        // The transitive `types` interfaces in particular: they appear in no
        // world, and every shipped app imports at least one.
        if !KNOWN_KRATE_INTERFACES.is_empty() {
            assert!(is_krate_import("krate:io/types@0.1.0"));
            assert!(is_krate_import("krate:io/streams@0.1.0"));
        }
    }
}
