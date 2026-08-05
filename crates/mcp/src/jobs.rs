//! Build jobs: the async shape a slow tool needs.
//!
//! A Krate build takes two to five minutes. An MCP tool call that blocks that
//! long hits the client's request timeout, and the client cancels a build that
//! is going fine. So a build is a *job*: `krate_start_build` spawns it and
//! returns an id straight away, and `krate_build_status` reports where it got
//! to. The model polls, and can narrate real progress while it waits.
//!
//! The store lives for one server run, which matches how a stdio server is
//! used: the client launches the process, talks to it, and kills it. Nothing is
//! persisted, because a job id that survives a restart would point at a build
//! whose thread is gone.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a job is. Deliberately coarse: these are the words a model can
/// truthfully repeat to a person waiting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// The authoring process is running: writing code, building, checking.
    Running,
    /// Finished, and a `.krate` exists.
    Succeeded,
    /// Finished, and it did not work.
    Failed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Running => "running",
            Phase::Succeeded => "succeeded",
            Phase::Failed => "failed",
        }
    }
}

/// Everything known about one build. Cloned out under the lock so a status read
/// never holds the mutex while it serializes.
#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub description: String,
    pub name: String,
    pub phase: Phase,
    /// The most recent human-readable progress line, e.g. "building the app".
    pub progress: String,
    /// Every progress line so far, so a model that polls slowly still sees what
    /// happened rather than only the last thing.
    pub log: Vec<String>,
    /// The finished bundle, once there is one.
    pub output: Option<PathBuf>,
    /// The working directory the app was authored in. Kept for the whole server
    /// run so `krate_check` and `krate_run` have real source to work against.
    pub work_dir: PathBuf,
    /// The app directory inside the work dir (work_dir/<name>).
    pub app_dir: PathBuf,
    /// Why it failed, in words a model can act on.
    pub error: Option<String>,
    /// The `krate.author.v1` transcript, on success.
    pub transcript: Option<serde_json::Value>,
    pub started: Instant,
    pub finished: Option<Instant>,
}

impl Job {
    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(Instant::now) - self.started
    }
}

/// The set of jobs for one server run.
#[derive(Clone, Default)]
pub struct JobStore {
    inner: Arc<Mutex<HashMap<String, Job>>>,
    next: Arc<AtomicU64>,
}

/// How a build is driven. The AI path shells out to a coding agent and can take
/// minutes; the template path uses the built-in generator and takes seconds.
#[derive(Clone, Debug)]
pub enum Author {
    /// `krate create --agent <name>`: an AI writes the app.
    Agent(String),
    /// `krate create` with no agent: the built-in template generator. Limited
    /// to the shapes it knows, but fast and needs no model.
    BuiltIn,
}

/// What `start` needs to launch a build.
pub struct BuildSpec {
    /// The path to the `krate` binary that will do the work.
    pub krate_bin: PathBuf,
    /// The plain-English request.
    pub description: String,
    /// The kebab-case app name.
    pub name: String,
    /// Who writes the code.
    pub author: Author,
    /// Where the `.krate` and the work dir go. This directory is created.
    pub root: PathBuf,
}

impl JobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a build and return its id immediately. The build runs on a thread;
    /// this function never waits for it.
    pub fn start(&self, spec: BuildSpec) -> Result<String, String> {
        let id = format!("build-{}", self.next.fetch_add(1, Ordering::SeqCst) + 1);

        let job_root = spec.root.join(&id);
        std::fs::create_dir_all(&job_root)
            .map_err(|err| format!("could not create the build directory {job_root:?}: {err}"))?;
        let work_dir = job_root.join("work");
        let output = job_root.join(format!("{}.krate", spec.name));
        let app_dir = work_dir.join(&spec.name);

        let job = Job {
            id: id.clone(),
            description: spec.description.clone(),
            name: spec.name.clone(),
            phase: Phase::Running,
            progress: "starting".to_string(),
            log: vec!["starting".to_string()],
            output: None,
            work_dir: work_dir.clone(),
            app_dir,
            error: None,
            transcript: None,
            started: Instant::now(),
            finished: None,
        };
        self.inner
            .lock()
            .map_err(|_| "the job store is poisoned".to_string())?
            .insert(id.clone(), job);

        let store = self.clone();
        let thread_id = id.clone();
        let builder = std::thread::Builder::new().name(format!("krate-mcp-{id}"));
        builder
            .spawn(move || {
                let outcome = run_build(&spec, &work_dir, &output, |line| {
                    store.note(&thread_id, line);
                });
                store.finish(&thread_id, outcome, &output);
            })
            .map_err(|err| format!("could not start a build thread: {err}"))?;

        Ok(id)
    }

    /// Look one job up.
    pub fn get(&self, id: &str) -> Option<Job> {
        self.inner.lock().ok()?.get(id).cloned()
    }

    /// Every job id, for the "no such job" message.
    pub fn ids(&self) -> Vec<String> {
        match self.inner.lock() {
            Ok(jobs) => {
                let mut ids: Vec<String> = jobs.keys().cloned().collect();
                ids.sort();
                ids
            }
            Err(_) => Vec::new(),
        }
    }

    /// Record a progress line.
    fn note(&self, id: &str, line: String) {
        if let Ok(mut jobs) = self.inner.lock() {
            if let Some(job) = jobs.get_mut(id) {
                job.progress = line.clone();
                job.log.push(line);
                // A very long build should not grow memory without bound. The
                // recent lines are the useful ones.
                if job.log.len() > MAX_LOG_LINES {
                    job.log.drain(0..job.log.len() - MAX_LOG_LINES);
                }
            }
        }
    }

    /// Record the final outcome.
    fn finish(&self, id: &str, outcome: Result<serde_json::Value, String>, output: &Path) {
        if let Ok(mut jobs) = self.inner.lock() {
            if let Some(job) = jobs.get_mut(id) {
                job.finished = Some(Instant::now());
                match outcome {
                    Ok(transcript) => {
                        job.phase = Phase::Succeeded;
                        job.progress = "done".to_string();
                        job.log.push("done".to_string());
                        job.output = Some(output.to_path_buf());
                        job.transcript = Some(transcript);
                    }
                    Err(error) => {
                        job.phase = Phase::Failed;
                        job.progress = "failed".to_string();
                        job.error = Some(error);
                    }
                }
            }
        }
    }
}

