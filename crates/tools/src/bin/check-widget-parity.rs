//! Generate the widget parity table from the code that actually renders.
//!
//! Krate's promise is that one file behaves the same on macOS, Windows, and
//! Linux. Widgets are where that promise is easiest to break quietly: a widget
//! declared in WIT but missing from one host's lowering means an app built on
//! one machine fails on another, and nothing in the build catches it.
//!
//! This reads three sources and compares them:
//!
//! - `wit/krate/phase3/deps/ui/ui.wit` — what apps are allowed to ask for;
//! - `crates/adapter-macos/src/appkit.rs` — what macOS lowers to real controls;
//! - `crates/adapter-common/src/painter.rs` — what the drawn path used by
//!   Linux and Windows paints.
//!
//! Run it with `--check` in CI to fail when the committed table drifts from the
//! code, and with `--write` to regenerate that table. The point is that the
//! published parity claim is derived from the implementation, never typed by
//! hand, so it cannot flatter us.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const TABLE_PATH: &str = "docs/book/src/reference/widget-parity.md";

fn main() -> ExitCode {
    let root = match repo_root() {
        Some(root) => root,
        None => {
            eprintln!("error: could not locate the repository root");
            return ExitCode::FAILURE;
        }
    };

    let report = match Report::collect(&root) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let rendered = report.render();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "--check".into());

    match mode.as_str() {
        "--write" => {
            let path = root.join(TABLE_PATH);
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(err) = fs::write(&path, &rendered) {
                eprintln!("error: could not write {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        "--check" => {
            let path = root.join(TABLE_PATH);
            let current = fs::read_to_string(&path).unwrap_or_default();
            if current == rendered {
                println!(
                    "widget parity table is current ({} of {} widgets work on all three systems)",
                    report.everywhere().len(),
                    report.declared.len()
                );
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "error: {TABLE_PATH} is out of date; run `cargo run -p krate-tools \
                     --bin check-widget-parity -- --write`"
                );
                ExitCode::FAILURE
            }
        }
        "--print" => {
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("error: unknown argument {other:?} (use --check, --write, or --print)");
            ExitCode::FAILURE
        }
    }
}

fn repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("wit").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

struct Report {
    declared: BTreeSet<String>,
    macos: BTreeSet<String>,
    drawn: BTreeSet<String>,
}

impl Report {
    fn collect(root: &Path) -> Result<Self, String> {
        let wit = read(root, "wit/krate/phase3/deps/ui/ui.wit")?;
        let appkit = read(root, "crates/adapter-macos/src/appkit.rs")?;
        let painter = read(root, "crates/adapter-common/src/painter.rs")?;

        let declared = declared_widgets(&wit);
        if declared.is_empty() {
            return Err("no widget-kind enum found in the UI WIT contract".to_string());
        }

        Ok(Self {
            macos: handled_widgets(&appkit, &declared),
            drawn: handled_widgets(&painter, &declared),
            declared,
        })
    }

    /// Widgets every host can render. This is the only number an app author can
    /// safely build against.
    fn everywhere(&self) -> BTreeSet<String> {
        self.declared
            .iter()
            .filter(|w| self.macos.contains(*w) && self.drawn.contains(*w))
            .cloned()
            .collect()
    }

    fn render(&self) -> String {
        let everywhere = self.everywhere();
        let mut out = String::new();

        out.push_str("# Widget parity\n\n");
        out.push_str(
            "Generated from the code that renders, not written by hand. Run\n\
             `cargo run -p krate-tools --bin check-widget-parity -- --write` to refresh it.\n\n",
        );
        out.push_str(&format!(
            "**{} of {} declared widgets work on all three systems.**\n\n",
            everywhere.len(),
            self.declared.len()
        ));
        out.push_str(
            "A widget that renders on one system and not another is the failure this\n\
             table exists to make visible: an app built on the machine that supports it\n\
             will not work when it is shared with someone on another.\n\n",
        );

        out.push_str("| Widget | macOS | Windows | Linux | Everywhere |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for widget in &self.declared {
            let mac = self.macos.contains(widget);
            // Windows and Linux share the drawn path, so they never disagree.
            let drawn = self.drawn.contains(widget);
            let mark = |ok: bool| if ok { "yes" } else { "no" };
            out.push_str(&format!(
                "| `{widget}` | {} | {} | {} | {} |\n",
                mark(mac),
                mark(drawn),
                mark(drawn),
                if mac && drawn { "**yes**" } else { "no" }
            ));
        }

        let mac_only: Vec<_> = self.macos.difference(&self.drawn).cloned().collect();
        let drawn_only: Vec<_> = self.drawn.difference(&self.macos).cloned().collect();
        let nowhere: Vec<_> = self
            .declared
            .iter()
            .filter(|w| !self.macos.contains(*w) && !self.drawn.contains(*w))
            .cloned()
            .collect();

        out.push_str("\n## Gaps\n\n");
        out.push_str(&format!("- macOS only: {}\n", list_or_none(&mac_only)));
        out.push_str(&format!(
            "- Windows and Linux only: {}\n",
            list_or_none(&drawn_only)
        ));
        out.push_str(&format!(
            "- Not implemented anywhere: {}\n",
            list_or_none(&nowhere)
        ));

        out
    }
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items
            .iter()
            .map(|i| format!("`{i}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    fs::read_to_string(root.join(rel)).map_err(|err| format!("could not read {rel}: {err}"))
}

/// Pull the `widget-kind` variants out of the WIT enum.
fn declared_widgets(wit: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in wit.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("enum widget-kind") {
            inside = true;
            continue;
        }
        if inside {
            if trimmed.starts_with('}') {
                break;
            }
            let name = trimmed.trim_end_matches(',').trim();
            if name.is_empty() || name.starts_with("//") {
                continue;
            }
            if name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                out.insert(name.to_string());
            }
        }
    }
    out
}

/// Which declared widgets a host source file names.
///
/// Matching is on `WidgetKind::Variant` in the adapter source. A host that
/// mentions a widget only to reject it would be counted as supporting it, so
/// the catch-all arms are deliberately not written that way -- they use `_` or
/// `other`, which this never matches.
fn handled_widgets(source: &str, declared: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for widget in declared {
        if source.contains(&format!("WidgetKind::{}", to_pascal(widget))) {
            out.insert(widget.clone());
        }
    }
    out
}

/// `text-field` -> `TextField`, matching the generated Rust binding names.
fn to_pascal(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_widget_kinds_out_of_wit() {
        let wit = "\
interface types {
  enum widget-kind {
    stack,
    button,
    text-field,
  }
}
";
        let declared = declared_widgets(wit);
        assert_eq!(declared.len(), 3);
        assert!(declared.contains("text-field"));
    }

    #[test]
    fn a_host_counts_only_the_widgets_it_names() {
        let declared: BTreeSet<String> = ["button", "canvas", "text-field"]
            .into_iter()
            .map(String::from)
            .collect();
        let source = "match kind { WidgetKind::Button => (), WidgetKind::TextField => (), \
                      other => unsupported(other) }";
        let handled = handled_widgets(source, &declared);
        assert!(handled.contains("button"));
        assert!(handled.contains("text-field"));
        // The catch-all arm must not be read as support for everything else.
        assert!(!handled.contains("canvas"));
    }

    #[test]
    fn kebab_names_become_the_rust_variant_spelling() {
        assert_eq!(to_pascal("text-field"), "TextField");
        assert_eq!(to_pascal("stack"), "Stack");
        assert_eq!(to_pascal("list-view"), "ListView");
    }
}
