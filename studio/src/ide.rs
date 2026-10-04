//! The IDE view: a Krate app's own crate, opened as a project.
//!
//! Same rule as the rest of Studio -- every real operation is the `krate`
//! engine the terminal uses (`create`, `check-app`, `pack`, `revise`). What
//! lives here is the part only a desktop shell can do: knowing which folders
//! are projects, reading and writing their files safely, and streaming the
//! engine's lines to the window as `ide-line` events.
//!
//! Projects live in `<the output folder>/Projects`, one folder per app, with
//! the packed app beside it as `<name>.krate`. A folder opened from anywhere
//! else is remembered in `~/.krate/studio/ide-recent.json`.
//!
//! Every command that takes `(path, rel)` resolves `rel` INSIDE the project
//! or refuses: no `..`, no absolute path, no symlink that leads out. The UI
//! is ours, but the webview is the one place in Studio that renders text an
//! AI wrote, so the shell does not take its word for a path.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use base64::Engine as _;
use tauri::Emitter;

use crate::{
    agent_env, dirs_home, engine, engine_env, follow_lines, kill_tree, off_request_detail,
    open_app, pick_dir, picker_start_dir, settings_get, silent_cmd, studio_dir, unix_now,
    write_private_atomic, Settings,
};

/// Largest file the editor opens. A Krate app's lib.rs is tens of KB; the
/// generated bindings.rs is over a megabyte and is not source anyone edits.
const READ_LIMIT: u64 = 1024 * 1024;
/// Deepest path the tree lists, in components (`src/lib.rs` is 2).
const MAX_DEPTH: usize = 6;
/// Most entries the tree returns, so a folder that is not really a small
/// crate cannot flood the window.
const TREE_LIMIT: usize = 5000;
/// Largest single file copied back out of a revised bundle.
const COPY_BACK_LIMIT: u64 = 16 * 1024 * 1024;
const RECENT_FILE: &str = "ide-recent.json";
const RECENT_KEEP: usize = 30;
/// Never listed and never copied back: build output, other tools' state,
/// OS litter, and check-app's own pace stamp.
const SKIP: [&str; 5] = [
    "target",
    ".git",
    "node_modules",
    ".DS_Store",
    ".last-full-check",
];
/// What a bundle's source carries where this machine's SDK path goes
/// (krate_bundle::SDK_PLACEHOLDER).
const SDK_PLACEHOLDER: &str = "{KRATE_SDK}";
/// The starter `ide_new` makes: the built-in checklist, no AI involved.
const STARTER_KIND: &str = "checklist";
const STARTER_REQUEST: &str = "a checklist that saves locally";

/* ---- shapes the UI reads ---------------------------------------------- */

#[derive(serde::Serialize, Debug)]
pub(crate) struct ProjectEntry {
    name: String,
    path: String,
    updated: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct ProjectRef {
    name: String,
    path: String,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct TreeEntry {
    rel: String,
    dir: bool,
    size: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct BuildResult {
    ok: bool,
    stage: String,
    message: String,
    shot: Option<String>,
    size_bytes: Option<u64>,
    millis: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct PackResult {
    krate: String,
    size_bytes: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct AskResult {
    changed: Vec<String>,
}

#[derive(serde::Serialize, Clone)]
struct IdeLine {
    path: String,
    line: String,
}

/* ---- where projects live ---------------------------------------------- */

/// `<the folder Studio writes apps to>/Projects`, the same output folder
/// setting create_app uses, and its default when unset.
fn ide_root() -> PathBuf {
    let out = settings_get().out_dir;
    let base = if out.trim().is_empty() {
        PathBuf::from(Settings::default().out_dir)
    } else {
        PathBuf::from(out)
    };
    base.join("Projects")
}

fn recent_file() -> PathBuf {
    studio_dir().join(RECENT_FILE)
}

fn is_project(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() && dir.join("manifest.toml").is_file()
}

fn display_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.display().to_string())
}

/// The project at `path`, canonical, or a sentence saying why not.
fn project_dir(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("no project is open".to_string());
    }
    let canon = std::fs::canonicalize(path).map_err(|_| {
        format!(
            "{} is not there any more -- it may have been moved or deleted.",
            display_name(Path::new(path))
        )
    })?;
    if !canon.is_dir() {
        return Err(format!("{} is not a folder", display_name(&canon)));
    }
    if !is_project(&canon) {
        return Err(format!(
            "{} is not a Krate project: it needs Cargo.toml and manifest.toml side by side.",
            display_name(&canon)
        ));
    }
    Ok(canon)
}

/* ---- the path guard ---------------------------------------------------- */

const OUTSIDE: &str = "that file is outside the project";

/// `rel` as plain path segments, or a refusal. Both `/` and `\` separate,
/// on every OS, so one rule holds everywhere; `..`, a leading separator, a
/// drive letter (`C:`) and NUL are refused outright rather than resolved.
fn clean_rel(rel: &str) -> Result<PathBuf, String> {
    if rel.trim().is_empty() {
        return Err("no file named".to_string());
    }
    if rel.contains('\0') || rel.starts_with('/') || rel.starts_with('\\') {
        return Err(OUTSIDE.to_string());
    }
    let mut out = PathBuf::new();
    for segment in rel.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => return Err(OUTSIDE.to_string()),
            s if s.contains(':') => return Err(OUTSIDE.to_string()),
            s => out.push(s),
        }
    }
    if out.as_os_str().is_empty() || out.is_absolute() {
        return Err(OUTSIDE.to_string());
    }
    Ok(out)
}

/// An existing file or folder inside `project` (canonical), resolved
/// through any symlinks and still inside.
fn inside_existing(project: &Path, rel: &str) -> Result<PathBuf, String> {
    let clean = clean_rel(rel)?;
    let canon = std::fs::canonicalize(project.join(&clean))
        .map_err(|_| format!("{} is not there", clean.display()))?;
    if canon == project || !canon.starts_with(project) {
        return Err(OUTSIDE.to_string());
    }
    Ok(canon)
}

