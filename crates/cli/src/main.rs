use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use krate_manifest::{
    supported_capability_specs, App, AppWorld, Capability, CapabilityRequest, Manifest,
    PHASE2_CLI_WORLD,
};
use krate_policy::{resolve_session_policy, SessionPolicy};
use krate_runtime::{
    Config, RunOutcome, Runtime, RuntimeError, RuntimeWorld, DEFAULT_HTTP_TIMEOUT_MILLIS,
    DEFAULT_MAX_HTTP_RESPONSE_BYTES,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

mod agent_provider;
mod authoring_context;
mod github_auth;
mod krate_mode;
mod lineedit;
mod mcp;
mod port_report;
mod progress;
mod sdk;
mod sdk_reference;
mod speech_model;
mod style;
mod tui;
mod usage;

const MAX_PHASE2_ARGS_RAW_BYTES: usize = 64 * 1024;
const MAX_PHASE2_ARG_COUNT: usize = 1024;
/// Fuel budget applied to an untrusted run (`run --untrusted`, and the run
/// Krate makes when it verifies an app it just authored). Large enough that a
/// real app finishing its work never trips it, small enough that a runaway or
/// infinite loop is stopped in well under a second instead of hanging. An
/// explicit `--fuel` always overrides this.
const UNTRUSTED_FUEL_BUDGET: u64 = 5_000_000_000;

/// How long `krate create --agent` waits for the AI to author the app before
/// giving up with a clear message.
///
/// The agent now iterates: it writes code, runs `check-app`, reads the failure,
/// and fixes it, across several build cycles -- each cargo-component build alone
/// is tens of seconds. So the budget is minutes, not the ~3 the old one-shot
/// needed. Override with KRATE_AUTHOR_TIMEOUT_SECS for a slow machine or a hard
/// request. Short enough that a genuinely stuck agent still fails with a clear
/// message rather than hanging forever.
const AGENT_AUTHOR_TIMEOUT_SECS: u64 = 900;

/// Version shown by `krate --version`. The release workflow sets
/// `KRATE_RELEASE_VERSION` to the git tag so a released binary reports its real
/// version; local and CI builds fall back to the crate version from Cargo.toml.
const KRATE_VERSION: &str = match option_env!("KRATE_RELEASE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(
    name = "krate",
    version = KRATE_VERSION,
    about = "Krate: write once, run on everything."
)]
struct Cli {
    /// No subcommand opens the interactive front door. `krate` is what a
    /// newcomer types, and it used to answer with sixteen commands and no
    /// suggestion which one to start with.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a WebAssembly component through the Krate runtime.
    Run {
        /// Path to a .wasm component, a .krate bundle, or an https URL to one.
        target: String,

        /// Max fuel units to allow. Omit for unlimited.
        #[arg(long)]
        fuel: Option<u64>,

        /// Treat this app as untrusted: cap it with a default fuel budget so a
        /// runaway or infinite loop stops instead of hanging. This is what
        /// Krate uses when it verifies an app it just authored. An explicit
        /// `--fuel` always wins over this default.
        #[arg(long)]
        untrusted: bool,

        /// Max memory in MiB.
        #[arg(long, default_value_t = 256)]
        mem_limit: u64,

        /// Max bytes accepted for one Phase 2 HTTP response.
        #[arg(long, default_value_t = DEFAULT_MAX_HTTP_RESPONSE_BYTES)]
        max_http_response_bytes: usize,

        /// Default timeout in milliseconds for helper Phase 2 HTTP GET calls (`0` disables).
        #[arg(long, default_value_t = DEFAULT_HTTP_TIMEOUT_MILLIS)]
        http_timeout_millis: u32,

        /// Root directory used for relative Phase 2 filesystem paths.
        #[arg(long, default_value = ".")]
        sandbox_root: PathBuf,

        /// Path to a Phase 2 manifest.toml. If omitted, Krate checks next to the .wasm file.
        #[arg(long)]
        manifest: Option<PathBuf>,

        /// Grant a capability for this run session. Repeat for multiple grants.
        #[arg(long, value_name = "CAP")]
        grant: Vec<String>,

        /// Grant every capability declared in the manifest for this run session.
        #[arg(long)]
        auto_grant: bool,

        /// Ask before granting missing capabilities declared by the manifest.
        #[arg(long)]
        prompt: bool,

        /// Ask for missing capabilities in a native consent window instead of
        /// the terminal. macOS only today; on other platforms this falls back
        /// to the terminal prompt. This is the path a double-clicked `.krate`
        /// uses, where there is no terminal to answer.
        #[arg(long)]
        consent: bool,

        /// Force the native window path, failing rather than falling back if
        /// this machine has no display.
        ///
        /// GUI apps open a window by default now, so this is only needed to
        /// turn a missing display into an error instead of a headless run.
        #[arg(long)]
        native_window: bool,

        /// Run a GUI app without opening a window.
        ///
        /// For scripts, tests and servers, where a window would be in the way
        /// or impossible. The app still runs and still prints what it prints.
        #[arg(long, conflicts_with = "native_window")]
        headless: bool,

        /// Allow fetching a bundle over plain http. Intended for a local test
        /// server; https is required otherwise.
        #[arg(long)]
        insecure_http: bool,

        /// Print one machine-readable JSON object describing the run instead
        /// of streaming the app's stdout. Schema: krate.run.v1.
        #[arg(long)]
        json: bool,

        /// Print the effective session capabilities and exit before running the component.
        #[arg(long)]
        dump_caps: bool,

        /// Output format used with --dump-caps.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        dump_caps_format: OutputFormat,

        /// Append the effective session grants to a local audit log file.
        #[arg(long, value_name = "FILE")]
        log_grants: Option<PathBuf>,

        /// Output format used with --log-grants.
        #[arg(long, value_enum, default_value_t = GrantLogFormat::Text)]
        log_grants_format: GrantLogFormat,

        /// Fixed wall-clock time in milliseconds since Unix epoch. Intended for deterministic tests.
        #[arg(long, hide = true)]
        test_time: Option<u64>,

        /// Fixed locale tag for deterministic tests.
        #[arg(long, hide = true)]
        test_locale: Option<String>,

        /// Fixed timezone for deterministic tests.
        #[arg(long, hide = true)]
        test_timezone: Option<String>,

        /// Run headless and paint the app's window to this PNG once it has
        /// drawn a frame. This is how you see what an app actually renders --
        /// on any machine, in CI, without a display -- instead of trusting that
        /// it exited cleanly. Implies --headless.
        #[arg(long, value_name = "FILE")]
        shoot: Option<PathBuf>,

        /// Display scale for --shoot. 2 mimics a HiDPI window; 1 is raw logical
        /// pixels.
        #[arg(long, default_value_t = 2.0)]
        shoot_scale: f32,

        /// Drive the app the way a person would -- resize it, press it, and
        /// watch that it stays open -- and write what was observed to this
        /// path as JSON. Implies --headless. Used by `check-app`'s usability
        /// stage; not something you need by hand.
        #[arg(long, hide = true, value_name = "FILE")]
        usability_report: Option<PathBuf>,

        /// Arguments passed to the Krate app. Put them after `--`.
        #[arg(last = true, value_name = "ARG")]
        app_args: Vec<String>,
    },
    /// Print version information.
    Version,
    /// Check the local development environment.
    Doctor,
    /// Show which AI coding tools are installed, so you know what you can
    /// author apps with. Reads nothing but your PATH.
    Ai,

    /// Set up Claude Desktop or Cursor to build Krate apps for you.
    ///
    /// Edits the app's config file so you never have to. Shows you the change
    /// and asks before writing anything.
    Connect {
        /// Which app to set up: `claude-desktop` or `cursor`. Omit to be asked.
        app: Option<String>,

        /// Write the change without asking first.
        #[arg(long)]
        yes: bool,

        /// Print what would change and stop.
        #[arg(long)]
        dry_run: bool,
    },

    /// Build, import-check, and run an app directory, printing one clear
    /// verdict. This is the oracle an AI author (or a human, or CI) runs after
    /// every change: it compiles the crate with the right toolchain, confirms
    /// the component imports only Krate APIs, and runs it once headless. On
    /// failure it names the stage and the fix -- including mapping a leaked
    /// `wasi:*` import back to the no_std discipline that removes it. Prints
    /// `OK` and exits 0 only when every stage passes.
    CheckApp {
        /// The app directory: the folder holding Cargo.toml, src/lib.rs, and
        /// manifest.toml. Defaults to the current directory.
        #[arg(default_value = ".")]
        dir: PathBuf,

        /// Paint the app's first frame to this PNG after it runs, so a GUI
        /// app's output can be seen. Ignored for CLI apps that open no window.
        #[arg(long, value_name = "FILE")]
        shoot: Option<PathBuf>,

        /// Stop after the import check: build and confirm krate:*-only imports,
        /// but do not run the app. Useful when the app needs input or resources
        /// a headless run cannot provide.
        #[arg(long)]
        no_run: bool,

        /// Print one machine-readable JSON object instead of human lines. The
        /// object names the stage that failed and carries the actionable fix,
        /// so an agent can branch on it. Errors are reported as JSON too.
        #[arg(long)]
        json: bool,
    },

    /// Write the authoring context pack (KRATE_AUTHORING.md) for an app dir.
    ///
    /// This is the file an AI author reads before writing code: the SDK API
    /// surface, the capability catalog, the no_std discipline, the GUI-world
    /// interfaces, and an index of the shipped example apps -- all generated
    /// from the same sources the runtime builds against, so it cannot drift. A
    /// human can read it too, to see exactly what an app may call and declare.
    AuthoringContext {
        /// The app directory the pack is for. Its sibling apps/ tree, when
        /// present, seeds the example index. Defaults to the current directory.
        #[arg(default_value = ".")]
        dir: PathBuf,

        /// Write to this file instead of stdout. `KRATE_AUTHORING.md` in the app
        /// dir is the conventional location the authoring loop uses.
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    /// Print Krate Mode: the paste-in prompt that teaches any chat model to
    /// write correct Krate apps.
    ///
    /// Where `authoring-context` targets an agent that has the repo and can run
    /// `check-app` in a loop, this targets someone in ChatGPT, Claude, or Cursor
    /// with nothing installed. It carries the same generated API surface, but
    /// adds complete file templates, two shipped apps inlined as worked
    /// examples, and an honest handoff -- because a chat model cannot compile.
    ///
    /// `docs/krate-mode.md` is the published copy; regenerate it with
    /// `krate krate-mode --output docs/krate-mode.md` whenever the API changes.
    KrateMode {
        /// Write to this file instead of stdout.
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },

    /// Inspect and validate Phase 2 app manifests.
    Manifest {
        #[command(subcommand)]
        command: ManifestCommand,
    },

    /// Entry point used inside Krate.app: wait for the document Finder asked
    /// us to open, then run it behind the consent wall. Not intended for
    /// direct use; double-click a .krate instead.
    #[cfg(target_os = "macos")]
    #[command(hide = true)]
    OpenApp,

    /// Pack a component and its manifest into one shareable .krate bundle.
    Pack {
        /// Path to the .wasm component.
        file: PathBuf,

        /// Path to the manifest.toml describing it.
        #[arg(long)]
        manifest: PathBuf,

        /// Where to write the bundle.
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Upload a .krate to a hub and print a URL anyone can `krate run`.
    ///
    /// The bundle is stored by the hash of its bytes, so republishing the same
    /// app returns the same URL. The hub to use comes from KRATE_HUB_URL,
    /// defaulting to a local dev server at http://127.0.0.1:8787.
    Publish {
        /// Path to the .krate bundle to upload.
        bundle: PathBuf,

        /// Hub to upload to. Overrides the KRATE_HUB_URL environment variable.
        #[arg(long)]
        hub: Option<String>,

        /// One line describing the app, shown on its cloud page. Defaults to
        /// what you asked for when the app was made.
        #[arg(long)]
        description: Option<String>,
    },

    /// Author a small app from a request and package it as one shareable
    /// .krate: generate the source, build it, check it imports only Krate
    /// APIs, pack it, and verify its permission wall before writing the file.
    Create {
        /// What to build, in plain words, e.g.
        /// "Make a checklist app that saves locally".
        request: String,

        /// Where to write the finished .krate.
        #[arg(short, long)]
        output: PathBuf,

        /// Author the app with an AI coding agent instead of the built-in
        /// generator. `--agent claude` drives Claude Code: Krate hands it the
        /// request and a working starter, and it writes the app. This is the
        /// clean, supported way to plug in an agent; `--author-cmd` below is the
        /// lower-level escape hatch for any other tool. An unknown name lists
        /// the providers that are supported.
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,

        /// A command that writes the app source instead of the built-in
        /// generator — the lower-level agent seam. It is handed
        /// `KRATE_APP_DIR`, `KRATE_APP_NAME`, and `KRATE_REQUEST` and must
        /// write Cargo.toml, src/lib.rs, and manifest.toml into the app dir.
        /// Prefer `--agent` for a supported agent.
        #[arg(long)]
        author_cmd: Option<String>,

        /// Which built-in template to use when no --author-cmd is given.
        /// Inferred from the request when omitted.
        #[arg(long, value_enum)]
        kind: Option<CreateKind>,

        /// Kebab-case name for the generated app. Defaults per kind.
        #[arg(long)]
        name: Option<String>,

        /// Write the authoring transcript (JSON) to this path: the request, the
        /// permissions the app asks for, and the verification that it runs with
        /// its grants and refuses without the gating one. Off unless asked for;
        /// `--json` prints the same record to stdout instead.
        #[arg(long)]
        transcript: Option<PathBuf>,

        /// Keep the generated crate directory instead of using a temp dir,
        /// for inspecting or hand-editing what was authored.
        #[arg(long)]
        work_dir: Option<PathBuf>,

        /// Answer yes to the toolchain-install prompt: if the Rust build tools
        /// `create` needs are missing, install them without asking.
        #[arg(long)]
        yes: bool,

        /// Never install anything: if a build tool is missing, print how to
        /// install it and stop, instead of offering to install it.
        #[arg(long)]
        no_install: bool,

        /// Print one machine-readable JSON object (schema `krate.author.v1`)
        /// on stdout instead of the human progress lines. For agents and
        /// scripts. Errors are reported as JSON too.
        #[arg(long)]
        json: bool,

        /// Author the app even when the request asks for something Krate
        /// cannot do. The screen that stops "download my email" is a judgement
        /// about your words, and it can be wrong; this overrides it. What you
        /// get back will not do the impossible part.
        #[arg(long)]
        force: bool,
    },

    /// Analyze an existing source project and explain how it can become a
    /// portable Krate app. This command is read-only: it does not build,
    /// execute, or edit the source.
    /// Send us a port failure report, after showing you exactly what it says.
    ///
    /// Nothing is sent until you have seen the whole file and agreed. A report
    /// can contain your source and your paths, so the default is that it stays
    /// on your computer.
    Report {
        /// The FAILURE-REPORT.md a failed port wrote.
        report: PathBuf,

        /// Print the report and stop, without offering to send it.
        #[arg(long)]
        show: bool,
    },

    Port {
        /// Source project directory to analyze.
        source: PathBuf,

        /// Produce a porting plan. This is optional while planning is the only
        /// port operation, so both `krate port app` and
        /// `krate port app --plan` are accepted.
        #[arg(long)]
        plan: bool,

        /// Output format for the plan.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,

        /// Also write the JSON plan to this path.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Create an isolated, reviewable porting workspace containing the
        /// plan, an AI task, and a compiling Krate candidate. The source
        /// project is never copied, run, or changed.
        #[arg(long, value_name = "DIR")]
        prepare: Option<PathBuf>,

        /// Ask a supported AI coding agent to transform the prepared candidate.
        /// Requires --to. The original source is re-analyzed after the agent
        /// runs and the command stops if its scanned contents changed.
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,

        /// Lower-level agent seam. The command runs inside the prepared
        /// workspace with KRATE_PORT_SOURCE, KRATE_PORT_PLAN,
        /// KRATE_PORT_CANDIDATE, and KRATE_PORT_TASK set. Requires --to.
        #[arg(long, conflicts_with = "agent")]
        author_cmd: Option<String>,

        /// Build, inspect, package, and permission-test the transformed
        /// candidate into this .krate file.
        #[arg(long, value_name = "FILE")]
        to: Option<PathBuf>,

        /// Keep the completed port transcript at this path.
        #[arg(long, value_name = "FILE")]
        transcript: Option<PathBuf>,

        /// Let the selected agent repair a candidate that fails to build,
        /// imports unsupported host APIs, or has an invalid manifest. Each
        /// attempt receives the exact validation error. Capped at 5.
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(0..=5))]
        repair_attempts: u8,

        /// Answer yes to any required toolchain installation prompt.
        #[arg(long)]
        yes: bool,

        /// Do not offer to install missing build tools.
        #[arg(long)]
        no_install: bool,
    },

    /// Turn the anonymous usage count on or off, or see what it sends.
    Telemetry {
        /// `off` stops it, `on` resumes it, `status` shows what is sent.
        #[arg(value_parser = ["on", "off", "status"], default_value = "status")]
        state: String,
    },

    /// Run the Krate MCP server, so a model can build Krate apps and run them
    /// by talking rather than by anyone typing commands.
    ///
    /// Exposes the authoring loop as tools -- the API reference, complete
    /// example apps, an async build job, the six-stage check-app oracle,
    /// packaging, and rendering an app's first frame -- plus the sandboxed
    /// execution tools. Speaks JSON-RPC 2.0 over stdio. Add it once to Claude
    /// Desktop or Cursor; see docs/mcp-setup.md. Builds run here, on this
    /// machine, never on a server.
    Mcp,

    /// Internal: drive a supported AI agent to author the app. `krate create
    /// --agent claude` runs this; it reads KRATE_REQUEST / KRATE_APP_DIR from
    /// the environment create sets. Hidden because it is not a user entry point.
    #[command(hide = true)]
    AuthorAgent {
        #[arg(value_name = "NAME")]
        agent: String,
    },
}

/// The built-in app templates `krate create` can generate.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CreateKind {
    /// A CLI app: read a file and print its most frequent words.
    WordFrequency,
    /// A GUI app: a checklist with checkboxes that saves locally.
    Checklist,
    /// A GUI app: a microphone-driven teleprompter.
    VoicePrompter,
}

/// Single-quote a path for use inside a shell command string, so an install
/// path containing spaces (or an apostrophe) cannot split into two arguments.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The `--author-cmd` string that drives a provider. Krate builds the headless
/// prompt and passes the request through the environment, so the user never has
/// to write agent glue -- `--agent claude` just works.
fn agent_author_command(provider: &dyn agent_provider::AgentProvider) -> String {
    // Invoke THIS binary by its own path, never the bare name `krate`.
    // A bare name resolves through PATH, so a `krate` installed earlier
    // would drive the agent instead of the one the person just ran -- the
    // authoring prompt, the progress reporting, and the check-app oracle
    // would all silently come from a different version than the command
    // that is running. Same class of trap as double-clicking a stale
    // installed app: everything appears to work while the wrong code runs.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "krate".to_string());
    // `author-agent <name>` is a hidden subcommand that runs the agent with the
    // right prompt and flags, reading KRATE_REQUEST etc. from the environment
    // create already sets. Invoking our own binary keeps the prompt versioned
    // with the tool instead of in a script.
    format!("{} author-agent {}", shell_quote(&exe), provider.name())
}

/// Resolve `--agent <name>` to a provider, and check its CLI is actually here.
///
/// Both failures are answered with the fix rather than the symptom: an unknown
/// name gets the list of names that work, and a provider whose CLI is missing
/// gets the install step. Neither should ever surface as a spawn error.
fn resolve_agent(name: &str) -> Result<&'static dyn agent_provider::AgentProvider> {
    let provider = agent_provider::resolve(name).map_err(|message| anyhow::anyhow!(message))?;
    if !agent_provider::is_installed(provider) {
        anyhow::bail!(agent_provider::missing_cli_error(provider));
    }
    Ok(provider)
}

