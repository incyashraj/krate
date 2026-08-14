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

    let name = if cfg!(windows) { "krate.exe" } else { "krate" };
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            // Beside the executable: a dev build, or a Windows/Linux install
            // where the bundler puts resources alongside the binary.
            let sibling = dir.join(name);
            if sibling.exists() {
                return Ok(sibling);
            }
            // Inside a macOS .app the bundler puts resources in
            // Contents/Resources/, NOT Contents/MacOS/ beside the binary.
            // Checking only for a sibling meant a bundled Krate.app shipped
            // its engine and then failed to find it -- the app would install
            // cleanly and be unable to make anything.
            for rel in ["../Resources/bin", "../Resources"] {
                let candidate = dir.join(rel).join(name);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }
    Ok(PathBuf::from(name))
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
            // ~/Krate Apps, not ~/Documents/Krate Apps.
            //
            // macOS guards Documents, Desktop and Downloads with TCC, so
            // writing the first app there made the system demand access to the
            // person's Documents folder -- an alarming prompt from an app that
            // only wanted to save a file it just made. The home folder itself
            // is not guarded, so the default now writes somewhere the person
            // can see without anyone being asked for anything.
            //
            // Still a setting: someone who wants it in Documents picks that,
            // and macOS asks them once, in response to their own choice.
            out_dir: dirs_home().join("Krate Apps").display().to_string(),
            agent: "claude".to_string(),
        }
    }
}

#[tauri::command]
fn settings_get() -> Settings {
    let mut settings: Settings = std::fs::read_to_string(studio_dir().join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    // Move anyone still pointing at the old default out of ~/Documents.
    //
    // Changing the default only helps new installs; a person who ran an
    // earlier build has the Documents path saved and would keep meeting the
    // TCC prompt forever. Only the exact old default is migrated -- a folder
    // someone chose themselves is their choice and is left alone, even if it
    // is inside Documents.
    let old_default = dirs_home().join("Documents").join("Krate Apps");
    if PathBuf::from(&settings.out_dir) == old_default {
        settings.out_dir = Settings::default().out_dir;
        let _ = settings_set(settings.clone());
    }
    settings
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
        // Start in the person's Documents, never wherever macOS last was.
        // Without a directory the picker can open in Photos or Downloads,
        // and macOS then demands access to that library on the spot -- an
        // alarming prompt from an app that only wanted a file they pick.
        let picked = rfd::FileDialog::new()
            .set_title("Attach files for the AI to read")
            .set_directory(dirs_home())
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
            .set_directory(dirs_home())
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
    session: String,
) -> Result<CreateResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let dir = if out_dir.is_empty() {
            PathBuf::from(Settings::default().out_dir)
        } else {
            PathBuf::from(out_dir)
        };
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let out_path = free_path(&dir, &slugify(&request));
        remember_target(&session, &out_path);

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
        // Fail before spending anyone's AI quota on a file that is gone.
        let out_path = existing(&path)?;
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
    if let Some(pid) = pid {
        kill_tree(pid);
    }
    Ok(())
}

/// End a build and everything under it.
///
/// The agent CLI and cargo are children of the engine, so signalling only
/// the engine leaves them running -- an invisible process still spending the
/// person's AI quota after they thought they had stopped.
fn kill_tree(pid: u32) {
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
}

/// Remember which app a session is producing, before the build starts.
///
/// The UI writes the result when `finishBuild` runs -- but a build can
/// finish on disk after the window is gone (quit, or a crash), and then the
/// app exists with no session pointing at it. Observed: a unit converter
/// landed at 31,735 bytes while its session still read "unfinished".
///
/// The shell knows the path before it spawns anything, so it records it up
/// front. Whatever happens to the window, the session can find its app.
fn remember_target(session: &str, path: &Path) {
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return;
    }
    let file = studio_dir().join("sessions").join(format!("{session}.json"));
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    value["pending_path"] = serde_json::json!(path.display().to_string());
    if let Ok(out) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(file, out);
    }
}

