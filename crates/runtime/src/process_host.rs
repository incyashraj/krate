//! Programs on the computer the app opens on (`krate:process`, IC-977): the
//! operating-system half.
//!
//! This file knows nothing of grants, manifests or app paths. The Phase 2
//! host decides whether a call may happen (the manifest names the program
//! and the person allowed it) and turns the app's folder into a real one by
//! the same rules the file calls use; what arrives here is a program name
//! already admitted and a folder already resolved. Kept that way so it can
//! be compiled and checked for another system on its own.
//!
//! What it guarantees:
//!
//! - **A program is found, never named by path.** On the person's PATH (its
//!   absolute entries only, so a `.` in PATH never runs something from the
//!   current folder), then in `~/.cargo/bin`. The path found never leaves
//!   this file.
//! - **Every process ends with the app.** Each program starts as the leader
//!   of its own process group (a terminal: its own session), and the whole
//!   group is killed -- a build's compiler and a test's children with it --
//!   when the app kills it, releases it, or the run ends: [`Processes`] kills
//!   everything it holds when dropped, and an exit hook kills what is still
//!   registered if the process leaves without unwinding (`std::process::exit`
//!   from a window's close). On Windows each process is in a job object that
//!   kills its tree when the handle closes, which the system does even if
//!   Krate itself is killed. When a program ends on its own, whatever it left
//!   behind in its group is killed then, so nothing outlives its parent.
//! - **Output waits, nothing is dropped.** A reader thread per stream holds
//!   up to [`STREAM_LIMIT`] unread bytes; past that it stops reading and the
//!   program blocks on its next write until the app reads (backpressure).
//!   A compiler's JSON messages arrive whole or not yet, never with a hole.
//! - **Writing never waits.** Bytes for stdin go to a writer thread; at most
//!   [`STDIN_LIMIT`] may wait for the program to read them.
//! - **The person's environment, without Krate's.** The child inherits the
//!   environment Krate was started with, minus every `KRATE_*` variable
//!   (Krate's own switches and hub addresses), plus what the app adds.
//!
//! Not guaranteed: a program that leaves its process group on purpose (a
//! daemon calling `setsid`) is not followed, and on macOS and Linux a Krate
//! killed with SIGKILL cannot kill anything (no exit hook runs). A terminal's
//! background jobs in other process groups are sent SIGHUP by the hangup, as
//! closing any terminal window does, and the foreground job is killed.

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};

/// Processes running at once in one app.
pub const MAX_RUNNING: usize = 16;
/// Processes held at once in one app: running, or ended and not released.
pub const MAX_HELD: usize = 64;
/// Unread bytes held per output stream before the program waits.
pub const STREAM_LIMIT: usize = 8 * 1024 * 1024;
/// Bytes one read may take.
pub const MAX_READ: usize = 4 * 1024 * 1024;
/// Bytes that may wait to be written to a program's stdin.
pub const STDIN_LIMIT: usize = 8 * 1024 * 1024;
/// The terminal type a terminal says it is.
pub const TERMINAL_TYPE: &str = "xterm-256color";

/// Why the operating-system half refused or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsError {
    /// The program is not installed (the words name it, never a path).
    NotFound(String),
    /// Too many processes running or held.
    TooMany(String),
    /// No such handle in this run.
    UnknownHandle,
    /// The system refused, in its words.
    Platform(String),
}

/// How a process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// What a read found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub bytes: Vec<u8>,
    pub ended: bool,
}

/// Which output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// One output stream: what has arrived and not been read.
#[derive(Default)]
struct Pipe {
    state: Mutex<PipeState>,
    room: Condvar,
}

#[derive(Default)]
struct PipeState {
    unread: VecDeque<u8>,
    /// The program closed the stream (or ended).
    closed: bool,
    /// The app let go of the process: stop waiting for room.
    abandoned: bool,
}

