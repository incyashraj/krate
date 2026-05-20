use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const HOST_ADAPTER_CRATES: &[&str] = &[
    "crates/adapter-linux/src/lib.rs",
    "crates/adapter-macos/src/lib.rs",
    "crates/adapter-windows/src/lib.rs",
];

const PHASE3_UI_ADAPTERS: &[HostUiAdapter] = &[
    HostUiAdapter::new(
        "crates/adapter-linux/src/lib.rs",
        "LinuxUiAdapter",
        "linux-headless-draft",
    ),
    HostUiAdapter::new(
        "crates/adapter-macos/src/lib.rs",
        "MacosUiAdapter",
        "macos-headless-draft",
    ),
    HostUiAdapter::new(
        "crates/adapter-windows/src/lib.rs",
        "WindowsUiAdapter",
        "windows-headless-draft",
    ),
];

const RUNTIME_BOUNDARY_CALLS: &[BoundaryCall] = &[
    BoundaryCall::new(
        "discover_host_locale",
        "discover_locale",
        BoundaryArea::Locale,
    ),
    BoundaryCall::new("discover_host_clock", "discover_clock", BoundaryArea::Time),
    BoundaryCall::new("sleep_on_host", "sleep_millis", BoundaryArea::Time),
    BoundaryCall::new(
        "host_current_locale",
        "current_locale",
        BoundaryArea::Locale,
    ),
    BoundaryCall::new(
        "timezone_from_host_locale",
        "timezone",
        BoundaryArea::Locale,
    ),
    BoundaryCall::new("format_date_on_host", "format_date", BoundaryArea::Locale),
    BoundaryCall::new(
        "format_number_on_host",
        "format_number",
        BoundaryArea::Locale,
    ),
    BoundaryCall::new(
        "apply_no_follow_final_symlink_on_host",
        "apply_no_follow_final_symlink",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new("stat_path_on_host", "stat_path", BoundaryArea::Filesystem),
    BoundaryCall::new("read_dir_on_host", "read_dir", BoundaryArea::Filesystem),
    BoundaryCall::new(
        "remove_file_on_host",
        "remove_file",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new("remove_dir_on_host", "remove_dir", BoundaryArea::Filesystem),
    BoundaryCall::new("create_dir_on_host", "create_dir", BoundaryArea::Filesystem),
    BoundaryCall::new(
        "rename_path_on_host",
        "rename_path",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new(
        "symlink_metadata_on_host",
        "symlink_metadata",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new("read_stdin_on_host", "read_stdin", BoundaryArea::Io),
    BoundaryCall::new("write_stderr_on_host", "write_stderr", BoundaryArea::Io),
    BoundaryCall::new("flush_stderr_on_host", "flush_stderr", BoundaryArea::Io),
    BoundaryCall::new(
        "print_stdout_line_on_host",
        "print_stdout_line",
        BoundaryArea::Io,
    ),
    BoundaryCall::new("write_stdout_on_host", "write_stdout", BoundaryArea::Io),
    BoundaryCall::new("flush_stdout_on_host", "flush_stdout", BoundaryArea::Io),
    BoundaryCall::new("read_path_on_host", "read_path", BoundaryArea::Filesystem),
    BoundaryCall::new(
        "create_dir_all_on_host",
        "create_dir_all",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new("read_file_on_host", "read_file", BoundaryArea::Filesystem),
    BoundaryCall::new("write_file_on_host", "write_file", BoundaryArea::Filesystem),
    BoundaryCall::new("seek_file_on_host", "seek_file", BoundaryArea::Filesystem),
    BoundaryCall::new(
        "file_metadata_on_host",
        "file_metadata",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new(
        "canonicalize_path_on_host",
        "canonicalize_path",
        BoundaryArea::Filesystem,
    ),
    BoundaryCall::new("open_path_on_host", "open_path", BoundaryArea::Filesystem),
    BoundaryCall::new(
        "resolve_socket_addrs_on_host",
        "resolve_socket_addrs",
        BoundaryArea::Network,
    ),
    BoundaryCall::new("connect_tcp_on_host", "connect_tcp", BoundaryArea::Network),
    BoundaryCall::new(
        "apply_tcp_timeouts_on_host",
        "apply_tcp_timeouts",
        BoundaryArea::Network,
    ),
    BoundaryCall::new(
        "write_all_tcp_on_host",
        "write_all_tcp",
        BoundaryArea::Network,
    ),
    BoundaryCall::new("read_tcp_on_host", "read_tcp", BoundaryArea::Network),
];

fn main() -> Result<()> {
    let report = check_adapter_boundary()?;

    println!("Layer36 adapter boundary check passed");
    println!("- runtime wrappers: {}", report.runtime_wrappers);
    println!("- adapter crates: {}", report.adapter_crates);
    println!("- filesystem wrappers: {}", report.filesystem_wrappers);
    println!("- network wrappers: {}", report.network_wrappers);
    println!("- io wrappers: {}", report.io_wrappers);
    println!("- time wrappers: {}", report.time_wrappers);
    println!("- locale wrappers: {}", report.locale_wrappers);
    println!("- phase 3 UI adapter crates: {}", report.ui_adapter_crates);

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct BoundaryCall {
    wrapper_fn: &'static str,
    adapter_fn: &'static str,
    area: BoundaryArea,
}

#[derive(Debug, Clone, Copy)]
struct HostUiAdapter {
    crate_path: &'static str,
    adapter_type: &'static str,
    backend_name: &'static str,
}

impl HostUiAdapter {
    const fn new(
        crate_path: &'static str,
        adapter_type: &'static str,
        backend_name: &'static str,
    ) -> Self {
        Self {
            crate_path,
            adapter_type,
            backend_name,
        }
    }
}

impl BoundaryCall {
    const fn new(wrapper_fn: &'static str, adapter_fn: &'static str, area: BoundaryArea) -> Self {
        Self {
            wrapper_fn,
            adapter_fn,
            area,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryArea {
    Filesystem,
    Network,
    Io,
    Time,
    Locale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundaryReport {
    runtime_wrappers: usize,
    adapter_crates: usize,
    filesystem_wrappers: usize,
    network_wrappers: usize,
    io_wrappers: usize,
    time_wrappers: usize,
    locale_wrappers: usize,
    ui_adapter_crates: usize,
}

fn check_adapter_boundary() -> Result<BoundaryReport> {
    let root = workspace_root();
    let runtime_path = root.join("crates/runtime/src/lib.rs");
    let runtime = fs::read_to_string(&runtime_path)
        .with_context(|| format!("read {}", runtime_path.display()))?;

    for call in RUNTIME_BOUNDARY_CALLS {
        let body = function_body(&runtime, call.wrapper_fn)
            .with_context(|| format!("find runtime wrapper `{}`", call.wrapper_fn))?;
        let expected_call = format!("host_os_adapter::{}", call.adapter_fn);
        ensure(
            body.contains(&expected_call),
            format!(
                "runtime wrapper `{}` must route through `{expected_call}` on supported hosts",
                call.wrapper_fn
            ),
        )?;
    }

    for crate_path in HOST_ADAPTER_CRATES {
        let path = root.join(crate_path);
        let source =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        for call in RUNTIME_BOUNDARY_CALLS {
            let signature = format!("pub fn {}", call.adapter_fn);
            ensure(
                source.contains(&signature),
                format!(
                    "{crate_path} must expose adapter function `{}`",
                    call.adapter_fn
                ),
            )?;
        }
    }

    for adapter in PHASE3_UI_ADAPTERS {
        let path = root.join(adapter.crate_path);
        let source =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        ensure(
            source.contains(&format!("pub struct {}", adapter.adapter_type)),
            format!(
                "{} must expose Phase 3 UI adapter `{}`",
                adapter.crate_path, adapter.adapter_type
            ),
        )?;
        ensure(
            source.contains(&format!("impl UiAdapter for {}", adapter.adapter_type)),
            format!(
                "{} must implement UiAdapter for `{}`",
                adapter.crate_path, adapter.adapter_type
            ),
        )?;
        ensure(
            source.contains("fn info(&self) -> UiAdapterInfo"),
            format!("{} must report Phase 3 UI adapter info", adapter.crate_path),
        )?;
        ensure(
            source.contains("pub fn discover_ui_adapter()"),
            format!(
                "{} must expose `discover_ui_adapter` for Phase 3 UI startup",
                adapter.crate_path
            ),
        )?;
        ensure(
            source.contains(adapter.backend_name),
            format!(
                "{} must name current backend `{}`",
                adapter.crate_path, adapter.backend_name
            ),
        )?;
    }

    Ok(BoundaryReport {
        runtime_wrappers: RUNTIME_BOUNDARY_CALLS.len(),
        adapter_crates: HOST_ADAPTER_CRATES.len(),
        filesystem_wrappers: count_area(BoundaryArea::Filesystem),
        network_wrappers: count_area(BoundaryArea::Network),
        io_wrappers: count_area(BoundaryArea::Io),
        time_wrappers: count_area(BoundaryArea::Time),
        locale_wrappers: count_area(BoundaryArea::Locale),
        ui_adapter_crates: PHASE3_UI_ADAPTERS.len(),
    })
}

fn function_body<'a>(source: &'a str, name: &str) -> Result<&'a str> {
    let needle = format!("fn {name}");
    let start = source
        .find(&needle)
        .ok_or_else(|| anyhow::anyhow!("missing function `{name}`"))?;
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .ok_or_else(|| anyhow::anyhow!("missing body for function `{name}`"))?;

    let mut depth = 0usize;
    for (offset, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = body_start + offset + ch.len_utf8();
                    return Ok(&source[body_start..end]);
                }
            }
            _ => {}
        }
    }

    bail!("unterminated body for function `{name}`")
}

fn count_area(area: BoundaryArea) -> usize {
    RUNTIME_BOUNDARY_CALLS
        .iter()
        .filter(|call| call.area == area)
        .count()
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
    fn adapter_boundary_check_passes() {
        check_adapter_boundary().expect("adapter boundary check");
    }

    #[test]
    fn function_body_extracts_exact_wrapper() {
        let source = r#"
            fn first() { direct(); }
            fn second(value: u32) -> u32 {
                if value > 0 { return value; }
                0
            }
        "#;

        let body = function_body(source, "second").expect("second body");

        assert!(body.contains("return value"));
        assert!(!body.contains("direct()"));
    }
}
