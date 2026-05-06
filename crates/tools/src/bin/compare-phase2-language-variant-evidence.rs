use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const REQUIRED_STEPS: &[&str] = &[
    "Build language fixtures (`scripts/build-phase2-language-variant-fixtures.sh`)",
    "Test language variants (`scripts/test-phase2-language-variants.sh`)",
];

const REQUIRED_FIXTURES: &[&str] = &[
    "layer36_go_clock.wasm",
    "layer36_go_cat.wasm",
    "layer36_go_curl.wasm",
    "layer36_ts_clock.wasm",
    "layer36_ts_cat.wasm",
    "layer36_ts_curl.wasm",
];

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
                        "Usage: compare-phase2-language-variant-evidence --linux <md> --macos <md> --windows <md>"
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

#[derive(Debug, Clone)]
struct HostReport {
    label: &'static str,
    source: PathBuf,
    git_commit: String,
    host_line: String,
    host_os: String,
    step_rows: BTreeMap<String, StepRow>,
    fixture_rows: BTreeMap<String, FixtureRow>,
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
        let (step_rows, fixture_rows) = parse_tables(&source)?;

        for required in REQUIRED_STEPS {
            if !step_rows.contains_key(*required) {
                bail!(
                    "report {} is missing required step row `{required}`",
                    path.display()
                );
            }
        }

