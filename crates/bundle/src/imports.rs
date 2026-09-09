//! Reading a component's imports and checking they are all Krate APIs.
//!
//! A Krate component may import only `krate:*` interfaces — anything else
//! (`wasi:*`, a host-specific package) means the component would reach for a
//! capability the Krate runtime does not provide, and it would fail to
//! instantiate. `krate create` uses this to reject a generated app before
//! packaging it, so a broken component never becomes a `.krate`.

use std::collections::BTreeSet;

use wasmparser::{Parser, Payload};

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

/// The imports that are not Krate APIs. Empty means the component is clean.
pub fn non_krate_imports(bytes: &[u8]) -> Result<Vec<String>, String> {
    Ok(component_imports(bytes)?
        .into_iter()
        .filter(|import| !is_krate_import(import))
        .collect())
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
