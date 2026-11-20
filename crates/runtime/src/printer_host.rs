//! Printers the person picks (`krate:print`, IC-986): the operating-system
//! half.
//!
//! This file knows nothing of grants or manifests. The Phase 2 host admits
//! every call first -- the manifest declares `device.printer`, the person
//! allowed it, and the organisation does not forbid printing -- and only then
//! asks this file to show the list or send a job. What it guarantees:
//!
//! - **The app never names a printer.** [`Printers::pick`] shows the person
//!   the system's list and keeps the one they chose under a fresh handle; the
//!   system's name for the queue never leaves this file except as the name
//!   shown to the person. No call here takes a printer name from the app.
//! - **A pick is remembered, and only where the caller says.** The CLI hands
//!   over a file for this app's principal ([`Printers::with_memory`]) when
//!   the app's lineage can be checked again next time and a person, not an
//!   `--auto-grant` run, is deciding. With no file, a pick lasts for the run.
//!   `krate permissions --forget` deletes the file, and the next print asks.
//! - **Bounded.** A job is at most [`MAX_JOB_BYTES`] and [`MAX_COPIES`]
//!   copies; a document's bytes must start the way its format starts, so a
//!   "PDF" cannot smuggle anything else through the system's filters.
//! - **One backend seam.** Everything the system does is behind [`Backend`]:
//!   CUPS (`lp`, `lpstat`) on macOS and Linux, the print spooler on Windows,
//!   a folder when [`SPOOL_ENV`] is set, and a fake under test.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// The largest job Krate sends: a document or one raw label stream.
pub const MAX_JOB_BYTES: usize = 16 * 1024 * 1024;
/// The most copies of one job.
pub const MAX_COPIES: u32 = 100;
/// The longest title an app may give a job or a list, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// A custom paper side, in millimetres.
pub const MIN_PAPER_MM: u32 = 10;
pub const MAX_PAPER_MM: u32 = 1000;
/// How many times round the list the person may go before it counts as
/// cancelled: a list that never ends is not a choice.
const WALK_ROUNDS: usize = 3;
/// The remembered pick's file format.
pub const MEMORY_SCHEMA: &str = "krate.printer.v1";

/// The person's pick, made by the environment rather than a dialog: a test,
/// or someone driving Krate from a script. The value is the printer as the
/// system names its queue, or `cancel` (or nothing) for a person who
/// cancels. Only the person running Krate sets it; an app cannot, it passes
/// the same grant, and it must name a printer that is set up.
pub const PICK_ENV: &str = "KRATE_PRINTER_PICK";

/// Print into a folder instead of to the system: each sub-folder is a
/// printer, and each job lands in it as its bytes and a note of its options.
/// For tests and for someone checking what an app would print. Only the
/// person running Krate sets it.
pub const SPOOL_ENV: &str = "KRATE_PRINTER_SPOOL";

/// Why a call did not send a job, in words for a person where it has any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// No printer is set up, or the system's printing is not running.
    Unavailable(String),
    /// The handle was never given this run.
    UnknownHandle,
    /// A job Krate does not accept.
    BadJob(String),
}

/// What became of a job that was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// In the printer's queue; the words are the system's name for the job.
    Queued(String),
    /// The system would not take it, in its words.
    Failed(String),
    /// The printer is no longer set up.
    Gone(String),
}

/// What a job's bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Pdf,
    Png,
    /// Printer language (ZPL, EPL, ESC/POS), sent unchanged.
    Raw,
}

/// The paper a document asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    PrinterDefault,
    A4,
    A5,
    A6,
    Letter,
    Legal,
    Custom { width_mm: u32, height_mm: u32 },
}

impl Paper {
    /// The name CUPS knows the size by (`-o media=`), none for the
    /// printer's own setting.
    pub fn cups_media(&self) -> Option<String> {
        Some(match self {
            Paper::PrinterDefault => return None,
            Paper::A4 => "A4".to_string(),
            Paper::A5 => "A5".to_string(),
            Paper::A6 => "A6".to_string(),
            Paper::Letter => "Letter".to_string(),
            Paper::Legal => "Legal".to_string(),
            Paper::Custom {
                width_mm,
                height_mm,
            } => format!("Custom.{width_mm}x{height_mm}mm"),
        })
    }
}

/// One job, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub kind: JobKind,
    pub bytes: Vec<u8>,
    pub copies: u32,
    pub paper: Paper,
    pub title: String,
}

impl Job {
    /// Refuse what Krate does not send, with words that say the limit; tidy
    /// the title (control characters out, at most [`MAX_TITLE_CHARS`]).
    pub fn check(mut self, app_name: &str) -> Result<Self, Fault> {
        if self.bytes.is_empty() {
            return Err(Fault::BadJob("there is nothing to print".to_string()));
        }
        if self.bytes.len() > MAX_JOB_BYTES {
            return Err(Fault::BadJob(format!(
                "a print job is at most {} MiB ({MAX_JOB_BYTES} bytes), not {} bytes",
                MAX_JOB_BYTES / (1024 * 1024),
                self.bytes.len()
            )));
        }
        if !(1..=MAX_COPIES).contains(&self.copies) {
            return Err(Fault::BadJob(format!(
                "copies are from 1 to {MAX_COPIES}, not {}",
                self.copies
            )));
        }
        match self.kind {
            JobKind::Pdf if !self.bytes.starts_with(b"%PDF-") => {
                return Err(Fault::BadJob(
                    "a PDF starts with %PDF-, and these bytes do not".to_string(),
                ));
            }
            JobKind::Png if !self.bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
                return Err(Fault::BadJob(
                    "a PNG starts with the PNG signature, and these bytes do not".to_string(),
                ));
            }
            _ => {}
        }
        if let Paper::Custom {
            width_mm,
            height_mm,
        } = self.paper
        {
            for side in [width_mm, height_mm] {
                if !(MIN_PAPER_MM..=MAX_PAPER_MM).contains(&side) {
                    return Err(Fault::BadJob(format!(
                        "a paper side is from {MIN_PAPER_MM} to {MAX_PAPER_MM} millimetres, \
                         not {side}"
                    )));
                }
            }
        }
        self.title = tidy_title(&self.title, app_name);
        Ok(self)
    }
}

