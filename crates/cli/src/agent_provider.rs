//! The AI provider seam: which coding agents `--agent` can drive.
//!
//! Krate is not tied to one AI. A provider answers four questions -- what it is
//! called, how to check it is installed, how to invoke it headlessly, and how to
//! read its output -- and Krate owns everything else. The prompt, the timeout,
//! the guard against an agent that answered in chat without writing code, and
//! the `check-app` verdict are Krate's authoring policy, identical for every
//! provider, so they cannot drift apart as providers are added.
//!
//! Adding a provider is a new implementation plus one line in [`PROVIDERS`].
//!
//! The design and the verified invocation for each provider we plan to support
//! are in `Plan/ai-providers-2026-08.md`.

use std::process::{Command as ProcessCommand, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// One AI coding agent Krate knows how to drive.
pub trait AgentProvider: Send + Sync {
    /// The value accepted after `--agent`, e.g. `claude`. Lowercase and stable:
    /// this is a user-facing name that appears in commands people save.
    fn name(&self) -> &'static str;

    /// One line describing the provider, for `--help` and for the listing shown
    /// when someone names a provider that does not exist.
    fn description(&self) -> &'static str;

    /// The executable Krate spawns. Looked up on PATH to decide whether this
    /// provider is available on this machine.
    fn program(&self) -> &'static str;

    /// What to tell someone whose machine does not have this CLI installed.
    fn install_hint(&self) -> &'static str;

    /// The npm package that installs this CLI, when there is one.
    ///
    /// `install_hint` is prose for a person reading a terminal. This is the
    /// same fact in a form a program can act on, so Krate Studio can run the
    /// install itself instead of printing a command and expecting someone to
    /// find a terminal, leave the app, and come back. `None` means the tool
    /// cannot be installed unattended and the hint is the only answer.
    fn install_package(&self) -> Option<&'static str> {
        None
    }

    /// The arguments for one headless authoring run of `prompt`.
    fn author_args(&self, prompt: &str) -> Vec<String>;

    /// Arguments for a short text-only call: a prompt in, prose out, no
    /// tools, no file edits. Used by `krate plan`, which must answer in
    /// seconds. The default reuses the authoring arguments -- every
    /// provider can serve a text answer through them, just slower --
    /// and providers with a lighter one-shot mode override it.
    fn plan_args(&self, prompt: &str) -> Vec<String> {
        self.author_args(prompt)
    }

    /// A cheap round trip that proves the tool works, not merely that it is on
    /// PATH. Defaults to asking it to echo one word: short enough to be quick,
    /// real enough that an expired sign-in fails it the way it would fail a
    /// genuine authoring run.
    fn probe_args(&self) -> Vec<String> {
        self.author_args("Reply with the single word: ok")
    }

    /// What to run to sign in, for the provider whose credentials have expired.
    fn login_hint(&self) -> String {
        format!("{} login", self.program())
    }

    /// Provider-specific spawn setup.
    ///
    /// The default closes stdin, which every provider needs: a headless agent
    /// that inherits the parent's stdin can block forever waiting for input
    /// that never arrives.
    fn configure(&self, command: &mut ProcessCommand) {
        command.stdin(Stdio::null());
    }

    /// Turn one line of the agent's streamed output into a plain-English
    /// progress line, or `None` for lines a person watching does not care
    /// about.
    ///
    /// Best-effort by design: an output shape we do not recognize prints
    /// nothing, and the raw line still reaches the transcript.
    fn progress_line(&self, line: &str) -> Option<String>;

    /// The RAW tool call in a streamed line, as `(tool, target)`, for the
    /// pipeline trace -- not the polished progress sentence.
    ///
    /// `progress_line` only fires for calls that map to a user-facing step, so
    /// it misses the agent's own exploration: on the study's first run, claude
    /// read the pack and the whole example repo with `Bash`/`cat`/`sed`, and
    /// none of it showed up because those are not progress steps. The study
    /// needs the full sequence -- every read, every command -- to answer "what
    /// did it read, and where did it go outside its workspace". This returns the
    /// tool name and its primary argument (a file path or a command) for exactly
    /// that. Default finds nothing; each provider overrides for its own shape.
    fn raw_tool_call(&self, _line: &str) -> Option<(String, String)> {
        None
    }

    /// Whether this provider streams what it is doing while it works.
    ///
    /// Claude Code emits an event per tool call, so a progress display can
    /// follow along. Grok writes one JSON object when the whole run finishes,
    /// so there is genuinely nothing to show in between -- and somebody
    /// watching a still screen for ten minutes concludes it has hung and kills
    /// it. Providers that cannot stream say so, and the front door warns.
    fn reports_progress(&self) -> bool {
        true
    }

    /// Whether the run failed. Exit status is the truth for most providers; one
    /// that reports failure inside its final event can override this.
    fn failed(&self, status: &ExitStatus) -> bool {
        !status.success()
    }

    /// A broken-environment signature in the tool's own output, for a tool that
    /// exits 0 while having actually failed. Returns (reason, remedy) if found.
    ///
    /// Codex logs its Windows sandbox helper failure to stderr and then exits
    /// 0, so status and empty-output checks both pass while every real build
    /// fails. The default finds nothing; codex overrides.
    fn output_failure(&self, _stdout: &str, _stderr: &str) -> Option<(String, Option<String>)> {
        None
    }
}

/// Every provider Krate can drive, in the order they are listed to a person.
///
/// This is the whole registry. A new provider is appended here and nowhere
/// else: `--agent` accepts it, the error listing mentions it, and the
/// installed-check covers it, all from this one line.
pub const PROVIDERS: &[&dyn AgentProvider] = &[
    &ClaudeProvider,
    &CodexProvider,
    &GeminiProvider,
    &CopilotProvider,
    &GrokProvider,
];

/// Look up a provider by the name given to `--agent`.
///
/// Returns a listing error rather than a bare "invalid value", because the
/// useful reply to a name we do not know is the set of names we do know.
pub fn resolve(name: &str) -> Result<&'static dyn AgentProvider, String> {
    let wanted = name.trim().to_ascii_lowercase();
    if let Some(provider) = PROVIDERS.iter().find(|p| p.name() == wanted) {
        return Ok(*provider);
    }
    let mut message = format!("unknown AI provider \"{name}\".\n\nAvailable providers:\n");
    for provider in PROVIDERS {
        message.push_str(&format!(
            "  {:<8} {}\n",
            provider.name(),
            provider.description()
        ));
    }
    message.push_str(
        "\nOr use --author-cmd <command> to drive any other tool: it is handed \
         KRATE_APP_DIR, KRATE_APP_NAME, and KRATE_REQUEST.",
    );
    Err(message)
}

/// Whether this provider's CLI is installed on this machine.
///
/// One shared implementation on purpose: "is this program on PATH" has exactly
/// one right answer, and a provider allowed to define its own could only get it
/// wrong.
/// The first provider whose CLI is on this machine, in PROVIDERS order --
/// the same preference `krate ai` displays. For callers that need "whichever
/// AI the person has" without asking.
pub fn first_installed() -> Option<&'static dyn AgentProvider> {
    PROVIDERS.iter().copied().find(|p| is_installed(*p))
}

pub fn is_installed(provider: &dyn AgentProvider) -> bool {
    which_on_path(provider.program()).is_some()
}