#[derive(Debug, Subcommand)]
enum ManifestCommand {
    /// Validate a manifest.toml file.
    Check {
        /// Path to manifest.toml.
        file: PathBuf,

        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Explain app identity and capability grants in a manifest.toml file.
    Explain {
        /// Path to manifest.toml.
        file: PathBuf,

        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Create a starter Phase 2 manifest.toml.
    Init {
        /// Reverse-DNS app id, for example com.example.notes.
        #[arg(long)]
        id: String,

        /// Human-readable app name.
        #[arg(long)]
        name: String,

        /// App version.
        #[arg(long, default_value = "0.1.0-dev")]
        version: String,

        /// Component path written into app.entry.
        #[arg(long)]
        entry: PathBuf,

        /// Capability to request. Repeat for multiple capabilities.
        #[arg(long, value_name = "CAP")]
        cap: Vec<String>,

        /// Write to a file instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Overwrite --output if it already exists.
        #[arg(long)]
        force: bool,
    },
    /// Print the Phase 2 capability strings understood by this runtime.
    Capabilities {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum GrantLogFormat {
    Text,
    Jsonl,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("KRATE_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .without_time()
        .init();

    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {}", friendly_error(&err));
            ExitCode::from(1)
        }
    }
}

/// Turn an error chain into a single line a person can act on. When a known
/// bundle problem is in the chain (a corrupt or non-.krate file, a missing
/// file), print its plain sentence instead of the raw zip/io wording, and
/// print it once rather than the doubled `{:#}` chain. Everything else keeps
/// the full `{:#}` context, which is already useful for the CLI's own errors.
fn friendly_error(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(bundle) = cause.downcast_ref::<krate_bundle::BundleError>() {
            if let Some(message) = bundle.user_message() {
                return message;
            }
        }
        // A component that imports a host interface this runtime does not
        // provide fails to instantiate with wasmtime's "a matching
        // implementation was not found in the linker". To a person that is
        // noise; the real meaning is almost always "this app was built for a
        // newer Krate than you have". Say that instead, with the fix.
        let text = cause.to_string();
        if text.contains("matching implementation was not found")
            || (text.contains("imports instance") && text.contains("not found"))
        {
            return "this app needs a newer version of Krate than you have installed. \
                    Update Krate and try again:\n  \
                    curl -fsSL https://krate.tech/install.sh | sh\n\
                    (on Windows: irm https://krate.tech/install.ps1 | iex)"
                .to_string();
        }
    }
    format!("{err:#}")
}

fn run() -> Result<u8> {
    let cli = Cli::parse();

    let Some(command) = cli.command else {
        return tui::run();
    };

    match command {
        Command::Run {
            target,
            fuel,
            untrusted,
            mem_limit,
            max_http_response_bytes,
            http_timeout_millis,
            sandbox_root,
            manifest,
            grant,
            auto_grant,
            prompt,
            consent,
            native_window,
            headless,
            insecure_http,
            json,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time,
            test_locale,
            test_timezone,
            shoot,
            shoot_scale,
            usability_report,
            app_args,
        } => run_component(RunRequest {
            target,
            file: PathBuf::new(),
            insecure_http,
            // An explicit --fuel wins; otherwise --untrusted applies the default
            // budget, and a plain trusted run stays unlimited (None).
            fuel: fuel.or(if untrusted {
                Some(UNTRUSTED_FUEL_BUDGET)
            } else {
                None
            }),
            mem_limit,
            max_http_response_bytes,
            http_timeout_millis,
            sandbox_root,
            manifest_path: manifest,
            grants: grant,
            auto_grant,
            prompt,
            consent,
            // A screenshot is a headless render by definition: there is no way
            // to grab a native window's pixels here, and forcing headless makes
            // --shoot work the same on a laptop, a server, and CI. A driven
            // usability run is headless for the same reason -- it compares
            // frames it paints itself, and must run the same way in CI as on a
            // laptop with a display attached.
            ui_mode: if headless || shoot.is_some() || usability_report.is_some() {
                krate_runtime::phase3_ui::Phase3HostUiMode::HeadlessDraft
            } else if native_window {
                krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype
            } else {
                krate_runtime::phase3_ui::Phase3HostUiMode::NativeWithHeadlessFallback
            },
            json,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time_millis: test_time,
            test_locale,
            test_timezone,
            screenshot_path: shoot,
            screenshot_scale: shoot_scale,
            usability_report,
            app_args,
        }),
        #[cfg(target_os = "macos")]
        Command::OpenApp => open_app(),
        Command::Pack {
            file,
            manifest,
            output,
        } => pack_bundle(&file, &manifest, &output),
        Command::Telemetry { state } => usage::telemetry_command(&state),
        Command::Publish {
            bundle,
            hub,
            description,
        } => publish_bundle(&bundle, hub.as_deref(), description.as_deref()),
        Command::Create {
            request,
            output,
            agent,
            author_cmd,
            kind,
            name,
            transcript,
            work_dir,
            yes,
            no_install,
            json,
            force,
        } => create_krate(CreateRequest {
            request,
            output,
            // --agent is the clean front door; it resolves to the command that
            // drives that provider. An explicit --author-cmd still wins for any
            // other tool. Resolving here means an unknown name or a missing CLI
            // is reported before any authoring work begins.
            author_cmd: match (author_cmd, agent) {
                (Some(command), _) => Some(command),
                (None, Some(name)) => Some(agent_author_command(resolve_agent(&name)?)),
                (None, None) => None,
            },
            kind,
            name,
            transcript,
            work_dir,
            yes,
            no_install,
            json,
            force,
        }),
        Command::Report { report, show } => run_report_command(&report, show),
        Command::Port {
            source,
            plan: _,
            format,
            output,
            prepare,
            agent,
            author_cmd,
            to,
            transcript,
            repair_attempts,
            yes,
            no_install,
        } => port_project(PortRequest {
            source,
            plan_format: format,
            plan_output: output,
            prepare,
            agent,
            author_cmd,
            to,
            transcript,
            repair_attempts,
            yes,
            no_install,
        }),
        Command::Mcp => mcp::serve().map(|()| 0),
        Command::AuthorAgent { agent } => run_author_agent(&agent),
        Command::Version => {
            print_version();
            Ok(0)
        }
        Command::Doctor => doctor(),
        Command::Ai => ai_status(),
        Command::Connect { app, yes, dry_run } => connect(app.as_deref(), yes, dry_run),
        Command::CheckApp {
            dir,
            shoot,
            no_run,
            json,
        } => check_app(&dir, shoot.as_deref(), no_run, json),
        Command::AuthoringContext { dir, output } => {
            let pack = authoring_context::generate(&dir);
            match output {
                Some(path) => {
                    fs::write(&path, pack).with_context(|| format!("write {}", path.display()))?;
                    println!("wrote {}", path.display());
                }
                None => print!("{pack}"),
            }
            Ok(0)
        }
        Command::KrateMode { output } => {
            let prompt = krate_mode::generate();
            match output {
                Some(path) => {
                    fs::write(&path, prompt)
                        .with_context(|| format!("write {}", path.display()))?;
                    println!("wrote {}", path.display());
                }
                None => print!("{prompt}"),
            }
            Ok(0)
        }
        Command::Manifest { command } => match command {
            ManifestCommand::Check { file, format } => check_manifest(&file, format),
            ManifestCommand::Explain { file, format } => explain_manifest(&file, format),
            ManifestCommand::Init {
                id,
                name,
                version,
                entry,
                cap,
                output,
                force,
            } => init_manifest(ManifestInitRequest {
                id,
                name,
                version,
                entry,
                capabilities: cap,
                output,
                force,
            }),
            ManifestCommand::Capabilities { format } => print_manifest_capabilities(format),
        },
    }
}

struct PortRequest {
    source: PathBuf,
    plan_format: OutputFormat,
    plan_output: Option<PathBuf>,
    prepare: Option<PathBuf>,
    agent: Option<String>,
    author_cmd: Option<String>,
    to: Option<PathBuf>,
    transcript: Option<PathBuf>,
    repair_attempts: u8,
    yes: bool,
    no_install: bool,
}

fn port_project(req: PortRequest) -> Result<u8> {
    let report = krate_port::analyze(&req.source)
        .with_context(|| format!("could not analyze {}", req.source.display()))?;
    let json = serde_json::to_string_pretty(&report)?;

    if let Some(path) = req.plan_output.as_deref() {
        if path.exists() {
            anyhow::bail!(
                "{} already exists; choose another --output path",
                path.display()
            );
        }
        fs::write(path, format!("{json}\n"))
            .with_context(|| format!("write {}", path.display()))?;
    }

    let wants_transform = req.agent.is_some() || req.author_cmd.is_some() || req.to.is_some();
    if wants_transform && req.to.is_none() {
        anyhow::bail!("--agent or --author-cmd requires --to <app.krate>");
    }
    if req.to.is_some() && req.agent.is_none() && req.author_cmd.is_none() {
        anyhow::bail!("--to requires --agent <agent> or --author-cmd <command>");
    }
    if req.yes && req.no_install {
        anyhow::bail!("--yes and --no-install cannot be used together");
    }

    let held_temp = if req.prepare.is_none() && wants_transform {
        Some(tempfile::tempdir().context("create port work dir")?)
    } else {
        None
    };
    let workspace_path = req.prepare.clone().unwrap_or_else(|| {
        held_temp
            .as_ref()
            .map(|temp| temp.path().join("workspace"))
            .unwrap_or_default()
    });
    let workspace = workspace_path.as_path();

    if req.prepare.is_some() || wants_transform {
        prepare_port_workspace(&report, workspace)?;
    }

    match req.plan_format {
        OutputFormat::Json => println!("{json}"),
        OutputFormat::Text => print!("{}", report.to_text()),
    }

    if req.prepare.is_some() {
        println!();
        println!("Prepared {}", workspace.display());
        println!("  review: {}", workspace.join("PORTING.md").display());
        println!("  AI task: {}", workspace.join("AGENT_TASK.md").display());
        println!("  candidate: {}", workspace.join("candidate").display());
        println!();
        println!("The original source was not changed.");
    }

    if let Some(output) = req.to.as_deref() {
        preflight_toolchain(req.yes, req.no_install)?;
        let command = match (req.agent.as_deref(), req.author_cmd.as_deref()) {
            (Some(name), None) => {
                let provider = resolve_agent(name)?;
                match provider.name() {
                    "claude" => PortAuthor::Claude,
                    // Porting drives the agent with its own prompts and repair
                    // loop, which have only been built for Claude so far. Say
                    // that plainly rather than silently doing something else.
                    other => anyhow::bail!(
                        "--agent {other} cannot port yet; porting supports only `claude` today. \
                         Use --author-cmd <command> to drive {other} yourself."
                    ),
                }
            }
            (None, Some(command)) => PortAuthor::Command(command),
            _ => anyhow::bail!("choose exactly one of --agent or --author-cmd"),
        };
        complete_port(
            &report,
            workspace,
            output,
            command,
            req.transcript.as_deref(),
            req.repair_attempts,
        )?;
    }

    Ok(0)
}

#[derive(Clone, Copy)]
enum PortAuthor<'a> {
    Claude,
    Command(&'a str),
}

/// Build the deterministic handoff between analysis and source transformation.
///
/// This intentionally stops before running an AI model. The workspace makes
/// every input visible first: the exact plan, a candidate that already obeys
/// Krate's strict component rules, and a task that forbids silent feature
/// deletion. A later agent run edits only `candidate/`; the original project
/// remains outside the workspace and untouched.
fn prepare_port_workspace(plan: &krate_port::PortPlan, workspace: &Path) -> Result<()> {
    use krate_author::{generate, AppKind, AppRequest};

    if workspace.exists() {
        anyhow::bail!(
            "{} already exists; choose a new --prepare directory",
            workspace.display()
        );
    }

    let sdk_root = match std::env::var_os("KRATE_SDK_ROOT") {
        Some(root) => PathBuf::from(root),
        None => sdk::ensure_materialized().context("prepare the embedded Krate SDK")?,
    };

    fs::create_dir_all(workspace).with_context(|| format!("create {}", workspace.display()))?;
    let source_snapshot = workspace.join("reference-source");
    let snapshot = krate_port::snapshot(&plan.source, &source_snapshot)
        .with_context(|| format!("create {}", source_snapshot.display()))?;
    let candidate = workspace.join("candidate");
    fs::create_dir_all(&candidate).with_context(|| format!("create {}", candidate.display()))?;

    let name = port_candidate_name(Path::new(&plan.source));
    // The starter is scaffolding the agent replaces, not a description of the
    // app being ported. Seeding a hex viewer with a word-frequency counter left
    // the candidate's own doc comment claiming it counts words while the task
    // said port hexyl -- a contradiction the agent has to notice and undo. The
    // choice is by shape (does it keep data? does it listen?), and the header
    // is rewritten below to say what it actually is.
    let kind = if plan.profile == "krate-cli-v1-candidate" {
        AppKind::WordFrequency
    } else if plan
        .suggested_capabilities
        .iter()
        .any(|capability| capability == "audio.capture")
    {
        AppKind::VoicePrompter
    } else {
        AppKind::Checklist
    };
    let mut request = match kind {
        AppKind::WordFrequency => AppRequest::word_frequency(&name),
        AppKind::Checklist => AppRequest::checklist(&name),
        AppKind::VoicePrompter => AppRequest::voice_prompter(&name),
    };
    request.description = format!("Port the existing {name} application to Krate.");
    let sdk_prefix = relative_sdk_prefix(&candidate, &sdk_root)?;
    let generated = generate(&request, &sdk_prefix).map_err(anyhow::Error::msg)?;
    for mut file in generated.files {
        // Replace the starter's own description so the candidate does not claim
        // to be something it is not. An agent opening src/lib.rs was told it
        // counts word frequencies while its task said port a hex viewer.
        if file.path == "src/lib.rs" {
            file.contents = rewrite_candidate_header(&file.contents, &name, &plan.source);
        }
        let destination = candidate.join(file.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&destination, file.contents)?;
    }
    fs::write(candidate.join("CONTRACT.md"), author_contract(&name))?;
    let mut workspace_plan = plan.clone();
    workspace_plan.source = "reference-source".to_string();
    fs::write(
        workspace.join("port-plan.json"),
        serde_json::to_string_pretty(&workspace_plan)? + "\n",
    )?;
    fs::write(
        workspace.join("snapshot-summary.json"),
        serde_json::to_string_pretty(&snapshot)? + "\n",
    )?;
    fs::write(workspace.join("PORTING.md"), porting_readme(plan, &name))?;
    fs::write(
        workspace.join("AGENT_TASK.md"),
        port_agent_task(&workspace_plan, &name),
    )?;
    let journeys = port_behavior_journeys(&workspace_plan);
    fs::write(
        workspace.join("journeys.json"),
        serde_json::to_string_pretty(&journeys)? + "\n",
    )?;
    fs::write(
        workspace.join("JOURNEYS.md"),
        port_behavior_journeys_markdown(&journeys),
    )?;
    Ok(())
}

/// Replace a generated starter's doc header with one that says what the
/// candidate actually is: scaffolding to be rewritten into a port of a specific
/// project. The starters are real working apps, so their headers describe those
/// apps -- accurate for `krate create`, actively misleading for `krate port`.
fn rewrite_candidate_header(source: &str, name: &str, origin: &str) -> String {
    // The header runs to the first line that is not a `//!` doc comment or
    // blank, which is where the starter's real code begins.
    let body_start = source
        .lines()
        .position(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with("//!")
        })
        .unwrap_or(0);
    let body: Vec<&str> = source.lines().skip(body_start).collect();

    let mut header = String::new();
    header.push_str(&format!("//! Port candidate for `{name}`.\n"));
    header.push_str("//!\n");
    header.push_str("//! This is a starting point, not the ported app. It is a working Krate\n");
    header.push_str("//! app of a similar shape, here so the crate compiles and the build,\n");
    header.push_str("//! import, and permission checks can run from the first attempt.\n");
    header.push_str("//!\n");
    header.push_str("//! Replace this with the behaviour of the original project:\n");
    header.push_str(&format!("//!   {origin}\n"));
    header.push_str("//!\n");
    header.push_str("//! Read `../PORTING.md` for what was found in that source and which\n");
    header.push_str("//! capabilities it maps onto. Keep the `no_std` discipline below: a\n");
    header.push_str("//! Krate component may import only `krate:*`, and a growable `String`\n");
    header.push_str("//! or `format!` pulls in the `wasi:*` set that packaging rejects.\n");

    format!("{header}\n{}", body.join("\n"))
}

fn port_candidate_name(source: &Path) -> String {
    let raw = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("ported-app");
    let mut name = String::new();
    let mut previous_dash = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if !name.is_empty() && !previous_dash {
            name.push('-');
            previous_dash = true;
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() {
        "ported-app".to_string()
    } else {
        name
    }
}

fn porting_readme(plan: &krate_port::PortPlan, name: &str) -> String {
    format!(
        "# Krate port workspace for `{name}`\n\
\n\
This directory was generated from a read-only analysis of:\n\
\n\
`{source}`\n\
\n\
The original project was not executed or changed. `reference-source/` is a\n\
read-only snapshot for the agent. Common credential files, dependency caches,\n\
build outputs, symlinks, and oversized files are excluded.\n\
\n\
## Files\n\
\n\
- `port-plan.json` is the machine-readable analysis.\n\
- `snapshot-summary.json` records what the safe snapshot included or excluded.\n\
- `reference-source/` is the read-only source snapshot.\n\
- `AGENT_TASK.md` is the bounded task for an AI coding agent.\n\
- `journeys.json` and `JOURNEYS.md` define the behavior that must be checked.\n\
- `candidate/` is a compiling Krate app that the agent may edit.\n\
- `candidate/CONTRACT.md` explains the component and permission rules.\n\
\n\
## Current assessment\n\
\n\
- Verdict: `{verdict:?}`\n\
- Profile: `{profile}`\n\
- Languages: {languages}\n\
- Frameworks: {frameworks}\n\
\n\
Review every blocker and planned behavior change before asking an agent to\n\
edit the candidate. A successful compile is not enough. The result must retain\n\
the accepted user journeys, import only `krate:*` interfaces, declare precise\n\
capabilities, and pass allow, deny, persistence, close, and reopen checks on\n\
Mac, Windows, and Linux.\n",
        source = plan.source,
        verdict = plan.verdict,
        profile = plan.profile,
        languages = if plan.languages.is_empty() {
            "not detected".to_string()
        } else {
            plan.languages.join(", ")
        },
        frameworks = if plan.frameworks.is_empty() {
            "not detected".to_string()
        } else {
            plan.frameworks.join(", ")
        },
    )
}

fn port_behavior_journeys(plan: &krate_port::PortPlan) -> serde_json::Value {
    let entry_points = if plan.entry_points.is_empty() {
        serde_json::json!([])
    } else {
        serde_json::json!(plan.entry_points)
    };
    let evidence = plan
        .findings
        .iter()
        .flat_map(|finding| {
            finding.evidence.iter().map(move |item| {
                serde_json::json!({
                    "finding": finding.id,
                    "path": item.path,
                    "line": item.line
                })
            })
        })
        .collect::<Vec<_>>();
    let mut journeys = vec![
        serde_json::json!({
            "id": "launch",
            "title": "Open the application",
            "kind": "automated",
            "steps": ["Open the .krate file with all requested access granted"],
            "expected": "The app starts successfully and its bounded quick path exits with code 0",
            "source_evidence": entry_points
        }),
        serde_json::json!({
            "id": "primary-task",
            "title": "Complete the source application's primary task",
            "kind": "manual",
            "steps": [
                "Run the original application and complete its main user task",
                "Run the ported application and repeat the same task",
                "Compare inputs, visible results, saved data, and errors"
            ],
            "expected": "The port preserves the accepted primary behavior or records the difference explicitly",
            "source_evidence": evidence
        }),
        serde_json::json!({
            "id": "same-bundle-three-systems",
            "title": "Open the same bundle on all supported systems",
            "kind": "ci",
            "steps": [
                "Use the exact same .krate bundle on macOS, Windows, and Linux",
                "Run its bounded quick path on each system"
            ],
            "expected": "The same bundle exits successfully on all three systems",
            "source_evidence": []
        }),
    ];

    if !plan.suggested_capabilities.is_empty() {
        journeys.push(serde_json::json!({
            "id": "permission-denial",
            "title": "Refuse required access",
            "kind": "automated",
            "steps": [
                "Open the ported app without one required capability",
                "Confirm the protected operation does not run"
            ],
            "expected": "Krate refuses the required operation with exit code 5 and does not grant ambient access",
            "capabilities": plan.suggested_capabilities,
            "source_evidence": []
        }));
    }
    if plan
        .suggested_capabilities
        .iter()
        .any(|capability| capability.starts_with("fs.write"))
    {
        journeys.push(serde_json::json!({
            "id": "persistence",
            "title": "Save, close, and reopen",
            "kind": "manual",
            "steps": [
                "Create or change a unique value",
                "Close the app",
                "Open the same .krate file again"
            ],
            "expected": "The saved value is visible after reopening when file access is granted",
            "source_evidence": []
        }));
    }

    serde_json::json!({
        "schema": "krate.port.journeys.v1",
        "source": plan.source,
        "profile": plan.profile,
        "journeys": journeys
    })
}

fn port_behavior_journeys_markdown(journeys: &serde_json::Value) -> String {
    let mut out = String::from(
        "# Port behavior journeys\n\n\
These checks prevent a successful compile from being mistaken for a successful\n\
port. Automated checks run during packaging. Manual and CI checks remain open\n\
until evidence is recorded.\n\n",
    );
    if let Some(items) = journeys["journeys"].as_array() {
        for item in items {
            let id = item["id"].as_str().unwrap_or("journey");
            let title = item["title"].as_str().unwrap_or("Untitled journey");
            let kind = item["kind"].as_str().unwrap_or("manual");
            out.push_str(&format!("## {title}\n\nID: `{id}`  \nKind: `{kind}`\n\n"));
            if let Some(steps) = item["steps"].as_array() {
                for (index, step) in steps.iter().enumerate() {
                    if let Some(step) = step.as_str() {
                        out.push_str(&format!("{}. {step}\n", index + 1));
                    }
                }
            }
            out.push_str(&format!(
                "\nExpected: {}\n\nStatus: not yet verified\n\n",
                item["expected"].as_str().unwrap_or("Record the result")
            ));
        }
    }
    out
}

fn port_agent_task(plan: &krate_port::PortPlan, name: &str) -> String {
    let findings = if plan.findings.is_empty() {
        "No source-level blockers were detected.".to_string()
    } else {
        plan.findings
            .iter()
            .map(|finding| {
                format!(
                    "- [{:?}] {}: {}",
                    finding.severity, finding.title, finding.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let entry_points = if plan.entry_points.is_empty() {
        "not detected".to_string()
    } else {
        plan.entry_points.join(", ")
    };

    format!(
        "# Port `{name}` to Krate\n\
\n\
Read-only source snapshot: `{source}`\n\
Candidate to edit: `candidate/`\n\
Portability profile: `{profile}`\n\
Detected entry points: {entry_points}\n\
\n\
Read `port-plan.json`, `journeys.json`, `reference-source/`, and\n\
`candidate/CONTRACT.md` before editing. Edit files only inside `candidate/`.\n\
\n\
Do not change `reference-source/`. Do not remove, replace, or defer behavior\n\
silently.\n\
\n\
Do not edit `candidate/src/bindings.rs`. It is generated from the WIT on every\n\
build, so edits there are overwritten, and a half-edited copy fails to compile\n\
in ways that look like your own code is wrong. A port spent two repair attempts\n\
stripping `std::` prefixes out of that file. If a name in it looks wrong, the\n\
call site is wrong, not the bindings.\n\
\n\
## Detected work\n\
\n\
{findings}\n\
\n\
## Required result\n\
\n\
1. Preserve the app's accepted user journeys using interfaces the selected\n\
   Krate profile supports.\n\
2. Record every behavior that cannot be preserved in `PORT_RESULT.md`.\n\
3. Import only `krate:*` interfaces. Do not add WASI or ambient host access.\n\
4. Declare only the precise capabilities the candidate needs.\n\
5. Keep the candidate's bounded `quick` verification path.\n\
6. Do not claim completion until the candidate builds, its imports pass, and\n\
   its permission wall has been tested.\n",
        source = plan.source,
        profile = plan.profile,
    )
}

fn complete_port(
    original_plan: &krate_port::PortPlan,
    workspace: &Path,
    output: &Path,
    author: PortAuthor<'_>,
    transcript_path: Option<&Path>,
    repair_attempts: u8,
) -> Result<()> {
    if original_plan.verdict == krate_port::Verdict::Unsupported {
        anyhow::bail!(
            "this source has blocking behavior; review {} before attempting a port",
            workspace.join("PORTING.md").display()
        );
    }
    if output.exists() {
        anyhow::bail!(
            "{} already exists; choose another --to path",
            output.display()
        );
    }

    let candidate = workspace.join("candidate");
    let original_source = Path::new(&original_plan.source);
    let source_snapshot = workspace.join("reference-source");
    let plan_path = workspace.join("port-plan.json");
    let task_path = workspace.join("AGENT_TASK.md");

    println!();
    println!("==> transforming the candidate");
    // Snapshot the app body before the agent runs. The scaffold compiles and
    // passes every check by design -- that is what makes the repair loop able
    // to start -- which also means an agent that does nothing produces a
    // "successful" port of the wrong program. It happened: an agent answered
    // with a thoughtful analysis in chat, edited nothing, exited 0, and a
    // 4,863-line markdown viewer "ported with zero repairs" as the checklist
    // starter. The exit code cannot catch that; comparing the code can.
    let scaffold_lib = fs::read_to_string(candidate.join("src/lib.rs")).unwrap_or_default();
    let author_name = match author {
        PortAuthor::Claude => {
            run_claude_port(workspace, &source_snapshot, &candidate, &task_path)?;
            "claude"
        }
        PortAuthor::Command(command) => {
            run_port_author_command(
                command,
                workspace,
                &source_snapshot,
                &candidate,
                &plan_path,
                &task_path,
                None,
            )?;
            "external-command"
        }
    };

    // The agent receives read access to the source in order to understand it,
    // but it must edit only the isolated candidate. Re-analyze the source and
    // compare the complete deterministic plan before building anything.
    let after_plan = krate_port::analyze(original_source)
        .with_context(|| format!("re-check original source {}", original_source.display()))?;
    if serde_json::to_vec(original_plan)? != serde_json::to_vec(&after_plan)? {
        anyhow::bail!(
            "the original source changed while the port agent was running; \
             Krate stopped before packaging. Review the source and workspace."
        );
    }

    let lib_after = fs::read_to_string(candidate.join("src/lib.rs")).unwrap_or_default();
    if lib_after == scaffold_lib {
        anyhow::bail!(
            "the port agent finished without changing the candidate: src/lib.rs is \
             byte-identical to the starter, so this would package the scaffold as if \
             it were the app. The agent's transcript is at {} -- it usually means the \
             agent explained the port instead of performing it.",
            workspace.join(".agent-transcript.txt").display()
        );
    }
    if lib_after.contains("This is a starting point, not the ported app") {
        anyhow::bail!(
            "the candidate still carries the starter's own header, so the app body \
             was not replaced. See the transcript at {}.",
            workspace.join(".agent-transcript.txt").display()
        );
    }

    let mut repairs_used = 0_u8;
    let validated = loop {
        println!("==> validating the port candidate");
        match validate_port_candidate(&candidate) {
            Ok(validated) => break validated,
            Err(error) if repairs_used < repair_attempts => {
                repairs_used += 1;
                let repair_dir = workspace.join("repair");
                fs::create_dir_all(&repair_dir)?;
                let error_path = repair_dir.join(format!("attempt-{repairs_used}.txt"));
                fs::write(&error_path, format!("{error}\n"))?;
                println!(
                    "==> repair attempt {repairs_used}/{repair_attempts}: {}",
                    first_error_line(&error)
                );
                run_port_repair(
                    author,
                    workspace,
                    &source_snapshot,
                    &candidate,
                    &plan_path,
                    &task_path,
                    PortRepair {
                        attempt: repairs_used,
                        error_path: &error_path,
                    },
                )?;
                ensure_original_source_unchanged(original_plan, original_source)?;
            }
            Err(error) => {
                // The port is over. Before the error goes to the terminal and
                // is lost, say what kind of failure it was and what that means
                // for the person waiting -- and write the report they can
                // choose to send us. Nothing leaves this machine here.
                let report_path = write_port_failure_report(workspace, &error, original_source);
                print_port_failure_guidance(&error, report_path.as_deref());
                anyhow::bail!(
                    "the port candidate did not pass validation after {} repair attempt(s):\n{}",
                    repairs_used,
                    error
                )
            }
        }
    };
    let wasm = validated.wasm;
    let manifest = validated.manifest;

    println!("==> packing {}", output.display());
    let manifest_src = candidate.join("manifest.toml");
    let pack_dir = tempfile::tempdir().context("create port pack dir")?;
    let code = pack_dir.path().join("code.wasm");
    fs::copy(&wasm, &code)?;
    let packed_manifest = pack_dir.path().join("manifest.toml");
    write_manifest_with_entry(&manifest_src, &packed_manifest, "code.wasm")?;
    let assets = candidate.join("assets");
    let size = krate_bundle::pack_with_assets(
        &packed_manifest,
        &code,
        assets.is_dir().then_some(assets.as_path()),
        output,
    )
    .with_context(|| format!("pack {}", output.display()))?;

    println!("==> verifying the permission wall");
    // Verification is inside the repair budget too. It used to sit outside it,
    // so a candidate that compiled cleanly and computed the wrong answer got
    // zero attempts while a build error got two -- backwards, because a failing
    // self-check names the problem in one line and is the most repairable
    // failure there is. A ported RSS reader stripped tags before decoding
    // entities, said so in its own output, and was never asked to try again.
    let gating = loop {
        match verify_packed_app(output, &manifest) {
            Ok(gating) => break gating,
            Err(error) if repairs_used < repair_attempts => {
                repairs_used += 1;
                let repair_dir = workspace.join("repair");
                fs::create_dir_all(&repair_dir)?;
                let error_path = repair_dir.join(format!("verify-{repairs_used}.txt"));
                fs::write(&error_path, format!("{error}\n"))?;
                println!(
                    "==> repair attempt {repairs_used}/{repair_attempts}: {}",
                    first_error_line(&error.to_string())
                );
                run_port_repair(
                    author,
                    workspace,
                    &source_snapshot,
                    &candidate,
                    &plan_path,
                    &task_path,
                    PortRepair {
                        attempt: repairs_used,
                        error_path: &error_path,
                    },
                )?;
                ensure_original_source_unchanged(original_plan, original_source)?;
                // The repair changed the source, so rebuild and repack before
                // asking again -- otherwise the next attempt verifies the same
                // bundle and fails identically.
                let revalidated = validate_port_candidate(&candidate).map_err(|err| {
                    anyhow::anyhow!("the repaired candidate no longer builds:\n{err}")
                })?;
                fs::copy(&revalidated.wasm, &code)?;
                write_manifest_with_entry(&manifest_src, &packed_manifest, "code.wasm")?;
                krate_bundle::pack_with_assets(
                    &packed_manifest,
                    &code,
                    assets.is_dir().then_some(assets.as_path()),
                    output,
                )
                .with_context(|| format!("repack {}", output.display()))?;
            }
            Err(error) => {
                let report_path =
                    write_port_failure_report(workspace, &error.to_string(), original_source);
                print_port_failure_guidance(&error.to_string(), report_path.as_deref());
                return Err(error);
            }
        }
    };
    let bundle_sha256 = sha256_file(output)?;
    let plan_sha256 = sha256_file(&workspace.join("port-plan.json"))?;
    let permissions: Vec<String> = manifest
        .capabilities
        .iter()
        .map(|capability| capability.cap.clone())
        .collect();
    let port_result = workspace.join("PORT_RESULT.md");
    let result_note = if port_result.is_file() {
        Some(
            fs::read_to_string(&port_result)
                .with_context(|| format!("read {}", port_result.display()))?,
        )
    } else {
        None
    };
    let journey_results = serde_json::json!({
        "schema": "krate.port.journey-results.v1",
        "bundle_sha256": bundle_sha256,
        "results": [
            {
                "id": "launch",
                "status": "passed",
                "evidence": "bounded quick path exited with code 0"
            },
            {
                "id": "permission-denial",
                "status": "passed",
                "evidence": format!("withholding {gating} exited with code 5")
            },
            {
                "id": "primary-task",
                "status": "not-verified",
                "evidence": "manual comparison with the source application is required"
            },
            {
                "id": "same-bundle-three-systems",
                "status": "not-verified",
                "evidence": "the exact bundle must run on macOS, Windows, and Linux"
            }
        ]
    });
    fs::write(
        workspace.join("journey-results.json"),
        serde_json::to_string_pretty(&journey_results)? + "\n",
    )?;
    let artifact = serde_json::json!({
        "schema": "krate.port.artifact.v1",
        "bundle": output.to_string_lossy(),
        "bundle_sha256": bundle_sha256,
        "bundle_bytes": size,
        "port_plan_sha256": plan_sha256,
        "profile": original_plan.profile,
        "requested_permissions": permissions,
        "source_unchanged": true
    });
    fs::write(
        workspace.join("artifact.json"),
        serde_json::to_string_pretty(&artifact)? + "\n",
    )?;
    let transcript = serde_json::json!({
        "schema": "krate.port.result.v1",
        "source": original_plan.source,
        "profile": original_plan.profile,
        "plan_verdict": original_plan.verdict,
        "author": author_name,
        "repair_attempts_allowed": repair_attempts,
        "repair_attempts_used": repairs_used,
        "source_unchanged": true,
        "output": output.to_string_lossy(),
        "bundle_sha256": bundle_sha256,
        "port_plan_sha256": plan_sha256,
        "krate_bytes": size,
        "requested_permissions": permissions,
        "gating_permission": gating,
        "agent_result": result_note,
        "checks": [
            "candidate built as a WebAssembly component",
            "component imports only krate:* interfaces",
            "bundle runs with all declared grants",
            "bundle refuses when its required gating permission is withheld"
        ],
        "behavior_journeys": workspace.join("journeys.json").to_string_lossy(),
        "journey_results": workspace.join("journey-results.json").to_string_lossy(),
        "artifact_evidence": workspace.join("artifact.json").to_string_lossy(),
        "remaining_verification": [
            "compare the original and ported user journeys",
            "test interactive behavior and persistence",
            "run the same bundle on Mac, Windows, and Linux"
        ]
    });
    let transcript_json = serde_json::to_string_pretty(&transcript)? + "\n";
    fs::write(workspace.join("port-result.json"), &transcript_json)?;
    if let Some(path) = transcript_path {
        if path.exists() {
            anyhow::bail!(
                "{} already exists; choose another --transcript path",
                path.display()
            );
        }
        fs::write(path, &transcript_json).with_context(|| format!("write {}", path.display()))?;
    }

    println!();
    println!("Created {}", output.display());
    println!("  source unchanged: yes");
    println!("  sha256: {bundle_sha256}");
    println!("  repair attempts used: {repairs_used}");
    println!("  requested access:");
    for permission in &permissions {
        println!("    - {permission}");
    }
    println!();
    println!(
        "Build and permission checks passed. Compare the original and ported \
         user journeys before sharing this app."
    );
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

struct ValidatedPortCandidate {
    wasm: PathBuf,
    manifest: krate_manifest::Manifest,
}

fn validate_port_candidate(
    candidate: &Path,
) -> std::result::Result<ValidatedPortCandidate, String> {
    for required in ["Cargo.toml", "src/lib.rs", "manifest.toml"] {
        if !candidate.join(required).is_file() {
            return Err(format!("candidate/{required} is missing"));
        }
    }

    let wasm = build_component_captured(candidate)?;
    let wasm_bytes =
        fs::read(&wasm).map_err(|error| format!("could not read {}: {error}", wasm.display()))?;
    let bad = krate_bundle::imports::non_krate_imports(&wasm_bytes)
        .map_err(|error| format!("could not inspect component imports: {error}"))?;
    if !bad.is_empty() {
        // This exact message is what a repair attempt is handed, so it has to
        // carry the cause and not just the symptom. A port of an image viewer
        // spent all four attempts on this list: the agent kept rewriting its
        // own code, and the leak was `image` in Cargo.toml, which requires
        // `std` and pulls the whole wasi surface in no matter how the app is
        // written. Naming the usual cause turns four guesses into one edit.
        // This exact message is what a repair attempt is handed, so it carries
        // the cause and not just the symptom. Two ports burned every attempt
        // here: one on `image` in Cargo.toml, one on a `buf[i]` whose bounds
        // check kept std's panic path reachable. Both look like the Krate
        // calls are wrong, and neither is.
        let mut hints = panic_site_hints(candidate);
        if hints.is_empty() {
            hints.push_str(
                "\n  (no obvious panic site found in src/lib.rs -- check dependencies next)",
            );
        }
        return Err(format!(
            "the component imports unsupported host APIs: {}\n\
             \n\
             This is almost always one reachable panic, not a Krate call. std's \
             failure path formats a message, writes it, and exits, which is \
             wasi:cli, wasi:filesystem, and wasi:io arriving together -- so it is \
             all-or-nothing: one panic site is the whole list above.\n\
             \n\
             The two usual causes, in order:\n\
             \n\
             1. Indexing. `buf[i]` carries a bounds check that can panic even when \
             the index is provably fine. Use `.get(i)` / `.get_mut(i)` and handle \
             the `None`.\n\
             1b. A `Vec` grown inside a loop -- `push` or `extend_from_slice` \
             repeatedly -- keeps std's reallocation path reachable, and that path \
             ends at the out-of-memory handler. `.get()` everywhere does not save \
             it. If the size is known, build a fixed `[T; N]` instead; a mesh \
             builder written both ways measured thirty-three wasi imports as a \
             `Vec` and zero as an array.\n\
             2. `.to_string()` or `format!`, which route through the allocator's \
             out-of-memory handler. Copy `pure_string` from the samples.\n\
             \n\
             Then dependencies: a crate needing `std` brings all of this with it \
             whatever the app does. `image` is the usual culprit -- `zune-png` and \
             `zune-jpeg` (with `default-features = false`, plus `zune-core`) decode \
             the same formats cleanly.\n\
             \n\
             Places in this candidate worth looking at first:{}",
            bad.join(", "),
            hints
        ));
    }

    let manifest_src = candidate.join("manifest.toml");
    let manifest = krate_manifest::Manifest::parse_file(&manifest_src)
        .map_err(|error| format!("invalid candidate/manifest.toml: {error:#}"))?;
    Ok(ValidatedPortCandidate { wasm, manifest })
}

/// Show a failure report and, with consent, help send it.
///
/// The whole file is printed first. Someone deciding whether to share their
/// source with us has to be able to see what "the report" actually contains,
/// and a summary is not that -- the point of showing it is that they can find
/// anything in it they would rather not send.
///
/// There is no automatic upload. Krate does not send anything from this
/// machine; it opens the issue form with the report ready to paste, so the last
/// action is theirs, in their own browser, where they can still edit it.
fn run_report_command(report: &Path, show_only: bool) -> Result<u8> {
    let text = fs::read_to_string(report)
        .with_context(|| format!("read the report at {}", report.display()))?;

    println!("{text}");
    println!("---");

    if show_only {
        return Ok(0);
    }

    println!();
    println!("Everything above is what would be sent. It is still only on your computer.");
    println!();
    println!("Krate will not upload this report for you -- it can contain your");
    println!("source and paths, so sending it stays your call. To send it:");
    println!("  1. Copy the text above.");
    println!("  2. Open https://github.com/incyashraj/krate/issues/new");
    println!("  3. Paste it, edit out anything you would rather not share, and post.");
    println!();
    println!("Anything you leave in is public on that page, so read it once more first.");
    println!();
    println!("(Sharing a working app is a different thing: `krate publish <app.krate>`");
    println!("uploads the app and prints a URL anyone can `krate run`.)");

    Ok(0)
}

/// Every capability an app has to ask for, in the form a manifest writes it.
///
/// Generated from the runtime's capability registry rather than typed out, so a
/// capability added without being listed here cannot happen. Default-granted
/// ones are left out on purpose: declaring `io.stdout` is noise, and an app
/// that lists it is telling a person something that is true of every app.
fn requestable_capability_list() -> String {
    let mut out = String::from("These are the capability names a manifest may use:\n\n");
    for spec in krate_manifest::supported_capability_specs() {
        if spec.default_granted() {
            continue;
        }
        out.push_str(&format!("- `{}`\n", spec.display_pattern()));
    }
    out.push_str(
        "\nA name outside this list is refused when the app is packed. Where a\n\
         pattern shows `<path-glob>` or `<host>:<port>`, scope it to the\n\
         narrowest thing the app actually needs.\n",
    );
    out
}

/// Write a report about a failed port, next to the workspace it failed in.
///
/// Local only. Nothing is transmitted here and nothing is transmitted later
/// without the person choosing to send it: a failure report can contain their
/// source, their paths, and their project's name, and taking that quietly is
/// the kind of thing a developer tool does not come back from.
///
/// Returns the path so the caller can tell them where it is. A failure to write
/// the report is not a failure of the port -- the port already failed -- so it
/// returns `None` rather than compounding one error with another.
fn write_port_failure_report(
    workspace: &Path,
    error: &str,
    source: &Path,
) -> Option<std::path::PathBuf> {
    let failure = port_report::classify(error);
    let path = workspace.join("FAILURE-REPORT.md");

    let mut text = String::new();
    text.push_str("# Krate port failure report\n\n");
    text.push_str(
        "This file is on your computer and has not been sent anywhere. Read it, and\n\
         send it only if you want to.\n\n",
    );
    text.push_str(&format!("- What kind: {}\n", failure.kind.label()));
    text.push_str(&format!("- Source: {}\n", source.display()));
    text.push_str(&format!("- Krate: {}\n", env!("CARGO_PKG_VERSION")));
    text.push_str(&format!("- Platform: {}\n\n", std::env::consts::OS));

    if !failure.unknown_names.is_empty() {
        text.push_str("## Names the AI used that Krate does not have\n\n");
        for name in &failure.unknown_names {
            text.push_str(&format!("- `{name}`\n"));
        }
        text.push('\n');
    }
    if !failure.foreign_imports.is_empty() {
        text.push_str("## Imports outside krate:*\n\n");
        for import in &failure.foreign_imports {
            text.push_str(&format!("- `{import}`\n"));
        }
        text.push('\n');
    }

    text.push_str("## What this means\n\n");
    text.push_str(failure.kind.promise());
    text.push_str("\n\n## The full error\n\n```\n");
    text.push_str(error.trim());
    text.push_str("\n```\n");

    fs::write(&path, text).ok().map(|()| path)
}

/// Tell the person what kind of failure this was and what happens next.
///
/// Printed before the error itself, because the error is long and the part they
/// need -- is this our gap or their code, and how long -- is one line.
fn print_port_failure_guidance(error: &str, report_path: Option<&Path>) {
    let failure = port_report::classify(error);

    eprintln!();
    eprintln!("This port failed: {}.", failure.kind.label());
    eprintln!("{}", failure.kind.promise());

    if !failure.unknown_names.is_empty() {
        eprintln!();
        eprintln!("The AI used names that do not exist:");
        for name in &failure.unknown_names {
            eprintln!("  - {name}");
        }
    }

    if let Some(path) = report_path {
        eprintln!();
        eprintln!("A report is saved at {}.", path.display());
        eprintln!("It has not been sent anywhere. To send it to us, run:");
        eprintln!("  krate report {}", path.display());
        // Only ask for a report where one changes what we do. A borrow error in
        // generated code is the agent having an off day, and asking for it
        // trains people to ignore the request for the failures that matter.
        if failure.kind.is_quick_fix() {
            eprintln!();
            eprintln!("Sending it is what tells us to close this gap.");
        }
    }
    eprintln!();
}

fn first_error_line(error: &str) -> &str {
    error
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("candidate validation failed")
}

fn ensure_original_source_unchanged(
    original_plan: &krate_port::PortPlan,
    original_source: &Path,
) -> Result<()> {
    let after_plan = krate_port::analyze(original_source)
        .with_context(|| format!("re-check original source {}", original_source.display()))?;
    if serde_json::to_vec(original_plan)? != serde_json::to_vec(&after_plan)? {
        anyhow::bail!(
            "the original source changed while the port agent was running; \
             Krate stopped before packaging. Review the source and workspace."
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PortRepair<'a> {
    attempt: u8,
    error_path: &'a Path,
}

fn run_port_repair(
    author: PortAuthor<'_>,
    workspace: &Path,
    source: &Path,
    candidate: &Path,
    plan: &Path,
    task: &Path,
    repair: PortRepair<'_>,
) -> Result<()> {
    match author {
        PortAuthor::Claude => run_claude_port_repair(
            workspace,
            source,
            candidate,
            task,
            repair.attempt,
            repair.error_path,
        ),
        PortAuthor::Command(command) => run_port_author_command(
            command,
            workspace,
            source,
            candidate,
            plan,
            task,
            Some((repair.attempt, repair.error_path)),
        ),
    }
}

fn run_port_author_command(
    command: &str,
    workspace: &Path,
    source: &Path,
    candidate: &Path,
    plan: &Path,
    task: &Path,
    repair: Option<(u8, &Path)>,
) -> Result<()> {
    let shell = author_shell();
    let mut child = ProcessCommand::new(shell);
    child
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .env("KRATE_PORT_SOURCE", source)
        .env("KRATE_PORT_CANDIDATE", candidate)
        .env("KRATE_PORT_PLAN", plan)
        .env("KRATE_PORT_TASK", task);
    if let Some((attempt, error_path)) = repair {
        child
            .env("KRATE_PORT_REPAIR_ATTEMPT", attempt.to_string())
            .env("KRATE_PORT_REPAIR_LOG", error_path);
    }
    let status = child.status().context("run --author-cmd")?;
    if !status.success() {
        anyhow::bail!("port author command failed");
    }
    Ok(())
}

fn run_claude_port(workspace: &Path, source: &Path, candidate: &Path, task: &Path) -> Result<()> {
    let task_text = fs::read_to_string(task).with_context(|| format!("read {}", task.display()))?;
    let prompt = format!(
        "{task_text}\n\
\n\
The source path and candidate path are absolute:\n\
- source, read only: {source}\n\
- candidate, edit here: {candidate}\n\
\n\
Use Read to understand the source. Use Edit or Write only inside the candidate\n\
directory. When finished, write a short PORT_RESULT.md in the workspace listing\n\
what was preserved, changed, and not yet supported. Do not explain in chat;\n\
perform the port.",
        source = source.display(),
        candidate = candidate.display(),
    );
    let transcript = workspace.join(".agent-transcript.txt");
    let file = fs::File::create(&transcript).ok();
    let mut command = ProcessCommand::new("claude");
    command
        .arg("-p")
        .arg(prompt)
        .arg("--allowed-tools")
        .arg("Read,Edit,Write")
        .arg("--permission-mode")
        .arg("acceptEdits")
        .current_dir(workspace);
    if let Some(file) = &file {
        if let Ok(clone) = file.try_clone() {
            command.stdout(std::process::Stdio::from(clone));
        }
        if let Ok(clone) = file.try_clone() {
            command.stderr(std::process::Stdio::from(clone));
        }
    }
    let status = command
        .status()
        .context("run the `claude` CLI (is Claude Code installed and signed in?)")?;
    if !status.success() {
        anyhow::bail!(
            "the Claude port agent did not finish successfully; see {}",
            transcript.display()
        );
    }
    Ok(())
}

fn run_claude_port_repair(
    workspace: &Path,
    source: &Path,
    candidate: &Path,
    task: &Path,
    attempt: u8,
    error_path: &Path,
) -> Result<()> {
    let task_text = fs::read_to_string(task).with_context(|| format!("read {}", task.display()))?;
    let error_text =
        fs::read_to_string(error_path).with_context(|| format!("read {}", error_path.display()))?;
    let prompt = format!(
        "{task_text}\n\
\n\
The previous candidate failed Krate validation. This is repair attempt\n\
{attempt}. Read the candidate and fix only the reported failure. Do not weaken\n\
the manifest, remove behavior, add WASI imports, or edit the source snapshot.\n\
\n\
Source snapshot, read only: {source}\n\
Candidate, edit here: {candidate}\n\
\n\
Validation error:\n\
{error_text}\n\
\n\
Use Read to inspect files and Edit or Write only inside the candidate. Do not\n\
explain in chat. Make the smallest complete repair.",
        source = source.display(),
        candidate = candidate.display(),
    );
    let transcript = workspace.join(format!(".agent-repair-{attempt}.txt"));
    let file = fs::File::create(&transcript).ok();
    let mut command = ProcessCommand::new("claude");
    command
        .arg("-p")
        .arg(prompt)
        .arg("--allowed-tools")
        .arg("Read,Edit,Write")
        .arg("--permission-mode")
        .arg("acceptEdits")
        .current_dir(workspace);
    if let Some(file) = &file {
        if let Ok(clone) = file.try_clone() {
            command.stdout(std::process::Stdio::from(clone));
        }
        if let Ok(clone) = file.try_clone() {
            command.stderr(std::process::Stdio::from(clone));
        }
    }
    let status = command
        .status()
        .context("run the `claude` CLI for a port repair")?;
    if !status.success() {
        anyhow::bail!(
            "the Claude repair attempt did not finish successfully; see {}",
            transcript.display()
        );
    }
    Ok(())
}

fn verify_packed_app(output: &Path, manifest: &krate_manifest::Manifest) -> Result<String> {
    let gating = gating_capability(manifest);
    let verify_dir = tempfile::tempdir().context("create port verification dir")?;
    let verify_arg =
        prepare_verify_dir(verify_dir.path(), manifest)?.unwrap_or_else(|| "quick".to_string());
    let bundle = absolute_output_path(output)?;

    let bundle_str = bundle.to_str().context("bundle path is not valid UTF-8")?;
    let run_with = |arg: &str| -> Result<i32> {
        run_self(
            verify_dir.path(),
            // Headless: verification is an automated run, not a windowed
            // session. A GUI app opened windowed in this non-interactive
            // context traps; headless runs the same code without a window.
            &[
                "run",
                bundle_str,
                "--untrusted",
                "--auto-grant",
                "--headless",
                "--",
                arg,
            ],
        )
    };

    // The contract asks a ported app to accept both a file path and the bare
    // word `quick`, so either one working is proof the app runs. Trying only
    // the path failed two real ports that were working correctly: a duplicate
    // finder that takes directories and a database CLI that takes subcommands,
    // both handed `input/sample.txt` because they declared an `fs.read` grant.
    let allow_exit = match run_with(&verify_arg)? {
        0 => 0,
        _ if verify_arg != "quick" => run_with("quick")?,
        other => other,
    };
    if allow_exit != 0 {
        anyhow::bail!(
            "the ported app failed with all grants (exit {allow_exit}); \
             it was run with `{verify_arg}` and then with `quick`"
        );
    }

    // Nothing suitable to withhold: the app asks only for what every app gets
    // plus its own window. Say so rather than inventing a capability it never
    // requested and calling the result a failure.
    let Some(gating) = gating else {
        return Ok(
            "(nothing to withhold: the app asks only for defaults and its window)".to_string(),
        );
    };

    let mut deny_args = vec!["run".to_string(), bundle.to_string_lossy().into_owned()];
    for capability in &manifest.capabilities {
        if capability.cap == gating {
            continue;
        }
        deny_args.push("--grant".to_string());
        deny_args.push(capability.cap.clone());
    }
    deny_args.push("--".to_string());
    deny_args.push(verify_arg);
    let deny_refs: Vec<&str> = deny_args.iter().map(String::as_str).collect();
    let deny_exit = run_self(verify_dir.path(), &deny_refs)?;
    if deny_exit != 5 {
        anyhow::bail!(
            "withholding {gating} should refuse the ported app with exit 5, got {deny_exit}"
        );
    }
    Ok(gating)
}

struct RunRequest {
    /// What the user asked to run: a path, a bundle, or a URL.
    target: String,
    /// Resolved component path. Filled in by resolve_run_target.
    file: PathBuf,
    fuel: Option<u64>,
    mem_limit: u64,
    max_http_response_bytes: usize,
    http_timeout_millis: u32,
    sandbox_root: PathBuf,
    manifest_path: Option<PathBuf>,
    grants: Vec<String>,
    auto_grant: bool,
    prompt: bool,
    consent: bool,
    /// How this run should present a GUI: a real window, headless, or a
    /// window with a headless fallback when the machine has no display.
    ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode,
    insecure_http: bool,
    json: bool,
    dump_caps: bool,
    dump_caps_format: OutputFormat,
    log_grants: Option<PathBuf>,
    log_grants_format: GrantLogFormat,
    test_time_millis: Option<u64>,
    test_locale: Option<String>,
    test_timezone: Option<String>,
    /// When set, a headless GUI run paints the window to this PNG.
    screenshot_path: Option<PathBuf>,
    /// Display scale for the screenshot.
    screenshot_scale: f32,
    /// When set, drive the run against the usability script and write what was
    /// observed here.
    usability_report: Option<PathBuf>,
    app_args: Vec<String>,
}

struct ManifestInitRequest {
    id: String,
    name: String,
    version: String,
    entry: PathBuf,
    capabilities: Vec<String>,
    output: Option<PathBuf>,
    force: bool,
}

/// Remove Krate from an AI app's config: the exact reverse of `connect`.
///
/// Needed for more than tidiness. The common case is reconnecting after
/// something broke -- moving the binary, or a half-written config -- and
/// without a way to remove the old entry the only fix was hand-editing JSON.
pub(crate) fn disconnect_target(target: &ClientTarget) -> Result<bool> {
    if !target.path.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(&target.path)?;
    let mut config: serde_json::Value = serde_json::from_str(&existing).with_context(|| {
        format!(
            "{} is not valid JSON, so I will not rewrite it.",
            target.path.display()
        )
    })?;

    let Some(servers) = config
        .as_object_mut()
        .and_then(|object| object.get_mut("mcpServers"))
        .and_then(|servers| servers.as_object_mut())
    else {
        return Ok(false);
    };
    if servers.remove("krate").is_none() {
        return Ok(false);
    }

    // Only Krate's own entry is touched; anything else in the file is left
    // exactly as it was.
    fs::write(
        &target.path,
        format!("{}\n", serde_json::to_string_pretty(&config)?),
    )?;
    Ok(true)
}

/// Which AI apps currently have Krate connected.
pub(crate) fn connected_targets() -> Vec<(ClientTarget, bool)> {
    connect_targets()
        .into_iter()
        .map(|target| {
            let connected = fs::read_to_string(&target.path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .map(|config| config.pointer("/mcpServers/krate").is_some())
                .unwrap_or(false);
            (target, connected)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Seams the interactive front door calls into.
//
// Each one wraps an existing command path rather than duplicating it, so the
// menu and the flags stay the same product. If these ever diverge, the menu is
// lying about what Krate does.
// ---------------------------------------------------------------------------

/// The old bare-`krate` behaviour, kept for pipes and scripts.
pub(crate) fn print_help_summary() -> Result<u8> {
    use clap::CommandFactory;
    Cli::command().print_help()?;
    println!();
    Ok(0)
}

/// Author an app for the interactive menu.
/// Where authoring progress goes when the interactive front door is driving.
///
/// A process-global sink rather than a parameter threaded through
/// `create_krate` and everything below it: there is exactly one authoring run
/// at a time, and the alternative touches a dozen signatures for one optional
/// display.
static PROGRESS_SINK: std::sync::Mutex<Option<std::sync::Arc<progress::Progress>>> =
    std::sync::Mutex::new(None);

/// Set on the authoring child when the parent is drawing a progress display.
///
/// The front door re-invokes this binary (`krate author-agent <name>`) to do
/// the authoring, so the display and the code that knows what the AI is doing
/// end up in different processes. This variable tells the child to report over
/// stdout instead of drawing, and [`PROGRESS_PREFIX`] tags those lines so the
/// parent can tell them apart from the agent's own output.
const PROGRESS_CHANNEL: &str = "KRATE_PROGRESS_CHANNEL";

/// Marks a line as a progress report from the authoring child.
///
/// Deliberately unlikely to occur in compiler output or an agent transcript:
/// anything that is not this is passed through as ordinary output.
const PROGRESS_PREFIX: &str = "\u{1}krate-progress\u{1}";

/// Marks a "still working, same step" report from the authoring child.
const PROGRESS_TICK: &str = "\u{1}krate-tick\u{1}";

fn set_progress_sink(sink: Option<std::sync::Arc<progress::Progress>>) {
    if let Ok(mut slot) = PROGRESS_SINK.lock() {
        *slot = sink;
    }
}

/// The display currently drawing, if there is one.
fn progress_sink() -> Option<std::sync::Arc<progress::Progress>> {
    PROGRESS_SINK.lock().ok().and_then(|slot| slot.clone())
}

/// Tell the display the agent is still working, without changing the stage.
///
/// Sent for a step identical to the previous one. The person needs to know the
/// difference between "reading, still" and "stopped", and the elapsed clock
/// alone cannot say which -- it counts up either way.
fn report_progress_alive(step: &str) {
    if let Some(progress) = progress_sink() {
        progress.tick(step);
    } else if std::env::var_os(PROGRESS_CHANNEL).is_some() {
        println!("{PROGRESS_TICK}{step}");
        let _ = io::stdout().flush();
    }
}

/// Report one line of agent progress, if anything is listening.
fn report_progress(step: &str) -> bool {
    let Ok(slot) = PROGRESS_SINK.lock() else {
        return false;
    };
    let Some(progress) = slot.as_ref() else {
        return false;
    };
    // Map what the agent says it is doing onto a phase a person understands,
    // and never move backwards.
    //
    // An AI reads all the way through a run -- it re-reads the reference while
    // fixing a compile error, and looks at an example again while packaging.
    // Sending it back to stage one for each of those is what kept the display
    // pinned on the first line for a whole five-minute run while the app was
    // being written, built and packed. What it is reading right now belongs on
    // the detail line, not in the stage.
    let lower = step.to_lowercase();
    let stage = if lower.contains("check-app")
        || lower.contains("cargo")
        || lower.contains("build")
        || lower.contains("compil")
    {
        2
    } else if lower.starts_with("writing")
        || lower.starts_with("editing")
        || lower.starts_with("setting up")
        || lower.starts_with("declaring")
    {
        1
    } else {
        // Reading, searching, anything else: whatever phase we are already in.
        0
    };
    progress.advance_to_at_least(stage);
    progress.note(step.to_string());
    true
}

/// Author an app with a live progress display driving the terminal.
pub(crate) fn author_app_for_tui_watched(
    request: &str,
    provider: &'static dyn agent_provider::AgentProvider,
    output: &Path,
    progress: &std::sync::Arc<progress::Progress>,
    attachments: &[PathBuf],
) -> Result<()> {
    set_progress_sink(Some(std::sync::Arc::clone(progress)));
    let result = author_app_for_tui(request, provider, output, attachments);
    set_progress_sink(None);
    result
}

pub(crate) fn author_app_for_tui(
    request: &str,
    provider: &'static dyn agent_provider::AgentProvider,
    output: &Path,
    attachments: &[PathBuf],
) -> Result<()> {
    // Attachments are staged into the app directory, so the agent can open
    // them with the ordinary file tools it already has. Passing an image
    // through the prompt is not possible for every provider; a file on disk
    // beside the code works for all of them.
    let staged = if attachments.is_empty() {
        None
    } else {
        Some(tempfile::tempdir().context("make a working directory for the app")?)
    };
    let mut request = request.to_string();
    if let Some(staged) = &staged {
        let inbox = staged.path().join("attached");
        fs::create_dir_all(&inbox).context("make the attachments directory")?;
        let mut named = Vec::new();
        for source in attachments {
            let Some(name) = source.file_name() else {
                continue;
            };
            let destination = inbox.join(name);
            if fs::copy(source, &destination).is_ok() {
                named.push(format!("attached/{}", name.to_string_lossy()));
            }
        }
        if !named.is_empty() {
            request.push_str(
                "\n\nThe person attached these files, in this directory. Read them \
                 before you write any code -- they are part of the request, and \
                 usually say more about what is wanted than the sentence above:\n",
            );
            for name in &named {
                request.push_str(&format!("  {name}\n"));
            }
            request.push_str(
                "\nIf one is a screenshot or a design, build something that looks like \
                 it. If one is the source of an app they already have, build the same \
                 thing as a Krate app -- keeping what it does, not how it was written, \
                 since it was written against a different system.",
            );
        }
    }

    let code = create_krate(CreateRequest {
        request,
        output: output.to_path_buf(),
        author_cmd: Some(agent_author_command(provider)),
        kind: None,
        name: None,
        transcript: None,
        work_dir: staged.as_ref().map(|dir| dir.path().to_path_buf()),
        // The menu already asked; asking again inside would be a second
        // question about a decision the person has made.
        yes: true,
        no_install: false,
        json: false,
        force: false,
    })?;
    usage::record_with(
        usage::Action::Make,
        usage::Facts {
            ai: Some(true),
            ok: Some(code == 0),
        },
    );
    if code == 0 {
        remember_app(output);
        Ok(())
    } else {
        Err(anyhow::anyhow!("the app could not be built"))
    }
}

/// Extract a bundle's source, when it carries any.
pub(crate) fn bundle_source_dir(bundle: &Path) -> Result<Option<PathBuf>> {
    let opened = krate_bundle::open(bundle)?;
    let Some(source) = opened.source_path() else {
        return Ok(None);
    };
    // The opened bundle deletes its temp directory on drop, so the source is
    // copied somewhere that outlives this call.
    let target = std::env::temp_dir().join(format!("krate-edit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&target);
    copy_tree(source, &target)?;

    // The bundle carries {KRATE_SDK} where this machine's SDK path goes, so
    // the source rebuilds anywhere rather than only where it was made. Point
    // it at the local SDK now that there is one to point at.
    let manifest = target.join("Cargo.toml");
    if let Ok(text) = fs::read_to_string(&manifest) {
        if text.contains(krate_bundle::SDK_PLACEHOLDER) {
            // The SDK inside the bundle wins: it is the one this source was
            // written against, so it compiles where the current one may not.
            let sdk = match opened.sdk_path() {
                Some(bundled) => {
                    let kept = target.join("..").join("krate-sdk");
                    let _ = fs::remove_dir_all(&kept);
                    copy_tree(bundled, &kept)?;
                    kept
                }
                None => sdk::ensure_materialized()?,
            };
            let resolved = text.replace(
                krate_bundle::SDK_PLACEHOLDER,
                &sdk.to_string_lossy().replace('\\', "/"),
            );
            fs::write(&manifest, resolved)?;
        }
    }
    Ok(Some(target))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_tree(&source, &destination)?;
        } else {
            fs::copy(&source, &destination)?;
        }
    }
    Ok(())
}

/// Change an app that already exists, in place.
///
/// The AI is handed the app's own source and told what to change, which is why
/// this is quicker and more faithful than describing the whole app again.
pub(crate) fn revise_app_for_tui(
    source: &Path,
    change: &str,
    provider: &'static dyn agent_provider::AgentProvider,
    output: &Path,
    attachments: &[PathBuf],
) -> Result<()> {
    // The change is authored in the app's own directory, so an attachment
    // goes in beside the code the same way it does for a new app.
    let mut attached = String::new();
    if !attachments.is_empty() {
        let inbox = source.join("attached");
        let _ = fs::create_dir_all(&inbox);
        let mut named = Vec::new();
        for file in attachments {
            let Some(name) = file.file_name() else { continue };
            if fs::copy(file, inbox.join(name)).is_ok() {
                named.push(format!("attached/{}", name.to_string_lossy()));
            }
        }
        if !named.is_empty() {
            attached.push_str(
                "\n\nThe person attached these files. Read them first -- they say \
                 what the change should look like:\n",
            );
            for name in &named {
                attached.push_str(&format!("  {name}\n"));
            }
        }
    }
    let request = format!(
        "The Krate app in this directory already works. Change it as follows, \
         and change nothing else: {change}{attached}\n\n\
         Keep the same crate name and manifest. When you are done, run \
         `krate check-app .` and make sure every stage passes."
    );
    let code = create_krate(CreateRequest {
        request,
        output: output.to_path_buf(),
        author_cmd: Some(agent_author_command(provider)),
        kind: None,
        name: None,
        transcript: None,
        // Author into the existing source rather than an empty directory --
        // this is the whole difference between changing an app and replacing
        // it.
        work_dir: Some(source.to_path_buf()),
        yes: true,
        no_install: false,
        json: false,
        // The output already exists; that is the point.
        force: true,
    })?;
    if code == 0 {
        remember_app(output);
        Ok(())
    } else {
        Err(anyhow::anyhow!("the change could not be applied"))
    }
}

/// Who published apps are credited to, if anyone is signed in.
pub(crate) fn github_identity() -> Option<String> {
    github_auth::current().map(|identity| identity.display_name().to_string())
}

/// Forget the GitHub sign-in.
pub(crate) fn github_sign_out() -> Result<bool> {
    github_auth::sign_out()
}

/// Connect one named target, without asking again which one.
pub(crate) fn connect_one_for_tui(target: &ClientTarget) -> Result<()> {
    connect(Some(target.key), true, false)?;
    Ok(())
}

/// Bring an AI app to the front so it reloads its config.
///
/// Returns false when there is nothing to open rather than treating it as an
/// error: on Linux, and for a client we cannot name, the honest answer is to
/// print the restart instruction instead.
pub(crate) fn reopen_app(target: &ClientTarget) -> Result<bool> {
    let app = match target.key {
        "claude" => "Claude",
        "cursor" => "Cursor",
        _ => return Ok(false),
    };
    #[cfg(target_os = "macos")]
    {
        // Quit first: an app already running will not re-read its config.
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!("tell application \"{app}\" to quit"))
            .status();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let opened = std::process::Command::new("open")
            .arg("-a")
            .arg(app)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        Ok(opened)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(false)
    }
}

/// Run a bundle for the menu, in a child process.
///
/// Deliberately a child rather than an in-process call. An app that will not
/// close from its own close button leaves Ctrl-C as the only way out, and in
/// one process that interrupt kills the front door too -- the person loses
/// their whole session to close one app. A child takes the interrupt by
/// itself and the menu survives it.
/// Absorb a Ctrl-C while an app is running.
///
/// Doing nothing here is the point: the same signal reaches the app, which
/// exits, and the menu survives to show itself again. A handler that exits
/// would take the menu down with the app; ignoring the signal instead would
/// be inherited by the app, which is why neither worked.
#[cfg(unix)]
extern "C" fn handle_interrupt(_signal: libc::c_int) {}

pub(crate) fn run_bundle_for_tui(bundle: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("could not find Krate's own binary")?;
    // Ctrl-C reaches the whole foreground process group, so it hits the menu
    // as well as the app. Two earlier attempts were both wrong: ignoring the
    // signal here made the child inherit the ignore, so three presses did
    // nothing; setsid would detach the app from the terminal, so the signal
    // would reach neither.
    //
    // The right shape is to let the signal reach both, and simply not die
    // from it. The menu installs a handler that records the interrupt rather
    // than exiting, so the app takes it and quits while the menu carries on.
    #[cfg(unix)]
    let previous = unsafe { libc::signal(libc::SIGINT, handle_interrupt as libc::sighandler_t) };

    let status = std::process::Command::new(exe)
        .arg("run")
        .arg(bundle)
        .arg("--auto-grant")
        .status()
        .context("could not start the app")?;

    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, previous);
    }

    // 130 is the shell's convention for "ended by Ctrl-C", which is a normal
    // way to close an app here rather than a failure worth reporting.
    match status.code() {
        Some(0) | Some(130) | None => Ok(()),
        Some(code) => anyhow::bail!("the app exited with code {code}"),
    }
}

/// Run a bundle in this process, with the same defaults double-clicking uses.
#[allow(dead_code)]
pub(crate) fn run_bundle_inline(bundle: &Path) -> Result<()> {
    // Mirrors what `krate run <bundle>` does with no flags, plus the auto-grant
    // that double-clicking already uses: the person chose this app from a list
    // of apps they made, so a permission prompt per capability is friction
    // without a decision behind it.
    run_component(RunRequest {
        target: bundle.display().to_string(),
        file: PathBuf::new(),
        insecure_http: false,
        fuel: None,
        mem_limit: 256,
        max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
        http_timeout_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
        sandbox_root: PathBuf::from("."),
        manifest_path: None,
        grants: Vec::new(),
        auto_grant: true,
        prompt: false,
        consent: false,
        ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativeWithHeadlessFallback,
        json: false,
        dump_caps: false,
        dump_caps_format: OutputFormat::Text,
        log_grants: None,
        log_grants_format: GrantLogFormat::Text,
        test_time_millis: None,
        test_locale: None,
        test_timezone: None,
        screenshot_path: None,
        screenshot_scale: 2.0,
        usability_report: None,
        app_args: Vec::new(),
    })?;
    Ok(())
}

/// Where the list of made apps is kept.
fn recent_apps_file() -> Option<PathBuf> {
    let home = home_dir()?;
    Some(home.join(".krate").join("recent-apps"))
}

/// Note that an app was made, so "My apps" can list it later.
fn remember_app(bundle: &Path) {
    let Some(file) = recent_apps_file() else {
        return;
    };
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let absolute = bundle
        .canonicalize()
        .unwrap_or_else(|_| bundle.to_path_buf());
    let line = absolute.display().to_string();
    let mut kept: Vec<String> = fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .filter(|existing| *existing != line)
        .map(str::to_string)
        .collect();
    kept.insert(0, line);
    kept.truncate(50);
    let _ = fs::write(&file, kept.join("\n"));
}

/// Apps made on this machine that still exist, newest first.
pub(crate) fn recent_apps() -> Vec<PathBuf> {
    let Some(file) = recent_apps_file() else {
        return Vec::new();
    };
    fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        // An app the person has since deleted or moved should not be offered.
        .filter(|path| path.exists())
        .collect()
}

/// Write a `.krate` bundle from a component and its manifest.
fn pack_bundle(file: &Path, manifest: &Path, output: &Path) -> Result<u8> {
    let assets = manifest
        .parent()
        .map(|parent| parent.join("assets"))
        .filter(|path| path.is_dir());
    // Ship the source next to the wasm when the manifest sits in a crate, so
    // the app can be changed later rather than only run. Identified by
    // Cargo.toml: a manifest elsewhere just packs as before.
    // `--manifest manifest.toml` has an empty parent, which is not the same as
    // having no parent -- without this the source was silently skipped for the
    // most natural way to type the command.
    let source = manifest
        .parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent.to_path_buf()
            }
        })
        .filter(|parent| parent.join("Cargo.toml").is_file());
    // Ship the SDK alongside the source. Source alone does not survive an SDK
    // change -- an app written before a WIT change fails to compile against
    // the current one -- so an app is only genuinely editable later if it
    // carries what it was written against.
    let sdk = source
        .as_deref()
        .and_then(|_| sdk::ensure_materialized().ok())
        .filter(|path| path.is_dir());
    let size = krate_bundle::pack_with_sdk(
        manifest,
        file,
        assets.as_deref(),
        source.as_deref(),
        sdk.as_deref(),
        output,
    )
    .with_context(|| format!("could not pack {}", output.display()))?;
    println!("wrote {} ({size} bytes)", output.display());
    if source.is_some() {
        println!("included the app's source, so it can be changed later");
    }
    if let Some(assets) = assets {
        println!("included portable assets from {}", assets.display());
    }
    Ok(0)
}

/// Default hub used when neither `--hub` nor `KRATE_HUB_URL` is set. A local
/// dev server, so the demo works out of the box once someone runs `krate-hub`.
/// The public hub. KRATE_HUB_URL still overrides it, which is how the local
/// dev server is used -- but the default has to be the real one, or publishing
/// silently uploads to a laptop nobody else can reach.
const DEFAULT_HUB_URL: &str = "https://hub.krate.tech";

/// Upload a `.krate` to a hub and print the URL anyone can `krate run`.
///
/// The hub stores by content hash, so this is idempotent: publishing the same
/// bundle twice hands back the same URL. All the interesting failure modes are
/// "the hub is not reachable" and "that file is not a bundle", and both get a
/// plain message rather than a stack of transport errors.
/// Publish from the front door.
///
/// The menu could build an app and open it but never offered to share it,
/// which is the one thing a `.krate` exists for. This is the same code path as
/// `krate publish`, so there is one publisher rather than two that can drift.
pub(crate) fn publish_bundle_for_tui(bundle: &Path, description: Option<&str>) -> Result<()> {
    let code = publish_bundle(bundle, None, description)?;
    if code == 0 {
        Ok(())
    } else {
        Err(anyhow::anyhow!("the upload did not finish"))
    }
}

fn publish_bundle(
    bundle: &Path,
    hub_override: Option<&str>,
    description: Option<&str>,
) -> Result<u8> {
    // Only upload something that is actually a bundle. Catching it here means a
    // wrong path fails locally with a clear message instead of round-tripping
    // to the hub to be rejected.
    if !krate_bundle::is_bundle_path(bundle) {
        anyhow::bail!(
            "not a .krate bundle: {}\nPack one first with `krate pack` (or `krate create`).",
            bundle.display()
        );
    }
    let bytes =
        fs::read(bundle).with_context(|| format!("could not read bundle {}", bundle.display()))?;
    if bytes.is_empty() {
        anyhow::bail!("bundle is empty: {}", bundle.display());
    }

    let hub = hub_override
        .map(str::to_string)
        .or_else(|| std::env::var("KRATE_HUB_URL").ok())
        .unwrap_or_else(|| DEFAULT_HUB_URL.to_string());
    let endpoint = format!("{}/publish", hub.trim_end_matches('/'));

    // The app already knows its own name; the description is what the person
    // typed when they asked for it, and the author comes from the GitHub
    // sign-in. All three are optional -- publishing still works signed out,
    // it just lands as "anonymous".
    let app_name = krate_bundle::open(bundle)
        .ok()
        .map(|opened| opened.manifest().app.name.clone())
        .unwrap_or_default();
    // The error message told people to run `krate publish` and be asked, so
    // it had better ask. Signing in here rather than failing with advice is
    // the difference between one command and a scavenger hunt.
    let identity = match github_auth::current() {
        Some(identity) => Some(identity),
        None => {
            println!("Publishing puts your name on the app, so it needs a GitHub sign-in.");
            match github_auth::sign_in() {
                Ok(identity) => Some(identity),
                Err(err) => {
                    anyhow::bail!("could not sign in: {err}");
                }
            }
        }
    };
    let mut request = ureq::post(&endpoint).set("Content-Type", "application/octet-stream");
    if !app_name.is_empty() {
        request = request.set("X-Krate-Name", &app_name);
    }
    if let Some(description) = description {
        request = request.set("X-Krate-Description", description);
    }
    if let Some(identity) = &identity {
        // The hub verifies this against GitHub rather than trusting a name in
        // a header, so the author shown on an app's page is a real account.
        request = request.set("Authorization", &format!("Bearer {}", identity.token));
    }

    let response = match request.send_bytes(&bytes) {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let detail = response
                .into_string()
                .unwrap_or_else(|_| "(no detail)".to_string());
            anyhow::bail!(
                "the hub rejected the bundle (HTTP {code}): {}",
                detail.trim()
            );
        }
        Err(ureq::Error::Transport(err)) => {
            anyhow::bail!(
                "could not reach the hub at {hub}: {err}\n\
                 Is a hub running? Start one locally with `krate-hub`, or point \
                 `KRATE_HUB_URL` at a reachable one.",
            );
        }
    };

    let body = response
        .into_string()
        .context("could not read the hub's response")?;
    // The response is a small JSON object { "url", "id" }. Rather than pull in a
    // parser for two fields, pluck the url out; if the shape is unexpected, show
    // the raw body so it is still debuggable.
    let url = extract_json_string(&body, "url")
        .ok_or_else(|| anyhow::anyhow!("the hub returned an unexpected response: {body}"))?;

    usage::record(usage::Action::Publish);
    println!("Published. Anyone can run it with:");
    println!("  krate run {url}");
    Ok(0)
}

/// Pull one string field out of a flat JSON object like `{"url":"...","id":"..."}`.
///
/// Deliberately tiny: the hub's response has exactly two string fields and no
/// nesting, so a full JSON dependency here would be more moving parts than the
/// job needs.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let colon = rest.find(':')?;
    let after_colon = &rest[colon + 1..];
    let open = after_colon.find('"')? + 1;
    let value = &after_colon[open..];
    let close = value.find('"')?;
    Some(value[..close].to_string())
}

struct CreateRequest {
    request: String,
    output: PathBuf,
    author_cmd: Option<String>,
    kind: Option<CreateKind>,
    name: Option<String>,
    transcript: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    yes: bool,
    no_install: bool,
    json: bool,
    force: bool,
}

/// Fewest characters a create request must have to be worth authoring from.
const MIN_CREATE_REQUEST_CHARS: usize = 3;

/// Longest kebab-case app name derived from a request. Long enough for
/// "reading-list" or "packing-list", short enough to stay a readable folder.
const MAX_DERIVED_NAME_WORDS: usize = 3;

/// Derive a kebab-case app name from the subject of a plain-English request,
/// e.g. "A reading list app to track books" -> `reading-list`.
///
/// The name becomes the window title and the data folder, and the folder is
/// what the permission wall shows, so it should say what the app is. Returns
/// `None` when the request has no subject worth naming, leaving the caller's
/// default in place rather than inventing something worse.
/// Whether a name can survive becoming a WIT package label and a crate name.
fn validate_app_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("it is empty".to_string());
    }
    for word in name.split('-') {
        if word.is_empty() {
            return Err("it has an empty word between dashes".to_string());
        }
        if !word.starts_with(|c: char| c.is_ascii_lowercase()) {
            return Err(format!(
                "the word `{word}` does not start with a lowercase letter"
            ));
        }
        if !word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(format!(
                "the word `{word}` contains something other than lowercase letters and digits"
            ));
        }
    }
    Ok(())
}

fn name_from_request(request: &str) -> Option<String> {
    // Words that describe the act of asking, not the thing being asked for.
    const SKIP: &[&str] = &[
        "a",
        "an",
        "the",
        "make",
        "build",
        "create",
        "write",
        "me",
        "my",
        "app",
        "application",
        "simple",
        "small",
        "basic",
        "little",
        "some",
        "please",
        "that",
        "which",
        "for",
        "to",
        "with",
        "and",
        "of",
        "called",
        "named",
        "new",
        "i",
        "want",
    ];

    let mut words: Vec<String> = Vec::new();
    for raw in request.split_whitespace() {
        let word: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        if word.is_empty() {
            continue;
        }
        // A word that starts with a digit cannot appear in the name: it flows
        // into the WIT package label, whose dash-separated words must begin
        // with a lowercase letter. "pomodoro timer: 25 minute sessions" was
        // slugged to `pomodoro-timer-25`, and the build died on "invalid
        // label" long after the person's request looked accepted. A number is
        // detail, not subject -- it ends the name the way a stop word does.
        if !word.starts_with(|c: char| c.is_ascii_lowercase()) {
            if words.is_empty() {
                continue;
            }
            break;
        }
        // Leading filler is dropped, but once the subject starts, a stop word
        // means the subject ended: "reading list app to track books" stops at
        // "to" rather than running on into the explanation.
        if SKIP.contains(&word.as_str()) {
            if words.is_empty() {
                continue;
            }
            break;
        }
        words.push(word);
        if words.len() == MAX_DERIVED_NAME_WORDS {
            break;
        }
    }

    if words.is_empty() {
        return None;
    }
    Some(words.join("-"))
}

/// Check that a create request has enough to author from. Returns a plain,
/// user-facing message on failure.
fn validate_create_request(request: &str) -> std::result::Result<(), String> {
    let trimmed = request.trim();
    if trimmed.is_empty() {
        return Err("please say what the app should do, for example: \
                    krate create \"a checklist\" --output checklist.krate"
            .to_string());
    }
    if trimmed.chars().count() < MIN_CREATE_REQUEST_CHARS {
        return Err(format!(
            "that request is too short to build an app from; \
             please describe what you want in a few words (at least {MIN_CREATE_REQUEST_CHARS} characters)"
        ));
    }
    Ok(())
}

/// Turn a refusal into what the person (or the calling model) sees, and the
/// error that stops `create`.
///
/// A refusal is a good outcome, not a crash: it arrives in a second instead of
/// five minutes, and it saves someone from being handed an app that cannot do
/// the thing they asked for. So it reads as an answer -- the reason, the
/// nearest buildable thing, and the way to override -- rather than as a fault.
fn report_refusal(
    req: &CreateRequest,
    refusal: &krate_author::feasibility::Refusal,
) -> anyhow::Error {
    if req.json {
        // Agents and the MCP server read this. `ok: false` with a distinct
        // error keeps a refusal from being mistaken for a build failure, which
        // is the difference between "ask for something else" and "retry".
        let report = serde_json::json!({
            "schema": "krate.author.v1",
            "ok": false,
            "error": "cannot-build",
            "request": req.request,
            "limit": refusal.limit,
            "reason": refusal.reason,
            "instead": refusal.instead,
            "message": format!("{}. Try instead: {}.", refusal.reason, refusal.instead),
        });
        if let Ok(line) = serde_json::to_string(&report) {
            println!("{line}");
        }
    }
    anyhow::anyhow!(
        "Krate cannot build that: {}.\n\n\
         Try instead: {}.\n\n\
         Stopped before writing any code, so nothing was spent on an app that could \
         not have worked. If you think this is wrong, re-run with --force and Krate \
         will build what it can.",
        refusal.reason,
        refusal.instead
    )
}

/// Author a small app from a request and package it as one shareable `.krate`.
///
/// The steps mirror the authoring loop: generate the source (built-in template
/// or an agent command), build it to a component, check it imports only Krate
/// APIs, pack it, and verify its permission wall by running the packed bundle
/// with and without its gating capability. A transcript records every step.
fn create_krate(req: CreateRequest) -> Result<u8> {
    use krate_author::{generate, AppKind, AppRequest};

    // Reject an empty or too-short request before doing any work: authoring
    // needs something to go on, and a blank request otherwise burns a full
    // toolchain probe and build on nothing. In --json mode report it as data.
    if let Err(message) = validate_create_request(&req.request) {
        if req.json {
            let report = serde_json::json!({
                "schema": "krate.author.v1",
                "ok": false,
                "error": "empty-request",
                "message": message,
            });
            println!("{}", serde_json::to_string(&report)?);
        }
        anyhow::bail!("{message}");
    }

    // Screen the request against what Krate can actually do, before spending
    // three to five minutes and an AI budget. Nothing downstream compares the
    // finished app to the request -- all six check-app stages are mechanical --
    // so "download my email" would otherwise produce a mail-reader UI over
    // invented data that builds, runs, exits 0, and is reported as ready.
    //
    // The screen refuses only what is certainly impossible and says so in one
    // sentence; anything it is unsure about is built. A caveat is not a
    // refusal: the app is authored and the note is printed with it.
    let feasibility = krate_author::feasibility::screen(&req.request);
    if let krate_author::feasibility::Verdict::Refuse(refusal) = &feasibility {
        if !req.force {
            return Err(report_refusal(&req, refusal));
        }
    }

    // Building the app needs a Rust toolchain, cargo-component, and the wasm
    // target. Check for them before anything else and, when a terminal is
    // present, offer to install what is missing — so a first run fails with a
    // clear next step instead of a raw cargo error mid-build. In --json mode
    // (agents/scripts) never prompt or install: report the gap as data.
    if req.json {
        preflight_toolchain_report_json(&req.output)?;
    } else {
        preflight_toolchain(req.yes, req.no_install)?;
    }

    // Where the Krate SDK and WIT live, so a generated crate can build against
    // them. By default the binary materializes the SDK it carries embedded, so
    // no checkout is needed. KRATE_SDK_ROOT overrides that with a checkout, for
    // development against local WIT changes.
    let sdk_root = match std::env::var_os("KRATE_SDK_ROOT") {
        Some(root) => PathBuf::from(root),
        None => sdk::ensure_materialized().context("prepare the embedded Krate SDK")?,
    };

    // Decide the app kind: an explicit --kind wins, otherwise infer from the
    // request text (e.g. "checklist" -> the GUI checklist).
    let kind = match req.kind {
        Some(CreateKind::Checklist) => AppKind::Checklist,
        Some(CreateKind::WordFrequency) => AppKind::WordFrequency,
        Some(CreateKind::VoicePrompter) => AppKind::VoicePrompter,
        None => AppKind::infer(&req.request),
    };
    // When the built-in maker (no agent) is about to generate an app the request
    // did not actually name, say so. The built-in path has three templates and
    // cannot write arbitrary apps; a request like "a pdf merger" silently became
    // a checklist wearing that name, which reads as the AI being broken rather
    // than unbuilt. Only warn when it truly fell back: an explicit --kind is a
    // choice, and an --agent run does not touch these templates at all.
    let fell_back_to_default = req.kind.is_none()
        && req.author_cmd.is_none()
        && AppKind::infer_matched(&req.request).is_none();
    if fell_back_to_default && !req.json {
        eprintln!(
            "note: \"{}\" did not match a built-in template, so Krate is starting from \
             its checklist starter and naming it after your request. The built-in maker \
             has three templates (checklist, word-count, voice-prompter) and does not \
             write arbitrary apps. To have an AI actually write this app, re-run with \
             `--agent claude`. To pick a template on purpose, pass `--kind`.",
            req.request
        );
    }
    let default_name = match kind {
        AppKind::Checklist => "checklist",
        AppKind::WordFrequency => "word-count",
        AppKind::VoicePrompter => "voice-prompter",
    };
    // Prefer a name taken from the request itself. The app's name becomes its
    // window title and its data folder, and the data folder is what the
    // permission wall shows the person deciding whether to allow it -- so a
    // reading list that asks to "save files in checklist" reads as though it
    // came from somewhere else. Fall back to the kind's name when the request
    // has no usable subject of its own.
    let name = req
        .name
        .clone()
        .or_else(|| name_from_request(&req.request))
        .unwrap_or_else(|| default_name.to_string());
    // The name flows into the WIT package label and the crate name, both of
    // which reject anything but dash-separated words that begin with a
    // lowercase letter. `--name 2048` used to sail past this point and die
    // mid-build with "failed to load cargo metadata", which names neither the
    // rule nor the flag. Refuse it here, where the message can.
    if let Err(reason) = validate_app_name(&name) {
        anyhow::bail!(
            "the app name `{name}` cannot be used: {reason}. Use dash-separated \
             words that each start with a lowercase letter, like `tile-game`."
        );
    }

    // The app is built inside a work dir. A temp dir is cleaned up; --work-dir
    // keeps it for inspection.
    let held_temp;
    let app_dir = match &req.work_dir {
        Some(dir) => {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            dir.join(&name)
        }
        None => {
            let temp = tempfile::tempdir().context("create work dir")?;
            let path = temp.path().join(&name);
            held_temp = temp;
            let _ = &held_temp;
            path
        }
    };
    let _ = fs::remove_dir_all(&app_dir);
    fs::create_dir_all(&app_dir).with_context(|| format!("create {}", app_dir.display()))?;

    let mut steps: Vec<serde_json::Value> = Vec::new();

    // Step 1: author. Either an agent command writes the source, or the
    // built-in generator does.
    if !req.json {
        println!("==> authoring \"{}\"", req.request);
    }
    let author_note = if let Some(cmd) = &req.author_cmd {
        let sdk_prefix = relative_sdk_prefix(&app_dir, &sdk_root)?;
        run_author_command(AuthorContext {
            cmd,
            app_dir: &app_dir,
            name: &name,
            request: &req.request,
            sdk_dir: &sdk_root,
            sdk_prefix: &sdk_prefix,
            kind,
        })?;
        format!("external agent command: {cmd}")
    } else {
        let sdk_prefix = relative_sdk_prefix(&app_dir, &sdk_root)?;
        let mut request = match kind {
            AppKind::Checklist => AppRequest::checklist(&name),
            AppKind::WordFrequency => AppRequest::word_frequency(&name),
            AppKind::VoicePrompter => AppRequest::voice_prompter(&name),
        };
        request.description = req.request.clone();
        let app = generate(&request, &sdk_prefix).map_err(|err| anyhow::anyhow!(err))?;
        for file in &app.files {
            let dest = app_dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, &file.contents)?;
        }
        "built-in generator".to_string()
    };
    steps.push(serde_json::json!({"step": "author", "detail": author_note}));

    if matches!(kind, AppKind::VoicePrompter) {
        let model = speech_model::provision(&app_dir, req.json)
            .context("prepare the self-contained local speech model")?;
        steps.push(serde_json::json!({
            "step": "asset",
            "detail": format!("verified local speech model: {}", model.display()),
        }));
    }

    // Step 2: build to a wasm component.
    if !req.json {
        println!("==> building the component");
    }
    let wasm = build_component(&app_dir)?;
    steps.push(serde_json::json!({"step": "build", "detail": "cargo-component build --release"}));

    // Step 3: the component must import only krate:*.
    let wasm_bytes = fs::read(&wasm).with_context(|| format!("read {}", wasm.display()))?;
    let bad = krate_bundle::imports::non_krate_imports(&wasm_bytes)
        .map_err(|err| anyhow::anyhow!(err))?;
    if !bad.is_empty() {
        anyhow::bail!(
            "the generated app imports non-Krate host APIs, so it cannot run under Krate: {}",
            bad.join(", ")
        );
    }
    steps.push(serde_json::json!({"step": "check-imports", "detail": "krate:* imports only"}));

    // Step 4: pack. Copy the manifest and point its entry at code.wasm.
    if !req.json {
        println!("==> packing {}", req.output.display());
    }
    let manifest_src = app_dir.join("manifest.toml");
    let manifest = krate_manifest::Manifest::parse_file(&manifest_src)
        .with_context(|| format!("read {}", manifest_src.display()))?;
    let pack_dir = tempfile::tempdir().context("pack dir")?;
    let code = pack_dir.path().join("code.wasm");
    fs::copy(&wasm, &code)?;
    let packed_manifest = pack_dir.path().join("manifest.toml");
    write_manifest_with_entry(&manifest_src, &packed_manifest, "code.wasm")?;
    let assets = app_dir.join("assets");
    // Ship the source. Every app an AI writes is a first draft, and the
    // person who asked for it will want it changed -- without this the
    // bundle is a dead end and "make a change" has nothing to work from.
    // This was the one pack call that still dropped it.
    let size = krate_bundle::pack_with_source(
        &packed_manifest,
        &code,
        assets.is_dir().then_some(assets.as_path()),
        app_dir
            .join("Cargo.toml")
            .is_file()
            .then_some(app_dir.as_path()),
        &req.output,
    )
    .with_context(|| format!("pack {}", req.output.display()))?;
    steps.push(serde_json::json!({"step": "pack", "detail": format!("{} bytes", size)}));

    // Step 5: verify the permission wall by running the packed bundle with all
    // grants (must succeed) and without the gating capability (must refuse).
    if !req.json {
        println!("==> verifying the permission wall");
    }
    let gating = gating_capability(&manifest);
    let verify_dir = tempfile::tempdir().context("verify dir")?;
    // The verify arg is a seeded fixture path for read-gated apps (the
    // word-frequency kind needs a real file to read), else a plain task word
    // for the checklist kind.
    let verify_arg =
        prepare_verify_dir(verify_dir.path(), &manifest)?.unwrap_or_else(|| "quick".to_string());
    // Resolve the bundle to an absolute path robustly. `fs::canonicalize` on a
    // relative output path depends on the process's current directory, which can
    // be stale or deleted (e.g. a shell that `rm -rf`'d the directory it was
    // standing in) — that made the verify run against a wrong or missing path
    // and hang or fail intermittently. Canonicalize the file's parent (which
    // exists) and rejoin the file name, and fall back to a manual absolute join
    // if the current directory itself is unreadable.
    let bundle_abs = absolute_output_path(&req.output)?;

    // Run the freshly authored app as untrusted during verification: it gets a
    // finite fuel budget, so a generated runaway or infinite loop fails here
    // (limit-exceeded) instead of hanging create.
    let allow_exit = run_self(
        verify_dir.path(),
        &[
            "run",
            bundle_abs.to_str().unwrap(),
            "--untrusted",
            "--auto-grant",
            // Headless: verification is an automated check that the app runs
            // and honors its permission wall, not a play session. A GUI app run
            // windowed here tries to open a real window in a non-interactive
            // context and traps -- which failed `krate create` at the last step
            // for a checklist that in fact runs perfectly. Headless runs the
            // same code without a window.
            "--headless",
            "--",
            &verify_arg,
        ],
    )?;
    if allow_exit != 0 {
        anyhow::bail!(
            "the packed app failed to run with all grants (exit {allow_exit}); \
             exit 4 means it exhausted its fuel budget (a runaway or infinite loop)"
        );
    }

    // An app that asks only for the defaults and its own window has no
    // capability whose absence would stop it, so there is nothing to withhold.
    // Record that honestly instead of testing against a capability it never
    // requested -- which is what made a ported GUI app fail after building,
    // packing, and passing its import check.
    if let Some(gating) = gating.as_deref() {
        let mut deny_args = vec!["run".to_string(), bundle_abs.to_string_lossy().into_owned()];
        for cap in manifest.capabilities.iter() {
            let name = cap.cap.clone();
            if name == gating {
                continue;
            }
            deny_args.push("--grant".to_string());
            deny_args.push(name);
        }
        // Headless for the same reason as the allow run: this is an automated
        // permission-wall check, not a windowed session.
        deny_args.push("--headless".to_string());
        deny_args.push("--".to_string());
        deny_args.push(verify_arg.clone());
        let deny_arg_refs: Vec<&str> = deny_args.iter().map(String::as_str).collect();
        let deny_exit = run_self(verify_dir.path(), &deny_arg_refs)?;
        if deny_exit != 5 {
            anyhow::bail!("withholding {gating} should refuse with exit 5, got {deny_exit}");
        }
        steps.push(serde_json::json!({
            "step": "verify",
            "detail": format!("runs with all grants (exit 0), refuses without {gating} (exit 5)")
        }));
    } else {
        steps.push(serde_json::json!({
            "step": "verify",
            "detail": "runs with all grants (exit 0); asks only for defaults and its own window, so there is no capability to withhold"
        }));
    }

    // The transcript: request, app, requested permissions, verification.
    let requested: Vec<String> = manifest
        .capabilities
        .iter()
        .map(|cap| cap.cap.clone())
        .collect();
    // What the app honestly does. When the request asked for something the app
    // can only partly serve -- live data with no host named being the usual
    // case -- say so here, with the finished app, rather than letting the
    // person find out when the numbers never change.
    let caveat = match &feasibility {
        krate_author::feasibility::Verdict::Caveat(c) => Some(c.note.clone()),
        _ => None,
    };
    let transcript = serde_json::json!({
        "schema": "krate.author.v1",
        "request": req.request,
        "caveat": caveat,
        "app": {"name": name, "kind": format!("{kind:?}")},
        "requested_permissions": requested,
        "gating_permission": gating,
        "output": req.output.to_string_lossy(),
        "krate_bytes": size,
        "steps": steps,
        "verdict": "authored a working, permission-gated .krate: runs with its grants, refuses without the gating one",
    });
    // The transcript sidecar is opt-in: written only when --transcript names a
    // path, so a normal user's folder is not littered with a JSON file they did
    // not ask for. --json emits the transcript on stdout instead.
    let sidecar = req.transcript.clone();
    if let Some(path) = &sidecar {
        fs::write(path, serde_json::to_string_pretty(&transcript)? + "\n")?;
    }

    if req.json {
        // One machine-readable object on stdout, and nothing else.
        let mut out = transcript;
        out["ok"] = serde_json::Value::Bool(true);
        println!("{}", serde_json::to_string(&out)?);
        return Ok(0);
    }

    println!();
    println!("Created {}", req.output.display());
    if let Some(note) = &caveat {
        // Printed with the success, not instead of it: the app was built and
        // works, and this is the part of the request it could not make real.
        println!();
        println!("  One thing to know: {note}.");
    }
    if let Some(path) = &sidecar {
        println!("  transcript: {}", path.display());
    }
    println!("  requested access:");
    for cap in &requested {
        println!("    - {cap}");
    }
    println!();
    println!(
        "Send {} to someone; they can double-click it to open it.",
        req.output.display()
    );
    Ok(0)
}

/// The relative path from the app dir to the SDK root, for the generated
/// Cargo.toml's path dependencies.
fn relative_sdk_prefix(app_dir: &Path, sdk_root: &Path) -> Result<String> {
    let app_abs = if app_dir.is_absolute() {
        app_dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(app_dir)
    };
    let sdk_abs = fs::canonicalize(sdk_root)?;
    // Count how many directories deep the app dir is, then walk up to the SDK.
    // The generated crate is standalone (empty [workspace]), so a relative
    // prefix that resolves at build time is all that is needed.
    let common = fs::canonicalize(&app_abs).unwrap_or(app_abs);
    let depth = common.components().count();
    let sdk_depth = sdk_abs.components().count();
    // Simple case used everywhere in practice: the app dir is under the SDK
    // root, so the prefix is "../" repeated by how much deeper it sits.
    if common.starts_with(&sdk_abs) {
        let up = depth - sdk_depth;
        return Ok(vec![".."; up].join("/"));
    }
    // Otherwise use an absolute path to the SDK — always correct, less tidy.
    // Normalize it for use as a Cargo.toml path dependency: forward slashes
    // (TOML needs no escaping then, and cargo accepts them on Windows) and no
    // `\\?\` UNC verbatim prefix, which cargo cannot resolve in a manifest.
    Ok(toml_path(&sdk_abs))
}

/// A path string safe to drop into a `Cargo.toml` `path = "..."` on any OS:
/// forward slashes, and with Windows' `\\?\` verbatim prefix stripped.
fn toml_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = s
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or_else(|| s.into_owned());
    s.replace('\\', "/")
}