/// Where to write `rel` inside `project` (canonical). Every component that
/// already exists must resolve inside the project -- so a symlinked folder
/// cannot carry the write out -- and the rest will be made as real folders.
fn inside_for_write(project: &Path, rel: &str) -> Result<PathBuf, String> {
    let clean = clean_rel(rel)?;
    let mut cur = project.to_path_buf();
    for part in clean.components() {
        cur.push(part);
        if std::fs::symlink_metadata(&cur).is_err() {
            // Neither this nor anything under it exists yet.
            return Ok(project.join(&clean));
        }
        let canon = std::fs::canonicalize(&cur).map_err(|_| OUTSIDE.to_string())?;
        if canon == project || !canon.starts_with(project) {
            return Err(OUTSIDE.to_string());
        }
    }
    let canon = std::fs::canonicalize(&cur).map_err(|_| OUTSIDE.to_string())?;
    if canon.is_dir() {
        return Err(format!("{} is a folder", clean.display()));
    }
    Ok(canon)
}

/// Write so a reader never sees half a file: a unique sibling, flushed,
/// renamed into place. Keeps the existing file's permissions.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| "no parent folder".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("could not make the folder: {e}"))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let staging = parent.join(format!(
        ".{name}.ide-{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&staging, meta.permissions());
        }
        std::fs::rename(&staging, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&staging);
        format!("could not save {name}: {e}")
    })
}

/* ---- reading the project ----------------------------------------------- */

fn tree_of(project: &Path) -> Vec<TreeEntry> {
    fn walk(project: &Path, dir: &Path, depth: usize, out: &mut Vec<TreeEntry>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<(bool, String, PathBuf, u64)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            // Links are not followed or listed: the bundle refuses them,
            // and one leading out of the project is not the project's.
            if meta.file_type().is_symlink() {
                continue;
            }
            items.push((meta.is_dir(), name, entry.path(), meta.len()));
        }
        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });
        for (is_dir, _, path, size) in items {
            if out.len() >= TREE_LIMIT {
                return;
            }
            let Ok(rel) = path.strip_prefix(project) else {
                continue;
            };
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push(TreeEntry {
                rel,
                dir: is_dir,
                size: if is_dir { 0 } else { size },
            });
            if is_dir && depth < MAX_DEPTH {
                walk(project, &path, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(project, project, 1, &mut out);
    out
}

fn read_text(project: &Path, rel: &str) -> Result<String, String> {
    let file = inside_existing(project, rel)?;
    let meta = std::fs::metadata(&file).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err(format!("{rel} is a folder"));
    }
    if meta.len() > READ_LIMIT {
        return Err(format!(
            "{rel} is {} -- too big to open here (the limit is 1 MB).",
            crate::human_size(meta.len())
        ));
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("could not read {rel}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{rel} is not a text file"))
}

fn write_text(project: &Path, rel: &str, text: &str) -> Result<(), String> {
    let dest = inside_for_write(project, rel)?;
    write_atomic(&dest, text.as_bytes())
}

/// The newest modification time of any source file, for "is the built
/// component older than what it was built from".
fn newest_source(project: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![(project.to_path_buf(), 1usize)];
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            seen += 1;
            if seen > TREE_LIMIT {
                return newest;
            }
            if let Ok(at) = meta.modified() {
                newest = Some(newest.map_or(at, |n| n.max(at)));
            }
        }
    }
    newest
}

fn secs(at: Option<SystemTime>) -> u64 {
    at.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/* ---- the project list -------------------------------------------------- */

#[derive(serde::Serialize, serde::Deserialize)]
struct Recent {
    path: String,
    at: u64,
}

fn recent_read(file: &Path) -> Vec<Recent> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn recent_record(file: &Path, project: &Path) {
    let key = project.display().to_string();
    let mut list = recent_read(file);
    list.retain(|r| r.path != key);
    list.insert(
        0,
        Recent {
            path: key,
            at: unix_now(),
        },
    );
    list.truncate(RECENT_KEEP);
    if let Ok(body) = serde_json::to_string_pretty(&list) {
        let _ = write_private_atomic(file, body.as_bytes());
    }
}

fn projects_in(root: &Path, recent: &Path) -> Vec<ProjectEntry> {
    let mut found: HashMap<PathBuf, u64> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && is_project(&path) {
                if let Ok(canon) = std::fs::canonicalize(&path) {
                    found.entry(canon).or_insert(0);
                }
            }
        }
    }
    for r in recent_read(recent) {
        // A remembered folder that is gone, or no longer a project, is
        // skipped rather than shown as a dead row.
        let Ok(canon) = std::fs::canonicalize(&r.path) else {
            continue;
        };
        if is_project(&canon) {
            let opened = found.entry(canon).or_insert(0);
            *opened = (*opened).max(r.at);
        }
    }
    let mut out: Vec<ProjectEntry> = found
        .into_iter()
        .map(|(path, opened)| ProjectEntry {
            name: display_name(&path),
            updated: secs(newest_source(&path)).max(opened),
            path: path.display().to_string(),
        })
        .collect();
    out.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.name.cmp(&b.name)));
    out
}

/// A project folder name from what the person typed: lowercase ASCII
/// letters, digits and single dashes, never starting with a digit (Cargo
/// refuses a package name that does).
fn kebab(name: &str) -> Result<String, String> {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        return Err("Give the project a name with at least one letter or number.".to_string());
    }
    if out.len() > 40 {
        out.truncate(40);
        out = out.trim_end_matches('-').to_string();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out = format!("app-{out}");
    }
    Ok(out)
}

/* ---- one job per project, stoppable ------------------------------------ */

#[derive(Default)]
struct Job {
    pid: Option<u32>,
    stopped: bool,
}

fn jobs() -> &'static Mutex<HashMap<PathBuf, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<PathBuf, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Held for the life of one build, pack or ask on a project; dropping it
/// frees the project whatever path the command left by.
struct JobGuard(PathBuf);

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = jobs().lock() {
            map.remove(&self.0);
        }
    }
}