/// What a provider can actually do right now, as opposed to whether its binary
/// exists.
///
/// `is_installed` only looks at PATH, and PATH lies. Tested cold on a clean
/// machine, three of four "installed" tools could not write an app: Claude's
/// sign-in had expired, Codex refused outside a git repository, and Copilot
/// exited 1 with empty stdout *and* empty stderr. Offering those in a menu is
/// worse than not offering them, because the person picks one and blames Krate
/// for the failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// Ran, answered, and can be used right now.
    Working,
    /// Installed, but refused to work, with the reason it gave.
    NotReady {
        /// One line fit to print in a menu.
        summary: String,
        /// The command that most likely fixes it, if there is an obvious one.
        remedy: Option<String>,
    },
    /// Not on PATH at all.
    Missing,
}

impl Readiness {
    pub fn is_working(&self) -> bool {
        matches!(self, Readiness::Working)
    }
}

/// Actually run the provider and see whether it answers.
///
/// Deliberately a real invocation rather than a version check: `--version`
/// passes for a tool whose sign-in has expired, which is the single most common
/// way these fail. The prompt is trivial so the round trip is short, and the
/// whole thing is bounded by `timeout` so a hung tool cannot freeze a menu.
/// Read a child pipe to EOF into a String, so a reader thread can keep it
/// drained. Any read error yields what was gathered so far rather than losing
/// it; a missing pipe is an empty string.
fn read_all(pipe: Option<impl std::io::Read>) -> String {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

pub fn probe(provider: &dyn AgentProvider, timeout: Duration) -> Readiness {
    let Some(path) = which_on_path(provider.program()) else {
        return Readiness::Missing;
    };

    let mut command = ProcessCommand::new(&path);
    with_tool_path(&mut command);
    for arg in provider.probe_args() {
        command.arg(arg);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return Readiness::NotReady {
                summary: format!("could not be started ({err})"),
                remedy: None,
            };
        }
    };

    // Drain both pipes in their own threads for the whole run.
    //
    // The single-threaded poll below reads nothing until the process exits, so
    // a chatty tool that fills its ~64KB stderr pipe blocks on the write and
    // never exits -- try_wait returns "still running" forever and even a
    // timeout kill leaves the wedged children behind. A codex whose sandbox
    // helper is broken does exactly this, and a probe meant to CATCH that tool
    // instead hung and piled up processes until the machine bogged down. Two
    // reader threads keep the pipes empty so the child can always make
    // progress -- to its answer, or to its own error and exit.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || read_all(stdout_pipe));
    let err_handle = std::thread::spawn(move || read_all(stderr_pipe));

    // Poll rather than wait, so a tool that never answers is reported as slow
    // instead of hanging the caller forever.
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_handle.join().unwrap_or_default();
                let stderr = err_handle.join().unwrap_or_default();
                let stdout = stdout.trim().to_string();
                let stderr = stderr.trim().to_string();
                // A tool can exit 0 while its environment is actually broken --
                // codex logs its Windows sandbox helper failure and exits 0
                // anyway. Catch that from the output before trusting the exit
                // code, or the probe passes a tool that fails every real build.
                if let Some((reason, remedy)) = provider.output_failure(&stdout, &stderr) {
                    return Readiness::NotReady {
                        summary: reason,
                        remedy,
                    };
                }
                // Exit 0 with output on *either* stream. Codex reports a
                // healthy login on stderr, so demanding stdout marked a working
                // tool as broken. The empty-and-failed case is still caught
                // below, which is the one that matters.
                if status.success() && !(stdout.is_empty() && stderr.is_empty()) {
                    return Readiness::Working;
                }
                return diagnose(provider, &stdout, &stderr);
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    // Kill the whole tree, not just the direct child. A hung
                    // codex has spawned a sandbox helper subprocess; killing
                    // only the parent leaves that child running. On Windows
                    // taskkill /T reaches the tree; elsewhere killing the child
                    // is enough because these tools do not detach. The reader
                    // threads end on their own once the pipes close.
                    #[cfg(windows)]
                    {
                        let _ = ProcessCommand::new("taskkill")
                            .args(["/PID", &child.id().to_string(), "/T", "/F"])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                    let _ = child.kill();
                    return Readiness::NotReady {
                        summary: format!("did not answer within {}s", timeout.as_secs()),
                        remedy: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            Err(err) => {
                return Readiness::NotReady {
                    summary: format!("could not be checked ({err})"),
                    remedy: None,
                };
            }
        }
    }
}

/// Turn whatever the tool printed into one line a person can act on.
///
/// The hardest case is a tool that fails with nothing printed at all. Saying
/// "unknown" is honest and still useful, because the menu can then offer a
/// different AI rather than pretending the choice was fine.
fn diagnose(provider: &dyn AgentProvider, stdout: &str, stderr: &str) -> Readiness {
    let blob = format!("{stdout}\n{stderr}").to_lowercase();
    let name = provider.name();

    let (summary, remedy) = if blob.contains("not logged in")
        || blob.contains("unauthorized")
        || blob.contains("authentication")
        || blob.contains("expired")
        || blob.contains("sign in")
        || blob.contains("login")
        || blob.contains("api key")
    {
        (
            "is installed but not signed in".to_string(),
            Some(provider.login_hint().to_string()),
        )
    } else if blob.contains("git repository") || blob.contains("not a git repo") {
        (
            "will only run inside a git repository".to_string(),
            Some("git init".to_string()),
        )
    } else if blob.contains("rate limit") || blob.contains("quota") {
        ("has hit its usage limit".to_string(), None)
    } else if blob.contains("requires a newer version")
        || blob.contains("please upgrade to the latest")
        || (blob.contains("model") && blob.contains("not found") && blob.contains("upgrade"))
    {
        // The tool runs but its own service refuses it as out of date. This is
        // the exact state a real codex install was in -- CLI 0.142.0 asking for
        // a model that needs a newer build -- and the old code reported the
        // harmless "Reading additional input from stdin..." notice instead,
        // which told nobody anything.
        (
            "is installed but out of date -- its service asks for a newer version".to_string(),
            Some(match provider.name() {
                "codex" => "npm install -g @openai/codex@latest".to_string(),
                other => format!("update {other} to the latest version"),
            }),
        )
    } else if stdout.is_empty() && stderr.is_empty() {
        ("fails to start, and prints no reason why".to_string(), None)
    } else {
        // The FIRST line is usually not the reason. Codex opens every run with
        // "Reading additional input from stdin...", and reporting that as the
        // verdict hid a perfectly actionable error underneath it. Prefer a line
        // that actually looks like a failure, and only fall back to the first
        // line when nothing does.
        let looks_like_error = |l: &str| {
            let low = l.to_lowercase();
            (low.contains("error")
                || low.contains("failed")
                || low.contains("cannot")
                || low.contains("not found")
                || low.contains("refused")
                || low.contains("denied"))
                && !low.contains("reading additional input")
        };
        let lines: Vec<&str> = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let chosen = lines
            .iter()
            .copied()
            .find(|l| looks_like_error(l))
            .or_else(|| lines.first().copied())
            .unwrap_or("failed for an unknown reason");
        // Pull the human sentence out of a JSON error blob when there is one,
        // so a person reads the message rather than the envelope.
        let message = extract_json_message(chosen).unwrap_or_else(|| chosen.to_string());
        let clipped: String = message.chars().take(120).collect();
        (clipped, None)
    };

    let _ = name;
    Readiness::NotReady { summary, remedy }
}

