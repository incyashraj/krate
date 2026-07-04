use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const DEFAULT_EVIDENCE_PATH: &str = "target/phase2-walkthrough/walkthrough-template.md";
const MAX_PASS_MINUTES: f64 = 30.0;

const REQUIRED_SECTIONS: &[&str] = &[
    "# Phase 2 Timed Walkthrough Evidence",
    "## Run Metadata",
    "## Pass Rule",
    "## Step Results",
    "## Reviewer Notes",
    "## Evidence To Attach",
];

const REQUIRED_METADATA_FIELDS: &[&str] = &[
    "Git commit under review",
    "Template generated at (UTC)",
    "Host used to generate template",
    "Reviewer name or handle",
    "Reviewer background",
    "Review host OS and arch",
    "Started at",
    "Finished at",
    "Total minutes",
    "Result",
];

const REQUIRED_STEPS: &[&str] = &[
    "Tool check",
    "Build CLI",
    "Build cat component",
    "Generate manifest",
    "Explain manifest",
    "Granted run",
    "Denied run",
];

fn main() -> Result<()> {
    let path = evidence_path_from_args()?;
    let report = check_walkthrough_evidence(&path)?;

    println!("Krate Phase 2 walkthrough evidence check passed");
    println!("- file: {}", path.display());
    println!("- result: {}", report.result);
    println!("- total minutes: {:.1}", report.total_minutes);
    println!("- steps checked: {}", report.steps_checked);

    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct WalkthroughReport {
    result: String,
    total_minutes: f64,
    steps_checked: usize,
}

fn evidence_path_from_args() -> Result<PathBuf> {
    let mut args = env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(workspace_root().join(DEFAULT_EVIDENCE_PATH));
    };

    if first == "--help" || first == "-h" {
        println!(
            "Usage: cargo run -p krate-tools --bin check-phase2-walkthrough-evidence -- [path]"
        );
        println!();
        println!("Default path: {DEFAULT_EVIDENCE_PATH}");
        std::process::exit(0);
    }

    if args.next().is_some() {
        bail!("expected at most one evidence path");
    }

    Ok(resolve_path(&first))
}

fn check_walkthrough_evidence(path: &Path) -> Result<WalkthroughReport> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    check_source(&source)
}

fn check_source(source: &str) -> Result<WalkthroughReport> {
    for section in REQUIRED_SECTIONS {
        ensure(
            source.contains(section),
            format!("walkthrough evidence is missing section `{section}`"),
        )?;
    }

    let metadata = metadata_fields(source)?;
    for field in REQUIRED_METADATA_FIELDS {
        let Some(value) = metadata.get(*field) else {
            bail!("walkthrough evidence is missing metadata field `{field}`");
        };
        ensure(
            is_filled(value),
            format!("walkthrough metadata field `{field}` is blank"),
        )?;
    }

    let result = normalized_value(
        metadata
            .get("Result")
            .expect("result field checked above")
            .as_str(),
    );
    ensure(
        result == "pass" || result == "fail",
        "walkthrough `Result` must be `pass` or `fail`".to_string(),
    )?;

    let total_minutes = parse_minutes(
        metadata
            .get("Total minutes")
            .expect("total minutes field checked above"),
    )?;
    ensure(
        total_minutes > 0.0,
        "walkthrough `Total minutes` must be greater than 0".to_string(),
    )?;
    if result == "pass" {
        ensure(
            total_minutes <= MAX_PASS_MINUTES,
            format!(
                "walkthrough is marked pass but took {total_minutes:.1} minutes; limit is {MAX_PASS_MINUTES:.1}"
            ),
        )?;
    }

    let step_results = step_results(source)?;
    for step in REQUIRED_STEPS {
        let Some(result) = step_results.get(*step) else {
            bail!("walkthrough step `{step}` is missing from the step table");
        };
        ensure(
            is_filled(result),
            format!("walkthrough step `{step}` has no reviewer result"),
        )?;
    }

    Ok(WalkthroughReport {
        result,
        total_minutes,
        steps_checked: step_results.len(),
    })
}

fn metadata_fields(source: &str) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    let mut in_metadata = false;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed == "## Run Metadata" {
            in_metadata = true;
            continue;
        }
        if in_metadata && trimmed.starts_with("## ") {
            break;
        }
        if !in_metadata {
            continue;
        }

        let Some(stripped) = trimmed.strip_prefix("- ") else {
            continue;
        };
        let Some((key, value)) = stripped.split_once(':') else {
            continue;
        };
        fields.insert(key.trim().to_string(), value.trim().to_string());
    }

    if fields.is_empty() {
        bail!("walkthrough evidence has no run metadata fields");
    }

    Ok(fields)
}

