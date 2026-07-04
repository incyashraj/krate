use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const DECISION_PATH: &str = "docs/book/src/phase2/uapi-freeze-decision.md";

const REQUIRED_PHRASES: &[&str] = &[
    "# UAPI Freeze Decision Packet",
    "**Status:** Draft. UAPI v0.1 is not frozen yet.",
    "Decision: Not frozen yet.",
    "## Decision State",
    "## Scope",
    "## What Must Be True",
    "## No-Go Conditions",
    "## Reviewer Signoff",
    "## If Accepted Later",
    "scripts/check-uapi.sh",
    "scripts/check-uapi-freeze-lock.sh",
    "scripts/record-phase2-uapi-freeze-review.sh --strict",
    "scripts/record-phase2-exit-bundle.sh --strict",
    "krate:io@0.1.0",
    "krate:fs@0.1.0",
    "krate:net@0.1.0",
    "krate:time@0.1.0",
    "krate:locale@0.1.0",
];

const FORBIDDEN_PREMATURE_CLAIMS: &[&str] = &[
    "Decision: Frozen.",
    "Status: Accepted",
    "UAPI v0.1 is frozen.",
    "Phase 2 UAPI freeze accepted",
];

fn main() -> Result<()> {
    let root = workspace_root();
    check_freeze_decision(&root)?;

    println!("Krate Phase 2 freeze decision check passed");
    println!("- decision packet: {DECISION_PATH}");

    Ok(())
}

fn check_freeze_decision(root: &Path) -> Result<()> {
    let path = root.join(DECISION_PATH);
    let source = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

    for phrase in REQUIRED_PHRASES {
        ensure(
            source.contains(phrase),
            format!("{DECISION_PATH} is missing `{phrase}`"),
        )?;
    }

    for phrase in FORBIDDEN_PREMATURE_CLAIMS {
        ensure(
            !source.contains(phrase),
            format!("{DECISION_PATH} claims final freeze too early with `{phrase}`"),
        )?;
    }

    ensure(
        source.contains("| Reviewer | Pending |"),
        format!("{DECISION_PATH} should keep reviewer signoff pending"),
    )?;
    ensure(
        source.contains("| Freeze commit | Pending |"),
        format!("{DECISION_PATH} should keep freeze commit pending"),
    )?;
    ensure(
        source.contains("outside Rust walkthrough"),
        format!("{DECISION_PATH} should name the outside walkthrough dependency"),
    )?;

    Ok(())
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
    fn current_freeze_decision_packet_is_well_formed() {
        let root = workspace_root();
        check_freeze_decision(&root).expect("freeze decision packet");
    }
}
