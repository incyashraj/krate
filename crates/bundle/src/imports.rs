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

/// Whether an import names a Krate interface.
pub fn is_krate_import(import: &str) -> bool {
    import.starts_with("krate:")
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
}