/// Run an agent's author command with the app context in the environment.
/// Everything an agent command is given to author an app.
struct AuthorContext<'a> {
    cmd: &'a str,
    app_dir: &'a Path,
    name: &'a str,
    request: &'a str,
    sdk_dir: &'a Path,
    sdk_prefix: &'a str,
    kind: krate_author::AppKind,
}

/// Drive a supported AI agent to author the app (the `--agent` path). Reads the
/// request and app dir from the environment `create` set, builds the agent
/// prompt here -- versioned with the tool, not in an external script -- and runs
/// the provider headless.
fn run_author_agent(agent: &str) -> Result<u8> {
    let app_dir = std::env::var("KRATE_APP_DIR")
        .context("KRATE_APP_DIR is not set; run this through `krate create --agent`")?;
    let request =
        std::env::var("KRATE_REQUEST").unwrap_or_else(|_| "a small useful app".to_string());

    run_provider_author(resolve_agent(agent)?, &app_dir, &request)
}

/// The prompt handed to Claude Code: the loop instruction.
///
/// The agent is no longer asked to adapt a behavioral template. It is given a
/// minimal compiling skeleton, the full context pack, and the one tool that
/// changes everything -- it can run `krate check-app .`, see exactly what is
/// wrong, and fix it. So the prompt is short: build the app, and do not stop
/// until the oracle says OK. Everything the agent needs to know is in the pack,
/// which is generated from real sources, not restated here where it could drift.
fn claude_author_prompt(app_dir: &str, request: &str, krate_bin: &str) -> String {
    format!(
        "You are building a Krate desktop app in Rust from this request:\n\
\n\
    {request}\n\
\n\
Work in {app_dir}. A minimal compiling skeleton is already there (Cargo.toml,\n\
src/lib.rs, manifest.toml): it opens a window (or prints a line, for a CLI app),\n\
builds cleanly, and imports only krate:* -- but it does nothing yet. Your job is\n\
to make it the app the request describes.\n\
\n\
How to work:\n\
1. Read KRATE_AUTHORING.md in this directory first. It lists every function you\n\
   can call, every capability a manifest may declare, the no_std rules, the GUI\n\
   interfaces, and an index of the shipped example apps. It is generated from\n\
   the real SDK, so everything in it is accurate.\n\
2. Find the closest example in section 5 and read that app's src/lib.rs and\n\
   manifest.toml under the apps/ directory. Adapt its proven, working code --\n\
   do not write the no_std/krate:* discipline from a blank page.\n\
3. Write the app: edit src/lib.rs, and set manifest.toml to exactly the\n\
   capabilities the app uses.\n\
4. After every change, run exactly this from {app_dir}:\n\
\n\
       {krate_bin} check-app .\n\
\n\
   It builds the app, checks it imports only krate:*, and runs it once. On\n\
   failure it names the stage and the exact fix -- including how to remove a\n\
   leaked wasi:* import. Do whatever it says, then run it again.\n\
\n\
Use `{krate_bin} check-app .` to build -- do NOT run `cargo build` or\n\
`cargo component build` yourself. check-app builds with the correct rustup\n\
toolchain and the wasm target; a bare `cargo` on this machine is often the wrong\n\
one and fails with \"can't find crate for core\". check-app is the only build\n\
command you need.\n\
\n\
Do not stop until `check-app` prints OK. That is the whole definition of done:\n\
not \"looks right\", not \"should work\" -- the oracle prints OK, having actually\n\
run. If a command seems blocked, try it again; you have permission to run it.\n\
Use the Read, Edit, Write, and Bash tools. Do not explain what you did; just\n\
build the app until the check passes.\n\
\n\
One exception, and only one. If the request needs something no Krate app can\n\
do -- reading the mail, photos, contacts or messages already on this computer,\n\
signing in to somebody's Google/Spotify/Twitter account, reaching another\n\
person's device, or running while the app is closed -- then do NOT build a\n\
convincing app over invented data and let check-app pass it. An app that looks\n\
like a mail reader but can never read mail is worse than no app. Write the\n\
single line\n\
\n\
    KRATE-CANNOT-BUILD: <one plain sentence saying what is out of reach>\n\
\n\
to a file named CANNOT-BUILD.txt in {app_dir}, and stop. Krate will show that\n\
sentence to the person.\n\
\n\
Use this only when you are certain. If the request merely sounds ambitious, or\n\
could be read as wanting a local, offline, or example-data version of the\n\
thing, build that version -- and make the app itself honest about it on screen\n\
(a label saying the data is built-in sample data, say). Building something\n\
useful is almost always the right answer; refusing something buildable is not."
    )
}