/// Spawn an authoring child, stream its lines to the UI, read the result.
fn run_author(
    app: &tauri::AppHandle,
    mut cmd: Command,
    engine: &PathBuf,
    out_path: &Path,
) -> Result<CreateResult, String> {
    // Run the agent from a scratch directory of ours, never from whatever
    // the studio happened to inherit.
    //
    // A Finder-launched .app has `/` as its working directory, so the agent
    // started at the filesystem root and explored outward -- which is why
    // macOS asked to "access files in your Downloads folder" during a build.
    // An AI writing a tip splitter has no business anywhere near a person's
    // Downloads, and the prompt is alarming precisely because it is
    // unjustifiable.
    let work = studio_dir().join("work");
    let _ = std::fs::create_dir_all(&work);
    cmd.current_dir(&work)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    // Give the agent its own config directory, with no history in it.
    //
    // This is what actually caused "Krate Studio would like to access files
    // in your Downloads folder". Claude Code reads ~/.claude.json at
    // startup, which holds every project the person has ever opened -- two
    // of them under Downloads on this machine -- and stats those paths. It
    // happens inside our process, so macOS names US in the prompt.
    //
    // Neither setting the file picker's directory nor setting the child's
    // cwd fixed it, because the dialog and the cwd were never the cause. A
    // private CLAUDE_CONFIG_DIR means the agent starts with no project
    // history and therefore no reason to touch anything outside its
    // workspace. The person's own terminal history is untouched.
    let agent_home = studio_dir().join("agent");
    let _ = std::fs::create_dir_all(&agent_home);
    cmd.env("CLAUDE_CONFIG_DIR", &agent_home);

    // HOME too, not only CLAUDE_CONFIG_DIR.
    //
    // This is the fix that actually stops the prompt, and the previous two
    // attempts did not: CLAUDE_CONFIG_DIR moves ~/.claude.json, but the agent
    // still reads ~/.claude/ beside it -- and on this machine
    // ~/.claude/history.jsonl held 27 paths under Downloads and Documents
    // from months of unrelated work. The agent stats those at startup, inside
    // a process we spawned, so macOS names KRATE STUDIO in the dialog and the
    // person is asked why their app maker wants their documents. There is no
    // good answer, because it never wanted them.
    //
    // Pointing HOME at our own directory means every tool we spawn resolves
    // "~" to a folder that contains nothing but this app's work. The person's
    // own ~/.claude is untouched and their terminal sessions are unaffected.
    cmd.env("HOME", &agent_home);
    // XDG equivalents, for tools that follow that convention instead of HOME.
    cmd.env("XDG_CONFIG_HOME", agent_home.join(".config"));
    cmd.env("XDG_CACHE_HOME", agent_home.join(".cache"));
    cmd.env("XDG_DATA_HOME", agent_home.join(".local/share"));
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
    // One build at a time. Two at once would overwrite each other's pid in
    // `Running`, so Stop could only ever reach the second and the first
    // would keep burning AI quota with nothing able to end it.
    {
        let running = app.state::<Running>();
        let guard = running.0.lock().map_err(|_| "poisoned")?;
        if guard.is_some() {
            return Err("one app is already being made -- wait for it, or press Stop".to_string());
        }
    }

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
    let work = studio_dir().join("work");
    let _ = std::fs::create_dir_all(&work);
    let ok = Command::new(engine)
        .current_dir(&work)
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

/// The app file, if it is still there.
///
/// A .krate can be moved, renamed or deleted between making it and pressing
/// a button -- especially a session reopened days later. Every action that
/// touches the file checks first, so the answer is a sentence rather than a
/// silent no-op or an OS error dialog.
fn existing(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if p.exists() {
        return Ok(p);
    }
    Err(format!(
        "{} is not there any more -- it may have been moved or deleted.",
        p.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string())
    ))
}