impl Pipe {
    /// Read `reader` until it ends, holding at most [`STREAM_LIMIT`] unread.
    fn pump(self: Arc<Self>, mut reader: impl Read) {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                // A terminal whose last program ended reads EIO, not 0.
                Err(_) => break,
            };
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.unread.len() >= STREAM_LIMIT && !state.abandoned {
                state = self.room.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.abandoned {
                break;
            }
            state.unread.extend(&chunk[..n]);
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
    }

    fn take(&self, max: usize) -> Output {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let n = max.min(MAX_READ).min(state.unread.len());
        let bytes: Vec<u8> = state.unread.drain(..n).collect();
        let ended = state.closed && state.unread.is_empty();
        drop(state);
        self.room.notify_all();
        Output { bytes, ended }
    }

    fn abandon(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.abandoned = true;
        state.unread.clear();
        drop(state);
        self.room.notify_all();
    }
}

/// The stdin side: a queue a writer thread empties into the program.
struct Input {
    queue: Option<mpsc::Sender<Vec<u8>>>,
    waiting: Arc<AtomicUsize>,
    /// The program stopped reading (closed its stdin or ended).
    broken: Arc<AtomicBool>,
}

impl Input {
    fn new(mut writer: Box<dyn Write + Send>) -> Self {
        let (queue, chunks) = mpsc::channel::<Vec<u8>>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let broken = Arc::new(AtomicBool::new(false));
        let (left, gone) = (waiting.clone(), broken.clone());
        std::thread::Builder::new()
            .name("krate-process-stdin".into())
            .spawn(move || {
                for chunk in chunks {
                    if !gone.load(Ordering::Relaxed)
                        && (writer.write_all(&chunk).is_err() || writer.flush().is_err())
                    {
                        gone.store(true, Ordering::Relaxed);
                    }
                    left.fetch_sub(chunk.len(), Ordering::Relaxed);
                }
                // The queue closed: dropping the writer is end of input.
            })
            .ok();
        Self {
            queue: Some(queue),
            waiting,
            broken,
        }
    }