/// The file name an agent writes to say the request is out of reach.
const AGENT_REFUSAL_FILE: &str = "CANNOT-BUILD.txt";

/// Read an agent's refusal, if it left one. Returns the one plain sentence it
/// gave, with the marker stripped.
///
/// This is the second half of the refusal path. The pre-screen in
/// `feasibility` stops the phrasings we listed; this catches the ones we did
/// not think of, judged by the one participant that has read both the request
/// and the whole API reference. Deliberately forgiving about the exact shape --
/// the marker anywhere in the file is enough -- because a refusal that fails to
/// parse would silently become a wrong app, which is the outcome this whole
/// path exists to prevent.
fn agent_refusal(app_dir: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(app_dir).join(AGENT_REFUSAL_FILE)).ok()?;
    let reason = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("KRATE-CANNOT-BUILD:"))
        .map(str::trim)
        .unwrap_or_else(|| text.trim());
    if reason.is_empty() {
        // The file exists but says nothing. Still a refusal -- treat the empty
        // case as one rather than building the app it was warning about.
        return Some(
            "the AI judged this request to be outside what a Krate app can do".to_string(),
        );
    }
    Some(reason.to_string())
}

/// Run one provider through Krate's authoring policy.
///
/// Everything here is the same for every provider: the prompt, the transcript,
/// the skeleton snapshot that catches an agent which answered in chat without
/// writing code, the progress reporting, the timeout, and the `check-app`
/// verdict. Only the argument list, the spawn setup, and the progress parsing
/// come from the provider -- which is exactly the split the trait draws.
fn run_provider_author(
    provider: &'static dyn agent_provider::AgentProvider,
    app_dir: &str,
    request: &str,
) -> Result<u8> {
    // The agent runs `krate check-app .` itself, so it needs this binary on a
    // known path. current_exe is the running krate; hand its absolute path to
    // the prompt so the agent's Bash calls resolve it regardless of PATH.
    let krate_bin = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "krate".to_string());
    let prompt = claude_author_prompt(app_dir, request, &krate_bin);
    let transcript = Path::new(app_dir).join(".agent-transcript.txt");
    // A snapshot of the skeleton, to detect an agent that answered in chat and
    // never wrote code -- that would leave the blank skeleton, which builds and
    // passes check-app but is not the requested app.
    let starter_lib = fs::read_to_string(Path::new(app_dir).join("src/lib.rs")).unwrap_or_default();
    let file = fs::File::create(&transcript).ok();

    let mut command = ProcessCommand::new(provider.program());
    command.args(provider.author_args(&prompt));
    // Provider-specific spawn setup: closing stdin so a headless run never
    // blocks on input, plus anything else that provider needs.
    provider.configure(&mut command);
    // stdout is piped, not sent straight to the transcript: the reporter reads
    // the streamed events to show live progress and writes every line it reads
    // to the transcript, so the file ends up with the same content it always
    // had. stderr still goes straight to the transcript.
    command.stdout(std::process::Stdio::piped());
    if let Some(file) = &file {
        if let Ok(clone) = file.try_clone() {
            command.stderr(std::process::Stdio::from(clone));
        }
    }

    // Authoring iterates now (write, check, fix) and prints nothing to the user
    // (the model's output goes to the transcript), so reassure them, show a
    // heartbeat so it never looks dead, and bound the wait: a stuck agent fails
    // with the app's own check-app verdict, never an endless silent hang.
    let timeout_secs = std::env::var("KRATE_AUTHOR_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(AGENT_AUTHOR_TIMEOUT_SECS);
    eprintln!(
        "    the AI is writing your app and checking it as it goes -- this can take \
         several minutes"
    );
    let mut child = command.spawn().with_context(|| {
        // Availability was checked before authoring began, so reaching here
        // means something else went wrong -- but still name the provider and the
        // sign-in step rather than leaving a bare OS error.
        format!(
            "run the `{program}` CLI (is {name} installed and signed in?)",
            program = provider.program(),
            name = provider.name(),
        )
    })?;

    // Read the agent's streamed events on a worker thread and turn each one
    // into a plain-English progress line. The thread owns the pipe and appends
    // every raw line to the transcript, so the transcript is unchanged while
    // the person watching gets to see real work instead of dots.
    let stdout = child.stdout.take();
    let transcript_for_thread = transcript.clone();
    let reporter = stdout.map(|stdout| {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut log = fs::OpenOptions::new()
                .append(true)
                .open(&transcript_for_thread)
                .ok();
            let mut steps = 0usize;
            let mut last = String::new();
            for line in io::BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(log) = log.as_mut() {
                    let _ = writeln!(log, "{line}");
                }
                if let Some(step) = provider.progress_line(&line) {
                    // Collapse repeats: an agent editing one file five times
                    // should not print the same sentence five times.
                    //
                    // But send it onward anyway when a display is drawing. The
                    // display shows one live line, not a list, and it uses
                    // each report as proof the agent is still working. Skipping
                    // repeats here is what let it sit on one sentence for ten
                    // minutes while the agent was reading steadily -- the run
                    // was healthy and looked dead.
                    if step == last {
                        report_progress_alive(&step);
                        continue;
                    }
                    steps += 1;
                    // Three cases, and the middle one used to be missed.
                    //
                    // In-process: hand it to the display directly.
                    //
                    // In the child of a front-door run: the display lives in
                    // the parent, so `report_progress` finds nothing here. Emit
                    // a tagged line the parent parses instead of printing --
                    // printing is what made the display and cargo's output
                    // fight over the same terminal, leaving the first stage
                    // frozen for five minutes while the app really was
                    // compiling.
                    //
                    // Plain CLI run with no display anywhere: print it.
                    if !report_progress(&step) {
                        if std::env::var_os(PROGRESS_CHANNEL).is_some() {
                            println!("{PROGRESS_PREFIX}{step}");
                            let _ = io::stdout().flush();
                        } else {
                            eprintln!("    {steps:>2}. {step}");
                            let _ = io::stderr().flush();
                        }
                    }
                    last = step;
                }
            }
        })
    });

    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Report what actually blocked it, not a generic stall: run
                    // the same oracle the agent was running and surface its last
                    // verdict.
                    return Err(author_stalled_error(app_dir, &transcript, timeout_secs));
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("waiting for the `{}` agent", provider.name()))
            }
        }
    };
    if let Some(reporter) = reporter {
        let _ = reporter.join();
    }
    // The agent's own refusal, checked before the exit status: the pre-screen
    // catches the phrasings we know, and the agent is the one that can judge a
    // request nobody anticipated. It only gets here after reading the API
    // reference, so when it says the request is out of reach it has a better
    // basis for that than a phrase match does. Read this first because a
    // refusing agent may well exit non-zero, and a refusal is a clearer answer
    // than "the agent failed".
    if let Some(reason) = agent_refusal(app_dir) {
        anyhow::bail!(
            "Krate cannot build that: {reason}\n\n\
             The AI read the request and Krate's full API reference and stopped rather \
             than build an app that looks right but cannot do what you asked. If you \
             think it is wrong, re-run with --force."
        );
    }
    if provider.failed(&status) {
        // Surface the agent's own error rather than pointing at a file. A
        // failure here is usually about the person's AI account, not their
        // app -- an expired login, a model their plan cannot reach, a CLI
        // that needs updating -- and "see this transcript" makes them go
        // digging through JSON to find one sentence they could have been
        // told directly.
        let reason = agent_failure_reason(&transcript);
        match reason {
            Some(reason) => anyhow::bail!(
                "{} could not write the app:\n\n  {reason}\n\n\
                 This is a problem with the AI tool, not with Krate or your \
                 request. Check that `{}` runs on its own, then try again. \
                 The full transcript is at {}.",
                provider.name(),
                provider.program(),
                transcript.display()
            ),
            None => anyhow::bail!(
                "the {} agent did not finish successfully; see {}",
                provider.name(),
                transcript.display()
            ),
        }
    }
    let lib_after = fs::read_to_string(Path::new(app_dir).join("src/lib.rs")).unwrap_or_default();
    if lib_after == starter_lib {
        anyhow::bail!(
            "the agent finished without changing the app: src/lib.rs is byte-identical \
             to the blank skeleton, so this would package an empty app as if it were \
             \"{request}\". The agent's transcript is at {} -- it usually means the \
             agent explained the app instead of writing it.",
            transcript.display()
        );
    }
    // Put the `krate` dependency back if the agent deleted it. Measured on five
    // authored apps in a row: converting to `#![no_std]` makes an agent reason
    // that a no_std crate should not depend on anything and drop the line, and
    // the app then cannot build at all.
    restore_krate_dependency(Path::new(app_dir));

    // The agent claims it is done. Confirm with the same oracle it was told to
    // satisfy: if check-app does not pass, say exactly why rather than letting
    // the downstream create pipeline fail with a less specific message.
    if let Err(failure) = check_app_verdict(app_dir) {
        anyhow::bail!(
            "the agent finished, but `check-app` does not pass yet:\n\n{failure}\n\n\
             The agent's transcript is at {}. Running the command again often gets it \
             the rest of the way.",
            transcript.display()
        );
    }
    Ok(0)
}

/// Pull the agent's own error sentence out of its transcript.
///
/// Providers stream JSON lines, and a failure is usually one clear sentence
/// buried among hundreds of events -- often nested as a JSON string inside a
/// JSON field. Best-effort by design: an unrecognized shape returns None and
/// the caller falls back to naming the transcript.
fn agent_failure_reason(transcript: &Path) -> Option<String> {
    let text = fs::read_to_string(transcript).ok()?;
    let mut last = None;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Look wherever providers put a message, then unwrap one layer of
        // JSON-in-a-string, which is how the useful sentence usually arrives.
        let raw = event
            .pointer("/error/message")
            .or_else(|| event.pointer("/item/message"))
            .or_else(|| event.get("message"))
            .and_then(|v| v.as_str());
        let Some(raw) = raw else { continue };
        let unwrapped = serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|inner| {
                inner
                    .pointer("/error/message")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| raw.to_string());
        let is_error = event.get("type").and_then(|t| t.as_str()) == Some("error")
            || event.pointer("/item/type").and_then(|t| t.as_str()) == Some("error")
            || event.get("error").is_some();
        if is_error && !unwrapped.trim().is_empty() {
            last = Some(unwrapped.trim().to_string());
        }
    }
    last.map(|reason| {
        // One sentence, not a wall. Long provider errors repeat themselves.
        let trimmed: String = reason.chars().take(300).collect();
        trimmed
    })
}

/// Ask a yes/no question. Defaults to no when there is nobody to answer.
///
/// EOF means no terminal -- the command was piped or run by a script -- and
/// silently writing to somebody's config file in that case would be the wrong
/// guess. `--yes` is the way to say yes without a terminal.
fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer)? == 0 {
        eprintln!();
        return Ok(false);
    }
    let answer = answer.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Where a client keeps its MCP config, and what to call it in a sentence.
struct ClientTarget {
    key: &'static str,
    label: &'static str,
    path: PathBuf,
    /// What the person does after the file is written.
    restart: &'static str,
}

/// The config files we know how to edit, on this machine.
///
/// Paths are the ones the vendors document. Claude Desktop ships for macOS and
/// Windows only, so on Linux it simply is not offered rather than pointing at a
/// path nobody has verified.
fn connect_targets() -> Vec<ClientTarget> {
    let home = dirs_home();
    let mut targets = Vec::new();

    let claude = if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/Claude/claude_desktop_config.json"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(|appdata| PathBuf::from(appdata).join("Claude/claude_desktop_config.json"))
    } else {
        None
    };
    if let Some(path) = claude {
        targets.push(ClientTarget {
            key: "claude-desktop",
            label: "Claude Desktop",
            path,
            restart: "Quit Claude Desktop and open it again.",
        });
    }

    targets.push(ClientTarget {
        key: "cursor",
        label: "Cursor",
        path: home.join(".cursor/mcp.json"),
        restart: "Reload Cursor.",
    });
    targets
}

fn dirs_home() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Set up an AI app to build Krate apps, by editing its config file.
///
/// This exists because the alternative was a documentation page telling people
/// to hand-edit JSON in a file whose path differs per operating system. That is
/// a wall in front of the product for anyone who does not already know what MCP
/// is, and the people we most want are exactly those people.
///
/// It is careful with a file it did not write: it merges into whatever is
/// already there, keeps every other server, shows the change, and asks before
/// writing.
fn connect(app: Option<&str>, yes: bool, dry_run: bool) -> Result<u8> {
    let targets = connect_targets();

    let chosen = match app {
        Some(name) => {
            let name = name.trim().to_lowercase();
            match targets.iter().find(|t| t.key == name) {
                Some(target) => target,
                None => {
                    println!("I do not know how to set up \"{name}\".");
                    println!();
                    println!("I can set up:");
                    for target in &targets {
                        println!("  {:<16}{}", target.key, target.label);
                    }
                    return Ok(1);
                }
            }
        }
        None => {
            // No argument: pick the one that is actually installed, and say so.
            let installed: Vec<&ClientTarget> =
                targets.iter().filter(|t| connect_app_present(t)).collect();
            match installed.len() {
                1 => installed[0],
                0 => {
                    println!("I could not find Claude Desktop or Cursor on this computer.");
                    println!();
                    println!("Install one of them, sign in, then run this again. Or make an app");
                    println!("right now without either:");
                    println!();
                    println!(
                        "  krate create \"a habit tracker\" --output habit.krate --agent claude"
                    );
                    return Ok(0);
                }
                _ => {
                    println!("Which one do you want to set up?");
                    println!();
                    for target in &installed {
                        println!("  krate connect {}", target.key);
                    }
                    return Ok(0);
                }
            }
        }
    };

    // The path to this binary, not a bare `krate`. A client launches the server
    // with its own environment, where PATH may not include the install
    // directory -- and a bare name would find an older installed Krate anyway,
    // which is how a fixed bug appeared to come back once already.
    let exe = std::env::current_exe()
        .context("locate the krate binary")?
        .to_string_lossy()
        .to_string();

    let existing = fs::read_to_string(&chosen.path).unwrap_or_default();
    let mut config: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&existing).with_context(|| {
            format!(
                "{} is not valid JSON. Fix or move it, then run this again -- \
                 I will not overwrite a file I cannot read.",
                chosen.path.display()
            )
        })?
    };

    let already = config
        .pointer("/mcpServers/krate/command")
        .and_then(|v| v.as_str())
        .map(|command| command == exe)
        .unwrap_or(false);
    if already {
        println!("{} is already set up.", chosen.label);
        println!();
        println!("{}", chosen.restart);
        println!("Then ask it: \"build me a habit tracker and package it as a .krate\".");
        return Ok(0);
    }

    let servers = config
        .as_object_mut()
        .context("the config file's top level is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .context("`mcpServers` in the config file is not a JSON object")?;
    let other_count = servers.len();
    servers.insert(
        "krate".to_string(),
        serde_json::json!({ "command": exe, "args": ["mcp"] }),
    );

    let rendered = format!("{}\n", serde_json::to_string_pretty(&config)?);

    println!("Setting up {}.", chosen.label);
    println!();
    println!("  file:  {}", chosen.path.display());
    if other_count > 0 {
        println!("  keeps: {other_count} other server(s) already in the file");
    }
    println!();

    if dry_run {
        println!("Would write:");
        println!();
        for line in rendered.lines() {
            println!("  {line}");
        }
        return Ok(0);
    }

    if !yes && !confirm("Write this change?")? {
        println!("Nothing was changed.");
        return Ok(0);
    }

    if let Some(parent) = chosen.path.parent() {
        fs::create_dir_all(parent).ok();
    }
    // Back up a file somebody else wrote, before touching it.
    if !existing.trim().is_empty() {
        let backup = chosen.path.with_extension("json.krate-backup");
        if fs::write(&backup, &existing).is_ok() {
            println!("Saved a copy of the old file at {}", backup.display());
        }
    }
    fs::write(&chosen.path, rendered)
        .with_context(|| format!("write {}", chosen.path.display()))?;

    println!();
    println!("Done. Two more steps:");
    println!();
    println!("  1. {}", chosen.restart);
    println!("  2. Ask it: \"build me a habit tracker and package it as a .krate\"");
    println!();
    println!("It will take a few minutes and hand you a file you can send to anyone.");
    Ok(0)
}

/// Whether the app looks installed, so `krate connect` with no argument can
/// pick the right one instead of asking a question the machine can answer.
fn connect_app_present(target: &ClientTarget) -> bool {
    if target.path.exists() {
        return true;
    }
    match target.key {
        "claude-desktop" => {
            Path::new("/Applications/Claude.app").exists()
                || dirs_home().join("Applications/Claude.app").exists()
        }
        "cursor" => {
            Path::new("/Applications/Cursor.app").exists()
                || dirs_home().join("Applications/Cursor.app").exists()
                || agent_provider::which_on_path("cursor").is_some()
        }
        _ => false,
    }
}

/// Show which AI coding tools are on this machine.
///
/// This is the "connect your AI" step, and it is deliberately a lookup rather
/// than a login: every one of these tools already has its own sign-in, and
/// Krate holding a copy of someone's credentials would be strictly worse than
/// the tool holding its own. Nothing here reads a key, opens a browser, or
/// talks to a server -- it looks at PATH and tells you what you can use.
fn ai_status() -> Result<u8> {
    let installed: Vec<_> = agent_provider::PROVIDERS
        .iter()
        .filter(|provider| agent_provider::is_installed(**provider))
        .collect();

    if installed.is_empty() {
        println!("No AI coding tool found on this machine.");
        println!();
        println!("Install any one of these, sign in to it once, and Krate will use it:");
        for provider in agent_provider::PROVIDERS {
            println!("  {:<9}{}", provider.name(), provider.install_hint());
        }
        println!();
        println!("You can also build an app without AI:");
        println!("  krate create \"a checklist\" --output checklist.krate");
        return Ok(0);
    }

    // Probe rather than trust PATH. Every provider is checked in parallel so
    // the whole listing costs one round trip rather than four.
    let probes: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = installed
            .iter()
            .map(|provider| {
                scope.spawn(move || {
                    (
                        **provider,
                        agent_provider::probe(**provider, std::time::Duration::from_secs(20)),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });

    let working: Vec<_> = probes.iter().filter(|(_, r)| r.is_working()).collect();
    let broken: Vec<_> = probes.iter().filter(|(_, r)| !r.is_working()).collect();

    if !working.is_empty() {
        println!("Ready to use:");
        for (provider, _) in &working {
            println!("  {:<9}{}", provider.name(), provider.description());
        }
        println!();
    }

    if !broken.is_empty() {
        println!("Installed, but not usable yet:");
        for (provider, readiness) in &broken {
            if let agent_provider::Readiness::NotReady { summary, remedy } = readiness {
                println!("  {:<9}{}", provider.name(), summary);
                if let Some(remedy) = remedy {
                    println!("           fix it with: {remedy}");
                }
            }
        }
        println!();
    }

    let missing: Vec<_> = agent_provider::PROVIDERS
        .iter()
        .filter(|provider| !agent_provider::is_installed(**provider))
        .collect();
    if !missing.is_empty() {
        println!();
        println!("Not installed:");
        for provider in &missing {
            println!("  {:<9}{}", provider.name(), provider.install_hint());
        }
    }

    // Suggest a provider that actually answered, not merely one on PATH.
    let Some(first) = working.first().map(|(provider, _)| provider.name()) else {
        println!("None of the installed tools can write an app right now.");
        println!("Fix one of the above, or install another, then run `krate ai` again.");
        return Ok(0);
    };
    println!("Make an app:");
    println!("  krate create \"a habit tracker\" --output habit.krate --agent {first}");
    Ok(0)
}

/// The error for an agent that ran out of time: the last `check-app` verdict, so
/// the person sees how close it got and what remained, not just "it stalled".
fn author_stalled_error(app_dir: &str, transcript: &Path, timeout_secs: u64) -> anyhow::Error {
    let minutes = timeout_secs / 60;
    let verdict = match check_app_verdict(app_dir) {
        Ok(()) => "The last check-app run actually passed -- re-running the command should \
                   finish the packaging."
            .to_string(),
        Err(failure) => format!("The last check-app run reported:\n\n{failure}"),
    };
    anyhow::anyhow!(
        "the AI agent did not finish within {minutes} minutes and was stopped.\n\n{verdict}\n\n\
         Two things to try:\n  \
         1. Run the command again -- authoring often finishes on a second try, and it \
         resumes from the code already written.\n  \
         2. Raise the budget: set KRATE_AUTHOR_TIMEOUT_SECS to more seconds.\n\
         The agent's transcript is at {}.",
        transcript.display()
    )
}

/// Run the check-app oracle against an authored app and return its verdict as a
/// string on failure. This is the same check the agent was told to satisfy, run
/// once more so `create` reports the true blocker instead of a generic message.
fn check_app_verdict(app_dir: &str) -> std::result::Result<(), String> {
    match run_check_app(Path::new(app_dir), None, false) {
        Ok(_) => Ok(()),
        Err(failure) => {
            let mut message = format!("{} failed: {}", failure.stage.label(), failure.detail);
            if !failure.fix.is_empty() {
                message.push_str("\n\nFix: ");
                message.push_str(&failure.fix);
            }
            Err(message)
        }
    }
}

fn run_author_command(ctx: AuthorContext<'_>) -> Result<()> {
    use krate_author::skeleton;

    fs::create_dir_all(ctx.app_dir.join("src"))?;

    // Give the agent a minimal compiling skeleton -- a blank that already
    // builds, imports only krate:*, and passes check-app -- not a behavioral
    // template to adapt. The skeleton's world (GUI vs CLI) is chosen from the
    // request, because that sets the WIT wiring the agent should not have to
    // redo; everything else it writes. The agent overwrites src/lib.rs and
    // tunes manifest.toml.
    let world = krate_author::AppKind::wants_gui(ctx.request);
    if let Ok(app) = skeleton(ctx.name, ctx.sdk_prefix, world) {
        for file in &app.files {
            let dest = ctx.app_dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&dest, &file.contents);
        }
    }
    // The context pack: the real facts the agent builds against (the SDK
    // surface, the capability catalog, the no_std discipline, the GUI-world
    // interfaces, and an index of the shipped example apps). Replaces the old
    // one-page CONTRACT.
    fs::write(
        ctx.app_dir.join("KRATE_AUTHORING.md"),
        authoring_context::generate(ctx.app_dir),
    )?;

    let shell = author_shell();
    let mut command = std::process::Command::new(shell);
    command
        .arg("-c")
        .arg(ctx.cmd)
        .env("KRATE_APP_DIR", ctx.app_dir)
        .env("KRATE_APP_NAME", ctx.name)
        .env("KRATE_REQUEST", ctx.request)
        .env("KRATE_APP_KIND", app_kind_name(ctx.kind))
        // The materialized SDK: the agent resolves WIT/bindings from here.
        .env("KRATE_SDK_DIR", ctx.sdk_dir);

    // With a display running, take the child's output rather than letting it
    // reach the terminal. Inheriting meant the child's progress lines and the
    // compiler's warnings scrolled through the display's own redraw region:
    // the first stage appeared frozen for the whole run while the app really
    // was being written, compiled and packed.
    let drawing = progress_sink().is_some();
    let status = if drawing {
        command.env(PROGRESS_CHANNEL, "1");
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let mut child = command.spawn().context("run --author-cmd")?;

        // stderr is where cargo writes. Drain it so a full pipe cannot block
        // the child, but keep it for the failure message rather than showing
        // it: a warning about an unused variable is not something the person
        // asked to see.
        let stderr = child.stderr.take();
        let drain = stderr.map(|stderr| {
            std::thread::spawn(move || {
                use std::io::BufRead;
                let mut kept = String::new();
                for line in io::BufReader::new(stderr).lines().map_while(Result::ok) {
                    if kept.len() < 64 * 1024 {
                        kept.push_str(&line);
                        kept.push('\n');
                    }
                }
                kept
            })
        });

        if let Some(stdout) = child.stdout.take() {
            use std::io::BufRead;
            for line in io::BufReader::new(stdout).lines().map_while(Result::ok) {
                // Anything untagged is the agent's own chatter. It is in the
                // transcript already, so it does not go on screen.
                if let Some(step) = line.strip_prefix(PROGRESS_PREFIX) {
                    report_progress(step);
                } else if let Some(step) = line.strip_prefix(PROGRESS_TICK) {
                    report_progress_alive(step);
                }
            }
        }
        let status = child.wait().context("wait for --author-cmd")?;
        if !status.success() {
            let detail = drain
                .and_then(|handle| handle.join().ok())
                .unwrap_or_default();
            let tail: Vec<&str> = detail.lines().rev().take(20).collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            if tail.trim().is_empty() {
                anyhow::bail!("author command failed");
            }
            anyhow::bail!("author command failed:\n\n{tail}");
        }
        status
    } else {
        command.status().context("run --author-cmd")?
    };
    if !status.success() {
        anyhow::bail!("author command failed");
    }
    // The agent must have (kept or written) the three files.
    for file in ["Cargo.toml", "src/lib.rs", "manifest.toml"] {
        if !ctx.app_dir.join(file).exists() {
            anyhow::bail!("author command did not write {file}");
        }
    }
    Ok(())
}

fn app_kind_name(kind: krate_author::AppKind) -> &'static str {
    match kind {
        krate_author::AppKind::Checklist => "checklist",
        krate_author::AppKind::WordFrequency => "word-frequency",
        krate_author::AppKind::VoicePrompter => "voice-prompter",
    }
}

/// The briefing dropped into the app dir for an agent. States the one hard rule
/// and how the app is checked, so the agent gets it right the first time.
/// Every `.wit` file under a root, for checking documented paths are real.
#[cfg(test)]
fn walkdir_wit(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walkdir_wit(&path));
        } else if path.extension().is_some_and(|e| e == "wit") {
            found.push(path);
        }
    }
    found
}

