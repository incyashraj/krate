//! The Krate Studio shell: a window around the `krate` engine.
//!
//! Every command here spawns the same binary the terminal uses and reads the
//! same output a person at a terminal would see. The studio adds no second
//! implementation of anything -- if the engine and the studio ever disagree,
//! one of them is lying, and thin wrappers make that class of bug impossible.
//!
//! The one rule of everything user-facing: **plain words out**. Raw engine
//! lines stream to the UI's collapsed details log; what the person reads at
//! eye level is written here or in the frontend, never by a compiler.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use base64::Engine as _;
use tauri::Emitter;

/// Where the engine lives, most-deliberate first.
///
/// 1. `KRATE_STUDIO_ENGINE` -- development, and the only way to be certain
///    which binary answered (K-030: a debug build shadowing the release on
///    PATH made a fixed bug appear to come back twice).
/// 2. Beside this executable -- how a bundled Krate.app ships, engine and
///    shell in one directory, versioned together.
/// 3. `krate` on PATH -- a plain CLI install, last because it is the least
///    certain of the three.
fn engine() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("KRATE_STUDIO_ENGINE") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!(
            "KRATE_STUDIO_ENGINE points at {}, which does not exist",
            path.display()
        ));
    }
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let name = if cfg!(windows) { "krate.exe" } else { "krate" };
            let sibling = dir.join(name);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }
    Ok(PathBuf::from("krate"))
}

#[derive(serde::Serialize)]
struct AgentInfo {
    name: String,
    label: String,
    state: String,
    detail: String,
}

/// Which AIs this machine can author with, through `krate ai --json`.
#[tauri::command]
async fn agents() -> Result<Vec<AgentInfo>, String> {
    let engine = engine()?;
    let out = Command::new(&engine)
        .args(["ai", "--json"])
        .output()
        .map_err(|err| format!("could not run the Krate engine at {}: {err}", engine.display()))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout)
        .map_err(|err| format!("the engine's agent list did not parse: {err}"))?;
    Ok(parsed
        .into_iter()
        .map(|a| AgentInfo {
            name: a["name"].as_str().unwrap_or("").to_string(),
            label: a["label"].as_str().unwrap_or("").to_string(),
            state: a["state"].as_str().unwrap_or("missing").to_string(),
            detail: a["detail"].as_str().unwrap_or("").to_string(),
        })
        .collect())
}

#[derive(serde::Serialize)]
struct CreateResult {
    path: String,
    name: String,
    size: String,
    asks: Vec<String>,
    shot: String,
}

/// Author an app from the accumulated request, streaming every engine line
/// to the UI as it happens.
#[tauri::command]
async fn create_app(app: tauri::AppHandle, request: String) -> Result<CreateResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_create(&app, &request))
        .await
        .map_err(|err| err.to_string())?
}

/// Change an app that already exists: every studio message after the first.
///
/// The .krate carries its own source, so the engine edits the app in place --
/// "make the button blue" is a few lines' diff, never a from-scratch rebuild.
/// The engine also owns the fallback for old bundles with no source inside.
#[tauri::command]
async fn revise_app(
    app: tauri::AppHandle,
    path: String,
    change: String,
) -> Result<CreateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let out_path = PathBuf::from(&path);
        let child = Command::new(&engine)
            .arg("revise")
            .arg(&out_path)
            .arg(&change)
            .args(["--agent", "claude"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| format!("could not start the Krate engine: {err}"))?;
        finish_author(&app, child, &engine, &out_path)
    })
    .await
    .map_err(|err| err.to_string())?
}

fn run_create(app: &tauri::AppHandle, request: &str) -> Result<CreateResult, String> {
    let engine = engine()?;

    // Finished apps land somewhere a person actually looks, named after
    // their own words -- never a temp dir they will lose.
    let out_dir = dirs_home()
        .join("Documents")
        .join("Krate Apps");
    std::fs::create_dir_all(&out_dir).map_err(|err| err.to_string())?;
    let slug = slugify(request);
    let out_path = out_dir.join(format!("{slug}.krate"));

    let child = Command::new(&engine)
        .arg("create")
        .arg(request)
        .args(["--agent", "claude", "--yes", "--output"])
        .arg(&out_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|err| format!("could not start the Krate engine: {err}"))?;
    finish_author(app, child, &engine, &out_path)
}