/// A job's or a list's title: printable characters only, at most
/// [`MAX_TITLE_CHARS`], and the app's name when empty.
pub fn tidy_title(title: &str, fallback: &str) -> String {
    let tidy: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TITLE_CHARS)
        .collect();
    let tidy = tidy.trim();
    if tidy.is_empty() {
        let fallback: String = fallback
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_TITLE_CHARS)
            .collect();
        if fallback.trim().is_empty() {
            "Krate app".to_string()
        } else {
            fallback.trim().to_string()
        }
    } else {
        tidy.to_string()
    }
}

/// A printer as the system lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPrinter {
    /// The system's name for the queue. Never handed to the app.
    pub queue: String,
    /// The name to show the person.
    pub name: String,
}

/// What the system does. Every implementation is told only queues the
/// person picked, and jobs already checked.
pub trait Backend: Send + Sync {
    /// The printers set up on this computer. `Err` is why the list could
    /// not be had, in words.
    fn printers(&self) -> Result<Vec<SystemPrinter>, String>;
    /// Send one checked job to `queue`.
    fn submit(&self, queue: &str, job: &Job) -> Outcome;
}

/// The backend for this computer: a folder when [`SPOOL_ENV`] is set, else
/// the system's own printing.
pub fn system_backend() -> Box<dyn Backend> {
    if let Some(dir) = std::env::var_os(SPOOL_ENV).filter(|dir| !dir.is_empty()) {
        return Box::new(SpoolFolder::new(PathBuf::from(dir)));
    }
    #[cfg(unix)]
    {
        Box::new(Cups::system())
    }
    #[cfg(windows)]
    {
        Box::new(windows::Spooler)
    }
}

/// The printers one run was given, by handle, and where the pick is
/// remembered.
pub struct Printers {
    backend: Box<dyn Backend>,
    memory: Option<PathBuf>,
    next: u64,
    given: BTreeMap<u64, SystemPrinter>,
}

impl Printers {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            memory: None,
            next: 1,
            given: BTreeMap::new(),
        }
    }

    /// Remember picks in `path` (the CLI's file for this app's principal),
    /// or nowhere.
    pub fn with_memory(mut self, path: Option<PathBuf>) -> Self {
        self.memory = path;
        self
    }

    /// A handle for `printer`: the one it already has this run, or a fresh
    /// one. Numbers are never reused.
    fn hand_out(&mut self, printer: SystemPrinter) -> (u64, String) {
        if let Some((handle, _)) = self.given.iter().find(|(_, p)| p.queue == printer.queue) {
            return (*handle, printer.name);
        }
        let handle = self.next;
        self.next += 1;
        let name = printer.name.clone();
        self.given.insert(handle, printer);
        (handle, name)
    }

    /// The printer picked for this app before, if it is still set up. Never
    /// asks anyone.
    pub fn remembered(&mut self) -> Result<Option<(u64, String)>, Fault> {
        let Some(queue) = self.memory.as_deref().and_then(load_memory) else {
            return Ok(None);
        };
        // A system that cannot list its printers right now has none to give.
        let Ok(printers) = self.backend.printers() else {
            return Ok(None);
        };
        Ok(printers
            .into_iter()
            .find(|p| p.queue == queue)
            .map(|p| self.hand_out(p)))
    }

    /// Ask the person to choose a printer; remember the one they choose.
    /// `None` when they cancelled or nobody is there to ask.
    pub fn pick(
        &mut self,
        title: &str,
        nobody_to_ask: bool,
    ) -> Result<Option<(u64, String)>, Fault> {
        if let Some(scripted) = std::env::var_os(PICK_ENV) {
            return self.pick_scripted(&scripted.to_string_lossy());
        }
        let picked = if nobody_to_ask || !display_present() {
            None
        } else {
            let printers = self.backend.printers().map_err(|why| {
                Fault::Unavailable(format!("Krate could not list the printers: {why}"))
            })?;
            if printers.is_empty() {
                return Err(no_printers());
            }
            let title = tidy_title(title, "Choose a printer");
            walk(printers.len(), |at| {
                ask_person(&title, &printers[at].name, at, printers.len())
            })
            .map(|at| printers[at].clone())
        };
        Ok(picked.map(|printer| self.accept(printer)))
    }

    /// The pick [`PICK_ENV`] makes, by its value: a printer that is set up
    /// here, or `cancel`.
    pub fn pick_scripted(&mut self, value: &str) -> Result<Option<(u64, String)>, Fault> {
        let picked = scripted_pick(self.backend.as_ref(), value)?;
        Ok(picked.map(|printer| self.accept(printer)))
    }

    /// The person chose `printer`: remember it for the app, in place of any
    /// earlier pick, and hand out its handle.
    fn accept(&mut self, printer: SystemPrinter) -> (u64, String) {
        if let Some(path) = &self.memory {
            save_memory(path, &printer);
        }
        self.hand_out(printer)
    }

    /// Send a job to a printer this run was given.
    pub fn print(&self, handle: u64, job: Job, app_name: &str) -> Result<Outcome, Fault> {
        let printer = self.given.get(&handle).ok_or(Fault::UnknownHandle)?;
        let job = job.check(app_name)?;
        // Removed since the pick: say so rather than hand the system a
        // queue it no longer has.
        if let Ok(printers) = self.backend.printers() {
            if !printers.iter().any(|p| p.queue == printer.queue) {
                return Ok(gone(&printer.name));
            }
        }
        Ok(self.backend.submit(&printer.queue, &job))
    }
}

fn no_printers() -> Fault {
    Fault::Unavailable(
        "No printer is set up on this computer. Add one in the system's printer settings, then \
         try again."
            .to_string(),
    )
}

/// What a printer that went away answers.
pub fn gone(name: &str) -> Outcome {
    Outcome::Gone(format!(
        "The printer \"{name}\" is no longer set up on this computer. Choose a printer again."
    ))
}

/// The pick [`PICK_ENV`] makes: a printer that must be set up here, or a
/// cancel.
fn scripted_pick(backend: &dyn Backend, value: &str) -> Result<Option<SystemPrinter>, Fault> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("cancel") {
        return Ok(None);
    }
    let printers = backend
        .printers()
        .map_err(|why| Fault::Unavailable(format!("Krate could not list the printers: {why}")))?;
    printers
        .into_iter()
        .find(|p| p.queue == value)
        .map(Some)
        .ok_or_else(|| {
            Fault::Unavailable(format!(
                "{PICK_ENV} names `{value}`, which is not a printer set up on this computer"
            ))
        })
}

