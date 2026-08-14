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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use base64::Engine as _;
use tauri::{Emitter, Manager};

/// The one build allowed at a time, by process id, so Stop can reach it.
///
/// The engine child gets its own process group (Unix) so stopping kills the
/// whole tree -- the agent CLI and cargo underneath it -- not just the
/// parent, which would leave an orphan burning the person's AI quota after
/// they pressed Stop.
struct Running(Mutex<Option<u32>>);

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

fn studio_dir() -> PathBuf {
    let dir = dirs_home().join(".krate").join("studio");
    let _ = std::fs::create_dir_all(dir.join("sessions"));
    dir
}

/* ---- settings --------------------------------------------------------- */

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Settings {
    /// Where finished .krate files land. Their folder, their choice.
    out_dir: String,
    /// Which AI authors, by provider name.
    agent: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            out_dir: dirs_home()
                .join("Documents")
                .join("Krate Apps")
                .display()
                .to_string(),
            agent: "claude".to_string(),
        }
    }
}

#[tauri::command]
fn settings_get() -> Settings {
    std::fs::read_to_string(studio_dir().join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn settings_set(settings: Settings) -> Result<(), String> {
    std::fs::write(
        studio_dir().join("settings.json"),
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/* ---- sessions: the development history -------------------------------- */

/// One conversation and the app it produced. Stored whole as JSON, one file
/// per session, so history survives anything short of deleting the folder --
/// a person who gets feedback on their app next week reopens this and says
/// what to change.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Session {
    id: String,
    title: String,
    created: u64,
    updated: u64,
    messages: Vec<serde_json::Value>,
    result: Option<serde_json::Value>,
}

#[tauri::command]
fn sessions_list() -> Vec<Session> {
    let dir = studio_dir().join("sessions");
    let mut sessions: Vec<Session> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|text| serde_json::from_str(&text).ok())
                .collect()
        })
        .unwrap_or_default();
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated));
    sessions
}

#[tauri::command]
fn session_save(session: Session) -> Result<(), String> {
    // The id is ours (a timestamp), but never trust a path component you did
    // not mint this second.
    if !session.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad session id".to_string());
    }
    let path = studio_dir().join("sessions").join(format!("{}.json", session.id));
    std::fs::write(
        path,
        serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn session_delete(id: String) -> Result<(), String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad session id".to_string());
    }
    let _ = std::fs::remove_file(studio_dir().join("sessions").join(format!("{id}.json")));
    Ok(())
}

/* ---- account ---------------------------------------------------------- */

#[tauri::command]
async fn account_status() -> Result<serde_json::Value, String> {
    let engine = engine()?;
    let out = Command::new(&engine)
        .args(["account", "--json"])
        .output()
        .map_err(|err| format!("could not run the Krate engine: {err}"))?;
    serde_json::from_slice(&out.stdout)
        .map_err(|_| String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Sign in: the engine runs GitHub's device flow and speaks NDJSON. Each
/// step goes straight to the UI -- the code the person must type appears the
/// moment GitHub issues it, and the screen flips the moment approval lands.
#[tauri::command]
async fn account_login(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let mut child = Command::new(&engine)
            .args(["account", "login", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| format!("could not start the Krate engine: {err}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(step) = serde_json::from_str::<serde_json::Value>(&line) {
                let _ = app.emit("login-step", &step);
            }
        }
        let status = child.wait().map_err(|err| err.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("sign-in did not complete".to_string())
        }
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
async fn account_logout() -> Result<(), String> {
    let engine = engine()?;
    Command::new(&engine)
        .args(["account", "logout"])
        .output()
        .map_err(|err| err.to_string())?;
    Ok(())
}

/* ---- agents ----------------------------------------------------------- */

#[derive(serde::Serialize)]
struct AgentInfo {
    name: String,
    label: String,
    state: String,
    detail: String,
    remedy: Option<String>,
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
            remedy: a["remedy"].as_str().map(str::to_string),
        })
        .collect())
}

/* ---- files ------------------------------------------------------------ */

/// A native file picker for attachments. `rfd` is the same crate the
/// runtime's own host dialogs use; macOS requires it on the main thread.
#[tauri::command]
async fn pick_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let picked = rfd::FileDialog::new()
            .set_title("Attach files for the AI to read")
            .pick_files()
            .unwrap_or_default();
        let _ = tx.send(picked);
    })
    .map_err(|err| err.to_string())?;
    let picked = rx
        .recv_timeout(std::time::Duration::from_secs(300))
        .map_err(|_| "the file picker did not answer".to_string())?;
    Ok(picked.iter().map(|p| p.display().to_string()).collect())
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let picked = rfd::FileDialog::new()
            .set_title("Where finished apps are saved")
            .pick_folder();
        let _ = tx.send(picked);
    })
    .map_err(|err| err.to_string())?;
    let picked = rx
        .recv_timeout(std::time::Duration::from_secs(300))
        .map_err(|_| "the folder picker did not answer".to_string())?;
    Ok(picked.map(|p| p.display().to_string()))
}