    fn write(&self, bytes: Vec<u8>) -> Result<(), OsError> {
        let Some(queue) = self.queue.as_ref() else {
            return Err(OsError::Platform(
                "its input was closed; nothing more can be written to it".to_string(),
            ));
        };
        if self.broken.load(Ordering::Relaxed) {
            return Err(OsError::Platform(
                "the program has stopped reading its input".to_string(),
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let waiting = self.waiting.load(Ordering::Relaxed);
        if waiting.saturating_add(bytes.len()) > STDIN_LIMIT {
            return Err(OsError::Platform(format!(
                "the program has not read the last {} MiB written to it yet; write again \
                 once it reads",
                STDIN_LIMIT / (1024 * 1024)
            )));
        }
        self.waiting.fetch_add(bytes.len(), Ordering::Relaxed);
        let len = bytes.len();
        if queue.send(bytes).is_err() {
            self.waiting.fetch_sub(len, Ordering::Relaxed);
            return Err(OsError::Platform(
                "the program has stopped reading its input".to_string(),
            ));
        }
        Ok(())
    }

    fn close(&mut self) {
        self.queue = None;
    }
}

/// The program itself, as the system holds it.
enum Child {
    Std(std::process::Child),
    /// A terminal on Windows: ConPTY's own child.
    #[cfg(windows)]
    Pty(Box<dyn portable_pty::Child + Send + Sync>),
}

impl Child {
    fn id(&self) -> u32 {
        match self {
            Self::Std(child) => child.id(),
            #[cfg(windows)]
            Self::Pty(child) => child.process_id().unwrap_or(0),
        }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<Exit>> {
        match self {
            Self::Std(child) => Ok(child.try_wait()?.map(exit_of)),
            #[cfg(windows)]
            Self::Pty(child) => Ok(child.try_wait()?.map(|status| Exit {
                code: Some(status.exit_code() as i32),
                signal: None,
            })),
        }
    }

    /// Wait for a child already killed: it ends at once.
    fn reap(&mut self) -> Option<Exit> {
        match self {
            Self::Std(child) => child.wait().ok().map(exit_of),
            #[cfg(windows)]
            Self::Pty(child) => child.wait().ok().map(|status| Exit {
                code: Some(status.exit_code() as i32),
                signal: None,
            }),
        }
    }
}

fn exit_of(status: std::process::ExitStatus) -> Exit {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    Exit {
        code: status.code(),
        signal,
    }
}

/// One process this run holds.
struct Entry {
    child: Child,
    /// The process group (macOS, Linux): the child's own pid.
    group: Group,
    stdout: Arc<Pipe>,
    /// None for a terminal: everything comes out of the one pty.
    stderr: Option<Arc<Pipe>>,
    stdin: Input,
    /// The terminal's controlling side, for resize and for its foreground job.
    tty: Option<Box<dyn portable_pty::MasterPty + Send>>,
    status: Option<Exit>,
}

/// What ending a process's whole tree takes, per system.
struct Group {
    #[cfg(unix)]
    pgid: libc::pid_t,
    #[cfg(windows)]
    job: windows_job::Job,
}

impl Entry {
    fn running(&self) -> bool {
        self.status.is_none()
    }

    /// Look once; when it has ended, sweep what it left in its group.
    fn poll(&mut self) -> Result<Option<Exit>, OsError> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.ended(status);
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(err) => Err(OsError::Platform(err.to_string())),
        }
    }

    fn ended(&mut self, status: Exit) {
        self.status = Some(status);
        // Whatever it started and left running goes with it. Done the moment
        // the leader is reaped, so this group id is never signalled again.
        self.kill_group();
        live::forget(self.child.id());
        // ConPTY keeps its output open until the console closes.
        #[cfg(windows)]
        if self.tty.is_some() {
            self.tty = None;
        }
    }

    /// Kill the process and everything in its group (and, for a terminal,
    /// its foreground job), then reap it.
    fn kill(&mut self) {
        if self.status.is_some() {
            return;
        }
        #[cfg(unix)]
        {
            let foreground = self.tty.as_ref().and_then(|tty| tty.process_group_leader());
            // A terminal hangs up first, as closing a terminal window does,
            // so the shell passes the hangup to its jobs.
            if self.tty.is_some() {
                signal_group(self.group.pgid, libc::SIGHUP);
                if let Some(fg) = foreground {
                    signal_group(fg, libc::SIGHUP);
                }
                let deadline = Instant::now() + Duration::from_millis(100);
                while Instant::now() < deadline {
                    if matches!(self.child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            if let Some(fg) = foreground {
                signal_group(fg, libc::SIGKILL);
            }
        }
        self.kill_group();
        let status = self.child.reap().unwrap_or(Exit {
            code: None,
            signal: None,
        });
        self.status = Some(status);
        live::forget(self.child.id());
    }

    fn kill_group(&self) {
        #[cfg(unix)]
        signal_group(self.group.pgid, libc::SIGKILL);
        #[cfg(windows)]
        self.group.job.terminate();
    }

    fn abandon(&mut self) {
        self.kill();
        self.stdin.close();
        self.stdout.abandon();
        if let Some(stderr) = self.stderr.as_ref() {
            stderr.abandon();
        }
        self.tty = None;
    }
}

#[cfg(unix)]
fn signal_group(pgid: libc::pid_t, signal: libc::c_int) {
    if pgid > 1 {
        // SAFETY: killpg only sends a signal; a group that is gone is ESRCH.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }
}

/// Every process one run holds, by handle. Dropping it kills them all.
#[derive(Default)]
pub struct Processes {
    entries: BTreeMap<u64, Entry>,
    issued: u64,
}

impl Processes {
    pub fn new() -> Self {
        Self::default()
    }

    fn admit(&mut self) -> Result<(), OsError> {
        for entry in self.entries.values_mut() {
            let _ = entry.poll();
        }
        let running = self.entries.values().filter(|e| e.running()).count();
        if running >= MAX_RUNNING {
            return Err(OsError::TooMany(format!(
                "{MAX_RUNNING} programs are already running in this app; one must end first"
            )));
        }
        if self.entries.len() >= MAX_HELD {
            return Err(OsError::TooMany(format!(
                "this app holds {MAX_HELD} programs that ended and were not released; \
                 release one first"
            )));
        }
        Ok(())
    }

    fn insert(&mut self, entry: Entry) -> u64 {
        self.issued += 1;
        self.entries.insert(self.issued, entry);
        self.issued
    }

    fn entry(&mut self, handle: u64) -> Result<&mut Entry, OsError> {
        self.entries.get_mut(&handle).ok_or(OsError::UnknownHandle)
    }

    /// Start `program` (a bare name already admitted) in `cwd` (a real
    /// folder already resolved), with stdin, stdout and stderr as pipes.
    pub fn start(
        &mut self,
        program: &str,
        args: &[String],
        cwd: &Path,
        env: &[(String, String)],
    ) -> Result<u64, OsError> {
        self.admit()?;
        check_args(args)?;
        check_env(env)?;
        let found = find_program(program).ok_or_else(|| not_installed(program))?;
        let mut command = std::process::Command::new(&found);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for key in krate_variables() {
            command.env_remove(key);
        }
        if let Some(path) = path_with_cargo_bin() {
            command.env("PATH", path);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        let job = windows_job::Job::new()?;
        let mut child = command
            .spawn()
            .map_err(|err| OsError::Platform(format!("{program} did not start: {err}")))?;
        #[cfg(windows)]
        job.assign(std::os::windows::io::AsRawHandle::as_raw_handle(&child));
        let group = Group {
            #[cfg(unix)]
            pgid: child.id() as libc::pid_t,
            #[cfg(windows)]
            job,
        };
        live::remember(child.id(), None);
        let stdout = Arc::new(Pipe::default());
        let stderr = Arc::new(Pipe::default());
        if let Some(out) = child.stdout.take() {
            spawn_pump(stdout.clone(), out);
        } else {
            stdout
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = true;
        }
        if let Some(err) = child.stderr.take() {
            spawn_pump(stderr.clone(), err);
        } else {
            stderr
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = true;
        }
        let stdin: Box<dyn Write + Send> = match child.stdin.take() {
            Some(stdin) => Box::new(stdin),
            None => Box::new(std::io::sink()),
        };
        Ok(self.insert(Entry {
            child: Child::Std(child),
            group,
            stdout,
            stderr: Some(stderr),
            stdin: Input::new(stdin),
            tty: None,
            status: None,
        }))
    }

    /// Open the person's own login shell in a pseudo-terminal.
    pub fn open_terminal(&mut self, cwd: &Path, cols: u16, rows: u16) -> Result<u64, OsError> {
        self.admit()?;
        let size = portable_pty::PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = portable_pty::native_pty_system()
            .openpty(size)
            .map_err(|err| OsError::Platform(format!("no terminal could be opened: {err}")))?;
        let mut command = portable_pty::CommandBuilder::new_default_prog();
        command.cwd(cwd);
        for key in krate_variables() {
            command.env_remove(key);
        }
        command.env("TERM", TERMINAL_TYPE);
        #[cfg(windows)]
        let job = windows_job::Job::new()?;
        let spawned = pair
            .slave
            .spawn_command(command)
            .map_err(|err| OsError::Platform(format!("the shell did not start: {err}")))?;
        // The terminal ends when its programs do only once this side of the
        // slave is closed.
        drop(pair.slave);
        // On macOS and Linux the pty's child is a std child: held as one, so
        // its exit says which signal ended it.
        #[cfg(unix)]
        let child = {
            let spawned: Box<dyn portable_pty::Child> = spawned;
            match spawned.downcast::<std::process::Child>() {
                Ok(child) => Child::Std(*child),
                Err(_) => {
                    return Err(OsError::Platform(
                        "the terminal's shell is not a process Krate can hold".to_string(),
                    ))
                }
            }
        };
        #[cfg(windows)]
        let child = {
            if let Some(handle) = spawned.as_raw_handle() {
                job.assign(handle);
            }
            Child::Pty(spawned)
        };
        let group = Group {
            #[cfg(unix)]
            pgid: child.id() as libc::pid_t,
            #[cfg(windows)]
            job,
        };
        #[cfg(unix)]
        live::remember(child.id(), pair.master.as_raw_fd());
        #[cfg(not(unix))]
        live::remember(child.id(), None);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| OsError::Platform(format!("the terminal cannot be read: {err}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| OsError::Platform(format!("the terminal cannot be written: {err}")))?;
        let stdout = Arc::new(Pipe::default());
        spawn_pump(stdout.clone(), reader);
        Ok(self.insert(Entry {
            child,
            group,
            stdout,
            stderr: None,
            stdin: Input::new(writer),
            tty: Some(pair.master),
            status: None,
        }))
    }

    pub fn write_stdin(&mut self, handle: u64, bytes: Vec<u8>) -> Result<(), OsError> {
        self.entry(handle)?.stdin.write(bytes)
    }

    /// End of input: a pipe closes once what was queued is written; a
    /// terminal is sent Ctrl-D.
    pub fn close_stdin(&mut self, handle: u64) -> Result<(), OsError> {
        let entry = self.entry(handle)?;
        if entry.tty.is_some() {
            return entry.stdin.write(vec![0x04]);
        }
        entry.stdin.close();
        Ok(())
    }

    pub fn read(&mut self, handle: u64, stream: Stream, max: u32) -> Result<Output, OsError> {
        let entry = self.entry(handle)?;
        Ok(match (stream, entry.stderr.as_ref()) {
            (Stream::Stdout, _) => entry.stdout.take(max as usize),
            (Stream::Stderr, Some(stderr)) => stderr.take(max as usize),
            (Stream::Stderr, None) => Output {
                bytes: Vec::new(),
                ended: true,
            },
        })
    }

    /// Wait up to `timeout_ms` for the process to end.
    pub fn wait(&mut self, handle: u64, timeout_ms: u32) -> Result<Option<Exit>, OsError> {
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let entry = self.entry(handle)?;
        loop {
            if let Some(status) = entry.poll()? {
                return Ok(Some(status));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(2)));
        }
    }

    pub fn kill(&mut self, handle: u64) -> Result<(), OsError> {
        let entry = self.entry(handle)?;
        let _ = entry.poll();
        entry.kill();
        Ok(())
    }

    pub fn resize(&mut self, handle: u64, cols: u16, rows: u16) -> Result<(), OsError> {
        let entry = self.entry(handle)?;
        let Some(tty) = entry.tty.as_ref() else {
            return Err(OsError::Platform(
                "only a terminal has a size; this is a program".to_string(),
            ));
        };
        tty.resize(portable_pty::PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| OsError::Platform(format!("the terminal was not resized: {err}")))
    }

    /// Kill it if it runs, drop what is unread, forget the handle.
    pub fn release(&mut self, handle: u64) {
        if let Some(mut entry) = self.entries.remove(&handle) {
            entry.abandon();
        }
    }

    /// The system's process id, for tests that check a process is gone.
    pub fn pid(&self, handle: u64) -> Option<u32> {
        self.entries.get(&handle).map(|entry| entry.child.id())
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        for (_, mut entry) in std::mem::take(&mut self.entries) {
            entry.abandon();
        }
    }
}

fn spawn_pump(pipe: Arc<Pipe>, reader: impl Read + Send + 'static) {
    let fallback = pipe.clone();
    if std::thread::Builder::new()
        .name("krate-process-output".into())
        .spawn(move || pipe.pump(reader))
        .is_err()
    {
        fallback
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
    }
}

fn not_installed(program: &str) -> OsError {
    OsError::NotFound(format!("{program} is not installed on this computer"))
}

fn check_args(args: &[String]) -> Result<(), OsError> {
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err(OsError::Platform(
            "an argument contains a NUL character, which no program can be given".to_string(),
        ));
    }
    Ok(())
}

fn check_env(env: &[(String, String)]) -> Result<(), OsError> {
    for (key, value) in env {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err(OsError::Platform(format!(
                "`{}` cannot be an environment variable: a name is not empty and has no `=`, \
                 and neither has a NUL character",
                key.escape_default()
            )));
        }
        if is_krate_variable(key.as_ref()) {
            return Err(OsError::Platform(format!(
                "`{key}` is one of Krate's own settings, which an app cannot pass on"
            )));
        }
    }
    Ok(())
}

fn is_krate_variable(key: &std::ffi::OsStr) -> bool {
    key.to_str()
        .is_some_and(|key| key.len() >= 6 && key[..6].eq_ignore_ascii_case("KRATE_"))
}

/// Krate's own variables in this process's environment, never passed on.
fn krate_variables() -> Vec<OsString> {
    std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| is_krate_variable(key))
        .collect()
}

/// Where Rust's tools live for this person: `$CARGO_HOME/bin`, or
/// `~/.cargo/bin`.
fn cargo_bin() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME").filter(|h| !h.is_empty()) {
        let home = PathBuf::from(home);
        if home.is_absolute() {
            return Some(home.join("bin"));
        }
    }
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .map(|h| h.join(".cargo").join("bin"))
}

/// The folders a program is looked for in: PATH's absolute entries, then
/// the cargo bin folder.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // A relative entry (`.`, `bin`) would run whatever sits in the folder
    // Krate was started from.
    dirs.retain(|dir| dir.is_absolute());
    if let Some(bin) = cargo_bin() {
        if !dirs.contains(&bin) {
            dirs.push(bin);
        }
    }
    dirs
}

/// PATH for a child: the person's, with the cargo bin folder added when it
/// is missing (an app opened from the Finder gets a PATH without it, and
/// cargo's own subcommands live there). None leaves PATH as inherited.
fn path_with_cargo_bin() -> Option<OsString> {
    let bin = cargo_bin()?;
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&current).collect();
    if dirs.contains(&bin) || !bin.is_dir() {
        return None;
    }
    dirs.push(bin);
    std::env::join_paths(dirs).ok()
}

/// Find a program by its bare name. The result never reaches the app.
pub fn find_program(name: &str) -> Option<PathBuf> {
    find_program_in(name, &search_dirs())
}

fn find_program_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return None;
    }
    for dir in dirs {
        for candidate in candidates(dir, name) {
            if is_runnable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    // Only real programs: a `.cmd` or `.bat` runs through cmd.exe, whose
    // quoting is not what the app's arguments say.
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".exe") || lower.ends_with(".com") {
        vec![dir.join(name)]
    } else {
        vec![
            dir.join(format!("{name}.exe")),
            dir.join(format!("{name}.com")),
        ]
    }
}