/// The remembered pick, as kept on disk.
#[derive(serde::Serialize, serde::Deserialize)]
struct Memory {
    schema: String,
    /// The system's name for the queue.
    printer: String,
    /// What the person was shown.
    name: String,
}

/// The queue remembered in `path`. A file that does not parse is no
/// memory: the safe reading of "I cannot tell what was picked" is "nothing
/// was", which asks again.
fn load_memory(path: &Path) -> Option<String> {
    remembered_in(path).map(|(queue, _)| queue)
}

/// The printer remembered in `path`, as `(queue, name shown)`: what
/// `krate permissions` lists.
pub fn remembered_in(path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let memory: Memory = serde_json::from_str(&text).ok()?;
    (memory.schema == MEMORY_SCHEMA && !memory.printer.is_empty())
        .then_some((memory.printer, memory.name))
}

/// Best effort: a machine that cannot write here still printed, and the
/// next run simply asks again.
fn save_memory(path: &Path, printer: &SystemPrinter) {
    let memory = Memory {
        schema: MEMORY_SCHEMA.to_string(),
        printer: printer.queue.clone(),
        name: printer.name.clone(),
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(json) = serde_json::to_vec_pretty(&memory) {
        let _ = std::fs::write(path, json);
    }
}

/// Linux needs a display for a dialog; macOS and Windows always have one.
fn display_present() -> bool {
    if cfg!(target_os = "linux") {
        ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()))
    } else {
        true
    }
}

/// One answer at one step of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Use,
    Next,
    Cancel,
}

/// Go round `count` printers asking at each, for at most [`WALK_ROUNDS`]
/// rounds: the index the person used, or `None`.
fn walk(count: usize, mut ask: impl FnMut(usize) -> Step) -> Option<usize> {
    for step in 0..count * WALK_ROUNDS {
        let at = step % count;
        match ask(at) {
            Step::Use => return Some(at),
            Step::Next => continue,
            Step::Cancel => return None,
        }
    }
    None
}

/// One step of Krate's list, as a dialog: the printer, then Use, Next (when
/// there is more than one) and Cancel.
fn ask_person(title: &str, name: &str, at: usize, count: usize) -> Step {
    const USE: &str = "Use this printer";
    const NEXT: &str = "Next printer";
    const CANCEL: &str = "Cancel";
    let mut body = format!(
        "An app asked to print. It prints only to the printer you choose, and remembers it so \
         it does not ask every time. Take it back with krate permissions --forget.\n\n{name}"
    );
    if count > 1 {
        body.push_str(&format!("\n\nPrinter {} of {count}.", at + 1));
    }
    let buttons = if count > 1 {
        rfd::MessageButtons::YesNoCancelCustom(USE.into(), NEXT.into(), CANCEL.into())
    } else {
        rfd::MessageButtons::OkCancelCustom(USE.into(), CANCEL.into())
    };
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title(title)
        .set_description(&body)
        .set_buttons(buttons)
        .show();
    match answer {
        rfd::MessageDialogResult::Custom(label) if label == USE => Step::Use,
        rfd::MessageDialogResult::Custom(label) if label == NEXT => Step::Next,
        // Some systems answer a custom button with the plain one it stands
        // in for.
        rfd::MessageDialogResult::Yes | rfd::MessageDialogResult::Ok => Step::Use,
        rfd::MessageDialogResult::No if count > 1 => Step::Next,
        _ => Step::Cancel,
    }
}

/// A folder of printers ([`SPOOL_ENV`]): each sub-folder is one, and each
/// job is written into it as `job-<n>.<pdf|png|raw>` beside `job-<n>.json`,
/// which says how it was asked to print.
pub struct SpoolFolder {
    root: PathBuf,
    jobs: Mutex<u64>,
}

impl SpoolFolder {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            jobs: Mutex::new(0),
        }
    }
}

impl Backend for SpoolFolder {
    fn printers(&self) -> Result<Vec<SystemPrinter>, String> {
        let entries = std::fs::read_dir(&self.root)
            .map_err(|err| format!("{} cannot be read: {err}", self.root.display()))?;
        let mut printers: Vec<SystemPrinter> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .map(|queue| SystemPrinter {
                name: queue.replace('_', " "),
                queue,
            })
            .collect();
        printers.sort_by(|a, b| a.queue.cmp(&b.queue));
        Ok(printers)
    }

    fn submit(&self, queue: &str, job: &Job) -> Outcome {
        let dir = self.root.join(queue);
        if !dir.is_dir() {
            return gone(queue);
        }
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        // Past any job already in the folder, so a second run adds rather
        // than overwrites.
        while dir.join(format!("job-{}.json", *jobs + 1)).exists() {
            *jobs += 1;
        }
        *jobs += 1;
        let n = *jobs;
        let ext = match job.kind {
            JobKind::Pdf => "pdf",
            JobKind::Png => "png",
            JobKind::Raw => "raw",
        };
        let note = serde_json::json!({
            "kind": ext,
            "copies": job.copies,
            "paper": job.paper.cups_media().unwrap_or_else(|| "printer-default".to_string()),
            "title": job.title,
            "bytes": job.bytes.len(),
        });
        let written =
            std::fs::write(dir.join(format!("job-{n}.{ext}")), &job.bytes).and_then(|_| {
                std::fs::write(
                    dir.join(format!("job-{n}.json")),
                    serde_json::to_vec_pretty(&note).unwrap_or_default(),
                )
            });
        match written {
            Ok(()) => Outcome::Queued(format!("{queue}-{n}")),
            Err(err) => Outcome::Failed(format!("the job could not be written: {err}")),
        }
    }
}

/// CUPS through its own commands (macOS, Linux): `lpstat` for the list,
/// `lp` for a job, which reads it from standard input -- no file is left
/// behind, and nothing goes through a shell.
#[cfg(unix)]
pub struct Cups {
    lp: PathBuf,
    lpstat: PathBuf,
}