/* ---- authoring -------------------------------------------------------- */

#[derive(serde::Serialize)]
struct CreateResult {
    path: String,
    name: String,
    size: String,
    asks: Vec<String>,
    shot: String,
}

#[tauri::command]
async fn create_app(
    app: tauri::AppHandle,
    request: String,
    agent: String,
    attachments: Vec<String>,
    out_dir: String,
) -> Result<CreateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let dir = if out_dir.is_empty() {
            PathBuf::from(Settings::default().out_dir)
        } else {
            PathBuf::from(out_dir)
        };
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let out_path = dir.join(format!("{}.krate", slugify(&request)));

        let mut cmd = Command::new(&engine);
        cmd.arg("create")
            .arg(&request)
            .args(["--agent", &agent, "--yes", "--output"])
            .arg(&out_path);
        for file in &attachments {
            cmd.args(["--attach", file]);
        }
        run_author(&app, cmd, &engine, &out_path)
    })
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
    agent: String,
    attachments: Vec<String>,
) -> Result<CreateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let out_path = PathBuf::from(&path);
        let mut cmd = Command::new(&engine);
        cmd.arg("revise")
            .arg(&out_path)
            .arg(&change)
            .args(["--agent", &agent]);
        for file in &attachments {
            cmd.args(["--attach", file]);
        }
        run_author(&app, cmd, &engine, &out_path)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Stop the running build: kill the engine child's whole process group so
/// the agent and cargo underneath stop too, not just the parent.
#[tauri::command]
fn stop_build(state: tauri::State<Running>) -> Result<(), String> {
    let pid = state.0.lock().map_err(|_| "poisoned")?.take();
    let Some(pid) = pid else {
        return Ok(());
    };
    #[cfg(unix)]
    unsafe {
        // Negative pid: the whole process group started for this build.
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
    Ok(())
}

/// Spawn an authoring child, stream its lines to the UI, read the result.
fn run_author(
    app: &tauri::AppHandle,
    mut cmd: Command,
    engine: &PathBuf,
    out_path: &Path,
) -> Result<CreateResult, String> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so Stop can end the whole tree at once.
        cmd.process_group(0);
    }
    // The first line arrives before the engine says anything, so a working
    // event pipeline is visible within milliseconds -- and a broken one is
    // visible too, as this exact line missing from the details log. The
    // silent version of this bug shipped once: Tauri denies event.listen
    // without a capabilities grant, invokes kept working, and the build
    // screen froze on stage one while the engine worked perfectly.
    let _ = app.emit("engine-line", "==> starting the Krate engine");
    let mut child = cmd
        .spawn()
        .map_err(|err| format!("could not start the Krate engine: {err}"))?;

    let running = app.state::<Running>();
    *running.0.lock().map_err(|_| "poisoned")? = Some(child.id());

    // Stream both pipes as one story. Order between the two is best-effort,
    // which is fine: the UI folds these into a details log and a coarse
    // stage indicator, not a transcript that must be exact.
    let mut tail: Vec<String> = Vec::new();
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let debug_log = std::env::var("KRATE_STUDIO_DEBUG").ok().map(PathBuf::from);
    let app2 = app.clone();
    let debug2 = debug_log.clone();
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = app2.emit("engine-line", &line);
            if let Some(path) = &debug2 {
                append_line(path, &line);
            }
            lines.push(line);
        }
        lines
    });
    let mut asks: Vec<String> = Vec::new();
    let mut in_asks = false;
    for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
        let _ = app.emit("engine-line", &line);
        if let Some(path) = &debug_log {
            append_line(path, &line);
        }
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
    // Stop takes the pid out before killing; an empty slot on a failed exit
    // means the person asked for this outcome.
    let was_stopped = running.0.lock().map(|g| g.is_none()).unwrap_or(false);
    *running.0.lock().map_err(|_| "poisoned")? = None;

    if !status.success() {
        if was_stopped {
            return Err("stopped".to_string());
        }
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
fn shoot(engine: &PathBuf, krate_path: &Path) -> Option<String> {
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

/// Publish to the hub and hand back the short run-by-URL link.
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
            Path::new(&path)
                .parent()
                .unwrap_or(Path::new("."))
                .as_os_str(),
        )
        .status();
    ok.map_err(|err| err.to_string()).map(|_| ())
}

/// A request to run the moment the studio opens, for driving a real
/// end-to-end build in automation without faking anyone's keyboard.
/// Development and testing only; unset for people.
#[tauri::command]
fn autorun() -> Option<String> {
    std::env::var("KRATE_STUDIO_AUTORUN").ok().filter(|s| !s.is_empty())
}

fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
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
        .manage(Running(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            agents,
            create_app,
            revise_app,
            stop_build,
            open_app,
            publish,
            reveal,
            settings_get,
            settings_set,
            sessions_list,
            session_save,
            session_delete,
            account_status,
            account_login,
            account_logout,
            pick_files,
            pick_folder,
            autorun
        ])
        .run(tauri::generate_context!())
        .expect("the studio window could not start");
}