/// Stream an authoring child's output to the UI, then read back the result.
fn finish_author(
    app: &tauri::AppHandle,
    mut child: std::process::Child,
    engine: &PathBuf,
    out_path: &std::path::Path,
) -> Result<CreateResult, String> {

    // Stream both pipes as one story. Order between the two is best-effort,
    // which is fine: the UI folds these into a details log and a coarse
    // stage indicator, not a transcript that must be exact.
    let mut tail: Vec<String> = Vec::new();
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let app2 = app.clone();
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = app2.emit("engine-line", &line);
            lines.push(line);
        }
        lines
    });
    let mut asks: Vec<String> = Vec::new();
    let mut in_asks = false;
    for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
        let _ = app.emit("engine-line", &line);
        if line.trim_start().starts_with("requested access") {
            in_asks = true;
        } else if in_asks {
            let t = line.trim();
            if let Some(cap) = t.strip_prefix("- ") {
                asks.push(cap.to_string());
            } else if !t.is_empty() {
                in_asks = false;
            }
        }
        tail.push(line);
        if tail.len() > 40 {
            tail.remove(0);
        }
    }
    let err_lines = err_thread.join().unwrap_or_default();
    let status = child.wait().map_err(|err| err.to_string())?;

    if !status.success() {
        // The UI turns this into plain words; give it the most recent real
        // output to classify with, stderr preferred.
        let mut why = err_lines.join("\n");
        if why.trim().is_empty() {
            why = tail.join("\n");
        }
        return Err(why);
    }
    if !out_path.exists() {
        return Err("the build finished but no app file was written".to_string());
    }

    let bytes = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    Ok(CreateResult {
        path: out_path.display().to_string(),
        name: out_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "app.krate".to_string()),
        size: human_size(bytes),
        asks,
        shot: shoot(engine, out_path).unwrap_or_default(),
    })
}

/// Render the finished app's real first frame, as a data URL.
///
/// This is the moment the person decides the thing is real, so it must be
/// the app's own pixels -- `krate run --shoot` renders headlessly through
/// the same runtime that will draw the window.
fn shoot(engine: &PathBuf, krate_path: &std::path::Path) -> Option<String> {
    let png = std::env::temp_dir().join(format!("krate-studio-shot-{}.png", std::process::id()));
    let ok = Command::new(engine)
        .arg("run")
        .arg(krate_path)
        .args(["--shoot"])
        .arg(&png)
        .args(["--auto-grant", "--", "quick"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    let bytes = std::fs::read(&png).ok()?;
    let _ = std::fs::remove_file(&png);
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Open the finished app the way a double-click would.
#[tauri::command]
fn open_app(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let ok = Command::new("open").arg(&path).status();
    #[cfg(target_os = "windows")]
    let ok = Command::new("cmd").args(["/C", "start", "", &path]).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let ok = Command::new("xdg-open").arg(&path).status();
    ok.map_err(|err| err.to_string()).and_then(|s| {
        if s.success() {
            Ok(())
        } else {
            Err("could not open the app".to_string())
        }
    })
}

/// Publish to the hub and hand back the run-by-URL link.
#[tauri::command]
async fn publish(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let out = Command::new(&engine)
            .arg("publish")
            .arg(&path)
            .output()
            .map_err(|err| err.to_string())?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if !out.status.success() {
            return Err(text.trim().to_string());
        }
        text.split_whitespace()
            .find(|w| w.starts_with("https://"))
            .map(str::to_string)
            .ok_or_else(|| "published, but no link came back".to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Show the file itself, for people who want to drag it into a chat.
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let ok = Command::new("open").args(["-R", &path]).status();
    #[cfg(target_os = "windows")]
    let ok = Command::new("explorer").arg(format!("/select,{path}")).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let ok = Command::new("xdg-open")
        .arg(
            std::path::Path::new(&path)
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .as_os_str(),
        )
        .status();
    ok.map_err(|err| err.to_string()).map(|_| ())
}

fn dirs_home() -> PathBuf {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var(var).map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

/// A file name from the person's own words: first few, kebab-case.
fn slugify(request: &str) -> String {
    let words: Vec<String> = request
        .split_whitespace()
        .filter(|w| !matches!(w.to_lowercase().as_str(), "a" | "an" | "the" | "make" | "me"))
        .take(4)
        .map(|w| {
            w.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        "my-app".to_string()
    } else {
        words.join("-")
    }
}

fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            agents, create_app, revise_app, open_app, publish, reveal
        ])
        .run(tauri::generate_context!())
        .expect("the studio window could not start");
}