fn claim(project: &Path) -> Result<JobGuard, String> {
    let mut map = jobs()
        .lock()
        .map_err(|_| "Krate lost track of that project. Try again.".to_string())?;
    if map.contains_key(project) {
        return Err("this project is already building -- wait for it, or press Stop".to_string());
    }
    map.insert(project.to_path_buf(), Job::default());
    Ok(JobGuard(project.to_path_buf()))
}

fn set_pid(project: &Path, pid: Option<u32>) {
    if let Ok(mut map) = jobs().lock() {
        if let Some(job) = map.get_mut(project) {
            job.pid = pid;
        }
    }
}

fn was_stopped(project: &Path) -> bool {
    jobs()
        .lock()
        .ok()
        .and_then(|map| map.get(project).map(|j| j.stopped))
        .unwrap_or(false)
}

fn stop(project: &Path) {
    let pid = jobs().lock().ok().and_then(|mut map| {
        map.get_mut(project).and_then(|job| {
            job.stopped = true;
            job.pid.take()
        })
    });
    if let Some(pid) = pid {
        kill_tree(pid);
    }
}

/// Stop every IDE job: the window is closing, and nothing should outlive
/// it spending the person's AI quota.
pub(crate) fn stop_all() {
    let pids: Vec<u32> = jobs()
        .lock()
        .map(|mut map| {
            map.values_mut()
                .filter_map(|job| {
                    job.stopped = true;
                    job.pid.take()
                })
                .collect()
        })
        .unwrap_or_default();
    for pid in pids {
        kill_tree(pid);
    }
}

/* ---- running the engine ------------------------------------------------ */

struct Ran {
    code: Option<i32>,
    success: bool,
    out: Vec<String>,
    err: Vec<String>,
}

type Sink<'a> = &'a (dyn Fn(&str) + Sync);

/// Run one engine command for `project`, every line it prints going to
/// `sink` as it arrives. Output goes through files followed with
/// `follow_lines`, the same as run_author, so an agent grandchild that
/// keeps a pipe open can never wedge the reader. The child gets its own
/// process group so `ide_stop` ends the whole tree.
fn run_streamed(mut cmd: Command, project: &Path, sink: Sink) -> Result<Ran, String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let logs = studio_dir().join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| format!("could not start the log: {e}"))?;
    let stamp = format!(
        "ide-{}-{}-{}",
        unix_now(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let out_log = logs.join(format!("{stamp}-out.log"));
    let err_log = logs.join(format!("{stamp}-err.log"));
    let out_file =
        std::fs::File::create(&out_log).map_err(|e| format!("could not start the log: {e}"))?;
    let err_file =
        std::fs::File::create(&err_log).map_err(|e| format!("could not start the log: {e}"))?;
    cmd.stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    if was_stopped(project) {
        let _ = std::fs::remove_file(&out_log);
        let _ = std::fs::remove_file(&err_log);
        return Err("stopped".to_string());
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&out_log);
            let _ = std::fs::remove_file(&err_log);
            return Err(format!("could not start the Krate engine: {e}"));
        }
    };
    set_pid(project, Some(child.id()));
    let finished = AtomicBool::new(false);
    let (status, out, err) = std::thread::scope(|scope| {
        let out_reader = scope.spawn(|| {
            let mut lines = Vec::new();
            follow_lines(&out_log, &finished, |line| {
                sink(&line);
                lines.push(line);
            });
            lines
        });
        let err_reader = scope.spawn(|| {
            let mut lines = Vec::new();
            follow_lines(&err_log, &finished, |line| {
                sink(&line);
                lines.push(line);
            });
            lines
        });
        let status = child.wait();
        finished.store(true, Ordering::SeqCst);
        (
            status,
            out_reader.join().unwrap_or_default(),
            err_reader.join().unwrap_or_default(),
        )
    });
    set_pid(project, None);
    let _ = std::fs::remove_file(&out_log);
    let _ = std::fs::remove_file(&err_log);
    let status = status.map_err(|e| e.to_string())?;
    if was_stopped(project) {
        return Err("stopped".to_string());
    }
    Ok(Ran {
        code: status.code(),
        success: status.success(),
        out,
        err,
    })
}

/// An engine command with the environment a build needs: the confined
/// agent environment when an AI runs under it, else just USER and PATH.
///
/// CARGO_TARGET_DIR is removed. The engine builds into an explicit one but
/// then looks for the component under `<app>/target`, so a Studio started
/// from a terminal that exports it failed every build with "the build
/// produced no <name>.wasm" (measured: this module's end-to-end test, run
/// under `CARGO_TARGET_DIR=... cargo test`). Without it the engine uses its
/// own shared cache and brings the component home.
fn engine_cmd(engine: &Path, with_agent: bool) -> Command {
    let mut cmd = silent_cmd(engine);
    if with_agent {
        agent_env(&mut cmd);
    } else {
        engine_env(&mut cmd);
    }
    cmd.env_remove("CARGO_TARGET_DIR");
    cmd
}

/// The last few things the engine said, for an error the person reads.
fn tail_of(ran: &Ran, fallback: &str) -> String {
    let source = if ran.err.iter().any(|l| !l.trim().is_empty()) {
        &ran.err
    } else {
        &ran.out
    };
    let lines: Vec<&str> = source
        .iter()
        .map(String::as_str)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(12)..].join("\n");
    if tail.is_empty() {
        fallback.to_string()
    } else {
        tail
    }
}

/* ---- new project ------------------------------------------------------- */

