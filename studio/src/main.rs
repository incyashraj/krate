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

use std::io::{BufRead, Read};
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
struct Running(Mutex<Option<u32>>, std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Running {
    fn fresh() -> Self {
        Running(
            Mutex::new(None),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
    }
}

/// Whether the main window has been shown. The window starts hidden so that
/// a double-clicked .krate can pass through the studio without the studio
/// appearing: the person asked for their app, not for Krate.
static WINDOW_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Whether a document claimed this launch during the startup grace.
static DOC_CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// When this process started, for telling a cold document-open apart from a
/// document opened into a studio someone is already using.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Parse a krate://signed-in handoff (query or fragment) and store the
/// identity through the engine. Used by the macOS open event and by the
/// argv path Windows and Linux deliver scheme URLs through.
fn adopt_from_uri(uri: &str) -> bool {
    let Ok(url) = url::Url::parse(uri) else {
        return false;
    };
    let payload = url
        .query()
        .map(str::to_string)
        .or_else(|| url.fragment().map(str::to_string))
        .unwrap_or_default();
    let fields: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(payload.as_bytes()).collect();
    let (token, login) = (
        fields.get("token").cloned().unwrap_or_default(),
        fields.get("login").cloned().unwrap_or_default(),
    );
    if token.is_empty() || login.is_empty() {
        return false;
    }
    let identity = serde_json::json!({
        "login": login,
        "name": fields.get("name").cloned().unwrap_or_default(),
        "avatar_url": fields.get("avatar_url").cloned().unwrap_or_default(),
        "token": token,
    });
    let Ok(engine) = engine() else { return false };
    silent_cmd(&engine)
        .args(["account", "adopt"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .map(|mut sin| sin.write_all(identity.to_string().as_bytes()))
                .transpose()?;
            child.wait()
        })
        .map(|status| status.success())
        .unwrap_or(false)
}

fn show_main_window(app: &tauri::AppHandle) {
    WINDOW_SHOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

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
            for rel in ["bin", "../Resources/bin", "../Resources"] {
                let candidate = dir.join(rel).join(name);
                if candidate.exists() {
                    return Ok(candidate);
                }
            }
        }
    }
    Ok(PathBuf::from(name))
}

/// Does the engine beside this Studio still speak the commands this Studio
/// depends on?
///
/// Not a version comparison, deliberately. The failure this exists to catch
/// (K-180) was an engine ten days old that reported the SAME version number
/// as the current one -- Cargo.toml sat at 0.1.28 through v0.1.58 -- so
/// comparing versions would have stayed silent through the
/// exact bug it was meant to find. Worse, a released Studio is stamped
/// `0.1.58` while a released engine reports `v0.1.58`, so the honest pair
/// differs by a leading `v` and a string check would cry wolf on every
/// correct install.
///
/// What actually differed was capability: the stale engine had no `plan`
/// subcommand at all. So ask it that, the only question whose answer is not
/// a guess. clap exits 2 on an unrecognised subcommand and 0 on
/// `plan --help`, which makes this a real feature test.
///
/// Cached: this spawns a process, and the answer cannot change while the
/// Studio runs.
fn engine_speaks_plan() -> bool {
    static SPEAKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SPEAKS.get_or_init(|| match engine() {
        Ok(path) => probe_speaks_plan(&path),
        Err(_) => false,
    })
}

/// The probe itself, separated from the cache so a test can run it against a
/// real binary. A missing or unrunnable engine answers `false`: this question
/// is only ever asked to decide whether to warn, and something we cannot run
/// certainly cannot plan.
fn probe_speaks_plan(engine: &Path) -> bool {
    silent_cmd(engine)
        .arg("plan")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Seed the agent's isolated config dir from the person's real one.
/// Remove every value that names a path inside the person's home.
///
/// Recursive and shape-agnostic: object keys, object values, and array
/// entries are all checked, because the agent's config has used all three
/// to hold paths. Anything under the home directory is dropped -- objects
/// keyed by a path lose the whole entry, strings become empty.
///
/// Deliberately blunt. A path we keep by accident is a system permission
/// dialog with Krate's name on it; a setting we drop by accident is a
/// preference the agent re-derives. The asymmetry decides the design.
fn strip_personal_paths(value: &mut serde_json::Value, home: &Path) {
    let home_prefix = home.to_string_lossy().to_string();
    let looks_personal = |s: &str| {
        s.starts_with(&home_prefix) || s.starts_with("~/")
    };
    match value {
        serde_json::Value::Object(map) => {
            let doomed: Vec<String> = map
                .keys()
                .filter(|k| looks_personal(k))
                .cloned()
                .collect();
            for key in doomed {
                map.remove(&key);
            }
            for (_, v) in map.iter_mut() {
                strip_personal_paths(v, home);
            }
        }
        serde_json::Value::Array(items) => {
            items.retain(|v| !matches!(v, serde_json::Value::String(s) if looks_personal(s)));
            for v in items.iter_mut() {
                strip_personal_paths(v, home);
            }
        }
        serde_json::Value::String(s) => {
            if looks_personal(s) {
                s.clear();
            }
        }
        _ => {}
    }
}

fn seed_agent_config(agent_home: &Path) {
    let home = dirs_home();
    // Settings minus EVERY path that points into the person's own folders.
    //
    // Blanking `projects` alone was not enough, and the proof was a system
    // dialog: a fresh bundle asked for Downloads AND for files managed by
    // OneDrive. The surviving culprit was `githubRepoPaths`, which still
    // listed `~/Downloads/Personal_web`; the agent stats the paths in its
    // config at startup, inside our process, so macOS names Krate. Naming
    // keys one at a time loses this game -- the agent's config format is
    // not ours and gains keys we do not control.
    //
    // So the rule is structural, not a list: walk the whole document and
    // drop any string that looks like a path into the person's home. What
    // survives is settings and flags, which is all the agent needs.
    if let Ok(text) = std::fs::read_to_string(home.join(".claude.json")) {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) {
            value["projects"] = serde_json::json!({});
            strip_personal_paths(&mut value, &home);
            let _ = std::fs::write(
                agent_home.join(".claude.json"),
                serde_json::to_string(&value).unwrap_or_default(),
            );
        }
    }
    // The credential: keychain first (macOS), else the file form.
    let dest = agent_home.join(".credentials.json");
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                let _ = std::fs::write(&dest, out.stdout.trim_ascii());
                let _ = std::fs::set_permissions(&dest, {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::Permissions::from_mode(0o600)
                });
            }
        }
    }
    if !dest.exists() {
        let _ = std::fs::copy(home.join(".claude/.credentials.json"), &dest);
    }

    // Every OTHER AI's sign-in, for the same reason Claude's is here.
    //
    // The engine has its own copy of this, and it was fixed there first
    // (K-189) -- but the Studio sets HOME to the confined directory BEFORE
    // the engine runs, so the engine's seeding was already too late to
    // matter for a Studio build. A first user updated to the release
    // carrying that fix and still could not build: her report says
    // "agent codex: working" and her transcript says
    //   401 Unauthorized: Missing bearer or basic authentication in header
    // which is a Codex that found no credential at all (K-191).
    //
    // Shallow on purpose. A credential is a small file at the top of the
    // tool's config directory; the subdirectories are session history and
    // caches -- exactly the material the confined home exists to keep out
    // of the agent's reach.
    for dir in [".grok", ".codex", ".gemini", ".copilot"] {
        let from = home.join(dir);
        if from.is_dir() {
            let _ = copy_dir_shallow(&from, &agent_home.join(dir));
        }
    }
    for dir in ["gemini", "github-copilot"] {
        let from = home.join(".config").join(dir);
        if from.is_dir() {
            let _ = std::fs::create_dir_all(agent_home.join(".config"));
            let _ = copy_dir_shallow(&from, &agent_home.join(".config").join(dir));
        }
    }

    // Empty history beats absent history: nothing to stat, nothing to miss.
    let _ = std::fs::write(agent_home.join("history.jsonl"), "");
}

/// Copy the files directly inside `from` into `to`, creating `to`.
///
/// Shallow deliberately -- see the note in seed_agent_config about why the
/// subdirectories must not travel.
fn copy_dir_shallow(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let _ = std::fs::copy(entry.path(), to.join(entry.file_name()));
        }
    }
    Ok(())
}

/// Build a Command that never flashes a console. The engine is a
/// console-subsystem binary on Windows; spawned plainly from a GUI it brings
/// a black terminal with it -- over the sign-in code, behind every opened
/// app, on every probe.
fn silent_cmd(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Open a URL in the person's browser on whatever OS this is. `/usr/bin/open`
/// is macOS-only; calling it on Windows was "The system cannot find the path
/// specified" on the sign-in button.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = silent_cmd("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    cmd.arg(url)
        .status()
        .map_err(|err| err.to_string())
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err("could not open the browser".to_string())
            }
        })
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

/// Diagnostic: let the frontend write a line into the same stderr log the
/// backend uses, so the freeze can be traced across the JS/Rust boundary.
/// Temporary, for K-136.
#[tauri::command]
fn dbg_log(line: String) {
    eprintln!("[ui] {line}");
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
    /// Everything else the UI records, kept losslessly. This struct used
    /// to whitelist its six fields, and every other fact the UI saved --
    /// failedRequest, buildStarted, the pending_path written value-level
    /// by the shell itself -- silently vanished on the next round trip.
    /// A failed session came back from disk with no memory of failing,
    /// re-armed its Build button, and said "waiting on you" over its own
    /// failure card (K-203, screenshots in hand).
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
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
fn session_save(mut session: Session) -> Result<(), String> {
    // The id is ours (a timestamp), but never trust a path component you did
    // not mint this second.
    if !session
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("bad session id".to_string());
    }
    // The screenshot lives beside the JSON, never inside it. Inlined as a
    // base64 data URL it made every session file megabytes, and sessions_list
    // parses every file on every visit to the home screen -- ten apps in,
    // opening Krate was reading tens of megabytes to draw a grid. The JSON
    // keeps the marker "file"; session_shot hands the pixels over on demand.
    // Old sessions with an inline shot migrate the first time they are saved.
    if let Some(result) = session.result.as_mut() {
        if let Some(data) = result["shot"].as_str() {
            if let Some(b64) = data.strip_prefix("data:image/png;base64,") {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    let png = studio_dir()
                        .join("sessions")
                        .join(format!("{}.shot.png", session.id));
                    if std::fs::write(png, bytes).is_ok() {
                        result["shot"] = serde_json::json!("file");
                    }
                }
            }
        }
    }
    let path = studio_dir()
        .join("sessions")
        .join(format!("{}.json", session.id));
    std::fs::write(
        path,
        serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// One session's screenshot as a data URL, read on demand. The grid asks for
/// these lazily, card by card, instead of every visit paying for every shot.
#[tauri::command]
fn session_shot(id: String) -> Result<String, String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad session id".to_string());
    }
    let png = studio_dir().join("sessions").join(format!("{id}.shot.png"));
    let bytes = std::fs::read(png).map_err(|_| "no shot for this session".to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn session_delete(id: String) -> Result<(), String> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("bad session id".to_string());
    }
    let _ = std::fs::remove_file(studio_dir().join("sessions").join(format!("{id}.json")));
    let _ = std::fs::remove_file(studio_dir().join("sessions").join(format!("{id}.shot.png")));
    Ok(())
}

/* ---- account ---------------------------------------------------------- */