/// How long `lp` or `lpstat` may take before Krate stops waiting.
#[cfg(unix)]
const CUPS_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(unix)]
impl Cups {
    /// The system's own `lp` and `lpstat`, by absolute path: a program
    /// called `lp` earlier on PATH is not CUPS.
    pub fn system() -> Self {
        let find = |name: &str| {
            ["/usr/bin", "/bin", "/usr/local/bin", "/opt/homebrew/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(name))
                .find(|path| path.is_file())
                .unwrap_or_else(|| Path::new("/usr/bin").join(name))
        };
        Self {
            lp: find("lp"),
            lpstat: find("lpstat"),
        }
    }

    /// These two programs instead: a test's stand-ins.
    pub fn with_programs(lp: PathBuf, lpstat: PathBuf) -> Self {
        Self { lp, lpstat }
    }

    /// The arguments `lp` gets for `job` on `queue`, after which it reads
    /// the job from standard input.
    pub fn lp_args(queue: &str, job: &Job) -> Vec<String> {
        let mut args = vec![
            "-d".to_string(),
            queue.to_string(),
            "-n".to_string(),
            job.copies.to_string(),
            "-t".to_string(),
            job.title.clone(),
        ];
        match job.kind {
            JobKind::Raw => args.extend(["-o".to_string(), "raw".to_string()]),
            JobKind::Pdf | JobKind::Png => {
                if let Some(media) = job.paper.cups_media() {
                    args.extend(["-o".to_string(), format!("media={media}")]);
                }
                if job.kind == JobKind::Png {
                    args.extend(["-o".to_string(), "fit-to-page".to_string()]);
                }
            }
        }
        args
    }

    fn run(&self, program: &Path, args: &[String], input: &[u8]) -> Result<Ran, String> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let mut child = Command::new(program)
            .args(args)
            // Words Krate can read back; the job name and the system's
            // refusal come out in the same plain form on every machine.
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                format!(
                    "this computer's printing (CUPS, {}) could not be started: {err}",
                    program.display()
                )
            })?;
        let mut stdin = child.stdin.take();
        let input = input.to_vec();
        let feeder = std::thread::spawn(move || {
            if let Some(stdin) = stdin.as_mut() {
                let _ = stdin.write_all(&input);
            }
            drop(stdin);
        });
        let deadline = std::time::Instant::now() + CUPS_WAIT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = feeder.join();
                    return Err(format!(
                        "this computer's printing did not answer within {} seconds",
                        CUPS_WAIT.as_secs()
                    ));
                }
                Err(err) => return Err(err.to_string()),
            }
        };
        let _ = feeder.join();
        let read = |pipe: Option<&mut dyn std::io::Read>| {
            let mut text = String::new();
            if let Some(pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        };
        let out = read(child.stdout.as_mut().map(|p| p as &mut dyn std::io::Read));
        let err = read(child.stderr.as_mut().map(|p| p as &mut dyn std::io::Read));
        Ok(Ran {
            ok: status.success(),
            out,
            err,
        })
    }
}

#[cfg(unix)]
struct Ran {
    ok: bool,
    out: String,
    err: String,
}

/// The system's words, without the program's own name in front.
#[cfg(unix)]
fn system_words(text: &str) -> String {
    let text = text.trim();
    let text = text
        .strip_prefix("lp:")
        .or_else(|| text.strip_prefix("lpstat:"))
        .unwrap_or(text)
        .trim();
    if text.is_empty() {
        "the printer did not take the job, and the system gave no reason".to_string()
    } else {
        text.chars().take(500).collect()
    }
}

/// The printers in `lpstat -l -p` output: a `printer <queue> ...` line
/// starts each, and its `Description:` line is the name a person knows.
#[cfg(unix)]
pub fn parse_lpstat(text: &str) -> Vec<SystemPrinter> {
    let mut printers: Vec<SystemPrinter> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            if let Some(queue) = rest.split_whitespace().next() {
                printers.push(SystemPrinter {
                    queue: queue.to_string(),
                    name: queue.replace('_', " "),
                });
            }
        } else if let Some(description) = line.trim().strip_prefix("Description:") {
            let description = description.trim();
            if let (Some(last), false) = (printers.last_mut(), description.is_empty()) {
                last.name = description.to_string();
            }
        }
    }
    printers
}

/// The job id in `lp`'s answer, "request id is Zebra-42 (1 file(s))".
#[cfg(unix)]
pub fn parse_request_id(text: &str) -> Option<String> {
    let rest = text.split("request id is ").nth(1)?;
    rest.split_whitespace().next().map(str::to_string)
}

#[cfg(unix)]
impl Backend for Cups {
    fn printers(&self) -> Result<Vec<SystemPrinter>, String> {
        let ran = self.run(&self.lpstat, &["-l".to_string(), "-p".to_string()], &[])?;
        let printers = parse_lpstat(&ran.out);
        // "No destinations added." is an empty list, not a failure.
        if !ran.ok && printers.is_empty() && !ran.err.contains("No destinations") {
            return Err(system_words(&ran.err));
        }
        Ok(printers)
    }

    fn submit(&self, queue: &str, job: &Job) -> Outcome {
        let ran = match self.run(&self.lp, &Self::lp_args(queue, job), &job.bytes) {
            Ok(ran) => ran,
            Err(why) => return Outcome::Failed(why),
        };
        if ran.ok {
            return Outcome::Queued(
                parse_request_id(&ran.out).unwrap_or_else(|| format!("a job for {queue}")),
            );
        }
        let lower = ran.err.to_ascii_lowercase();
        if lower.contains("does not exist")
            || lower.contains("unknown destination")
            || lower.contains("invalid destination")
        {
            return gone(queue);
        }
        Outcome::Failed(system_words(&ran.err))
    }
}