fn new_project(engine: &Path, root: &Path, name: &str, sink: Sink) -> Result<PathBuf, String> {
    let kebab = kebab(name)?;
    std::fs::create_dir_all(root).map_err(|e| format!("could not make {}: {e}", root.display()))?;
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let dest = root.join(&kebab);
    if std::fs::symlink_metadata(&dest).is_ok() {
        return Err(format!("A project named {kebab} already exists."));
    }
    let _job = claim(&dest)?;
    // `create --work-dir D` keeps the crate at D/<name>, so it is made in a
    // private staging folder and moved into place whole: the project folder
    // never exists half-made, and holds the crate itself, not a wrapper.
    let staging = root.join(format!(
        ".new-{kebab}-{}-{}",
        std::process::id(),
        unix_now()
    ));
    let mut cmd = engine_cmd(engine, false);
    cmd.arg("create")
        .args(["--kind", STARTER_KIND, "--name", &kebab, "--work-dir"])
        .arg(&staging)
        .arg("--output")
        .arg(root.join(format!("{kebab}.krate")))
        .arg("--no-install")
        .arg("--")
        .arg(STARTER_REQUEST);
    let made = (|| {
        let ran = run_streamed(cmd, &dest, sink)?;
        if !ran.success {
            return Err(tail_of(&ran, "the starter project could not be made"));
        }
        let made = if is_project(&staging.join(&kebab)) {
            staging.join(&kebab)
        } else {
            std::fs::read_dir(&staging)
                .ok()
                .and_then(|entries| entries.flatten().map(|e| e.path()).find(|p| is_project(p)))
                .ok_or_else(|| "the engine made no project folder".to_string())?
        };
        std::fs::rename(&made, &dest).map_err(|e| format!("could not move the project: {e}"))
    })();
    let _ = std::fs::remove_dir_all(&staging);
    made?;
    Ok(dest)
}

/* ---- build, pack ------------------------------------------------------- */

/// What `check-app --json` said.
struct Verdict {
    ok: bool,
    stage: String,
    message: String,
}

fn check(
    engine: &Path,
    project: &Path,
    shoot: Option<&Path>,
    no_run: bool,
    sink: Sink,
) -> Result<Verdict, String> {
    let mut cmd = engine_cmd(engine, false);
    cmd.arg("check-app").arg(project).arg("--json");
    if let Some(png) = shoot {
        cmd.arg("--shoot").arg(png);
    }
    if no_run {
        cmd.arg("--no-run");
    }
    let ran = run_streamed(cmd, project, sink)?;
    let json = ran
        .out
        .iter()
        .rev()
        .filter(|l| l.trim_start().starts_with('{'))
        .find_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("ok").is_some());
    let Some(v) = json else {
        return Ok(Verdict {
            ok: false,
            stage: "engine".to_string(),
            message: tail_of(&ran, "the engine stopped without a verdict"),
        });
    };
    if v["ok"].as_bool() == Some(true) {
        let stages: Vec<&str> = v["stages"]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let mut message = format!("Passed: {}.", stages.join(", "));
        for key in ["usability_notes", "source_changes"] {
            for note in v[key].as_array().into_iter().flatten() {
                if let Some(note) = note.as_str() {
                    message.push_str("\nnote: ");
                    message.push_str(note);
                }
            }
        }
        return Ok(Verdict {
            ok: true,
            stage: "done".to_string(),
            message,
        });
    }
    let mut message = v["detail"].as_str().unwrap_or("").trim_end().to_string();
    let fix = v["fix"].as_str().unwrap_or("").trim_end();
    if !fix.is_empty() {
        message.push_str("\n\nFix:\n");
        message.push_str(fix);
    }
    Ok(Verdict {
        ok: false,
        stage: v["stage"].as_str().unwrap_or("check").to_string(),
        message,
    })
}

/// The built component: the manifest's `[app] entry` (where check-app
/// always brings the artifact home), else Cargo's default path for the
/// crate's name.
fn wasm_path(project: &Path) -> PathBuf {
    let manifest = std::fs::read_to_string(project.join("manifest.toml")).unwrap_or_default();
    let mut section = String::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            continue;
        }
        if section != "[app]" {
            continue;
        }
        if let Some(rest) = t.strip_prefix("entry") {
            if let Some(value) = rest.trim_start().strip_prefix('=') {
                let value = value.trim().trim_matches('"');
                if let Ok(rel) = clean_rel(value) {
                    return project.join(rel);
                }
            }
        }
    }
    let cargo = std::fs::read_to_string(project.join("Cargo.toml")).unwrap_or_default();
    let name = cargo
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("name")
                .and_then(|r| r.trim_start().strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').replace('-', "_"))
        })
        .unwrap_or_else(|| "app".to_string());
    project
        .join("target/wasm32-wasip1/release")
        .join(format!("{name}.wasm"))
}

/// Build unless the component is already newer than every source file.
fn ensure_built(engine: &Path, project: &Path, sink: Sink) -> Result<PathBuf, String> {
    let wasm = wasm_path(project);
    let built = std::fs::metadata(&wasm).and_then(|m| m.modified()).ok();
    if let (Some(built), Some(newest)) = (built, newest_source(project)) {
        if built >= newest {
            return Ok(wasm);
        }
    }
    let verdict = check(engine, project, None, true, sink)?;
    if !verdict.ok {
        return Err(format!(
            "The app does not build ({}).\n\n{}",
            verdict.stage, verdict.message
        ));
    }
    if !wasm.is_file() {
        return Err(format!(
            "the build passed but {} is not there",
            wasm.strip_prefix(project).unwrap_or(&wasm).display()
        ));
    }
    Ok(wasm)
}

fn pack_to(
    engine: &Path,
    project: &Path,
    wasm: &Path,
    out: &Path,
    sink: Sink,
) -> Result<u64, String> {
    let mut cmd = silent_cmd(engine);
    cmd.arg("pack")
        .arg("--manifest")
        .arg(project.join("manifest.toml"))
        .arg("--output")
        .arg(out)
        .arg("--")
        .arg(wasm);
    let ran = run_streamed(cmd, project, sink)?;
    if !ran.success || !out.is_file() {
        return Err(tail_of(&ran, "the app could not be packed"));
    }
    std::fs::metadata(out)
        .map(|m| m.len())
        .map_err(|e| e.to_string())
}