/// Pull the human sentence out of a JSON error line, if it is one.
///
/// Codex reports failures as `{"type":"error","message":"{\"error\":{\"message\":
/// \"...\"}}"}` -- the sentence a person needs, wrapped in one or two layers of
/// envelope. Showing the raw blob makes a readable error unreadable, so dig for
/// the innermost `message` and show that. Anything unrecognised returns None
/// and the caller keeps the original line.
fn extract_json_message(line: &str) -> Option<String> {
    fn deepest(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => {
                // An inner error object wins over the outer wrapper's message.
                if let Some(found) = map.get("error").and_then(deepest) {
                    return Some(found);
                }
                if let Some(msg) = map.get("message") {
                    if let Some(text) = msg.as_str() {
                        // The message may itself be JSON; unwrap once more.
                        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(text) {
                            if let Some(found) = deepest(&inner) {
                                return Some(found);
                            }
                        }
                        return Some(text.to_string());
                    }
                    return deepest(msg);
                }
                None
            }
            _ => None,
        }
    }
    let start = line.find('{')?;
    let value: serde_json::Value = serde_json::from_str(line[start..].trim()).ok()?;
    deepest(&value)
}

/// The error for a provider whose CLI is not installed.
///
/// Without this, asking for a provider you do not have produces a raw spawn
/// failure naming a program you never typed. This says what is missing and how
/// to get it.
pub fn missing_cli_error(provider: &dyn AgentProvider) -> String {
    format!(
        "the `{program}` command is not installed, so Krate cannot use {name} to write your app.\n\n\
         {hint}\n\n\
         Already installed it? Open a new terminal so `{program}` is on your PATH, or use \
         --author-cmd to point Krate straight at it.",
        program = provider.program(),
        name = provider.name(),
        hint = provider.install_hint(),
    )
}

/// The places AI CLIs install, beyond whatever PATH happens to say.
///
/// An app launched from Finder does NOT inherit a shell PATH -- it gets the
/// bare system one. Every AI tool installs somewhere else: npm globals under a
/// version manager, Homebrew, or `~/.local/bin`. So Krate Studio reported
/// "Claude - not installed" on a machine with Claude installed and signed in,
/// while `krate ai` in a terminal said it was working. Same code, different
/// PATH, and the GUI is the one a person actually sees.
///
/// Searched after PATH, so an explicit PATH entry still wins.
fn extra_tool_dirs() -> Vec<std::path::PathBuf> {
    // An explicitly empty PATH means "find nothing", and it must keep meaning
    // that. Tests set `PATH=""` to prove the missing-tool message, and a
    // person who empties PATH is asking for exactly this. Searching our
    // fallback list anyway would make PATH unfalsifiable.
    if matches!(std::env::var("PATH").as_deref(), Ok("")) {
        return Vec::new();
    }

    let mut dirs = Vec::new();
    if let Some(home) = crate::home_dir() {
        // The Rust toolchain: authoring BUILDS the app, and a GUI-launched
        // engine saw no cargo, decided the machine had no compiler, and set
        // off installing one -- with an error tail the studio misread as a
        // sign-in problem. Terminals never hit this because shells carry
        // ~/.cargo/bin.
        dirs.push(home.join(".cargo/bin"));
        // Where Claude Code and many single-binary installers land.
        dirs.push(home.join(".local/bin"));
        // Grok's installer puts its agent here on every OS.
        dirs.push(home.join(".grok/bin"));
        dirs.push(home.join("bin"));
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join(".volta/bin"));
        dirs.push(home.join(".deno/bin"));
        // A desktop app the person already installed often CARRIES the CLI we
        // need. The ChatGPT app ships a full codex (verified: codex-cli
        // 0.148.0-alpha.15 with `exec`) in Contents/Resources. Somebody who
        // installed the desktop app and does not want a separate CLI install
        // already has everything Krate needs -- we simply were not looking.
        // Checking these costs one stat each and turns "you must install a CLI"
        // into "you are already set up".
        #[cfg(target_os = "macos")]
        {
            for app in [
                "/Applications/ChatGPT.app/Contents/Resources",
                "/Applications/Codex.app/Contents/Resources",
                "/Applications/Claude.app/Contents/Resources",
            ] {
                dirs.push(std::path::PathBuf::from(app));
            }
            // The same apps, installed for one user rather than system-wide.
            for app in [
                "Applications/ChatGPT.app/Contents/Resources",
                "Applications/Codex.app/Contents/Resources",
                "Applications/Claude.app/Contents/Resources",
            ] {
                dirs.push(home.join(app));
            }
        }
        // npm globals under a Node version manager, newest version first.
        for (root, tail) in [
            (home.join(".nvm/versions/node"), "bin"),
            (home.join(".fnm/node-versions"), "installation/bin"),
        ] {
            if let Ok(entries) = std::fs::read_dir(&root) {
                let mut found: Vec<std::path::PathBuf> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().join(tail))
                    .collect();
                found.sort();
                found.reverse();
                dirs.extend(found);
            }
        }
        dirs.push(home.join(".npm-global/bin"));
    }
    // Homebrew, both architectures, and the usual system prefixes.
    dirs.push(std::path::PathBuf::from("/opt/homebrew/bin"));
    dirs.push(std::path::PathBuf::from("/usr/local/bin"));
    dirs
}

/// Keep a spawned child's console window off the screen when this process
/// has no console of its own.
///
/// A GUI-launched engine has none: the studio spawns it hidden, and a
/// double-clicked engine detaches the console it solely owns. From that
/// state, spawning a console-subsystem child makes Windows mint a brand-new
/// terminal window right on screen -- seen as agent windows popping up over
/// the studio after every probe, each one stealing focus and so triggering
/// the studio's on-focus re-probe into the next popup. With a real terminal
/// attached this does nothing, so CLI runs keep ordinary console behavior.
pub fn hide_child_console(command: &mut ProcessCommand) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let attached = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
        if attached.is_null() {
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// Give a spawned tool a PATH that includes where tools actually live.
///
/// Finding the binary is only half of it. These CLIs shell out to `node`, to
/// `git`, and to each other, and they read credentials through helpers that
/// have to be findable. Launched from Finder the child inherits a bare system
/// PATH, so a tool that IS signed in reports "not signed in" -- which is what
/// the studio showed for Claude on a machine where it worked fine in a
/// terminal.
///
/// Existing PATH entries keep their priority; the extra directories are
/// appended.
pub fn with_tool_path(command: &mut ProcessCommand) {
    hide_child_console(command);
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&existing)
        .chain(extra_tool_dirs())
        .collect::<Vec<_>>();
    if let Ok(joined) = std::env::join_paths(dirs) {
        command.env("PATH", joined);
    }

    // USER, if the launcher did not set it.
    //
    // Claude Code reads its credentials from the login keychain and needs
    // USER to find the entry; without it, a tool that is signed in answers
    // "Not logged in - Please run /login". Measured one variable at a time:
    // USER alone fixes it, and LOGNAME, SHELL and TMPDIR do not. A GUI app
    // launched from Finder does not reliably have USER set, which is why the
    // studio reported "Claude - not installed" on a machine where the same
    // check passed in a terminal.
    if std::env::var_os("USER").is_none() {
        if let Some(home) = crate::home_dir() {
            if let Some(name) = home.file_name() {
                command.env("USER", name);
                command.env("LOGNAME", name);
            }
        }
    }
}

