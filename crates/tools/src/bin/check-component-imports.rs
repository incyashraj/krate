use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use wasmparser::{Parser, Payload};

fn main() -> Result<()> {
    let paths = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        bail!("usage: check-component-imports <component.wasm>...");
    }

    let mut reports = Vec::new();
    for path in &paths {
        reports.push(check_component_imports(path)?);
    }

    println!("Layer36 component import check passed");
    for report in reports {
        println!(
            "- {}: {} imports",
            report.path.display(),
            report.imports.len()
        );
    }

    Ok(())
}

struct ImportReport {
    path: PathBuf,
    imports: BTreeSet<String>,
}

fn check_component_imports(path: &Path) -> Result<ImportReport> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let imports = component_imports(&bytes).with_context(|| format!("parse {}", path.display()))?;

    let bad_imports = imports
        .iter()
        .filter(|import| !is_layer36_import(import))
        .cloned()
        .collect::<Vec<_>>();

    if !bad_imports.is_empty() {
        bail!(
            "{} imports non-Layer36 host APIs: {}",
            path.display(),
            bad_imports.join(", ")
        );
    }

    Ok(ImportReport {
        path: path.to_path_buf(),
        imports,
    })
}

fn component_imports(bytes: &[u8]) -> Result<BTreeSet<String>> {
    let mut imports = BTreeSet::new();

    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::ComponentImportSection(section) = payload? {
            for import in section {
                imports.insert(import?.name.0.to_string());
            }
        }
    }

    Ok(imports)
}

fn is_layer36_import(import: &str) -> bool {
    import.starts_with("layer36:")
}

#[cfg(test)]
mod tests {
    use super::is_layer36_import;

    #[test]
    fn import_prefix_check_allows_only_layer36_names() {
        assert!(is_layer36_import("layer36:io/stdio@0.1.0"));
        assert!(!is_layer36_import("wasi:cli/environment@0.2.3"));
        assert!(!is_layer36_import("example:host/api@0.1.0"));
    }
}
