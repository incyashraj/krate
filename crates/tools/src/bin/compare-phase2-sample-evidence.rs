use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const REQUIRED_SAMPLES: &[&str] = &["krate-clock", "krate-cat", "krate-curl"];

fn main() -> Result<()> {
    let config = Config::parse(env::args().skip(1))?;
    let reports = config.load_reports()?;
    compare_reports(&reports, config.allow_blocked_curl)
}

#[derive(Debug, Clone)]
struct Config {
    linux: PathBuf,
    macos: PathBuf,
    windows: PathBuf,
    allow_blocked_curl: bool,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut linux = None;
        let mut macos = None;
        let mut windows = None;
        let mut allow_blocked_curl = false;

        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--linux" => linux = Some(next_path(&mut args, "--linux")?),
                "--macos" => macos = Some(next_path(&mut args, "--macos")?),
                "--windows" => windows = Some(next_path(&mut args, "--windows")?),
                "--allow-blocked-curl" => allow_blocked_curl = true,
                "--help" | "-h" => {
                    println!(
                        "Usage: compare-phase2-sample-evidence --linux <md> --macos <md> --windows <md> [--allow-blocked-curl]"
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
            allow_blocked_curl,
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
    sample_rows: BTreeMap<String, SampleRow>,
}

impl HostReport {
    fn parse(label: &'static str, path: &Path) -> Result<Self> {
        let source =
            fs::read_to_string(path).with_context(|| format!("read report {}", path.display()))?;
        let git_commit_line = find_git_commit_line(&source).context("missing git commit line")?;
        let git_commit = parse_git_commit(&git_commit_line)
            .with_context(|| format!("invalid git commit line in report {}", path.display()))?;
        let host_line = find_host_line(&source).context("missing host metadata line")?;
        let host_os = parse_host_os(&host_line)
            .with_context(|| format!("invalid host line in report {}", path.display()))?;
        let sample_rows = parse_sample_rows(&source)?;
        for required in REQUIRED_SAMPLES {
            if !sample_rows.contains_key(*required) {
                bail!(
                    "report {} is missing sample row `{required}`",
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
            sample_rows,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SampleRow {
    sample: String,
    status: String,
    exit_code: Option<i32>,
    stdout_sha256: Option<String>,
}

fn find_host_line(source: &str) -> Option<String> {
    source
        .lines()
        .find(|line| line.trim_start().starts_with("- Host:"))
        .map(str::to_string)
}

fn find_git_commit_line(source: &str) -> Option<String> {
    source
        .lines()
        .find(|line| line.trim_start().starts_with("- Git commit:"))
        .map(str::to_string)
}

fn parse_git_commit(git_commit_line: &str) -> Result<String> {
    let mut parts = git_commit_line.split('`');
    let _ = parts.next();
    let Some(commit) = parts.next() else {
        bail!("git commit line must include markdown code ticks around commit");
    };
    let commit = commit.trim();
    if commit.is_empty() {
        bail!("git commit value is empty");
    }
    Ok(commit.to_string())
}

fn parse_host_os(host_line: &str) -> Result<String> {
    let mut parts = host_line.split('`');
    let _ = parts.next();
    let Some(os) = parts.next() else {
        bail!("host line must include markdown code ticks around OS");
    };
    let os = os.trim();
    if os.is_empty() {
        bail!("host OS value is empty");
    }
    Ok(os.to_string())
}

fn parse_sample_rows(source: &str) -> Result<BTreeMap<String, SampleRow>> {
    let mut rows = BTreeMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("| krate-") {
            continue;
        }
        let columns = trimmed
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(|column| column.trim().to_string())
            .collect::<Vec<_>>();
        if columns.len() != 5 {
            bail!("invalid sample table row: `{line}`");
        }
        let sample = columns[0].clone();
        let status = columns[1].clone();
        let exit_code = parse_exit_code(&columns[2])?;
        let stdout_sha256 = parse_hash(&columns[3]);
        rows.insert(
            sample.clone(),
            SampleRow {
                sample,
                status,
                exit_code,
                stdout_sha256,
            },
        );
    }

    if rows.is_empty() {
        bail!("no sample rows found in evidence report");
    }
    Ok(rows)
}

fn parse_exit_code(value: &str) -> Result<Option<i32>> {
    if value == "n/a" {
        return Ok(None);
    }
    let parsed = value
        .parse::<i32>()
        .with_context(|| format!("invalid exit code `{value}`"))?;
    Ok(Some(parsed))
}

fn parse_hash(value: &str) -> Option<String> {
    let cleaned = value.trim_matches('`').trim();
    if cleaned == "n/a" || cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn compare_reports(reports: &[HostReport], allow_blocked_curl: bool) -> Result<()> {
    println!("Krate Phase 2 sample evidence comparison");
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

    for sample in REQUIRED_SAMPLES {
        compare_sample(reports, sample, allow_blocked_curl)?;
    }

    println!("comparison passed: sample stdout hashes are aligned");
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
        let expected = match report.label {
            "linux" => "linux",
            "macos" => "macos",
            "windows" => "windows",
            other => bail!("unsupported report label `{other}`"),
        };
        if report.host_os != expected {
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

fn compare_sample(reports: &[HostReport], sample: &str, allow_blocked_curl: bool) -> Result<()> {
    let mut passed_hashes = Vec::new();
    let mut blocked_hosts = Vec::new();

    for report in reports {
        let row = report
            .sample_rows
            .get(sample)
            .with_context(|| format!("missing `{sample}` row in {}", report.source.display()))?;
        match row.status.as_str() {
            "passed" => {
                let exit = row.exit_code.unwrap_or(-1);
                if exit != 0 {
                    bail!(
                        "{} reports `{sample}` as passed but exit code is `{exit}`",
                        report.source.display()
                    );
                }
                let Some(hash) = &row.stdout_sha256 else {
                    bail!(
                        "{} reports `{sample}` as passed but stdout hash is missing",
                        report.source.display()
                    );
                };
                passed_hashes.push((report.label, hash.clone()));
            }
            "blocked" => blocked_hosts.push(report.label),
            other => {
                bail!(
                    "{} reports `{sample}` with unsupported status `{other}`",
                    report.source.display()
                )
            }
        }
    }

    if !blocked_hosts.is_empty() {
        if sample == "krate-curl" && allow_blocked_curl {
            if passed_hashes.len() >= 2 {
                validate_passed_hashes_match(sample, &passed_hashes)?;
                println!(
                    "- {sample}: blocked on {}; passed hosts match ({})",
                    blocked_hosts.join(", "),
                    passed_hashes[0].1
                );
            } else {
                println!(
                    "- {sample}: blocked on {} (allowed by flag)",
                    blocked_hosts.join(", ")
                );
            }
            return Ok(());
        }
        bail!(
            "{sample} is blocked on {}. Re-run on those hosts or pass --allow-blocked-curl for temporary curl-only exception.",
            blocked_hosts.join(", ")
        );
    }

    validate_passed_hashes_match(sample, &passed_hashes)?;
    println!("- {sample}: match ({})", passed_hashes[0].1);
    Ok(())
}

fn validate_passed_hashes_match(
    sample: &str,
    passed_hashes: &[(&'static str, String)],
) -> Result<()> {
    let first_hash = &passed_hashes[0].1;
    if passed_hashes.iter().any(|(_, hash)| hash != first_hash) {
        let detail = passed_hashes
            .iter()
            .map(|(host, hash)| format!("{host}={hash}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("{sample} stdout hash mismatch across hosts: {detail}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report(host_os: &str, status_curl: &str, hash_suffix: &str) -> String {
        format!(
            r#"# Phase 2 Sample Evidence Run

## Host

- Git commit: `abc1234`
- Host: `{host_os}` / `aarch64`

## Results

| Sample | Status | Exit | Stdout SHA-256 | Stderr SHA-256 |
|---|---|---:|---|---|
| krate-clock | passed | 0 | `clock{hash_suffix}` | `z` |
| krate-cat | passed | 0 | `cat{hash_suffix}` | `z` |
| krate-curl | {status_curl} | n/a | `n/a` | `n/a` |
"#
        )
    }

    #[test]
    fn parses_rows_from_markdown_table() {
        let rows = parse_sample_rows(&sample_report("linux", "blocked", "123")).expect("rows");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows["krate-clock"].status, "passed");
        assert_eq!(rows["krate-curl"].status, "blocked");
    }

    #[test]
    fn parses_host_os_from_host_line() {
        let os = parse_host_os("- Host: `windows` / `x86_64`").expect("host os");
        assert_eq!(os, "windows");
    }

    #[test]
    fn parses_git_commit_from_line() {
        let commit = parse_git_commit("- Git commit: `deadbee`").expect("git commit");
        assert_eq!(commit, "deadbee");
    }

    #[test]
    fn compare_allows_blocked_curl_when_flag_set() {
        let make = |label: &'static str, host_os: &'static str| HostReport {
            label,
            source: PathBuf::from(format!("{label}.md")),
            git_commit: "abc1234".to_string(),
            host_line: format!("- Host: `{host_os}` / `aarch64`"),
            host_os: host_os.to_string(),
            sample_rows: parse_sample_rows(&sample_report(host_os, "blocked", "aaa"))
                .expect("rows"),
        };

        compare_reports(
            &[
                make("linux", "linux"),
                make("macos", "macos"),
                make("windows", "windows"),
            ],
            true,
        )
        .expect("comparison should pass");
    }

    #[test]
    fn compare_still_checks_passed_curl_hashes_when_another_host_is_blocked() {
        let host = |label: &'static str,
                    host_os: &'static str,
                    curl_status: &'static str,
                    curl_exit: &'static str,
                    curl_hash: &'static str|
         -> HostReport {
            HostReport {
                label,
                source: PathBuf::from(format!("{label}.md")),
                git_commit: "abc1234".to_string(),
                host_line: format!("- Host: `{host_os}` / `x86_64`"),
                host_os: host_os.to_string(),
                sample_rows: parse_sample_rows(&format!(
                    r#"# Phase 2 Sample Evidence Run
## Host
- Git commit: `abc1234`
- Host: `{host_os}` / `x86_64`
## Results
| Sample | Status | Exit | Stdout SHA-256 | Stderr SHA-256 |
|---|---|---:|---|---|
| krate-clock | passed | 0 | `clock1` | `z` |
| krate-cat | passed | 0 | `cat1` | `z` |
| krate-curl | {curl_status} | {curl_exit} | `{curl_hash}` | `z` |
"#
                ))
                .expect("rows"),
            }
        };

        let result = compare_reports(
            &[
                host("linux", "linux", "passed", "0", "curl1"),
                host("macos", "macos", "passed", "0", "curl2"),
                host("windows", "windows", "blocked", "n/a", "n/a"),
            ],
            true,
        );

        assert!(result.is_err());
    }

    #[test]
    fn compare_fails_when_hashes_differ() {
        let host = |label: &'static str, host_os: &'static str, suffix: &str| HostReport {
            label,
            source: PathBuf::from(format!("{label}.md")),
            git_commit: "abc1234".to_string(),
            host_line: format!("- Host: `{host_os}` / `x86_64`"),
            host_os: host_os.to_string(),
            sample_rows: parse_sample_rows(&format!(
                r#"# Phase 2 Sample Evidence Run
## Host
- Git commit: `abc1234`
- Host: `{host_os}` / `x86_64`
## Results
| Sample | Status | Exit | Stdout SHA-256 | Stderr SHA-256 |
|---|---|---:|---|---|
| krate-clock | passed | 0 | `clock{suffix}` | `z` |
| krate-cat | passed | 0 | `cat{suffix}` | `z` |
| krate-curl | passed | 0 | `curl{suffix}` | `z` |
"#
            ))
            .expect("rows"),
        };

        let result = compare_reports(
            &[
                host("linux", "linux", "1"),
                host("macos", "macos", "2"),
                host("windows", "windows", "1"),
            ],
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn compare_fails_when_report_host_does_not_match_flag() {
        let host = |label: &'static str, host_os: &'static str| HostReport {
            label,
            source: PathBuf::from(format!("{label}.md")),
            git_commit: "abc1234".to_string(),
            host_line: format!("- Host: `{host_os}` / `x86_64`"),
            host_os: host_os.to_string(),
            sample_rows: parse_sample_rows(&sample_report(host_os, "blocked", "aaa"))
                .expect("rows"),
        };

        let result = compare_reports(
            &[
                host("linux", "macos"),
                host("macos", "macos"),
                host("windows", "windows"),
            ],
            true,
        );
        assert!(result.is_err());
    }

    #[test]
    fn compare_fails_when_git_commits_differ() {
        let host =
            |label: &'static str, host_os: &'static str, git_commit: &'static str| HostReport {
                label,
                source: PathBuf::from(format!("{label}.md")),
                git_commit: git_commit.to_string(),
                host_line: format!("- Host: `{host_os}` / `x86_64`"),
                host_os: host_os.to_string(),
                sample_rows: parse_sample_rows(&format!(
                    r#"# Phase 2 Sample Evidence Run
## Host
- Git commit: `{git_commit}`
- Host: `{host_os}` / `x86_64`
## Results
| Sample | Status | Exit | Stdout SHA-256 | Stderr SHA-256 |
|---|---|---:|---|---|
| krate-clock | passed | 0 | `clock1` | `z` |
| krate-cat | passed | 0 | `cat1` | `z` |
| krate-curl | blocked | n/a | `n/a` | `n/a` |
"#
                ))
                .expect("rows"),
            };

        let result = compare_reports(
            &[
                host("linux", "linux", "abc1234"),
                host("macos", "macos", "def5678"),
                host("windows", "windows", "abc1234"),
            ],
            true,
        );
        assert!(result.is_err());
    }
}