/// Open the finished app the way a double-click would.
#[tauri::command]
fn open_app(path: String) -> Result<(), String> {
    let path = existing(&path)?;
    let path = path.display().to_string();
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
        let path = existing(&path)?;
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

/// Everything knowable about one app, for the details screen.
///
/// Read through the engine, so the studio never parses a .krate itself and
/// cannot drift from what the runtime believes about the same file.
#[tauri::command]
async fn app_info(path: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // A cloud URL is as valid a target as a local file: the engine reads
        // both, and inspecting a published app before running it is exactly
        // what the cloud detail page is for.
        let target = if path.starts_with("https://") {
            path.clone()
        } else {
            existing(&path)?.display().to_string()
        };
        let engine = engine()?;
        let out = Command::new(&engine)
            .arg("run")
            .arg("--dump-caps")
            .arg(&target)
            .output()
            .map_err(|err| format!("could not run the Krate engine: {err}"))?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();

        // `--dump-caps` prints two sections: an identity hash, then one
        // capability per line. Parsed here rather than adding a JSON mode to
        // the engine, because the shape is small and stable.
        let mut identity = String::new();
        let mut caps: Vec<String> = Vec::new();
        let mut in_caps = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Effective capabilities") {
                in_caps = true;
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("- ") {
                if in_caps {
                    caps.push(value.to_string());
                } else if identity.is_empty() {
                    identity = value.to_string();
                }
            }
        }

        // A remote app has no local size; the hub already reported it.
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        Ok(serde_json::json!({
            "path": target,
            "identity": identity,
            "capabilities": caps,
            "size": size,
        }))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Install an AI CLI from inside the studio, streaming progress to the UI.
///
/// The alternative was printing "npm install -g @google/gemini-cli" and
/// expecting someone to find a terminal, leave the app, run it, and come
/// back. Most people making their first app do not have a terminal open and
/// should not need one.
///
/// The package name comes from the engine (`krate ai --json`), never from the
/// UI, and is checked against that list before anything runs -- so this cannot
/// be talked into installing an arbitrary package.
#[tauri::command]
async fn install_agent(app: tauri::AppHandle, name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let listed = Command::new(&engine)
            .args(["ai", "--json"])
            .output()
            .map_err(|err| format!("could not run the Krate engine: {err}"))?;
        let rows: serde_json::Value =
            serde_json::from_slice(&listed.stdout).map_err(|err| err.to_string())?;
        let package = rows
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|row| row["name"] == serde_json::json!(name))
            })
            .and_then(|row| row["install_package"].as_str())
            .ok_or_else(|| "that tool cannot be installed automatically".to_string())?
            .to_string();

        let npm = which_npm().ok_or_else(|| {
            "Installing an AI needs Node.js, which is not on this machine.              Install Node from nodejs.org, then try again."
                .to_string()
        })?;

        emit_line(&app, &format!("Installing {package}"));
        let mut child = Command::new(&npm);
        // npm is a Node script, so it needs `node` on PATH -- and node lives
        // beside npm. Without this the install dies with "env: node: No such
        // file or directory" whenever npm is anywhere but /usr/bin, which is
        // every version-manager install. Found by running the exact command
        // the studio runs, with the environment a GUI app actually has.
        if let Some(bin) = npm.parent() {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut dirs = vec![bin.to_path_buf()];
            dirs.extend(std::env::split_paths(&existing));
            if let Ok(joined) = std::env::join_paths(dirs) {
                child.env("PATH", joined);
            }
        }
        let mut child = child
            .args(["install", "-g", &package])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| format!("could not start npm: {err}"))?;

        // npm writes progress to stderr, so both streams matter.
        if let Some(out) = child.stdout.take() {
            let app = app.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    emit_line(&app, &line);
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let app = app.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                    emit_line(&app, &line);
                }
            });
        }

        let status = child.wait().map_err(|err| err.to_string())?;
        if !status.success() {
            return Err(
                "The install did not finish. Node may need permission to write                  its global folder."
                    .to_string(),
            );
        }
        Ok(())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// npm, found without a shell so a login-shell PATH is not required.
