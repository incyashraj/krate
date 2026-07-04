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

    let failures = reports
        .iter()
        .filter(|report| !report.bad_imports.is_empty())
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        for report in &failures {
            eprintln!(
                "- {} imports non-Krate host APIs: {}",
                report.path.display(),
                report.bad_imports.join(", ")
            );
        }
        bail!(
            "Krate component import check failed for {} component(s)",
            failures.len()
        );
    }

    println!("Krate component import check passed");
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
    bad_imports: Vec<String>,
}

fn check_component_imports(path: &Path) -> Result<ImportReport> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let imports = component_imports(&bytes).with_context(|| format!("parse {}", path.display()))?;

    let bad_imports = imports
        .iter()
        .filter(|import| !is_krate_import(import))
        .cloned()
        .collect::<Vec<_>>();

    Ok(ImportReport {
        path: path.to_path_buf(),
        imports,
        bad_imports,
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

fn is_krate_import(import: &str) -> bool {
    import.starts_with("krate:")
}

#[cfg(test)]
mod tests {
    use super::is_krate_import;

    #[test]
    fn import_prefix_check_allows_only_krate_names() {
        assert!(is_krate_import("krate:io/stdio@0.1.0"));
        assert!(!is_krate_import("wasi:cli/environment@0.2.3"));
        assert!(!is_krate_import("example:host/api@0.1.0"));
    }
}
