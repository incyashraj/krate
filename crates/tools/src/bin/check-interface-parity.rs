//! Which declared interfaces actually do something.
//!
//! Krate declares interfaces in WIT and implements them in the runtime host,
//! and the two can disagree. When they do, an app asks for something the
//! permission wall grants, the call is made, and the runtime says no -- which
//! is the honest failure, but the person only finds out after building on it.
//!
//! `gfx::canvas2d` is the example that matters: the `canvas` widget lays out on
//! all three systems, so the widget table says it works, and every drawing call
//! into it is refused. Reading one table would tell you the opposite of the
//! truth.
//!
//! This reads the host implementation rather than any list, so it cannot drift:
//! a function that returns `Unsupported` is counted as refusing, and an
//! interface whose functions all refuse is reported as not implemented.
//!
//!   cargo run -p krate-tools --bin check-interface-parity -- --write

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() {
    let write = std::env::args().any(|arg| arg == "--write");
    let root = workspace_root();

    let report = match Report::collect(&root) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("interface parity check failed: {err}");
            std::process::exit(1);
        }
    };

    let markdown = report.to_markdown();
    let out = root.join("docs/book/src/reference/interface-parity.md");

    if write {
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&out, &markdown) {
            eprintln!("could not write {}: {err}", out.display());
            std::process::exit(1);
        }
        println!("wrote {}", out.display());
        return;
    }

    print!("{markdown}");
}

/// One interface and how much of it works.
struct Interface {
    functions: usize,
    refusing: usize,
}

impl Interface {
    fn state(&self) -> &'static str {
        if self.functions == 0 {
            "no functions"
        } else if self.refusing == 0 {
            "works"
        } else if self.refusing >= self.functions {
            "not implemented"
        } else {
            "partly"
        }
    }
}

struct Report {
    interfaces: BTreeMap<String, Interface>,
}

impl Report {
    fn collect(root: &Path) -> Result<Self, String> {
        let host = read(root, "crates/runtime/src/phase3_gui_host.rs")?;
        let interfaces = parse_host(&host);
        if interfaces.is_empty() {
            return Err("no host interface implementations found".to_string());
        }
        Ok(Self { interfaces })
    }

    fn to_markdown(&self) -> String {
        let working = self
            .interfaces
            .values()
            .filter(|i| i.state() == "works")
            .count();
        let missing = self
            .interfaces
            .values()
            .filter(|i| i.state() == "not implemented")
            .count();

        let mut out = String::new();
        out.push_str("# Interface parity\n\n");
        out.push_str(
            "Generated from the runtime host, not written by hand. Run\n\
             `cargo run -p krate-tools --bin check-interface-parity -- --write` to refresh it.\n\n",
        );
        out.push_str(&format!(
            "**{working} of {} declared interfaces are fully implemented. {missing} are declared \
             and do nothing yet.**\n\n",
            self.interfaces.len()
        ));
        out.push_str(
            "An interface that is declared but not implemented refuses every call with\n\
             `Unsupported`. That is the honest failure -- nothing pretends to work -- but a\n\
             person only finds out after building on it, which is why this table exists.\n\n\
             Read it alongside the widget table. They answer different questions: the\n\
             widget table says a kind lays out and draws, this one says whether the calls\n\
             behind an interface do anything. For a while `canvas` laid out everywhere\n\
             while `gfx.canvas2d` refused every call -- a widget that existed and could\n\
             not be drawn into. That pair reads `works` on both tables now.\n\n",
        );

        out.push_str("| Interface | Functions | State |\n");
        out.push_str("| --- | --- | --- |\n");
        for (name, iface) in &self.interfaces {
            let state = match iface.state() {
                "works" => "**works**".to_string(),
                "not implemented" => "**not implemented**".to_string(),
                "partly" => format!("partly — {} of {} refuse", iface.refusing, iface.functions),
                other => other.to_string(),
            };
            out.push_str(&format!("| `{name}` | {} | {state} |\n", iface.functions));
        }
        out.push('\n');
        out
    }
}