/// Find an executable on PATH, honoring PATHEXT on Windows.
///
/// Also searches [`extra_tool_dirs`], because a GUI app's PATH does not
/// include where these tools live. Without that, the studio tells people
/// their AI is not installed when it is.
pub fn which_on_path(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    // On Windows a bare name resolves against a list of extensions; on Unix the
    // file itself must be executable.
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };

    let searched = std::env::split_paths(&path)
        .chain(extra_tool_dirs())
        .collect::<Vec<_>>();
    for dir in searched {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let direct = dir.join(program);
        if is_executable_file(&direct) {
            return Some(direct);
        }
        for extension in &extensions {
            let candidate = dir.join(format!("{program}{extension}"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Claude Code, driven headlessly through the `claude` CLI.
pub struct ClaudeProvider;

impl AgentProvider for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn description(&self) -> &'static str {
        "Claude Code (the `claude` CLI)"
    }

    fn program(&self) -> &'static str {
        "claude"
    }

    fn install_hint(&self) -> &'static str {
        "Install Claude Code from https://claude.com/claude-code, then run `claude` once to \
         sign in."
    }

    fn install_package(&self) -> Option<&'static str> {
        Some("@anthropic-ai/claude-code")
    }

    fn author_args(&self, prompt: &str) -> Vec<String> {
        [
            "-p",
            prompt,
            // Bash is the flag that makes authoring a loop rather than a single
            // shot: the agent can run `krate check-app .`, read the failure, and
            // fix it. Read/Edit/Write let it write the code; Bash lets it verify.
            "--allowed-tools",
            "Read,Edit,Write,Bash",
            // Streamed JSON events, one object per line, so the progress
            // reporter can say what the agent is doing right now instead of
            // printing dots. The transcript keeps every line either way.
            "--output-format",
            "stream-json",
            "--verbose",
            // bypassPermissions, not acceptEdits. This session is headless, so
            // any permission prompt blocks forever with nobody to clear it.
            // acceptEdits auto-accepts file edits but still prompts on Bash --
            // which meant the agent could not run `krate check-app` at all and
            // fell back to writing code blind, the exact failure the loop exists
            // to prevent. The agent works inside a throwaway app dir on the
            // user's own machine, doing precisely what `krate create` was asked
            // to do, so bypassing prompts here is scoped and safe.
            "--permission-mode",
            "bypassPermissions",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn plan_args(&self, prompt: &str) -> Vec<String> {
        // -p alone: pure text one-shot, no tools, no permission machinery.
        // The plan call reads nothing and writes nothing; speed is the
        // feature.
        vec!["-p".to_string(), prompt.to_string()]
    }

    fn configure(&self, command: &mut ProcessCommand) {
        // Close stdin. `claude -p` reads stdin for piped input; inheriting the
        // parent's made it block waiting for input that never comes -- the
        // transcript literally said "no stdin data received in 3s, proceeding
        // without it", and create hung.
        command.stdin(Stdio::null());
        // Run the child `claude` as a fresh, clean session. If `krate create` is
        // itself launched from inside a Claude Code session (which a developer
        // may well do), the inherited CLAUDE_CODE_* / session environment can
        // confuse the nested agent into stalling. Strip those so the child
        // starts as if launched from a plain terminal, with its own auth.
        for (key, _) in std::env::vars() {
            if key.starts_with("CLAUDE_CODE_") || key == "CLAUDECODE" {
                command.env_remove(key);
            }
        }
    }

    fn raw_tool_call(&self, line: &str) -> Option<(String, String)> {
        // claude's stream-json: message.content[] with a tool_use part. The
        // target is the file path for a read/write, or the command for Bash --
        // enough to reconstruct the full read/explore/write sequence, including
        // the Bash `cat`/`sed` exploration progress_line never surfaces.
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let content = event.get("message")?.get("content")?.as_array()?;
        for part in content {
            if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            let name = part
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let input = part.get("input");
            let target = input
                .and_then(|i| i.get("file_path").or_else(|| i.get("path")))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    input
                        .and_then(|i| i.get("command").or_else(|| i.get("pattern")))
                        .and_then(|v| v.as_str())
                        .map(|s| s.chars().take(200).collect())
                })
                .unwrap_or_default();
            return Some((name, target));
        }
        None
    }

    fn progress_line(&self, line: &str) -> Option<String> {
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let content = event.get("message")?.get("content")?.as_array()?;

        for part in content {
            if part.get("type")?.as_str()? != "tool_use" {
                continue;
            }
            let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let input = part.get("input");
            let arg =
                |key: &str| -> Option<String> { Some(input?.get(key)?.as_str()?.to_string()) };

            return Some(match name {
                "Write" | "Edit" => {
                    let path = arg("file_path").unwrap_or_default();
                    // Both separators: a Windows path is backslash-delimited,
                    // and '/' alone left the whole path as the file name.
                    let file = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
                    match file.as_str() {
                        "lib.rs" => "writing the app's code".to_string(),
                        "Cargo.toml" => "setting up the build".to_string(),
                        "manifest.toml" => "declaring what the app needs access to".to_string(),
                        "" => "writing a file".to_string(),
                        other => format!("writing {other}"),
                    }
                }
                // Name what is being read.
                //
                // These used to collapse to one fixed sentence, on the theory
                // that "it is studying the reference" is one fact rather than
                // six. But the reporter drops a step identical to the last
                // one, so twenty consecutive reads produced a single line and
                // the display sat frozen on it -- ten minutes on one machine,
                // with the person believing it had hung when it was working.
                // A changing line is the difference between waiting and
                // giving up.
                "Read" | "Glob" | "Grep" => describe_read(name, input),
                "Bash" => {
                    let cmd = arg("command").unwrap_or_default();
                    if cmd.contains("check-app") && cmd.contains("--shoot") {
                        // This is the step that opens the app for real: a
                        // window flashes and its sound plays. Unexplained
                        // that reads as the machine misbehaving, so name it
                        // before it happens (K-132).
                        "opening your app to see it -- a window may flash".to_string()
                    } else if cmd.contains("check-app") {
                        "checking it builds, runs, and only uses what it declared".to_string()
                    } else if cmd.contains("krate run") {
                        "running your app to test it -- a window may flash".to_string()
                    } else if cmd.contains("cargo build") || cmd.contains("cargo component") {
                        "building the app".to_string()
                    } else if cmd.contains("cargo") {
                        "running the Rust toolchain".to_string()
                    } else {
                        continue;
                    }
                }
                _ => continue,
            });
        }
        None
    }
}