#[cfg(not(windows))]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![dir.join(name)]
}

fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Process groups still alive in this Krate, killed by an exit hook when
/// Krate leaves without dropping [`Processes`] (`std::process::exit`).
mod live {
    use std::sync::Mutex;

    struct Registered {
        pid: u32,
        #[cfg_attr(not(unix), allow(dead_code))]
        tty_fd: Option<i32>,
    }

    static LIVE: Mutex<Vec<Registered>> = Mutex::new(Vec::new());

    pub(super) fn remember(pid: u32, tty_fd: Option<i32>) {
        hook();
        if let Ok(mut live) = LIVE.lock() {
            live.push(Registered { pid, tty_fd });
        }
    }

    pub(super) fn forget(pid: u32) {
        if let Ok(mut live) = LIVE.lock() {
            live.retain(|r| r.pid != pid);
        }
    }

    #[cfg(unix)]
    fn hook() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: registering a plain extern "C" function.
            unsafe {
                libc::atexit(at_exit);
            }
        });
    }

    #[cfg(not(unix))]
    fn hook() {
        // Windows: each job object kills its tree when the system closes
        // its handle at exit, whatever way Krate leaves.
    }

    #[cfg(unix)]
    extern "C" fn at_exit() {
        // try_lock: a thread holding the lock as the process exits must not
        // hang the exit.
        let Ok(live) = LIVE.try_lock() else {
            return;
        };
        for r in live.iter() {
            let pgid = r.pid as libc::pid_t;
            // SAFETY: only signals are sent; tcgetpgrp reads a number.
            unsafe {
                if let Some(fd) = r.tty_fd {
                    let fg = libc::tcgetpgrp(fd);
                    if fg > 1 {
                        libc::killpg(fg, libc::SIGHUP);
                        libc::killpg(fg, libc::SIGKILL);
                    }
                }
                if pgid > 1 {
                    libc::killpg(pgid, libc::SIGHUP);
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        }
    }
}

/// A job object that ends a process's whole tree (Windows).
#[cfg(windows)]
mod windows_job {
    use super::OsError;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    pub(super) struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object handle, usable from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub(super) fn new() -> Result<Self, OsError> {
            // SAFETY: plain Win32 calls on a handle this function owns.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(OsError::Platform(format!(
                        "Windows did not make a job for the program: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                Ok(Self(job))
            }
        }

        pub(super) fn assign(&self, process: std::os::windows::io::RawHandle) {
            // SAFETY: the process handle is the child's, alive for this call.
            unsafe {
                AssignProcessToJobObject(self.0, process as HANDLE);
            }
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the job handle is owned by self.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing the handle this Job owns; the system kills
            // everything still in the job (KILL_ON_JOB_CLOSE).
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn gone(pid: u32) -> bool {
        // A process that is gone answers ESRCH; a zombie still answers.
        for _ in 0..200 {
            // SAFETY: signal 0 only asks whether the process exists.
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn read_all(processes: &mut Processes, h: u64, stream: Stream) -> Vec<u8> {
        let mut all = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = processes.read(h, stream, 1 << 20).expect("read");
            all.extend(out.bytes);
            if out.ended || Instant::now() > deadline {
                return all;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_program_is_found_only_by_bare_name_in_absolute_folders() {
        let dir = tempfile::tempdir().expect("dir");
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").expect("tool");
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "not runnable").expect("plain");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(find_program_in("tool", &dirs), Some(tool.clone()));
        assert_eq!(find_program_in("plain", &dirs), None, "not executable");
        assert_eq!(find_program_in("missing", &dirs), None);
        let full = tool.to_string_lossy().to_string();
        assert_eq!(find_program_in(&full, &dirs), None, "never a path");
        assert_eq!(find_program_in("../tool", &dirs), None);
    }

    #[test]
    fn output_both_streams_and_exit_code_arrive() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        let h = processes
            .start(
                "sh",
                &["-c".into(), "echo out; echo err >&2; exit 3".into()],
                dir.path(),
                &[],
            )
            .expect("start");
        assert_eq!(read_all(&mut processes, h, Stream::Stdout), b"out\n");
        assert_eq!(read_all(&mut processes, h, Stream::Stderr), b"err\n");
        let exit = processes.wait(h, 10_000).expect("wait").expect("ended");
        assert_eq!(exit.code, Some(3));
        assert_eq!(processes.wait(h, 0).expect("again"), Some(exit));
    }

    #[test]
    fn stdin_reaches_the_program_and_closing_it_ends_input() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        let h = processes.start("cat", &[], dir.path(), &[]).expect("cat");
        processes
            .write_stdin(h, b"one\ntwo\n".to_vec())
            .expect("write");
        processes.close_stdin(h).expect("close");
        assert_eq!(read_all(&mut processes, h, Stream::Stdout), b"one\ntwo\n");
        assert_eq!(
            processes.wait(h, 10_000).expect("wait").unwrap().code,
            Some(0)
        );
    }

    #[test]
    fn krate_variables_stay_behind_and_additions_arrive() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        // SAFETY: tests in this module do not read KRATE_SECRET_FOR_TEST
        // from another thread.
        unsafe { std::env::set_var("KRATE_SECRET_FOR_TEST", "hush") };
        let h = processes
            .start(
                "sh",
                &[
                    "-c".into(),
                    "echo \"[$KRATE_SECRET_FOR_TEST][$ADDED]\"".into(),
                ],
                dir.path(),
                &[("ADDED".into(), "yes".into())],
            )
            .expect("start");
        assert_eq!(read_all(&mut processes, h, Stream::Stdout), b"[][yes]\n");
        assert!(matches!(
            processes.start(
                "sh",
                &[],
                dir.path(),
                &[("KRATE_HUB_URL".into(), "x".into())]
            ),
            Err(OsError::Platform(_))
        ));
    }

    #[test]
    fn output_waits_for_the_app_and_loses_nothing() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        // 12 MiB, more than one stream holds.
        let h = processes
            .start(
                "sh",
                &["-c".into(), "head -c 12582912 /dev/zero".into()],
                dir.path(),
                &[],
            )
            .expect("start");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            processes.wait(h, 0).expect("wait"),
            None,
            "held at the limit until read"
        );
        let all = read_all(&mut processes, h, Stream::Stdout);
        assert_eq!(all.len(), 12 * 1024 * 1024);
        assert!(processes.wait(h, 10_000).expect("wait").is_some());
    }

    #[test]
    fn kill_ends_the_whole_group_and_drop_ends_everything() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        // A program that starts another and waits: the grandchild's pid
        // comes out first.
        let script = "sleep 300 & echo $!; wait";
        let h = processes
            .start("sh", &["-c".into(), script.into()], dir.path(), &[])
            .expect("start");
        let mut line = Vec::new();
        while !line.ends_with(b"\n") {
            line.extend(processes.read(h, Stream::Stdout, 64).expect("read").bytes);
            std::thread::sleep(Duration::from_millis(2));
        }
        let grandchild: u32 = String::from_utf8_lossy(&line).trim().parse().expect("pid");
        let child = processes.pid(h).expect("pid");
        processes.kill(h).expect("kill");
        let exit = processes.wait(h, 5_000).expect("wait").expect("ended");
        assert_eq!(exit.signal, Some(libc::SIGKILL));
        assert!(gone(child) && gone(grandchild), "the whole group ended");

        let other = processes
            .start("sleep", &["300".into()], dir.path(), &[])
            .expect("sleep");
        let pid = processes.pid(other).expect("pid");
        drop(processes);
        assert!(gone(pid), "dropping the run's processes ends them");
    }

    #[test]
    fn a_terminal_echoes_resizes_and_ends() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        let h = processes
            .open_terminal(dir.path(), 80, 24)
            .expect("terminal");
        processes
            .write_stdin(h, b"echo krate-$((40+2))\n".to_vec())
            .expect("type");
        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&seen).contains("krate-42") && Instant::now() < deadline {
            seen.extend(
                processes
                    .read(h, Stream::Stdout, 1 << 16)
                    .expect("read")
                    .bytes,
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            String::from_utf8_lossy(&seen).contains("krate-42"),
            "{}",
            String::from_utf8_lossy(&seen)
        );
        processes.resize(h, 100, 30).expect("resize");
        let pid = processes.pid(h).expect("pid");
        processes.kill(h).expect("kill");
        assert!(processes.wait(h, 5_000).expect("wait").is_some());
        assert!(gone(pid));
    }

    #[test]
    fn limits_hold() {
        let dir = tempfile::tempdir().expect("dir");
        let mut processes = Processes::new();
        assert!(matches!(
            processes.start("no-such-program-krate", &[], dir.path(), &[]),
            Err(OsError::NotFound(words)) if words == "no-such-program-krate is not installed on this computer"
        ));
        assert_eq!(processes.wait(999, 0), Err(OsError::UnknownHandle));
        for _ in 0..MAX_RUNNING {
            processes
                .start("sleep", &["300".into()], dir.path(), &[])
                .expect("start");
        }
        assert!(matches!(
            processes.start("sleep", &["300".into()], dir.path(), &[]),
            Err(OsError::TooMany(_))
        ));
    }
}