/// The Windows print spooler: raw printer language through `WritePrinter`
/// with the RAW data type, a PNG page drawn onto the printer with GDI.
#[cfg(windows)]
mod windows {
    use super::{gone, Backend, Job, JobKind, Outcome, SystemPrinter};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INVALID_PRINTER_NAME};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, DeleteDC, GetDeviceCaps, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HORZRES, SRCCOPY, VERTRES,
    };
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, OpenPrinterW, StartDocPrinterW,
        StartPagePrinter, WritePrinter, DOC_INFO_1W, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
        PRINTER_HANDLE, PRINTER_INFO_4W,
    };
    use windows_sys::Win32::Storage::Xps::{EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};

    pub struct Spooler;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// A NUL-terminated wide string the system handed back.
    ///
    /// # Safety
    /// `ptr` is null or points at a NUL-terminated UTF-16 string.
    unsafe fn read_wide(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }

    impl Backend for Spooler {
        fn printers(&self) -> Result<Vec<SystemPrinter>, String> {
            let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
            let mut needed = 0u32;
            let mut count = 0u32;
            // SAFETY: the first call only measures; the second fills a
            // buffer of exactly the size the first asked for.
            unsafe {
                EnumPrintersW(
                    flags,
                    std::ptr::null(),
                    4,
                    std::ptr::null_mut(),
                    0,
                    &mut needed,
                    &mut count,
                );
                if needed == 0 {
                    return Ok(Vec::new());
                }
                let mut buffer = vec![0u8; needed as usize];
                if EnumPrintersW(
                    flags,
                    std::ptr::null(),
                    4,
                    buffer.as_mut_ptr(),
                    needed,
                    &mut needed,
                    &mut count,
                ) == 0
                {
                    return Err(format!(
                        "the print spooler could not list the printers (error {})",
                        GetLastError()
                    ));
                }
                let infos = std::slice::from_raw_parts(
                    buffer.as_ptr() as *const PRINTER_INFO_4W,
                    count as usize,
                );
                Ok(infos
                    .iter()
                    .map(|info| read_wide(info.pPrinterName))
                    .filter(|name| !name.is_empty())
                    .map(|name| SystemPrinter {
                        queue: name.clone(),
                        name,
                    })
                    .collect())
            }
        }

        fn submit(&self, queue: &str, job: &Job) -> Outcome {
            match job.kind {
                JobKind::Raw => raw(queue, job),
                JobKind::Png => png(queue, job),
                JobKind::Pdf => Outcome::Failed(
                    "Windows prints a PNG page or a label printer's own language from Krate; a \
                     PDF prints on macOS and Linux"
                        .to_string(),
                ),
            }
        }
    }

    /// Printer language, byte for byte: the RAW data type skips the
    /// printer driver entirely.
    fn raw(queue: &str, job: &Job) -> Outcome {
        let name = wide(queue);
        let mut title = wide(&job.title);
        let mut datatype = wide("RAW");
        let mut handle = PRINTER_HANDLE::default();
        // SAFETY: every pointer is to a NUL-terminated buffer that outlives
        // the call; the handle is closed on every path once opened.
        unsafe {
            if OpenPrinterW(name.as_ptr(), &mut handle, std::ptr::null()) == 0 {
                let err = GetLastError();
                if err == ERROR_INVALID_PRINTER_NAME {
                    return gone(queue);
                }
                return Outcome::Failed(format!(
                    "the print spooler could not open the printer (error {err})"
                ));
            }
            let doc = DOC_INFO_1W {
                pDocName: title.as_mut_ptr(),
                pOutputFile: std::ptr::null_mut(),
                pDatatype: datatype.as_mut_ptr(),
            };
            let id = StartDocPrinterW(handle, 1, &doc);
            if id == 0 {
                let err = GetLastError();
                ClosePrinter(handle);
                return Outcome::Failed(format!(
                    "the print spooler did not take the job (error {err})"
                ));
            }
            let mut ok = StartPagePrinter(handle) != 0;
            for _ in 0..job.copies {
                let mut at = 0usize;
                while ok && at < job.bytes.len() {
                    let mut written = 0u32;
                    let chunk = &job.bytes[at..];
                    let len = chunk.len().min(1 << 20) as u32;
                    ok = WritePrinter(handle, chunk.as_ptr().cast(), len, &mut written) != 0
                        && written > 0;
                    at += written as usize;
                }
            }
            let err = if ok { 0 } else { GetLastError() };
            EndPagePrinter(handle);
            EndDocPrinter(handle);
            ClosePrinter(handle);
            if ok {
                Outcome::Queued(format!("{queue}-{id}"))
            } else {
                Outcome::Failed(format!(
                    "the print spooler stopped taking the job (error {err})"
                ))
            }
        }
    }

    /// One PNG page, scaled to fit the printable area, through GDI.
    fn png(queue: &str, job: &Job) -> Outcome {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(&job.bytes));
        // Palette and 16-bit pictures come out as plain 8-bit colour.
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = match decoder.read_info() {
            Ok(reader) => reader,
            Err(err) => return Outcome::Failed(format!("the PNG could not be read: {err}")),
        };
        let mut pixels = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
        let info = match reader.next_frame(&mut pixels) {
            Ok(info) => info,
            Err(err) => return Outcome::Failed(format!("the PNG could not be read: {err}")),
        };
        let (width, height) = (info.width as usize, info.height as usize);
        let channels = match (info.color_type, info.bit_depth) {
            (png::ColorType::Rgba, png::BitDepth::Eight) => 4,
            (png::ColorType::Rgb, png::BitDepth::Eight) => 3,
            (png::ColorType::Grayscale, png::BitDepth::Eight) => 1,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => 2,
            _ => {
                return Outcome::Failed(
                    "Windows prints an 8-bit RGB, RGBA or grey PNG; this one is another kind"
                        .to_string(),
                )
            }
        };
        // Top-down 32-bit BGRA, alpha laid over white paper.
        let mut bgra = Vec::with_capacity(width * height * 4);
        for px in pixels[..width * height * channels].chunks_exact(channels) {
            let (r, g, b, a) = match channels {
                4 => (px[0], px[1], px[2], px[3]),
                3 => (px[0], px[1], px[2], 255),
                2 => (px[0], px[0], px[0], px[1]),
                _ => (px[0], px[0], px[0], 255),
            };
            let over = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
            bgra.extend_from_slice(&[over(b), over(g), over(r), 0]);
        }
        let device = wide(queue);
        let title = wide(&job.title);
        // SAFETY: the DC is deleted on every path once created; the pixel
        // buffer and header outlive every call that reads them.
        unsafe {
            let dc = CreateDCW(
                std::ptr::null(),
                device.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            );
            if dc.is_null() {
                let err = GetLastError();
                if err == ERROR_INVALID_PRINTER_NAME {
                    return gone(queue);
                }
                return Outcome::Failed(format!(
                    "the printer could not be opened for drawing (error {err})"
                ));
            }
            let doc = DOCINFOW {
                cbSize: std::mem::size_of::<DOCINFOW>() as i32,
                lpszDocName: title.as_ptr(),
                lpszOutput: std::ptr::null(),
                lpszDatatype: std::ptr::null(),
                fwType: 0,
            };
            let id = StartDocW(dc, &doc);
            if id <= 0 {
                let err = GetLastError();
                DeleteDC(dc);
                return Outcome::Failed(format!(
                    "the print spooler did not take the job (error {err})"
                ));
            }
            let page_w = GetDeviceCaps(dc, HORZRES as i32).max(1) as f64;
            let page_h = GetDeviceCaps(dc, VERTRES as i32).max(1) as f64;
            let scale = (page_w / width as f64).min(page_h / height as f64);
            let (dest_w, dest_h) = (
                (width as f64 * scale) as i32,
                (height as f64 * scale) as i32,
            );
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negative: the rows run top to bottom.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut ok = true;
            for _ in 0..job.copies {
                ok = ok
                    && StartPage(dc) > 0
                    && StretchDIBits(
                        dc,
                        0,
                        0,
                        dest_w,
                        dest_h,
                        0,
                        0,
                        width as i32,
                        height as i32,
                        bgra.as_ptr().cast(),
                        &header,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    ) != 0
                    && EndPage(dc) > 0;
            }
            EndDoc(dc);
            DeleteDC(dc);
            if ok {
                Outcome::Queued(format!("{queue}-{id}"))
            } else {
                Outcome::Failed("the printer stopped drawing the page".to_string())
            }
        }
    }
}