/// What a read-shaped tool call is looking at, in plain words.
///
/// The point is that consecutive reads read *differently*. A person watching a
/// five-minute run needs to see it moving; a fixed sentence is the same as a
/// frozen one, and the reporter drops exact repeats so a fixed sentence
/// literally stops the display.
fn describe_read(tool: &str, input: Option<&serde_json::Value>) -> String {
    let field = |key: &str| -> Option<String> { Some(input?.get(key)?.as_str()?.to_string()) };

    // Reading the rendered frame is the AI looking at the picture the app
    // just drew -- the human half of "does this actually work". "reading
    // frame.png" told nobody that.
    if let Some(path) = field("file_path") {
        if path.ends_with(".png") {
            return "looking at how your app turned out".to_string();
        }
    }

    if let Some(pattern) = field("pattern") {
        let pattern = pattern.trim();
        if !pattern.is_empty() {
            let short = truncate_middle(pattern, 40);
            return if tool == "Grep" {
                format!("searching the reference for {short}")
            } else {
                format!("looking for {short}")
            };
        }
    }

    let Some(path) = field("file_path").or_else(|| field("path")) else {
        return "reading Krate's API reference".to_string();
    };
    let file = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
    match file.as_str() {
        "KRATE_AUTHORING.md" => "reading Krate's API reference".to_string(),
        "manifest.toml" => "reading the app's capabilities".to_string(),
        "Cargo.toml" => "reading the build setup".to_string(),
        "lib.rs" => {
            // Which app's lib.rs matters: reading an example is a different
            // activity from re-reading the app being written.
            match example_app_name(&path) {
                Some(app) => format!("reading the {app} example"),
                None => "re-reading the app's code".to_string(),
            }
        }
        "" => "reading Krate's API reference".to_string(),
        other => format!("reading {}", truncate_middle(other, 40)),
    }
}

/// The example app a path points into, e.g. `apps/krate-paint/src/lib.rs`.
fn example_app_name(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let rest = normalized.split("apps/").nth(1)?;
    let app = rest.split('/').next()?;
    if app.is_empty() {
        return None;
    }
    Some(app.trim_start_matches("krate-").to_string())
}

/// Shorten with an ellipsis in the middle, keeping both ends readable.
fn truncate_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(1) / 2;
    let head: String = text.chars().take(keep).collect();
    let tail: String = text.chars().skip(count - keep).collect();
    format!("{head}…{tail}")
}

/// A shared plain-English translation of a tool-use event.
///
/// Every provider streams a different envelope, but underneath they all report
/// the same handful of actions: wrote a file, read a file, ran a command. The
/// mapping from those actions to what a waiting person wants to read is the
/// same regardless of which AI did it, so it lives here once.
/// The file a read-shaped shell command is pointing at, if it is one.
///
/// Only the plainly-reading commands, and only the last path-looking word --
/// enough to say "reading lib.rs" instead of nothing, without trying to parse
/// a shell.
fn read_target(command: &str) -> Option<String> {
    let reads = [
        "cat ", "sed ", "head ", "tail ", "less ", "grep ", "rg ", "nl ",
    ];
    if !reads.iter().any(|verb| command.contains(verb)) {
        return None;
    }
    command
        .split_whitespace()
        .filter(|word| word.contains('.') && !word.starts_with('-'))
        .next_back()
        .map(|word| {
            let word = word.trim_matches(|c| c == '"' || c == '\'' || c == '\\');
            word.rsplit(['/', '\\']).next().unwrap_or(word).to_string()
        })
        .filter(|file| !file.is_empty())
}

fn describe_tool_use(tool: &str, path: Option<&str>, command: Option<&str>) -> Option<String> {
    let tool = tool.to_ascii_lowercase();
    if tool.contains("write") || tool.contains("edit") || tool.contains("apply") {
        let path = path.unwrap_or("");
        // Split on both separators: grok on Windows reports backslash paths
        // (C:\app\src\lib.rs), and splitting on '/' alone left the whole path
        // as the "file", so the step read "writing C:\app\src\lib.rs" instead
        // of "writing the app's code".
        let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
        return Some(match file {
            "lib.rs" => "writing the app's code".to_string(),
            "Cargo.toml" => "setting up the build".to_string(),
            "manifest.toml" => "declaring what the app needs access to".to_string(),
            "" => "writing a file".to_string(),
            other => format!("writing {other}"),
        });
    }
    if tool.contains("read")
        || tool.contains("glob")
        || tool.contains("grep")
        || tool.contains("search")
        || tool.contains("list")
    {
        // Name the file for the same reason the Claude path does: identical
        // consecutive lines are dropped, so a fixed sentence freezes the
        // display for as long as the agent keeps reading.
        let Some(path) = path.filter(|path| !path.is_empty()) else {
            return Some("reading Krate's API reference".to_string());
        };
        let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
        return Some(match file {
            "KRATE_AUTHORING.md" | "" => "reading Krate's API reference".to_string(),
            "manifest.toml" => "reading the app's capabilities".to_string(),
            "Cargo.toml" => "reading the build setup".to_string(),
            "lib.rs" => match example_app_name(path) {
                Some(app) => format!("reading the {app} example"),
                None => "re-reading the app's code".to_string(),
            },
            other => format!("reading {}", truncate_middle(other, 40)),
        });
    }
    if tool.contains("bash")
        || tool.contains("shell")
        || tool.contains("exec")
        || tool.contains("terminal")
    {
        let command = command.unwrap_or("");
        if command.contains("check-app") {
            return Some("checking it builds, runs, and only uses what it declared".to_string());
        }
        if command.contains("cargo build") || command.contains("cargo component") {
            return Some("building the app".to_string());
        }
        if command.contains("cargo") {
            return Some("running the Rust toolchain".to_string());
        }
        // An agent that reads through a SHELL rather than a read tool -- codex
        // does its whole pack-read with `cat` and `sed` -- was silent here,
        // and silence is what the person watches. A build spent twelve minutes
        // on "Reading Krate's API" with nothing under it because every one of
        // those commands landed on the `return None` below (K-155).
        if command.contains("KRATE_AUTHORING") {
            return Some("reading Krate's API reference".to_string());
        }
        if let Some(file) = read_target(command) {
            return Some(match file.as_str() {
                "lib.rs" => "reading the app's code".to_string(),
                "Cargo.toml" => "reading the build setup".to_string(),
                "manifest.toml" => "reading what the app declares".to_string(),
                other => format!("reading {}", truncate_middle(other, 40)),
            });
        }
        if command.starts_with("ls") || command.contains(" ls ") {
            return Some("looking at what is there".to_string());
        }
        return None;
    }
    None
}

/// OpenAI Codex CLI. Flags verified against `codex exec --help` on 2026-08-04.
struct CodexProvider;