fn author_contract(name: &str) -> String {
    // The contract used to state rules and prohibitions without ever listing
    // what exists. An agent porting a hex viewer invented `stdio::write`,
    // because nothing it had been given said the API was text-only. The list
    // is generated from the SDK source, so it cannot drift out of date.
    let reference =
        sdk_reference::render_reference(&sdk_reference::parse_sdk(sdk_reference::GUEST_SDK_SOURCE));
    // The capability names a manifest may use, generated from the runtime's own
    // registry. The contract listed sixty-three functions and exactly one
    // capability name, so an agent knew how to call `store::set` and had to
    // guess that the manifest needs `store.kv`.
    let capabilities = requestable_capability_list();
    format!(
        "# Krate app contract for `{name}`\n\
\n\
You are writing a Krate guest app in Rust. Three files must exist in this\n\
directory when you finish: `Cargo.toml`, `src/lib.rs`, `manifest.toml`.\n\
A compiling starter for each is already here — edit it to fit the request in\n\
the `KRATE_REQUEST` environment variable.\n\
\n\
## The one hard rule\n\
A Krate component may import ONLY `krate:*` interfaces. Anything that reaches\n\
the operating system through std instead of through Krate pulls `wasi:*`\n\
imports in, and the app is rejected. So never use:\n\
\n\
- `std::fs` -- use `krate::fs`\n\
- `std::io` (including `println!`, `eprintln!`, and `dbg!`) -- use `krate::io`\n\
- `std::time` -- use `krate::time`\n\
- `std::env` -- use `krate::io::args`\n\
- `std::process`, `std::net`, `std::thread`\n\
\n\
Ordinary in-memory std is fine: `String`, `format!`, `Vec`, `HashMap`, and\n\
iterators do not reach the operating system and do not leak.\n\
\n\
The sharp exception is **anything that can panic**. A reachable panic makes\n\
std's failure path reachable, and that path formats a message, writes it to\n\
stderr, and exits -- which is `wasi:cli`, `wasi:filesystem`, and `wasi:io`\n\
arriving together. It is all-or-nothing: one panic site takes a component from\n\
zero wasi imports to thirty-three.\n\
\n\
The two that catch people, both measured:\n\
\n\
- **Indexing.** `buf[i]` carries a bounds check that can panic, even when the\n\
  index is provably fine. Use `.get(i)` / `.get_mut(i)` and handle the `None`.\n\
- **`.to_string()` and `format!`** on a literal, which route through the\n\
  allocator's out-of-memory handler. Copy `pure_string` from the in-repo\n\
  samples; it allocates the bytes directly.\n\
\n\
When a component that looks clean still imports `wasi:*`, do not re-read the\n\
Krate calls -- look for the nearest `[` and the nearest `to_string`. Keep\n\
`panic = \"abort\"` and `opt-level = \"s\"` in the release profile, which is what\n\
stops std's unwinding and formatting machinery dragging its own I/O in.\n\
\n\
Both `#![no_std]` and plain std work. Which one you need depends on your\n\
dependencies, and getting this wrong is the most expensive mistake here:\n\
\n\
- **No dependencies beyond the bindings?** Use std. `krate-notes` is a shipped\n\
  GUI app that does exactly this and imports zero `wasi:*`.\n\
- **Any real dependency -- a decoder, a parser?** Use `#![no_std]`. A crate\n\
  that never touches the operating system still leaks through std's panic\n\
  path: one reachable panic pulls in `fd_write`, `environ_get`, and\n\
  `proc_exit` to format the message and exit. `zune-png` imports nothing under\n\
  `no_std` and four wasi functions with std linked.\n\
\n\
`no_std` works here because `Cargo.toml` sets `std_feature = true` under\n\
`[package.metadata.component.bindings]`, which puts the generated\n\
`impl std::error::Error` behind a feature nobody turns on. Leave that line\n\
alone. A `no_std` crate must own its allocator, `#[panic_handler]`, and the\n\
`mem*` intrinsics -- the starter already has all three.\n\
\n\
If you write a fixed-size arena allocator, keep it well under 256 MB. That is\n\
the runtime's whole memory limit, and the module's own code and stack come out\n\
of the same budget -- an arena of exactly 256 MB fails at startup with \"memory\n\
limit exceeded\" before a line of the app runs. 192 MB leaves room; far less is\n\
usually plenty.\n\
\n\
## The manifest\n\
Declare only the capabilities the app uses. Mark the one that gates it\n\
(`fs.write` for a saving app) `required = true`. Anything not listed here is\n\
granted to every app and must not be declared.\n\
\n\
`required = true` is a promise the verification run tests: it withholds that\n\
one capability and the app must refuse to start, exiting 5. So mark required\n\
only what the app cannot begin without. A photo frame needs the file dialog to\n\
be *useful*, but it can still open its window and wait -- so `ui.dialog` is\n\
`required = false` and `ui.window:create` is the one that gates it. Marking a\n\
capability the `quick` path never reaches makes the app fail its own wall test\n\
after building and packaging correctly.\n\
\n\
{capabilities}\n\
\n\
Paths are relative to the sandbox the app is given, and `~` is refused: a home\n\
directory is not reachable, by design. Declare `fs.read:images/**`, not\n\
`fs.read:~/Pictures/**`. To reach anything outside that sandbox, ask the person\n\
-- see below.\n\
\n\
## Getting a file from the person (GUI apps)\n\
A windowed app does not have to be handed a path. Call\n\
`bindings::krate::ui::dialog::open_file(window, title, filter)` and the system's\n\
own file dialog opens. `filter` is a comma-separated extension list such as\n\
`\"png,jpg\"`, or an empty string for any file.\n\
\n\
It returns a name and a token, never a path, and `none` when the person\n\
cancelled -- which is a normal answer, not an error. Pass the token to\n\
`bindings::krate::fs::files::open_chosen(token, mode)` to read or write that\n\
one file. The app never learns where the file lives, and the token stops working\n\
when the run ends.\n\
\n\
Declare `ui.dialog:file-open`. You do not need an `fs.read` grant for a file the\n\
person chose: their click is the grant. Prefer this over asking someone to put\n\
files in a folder before starting -- an app that opens with \"choose a file\" is\n\
one anybody can use.\n\
\n\
## Choosing where data lives\n\
Three stores exist and the choice is meaning, not preference:\n\
\n\
- `store.kv` -- settings, lists, app state. Plain storage.\n\
- `store.secret` -- anything a person would call a password, token, key, or\n\
  PIN. It is backed by the operating system's own keychain. A \"password\n\
  keeper\" built on `store.kv` stores passwords in plain app data, which is\n\
  the one thing its user asked it not to do.\n\
- `store.sql` -- rows you filter, join, or sum. If the request says\n\
  \"between runs\" and \"running total\", this is usually it.\n\
\n\
## Showing a picture\n\
Build a widget of kind `Image`, then call\n\
`bindings::krate::ui::image::set_pixels(window, widget, pixels)` with\n\
`bindings::krate::ui::image::ImagePixels {{ width, height, rgba }}` -- note the\n\
record lives in `ui::image`, not in `ui::types` where most records are --\n\
straight RGBA bytes, four per pixel,\n\
top row first, exactly `width * height * 4` of them. `image::clear(window,\n\
widget)` takes the picture away again.\n\
\n\
Not PNG or JPEG. Decode the file yourself and send the result.\n\
\n\
Use these exact versions, all with `default-features = false`:\n\
\n\
    zune-png = {{ version = \"0.5\", default-features = false }}\n\
    zune-jpeg = {{ version = \"0.5\", default-features = false }}\n\
    zune-core = {{ version = \"0.5\", default-features = false }}\n\
\n\
The version matters: `ZCursor` below is 0.5 only. On 0.4 the decoders take a\n\
byte slice directly and the import fails to resolve.\n\
\n\
Do **not** reach for the `image` crate. It is the obvious choice and it cannot\n\
work here: it requires `std` unconditionally, so linking it drags in the whole\n\
`wasi:*` surface and the component is rejected for importing host APIs that are\n\
not `krate:*`. That failure appears at the import check, long after the build\n\
succeeds, and it looks like a Krate bug rather than a dependency choice.\n\
\n\
The shape that works:\n\
\n\
    let mut d = zune_png::PngDecoder::new(zune_core::bytestream::ZCursor::new(bytes));\n\
    let rgba = d.decode_raw()?;      // already RGBA, four bytes per pixel\n\
    let (w, h) = d.dimensions()?;    // usize, cast to u32\n\
\n\
Every host scales the picture to fit the widget and centres it, keeping the\n\
original proportions, so a photo looks the same on macOS, Windows, and Linux.\n\
An image widget with no pixels yet draws an empty frame, which is what a viewer\n\
should show before anybody has chosen a file.\n\
\n\
## The verification run\n\
After building, Krate runs the app once with every capability granted and one\n\
argument, then requires exit 0.\n\
\n\
The argument is `quick` -- bare, not `--quick`, not a flag -- unless the app\n\
declares an `fs.read:` grant and no window, in which case it is instead a path\n\
to a sample text file inside the granted directory. So a file-reading CLI must\n\
accept **both**: a path as its first positional argument, and the bare word\n\
`quick`. Handle `quick` before any other argument parsing.\n\
\n\
Whichever it gets, the app must do its real work once, print something, and\n\
exit 0 without waiting for input or opening a window nobody will close.\n\
\n\
An app that parses arguments strictly will reject an unknown `quick` and exit\n\
non-zero, and the port then fails at the last step having built and packed\n\
correctly. Check for it before any other argument handling.\n\
\n\
## What happens next\n\
`krate create` builds what you write, checks it imports only `krate:*`, packs\n\
it, and verifies its permission wall. If you reach for something unsafe, the\n\
import check stops it here — it never ships. The SDK (WIT + Rust bindings) is\n\
at `$KRATE_SDK_DIR`.\n\
\n\
{reference}"
    )
}

/// The directory holding a rustup-managed `cargo`/`rustc` that has the wasm
/// target, if rustup is present.
///
/// On many Macs `brew install rust` puts a Homebrew `cargo`/`rustc` first on
/// PATH. That toolchain is NOT rustup-managed and has no `wasm32-wasip1`
/// target, so `cargo-component` picks it up and fails with "failed to find the
/// `wasm32-wasip1` target and rustup is not available" even though a rustup
/// toolchain with the target is installed. Prepending rustup's own toolchain
/// bin to the child PATH makes the right cargo/rustc win.
fn rustup_toolchain_bin() -> Option<PathBuf> {
    // On Windows, prefer the gnullvm toolchain when it is installed: it links
    // host build scripts with LLVM's linker, so a build works without Visual
    // Studio Build Tools. Falling through to the default toolchain keeps
    // machines that already have MSVC working exactly as before.
    #[cfg(windows)]
    if gnullvm_toolchain_present() {
        let out = ProcessCommand::new("rustup")
            .args([
                "run",
                gnullvm_toolchain_name(),
                "rustc",
                "--print",
                "sysroot",
            ])
            .output()
            .ok();
        if let Some(out) = out {
            if out.status.success() {
                let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let bin = PathBuf::from(sysroot).join("bin");
                if bin.join("cargo.exe").exists() {
                    return Some(bin);
                }
            }
        }
    }

    // Prefer PATH, then rustup's own home. A shell that was already open when
    // rustup installed does not see the new PATH entry, so a run that just
    // installed rustup would otherwise report it as still missing -- which is
    // exactly what happened on Windows right after a successful winget
    // install.
    let exe = if cfg!(windows) {
        "rustup.exe"
    } else {
        "rustup"
    };
    let fallback = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".cargo")))
        .map(|home| home.join("bin").join(exe))
        .filter(|path| path.exists());

    let mut command = match &fallback {
        Some(path) if agent_provider::which_on_path("rustup").is_none() => {
            ProcessCommand::new(path)
        }
        _ => ProcessCommand::new("rustup"),
    };
    let out = command.args(["which", "cargo"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    PathBuf::from(path.trim()).parent().map(Path::to_path_buf)
}

/// Build the app dir to a wasm component with cargo-component, returning the
/// path to the produced wasm.
///
/// The child's PATH is made robust against two common local setups:
/// - `cargo-component` is resolved to its real path (PATH, then the cargo bin
///   dir) rather than spawned by bare name, so a conda base env or a login
///   shell that puts `~/.cargo/bin` last still finds it.
/// - rustup's active-toolchain bin is prepended, so a Homebrew `cargo`/`rustc`
///   that shadows rustup (and lacks the wasm target) does not get used.
fn component_build_command(app_dir: &Path) -> ProcessCommand {
    let resolved = resolve_tool("cargo-component");
    let program: std::ffi::OsString = match &resolved {
        Some(path) => path.clone().into_os_string(),
        None => "cargo-component".into(),
    };
    let mut command = ProcessCommand::new(&program);
    command.arg("build").arg("--release").current_dir(app_dir);

    // Build the child PATH: rustup's toolchain bin first (so a rustup cargo/rustc
    // with the wasm target wins over a Homebrew one), then the cargo bin dir that
    // holds cargo-component, then the inherited PATH.
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut prefix: Vec<PathBuf> = Vec::new();
    if let Some(bin) = rustup_toolchain_bin() {
        prefix.push(bin);
    }
    if let Some(dir) = resolved.as_deref().and_then(Path::parent) {
        prefix.push(dir.to_path_buf());
    }
    if !prefix.is_empty() {
        prefix.extend(std::env::split_paths(&existing));
        if let Ok(joined) = std::env::join_paths(prefix) {
            command.env("PATH", joined);
        }
    }
    command
}

fn find_built_component(app_dir: &Path) -> Result<PathBuf> {
    let release = app_dir.join("target/wasm32-wasip1/release");
    for entry in fs::read_dir(&release).with_context(|| format!("read {}", release.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
            return Ok(path);
        }
    }
    anyhow::bail!("no wasm produced in {}", release.display())
}

fn build_component(app_dir: &Path) -> Result<PathBuf> {
    let mut command = component_build_command(app_dir);
    let output = command
        .output()
        .context("run cargo-component (is it installed? `cargo install cargo-component`)")?;
    // The compiler's own words reach the person either way.
    std::io::Write::write_all(&mut std::io::stdout(), &output.stdout).ok();
    std::io::Write::write_all(&mut std::io::stderr(), &output.stderr).ok();
    if !output.status.success() {
        // The old message appended toolchain advice to every failure, so a
        // WIT naming error surfaced as "install the wasm32-wasip1 target" --
        // advice that had nothing to do with the cause and pointed the person
        // away from it. The hint appears only when the output mentions the
        // thing the hint is about.
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if text.contains("wasm32-wasip1") {
            anyhow::bail!(
                "cargo-component build failed: the `wasm32-wasip1` target looks missing. \
                 Install it with `rustup target add wasm32-wasip1`. If rustup itself was \
                 not found, a non-rustup Rust (for example `brew install rust`) may be \
                 shadowing it on your PATH."
            );
        }
        anyhow::bail!("cargo-component build failed; the compiler's message above is the cause");
    }
    find_built_component(app_dir)
}

/// Point at the lines in a candidate most likely to be the reachable panic.
///
/// Not a proof -- a grep. But the failure it explains is one where the compiler
/// says nothing useful and the honest debugging technique is a bisect that
/// takes an hour, so naming three candidate lines is worth far more than its
/// false-positive rate.
fn panic_site_hints(candidate: &Path) -> String {
    let Ok(source) = fs::read_to_string(candidate.join("src/lib.rs")) else {
        return String::new();
    };
    let mut hints = String::new();
    let mut found = 0;
    for (number, line) in source.lines().enumerate() {
        if found >= 5 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        // An index expression: `name[` where the name is not a type parameter
        // or an attribute. Crude on purpose; a false hint costs one glance.
        let indexes = trimmed.contains('[')
            && trimmed.contains(']')
            && !trimmed.starts_with('#')
            && !trimmed.contains(": [")
            && !trimmed.contains("= [");
        let allocates = trimmed.contains(".to_string()") || trimmed.contains("format!");
        if indexes || allocates {
            let why = if allocates {
                "allocates a String"
            } else {
                "indexes, so it can panic"
            };
            hints.push_str(&format!("\n  src/lib.rs:{}: {why}", number + 1));
            found += 1;
        }
    }
    hints
}

fn build_component_captured(app_dir: &Path) -> std::result::Result<PathBuf, String> {
    let mut command = component_build_command(app_dir);
    let output = command.output().map_err(|error| {
        format!(
            "could not run cargo-component: {error}. Is it installed with `cargo install cargo-component`?"
        )
    })?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut detail = format!(
            "cargo-component build failed with {}\n\nstdout:\n{}\n\nstderr:\n{}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
        const MAX_REPAIR_ERROR_BYTES: usize = 24 * 1024;
        if detail.len() > MAX_REPAIR_ERROR_BYTES {
            detail.truncate(MAX_REPAIR_ERROR_BYTES);
            detail.push_str("\n[build output truncated]");
        }
        return Err(detail);
    }
    find_built_component(app_dir).map_err(|error| format!("{error:#}"))
}

/// The capability whose grant gates the app — the required one the verify step
/// withholds to prove the wall. Prefers fs.write, then fs.read, else the first
/// required capability.
/// The capability to withhold when proving an app's permission wall works.
///
/// Returns `None` when the app declares nothing suitable to withhold. That is a
/// real state, not a failure: a GUI app whose required set is a window and its
/// own output has no capability that gates it, because the runtime grants
/// stdout and stdin by default and refusing the window is refusing the app.
///
/// It used to fall back to `fs.write` regardless. A ported budget app that
/// never touches a file was then tested by withholding a capability it had
/// never asked for, which of course did not refuse it, and the port failed at
/// the last step after building and packaging correctly.
fn gating_capability(manifest: &krate_manifest::Manifest) -> Option<String> {
    let required: Vec<String> = manifest
        .capabilities
        .iter()
        .filter(|c| c.required)
        .map(|c| c.cap.clone())
        .collect();
    // Filesystem access first: it is the clearest thing for a person reading
    // the evidence to understand being withheld.
    for prefer in ["fs.write", "fs.read"] {
        if let Some(cap) = required.iter().find(|c| c.starts_with(prefer)) {
            return Some(cap.clone());
        }
    }
    // Otherwise any capability that is not granted by default and is not the
    // window itself, since withholding the window just closes the app.
    required
        .into_iter()
        .find(|c| !c.starts_with("io.") && !c.starts_with("ui.window"))
}

/// Create the data directories the app expects under the verify dir, so a
/// granted run has somewhere to write, and seed a read directory with a small
/// fixture file when the app reads a file argument.
///
/// Returns the argument the verify run should pass. A file-reading CLI app (the
/// word-frequency kind) needs a real file path to read, so a fixture is written
/// and its path returned. A GUI app (the checklist, which declares `ui.window`)
/// must instead be given the plain `quick` token so it exits promptly rather
/// than opening a window and waiting — passing it a file path would leave it
/// blocked on its window, which the caller returns `None` for so the plain
/// `quick` fallback is used.
fn prepare_verify_dir(dir: &Path, manifest: &krate_manifest::Manifest) -> Result<Option<String>> {
    // A GUI app opens a window and honors `quick` to exit; it must not be handed
    // a file-path argument. Detect it by its window capability.
    let is_gui = manifest
        .capabilities
        .iter()
        .any(|cap| cap.cap.starts_with("ui.window"));

    let mut read_arg = None;
    for cap in manifest.capabilities.iter() {
        let name = cap.cap.clone();
        let is_read = name.starts_with("fs.read:");
        if let Some(rest) = name
            .strip_prefix("fs.read:")
            .or_else(|| name.strip_prefix("fs.write:"))
        {
            // Turn "./checklist/**" into the directory "checklist".
            let trimmed = rest.trim_start_matches("./");
            if let Some(first) = trimmed.split('/').next() {
                if !first.is_empty() && first != "**" {
                    let subdir = dir.join(first);
                    let _ = fs::create_dir_all(&subdir);
                    // Drop a fixture only for a file-reading CLI app, so its
                    // all-grants verify run has a real file to analyze. A GUI app
                    // gets `quick` instead (read_arg stays None).
                    if is_read && !is_gui && read_arg.is_none() {
                        // Write the fixture either way: an app handed `quick`
                        // still needs something real inside its granted subtree
                        // to do its work against.
                        let fixture = subdir.join("sample.txt");
                        fs::write(&fixture, "the quick brown fox the lazy dog the fox\n")
                            .with_context(|| format!("write {}", fixture.display()))?;
                        read_arg = Some(format!("{first}/sample.txt"));
                    }
                }
            }
        }
    }
    Ok(read_arg)
}

/// Re-invoke this same `krate` binary in `dir` with `args`, returning its exit
/// code. Used to verify a packed bundle in isolation.
/// How long the verification run may take before it is treated as hung and
/// killed. Generous for a real app finishing its work under `quick`, but bounded
/// so `create` can never hang a user's terminal indefinitely — e.g. if a
/// generated GUI app fails to honor `quick` and waits on a window that never
/// closes.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Resolve a possibly-relative output path to an absolute one without depending
/// on a healthy current directory. `fs::canonicalize` on a relative path reads
/// the process cwd, which a shell can invalidate (deleting the directory it is
/// in). We canonicalize the file's parent — which was just written and exists —
/// and rejoin the file name; if even that fails we fall back to joining the file
/// onto the current dir manually. The bundle was already written to `output`, so
/// its parent is guaranteed to exist here.
/// Resolve a path to absolute against the current directory, without requiring
/// it to exist. Canonicalizes when the path is present (following symlinks),
/// otherwise joins it onto the current dir. Used by check-app so paths handed to
/// a child process -- which runs in a different cwd -- resolve to the same file.
fn absolute_from_cwd(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    fs::canonicalize(path)
        .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(path)))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn absolute_output_path(output: &Path) -> Result<PathBuf> {
    if output.is_absolute() {
        return Ok(output.to_path_buf());
    }
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty());
    let file = output.file_name().context("output path has no file name")?;
    match parent {
        Some(parent) => {
            let parent_abs = fs::canonicalize(parent)
                .or_else(|_| std::env::current_dir().map(|cwd| cwd.join(parent)))
                .with_context(|| format!("resolve output directory {}", parent.display()))?;
            Ok(parent_abs.join(file))
        }
        None => {
            // Bare file name in the current directory.
            let cwd = std::env::current_dir()
                .context("the current directory is unavailable; pass an absolute --output path")?;
            Ok(cwd.join(file))
        }
    }
}