/// Pack to `<root>/<project folder name>.krate`, through a hidden sibling
/// so a copy that is open somewhere is replaced whole.
fn pack_project(engine: &Path, root: &Path, project: &Path, sink: Sink) -> Result<PathBuf, String> {
    let wasm = ensure_built(engine, project, sink)?;
    if was_stopped(project) {
        return Err("stopped".to_string());
    }
    std::fs::create_dir_all(root).map_err(|e| format!("could not make {}: {e}", root.display()))?;
    let name = display_name(project);
    let out = root.join(format!("{name}.krate"));
    let staging = root.join(format!(".{name}.{}.krate", std::process::id()));
    let packed = pack_to(engine, project, &wasm, &staging, sink).and_then(|_| {
        std::fs::rename(&staging, &out).map_err(|e| format!("could not save the app: {e}"))
    });
    if packed.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    packed?;
    Ok(out)
}

/* ---- ask: the AI edits the project ------------------------------------- */

/// Every file under `source/` in a bundle, as bytes. The same zip reading
/// `contents_of` does for the Files tab, bounded the same way, but whole
/// files rather than text previews: this is what gets written back.
fn source_files_of(bundle: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    const NOT_READABLE: &str = "the changed app could not be read";
    let file = std::fs::File::open(bundle).map_err(|_| NOT_READABLE.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| NOT_READABLE.to_string())?;
    let mut out = Vec::new();
    for i in 0..archive.len().min(4000) {
        let mut entry = archive.by_index(i).map_err(|_| NOT_READABLE.to_string())?;
        let name = entry.name().to_string();
        let Some(rel) = name.strip_prefix("source/") else {
            continue;
        };
        if entry.is_dir() || rel.is_empty() {
            continue;
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(COPY_BACK_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| NOT_READABLE.to_string())?;
        if bytes.len() as u64 > COPY_BACK_LIMIT {
            return Err(format!(
                "{rel} in the changed app is too large to copy back"
            ));
        }
        out.push((rel.to_string(), bytes));
    }
    Ok(out)
}

/// Where this project's Cargo.toml points for the Krate SDK: the part of
/// the `krate` dependency path before `/crates/bindings-rust`.
fn sdk_root_of(project: &Path) -> Option<String> {
    let cargo = std::fs::read_to_string(project.join("Cargo.toml")).ok()?;
    cargo.lines().find_map(|line| {
        let start = line.find("path = \"")? + "path = \"".len();
        let end = start + line[start..].find('"')?;
        let value = &line[start..end];
        let at = value.replace('\\', "/").find("/crates/bindings-rust")?;
        Some(value[..at].to_string())
    })
}

/// Copy the revised bundle's source into the project. Only files that
/// differ are written, and nothing the person has is ever deleted -- a file
/// the bundle does not carry (bindings.rs, notes, assets the pack skipped)
/// stays exactly as it was. Every entry is checked before anything is
/// written, so a bundle with one bad path changes nothing at all.
fn copy_back(project: &Path, bundle: &Path) -> Result<Vec<String>, String> {
    let sdk_root = sdk_root_of(project);
    let mut planned: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
    for (rel, mut bytes) in source_files_of(bundle)? {
        let segments: Vec<&str> = rel.split(['/', '\\']).collect();
        if segments.iter().any(|s| SKIP.contains(s)) {
            continue;
        }
        let dest = inside_for_write(project, &rel)
            .map_err(|why| format!("the changed app carries {rel}: {why}; nothing was copied"))?;
        // The bundle writes {KRATE_SDK} where the SDK path goes, so its
        // source rebuilds on any machine. Put this project's path back.
        if bytes
            .windows(SDK_PLACEHOLDER.len())
            .any(|w| w == SDK_PLACEHOLDER.as_bytes())
        {
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("{rel} in the changed app is not text"))?;
            let root = sdk_root.as_deref().ok_or_else(|| {
                format!(
                    "{rel} in the changed app points at the Krate SDK, and this project's \
                     Cargo.toml does not say where the SDK is; nothing was copied"
                )
            })?;
            bytes = text.replace(SDK_PLACEHOLDER, root).into_bytes();
        }
        if std::fs::read(&dest).ok().as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        planned.push((rel, dest, bytes));
    }
    let mut changed = Vec::new();
    for (rel, dest, bytes) in planned {
        write_atomic(&dest, &bytes)?;
        changed.push(rel.replace('\\', "/"));
    }
    changed.sort();
    Ok(changed)
}