        for required in REQUIRED_FIXTURES {
            if !fixture_rows.contains_key(*required) {
                bail!(
                    "report {} is missing required fixture row `{required}`",
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
            fixture_rows,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepRow {
    step: String,
    exit_code: i32,
    result: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FixtureRow {
    fixture: String,
    exists: bool,
    sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseMode {
    None,
    Step,
    Fixture,
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

fn parse_tables(source: &str) -> Result<(BTreeMap<String, StepRow>, BTreeMap<String, FixtureRow>)> {
    let mut steps = BTreeMap::new();
    let mut fixtures = BTreeMap::new();
    let mut mode = ParseMode::None;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed == "| Step | Exit code | Result |" {
            mode = ParseMode::Step;
            continue;
        }
        if trimmed == "| Fixture | Exists | SHA-256 |" {
            mode = ParseMode::Fixture;
            continue;
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

        match mode {
            ParseMode::Step => {
                if columns.len() != 3 {
                    bail!("invalid step row format: `{line}`");
                }
                let step = columns[0].clone();
                let exit_code = columns[1]
                    .parse::<i32>()
                    .with_context(|| format!("invalid step exit code `{}`", columns[1]))?;
                let result = columns[2].clone();
                steps.insert(
                    step.clone(),
                    StepRow {
                        step,
                        exit_code,
                        result,
                    },
                );
            }
            ParseMode::Fixture => {
                if columns.len() != 3 {
                    bail!("invalid fixture row format: `{line}`");
                }
                let fixture = columns[0].clone();
                let exists = match columns[1].as_str() {
                    "yes" => true,
                    "no" => false,
                    other => bail!("invalid fixture exists value `{other}`"),
                };
                let sha_clean = columns[2].trim_matches('`').trim();
                let sha256 = if sha_clean == "n/a" || sha_clean.is_empty() {
                    None
                } else {
                    Some(sha_clean.to_string())
                };
                fixtures.insert(
                    fixture.clone(),
                    FixtureRow {
                        fixture,
                        exists,
                        sha256,
                    },
                );
            }
            ParseMode::None => {}
        }
    }

    if steps.is_empty() {
        bail!("no step rows found in language-variant evidence report");
    }
    if fixtures.is_empty() {
        bail!("no fixture rows found in language-variant evidence report");
    }

    Ok((steps, fixtures))
}

fn compare_reports(reports: &[HostReport]) -> Result<()> {
    println!("Layer36 Phase 2 language-variant evidence comparison");
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
    compare_fixtures(reports)?;

    println!("comparison passed: language-variant evidence is aligned");
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
            "linux" => normalized == "linux",
            "macos" => normalized == "darwin" || normalized == "macos",
            "windows" => {
                normalized.contains("windows")
                    || normalized.contains("mingw")
                    || normalized.contains("msys")
                    || normalized.contains("cygwin")
            }
            _ => false,
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
    for step in REQUIRED_STEPS {
        for report in reports {
            let row = report
                .step_rows
                .get(*step)
                .with_context(|| format!("missing step `{step}` in {}", report.source.display()))?;
            if row.exit_code != 0 {
                bail!(
                    "{} reports step `{step}` with non-zero exit code {}",
                    report.source.display(),
                    row.exit_code
                );
            }
            if row.result != "passed" {
                bail!(
                    "{} reports step `{step}` with non-passed result `{}`",
                    report.source.display(),
                    row.result
                );
            }
        }
        println!("- step `{step}`: passed on all hosts");
    }
    Ok(())
}

fn compare_fixtures(reports: &[HostReport]) -> Result<()> {
    for fixture in REQUIRED_FIXTURES {
        let mut exists_values = Vec::with_capacity(reports.len());
        let mut hashes = Vec::with_capacity(reports.len());

        for report in reports {
            let row = report.fixture_rows.get(*fixture).with_context(|| {
                format!(
                    "missing fixture `{fixture}` row in {}",
                    report.source.display()
                )
            })?;
            exists_values.push((report.label, row.exists));
            hashes.push((report.label, row.sha256.clone()));
        }

        let all_exists = exists_values.iter().all(|(_, exists)| *exists);
        let none_exists = exists_values.iter().all(|(_, exists)| !*exists);
        if !all_exists && !none_exists {
            let detail = exists_values
                .iter()
                .map(|(host, exists)| format!("{host}={exists}"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!("fixture `{fixture}` exists mismatch across hosts: {detail}");
        }

        if none_exists {
            println!("- fixture `{fixture}`: missing on all hosts");
            continue;
        }

        let Some(first_hash) = hashes[0].1.as_ref() else {
            bail!(
                "fixture `{fixture}` is present but hash is missing on {}",
                hashes[0].0
            );
        };
        for (host, hash) in hashes.iter().skip(1) {
            let Some(hash) = hash else {
                bail!("fixture `{fixture}` is present but hash is missing on {host}");
            };
            if hash != first_hash {
                let detail = hashes
                    .iter()
                    .map(|(h, value)| {
                        format!("{h}={}", value.clone().unwrap_or_else(|| "n/a".to_string()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("fixture `{fixture}` hash mismatch across hosts: {detail}");
            }
        }

        println!("- fixture `{fixture}`: match ({first_hash})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(host: &str, commit: &str, ts_hash_suffix: &str, go_exists: bool) -> String {
        let go_exists_text = if go_exists { "yes" } else { "no" };
        let go_hash = if go_exists { "`gohash`" } else { "`n/a`" };

        format!(
            r#"# Phase 2 Language Variant Evidence

## Host

- Git commit: `{commit}`
- Host: `{host}` / `x86_64`
- Generated at (UTC): `2026-01-01T00:00:00Z`
- Fixture mode: `optional`

## Command Results

| Step | Exit code | Result |
|---|---:|---|
| Build language fixtures (`scripts/build-phase2-language-variant-fixtures.sh`) | 0 | passed |
| Test language variants (`scripts/test-phase2-language-variants.sh`) | 0 | passed |

## Fixture Files

| Fixture | Exists | SHA-256 |
|---|---|---|
| layer36_go_clock.wasm | {go_exists_text} | {go_hash} |
| layer36_go_cat.wasm | {go_exists_text} | {go_hash} |
| layer36_go_curl.wasm | {go_exists_text} | {go_hash} |
| layer36_ts_clock.wasm | yes | `ts-clock-{ts_hash_suffix}` |
| layer36_ts_cat.wasm | yes | `ts-cat-{ts_hash_suffix}` |
| layer36_ts_curl.wasm | yes | `ts-curl-{ts_hash_suffix}` |
"#
        )
    }

    fn parsed(
        label: &'static str,
        host: &str,
        commit: &str,
        ts_hash_suffix: &str,
        go_exists: bool,
    ) -> HostReport {
        let text = report(host, commit, ts_hash_suffix, go_exists);
        let (steps, fixtures) = parse_tables(&text).expect("parse tables");
        HostReport {
            label,
            source: PathBuf::from(format!("{label}.md")),
            git_commit: commit.to_string(),
            host_line: format!("- Host: `{host}` / `x86_64`"),
            host_os: host.to_string(),
            step_rows: steps,
            fixture_rows: fixtures,
        }
    }

    #[test]
    fn parses_tables_from_markdown() {
        let (steps, fixtures) =
            parse_tables(&report("Darwin", "abc123", "same", false)).expect("tables");
        assert_eq!(steps.len(), 2);
        assert_eq!(fixtures.len(), 6);
        assert!(fixtures["layer36_ts_curl.wasm"].exists);
        assert!(!fixtures["layer36_go_curl.wasm"].exists);
    }

    #[test]
    fn compare_accepts_matching_reports() {
        let reports = vec![
            parsed("linux", "Linux", "abc123", "same", false),
            parsed("macos", "Darwin", "abc123", "same", false),
            parsed("windows", "MINGW64_NT-10.0", "abc123", "same", false),
        ];
        compare_reports(&reports).expect("comparison should pass");
    }

    #[test]
    fn compare_fails_when_commit_differs() {
        let reports = vec![
            parsed("linux", "Linux", "abc123", "same", false),
            parsed("macos", "Darwin", "def999", "same", false),
            parsed("windows", "MINGW64_NT-10.0", "abc123", "same", false),
        ];
        assert!(compare_reports(&reports).is_err());
    }

    #[test]
    fn compare_fails_when_fixture_hash_differs() {
        let reports = vec![
            parsed("linux", "Linux", "abc123", "same", false),
            parsed("macos", "Darwin", "abc123", "drift", false),
            parsed("windows", "MINGW64_NT-10.0", "abc123", "same", false),
        ];
        assert!(compare_reports(&reports).is_err());
    }

    #[test]
    fn compare_fails_when_fixture_exists_mismatch() {
        let reports = vec![
            parsed("linux", "Linux", "abc123", "same", false),
            parsed("macos", "Darwin", "abc123", "same", true),
            parsed("windows", "MINGW64_NT-10.0", "abc123", "same", false),
        ];
        assert!(compare_reports(&reports).is_err());
    }
}