///
/// A GUI app on macOS does not inherit the PATH from a terminal profile, so a
/// bare `npm` often fails inside the app while working fine in a shell. These
/// are where Node actually installs.
fn which_npm() -> Option<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/opt/homebrew/bin/npm"),
        PathBuf::from("/usr/local/bin/npm"),
        PathBuf::from("/usr/bin/npm"),
    ];
    // Node version managers put it under the home directory, and nvm -- the
    // most common of them -- nests it one directory per installed version.
    // Missing this was not hypothetical: npm was under nvm on the machine
    // this was written on, and the standard three paths all missed it.
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(&home);
        roots.push(home.join(".volta/bin/npm"));
        roots.push(home.join(".local/bin/npm"));
        roots.push(home.join(".bun/bin/npm"));
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            // Newest version first, so a stale old install is not preferred.
            let mut found: Vec<PathBuf> = versions
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path().join("bin/npm"))
                .filter(|path| path.is_file())
                .collect();
            found.sort();
            found.reverse();
            roots.extend(found);
        }
        if let Ok(versions) = std::fs::read_dir(home.join(".fnm/node-versions")) {
            let mut found: Vec<PathBuf> = versions
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path().join("installation/bin/npm"))
                .filter(|path| path.is_file())
                .collect();
            found.sort();
            found.reverse();
            roots.extend(found);
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            roots.push(dir.join("npm"));
        }
    }
    roots.into_iter().find(|candidate| candidate.is_file())
}

/// One line of progress to the UI, on the same channel the build log uses.
fn emit_line(app: &tauri::AppHandle, line: &str) {
    let _ = app.emit("agent-install", line.to_string());
}

/// The hub the studio reads and publishes to. `KRATE_HUB_URL` overrides it,
/// the same variable the engine honours, so a local hub serves both.
fn hub_url() -> String {
    std::env::var("KRATE_HUB_URL").unwrap_or_else(|_| "https://hub.krate.tech".to_string())
}

/// Everything published to Krate Cloud, newest first.
///
/// Fetched here rather than from the webview so the page keeps its locked-down
/// CSP: the UI never talks to the network itself.
#[tauri::command]
async fn cloud_apps() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let url = format!("{}/apps", hub_url());
        let response = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(20))
            .call()
            .map_err(|err| match err {
                // The only failure worth distinguishing: no connection at all
                // is a different situation from a hub that answered badly.
                ureq::Error::Transport(_) => {
                    "Krate Cloud could not be reached. Check your connection.".to_string()
                }
                other => other.to_string(),
            })?;
        response.into_string().map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Run a published app straight from the cloud, by its URL.