fn run_self(dir: &Path, args: &[&str]) -> Result<i32> {
    let exe = std::env::current_exe().context("locate self")?;
    // Capture the child's output rather than inherit it: the verified app's own
    // stdout (e.g. its "saved" line) is noise to the create caller, and would
    // corrupt the single-object stream under --json. Only the exit code matters.
    //
    // KRATE_VERIFY_LOG, when set to a path, streams the child's output there
    // instead of discarding it — a diagnostic for when verification hangs, so
    // the last thing the verified app printed is visible.
    let (out, err) = match std::env::var_os("KRATE_VERIFY_LOG") {
        Some(path) => {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok();
            match file {
                Some(file) => {
                    let clone = file.try_clone().ok();
                    (
                        std::process::Stdio::from(file),
                        clone
                            .map(std::process::Stdio::from)
                            .unwrap_or_else(std::process::Stdio::null),
                    )
                }
                None => (std::process::Stdio::null(), std::process::Stdio::null()),
            }
        }
        None => (std::process::Stdio::null(), std::process::Stdio::null()),
    };
    let mut child = std::process::Command::new(exe)
        .args(args)
        // Null stdin so the verification run is non-interactive. Otherwise it
        // inherits the caller's terminal, and the deny half — which withholds a
        // required capability — sees a TTY, shows the interactive "Grant
        // [A]ll/[N]one" prompt, and blocks forever waiting for a keypress that
        // never comes (the create hangs until the watchdog fires). With no TTY
        // it refuses immediately with exit 5, which is exactly what verify wants.
        .stdin(std::process::Stdio::null())
        .current_dir(dir)
        .stdout(out)
        .stderr(err)
        .spawn()
        .context("re-invoke krate for verification")?;

    // Poll for completion up to VERIFY_TIMEOUT, then kill a hung run so create
    // fails cleanly instead of blocking forever.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().context("wait for verification run")? {
            Some(status) => return Ok(status.code().unwrap_or(-1)),
            None => {
                if start.elapsed() >= VERIFY_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "the app did not finish its verification run within {} seconds and was \
                         stopped. A generated app should exit promptly when run with `quick`; \
                         if it opens a window and waits, it cannot be verified automatically.",
                        VERIFY_TIMEOUT.as_secs()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Copy a manifest, replacing its `entry` line with a new path.
fn write_manifest_with_entry(src: &Path, dest: &Path, entry: &str) -> Result<()> {
    let text = fs::read_to_string(src).with_context(|| format!("read {}", src.display()))?;
    let mut out = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("entry =") {
            out.push_str(&format!("entry = \"{entry}\"\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    fs::write(dest, out).with_context(|| format!("write {}", dest.display()))?;
    Ok(())
}

/// Resolve what the user asked to run into a component on disk.
///
/// A bare `.wasm` path is returned as-is. A `.krate` bundle, whether local or
/// fetched over the network, is unpacked into a temporary directory and its
/// in-bundle manifest is used. The returned `OpenBundle` must outlive the run:
/// dropping it deletes the extracted files.
///
/// Nothing here grants authority. A fetched bundle reaches the same policy
/// resolution as a component sitting on disk, so downloading something does not
/// make it trusted.
fn resolve_run_target(
    target: &str,
    insecure_http: bool,
) -> Result<(PathBuf, Option<PathBuf>, Option<krate_bundle::OpenBundle>)> {
    if krate_bundle::is_url(target) {
        let bundle = krate_bundle::fetch(target, insecure_http)
            .with_context(|| format!("could not open bundle from {target}"))?;
        return Ok((
            bundle.component_path().to_path_buf(),
            Some(bundle.manifest_path().to_path_buf()),
            Some(bundle),
        ));
    }

    let path = PathBuf::from(target);
    if krate_bundle::is_bundle_path(&path) {
        let bundle = krate_bundle::open(&path)
            .with_context(|| format!("could not open bundle {}", path.display()))?;
        return Ok((
            bundle.component_path().to_path_buf(),
            Some(bundle.manifest_path().to_path_buf()),
            Some(bundle),
        ));
    }

    Ok((path, None, None))
}

fn run_component(request: RunRequest) -> Result<u8> {
    // Ctrl-C must reach a running app. Without this the signal arrives, the
    // wasm keeps looping, and the window stays open -- which is what three
    // unanswered presses looked like.
    krate_runtime::phase3_gui_host::install_interrupt_handler();
    // Check the X11 keyboard library before anything opens a window, so a
    // machine without it gets one sentence instead of a Rust panic.
    check_window_libraries()?;
    // Every way an app is opened lands here: the menu, a double-click, and a
    // bare `krate run`. Counting anywhere else would miss most of them.
    usage::record_install_once();
    // Whether the app actually started is the number that matters: an open
    // that fails is exactly what a count of successes alone would hide.
    let opened = run_component_inner(request);
    usage::record_with(
        usage::Action::Open,
        usage::Facts {
            ai: None,
            ok: Some(matches!(opened, Ok(0))),
        },
    );
    opened
}

/// Fail early, and in plain words, when the X11 keyboard library is absent.
///
/// winit loads `libxkbcommon-x11.so` with dlopen when it opens a window, and
/// **panics** if it is not there. On a stock Ubuntu desktop it is not: the
/// runtime package ships `libxkbcommon-x11.so.0`, and the unversioned name
/// winit asks for only comes with the `-dev` package. So a person who was
/// promised they would never see a compiler error gets a Rust backtrace with a
/// crate path and a line number, for an app that built and packed perfectly.
///
/// The check is deliberately the same dlopen winit will do, rather than a
/// filesystem guess: the loader searches paths we do not want to reimplement,
/// and being wrong in the optimistic direction just restores the panic.
#[cfg(all(unix, not(target_os = "macos")))]
fn check_window_libraries() -> Result<()> {
    // Only X11 needs this. Under Wayland winit never loads the X11 bridge.
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    if wayland || !x11 {
        return Ok(());
    }

    const LIBRARY: &[u8] = b"libxkbcommon-x11.so\0";
    // SAFETY: a null-terminated literal, and the handle is closed on success.
    let handle = unsafe { libc::dlopen(LIBRARY.as_ptr().cast(), libc::RTLD_LAZY) };
    if !handle.is_null() {
        unsafe { libc::dlclose(handle) };
        return Ok(());
    }

    anyhow::bail!(
        "this computer is missing a library apps need to read the keyboard.\n\n\
         Install it with:\n\n    \
         sudo apt install libxkbcommon-x11-dev\n\n\
         (on Fedora: sudo dnf install libxkbcommon-x11-devel)\n\n\
         The plain `libxkbcommon-x11-0` package is not enough -- it provides \
         libxkbcommon-x11.so.0, and the name that has to exist is \
         libxkbcommon-x11.so, which only the dev package creates."
    )
}

/// Nothing to check: macOS and Windows do not use X11.
#[cfg(any(not(unix), target_os = "macos"))]
fn check_window_libraries() -> Result<()> {
    Ok(())
}

fn run_component_inner(request: RunRequest) -> Result<u8> {
    validate_app_args(&request.app_args)?;

    // Held for the whole run: dropping it removes the extracted bundle.
    let (file, bundle_manifest, bundle) =
        resolve_run_target(&request.target, request.insecure_http)?;

    if !file.exists() {
        anyhow::bail!("input file does not exist: {}", file.display());
    }

    // A bundle carries its own manifest, and an explicit --manifest must not be
    // able to widen what the bundle asked for.
    if bundle_manifest.is_some() && request.manifest_path.is_some() {
        anyhow::bail!("--manifest cannot be combined with a .krate bundle: the bundle carries its own manifest");
    }
    let manifest_path = bundle_manifest.or(request.manifest_path.clone());

    let request = RunRequest {
        file: file.clone(),
        manifest_path,
        ..request
    };

    let loaded_manifest = load_run_manifest(&request.file, request.manifest_path.as_deref())?;
    if let Some(loaded) = &loaded_manifest {
        if !manifest_entry_matches(&request.file, loaded)? {
            eprintln!(
                "permission denied: manifest entry `{}` does not match `{}`",
                loaded.manifest.app.entry.display(),
                request.file.display()
            );
            return Ok(5);
        }
    }

    let manifest = loaded_manifest.as_ref().map(|loaded| &loaded.manifest);
    let mut policy = resolve_session_policy(manifest, &request.grants, request.auto_grant)?;

    // Before the wall, not after it: --dump-caps answers "what would this app
    // ask for?" without running a single instruction, and it is the safe way to
    // look at a file someone sent you. Behind the wall it refused with exit 5
    // on exactly the apps a person most wants to inspect first. --log-grants
    // is written first so pairing the two still records what was inspected.
    if request.dump_caps {
        if let Some(log_path) = &request.log_grants {
            write_grant_log(
                log_path,
                &request.file,
                manifest,
                &policy,
                request.log_grants_format,
            )?;
        }
        print_effective_capabilities(
            &request.file,
            manifest,
            &policy,
            request.dump_caps_format,
            // A bundle's identity belongs on the screen where someone decides
            // whether to trust it. Without it, "the app I was told to verify"
            // and "the app I am about to run" are the same claim only by trust.
            bundle.as_ref().and_then(|bundle| bundle.digest().ok()),
        )?;
        return Ok(0);
    }

    if let Some(manifest) = manifest {
        let can_prompt = request.prompt || request.consent || io::stdin().is_terminal();
        let missing = policy.missing_required_for_manifest(manifest)?;
        if !missing.is_empty() && can_prompt && !request.auto_grant {
            // A double-clicked bundle asks in a native window; a terminal run
            // asks in the terminal. The two paths fold the same grant set into
            // the same SessionPolicy, so enforcement downstream is identical.
            policy = if request.consent {
                consent_for_session_grants(manifest, &policy)?
            } else {
                prompt_for_session_grants(manifest, &policy)?
            };
        }

        let missing = policy.missing_required_for_manifest(manifest)?;
        if !missing.is_empty() {
            if request.json {
                print_run_json(
                    Some(manifest),
                    &policy,
                    RunJsonExit::denied_before_run(&missing),
                    None,
                    "",
                );
            } else {
                eprintln!("This app needs permission it was not given, so it did not run.");
                eprintln!("It needs to:");
                for cap in &missing {
                    eprintln!("  - {} ({cap})", human_label(cap));
                }
                // On the double-click path there is no terminal to read, so a
                // native alert explains the refusal instead of the app just
                // vanishing. Terminal runs keep the text guidance below.
                #[cfg(target_os = "macos")]
                if request.consent {
                    let denied = missing
                        .iter()
                        .map(|cap| cap.to_string())
                        .collect::<Vec<_>>();
                    let app_name = manifest.app.name.as_str();
                    krate_adapter_macos::present_denied_alert(app_name, &denied);
                }
                // Someone running a shared app for the first time hits this
                // without knowing the vocabulary yet. Saying what is missing
                // and stopping leaves them stuck, so name the two ways out and
                // put the narrow one first.
                if !can_prompt {
                    eprintln!();
                    eprintln!("To allow one of these for this run:");
                    if let Some(first) = missing.first() {
                        eprintln!("  krate run --grant {first} {}", request.target);
                    }
                    eprintln!("Or review them one at a time:");
                    eprintln!("  krate run --prompt {}", request.target);
                }
            }
            return Ok(5);
        }
    }

    if let Some(log_path) = &request.log_grants {
        write_grant_log(
            log_path,
            &request.file,
            manifest,
            &policy,
            request.log_grants_format,
        )?;
    }

    let config = Config {
        fuel: request.fuel,
        memory_bytes: request
            .mem_limit
            .checked_mul(1024 * 1024)
            .context("memory limit is too large")?,
        session_policy: policy,
        test_time_millis: request.test_time_millis,
        test_locale: request.test_locale,
        test_timezone: request.test_timezone,
        app_args: request.app_args,
        max_http_response_bytes: request.max_http_response_bytes,
        default_http_timeout_millis: match request.http_timeout_millis {
            0 => None,
            millis => Some(millis),
        },
        sandbox_root: request.sandbox_root,
        bundle_assets_root: bundle
            .as_ref()
            .and_then(|bundle| bundle.assets_path().map(Path::to_path_buf)),
        // Keyed on the app's declared id, so its data follows the app rather
        // than the file: renaming or moving the `.krate` keeps the same store,
        // and two different apps can never read each other's.
        app_store_path: manifest.map(|manifest| app_store_path(&manifest.app.id)),
        app_database_path: manifest.map(|manifest| app_database_path(&manifest.app.id)),
        app_secrets: manifest.map(|manifest| {
            (
                app_secrets_path(&manifest.app.id),
                manifest.app.id.clone(),
                machine_key(),
            )
        }),
        phase3_ui_mode: request.ui_mode,
        screenshot_path: request.screenshot_path.clone(),
        screenshot_scale: request.screenshot_scale,
        usability_plan: request.usability_report.as_ref().map(|path| {
            krate_runtime::usability::UsabilityPlan {
                report_path: path.clone(),
                check_resize: true,
                check_click: true,
                check_stay_open: true,
            }
        }),
    };
    let runtime = Runtime::new(&config)?;
    let runtime_world = match manifest.map(Manifest::app_world).transpose()? {
        Some(AppWorld::Phase2Cli) => RuntimeWorld::Cli,
        Some(AppWorld::Phase3Gui) => RuntimeWorld::Gui,
        None => RuntimeWorld::Auto,
    };

    if request.json {
        let started = std::time::Instant::now();
        let (exit, stdout, cli_code) =
            match runtime.run_file_captured_for_world(&request.file, &config, runtime_world) {
                Ok((RunOutcome::Exited(code), stdout)) => {
                    let cli_code = code.clamp(0, 255) as u8;
                    (RunJsonExit::exited(code), stdout, cli_code)
                }
                Ok((RunOutcome::LimitExceeded(message), stdout)) => {
                    (RunJsonExit::failure("limit-exceeded", &message), stdout, 4)
                }
                Err(RuntimeError::InvalidComponent(message)) => (
                    RunJsonExit::failure("invalid-component", &message),
                    Vec::new(),
                    2,
                ),
                Err(RuntimeError::Trap(message)) => {
                    (RunJsonExit::failure("trap", &message), Vec::new(), 3)
                }
                Err(err) => return Err(err.into()),
            };
        let duration_ms = started.elapsed().as_millis();
        print_run_json(
            manifest,
            &config.session_policy,
            exit,
            Some(duration_ms),
            &String::from_utf8_lossy(&stdout),
        );
        return Ok(cli_code);
    }

    match runtime.run_file_for_world(&request.file, &config, runtime_world) {
        Ok(RunOutcome::Exited(code)) => Ok(code.clamp(0, 255) as u8),
        Ok(RunOutcome::LimitExceeded(message)) => {
            eprintln!("limit exceeded: {message}");
            Ok(4)
        }
        Err(RuntimeError::InvalidComponent(message)) => {
            eprintln!("invalid wasm component: {message}");
            Ok(2)
        }
        Err(RuntimeError::Trap(message)) => {
            eprintln!("trap: {message}");
            Ok(3)
        }
        Err(err) => Err(err.into()),
    }
}

/// Exit portion of the `krate run --json` payload (schema krate.run.v1).
struct RunJsonExit {
    code: Option<i32>,
    class: &'static str,
    message: Option<String>,
    denied: Vec<String>,
}

impl RunJsonExit {
    fn exited(code: i32) -> Self {
        let class = match code {
            0 => "success",
            5 => "permission-denied",
            _ => "app-error",
        };
        Self {
            code: Some(code),
            class,
            message: None,
            denied: Vec::new(),
        }
    }

    fn failure(class: &'static str, message: &str) -> Self {
        Self {
            code: None,
            class,
            message: Some(message.to_string()),
            denied: Vec::new(),
        }
    }

    fn denied_before_run(missing: &[Capability]) -> Self {
        Self {
            code: Some(5),
            class: "permission-denied",
            message: Some("missing required capabilities".to_string()),
            denied: missing.iter().map(|cap| cap.to_string()).collect(),
        }
    }
}

/// Print the krate.run.v1 JSON object describing one run.
fn print_run_json(
    manifest: Option<&Manifest>,
    policy: &SessionPolicy,
    exit: RunJsonExit,
    duration_ms: Option<u128>,
    stdout: &str,
) {
    let app = manifest.map(|manifest| {
        serde_json::json!({
            "id": manifest.app.id,
            "name": manifest.app.name,
            "version": manifest.app.version,
            "world": manifest.app.world,
        })
    });
    let granted: Vec<String> = policy.grants().iter().map(|cap| cap.to_string()).collect();

    // A denial that only says "no" is a dead end for an agent, which then has
    // to guess that the fix is re-issuing the same call with the refused
    // strings added. Hand it the exact remedy instead, so a refusal is a step
    // rather than a stop.
    let remedy = (!exit.denied.is_empty()).then(|| {
        serde_json::json!({
            "action": "grant-and-retry",
            "grants": exit.denied,
            "note": "Re-run with these capability strings in `grants` to proceed. \
                    Each one is narrow: granting it allows only what it names.",
        })
    });

    let payload = serde_json::json!({
        "schema": "krate.run.v1",
        "app": app,
        "capabilities": {
            "granted": granted,
            "denied": exit.denied,
        },
        "exit": {
            "code": exit.code,
            "class": exit.class,
            "message": exit.message,
        },
        "remedy": remedy,
        "duration_ms": duration_ms,
        "stdout": stdout,
    });

    println!("{payload}");
}

fn validate_app_args(app_args: &[String]) -> Result<()> {
    if app_args.len() > MAX_PHASE2_ARG_COUNT {
        anyhow::bail!(
            "app arguments exceed count limit ({} arguments)",
            MAX_PHASE2_ARG_COUNT
        );
    }

    let mut encoded_len = 0usize;
    for arg in app_args {
        if arg.is_empty() {
            anyhow::bail!("app arguments cannot contain empty values in Phase 2 raw args");
        }
        if arg.contains('\n') || arg.contains('\0') {
            anyhow::bail!(
                "app arguments cannot contain newline or NUL characters in Phase 2 raw args"
            );
        }
        encoded_len += arg.len() + 1;
        if encoded_len > MAX_PHASE2_ARGS_RAW_BYTES {
            anyhow::bail!(
                "app arguments exceed raw args limit ({} bytes)",
                MAX_PHASE2_ARGS_RAW_BYTES
            );
        }
    }

    Ok(())
}

/// The Krate.app entry point (P3-OPEN-03): receive the document Finder asked
/// us to open, then run it through the ordinary consent + native-window flow.
/// The sandbox root is the folder the `.krate` sits in, so an app that writes
/// `./notes/**` keeps its data in a folder next to the document — visible,
/// understandable, and identical to running it from a terminal in that folder.
#[cfg(target_os = "macos")]
fn open_app() -> Result<u8> {
    // A document that arrives while this instance is already running an app
    // (double-click in Finder mid-session) gets its own process, so every
    // opened .krate behaves like its own application.
    let late_open = Box::new(|path: PathBuf| {
        spawn_open_run(&path);
    });
    let opened = krate_adapter_macos::wait_for_opened_documents(late_open)
        .map_err(|error| anyhow::anyhow!("waiting for the opened document failed: {error}"))?;
    // AppKit also feeds process arguments through application:openFiles:, so
    // our own subcommand name can arrive as a "document". Only paths that
    // actually exist on disk are documents.
    let opened: Vec<PathBuf> = opened.into_iter().filter(|path| path.exists()).collect();
    // Launched with no document (Krate.app opened directly): offer the native
    // picker instead of dying silently — there is no terminal to print to.
    let picked;
    let target = match opened.first() {
        Some(target) => target,
        None => {
            match krate_adapter_macos::choose_document()
                .map_err(|error| anyhow::anyhow!("the document picker failed: {error}"))?
            {
                Some(path) => {
                    picked = path;
                    &picked
                }
                // Cancelled: quitting quietly is the correct outcome.
                None => return Ok(0),
            }
        }
    };
    let sandbox_root = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Several documents opened at once: the first runs in this process, the
    // rest each get their own.
    for extra in opened.iter().skip(1) {
        spawn_open_run(extra);
    }

    run_component(RunRequest {
        target: target.display().to_string(),
        file: PathBuf::new(),
        insecure_http: false,
        fuel: None,
        mem_limit: 256,
        max_http_response_bytes: DEFAULT_MAX_HTTP_RESPONSE_BYTES,
        http_timeout_millis: DEFAULT_HTTP_TIMEOUT_MILLIS,
        sandbox_root,
        manifest_path: None,
        grants: Vec::new(),
        auto_grant: false,
        prompt: false,
        consent: true,
        ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
        json: false,
        dump_caps: false,
        dump_caps_format: OutputFormat::Text,
        log_grants: None,
        log_grants_format: GrantLogFormat::Text,
        test_time_millis: None,
        test_locale: None,
        test_timezone: None,
        screenshot_path: None,
        screenshot_scale: 2.0,
        usability_report: None,
        app_args: Vec::new(),
    })
}

/// Run one opened document in its own process, mirroring what open_app does
/// for the first document. Fire-and-forget: the child owns its own consent
/// window and lifetime.
#[cfg(target_os = "macos")]
fn spawn_open_run(path: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let sandbox_root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = ProcessCommand::new(exe)
        .arg("run")
        .arg(path)
        .arg("--consent")
        .arg("--native-window")
        .arg("--sandbox-root")
        .arg(sandbox_root)
        .spawn();
}

/// Render a capability as a short plain phrase for a person, e.g.
/// `fs.write:./checklist/**` -> "save files in ./checklist". Falls back to the
/// technical form for anything without a friendly phrasing, so the label is
/// always at least as informative as the raw capability. The exact capability
/// is still shown alongside this in prompts and denials.
fn human_label(cap: &Capability) -> String {
    let resource = cap.resource();
    match (cap.module(), cap.action()) {
        ("fs", "read") => match resource {
            Some(r) => format!("read files in {}", tidy_resource(r)),
            None => "read files".to_string(),
        },
        ("fs", "write") => match resource {
            Some(r) => format!("save files in {}", tidy_resource(r)),
            None => "save files".to_string(),
        },
        ("fs", "list") => match resource {
            Some(r) => format!("see the list of files in {}", tidy_resource(r)),
            None => "see the list of files".to_string(),
        },
        ("fs", "mkdir") => match resource {
            Some(r) => format!("create folders in {}", tidy_resource(r)),
            None => "create folders".to_string(),
        },
        ("net", "connect") => match resource {
            Some(r) => format!("connect to {r} over the network"),
            None => "connect over the network".to_string(),
        },
        ("ui", "window") => "open a window on your screen".to_string(),
        ("ui", "clipboard") if cap.resource() == Some("read") => {
            "read from the clipboard".to_string()
        }
        ("ui", "clipboard") if cap.resource() == Some("write") => {
            "copy to the clipboard".to_string()
        }
        // Say what the app gains, not how it is stored. "read files in
        // checklist" described the mechanism and made saving a preference sound
        // like reading the user's folders.
        ("store", "kv") => "save its own settings and data".to_string(),
        ("store", "sql") => "keep its own database".to_string(),
        ("store", "secret") => "save sign-in details for itself".to_string(),
        // Says what the app does with it, not where the bytes come from.
        // Someone reading a permission list wants to know an app rolls dice or
        // generates a key, not that it reads an entropy pool.
        ("random", "bytes") => "use random numbers".to_string(),
        ("ui", "open-url") => "open links in your browser".to_string(),
        ("ui", "notify") => "send you notifications".to_string(),
        ("audio", "capture") => "listen through your microphone".to_string(),
        ("audio", "playback") => "play sound through your speakers".to_string(),
        ("time", "clock") => "read the current time".to_string(),
        ("io", "stdout") => "print output".to_string(),
        // Unknown module/action: the technical form is the honest fallback.
        _ => cap.to_string(),
    }
}

/// Trim a filesystem capability resource to something readable: drop a trailing
/// glob so `./checklist/**` reads as `./checklist`.
fn tidy_resource(resource: &str) -> String {
    let trimmed = resource
        .trim_end_matches("**")
        .trim_end_matches('*')
        .trim_end_matches('/');
    if trimmed.is_empty() {
        resource.to_string()
    } else {
        trimmed.to_string()
    }
}

fn prompt_for_session_grants(manifest: &Manifest, policy: &SessionPolicy) -> Result<SessionPolicy> {
    let prompt_caps = manifest
        .declared_capabilities()?
        .into_iter()
        .filter(|cap| !policy.allows(cap) && !cap.is_default_granted())
        .collect::<Vec<_>>();

    if prompt_caps.is_empty() {
        return Ok(policy.clone());
    }

    eprintln!("App: {} ({})", manifest.app.name, manifest.app.id);
    eprintln!("This app is asking to:");
    for (index, cap) in prompt_caps.iter().enumerate() {
        // Lead with the plain phrase; show the exact capability in parentheses
        // so a careful reader still sees precisely what is granted.
        eprintln!("  [{}] {} ({cap})", index + 1, human_label(cap));
        if let Some(request) = manifest.capabilities.iter().find(|request| {
            request
                .cap
                .parse::<Capability>()
                .ok()
                .as_ref()
                .is_some_and(|parsed| parsed == cap)
        }) {
            eprintln!("      {}", request.rationale);
        }
    }
    eprint!("Grant [A]ll / [N]one / numbers (for example 1,2): ");
    io::stderr().flush()?;

    let mut input = String::new();
    let read = io::stdin().read_line(&mut input)?;

    // read == 0 is EOF with nothing typed or piped: there was no terminal and
    // no answer to read — the double-click-with-no-terminal case. On Linux try
    // a graphical dialog before giving up; elsewhere fall through to "none".
    if read == 0 && input.is_empty() {
        #[cfg(target_os = "linux")]
        {
            if let Some(selected) = linux_graphical_consent(manifest, &prompt_caps)? {
                let grants = policy.grants().iter().cloned().chain(selected);
                return Ok(SessionPolicy::from_grants(grants));
            }
            // A dialog ran and was declined, or none was available (a message
            // was printed): grant nothing, and the run is refused downstream.
            return Ok(policy.clone());
        }
    }

    let selected = parse_grant_response(input.trim(), &prompt_caps)?;
    let grants = policy.grants().iter().cloned().chain(selected);

    Ok(SessionPolicy::from_grants(grants))
}

/// Ask for missing capabilities in a native consent window instead of the
/// terminal. This is the path a double-clicked `.krate` takes, where there is
/// no terminal to answer. It mirrors `prompt_for_session_grants` exactly — same
/// filter to non-default missing caps, same `SessionPolicy::from_grants` fold —
/// so the native and terminal paths cannot diverge in what they enforce.
///
/// The rich window is macOS-only for now (founder decision, 2026-07-23). On
/// other platforms this falls back to the terminal prompt, so a `--consent` run
/// there still works; a portable window is a later P3-OPEN slice.
fn consent_for_session_grants(
    manifest: &Manifest,
    policy: &SessionPolicy,
) -> Result<SessionPolicy> {
    let consent_caps = manifest
        .declared_capabilities()?
        .into_iter()
        .filter(|cap| !policy.allows(cap) && !cap.is_default_granted())
        .collect::<Vec<_>>();

    if consent_caps.is_empty() {
        return Ok(policy.clone());
    }

    let requests = consent_caps
        .iter()
        .map(|cap| {
            let request = manifest.capabilities.iter().find(|request| {
                request
                    .cap
                    .parse::<Capability>()
                    .ok()
                    .as_ref()
                    .is_some_and(|parsed| parsed == cap)
            });
            ConsentCapability {
                cap: cap.clone(),
                display: cap.to_string(),
                rationale: request
                    .map(|request| request.rationale.clone())
                    .unwrap_or_default(),
                required: request.map(|request| request.required).unwrap_or(true),
            }
        })
        .collect::<Vec<_>>();

    match show_consent_window(&manifest.app.name, &manifest.app.id, &requests)? {
        ConsentOutcome::Allowed(selected) => {
            let grants = policy.grants().iter().cloned().chain(selected);
            Ok(SessionPolicy::from_grants(grants))
        }
        // Cancel leaves the policy unchanged; the missing-required check that
        // follows this call then refuses the run with the standard denial.
        ConsentOutcome::Cancelled => Ok(policy.clone()),
        // No native window on this platform: fall back to the terminal prompt.
        // The prompt reads stdin, so an interactive tty and a piped answer
        // (`printf 'A\n' | krate run ... --consent`, and CI) both work. Only
        // when the prompt gets no input at all — a double-clicked bundle with
        // no terminal — does it try a graphical dialog on Linux.
        ConsentOutcome::Unsupported => prompt_for_session_grants(manifest, policy),
    }
}

/// Ask for consent through a graphical dialog on Linux, for the case where a
/// `.krate` was double-clicked and there is no terminal to prompt in. Uses
/// `zenity` or `kdialog` if either is installed. Returns:
/// - `Ok(Some(caps))` if the user approved (all requested caps granted),
/// - `Ok(None)` if a dialog ran and the user declined,
/// - and, when no dialog tool is available, prints a clear next step and
///   returns `Ok(None)` so the run is refused rather than hanging.
///
/// This is deliberately all-or-nothing (approve everything or nothing): the
/// per-capability window is the macOS native one, and a plain yes/no dialog is
/// the honest shape of what zenity/kdialog can show without more machinery.
#[cfg(target_os = "linux")]
fn linux_graphical_consent(
    manifest: &Manifest,
    consent_caps: &[Capability],
) -> Result<Option<Vec<Capability>>> {
    let mut lines = vec![format!("{} wants permission to:", manifest.app.name)];
    for cap in consent_caps {
        lines.push(format!("  • {}", human_label(cap)));
    }
    lines.push(String::new());
    lines.push("Allow this app to run?".to_string());
    let message = lines.join("\n");

    if has_tool("zenity", &["--version"]) {
        let status = ProcessCommand::new("zenity")
            .args(["--question", "--title", "Krate", "--text", &message])
            .status();
        if let Ok(status) = status {
            // zenity --question exits 0 for Yes, non-zero for No/close.
            return Ok(status.success().then(|| consent_caps.to_vec()));
        }
    }

    if has_tool("kdialog", &["--version"]) {
        let status = ProcessCommand::new("kdialog")
            .args(["--title", "Krate", "--yesno", &message])
            .status();
        if let Ok(status) = status {
            return Ok(status.success().then(|| consent_caps.to_vec()));
        }
    }

    // No terminal and no dialog tool: don't hang on a prompt nobody can answer.
    // Point the user at the way that always works.
    eprintln!("This app needs your permission, but there is no terminal or dialog to ask in.");
    eprintln!("Run it from a terminal to review and allow it:");
    eprintln!("  krate run <the .krate file> --consent");
    eprintln!("Or install `zenity` (or `kdialog`) so double-click can ask you.");
    Ok(None)
}

/// One capability shown in the consent window.
///
/// Off macOS the native window does not exist, so nothing reads these fields
/// there — the terminal fallback re-derives what it needs from the manifest.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct ConsentCapability {
    cap: Capability,
    display: String,
    rationale: String,
    required: bool,
}

/// What the user decided in the consent window.
///
/// Which variants are constructed depends on the platform: macOS builds return
/// `Allowed`/`Cancelled` from the native window and never `Unsupported`, while
/// non-macOS builds only ever return `Unsupported`. Rather than cfg each
/// variant, allow the unused ones per build — every variant is live on some
/// platform.
#[allow(dead_code)]
enum ConsentOutcome {
    /// They pressed Open; the vec is the capabilities they allowed.
    Allowed(Vec<Capability>),
    /// They pressed Cancel; nothing is granted and the run is refused.
    Cancelled,
    /// This platform has no native consent window; caller should fall back.
    Unsupported,
}

#[cfg(target_os = "macos")]
fn show_consent_window(
    app_name: &str,
    app_id: &str,
    requests: &[ConsentCapability],
) -> Result<ConsentOutcome> {
    let items = requests
        .iter()
        .map(|item| krate_adapter_macos::ConsentItem {
            display: item.display.clone(),
            rationale: item.rationale.clone(),
            required: item.required,
        })
        .collect::<Vec<_>>();

    match krate_adapter_macos::present_consent_window(app_name, app_id, &items)? {
        krate_adapter_macos::ConsentChoice::Open(allowed_indices) => {
            let selected = allowed_indices
                .into_iter()
                .filter_map(|index| requests.get(index).map(|item| item.cap.clone()))
                .collect();
            Ok(ConsentOutcome::Allowed(selected))
        }
        krate_adapter_macos::ConsentChoice::Cancel => Ok(ConsentOutcome::Cancelled),
    }
}

#[cfg(not(target_os = "macos"))]
fn show_consent_window(
    _app_name: &str,
    _app_id: &str,
    _requests: &[ConsentCapability],
) -> Result<ConsentOutcome> {
    // No native window off macOS yet; the caller falls back to the terminal
    // prompt. This arm keeps the Linux and Windows builds compiling and is
    // exercised on those CI lanes, closing the off-macOS-stub gap by design.
    Ok(ConsentOutcome::Unsupported)
}

fn parse_grant_response(input: &str, caps: &[Capability]) -> Result<Vec<Capability>> {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized == "n"
        || normalized == "no"
        || normalized == "none"
        || normalized == "s"
        || normalized == "skip"
    {
        return Ok(Vec::new());
    }

    if normalized == "a" || normalized == "all" || normalized == "y" || normalized == "yes" {
        return Ok(caps.to_vec());
    }

    let mut selected = Vec::new();
    for token in normalized
        .split([',', ' '])
        .filter(|token| !token.is_empty())
    {
        let index: usize = token
            .parse()
            .with_context(|| format!("invalid grant selection `{token}`"))?;
        if index == 0 {
            anyhow::bail!("grant selection `0` is out of range");
        }
        let cap = caps
            .get(index - 1)
            .with_context(|| format!("grant selection `{index}` is out of range"))?;
        if !selected.contains(cap) {
            selected.push(cap.clone());
        }
    }

    Ok(selected)
}

fn print_effective_capabilities(
    wasm_file: &Path,
    manifest: Option<&Manifest>,
    policy: &SessionPolicy,
    format: OutputFormat,
    digest: Option<krate_bundle::provenance::BundleDigest>,
) -> Result<()> {
    if format == OutputFormat::Json {
        let dump = RunCapsDump {
            wasm: wasm_file.display().to_string(),
            app: manifest.map(RunCapsApp::from_manifest),
            capabilities: policy.grants().iter().map(ToString::to_string).collect(),
            // The identity a registry or a reviewer would key on.
            digest: digest.as_ref().map(|d| d.digest.clone()),
            // What the app declares it needs, granted or not, so a tool reading
            // this sees the whole ask and not just the default grants.
            requested: manifest
                .map(|m| m.capabilities.iter().map(|c| c.cap.to_string()).collect())
                .unwrap_or_default(),
        };
        println!("{}", serde_json::to_string_pretty(&dump)?);
        return Ok(());
    }

    if let Some(digest) = &digest {
        // Printed before the capability list, because "is this the app I think
        // it is?" comes before "what does it want?".
        println!("Identity");
        println!("  - {}", digest.digest);
        println!();
    }
    println!("Effective capabilities");
    for cap in policy.grants() {
        println!("  - {cap}");
    }

    // The grants above are the ones a run starts with, which for a shared app
    // is everything except the interesting part. What someone inspecting a file
    // wants to know is what it will ask them for, so say that too -- otherwise
    // the listing shows no file access at all on an app whose whole point is
    // saving files, and the first prompt comes as a surprise.
    if let Some(manifest) = manifest {
        let asks: Vec<Capability> = manifest
            .required_capabilities()?
            .into_iter()
            .filter(|cap| !policy.grants().iter().any(|grant| grant == cap))
            .collect();
        if !asks.is_empty() {
            println!();
            println!("This app will ask for");
            for cap in &asks {
                println!("  - {} ({cap})", human_label(cap));
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct RunCapsDump {
    wasm: String,
    app: Option<RunCapsApp>,
    capabilities: Vec<String>,
    /// Content identity of the bundle, when the input was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    /// Everything the manifest declares, whether or not it is granted yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requested: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RunCapsApp {
    id: String,
    name: String,
    version: String,
    world: String,
}

impl RunCapsApp {
    fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            id: manifest.app.id.clone(),
            name: manifest.app.name.clone(),
            version: manifest.app.version.clone(),
            world: manifest.app.world.clone(),
        }
    }
}

fn write_grant_log(
    path: &Path,
    wasm_file: &Path,
    manifest: Option<&Manifest>,
    policy: &SessionPolicy,
    format: GrantLogFormat,
) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open grant log {}", path.display()))?;

    if format == GrantLogFormat::Jsonl {
        let record = GrantLogRecord {
            format_version: 1,
            event: "krate.grants",
            wasm: wasm_file.display().to_string(),
            app: manifest.map(RunCapsApp::from_manifest),
            capabilities: policy.grants().iter().map(ToString::to_string).collect(),
        };
        serde_json::to_writer(&mut file, &record)?;
        writeln!(file)?;
        return Ok(());
    }

    writeln!(file, "Krate grant log")?;
    writeln!(file, "wasm             {}", wasm_file.display())?;
    if let Some(manifest) = manifest {
        writeln!(file, "app id           {}", manifest.app.id)?;
        writeln!(file, "app name         {}", manifest.app.name)?;
        writeln!(file, "manifest world   {}", manifest.app.world)?;
    } else {
        writeln!(file, "app id           <no manifest>")?;
    }
    writeln!(file, "grants")?;
    for cap in policy.grants() {
        writeln!(file, "  - {cap}")?;
    }
    writeln!(file)?;

    Ok(())
}

#[derive(Debug, Serialize)]
struct GrantLogRecord {
    format_version: u8,
    event: &'static str,
    wasm: String,
    app: Option<RunCapsApp>,
    capabilities: Vec<String>,
}

struct LoadedManifest {
    manifest: Manifest,
    path: PathBuf,
}

fn load_run_manifest(file: &Path, manifest_path: Option<&Path>) -> Result<Option<LoadedManifest>> {
    if let Some(path) = manifest_path {
        return Ok(Some(LoadedManifest {
            manifest: Manifest::parse_file(path)?,
            path: path.to_path_buf(),
        }));
    }

    let Some(parent) = file.parent() else {
        return Ok(None);
    };

    let candidate = parent.join("manifest.toml");
    if candidate.exists() {
        Ok(Some(LoadedManifest {
            manifest: Manifest::parse_file(&candidate)?,
            path: candidate,
        }))
    } else {
        Ok(None)
    }
}

fn manifest_entry_matches(file: &Path, loaded: &LoadedManifest) -> Result<bool> {
    let manifest_dir = loaded
        .path
        .parent()
        .context("manifest path has no parent directory")?;
    let expected = if loaded.manifest.app.entry.is_absolute() {
        loaded.manifest.app.entry.clone()
    } else {
        manifest_dir.join(&loaded.manifest.app.entry)
    };

    let file = std::fs::canonicalize(file)?;
    let Ok(expected) = std::fs::canonicalize(expected) else {
        return Ok(false);
    };

    Ok(file == expected)
}

fn print_version() {
    // KRATE_VERSION, not CARGO_PKG_VERSION: a released binary is stamped with
    // its tag, and reporting the crate's in-repo `-dev` version here made
    // `krate version` contradict `krate --version` on the very same binary.
    println!("krate   {KRATE_VERSION}");
    println!("wasmtime  43.0.2");
    println!("rustc     {}", env!("KRATE_RUSTC_VERSION"));
    println!("commit    {}", env!("KRATE_GIT_SHA"));
}

fn doctor() -> Result<u8> {
    println!("Krate doctor");
    println!("--------------");
    println!("Core tools");
    print_tool_status("cargo-component", &["--version"]);
    print_target_status("wasm32-wasip1")?;
    print_target_status("wasm32-wasip2")?;
    println!();
    println!("Phase 2 language tools");
    print_tool_status("wasm-tools", &["--version"]);
    print_tool_status("tinygo", &["version"]);
    print_tool_status("go", &["version"]);
    print_tool_status("node", &["--version"]);
    print_tool_status("npm", &["--version"]);
    print_jco_status();
    println!();
    print_rust_toolchain_status();
    println!();
    // Show the state dir, and say whether it exists. It is created lazily on
    // first use (the key-value store, the machine key), so on a fresh install
    // this path does not exist yet -- printing it bare read as though doctor
    // was pointing at a missing folder. Name the state as well as the path.
    let home = krate_home();
    let home_state = if home.exists() {
        "exists"
    } else {
        "not created yet (made on first use)"
    };
    println!("state dir       {} ({home_state})", home.display());
    Ok(0)
}

/// Report which `cargo`/`rustc` a plain build would use, and warn when it is not
/// rustup's.
///
/// `krate create` prepends rustup's toolchain itself, so its own builds are
/// safe. But someone building a library or CLI with plain `cargo` -- which is
/// most of the point of a component you hand to Krate -- uses whatever is first
/// on PATH. On a Mac with `brew install rust`, that is a Homebrew cargo with no
/// `wasm32-wasip1` target, and the build dies with "can't find crate for core".
/// Doctor used to check the target but never say which cargo it checked, so a
/// Homebrew or distro Rust got a green light straight into that wall. Name the
/// cargo, and warn when it is not the rustup one.
fn print_rust_toolchain_status() {
    println!("Rust toolchain");

    let path_cargo = find_on_path("cargo");
    match &path_cargo {
        Some(path) => println!("  cargo (PATH)  {}", path.display()),
        None => println!("  cargo (PATH)  not found"),
    }
    let path_rustc = find_on_path("rustc");
    if let Some(path) = &path_rustc {
        println!("  rustc (PATH)  {}", path.display());
    }

    match rustup_toolchain_bin() {
        Some(rustup_bin) => {
            let rustup_cargo = rustup_bin.join("cargo");
            println!("  rustup cargo  {}", rustup_cargo.display());
            // Warn when the cargo a plain build would pick up is not the rustup
            // one. Compare the resolved cargo against rustup's bin dir.
            let path_is_rustup = path_cargo
                .as_deref()
                .and_then(Path::parent)
                .map(|dir| dir == rustup_bin)
                .unwrap_or(false);
            if !path_is_rustup {
                println!(
                    "  warning: the cargo first on your PATH is not rustup's. A plain \
                     `cargo build` for wasm32-wasip1 may fail with \"can't find crate \
                     for core\". Krate's own `krate create`/`krate build` use rustup's \
                     toolchain regardless, but if you build a library yourself, run it \
                     with rustup's cargo (e.g. `rustup run stable cargo build`)."
                );
            }
        }
        None => {
            println!("  rustup        not found");
            if path_cargo.is_some() {
                println!(
                    "  warning: cargo is on your PATH but rustup is not. Krate needs the \
                     wasm32-wasip1 target, which rustup manages. Install rustup from \
                     https://rustup.rs and add the target with `rustup target add \
                     wasm32-wasip1`."
                );
            }
        }
    }
}

/// The stage of `check-app` a failure happened in. Ordered as the check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckStage {
    /// Locating the app files (Cargo.toml, src/lib.rs, manifest.toml).
    Layout,
    /// Parsing manifest.toml.
    Manifest,
    /// cargo-component build.
    Build,
    /// The component imports only krate:* interfaces.
    Imports,
    /// A headless run with all grants.
    Run,
    /// Painting the app's first frame to a PNG.
    Shoot,
    /// Driving the app the way a person would: does it stay open, does it
    /// follow a resize, does pressing it do anything.
    Usability,
}

impl CheckStage {
    /// The exit code for a failure at this stage. Distinct per stage so an
    /// agent or CI can branch on *where* it failed without parsing text.
    fn exit_code(self) -> u8 {
        match self {
            CheckStage::Layout => 10,
            CheckStage::Manifest => 11,
            CheckStage::Build => 12,
            CheckStage::Imports => 13,
            CheckStage::Run => 14,
            CheckStage::Shoot => 15,
            CheckStage::Usability => 16,
        }
    }

    fn label(self) -> &'static str {
        match self {
            CheckStage::Layout => "layout",
            CheckStage::Manifest => "manifest",
            CheckStage::Build => "build",
            CheckStage::Imports => "imports",
            CheckStage::Run => "run",
            CheckStage::Shoot => "shoot",
            CheckStage::Usability => "usability",
        }
    }
}

/// A single failure from `check-app`: which stage, what went wrong, and the
/// concrete next action. `fix` is the load-bearing field for an AI author --
/// it turns an opaque failure into an instruction it can act on.
struct CheckFailure {
    stage: CheckStage,
    /// What went wrong, in the tool's own words (compiler output, the leaking
    /// imports, the failing exit code).
    detail: String,
    /// The concrete thing to do about it. Empty when the detail is already the
    /// whole story.
    fix: String,
}

/// Put the `krate` dependency back into a `#![no_std]` app that lost it.
///
/// Returns whether it changed anything. This is a repair, not a check, and it
/// exists because telling an author the rule demonstrably does not work: the
/// context pack says to keep the dependency and authors still drop it while
/// converting an app to `no_std`. The failure it causes -- "no global memory
/// allocator" / "`#[panic_handler]` function required" -- names neither the
/// dependency nor the file it belongs in, so the author usually reaches for a
/// hand-written allocator and panic handler instead, which is the wrong answer
/// twice over.
///
/// Deliberately narrow. It only acts when all three are true:
///   - `src/lib.rs` is `#![no_std]` (a std guest genuinely does not need the dep)
///   - `Cargo.toml` has a `[dependencies]` table with no `krate` entry
///   - the manifest's WIT target path tells us where the SDK lives
///
/// That last condition is what keeps this honest: the path is taken from the
/// `[package.metadata.component.target]` entry cargo-component already resolves
/// against, so the restored dependency points at the same SDK the bindings come
/// from, and we never guess a path.
fn restore_krate_dependency(dir: &Path) -> bool {
    let cargo_path = dir.join("Cargo.toml");
    let Ok(cargo) = fs::read_to_string(&cargo_path) else {
        return false;
    };
    // Already there: nothing to do. Match the key at the start of a line so a
    // `krate` mentioned inside a comment or a path string does not count.
    if cargo.lines().any(|line| {
        line.trim_start().starts_with("krate ") || line.trim_start().starts_with("krate=")
    }) {
        return false;
    }
    let Ok(lib) = fs::read_to_string(dir.join("src/lib.rs")) else {
        return false;
    };
    if !lib.contains("#![no_std]") {
        return false;
    }
    let Some(sdk_prefix) = sdk_prefix_from_cargo(&cargo) else {
        return false;
    };
    let Some(deps_at) = cargo.find("\n[dependencies]\n") else {
        return false;
    };
    let insert_at = deps_at + "\n[dependencies]\n".len();
    let line = format!(
        "# Restored by `krate check-app`: a #![no_std] guest cannot link without the\n\
         # SDK, which owns the global allocator, the panic handler, and the mem*\n\
         # intrinsics. Do not remove it.\n\
         krate = {{ path = \"{sdk_prefix}/crates/bindings-rust\" }}\n"
    );
    let mut repaired = String::with_capacity(cargo.len() + line.len());
    repaired.push_str(&cargo[..insert_at]);
    repaired.push_str(&line);
    repaired.push_str(&cargo[insert_at..]);
    fs::write(&cargo_path, repaired).is_ok()
}

/// The SDK root a generated Cargo.toml points at, read back out of its WIT
/// target path. The template writes `<prefix>/wit/krate/phaseN`, so the prefix
/// is everything before `/wit/`.
fn sdk_prefix_from_cargo(cargo: &str) -> Option<String> {
    for line in cargo.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("path") {
            continue;
        }
        let value = trimmed.split('"').nth(1)?;
        if let Some(prefix) = value.split("/wit/").next() {
            if prefix != value && !prefix.is_empty() {
                return Some(prefix.to_string());
            }
        }
    }
    None
}

/// Turn a failed build into a specific fix when we recognize the failure.
///
/// A raw cargo dump is the least useful thing to hand an author that is already
/// looping on the build. Two failures are common enough and cryptic enough to
/// be worth naming: a `no_std` guest missing the SDK's lang items, and a `std`
/// guest whose bindings were built with `std_feature` off.
fn build_fix(detail: &str) -> String {
    let generic = "Fix the compiler errors above. The build uses rustup's toolchain and the \
                   wasm32-wasip1 target; run `krate doctor` if the target or toolchain looks \
                   wrong.";
    // The no_std guest with no SDK. rustc reports the missing lang items one at
    // a time, so match any of them rather than one exact string.
    if detail.contains("`#[panic_handler]` function required")
        || detail.contains("no global memory allocator")
        || detail.contains("no #[default_lib_allocator]")
        || detail.contains("undefined symbol: memcpy")
    {
        return "This is a `#![no_std]` guest with no allocator, panic handler, or memory \
                intrinsics. Do NOT write your own -- add the SDK, which provides all three:\n\
                \u{20}\u{20}- put `krate = { path = \"<sdk>/crates/bindings-rust\" }` under \
                `[dependencies]` in Cargo.toml (copy the path prefix from the \
                `[package.metadata.component.target]` entry already there)\n\
                \u{20}\u{20}- add `extern crate krate as _krate_runtime;` near the top of \
                src/lib.rs so the crate is linked even when nothing calls it\n\
                apps/krate-notes is a shipped GUI app that does exactly this."
            .to_string();
    }
    // A std guest whose bindings were gated behind std_feature.
    if detail.contains("failed to load bitcode of module std") {
        return "The guest links `std` but the generated bindings were built without it. \
                Either make the guest `#![no_std]` (the usual answer -- see \
                KRATE_AUTHORING.md section 3), or keep std and add `features = [\"std\"]` to \
                the `krate` dependency in Cargo.toml."
            .to_string();
    }
    generic.to_string()
}

/// Turn a set of non-krate imports into a specific, actionable fix.
///
/// This is the piece that makes `check-app` an oracle rather than a linter: a
/// leaked `wasi:*` import is never the real problem, it is a symptom of the
/// guest linking std or hitting a panic/alloc path. Name the symptom, name the
/// cause, and name the cure -- including the getrandom case, which has its own
/// specific remedy (the SDK backend) rather than the general no_std one.
fn imports_fix(bad: &[String], app_dir: &Path) -> String {
    let has_wasi = bad.iter().any(|i| i.starts_with("wasi:"));
    if !has_wasi {
        // A non-wasi, non-krate import means the crate reached for a host API
        // Krate does not model at all -- not a leak, a genuine mismatch.
        return format!(
            "This component imports host APIs Krate does not provide: {}. \
             Krate offers only krate:* interfaces. Remove the dependency or \
             feature that needs these, or replace it with a krate:* capability \
             (io, fs, net, time, locale, random, resources, store, and the GUI \
             world's ui/gfx/audio/speech).",
            bad.join(", ")
        );
    }

    let wants_entropy = bad
        .iter()
        .any(|i| i.contains("random") || i.contains("getrandom"));
    let mut fix = String::from(
        "The guest linked std or hit a reachable panic/alloc path, which drags \
         in all of std's wasi:* imports and stops the component from \
         instantiating under Krate. Make the guest no_std:\n\
         \u{20}\u{20}- put `#![no_std]` at the top of src/lib.rs and add `extern crate alloc;`\n\
         \u{20}\u{20}- depend on the `krate` SDK (it owns the allocator, panic handler, and mem \
         intrinsics)\n\
         \u{20}\u{20}- set `std_feature = true` under `[package.metadata.component.bindings]` in \
         Cargo.toml so generated `impl std::error::Error` blocks do not force std\n\
         \u{20}\u{20}- avoid reachable panics: no `format!`, `.unwrap()`, `a[i]` indexing, or \
         growable-Vec realloc on a hostile path; use the SDK's string/number helpers instead",
    );
    if wants_entropy {
        fix.push_str(
            "\n\nOne of the leaked imports is entropy. A dependency here pulls getrandom \
             (rand, uuid, and much of the ecosystem do). Do not hand-shim it: add \
             `features = [\"getrandom-backend\"]` to the `krate` dependency, add a \
             `.cargo/config.toml` with `rustflags = [\"--cfg\", \"getrandom_backend=\\\"custom\\\"\"]`, \
             and declare the `random.bytes` capability in manifest.toml. The SDK then routes \
             every draw to the host. See apps/krate-diceroll for a working example.",
        );
    }
    let hints = panic_site_hints(app_dir);
    if !hints.is_empty() {
        fix.push_str("\n\nLikely panic/alloc sites (a grep, not a proof):");
        fix.push_str(&hints);
    }
    fix
}

/// Whether the app opens a window, from its manifest capabilities. A GUI app is
/// run with the `quick` token so it draws a frame and exits instead of blocking
/// on a window; a CLI app takes no such arg.
fn manifest_is_gui(manifest: &krate_manifest::Manifest) -> bool {
    manifest
        .capabilities
        .iter()
        .any(|cap| cap.cap.starts_with("ui.window"))
}

/// Build, import-check, and run an app directory, printing one verdict.
///
/// The feedback oracle: an AI author runs this after every change and fixes
/// whatever it reports until it prints OK. Stops at the first failing stage and
/// names both the cause and the fix. Reuses the exact primitives `create` uses
/// -- the rustup-pinned build, the import checker, a headless run -- so a green
/// verdict here means the same thing a successful `create` does.
fn check_app(dir: &Path, shoot: Option<&Path>, no_run: bool, json: bool) -> Result<u8> {
    let outcome = run_check_app(dir, shoot, no_run);
    match &outcome {
        Ok(summary) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "dir": dir.display().to_string(),
                        "stages": summary.passed,
                        "imports": summary.imports,
                        "shoot": summary.shoot.as_ref().map(|p| p.display().to_string()),
                        "usability_notes": summary.usability_notes,
                    })
                );
            } else {
                println!("OK");
                for stage in &summary.passed {
                    println!("  {stage} passed");
                }
                if let Some(shot) = &summary.shoot {
                    println!("  wrote {}", shot.display());
                }
                // Said out loud so a green result is never read as more than it
                // is: these are the usability checks that could not be made on
                // this app, not ones it passed.
                for note in &summary.usability_notes {
                    println!("  note: {note}");
                }
            }
            Ok(0)
        }
        Err(failure) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": false,
                        "dir": dir.display().to_string(),
                        "stage": failure.stage.label(),
                        "detail": failure.detail,
                        "fix": failure.fix,
                    })
                );
            } else {
                eprintln!("FAILED at {}", failure.stage.label());
                eprintln!();
                eprintln!("{}", failure.detail.trim_end());
                if !failure.fix.is_empty() {
                    eprintln!();
                    eprintln!("Fix:");
                    eprintln!("{}", failure.fix.trim_end());
                }
            }
            Ok(failure.stage.exit_code())
        }
    }
}