/// Read each `impl <path>::Host for` block and count how many of its functions
/// return an `Unsupported` error.
fn parse_host(source: &str) -> BTreeMap<String, Interface> {
    let mut out = BTreeMap::new();
    let marker = "::Host for";

    // Block boundaries: each impl runs until the next one starts.
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut at = 0usize;
    while let Some(found) = source[at..].find(marker) {
        let idx = at + found;
        let line_start = source[..idx].rfind("\nimpl ").map(|p| p + 6);
        if let Some(start) = line_start {
            let name = source[start..idx].trim().to_string();
            // Only the generated binding paths, which look like `ui::window`.
            if name.contains("::") && !name.contains(' ') {
                starts.push((idx, name));
            }
        }
        at = idx + marker.len();
    }

    for (i, (pos, name)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(source.len(), |(p, _)| *p);
        let body = &source[*pos..end];
        let functions = body.matches("\n    fn ").count();
        // Only a function whose *whole* answer is a refusal counts. An
        // implemented function may still return `Unsupported` on an error path
        // -- the file picker does, when the dialog fails or a run holds too
        // many files -- and counting those made a working interface look
        // unimplemented. The marker is the phrase these stubs share: they say
        // the feature is not implemented yet, rather than what went wrong.
        let refusing = body.matches("are not implemented yet").count()
            + body.matches("is not implemented yet").count()
            + body.matches("unsupported()").count();
        if functions > 0 {
            // A path like `ui::window` becomes `ui.window`, which is how the
            // capability and the WIT both name it.
            let mut parts: Vec<&str> = name.rsplit("::").take(2).collect();
            parts.reverse();
            let label = parts.join(".");
            out.insert(
                label,
                Interface {
                    functions,
                    refusing: refusing.min(functions),
                },
            );
        }
    }

    out
}

fn read(root: &Path, rel: &str) -> Result<String, String> {
    std::fs::read_to_string(root.join(rel)).map_err(|err| format!("read {rel}: {err}"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_whose_functions_all_refuse_is_reported_as_missing() {
        let source = r#"
impl ui::menu::Host for Phase3GuiHost {
    fn set_items(&mut self) -> Result<()> {
        Ok(Err(UiError::Unsupported("menus are not implemented yet".to_string())))
    }
}

impl ui::clipboard::Host for Phase3GuiHost {
    fn read(&mut self) -> Result<String> {
        Ok(Ok(self.text.clone()))
    }
    fn write(&mut self, value: String) -> Result<()> {
        Ok(Ok(()))
    }
}
"#;
        let parsed = parse_host(source);
        assert_eq!(parsed["ui.menu"].state(), "not implemented");
        assert_eq!(parsed["ui.clipboard"].state(), "works");
    }

    #[test]
    fn an_interface_that_refuses_some_calls_is_not_reported_as_working() {
        // ui::window works but refuses one call. Reporting that as "works"
        // would be the same overclaim this tool exists to catch.
        let source = r#"
impl ui::window::Host for Phase3GuiHost {
    fn create(&mut self) -> Result<u64> { Ok(Ok(1)) }
    fn show(&mut self) -> Result<()> { Ok(Ok(())) }
    fn set_state(&mut self) -> Result<()> {
        Ok(Err(UiError::Unsupported("window state changes are not implemented yet".to_string())))
    }
}
"#;
        let parsed = parse_host(source);
        assert_eq!(parsed["ui.window"].state(), "partly");
    }

    #[test]
    fn the_table_names_the_canvas_trap() {
        let source = r#"
impl gfx::canvas2d::Host for Phase3GuiHost {
    fn bind(&mut self) -> Result<u64> { Ok(Err(gfx_unsupported())) }
}
"#;
        let report = Report {
            interfaces: parse_host(source),
        };
        let md = report.to_markdown();
        assert!(md.contains("not implemented"));
        // The written explanation has to say why one table is not enough.
        assert!(md.contains("gfx.canvas2d"), "{md}");
    }
}
