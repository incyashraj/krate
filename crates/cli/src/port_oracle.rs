//! Recorded cases a port must reproduce (IC-120).
//!
//! A port that builds, imports only `krate:*`, runs and keeps its permission
//! wall can still be the wrong program: E5 ported an invoice app that printed
//! `total=100` into a word counter, and every mechanical check was green. The
//! only thing that tells those apart is running the port on the original's
//! input and comparing what comes out. Chapter 49 orders the oracle kinds and
//! recorded I/O comes first, because it is exact.
//!
//! The cases come from the person (`krate port --oracle cases.json`): the
//! inputs they gave the original and exactly what it printed. Krate does not
//! run the original here -- observing it is its own consented step (IC-126).
//!
//! This file holds the parts that need no process: reading and checking the
//! case file, what the agent is told, and how a mismatch is described. The
//! run itself is in `main.rs`, beside the permission wall it shares grants
//! with.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The schema name a case file must declare.
pub const ORACLE_SCHEMA: &str = "krate.port.oracle.v1";

/// The journey a case belongs to when it names none.
pub const PRIMARY_TASK: &str = "primary-task";

/// A whole case file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortOracle {
    pub schema: String,
    pub cases: Vec<OracleCase>,
}

/// One recorded case: an input and exactly what the original did with it.
///
/// Unknown fields are refused, so `expected_stdout` for `stdout` is an error
/// the person sees before the port, not a case that silently checks nothing.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OracleCase {
    pub id: String,
    /// Which journey this case proves.
    #[serde(default = "primary_task")]
    pub journey: String,
    /// The arguments the app is run with.
    #[serde(default)]
    pub args: Vec<String>,
    /// Files written into the run's directory first, by relative path.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// Exactly what the original printed. Line endings are compared as `\n`.
    pub stdout: String,
    /// The original's exit code.
    #[serde(default)]
    pub exit: i32,
}

fn primary_task() -> String {
    PRIMARY_TASK.to_string()
}

/// Read and check a case file. Every refusal names the file and the fix.
pub fn load(path: &Path) -> Result<PortOracle> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read the recorded cases at {}", path.display()))?;
    let oracle: PortOracle = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not a {ORACLE_SCHEMA} case file. Each case needs an `id` and the \
             exact `stdout`; `args`, `files`, `exit` and `journey` are optional",
            path.display()
        )
    })?;
    validate(&oracle).with_context(|| format!("check the recorded cases in {}", path.display()))?;
    Ok(oracle)
}

/// The rules a case file must meet, apart from its shape.
pub fn validate(oracle: &PortOracle) -> Result<()> {
    if oracle.schema != ORACLE_SCHEMA {
        anyhow::bail!(
            "the schema is `{}`; this Krate reads `{ORACLE_SCHEMA}`",
            oracle.schema
        );
    }
    if oracle.cases.is_empty() {
        // An empty list would let a port pass on nothing at all.
        anyhow::bail!("there are no cases; a port is judged on at least one");
    }
    let mut seen = BTreeSet::new();
    for case in &oracle.cases {
        if case.id.trim().is_empty() {
            anyhow::bail!("a case has an empty `id`");
        }
        if !seen.insert(case.id.as_str()) {
            anyhow::bail!("two cases are called `{}`; each id must be unique", case.id);
        }
        if case.journey.trim().is_empty() {
            anyhow::bail!("case `{}` has an empty `journey`", case.id);
        }
        for name in case.files.keys() {
            if !is_inside_the_run(name) {
                anyhow::bail!(
                    "case `{}` writes `{name}`, which is not a relative path inside the \
                     run's own folder",
                    case.id
                );
            }
        }
    }
    Ok(())
}

/// A relative path with no `..`, root or drive: a case may only place files
/// inside the folder it runs in.
pub fn is_inside_the_run(name: &str) -> bool {
    let path = Path::new(name);
    !name.is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        && path
            .components()
            .any(|part| matches!(part, Component::Normal(_)))
}

/// What the agent's task says about the cases.
pub fn task_section(oracle: &PortOracle) -> String {
    let mut text = String::from(
        "\n## Recorded cases\n\n\
`oracle.json` holds cases recorded from the original. After packaging, Krate\n\
runs the port on each case's input, with its declared access, and compares\n\
stdout and the exit code exactly. A port that differs on any case is not\n\
finished, and a repair attempt is handed the difference. Do not edit\n\
`oracle.json`: it is part of what the port is judged on.\n\n",
    );
    for case in &oracle.cases {
        text.push_str(&format!(
            "- `{}` ({}): run with `{}`, expect exit {} and stdout `{}`\n",
            case.id,
            case.journey,
            case.args.join(" "),
            case.exit,
            case.stdout.escape_debug()
        ));
    }
    text
}