impl AgentProvider for CodexProvider {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn description(&self) -> &'static str {
        "OpenAI Codex CLI"
    }

    fn program(&self) -> &'static str {
        "codex"
    }

    fn install_hint(&self) -> &'static str {
        "npm install -g @openai/codex@latest, then run `codex` once to sign in"
    }

    fn install_package(&self) -> Option<&'static str> {
        Some("@openai/codex")
    }

    fn author_args(&self, prompt: &str) -> Vec<String> {
        [
            "exec",
            // Streamed JSON events, so progress can be reported as real steps.
            "--json",
            // Krate authors into a throwaway directory that is not a git repo.
            // Without this, codex refuses to start there.
            "--skip-git-repo-check",
            // The agent must write files and run check-app inside the app dir.
            // workspace-write is the narrowest mode that allows both; it is not
            // full access.
            "--sandbox",
            "workspace-write",
            prompt,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn configure(&self, command: &mut ProcessCommand) {
        command.stdin(Stdio::null());
    }

    /// Probe with a real `exec` that RUNS A COMMAND, not `login status` and not
    /// a text-only reply.
    ///
    /// `login status` only answers "is the sign-in valid" and returns instantly
    /// -- and on a Windows machine whose codex sandbox helper
    /// (`codex-windows-sandbox-setup.exe`) is broken it says "Logged in using
    /// ChatGPT" while every actual build fails the moment codex runs a tool. So
    /// codex passed the probe, was offered, and the user picked it and hit a
    /// confusing "that build didn't come together" -- twice, on the founder's
    /// PC.
    ///
    /// But a text-only `exec` ("reply with ok") does NOT catch it either:
    /// verified on the broken machine, codex replies "ok" cleanly because a pure
    /// text answer never touches the sandbox. The sandbox only engages when
    /// codex runs a shell command or writes a file -- which is exactly what
    /// authoring does and what fails there. So the probe asks codex to run one
    /// command. On a healthy machine it runs `echo` and exits 0; on the broken
    /// one the sandbox helper fails and the exec errors, which marks codex
    /// not-ready with the reason instead of offering it as working.
    ///
    /// `--sandbox workspace-write` matches author_args so the probe exercises
    /// the same sandbox mode the build uses.
    fn probe_args(&self) -> Vec<String> {
        self.author_args(
            "Run this shell command and nothing else: echo krate-probe-ok. \
             Do not write any files.",
        )
    }

    fn output_failure(&self, stdout: &str, stderr: &str) -> Option<(String, Option<String>)> {
        // Codex exits 0 even when its Windows sandbox helper fails to launch --
        // it logs the error and moves on -- so the sandbox break is only
        // visible in the output. This is the exact signature from the founder's
        // machine, where the helper was present but codex reported it "not
        // found". When it appears, no build can run a command or write a file.
        let blob = format!("{stdout}\n{stderr}");
        if blob.contains("orchestrator_helper_launch_failed")
            || blob.contains("codex-windows-sandbox-setup.exe")
            || (blob.contains("windows sandbox") && blob.contains("helper"))
        {
            return Some((
                "is installed but its Windows sandbox helper will not launch, so it \
                 cannot run builds"
                    .to_string(),
                Some("reinstall the Codex CLI to restore its sandbox helper".to_string()),
            ));
        }
        None
    }

    fn login_hint(&self) -> String {
        "codex login".to_string()
    }

    fn progress_line(&self, line: &str) -> Option<String> {
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        // Only the START of a step. Codex emits `item.started` and then
        // `item.completed` for the same work, and reporting both made every
        // line appear twice.
        if event.get("type").and_then(|v| v.as_str()) == Some("item.completed") {
            return None;
        }
        // Codex's real shape, measured against a build that printed nothing
        // for twelve minutes: the kind of work is `/item/type` and the command
        // is `/item/command`. The parser looked for `name`, `tool` and
        // `/item/name`, none of which codex sends -- so 68 command executions
        // and 6 file changes were all invisible, the stage list sat on
        // "Reading Krate's API" for the whole build, and the trace recorded
        // one unbroken 730-second silence (K-155).
        //
        // The older shapes are kept as fallbacks: they cost nothing, and a
        // codex version that goes back to them should not go silent again.
        let name = event
            .pointer("/item/type")
            .or_else(|| event.get("name"))
            .or_else(|| event.get("tool"))
            .or_else(|| event.pointer("/item/name"))
            .and_then(|v| v.as_str())?;
        let path = event
            .pointer("/item/path")
            .or_else(|| event.pointer("/arguments/path"))
            .or_else(|| event.pointer("/input/path"))
            .or_else(|| event.pointer("/arguments/file_path"))
            .and_then(|v| v.as_str());
        let command = event
            .pointer("/item/command")
            .or_else(|| event.pointer("/arguments/command"))
            .or_else(|| event.pointer("/input/command"))
            .and_then(|v| v.as_str());
        // `command_execution` and `file_change` are codex's own words for
        // "ran something" and "wrote something"; describe_tool_use matches on
        // substrings like "write" and "read", so translate first.
        let name = match name {
            "command_execution" => "bash",
            "file_change" => "write",
            other => other,
        };
        describe_tool_use(name, path, command)
    }
}

/// Google Gemini CLI. Flags from Google's published CLI reference; the binary
/// was not installed on the machine where this was written, so the event shape
/// is parsed defensively rather than assumed.
struct GeminiProvider;

impl AgentProvider for GeminiProvider {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn description(&self) -> &'static str {
        "Google Gemini CLI"
    }

    fn program(&self) -> &'static str {
        "gemini"
    }

    fn install_hint(&self) -> &'static str {
        "npm install -g @google/gemini-cli@latest, then run `gemini` once to sign in"
    }

    fn install_package(&self) -> Option<&'static str> {
        Some("@google/gemini-cli")
    }

    fn author_args(&self, prompt: &str) -> Vec<String> {
        [
            "--prompt",
            prompt,
            // Headless: approve the agent's own tool calls, since there is
            // nobody at the terminal to answer a prompt.
            "--approval-mode",
            "yolo",
            "--output-format",
            "stream-json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn configure(&self, command: &mut ProcessCommand) {
        command.stdin(Stdio::null());
    }

    fn progress_line(&self, line: &str) -> Option<String> {
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        let name = event
            .pointer("/toolCall/name")
            .or_else(|| event.get("name"))
            .and_then(|v| v.as_str())?;
        let args = event
            .pointer("/toolCall/args")
            .or_else(|| event.get("args"));
        let path = args
            .and_then(|a| a.get("file_path").or_else(|| a.get("path")))
            .and_then(|v| v.as_str());
        let command = args.and_then(|a| a.get("command")).and_then(|v| v.as_str());
        describe_tool_use(name, path, command)
    }
}

/// GitHub Copilot CLI. Flags verified against `copilot --help` on 2026-08-04.
///
/// Copilot does not stream structured events in non-interactive mode, so there
/// is no progress to report beyond the heartbeat the caller prints. The app
/// still gets built and checked; only the step-by-step narration is missing.
struct CopilotProvider;

