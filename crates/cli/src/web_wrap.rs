//! `krate port <folder> --web` (IC-1004): a built web app -- a Vite, React
//! or plain HTML `dist/` folder -- becomes one signed .krate in one command.
//!
//! Nothing is compiled on the person's machine: the Krate side is the web
//! shell (apps/krate-web-shell), built once and carried inside this binary.
//! The folder goes into the bundle's assets as `web/`, a `krate-web.json`
//! says the window's title and size and where to start, and the manifest
//! asks for exactly what the shell needs -- a window and the app's own
//! store -- plus a `net.connect` for each host the page talks to, named with
//! `--allow`. The page can reach nothing else.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// The web shell component (apps/krate-web-shell, cargo component build
/// --release). Rebuilt and copied here when the shell changes; the test
/// below checks it still speaks krate:ui/app-view.
pub(crate) const WEB_SHELL: &[u8] = include_bytes!("../web-shell/krate_web_shell.wasm");

/// The most files and bytes a wrapped folder may carry.
const MAX_FILES: usize = 20_000;
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// What to wrap and how.
pub(crate) struct WebWrap {
    pub(crate) folder: PathBuf,
    pub(crate) to: PathBuf,
    pub(crate) name: Option<String>,
    pub(crate) allow: Vec<String>,
    pub(crate) size: Option<String>,
    pub(crate) start: Option<String>,
}

/// A name as an app id segment: lowercase letters, digits and hyphens.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "web-app".to_string()
    } else {
        out
    }
}

/// `host` or `host:port` as a net.connect grant (443 when no port).
pub(crate) fn grant(allow: &str) -> Result<String> {
    let allow = allow
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("wss://")
        .trim_end_matches('/');
    let (host, port) = allow.rsplit_once(':').unwrap_or((allow, "443"));
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '*'));
    let port_ok = port.parse::<u16>().is_ok_and(|p| p > 0);
    if !host_ok || !port_ok {
        bail!("`{allow}` is not a host the page can be allowed (use api.example.com or api.example.com:8443)");
    }
    Ok(format!("net.connect:{}:{port}", host.to_ascii_lowercase()))
}

/// `1100x760` as a window size.
pub(crate) fn window_size(size: Option<&str>) -> Result<(u32, u32)> {
    let Some(size) = size else {
        return Ok((1100, 760));
    };
    let (w, h) = size
        .split_once(['x', 'X'])
        .context("a size is WIDTHxHEIGHT, for example 1100x760")?;
    let (w, h): (u32, u32) = (w.trim().parse()?, h.trim().parse()?);
    if !(320..=4096).contains(&w) || !(320..=4096).contains(&h) {
        bail!("a window is 320 to 4096 points a side");
    }
    Ok((w, h))
}

/// The manifest the wrapped app carries.
pub(crate) fn manifest(name: &str, grants: &[String]) -> String {
    let quoted = toml_string(name);
    let mut text = format!(
        "[app]\nid = \"local.web.{}\"\nname = {quoted}\nversion = \"1.0.0\"\nentry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n\n\
         [[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"The window the web app fills\"\nrequired = true\n\n\
         [[capabilities]]\ncap = \"store.kv\"\nrationale = \"Keep what the web app saves\"\nrequired = true\n",
        slug(name)
    );
    for g in grants {
        let host = g.trim_start_matches("net.connect:");
        text.push_str(&format!(
            "\n[[capabilities]]\ncap = \"{g}\"\nrationale = {}\nrequired = true\n",
            toml_string(&format!("The web app talks to {host}"))
        ));
    }
    text
}

fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Copy the built folder, refusing links that leave it and anything past
/// the limits.
fn copy_folder(from: &Path, to: &Path, files: &mut usize, bytes: &mut u64) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let name = entry.file_name();
        if name == ".DS_Store" || name == ".git" {
            continue;
        }
        let target = to.join(&name);
        if kind.is_symlink() {
            // A link would carry whatever it points at; built folders have none.
            continue;
        }
        if kind.is_dir() {
            copy_folder(&entry.path(), &target, files, bytes)?;
        } else if kind.is_file() {
            *files += 1;
            *bytes += entry.metadata()?.len();
            if *files > MAX_FILES || *bytes > MAX_BYTES {
                bail!(
                    "the folder is larger than a wrapped web app may be ({MAX_FILES} files, {} MB): wrap the built folder (dist/), not the project",
                    MAX_BYTES / 1024 / 1024
                );
            }
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Wrap the folder; the bundle's path.
pub(crate) fn wrap(request: &WebWrap) -> Result<PathBuf> {
    let folder = &request.folder;
    let start = request.start.as_deref().unwrap_or("index.html");
    let start_rel = krate_adapter_common::appview::resolve_path(start)
        .map_err(|r| anyhow::anyhow!("--start {start}: {r:?}"))?;
    if !folder.join(&start_rel).is_file() {
        bail!(
            "{} has no {start_rel}: point at the BUILT folder (run `npm run build` and wrap dist/)",
            folder.display()
        );
    }
    if folder.join("node_modules").is_dir() || folder.join("package.json").is_file() {
        bail!(
            "{} looks like the project, not its build: run `npm run build` and wrap the folder it writes (usually dist/)",
            folder.display()
        );
    }
    let name = match &request.name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => folder
            .canonicalize()
            .ok()
            .and_then(|p| {
                let leaf = p.file_name()?.to_string_lossy().to_string();
                // dist/ and build/ are the build, not the app: use the parent.
                if matches!(leaf.as_str(), "dist" | "build" | "out" | "public") {
                    p.parent()?
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                } else {
                    Some(leaf)
                }
            })
            .unwrap_or_else(|| "Web App".to_string()),
    };
    let grants = request
        .allow
        .iter()
        .map(|a| grant(a))
        .collect::<Result<Vec<_>>>()?;
    let (width, height) = window_size(request.size.as_deref())?;

    let work = std::env::temp_dir().join(format!("krate-web-wrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let assets = work.join("assets");
    let (mut files, mut bytes) = (0usize, 0u64);
    copy_folder(folder, &assets.join("web"), &mut files, &mut bytes)?;
    let config = serde_json::json!({
        "title": name, "width": width, "height": height, "folder": "web", "start": start_rel,
    });
    std::fs::write(assets.join("krate-web.json"), config.to_string())?;
    let manifest_path = work.join("manifest.toml");
    std::fs::write(&manifest_path, manifest(&name, &grants))?;
    let wasm = work.join("code.wasm");
    std::fs::write(&wasm, WEB_SHELL)?;
    println!(
        "wrapping {} ({files} files, {:.1} MB) as \"{name}\"",
        folder.display(),
        bytes as f64 / 1e6
    );
    let code = crate::pack_bundle(&wasm, &manifest_path, &request.to, &[]);
    let _ = std::fs::remove_dir_all(&work);
    if code? != 0 {
        bail!("packing the wrapped app failed");
    }
    Ok(request.to.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hosts_and_sizes_are_checked() {
        assert_eq!(slug("My Todo App!"), "my-todo-app");
        assert_eq!(slug("***"), "web-app");
        assert_eq!(
            grant("api.example.com").unwrap(),
            "net.connect:api.example.com:443"
        );
        assert_eq!(
            grant("https://API.example.com:8443/").unwrap(),
            "net.connect:api.example.com:8443"
        );
        assert!(grant("evil.com; rm -rf").is_err());
        assert!(grant("example.com:99999").is_err());
        assert_eq!(window_size(Some("800x600")).unwrap(), (800, 600));
        assert!(window_size(Some("10x10")).is_err());
        assert_eq!(window_size(None).unwrap(), (1100, 760));
    }

    #[test]
    fn the_generated_manifest_asks_for_exactly_what_the_shell_needs() {
        let text = manifest(
            "Todo \"MVC\"",
            &["net.connect:api.example.com:443".to_string()],
        );
        let parsed = krate_manifest::Manifest::parse(&text).expect("a valid manifest");
        assert_eq!(parsed.app.id, "local.web.todo-mvc");
        assert_eq!(parsed.app.name, "Todo \"MVC\"");
        let caps: Vec<String> = parsed
            .capabilities
            .iter()
            .map(|c| c.cap.to_string())
            .collect();
        assert_eq!(
            caps,
            [
                "ui.window:create",
                "store.kv",
                "net.connect:api.example.com:443"
            ]
        );
    }

    #[test]
    fn the_carried_shell_speaks_app_view() {
        let text = String::from_utf8_lossy(WEB_SHELL);
        assert!(
            text.contains("krate:ui/app-view@0.1.0"),
            "rebuild apps/krate-web-shell and copy it"
        );
        assert!(text.contains("krate:store/kv@0.1.0"));
    }

    #[test]
    fn a_project_folder_is_refused_with_what_to_wrap_instead() {
        let dir = std::env::temp_dir().join(format!("krate-web-wrap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "x").unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        let err = wrap(&WebWrap {
            folder: dir.clone(),
            to: dir.join("x.krate"),
            name: None,
            allow: vec![],
            size: None,
            start: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("npm run build"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