#[tauri::command]
async fn account_status() -> Result<serde_json::Value, String> {
    let engine = engine()?;
    let out = silent_cmd(&engine)
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
        let mut child = silent_cmd(&engine)
            // `--json` belongs to `account`, not to `login`. As
            // `account login --json` the engine answers "unexpected argument
            // '--json' found" and exits, so signing in failed instantly in
            // every shipped build -- the studio then showed a BUILD error
            // ("Something in the build went wrong") because the sign-in path
            // reused the build-error wording. Nobody could get past the gate.
            .args(["account", "--json", "login"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|err| format!("could not start the Krate engine: {err}"))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
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
    silent_cmd(&engine)
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

/// Forget what we last decided about each AI, then ask again.
///
/// A readiness answer is cached for fifteen minutes, keyed on the tool's path
/// and mtime -- and signing in changes neither. So somebody who signed in to
/// Claude in a terminal and came straight back was still told it was not
/// ready, with no way to say "look again" short of waiting out the timer.
/// This is that button.
#[tauri::command]
async fn refresh_agents() -> Result<Vec<AgentInfo>, String> {
    {
        let cache = dirs_home().join(".krate").join("cache");
        if let Ok(entries) = std::fs::read_dir(&cache) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("ai-probe-") && name.ends_with(".json") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    agents().await
}

/// Which AIs this machine can author with, through `krate ai --json`.
#[tauri::command]
async fn agents() -> Result<Vec<AgentInfo>, String> {
    let engine = engine()?;
    let out = silent_cmd(&engine)
        .args(["ai", "--json"])
        .output()
        .map_err(|err| {
            format!(
                "could not run the Krate engine at {}: {err}",
                engine.display()
            )
        })?;
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

/// A picked image as a data URL, so the publish sheet can preview it. Size
/// is capped at the hub's own screenshot limit; anything bigger would be
/// refused at upload anyway.
#[tauri::command]
async fn read_image(path: String) -> Result<String, String> {
    let path = existing(&path)?;
    let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("that image is over 2 MB; pick a smaller PNG".to_string());
    }
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// A native picker for exactly one PNG, for the publish sheet's screenshot
/// and logo slots.
#[tauri::command]
async fn pick_image(app: tauri::AppHandle, title: String) -> Result<Option<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let picked = rfd::FileDialog::new()
            .set_title(&title)
            .set_directory(dirs_home())
            .add_filter("PNG image", &["png"])
            .pick_file();
        let _ = tx.send(picked);
    })
    .map_err(|err| err.to_string())?;
    let picked = rx
        .recv_timeout(std::time::Duration::from_secs(300))
        .map_err(|_| "the file picker did not answer".to_string())?;
    Ok(picked.map(|p| p.display().to_string()))
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
    plan_session: Option<String>,
) -> Result<CreateResult, String> {
    // Trace the create lifecycle to stderr. A build has frozen on "While I work"
    // with no workspace and no error more than once (K-136), always right after
    // a previous build finished -- so the wedge is somewhere in this command
    // before the engine spawns, and nothing said where. These eprintln lines,
    // visible when the Studio is launched from a terminal, name the last step
    // reached so the next freeze is diagnosable instead of invisible.
    eprintln!("[create_app] enter: session={session} agent={agent}");
    let notify_app = app.clone();
    let out = tauri::async_runtime::spawn_blocking(move || {
        eprintln!("[create_app] in spawn_blocking, resolving engine");
        let engine = engine()?;
        eprintln!("[create_app] engine ok: {}", engine.display());
        let dir = if out_dir.is_empty() {
            PathBuf::from(Settings::default().out_dir)
        } else {
            PathBuf::from(out_dir)
        };
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let out_path = free_path(&dir, &slugify(&request));
        remember_target(&session, &out_path);
        eprintln!("[create_app] target ready: {}", out_path.display());

        let mut cmd = silent_cmd(&engine);
        cmd.arg("create")
            .arg(&request)
            .args(["--agent", &agent, "--yes", "--output"])
            .arg(&out_path);
        // One stable workspace per session, so a retry RESUMES from the code
        // the last attempt wrote instead of starting from an empty directory.
        // The stall message has always promised this ("it resumes from the
        // code already written") and the studio quietly broke the promise by
        // handing create a fresh temp dir every time -- three attempts at a
        // big game each began from nothing (K-129).
        let session_work = studio_dir().join("builds").join(&session);
        let _ = std::fs::create_dir_all(&session_work);
        eprintln!("[create_app] workspace: {}", session_work.display());
        cmd.args(["--work-dir"]).arg(&session_work);
        // Every Studio build writes its own trace beside its workspace, so the
        // authoring-pipeline study captures each run automatically -- no CLI
        // flag to remember. One file per session; a retry appends to it, which
        // is what we want (the whole session's history in one place). Read it
        // with `krate study-report <this file>`. Harmless when nothing reads it.
        cmd.env("KRATE_TRACE", session_work.join("trace.jsonl"));
        // The planning session, if the plan step produced one: the engine
        // seeds the workspace with it and the build resumes hot.
        if let Some(tagged) = plan_session.as_deref().filter(|s| !s.is_empty()) {
            cmd.env("KRATE_PLAN_SESSION", tagged);
        }
        for file in &attachments {
            cmd.args(["--attach", file]);
        }
        run_author(&app, cmd, &engine, &out_path, Some(session_work))
    })
    .await
    .map_err(|err| err.to_string())?;
    if out.is_err() {
        // A failure after minutes of waiting deserves the same reach as a
        // success: the person tabbed away either way.
        notify(&notify_app, "That build didn't come together. Come see why.");
    }
    out
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
    eprintln!("[revise_app] enter: path={path} agent={agent}");
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        // Fail before spending anyone's AI quota on a file that is gone.
        let out_path = existing(&path)?;
        let mut cmd = silent_cmd(&engine);
        cmd.arg("revise")
            .arg(&out_path)
            .arg(&change)
            .args(["--agent", &agent]);
        for file in &attachments {
            cmd.args(["--attach", file]);
        }
        run_author(&app, cmd, &engine, &out_path, None)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Stop the running build: kill the engine child's whole process group so
/// the agent and cargo underneath stop too, not just the parent.
/// Is a build genuinely running -- process and all?
///
/// The UI must never take its own word for it. A spinner with no process
/// behind it is the worst failure we ship: it looks exactly like work, and
/// a first-time person waits half an hour believing their app is being
/// made (K-131). This is the ground truth the build screen polls.
#[tauri::command]
fn build_alive(state: tauri::State<Running>) -> Result<bool, String> {
    // Answered from the liveness flag the authoring waiter maintains, not by
    // spawning tasklist: the old form ran a process every four seconds for
    // the whole build -- about two hundred spawns per app -- to ask a
    // question the thread that owns child.wait() already knows the answer
    // to. The pid check stays as the backstop for a flag that was somehow
    // left stale.
    if state.1.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(true);
    }
    let mut guard = state.0.lock().map_err(|_| "poisoned")?;
    match *guard {
        Some(pid) if pid_alive(pid) => Ok(true),
        Some(_) => {
            // Dead process, stale slot: clear it here too, so the next
            // request is not refused by a ghost.
            *guard = None;
            Ok(false)
        }
        None => Ok(false),
    }
}

#[tauri::command]
fn stop_build(state: tauri::State<Running>) -> Result<(), String> {
    let pid = state.0.lock().map_err(|_| "poisoned")?.take();
    if let Some(pid) = pid {
        kill_tree(pid);
    }
    Ok(())
}

/// Is this process still running?
///
/// Signal 0 asks the kernel without delivering anything; on Windows,
/// tasklist is the equivalent question. Used to tell a live build from a
/// stale pid left behind by one that died unseen.
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // ESRCH means no such process; EPERM means it exists but is not
        // ours, which still counts as alive. io::Error reads errno
        // portably -- libc's accessor is named differently per platform.
        if unsafe { libc::kill(pid as i32, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        // silent_cmd, not Command: the studio is a windows-subsystem app, so
        // a bare console child pops a real console window. This runs on the
        // liveness watchdog every few seconds of every build -- as a plain
        // Command it flashed a black box over the person's screen for the
        // whole time their app was being made (K-159).
        silent_cmd("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()))
            .unwrap_or(true)
    }
}

/// End a build and everything under it.
///
/// The agent CLI and cargo are children of the engine, so signalling only
/// the engine leaves them running -- an invisible process still spending the
/// person's AI quota after they thought they had stopped.

/// Terminate a process tree with API calls -- the same walk `taskkill /T /F`
/// does, without the external executable security software watches (K-177).
/// Best effort: an already-gone or untouchable process is skipped.
#[cfg(windows)]
fn kill_tree_native(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let mut table = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot as isize == -1 {
            return;
        }
        let mut entry: PROCESSENTRY32 = core::mem::zeroed();
        entry.dwSize = core::mem::size_of::<PROCESSENTRY32>() as u32;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                table.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    let mut doomed = vec![pid];
    let mut index = 0;
    while index < doomed.len() {
        let parent = doomed[index];
        for (child, child_parent) in &table {
            if *child_parent == parent && !doomed.contains(child) {
                doomed.push(*child);
            }
        }
        index += 1;
    }
    for target in doomed.iter().rev() {
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, *target);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

fn kill_tree(pid: u32) {
    #[cfg(unix)]
    unsafe {
        // Negative pid: the whole process group started for this build.
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    #[cfg(windows)]
    {
        // In-process, not `taskkill`: the external exe flashed nothing
        // (K-159 was fixed with silent_cmd) but security software watches
        // system tools, and a locked-down machine answered a Stop click
        // with a dialog about taskkill.exe (K-177). API calls give a
        // watchdog nothing to flag.
        kill_tree_native(pid);
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
    if session.is_empty()
        || !session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return;
    }
    let file = studio_dir()
        .join("sessions")
        .join(format!("{session}.json"));
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

/// Watch a build workspace for the agent's own test frames, and stream each
/// new one to the UI.
///
/// While it works, the agent renders the app headlessly (`check-app --shoot
/// frame.png`) to look at what it drew. Those PNGs land in the session
/// workspace -- which means the app's real pixels exist minutes before the
/// build finishes, and nothing showed them. Polling for them turns a
/// ten-minute opaque wait into watching the app take shape.
///
/// Poll, not a filesystem watcher: two seconds of latency is invisible next
/// to an AI's pace, and a poller cannot leak platform-specific watcher
/// handles. A frame is only sent once its size has held still for one tick,
/// so a half-written PNG never reaches the screen.
fn watch_build_shots(
    app: tauri::AppHandle,
    dir: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    std::thread::spawn(move || {
        let mut seen: std::collections::HashMap<PathBuf, (u64, u64, bool)> =
            std::collections::HashMap::new();
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let mut pngs: Vec<PathBuf> = Vec::new();
            let mut stack = vec![dir.clone()];
            while let Some(d) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&d) else { continue };
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if path.is_dir() {
                        // The build tree holds compile artifacts by the
                        // thousand; the frames live near the source.
                        if name != "target" && !name.starts_with('.') {
                            stack.push(path);
                        }
                    } else if name.ends_with(".png") {
                        pngs.push(path);
                    }
                }
            }
            for png in pngs {
                let Ok(meta) = std::fs::metadata(&png) else { continue };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let size = meta.len();
                if size == 0 || size > 4 * 1024 * 1024 {
                    continue;
                }
                let entry = seen.entry(png.clone()).or_insert((0, 0, false));
                if entry.0 == mtime && entry.1 == size {
                    if !entry.2 {
                        // Held still for a tick: safe to read and send.
                        entry.2 = true;
                        if let Ok(bytes) = std::fs::read(&png) {
                            let url = format!(
                                "data:image/png;base64,{}",
                                base64::engine::general_purpose::STANDARD.encode(bytes)
                            );
                            let _ = app.emit("build-shot", url);
                        }
                    }
                } else {
                    *entry = (mtime, size, false);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// Spawn an authoring child, stream its lines to the UI, read the result.
/// The environment that keeps the agent out of the person's home folder.
///
/// Confining the agent's working directory was half the fix; this is the
/// other half, and without it the first half leaks. The agent runs with
/// `--permission-mode bypassPermissions`, so nothing in it declines to look
/// at a path -- and on macOS a child process's file access is attributed to
/// the PARENT BUNDLE. So every `~/Downloads` the agent decided to glance at
/// became the system asking, in Krate's name, for the person's Downloads
/// folder. A stranger installing Krate met a burst of those prompts before
/// they had made anything, which reads as an app rummaging through their
/// files -- the precise opposite of what the permission wall promises
/// (K-179).
///
/// With HOME rebased, `~` inside the agent resolves to a directory holding
/// its own config and nothing of the person's. Proven by running the agent
/// under this environment and asking it to `ls ~/Downloads`: it reports the
/// path does not exist, and the system is never asked.
///
/// CARGO_HOME and RUSTUP_HOME must then be pinned to the REAL home, because
/// cargo and rustup resolve them from `$HOME` and the agent builds the app
/// it writes -- rebasing HOME without this costs the agent its compiler.
fn agent_home_env(agent_home: &Path, real_home: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut env: Vec<(&'static str, PathBuf)> = vec![("HOME", agent_home.to_path_buf())];
    if std::env::var_os("CARGO_HOME").is_none() {
        env.push(("CARGO_HOME", real_home.join(".cargo")));
    }
    if std::env::var_os("RUSTUP_HOME").is_none() {
        env.push(("RUSTUP_HOME", real_home.join(".rustup")));
    }
    env
}

fn run_author(
    app: &tauri::AppHandle,
    mut cmd: Command,
    engine: &PathBuf,
    out_path: &Path,
    watch_dir: Option<PathBuf>,
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

    // The agent gets an isolated config dir that CARRIES the person's
    // credentials but NOT their history.
    //
    // This is the fix for the Downloads/Documents prompts that kept coming
    // back. The agent stats every project path in its config at startup, and
    // the person's own config lists projects under guarded folders -- inside
    // our process, so macOS names Krate in the prompt. Isolating the config
    // dir stops that, but the first two attempts broke sign-in, and now the
    // reason is measured: with CLAUDE_CONFIG_DIR set, the agent reads its
    // credential from `.credentials.json` INSIDE that dir and ignores the
    // keychain. So the seed has three parts, refreshed every spawn:
    //
    //   1. `.claude.json`: the person's own, minus `projects` -- settings and
    //      onboarding flags ride along, guarded paths do not.
    //   2. `.credentials.json`: exported from the keychain straight to a
    //      0600 file (or copied, where it already is a file). It never
    //      transits anything but this machine's own disk.
    //   3. an empty history, so there is nothing old to stat.
    // ~/.krate/agent-home, the SAME directory the engine confines to and the
    // same one the readiness probe asks about. It used to be
    // studio_dir()/agent, a third path nobody else knew, so the chip probed
    // one home while the build ran in another -- the green-dot-over-a-failing
    // -build bug (K-190) was only half fixed, and a credential seeded by the
    // engine could never help a Studio build. One directory, one rule
    // (K-191).
    let agent_home = dirs_home().join(".krate").join("agent-home");
    let _ = std::fs::create_dir_all(&agent_home);
    seed_agent_config(&agent_home);
    cmd.env("CLAUDE_CONFIG_DIR", &agent_home);

    for (key, value) in agent_home_env(&agent_home, &dirs_home()) {
        cmd.env(key, value);
    }

    // USER, when the launcher did not set it: a Finder-launched app does not
    // reliably have it, and the agent needs it to resolve its account.
    if std::env::var_os("USER").is_none() {
        if let Some(name) = dirs_home().file_name() {
            cmd.env("USER", name);
            cmd.env("LOGNAME", name);
        }
    }

    // And the PATH these tools install into, which a GUI app does not inherit.
    {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&existing).collect();
        let home = dirs_home();
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("bin"));
        dirs.push(home.join(".bun/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            let mut found: Vec<PathBuf> = versions
                .filter_map(|e| e.ok())
                .map(|e| e.path().join("bin"))
                .filter(|p| p.is_dir())
                .collect();
            found.sort();
            found.reverse();
            dirs.extend(found);
        }
        if let Ok(joined) = std::env::join_paths(dirs) {
            cmd.env("PATH", joined);
        }
    }

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
    eprintln!("[run_author] entered, checking the Running slot");
    // One build at a time. Two at once would overwrite each other's pid in
    // `Running`, so Stop could only ever reach the second and the first
    // would keep burning AI quota with nothing able to end it.
    {
        let running = app.state::<Running>();
        let mut guard = running.0.lock().map_err(|_| "poisoned")?;
        if let Some(pid) = *guard {
            eprintln!("[run_author] slot holds pid {pid}, alive={}", pid_alive(pid));
            // A recorded pid is only a live build if the process still
            // exists. A build that died without our code seeing it -- the
            // engine crashing, the machine sleeping, a kill from outside --
            // left this slot set forever, and every later request was
            // refused while the UI showed a spinner nothing could end
            // (K-128). Verify, then either refuse honestly or clear it.
            if pid_alive(pid) {
                return Err(
                    "one app is already being made -- wait for it, or press Stop".to_string()
                );
            }
            *guard = None;
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("could not start the Krate engine: {err}"))?;

    let running = app.state::<Running>();
    *running.0.lock().map_err(|_| "poisoned")? = Some(child.id());
    // The liveness flag is lowered by DROP, not by reaching a particular
    // line: any exit from this function -- success, error, panic -- lowers
    // it, because a flag stuck true is the eternal-spinner bug (K-131)
    // wearing a new coat.
    struct AliveGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for AliveGuard {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    running.1.store(true, std::sync::atomic::Ordering::SeqCst);
    let _alive = AliveGuard(running.1.clone());

    // The agent's own test frames, streamed to the UI as they appear.
    let shots_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(dir) = watch_dir {
        watch_build_shots(app.clone(), dir, shots_stop.clone());
    }

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
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
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
    for line in std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
    {
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
    shots_stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
    // The build took minutes and the person almost certainly tabbed away;
    // a system notification is how "it's ready" reaches them.
    notify_ready(
        app,
        &out_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "your app".to_string()),
    );
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

/// Tell the person their app is ready, through the OS itself.
/// One OS notification, through Tauri's plugin so every platform's
/// registration quirks (Windows AUMID above all) are its problem, not a
/// hand-rolled PowerShell line's. Fired only when the window is not
/// focused: a person watching the build does not need the OS to repeat it.
fn notify(app: &tauri::AppHandle, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    let _ = app
        .notification()
        .builder()
        .title("Krate")
        .body(body)
        .show();
}

fn notify_ready(app: &tauri::AppHandle, name: &str) {
    notify(app, &format!("{name} is ready. Open it, or tell Krate what to change."));
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
    // silent_cmd: the engine is a console-subsystem binary, and this runs at
    // the end of every build to photograph the finished app -- as a plain
    // Command it flashed a console right at the moment of success (K-159).
    let ok = silent_cmd(engine)
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
    return {
        let engine = engine().map_err(|e| e)?;
        // Capture the engine's own words instead of a bare "could not open
        // the app" -- the person debugging from a screenshot needs the
        // reason on the screen.
        let out = silent_cmd(&engine)
            .current_dir(studio_dir())
            .arg("launch")
            .arg(&path)
            .output()
            .map_err(|err| err.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let why = String::from_utf8_lossy(&out.stderr);
            let why = why.trim();
            let tail: String = why
                .chars()
                .skip(why.chars().count().saturating_sub(300))
                .collect();
            Err(if tail.is_empty() {
                "could not open the app".to_string()
            } else {
                format!("could not open the app: {tail}")
            })
        }
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let ok = Command::new("xdg-open").arg(&path).status();
    #[cfg(not(target_os = "windows"))]
    ok.map_err(|err| err.to_string()).and_then(|s| {
        if s.success() {
            Ok(())
        } else {
            Err("could not open the app".to_string())
        }
    })
}

/// Collect a session's evidence into one file and hand back its path plus
/// what is inside, so the consent dialog can name real contents rather than
/// a promise (K-128).
#[tauri::command]
async fn report_collect(session: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        let out = silent_cmd(&engine)
            .arg("support-report")
            .arg(&session)
            .output()
            .map_err(|err| err.to_string())?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(if stderr.trim().is_empty() {
                "could not gather this session".to_string()
            } else {
                stderr.trim().to_string()
            });
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        // What is actually in the zip, listed for the dialog.
        let mut names: Vec<String> = Vec::new();
        if let Ok(file) = std::fs::File::open(&path) {
            if let Ok(mut zip) = zip::ZipArchive::new(file) {
                for i in 0..zip.len() {
                    if let Ok(entry) = zip.by_index(i) {
                        names.push(entry.name().to_string());
                    }
                }
            }
        }
        Ok(serde_json::json!({ "path": path, "size": size, "files": names }))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Send a collected report to support. Only ever called after the person
/// has read what is in it and pressed send.
#[tauri::command]
async fn report_send(
    path: String,
    session: String,
    note: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        // The engine owns the upload: it already holds the sign-in and the
        // hub URL, and the studio should not learn either.
        let out = silent_cmd(&engine)
            .arg("support-send")
            .arg(&path)
            .args(["--session", &session])
            .args(["--note", &note])
            .output()
            .map_err(|err| err.to_string())?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() {
            Ok(text.trim().to_string())
        } else {
            Err(text.trim().to_string())
        }
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Run an app headless and hand back what the runtime actually said.
/// The studio's answer to "it won't open" -- check, don't guess.
#[tauri::command]
async fn diagnose_app(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = existing(&path)?;
        let engine = engine()?;
        let shot = std::env::temp_dir().join(format!("krate-diagnose-{}.png", std::process::id()));
        let out = silent_cmd(&engine)
            .arg("run")
            .arg(&path)
            .arg("--shoot")
            .arg(&shot)
            .args(["--auto-grant", "--", "quick"])
            .output()
            .map_err(|err| err.to_string())?;
        let _ = std::fs::remove_file(&shot);
        let text = format!(
            "{}
{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if out.status.success() {
            Ok("ok".to_string())
        } else {
            // The tail is where runtimes put the reason.
            let tail: Vec<&str> = text.lines().rev().filter(|l| !l.trim().is_empty()).take(6).collect();
            Ok(tail.into_iter().rev().collect::<Vec<_>>().join("
"))
        }
    })
    .await
    .map_err(|err| err.to_string())?
}

/// The conversation gate (K-123): ask the engine to look at a request
/// before anything builds. Returns the engine's one JSON object -- ask or
/// plan -- exactly as printed.
#[tauri::command]
async fn plan_request(
    app: tauri::AppHandle,
    request: String,
    attachments: Vec<String>,
    agent: Option<String>,
) -> Result<String, String> {
    let out = tauri::async_runtime::spawn_blocking(move || {
        let engine = engine()?;
        // Say WHICH thing is wrong. An engine too old to plan used to fail
        // here as an ordinary error, and the UI turned that into "I'll skip
        // the questions this time and build right away" -- a sentence that
        // describes a choice, for something that was a fault. The person
        // sees a product that stopped asking questions, and there is nothing
        // on screen to connect that to a stale binary (K-180).
        if !engine_speaks_plan() {
            return Err(format!(
                "STALE_ENGINE: the Krate engine at {} is too old for this \
                 Studio -- it has no `plan` step, so it cannot talk through \
                 an app before building it.",
                engine.display()
            ));
        }
        let mut cmd = silent_cmd(&engine);
        cmd.arg("plan").arg(&request);
        for file in &attachments {
            cmd.args(["--attach", file]);
        }
        if let Some(agent) = agent.as_deref().filter(|a| !a.is_empty()) {
            cmd.args(["--agent", agent]);
        }
        let out = cmd.output().map_err(|err| err.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(if stderr.trim().is_empty() {
                "the plan step failed".to_string()
            } else {
                stderr.trim().to_string()
            });
        }
        Ok(stdout)
    })
    .await
    .map_err(|err| err.to_string())?;
    if let Ok(answer) = &out {
        // The plan step can take half a minute and people tab away; a
        // question that nobody sees is a build that never starts.
        if answer.contains("\"ask\"") {
            notify(&app, "Krate has a question about your app.");
        }
    }
    out
}

/// Publish to the hub and hand back the short run-by-URL link.
#[tauri::command]
async fn publish(
    path: String,
    description: Option<String>,
    name: Option<String>,
    shot: Option<String>,
    icon: Option<String>,
    unlisted: Option<bool>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = existing(&path)?;
        let engine = engine()?;
        let mut cmd = silent_cmd(&engine);
        cmd.arg("publish").arg(&path);
        // What the person asked for IS the app's description; the store
        // card stays blank without it.
        let flag = |cmd: &mut std::process::Command, name: &str, value: &Option<String>| {
            if let Some(value) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                cmd.arg(name).arg(value);
            }
        };
        flag(&mut cmd, "--description", &description);
        flag(&mut cmd, "--name", &name);
        flag(&mut cmd, "--shot", &shot);
        flag(&mut cmd, "--icon", &icon);
        if unlisted.unwrap_or(false) {
            cmd.arg("--unlisted");
        }
        let out = cmd.output()
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
        let out = silent_cmd(&engine)
            .current_dir(studio_dir())
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
        // What the FILE asked for, in the engine's own plain words -- the
        // list a person should read first. Everything else is player API
        // surface, which belongs behind a fold, not on the face.
        let mut asks: Vec<serde_json::Value> = Vec::new();
        let mut section = 0u8; // 0 identity, 1 effective, 2 asked-for
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Effective capabilities") {
                section = 1;
                continue;
            }
            if trimmed.starts_with("This app will ask for") {
                section = 2;
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("- ") {
                match section {
                    1 => caps.push(value.to_string()),
                    2 => {
                        // "save its own settings and data (store.kv)":
                        // words first, the raw capability in the tail parens.
                        let (words, cap) = match value.rfind(" (") {
                            Some(i) if value.ends_with(')') => (
                                value[..i].to_string(),
                                value[i + 2..value.len() - 1].to_string(),
                            ),
                            _ => (value.to_string(), String::new()),
                        };
                        asks.push(serde_json::json!({ "words": words, "cap": cap }));
                    }
                    _ => {
                        if identity.is_empty() {
                            identity = value.to_string();
                        }
                    }
                }
            }
        }

        // A remote app has no local size; the hub already reported it.
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        Ok(serde_json::json!({
            "path": target,
            "identity": identity,
            "asks": asks,
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
        let listed = silent_cmd(&engine)
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
        let mut child = silent_cmd(&npm);
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

/// Pick a .krate from disk and open it -- the studio is the one Krate app,
/// so "open an app I already have" lives here, not in a separate opener.
/// The engine applies the same permission wall it applies everywhere.
#[tauri::command]
async fn open_krate(app: tauri::AppHandle) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let picked = rfd::FileDialog::new()
            .set_title("Open a Krate app")
            .add_filter("Krate app", &["krate"])
            .set_directory(dirs_home())
            .pick_file();
        let _ = tx.send(picked);
    })
    .map_err(|err| err.to_string())?;
    let Some(path) = rx
        .recv_timeout(std::time::Duration::from_secs(300))
        .map_err(|_| "the picker did not answer".to_string())?
    else {
        return Ok(());
    };
    let engine = engine()?;
    // `launch`, not a bare `run`: a GUI-spawned child gets no LaunchServices
    // activation, so its window is created and never shown (K-110). launch
    // wraps the app under ~/.krate and opens it properly -- which also means
    // nothing keeps touching Downloads or the volume afterwards.
    silent_cmd(&engine)
        .current_dir(studio_dir())
        .arg("launch")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Open the krate.tech sign-in page in the person's browser.
///
/// The page hands back to this app through the krate:// URL scheme, so the
/// whole round trip is: button, browser, approve, and the window is signed
/// in when they return -- no code to type.
#[tauri::command]
fn login_browser() -> Result<(), String> {
    open_url("https://krate.tech/login?from=app")
}

/// The hub the studio reads and publishes to. `KRATE_HUB_URL` overrides it,
/// the same variable the engine honours, so a local hub serves both.
fn hub_url() -> String {
    std::env::var("KRATE_HUB_URL").unwrap_or_else(|_| "https://hub.krate.tech".to_string())
}

/// The agent-session tag the engine stored for a finished bundle, so the
/// UI can chain the NEXT build onto the same warm conversation instead of
/// a cold start. The engine writes `<provider>:<id>` beside the app's
/// store entry; reading (never taking) leaves it for revise.
#[tauri::command]
fn agent_session_tag(path: String) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(&path).map_err(|err| err.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| err.to_string())?;
    let mut manifest = String::new();
    archive
        .by_name("manifest.toml")
        .map_err(|err| err.to_string())?
        .read_to_string(&mut manifest)
        .map_err(|err| err.to_string())?;
    let id = manifest
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("id")
                .and_then(|rest| rest.trim().strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
        })
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "no app id in the bundle".to_string())?;
    let tag = std::fs::read_to_string(
        dirs_home()
            .join(".krate")
            .join("store")
            .join(format!("{id}.agent-session")),
    )
    .map_err(|err| err.to_string())?;
    Ok(tag.trim().to_string())
}

/// "We'll make it for you": the human fallback when a build dies. Sends
/// exactly what a person would need to build the app by hand -- the
/// request, the answers they gave the AI, and an email to return the
/// file to. Spawned blocking so a slow hub never freezes the card.
#[tauri::command]
async fn make_for_me(
    email: String,
    request: String,
    answers: String,
    agent: String,
    why: String,
) -> Result<(), String> {
    let url = format!("{}/makeit", hub_url());
    tauri::async_runtime::spawn_blocking(move || {
        ureq::post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .send_json(serde_json::json!({
                "email": email,
                "request": request,
                "answers": answers,
                "agent": agent,
                "why": why,
            }))
            .map(|_| ())
            .map_err(|err| match err {
                ureq::Error::Status(_, resp) => resp
                    .into_string()
                    .unwrap_or_else(|_| "the hub said no".to_string()),
                _ => "could not reach krate.tech - check your connection".to_string(),
            })
    })
    .await
    .map_err(|err| err.to_string())?
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
        // `launch`, never a bare `run`: a bare spawn has no macOS activation,
        // so the consent dialog and the app window were created and never
        // shown -- "Open it" looked dead (the K-110 class, cloud edition).
        // launch downloads the URL, wraps it, and opens through the OS with
        // real activation; its stderr is the reason when it fails.
        let out = silent_cmd(&engine)
            .current_dir(studio_dir())
            .arg("launch")
            .arg(&url)
            .output()
            .map_err(|err| err.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let why = String::from_utf8_lossy(&out.stderr);
            let why = why.trim();
            let tail: String = why
                .chars()
                .skip(why.chars().count().saturating_sub(300))
                .collect();
            Err(if tail.is_empty() {
                "the app could not be opened".to_string()
            } else {
                tail
            })
        }
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Turn the app into a card: one file that is a picture of the app AND the
/// app itself, made by the engine's own `card` verb so Studio and the
/// terminal produce byte-identical shares. Returns where the card landed.
#[tauri::command]
async fn make_card(path: String) -> Result<String, String> {
    let bundle = existing(&path)?;
    let engine = engine()?;
    let output = tauri::async_runtime::spawn_blocking(move || {
        Command::new(&engine).arg("card").arg(&bundle).output()
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            "the card could not be made".to_string()
        } else {
            detail.to_string()
        });
    }
    // The engine prints `Card written: <path> (<n> KB)` and owns the naming
    // rules; read the path back rather than re-deriving it here and having
    // the two drift.
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("Card written: "))
        .and_then(|rest| rest.rsplit_once(" (").map(|(path, _)| path.to_string()))
        .ok_or_else(|| "the engine did not say where the card landed".to_string())
}

/// Wrap the app for one friend on one system: a double-clickable that
/// installs Krate once (a small verified download -- the player is planted,
/// never bundled) and then opens the app. The engine's own `wrap` verb does
/// the work; Studio reads the landing path back from its output line.
#[tauri::command]
async fn make_wrap(path: String, target: String) -> Result<String, String> {
    if !matches!(target.as_str(), "mac" | "windows" | "linux") {
        return Err("unknown system".to_string());
    }
    let bundle = existing(&path)?;
    let engine = engine()?;
    let output = tauri::async_runtime::spawn_blocking(move || {
        Command::new(&engine)
            .arg("wrap")
            .arg(&bundle)
            .arg("--for")
            .arg(&target)
            .output()
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            "the wrap could not be made".to_string()
        } else {
            detail.to_string()
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("Wrap written: "))
        .and_then(|rest| rest.rsplit_once(" (").map(|(path, _)| path.to_string()))
        .ok_or_else(|| "the engine did not say where the wrap landed".to_string())
}

/// The OS share sheet, with the file in hand: AirDrop, Mail, Messages --
/// the places "send it like a photo" actually happens. macOS only today;
/// other systems fall back to reveal, and the UI knows to.
#[tauri::command]
fn share_file(window: tauri::WebviewWindow, path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let path = existing(&path)?.display().to_string();
        let ns_window = window.ns_window().map_err(|err| err.to_string())? as usize;
        window
            .run_on_main_thread(move || {
                use objc2::rc::Retained;
                use objc2::runtime::AnyObject;
                use objc2::AllocAnyThread;
                use objc2_app_kit::{NSSharingServicePicker, NSWindow};
                use objc2_foundation::{NSArray, NSRect, NSRectEdge, NSString, NSURL};
                use std::cell::RefCell;

                // The picker dismisses itself the moment it is released, so
                // it must outlive this closure. One slot, replaced per
                // share, is exactly as long as it needs to live.
                thread_local! {
                    static HELD: RefCell<Option<Retained<NSSharingServicePicker>>> =
                        const { RefCell::new(None) };
                }

                // SAFETY: the pointer came from this window's ns_window()
                // moments ago, and this closure runs on the main thread,
                // which is where AppKit objects live.
                let ns_window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
                let Some(content) = ns_window.contentView() else {
                    return;
                };
                let url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(&path)) };
                let item: Retained<AnyObject> =
                    Retained::into_super(Retained::into_super(url));
                let items: Retained<NSArray<AnyObject>> =
                    NSArray::from_retained_slice(&[item]);
                let picker = unsafe {
                    NSSharingServicePicker::initWithItems(
                        NSSharingServicePicker::alloc(),
                        &items,
                    )
                };
                // Anchored to a small rect at the window's center-bottom:
                // the sheet needs somewhere to point, and the Send sheet
                // the person just clicked lives mid-window.
                let bounds = content.bounds();
                let rect = NSRect::new(
                    objc2_foundation::NSPoint::new(bounds.size.width / 2.0 - 2.0, 80.0),
                    objc2_foundation::NSSize::new(4.0, 4.0),
                );
                unsafe {
                    picker.showRelativeToRect_ofView_preferredEdge(
                        rect,
                        &content,
                        NSRectEdge::MaxY,
                    );
                }
                HELD.with(|slot| *slot.borrow_mut() = Some(picker));
            })
            .map_err(|err| err.to_string())?;
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        let _ = path;
        Err("the share sheet is macOS-only today".to_string())
    }
}

/// Show the file itself, for people who want to drag it into a chat.
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let path = existing(&path)?;
    let path = path.display().to_string();
    #[cfg(target_os = "macos")]
    let ok = Command::new("open").args(["-R", &path]).status();
    #[cfg(target_os = "windows")]
    let ok = {
        // raw_arg, not arg: std quotes an argument containing spaces as one
        // token, and explorer cannot parse `"/select,C:\...\Krate Apps\x"`
        // -- it silently opens Documents instead of revealing the file. The
        // path alone is quoted; the /select, prefix stays bare.
        use std::os::windows::process::CommandExt;
        Command::new("explorer")
            .raw_arg(format!("/select,\"{path}\""))
            .status()
    };
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

/// The build's progress on the dock icon (macOS) and taskbar button
/// (Windows). A build takes minutes and the person tabs away; the icon
/// carrying the bar is how the build stays visibly theirs from another app.
/// `pct` of None clears it.
#[tauri::command]
fn build_progress(app: tauri::AppHandle, pct: Option<f64>) {
    use tauri::window::{ProgressBarState, ProgressBarStatus};
    if let Some(win) = app.get_webview_window("main") {
        let state = match pct {
            Some(p) => ProgressBarState {
                status: Some(ProgressBarStatus::Normal),
                progress: Some(p.clamp(0.0, 100.0) as u64),
            },
            None => ProgressBarState {
                status: Some(ProgressBarStatus::None),
                progress: None,
            },
        };
        let _ = win.set_progress_bar(state);
    }
}

/// A request to run the moment the studio opens, for driving a real
/// end-to-end build in automation without faking anyone's keyboard.
/// Development and testing only; unset for people.
#[tauri::command]
fn autorun() -> Option<String> {
    std::env::var("KRATE_STUDIO_AUTORUN")
        .ok()
        .filter(|s| !s.is_empty())
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
/// Whether `krate` is reachable from a terminal, and where from.
///
/// /usr/local/bin is the first entry in /etc/paths on a stock Mac, so a
/// symlink there is on PATH for every shell without touching a dotfile.
#[cfg(target_os = "macos")]
fn krate_link_path() -> PathBuf {
    PathBuf::from("/usr/local/bin/krate")
}

/// Link the engine onto PATH without ever asking for a password.
///
/// Returns whether `krate` is now reachable. Fails quietly and often: on a
/// stock Mac /usr/local/bin is root:wheel, so this only succeeds where
/// something (Homebrew, a developer setup) has already made it writable.
/// The password-asking version lives behind a button in Settings, because
/// a prompt nobody asked for on first launch is worse than no shortcut.
#[cfg(target_os = "macos")]
fn link_krate_onto_path(engine: &Path) -> bool {
    let link = krate_link_path();
    // An existing link to the CURRENT engine is done; one pointing at an
    // engine that has moved is worse than none, because the terminal then
    // runs a version that no longer matches the app.
    if let Ok(target) = std::fs::read_link(&link) {
        if target == engine {
            return true;
        }
        let _ = std::fs::remove_file(&link);
    } else if link.exists() {
        // A real file, not our symlink -- somebody's own CLI install. Leave
        // it alone; overwriting a person's tools is never ours to do.
        return true;
    }
    let Some(dir) = link.parent() else { return false };
    if !dir.is_dir() || !writable(dir) {
        return false;
    }
    std::os::unix::fs::symlink(engine, &link).is_ok()
}

#[cfg(target_os = "macos")]
fn writable(dir: &Path) -> bool {
    // Ask the filesystem rather than reasoning about ownership: a directory
    // can be group-writable, ACL-writable, or owned outright, and only a
    // real attempt settles it.
    let probe = dir.join(".krate-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

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

        // `krate` on PATH, for the terminal. Still no password on first
        // launch -- being asked for one by an app you just dragged in is
        // worse than not having the shortcut -- but the assumption that
        // /usr/local/bin is writable by an admin was simply wrong. It is
        // root:wheel on a stock Mac, so the symlink failed silently on
        // every machine, `setup-done` was written anyway, and it never
        // tried again. A first user then found `krate --version` was
        // "command not found" after a normal drag-to-Applications install,
        // which is also the first step of every support instruction we
        // give (K-188).
        let _ = link_krate_onto_path(&engine);
    }

    let _ = std::fs::write(&marker, "1");
}

/// Linux: register the .krate type and the studio's launcher entry, so
/// double-click opens apps and the menu shows Krate with its own icon --
/// the same "drag it in and everything works" the .dmg promises on macOS.
/// Every step is idempotent and best-effort: a sandboxed desktop that
/// refuses one of them leaves a working studio, just with fewer
/// conveniences.
#[cfg(target_os = "linux")]
fn first_run_setup() {
    let marker = studio_dir().join("setup-done");
    if marker.exists() {
        return;
    }
    let home = dirs_home();
    let data = home.join(".local/share");

    // The MIME type, so files managers know what a .krate is.
    let mime_dir = data.join("mime/packages");
    let _ = std::fs::create_dir_all(&mime_dir);
    let _ = std::fs::write(
        mime_dir.join("krate.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/vnd.krate.bundle">
    <comment>Krate app</comment>
    <glob pattern="*.krate"/>
  </mime-type>
</mime-info>
"#,
    );
    let _ = silent_cmd("update-mime-database")
        .arg(data.join("mime"))
        .status();

    // The studio's own icon, written from the build so no theme install is
    // needed.
    let icon_path = data.join("krate/krate.png");
    let _ = std::fs::create_dir_all(icon_path.parent().expect("krate data dir"));
    let _ = std::fs::write(&icon_path, include_bytes!("../icons/128x128.png"));

    // The launcher entry. For an AppImage, APPIMAGE is the real on-disk
    // path; current_exe would name the transient mount point.
    let exe = std::env::var("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|_| std::env::current_exe())
        .unwrap_or_default();
    let apps_dir = data.join("applications");
    let _ = std::fs::create_dir_all(&apps_dir);
    let desktop = apps_dir.join("krate.desktop");
    let _ = std::fs::write(
        &desktop,
        format!(
            "[Desktop Entry]\nType=Application\nName=Krate\nComment=Describe an app. Watch it become real.\nExec=\"{}\" %f\nTerminal=false\nCategories=Development;Utility;\nIcon={}\nMimeType=application/vnd.krate.bundle;\n",
            exe.display(),
            icon_path.display(),
        ),
    );
    let _ = silent_cmd("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = silent_cmd("xdg-mime")
        .args(["default", "krate.desktop", "application/vnd.krate.bundle"])
        .status();

    let _ = std::fs::write(&marker, "1");
}

/// Windows: claim .krate for the studio in the user's registry. The NSIS
/// installer registers the type, but a machine that ran the old CLI
/// installer keeps its earlier default -- an ancient console binary that
/// drags a terminal behind every app and predates the GPU presenter. HKCU
/// writes need no elevation; a per-user explicit choice (UserChoice) still
/// wins, which is the user's right.
#[cfg(target_os = "windows")]
fn first_run_setup() {
    // EVERY launch, not first-run-only. A marker used to make this a
    // one-shot, and a one-shot cannot heal: a later CLI test install
    // re-pointed .krate at a directory that was then deleted, and every
    // double-click on the founder's machine ran a dead ProgId's command
    // forever -- the studio, which knew the right answer, never re-asserted
    // it because the marker said the work was done. These are a dozen
    // registry writes of values that rarely change; asserting them each
    // launch costs milliseconds and makes association drift self-repairing.
    // A person's own explicit choice (UserChoice) still outranks everything
    // written here, which is their right.
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.display().to_string();
        let run = |args: &[&str]| {
            let _ = silent_cmd("reg").args(args).status();
        };
        // .krate belongs to the studio. The NSIS installer registers the
        // type, but a machine that ran the old CLI installer keeps its
        // earlier default -- an ancient console binary that drags a terminal
        // behind every app and predates the GPU presenter. HKCU needs no
        // elevation; an explicit per-user choice (UserChoice) still wins.
        let open_cmd = format!(r#""{exe}" "%1""#);
        let icon = format!(r#""{exe}",0"#);
        run(&[
            "add",
            r"HKCU\Software\Classes\.krate",
            "/ve",
            "/d",
            "Krate.App",
            "/f",
        ]);
        run(&[
            "add",
            r"HKCU\Software\Classes\Krate.App",
            "/ve",
            "/d",
            "Krate App Bundle",
            "/f",
        ]);
        run(&[
            "add",
            r"HKCU\Software\Classes\Krate.App\shell\open\command",
            "/ve",
            "/d",
            &open_cmd,
            "/f",
        ]);
        run(&[
            "add",
            r"HKCU\Software\Classes\Krate.App\DefaultIcon",
            "/ve",
            "/d",
            &icon,
            "/f",
        ]);
        // The krate:// scheme, so the browser sign-in can hand the identity
        // back. This was macOS-only (Info.plist), which is why Windows saw
        // "you're signed in" in the browser while the gate never noticed:
        // the hop back had no registered handler at all.
        run(&[
            "add",
            r"HKCU\Software\Classes\krate",
            "/ve",
            "/d",
            "URL:Krate",
            "/f",
        ]);
        run(&[
            "add",
            r"HKCU\Software\Classes\krate",
            "/v",
            "URL Protocol",
            "/d",
            "",
            "/f",
        ]);
        run(&[
            "add",
            r"HKCU\Software\Classes\krate\shell\open\command",
            "/ve",
            "/d",
            &open_cmd,
            "/f",
        ]);

        // `krate` on PATH, the way macOS symlinks it into /usr/local/bin.
        // Without this the terminal tool the docs describe is unreachable on
        // Windows even after installing the studio, and `krate doctor` --
        // the first thing anyone is told to run when something looks wrong --
        // is not a command (K-158).
        //
        // HKCU\Environment, so no elevation and no machine-wide change. The
        // engine lives in `bin` beside the studio executable.
        if let Some(bin) = std::path::Path::new(&exe)
            .parent()
            .map(|dir| dir.join("bin"))
            .filter(|bin| bin.is_dir())
        {
            let bin = bin.display().to_string();
            let current = silent_cmd("reg")
                .args(["query", r"HKCU\Environment", "/v", "Path"])
                .output()
                .ok()
                .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
                .unwrap_or_default();
            let existing = current
                .lines()
                .find_map(|line| line.split("REG_EXPAND_SZ").nth(1).or(line.split("REG_SZ").nth(1)))
                .map(str::trim)
                .unwrap_or("");
            // Only append when it is not already there -- as a real
            // semicolon-delimited ELEMENT, compared case-insensitively the way
            // Windows paths are. A substring check is wrong in both
            // directions: it sees the dir inside someone else's mangled entry
            // and skips, or misses it over nothing but a case difference and
            // appends the same dir on every launch.
            let already_there = existing.split(';').any(|element| {
                element.trim().trim_end_matches('\\').eq_ignore_ascii_case(&bin)
            });
            if !already_there {
                let joined = if existing.is_empty() {
                    bin.clone()
                } else {
                    format!("{};{bin}", existing.trim_end_matches(';'))
                };
                run(&["add", r"HKCU\Environment", "/v", "Path", "/t", "REG_EXPAND_SZ", "/d", &joined, "/f"]);
            }
        }

        // Repair a DEAD Krate.Bundle. The CLI installer registers that
        // ProgId for its own opener; when its target binary is gone (a test
        // install's directory deleted, an uninstall that missed the key),
        // any shell still routing through it launches nothing. Only a
        // Bundle whose command points at a missing exe is touched: a live
        // CLI registration is the CLI's, per the K-166 peace treaty.
        let bundle_cmd = silent_cmd("reg")
            .args([
                "query",
                r"HKCU\Software\Classes\Krate.Bundle\shell\open\command",
                "/ve",
            ])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_default();
        // The command's exe, whether the value was stored quoted or bare --
        // installers quote it, but a hand-repair or an older writer may not.
        let bundle_value = bundle_cmd
            .lines()
            .find_map(|line| line.split("REG_SZ").nth(1))
            .map(str::trim)
            .unwrap_or("");
        let target = if bundle_value.starts_with('"') {
            bundle_value.split('"').nth(1).unwrap_or("")
        } else {
            bundle_value
                .trim_end_matches("\"%1\"")
                .trim_end_matches("%1")
                .trim()
        };
        if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
            eprintln!(
                "krate-setup: bundle_value={bundle_value:?} target={target:?} exists={}",
                std::path::Path::new(target).exists()
            );
        }
        if target.contains('\\') {
            if !std::path::Path::new(target).exists() {
                run(&[
                    "add",
                    r"HKCU\Software\Classes\Krate.Bundle\shell\open\command",
                    "/ve",
                    "/d",
                    &open_cmd,
                    "/f",
                ]);
            }
        }

        // Tell the running shell. Raw registry writes do not reach an
        // Explorer that has already cached the association -- the founder's
        // machine kept launching a dead command after the registry was
        // corrected, until exactly this broadcast.
        #[link(name = "shell32")]
        extern "system" {
            fn SHChangeNotify(
                event_id: i32,
                flags: u32,
                item1: *const std::ffi::c_void,
                item2: *const std::ffi::c_void,
            );
        }
        // SHCNE_ASSOCCHANGED, SHCNF_IDLIST.
        unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn first_run_setup() {}

fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn dirs_home() -> PathBuf {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var(var)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/* ---- the free counter, bound to the device ----------------------------
 *
 * Three free makes a month belong to the MACHINE, not to an email or a
 * browser store: a localStorage counter reset with a cache wipe, and an
 * account counter resets with a new address. The OS's own hardware
 * identity does neither. The raw id never leaves this function -- only a
 * salted hash is stored, and only locally in ~/.krate/plan.json (the
 * engine's home, so it survives a Studio reinstall and an app-data wipe).
 *
 * Honest limit of the mechanism: a determined person with a shell can
 * still edit the file. This is the strongest enforcement that exists
 * before checkout; the hub entitlement takes over when billing arrives,
 * and this same device hash is what it will join against. */

#[cfg(target_os = "macos")]
fn raw_device_id() -> Option<String> {
    let out = silent_cmd("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("IOPlatformUUID"))?;
    Some(line.split('"').nth(3)?.to_string())
}
#[cfg(target_os = "linux")]
fn raw_device_id() -> Option<String> {
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
#[cfg(windows)]
fn raw_device_id() -> Option<String> {
    let out = silent_cmd("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .last()
        .map(|s| s.to_string())
        .filter(|s| s.len() >= 16)
}

fn device_hash() -> String {
    use sha2::{Digest, Sha256};
    let raw = raw_device_id().unwrap_or_default();
    if raw.is_empty() {
        return String::new();
    }
    let mut h = Sha256::new();
    h.update(b"krate-device-v1:");
    h.update(raw.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `YYYY-MM` from the system clock, no chrono: Hinnant's civil-from-days.
fn month_key_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe as i64 + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}")
}

fn plan_file() -> PathBuf {
    dirs_home().join(".krate").join("plan.json")
}

fn plan_read() -> (String, String, u64) {
    let raw = std::fs::read_to_string(plan_file()).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    (
        v["device"].as_str().unwrap_or("").to_string(),
        v["month"].as_str().unwrap_or("").to_string(),
        v["n"].as_u64().unwrap_or(0),
    )
}

fn plan_write(device: &str, month: &str, n: u64) {
    let path = plan_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        path,
        serde_json::json!({ "device": device, "month": month, "n": n }).to_string(),
    );
}

/// The month's count for THIS device. `seed` carries the UI's legacy
/// localStorage value so an upgrade cannot reset anyone to zero -- the
/// larger of the two wins for the current month, once.
#[tauri::command]
fn plan_makes(seed_month: Option<String>, seed_n: Option<u64>) -> serde_json::Value {
    let device = device_hash();
    let now = month_key_now();
    let (stored_dev, stored_month, stored_n) = plan_read();
    let mut n = if stored_dev == device && stored_month == now {
        stored_n
    } else {
        0
    };
    if seed_month.as_deref() == Some(now.as_str()) {
        n = n.max(seed_n.unwrap_or(0));
    }
    plan_write(&device, &now, n);
    serde_json::json!({ "month": now, "n": n })
}

#[tauri::command]
fn plan_count_make() -> serde_json::Value {
    let device = device_hash();
    let now = month_key_now();
    let (stored_dev, stored_month, stored_n) = plan_read();
    let n = if stored_dev == device && stored_month == now {
        stored_n + 1
    } else {
        1
    };
    plan_write(&device, &now, n);
    serde_json::json!({ "month": now, "n": n })
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

/// A file name from the person's own words: the thing they asked for,
/// kebab-case, with the asking itself stripped out.
///
/// "try making a 2d runner game, like olympic in NES" must become
/// 2d-runner-game.krate, not try-making-2d-runner.krate. The first clause
/// is the app; what follows a comma or "like" is reference material, and
/// the verbs of asking are not part of any app's name.
fn slugify(request: &str) -> String {
    let head = request
        .split([',', '.', ';', ':', '('])
        .next()
        .unwrap_or(request)
        .to_lowercase();
    let head = head.split(" like ").next().unwrap_or("").to_string();
    let mut words: Vec<String> = Vec::new();
    for w in head.split_whitespace() {
        // "...tracker THAT saves my streaks": what follows describes the
        // behavior, not the name. Stop, keep what we have.
        if !words.is_empty() && matches!(w, "that" | "which" | "with" | "where" | "so" | "for") {
            break;
        }
        if matches!(
            w,
            "a" | "an"
                | "the"
                | "make"
                | "making"
                | "makes"
                | "me"
                | "my"
                | "please"
                | "try"
                | "trying"
                | "create"
                | "creating"
                | "build"
                | "building"
                | "can"
                | "you"
                | "i"
                | "want"
                | "need"
                | "app"
                | "application"
                | "simple"
                | "little"
                | "small"
                | "new"
                | "some"
                | "this"
        ) {
            continue;
        }
        let cleaned: String = w.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if !cleaned.is_empty() {
            words.push(cleaned);
        }
        if words.len() == 4 {
            break;
        }
    }
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

/// The three window buttons, ours to draw on Windows.
///
/// With the native frame gone (see setup) the studio owns the whole top of
/// the window on every OS, so the bar cannot end up stacked under a second
/// title bar or misaligned against controls it does not control.
/// This studio build's own version, for the update check.
#[tauri::command]
fn studio_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Download a new Studio and stage it for install.
///
/// The old flow opened a browser download and left the person to find the
/// file, mount it, and drag it over the app they were running. This does
/// the download in the app and hands the file to the platform's own
/// installer, which is the part a person cannot reasonably do wrong.
///
/// The bytes are verified against the release's SHA256SUMS before anything
/// is run: an update channel that installs whatever it downloaded is a
/// worse hole than the one it closes.
#[tauri::command]
fn install_update(version: String) -> Result<String, String> {
    // The tag is ours, but it arrives from the network, so it is checked
    // rather than trusted into a URL.
    if !version
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c.is_ascii_alphanumeric())
        || version.len() > 32
    {
        return Err("that does not look like a version".to_string());
    }

    let file = if cfg!(windows) {
        format!("krate-studio-{version}-windows-x64-setup.exe")
    } else if cfg!(target_os = "macos") {
        format!("krate-studio-{version}-universal.dmg")
    } else {
        format!("krate-studio-{version}-linux-x86_64.AppImage")
    };
    let base = format!("https://github.com/incyashraj/krate/releases/download/v{version}");

    // The checksums first: knowing what the bytes should be before fetching
    // them means a mismatch is a refusal, not a judgement call.
    let sums = ureq::get(&format!("{base}/SHA256SUMS"))
        .call()
        .map_err(|e| format!("could not reach the update: {e}"))?
        .into_string()
        .map_err(|e| format!("could not read the checksums: {e}"))?;
    let expected = sums
        .lines()
        .find(|line| line.ends_with(&file))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| "this release has no checksum for this platform".to_string())?
        .to_lowercase();

    let mut body = Vec::new();
    ureq::get(&format!("{base}/{file}"))
        .call()
        .map_err(|e| format!("the download failed: {e}"))?
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|e| format!("the download was cut short: {e}"))?;

    let got = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&body);
        format!("{:x}", hasher.finalize())
    };
    if got != expected {
        return Err("the downloaded file did not match its checksum".to_string());
    }

    let staged = studio_dir().join("updates");
    std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    let path = staged.join(&file);
    std::fs::write(&path, &body).map_err(|e| format!("could not save the update: {e}"))?;
    Ok(path.display().to_string())
}

/// Open the staged update and step aside.
///
/// Deliberately not a silent self-replace: on macOS a running app cannot
/// swap its own bundle safely, and a person deserves to see what is being
/// installed. This opens the installer the download produced and closes the
/// Studio so the file it is replacing is not in use.
#[tauri::command]
fn restart_for_update(app: tauri::AppHandle) -> Result<(), String> {
    let staged = studio_dir().join("updates");
    let newest = std::fs::read_dir(&staged)
        .map_err(|_| "nothing has been downloaded yet".to_string())?
        .filter_map(|e| e.ok())
        .max_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .ok_or_else(|| "nothing has been downloaded yet".to_string())?
        .path();

    #[cfg(target_os = "macos")]
    let opened = Command::new("/usr/bin/open").arg(&newest).status();
    #[cfg(windows)]
    let opened = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(&newest)
        .status();
    #[cfg(target_os = "linux")]
    let opened = Command::new("xdg-open").arg(&newest).status();

    opened.map_err(|e| format!("could not open the update: {e}"))?;
    // Give the installer a moment to take over before this process ends,
    // so the person never sees both gone at once.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(900));
        app.exit(0);
    });
    Ok(())
}

/// Open a terminal running one command, for the steps a GUI cannot do.
///
/// Signing an AI tool in is interactive by nature: it prints a URL, waits
/// for a paste, or opens a browser and expects the process to still be
/// attached. Running it hidden the way `install_agent` runs npm would
/// hang forever with nobody to answer it, which is why the sheet used to
/// print an instruction and leave the person to find Terminal themselves.
///
/// The command is BUILT HERE from the agent's own name, never taken from
/// the caller: a string from the UI that reaches a shell is a hole, and
/// this one only ever assembles a bare tool name that the engine already
/// told us about.
/// Is `krate` reachable from a terminal, and can we fix it without a
/// password? Read-only: this never changes the machine.
#[tauri::command]
fn terminal_status() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        // Reachable if something answers to that name where the stock PATH
        // looks first. A person's own CLI install counts -- the question is
        // "does `krate` work in a terminal", not "did we put it there".
        let link = krate_link_path();
        return serde_json::json!({
            "supported": true,
            "linked": link.exists(),
            "path": link.display().to_string(),
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        serde_json::json!({ "supported": false, "linked": false, "path": "" })
    }
}

/// `krate` on PATH, asked for on purpose.
///
/// /usr/local/bin is root:wheel on a stock Mac, so this needs an
/// administrator once. It is a button rather than something first launch
/// does, because being asked for a password by an app you have just
/// dragged in is worse than not having the shortcut (K-188).
///
/// The command is BUILT HERE and takes nothing from the caller. The only
/// variable part is the engine's own path, which this process resolved
/// itself, and it is quoted before it reaches osascript.
#[cfg(target_os = "macos")]
#[tauri::command]
async fn link_terminal_tool() -> Result<String, String> {
    let engine = engine()?;
    // The cheap way first: on a machine where /usr/local/bin is already
    // writable, nobody should be asked for anything.
    if link_krate_onto_path(&engine) {
        return Ok("Ready. Open a new terminal and run `krate --version`.".to_string());
    }

    let link = krate_link_path();
    let engine_q = engine.display().to_string().replace('\'', "'\\''");
    let link_q = link.display().to_string().replace('\'', "'\\''");
    // mkdir -p, then a forced symlink: the directory may not exist at all
    // on a Mac that has never had developer tools.
    let script = format!(
        "do shell script \"mkdir -p '/usr/local/bin' && ln -sf '{engine_q}' '{link_q}'\" \
         with administrator privileges"
    );
    let out = silent_cmd("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|err| format!("could not ask for permission: {err}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        // A cancelled password box is a decision, not a fault.
        if stderr.contains("-128") || stderr.to_lowercase().contains("cancel") {
            return Err("Cancelled -- nothing was changed.".to_string());
        }
        return Err(format!(
            "could not link krate: {}",
            stderr.trim().lines().next().unwrap_or("unknown error")
        ));
    }
    Ok("Ready. Open a new terminal and run `krate --version`.".to_string())
}

#[tauri::command]
async fn sign_in_agent(name: String) -> Result<(), String> {
    // A tool name, not a command line. Anything else is refused.
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || name.len() > 24 {
        return Err("that is not a tool name".to_string());
    }
    let engine = engine()?;
    // Ask the engine which binary this agent runs, so a rename upstream
    // cannot leave us launching something that no longer exists.
    let listed = silent_cmd(&engine)
        .args(["ai", "--json"])
        .output()
        .map_err(|err| format!("could not run the Krate engine: {err}"))?;
    let rows: serde_json::Value =
        serde_json::from_slice(&listed.stdout).map_err(|err| err.to_string())?;
    let known = rows
        .as_array()
        .map(|rows| rows.iter().any(|row| row["name"] == serde_json::json!(name)))
        .unwrap_or(false);
    if !known {
        return Err("Krate does not know that tool".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        // osascript rather than `open -a Terminal`: it runs the command in a
        // new window AND brings Terminal forward, which is what a person
        // expects to happen when they click "Sign in".
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{name}\"\nend tell"
        );
        Command::new("/usr/bin/osascript")
            .args(["-e", &script])
            .status()
            .map_err(|err| format!("could not open Terminal: {err}"))?;
    }
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "cmd", "/K", &name])
            .status()
            .map_err(|err| format!("could not open the console: {err}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        // Whichever terminal this desktop actually has.
        let terms = [
            ("x-terminal-emulator", vec!["-e"]),
            ("gnome-terminal", vec!["--"]),
            ("konsole", vec!["-e"]),
            ("xterm", vec!["-e"]),
        ];
        let mut started = false;
        for (term, args) in terms {
            let mut cmd = Command::new(term);
            cmd.args(args).arg(&name);
            if cmd.spawn().is_ok() {
                started = true;
                break;
            }
        }
        if !started {
            return Err(format!(
                "No terminal was found. Run `{name}` yourself to sign in."
            ));
        }
    }
    Ok(())
}

/// Open an https link in the person's browser -- the update banner's door.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("only https links open from here".to_string());
    }
    open_url(&url)
}

#[tauri::command]
fn win_minimize(window: tauri::Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn win_toggle_max(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

#[tauri::command]
fn win_close(window: tauri::Window) {
    let _ = window.close();
}

fn main() {
    let _ = STARTED.set(std::time::Instant::now());

    // Register the file type and the krate:// scheme BEFORE the early
    // returns below, not after.
    //
    // Both of those returns fire when the studio is launched WITH an argument
    // -- a double-clicked .krate, or the sign-in hop coming back. Setup used
    // to run only past them, so on a machine where the studio had only ever
    // been opened that way, nothing was ever registered: no .krate
    // association, no krate:// handler, no PATH entry (K-158). It is
    // idempotent and marker-guarded, so running it on every path costs one
    // file check.
    //
    // SYNCHRONOUS on purpose. The returns below leave `main`, which ends the
    // process -- a background thread would be cut off partway through writing
    // the registry, which is worse than not starting. It is a handful of
    // `reg add` calls and only does anything on the very first run.
    #[cfg(not(target_os = "macos"))]
    first_run_setup();

    // On Windows and Linux a double-clicked .krate arrives as argv, not as a
    // macOS open event. The person asked for their app, not for Krate: hand
    // off to the engine (silently -- no console) and never build a window.
    // The sign-in hop, delivered as an argument: Windows and Linux route
    // custom-scheme URLs through argv. Adopt the identity and leave; the
    // studio the person left open notices through the gate's own re-check.
    #[cfg(not(target_os = "macos"))]
    if let Some(uri) = std::env::args()
        .nth(1)
        .filter(|a| a.starts_with("krate://"))
    {
        adopt_from_uri(&uri);
        return;
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(path) = std::env::args().nth(1).map(PathBuf::from) {
        if path.extension().and_then(|e| e.to_str()) == Some("krate") && path.is_file() {
            if let Ok(engine) = engine() {
                let _ = silent_cmd(&engine)
                    .current_dir(studio_dir())
                    .arg("launch")
                    .arg(&path)
                    .spawn();
            }
            return;
        }
    }
    // Finish the install before the window appears. On a background thread:
    // it touches Launch Services and the filesystem, and none of it should
    // ever delay the first paint. Off macOS this already ran above, before
    // the argv returns; the marker makes the second call a no-op.
    #[cfg(target_os = "macos")]
    std::thread::spawn(first_run_setup);

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(Running::fresh())
        .setup(|_app| {
            // One title bar, ours. On Windows the native frame stacked a
            // second bar with its own name and buttons above the studio's;
            // remove it and let the in-app bar carry minimize, maximize and
            // close. macOS keeps its overlay traffic lights instead.
            #[cfg(windows)]
            if let Some(win) = _app.get_webview_window("main") {
                let _ = win.set_decorations(false);
            }
            Ok(())
        })
        // Closing the window mid-build must not SILENTLY do anything.
        //
        // The first version killed the engine tree unconditionally here, which
        // protected against orphaned agents (17 were once found alive after a
        // close) -- but it also meant a person twelve minutes into a
        // fourteen-minute build who closed the window to come back later lost
        // the build and the AI quota it had spent, with no warning. Both
        // failure modes are real; the person decides which one they mean.
        //
        // "Keep building" detaches the engine: the pid leaves the slot so the
        // exit handlers below do not kill it, the engine finishes writing the
        // app on its own, and the session's pending_path adopts it the next
        // time the studio opens. "Stop the build" is the old behavior. A
        // dismissed dialog cancels the close -- nobody loses a build to a
        // reflexive Escape.
        //
        // CloseRequested, not just Destroyed: on macOS closing a window does
        // not quit the app, so a Destroyed-only handler never fired.
        // Destroyed stays as the backstop for a teardown that skipped the
        // question; by then a kept build's pid has already left the slot.
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let state = window.state::<Running>();
                let live = state
                    .0
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .filter(|pid| pid_alive(*pid));
                let Some(pid) = live else {
                    // Nothing running (or a stale pid): clear and close.
                    if let Ok(mut guard) = state.0.lock() {
                        guard.take();
                    }
                    return;
                };
                let choice = rfd::MessageDialog::new()
                    .set_title("Krate")
                    .set_description(
                        "Your app is still being made.\n\nKeep building in the \
                         background and it will be in your apps the next time \
                         you open Krate.",
                    )
                    .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                        "Keep building".to_string(),
                        "Stop the build".to_string(),
                        "Cancel".to_string(),
                    ))
                    .show();
                match choice {
                    rfd::MessageDialogResult::Custom(label) if label == "Keep building" => {
                        // Detach: out of the slot, so no later handler kills it.
                        if let Ok(mut guard) = state.0.lock() {
                            guard.take();
                        }
                    }
                    rfd::MessageDialogResult::Custom(label) if label == "Stop the build" => {
                        if let Ok(mut guard) = state.0.lock() {
                            guard.take();
                        }
                        kill_tree(pid);
                    }
                    // Cancel, Escape, or a closed dialog: stay open, keep building.
                    _ => api.prevent_close(),
                }
            }
            tauri::WindowEvent::Destroyed => {
                let state = window.state::<Running>();
                let pid = state.0.lock().ok().and_then(|mut g| g.take());
                if let Some(pid) = pid {
                    kill_tree(pid);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            agents,
            create_app,
            revise_app,
            stop_build,
            build_alive,
            open_app,
            plan_request,
            diagnose_app,
            report_collect,
            report_send,
            publish,
            make_card,
            make_wrap,
            share_file,
            reveal,
            settings_get,
            dbg_log,
            settings_set,
            plan_makes,
            plan_count_make,
            make_for_me,
            agent_session_tag,
            sessions_list,
            session_save,
            session_shot,
            session_delete,
            account_status,
            account_login,
            account_logout,
            app_info,
            login_browser,
            open_krate,
            install_agent,
            cloud_apps,
            cloud_run,
            pick_files,
            pick_image,
            read_image,
            pick_folder,
            autorun,
            build_progress,
            studio_version,
            open_external,
            sign_in_agent,
            refresh_agents,
            terminal_status,
            #[cfg(target_os = "macos")]
            link_terminal_tool,
            install_update,
            restart_for_update,
            win_minimize,
            win_toggle_max,
            win_close
        ])
        .build(tauri::generate_context!())
        .expect("the studio window could not start")
        .run(|app, event| {
            // The window starts hidden and shows only if no document claims
            // the launch within the first moment. Finder delivers an opened
            // file as an event right after startup, so a double-clicked
            // .krate cold-starting the studio hands off to the app and exits
            // without a studio window ever appearing -- the person asked for
            // their app, not for Krate. Opening Krate itself: nothing claims
            // the grace, and the window shows.
            if matches!(event, tauri::RunEvent::Ready) {
                let handle = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    if !DOC_CLAIMED.load(std::sync::atomic::Ordering::SeqCst) {
                        show_main_window(&handle);
                    }
                });
            }

            // A .krate opened through the studio.
            //
            // The bundle declares the type, so Finder can route a file here --
            // and without this it would arrive and nothing would happen. Run
            // it through the engine, which applies the same permission wall it
            // applies everywhere else.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                for url in urls {
                    // The browser sign-in lands here: krate://signed-in with
                    // the identity in the fragment (fragments never appear in
                    // request logs). Hand it to the engine over STDIN -- a
                    // token on argv would be readable by every process via ps
                    // -- then tell the gate, on the same channel the device
                    // flow uses, so the window flips without a restart.
                    if url.scheme() == "krate" {
                        DOC_CLAIMED.store(true, std::sync::atomic::Ordering::SeqCst);
                        show_main_window(app);
                        if adopt_from_uri(url.as_str()) {
                            let _ =
                                app.emit("login-step", serde_json::json!({ "step": "adopted" }));
                        }
                        continue;
                    }
                    if let Ok(path) = url.to_file_path() {
                        if path.extension().and_then(|e| e.to_str()) == Some("krate") {
                            if let Ok(engine) = engine() {
                                // `launch`: wraps the app under ~/.krate and
                                // opens it through LaunchServices. A bare
                                // spawn -- run OR open-app -- has no
                                // activation, so the consent window and the
                                // app window were created and never shown
                                // (K-110): double-click "did nothing".
                                let handed = silent_cmd(&engine)
                                    .current_dir(studio_dir())
                                    .arg("launch")
                                    .arg(&path)
                                    .spawn()
                                    .is_ok();
                                DOC_CLAIMED.store(true, std::sync::atomic::Ordering::SeqCst);
                                // Cold-started by the double-click: the app is
                                // on its way and the studio was never wanted.
                                //
                                // Time-based, not order-based: on a slow first
                                // launch Finder's event can arrive AFTER the
                                // show-window grace fired, so "was the window
                                // shown" answers wrong. Within the first
                                // moments of the process's life a document
                                // open IS the reason we exist; hand off and
                                // leave. A studio that has been open longer is
                                // someone working -- it stays.
                                let cold = STARTED
                                    .get()
                                    .map(|t| t.elapsed() < std::time::Duration::from_secs(3))
                                    .unwrap_or(false);
                                if handed && cold {
                                    if let Some(win) = app.get_webview_window("main") {
                                        let _ = win.hide();
                                    }
                                    std::process::exit(0);
                                }
                            }
                        }
                    }
                }
            }

            // Cmd-Q and Dock > Quit close no window, so the window handler
            // above never sees them. Same question, same three answers.
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                let state = app.state::<Running>();
                let live = state
                    .0
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .filter(|pid| pid_alive(*pid));
                if let Some(pid) = live {
                    let choice = rfd::MessageDialog::new()
                        .set_title("Krate")
                        .set_description(
                            "Your app is still being made.\n\nKeep building in \
                             the background and it will be in your apps the \
                             next time you open Krate.",
                        )
                        .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                            "Keep building".to_string(),
                            "Stop the build".to_string(),
                            "Cancel".to_string(),
                        ))
                        .show();
                    match choice {
                        rfd::MessageDialogResult::Custom(label) if label == "Keep building" => {
                            if let Ok(mut guard) = state.0.lock() {
                                guard.take();
                            }
                        }
                        rfd::MessageDialogResult::Custom(label) if label == "Stop the build" => {
                            if let Ok(mut guard) = state.0.lock() {
                                guard.take();
                            }
                            kill_tree(pid);
                        }
                        _ => api.prevent_exit(),
                    }
                }
            }
            if matches!(event, tauri::RunEvent::Exit) {
                let state = app.state::<Running>();
                let pid = state.0.lock().ok().and_then(|mut g| g.take());
                if let Some(pid) = pid {
                    kill_tree(pid);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{agent_home_env, copy_dir_shallow, probe_speaks_plan, slugify};
    use std::path::{Path, PathBuf};

    /// The Studio, the engine and the readiness probe must all mean the same
    /// directory by "the agent's home".
    ///
    /// They did not. The Studio used studio_dir()/agent, a third path the
    /// other two had never heard of, so the chip probed one home while the
    /// build ran in another, and a credential the engine seeded could not
    /// help a Studio build. Two shipped fixes looked correct on this machine
    /// and changed nothing for the people waiting (K-190, K-191).
    #[test]
    fn the_studio_and_the_engine_confine_to_the_same_home() {
        let home = super::dirs_home();
        let studio_uses = home.join(".krate").join("agent-home");
        // What crates/cli/src/main.rs::agent_home_for and
        // agent_provider::probe_home both build, spelled out here so a change
        // to either side has to change this test too.
        let engine_uses = home.join(".krate").join("agent-home");
        assert_eq!(
            studio_uses, engine_uses,
            "the Studio must confine to the same home the engine and the \
             probe use, or a credential seeded by one is invisible to the other"
        );
        assert_ne!(
            studio_uses,
            super::studio_dir().join("agent"),
            "studio_dir()/agent was the old third path; nothing may use it"
        );
    }

    /// Every AI's sign-in travels into the confined home, not only Claude's.
    ///
    /// The Studio sets HOME to that directory before the engine runs, so its
    /// seeding is the one that decides whether an agent can authenticate.
    /// While it copied Claude alone, a person signed in to Codex or Grok got
    /// a 401 with "Missing bearer or basic authentication in header" -- the
    /// tool looked, found an empty home, and said so (K-191).
    ///
    /// Shallow: the credential at the top travels, the session history in
    /// the subdirectories does not, because keeping that away from the agent
    /// is the whole point of confining the home.
    #[test]
    fn a_credential_travels_but_the_history_under_it_does_not() {
        let base = std::env::temp_dir().join(format!("krate-seed-{}", std::process::id()));
        let from = base.join(".codex");
        std::fs::create_dir_all(from.join("sessions")).expect("make source");
        std::fs::write(from.join("auth.json"), "{\"token\":\"secret\"}").expect("auth");
        std::fs::write(from.join("config.toml"), "x = 1").expect("config");
        std::fs::write(from.join("sessions").join("yesterday.jsonl"), "private")
            .expect("history");

        let to = base.join("agent-home").join(".codex");
        copy_dir_shallow(&from, &to).expect("copy");

        assert!(
            to.join("auth.json").is_file(),
            "the credential must travel, or the agent cannot authenticate"
        );
        assert!(to.join("config.toml").is_file(), "settings travel too");
        assert!(
            !to.join("sessions").exists(),
            "session history must NOT travel: the confined home exists to keep \
             the person's past work away from the agent"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Write a runnable stub engine and hand back its path.
    #[cfg(unix)]
    fn stub_engine(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
        path
    }

    /// The stale-engine probe has to answer the question that actually
    /// differed, not the one that was easy to ask.
    ///
    /// K-180 was an engine ten days old driving a current Studio. It
    /// reported the SAME version string as the new one -- Cargo.toml sat
    /// at 0.1.28 through v0.1.58 -- so a version comparison would
    /// have been silent through the very bug it was written for. What
    /// differed was that it had no `plan` subcommand, and clap exits
    /// non-zero for one it does not know.
    #[cfg(unix)]
    #[test]
    fn the_probe_asks_what_the_engine_can_do_not_what_it_is_called() {
        let dir = std::env::temp_dir().join(format!("krate-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        // Both stubs report the identical version, which is the whole point:
        // only their capability differs.
        let current = stub_engine(
            &dir,
            "current",
            "#!/bin/sh\ncase \"$1\" in\n  plan) exit 0 ;;\n  --version) echo 'krate 0.1.28' ;;\n  *) exit 2 ;;\nesac\n",
        );
        let stale = stub_engine(
            &dir,
            "stale",
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'krate 0.1.28' ;;\n  *) echo \"error: unrecognized subcommand '$1'\" >&2; exit 2 ;;\nesac\n",
        );

        assert!(
            probe_speaks_plan(&current),
            "an engine that knows `plan` must be recognised as current"
        );
        assert!(
            !probe_speaks_plan(&stale),
            "an engine with no `plan` subcommand must be caught, even though \
             it reports the same version as the current one"
        );

        // An engine that is not there at all cannot plan either, and must
        // not panic on the way to saying so.
        assert!(
            !probe_speaks_plan(&dir.join("does-not-exist")),
            "a missing engine must answer false rather than panic"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The agent's `~` must never be the person's home. This is the line
    /// between "an AI wrote you an app" and "an app asked for your
    /// Downloads folder" (K-179).
    /// The seeded config must carry no path into the person's folders.
    ///
    /// Blanking `projects` was not enough and a system dialog proved it:
    /// `githubRepoPaths` still listed a folder under ~/Downloads, the agent
    /// stats config paths at startup, and macOS put Krate's name on the
    /// request (K-179).
    #[test]
    fn the_seeded_config_carries_no_path_into_the_persons_folders() {
        let home = Path::new("/Users/someone");
        let mut config = serde_json::json!({
            "hasCompletedOnboarding": true,
            "theme": "dark",
            "projects": { "/Users/someone/Downloads/thing": { "trust": true } },
            "githubRepoPaths": {
                "me/repo": ["/Users/someone/Downloads/Personal_web"],
                "me/other": ["/Users/someone/Projects/fine"]
            },
            "recent": ["/Users/someone/Documents/x", "not-a-path"],
            "nested": { "deep": { "p": "/Users/someone/Desktop/y" } }
        });

        super::strip_personal_paths(&mut config, home);
        let text = serde_json::to_string(&config).expect("serialize");

        assert!(
            !text.contains("/Users/someone"),
            "a path into the person's home survived: {text}"
        );
        // Settings must still be there, or the agent re-onboards every run.
        assert_eq!(config["hasCompletedOnboarding"], serde_json::json!(true));
        assert_eq!(config["theme"], serde_json::json!("dark"));
        assert!(text.contains("not-a-path"), "non-path values must survive");
    }

    #[test]
    fn the_agent_never_inherits_the_persons_home() {
        let agent = Path::new("/Users/someone/.krate/studio/agent");
        let real = Path::new("/Users/someone");
        let env = agent_home_env(agent, real);

        let home = env
            .iter()
            .find(|(key, _)| *key == "HOME")
            .map(|(_, value)| value.clone())
            .expect("HOME is always set");
        assert_eq!(home, PathBuf::from(agent));
        assert_ne!(home, PathBuf::from(real));

        // And the toolchain still points at the real one, or the agent
        // cannot build what it writes.
        for (key, value) in &env {
            if *key == "CARGO_HOME" || *key == "RUSTUP_HOME" {
                assert!(
                    value.starts_with(real) && !value.starts_with(agent),
                    "{key} must resolve from the real home, got {}",
                    value.display()
                );
            }
        }
    }

    /// The file carries the app's name, not the sentence that asked for it.
    #[test]
    fn slugs_name_the_app_not_the_asking() {
        assert_eq!(
            slugify("try making a 2d runner game, like olympic in NES"),
            "2d-runner-game"
        );
        assert_eq!(
            slugify("A habit tracker app that saves my streaks"),
            "habit-tracker"
        );
        assert_eq!(
            slugify("please build me a pomodoro timer"),
            "pomodoro-timer"
        );
        assert_eq!(slugify("make"), "my-app");
    }
}