/// What a passing `check-app` recorded, for the OK summary.
struct CheckSummary {
    passed: Vec<&'static str>,
    imports: Vec<String>,
    shoot: Option<PathBuf>,
    /// What the usability stage could not measure, in plain words. Notes, not
    /// failures: a green check-app with a note here means the app passed
    /// everything that could be checked, and says which checks those were.
    usability_notes: Vec<String>,
}

fn run_check_app(
    dir: &Path,
    shoot: Option<&Path>,
    no_run: bool,
) -> std::result::Result<CheckSummary, CheckFailure> {
    let mut passed: Vec<&'static str> = Vec::new();
    let mut usability_notes: Vec<String> = Vec::new();

    // Stage: layout. The three files every Krate app has.
    let manifest_path = dir.join("manifest.toml");
    for (name, path) in [
        ("Cargo.toml", dir.join("Cargo.toml")),
        ("src/lib.rs", dir.join("src/lib.rs")),
        ("manifest.toml", manifest_path.clone()),
    ] {
        if !path.exists() {
            return Err(CheckFailure {
                stage: CheckStage::Layout,
                detail: format!(
                    "{} is not an app directory: {name} is missing",
                    dir.display()
                ),
                fix: "Point check-app at the folder that holds Cargo.toml, src/lib.rs, and \
                      manifest.toml."
                    .to_string(),
            });
        }
    }
    // Still layout: the `krate` dependency must survive whatever rewrote
    // Cargo.toml.
    //
    // This is the single most common way an authored app fails. An agent
    // converting the app to `#![no_std]` reasons that a no_std crate should not
    // depend on things, deletes the `krate` line, and the build then dies with
    // three errors that all point away from the cause: "can't find crate for
    // `krate`", "no global memory allocator found", "`#[panic_handler]`
    // function required". Measured 5 out of 5 authored apps hitting exactly
    // this. The context pack already says to keep it, in capitals; saying it
    // louder does not work, so catch it here and say precisely what to restore.
    //
    // Only for a `#![no_std]` app. A `std` guest links std's own allocator and
    // panic handler and does not need the SDK at all -- fifteen shipped apps
    // are exactly that, and failing them here was a false failure of my own
    // making, caught by W14 sweeping every app.
    let is_no_std = fs::read_to_string(dir.join("src/lib.rs"))
        .map(|lib| lib.contains("#![no_std]"))
        .unwrap_or(false);
    if let Ok(cargo) = fs::read_to_string(dir.join("Cargo.toml")) {
        let declares_krate = cargo
            .lines()
            .map(str::trim_start)
            .any(|line| line.starts_with("krate ") || line.starts_with("krate="));
        if is_no_std && !declares_krate {
            return Err(CheckFailure {
                stage: CheckStage::Layout,
                detail: "Cargo.toml has no `krate` dependency. The SDK owns this app's \
                         global allocator, panic handler, and memory intrinsics, so \
                         without it the build fails with \"can't find crate for `krate`\", \
                         \"no global memory allocator found\", and \"`#[panic_handler]` \
                         function required\" -- three errors with one cause."
                    .to_string(),
                fix: "Put the dependency back under [dependencies] in Cargo.toml, exactly \
                      as the starter had it:\n\n    krate = { path = \"<the path the \
                      starter used>\" }\n\nKeep it even when the app is `#![no_std]` -- \
                      especially then. no_std does not mean no dependencies; the SDK is \
                      what makes no_std work."
                    .to_string(),
            });
        }
    }
    passed.push("layout");

    // Stage: manifest. Parse it now so a bad manifest is named here, not as a
    // confusing run failure later.
    let manifest =
        krate_manifest::Manifest::parse_file(&manifest_path).map_err(|error| CheckFailure {
            stage: CheckStage::Manifest,
            detail: format!("manifest.toml did not parse: {error:#}"),
            fix: "Fix the manifest so it declares [app] (id, name, version, entry, world) and \
                  its [[capabilities]]. `krate manifest check manifest.toml` explains the shape."
                .to_string(),
        })?;
    passed.push("manifest");

    // Stage: build. Reuses component_build_command -> rustup toolchain, so it is
    // immune to the Homebrew-cargo-shadows-rustup wall.
    //
    // Before building, repair the one thing we know the right answer to. An
    // author that converts the app to `#![no_std]` -- which section 3 of the
    // context pack tells it to do as soon as there is a real dependency --
    // routinely drops the `krate` dependency along the way, and then the link
    // fails with "no global memory allocator" or "`#[panic_handler]` function
    // required", neither of which names the missing dep. Telling it not to did
    // not work; the pack said so in capitals and it still went missing. So put
    // the line back instead of complaining about it.
    let restored_sdk_dep = restore_krate_dependency(dir);
    let wasm = build_component_captured(dir).map_err(|detail| CheckFailure {
        stage: CheckStage::Build,
        fix: build_fix(&detail),
        detail,
    })?;
    if restored_sdk_dep {
        // Not a failure -- the build passed -- but say it happened, so an author
        // reading the output learns the rule rather than silently relying on it.
        eprintln!(
            "note: src/lib.rs is #![no_std] but Cargo.toml had no `krate` dependency, so \
             check-app put it back. The SDK owns the allocator, panic handler, and memory \
             intrinsics a no_std guest needs; do not remove it."
        );
    }
    passed.push("build");

    // Stage: imports. The component must import only krate:*.
    let wasm_bytes = fs::read(&wasm).map_err(|error| CheckFailure {
        stage: CheckStage::Build,
        detail: format!(
            "could not read the built component at {}: {error}",
            wasm.display()
        ),
        fix: String::new(),
    })?;
    let bad =
        krate_bundle::imports::non_krate_imports(&wasm_bytes).map_err(|error| CheckFailure {
            stage: CheckStage::Imports,
            detail: format!("could not read the component's imports: {error}"),
            fix: String::new(),
        })?;
    if !bad.is_empty() {
        return Err(CheckFailure {
            stage: CheckStage::Imports,
            detail: format!("the component imports non-Krate APIs: {}", bad.join(", ")),
            fix: imports_fix(&bad, dir),
        });
    }
    let imports = krate_bundle::imports::component_imports(&wasm_bytes)
        .map(|set| set.into_iter().collect())
        .unwrap_or_default();
    passed.push("imports");

    if no_run {
        return Ok(CheckSummary {
            passed,
            imports,
            shoot: None,
            usability_notes,
        });
    }

    // Stage: run. Run the built component with its manifest, headless, all
    // grants -- exactly as `create` verifies an app, so a green check-app means
    // the same thing a successful create does. Every app (GUI and CLI) is given
    // the bare `quick` token: the verification convention every Krate app
    // follows is "do the work once and exit 0 on `quick`". The one exception is
    // a file-reading CLI app (declares fs.read:, no window), which needs a real
    // file path -- prepare_verify_dir seeds a fixture and returns its path.
    // Untrusted + a fuel budget so a runaway fails here rather than hanging.
    //
    // Absolute paths: run_self sets the child's cwd to `dir`, so a wasm or
    // manifest path relative to *our* cwd would resolve wrong inside the child
    // (doubling the dir prefix). Canonicalize both against the current dir first.
    let wasm_str = absolute_from_cwd(&wasm).to_string_lossy().into_owned();
    let manifest_str = absolute_from_cwd(&manifest_path)
        .to_string_lossy()
        .into_owned();
    let is_gui = manifest_is_gui(&manifest);
    // A scratch dir the run happens in, seeded for a file-reading CLI app so it
    // has a real fixture to work against. Matches create's verify.
    let verify_dir = tempfile::tempdir().map_err(|error| CheckFailure {
        stage: CheckStage::Run,
        detail: format!("could not create a scratch dir for the run: {error}"),
        fix: String::new(),
    })?;
    let verify_arg = prepare_verify_dir(verify_dir.path(), &manifest)
        .map_err(|error| CheckFailure {
            stage: CheckStage::Run,
            detail: format!("could not prepare the run: {error:#}"),
            fix: String::new(),
        })?
        .unwrap_or_else(|| "quick".to_string());
    let run_args: Vec<String> = vec![
        "run".into(),
        wasm_str.clone(),
        "--manifest".into(),
        manifest_str.clone(),
        "--untrusted".into(),
        "--auto-grant".into(),
        "--headless".into(),
        "--".into(),
        verify_arg,
    ];
    let run_arg_refs: Vec<&str> = run_args.iter().map(String::as_str).collect();
    let exit = run_self(verify_dir.path(), &run_arg_refs).map_err(|error| CheckFailure {
        stage: CheckStage::Run,
        detail: format!("could not run the app: {error:#}"),
        fix: String::new(),
    })?;
    if exit != 0 {
        let hint = match exit {
            4 => " (exit 4 means it exhausted its fuel budget -- a runaway or infinite loop)",
            5 => " (exit 5 means a capability it needs is not declared in manifest.toml)",
            _ => "",
        };
        return Err(CheckFailure {
            stage: CheckStage::Run,
            detail: format!("the app failed to run headless with all grants (exit {exit}){hint}"),
            fix: "Run it yourself to see its output: \
                  `krate run <wasm> --manifest manifest.toml --auto-grant`. Set \
                  KRATE_VERIFY_LOG=/path/to/log to capture what it printed. If it needs a \
                  capability, declare it in manifest.toml."
                .to_string(),
        });
    }
    passed.push("run");

    // Stage: shoot (optional). Paint the first frame to a PNG so a GUI app's
    // output can be seen.
    let shot = if let Some(png) = shoot {
        // Absolute so the PNG lands next to where the user ran check-app, not
        // inside the app dir the child cd's into.
        let png_str = absolute_from_cwd(png).to_string_lossy().into_owned();
        let mut shoot_args: Vec<String> = vec![
            "run".into(),
            wasm_str.clone(),
            "--manifest".into(),
            manifest_str.clone(),
            "--auto-grant".into(),
            "--shoot".into(),
            png_str,
        ];
        if is_gui {
            shoot_args.push("--".into());
            shoot_args.push("quick".into());
        }
        let shoot_refs: Vec<&str> = shoot_args.iter().map(String::as_str).collect();
        let shoot_exit = run_self(dir, &shoot_refs).map_err(|error| CheckFailure {
            stage: CheckStage::Shoot,
            detail: format!("could not paint the app's frame: {error:#}"),
            fix: String::new(),
        })?;
        if shoot_exit != 0 {
            return Err(CheckFailure {
                stage: CheckStage::Shoot,
                detail: format!("painting the app's frame failed (exit {shoot_exit})"),
                fix: "The app runs but did not render a frame. A CLI app with no window cannot \
                      be shot; drop --shoot for it."
                    .to_string(),
            });
        }
        Some(png.to_path_buf())
    } else {
        None
    };

    // Stage: usability. Every stage above asks whether the app is *valid*.
    // This one asks whether it is *usable*: does it stay open, does it follow
    // the window when that is resized, does pressing it do anything. Those are
    // the properties a person actually experiences, and an app that fails all
    // three passed every other stage green.
    //
    // Only a GUI app has a window, so a CLI app skips this outright rather than
    // failing it.
    if is_gui {
        let notes = run_usability_stage(dir, &wasm_str, &manifest_str)?;
        passed.push("usability");
        usability_notes = notes;
    }

    Ok(CheckSummary {
        passed,
        imports,
        shoot: shot,
        usability_notes,
    })
}

/// Drive the app and turn what was observed into either a failure or notes.
///
/// The rule that governs every line here: **only fail on what was actually
/// seen to break.** Anything the driver could not measure comes back as a note.
/// A stage that fails an app it merely failed to measure gets skipped with
/// `--skip`, and a skipped stage protects nothing.
fn run_usability_stage(
    dir: &Path,
    wasm_str: &str,
    manifest_str: &str,
) -> std::result::Result<Vec<String>, CheckFailure> {
    let scratch = tempfile::tempdir().map_err(|error| CheckFailure {
        stage: CheckStage::Usability,
        detail: format!("could not create a scratch dir for the usability run: {error}"),
        fix: String::new(),
    })?;
    let report_path = scratch.path().join("usability.json");
    let report_str = report_path.to_string_lossy().into_owned();

    // No `quick`: the whole point is to run the app the way a person does. On
    // the quick path an app does its scripted work and exits, which would make
    // every app look like one that closes by itself.
    let args: Vec<String> = vec![
        "run".into(),
        wasm_str.to_string(),
        "--manifest".into(),
        manifest_str.to_string(),
        "--auto-grant".into(),
        "--usability-report".into(),
        report_str,
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let exit = run_self(dir, &arg_refs).map_err(|error| CheckFailure {
        stage: CheckStage::Usability,
        detail: format!("could not drive the app: {error:#}"),
        fix: String::new(),
    })?;

    let report = match krate_runtime::usability::UsabilityReport::read(&report_path) {
        Ok(report) => report,
        Err(error) => {
            // No report at all means the driver never got far enough to write
            // one. That is a gap in the measurement, not proof of a defect, so
            // it is a note. The run stage above already covers "the app does
            // not run".
            return Ok(vec![format!(
                "could not observe this app (exit {exit}, no report: {error}); \
                 the usability checks did not run"
            )]);
        }
    };

    if !report.opened_window {
        return Ok(vec![
            "this app never opened a window, so there was nothing to drive".to_string(),
        ]);
    }

    // Failures first, in the order a person would notice them.
    use krate_runtime::usability::Observation;
    if let Some(Observation::Broke { detail }) = &report.stay_open {
        return Err(CheckFailure {
            stage: CheckStage::Usability,
            detail: detail.clone(),
            fix: "A window should stay open until the person closes it. If the app has an \
                  idle timeout so a headless check cannot hang, gate it on the `quick` \
                  argument so it never fires in a real session."
                .to_string(),
        });
    }
    if let Some(Observation::Broke { detail }) = &report.resize {
        return Err(CheckFailure {
            stage: CheckStage::Usability,
            detail: detail.clone(),
            fix: "Lay the app out from `canvas2d::canvas_size` rather than from constants, \
                  and redraw on `Event::Resized`. Hit-testing must use the same numbers the \
                  drawing does, or clicks land in the wrong place after a resize."
                .to_string(),
        });
    }
    if let Some(Observation::Broke { detail }) = &report.click {
        return Err(CheckFailure {
            stage: CheckStage::Usability,
            detail: detail.clone(),
            fix: "A control that is drawn should do something when it is pressed. Handle \
                  `Event::Pointer` with `pressed` set, work out what was hit, change the \
                  app's state, and redraw."
                .to_string(),
        });
    }

    // Everything else becomes a note, so a person can see what was and was not
    // actually measured rather than reading a green line as more than it is.
    let mut notes = Vec::new();
    for (name, observation) in [
        ("stays open", &report.stay_open),
        ("survives a resize", &report.resize),
        ("responds to a press", &report.click),
    ] {
        if let Some(Observation::Unobserved { reason }) = observation {
            notes.push(format!("{name}: not checked -- {reason}"));
        }
    }
    Ok(notes)
}

fn check_manifest(file: &Path, format: OutputFormat) -> Result<u8> {
    let manifest = Manifest::parse_file(file)?;
    let declared_caps = manifest.declared_capabilities()?;
    let required_caps = manifest.required_capabilities()?;

    if format == OutputFormat::Json {
        let summary = ManifestCheckSummary {
            ok: true,
            app: ManifestAppExplanation::from_manifest(&manifest),
            capabilities: declared_caps.len(),
            required_capabilities: required_caps.len(),
        };
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(0);
    }

    println!("Manifest OK");
    println!("app id          {}", manifest.app.id);
    println!("app name        {}", manifest.app.name);
    println!("entry           {}", manifest.app.entry.display());
    println!("world           {}", manifest.app.world);
    println!(
        "app type       {}",
        app_world_human_label(manifest.app_world()?)
    );
    println!("capabilities    {}", declared_caps.len());
    println!("required caps   {}", required_caps.len());

    Ok(0)
}

fn explain_manifest(file: &Path, format: OutputFormat) -> Result<u8> {
    let manifest = Manifest::parse_file(file)?;
    let declared_caps = manifest.declared_capabilities()?;

    if format == OutputFormat::Json {
        let explanation = ManifestExplanation::from_manifest(&manifest, &declared_caps);
        println!("{}", serde_json::to_string_pretty(&explanation)?);
        return Ok(0);
    }

    println!("Manifest");
    println!("app id          {}", manifest.app.id);
    println!("app name        {}", manifest.app.name);
    println!("version         {}", manifest.app.version);
    println!("entry           {}", manifest.app.entry.display());
    println!("world           {}", manifest.app.world);
    println!(
        "app type       {}",
        app_world_human_label(manifest.app_world()?)
    );
    println!();

    if declared_caps.is_empty() {
        println!("Capabilities");
        println!("  none declared");
        return Ok(0);
    }

    println!("Capabilities");
    for (request, cap) in manifest.capabilities.iter().zip(declared_caps) {
        let default_grant = cap.is_default_granted();
        println!("  - {}", cap);
        println!("    required             {}", yes_no(request.required));
        println!("    default grant        {}", yes_no(default_grant));
        println!(
            "    launch grant needed  {}",
            yes_no(request.required && !default_grant)
        );
        if let Some(resource) = cap.resource() {
            println!("    resource             {resource}");
        }
        println!("    rationale            {}", request.rationale);
    }

    Ok(0)
}

#[derive(Debug, Serialize)]
struct ManifestCheckSummary {
    ok: bool,
    app: ManifestAppExplanation,
    capabilities: usize,
    required_capabilities: usize,
}

#[derive(Debug, Serialize)]
struct ManifestExplanation {
    app: ManifestAppExplanation,
    capabilities: Vec<CapabilityExplanation>,
}

impl ManifestExplanation {
    fn from_manifest(manifest: &Manifest, declared_caps: &[Capability]) -> Self {
        let capabilities = manifest
            .capabilities
            .iter()
            .zip(declared_caps)
            .map(|(request, cap)| {
                let default_grant = cap.is_default_granted();
                CapabilityExplanation {
                    capability: cap.to_string(),
                    module: cap.module().to_string(),
                    action: cap.action().to_string(),
                    resource: cap.resource().map(ToOwned::to_owned),
                    required: request.required,
                    default_grant,
                    launch_grant_needed: request.required && !default_grant,
                    rationale: request.rationale.clone(),
                }
            })
            .collect();

        Self {
            app: ManifestAppExplanation::from_manifest(manifest),
            capabilities,
        }
    }
}

#[derive(Debug, Serialize)]
struct ManifestAppExplanation {
    id: String,
    name: String,
    version: String,
    entry: String,
    world: String,
    world_kind: String,
}

impl ManifestAppExplanation {
    fn from_manifest(manifest: &Manifest) -> Self {
        let world = manifest.app_world().expect("validated manifest world");
        Self {
            id: manifest.app.id.clone(),
            name: manifest.app.name.clone(),
            version: manifest.app.version.clone(),
            entry: manifest.app.entry.display().to_string(),
            world: manifest.app.world.clone(),
            world_kind: app_world_label(world).to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct CapabilityExplanation {
    capability: String,
    module: String,
    action: String,
    resource: Option<String>,
    required: bool,
    default_grant: bool,
    launch_grant_needed: bool,
    rationale: String,
}

fn init_manifest(request: ManifestInitRequest) -> Result<u8> {
    let capabilities = request
        .capabilities
        .iter()
        .map(|cap| {
            let cap: Capability = cap.parse()?;
            Ok(CapabilityRequest {
                cap: cap.to_string(),
                rationale: if cap.is_default_granted() {
                    "Default app capability".to_string()
                } else {
                    "Required by app".to_string()
                },
                required: true,
            })
        })
        .collect::<krate_manifest::Result<Vec<_>>>()?;

    let manifest = Manifest {
        app: App {
            id: request.id,
            name: request.name,
            version: request.version,
            entry: request.entry,
            world: PHASE2_CLI_WORLD.to_string(),
        },
        capabilities,
    };
    let rendered = manifest.to_toml_pretty()?;

    if let Some(output) = request.output {
        if output.exists() && !request.force {
            anyhow::bail!(
                "refusing to overwrite existing manifest: {} (pass --force to replace it)",
                output.display()
            );
        }
        std::fs::write(&output, rendered)
            .with_context(|| format!("failed to write manifest {}", output.display()))?;
        println!("wrote {}", output.display());
    } else {
        print!("{rendered}");
    }

    Ok(0)
}

fn print_manifest_capabilities(format: OutputFormat) -> Result<u8> {
    if format == OutputFormat::Json {
        let specs = supported_capability_specs()
            .iter()
            .map(|spec| CapabilitySpecExplanation {
                capability: spec.display_pattern(),
                module: spec.module().to_string(),
                action: spec.action().to_string(),
                resource: spec.resource().map(ToOwned::to_owned),
                default_grant: spec.default_granted(),
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&specs)?);
        return Ok(0);
    }

    println!("Krate capabilities");
    println!("capability                         default");
    for spec in supported_capability_specs() {
        println!(
            "{:<34} {}",
            spec.display_pattern(),
            if spec.default_granted() { "yes" } else { "no" }
        );
    }

    Ok(0)
}

/// A stable machine token for the app's world, used in the `--json` output's
/// `world_kind`. Kept as-is so the schema does not shift under consumers.
fn app_world_label(world: AppWorld) -> &'static str {
    match world {
        AppWorld::Phase2Cli => "Phase 2 CLI",
        AppWorld::Phase3Gui => "Phase 3 GUI draft",
    }
}

/// A user-facing name for the kind of app a manifest describes, for the human
/// `manifest explain` output. The internal phase/world terms stay out of it.
fn app_world_human_label(world: AppWorld) -> &'static str {
    match world {
        AppWorld::Phase2Cli => "Command-line app",
        AppWorld::Phase3Gui => "Graphical app",
    }
}

#[derive(Debug, Serialize)]
struct CapabilitySpecExplanation {
    capability: String,
    module: String,
    action: String,
    resource: Option<String>,
    default_grant: bool,
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

// ---- build toolchain: shared checks + create's preflight -------------------

/// The wasm target `krate create` compiles apps for.
const CREATE_WASM_TARGET: &str = "wasm32-wasip1";
/// The cargo-component version the samples and CI pin.
const CARGO_COMPONENT_VERSION: &str = "0.21.1";

/// Whether a program runs successfully (used as a presence check). Resolves the
/// same way `doctor` does, so PATH and `~/.cargo/bin` are both considered.
fn has_tool(program: &str, args: &[&str]) -> bool {
    let command = resolve_tool(program).unwrap_or_else(|| PathBuf::from(program));
    ProcessCommand::new(command)
        .args(args)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Whether a rustup target is installed. Returns None when rustup itself is
/// absent (we cannot tell), so callers can treat that distinctly.
fn has_rust_target(target: &str) -> Option<bool> {
    let output = ProcessCommand::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let installed = String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line == target);
    Some(installed)
}

/// One missing piece of the build toolchain, with how to install it.
struct MissingTool {
    what: &'static str,
    install_cmd: Vec<String>,
    note: &'static str,
}

/// Gather what `krate create` needs but does not have. Empty means ready.
/// What the front door needs before it starts a build.
///
/// The toolchain used to be discovered mid-run: the person picked an AI,
/// watched "cooking with grok", and only then found out a compiler was
/// missing -- and on Windows the install command was a Unix shell script that
/// could not work, so it failed with "curl: (23) Failure writing output".
/// Checking first means the answer arrives before anyone has waited.
pub(crate) fn build_tools_missing() -> Vec<(String, String)> {
    missing_create_tools()
        .into_iter()
        .map(|tool| {
            (
                tool.what.to_string(),
                install_command_line(&tool.install_cmd),
            )
        })
        .collect()
}

/// Install everything `build_tools_missing` reported, in order.
pub(crate) fn install_build_tools() -> Result<()> {
    for tool in missing_create_tools() {
        // Silent on purpose: a progress bar is drawing over this, and rustup
        // and winget both narrate at length. Their output is captured and
        // shown only on failure, where it is the thing worth reading.
        let out = run_install_command_quiet(&tool.install_cmd)
            .with_context(|| format!("install {}", tool.what))?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr);
            let detail = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .next_back()
                .unwrap_or("no reason given");
            anyhow::bail!("{}: {detail}", tool.what);
        }
    }
    Ok(())
}

/// The rustup toolchain name that carries its own linker on this machine.
///
/// `gnullvm` is Rust's Windows toolchain built around LLVM's linker rather
/// than Microsoft's `link.exe`. It is what lets a Windows user build an app
/// without installing three gigabytes of Visual Studio Build Tools.
#[cfg(windows)]
fn gnullvm_toolchain_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "stable-aarch64-pc-windows-gnullvm"
    } else {
        "stable-x86_64-pc-windows-gnullvm"
    }
}

/// Whether that toolchain is already installed.
#[cfg(windows)]
fn gnullvm_toolchain_present() -> bool {
    ProcessCommand::new("rustup")
        .args(["toolchain", "list"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("gnullvm"))
        .unwrap_or(false)
}

/// Whether the MSVC linker is reachable.
///
/// `link.exe` is not on PATH in an ordinary shell even when Build Tools are
/// installed -- it lives inside the VC toolchain and is normally put on PATH
/// by a developer prompt. Looking for the install root is the reliable check;
/// cargo finds the linker itself once the tools exist.
#[cfg(windows)]
fn msvc_linker_present() -> bool {
    if agent_provider::which_on_path("link.exe").is_some() {
        return true;
    }
    // vswhere ships with any modern Visual Studio installer and is the
    // supported way to ask whether the C++ tools are present.
    let vswhere = PathBuf::from(
        std::env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string()),
    )
    .join("Microsoft Visual Studio")
    .join("Installer")
    .join("vswhere.exe");
    if !vswhere.exists() {
        return false;
    }
    ProcessCommand::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

fn missing_create_tools() -> Vec<MissingTool> {
    let mut missing = Vec::new();

    let have_cargo = has_tool("cargo", &["--version"]);
    if !have_cargo {
        missing.push(MissingTool {
            what: "Rust (cargo)",
            // rustup is the supported installer; we print its official command
            // and only run it with consent.
            //
            // Windows needs a different one entirely. The shell script at
            // sh.rustup.rs cannot run there, and piping it produced
            // "curl: (23) Failure writing output to destination" -- a message
            // that tells a first-time user nothing about what went wrong.
            #[cfg(windows)]
            install_cmd: vec![
                "winget".into(),
                "install".into(),
                "--id".into(),
                "Rustlang.Rustup".into(),
                "-e".into(),
                "--accept-source-agreements".into(),
                "--accept-package-agreements".into(),
            ],
            #[cfg(not(windows))]
            install_cmd: vec![
                "curl".into(),
                "--proto".into(),
                "=https".into(),
                "--tlsv1.2".into(),
                "-sSf".into(),
                "https://sh.rustup.rs".into(),
            ],
            note: "installs the Rust toolchain via rustup (https://rustup.rs)",
        });
    }

    // Windows needs a linker for host build scripts -- `wit-bindgen-rt` carries
    // one, so every app build links something for the host even though the app
    // itself targets wasm.
    //
    // The obvious answer is Visual Studio Build Tools, which is roughly three
    // gigabytes and an install almost nobody outside C++ already has. The
    // better answer is rustup's `gnullvm` toolchain, which brings its own LLVM
    // linker and needs no Microsoft tooling at all. Prefer that, and only ask
    // for Build Tools when it is not available.
    #[cfg(windows)]
    if !gnullvm_toolchain_present() && !msvc_linker_present() {
        missing.push(MissingTool {
            what: "a linker for Windows",
            install_cmd: vec![
                "rustup".into(),
                "toolchain".into(),
                "install".into(),
                gnullvm_toolchain_name().into(),
            ],
            note: "rustup's gnullvm toolchain brings its own linker, so Visual \
Studio Build Tools are not needed",
        });
    }

    if !has_tool("cargo-component", &["--version"]) {
        missing.push(MissingTool {
            what: "cargo-component",
            install_cmd: vec![
                "cargo".into(),
                "install".into(),
                "cargo-component".into(),
                "--locked".into(),
                "--version".into(),
                CARGO_COMPONENT_VERSION.into(),
            ],
            note: "the tool that builds a Rust app into a Krate component",
        });
    }

    // The wasm target. There are three cases:
    //   1. rustup present and the target installed  -> fine.
    //   2. rustup present but the target missing     -> add it (rustup target add).
    //   3. cargo present but NOT rustup-managed (e.g. a Homebrew `cargo` with no
    //      rustup)                                    -> the build cannot reach a
    //      wasm target at all; point the user at rustup, which is the supported
    //      toolchain, rather than letting the build fail later with a raw
    //      "wasm32-wasip1 target not found / rustup is not available" error.
    if have_cargo {
        match has_rust_target(CREATE_WASM_TARGET) {
            Some(true) => {}
            Some(false) => missing.push(MissingTool {
                what: CREATE_WASM_TARGET,
                install_cmd: vec![
                    "rustup".into(),
                    "target".into(),
                    "add".into(),
                    CREATE_WASM_TARGET.into(),
                ],
                note: "the WebAssembly target Krate apps compile to",
            }),
            // `has_rust_target` returns None when rustup could not be run. If a
            // rustup toolchain is not reachable either, the cargo on PATH is a
            // non-rustup one (commonly `brew install rust`) that cannot build a
            // wasm component. Installing rustup gives a toolchain that can.
            None if rustup_toolchain_bin().is_none() => missing.push(MissingTool {
                what: "a rustup-managed Rust toolchain",
                install_cmd: vec![
                    "curl".into(),
                    "--proto".into(),
                    "=https".into(),
                    "--tlsv1.2".into(),
                    "-sSf".into(),
                    "https://sh.rustup.rs".into(),
                ],
                note: "the Rust on your PATH is not rustup-managed (for example \
                       `brew install rust`) and cannot build the WebAssembly \
                       target; rustup provides one that can",
            }),
            None => {}
        }
    }

    missing
}

/// Ensure the build toolchain is present before `create` starts authoring.
///
/// When something is missing it explains what and how to fix it. If a terminal
/// is attached (and `--no-install` was not passed) it offers to run the
/// installs, honoring `--yes` to skip the prompt. A non-interactive run never
/// installs or prompts: it prints the commands and returns an error, so an
/// agent or CI pipeline gets a clear, actionable failure instead of a cargo
/// stack trace part-way through the build.
fn preflight_toolchain(assume_yes: bool, no_install: bool) -> Result<()> {
    let missing = missing_create_tools();
    if missing.is_empty() {
        return Ok(());
    }

    // Lead with what the wait buys and roughly how long it takes. Naming the
    // tools first told someone who does not write Rust that they were in the
    // wrong place, at the exact moment they had already committed and had
    // nothing of their own working yet.
    eprintln!("Making your own apps needs a compiler. It sets up once, in about");
    eprintln!("five minutes, and then every app you make is fast.");
    eprintln!();
    eprintln!("Still to install:");
    for tool in &missing {
        eprintln!("  - {} ({})", tool.what, tool.note);
    }
    eprintln!();

    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let may_install = !no_install && (assume_yes || interactive);

    if !may_install {
        eprintln!("Set it up, then run `krate create` again:");
        for tool in &missing {
            eprintln!("  {}", install_command_line(&tool.install_cmd));
        }
        eprintln!();
        eprintln!("Or check your setup any time with `krate doctor`.");
        anyhow::bail!("missing build tools; see the commands above");
    }

    if !assume_yes {
        eprint!("Set it up now? [Y/n] ");
        io::stderr().flush().ok();
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if !(answer.is_empty() || answer == "y" || answer == "yes") {
            eprintln!("Not installing. To do it yourself:");
            for tool in &missing {
                eprintln!("  {}", install_command_line(&tool.install_cmd));
            }
            anyhow::bail!("build tools are required to create an app");
        }
    }

    for tool in &missing {
        eprintln!("==> installing {}", tool.what);
        eprintln!("    {}", install_command_line(&tool.install_cmd));
        run_install_command(&tool.install_cmd).with_context(|| format!("install {}", tool.what))?;
    }

    // Re-check: installing rustup does not put cargo on the current PATH, and a
    // fresh target may still be needed, so verify and guide rather than fail
    // opaquely later.
    let still_missing = missing_create_tools();
    if !still_missing.is_empty() {
        eprintln!();
        eprintln!("Some tools still are not on this shell's PATH. Open a new terminal");
        eprintln!("(or run `source \"$HOME/.cargo/env\"`), then run `krate create` again.");
        anyhow::bail!("finish the toolchain setup, then re-run");
    }

    eprintln!("==> build tools ready");
    Ok(())
}

/// The `--json` counterpart of the preflight: never prompt or install. If the
/// toolchain is complete, return Ok; otherwise print one `krate.author.v1`
/// error object naming what is missing and the fix commands, and bail.
fn preflight_toolchain_report_json(output: &Path) -> Result<()> {
    let missing = missing_create_tools();
    if missing.is_empty() {
        return Ok(());
    }
    let report = serde_json::json!({
        "schema": "krate.author.v1",
        "ok": false,
        "error": "missing-build-tools",
        "output": output.to_string_lossy(),
        "missing": missing.iter().map(|t| serde_json::json!({
            "tool": t.what,
            "install": t.install_cmd.join(" "),
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string(&report)?);
    anyhow::bail!("missing build tools");
}

/// Run one install command, streaming its output. The rustup bootstrap is
/// `curl … | sh`; everything else runs directly.
/// The install command as a person would type it.
///
/// `install_cmd` is an argv array because the runner spawns it directly, and
/// the rustup bootstrap is a script that must be piped into a shell. Joining
/// the array with spaces produced `curl … https://sh.rustup.rs` with no `| sh`,
/// so anyone who copied the line printed the script to their terminal instead
/// of installing anything -- and the next `krate create` failed the same way.
fn install_command_line(cmd: &[String]) -> String {
    let joined = cmd.join(" ");
    if cmd.first().map(String::as_str) == Some("curl") {
        // Match what run_install_command actually pipes: -y and
        // --no-modify-path, so someone who copies the printed line gets the
        // same non-interactive, profile-safe install Krate runs itself.
        format!("{joined} | sh -s -- -y --no-modify-path")
    } else {
        joined
    }
}

/// Run an install command, capturing its output instead of printing it.
///
/// The interactive path draws a progress bar, and an installer narrating
/// underneath it produces a mess that hides both. Captured output is kept for
/// the failure message, where it is the only thing worth showing.
fn run_install_command_quiet(cmd: &[String]) -> Result<std::process::Output> {
    let (program, args) = cmd.split_first().context("empty install command")?;

    if program == "curl" {
        let curl = ProcessCommand::new("curl")
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("run curl for rustup")?;
        return ProcessCommand::new("sh")
            .args(["-s", "--", "-y", "--no-modify-path"])
            .stdin(curl.stdout.context("curl produced no output")?)
            .output()
            .context("run the rustup installer");
    }

    let program = if program == "cargo" {
        // rustup may have been installed moments ago, and an already-open
        // shell does not see the new PATH.
        rustup_toolchain_bin()
            .map(|bin| bin.join(if cfg!(windows) { "cargo.exe" } else { "cargo" }))
            .filter(|path| path.exists())
            .unwrap_or_else(|| PathBuf::from(program))
    } else {
        PathBuf::from(program)
    };

    ProcessCommand::new(&program)
        .args(args)
        .output()
        .with_context(|| format!("run {}", program.display()))
}

fn run_install_command(cmd: &[String]) -> Result<()> {
    let (program, args) = cmd.split_first().context("empty install command")?;

    // The rustup bootstrap is a piped shell script; run it as `curl … | sh -s -- -y`.
    let status = if program == "curl" {
        let curl = ProcessCommand::new("curl")
            .args(args)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .context("run curl for rustup")?;
        // -y: non-interactive, accept defaults. --no-modify-path: do NOT touch
        // the user's shell profile. rustup's profile edit failed outright on a
        // first user whose .bash_profile was not writable ("could not amend
        // shell profile ... Permission denied"), which killed the whole
        // install. Krate finds the toolchain through rustup's own bin dir
        // (rustup_toolchain_bin), so it never needed the profile edited anyway.
        let sh = ProcessCommand::new("sh")
            .args(["-s", "--", "-y", "--no-modify-path"])
            .stdin(curl.stdout.context("curl produced no output")?)
            .status()
            .context("run the rustup installer")?;
        sh
    } else if program == "cargo" {
        // rustup was very likely just installed in this same run, and a shell
        // that was already open does not pick up a PATH change. Asking rustup
        // where cargo is works immediately; relying on PATH fails with
        // "install cargo-component" and no reason, which is what a Windows
        // user hit right after a successful rustup install.
        let cargo = rustup_toolchain_bin()
            .map(|bin| bin.join(if cfg!(windows) { "cargo.exe" } else { "cargo" }))
            .filter(|path| path.exists());
        let mut command = match &cargo {
            Some(path) => ProcessCommand::new(path),
            None => ProcessCommand::new(program),
        };
        command
            .args(args)
            .status()
            .with_context(|| format!("run {program}"))?
    } else {
        ProcessCommand::new(program)
            .args(args)
            .status()
            .with_context(|| format!("run {program}"))?
    };

    if !status.success() {
        anyhow::bail!("install command failed: {}", cmd.join(" "));
    }
    Ok(())
}

fn print_tool_status(program: &str, args: &[&str]) {
    if let Some(line) = tool_status_line(program, args) {
        println!("{line}");
    } else {
        println!("{program:<15} missing");
    }
}

fn print_target_status(target: &str) -> Result<()> {
    let output = ProcessCommand::new("rustup")
        .args(["target", "list", "--installed"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let targets = String::from_utf8_lossy(&output.stdout);
            let installed = targets.lines().any(|line| line == target);
            println!(
                "{:<15} {}",
                target,
                if installed { "installed" } else { "missing" }
            );
        }
        _ => println!("{target:<15} unknown (rustup unavailable)"),
    }

    Ok(())
}

fn print_jco_status() {
    if let Some(line) = tool_status_line("jco", &["--version"]) {
        println!("{line}");
        return;
    }

    if let Some(line) = tool_status_line("npx", &["--no-install", "jco", "--version"]) {
        if let Some(version) = line.split_whitespace().nth(1) {
            println!("jco             {version} (via npx)");
            return;
        }
    }

    println!("jco             missing");
}

fn tool_status_line(program: &str, args: &[&str]) -> Option<String> {
    let command = resolve_tool(program).unwrap_or_else(|| PathBuf::from(program));
    let output = ProcessCommand::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout);
    Some(format!("{program:<15} {}", version.trim()))
}

fn resolve_tool(program: &str) -> Option<PathBuf> {
    if let Some(path) = find_on_path(program) {
        return Some(path);
    }

    // Beside our own binary. The installer places cargo-component next to
    // `krate`, and that directory is not always on PATH -- someone who ran the
    // installer and then invoked krate by its full path would otherwise be told
    // to spend minutes compiling a tool that is already sitting right there.
    if let Some(sibling) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(executable_name(program))))
        .filter(|candidate| candidate.exists())
    {
        return Some(sibling);
    }

    cargo_home().and_then(|home| {
        let candidate = home.join("bin").join(executable_name(program));
        candidate.exists().then_some(candidate)
    })
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|path| path.join(executable_name(program)))
        .find(|path| path.exists())
}

fn executable_name(program: &str) -> String {
    if cfg!(windows)
        && Path::new(program)
            .extension()
            .is_none_or(|ext| ext != "exe")
    {
        format!("{program}.exe")
    } else {
        program.to_string()
    }
}

fn cargo_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(home));
    }

    home_dir().map(|home| home.join(".cargo"))
}