fn valid_agent(agent: &str) -> bool {
    !agent.is_empty()
        && !agent.starts_with('-')
        && agent
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn ask(
    engine: &Path,
    project: &Path,
    request: &str,
    agent: &str,
    sink: Sink,
) -> Result<Vec<String>, String> {
    if request.trim().is_empty() {
        return Err("Say what to change.".to_string());
    }
    if !valid_agent(agent) {
        return Err(format!("{agent} is not an AI Krate knows"));
    }
    // A project that does not build right now is exactly when somebody asks
    // the AI for help, so the last good component is packed beside the
    // current source; revise works on the source and builds it itself.
    let wasm = match ensure_built(engine, project, sink) {
        Ok(wasm) => wasm,
        Err(why) => {
            let last = wasm_path(project);
            if why == "stopped" || !last.is_file() {
                return Err(why);
            }
            sink("==> the project does not build yet; asking the AI with its current source");
            last
        }
    };
    let work =
        studio_dir()
            .join("work")
            .join(format!("ide-ask-{}-{}", std::process::id(), unix_now()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let before = work.join("before.krate");
        let after = work.join("after.krate");
        pack_to(engine, project, &wasm, &before, sink)?;
        let mut cmd = engine_cmd(engine, true);
        cmd.arg("revise")
            .args(["--agent", agent, "--output"])
            .arg(&after)
            .arg("--")
            .arg(&before)
            .arg(request);
        let ran = run_streamed(cmd, project, sink)?;
        // Exit 6: the AI's change did not do what was asked, and revise
        // kept the app it started from (see is_off_request).
        if ran.code == Some(6) {
            let mut lines = ran.out.clone();
            lines.extend(ran.err.iter().cloned());
            return Err(off_request_detail(&lines).unwrap_or_else(|| {
                "The change did not do what you asked, so your project was left as it was."
                    .to_string()
            }));
        }
        if !ran.success {
            return Err(tail_of(&ran, "the change could not be made"));
        }
        if !after.is_file() {
            return Err("the AI finished but no changed app was written".to_string());
        }
        copy_back(project, &after)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/* ---- the commands ------------------------------------------------------ */

fn line_sink(app: tauri::AppHandle, project: &Path) -> impl Fn(&str) + Sync {
    let path = project.display().to_string();
    move |line: &str| {
        let _ = app.emit(
            "ide-line",
            IdeLine {
                path: path.clone(),
                line: line.to_string(),
            },
        );
    }
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn ide_projects() -> Result<Vec<ProjectEntry>, String> {
    blocking(|| Ok(projects_in(&ide_root(), &recent_file()))).await
}

#[tauri::command]
pub(crate) async fn ide_new(app: tauri::AppHandle, name: String) -> Result<ProjectRef, String> {
    blocking(move || {
        let engine = engine()?;
        let root = ide_root();
        let target = root.join(kebab(&name)?);
        let sink = line_sink(app, &target);
        let made = new_project(&engine, &root, &name, &sink)?;
        recent_record(&recent_file(), &made);
        Ok(ProjectRef {
            name: display_name(&made),
            path: made.display().to_string(),
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ide_open_folder(app: tauri::AppHandle) -> Result<Option<ProjectRef>, String> {
    let root = ide_root();
    let start = if root.is_dir() {
        root
    } else {
        picker_start_dir().unwrap_or_else(dirs_home)
    };
    let Some(picked) = pick_dir(&app, "Open a Krate project", start)? else {
        return Ok(None);
    };
    let project = std::fs::canonicalize(&picked).map_err(|e| e.to_string())?;
    if !is_project(&project) {
        return Err(format!(
            "{} is not a Krate project. Pick the folder that holds Cargo.toml and \
             manifest.toml side by side.",
            display_name(&project)
        ));
    }
    recent_record(&recent_file(), &project);
    Ok(Some(ProjectRef {
        name: display_name(&project),
        path: project.display().to_string(),
    }))
}

#[tauri::command]
pub(crate) async fn ide_tree(path: String) -> Result<Vec<TreeEntry>, String> {
    blocking(move || Ok(tree_of(&project_dir(&path)?))).await
}

#[tauri::command]
pub(crate) async fn ide_read(path: String, rel: String) -> Result<String, String> {
    blocking(move || read_text(&project_dir(&path)?, &rel)).await
}

#[tauri::command]
pub(crate) async fn ide_write(path: String, rel: String, text: String) -> Result<(), String> {
    blocking(move || write_text(&project_dir(&path)?, &rel, &text)).await
}

#[tauri::command]
pub(crate) async fn ide_build(app: tauri::AppHandle, path: String) -> Result<BuildResult, String> {
    blocking(move || {
        let project = project_dir(&path)?;
        let _job = claim(&project)?;
        let engine = engine()?;
        build(&engine, &project, &line_sink(app, &project))
    })
    .await
}

fn build(engine: &Path, project: &Path, sink: Sink) -> Result<BuildResult, String> {
    let started = Instant::now();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let png = std::env::temp_dir().join(format!(
        "krate-ide-shot-{}-{}.png",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&png);
    let verdict = check(engine, project, Some(&png), false, sink);
    let shot = std::fs::read(&png).ok().map(|bytes| {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    });
    let _ = std::fs::remove_file(&png);
    let verdict = verdict?;
    let size_bytes = if verdict.ok {
        std::fs::metadata(wasm_path(project)).ok().map(|m| m.len())
    } else {
        None
    };
    Ok(BuildResult {
        ok: verdict.ok,
        stage: verdict.stage,
        message: verdict.message,
        shot,
        size_bytes,
        millis: started.elapsed().as_millis() as u64,
    })
}

#[tauri::command]
pub(crate) async fn ide_pack(app: tauri::AppHandle, path: String) -> Result<PackResult, String> {
    blocking(move || {
        let project = project_dir(&path)?;
        let _job = claim(&project)?;
        let engine = engine()?;
        let out = pack_project(&engine, &ide_root(), &project, &line_sink(app, &project))?;
        Ok(PackResult {
            size_bytes: std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
            krate: out.display().to_string(),
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn ide_run(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let krate = blocking(move || {
        let project = project_dir(&path)?;
        let _job = claim(&project)?;
        let engine = engine()?;
        pack_project(&engine, &ide_root(), &project, &line_sink(app, &project))
    })
    .await?;
    open_app(krate.display().to_string())
}

#[tauri::command]
pub(crate) async fn ide_ask(
    app: tauri::AppHandle,
    path: String,
    request: String,
    agent: Option<String>,
) -> Result<AskResult, String> {
    blocking(move || {
        let project = project_dir(&path)?;
        let _job = claim(&project)?;
        let engine = engine()?;
        let agent = agent
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| settings_get().agent);
        let agent = if agent.trim().is_empty() {
            "claude".to_string()
        } else {
            agent
        };
        let changed = ask(
            &engine,
            &project,
            &request,
            &agent,
            &line_sink(app, &project),
        )?;
        Ok(AskResult { changed })
    })
    .await
}

/// Stop the build, pack or ask running on this project. Answers Ok when
/// nothing was running: the person's intent -- nothing running -- holds.
#[tauri::command]
pub(crate) fn ide_stop(path: String) -> Result<(), String> {
    let project = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
    stop(&project);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn project(root: &Path) -> PathBuf {
        let p = root.join("proj");
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::write(
            p.join("Cargo.toml"),
            "[package]\nname = \"proj\"\n\n[dependencies]\nkrate = { path = \"/sdk/root/crates/bindings-rust\" }\n",
        )
        .unwrap();
        std::fs::write(p.join("manifest.toml"), "[app]\nid = \"x\"\n").unwrap();
        std::fs::write(p.join("src/lib.rs"), "// hi\n").unwrap();
        std::fs::canonicalize(p).unwrap()
    }

    #[test]
    fn a_rel_path_cannot_climb_or_start_from_the_root() {
        for bad in [
            "../x",
            "src/../../x",
            "/etc/passwd",
            "\\windows\\x",
            "C:\\x",
            "C:x",
            "a\\..\\..\\x",
            "..",
            "",
            "src/\0x",
        ] {
            assert!(clean_rel(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(
            clean_rel("src/./lib.rs").unwrap(),
            PathBuf::from("src/lib.rs")
        );
        assert_eq!(
            clean_rel("src\\lib.rs").unwrap(),
            PathBuf::from("src/lib.rs")
        );
    }

    #[test]
    fn reads_and_writes_stay_inside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, "secret").unwrap();

        assert!(read_text(&p, "../secret.txt").is_err());
        assert!(read_text(&p, &outside.display().to_string()).is_err());
        assert!(write_text(&p, "../secret.txt", "x").is_err());
        assert!(write_text(&p, &outside.display().to_string(), "x").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");

        write_text(&p, "src/new/deep.rs", "fn x() {}\n").unwrap();
        assert_eq!(read_text(&p, "src/new/deep.rs").unwrap(), "fn x() {}\n");
        assert!(
            write_text(&p, "src", "x").is_err(),
            "a folder is not a file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_lead_a_read_or_a_write_out() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        let away = tmp.path().join("away");
        std::fs::create_dir_all(&away).unwrap();
        std::fs::write(away.join("file.txt"), "away").unwrap();
        std::os::unix::fs::symlink(&away, p.join("linkdir")).unwrap();
        std::os::unix::fs::symlink(away.join("file.txt"), p.join("linkfile")).unwrap();

        assert!(read_text(&p, "linkdir/file.txt").is_err());
        assert!(read_text(&p, "linkfile").is_err());
        assert!(write_text(&p, "linkdir/file.txt", "x").is_err());
        assert!(write_text(&p, "linkdir/new.txt", "x").is_err());
        assert!(write_text(&p, "linkfile", "x").is_err());
        assert_eq!(
            std::fs::read_to_string(away.join("file.txt")).unwrap(),
            "away"
        );
        assert!(!away.join("new.txt").exists());
        // And the tree never lists them.
        assert!(tree_of(&p).iter().all(|e| !e.rel.starts_with("link")));
    }

    #[test]
    fn a_folder_without_both_files_is_not_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        assert!(project_dir(&p.display().to_string()).is_ok());
        std::fs::remove_file(p.join("manifest.toml")).unwrap();
        assert!(project_dir(&p.display().to_string()).is_err());
        assert!(project_dir(&tmp.path().join("nope").display().to_string()).is_err());
    }

    #[test]
    fn reads_refuse_binary_and_huge_files() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        std::fs::write(p.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        std::fs::write(p.join("big.rs"), vec![b'a'; READ_LIMIT as usize + 1]).unwrap();
        assert!(read_text(&p, "bin.dat")
            .unwrap_err()
            .contains("not a text file"));
        assert!(read_text(&p, "big.rs").unwrap_err().contains("too big"));
    }

    #[test]
    fn the_tree_is_dirs_first_and_skips_build_output() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        for skipped in ["target/x", ".git/HEAD", "node_modules/a"] {
            std::fs::create_dir_all(p.join(skipped).parent().unwrap()).unwrap();
            std::fs::write(p.join(skipped), "x").unwrap();
        }
        std::fs::write(p.join(".DS_Store"), "x").unwrap();
        let mut deep = p.clone();
        for i in 0..8 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        let rels: Vec<String> = tree_of(&p).into_iter().map(|e| e.rel).collect();
        assert!(rels.iter().all(|r| !r.starts_with("target")
            && !r.starts_with(".git")
            && !r.starts_with("node_modules")
            && r != ".DS_Store"));
        assert!(rels.iter().all(|r| r.split('/').count() <= MAX_DEPTH));
        let first_file = rels.iter().position(|r| r == "Cargo.toml").unwrap();
        let src = rels.iter().position(|r| r == "src").unwrap();
        assert!(src < first_file, "dirs come first: {rels:?}");
        assert_eq!(rels[src + 1], "src/lib.rs", "children follow their dir");
    }

    #[test]
    fn names_become_cargo_safe_folders() {
        assert_eq!(kebab("My Notes!").unwrap(), "my-notes");
        assert_eq!(kebab("  2048 game ").unwrap(), "app-2048-game");
        assert!(kebab("!!!").is_err());
    }

    #[test]
    fn recent_projects_list_newest_first_and_skip_the_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Projects");
        std::fs::create_dir_all(&root).unwrap();
        let inside = project(&root);
        let elsewhere_parent = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere_parent).unwrap();
        let elsewhere = project(&elsewhere_parent);
        let recent = tmp.path().join("recent.json");
        recent_record(&recent, &tmp.path().join("gone"));
        recent_record(&recent, &elsewhere);
        let list = projects_in(&root, &recent);
        let paths: Vec<&str> = list.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(list.len(), 2, "{paths:?}");
        assert!(paths.contains(&inside.display().to_string().as_str()));
        assert!(paths.contains(&elsewhere.display().to_string().as_str()));
        assert!(list[0].updated >= list[1].updated);
    }

    fn zip_with(path: &Path, files: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn copy_back_writes_changes_restores_the_sdk_and_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        std::fs::write(p.join("NOTES.md"), "mine").unwrap();
        let cargo_now = std::fs::read_to_string(p.join("Cargo.toml")).unwrap();
        let bundle = tmp.path().join("after.krate");
        zip_with(
            &bundle,
            &[
                ("manifest.toml", b"[app]\n"),
                ("code.wasm", b"\0asm"),
                (
                    "source/Cargo.toml",
                    cargo_now.replace("/sdk/root", SDK_PLACEHOLDER).as_bytes(),
                ),
                ("source/manifest.toml", b"[app]\nid = \"x\"\n"),
                ("source/src/lib.rs", b"// changed\n"),
                ("source/src/extra.rs", b"// new\n"),
                ("source/.last-full-check", b""),
            ],
        );
        let changed = copy_back(&p, &bundle).unwrap();
        assert_eq!(changed, vec!["src/extra.rs", "src/lib.rs"]);
        assert_eq!(
            std::fs::read_to_string(p.join("Cargo.toml")).unwrap(),
            cargo_now,
            "the placeholder round-trips to this project's SDK path"
        );
        assert_eq!(std::fs::read_to_string(p.join("NOTES.md")).unwrap(), "mine");
        assert!(!p.join(".last-full-check").exists());
    }

    #[test]
    fn copy_back_refuses_a_bundle_with_a_path_out_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        let bundle = tmp.path().join("evil.krate");
        zip_with(
            &bundle,
            &[
                ("source/src/lib.rs", b"// changed\n"),
                ("source/../../evil.rs", b"evil"),
            ],
        );
        assert!(copy_back(&p, &bundle).is_err());
        assert_eq!(
            std::fs::read_to_string(p.join("src/lib.rs")).unwrap(),
            "// hi\n"
        );
        assert!(!tmp.path().join("evil.rs").exists());
    }

    #[test]
    fn one_job_per_project() {
        let tmp = tempfile::tempdir().unwrap();
        let p = project(tmp.path());
        let held = claim(&p).unwrap();
        assert!(claim(&p).is_err());
        drop(held);
        assert!(claim(&p).is_ok());
    }

    fn bundled_engine() -> Option<PathBuf> {
        if let Ok(explicit) = std::env::var("KRATE_STUDIO_ENGINE") {
            return Some(PathBuf::from(explicit));
        }
        let name = if cfg!(windows) { "krate.exe" } else { "krate" };
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin").join(name);
        p.is_file().then_some(p)
    }

    /// The whole loop against the real engine: new, tree, read, write,
    /// build with a preview, pack, and the copy-back of a revised bundle
    /// made from a real pack. Ignored by default: it needs studio/bin/krate
    /// (per machine, not checked in) and a Rust toolchain, and takes about
    /// half a minute. `cargo test -- --ignored ide_end_to_end`.
    #[test]
    #[ignore]
    fn ide_end_to_end_with_the_bundled_engine() {
        let engine = bundled_engine().expect("studio/bin/krate or KRATE_STUDIO_ENGINE");
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Projects");
        let lines = Mutex::new(Vec::<String>::new());
        let sink = |l: &str| lines.lock().unwrap().push(l.to_string());

        let made = new_project(&engine, &root, "Shop List", &sink).unwrap();
        assert_eq!(display_name(&made), "shop-list");
        assert!(is_project(&made));
        assert!(new_project(&engine, &root, "shop list", &sink)
            .unwrap_err()
            .contains("already exists"));
        let p = project_dir(&made.display().to_string()).unwrap();

        let tree = tree_of(&p);
        let rels: Vec<&str> = tree.iter().map(|e| e.rel.as_str()).collect();
        assert!(rels.contains(&"src/lib.rs"), "{rels:?}");
        assert!(rels.iter().all(|r| !r.starts_with("target")));

        let lib = read_text(&p, "src/lib.rs").unwrap();
        assert!(lib.contains("no_std"));
        write_text(&p, "src/lib.rs", &format!("{lib}\n// edited in the IDE\n")).unwrap();

        let built = build(&engine, &p, &sink).unwrap();
        assert!(built.ok, "{} {}", built.stage, built.message);
        let shot = built.shot.expect("a preview frame");
        assert!(shot.starts_with("data:image/png;base64,"));
        assert!(built.size_bytes.unwrap_or(0) > 0);

        let krate = pack_project(&engine, &root, &p, &sink).unwrap();
        assert_eq!(krate, root.join("shop-list.krate"));
        assert!(std::fs::metadata(&krate).unwrap().len() > 0);

        // A fabricated revise: the real pack, with lib.rs changed the way
        // an AI would. Cargo.toml carries {KRATE_SDK} and must come back
        // byte-identical; only lib.rs is reported.
        let revised = tmp.path().join("revised.krate");
        {
            let mut archive = zip::ZipArchive::new(std::fs::File::open(&krate).unwrap()).unwrap();
            let mut files: Vec<(String, Vec<u8>)> = Vec::new();
            for i in 0..archive.len() {
                let mut e = archive.by_index(i).unwrap();
                let mut b = Vec::new();
                e.read_to_end(&mut b).unwrap();
                if e.name() == "source/src/lib.rs" {
                    b.extend_from_slice(b"// changed by the AI\n");
                }
                files.push((e.name().to_string(), b));
            }
            let cargo = files
                .iter()
                .find(|(n, _)| n == "source/Cargo.toml")
                .unwrap();
            assert!(String::from_utf8_lossy(&cargo.1).contains(SDK_PLACEHOLDER));
            let refs: Vec<(&str, &[u8])> = files
                .iter()
                .map(|(n, b)| (n.as_str(), b.as_slice()))
                .collect();
            zip_with(&revised, &refs);
        }
        let changed = copy_back(&p, &revised).unwrap();
        assert_eq!(changed, vec!["src/lib.rs"]);
        assert!(read_text(&p, "src/lib.rs")
            .unwrap()
            .ends_with("// changed by the AI\n"));
        // And the result still builds.
        let wasm = ensure_built(&engine, &p, &sink).unwrap();
        assert!(wasm.is_file());
        eprintln!(
            "ide e2e: {} lines streamed, build {} ms, pack {} bytes",
            lines.lock().unwrap().len(),
            built.millis,
            std::fs::metadata(&krate).unwrap().len()
        );
    }
}
