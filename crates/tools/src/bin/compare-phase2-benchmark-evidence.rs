use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const REQUIRED_STEPS: &[&str] = &[
    "Startup benchmark (`cargo bench -p krate-runtime --bench startup`)",
    "Dispatch benchmark (`cargo bench -p krate-runtime --bench uapi_dispatch`)",
    "Regression check (`scripts/check-benchmark-regression.sh`)",
    "CLI release build (`cargo build -p krate-cli --release`)",
    "Clock component build (`scripts/build-krate-clock-component.sh`)",
    "Full CLI startup (`krate run krate-clock`)",
];

const REQUIRED_METRICS: &[&str] = &[
    "phase2_component_from_binary_smoke",
    "phase2_cold_start_to_main_smoke",
    "phase2_loaded_run_smoke",
    "phase2_loaded_run_clock_fixed_time",
    "phase2_uapi_default_stdout_grant",
    "phase2_uapi_fs_open_read_granted",
    "phase2_uapi_fs_handle_read_granted",
    "phase2_uapi_fs_handle_write_granted",
    "phase2_uapi_fs_missing_read_denied",
    "phase2_uapi_net_fetch_granted",
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
                        "Usage: compare-phase2-benchmark-evidence --linux <md> --macos <md> --windows <md>"
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
    metric_rows: BTreeMap<String, MetricRow>,
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
        let (step_rows, metric_rows) = parse_tables(&source)?;

        for required in REQUIRED_STEPS {
            if !step_rows.contains_key(*required) {
                bail!(
                    "report {} is missing required step row `{required}`",
                    path.display()
                );
            }
        }

        for metric in REQUIRED_METRICS {
            if !metric_rows.contains_key(*metric) {
                bail!(
                    "report {} is missing required metric row `{metric}`",
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
            metric_rows,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepRow {
    exit_code: i32,
    result: String,
}

#[derive(Debug, Clone, PartialEq)]
struct MetricRow {
    current_ns: Option<u64>,
    baseline_ns: u64,
    threshold_pct: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParseMode {
    None,
    Steps,
    Metrics,
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

fn parse_tables(source: &str) -> Result<(BTreeMap<String, StepRow>, BTreeMap<String, MetricRow>)> {
    let mut steps = BTreeMap::new();
    let mut metrics = BTreeMap::new();
    let mut mode = ParseMode::None;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed == "| Step | Exit code | Result |" {
            mode = ParseMode::Steps;
            continue;
        }
        if trimmed == "| Metric | Current ns | Baseline ns | Threshold % |" {
            mode = ParseMode::Metrics;
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
            ParseMode::Steps => {
                if columns.len() != 3 {
                    bail!("invalid step row format: `{line}`");
                }
                let step = columns[0].clone();
                let exit_code = columns[1]
                    .parse::<i32>()
                    .with_context(|| format!("invalid step exit code `{}`", columns[1]))?;
                let result = columns[2].clone();
                steps.insert(step, StepRow { exit_code, result });
            }
            ParseMode::Metrics => {
                if columns.len() != 4 {
                    bail!("invalid metric row format: `{line}`");
                }
                let metric = columns[0].clone();
                let current_ns = match columns[1].as_str() {
                    "n/a" => None,
                    value => Some(
                        value
                            .parse::<u64>()
                            .with_context(|| format!("invalid current ns `{value}`"))?,
                    ),
                };
                let baseline_ns = columns[2]
                    .parse::<u64>()
                    .with_context(|| format!("invalid baseline ns `{}`", columns[2]))?;
                let threshold_pct = columns[3]
                    .parse::<f64>()
                    .with_context(|| format!("invalid threshold % `{}`", columns[3]))?;
                metrics.insert(
                    metric,
                    MetricRow {
                        current_ns,
                        baseline_ns,
                        threshold_pct,
                    },
                );
            }
            ParseMode::None => {}
        }
    }

    if steps.is_empty() {
        bail!("no step rows found in benchmark evidence report");
    }
    if metrics.is_empty() {
        bail!("no metric rows found in benchmark evidence report");
    }

    Ok((steps, metrics))
}

fn compare_reports(reports: &[HostReport]) -> Result<()> {
    println!("Krate Phase 2 benchmark evidence comparison");
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
    validate_metric_shape(reports)?;
    validate_metric_thresholds(reports)?;

    println!("comparison passed: benchmark evidence is aligned");
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
    for step in REQUIRED_STEPS {
        for report in reports {
            let row = report.step_rows.get(*step).with_context(|| {
                format!(
                    "{} is missing required step row `{step}`",
                    report.source.display()
                )
            })?;
            if row.exit_code != 0 || row.result != "passed" {
                bail!(
                    "{} step `{step}` failed with exit code {} ({})",
                    report.source.display(),
                    row.exit_code,
                    row.result
                );
            }
        }
        println!("- step: {step} (passed on all hosts)");
    }
    Ok(())
}

fn validate_metric_shape(reports: &[HostReport]) -> Result<()> {
    let Some(reference) = reports.first() else {
        bail!("no reports provided");
    };

    for metric in REQUIRED_METRICS {
        let reference_row = reference.metric_rows.get(*metric).with_context(|| {
            format!(
                "{} is missing required metric row `{metric}`",
                reference.source.display()
            )
        })?;

        if reference_row.current_ns.is_none() {
            bail!(
                "{} metric `{metric}` has no current value (n/a)",
                reference.source.display()
            );
        }

        for report in reports.iter().skip(1) {
            let row = report.metric_rows.get(*metric).with_context(|| {
                format!(
                    "{} is missing required metric row `{metric}`",
                    report.source.display()
                )
            })?;

            if row.current_ns.is_none() {
                bail!(
                    "{} metric `{metric}` has no current value (n/a)",
                    report.source.display()
                );
            }
            if row.baseline_ns != reference_row.baseline_ns {
                bail!(
                    "baseline mismatch for `{metric}`: {} has {}, {} has {}",
                    reference.source.display(),
                    reference_row.baseline_ns,
                    report.source.display(),
                    row.baseline_ns
                );
            }
            if row.threshold_pct != reference_row.threshold_pct {
                bail!(
                    "threshold mismatch for `{metric}`: {} has {}, {} has {}",
                    reference.source.display(),
                    reference_row.threshold_pct,
                    report.source.display(),
                    row.threshold_pct
                );
            }
        }
    }

    println!("- metrics: present with shared baseline and threshold shape");
    Ok(())
}

fn validate_metric_thresholds(reports: &[HostReport]) -> Result<()> {
    for metric in REQUIRED_METRICS {
        for report in reports {
            let row = report.metric_rows.get(*metric).with_context(|| {
                format!(
                    "{} is missing required metric row `{metric}`",
                    report.source.display()
                )
            })?;
            let Some(current) = row.current_ns else {
                bail!(
                    "{} metric `{metric}` has no current value (n/a)",
                    report.source.display()
                );
            };

            let allowed = (row.baseline_ns as f64) * (1.0 + row.threshold_pct / 100.0);
            if (current as f64) > allowed {
                bail!(
                    "{} metric `{metric}` regressed: current {} ns, baseline {} ns, threshold {}%, allowed {} ns",
                    report.source.display(),
                    current,
                    row.baseline_ns,
                    row.threshold_pct,
                    allowed.round() as u64
                );
            }
        }
    }

    println!("- metrics: current values are within baseline thresholds on all hosts");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report(
        host_os: &str,
        commit: &str,
        step_result: &str,
        step_code: i32,
        metric_override: Option<(&str, u64)>,
    ) -> String {
        let mut metrics = String::new();
        for metric in REQUIRED_METRICS {
            let current = match metric_override {
                Some((target, value)) if *metric == target => value,
                _ => 95,
            };
            metrics.push_str(&format!("| {metric} | {current} | 90 | 10 |\n"));
        }
        format!(
            r#"# Phase 2 Benchmark Evidence

## Host

- Git commit: `{commit}`
- Host: `{host_os}` / `x86_64`
- Generated at (UTC): `2026-05-08T00:00:00Z`
- Benchmark run mode: `run`
- Regression mode: `warn`
- Regression threshold %: `10`
- Baseline file: `docs/book/src/phase2/benchmark-baseline.json`

## Command Results

| Step | Exit code | Result |
|---|---:|---|
| Startup benchmark (`cargo bench -p krate-runtime --bench startup`) | {step_code} | {step_result} |
| Dispatch benchmark (`cargo bench -p krate-runtime --bench uapi_dispatch`) | {step_code} | {step_result} |
| Regression check (`scripts/check-benchmark-regression.sh`) | {step_code} | {step_result} |
| CLI release build (`cargo build -p krate-cli --release`) | {step_code} | {step_result} |
| Clock component build (`scripts/build-krate-clock-component.sh`) | {step_code} | {step_result} |
| Full CLI startup (`krate run krate-clock`) | {step_code} | {step_result} |

## Metric Snapshot

| Metric | Current ns | Baseline ns | Threshold % |
|---|---:|---:|---:|
{metrics}
"#
        )
    }

    #[test]
    fn parses_benchmark_tables() {
        let (steps, metrics) = parse_tables(&sample_report("Linux", "abc123", "passed", 0, None))
            .expect("parse tables");
        assert_eq!(steps.len(), REQUIRED_STEPS.len());
        assert_eq!(metrics.len(), REQUIRED_METRICS.len());
    }

    #[test]
    fn rejects_failed_step() {
        let reports = vec![
            HostReport::parse_report_text(
                "linux",
                sample_report("Linux", "abc123", "failed", 1, None),
            )
            .expect("linux"),
            HostReport::parse_report_text(
                "macos",
                sample_report("Darwin", "abc123", "passed", 0, None),
            )
            .expect("macos"),
            HostReport::parse_report_text(
                "windows",
                sample_report("Windows_NT", "abc123", "passed", 0, None),
            )
            .expect("windows"),
        ];

        let err = compare_reports(&reports).expect_err("should fail");
        assert!(err.to_string().contains("Startup benchmark"));
    }

    #[test]
    fn rejects_metric_regression() {
        let bad_linux = sample_report(
            "Linux",
            "abc123",
            "passed",
            0,
            Some(("phase2_uapi_default_stdout_grant", 1000)),
        );
        let (_, bad_metrics) = parse_tables(&bad_linux).expect("parse bad metrics");
        assert_eq!(
            bad_metrics["phase2_uapi_default_stdout_grant"].current_ns,
            Some(1000)
        );
        let reports = vec![
            HostReport::parse_report_text("linux", bad_linux).expect("linux"),
            HostReport::parse_report_text(
                "macos",
                sample_report("Darwin", "abc123", "passed", 0, None),
            )
            .expect("macos"),
            HostReport::parse_report_text(
                "windows",
                sample_report("Windows_NT", "abc123", "passed", 0, None),
            )
            .expect("windows"),
        ];

        let err = compare_reports(&reports).expect_err("should fail");
        assert!(err.to_string().contains("phase2_uapi_default_stdout_grant"));
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
            let (step_rows, metric_rows) = parse_tables(&text)?;

            Ok(Self {
                label,
                source: PathBuf::from(format!("{label}.md")),
                git_commit,
                host_line,
                host_os,
                step_rows,
                metric_rows,
            })
        }
    }
}