fn step_results(source: &str) -> Result<BTreeMap<String, String>> {
    let mut steps = BTreeMap::new();

    for line in source.lines() {
        let trimmed = line.trim();
        let Some(table_body) = trimmed.strip_prefix('|') else {
            continue;
        };
        let columns = table_body
            .trim_end_matches('|')
            .split('|')
            .map(|column| column.trim().to_string())
            .collect::<Vec<_>>();

        if columns.len() != 4 {
            continue;
        }
        if columns[0] == "Step" || columns[0].starts_with("---") {
            continue;
        }
        if REQUIRED_STEPS.contains(&columns[0].as_str()) {
            steps.insert(columns[0].clone(), columns[2].clone());
        }
    }

    if steps.is_empty() {
        bail!("walkthrough evidence has no step results");
    }

    Ok(steps)
}

fn parse_minutes(value: &str) -> Result<f64> {
    let normalized = normalized_value(value);
    normalized
        .parse::<f64>()
        .with_context(|| format!("walkthrough `Total minutes` must be a number, got `{value}`"))
}

fn normalized_value(value: &str) -> String {
    value.trim().trim_matches('`').trim().to_ascii_lowercase()
}

fn is_filled(value: &str) -> bool {
    let normalized = normalized_value(value);
    !normalized.is_empty() && normalized != "pass / fail"
}

fn ensure(condition: bool, message: String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        bail!(message)
    }
}

fn resolve_path(path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root().join(path)
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_filled_passing_walkthrough() {
        let source = filled_source("pass", "24.5");
        let report = check_source(&source).expect("filled walkthrough");

        assert_eq!(report.result, "pass");
        assert_eq!(report.total_minutes, 24.5);
        assert_eq!(report.steps_checked, REQUIRED_STEPS.len());
    }

    #[test]
    fn rejects_unfilled_default_result() {
        let source = filled_source("pass / fail", "24");
        let err = check_source(&source).expect_err("unfilled result should fail");

        assert!(err.to_string().contains("Result"));
    }

    #[test]
    fn rejects_passing_walkthrough_over_limit() {
        let source = filled_source("pass", "31");
        let err = check_source(&source).expect_err("over-limit pass should fail");

        assert!(err.to_string().contains("limit"));
    }

    #[test]
    fn rejects_blank_step_result() {
        let source = filled_source("pass", "21").replace(
            "| Denied run | app exits before native file access with a missing-capability message | pass | ok |",
            "| Denied run | app exits before native file access with a missing-capability message |  | ok |",
        );
        let err = check_source(&source).expect_err("blank step result should fail");

        assert!(err.to_string().contains("Denied run"));
    }

    fn filled_source(result: &str, total_minutes: &str) -> String {
        format!(
            r#"# Phase 2 Timed Walkthrough Evidence

## Run Metadata

- Git commit under review: `abc1234`
- Template generated at (UTC): `2026-05-17T00:00:00Z`
- Host used to generate template: `Darwin` / `arm64`
- Reviewer name or handle: reviewer
- Reviewer background: Rust developer, new to Krate
- Review host OS and arch: macOS arm64
- Started at: 10:00
- Finished at: 10:24
- Total minutes: {total_minutes}
- Result: {result}

## Pass Rule

P2E-12 passes when a Rust developer who does not already know Krate can
complete the Rust UAPI walkthrough in 30 minutes or less without private help.

## Step Results

| Step | Expected result | Reviewer result | Notes |
|---|---|---|---|
| Tool check | `krate doctor` shows required Rust tooling or clear install guidance | pass | ok |
| Build CLI | `cargo build -p krate-cli` passes | pass | ok |
| Build cat component | `scripts/build-krate-cat-component.sh` prints a wasm path | pass | ok |
| Generate manifest | `manifest.toml` is written and capabilities parse | pass | ok |
| Explain manifest | reviewer can identify default grants and explicit fs grant | pass | ok |
| Granted run | app prints `hello from Krate` | pass | ok |
| Denied run | app exits before native file access with a missing-capability message | pass | ok |

## Reviewer Notes

- What was confusing? Nothing

## Evidence To Attach

Transcript saved separately.
"#
        )
    }
}