/// Mark each journey the cases cover as checked by recorded I/O, and add any
/// journey the cases name that the template set does not have.
pub fn mark_journeys(journeys: &mut serde_json::Value, oracle: &PortOracle) {
    let Some(list) = journeys["journeys"].as_array_mut() else {
        return;
    };
    let mut covered: Vec<&str> = Vec::new();
    for case in &oracle.cases {
        if !covered.contains(&case.journey.as_str()) {
            covered.push(&case.journey);
        }
    }
    for id in covered {
        let cases: Vec<&str> = oracle
            .cases
            .iter()
            .filter(|case| case.journey == id)
            .map(|case| case.id.as_str())
            .collect();
        if let Some(journey) = list.iter_mut().find(|j| j["id"] == id) {
            journey["kind"] = serde_json::json!("recorded-io");
            journey["oracle"] = serde_json::json!("oracle.json");
            journey["cases"] = serde_json::json!(cases);
        } else {
            list.push(serde_json::json!({
                "id": id,
                "title": id,
                "kind": "recorded-io",
                "oracle": "oracle.json",
                "cases": cases,
                "steps": ["Run the port on each recorded input"],
                "expected": "The port prints exactly what the original printed and exits the same way",
                "source_evidence": []
            }));
        }
    }
}

/// What one case did on the port.
#[derive(Clone, Debug)]
pub struct CaseResult {
    pub case: OracleCase,
    pub stdout: String,
    pub stderr: String,
    pub exit: i32,
}

impl CaseResult {
    pub fn passed(&self) -> bool {
        self.exit == self.case.exit && same_text(&self.stdout, &self.case.stdout)
    }

    /// One line: what the port did where the original did something else.
    pub fn summary(&self) -> String {
        if !same_text(&self.stdout, &self.case.stdout) {
            let (got, want) = first_difference(&self.stdout, &self.case.stdout);
            format!(
                "case `{}` printed {} where the original printed {}",
                self.case.id,
                quoted(&got),
                quoted(&want)
            )
        } else {
            format!(
                "case `{}` exited {} where the original exited {}",
                self.case.id, self.exit, self.case.exit
            )
        }
    }

    pub fn evidence(&self) -> serde_json::Value {
        serde_json::json!({
            "case": self.case.id,
            "status": if self.passed() { "passed" } else { "failed" },
            "args": self.case.args,
            "expected_exit": self.case.exit,
            "exit": self.exit,
            "expected_stdout": self.case.stdout,
            "stdout": self.stdout,
        })
    }
}

/// Every case's result, or why none could be run here.
#[derive(Clone, Debug, Default)]
pub struct BehaviorCheck {
    pub results: Vec<CaseResult>,
    /// Required access an automated run may not grant (a camera, the
    /// network). The cases cannot run without it, so they are not run --
    /// and the journey stays unchecked rather than failing on a refusal.
    pub not_run_because: Option<String>,
}

impl BehaviorCheck {
    pub fn failures(&self) -> Vec<&CaseResult> {
        self.results.iter().filter(|r| !r.passed()).collect()
    }

    /// The journeys these cases cover, in the order the cases name them.
    pub fn journeys(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for result in &self.results {
            if !out.contains(&result.case.journey) {
                out.push(result.case.journey.clone());
            }
        }
        out
    }

    /// The text a repair attempt is handed and the blocker report quotes.
    /// It says which case, what went in, what was expected and what came
    /// out -- the whole diagnosis, so the agent fixes the behaviour rather
    /// than guessing at it.
    pub fn report(&self) -> String {
        let failures = self.failures();
        let mut text = format!(
            "the port did not do what the original did: {} of {} recorded case(s) differ.\n",
            failures.len(),
            self.results.len()
        );
        for failure in failures {
            text.push_str(&format!(
                "\njourney `{}`, case `{}`\n  ran with: {}\n",
                failure.case.journey,
                failure.case.id,
                if failure.case.args.is_empty() {
                    "(no arguments)".to_string()
                } else {
                    failure.case.args.join(" ")
                }
            ));
            for (name, contents) in &failure.case.files {
                text.push_str(&format!("  file {name}: {} bytes\n", contents.len()));
            }
            text.push_str(&format!(
                "  expected exit {}, stdout:\n{}\n  got exit {}, stdout:\n{}\n",
                failure.case.exit,
                indent(&clip(&failure.case.stdout)),
                failure.exit,
                indent(&clip(&failure.stdout))
            ));
            if !failure.stderr.trim().is_empty() {
                text.push_str(&format!("  stderr:\n{}\n", indent(&clip(&failure.stderr))));
            }
        }
        text.push_str(
            "\nMake the port print exactly what the original printed for these inputs. \
             The cases are in oracle.json beside the candidate and may not be edited.",
        );
        text
    }
}

/// Stdout compared with Windows line endings read as `\n`, and nothing else
/// forgiven: a recorded case is exact or it is not a recording.
fn same_text(a: &str, b: &str) -> bool {
    a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
}

/// The first line where two outputs differ, as (got, wanted). A missing line
/// is shown as the end of the output.
fn first_difference(got: &str, want: &str) -> (String, String) {
    let got = got.replace("\r\n", "\n");
    let want = want.replace("\r\n", "\n");
    let mut g = got.split('\n');
    let mut w = want.split('\n');
    loop {
        match (g.next(), w.next()) {
            (Some(a), Some(b)) if a == b => continue,
            (a, b) => {
                return (
                    a.map(str::to_string)
                        .unwrap_or_else(|| "nothing more".to_string()),
                    b.map(str::to_string)
                        .unwrap_or_else(|| "nothing more".to_string()),
                )
            }
        }
    }
}

