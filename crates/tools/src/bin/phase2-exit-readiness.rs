use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const EVIDENCE_PAGE: &str = "docs/book/src/phase2/exit-evidence.md";

fn main() -> Result<()> {
    let config = Config::parse(env::args().skip(1))?;
    let source = fs::read_to_string(&config.evidence_page)
        .with_context(|| format!("read {}", config.evidence_page.display()))?;
    let rows = gate_rows(&source)?;
    let report = ReadinessReport::from_rows(rows);

    print_report(&config.evidence_page, &report, config.show_all);
    Ok(())
}

#[derive(Debug, Clone)]
struct Config {
    evidence_page: PathBuf,
    show_all: bool,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut evidence_page = workspace_root().join(EVIDENCE_PAGE);
        let mut show_all = false;

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--evidence-page" => {
                    let Some(path) = args.next() else {
                        bail!("--evidence-page requires a path");
                    };
                    evidence_page = PathBuf::from(path);
                }
                "--all" => {
                    show_all = true;
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: phase2-exit-readiness [--all] [--evidence-page docs/book/src/phase2/exit-evidence.md]"
                    );
                    std::process::exit(0);
                }
                _ => bail!("unknown argument `{arg}`"),
            }
        }

        Ok(Self {
            evidence_page,
            show_all,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GateRow {
    id: String,
    criterion: String,
    status: String,
    next_step: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadinessReport {
    gates: usize,
    done: usize,
    strong_draft: usize,
    partial: usize,
    pending: usize,
    blocked: usize,
    proof_path_exists: usize,
    needs_final_proof_or_decision: Vec<GateRow>,
    hard_blockers: Vec<GateRow>,
}

impl ReadinessReport {
    fn from_rows(rows: Vec<GateRow>) -> Self {
        let done = count_status(&rows, "Done");
        let strong_draft = count_status(&rows, "Strong draft");
        let partial = count_status(&rows, "Partial");
        let pending = count_status(&rows, "Pending");
        let blocked = count_status(&rows, "Blocked");
        let proof_path_exists = done + strong_draft + partial;
        let needs_final_proof_or_decision = rows
            .iter()
            .filter(|row| row.status != "Done")
            .cloned()
            .collect::<Vec<_>>();
        let hard_blockers = rows
            .iter()
            .filter(|row| row.status == "Pending" || row.status == "Blocked")
            .cloned()
            .collect::<Vec<_>>();

        Self {
            gates: rows.len(),
            done,
            strong_draft,
            partial,
            pending,
            blocked,
            proof_path_exists,
            needs_final_proof_or_decision,
            hard_blockers,
        }
    }
}

fn print_report(evidence_page: &Path, report: &ReadinessReport, show_all: bool) {
    println!("Krate Phase 2 exit readiness");
    println!("- evidence page: {}", evidence_page.display());
    println!("- gates tracked: {}", report.gates);
    println!("- fully done: {}", report.done);
    println!("- strong draft: {}", report.strong_draft);
    println!("- partial, proof in progress: {}", report.partial);
    println!("- pending: {}", report.pending);
    println!("- blocked: {}", report.blocked);
    println!(
        "- repeatable proof path exists: {}/{}",
        report.proof_path_exists, report.gates
    );
    println!(
        "- still needs final proof or decision: {}/{}",
        report.needs_final_proof_or_decision.len(),
        report.gates
    );

    if !report.hard_blockers.is_empty() {
        println!();
        println!("Hard blockers");
        for row in &report.hard_blockers {
            println!(
                "- {} ({}) is {}. Next: {}",
                row.id, row.criterion, row.status, row.next_step
            );
        }
    }

    let mut soft = report
        .needs_final_proof_or_decision
        .iter()
        .filter(|row| row.status != "Pending" && row.status != "Blocked")
        .collect::<Vec<_>>();

    let hidden = if show_all {
        0
    } else {
        let hidden = soft.len().saturating_sub(6);
        soft.truncate(6);
        hidden
    };

    if !soft.is_empty() {
        println!();
        println!("Main proof work still open");
        for row in soft {
            if show_all {
                println!(
                    "- {} ({}) is {}. Next: {}",
                    row.id, row.criterion, row.status, row.next_step
                );
            } else {
                println!("- {} ({}) is {}", row.id, row.criterion, row.status);
            }
        }
        if hidden > 0 {
            println!("- plus {hidden} more open proof items; run with --all to list every gate");
        }
    }
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
            next_step: columns[4].clone(),
        });
    }

    if rows.is_empty() {
        bail!("no P2E exit gate rows found");
    }

    Ok(rows)
}

fn count_status(rows: &[GateRow], status: &str) -> usize {
    rows.iter().filter(|row| row.status == status).count()
}

fn strip_markdown_bold(value: &str) -> String {
    value.trim_matches('*').trim().to_string()
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

    fn sample_source() -> &'static str {
        r#"
| Gate | Criterion | Status | Evidence | Next step |
|---|---|---|---|---|
| P2E-01 | UAPI modules frozen | **Strong draft** | `scripts/check-uapi.sh` | Freeze after review. |
| P2E-02 | Desktop host adapters | **Partial** | `scripts/check-adapter-boundary.sh` | Collect host reports. |
| P2E-12 | Timed developer walkthrough | **Pending** | `scripts/record-phase2-walkthrough-template.sh` | Ask an outside developer. |
| P2E-13 | Generated UAPI reference | **Done** | `scripts/generate-uapi-reference.sh` | Keep CI checks enabled. |
| P2E-04 | Go bindings usable | **Blocked** | `scripts/record-phase2-go-readiness-evidence.sh` | Decide Go promotion. |
"#
    }

    #[test]
    fn parses_exit_rows() {
        let rows = gate_rows(sample_source()).expect("rows");

        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].id, "P2E-01");
        assert_eq!(rows[0].status, "Strong draft");
        assert_eq!(rows[2].status, "Pending");
    }

    #[test]
    fn readiness_counts_statuses() {
        let rows = gate_rows(sample_source()).expect("rows");
        let report = ReadinessReport::from_rows(rows);

        assert_eq!(report.gates, 5);
        assert_eq!(report.done, 1);
        assert_eq!(report.strong_draft, 1);
        assert_eq!(report.partial, 1);
        assert_eq!(report.pending, 1);
        assert_eq!(report.blocked, 1);
        assert_eq!(report.proof_path_exists, 3);
        assert_eq!(report.needs_final_proof_or_decision.len(), 4);
        assert_eq!(report.hard_blockers.len(), 2);
    }

    #[test]
    fn config_accepts_full_open_gate_view() {
        let config = Config::parse(["--all".to_string()]).expect("config");

        assert!(config.show_all);
        assert!(config.evidence_page.ends_with(EVIDENCE_PAGE));
    }
}
