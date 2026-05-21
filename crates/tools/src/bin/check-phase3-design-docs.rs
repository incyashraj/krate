use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

fn main() -> Result<()> {
    let root = workspace_root();

    check_doc(
        &root.join("docs/adr/0013-widget-lowering-strategy.md"),
        &[
            "ADR-0013: Widget Lowering Strategy",
            "Status:** Proposed",
            "Answer D",
            "native three of five",
            "custom drawn fallback",
            "semantic match",
        ],
    )?;
    check_doc(
        &root.join("docs/rfc/0003-widget-protocol.md"),
        &[
            "RFC-0003: Layer36 Phase 3 Widget Protocol",
            "WidgetId",
            "First Widget Set",
            "Native Three Of Five Rule",
            "Event Flow",
            "Accessibility",
        ],
    )?;
    check_doc(
        &root.join("docs/book/src/phase3/widget-protocol.md"),
        &[
            "Widget Protocol",
            "Native widget",
            "Drawn fallback",
            "Current Status",
            "routed pointer event",
            "real native window backend",
            "layer36-notes",
        ],
    )?;
    check_doc(
        &root.join("docs/adr/0014-layout-engine-taffy.md"),
        &[
            "ADR-0014: Layout Engine Uses Taffy",
            "Status:** Proposed",
            "layer36-layout",
            "Taffy",
            "stable widget IDs",
        ],
    )?;
    check_doc(
        &root.join("docs/book/src/phase3/layout.md"),
        &[
            "Layout",
            "crates/layout/",
            "Taffy",
            "LayoutSnapshot",
            "PreparedLayoutTree",
            "hit-test",
            "10,000-node",
            "accessibility bounds",
        ],
    )?;

    println!("Phase 3 design docs check passed");
    Ok(())
}

fn check_doc(path: &PathBuf, required: &[&str]) -> Result<()> {
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;

    for marker in required {
        if !source.contains(marker) {
            bail!("{} is missing marker `{marker}`", path.display());
        }
    }

    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("tools crate lives under crates/tools")
        .to_path_buf()
}
