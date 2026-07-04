use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const ADAPTER_BOUNDARY_STEP: &str = "Adapter boundary check (`scripts/check-adapter-boundary.sh`)";
const SHARED_BEHAVIOR_STEP: &str =
    "Shared adapter behavior tests (`cargo test -p krate-adapter-common`)";

fn main() -> Result<()> {
    let config = Config::parse(env::args().skip(1))?;
    let reports = config.load_reports()?;
    compare_reports(&reports)
}

#[derive(Debug, Clone)]
struct Config {
    linux: PathBuf,
    macos: PathBuf,
    windows: PathBuf,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut linux = None;
        let mut macos = None;
        let mut windows = None;

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--linux" => linux = Some(next_path(&mut args, "--linux")?),
                "--macos" => macos = Some(next_path(&mut args, "--macos")?),
                "--windows" => windows = Some(next_path(&mut args, "--windows")?),
                "--help" | "-h" => {
                    println!(
                        "Usage: compare-phase2-adapter-evidence --linux <md> --macos <md> --windows <md>"
                    );
                    std::process::exit(0);
                }
                _ => bail!("unknown argument `{arg}`"),
            }
        }

        let config = Self {
            linux: linux.context("--linux is required")?,
            macos: macos.context("--macos is required")?,
            windows: windows.context("--windows is required")?,
        };
        config.validate_paths()?;
        Ok(config)
    }

    fn validate_paths(&self) -> Result<()> {
        for (label, path) in [
            ("linux", &self.linux),
            ("macos", &self.macos),
            ("windows", &self.windows),
        ] {
            if !path.is_file() {
                bail!("{label} report path does not exist: {}", path.display());
            }
        }
        Ok(())
    }

    fn load_reports(&self) -> Result<Vec<HostReport>> {
        vec![
            HostReport::parse("linux", &self.linux),
            HostReport::parse("macos", &self.macos),
            HostReport::parse("windows", &self.windows),
        ]
        .into_iter()
        .collect()
    }
}

fn next_path(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf> {
    args.next()
        .map(PathBuf::from)
        .with_context(|| format!("{flag} requires a path"))
}

fn native_adapter_step(label: &str) -> Result<&'static str> {
    match label {
        "linux" => Ok("Native adapter crate tests (`cargo test -p krate-adapter-linux`)"),
        "macos" => Ok("Native adapter crate tests (`cargo test -p krate-adapter-macos`)"),
        "windows" => Ok("Native adapter crate tests (`cargo test -p krate-adapter-windows`)"),
        other => bail!("unsupported report label `{other}`"),
    }
}

#[derive(Debug, Clone)]
struct HostReport {
    label: &'static str,
    source: PathBuf,
    git_commit: String,
    host_line: String,
    host_os: String,
    step_rows: BTreeMap<String, StepRow>,
}