///
/// The engine already runs by URL, and it applies the same permission wall it
/// applies to a local file -- nothing is trusted more for having come from the
/// hub.
#[tauri::command]
async fn cloud_run(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("that is not a Krate Cloud link".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        Command::new(&engine)
            .arg("run")
            .arg(&url)
            .spawn()
            .map(|_| ())
            .map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Show the file itself, for people who want to drag it into a chat.
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let path = existing(&path)?;
    let path = path.display().to_string();
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

/// Finish the install, the first time the studio runs after being dragged in.
///
/// A .dmg gives someone one gesture: drag Krate to Applications. Everything
/// else that makes Krate work has to happen without asking them to open a
/// terminal, which is the whole promise of this app.
///
/// Three things, all idempotent, all safe to fail:
///
/// 1. **Register the runtime with Launch Services**, so double-clicking a
///    `.krate` opens it. The studio ships the engine inside itself; without
///    this the file type belongs to nobody and a shared app is a dead icon.
/// 2. **Put `krate` on PATH**, via a symlink in /usr/local/bin when that is
///    writable. The terminal tool is what the docs describe, and someone who
///    installed the studio should have it too. Skipped silently when the
///    directory needs an administrator -- asking for a password on first
///    launch is worse than not having the shortcut.
/// 3. **Record that this ran**, so it happens once rather than every launch.
#[cfg(target_os = "macos")]
fn first_run_setup() {
    let marker = studio_dir().join("setup-done");
    if marker.exists() {
        return;
    }

    if let Ok(engine) = engine() {
        // The engine registers its own document types; running it once with
        // a no-op subcommand is enough for Launch Services to see the bundle
        // it lives in.
        let bundle = engine
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());
        if let Some(bundle) = bundle {
            let lsregister = "/System/Library/Frameworks/CoreServices.framework/\
                              Frameworks/LaunchServices.framework/Support/lsregister";
            let _ = Command::new(lsregister)
                .arg("-f")
                .arg(bundle)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        // `krate` on PATH, for the terminal. Only where it does not need a
        // password: /usr/local/bin is writable by the admin user on most
        // machines, and when it is not, this is simply skipped.
        let link = PathBuf::from("/usr/local/bin/krate");
        if !link.exists() {
            if let Some(dir) = link.parent() {
                if dir.is_dir() {
                    let _ = std::os::unix::fs::symlink(&engine, &link);
                }
            }
        }
    }

    let _ = std::fs::write(&marker, "1");
}

#[cfg(not(target_os = "macos"))]
fn first_run_setup() {}

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

/// A path that is not already taken, by adding ` 2`, ` 3` and so on.
///
/// Two sessions from the same words -- "a habit tracker" twice, or one
/// request retried after a failure -- produced the same slug and so the same
/// path, and the second silently overwrote the first. The person's earlier
/// app simply vanished, with its session still pointing at the file and
/// showing the newer app's contents. Observed with two "a coin flip app
/// with..." sessions both holding coin-flip-app-with.krate.
fn free_path(dir: &Path, slug: &str) -> PathBuf {
    let first = dir.join(format!("{slug}.krate"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{slug} {n}.krate"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
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
    // Finish the install before the window appears. On a background thread:
    // it touches Launch Services and the filesystem, and none of it should
    // ever delay the first paint.
    std::thread::spawn(first_run_setup);

    tauri::Builder::default()
        .manage(Running(Mutex::new(None)))
        // Closing the window mid-build must not leave the AI running.
        //
        // CloseRequested, not just Destroyed: on macOS closing a window does
        // not quit the app or destroy the window, so a Destroyed-only handler
        // never fired -- measured, with 17 agent processes still alive after
        // the close button. The build kept spending the person's quota with
        // no window left to stop it from.
        //
        // ExitRequested covers Cmd-Q and the Dock's Quit, which close no
        // window at all.
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                let state = window.state::<Running>();
                let pid = state.0.lock().ok().and_then(|mut g| g.take());
                if let Some(pid) = pid {
                    kill_tree(pid);
                }
            }
        })
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
            app_info,
            install_agent,
            cloud_apps,
            cloud_run,
            pick_files,
            pick_folder,
            autorun
        ])
        .build(tauri::generate_context!())
        .expect("the studio window could not start")
        .run(|app, event| {
            // A .krate opened through the studio.
            //
            // The bundle declares the type, so Finder can route a file here --
            // and without this it would arrive and nothing would happen. Run
            // it through the engine, which applies the same permission wall it
            // applies everywhere else.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                for url in urls {
                    if let Ok(path) = url.to_file_path() {
                        if path.extension().and_then(|e| e.to_str()) == Some("krate") {
                            if let Ok(engine) = engine() {
                                let _ = Command::new(&engine).arg("run").arg(&path).spawn();
                            }
                        }
                    }
                }
            }

            // Cmd-Q and Dock > Quit close no window, so the window handler
            // above never sees them.
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                let state = app.state::<Running>();
                let pid = state.0.lock().ok().and_then(|mut g| g.take());
                if let Some(pid) = pid {
                    kill_tree(pid);
                }
            }
        });
}