fn quoted(line: &str) -> String {
    if line == "nothing more" {
        return line.to_string();
    }
    let clipped: String = line.chars().take(120).collect();
    format!("\"{}\"", clipped.escape_debug())
}

fn clip(text: &str) -> String {
    const MOST: usize = 2000;
    if text.chars().count() <= MOST {
        return text.to_string();
    }
    let kept: String = text.chars().take(MOST).collect();
    format!("{kept}\n[... cut at {MOST} characters]")
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(id: &str, stdout: &str) -> OracleCase {
        OracleCase {
            id: id.to_string(),
            journey: PRIMARY_TASK.to_string(),
            args: vec!["input/qty.txt".to_string()],
            files: BTreeMap::from([("input/qty.txt".to_string(), "2\n3\n5\n".to_string())]),
            stdout: stdout.to_string(),
            exit: 0,
        }
    }

    fn oracle(cases: Vec<OracleCase>) -> PortOracle {
        PortOracle {
            schema: ORACLE_SCHEMA.to_string(),
            cases,
        }
    }

    #[test]
    fn a_case_file_with_nothing_to_check_is_refused() {
        assert!(validate(&oracle(Vec::new())).is_err());
    }

    #[test]
    fn a_misspelt_field_is_an_error_not_a_case_that_checks_nothing() {
        let text =
            r#"{"schema":"krate.port.oracle.v1","cases":[{"id":"a","expected_stdout":"x"}]}"#;
        assert!(serde_json::from_str::<PortOracle>(text).is_err());
    }

    #[test]
    fn a_case_may_only_write_inside_its_own_run() {
        for bad in ["../escape.txt", "/etc/passwd", "a/../../b", "", "."] {
            let mut c = case("a", "x");
            c.files = BTreeMap::from([(bad.to_string(), String::new())]);
            assert!(validate(&oracle(vec![c])).is_err(), "{bad:?} was accepted");
        }
        assert!(is_inside_the_run("input/qty.txt"));
        assert!(is_inside_the_run("./input/qty.txt"));
    }

    #[test]
    fn duplicate_ids_and_a_foreign_schema_are_refused() {
        assert!(validate(&oracle(vec![case("a", "x"), case("a", "y")])).is_err());
        let mut other = oracle(vec![case("a", "x")]);
        other.schema = "something.else".to_string();
        assert!(validate(&other).is_err());
    }

    #[test]
    fn a_mismatch_names_the_case_and_both_lines() {
        let result = CaseResult {
            case: case("total", "total=100\n"),
            stdout: "total=90\n".to_string(),
            stderr: String::new(),
            exit: 0,
        };
        assert!(!result.passed());
        let summary = result.summary();
        assert!(summary.contains("`total`"), "{summary}");
        assert!(summary.contains("\"total=90\""), "{summary}");
        assert!(summary.contains("\"total=100\""), "{summary}");
        let check = BehaviorCheck {
            results: vec![result],
            not_run_because: None,
        };
        let report = check.report();
        assert!(
            report.contains("did not do what the original did"),
            "{report}"
        );
        assert!(report.contains("input/qty.txt"), "{report}");
    }

    #[test]
    fn only_line_endings_are_forgiven() {
        let mut result = CaseResult {
            case: case("total", "total=100\n"),
            stdout: "total=100\r\n".to_string(),
            stderr: String::new(),
            exit: 0,
        };
        assert!(result.passed());
        result.stdout = "total=100 \n".to_string();
        assert!(!result.passed(), "a trailing space is a different output");
        result.stdout = "total=100\n".to_string();
        result.exit = 1;
        assert!(!result.passed());
        assert!(result.summary().contains("exited 1"));
    }

    #[test]
    fn a_missing_line_is_said_as_such() {
        let (got, want) = first_difference("a\n", "a\nb\n");
        assert_eq!(got, "");
        assert_eq!(want, "b");
        let (got, want) = first_difference("a", "a\nb");
        assert_eq!(got, "nothing more");
        assert_eq!(want, "b");
    }

    #[test]
    fn the_journeys_the_cases_cover_are_marked() {
        let mut journeys = serde_json::json!({"journeys": [
            {"id": "launch", "kind": "automated"},
            {"id": "primary-task", "kind": "manual"}
        ]});
        let mut extra = case("export", "ok\n");
        extra.journey = "export-csv".to_string();
        mark_journeys(&mut journeys, &oracle(vec![case("total", "x"), extra]));
        let list = journeys["journeys"].as_array().expect("list");
        let primary = list
            .iter()
            .find(|j| j["id"] == "primary-task")
            .expect("primary");
        assert_eq!(primary["kind"], "recorded-io");
        assert_eq!(primary["cases"][0], "total");
        assert!(list.iter().any(|j| j["id"] == "export-csv"));
        assert_eq!(
            list.iter().find(|j| j["id"] == "launch").expect("launch")["kind"],
            "automated"
        );
    }
}