impl HostReport {
    fn parse(label: &'static str, path: &Path) -> Result<Self> {
        let source =
            fs::read_to_string(path).with_context(|| format!("read report {}", path.display()))?;
        let git_commit_line = find_line_prefix(&source, "- Git commit:")
            .context("missing git commit metadata line")?;
        let host_line =
            find_line_prefix(&source, "- Host:").context("missing host metadata line")?;
        let git_commit = parse_markdown_tick_value(&git_commit_line)
            .context("git commit line is not in expected markdown-tick format")?;
        let host_os = parse_markdown_tick_value(&host_line)
            .context("host metadata line is not in expected markdown-tick format")?;
        let step_rows = parse_step_table(&source)?;
        for required in [
            ADAPTER_BOUNDARY_STEP,
            SHARED_BEHAVIOR_STEP,
            native_adapter_step(label)?,
        ] {
            if !step_rows.contains_key(required) {
                bail!(
                    "report {} is missing required step row `{required}`",
                    path.display()
                );
            }
        }

        Ok(Self {
            label,
            source: path.to_path_buf(),
            git_commit,
            host_line,
            host_os,
            step_rows,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepRow {
    exit_code: i32,
    result: String,
}

fn find_line_prefix(source: &str, prefix: &str) -> Option<String> {
    source
        .lines()
        .find(|line| line.trim_start().starts_with(prefix))
        .map(str::to_string)
}

fn parse_markdown_tick_value(line: &str) -> Result<String> {
    let mut parts = line.split('`');
    let _ = parts.next();
    let Some(value) = parts.next() else {
        bail!("line missing markdown tick value: {line}");
    };
    let value = value.trim();
    if value.is_empty() {
        bail!("markdown tick value is empty");
    }
    Ok(value.to_string())
}

fn parse_step_table(source: &str) -> Result<BTreeMap<String, StepRow>> {
    let mut rows = BTreeMap::new();
    let mut in_steps = false;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed == "| Step | Exit code | Result |" {
            in_steps = true;
            continue;
        }
        if !in_steps {
            continue;
        }
        if trimmed.starts_with("## ") {
            break;
        }
        if trimmed.starts_with("|---|") || !trimmed.starts_with('|') {
            continue;
        }

        let columns = trimmed
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(|column| column.trim().to_string())
            .collect::<Vec<_>>();

        if columns.len() != 3 {
            bail!("invalid step row format: `{line}`");
        }
        let step = columns[0].clone();
        let exit_code = columns[1]
            .parse::<i32>()
            .with_context(|| format!("invalid step exit code `{}`", columns[1]))?;
        let result = columns[2].clone();
        rows.insert(step, StepRow { exit_code, result });
    }

    if rows.is_empty() {
        bail!("no step rows found in adapter evidence report");
    }
    Ok(rows)
}

fn compare_reports(reports: &[HostReport]) -> Result<()> {
    println!("Krate Phase 2 adapter evidence comparison");
    for report in reports {
        println!(
            "- {}: {} ({})",
            report.label,
            report.source.display(),
            report.host_line
        );
    }

    validate_git_commit_alignment(reports)?;
    validate_host_assignments(reports)?;
    validate_step_outcomes(reports)?;

    println!("comparison passed: adapter evidence is aligned");
    Ok(())
}

fn validate_git_commit_alignment(reports: &[HostReport]) -> Result<()> {
    let Some(first) = reports.first() else {
        bail!("no reports provided");
    };
    let expected = &first.git_commit;
    for report in reports.iter().skip(1) {
        if report.git_commit != *expected {
            bail!(
                "git commit mismatch: expected `{}` but {} reports `{}`",
                expected,
                report.source.display(),
                report.git_commit
            );
        }
    }
    println!("- commit: match ({expected})");
    Ok(())
}

fn validate_host_assignments(reports: &[HostReport]) -> Result<()> {
    for report in reports {
        let normalized = report.host_os.to_ascii_lowercase();
        let matches = match report.label {
            "linux" => normalized.contains("linux"),
            "macos" => normalized.contains("darwin"),
            "windows" => {
                normalized.contains("windows")
                    || normalized.contains("mingw")
                    || normalized.contains("msys")
                    || normalized.contains("cygwin")
            }
            other => bail!("unsupported report label `{other}`"),
        };
        if !matches {
            bail!(
                "{} was passed as `--{}` but host metadata says `{}`",
                report.source.display(),
                report.label,
                report.host_os
            );
        }
    }
    Ok(())
}

fn validate_step_outcomes(reports: &[HostReport]) -> Result<()> {
    for report in reports {
        for required in [
            ADAPTER_BOUNDARY_STEP,
            SHARED_BEHAVIOR_STEP,
            native_adapter_step(report.label)?,
        ] {
            let row = report.step_rows.get(required).with_context(|| {
                format!(
                    "{} is missing required step row `{required}`",
                    report.source.display()
                )
            })?;
            if row.exit_code != 0 || row.result != "passed" {
                bail!(
                    "{} step `{required}` failed with exit code {} ({})",
                    report.source.display(),
                    row.exit_code,
                    row.result
                );
            }
        }
    }
    println!("- steps: adapter boundary and native adapter behavior tests passed on all hosts");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report(label: &str, host_os: &str, commit: &str, result: &str, code: i32) -> String {
        let native_step = native_adapter_step(label).expect("native step");
        format!(
            r#"# Phase 2 Adapter Evidence

## Host

- Git commit: `{commit}`
- Host: `{host_os}` / `x86_64`
- Generated at (UTC): `2026-05-08T00:00:00Z`

## Command Results

| Step | Exit code | Result |
|---|---:|---|
| Adapter boundary check (`scripts/check-adapter-boundary.sh`) | {code} | {result} |
| Shared adapter behavior tests (`cargo test -p krate-adapter-common`) | {code} | {result} |
| {native_step} | {code} | {result} |
"#
        )
    }

    #[test]
    fn parses_step_rows() {
        let rows = parse_step_table(&sample_report("linux", "Linux", "abc123", "passed", 0))
            .expect("rows");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[ADAPTER_BOUNDARY_STEP].result, "passed");
    }

    #[test]
    fn rejects_failed_step() {
        let reports = vec![
            HostReport::parse_report_text(
                "linux",
                sample_report("linux", "Linux", "abc123", "failed", 1),
            )
            .expect("linux"),
            HostReport::parse_report_text(
                "macos",
                sample_report("macos", "Darwin", "abc123", "passed", 0),
            )
            .expect("macos"),
            HostReport::parse_report_text(
                "windows",
                sample_report("windows", "Windows_NT", "abc123", "passed", 0),
            )
            .expect("windows"),
        ];

        let err = compare_reports(&reports).expect_err("should fail");
        assert!(err.to_string().contains(ADAPTER_BOUNDARY_STEP));
    }

    impl HostReport {
        fn parse_report_text(label: &'static str, text: String) -> Result<Self> {
            let git_commit_line = find_line_prefix(&text, "- Git commit:")
                .context("missing git commit metadata line")?;
            let host_line =
                find_line_prefix(&text, "- Host:").context("missing host metadata line")?;
            let git_commit = parse_markdown_tick_value(&git_commit_line)
                .context("git commit line is not in expected markdown-tick format")?;
            let host_os = parse_markdown_tick_value(&host_line)
                .context("host metadata line is not in expected markdown-tick format")?;
            let step_rows = parse_step_table(&text)?;
            Ok(Self {
                label,
                source: PathBuf::from(format!("{label}.md")),
                git_commit,
                host_line,
                host_os,
                step_rows,
            })
        }
    }
}