/// Most recent progress lines kept per job.
const MAX_LOG_LINES: usize = 200;

/// Run one build to completion, reporting progress as it goes.
///
/// This shells out to the same `krate create` a person would run. That is the
/// point: the MCP server is not a second implementation of authoring that can
/// drift from the first, it is a caller of it. And the build happens here, on
/// the user's own machine, because compiling model-written Rust is executing
/// model-written Rust.
fn run_build(
    spec: &BuildSpec,
    work_dir: &Path,
    output: &Path,
    mut note: impl FnMut(String),
) -> Result<serde_json::Value, String> {
    let mut command = Command::new(&spec.krate_bin);
    command
        .arg("create")
        .arg(&spec.description)
        .arg("--output")
        .arg(output)
        .arg("--name")
        .arg(&spec.name)
        .arg("--work-dir")
        .arg(work_dir)
        // --json makes stdout exactly one krate.author.v1 object, which is what
        // we parse for the transcript. Human progress lines are suppressed.
        .arg("--json")
        // Never offer to install a toolchain: there is no terminal here to
        // answer the prompt, and a hung prompt looks exactly like a hung build.
        .arg("--no-install");

    if let Author::Agent(agent) = &spec.author {
        command.arg("--agent").arg(agent);
    }

    let mut child = command
        // Nothing may read this process's stdin: it belongs to the MCP client.
        // A child that inherited it would eat the client's messages.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            format!(
                "could not run `{}`: {err}. The MCP server needs the krate binary it was \
                 launched beside; check the `command` path in your client's MCP config.",
                spec.krate_bin.display()
            )
        })?;

    // stderr carries the human-readable progress an agent run emits. Drain it on
    // a thread: if nobody reads it the pipe fills, the child blocks writing to
    // it, and the build deadlocks looking exactly like a slow compile.
    let stderr = child.stderr.take();
    let progress: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // Kept separately from the drained progress, because the progress lines are
    // consumed into the job log as they arrive and are gone by the time a
    // failure needs explaining. Without this, a build that died because the
    // agent could not authenticate reported only "krate create exited 1" -- a
    // message from which a model can learn nothing and a person can do nothing.
    let said: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&progress);
    let keep = Arc::clone(&said);
    let stderr_thread = stderr.map(|stderr| {
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if let Ok(mut keep) = keep.lock() {
                    keep.push(line.clone());
                }
                if let Ok(mut sink) = sink.lock() {
                    sink.push(line);
                }
            }
        })
    });

    // Read stdout to the end. `krate create --json` writes one object at the
    // very end, so reaching EOF here means the child is done talking.
    let mut stdout_text = String::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            stdout_text.push_str(&line);
            stdout_text.push('\n');
        }
    }

    // Mirror whatever stderr collected into the job log, so a status poll after
    // the build shows the steps it went through.
    let drain = |note: &mut dyn FnMut(String), progress: &Arc<Mutex<Vec<String>>>| {
        if let Ok(mut lines) = progress.lock() {
            for line in lines.drain(..) {
                note(line);
            }
        }
    };
    drain(&mut note, &progress);

    let status = child
        .wait()
        .map_err(|err| format!("the build process could not be waited on: {err}"))?;
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }
    drain(&mut note, &progress);

    let transcript: Option<serde_json::Value> = stdout_text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .and_then(|line| serde_json::from_str(line).ok());

    if status.success() {
        if !output.exists() {
            return Err(format!(
                "the build reported success but wrote no file at {}. This is a bug in Krate, \
                 not in the app; please report it.",
                output.display()
            ));
        }
        return Ok(transcript.unwrap_or_else(|| serde_json::json!({ "ok": true })));
    }

    // A failure. Say why, in the most specific words available, because "exited
    // 1" is a message a model cannot act on and a person cannot fix. Preference
    // order: the structured message, then whatever the process actually said,
    // and only then the bare exit code.
    let structured = transcript.as_ref().and_then(|value| {
        value
            .get("message")
            .or_else(|| value.get("error"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    });

    let spoken = said
        .lock()
        .ok()
        .map(|lines| {
            // The tail is where the reason is; the head is progress chatter.
            let tail: Vec<String> = lines.iter().rev().take(12).rev().cloned().collect();
            tail.join("\n")
        })
        .filter(|text| !text.trim().is_empty());

    let detail = match (structured, spoken) {
        (Some(message), Some(said)) => format!("{message}\n\nKrate said:\n{said}"),
        (Some(message), None) => message,
        (None, Some(said)) => format!(
            "The build failed (exit {}). Krate said:\n{said}",
            exit_label(&status)
        ),
        (None, None) => {
            let text = stdout_text.trim();
            if text.is_empty() {
                format!(
                    "The build failed (exit {}) without saying why. Try the same request from a \
                     terminal with `krate create` to see the full output.",
                    exit_label(&status)
                )
            } else {
                text.to_string()
            }
        }
    };

    Err(detail)
}

fn exit_label(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "on a signal".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the krate binary that exits 0 without writing anything.
    /// Proves the "said it worked but produced nothing" path is caught rather
    /// than reported to the model as a success with a missing file.
    fn silent_success_bin(dir: &Path) -> PathBuf {
        let path = dir.join("fake-krate");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write");
        make_executable(&path);
        path
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).expect("metadata").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).expect("chmod");
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    fn wait_for(store: &JobStore, id: &str) -> Job {
        for _ in 0..600 {
            let job = store.get(id).expect("job exists");
            if job.phase != Phase::Running {
                return job;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("job {id} never finished");
    }

    #[test]
    fn a_started_job_is_immediately_visible_as_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A binary that does not exist: start must still succeed, because
        // starting is not building. The failure shows up in status.
        let store = JobStore::new();
        let id = store
            .start(BuildSpec {
                krate_bin: dir.path().join("nope"),
                description: "a thing".to_string(),
                name: "thing".to_string(),
                author: Author::BuiltIn,
                root: dir.path().to_path_buf(),
            })
            .expect("start");

        let job = store.get(&id).expect("job");
        assert_eq!(job.description, "a thing");
        assert_eq!(job.name, "thing");

        let done = wait_for(&store, &id);
        assert_eq!(done.phase, Phase::Failed);
        // The message must name the fix, not just the failure.
        let error = done.error.expect("error");
        assert!(
            error.contains("MCP config") || error.contains("krate binary"),
            "unhelpful error: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn success_without_an_output_file_is_a_failure_not_a_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = JobStore::new();
        let id = store
            .start(BuildSpec {
                krate_bin: silent_success_bin(dir.path()),
                description: "a thing".to_string(),
                name: "thing".to_string(),
                author: Author::BuiltIn,
                root: dir.path().to_path_buf(),
            })
            .expect("start");

        let done = wait_for(&store, &id);
        // Reporting this as success is the exact failure mode the plan warns
        // about: a model confidently telling someone their app is ready when
        // there is no file.
        assert_eq!(done.phase, Phase::Failed);
        assert!(done.error.expect("error").contains("wrote no file"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_reports_what_krate_actually_said_not_just_the_exit_code() {
        // The real case this comes from: an agent build died in one second
        // because the coding agent's session had expired. The reason was on
        // stderr, and the job reported only "krate create exited 1" -- a
        // message from which neither a model nor a person can do anything.
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("noisy-krate");
        std::fs::write(
            &bin,
            "#!/bin/sh\n\
             echo 'authoring the app' >&2\n\
             echo 'error: the Claude agent did not finish successfully' >&2\n\
             exit 1\n",
        )
        .expect("write");
        make_executable(&bin);

        let store = JobStore::new();
        let id = store
            .start(BuildSpec {
                krate_bin: bin,
                description: "a thing".to_string(),
                name: "thing".to_string(),
                author: Author::Agent("claude".to_string()),
                root: dir.path().to_path_buf(),
            })
            .expect("start");

        let done = wait_for(&store, &id);
        assert_eq!(done.phase, Phase::Failed);
        let error = done.error.expect("error");
        assert!(
            error.contains("Claude agent did not finish successfully"),
            "the failure must carry the real reason, got: {error}"
        );
    }

    #[test]
    fn ids_are_unique_and_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = JobStore::new();
        let mut ids = Vec::new();
        for n in 0..3 {
            ids.push(
                store
                    .start(BuildSpec {
                        krate_bin: dir.path().join("nope"),
                        description: format!("app {n}"),
                        name: format!("app{n}"),
                        author: Author::BuiltIn,
                        root: dir.path().to_path_buf(),
                    })
                    .expect("start"),
            );
        }
        ids.sort();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "job ids must be unique");
        assert_eq!(store.ids(), ids);
    }

    #[test]
    fn an_unknown_job_is_none() {
        let store = JobStore::new();
        assert!(store.get("build-404").is_none());
    }
}
