use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const RETRO_PATH: &str = "docs/book/src/phase2/retro.md";
const KICKOFF_PATH: &str = "docs/governance/phase-3-kickoff-issue.md";

const RETRO_REQUIRED: &[&str] = &[
    "# Phase 2 Retrospective Draft",
    "**Status:** Draft until Phase 2 exit review passes.",
    "## What Shipped",
    "## What Did Not Ship Yet",
    "## UAPI Lessons",
    "## Adapter Lessons",
    "## Binding Lessons",
    "## UCap Lessons",
    "## Performance Lessons",
    "## Documentation Lessons",
    "## Build Plan Changes To Carry Forward",
    "## Phase 3 Notes Before Start",
];

const KICKOFF_REQUIRED: &[&str] = &[
    "# Phase 3 Kickoff Issue Draft",
    "Draft only. Do not open this issue until Phase 2 exit review passes.",
    "## Objective",
    "## Why This Starts After Phase 2",
    "## Prerequisites",
    "## Initial Task Slice",
    "## Non-Goals",
    "## Exit Signal",
    "## References",
];

const FORBIDDEN_FINAL_CLAIMS: &[&str] = &[
    "Phase 2 is complete",
    "Phase 2 has exited",
    "Phase 2 exit review passed",
    "Open this issue now",
];

fn main() -> Result<()> {
    let root = workspace_root();
    check_doc(&root, RETRO_PATH, RETRO_REQUIRED)?;
    check_doc(&root, KICKOFF_PATH, KICKOFF_REQUIRED)?;

    println!("Layer36 Phase 2 closeout docs check passed");
    println!("- retro draft: {RETRO_PATH}");
    println!("- Phase 3 kickoff draft: {KICKOFF_PATH}");

    Ok(())
}

fn check_doc(root: &Path, relative_path: &str, required: &[&str]) -> Result<()> {
    let path = root.join(relative_path);
    let source = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

    for phrase in required {
        ensure(
            source.contains(phrase),
            format!("{relative_path} is missing `{phrase}`"),
        )?;
    }

    for phrase in FORBIDDEN_FINAL_CLAIMS {
        ensure(
            !source.contains(phrase),
            format!("{relative_path} claims final exit too early with `{phrase}`"),
        )?;
    }

    ensure(
        source.contains("Phase 2"),
        format!("{relative_path} should name Phase 2"),
    )?;
    ensure(
        source.contains("Phase 3"),
        format!("{relative_path} should name Phase 3"),
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
    fn current_closeout_docs_are_well_formed() {
        let root = workspace_root();
        check_doc(&root, RETRO_PATH, RETRO_REQUIRED).expect("retro draft");
        check_doc(&root, KICKOFF_PATH, KICKOFF_REQUIRED).expect("kickoff draft");
    }
}