impl AgentProvider for CopilotProvider {
    fn name(&self) -> &'static str {
        "copilot"
    }

    fn description(&self) -> &'static str {
        "GitHub Copilot CLI"
    }

    fn program(&self) -> &'static str {
        "copilot"
    }

    fn install_hint(&self) -> &'static str {
        "npm install -g @github/copilot@latest, then run `copilot` once to sign in"
    }

    fn install_package(&self) -> Option<&'static str> {
        Some("@github/copilot")
    }

    fn author_args(&self, prompt: &str) -> Vec<String> {
        [
            "-p",
            prompt,
            // Headless: nobody is there to approve each tool call.
            "--allow-all-tools",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn configure(&self, command: &mut ProcessCommand) {
        command.stdin(Stdio::null());
    }

    fn progress_line(&self, _line: &str) -> Option<String> {
        // Plain prose output, not structured events. Reporting a guessed step
        // from a substring match would be worse than reporting nothing.
        None
    }
}

/// xAI Grok CLI, which installs its binary as `agent`. Flags verified against
/// `agent --help` on 2026-08-04 (grok 0.2.14).
struct GrokProvider;

impl AgentProvider for GrokProvider {
    fn reports_progress(&self) -> bool {
        // Authoring streams now (see author_args): grok emits one NDJSON event
        // per line -- thought and text deltas, then tool_call events -- so a
        // progress display CAN follow along, the transcript survives a kill,
        // and the silence-based stall detector has real lines to time from. The
        // old `--output-format json` wrote a single blob at the very end, which
        // meant a live run looked identical to a hung one and a killed run left
        // an empty transcript with no way to see what happened.
        true
    }

    fn name(&self) -> &'static str {
        "grok"
    }

    fn description(&self) -> &'static str {
        "xAI Grok CLI"
    }

    fn program(&self) -> &'static str {
        // Grok ships as `agent`, not `grok`. Using the wrong name here would
        // report "not installed" on a machine that has it.
        "agent"
    }

    fn install_hint(&self) -> &'static str {
        "install the Grok CLI from xAI (docs.x.ai), then run `agent login` once to sign in"
    }

    fn author_args(&self, prompt: &str) -> Vec<String> {
        // --always-approve is not optional here. Writing an app means writing
        // files, and a headless run has no terminal to approve that in: Grok
        // falls back to explaining the app instead, which Krate then rejects
        // as "the agent finished without changing the app".
        //
        // It went unnoticed because a machine that had used Grok interactively
        // carries permission_mode = "always-approve" in ~/.grok/config.toml,
        // so authoring worked there and failed on every fresh install.
        [
            "--single",
            prompt,
            "--always-approve",
            // Stream the run as NDJSON (one event per line) rather than a single
            // blob at the end. This is what lets progress be reported, keeps the
            // transcript alive if the run is killed, and gives the stall
            // detector real lines to measure silence between. The final app is
            // judged by files-changed + check-app, not by parsing this stream,
            // so the format is free to be the streaming one.
            "--output-format",
            "streaming-json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// The plan and probe steps keep the single-blob `json` format.
    ///
    /// Both read grok's answer out of stdout -- the plan wants one JSON object,
    /// the probe wants the word "ok" -- so the streaming NDNJSON authoring
    /// format would only add noise to parse around. Only authoring needs the
    /// live stream.
    fn plan_args(&self, prompt: &str) -> Vec<String> {
        vec![
            "--single".to_string(),
            prompt.to_string(),
            "--always-approve".to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ]
    }

    fn probe_args(&self) -> Vec<String> {
        self.plan_args("Reply with the single word: ok")
    }

    fn configure(&self, command: &mut ProcessCommand) {
        command.stdin(Stdio::null());
    }

    fn progress_line(&self, line: &str) -> Option<String> {
        // The streaming-json events, verified live against grok 0.2.14:
        //   {"type":"tool_call","toolName":"write",
        //    "rawInput":{"file_path":"...","content":"..."}}
        //   {"type":"tool_call","toolName":"run_terminal_command",
        //    "rawInput":{"command":"krate check-app ."}}
        // Only a tool_call is a step worth showing; thought/text deltas and
        // tool_call_update are proof-of-life for the heartbeat but not a line.
        let event: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if event.get("type").and_then(|t| t.as_str()) != Some("tool_call") {
            return None;
        }
        let name = event
            .get("toolName")
            .or_else(|| event.get("kind"))
            .and_then(|v| v.as_str())?;
        let path = event
            .pointer("/rawInput/file_path")
            .or_else(|| event.pointer("/rawInput/path"))
            .and_then(|v| v.as_str());
        let command = event.pointer("/rawInput/command").and_then(|v| v.as_str());
        describe_tool_use(name, path, command)
    }
}

#[cfg(test)]
mod tests {
    /// The bug that made a working eleven-minute run look hung.
    ///
    /// Every Read/Glob/Grep used to collapse to the one sentence "reading
    /// Krate's API reference". The reporter drops a step identical to the
    /// previous one, so a run that read twenty files reported one line and the
    /// display froze on it. Three people on three operating systems concluded
    /// the tool had hung while it was working normally; one waited eleven
    /// minutes and saw the screen go straight from that line to "done".
    #[test]
    fn consecutive_reads_do_not_all_say_the_same_thing() {
        let paths = [
            "/tmp/app/KRATE_AUTHORING.md",
            "/tmp/app/manifest.toml",
            "/tmp/app/Cargo.toml",
            "/repo/apps/krate-paint/src/lib.rs",
            "/repo/apps/krate-checklist/src/lib.rs",
        ];
        let lines: Vec<String> = paths
            .iter()
            .map(|path| {
                let input = serde_json::json!({ "file_path": path });
                describe_read("Read", Some(&input))
            })
            .collect();

        // Adjacent duplicates are exactly what the reporter discards.
        for pair in lines.windows(2) {
            assert_ne!(
                pair[0], pair[1],
                "consecutive reads must differ or the display freezes: {lines:?}"
            );
        }
        assert!(
            lines[3].contains("paint"),
            "names the example: {:?}",
            lines[3]
        );
        assert!(
            lines[4].contains("checklist"),
            "names the example: {:?}",
            lines[4]
        );
    }

    #[test]
    fn a_search_says_what_it_is_searching_for() {
        let input = serde_json::json!({ "pattern": "Event::Pointer" });
        let line = describe_read("Grep", Some(&input));
        assert!(line.contains("Event::Pointer"), "got {line:?}");
    }

    /// The twelve minutes of silence (K-155).
    ///
    /// These are real events from a build that showed nothing under "Reading
    /// Krate's API" for its whole 730 seconds. Codex puts the kind of work in
    /// `/item/type` and the command in `/item/command`; the parser looked for
    /// `name`, `tool` and `/item/name`, so every one of them fell through.
    #[test]
    fn codex_command_events_become_progress_lines() {
        let codex = super::CodexProvider;
        let read = r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'cat KRATE_AUTHORING.md'"}}"#;
        assert_eq!(
            codex.progress_line(read).as_deref(),
            Some("reading Krate's API reference"),
            "the pack read is the longest phase of a build and must not be silent"
        );

        let check = r#"{"type":"item.started","item":{"id":"item_9","type":"command_execution","command":"/bin/zsh -lc 'krate check-app .'"}}"#;
        assert_eq!(
            codex.progress_line(check).as_deref(),
            Some("checking it builds, runs, and only uses what it declared")
        );

        let write = r#"{"type":"item.started","item":{"id":"item_4","type":"file_change","path":"/tmp/app/src/lib.rs"}}"#;
        assert_eq!(
            codex.progress_line(write).as_deref(),
            Some("writing the app's code")
        );

        // Only the start of a step. Codex emits started AND completed for the
        // same work, and reporting both printed every line twice.
        let done = r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"/bin/zsh -lc 'cat KRATE_AUTHORING.md'"}}"#;
        assert_eq!(codex.progress_line(done), None);
    }

    /// An agent that reads through a shell rather than a read tool still has
    /// to show progress -- that is exactly what codex does.
    #[test]
    fn a_shell_read_names_the_file_it_is_reading() {
        assert_eq!(
            super::read_target("/bin/zsh -lc \"sed -n '1,400p' src/lib.rs\"").as_deref(),
            Some("lib.rs")
        );
        assert_eq!(
            super::read_target("/bin/zsh -lc 'cat manifest.toml'").as_deref(),
            Some("manifest.toml")
        );
        // Not a read: nothing to say, rather than a wrong guess.
        assert_eq!(super::read_target("cargo build --release"), None);
    }

    #[test]
    fn codex_sandbox_break_is_caught_from_output() {
        let codex = super::CodexProvider;
        // The exact stderr line from the founder's Windows PC. codex exits 0
        // and logs this; the probe must still mark it not-ready.
        let broken = "2026-08-19T08:51:39Z ERROR codex_core::exec: exec error: windows sandbox: \
             orchestrator_helper_launch_failed: setup refresh failed to launch helper: \
             helper=codex-windows-sandbox-setup.exe, error=program not found";
        let found = codex.output_failure("", broken);
        assert!(found.is_some(), "sandbox break must be detected");
        let (reason, remedy) = found.unwrap();
        assert!(reason.contains("sandbox helper"), "reason: {reason}");
        assert!(remedy.is_some(), "a remedy should be offered");

        // A clean codex run (the healthy machine) is not flagged.
        let ok = "{\"type\":\"item.completed\",\"item\":{\"text\":\"krate-probe-ok\"}}\n\
                  {\"type\":\"turn.completed\"}";
        assert_eq!(codex.output_failure(ok, ""), None);
    }

    #[test]
    fn grok_streaming_events_become_progress_lines() {
        // The real streaming-json events captured live from grok 0.2.14. A
        // tool_call turns into a step; a thought/text delta and a
        // tool_call_update do not (they are heartbeat, not a line).
        let grok = super::GrokProvider;
        let write = r#"{"type":"tool_call","toolName":"write","rawInput":{"file_path":"C:\\app\\src\\lib.rs","content":"..."}}"#;
        assert_eq!(
            grok.progress_line(write).as_deref(),
            Some("writing the app's code")
        );

        let check = r#"{"type":"tool_call","toolName":"run_terminal_command","rawInput":{"command":"krate check-app ."}}"#;
        assert_eq!(
            grok.progress_line(check).as_deref(),
            Some("checking it builds, runs, and only uses what it declared")
        );

        // Deltas and updates are not steps.
        assert_eq!(
            grok.progress_line(r#"{"type":"thought","data":"The"}"#),
            None
        );
        assert_eq!(grok.progress_line(r#"{"type":"text","data":"I'll"}"#), None);
        assert_eq!(
            grok.progress_line(r#"{"type":"tool_call_update","toolCallId":"x","status":null}"#),
            None
        );
        // And a non-JSON line never panics.
        assert_eq!(grok.progress_line("not json"), None);
    }

    #[test]
    fn a_read_with_nothing_to_name_still_says_something() {
        assert_eq!(describe_read("Read", None), "reading Krate's API reference");
    }

    #[test]
    fn the_app_being_written_is_not_confused_with_an_example() {
        let mine = serde_json::json!({ "file_path": "/tmp/work/tip-calc/src/lib.rs" });
        assert_eq!(
            describe_read("Read", Some(&mine)),
            "re-reading the app's code"
        );
    }

    use super::*;

    /// Resolve a name that must succeed, without requiring `Debug` on the
    /// trait object that `expect` would need.
    fn resolved(name: &str) -> &'static dyn AgentProvider {
        match resolve(name) {
            Ok(provider) => provider,
            Err(error) => panic!("{name} should resolve: {error}"),
        }
    }

    #[test]
    fn a_known_provider_name_resolves() {
        let provider = resolved("claude");
        assert_eq!(provider.name(), "claude");
        assert_eq!(provider.program(), "claude");
    }

    #[test]
    fn provider_names_are_matched_case_insensitively() {
        assert_eq!(resolved("Claude").name(), "claude");
        assert_eq!(resolved(" claude ").name(), "claude");
    }

    /// An unknown provider must list the ones that exist. A bare "invalid
    /// value" tells someone they were wrong without telling them what is right.
    #[test]
    fn an_unknown_provider_lists_the_available_ones() {
        let Err(error) = resolve("gpt-9") else {
            panic!("gpt-9 is not a provider and must not resolve");
        };
        assert!(
            error.contains("unknown AI provider \"gpt-9\""),
            "it must quote the name that was given: {error}"
        );
        assert!(
            error.contains("Available providers:"),
            "it must offer a listing: {error}"
        );
        for provider in PROVIDERS {
            assert!(
                error.contains(provider.name()),
                "the listing must name {}: {error}",
                provider.name()
            );
        }
        assert!(
            error.contains("--author-cmd"),
            "it must mention the escape hatch for unsupported tools: {error}"
        );
    }

    /// The missing-CLI message has to be actionable: what is missing, and the
    /// step that fixes it. This is the difference between a person installing
    /// Claude Code and a person filing a bug about a spawn failure.
    #[test]
    fn a_missing_cli_explains_how_to_install_it() {
        let message = missing_cli_error(&ClaudeProvider);
        assert!(
            message.contains("`claude` command is not installed"),
            "it must name the missing program: {message}"
        );
        assert!(
            message.contains("claude.com/claude-code"),
            "it must say where to get it: {message}"
        );
        assert!(
            !message.contains("No such file or directory"),
            "it must not read like a raw spawn error: {message}"
        );
    }

    #[test]
    fn installed_detection_finds_a_real_program_and_misses_a_fake_one() {
        struct Fake(&'static str);
        impl AgentProvider for Fake {
            fn name(&self) -> &'static str {
                "fake"
            }
            fn description(&self) -> &'static str {
                "not real"
            }
            fn program(&self) -> &'static str {
                self.0
            }
            fn install_hint(&self) -> &'static str {
                "there is nothing to install"
            }
            fn author_args(&self, _prompt: &str) -> Vec<String> {
                Vec::new()
            }
            fn progress_line(&self, _line: &str) -> Option<String> {
                None
            }
        }

        // A program every supported platform has, versus one nobody has.
        let real = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(is_installed(&Fake(real)), "{real} must be found on PATH");
        assert!(!is_installed(&Fake("krate-no-such-agent-cli-xyz")));
    }

    /// The Claude flags are load-bearing and were each fixed in response to a
    /// real hang or a real silent failure. This pins them so a refactor cannot
    /// quietly drop one.
    #[test]
    fn claude_is_invoked_headlessly_with_the_flags_that_keep_it_unblocked() {
        let args = ClaudeProvider.author_args("build me an app");
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], "build me an app");
        let joined = args.join(" ");
        assert!(joined.contains("--allowed-tools Read,Edit,Write,Bash"));
        assert!(joined.contains("--output-format stream-json"));
        assert!(joined.contains("--permission-mode bypassPermissions"));
    }

    #[test]
    fn claude_progress_lines_are_plain_english() {
        let write = r#"{"message":{"content":[{"type":"tool_use","name":"Write",
            "input":{"file_path":"/tmp/app/src/lib.rs"}}]}}"#;
        assert_eq!(
            ClaudeProvider.progress_line(write).as_deref(),
            Some("writing the app's code")
        );

        let check = r#"{"message":{"content":[{"type":"tool_use","name":"Bash",
            "input":{"command":"/usr/bin/krate check-app ."}}]}}"#;
        assert_eq!(
            ClaudeProvider.progress_line(check).as_deref(),
            Some("checking it builds, runs, and only uses what it declared")
        );

        // Anything we do not recognize prints nothing rather than guessing.
        assert_eq!(ClaudeProvider.progress_line("not json"), None);
        assert_eq!(ClaudeProvider.progress_line(r#"{"type":"other"}"#), None);
    }
}