/// The shell used to run an author or port command.
///
/// On Windows, a bare `bash` resolves to the Windows Subsystem for Linux stub,
/// which prints "has no installed distributions" and fails on a machine that
/// never asked for WSL -- including CI. Git for Windows ships a real POSIX bash
/// and is present wherever git is, so prefer it and fall back to `bash` only
/// when it is missing.
fn author_shell() -> String {
    if !cfg!(windows) {
        return "sh".to_string();
    }
    for candidate in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
    ] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "bash".to_string()
}

fn krate_home() -> PathBuf {
    home_dir()
        .map(|home| home.join(".krate"))
        .unwrap_or_else(|| PathBuf::from(".krate"))
}

/// Where one app's key-value store lives.
///
/// Under the user's `~/.krate/store/`, keyed on the app's declared id so the
/// data follows the app rather than the file it arrived in. The id is
/// sanitised rather than trusted: it comes from a manifest an app author wrote,
/// so an id containing `..` or a path separator must not be able to place the
/// store outside this directory or over another app's.
fn app_store_path(app_id: &str) -> PathBuf {
    let safe: String = app_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A run of dots cannot become a traversal, and an empty id still lands
    // somewhere deterministic rather than at the directory root.
    let safe = safe.replace("..", "__");
    let safe = if safe.trim_matches(['.', '_'].as_slice()).is_empty() {
        "unnamed-app".to_string()
    } else {
        safe
    };
    krate_home().join("store").join(format!("{safe}.kv"))
}

/// Where one app's database lives. Same directory and the same sanitising as
/// the key-value store, so both follow the app rather than the file.
fn app_database_path(app_id: &str) -> PathBuf {
    let kv = app_store_path(app_id);
    kv.with_extension("sqlite")
}

/// Where one app's secrets live, alongside its other storage.
fn app_secrets_path(app_id: &str) -> PathBuf {
    app_store_path(app_id).with_extension("secrets")
}

/// This computer's key, generated once and kept private to the user.
///
/// Secrets are encrypted with a key derived from this, so copying an app's
/// secret file to another machine does not carry usable secrets with it. It
/// never reaches an app: the runtime uses it to derive a per-app key and the
/// app only ever sees plaintext it already stored.
///
/// A machine that cannot keep the file falls back to a value derived from the
/// user's home directory. That is weaker, and deliberately not silent about
/// being a fallback -- it keeps an app working rather than failing to start,
/// which is the right trade for storage that is already local.
fn machine_key() -> Vec<u8> {
    let path = krate_home().join("machine.key");
    if let Ok(existing) = fs::read(&path) {
        if existing.len() >= 32 {
            return existing;
        }
    }
    let mut key = vec![0u8; 32];
    let sourced = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut key)
        })
        .is_ok();
    if !sourced {
        // No /dev/urandom (Windows): derive from values that differ per machine
        // and per install rather than shipping a constant.
        let mut hasher = Sha256::new();
        hasher.update(krate_home().to_string_lossy().as_bytes());
        hasher.update(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos().to_le_bytes())
                .unwrap_or([0; 16]),
        );
        hasher.update(std::process::id().to_le_bytes());
        key.copy_from_slice(&hasher.finalize());
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::write(&path, &key).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
    key
}

/// The person's home directory, on every system Krate runs on.
///
/// `HOME` alone is wrong: Windows does not set it. Ten places read `HOME`
/// directly and every one of them silently did nothing on Windows -- "My apps"
/// listed nothing the moment the menu was reopened, history was never kept,
/// the Desktop default never resolved, and GitHub sign-in could not be saved.
/// Each failed by returning `None` rather than by erroring, so it looked like
/// a product with no memory instead of a bug.
///
/// `pub(crate)` so nothing has to reimplement this. If a new call site needs a
/// home directory, it uses this.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod home_tests {
    /// Windows does not set HOME. Ten places read it directly and every one
    /// silently did nothing there: "My apps" listed nothing the moment the
    /// menu was reopened, though the app had just been built and was sitting
    /// on the Desktop. History, the Desktop default and GitHub sign-in failed
    /// the same way, all by returning None rather than by erroring.
    ///
    /// This guards the rule rather than the symptom: nothing under crates/cli
    /// may read HOME without a USERPROFILE fallback.
    #[test]
    fn nothing_reads_home_without_a_windows_fallback() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            // This test's own body names the pattern it looks for, so stop at
            // the module that holds it rather than matching itself.
            let source = match source.find("mod home_tests {") {
                Some(at) => source[..at].to_string(),
                None => source,
            };
            for (number, line) in source.lines().enumerate() {
                if !line.contains(r#"var_os("HOME")"#) && !line.contains(r#"var("HOME")"#) {
                    continue;
                }
                // The one definition allowed to read HOME is home_dir itself,
                // which is the function everything else must go through. It is
                // recognised by the USERPROFILE fallback on the next lines.
                let rest: String = source
                    .lines()
                    .skip(number)
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(" ");
                if rest.contains("USERPROFILE") {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}",
                    path.file_name().unwrap().to_string_lossy(),
                    number + 1
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "these read HOME with no USERPROFILE fallback, so they do nothing \
             on Windows -- use crate::home_dir(): {offenders:?}"
        );
    }
}

#[cfg(test)]
mod create_tests {
    use super::{
        app_kind_name, author_contract, claude_author_prompt, has_tool, human_label,
        name_from_request, toml_path, validate_create_request, MAX_DERIVED_NAME_WORDS,
    };
    use krate_author::AppKind;
    use krate_manifest::Capability;
    use std::path::Path;

    #[test]
    fn an_app_with_nothing_to_withhold_is_not_tested_against_a_capability_it_never_asked_for() {
        fn manifest(caps: &str) -> krate_manifest::Manifest {
            krate_manifest::Manifest::parse(&format!(
                "[app]\nid = \"dev.krate.budget\"\nname = \"Budget\"\n\
                 version = \"0.1.0\"\nentry = \"code.wasm\"\n\
                 world = \"krate:app/gui@0.2.0\"\n{caps}"
            ))
            .expect("manifest parses")
        }

        fn entry(cap: &str) -> String {
            format!("\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"t\"\nrequired = true\n")
        }

        // A ported GUI app: a window and its own output, nothing else. This
        // used to fall back to `fs.write`, so the permission wall was proven by
        // withholding something the app had never requested -- which of course
        // did not refuse it, and the port failed after building and packing
        // correctly.
        let gui = manifest(&format!(
            "{}{}{}",
            entry("ui.window:create"),
            entry("io.stdout"),
            entry("io.args")
        ));
        assert_eq!(
            super::gating_capability(&gui),
            None,
            "an app asking only for defaults and its window has nothing to withhold"
        );

        // An app that does touch files still gets a real gate.
        let writer = manifest(&format!(
            "{}{}",
            entry("ui.window:create"),
            entry("fs.write:./data/**")
        ));
        assert_eq!(
            super::gating_capability(&writer).as_deref(),
            Some("fs.write:./data/**")
        );

        // And so does one whose only real ask is storage.
        let saver = manifest(&format!(
            "{}{}",
            entry("ui.window:create"),
            entry("store.kv")
        ));
        assert_eq!(
            super::gating_capability(&saver).as_deref(),
            Some("store.kv")
        );
    }

    #[test]
    fn the_contract_hands_the_agent_the_api_it_must_write_against() {
        let contract = author_contract("demo");

        // The contract used to state rules without listing a single function,
        // so an agent porting a hex viewer invented `stdio::write`. If the
        // generator ever silently produces nothing, the contract goes back to
        // exactly that state -- and nothing else would catch it.
        let listed = contract.lines().filter(|l| l.starts_with("- `")).count();
        assert!(
            listed > 40,
            "the contract lists only {listed} functions; the agent is guessing again"
        );

        // The specific call that was invented now exists and is named.
        assert!(contract.contains("io::stdio::write(bytes: &[u8])"));

        // And the verification convention, which the agent was previously
        // expected to guess. A port of a duplicate-file finder built, packed,
        // and then failed its verification run because the app parsed
        // arguments strictly and rejected a bare `quick` it had never been
        // told to expect.
        assert!(
            contract.contains("`quick`") && contract.contains("not `--quick`"),
            "the contract must say the verification argument is bare, not a flag"
        );
        // And that a file-reading CLI gets a path instead, which is what
        // actually broke a duplicate-file finder: it handled `quick` correctly
        // and was handed `input/sample.txt`.
        assert!(
            contract.contains(
                "must\n\
             accept **both**"
            ) || contract.contains("accept **both**"),
            "the contract must say a file-reading app gets a path, not `quick`"
        );
        // And the instruction for what to do when something is genuinely absent.
        assert!(contract.contains("do not invent a call"));

        // Every `bindings::krate::` path the contract spells out must exist in
        // the WIT. The image record was documented as bare `ImagePixels`, so
        // the agent reached for `types::ImagePixels` -- the module every other
        // record lives in -- and burned a repair cycle finding out it is in
        // `ui::image`. A path the contract names and the contract alone is
        // worse than no path at all: it reads as authoritative.
        let wit_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit/krate");
        let mut wit = String::new();
        for entry in crate::walkdir_wit(&wit_root) {
            wit.push_str(&std::fs::read_to_string(&entry).unwrap_or_default());
        }
        for line in contract.lines() {
            for path in line.split('`') {
                let Some(rest) = path.strip_prefix("bindings::krate::") else {
                    continue;
                };
                let parts: Vec<&str> = rest.split("::").collect();
                let Some(last) = parts.last() else { continue };
                // Trim a call's arguments or a record literal's fields, so
                // `set_pixels(window, ..)` and `ImagePixels { width, .. }`
                // both reduce to the bare name the WIT declares.
                let name = last
                    .split(['(', ' ', '{'])
                    .next()
                    .unwrap_or(last)
                    .trim_end_matches(',');
                if name.is_empty() {
                    continue;
                }
                // WIT is kebab-case throughout: `set_pixels` is `set-pixels`
                // and `ImagePixels` is `image-pixels`, so both an underscore
                // and a capital start a new word.
                let mut kebab = String::new();
                for (index, ch) in name.chars().enumerate() {
                    if ch == '_' {
                        kebab.push('-');
                    } else if ch.is_ascii_uppercase() {
                        if index > 0 {
                            kebab.push('-');
                        }
                        kebab.push(ch.to_ascii_lowercase());
                    } else {
                        kebab.push(ch);
                    }
                }
                assert!(
                    wit.contains(&format!("{kebab}:")) || wit.contains(&format!("{kebab} ")),
                    "the contract names `bindings::krate::{rest}`, but `{kebab}` is \
                     nowhere in the WIT -- an agent following it writes a call that \
                     does not compile"
                );
            }
        }

        // Every capability an app must ask for, by name. The contract listed
        // sixty-three functions and exactly one capability, so an agent knew
        // how to call `store::set` and had to guess that the manifest needs
        // `store.kv`. Enumerated from the runtime's own registry rather than
        // sampled, so a capability added without being listed fails here.
        let missing: Vec<String> = krate_manifest::supported_capability_specs()
            .iter()
            .filter(|spec| !spec.default_granted())
            .map(|spec| spec.name())
            .filter(|name| !contract.contains(name.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "the contract does not name these capabilities, so an agent would \
             have to guess them: {missing:?}"
        );
    }

    fn cap(s: &str) -> Capability {
        s.parse().expect("parse capability")
    }

    #[test]
    fn human_label_renders_common_capabilities_in_plain_words() {
        // The parser normalizes a leading `./`, so the phrase reads cleanly.
        assert_eq!(
            human_label(&cap("fs.write:./checklist/**")),
            "save files in checklist"
        );
        assert_eq!(human_label(&cap("fs.read:data/**")), "read files in data");
        assert_eq!(
            human_label(&cap("fs.list:input/**")),
            "see the list of files in input"
        );
        assert_eq!(
            human_label(&cap("fs.mkdir:input/quick")),
            "create folders in input/quick"
        );
        assert_eq!(
            human_label(&cap("ui.window:create")),
            "open a window on your screen"
        );
        assert_eq!(human_label(&cap("time.clock")), "read the current time");
        assert_eq!(
            human_label(&cap("audio.capture")),
            "listen through your microphone"
        );
        assert_eq!(human_label(&cap("io.stdout")), "print output");
    }

    #[test]
    fn human_label_never_drops_information() {
        // Every friendly label must still convey what the capability does; a
        // recognized one gets a plain phrase, and the resource survives it.
        assert!(human_label(&cap("fs.read:notes/**")).contains("notes"));
        assert!(human_label(&cap("net.connect:example.com:443")).contains("example.com:443"));
    }

    #[test]
    fn empty_and_short_create_requests_are_rejected() {
        assert!(validate_create_request("").is_err());
        assert!(validate_create_request("   ").is_err());
        assert!(validate_create_request("ab").is_err());
        // A real request passes.
        assert!(validate_create_request("a checklist for groceries").is_ok());
    }

    #[test]
    fn toml_path_uses_forward_slashes_and_strips_unc() {
        // A Windows verbatim path becomes a clean forward-slash path.
        assert_eq!(
            toml_path(Path::new(r"\\?\C:\Users\a\wit\krate\phase3")),
            "C:/Users/a/wit/krate/phase3"
        );
        // A plain backslash path is normalized too.
        assert_eq!(
            toml_path(Path::new(r"C:\cache\krate\sdk")),
            "C:/cache/krate/sdk"
        );
        // A Unix path is unchanged.
        assert_eq!(toml_path(Path::new("/home/a/wit")), "/home/a/wit");
    }

    #[test]
    fn the_import_diagnostic_points_at_real_panic_sites() {
        // An animated sample leaked all thirty-three wasi imports, and finding
        // the cause took an hour of bisecting because nothing in the compiler
        // output mentioned it. It was three array indexes, whose bounds checks
        // keep std's panic path reachable. This names those lines instead.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "fn f(buf: &mut [u8; 4], v: u64) {\n\
             \x20   // digits[0] = 1; this comment must not be flagged\n\
             \x20   buf[0] = b'0';\n\
             \x20   let name = \"x\".to_string();\n\
             \x20   let _ = (name, v);\n\
             }\n",
        )
        .unwrap();

        let hints = super::panic_site_hints(dir.path());
        assert!(hints.contains("src/lib.rs:3"), "the index line: {hints}");
        assert!(hints.contains("indexes"), "{hints}");
        assert!(
            hints.contains("src/lib.rs:4"),
            "the to_string line: {hints}"
        );
        assert!(hints.contains("allocates"), "{hints}");
        assert!(
            !hints.contains("src/lib.rs:2"),
            "a comment must not be reported: {hints}"
        );

        // No source, no guesses.
        let empty = tempfile::tempdir().unwrap();
        assert!(super::panic_site_hints(empty.path()).is_empty());
    }

    #[test]
    fn an_explicit_name_is_checked_before_anything_is_built() {
        // `--name 2048` used to die mid-build with "failed to load cargo
        // metadata". The rule lives where the message can name the flag.
        assert!(super::validate_app_name("tile-game").is_ok());
        assert!(super::validate_app_name("mp3-tagger").is_ok());
        for bad in [
            "2048",
            "Tile-Game",
            "tile_game",
            "tile--game",
            "",
            "-x",
            "x-",
        ] {
            assert!(
                super::validate_app_name(bad).is_err(),
                "`{bad}` must be refused"
            );
        }
    }

    #[test]
    fn an_app_is_named_for_what_was_asked_for() {
        // The name becomes the data folder, and the folder is what the
        // permission wall shows, so it has to say what the app actually is.
        assert_eq!(
            name_from_request("A reading list app to track books I want to read").as_deref(),
            Some("reading-list")
        );
        assert_eq!(
            name_from_request("A grocery list app").as_deref(),
            Some("grocery-list")
        );
        assert_eq!(
            name_from_request("build me a todo list").as_deref(),
            Some("todo-list")
        );
        assert_eq!(
            name_from_request("Make a checklist app that saves locally").as_deref(),
            Some("checklist")
        );
    }

    #[test]
    fn a_request_with_no_subject_keeps_the_default_name() {
        // Nothing worth naming: the caller's per-kind default is better than
        // anything that could be invented from filler words alone.
        assert_eq!(name_from_request("Make an app"), None);

        // A digit-led word must never reach the name: it becomes a WIT package
        // label, and label words must begin with a lowercase letter. This
        // exact request produced `pomodoro-timer-25` and an "invalid label"
        // build failure that surfaced as toolchain advice.
        assert_eq!(
            name_from_request("Make a pomodoro timer: 25 minute work sessions and 5 minute breaks")
                .as_deref(),
            Some("pomodoro-timer")
        );
        // A digit-led word before the subject starts is simply skipped: the
        // name picks up at the first word a WIT label can begin with.
        assert_eq!(
            name_from_request("Make a 2048 clone").as_deref(),
            Some("clone")
        );
        assert_eq!(name_from_request("please build me the app"), None);
        assert_eq!(name_from_request(""), None);
    }

    #[test]
    fn a_derived_name_is_a_usable_folder_name() {
        // It ends up on disk and inside a capability string, so punctuation and
        // runaway length both have to be gone by this point.
        let name = name_from_request("Make a \"Reading List!!\" app for tracking everything")
            .expect("a name");
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(name.split('-').count() <= MAX_DERIVED_NAME_WORDS);
    }

    #[test]
    fn an_app_id_cannot_place_its_store_outside_the_store_directory() {
        // The id comes from a manifest the app author wrote, so it is input,
        // not a fact. A traversal here would let one app read or overwrite
        // another's saved data, or write anywhere in the user's home.
        let root = super::krate_home().join("store");
        for hostile in [
            "../../etc/passwd",
            "..",
            "../other-app",
            "a/b/c",
            "a\\b",
            "",
            "...",
        ] {
            let path = super::app_store_path(hostile);
            assert!(
                path.starts_with(&root),
                "{hostile:?} escaped to {}",
                path.display()
            );
            assert!(
                !path.to_string_lossy().contains(".."),
                "{hostile:?} kept a traversal: {}",
                path.display()
            );
        }
    }

    #[test]
    fn two_apps_get_two_stores_and_one_app_keeps_the_same_one() {
        assert_ne!(
            super::app_store_path("dev.krate.notes"),
            super::app_store_path("dev.krate.checklist")
        );
        // Stable across runs: the same id must always resolve to the same file,
        // or an app would lose its data on the next launch.
        assert_eq!(
            super::app_store_path("dev.krate.notes"),
            super::app_store_path("dev.krate.notes")
        );
    }

    #[test]
    fn the_author_shell_is_never_the_wsl_stub() {
        // On Windows a bare `bash` resolves to the WSL stub, which fails with
        // "has no installed distributions" on any machine that never asked for
        // WSL. That is what broke the port tests on the Windows lane while both
        // other systems passed.
        let shell = super::author_shell();
        if cfg!(windows) {
            assert!(
                shell.ends_with("bash.exe") || shell == "bash",
                "unexpected Windows shell: {shell}"
            );
        } else {
            assert_eq!(shell, "sh");
        }
    }

    #[test]
    fn a_port_candidate_does_not_claim_to_be_the_starter() {
        // The starters are real working apps, so their doc headers describe
        // those apps -- right for `krate create`, actively misleading for
        // `krate port`. A hex viewer's candidate opened by saying it counts
        // word frequencies, contradicting the task beside it.
        let starter = "//! Krate Word Count.\n                       //!\n                       //! Counts the most common words in a file.\n                       \n                       #![no_std]\n                       fn main() {}\n";
        let rewritten = super::rewrite_candidate_header(starter, "hexyl", "/src/hexyl");

        assert!(rewritten.contains("Port candidate for `hexyl`"));
        assert!(rewritten.contains("/src/hexyl"));
        assert!(
            !rewritten.contains("Counts the most common words"),
            "the starter's description must not survive"
        );
        // The code below the header is untouched, including the no_std line the
        // whole component depends on.
        assert!(rewritten.contains("#![no_std]"));
        assert!(rewritten.contains("fn main() {}"));
    }

    #[test]
    fn a_printed_install_command_can_be_pasted() {
        // The rustup bootstrap is a script that has to be piped into a shell.
        // Printing the argv array verbatim dropped the pipe, so anyone who
        // copied the line printed the installer to their terminal and then hit
        // the same missing-tools error again.
        let curl: Vec<String> = ["curl", "-sSf", "https://sh.rustup.rs"]
            .into_iter()
            .map(String::from)
            .collect();
        // The printed line pipes into a shell and matches what Krate runs
        // itself: -y for a non-interactive install and --no-modify-path so it
        // does not fail trying to amend a shell profile it cannot write.
        assert!(super::install_command_line(&curl).ends_with("| sh -s -- -y --no-modify-path"));

        // An ordinary command is unchanged; only the piped one is special.
        let cargo: Vec<String> = ["cargo", "install", "cargo-component"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            super::install_command_line(&cargo),
            "cargo install cargo-component"
        );
    }

    #[test]
    fn has_tool_detects_present_and_absent_programs() {
        // A program that always exists and reports a version cleanly.
        // `cargo` is present in every build/test environment.
        assert!(has_tool("cargo", &["--version"]));
        // A program that certainly does not exist.
        assert!(!has_tool(
            "krate-definitely-not-a-real-tool-xyz",
            &["--version"]
        ));
    }

    #[test]
    fn the_author_prompt_is_the_check_app_loop() {
        let prompt = claude_author_prompt("/work/app", "a tip calculator", "/usr/local/bin/krate");
        // The request and the working directory are in it.
        assert!(prompt.contains("a tip calculator"));
        assert!(prompt.contains("/work/app"));
        // The loop: read the pack, run check-app with this binary, do not stop
        // until OK. This is the whole mechanism piece 3 adds.
        assert!(prompt.contains("KRATE_AUTHORING.md"));
        assert!(prompt.contains("/usr/local/bin/krate check-app ."));
        assert!(prompt.contains("OK"));
        assert!(prompt.contains("Bash"), "the agent is told it may use Bash");
        // It is no longer anchored to a template kind.
        assert!(!prompt.contains("a checklist GUI"));
        assert!(!prompt.contains("streaming local speech match"));
    }

    #[test]
    fn the_prompt_lets_the_agent_refuse_an_impossible_request() {
        // "Do not stop until check-app prints OK" with no exception is what
        // makes an agent build a mail reader over invented data: every stage
        // passes and nothing ever compares the app to the request. The prompt
        // must give it one way out, and name the marker the CLI reads back.
        let prompt = claude_author_prompt("/work/app", "download my email", "/usr/local/bin/krate");
        assert!(
            prompt.contains("KRATE-CANNOT-BUILD:"),
            "the agent is never told how to refuse"
        );
        assert!(prompt.contains(super::AGENT_REFUSAL_FILE));
        // And it is told, in the same breath, not to over-use it -- a false
        // refusal is worse than a caveated app.
        assert!(
            prompt.contains("only when you are certain"),
            "the agent is not warned against refusing something buildable"
        );
    }

    #[test]
    fn an_agents_refusal_is_read_back_as_one_sentence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(super::AGENT_REFUSAL_FILE);
        std::fs::write(
            &path,
            "KRATE-CANNOT-BUILD: a Krate app cannot read the mail on this computer\n",
        )
        .expect("write");
        assert_eq!(
            super::agent_refusal(&dir.path().to_string_lossy()).as_deref(),
            Some("a Krate app cannot read the mail on this computer")
        );
    }

    #[test]
    fn no_refusal_file_means_the_app_was_built() {
        // The common case: the agent built the app and left no marker, so
        // nothing here may interrupt it.
        let dir = tempfile::tempdir().expect("temp dir");
        assert_eq!(super::agent_refusal(&dir.path().to_string_lossy()), None);
    }

    #[test]
    fn a_refusal_is_machine_readable_and_not_a_build_failure() {
        // What the MCP server will read. A refusal has to be distinguishable
        // from a build that broke: one means "ask for something else", the
        // other means "retry". Same schema, distinct error, and the reason and
        // the alternative are separate fields so a model can use them.
        let req = super::CreateRequest {
            request: "a Spotify client".to_string(),
            output: std::path::PathBuf::from("/dev/null"),
            author_cmd: None,
            kind: None,
            name: None,
            transcript: None,
            work_dir: None,
            yes: false,
            no_install: true,
            json: true,
            force: false,
        };
        let verdict = krate_author::feasibility::screen(&req.request);
        let krate_author::feasibility::Verdict::Refuse(refusal) = verdict else {
            panic!("a Spotify client must be refused");
        };
        // The error carries the one sentence and the way forward.
        let message = super::report_refusal(&req, &refusal).to_string();
        assert!(message.contains("Krate cannot build that"));
        assert!(message.contains("Try instead:"));
        assert!(
            message.contains("--force"),
            "a refusal must say how to override it"
        );
    }

    #[test]
    fn an_empty_refusal_file_still_stops_the_build() {
        // A refusal that fails to parse must never fall through into building
        // the app it was warning about. Erring toward the refusal is right
        // here: the agent only writes this file when it means to stop.
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join(super::AGENT_REFUSAL_FILE), "  \n").expect("write");
        assert!(super::agent_refusal(&dir.path().to_string_lossy()).is_some());
    }

    #[test]
    fn app_kinds_have_stable_agent_environment_names() {
        assert_eq!(app_kind_name(AppKind::Checklist), "checklist");
        assert_eq!(app_kind_name(AppKind::WordFrequency), "word-frequency");
        assert_eq!(app_kind_name(AppKind::VoicePrompter), "voice-prompter");
    }
}

#[cfg(test)]
mod check_app_tests {
    use super::{build_fix, imports_fix, manifest_is_gui, restore_krate_dependency, CheckStage};
    use std::path::Path;

    #[test]
    fn wasi_leak_fix_names_the_no_std_discipline() {
        let bad = vec![
            "wasi:cli/stdout@0.2.3".to_string(),
            "wasi:clocks/wall-clock@0.2.3".to_string(),
        ];
        let fix = imports_fix(&bad, Path::new("/nonexistent"));
        // The cause and the cure, not just the symptom.
        assert!(fix.contains("no_std"), "should name no_std: {fix}");
        assert!(fix.contains("#![no_std]"));
        assert!(fix.contains("panic"), "should name the panic path");
        // Not the getrandom branch: no entropy import was leaked.
        assert!(!fix.contains("getrandom-backend"));
    }

    #[test]
    fn wasi_leak_with_entropy_points_at_the_getrandom_backend() {
        let bad = vec![
            "wasi:cli/stdout@0.2.3".to_string(),
            "wasi:random/random@0.2.3".to_string(),
        ];
        let fix = imports_fix(&bad, Path::new("/nonexistent"));
        assert!(
            fix.contains("getrandom-backend"),
            "entropy leak should point at the SDK backend: {fix}"
        );
        assert!(fix.contains("random.bytes"), "should name the capability");
        assert!(
            fix.contains("krate-diceroll"),
            "should point at the example"
        );
    }

    #[test]
    fn a_non_wasi_host_import_reads_as_a_genuine_mismatch_not_a_leak() {
        let bad = vec!["example:host/api@0.1.0".to_string()];
        let fix = imports_fix(&bad, Path::new("/nonexistent"));
        // Not a std leak: a real "Krate does not model this" message.
        assert!(!fix.contains("no_std"), "should not blame std: {fix}");
        assert!(fix.contains("does not provide") || fix.contains("only krate:*"));
    }

    /// A generated app dir, for the repair tests: a Cargo.toml carrying the WIT
    /// target path the repair reads the SDK prefix out of, and a lib.rs.
    fn app_dir_with(cargo_extra: &str, lib: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp app dir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            format!(
                "[workspace]\n\n[package]\nname = \"x\"\nversion = \"0.1.0\"\n\
                 edition = \"2021\"\n\n[dependencies]\n{cargo_extra}\n\
                 [package.metadata.component.target]\n\
                 path = \"/sdk/root/wit/krate/phase3\"\nworld = \"gui\"\n"
            ),
        )
        .expect("write Cargo.toml");
        std::fs::write(dir.path().join("src/lib.rs"), lib).expect("write lib.rs");
        dir
    }

    #[test]
    fn a_no_std_app_that_lost_the_sdk_dependency_gets_it_back() {
        // The measured top failure of AI authoring: the author converts the app
        // to no_std -- which the context pack tells it to do -- and drops the
        // `krate` dependency on the way, so the link fails on lang items with a
        // message that names neither the dep nor Cargo.toml. Repair it.
        let dir = app_dir_with(
            "wit-bindgen-rt = { version = \"0.44.0\" }\n",
            "#![no_std]\nextern crate alloc;\n",
        );
        assert!(
            restore_krate_dependency(dir.path()),
            "a no_std app with no krate dep must be repaired"
        );
        let cargo = std::fs::read_to_string(dir.path().join("Cargo.toml")).expect("read back");
        assert!(
            cargo.contains("krate = { path = \"/sdk/root/crates/bindings-rust\" }"),
            "the restored dep points at the SDK the WIT target already names:\n{cargo}"
        );
        // Idempotent: a second pass must not add it twice.
        assert!(
            !restore_krate_dependency(dir.path()),
            "repairing twice must be a no-op"
        );
        assert_eq!(
            cargo.matches("krate = { path").count(),
            1,
            "exactly one krate dependency"
        );
    }

    #[test]
    fn the_sdk_dependency_repair_leaves_correct_apps_alone() {
        // Narrow on purpose. A std guest does not need the dep, an app that
        // already has it must not get a second one, and an app whose Cargo.toml
        // does not say where the SDK is must not have a path guessed for it.
        let std_guest = app_dir_with(
            "wit-bindgen-rt = { version = \"0.44.0\" }\n",
            "// plain std\n",
        );
        assert!(
            !restore_krate_dependency(std_guest.path()),
            "a std guest links std's own allocator and needs no repair"
        );

        let already = app_dir_with(
            "krate = { path = \"../../crates/bindings-rust\" }\n",
            "#![no_std]\n",
        );
        assert!(
            !restore_krate_dependency(already.path()),
            "an app that already depends on the SDK is untouched"
        );

        // No WIT target path means no honest way to know where the SDK lives.
        let no_prefix = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(no_prefix.path().join("src")).expect("src");
        std::fs::write(
            no_prefix.path().join("Cargo.toml"),
            "[package]\nname = \"x\"\n\n[dependencies]\n",
        )
        .expect("cargo");
        std::fs::write(no_prefix.path().join("src/lib.rs"), "#![no_std]\n").expect("lib");
        assert!(
            !restore_krate_dependency(no_prefix.path()),
            "never guess an SDK path"
        );
    }

    #[test]
    fn a_missing_lang_item_build_failure_names_the_sdk_dependency() {
        // The build stage used to hand back raw cargo output for this, and an
        // author reading "error: `#[panic_handler]` function required" writes
        // its own panic handler -- the wrong answer. Name the real cause.
        for error in [
            "error: `#[panic_handler]` function required, but not found",
            "error: no global memory allocator found but one is required",
            "rust-lld: error: undefined symbol: memcpy",
        ] {
            let fix = build_fix(error);
            assert!(
                fix.contains("krate = { path"),
                "must name the dependency to add for {error}:\n{fix}"
            );
            assert!(
                fix.contains("Do NOT write your own"),
                "must steer away from a hand-written allocator/panic handler:\n{fix}"
            );
        }

        // The other cryptic one: a std guest against bindings gated on
        // std_feature. Different cause, different cure.
        let fix = build_fix("error: failed to load bitcode of module std");
        assert!(
            fix.contains("features = [\"std\"]"),
            "a std guest needs the std feature, not the no_std treatment:\n{fix}"
        );

        // An ordinary compile error keeps the general advice.
        let fix = build_fix("error[E0308]: mismatched types");
        assert!(
            fix.contains("Fix the compiler errors above"),
            "an ordinary error is not diagnosed as something it is not:\n{fix}"
        );
    }

    #[test]
    fn every_stage_has_a_distinct_nonzero_exit_code() {
        use std::collections::BTreeSet;
        let stages = [
            CheckStage::Layout,
            CheckStage::Manifest,
            CheckStage::Build,
            CheckStage::Imports,
            CheckStage::Run,
            CheckStage::Shoot,
            CheckStage::Usability,
        ];
        let codes: BTreeSet<u8> = stages.iter().map(|s| s.exit_code()).collect();
        assert_eq!(codes.len(), stages.len(), "exit codes must be distinct");
        assert!(codes.iter().all(|&c| c != 0), "no stage exits 0 on failure");
    }

    #[test]
    fn every_stage_has_a_distinct_label() {
        use std::collections::BTreeSet;
        let stages = [
            CheckStage::Layout,
            CheckStage::Manifest,
            CheckStage::Build,
            CheckStage::Imports,
            CheckStage::Run,
            CheckStage::Shoot,
            CheckStage::Usability,
        ];
        let labels: BTreeSet<&str> = stages.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels.len(),
            stages.len(),
            "an agent branches on the label, so two stages must never share one"
        );
    }

    #[test]
    fn a_window_capability_marks_an_app_as_gui() {
        fn manifest(caps: &str) -> krate_manifest::Manifest {
            krate_manifest::Manifest::parse(&format!(
                "[app]\nid = \"dev.krate.x\"\nname = \"X\"\nversion = \"0.1.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n{caps}"
            ))
            .expect("manifest parses")
        }
        let gui = manifest(
            "\n[[capabilities]]\ncap = \"ui.window:create\"\nrationale = \"t\"\nrequired = true\n",
        );
        assert!(manifest_is_gui(&gui));
        let cli = manifest(
            "\n[[capabilities]]\ncap = \"io.stdout\"\nrationale = \"t\"\nrequired = true\n",
        );
        assert!(!manifest_is_gui(&cli));
    }
}
