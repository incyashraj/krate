use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const EVIDENCE_PAGE: &str = "docs/book/src/phase2/exit-evidence.md";

const REQUIRED_GATES: &[ExitGate] = &[
    ExitGate::new("P2E-01", "UAPI modules frozen"),
    ExitGate::new("P2E-02", "Desktop host adapters"),
    ExitGate::new("P2E-03", "Rust bindings usable"),
    ExitGate::new("P2E-04", "Go bindings usable"),
    ExitGate::new("P2E-05", "TypeScript bindings usable"),
    ExitGate::new("P2E-06", "curl cross-host"),
    ExitGate::new("P2E-07", "cat cross-host"),
    ExitGate::new("P2E-08", "clock cross-host"),
    ExitGate::new("P2E-09", "UCap enforcement"),
    ExitGate::new("P2E-10", "Startup performance"),
    ExitGate::new("P2E-11", "Dispatch performance"),
    ExitGate::new("P2E-12", "Timed developer walkthrough"),
    ExitGate::new("P2E-13", "Generated UAPI reference"),
    ExitGate::new("P2E-14", "WIT style guide"),
    ExitGate::new("P2E-15", "ADR set"),
];

const ALLOWED_STATUSES: &[&str] = &["Done", "Strong draft", "Partial", "Pending", "Blocked"];
const REQUIRED_SECTIONS: &[&str] = &[
    "# Phase 2 Exit Evidence",
    "## How To Read This Page",
    "## Exit Gate Ledger",
    "## What Is Already Strong",
    "## What Still Blocks Phase 2 Exit",
    "## Local Evidence Commands",
    "## CI Evidence We Still Need",
];

fn main() -> Result<()> {
    let report = check_exit_evidence()?;

    println!("Krate Phase 2 exit evidence check passed");
    println!("- page: {EVIDENCE_PAGE}");
    println!("- gates: {}", report.gates);
    println!("- done: {}", report.done);
    println!("- strong draft: {}", report.strong_draft);
    println!("- partial: {}", report.partial);
    println!("- pending: {}", report.pending);
    println!("- blocked: {}", report.blocked);

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct ExitGate {
    id: &'static str,
    phrase: &'static str,
}

impl ExitGate {
    const fn new(id: &'static str, phrase: &'static str) -> Self {
        Self { id, phrase }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvidenceReport {
    gates: usize,
    done: usize,
    strong_draft: usize,
    partial: usize,
    pending: usize,
    blocked: usize,
}

fn check_exit_evidence() -> Result<EvidenceReport> {
    let root = workspace_root();
    let path = root.join(EVIDENCE_PAGE);
    let source = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

    for section in REQUIRED_SECTIONS {
        ensure(
            source.contains(section),
            format!("{EVIDENCE_PAGE} is missing section `{section}`"),
        )?;
    }

    let rows = gate_rows(&source)?;
    let expected_ids = REQUIRED_GATES
        .iter()
        .map(|gate| gate.id)
        .collect::<BTreeSet<_>>();
    let actual_ids = rows
        .iter()
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();

    ensure(
        actual_ids == expected_ids,
        format!("exit gate ids changed\nexpected: {expected_ids:?}\nactual:   {actual_ids:?}"),
    )?;

    for gate in REQUIRED_GATES {
        let Some(row) = rows.iter().find(|row| row.id == gate.id) else {
            bail!("missing gate {}", gate.id);
        };
        ensure(
            row.criterion.contains(gate.phrase),
            format!(
                "{} must describe `{}` in the criterion column",
                gate.id, gate.phrase
            ),
        )?;
        ensure(
            ALLOWED_STATUSES.contains(&row.status.as_str()),
            format!(
                "{} has unsupported status `{}`; use one of {:?}",
                gate.id, row.status, ALLOWED_STATUSES
            ),
        )?;
        ensure(
            row.evidence.contains('`') || row.evidence.contains('['),
            format!(
                "{} needs a concrete command, file, or linked evidence source",
                gate.id
            ),
        )?;
        ensure(
            !row.next_step.trim().is_empty(),
            format!("{} needs a next-step note", gate.id),
        )?;
    }

    Ok(EvidenceReport {
        gates: rows.len(),
        done: count_status(&rows, "Done"),
        strong_draft: count_status(&rows, "Strong draft"),
        partial: count_status(&rows, "Partial"),
        pending: count_status(&rows, "Pending"),
        blocked: count_status(&rows, "Blocked"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GateRow {
    id: String,
    criterion: String,
    status: String,
    evidence: String,
    next_step: String,
}

fn gate_rows(source: &str) -> Result<Vec<GateRow>> {
    let mut rows = Vec::new();

    for line in source.lines() {
        let Some(trimmed) = line.trim().strip_prefix('|') else {
            continue;
        };

        let columns = trimmed
            .trim_end_matches('|')
            .split('|')
            .map(|column| column.trim().to_string())
            .collect::<Vec<_>>();

        if columns.len() != 5 || !columns[0].starts_with("P2E-") {
            continue;
        }

        rows.push(GateRow {
            id: columns[0].clone(),
            criterion: columns[1].clone(),
            status: strip_markdown_bold(&columns[2]),
            evidence: columns[3].clone(),
            next_step: columns[4].clone(),
        });
    }

    if rows.is_empty() {
        bail!("no P2E exit gate rows found in {EVIDENCE_PAGE}");
    }

    Ok(rows)
}

fn strip_markdown_bold(value: &str) -> String {
    value.trim_matches('*').trim().to_string()
}

fn count_status(rows: &[GateRow], status: &str) -> usize {
    rows.iter().filter(|row| row.status == status).count()
}

fn ensure(condition: bool, message: String) -> Result<()> {
    if condition {
        Ok(())
    } else {
        bail!(message)
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
    fn current_exit_evidence_page_is_well_formed() {
        check_exit_evidence().expect("phase 2 exit evidence");
    }

    #[test]
    fn parses_gate_rows() {
        let source = r#"
| Gate | Criterion | Status | Evidence | Next step |
|---|---|---|---|---|
| P2E-01 | UAPI modules frozen | **Strong draft** | `scripts/check-uapi.sh` | Review |
"#;

        let rows = gate_rows(source).expect("rows");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "P2E-01");
        assert_eq!(rows[0].status, "Strong draft");
    }
}