/// A backend that keeps what it was sent, for tests: the printers it lists
/// and every job, in order.
#[derive(Clone, Default)]
pub struct FakeBackend {
    pub printers: Arc<Mutex<Vec<SystemPrinter>>>,
    pub jobs: Arc<Mutex<Vec<(String, Job)>>>,
    /// What every submit answers; `None` queues it.
    pub answer: Arc<Mutex<Option<Outcome>>>,
}

impl FakeBackend {
    pub fn with_printers(queues: &[&str]) -> Self {
        let fake = Self::default();
        *fake.printers.lock().unwrap() = queues
            .iter()
            .map(|q| SystemPrinter {
                queue: q.to_string(),
                name: q.replace('_', " "),
            })
            .collect();
        fake
    }
}

impl Backend for FakeBackend {
    fn printers(&self) -> Result<Vec<SystemPrinter>, String> {
        Ok(self.printers.lock().unwrap().clone())
    }

    fn submit(&self, queue: &str, job: &Job) -> Outcome {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push((queue.to_string(), job.clone()));
        self.answer
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Outcome::Queued(format!("{queue}-{}", jobs.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(copies: u32, paper: Paper) -> Job {
        Job {
            kind: JobKind::Pdf,
            bytes: b"%PDF-1.4\n%fake\n".to_vec(),
            copies,
            paper,
            title: "Traveller WO-1042".to_string(),
        }
    }

    const ZPL: &[u8] = b"^XA\r\n^FO50,50^ADN,36,20^FDWO-1042\x00\xff^FS\r\n^XZ\r\n";

    fn raw(bytes: &[u8]) -> Job {
        Job {
            kind: JobKind::Raw,
            bytes: bytes.to_vec(),
            copies: 1,
            paper: Paper::PrinterDefault,
            title: String::new(),
        }
    }

    /// The fake backend receives the job: its bytes and its options,
    /// exactly as the app sent them, on the queue the person picked.
    #[test]
    fn the_backend_receives_the_bytes_and_the_options() {
        let fake = FakeBackend::with_printers(&["Office_Laser", "Zebra_ZD421"]);
        let mut printers = Printers::new(Box::new(fake.clone()));
        printers.given.insert(
            7,
            SystemPrinter {
                queue: "Zebra_ZD421".into(),
                name: "Zebra ZD421".into(),
            },
        );
        let label = Paper::Custom {
            width_mm: 100,
            height_mm: 150,
        };
        assert_eq!(
            printers.print(7, pdf(3, label), "Travellers"),
            Ok(Outcome::Queued("Zebra_ZD421-1".into()))
        );
        assert_eq!(
            printers.print(7, raw(ZPL), "Travellers"),
            Ok(Outcome::Queued("Zebra_ZD421-2".into()))
        );
        let jobs = fake.jobs.lock().unwrap();
        assert_eq!(jobs[0].0, "Zebra_ZD421");
        assert_eq!(jobs[0].1, pdf(3, label));
        // Raw printer language reaches the backend byte for byte, NULs and
        // all, and an empty title is named after the app.
        assert_eq!(jobs[1].1.bytes, ZPL);
        assert_eq!(jobs[1].1.title, "Travellers");
    }

    /// A handle nobody gave reaches no printer, and nothing is sent.
    #[test]
    fn a_made_up_handle_reaches_nothing() {
        let fake = FakeBackend::with_printers(&["Zebra_ZD421"]);
        let mut printers = Printers::new(Box::new(fake.clone()));
        // One printer was given, as handle 7: no other number reaches it.
        printers.given.insert(
            7,
            SystemPrinter {
                queue: "Zebra_ZD421".into(),
                name: "Zebra ZD421".into(),
            },
        );
        for handle in [0, 1, 6, 8, u64::MAX] {
            assert_eq!(
                printers.print(handle, raw(ZPL), "app"),
                Err(Fault::UnknownHandle)
            );
        }
        assert!(fake.jobs.lock().unwrap().is_empty());
    }

    /// Every bound refuses in words, and nothing is sent.
    #[test]
    fn a_job_out_of_bounds_is_refused_in_words() {
        let fake = FakeBackend::with_printers(&["P"]);
        let mut printers = Printers::new(Box::new(fake.clone()));
        printers.given.insert(
            1,
            SystemPrinter {
                queue: "P".into(),
                name: "P".into(),
            },
        );
        let bad = |job: Job| match printers.print(1, job, "app") {
            Err(Fault::BadJob(words)) => words,
            other => panic!("refused, got {other:?}"),
        };
        assert!(bad(raw(b"")).contains("nothing to print"));
        assert!(bad(raw(&vec![b'x'; MAX_JOB_BYTES + 1])).contains("at most 16 MiB"));
        assert!(bad(pdf(0, Paper::A4)).contains("copies are from 1 to 100"));
        assert!(bad(pdf(MAX_COPIES + 1, Paper::A4)).contains("copies"));
        let mut not_pdf = pdf(1, Paper::A4);
        not_pdf.bytes = b"%!PS-Adobe".to_vec();
        assert!(bad(not_pdf).contains("%PDF-"));
        let mut not_png = pdf(1, Paper::A4);
        not_png.kind = JobKind::Png;
        assert!(bad(not_png).contains("PNG signature"));
        assert!(bad(pdf(
            1,
            Paper::Custom {
                width_mm: 9,
                height_mm: 100
            }
        ))
        .contains("from 10 to 1000"));
        // The largest job that is allowed goes.
        assert!(matches!(
            printers.print(1, raw(&vec![b'x'; MAX_JOB_BYTES]), "app"),
            Ok(Outcome::Queued(_))
        ));
        assert_eq!(fake.jobs.lock().unwrap().len(), 1);
    }

    /// A printer removed since the pick answers gone, and the backend is
    /// not handed a queue the system no longer has.
    #[test]
    fn a_printer_removed_since_the_pick_is_gone() {
        let fake = FakeBackend::with_printers(&["Zebra_ZD421"]);
        let mut printers = Printers::new(Box::new(fake.clone()));
        printers.given.insert(
            1,
            SystemPrinter {
                queue: "Zebra_ZD421".into(),
                name: "Zebra ZD421".into(),
            },
        );
        fake.printers.lock().unwrap().clear();
        assert!(matches!(
            printers.print(1, raw(ZPL), "app"),
            Ok(Outcome::Gone(words)) if words.contains("\"Zebra ZD421\" is no longer set up")
        ));
        assert!(fake.jobs.lock().unwrap().is_empty());
        // The system's own refusal comes back as its words.
        fake.printers.lock().unwrap().push(SystemPrinter {
            queue: "Zebra_ZD421".into(),
            name: "Zebra ZD421".into(),
        });
        *fake.answer.lock().unwrap() = Some(Outcome::Failed("Printer is paused".into()));
        assert_eq!(
            printers.print(1, raw(ZPL), "app"),
            Ok(Outcome::Failed("Printer is paused".into()))
        );
    }

    /// The memory: kept in the file the caller names, read back next run,
    /// ignored when it cannot be trusted, and gone with the file.
    #[test]
    fn a_pick_is_remembered_in_the_callers_file_only() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir
            .path()
            .join("grants")
            .join("printers")
            .join("app@k.json");
        let fake = FakeBackend::with_printers(&["Office_Laser", "Zebra_ZD421"]);
        let printer = SystemPrinter {
            queue: "Zebra_ZD421".into(),
            name: "Zebra ZD421".into(),
        };
        save_memory(&path, &printer);
        assert_eq!(
            remembered_in(&path),
            Some(("Zebra_ZD421".into(), "Zebra ZD421".into()))
        );
        let mut next_run = Printers::new(Box::new(fake.clone())).with_memory(Some(path.clone()));
        assert_eq!(
            next_run.remembered(),
            Ok(Some((1, "Zebra ZD421".to_string())))
        );
        // Asked again in the same run, the same handle.
        assert_eq!(
            next_run.remembered(),
            Ok(Some((1, "Zebra ZD421".to_string())))
        );
        // No file named: nothing is remembered, whatever is on disk.
        assert_eq!(Printers::new(Box::new(fake.clone())).remembered(), Ok(None));
        // The printer was removed: nothing, rather than a dead handle.
        fake.printers
            .lock()
            .unwrap()
            .retain(|p| p.queue != "Zebra_ZD421");
        let mut removed = Printers::new(Box::new(fake.clone())).with_memory(Some(path.clone()));
        assert_eq!(removed.remembered(), Ok(None));
        // A file in another shape, or forgotten, is no memory.
        std::fs::write(
            &path,
            br#"{"schema":"krate.grants.v1","printer":"Zebra_ZD421","name":"x"}"#,
        )
        .expect("write");
        assert_eq!(remembered_in(&path), None);
        std::fs::remove_file(&path).expect("forget");
        assert_eq!(remembered_in(&path), None);
    }

    /// The list goes round, uses what the person uses, and a person who
    /// only ever says "next" has cancelled.
    #[test]
    fn the_list_walks_to_the_printer_the_person_uses() {
        let mut seen = Vec::new();
        let picked = walk(3, |at| {
            seen.push(at);
            if seen.len() == 5 {
                Step::Use
            } else {
                Step::Next
            }
        });
        assert_eq!(picked, Some(1));
        assert_eq!(seen, vec![0, 1, 2, 0, 1]);
        assert_eq!(walk(2, |_| Step::Next), None);
        assert_eq!(walk(2, |_| Step::Cancel), None);
    }

    /// A title is printable, bounded, and the app's name when empty.
    #[test]
    fn a_title_is_tidied() {
        assert_eq!(tidy_title("WO-1\n042\u{7}", "app"), "WO-1 042");
        assert_eq!(tidy_title("   ", "Travellers"), "Travellers");
        assert_eq!(tidy_title("", ""), "Krate app");
        assert_eq!(
            tidy_title(&"x".repeat(500), "app").chars().count(),
            MAX_TITLE_CHARS
        );
    }

    /// The folder backend: sub-folders are printers, a job lands as its
    /// bytes and its options, and a deleted folder is a printer gone.
    #[test]
    fn the_spool_folder_keeps_each_job_and_its_options() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::create_dir(dir.path().join("Zebra_ZD421")).expect("printer");
        std::fs::write(dir.path().join("stray.txt"), b"x").expect("file");
        let spool = SpoolFolder::new(dir.path().to_path_buf());
        assert_eq!(
            spool.printers(),
            Ok(vec![SystemPrinter {
                queue: "Zebra_ZD421".into(),
                name: "Zebra ZD421".into()
            }])
        );
        let mut job = raw(ZPL);
        job.title = "Label".into();
        job.copies = 2;
        assert_eq!(
            spool.submit("Zebra_ZD421", &job),
            Outcome::Queued("Zebra_ZD421-1".into())
        );
        let printed = dir.path().join("Zebra_ZD421");
        assert_eq!(std::fs::read(printed.join("job-1.raw")).expect("job"), ZPL);
        let note: serde_json::Value =
            serde_json::from_slice(&std::fs::read(printed.join("job-1.json")).expect("note"))
                .expect("json");
        assert_eq!(note["copies"], 2);
        assert_eq!(note["kind"], "raw");
        assert_eq!(note["title"], "Label");
        // A second backend over the same folder adds after the first.
        let again = SpoolFolder::new(dir.path().to_path_buf());
        assert_eq!(
            again.submit("Zebra_ZD421", &job),
            Outcome::Queued("Zebra_ZD421-2".into())
        );
        std::fs::remove_dir_all(&printed).expect("remove");
        assert!(matches!(
            spool.submit("Zebra_ZD421", &job),
            Outcome::Gone(_)
        ));
    }

    /// What CUPS is asked, and how its answers are read.
    #[cfg(unix)]
    #[test]
    fn cups_is_asked_plainly_and_read_back() {
        let label = Paper::Custom {
            width_mm: 100,
            height_mm: 150,
        };
        assert_eq!(
            Cups::lp_args("Office", &pdf(2, label)),
            [
                "-d",
                "Office",
                "-n",
                "2",
                "-t",
                "Traveller WO-1042",
                "-o",
                "media=Custom.100x150mm"
            ]
        );
        let mut page = pdf(1, Paper::A4);
        page.kind = JobKind::Png;
        assert!(Cups::lp_args("Office", &page).ends_with(&[
            "-o".to_string(),
            "media=A4".to_string(),
            "-o".to_string(),
            "fit-to-page".to_string()
        ]));
        let mut label_job = raw(ZPL);
        label_job.title = "-o raw; rm -rf".into();
        let args = Cups::lp_args("Zebra", &label_job);
        assert_eq!(&args[4..], ["-t", "-o raw; rm -rf", "-o", "raw"]);
        assert_eq!(
            parse_request_id("request id is Zebra_ZD421-42 (1 file(s))\n").as_deref(),
            Some("Zebra_ZD421-42")
        );
        let listed = parse_lpstat(
            "printer Zebra_ZD421 is idle.  enabled since Thu Nov 20 09:00:00 2026\n\
             \tForm mounted:\n\tDescription: Zebra ZD421 (shipping)\n\
             printer Office disabled since Thu Nov 20 09:00:00 2026 -\n\tpaused\n",
        );
        assert_eq!(
            listed,
            vec![
                SystemPrinter {
                    queue: "Zebra_ZD421".into(),
                    name: "Zebra ZD421 (shipping)".into()
                },
                SystemPrinter {
                    queue: "Office".into(),
                    name: "Office".into()
                }
            ]
        );
        assert_eq!(
            system_words("lp: The printer or class does not exist.\n"),
            "The printer or class does not exist."
        );
    }

    /// A real CUPS queue, when one is named: KRATE_TEST_CUPS_QUEUE a raw
    /// queue on a `file:` device, KRATE_TEST_CUPS_OUT the file it writes.
    /// A raw ZPL job lands there byte for byte. Without the two variables
    /// it proves nothing and says so; no CI lane sets them yet.
    #[cfg(unix)]
    #[test]
    fn a_real_cups_queue_prints_raw_bytes_unchanged() {
        let (Some(queue), Some(out)) = (
            std::env::var_os("KRATE_TEST_CUPS_QUEUE"),
            std::env::var_os("KRATE_TEST_CUPS_OUT"),
        ) else {
            eprintln!("no KRATE_TEST_CUPS_QUEUE: the real CUPS queue is not checked here");
            return;
        };
        let queue = queue.to_string_lossy().into_owned();
        let cups = Cups::system();
        assert!(
            cups.printers()
                .expect("lpstat")
                .iter()
                .any(|p| p.queue == queue),
            "{queue} is listed"
        );
        let mut job = raw(ZPL);
        job.title = "Label".into();
        assert!(matches!(cups.submit(&queue, &job), Outcome::Queued(_)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::fs::read(&out).unwrap_or_default() != ZPL {
            assert!(
                std::time::Instant::now() < deadline,
                "the job never reached {}",
                std::path::Path::new(&out).display()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// CUPS's own programs, stood in for by scripts: the job goes in on
    /// standard input byte for byte with the arguments above, the request id
    /// comes back, and CUPS's refusals become failed and gone.
    #[cfg(unix)]
    #[test]
    fn cups_receives_the_job_on_standard_input() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("dir");
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
            path
        };
        let got = dir.path().join("got");
        let lp = script(
            "lp",
            &format!(
                "printf '%s\\n' \"$@\" > '{args}'\ncat > '{job}'\n\
                 case \"$2\" in Gone) echo 'lp: The printer or class does not exist.' >&2; exit 1;;\n\
                 Paused) echo 'lp: Destination \"Paused\" is not accepting jobs.' >&2; exit 1;; esac\n\
                 echo \"request id is $2-7 (1 file(s))\"",
                args = got.with_extension("args").display(),
                job = got.with_extension("job").display()
            ),
        );
        let lpstat = script(
            "lpstat",
            "echo 'printer Zebra is idle.  enabled since today'\n\
             echo '\tDescription: Zebra ZD421'",
        );
        let cups = Cups::with_programs(lp, lpstat);
        assert_eq!(
            cups.printers(),
            Ok(vec![SystemPrinter {
                queue: "Zebra".into(),
                name: "Zebra ZD421".into()
            }])
        );
        let mut job = raw(ZPL);
        job.title = "Label".into();
        job.copies = 3;
        assert_eq!(
            cups.submit("Zebra", &job),
            Outcome::Queued("Zebra-7".into())
        );
        assert_eq!(std::fs::read(got.with_extension("job")).expect("job"), ZPL);
        assert_eq!(
            std::fs::read_to_string(got.with_extension("args")).expect("args"),
            "-d\nZebra\n-n\n3\n-t\nLabel\n-o\nraw\n"
        );
        assert!(matches!(cups.submit("Gone", &job), Outcome::Gone(_)));
        assert_eq!(
            cups.submit("Paused", &job),
            Outcome::Failed("Destination \"Paused\" is not accepting jobs.".into())
        );
        // No CUPS at all: the words say so.
        let none = Cups::with_programs(dir.path().join("no-lp"), dir.path().join("no-lpstat"));
        assert!(matches!(none.printers(), Err(words) if words.contains("could not be started")));
    }
}
