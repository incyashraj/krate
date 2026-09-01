use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use anyhow::{bail, Context, Result};
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
mod api_author;
mod api_key;
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
mod trace;
mod tui;
mod usage;
#[cfg(windows)]
mod winproc;

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
///
/// Forty minutes, not fifteen: a person asked for a full Contra-style stage
/// (sprites, tiles, physics, weapons, a boss) and the agent was still
/// writing real code when the old budget cut it off twice in a row. A
/// request that big is a legitimate use, not a stuck agent -- and a build
/// that dies at fifteen minutes wastes the person's whole wait. The stall
/// watchdog below is what actually catches a hung agent; this ceiling only
/// exists so nothing runs forever.
const AGENT_AUTHOR_TIMEOUT_SECS: u64 = 2400;

/// Version shown by `krate --version`. The release workflow sets
/// `KRATE_RELEASE_VERSION` to the git tag so a released binary reports its real
/// version; local and CI builds fall back to the crate version from Cargo.toml.
///
/// A debug build says so (K-030). On this project's machine a
/// `target/debug/krate` sits ahead of the installed release on PATH, and both
/// reported the identical string -- so `krate --version` could not tell you
/// which binary you had just run, and a fixed bug appeared to come back twice
/// because it was measured through the dev build. Anything measured through a
/// debug binary is contaminated: it is not the code a user runs, and it is
/// slower by an order of magnitude.
const KRATE_VERSION_NUMBER: &str = match option_env!("KRATE_RELEASE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// What a debug build appends to its version. Empty in release, so a released
/// binary reports the bare number exactly as it always did.
#[cfg(debug_assertions)]
const KRATE_BUILD_SUFFIX: &str = " (debug build -- not what a user runs)";
#[cfg(not(debug_assertions))]
const KRATE_BUILD_SUFFIX: &str = "";

/// What `krate --version` prints: the number, plus a warning when this is a
/// debug build. In release the suffix is empty, so this is the bare number and
/// a released binary reports exactly what it always did.
///
/// Leaked once at startup because clap wants a `&'static str` and the two
/// halves are only known separately. One small allocation for the life of the
/// process.
fn krate_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| format!("{KRATE_VERSION_NUMBER}{KRATE_BUILD_SUFFIX}"))
        .as_str()
}

#[derive(Debug, Parser)]
#[command(
    name = "krate",
    version = krate_version(),
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
    /// Send any spooled usage events, then exit. Spawned detached by the
    /// CLI itself; never meant to be typed by a person.
    #[command(hide = true)]
    UsageFlush,

    /// Run a WebAssembly component through the Krate runtime.
    Run {
        /// Path to a .wasm component, a .krate bundle, or an https URL to one.
        target: String,

        /// Max fuel units to allow. Omit for unlimited.
        #[arg(long)]
        fuel: Option<u64>,

        /// Folder of read-only assets to give the app, as if it had been
        /// packed into a `.krate`. Without this, an app run from loose
        /// source can never see its own images or data files, because
        /// assets otherwise only resolve out of a packed bundle (K-093).
        #[arg(long)]
        assets: Option<String>,

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

        /// Report any text the app drew on top of other text, using the draw
        /// calls themselves rather than the pixels. Use it with --shoot: the
        /// frame that is captured is the frame that gets checked.
        #[arg(long)]
        check_layout: bool,

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
    /// Your Krate account: who is signed in, sign in, sign out. Sign-in is
    /// GitHub's device flow -- a code, a browser page, and the wait ends the
    /// moment you approve it there.
    Account {
        #[command(subcommand)]
        action: Option<AccountAction>,

        /// Machine-readable output. For `account`, one JSON object of who is
        /// signed in; for `account login`, one JSON line per step so a
        /// frontend can show the code and flip the instant approval lands.
        #[arg(long)]
        json: bool,
    },

    /// Show which AI coding tools are installed, so you know what you can
    /// author apps with. Reads nothing but your PATH.
    Ai {
        /// Machine-readable output: one JSON array, a probe result per
        /// provider. The studio's agent chip reads this; anything else is
        /// welcome to.
        #[arg(long)]
        json: bool,
    },

    /// Store, check or remove an API key, for authoring without a CLI.
    ///
    /// `krate api-key set anthropic` reads the key from stdin so it never
    /// lands in shell history. The key goes to the OS keychain on macOS,
    /// and to a machine-encrypted file elsewhere.
    ApiKey {
        /// set, status, or forget.
        action: String,
        /// anthropic or openai.
        vendor: Option<String>,
    },

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

    /// Read a `KRATE_TRACE` file and print the one-build review row: the phases
    /// with their durations, what the agent did, every check-app verdict, any
    /// repair rounds, and the outcome. The study spine for "AI makes a Krate
    /// app" -- set `KRATE_TRACE=path` on a `create`, then read that path here.
    #[command(hide = true)]
    StudyReport {
        /// The JSONL trace file written by a `KRATE_TRACE`-enabled build.
        trace: PathBuf,
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
    OpenApp {
        /// Open this app directly, instead of waiting for a document from
        /// Launch Services. This is how Krate (the studio) opens a
        /// double-clicked .krate: it hands the file here and this path shows
        /// the same native consent a Finder-routed document gets. Without it
        /// the studio could only call `krate run`, which refuses ask-level
        /// permissions with TERMINAL text -- a consent question printed where
        /// no person is looking, so the app simply never opened.
        file: Option<PathBuf>,
    },

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
    /// Gather everything about one authoring session into a single report
    /// file: the conversation, the AI's own transcript, the code it wrote,
    /// the engine's log, and this machine's toolchain facts. Writes a
    /// .krate-report (a zip) and prints its path. Uploads nothing -- the
    /// studio asks the person first, then sends it.
    /// Send a report file to Krate support. Requires a sign-in, so a report
    /// arrives with a name attached.
    #[command(name = "support-send")]
    SupportSend {
        /// The .krate-report file to send.
        report: PathBuf,

        /// The session it came from, recorded alongside it.
        #[arg(long, default_value = "")]
        session: String,

        /// What the person says went wrong.
        #[arg(long, default_value = "")]
        note: String,

        /// Hub to send to. Overrides KRATE_HUB_URL.
        #[arg(long)]
        hub: Option<String>,
    },

    #[command(name = "support-report")]
    SupportReport {
        /// The studio session id, e.g. s-1786964129525.
        session: String,

        /// Where to write the report. Defaults to a temp file.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Look at a request BEFORE building: answer with either up to three
    /// short questions (when answering them would change what gets built)
    /// or a one-paragraph plan of what will be made and what it needs.
    /// Prints one JSON object -- {"ask":[...]} or {"plan":"...","needs":[...]}
    /// -- and never builds anything. This is the Studio's conversation door.
    Plan {
        /// What the person asked for, in their own words.
        request: String,

        /// Files the person attached; the plan takes them into account and
        /// asks for ones the request implies but does not include.
        #[arg(long)]
        attach: Vec<PathBuf>,

        /// Which installed AI answers. Same names as `krate create --agent`.
        #[arg(long)]
        agent: Option<String>,
    },

    /// Turn an app into a card: one file that is a picture of the app AND
    /// the app itself.
    ///
    /// The output is a valid PNG -- the app's window with a caption strip
    /// naming the file, its size, and what it is allowed to touch -- with
    /// the full bundle riding behind the picture. Image viewers see the
    /// picture; Krate reads the app straight out of the same bytes. Send it
    /// as a file (mail, AirDrop, a chat's paperclip): sent as a "photo",
    /// chat apps re-encode the image and strip the app half.
    Card {
        /// Path to the .krate bundle to turn into a card.
        bundle: PathBuf,

        /// Where to write the card. Defaults to the app's name next to the
        /// input, e.g. RateCard.krate.
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,

        /// Also write the same bytes under a .png name, for places that
        /// only take pictures. That copy still runs: `krate run Card.png`
        /// works, but chat apps treat a .png as a photo and re-encode it.
        #[arg(long)]
        png_copy: bool,

        /// Milliseconds the app runs before its window is photographed, so
        /// the still shows a settled first frame rather than a blank one.
        #[arg(long, default_value_t = 900)]
        settle_ms: u64,

        /// Use this PNG as the card's face instead of photographing the
        /// app.
        #[arg(long)]
        shot: Option<PathBuf>,
    },

    /// Wrap an app for one friend on one system: a double-clickable file
    /// that installs Krate once (a small download, checksums verified) and
    /// then opens the app.
    ///
    /// The wrap does NOT carry the 24 MB player -- it plants it, so the
    /// next .krate that friend receives just opens. The file stays roughly
    /// the app's own size, and it is still a valid bundle: `krate run`
    /// reads the app straight out of it.
    Wrap {
        /// Path to the .krate bundle to wrap.
        bundle: PathBuf,

        /// Which system the friend is on.
        #[arg(long = "for", value_enum)]
        target: WrapTarget,

        /// Where to write the wrap. Defaults to the app's name next to the
        /// input, e.g. RateCard-for-Mac.command.
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },

    /// Print the macOS gift opener's script to stdout.
    ///
    /// The release pipeline uses this to build, sign and notarize ONE
    /// opener per release, which every gift then reuses. It is here rather
    /// than duplicated in the workflow so that the script a gift ships and
    /// the script Apple notarized are the same text by construction.
    #[command(hide = true)]
    GiftOpener,

    Publish {
        /// Path to the .krate bundle to upload.
        bundle: PathBuf,

        /// Publish without a gallery listing: the link works for anyone who
        /// has it, and the app never appears in the public gallery. For the
        /// rate card that is a client's, not the world's.
        #[arg(long)]
        unlisted: bool,

        /// Hub to upload to. Overrides the KRATE_HUB_URL environment variable.
        #[arg(long)]
        hub: Option<String>,

        /// One line describing the app, shown on its cloud page. Defaults to
        /// what you asked for when the app was made.
        #[arg(long)]
        description: Option<String>,

        /// Name shown on the cloud page. Defaults to the app's own name.
        #[arg(long)]
        name: Option<String>,

        /// PNG to use as the listing screenshot. Without it the app's first
        /// frame is rendered headless and used automatically.
        #[arg(long)]
        shot: Option<PathBuf>,

        /// Small square PNG logo for the listing (under 512 KiB).
        #[arg(long)]
        icon: Option<PathBuf>,
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

        /// Give the AI a file along with the request -- a sketch, a CSV, a
        /// logo, anything that says more than the sentence can. Repeatable.
        /// Needs --agent: the built-in templates cannot read them.
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,

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

    /// Open a .krate the way a person expects an app to open: with its own
    /// name and icon, activated on screen.
    ///
    /// A bare `krate run` child spawned from a GUI gets no LaunchServices
    /// activation: the window is created, the runtime even says so, and
    /// nothing appears (K-110). This wraps the app under ~/.krate/launchers
    /// exactly as `install` does -- which also moves every later file access
    /// off guarded folders like Downloads -- and opens the wrapper through
    /// LaunchServices, which is what makes a window actually show.
    Launch {
        /// The .krate to open.
        bundle: PathBuf,
    },

    /// Install an app so it looks and behaves like any other app on the
    /// machine: its own name in the dock, its own icon, its own entry in
    /// Launchpad and the app switcher. The .krate keeps working as a file you
    /// can send; this gives the copy on your machine a real home.
    Install {
        /// The .krate to install.
        bundle: PathBuf,

        /// Install here instead of /Applications.
        #[arg(long)]
        prefix: Option<PathBuf>,

        /// Print where it would go and stop.
        #[arg(long)]
        dry_run: bool,
    },

    /// Change an app that already exists. The .krate carries its own source,
    /// so the AI edits the app you have rather than rebuilding it from a
    /// description -- "make the button blue" touches one function, not the
    /// whole app. The file is updated in place unless --output names a copy.
    Revise {
        /// The .krate to change.
        bundle: PathBuf,

        /// What to change, in plain words.
        #[arg(value_name = "CHANGE")]
        change: String,

        /// Which AI makes the change.
        #[arg(long, default_value = "claude")]
        agent: String,

        /// Give the AI a file along with the change -- a screenshot of the
        /// problem, the review you received, a mockup. Repeatable.
        #[arg(long = "attach", value_name = "FILE")]
        attachments: Vec<PathBuf>,

        /// Write the changed app here instead of updating the file in place.
        #[arg(long)]
        output: Option<PathBuf>,
    },

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

/// `krate api-key <action> [vendor]`.
///
/// The key is read from STDIN for `set`, never from an argument: an
/// argument lands in shell history and in the process list, where a
/// credential does not belong.
fn api_key_command(action: &str, vendor: Option<&str>) -> Result<u8> {
    use api_key::ApiVendor;

    let parse_vendor = |name: Option<&str>| -> Result<ApiVendor> {
        let name = name.unwrap_or("anthropic");
        ApiVendor::parse(name)
            .ok_or_else(|| anyhow::anyhow!("unknown vendor \"{name}\". Use anthropic or openai."))
    };

    match action {
        "set" => {
            let vendor = parse_vendor(vendor)?;
            let mut key = String::new();
            io::stdin()
                .read_line(&mut key)
                .context("could not read the key from stdin")?;
            let where_it_went = api_key::save(vendor, &key).map_err(|err| anyhow::anyhow!(err))?;
            println!("{} key saved, {}.", vendor.label(), where_it_went.describe());
            Ok(0)
        }
        "status" => {
            // Built as one string and written once: a println! loop panics
            // on a closed pipe (`krate api-key status | head -1`), and a
            // panic is not an acceptable answer to a closed pipe.
            let mut out = String::new();
            let mut any = false;
            for vendor in [ApiVendor::Anthropic, ApiVendor::OpenAi] {
                if let Some((key, source)) = api_key::load(vendor) {
                    any = true;
                    // Never print the key. The last four characters are
                    // enough to tell two keys apart.
                    let tail: String = {
                        let mut last: Vec<char> = key.chars().rev().take(4).collect();
                        last.reverse();
                        last.into_iter().collect()
                    };
                    out.push_str(&format!(
                        "{:<10} set, {} (...{tail})\n",
                        vendor.name(),
                        source.describe()
                    ));
                } else {
                    out.push_str(&format!("{:<10} not set\n", vendor.name()));
                }
            }
            if !any {
                out.push_str("\nAdd one with: krate api-key set anthropic\n");
            }
            let _ = io::stdout().write_all(out.as_bytes());
            Ok(0)
        }
        "forget" => {
            let vendor = parse_vendor(vendor)?;
            api_key::forget(vendor).map_err(|err| anyhow::anyhow!(err))?;
            println!("{} key removed.", vendor.label());
            Ok(0)
        }
        other => {
            anyhow::bail!("unknown action \"{other}\". Use set, status, or forget.")
        }
    }
}

/// The `--author-cmd` string for a model API, which has no CLI to resolve.
///
/// Same self-invocation as [`agent_author_command`]: the prompt stays
/// versioned with the binary rather than living in a script.
fn api_vendor_author_command(vendor: &str) -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "krate".to_string());
    format!("{} author-agent {}", shell_quote(&exe), vendor)
}

/// Whether the given work dir is an existing app's source that a change
/// should be authored directly inside -- as opposed to a plain workspace
/// that gets a fresh named subdirectory.
pub(crate) fn is_existing_app_workspace(dir: &Path, is_change: bool) -> bool {
    is_change && dir.join("src").join("lib.rs").is_file()
}

/// The provider name if `cmd` is exactly the self-invocation
/// `agent_author_command` builds, and nothing else.
fn self_author_agent(cmd: &str) -> Option<&str> {
    let exe = std::env::current_exe().ok()?;
    let prefix = format!("{} author-agent ", shell_quote(exe.to_str()?));
    let name = cmd.strip_prefix(&prefix)?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(name)
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
enum AccountAction {
    /// Sign in with GitHub.
    Login {
        /// Emit each step as JSON. Accepted here as well as on `account`,
        /// because `krate account login --json` is what a shipped Krate
        /// Studio sends, and rejecting it made signing in impossible.
        #[arg(long)]
        json: bool,
    },
    /// Store an identity delivered by the browser sign-in.
    ///
    /// Reads one JSON object from stdin: {login, name, avatar_url, token}.
    /// Stdin and not an argument, because a token on argv is visible to
    /// every process on the machine through `ps`.
    Adopt,
    /// Forget the stored sign-in on this machine.
    Logout,
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

/// Drop the console when we are its only owner AND it is actually on screen.
///
/// A double-clicked .krate routed to this console-subsystem binary makes
/// Windows allocate a fresh terminal that sits behind the app for its whole
/// life -- the black window users kept photographing. When the console's
/// process list is just us, nobody is reading it: it exists only because of
/// our subsystem, so free it. Launched from a real shell the list has the
/// shell in it too, and the console stays, prints and pipes intact.
///
/// The visibility check is the load-bearing half. The studio spawns this
/// engine with CREATE_NO_WINDOW, which gives it a console with NO window --
/// and every child (the agent, cargo, each build step) inherits that
/// invisible console for free. Unconditionally freeing it left the engine
/// bare, so each console-subsystem child minted a VISIBLE terminal: the
/// "grok window popping up on every step" report was this function's doing.
/// An invisible console is an asset; only a visible one is the bug.
/// Ask Windows for 1ms timer resolution for this process's lifetime.
///
/// Every pacing sleep in the frame loop -- the event wait and the present
/// budget -- rounds UP to the system timer tick, which defaults to ~15.6ms.
/// Two sleeps a frame turned 6ms of real work into ~65ms of wall clock: a
/// game producing 14fps with the CPU almost idle, reported as "the
/// character is barely moving". Games, browsers and media apps all raise
/// the resolution exactly like this; the OS restores it when we exit.
#[cfg(windows)]
fn raise_timer_resolution() {
    unsafe {
        let _ = windows_sys::Win32::Media::timeBeginPeriod(1);
    }
}

#[cfg(windows)]
fn detach_owned_console() {
    unsafe {
        use windows_sys::Win32::System::Console::{
            FreeConsole, GetConsoleProcessList, GetConsoleWindow,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        let mut ids = [0u32; 2];
        if GetConsoleProcessList(ids.as_mut_ptr(), 2) == 1 {
            let window = GetConsoleWindow();
            if !window.is_null() && IsWindowVisible(window) != 0 {
                FreeConsole();
            }
        }
    }
}

fn main() -> ExitCode {
    #[cfg(windows)]
    detach_owned_console();
    #[cfg(windows)]
    raise_timer_resolution();

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
            let message = friendly_error(&err);
            eprintln!("error: {message}");
            // A double-clicked run has no console -- detach_owned_console
            // freed it at startup -- so the line above went nowhere and the
            // person saw an app that "never opens" (K-178): consent shown,
            // Allow clicked, then silence, with the real error dying in a
            // window that no longer exists. Windowless is detectable, so
            // put the same words where they can be read. A terminal run
            // still has a console and never gets a dialog.
            #[cfg(windows)]
            unsafe {
                use windows_sys::Win32::System::Console::GetConsoleWindow;
                if GetConsoleWindow().is_null() {
                    let _ = rfd::MessageDialog::new()
                        .set_title("Krate could not open this app")
                        .set_description(&message)
                        .set_buttons(rfd::MessageButtons::Ok)
                        .show();
                }
            }
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
            // Deliberately does not guess which side is older.
            //
            // This said "this app needs a newer version of Krate" and pointed
            // at the installer. That is one of two possibilities and, in
            // practice, the rarer one: an app built before an interface grew
            // fails exactly the same way, and no amount of updating Krate
            // fixes it. K-035 recorded the message as backwards a day after
            // it was written and rebuilt the bundles rather than the wording.
            //
            // A person cannot act on a guess. Name the real condition and give
            // both moves.
            return format!(
                "this app and this copy of Krate were built against different \
                 versions of the app interface, so it cannot start.\n\n  \
                 If somebody sent you this app recently, update Krate:\n    \
                 curl -fsSL https://krate.tech/install.sh | sh\n    \
                 (on Windows: irm https://krate.tech/install.ps1 | iex)\n\n  \
                 If it is an app you made a while ago, it predates a change to \
                 the interface. Open it with `krate` and choose \"Make a \
                 change\" -- rebuilding it against this version is the fix.\n\n\
                 Details: {text}"
            );
        }
    }
    format!("{err:#}")
}

/// The `.krate` this process was installed to run, when it is the executable
/// inside an installed `<App>.app` -- `Contents/MacOS/<name>` beside a
/// `Contents/Resources/app.krate`. `None` for an ordinary CLI invocation, so a
/// plain `krate` in a terminal is unaffected.
fn installed_app_payload() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let payload = exe.parent()?.parent()?.join("Resources/app.krate");
    payload.is_file().then_some(payload)
}

fn run() -> Result<u8> {
    // `krate <file>.krate` means open it.
    //
    // Older releases registered the Windows file association as
    // `krate.exe "%1"`, with no subcommand, so a double-click ran the CLI with
    // a bundle path where a command should be. It printed "unrecognized
    // subcommand", the console flashed and closed, and nothing opened -- with
    // no way for the person to see what it said. Machines carrying that
    // association are already out there and a new installer cannot reach them,
    // so the binary has to understand what it was asked for.
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.len() >= 2 {
        let candidate = std::path::PathBuf::from(&args[1]);
        if candidate.extension().and_then(|e| e.to_str()) == Some("krate") && candidate.is_file() {
            args.insert(1, std::ffi::OsString::from("run"));
        }
    }

    // An installed app: this same binary, living in `<App>.app/Contents/MacOS`
    // under the app's own name, launched by Launch Services with no arguments.
    // Run the payload sitting beside it.
    //
    // The engine has to be the bundle's actual executable rather than a script
    // that runs it, because macOS takes the dock name from the executable that
    // is really running -- a shim that `exec`s krate shows "krate" in the dock,
    // which is the whole thing `krate install` exists to fix.
    if args.len() == 1 {
        if let Some(payload) = installed_app_payload() {
            // open-app, NOT run. Plain `run` refuses ask-level permissions
            // with terminal text, and an installed app has no terminal --
            // the person double-clicks, nothing appears, and there is
            // nothing to read. open-app shows the native consent window,
            // exactly as a Finder-routed document gets.
            args.push(std::ffi::OsString::from("open-app"));
            args.push(payload.into_os_string());
        }
    }
    let cli = Cli::parse_from(args);

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
            assets,
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
            check_layout,
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
            assets_root: assets.map(PathBuf::from),
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
            check_layout,
            screenshot_path: shoot,
            screenshot_scale: shoot_scale,
            usability_report,
            app_args,
        }),
        #[cfg(target_os = "macos")]
        Command::OpenApp { file } => open_app(file),
        Command::Pack {
            file,
            manifest,
            output,
        } => pack_bundle(&file, &manifest, &output),
        Command::Card {
            bundle,
            output,
            png_copy,
            settle_ms,
            shot,
        } => card_bundle(&bundle, output.as_deref(), png_copy, settle_ms, shot.as_deref()),
        Command::Wrap {
            bundle,
            target,
            output,
        } => wrap_bundle(&bundle, target, output.as_deref()),
        Command::GiftOpener => {
            print!("{}", mac_opener_script());
            Ok(0)
        }
        Command::UsageFlush => {
            usage::flush_spool_now();
            Ok(0)
        }
        Command::Telemetry { state } => usage::telemetry_command(&state),
        Command::Plan {
            request,
            attach,
            agent,
        } => plan_command(&request, &attach, agent.as_deref()),
        Command::SupportReport { session, output } => report_command(&session, output.as_deref()),
        Command::SupportSend {
            report,
            session,
            note,
            hub,
        } => report_send_command(&report, &session, &note, hub.as_deref()),
        Command::Publish {
            bundle,
            unlisted,
            hub,
            description,
            name,
            shot,
            icon,
        } => publish_bundle(
            &bundle,
            hub.as_deref(),
            description.as_deref(),
            name.as_deref(),
            shot.as_deref(),
            icon.as_deref(),
            unlisted,
        ),
        Command::Create {
            request,
            output,
            agent,
            author_cmd,
            kind,
            name,
            transcript,
            work_dir,
            attachments,
            yes,
            no_install,
            json,
            force,
        } => {
            if !attachments.is_empty() {
                // Attachments ride the same path the interactive menu uses:
                // staged beside the code, named in the prompt, and the stable
                // builds workspace so a retry resumes rather than restarts.
                let Some(agent) = agent else {
                    anyhow::bail!(
                        "--attach needs --agent: the built-in templates cannot read files.                          Try again with --agent claude."
                    );
                };
                for file in &attachments {
                    if !file.exists() {
                        anyhow::bail!("attached file {} does not exist", file.display());
                    }
                }
                let provider = resolve_agent(&agent)?;
                author_app_for_tui(&request, provider, &output, &attachments)?;
                return Ok(0);
            }
            create_krate(CreateRequest {
                request,
                output,
                // --agent is the clean front door; it resolves to the command that
                // drives that provider. An explicit --author-cmd still wins for any
                // other tool. Resolving here means an unknown name or a missing CLI
                // is reported before any authoring work begins.
                author_cmd: match (author_cmd, agent) {
                    (Some(command), _) => Some(command),
                    // An API vendor has no CLI to resolve; it self-invokes the
                    // same hidden subcommand, and api_author takes it there.
                    (None, Some(name)) if api_key::ApiVendor::parse(&name).is_some() => {
                        Some(api_vendor_author_command(&name))
                    }
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
            })
        }
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
        Command::Launch { bundle } => launch_app(&bundle),
        Command::Install {
            bundle,
            prefix,
            dry_run,
        } => install_app(&bundle, prefix.as_deref(), dry_run),
        Command::Revise {
            bundle,
            change,
            agent,
            attachments,
            output,
        } => revise_cli(&bundle, &change, &agent, &attachments, output.as_deref()),
        Command::AuthorAgent { agent } => run_author_agent(&agent),
        Command::Version => {
            print_version();
            Ok(0)
        }
        Command::Doctor => doctor(),
        Command::Account { action, json } => account_command(action, json),
        Command::Ai { json } => ai_status(json),
        Command::ApiKey { action, vendor } => api_key_command(&action, vendor.as_deref()),
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
        Command::StudyReport { trace } => study_report_command(&trace),
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
    // KRATE_VERSION_NUMBER, not CARGO_PKG_VERSION: a released binary is
    // stamped with its tag, and Cargo.toml sat at 0.1.28 through v0.1.58,
    // so this line told support the version of a build nobody is running.
    // A failure report that misstates its own version is worse than one
    // that omits it.
    text.push_str(&format!("- Krate: {KRATE_VERSION_NUMBER}\n"));
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
    /// Read-only assets for an app being run from loose source (K-093).
    assets_root: Option<PathBuf>,
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
    /// Report text drawn over other text in the captured frame.
    check_layout: bool,
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

/// Put a line of detail under the current stage, without changing the stage.
///
/// Returns false when nothing is drawing, so the caller can print instead.
pub(crate) fn report_progress_note(text: &str) -> bool {
    if let Some(progress) = progress_sink() {
        progress.note(text.to_string());
        return true;
    }
    false
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
    // A STABLE workspace under ~/.krate/builds, not a tempdir. The stall
    // error promises a retry "resumes from the code already written", and
    // create honors that -- but only if the retry lands in the same
    // directory. The TUI handed create a fresh tempdir every attempt, so
    // fifteen minutes of written code was abandoned on every "try again"
    // (K-084). One directory per app name: the retry resumes, the
    // transcript survives at a path that still exists, and attachments
    // stay staged across attempts.
    let builds = krate_home().join("builds");
    fs::create_dir_all(&builds).context("make the builds directory")?;

    let mut request = request.to_string();
    if !attachments.is_empty() {
        let inbox = builds.join("attached");
        fs::create_dir_all(&inbox).context("make the attachments directory")?;
        let mut named = Vec::new();
        for source in attachments {
            let Some(name) = source.file_name() else {
                continue;
            };
            let destination = inbox.join(name);
            if fs::copy(source, &destination).is_ok() {
                named.push(format!("attached/{}", name.to_string_lossy()));
                // A spreadsheet is a binary blob to a text-reading agent.
                // Convert each sheet to CSV beside the original so the data
                // is actually readable -- and embeddable in the app
                // (K-123 S3: the friend's Excel was the whole request, and
                // the agent could not open it).
                for sheet_csv in spreadsheet_to_csvs(&destination) {
                    named.push(format!("attached/{sheet_csv}"));
                }
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
                 since it was written against a different system. Spreadsheets have \
                 each sheet converted to a .csv beside the original: read the CSVs, \
                 never the binary. If the app is ABOUT that data, embed the data (or \
                 the relevant parts) in the app so it opens already useful.",
            );
        }
    }

    // Derived before `request` moves: the name create will derive, for
    // clearing exactly this app's build dir on success.
    let cleanup_name = name_from_request(&request).map(|name| {
        if name == "krate" {
            "krate-app".to_string()
        } else {
            name
        }
    });
    let code = create_krate(CreateRequest {
        request,
        output: output.to_path_buf(),
        author_cmd: Some(agent_author_command(provider)),
        kind: None,
        name: None,
        transcript: None,
        work_dir: Some(builds),
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
            why: None,
        },
    );
    if code == 0 {
        remember_app(output);
        // The stable workspace did its job; a finished app does not need its
        // build tree kept (target/ alone is hundreds of megabytes). Derive
        // the same name create derived and clear exactly that app's dir --
        // other apps' resumable failures stay untouched.
        // Only when the request yields a name -- a kind-fallback guess here
        // might delete a different app's resumable state.
        if let Some(name) = cleanup_name {
            let _ = fs::remove_dir_all(krate_home().join("builds").join(name));
        }
        let _ = fs::remove_dir_all(krate_home().join("builds").join("attached"));
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

/// `krate revise`: the TUI's change-an-app path, scriptable.
///
/// The same three-step fallback the interactive menu uses, in the same
/// order: edit the source the bundle carries; failing that, rebuild from the
/// history's original request plus the change; failing that, build fresh
/// from the change alone. The studio's every-message-after-the-first goes
/// through here, which is the point -- one revision path, three faces.
fn revise_cli(
    bundle: &Path,
    change: &str,
    agent: &str,
    attachments: &[PathBuf],
    output: Option<&Path>,
) -> Result<u8> {
    for file in attachments {
        if !file.exists() {
            anyhow::bail!("attached file {} does not exist", file.display());
        }
    }
    if !bundle.exists() {
        anyhow::bail!("{} does not exist", bundle.display());
    }
    let provider = resolve_agent(agent)?;
    let out = output.unwrap_or(bundle);

    match bundle_source_dir(bundle)? {
        Some(source) => {
            println!("==> changing the app in its own source");
            revise_app_for_tui(&source, change, provider, out, attachments)?;
        }
        None => {
            // An older bundle with no source inside: restate the whole app.
            let original = tui::history()
                .into_iter()
                .find(|entry| entry.bundle.as_deref() == Some(bundle))
                .map(|entry| entry.request)
                .unwrap_or_default();
            let request = if original.is_empty() {
                // Never rebuild from the change sentence alone: "change the
                // controls" as the whole request produces a stranger app
                // that silently replaces the person's own. Refusing with the
                // way forward is the only honest answer left.
                anyhow::bail!(
                    "this app has no source inside it (it was made by an older Krate), \
                     so it cannot be changed faithfully. Describe the whole app again \
                     -- with the change included -- to make a fresh one."
                );
            } else {
                println!("==> no source inside this app; rebuilding from the original request plus the change");
                format!("{original}. Then: {change}")
            };
            let code = create_krate(CreateRequest {
                request,
                output: out.to_path_buf(),
                author_cmd: Some(agent_author_command(provider)),
                kind: None,
                name: None,
                transcript: None,
                work_dir: None,
                yes: true,
                no_install: false,
                json: false,
                force: true,
            })?;
            if code != 0 {
                anyhow::bail!("the change could not be applied");
            }
        }
    }
    println!("Changed {}", out.display());
    Ok(0)
}

/// Change an app that already exists, in place.
///
/// The AI is handed the app's own source and told what to change, which is why
/// this is quicker and more faithful than describing the whole app again.
/// Revise an app with a live progress display driving the terminal.
///
/// The watched form matters for more than the display: with a sink installed,
/// the authoring child's output is piped rather than inherited, so cargo's
/// warnings stop pouring raw through the screen.
pub(crate) fn revise_app_for_tui_watched(
    source: &Path,
    change: &str,
    provider: &'static dyn agent_provider::AgentProvider,
    output: &Path,
    attachments: &[PathBuf],
    progress: &std::sync::Arc<progress::Progress>,
) -> Result<()> {
    set_progress_sink(Some(std::sync::Arc::clone(progress)));
    let result = revise_app_for_tui(source, change, provider, output, attachments);
    set_progress_sink(None);
    result
}

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
            let Some(name) = file.file_name() else {
                continue;
            };
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
    // Marked as a change so the prompt builder gives it edit instructions
    // rather than build-an-app-from-scratch instructions.
    let request = format!("{CHANGE_MARKER}{change}{attached}");
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

/// `krate account`: who is signed in; `login` and `logout` change it.
fn account_command(action: Option<AccountAction>, json: bool) -> Result<u8> {
    match action {
        None => {
            let identity = github_auth::current();
            if json {
                let value = match &identity {
                    Some(id) => serde_json::json!({
                        "signed_in": true,
                        "login": id.login,
                        "name": id.name,
                        "avatar_url": id.avatar_url,
                    }),
                    None => serde_json::json!({ "signed_in": false }),
                };
                println!("{value}");
            } else {
                match identity {
                    Some(id) => println!("signed in as {}", id.display_name()),
                    None => println!("not signed in -- `krate account login` to sign in"),
                }
            }
            Ok(0)
        }
        Some(AccountAction::Login { json: login_json }) => {
            if json || login_json {
                github_auth::sign_in_json()?;
            } else {
                github_auth::sign_in()?;
            }
            Ok(0)
        }
        Some(AccountAction::Adopt) => {
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            let identity: github_auth::Identity = serde_json::from_str(input.trim())
                .context("the identity on stdin was not valid JSON")?;
            github_auth::adopt(&identity)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "signed_in": true, "login": identity.login })
                );
            } else {
                println!("signed in as {}", identity.display_name());
            }
            Ok(0)
        }
        Some(AccountAction::Logout) => {
            let was = github_auth::sign_out()?;
            if json {
                println!("{}", serde_json::json!({ "signed_out": was }));
            } else if was {
                println!("signed out");
            } else {
                println!("nobody was signed in");
            }
            Ok(0)
        }
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
pub(crate) extern "C" fn handle_interrupt(_signal: libc::c_int) {}

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
    let previous = unsafe {
        libc::signal(
            libc::SIGINT,
            handle_interrupt as *const std::ffi::c_void as libc::sighandler_t,
        )
    };

    // Take the app's stdout rather than letting it into the menu.
    //
    // Apps print machine-readable lines for check-app to assert on --
    // "screensavers:4", "frames:782", "seconds:14". Useful to the checker,
    // meaningless to the person who just closed a screensaver and finds that
    // underneath their menu. stderr stays inherited: that is where a real
    // error would go, and swallowing those would be worse than showing noise.
    let output = std::process::Command::new(exe)
        .arg("run")
        .arg(bundle)
        .arg("--auto-grant")
        // The menu has already said which app is opening and how to get back,
        // so the runtime's own "krate: opened window ..." line is a second
        // copy of that in blunter words.
        .env("KRATE_QUIET_LAUNCH", "1")
        .stdout(std::process::Stdio::piped())
        .output()
        .context("could not start the app")?;
    let status = output.status;
    // Keep what it said for a failure message. An app that exits non-zero
    // usually explains itself on stdout, and that is exactly when the lines
    // are worth showing.
    let said = String::from_utf8_lossy(&output.stdout);

    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, previous);
    }

    // 130 is the shell's convention for "ended by Ctrl-C", which is a normal
    // way to close an app here rather than a failure worth reporting.
    match status.code() {
        Some(0) | Some(130) | None => Ok(()),
        Some(code) => {
            // Now the lines matter: an app that failed usually said why.
            let tail: Vec<&str> = said.lines().rev().take(6).collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n  ");
            if tail.trim().is_empty() {
                anyhow::bail!("the app exited with code {code}");
            }
            anyhow::bail!("the app exited with code {code}:\n\n  {tail}");
        }
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
        check_layout: false,
        // A packed bundle carries its own assets; nothing to override.
        assets_root: None,
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
    // A development manifest points `entry` at the build output; inside a
    // bundle the component is always stored as `code.wasm`. The difference is
    // knowable here, so it is rewritten here -- demanding a second, hand-kept
    // manifest made everyone packing by hand hit the refusal first. Assets,
    // source and the SDK still resolve against the REAL manifest's directory;
    // only the copy that goes into the bundle is touched.
    let manifest_text =
        fs::read_to_string(manifest).with_context(|| format!("read {}", manifest.display()))?;
    let needs_entry_rewrite = manifest_text
        .lines()
        .any(|line| line.trim_start().starts_with("entry =") && !line.contains("code.wasm"));
    let rewritten =
        std::env::temp_dir().join(format!("krate-pack-manifest-{}.toml", std::process::id()));
    let pack_manifest: &Path = if needs_entry_rewrite {
        write_manifest_with_entry(manifest, &rewritten, "code.wasm")?;
        println!(
            "entry points at a build path; the bundle's copy says code.wasm (yours is untouched)"
        );
        &rewritten
    } else {
        manifest
    };
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
        pack_manifest,
        file,
        assets.as_deref(),
        source.as_deref(),
        sdk.as_deref(),
        output,
    )
    .with_context(|| format!("could not pack {}", output.display()))?;
    if needs_entry_rewrite {
        let _ = fs::remove_file(&rewritten);
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum WrapTarget {
    Mac,
    Windows,
    Linux,
}

/// The Unix half of a wrap: a shell prefix that installs Krate if it is
/// missing, then opens the bundle riding behind it. The script copies
/// itself to a temporary .krate name before running, because a `.command`
/// extension is not one `krate run` recognizes by name and the content
/// sniff reads the script bytes, not the zip behind them.
///
/// Everything below the final `exit` is the app's own bytes; the shell
/// reads a script line by line as it executes, so it never parses them.
fn wrap_prefix_unix(app_name: &str, stem: &str) -> String {
    format!(
        r#"#!/bin/sh
# {app_name} -- a Krate app, wrapped for a friend.
#
# Double-click me (macOS) or run me (Linux). If Krate is not installed,
# I install it once -- a small free player from krate.tech, checksums
# verified -- and then the app opens. Every later .krate file anyone
# sends you just opens.
#
# macOS note for the sender: a downloaded script may need one
# right-click -> Open the first time. That is Apple checking the
# messenger, not the app; Krate itself still asks before the app runs.
set -u
find_krate() {{
  command -v krate 2>/dev/null && return 0
  for c in /usr/local/bin/krate "$HOME/.local/bin/krate"; do
    if [ -x "$c" ]; then printf '%s\n' "$c"; return 0; fi
  done
  return 1
}}
krate_bin="$(find_krate || true)"
if [ -z "$krate_bin" ]; then
  echo "This app runs on Krate, a small free player. Installing it once..."
  curl -fsSL https://krate.tech/install.sh | sh || {{
    echo "Krate did not install. The friendly instructions live at krate.tech/open"
    exit 1
  }}
  krate_bin="$(find_krate || true)"
  if [ -z "$krate_bin" ]; then
    echo "Krate installed somewhere this script cannot see; open krate.tech/open"
    exit 1
  fi
fi
tmp="${{TMPDIR:-/tmp}}/{stem}-$$.krate"
cp "$0" "$tmp"
"$krate_bin" run "$tmp" --consent
status=$?
rm -f "$tmp"
exit $status
# ---- the app itself follows; nothing below this line is a script ----
"#
    )
}

/// The Windows half: a .cmd prefix with CRLF line endings (cmd.exe is
/// unreliable with bare LF around labels), ending in `exit /b` so the
/// interpreter never reads the bundle bytes behind it. After a fresh
/// install the new PATH entry is invisible to the already-running cmd, so
/// the installer's default landing spot is checked explicitly.
fn wrap_prefix_windows(app_name: &str, stem: &str) -> String {
    let script = format!(
        r#"@echo off
rem {app_name} -- a Krate app, wrapped for a friend. Double-click me.
rem If Krate is missing I install it once (a small free player), then open.
setlocal
set "KRATE_BIN=krate"
where krate >nul 2>nul
if %ERRORLEVEL%==0 goto run
echo This app runs on Krate, a small free player. Installing it once...
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -Command "irm https://krate.tech/install.ps1 | iex"
where krate >nul 2>nul
if %ERRORLEVEL%==0 goto run
if exist "%LOCALAPPDATA%\Krate\bin\krate.exe" (
  set "KRATE_BIN=%LOCALAPPDATA%\Krate\bin\krate.exe"
  goto run
)
echo Krate did not install. The friendly instructions live at krate.tech/open
pause
exit /b 1
:run
copy /b "%~f0" "%TEMP%\{stem}.krate" >nul
"%KRATE_BIN%" run "%TEMP%\{stem}.krate" --consent
set "STATUS=%ERRORLEVEL%"
del "%TEMP%\{stem}.krate" >nul 2>nul
exit /b %STATUS%
"#
    );
    script.replace('\n', "\r\n")
}

/// The opener that goes inside a Mac gift's `.app`.
///
/// It sits at `Contents/MacOS/open` and finds the app file BESIDE the
/// bundle, never inside it. That placement is the whole design, and it is
/// forced by how Apple seals a signed bundle: dropping a payload into
/// `Contents/Resources/` after signing breaks the seal outright
/// (`codesign --verify` answers "a sealed resource is missing or invalid",
/// measured 2026-08-31). A sidecar leaves the signature intact, so ONE
/// opener can be notarized in CI and copied into every gift afterwards.
///
/// Which means this script must not name a particular app: it FINDS the
/// `.krate` beside it. A per-gift script would need a per-gift signature,
/// and the sender's laptop has no certificate.
fn mac_opener_script() -> String {
    r#"#!/bin/sh
# The opener for a Krate gift. It installs the free player once if it is
# missing, then opens the app file sitting next to this bundle.
set -u
here="$(cd "$(dirname "$0")/../../.." && pwd)"
# The gift's app file is whichever .krate shares this folder. Named by
# search, not baked in, so one notarized opener serves every gift.
app=""
for candidate in "$here"/*.krate; do
  if [ -f "$candidate" ]; then app="$candidate"; break; fi
done
find_krate() {
  command -v krate 2>/dev/null && return 0
  for c in /usr/local/bin/krate "$HOME/.local/bin/krate"; do
    if [ -x "$c" ]; then printf '%s\n' "$c"; return 0; fi
  done
  return 1
}
krate_bin="$(find_krate || true)"
if [ -z "$krate_bin" ]; then
  # No player yet. Say so in a window -- there is no console behind a
  # double-clicked .app, so a printed line would go nowhere.
  osascript -e 'display dialog "This app runs on Krate, a small free player (about 24 MB).\n\nInstall it once, and every Krate app anyone sends you from now on just opens." buttons {"Not now","Install once"} default button "Install once" with title "Krate"' \
    | grep -q "Install once" || exit 0
  curl -fsSL https://krate.tech/install.sh | sh >/dev/null 2>&1
  krate_bin="$(find_krate || true)"
  if [ -z "$krate_bin" ]; then
    osascript -e 'display dialog "Krate could not install itself.\n\nThe steps are at krate.tech/open" buttons {"OK"} with title "Krate"' >/dev/null 2>&1
    open "https://krate.tech/open"
    exit 1
  fi
fi
if [ ! -f "$app" ]; then
  osascript -e 'display dialog "The app file is missing.\n\nKeep this opener and the .krate file together in the same folder." buttons {"OK"} with title "Krate"' >/dev/null 2>&1
  exit 1
fi
exec "$krate_bin" run "$app" --consent
"#
    .to_string()
}

/// The Mac gift: a folder holding a signed-and-notarizable `.app` opener
/// and the app's own `.krate` beside it.
///
/// Why not the single self-installing script the other platforms get:
/// macOS refuses it. A downloaded `.command` is Gatekeeper-rejected ("no
/// usable signature"), signing it only moves the verdict to "Unnotarized
/// Developer ID", and it can never be notarized because `stapler` will not
/// process one -- "Stapler is incapable of working with Terminal shell
/// script files", Apple's own words. The receiver is left doing a
/// right-click -> Open ritual on a security warning, which is exactly the
/// moment a stranger decides the sender sent junk (K-211).
///
/// An `.app` is a shape Apple will notarize, so this one can be signed in
/// CI and trusted on arrival.
fn wrap_mac_folder(bundle: &Path, app_name: &str, stem: &str, output: Option<&Path>) -> Result<u8> {
    let out_dir = match output {
        Some(path) => path.to_path_buf(),
        None => bundle
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}-for-Mac")),
    };
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("could not replace {}", out_dir.display()))?;
    }

    let app_file = format!("{stem}.krate");
    let opener = out_dir.join(format!("Open {app_name}.app"));

    // Prefer the opener the release notarized. A gift is made on a sender's
    // laptop, where there is no certificate and no notary account, so an
    // opener written here is unsigned and the receiver meets the warning
    // this whole design exists to remove. The installed one carries Apple's
    // stapled ticket, and copying a bundle preserves it.
    //
    // Its name is generic ("Open") because one notarized bundle serves every
    // gift; renaming the .app on copy is cosmetic and leaves the seal alone,
    // but the executable and Info.plist inside must not be touched.
    if let Some(shipped) = shipped_gift_opener() {
        fs::create_dir_all(&out_dir)
            .with_context(|| format!("could not create {}", out_dir.display()))?;
        let status = std::process::Command::new("cp")
            .arg("-R")
            .arg(&shipped)
            .arg(&opener)
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return finish_mac_gift(bundle, &out_dir, &app_file, app_name, true);
        }
        // A failed copy is not a reason to refuse the gift; fall through and
        // write the plain opener, which works but shows the warning.
    }

    let macos_dir = opener.join("Contents/MacOS");
    fs::create_dir_all(&macos_dir)
        .with_context(|| format!("could not create {}", macos_dir.display()))?;

    fs::write(
        opener.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Open {app_name}</string>
  <key>CFBundleDisplayName</key><string>Open {app_name}</string>
  <key>CFBundleIdentifier</key><string>tech.krate.gift</string>
  <key>CFBundleExecutable</key><string>open</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>LSMinimumSystemVersion</key><string>10.13</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
"#
        ),
    )
    .context("could not write the opener's Info.plist")?;

    let script = mac_opener_script();
    let exe = macos_dir.join("open");
    fs::write(&exe, script).context("could not write the opener")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&exe)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(&exe, perms)?;
    }

    finish_mac_gift(bundle, &out_dir, &app_file, app_name, false)
}

/// The notarized opener that shipped with this install, if it is there.
///
/// It sits beside the binary in the release layout, and inside the app
/// bundle when Krate was installed as `Krate Player.app`.
fn shipped_gift_opener() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.join("Krate Opener.app"),
        dir.join("../Resources/Krate Opener.app"),
        dir.join("../../../Krate Opener.app"),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("Contents/MacOS/open").is_file())
}

/// The half of a Mac gift that is the same whichever opener it got: the app
/// file beside it, a read-me, and the sender's summary.
fn finish_mac_gift(
    bundle: &Path,
    out_dir: &Path,
    app_file: &str,
    app_name: &str,
    notarized: bool,
) -> Result<u8> {
    fs::copy(bundle, out_dir.join(app_file))
        .with_context(|| format!("could not copy {}", bundle.display()))?;

    // One line the receiver can read without opening anything.
    fs::write(
        out_dir.join("Read me first.txt"),
        format!(
            "{app_name}\n\n\
             Double-click the opener and the app opens.\n\n\
             The first time, it installs Krate -- a small free player, about\n\
             24 MB, the same idea as a video player. After that, every Krate\n\
             app anyone sends you just opens.\n\n\
             Keep these two files together in this folder.\n\n\
             More at krate.tech/open\n"
        ),
    )
    .context("could not write the read-me")?;

    println!("Gift written: {}", out_dir.display());
    println!("  for a friend on Mac who does not have Krate yet.");
    println!("  Double-clicking the opener installs the player once, then opens {app_name}.");
    println!("  The player is planted, never bundled: their next .krate just opens too.");
    println!("  Send the whole folder (zip it, or drop it in a shared drive).");
    if !notarized {
        println!();
        println!("  Note: this opener is not notarized, so your friend will see one");
        println!("  security warning. A Krate installed from krate.tech ships the");
        println!("  notarized opener and this note goes away.");
    }
    Ok(0)
}

fn wrap_bundle(bundle: &Path, target: WrapTarget, output: Option<&Path>) -> Result<u8> {
    if !krate_bundle::is_bundle_path(bundle) {
        anyhow::bail!(
            "not a .krate bundle: {}\nPack one first with `krate pack` (or `krate create`).",
            bundle.display()
        );
    }
    let opened = krate_bundle::open(bundle)
        .with_context(|| format!("could not open {}", bundle.display()))?;
    let app_name = opened.manifest().app.name.clone();
    drop(opened);
    let stem = card_file_stem(&app_name);

    // macOS gets a folder with a notarizable opener, not a script: a
    // downloaded .command cannot be made to pass Gatekeeper at all (K-211).
    if target == WrapTarget::Mac {
        return wrap_mac_folder(bundle, &app_name, &stem, output);
    }

    let (prefix, suffix, friend) = match target {
        WrapTarget::Mac => unreachable!("handled above"),
        WrapTarget::Linux => (wrap_prefix_unix(&app_name, &stem), "sh", "Linux"),
        WrapTarget::Windows => (wrap_prefix_windows(&app_name, &stem), "cmd", "Windows"),
    };

    let out_path = match output {
        Some(path) => path.to_path_buf(),
        None => bundle
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{stem}-for-{friend}.{suffix}")),
    };

    let bundle_bytes =
        fs::read(bundle).with_context(|| format!("could not read {}", bundle.display()))?;
    let mut wrap_bytes = prefix.into_bytes();
    wrap_bytes.extend_from_slice(&bundle_bytes);
    fs::write(&out_path, &wrap_bytes)
        .with_context(|| format!("could not write {}", out_path.display()))?;
    // Executable where the bit can survive the trip (USB, a zip, AirDrop
    // between Macs). Channels that strip it leave a file that needs one
    // `sh <file>` or a chmod -- a real limit of any script wrap, and part
    // of why the wrap is the courtesy option, not the default share.
    #[cfg(unix)]
    if target != WrapTarget::Windows {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&out_path)?.permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(&out_path, perms)?;
    }

    // The wrap is still a bundle: verify the reading `krate run` will do
    // before claiming success.
    let reopened = krate_bundle::open(&out_path)
        .with_context(|| "the wrap did not open as a bundle afterwards")?;
    anyhow::ensure!(
        reopened.manifest().app.name == app_name,
        "the wrap opened as a different app"
    );
    drop(reopened);

    let kb = (wrap_bytes.len() as f64 / 1024.0).ceil() as u64;
    println!("Wrap written: {} ({kb} KB)", out_path.display());
    println!("  for a friend on {friend} who does not have Krate yet.");
    println!("  First open installs Krate once (a small verified download), then {app_name} opens.");
    println!("  The player is planted, never bundled: their next .krate just opens too.");
    match target {
        WrapTarget::Mac => println!(
            "  Heads up: macOS may want one right-click -> Open on a downloaded script."
        ),
        WrapTarget::Windows => println!(
            "  Heads up: SmartScreen may ask once -- More info, then Run anyway."
        ),
        WrapTarget::Linux => {}
    }
    Ok(0)
}

/// One sentence of trust for the card's caption: what the app may touch, in
/// the same plain words the consent prompt uses, ending with the guarantee
/// that makes the sentence worth printing.
fn card_trust_line(manifest: &krate_manifest::Manifest) -> String {
    let caps = match manifest.declared_capabilities() {
        Ok(caps) => caps,
        Err(_) => return "shows what it may touch before it runs".to_string(),
    };
    let mut labels: Vec<String> = Vec::new();
    for cap in &caps {
        // Default-granted plumbing (io.args, io.stdout and friends) is never
        // asked about in the consent prompt, so it does not belong on the
        // one line a stranger reads before daring to tap. The window stays
        // even though it is default-granted: "can open a window · nothing
        // else" is the whole sentence, and a card that hides the window
        // would claim the app does nothing visible at all.
        if cap.is_default_granted() && !(cap.module() == "ui" && cap.action() == "window") {
            continue;
        }
        let label = human_label(cap);
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if labels.is_empty() {
        return "asks for nothing beyond the basics".to_string();
    }
    format!("can {} · nothing else", labels.join(" · "))
}

/// The card's default file stem: the app's name with the spaces closed up
/// and anything a filesystem would object to dropped. "Rate card" becomes
/// RateCard, which is the name a person forwards without thinking about it.
fn card_file_stem(name: &str) -> String {
    let mut stem = String::new();
    for word in name.split_whitespace() {
        let mut chars = word.chars().filter(|c| c.is_alphanumeric());
        if let Some(first) = chars.next() {
            stem.extend(first.to_uppercase());
            stem.extend(chars);
        }
    }
    if stem.is_empty() {
        stem.push_str("App");
    }
    stem
}

/// Read a PNG into host pixels. Only the shapes our own `--shoot` writer and
/// ordinary screenshots produce -- 8-bit RGB and RGBA -- because the input
/// is a picture Krate itself just took, not the open web.
fn read_png_rgba(path: &Path) -> Result<krate_adapter_common::ui::ImagePixels> {
    let file = fs::File::open(path)
        .with_context(|| format!("could not open the still at {}", path.display()))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .with_context(|| format!("{} is not a readable PNG", path.display()))?;
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or_default()];
    let info = reader
        .next_frame(&mut buf)
        .with_context(|| format!("could not decode the still at {}", path.display()))?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(pixels.len() / 3 * 4);
            for px in pixels.chunks_exact(3) {
                out.extend_from_slice(px);
                out.push(0xFF);
            }
            out
        }
        other => anyhow::bail!(
            "the still at {} is {other:?}; the card face needs 8-bit RGB or RGBA",
            path.display()
        ),
    };
    krate_adapter_common::ui::ImagePixels::new(info.width, info.height, rgba)
        .map_err(|err| anyhow::anyhow!("the still could not become pixels: {err}"))
}

/// Compose the card's face: the app's still with a caption strip under it --
/// filename and size on the first line, krate.tech/open on its right, and
/// the trust line beneath. Returns encoded PNG bytes ready to carry the
/// bundle behind them.
fn compose_card_face(
    shot: &krate_adapter_common::ui::ImagePixels,
    name_label: &str,
    size_label: &str,
    trust: &str,
) -> Result<Vec<u8>> {
    use krate_adapter_common::vector_text::{
        draw_canvas_text_styled, measure_canvas_text_styled, CanvasTarget, CanvasTextStyle,
    };

    let width = shot.width;
    // The strip scales with the shot so a 2x HiDPI still gets a 2x caption,
    // clamped so a tiny test image still has room for two readable lines.
    let bar = ((shot.height as f32) * 0.11).round().clamp(76.0, 152.0) as u32;
    let height = shot.height + bar;

    // Ground everything in the site's panel color, then lay the shot on top.
    const BAR_BG: u32 = 0xFF0F1012;
    const HAIRLINE: u32 = 0xFF1F2228;
    const INK: u32 = 0xFFFFFFFF;
    const MUTED: u32 = 0xFF8B8E94;
    const QUIET: u32 = 0xFF5C5F66;
    let mut buffer = vec![BAR_BG; (width as usize) * (height as usize)];
    for row in 0..shot.height as usize {
        for col in 0..width as usize {
            let src = (row * width as usize + col) * 4;
            let px = 0xFF000000
                | (u32::from(shot.rgba[src]) << 16)
                | (u32::from(shot.rgba[src + 1]) << 8)
                | u32::from(shot.rgba[src + 2]);
            buffer[row * width as usize + col] = px;
        }
    }
    let k = bar as f32 / 88.0;
    let hairline_px = (k.round() as usize).max(1);
    for row in 0..hairline_px {
        let y = shot.height as usize + row;
        let start = y * width as usize;
        buffer[start..start + width as usize].fill(HAIRLINE);
    }

    let pad = (bar as f32 * 0.18).round();
    let name_size = bar as f32 * 0.22;
    let meta_size = bar as f32 * 0.19;
    let line1 = shot.height as f32 + bar as f32 * 0.42;
    let line2 = shot.height as f32 + bar as f32 * 0.78;
    let bold = CanvasTextStyle {
        weight: 500,
        ..CanvasTextStyle::default()
    };
    let plain = CanvasTextStyle::default();

    let measure = |text: &str, size: f32, style: CanvasTextStyle| -> f32 {
        measure_canvas_text_styled(text, size, style)
            .map(|m| m.width)
            .unwrap_or(0.0)
    };
    // A caption that does not fit is cut with an ellipsis rather than drawn
    // off the edge; the full words are one `--dump-caps` away.
    let elide = |text: &str, size: f32, style: CanvasTextStyle, avail: f32| -> String {
        if measure(text, size, style) <= avail {
            return text.to_string();
        }
        let mut out: String = text.to_string();
        while !out.is_empty() {
            out.pop();
            let candidate = format!("{}…", out.trim_end());
            if measure(&candidate, size, style) <= avail {
                return candidate;
            }
        }
        String::new()
    };

    // On a host with no usable fonts these draws report false and the strip
    // stays a colored band; the card still works, it just says less. The
    // caller mentions it rather than failing the whole card over a caption.
    let mut drew = true;
    macro_rules! target {
        () => {
            CanvasTarget {
                buffer: &mut buffer,
                width,
                height,
            }
        };
    }
    drew &= draw_canvas_text_styled(target!(), name_label, pad, line1, name_size, INK, bold);
    let name_w = measure(name_label, name_size, bold);
    drew &= draw_canvas_text_styled(
        target!(),
        size_label,
        pad + name_w + name_size * 0.6,
        line1,
        meta_size,
        MUTED,
        plain,
    );
    const OPEN_URL: &str = "krate.tech/open";
    let url_w = measure(OPEN_URL, meta_size, plain);
    drew &= draw_canvas_text_styled(
        target!(),
        OPEN_URL,
        (width as f32 - pad - url_w).max(pad),
        line1,
        meta_size,
        QUIET,
        plain,
    );
    // When the trust line must shrink, the capability list shrinks and the
    // guarantee survives: "· nothing else" is the reason the line exists,
    // so it is the last thing allowed to disappear.
    let avail = width as f32 - pad * 2.0;
    const TAIL: &str = " · nothing else";
    let trust_fit = if measure(trust, meta_size, plain) <= avail {
        trust.to_string()
    } else if let Some(body) = trust.strip_suffix(TAIL) {
        let tail_w = measure(TAIL, meta_size, plain);
        format!("{}{TAIL}", elide(body, meta_size, plain, (avail - tail_w).max(0.0)))
    } else {
        elide(trust, meta_size, plain, avail)
    };
    drew &= draw_canvas_text_styled(target!(), &trust_fit, pad, line2, meta_size, MUTED, plain);
    if !drew {
        eprintln!(
            "note: no usable system fonts, so the caption strip is blank; the card still works"
        );
    }

    let mut rgba = vec![0u8; buffer.len() * 4];
    for (chunk, word) in rgba.chunks_exact_mut(4).zip(buffer.iter()) {
        chunk[0] = (word >> 16) as u8;
        chunk[1] = (word >> 8) as u8;
        chunk[2] = *word as u8;
        chunk[3] = (word >> 24) as u8;
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .context("could not start the card's PNG")?;
        writer
            .write_image_data(&rgba)
            .context("could not encode the card's PNG")?;
    }
    Ok(out)
}

fn card_bundle(
    bundle: &Path,
    output: Option<&Path>,
    png_copy: bool,
    settle_ms: u64,
    shot: Option<&Path>,
) -> Result<u8> {
    if !krate_bundle::is_bundle_path(bundle) {
        anyhow::bail!(
            "not a .krate bundle: {}\nPack one first with `krate pack` (or `krate create`).",
            bundle.display()
        );
    }
    let opened = krate_bundle::open(bundle)
        .with_context(|| format!("could not open {}", bundle.display()))?;
    let app_name = opened.manifest().app.name.clone();
    let trust = card_trust_line(opened.manifest());
    drop(opened);

    // The face: an existing still, or the app photographed by the same
    // `run --shoot` a person would use by hand. A separate process rather
    // than an in-process run, so a misbehaving app cannot take the card
    // command down with it.
    let temp = tempfile::TempDir::new().context("could not make a working folder")?;
    let shot_path: PathBuf = match shot {
        Some(path) => path.to_path_buf(),
        None => {
            let dest = temp.path().join("face.png");
            let me = std::env::current_exe().context("could not find the krate binary")?;
            let status = std::process::Command::new(&me)
                .arg("run")
                .arg(bundle)
                .arg("--shoot")
                .arg(&dest)
                .arg("--auto-grant")
                .env("KRATE_SHOOT_AFTER_MS", settle_ms.to_string())
                // The app's own prints belong to the app, not to the card
                // summary. Errors still come through on stderr.
                .stdout(std::process::Stdio::null())
                .status()
                .context("could not run the app to photograph it")?;
            if !status.success() || !dest.exists() {
                anyhow::bail!(
                    "the app did not paint a frame to photograph.\n\
                     If it needs longer to settle, try --settle-ms 3000; or pass \
                     an existing picture with --shot."
                );
            }
            dest
        }
    };
    let face_pixels = read_png_rgba(&shot_path)?;

    let bundle_bytes =
        fs::read(bundle).with_context(|| format!("could not read {}", bundle.display()))?;
    let stem = card_file_stem(&app_name);
    let size_label = format!("{} KB", (bundle_bytes.len() as f64 / 1024.0).ceil() as u64);
    let out_path = match output {
        Some(path) => path.to_path_buf(),
        None => {
            let parent = bundle.parent().unwrap_or_else(|| Path::new("."));
            let candidate = parent.join(format!("{stem}.krate"));
            // Carding RateCard.krate in place must not overwrite the input.
            if candidate == bundle || fs::canonicalize(&candidate).ok() == fs::canonicalize(bundle).ok()
            {
                parent.join(format!("{stem}-card.krate"))
            } else {
                candidate
            }
        }
    };
    // The caption names the file the person will actually forward, so a
    // custom --output shows its own name, not a guessed one.
    let name_label = out_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("{stem}.krate"));
    let face = compose_card_face(&face_pixels, &name_label, &size_label, &trust)?;
    let mut card_bytes = face;
    card_bytes.extend_from_slice(&bundle_bytes);
    fs::write(&out_path, &card_bytes)
        .with_context(|| format!("could not write {}", out_path.display()))?;

    // Verify the two readings of the one file before claiming success: the
    // bundle half by opening it, the picture half by decoding it. A card
    // that only one kind of program can read is not a card.
    let reopened = krate_bundle::open(&out_path)
        .with_context(|| "the card did not open as a bundle afterwards")?;
    anyhow::ensure!(
        reopened.manifest().app.name == app_name,
        "the card opened as a different app"
    );
    drop(reopened);
    let face_check = read_png_rgba(&out_path)
        .context("the card did not decode as a picture afterwards")?;

    if png_copy {
        let png_path = out_path.with_extension("png");
        fs::write(&png_path, &card_bytes)
            .with_context(|| format!("could not write {}", png_path.display()))?;
        println!("Also wrote {} -- the same bytes, for places that only take pictures.", png_path.display());
    }

    println!("Card written: {} ({} KB)", out_path.display(), (card_bytes.len() as f64 / 1024.0).ceil() as u64);
    println!("  the picture: {}x{} PNG -- opens in any image viewer", face_check.width, face_check.height);
    println!("  the app:     {app_name}, {size_label} -- opens in Krate by double-click");
    println!("  it says:     {trust}");
    println!("Send it as a file (mail, AirDrop, a chat's paperclip). Sent as a \"photo\", chats re-encode the image and the app half is lost.");
    Ok(0)
}

pub(crate) fn publish_bundle_for_tui(bundle: &Path, description: Option<&str>) -> Result<()> {
    let code = publish_bundle(bundle, None, description, None, None, None, false)?;
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
    name_override: Option<&str>,
    shot_override: Option<&Path>,
    icon: Option<&Path>,
    unlisted: bool,
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
    let app_name = name_override
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            krate_bundle::open(bundle)
                .ok()
                .map(|opened| opened.manifest().app.name.clone())
        })
        .unwrap_or_default();
    // The error message told people to run `krate publish` and be asked, so
    // it had better ask. Signing in here rather than failing with advice is
    // the difference between one command and a scavenger hunt.
    let identity = match github_auth::current() {
        Some(identity) => Some(identity),
        None => {
            // Behind a pipe (the studio, MCP, CI) there is no one to read a
            // printed code: the browser would open on GitHub's code page
            // while the code itself vanished into the captured stdout, and
            // this command would sit polling for 15 minutes. Fail fast in
            // words the caller can act on instead. (K-210)
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() {
                anyhow::bail!("not signed in. Sign in first, then publish again.");
            }
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
    if unlisted {
        // The link works; the gallery never lists it.
        request = request.set("X-Krate-Unlisted", "1");
    }
    if !app_name.is_empty() {
        request = request.set("X-Krate-Name", &app_name);
    }
    if let Some(description) = description {
        request = request.set("X-Krate-Description", description);
    }
    request = request.set(
        "X-Krate-Category",
        classify_app(&app_name, description.unwrap_or("")),
    );
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
    // The store lives on previews. Render the app's own first frame headless
    // and attach it; every failure here is swallowed on purpose -- the app
    // is published and the listing works either way.
    // The response's `id` is the content hash the shot route keys on; the
    // url may be the short alias and would 404 the upload.
    let shot_id = extract_json_string(&body, "id");
    if let (Some(id), Some(identity)) = (shot_id.as_deref(), &identity) {
        let upload_png = |route: &str, png: &[u8], limit: usize| {
            if png.is_empty() || png.len() > limit {
                return;
            }
            let _ = ureq::post(&format!("{}/{route}/{id}", hub.trim_end_matches('/')))
                .set("Authorization", &format!("Bearer {}", identity.token))
                .set("Content-Type", "image/png")
                .send_bytes(png);
        };
        // A hand-picked screenshot wins; otherwise render the app's own
        // first frame headless.
        if let Some(path) = shot_override {
            if let Ok(png) = fs::read(path) {
                upload_png("shot", &png, 2 * 1024 * 1024);
            }
        } else {
            let shot =
                std::env::temp_dir().join(format!("krate-publish-shot-{}.png", std::process::id()));
            let ok = std::process::Command::new(
                std::env::current_exe().unwrap_or_else(|_| "krate".into()),
            )
            .arg("run")
            .arg(bundle)
            .args(["--shoot"])
            .arg(&shot)
            .args(["--auto-grant", "--", "quick"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
            if ok {
                if let Ok(png) = fs::read(&shot) {
                    upload_png("shot", &png, 2 * 1024 * 1024);
                }
                let _ = fs::remove_file(&shot);
            }
        }
        if let Some(path) = icon {
            if let Ok(png) = fs::read(path) {
                upload_png("icon", &png, 512 * 1024);
            }
        }
    }
    // The hub degrades rather than dies when a metadata write cannot land
    // (a KV quota day): the app is live at its URL but the gallery row is
    // deferred, and it says so in a `note`. Swallowing that note showed a
    // clean success while the gallery quietly missed the app.
    if let Some(note) = extract_json_string(&body, "note") {
        println!("note: {note}");
    }
    println!("Published. Anyone can run it with:");
    println!("  krate run {url}");
    Ok(0)
}

/// Which shelf an app belongs on, from its own words. A fixed list on
/// purpose: free-text categories fragment a store into thirty shelves of
/// one app each.
fn classify_app(name: &str, description: &str) -> &'static str {
    let text = format!("{name} {description}").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    // Checked before the shelves: "dashboard" contains "dash" and landed a
    // workout dashboard on the games shelf.
    if has(&["dashboard"]) {
        return "productivity";
    }
    if has(&[
        "game", "dash", "runner", "puzzle", "arcade", "flip", "dice", "snake", "invader", "nova",
        "shooter", "space",
    ]) {
        "games"
    } else if has(&[
        "track", "habit", "journal", "todo", "note", "timer", "clock", "pomodoro", "focus",
        "streak", "list",
    ]) {
        "productivity"
    } else if has(&[
        "calc", "convert", "split", "counter", "invoice", "unit", "measure",
    ]) {
        "tools"
    } else if has(&[
        "draw", "paint", "photo", "image", "music", "player", "sound", "color",
    ]) {
        "media"
    } else if has(&["learn", "flash", "quiz", "study", "practice"]) {
        "learning"
    } else {
        "apps"
    }
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

/// Each sheet of a spreadsheet attachment written as CSV beside it.
///
/// Returns the file names written (not paths). Failures return empty: an
/// unreadable spreadsheet is the agent's problem to report, not a reason to
/// refuse the whole request.
fn spreadsheet_to_csvs(path: &Path) -> Vec<String> {
    let is_sheet = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "xlsx" | "xls" | "xlsm" | "ods"
            )
        })
        .unwrap_or(false);
    if !is_sheet {
        return Vec::new();
    }
    let Ok(mut workbook) = calamine::open_workbook_auto(path) else {
        return Vec::new();
    };
    use calamine::Reader;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sheet".to_string());
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut written = Vec::new();
    for sheet_name in workbook.sheet_names().to_vec() {
        let Ok(range) = workbook.worksheet_range(&sheet_name) else {
            continue;
        };
        if range.is_empty() {
            continue;
        }
        let safe: String = sheet_name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let file_name = format!("{stem}.{safe}.csv");
        let mut csv = String::new();
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| {
                    let text = cell.to_string();
                    if text.contains(',') || text.contains('"') || text.contains('\n') {
                        format!("\"{}\"", text.replace('"', "\"\""))
                    } else {
                        text
                    }
                })
                .collect();
            csv.push_str(&cells.join(","));
            csv.push('\n');
        }
        if fs::write(parent.join(&file_name), csv).is_ok() {
            written.push(file_name);
        }
    }
    written
}

/// Everything about one session, in one file the person can inspect before
/// it goes anywhere (K-128).
///
/// Deliberately a plain zip of plain text: a support report nobody can read
/// is a support report nobody trusts sending. What goes in is exactly what
/// the consent dialog names -- no more, and nothing gathered from outside
/// the session's own workspace.
fn report_command(session: &str, output: Option<&Path>) -> Result<u8> {
    if !session
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("that is not a session id");
    }
    let studio = krate_home().join("studio");
    let session_file = studio.join("sessions").join(format!("{session}.json"));
    if !session_file.exists() {
        anyhow::bail!("no session by that name on this machine");
    }
    let out = output.map(|p| p.to_path_buf()).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("krate-report-{session}.krate-report"))
    });

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if let Ok(bytes) = fs::read(&session_file) {
        files.push(("session.json".to_string(), bytes));
    }

    // The workspace the session's last build used: the agent transcript and
    // the code it had written are the evidence that actually explains a
    // stuck build.
    //
    // Two ways to find it, and the second one is why this exists. The
    // original only read a path out of the session text, printed by
    // WorkspaceKeeper when a TEMP workspace is kept after a failure. The
    // Studio never uses a temp workspace -- it passes --work-dir so a retry
    // resumes from the code already written (K-129) -- so that line is never
    // printed and EVERY report sent from the Studio arrived carrying only
    // the chat, with the transcript that names the real failure left on the
    // person's disk. Three reports came in that way before anyone noticed
    // (K-185).
    //
    // The Studio's path is not a guess: studio/src/main.rs builds it as
    // studio_dir()/builds/<session>, from the same session id this command
    // was given.
    let session_text = String::from_utf8_lossy(&files[0].1).to_string();
    let studio_workspace = studio.join("builds").join(session);
    let workspace = report_workspace_from(&session_text)
        .or_else(|| studio_workspace.is_dir().then_some(studio_workspace));
    if let Some(dir) = workspace {
        // Two levels, because the Studio's workspace holds trace.jsonl at the
        // top and the app -- with its transcript and source -- in a folder
        // named after the app. Checked on a real build directory rather than
        // assumed: looking only at the top would have collected the trace and
        // missed the transcript, which is the file that names the failure.
        let top = dir.clone();
        let mut roots = vec![dir.clone()];
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    roots.push(entry.path());
                }
            }
        }
        for dir in roots {
            let is_top = dir == top;
        for name in [
            ".agent-transcript.txt",
            // Written by every Studio build (KRATE_TRACE), and it carries the
            // timing spine: which phase, how long, what check-app said.
            "trace.jsonl",
            "src/lib.rs",
            "Cargo.toml",
            "manifest.toml",
        ] {
            if let Ok(bytes) = fs::read(dir.join(name)) {
                // The transcript can be megabytes; the tail is where a
                // stall shows itself.
                let bytes = if bytes.len() > 2 * 1024 * 1024 {
                    bytes[bytes.len() - 2 * 1024 * 1024..].to_vec()
                } else {
                    bytes
                };
                // Keep the subfolder in the name so two levels cannot
                // collide, and so the reader can see where each file lived.
                let label = match dir.file_name().and_then(|n| n.to_str()) {
                    Some(sub) if !is_top => format!("workspace/{sub}/{name}"),
                    _ => format!("workspace/{name}"),
                };
                if !files.iter().any(|(existing, _)| *existing == label) {
                    files.push((label, bytes));
                }
            }
        }
        }
    }

    // What this machine is, in the words that explain most failures.
    let mut about = String::new();
    about.push_str(&format!("krate: {}\n", krate_version()));
    about.push_str(&format!(
        "os: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    for tool in ["rustc", "cargo", "cargo-component"] {
        let found = agent_provider::which_on_path(tool)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not on PATH".to_string());
        about.push_str(&format!("{tool}: {found}\n"));
    }
    // The same three states the AI picker shows, not a yes/no.
    //
    // This said only "installed" or "missing", and "installed" covers both a
    // tool that works and one that is signed out -- which is the difference
    // between a build that runs and a build that dies in one second. Two
    // outside reports in a row could not be told apart from their own report
    // files, and both needed a round trip to the person to learn something
    // their machine already knew (K-182, K-187).
    for provider in agent_provider::PROVIDERS {
        let state = match probe_with_cache(*provider) {
            agent_provider::Readiness::Working => "working".to_string(),
            agent_provider::Readiness::NotReady { summary, .. } => {
                let summary = summary.trim();
                if summary.is_empty() {
                    "installed but not ready".to_string()
                } else {
                    format!("installed but not ready -- {summary}")
                }
            }
            agent_provider::Readiness::Missing => "missing".to_string(),
        };
        about.push_str(&format!("agent {}: {state}\n", provider.name()));
    }
    // Where the engine is, which is the other thing a report could not say.
    // A Studio user has no `krate` on PATH at all (K-188), so "which krate"
    // is not a question they can answer.
    if let Ok(exe) = std::env::current_exe() {
        about.push_str(&format!("engine: {}\n", exe.display()));
    }
    about.push_str(&format!(
        "engine on PATH: {}\n",
        agent_provider::which_on_path("krate")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "no".to_string())
    ));
    files.push(("about.txt".to_string(), about.into_bytes()));

    let file = fs::File::create(&out).with_context(|| format!("write {}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in &files {
        use std::io::Write;
        zip.start_file(name, options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    println!("{}", out.display());
    Ok(0)
}

/// Send one report file to the hub's support door.
fn report_send_command(
    report: &Path,
    session: &str,
    note: &str,
    hub_override: Option<&str>,
) -> Result<u8> {
    let bytes = fs::read(report)
        .with_context(|| format!("could not read the report {}", report.display()))?;
    if bytes.len() > 12 * 1024 * 1024 {
        anyhow::bail!("that report is over 12 MB; send the session's own log instead");
    }
    let identity = match github_auth::current() {
        Some(identity) => identity,
        None => {
            println!("Sending a report puts your name on it, so it needs a sign-in.");
            github_auth::sign_in().map_err(|err| anyhow::anyhow!("could not sign in: {err}"))?
        }
    };
    let hub = hub_override
        .map(str::to_string)
        .or_else(|| std::env::var("KRATE_HUB_URL").ok())
        .unwrap_or_else(|| DEFAULT_HUB_URL.to_string());
    let response = ureq::post(&format!("{}/report", hub.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {}", identity.token))
        .set("Content-Type", "application/zip")
        .set("X-Krate-Session", session)
        .set("X-Krate-Version", krate_version())
        .set("X-Krate-Os", std::env::consts::OS)
        .set("X-Krate-Note", &note.replace('\n', " "))
        .send_bytes(&bytes);
    match response {
        Ok(response) => {
            let body = response.into_string().unwrap_or_default();
            let id = extract_json_string(&body, "id").unwrap_or_default();
            println!("sent -- reference {id}");
            Ok(0)
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            anyhow::bail!(
                "support did not accept the report ({code}): {}",
                body.trim()
            )
        }
        Err(err) => anyhow::bail!("could not reach Krate support: {err}"),
    }
}

/// The build workspace a session's transcript points at, when its failure
/// message kept one.
fn report_workspace_from(session_json: &str) -> Option<PathBuf> {
    let at = session_json.find("the workspace is kept at ")?;
    let rest = &session_json[at + "the workspace is kept at ".len()..];
    let end = rest.find(['"', '\n', ' '])?;
    let path = PathBuf::from(rest[..end].trim());
    path.is_dir().then_some(path)
}

/// The conversation gate: what `krate plan` prints for a request, without
/// building anything (K-123, Plan/Authoring-Conversation-2026-08.md).
///
/// Two layers. A request too thin to mean anything gets a question
/// deterministically -- no AI needed, instant, and it is the acceptance
/// case: "Sadas" must become a question, never an app. Everything else goes
/// to the person's own AI for one short text call that returns either
/// questions or a plan.
/// Something the runtime has no capability for at all, on any platform.
///
/// Membership is deliberately narrow. Every entry here is a door that does
/// not exist in KRATE_CAPABILITY_SPECS -- not one that is merely unfinished
/// on some platform.
///
/// The camera is the cautionary case and is NOT listed. `camera.capture` is
/// declared, and a backend exists on all three desktop systems: AVFoundation
/// on macOS (K-119), and nokhwa over Media Foundation and V4L2 on Windows and
/// Linux (K-148). Saying "Krate cannot do cameras" would be false everywhere.
///
/// The reason it stays out is worth keeping even if that changes. This gate
/// answers "does the door exist", which is a fact about the manifest and is
/// the same on every machine. It must not try to answer "will it work here",
/// which depends on hardware, drivers and permissions -- nobody has pointed
/// the Windows or Linux camera path at a physical webcam yet. A request that
/// might work has to be allowed to try; only a door that does not exist at
/// all is safe to refuse from a keyword.
///
/// A wrong hit here refuses work Krate can really do, so the words that
/// trigger each one have to be words that cannot plausibly mean anything
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wall {
    ScreenCapture,
    OtherApps,
    Hardware,
    Background,
}

impl Wall {
    /// What the person is told. Says what cannot happen, then what can --
    /// the plan step's existing contract is to describe the closest thing
    /// that truly works rather than to refuse, and a person who came to
    /// build something deserves a door, not a wall.
    fn plan(self) -> &'static str {
        match self {
            Wall::ScreenCapture =>
                "Krate cannot record or capture the screen: a sandboxed app is \
                 given its own window and no view of anything else, and there \
                 is no screen-capture permission to ask for. I can build the \
                 app around a recording you already have, or an app that works \
                 on images you drop into it.",
            Wall::OtherApps =>
                "Krate cannot type into, click, or read other programs. That is \
                 the sandbox working rather than a missing feature: an app gets \
                 its own window and nothing outside it. I can build something \
                 that does the job inside its own window, and you copy the \
                 result where you need it.",
            Wall::Hardware =>
                "Krate has no capability for MIDI, Bluetooth, USB or serial \
                 devices, so an app cannot reach one. I can build the same idea \
                 driven by the keyboard, the mouse, or a file you give it.",
            Wall::Background =>
                "A Krate app is one window and one run: it cannot keep working \
                 after you close it, and there is no background permission to \
                 ask for. I can build it to remember where you were and pick \
                 up when you open it again.",
        }
    }
}

/// Read a request for a wall the runtime cannot cross.
///
/// Two-part matching, never a single keyword. "screen" alone appears in
/// "full screen" and "screen saver", both buildable; only "record/capture
/// the screen" is the wall. The cost of being wrong is refusing real work,
/// so every rule needs an action word AND its object.
fn wall_in_request(request: &str) -> Option<Wall> {
    let text = request.to_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|n| text.contains(n));

    // Screen capture: recording or grabbing the display itself.
    if any(&["screen record", "screen-record", "record the screen",
             "record my screen", "screen capture", "screen-capture",
             "capture the screen", "screenshot of my screen",
             "record the display"])
    {
        return Some(Wall::ScreenCapture);
    }

    // Driving other programs. The action has to name another program or the
    // system; "type into a box" inside our own window is ordinary work.
    if any(&["other apps", "other applications", "other programs",
             "another app", "another application", "another program",
             "whatever field", "whatever app", "whatever window",
             "active window", "focused field", "focused window",
             "any application", "control my computer", "control the os",
             "automate my mac", "automate windows"])
        && any(&["type", "click", "press", "send", "control", "read",
                 "automate", "paste into", "fill"])
    {
        return Some(Wall::OtherApps);
    }

    // Physical devices with no capability of any kind.
    if any(&["midi", "bluetooth", " usb", "usb ", "serial port", "gamepad",
             "game controller", "arduino"])
        && any(&["connect", "connected", "my ", "read", "listen", "play",
                 "device", "controller", "keyboard", "piano"])
    {
        return Some(Wall::Hardware);
    }

    // Running when the window is not open.
    //
    // "background" alone is not the wall -- "an app with a dark background"
    // is a buildable request, and an earlier cut of this rule refused it,
    // because the word satisfied both halves of the match by itself. The
    // wall is background *running*, so the word has to be next to a verb
    // about continuing, or the phrase has to be explicit on its own.
    if any(&["after i close", "after closing", "even when closed",
             "even after i close", "keeps running", "keep running",
             "runs in the tray", "system tray", "menu bar app",
             "background process", "background task", "background timer",
             "in the background", "runs in background"])
    {
        return Some(Wall::Background);
    }

    None
}

fn plan_command(request: &str, attachments: &[PathBuf], agent: Option<&str>) -> Result<u8> {
    // Deterministic thin gate. name_from_request already knows which words
    // carry meaning; a request it cannot name and that has no sentence
    // structure to speak of is not buildable as typed.
    let meaningful = request
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_ascii_alphanumeric()))
        .count();
    if name_from_request(request).is_none() && meaningful < 4 {
        println!(
            "{}",
            serde_json::json!({
                "ask": [
                    "What should this app do? Describe it in a sentence or two -- \
                     what you would use it for, and what it should show."
                ]
            })
        );
        return Ok(0);
    }

    // A wall the runtime genuinely cannot cross is worth naming in ten
    // seconds rather than discovering forty minutes into a build. This step
    // costs nothing -- no AI, no network -- and it runs before the provider
    // is even resolved.
    if let Some(wall) = wall_in_request(request) {
        println!(
            "{}",
            serde_json::json!({
                "plan": wall.plan(),
                "needs": Vec::<String>::new(),
            })
        );
        return Ok(0);
    }

    let provider = match agent {
        Some(name) => resolve_agent(name)?,
        None => agent_provider::first_installed().ok_or_else(|| {
            anyhow::anyhow!("no AI is installed to plan with; run `krate ai` to see the options")
        })?,
    };

    let attached: Vec<String> = attachments
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        })
        .collect();
    let attached_line = if attached.is_empty() {
        "Nothing is attached.".to_string()
    } else {
        format!("Attached files: {}.", attached.join(", "))
    };

    let prompt = format!(
        "A person asked for a desktop app in these words:\n\n{request}\n\n{attached_line}\n\n\
         You are deciding whether this is ready to build. Reply with EXACTLY ONE json object \
         and nothing else -- no prose, no code fences.\n\n\
         Ask a question ONLY about something that cannot be changed after v1 or that \
         decides what the app may touch: does it need the network, a folder, the camera, \
         or the microphone; whose data it holds; who it is for, if that is unclear. \
         If so, reply:\n\
         {{\"ask\": [\"question\", ...]}} -- at most three short questions, each one \
         something only this person can answer.\n\n\
         NEVER ask about preferences a v2 can change: options, modes, extra features, \
         history, tax rules, layouts. The person refines by asking for changes after v1; \
         a preference quiz before the first build reads as not listening.\n\n\
         Otherwise reply:\n\
         {{\"plan\": \"AT MOST three sentences, plain words: what will be built, what \
         it shows, and what data it works on. Never restate their answers back at \
         them.\", \"needs\": [\"things the person must \
         supply or approve: a file to attach, a choice to make, a permission the app will \
         request\"]}} -- needs may be empty.\n\n\
         Never ask about colors, fonts, or anything with a sensible default. A Krate app \
         runs on Mac, Windows and Linux automatically, so never ask about platforms. Ask \
         only when the answer changes the app.\n\n\
         One boundary matters: a Krate app is sandboxed. It cannot send keystrokes or \
         clicks to other programs, read other apps' windows, or control the OS. If the \
         request needs that, do not ask questions about it -- reply with a plan for the \
         CLOSEST thing that can truly work, saying plainly what changed and why.\n\n\
         WHICHEVER form you reply with, also include a top-level \"shape\" field: the \
         name of the working example whose SHAPE is closest to this request -- its \
         interaction and data pattern, never its topic -- or \"none\" if nothing fits. \
         The build will start FROM that example's working code and transform it, so \
         pick the one whose structure carries the most. The shapes:\n\
{shape_menu}",
        shape_menu = authoring_context::shape_menu(),
    );

    let program = agent_provider::which_on_path(provider.program())
        .unwrap_or_else(|| PathBuf::from(provider.program()));
    let mut command = ProcessCommand::new(program);
    agent_provider::with_tool_path(&mut command);
    // The session-carrying form when the provider has one: the build that
    // follows can then RESUME the session that planned, request and agreed
    // plan already in context, instead of paying a fresh cold start.
    let session_capable = provider.plan_args_with_session(&prompt);
    let wants_session = session_capable.is_some();
    command.args(session_capable.unwrap_or_else(|| provider.plan_args(&prompt)));
    provider.configure(&mut command);
    let scratch = std::env::temp_dir().join(format!("krate-plan-{}", std::process::id()));
    let _ = fs::create_dir_all(&scratch);
    command.current_dir(&scratch);
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let started = std::time::Instant::now();
    let mut child = command.spawn().context("start the AI for planning")?;
    // Two minutes is the hard stop; the target is under thirty seconds. A
    // plan that takes longer than a coffee is a build in disguise.
    let output = loop {
        if let Some(_status) = child.try_wait().context("wait for the planning AI")? {
            break child.wait_with_output().context("read the plan")?;
        }
        if started.elapsed() > std::time::Duration::from_secs(120) {
            let _ = child.kill();
            anyhow::bail!("the AI took over two minutes to plan; try again or pick another AI");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The plan is optional intelligence, never a gate. If the AI's answer
    // cannot be read as a plan -- an output shape we have not seen, a timeout, a
    // provider that frames things a new way tomorrow -- the request must NOT
    // die. It falls through to a build, which is what the user asked for in the
    // first place. A first-request failure in the Studio is a reputation
    // killer; "we could not pre-plan, so we are just building it" is not a
    // failure at all. Only a genuinely empty output is worth a soft note, and
    // even that still proceeds to build.
    // An AI that REFUSED to answer is not an AI that said "ready to build".
    //
    // The fallback below is right for an answer we could not parse, and
    // wrong for one that never came. When the provider's own stream carries
    // an error -- an expired sign-in, a usage limit, a 401 -- falling
    // through to an empty plan tells the Studio "the AI looked at this and
    // it is ready", and it builds. A person typed "do not create any app"
    // and got an app, because his Codex account had hit its usage limit and
    // that arrived here as silence (K-182).
    //
    // Reuses agent_failure_reason, the same reader the authoring path uses,
    // so a provider only has to be understood once.
    if extract_plan_json(&text).is_none() {
        if let Some(reason) = agent_failure_reason_in(&text) {
            anyhow::bail!(
                "{} could not look at your request:\n\n  {reason}\n\n\
                 Nothing was built. This is a problem with the AI tool, not \
                 with Krate or your request.",
                provider.name()
            );
        }
    }

    let json = extract_plan_json(&text).unwrap_or_else(|| {
        if text.trim().is_empty() {
            eprintln!("note: the planning AI returned nothing; building directly.");
        } else {
            eprintln!("note: could not read a plan from the AI; building directly.");
        }
        // An empty plan with no questions: the Studio reads this as "proceed to
        // build", the same as an AI that decided the request was ready.
        serde_json::json!({ "plan": "", "needs": [] }).to_string()
    });
    // Ride the planning session's id along, tagged with the provider, so
    // the studio can hand it to create and the build resumes hot.
    let json = if wants_session {
        match (
            serde_json::from_str::<serde_json::Value>(&json),
            provider.session_id_in_transcript(&text),
        ) {
            (Ok(mut value), Some(id)) if value.is_object() => {
                value["agent_session"] = serde_json::json!(format!("{}:{}", provider.name(), id));
                value.to_string()
            }
            _ => json,
        }
    } else {
        json
    };
    println!("{json}");
    Ok(0)
}

/// The first balanced JSON object in the text that carries an "ask" or
/// "plan" key. Agents wrap answers in prose and code fences no matter what
/// the prompt says; the contract survives by extraction, not by trust.
fn extract_plan_json(text: &str) -> Option<String> {
    // The plan can arrive in any of three shapes, because every provider frames
    // its output differently and the plan gate has to read all of them:
    //
    //   bare     {"ask": [...]}                          (claude -p)
    //   envelope {"text": "{\"ask\": [...]}", ...}        (grok --output-format json)
    //   stream   {"type":"item.completed","item":{"text":"{\"plan\":...}"}}
    //            ...one such JSON object PER LINE          (codex exec --json)
    //
    // The original scanner understood only the bare shape: it found the first
    // balanced object, and if that object did not itself carry an ask/plan key
    // it moved on. So grok and codex -- correct answers, wrong wrapping -- both
    // failed with "the AI did not answer in the expected shape", on every
    // request, on every machine. That is what a Windows user hit selecting
    // Grok, and then Codex.
    //
    // The robust rule: find every balanced JSON object anywhere in the text
    // (there may be one per line), parse each, and search it recursively --
    // any object with an ask/plan key is the answer, and any STRING that itself
    // parses as JSON is descended into (that is the escaped plan inside an
    // envelope's `text`/`item.text`). First hit in document order wins.
    for candidate in balanced_json_objects(text) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            // A bare plan is returned VERBATIM -- callers and existing behavior
            // expect the original bytes, not a serde reformat. Only when the
            // plan is buried in an envelope do we hand back the re-serialized
            // inner object, since its original bytes were an escaped string.
            if let serde_json::Value::Object(map) = &value {
                if map.contains_key("ask") || map.contains_key("plan") {
                    return Some(candidate);
                }
            }
            if let Some(found) = plan_within(&value) {
                return Some(found);
            }
        }
    }
    None
}

/// Search a parsed JSON value for the plan contract, descending into nested
/// objects/arrays AND into strings that are themselves JSON (the escaped plan a
/// provider buries in a `text` field). Returns the matching object re-serialized.
fn plan_within(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("ask") || map.contains_key("plan") {
                return serde_json::to_string(value).ok();
            }
            for v in map.values() {
                if let Some(found) = plan_within(v) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for v in items {
                if let Some(found) = plan_within(v) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::String(s) => {
            let inner: serde_json::Value = serde_json::from_str(s.trim()).ok()?;
            plan_within(&inner)
        }
        _ => None,
    }
}

/// Every balanced top-level `{...}` object in the text, in order. Handles many
/// objects (one per line, as codex streams) and braces inside strings.
fn balanced_json_objects(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        out.push(text[start.unwrap()..=i].to_string());
                        start = None;
                    }
                }
            }
            _ => {}
        }
    }
    out
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
        // Conversational openers. A pasted chat prompt starts "So i have
        // made..." and the inferred name was "so" (K-124's neighbor).
        "so",
        "hi",
        "hey",
        "hello",
        "ok",
        "okay",
        "now",
        "also",
        "just",
        "can",
        "you",
        "we",
        "have",
        "made",
        "need",
        "like",
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
/// Keeps the temp workspace alive if `create` fails, so the transcript the
/// error points at still exists when the person goes to read it.
///
/// The old shape held the TempDir in a local, so any `?` between authoring
/// and packaging deleted the workspace on the way out -- and the failure
/// message had just printed a path into it (K-078). Drop-based, so every
/// early return is covered without threading a flag through each one.
struct WorkspaceKeeper {
    temp: Option<tempfile::TempDir>,
    armed: bool,
}

impl WorkspaceKeeper {
    /// Call on the success path: the workspace served its purpose.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for WorkspaceKeeper {
    fn drop(&mut self) {
        if !self.armed {
            return; // success: the TempDir cleans up as normal
        }
        if let Some(temp) = self.temp.take() {
            let kept = temp.keep();
            eprintln!(
                "the workspace is kept at {} -- the agent transcript and any code written so far are inside",
                kept.display()
            );
        }
    }
}

fn create_krate(req: CreateRequest) -> Result<u8> {
    use krate_author::{generate, AppKind, AppRequest};

    // A change carries a marker so the prompt builder can tell the two jobs
    // apart. Everything else here -- the name, the feasibility screen, the
    // history entry, what is printed -- must see the person's own words, so
    // strip it once, here, rather than in each of those places.
    let mut req = req;
    let is_change = req.request.starts_with(CHANGE_MARKER);
    let marked_request = req.request.clone();
    if is_change {
        req.request = req.request[CHANGE_MARKER.len()..].to_string();
    }

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
    // An app may not be called `krate`. The SDK dependency is named `krate`,
    // so a package with the same name collides in the lockfile and the build
    // fails with "package collision in the lockfile" -- which names neither
    // the app nor the SDK. An AI handed that spent most of a run fighting it,
    // wrote CANNOT-BUILD, and only then found a workaround.
    let name = if name == "krate" {
        // "krate-app" rather than an error: the person asked for something
        // buildable and the clash is our naming, not their request.
        "krate-app".to_string()
    } else {
        name
    };

    // The app is built inside a work dir. A temp dir is cleaned up on
    // success; --work-dir keeps it for inspection either way.
    //
    // For a CHANGE, the work dir is the app's own unpacked source and the
    // agent must be put directly in it (see is_existing_app_workspace).
    // Joining a name inferred from the change text used to put the agent in
    // an empty directory BESIDE the app; the never-produced-source wipe then
    // handed it a fresh skeleton, and "change the controls" of a finished
    // game rebuilt a generic app from that one sentence. The person watched
    // their app get replaced by a stranger.
    let mut keeper = WorkspaceKeeper {
        temp: None,
        armed: true,
    };
    let app_dir = match &req.work_dir {
        Some(dir) if is_existing_app_workspace(dir, is_change) => dir.clone(),
        Some(dir) => {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            dir.join(&name)
        }
        None => {
            let temp = tempfile::tempdir().context("create work dir")?;
            let path = temp.path().join(&name);
            keeper.temp = Some(temp);
            path
        }
    };
    // A previous attempt in this workspace is evidence and progress, not
    // debris. The stall error promises that a retry "resumes from the code
    // already written" and points at the transcript in this directory --
    // wiping here made both claims false on every retry (K-078). Only a
    // directory that never produced source is cleared.
    if !app_dir.join("src").join("lib.rs").exists() {
        let _ = fs::remove_dir_all(&app_dir);
    }
    fs::create_dir_all(&app_dir).with_context(|| format!("create {}", app_dir.display()))?;

    let mut steps: Vec<serde_json::Value> = Vec::new();

    // Step 1: author. Either an agent command writes the source, or the
    // built-in generator does.
    if !req.json {
        // Some AIs stream what they are doing; Grok reports nothing until it
        // finishes. Say so, rather than showing a silent spinner that reads as
        // a hang -- which is when somebody kills the process.
        report_progress("working out what to build");
        if !report_progress_note(
            "the AI is thinking -- some tools report nothing until they finish",
        ) {
            println!("==> authoring \"{}\"", req.request);
        }
    }
    let author_note = if let Some(cmd) = &req.author_cmd {
        let sdk_prefix = relative_sdk_prefix(&app_dir, &sdk_root)?;
        run_author_command(AuthorContext {
            cmd,
            app_dir: &app_dir,
            name: &name,
            // The MARKED request goes to the agent, so the prompt builder in
            // the child can tell a change from a new app. Everything the
            // person sees uses req.request, which has the marker stripped.
            request: &marked_request,
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
        // Drive the display from what we know, not from what the AI says.
        // Grok emits one JSON object when it finishes rather than a stream,
        // so a run driven only by parsed agent output shows nothing at all
        // until it is over -- which is exactly when somebody gives up and
        // kills it. These four calls are facts about our own pipeline.
        report_progress("building the app");
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
        report_progress("packaging it");
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
        report_progress("checking it runs and paints a frame");
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
             exit 4 means it exhausted its fuel budget -- either a runaway loop, \
             or honest work that is too expensive per frame (hoist per-pixel \
             math out of inner loops, and draw fewer `quick` frames)"
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
    keeper.disarm();
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

    // An API vendor is not a program on PATH, so it never reaches
    // resolve_agent: it runs the loop in api_author instead, which drives
    // the same authoring prompt over HTTP. Everything downstream (the
    // workspace, check-app, packing) is identical.
    if let Some(vendor) = api_key::ApiVendor::parse(agent) {
        return api_author::run(vendor, &app_dir, &request);
    }

    run_provider_author(resolve_agent(agent)?, &app_dir, &request)
}

/// When the authoring agent last said anything.
///
/// Written by the reporter thread as lines arrive, read by the wait loop.
/// A process-global instant is enough: exactly one authoring run happens at
/// a time, and the alternative (threading a shared clock through the
/// reporter closure) buys nothing.
mod agent_progress {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    static EPOCH: OnceLock<Instant> = OnceLock::new();
    static LAST_MS: AtomicU64 = AtomicU64::new(0);

    fn epoch() -> Instant {
        *EPOCH.get_or_init(Instant::now)
    }

    /// Start (or restart) the clock: called when a run begins.
    pub fn begin() {
        let now = epoch().elapsed().as_millis() as u64;
        LAST_MS.store(now, Ordering::Relaxed);
    }

    /// The agent said something.
    pub fn beat() {
        let now = epoch().elapsed().as_millis() as u64;
        LAST_MS.store(now, Ordering::Relaxed);
    }

    /// How long the agent has been silent.
    pub fn since_last_line() -> Duration {
        let now = epoch().elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(LAST_MS.load(Ordering::Relaxed)))
    }
}

/// The prompt handed to Claude Code: the loop instruction.
///
/// The agent is no longer asked to adapt a behavioral template. It is given a
/// minimal compiling skeleton, the full context pack, and the one tool that
/// changes everything -- it can run `krate check-app .`, see exactly what is
/// wrong, and fix it. So the prompt is short: build the app, and do not stop
/// until the oracle says OK. Everything the agent needs to know is in the pack,
/// which is generated from real sources, not restated here where it could drift.
/// The marker that tells the prompt builder this is a change, not a new app.
///
/// Set by `revise_app_for_tui` and read here. A string rather than a parameter
/// because the request travels through `create_krate` and the author-agent
/// child process as one environment variable, and threading a second flag
/// through both would be more machinery than the distinction needs.
pub(crate) const CHANGE_MARKER: &str = "\u{1}krate-change\u{1}";

/// The instructions for changing an app that already works.
///
/// A change is a different job from writing an app, and it used to get the
/// same prompt: "find the closest example and adapt it", "write the app". So
/// an AI asked to move a button re-derived how to write a Krate app from
/// scratch, read a worked example it did not need, and took as long as the
/// original build.
///
/// What a change actually needs is: the app is already correct, find the one
/// place this belongs, change that, leave everything else alone. The API
/// reference stays available for a call it has not used before -- reading less
/// is not the goal, doing less is.
fn change_prompt(app_dir: &str, change: &str, krate_bin: &str) -> String {
    format!(
        "You are changing a Krate app that already works. It is in {app_dir}.\n\
\n\
    {change}\n\
\n\
This is an edit, not a rewrite. The app compiles, passes `check-app`, and\n\
already demonstrates the no_std and krate:* discipline in its own source --\n\
you do not need to re-derive any of that, and you do not need a worked\n\
example. Its code is the example.\n\
\n\
How to work:\n\
1. Read src/lib.rs and find the one place this change belongs. Most changes\n\
   are a few lines in one function. Search for the label, colour, number or\n\
   behaviour named in the request rather than reading the file end to end.\n\
2. Make that change and nothing else. Do not reformat, do not rename, do not\n\
   restructure code you were not asked about, and do not \"improve\" things in\n\
   passing. A diff that touches one function is the goal.\n\
3. KRATE_AUTHORING.md is in this directory if you need a function you have not\n\
   used before, or a capability the manifest does not declare yet. Consult it\n\
   for that; do not read it front to back.\n\
4. Check your edit with `{krate_bin} check-app . --no-run` from {app_dir}.\n\
   That builds it and confirms the imports in about two seconds.\n\
5. Once that passes, run the full `{krate_bin} check-app .` once to prove it\n\
   still opens and works. That takes around twenty seconds, so run it when\n\
   you are done rather than after every edit. Do not stop until it prints OK.\n\
\n\
Keep the same crate name, the same package name, and the same manifest unless\n\
the change genuinely needs a new capability. Changing them breaks the app's\n\
identity for somebody who already has it.\n\
\n\
Do not explain what you did; make the change until the check passes."
    )
}

#[cfg(test)]
fn claude_author_prompt(app_dir: &str, request: &str, krate_bin: &str) -> String {
    claude_author_prompt_with(app_dir, request, krate_bin, false)
}

pub(crate) fn claude_author_prompt_with(
    app_dir: &str,
    request: &str,
    krate_bin: &str,
    inline: bool,
) -> String {
    // A change carries a marker rather than a flag, because the request
    // travels to the author-agent child as one environment variable.
    if let Some(change) = request.strip_prefix(CHANGE_MARKER) {
        return change_prompt(app_dir, change, krate_bin);
    }
    let example = authoring_context::closest_example(request);
    let example_name = example.name;
    let example_shows = example.shows;
    // The model-starter seed (K-205): when run_author_command placed a
    // complete working example AS src/lib.rs, the job is transformation,
    // not authorship. A stamped build put 82% of wall time in the model
    // generating a full file from silence (263s before the first write on
    // a 560s build); every line it does not have to retype is real time.
    let model_starter = fs::read_to_string(Path::new(app_dir).join(".starter-mode"))
        .ok()
        .and_then(|s| s.strip_prefix("model:").map(|rest| rest.trim().to_string()));
    let situation = match &model_starter {
        Some(starter) => format!(
            "Work in {app_dir}. src/lib.rs ALREADY IS a complete, working Krate app\n\
({starter}). It builds, runs, and passes check-app, and its manifest.toml is\n\
beside it -- it was chosen as the closest working shape to this request. Your\n\
job is to TRANSFORM it into the app the request describes: change what\n\
differs, delete what the request does not need, and keep its event loop and\n\
no_std/krate:* discipline exactly as they are."
        ),
        None => format!(
            "Work in {app_dir}. A minimal compiling skeleton is already there (Cargo.toml,\n\
src/lib.rs, manifest.toml): it opens a window (or prints a line, for a CLI app),\n\
builds cleanly, and imports only krate:* -- but it does nothing yet. Your job is\n\
to make it the app the request describes."
        ),
    };
    let steps_two_three = match &model_starter {
        Some(_) => "\
2. Read src/lib.rs once, whole -- it is your model AND your canvas. Then\n\
   transform it with targeted edits. Prefer editing sections over retyping\n\
   the file: the working structure is already right, and every line you\n\
   rewrite costs real minutes. There is no EXAMPLE.rs; the app itself is\n\
   the example.\n\
3. Trim manifest.toml to exactly the capabilities the finished app uses --\n\
   the starter may declare things your app does not need.\n"
            .to_string(),
        None => format!(
            "\
2. Your model app is EXAMPLE.rs in this directory ({example_name}: {example_shows}),\n\
   picked for this request, with its manifest beside it as\n\
   EXAMPLE.manifest.toml. Read it once, whole, and adapt its proven, working\n\
   code -- do not write the no_std/krate:* discipline from a blank page, and\n\
   do not go hunting the filesystem for other examples.\n\
3. Write the app: edit src/lib.rs, and set manifest.toml to exactly the\n\
   capabilities the app uses.\n"
        ),
    };
    let mut prompt = format!(
        "You are building a Krate desktop app in Rust from this request:\n\
\n\
    {request}\n\
\n\
{situation}\n\
\n\
How to work:\n\
1. Read KRATE_AUTHORING.md in this directory first -- the WHOLE file in ONE\n\
   read call, never in grepped pieces (reading it fragment by fragment is the\n\
   single biggest time sink in authoring). It lists every function you can\n\
   call, every capability a manifest may declare, the no_std rules, and the\n\
   GUI interfaces. It is generated from the real SDK, so everything in it is\n\
   accurate.\n\
{steps_two_three}\
4. While you are working, check with:\n\
\n\
       {krate_bin} check-app . --no-run\n\
\n\
   That builds the app and confirms it imports only krate:* -- the two things\n\
   that actually break -- in about two seconds. Use it after every edit.\n\
\n\
5. When it passes and you believe the app is done, run the full check once:\n\
\n\
       {krate_bin} check-app .\n\
\n\
   This also runs the app, resizes its window and clicks it, which takes\n\
   around twenty seconds. It is the real verdict, so it has to pass -- but\n\
   running it after every small edit is the single biggest waste of time in\n\
   authoring an app. Iterate with --no-run, prove with the full check.\n\
\n\
   Either way, on failure it names the stage and the exact fix, including how\n\
   to remove a leaked wasi:* import. Do what it says, then check again.\n\
\n\
If you want to SEE what the app draws, use\n\
`{krate_bin} check-app . --shoot frame.png` -- it renders the app's window\n\
to a PNG headlessly. NEVER use `screencapture`, screen-recording tools, or\n\
anything that reads the real screen: on macOS that pops a scary permissions\n\
dialog on the person's screen in the middle of their build, and it hangs\n\
your run until they answer it.\n\
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
useful is almost always the right answer; refusing something buildable is not.\n\
\n\
Never type the KRATE-CANNOT-BUILD marker anywhere except in that file, as a\n\
final refusal -- not while thinking out loud, not to note a doubt you later\n\
resolve. Krate reads the marker as your verdict.\n\
\n\
And so you do not doubt them: these all genuinely work in every Krate app --\n\
store.kv and store.sql PERSIST across app restarts on the user's disk; mouse\n\
clicks, mouse position and key input all arrive through krate:ui/events;\n\
windows resize; the camera streams real frames; net.http reaches the\n\
internet. KRATE_AUTHORING.md is the authority, not your prior."
    );
    if inline {
        // The essentials ride inside this very prompt: zero read round-trips
        // for the patterns, the rules, the catalog, and the model app. Only
        // an exact SDK signature still needs the disk, and that is one read.
        prompt.push_str(&authoring_context::inline_essentials(request));
        prompt.push_str(
            "
---

Because the essentials are inlined above, steps 1 and 2              change: do NOT read KRATE_AUTHORING.md or EXAMPLE.rs up front --              everything they would tell you is already in this message. Consult              KRATE_AUTHORING.md's section 1 on disk ONLY if you need an exact              function signature not shown here, and read it once, whole, when              you do.
",
        );
    }
    prompt
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
    let Ok(text) = fs::read_to_string(Path::new(app_dir).join(AGENT_REFUSAL_FILE)) else {
        // No file. The agent may still have refused and been unable to say so
        // on disk, so ask the transcript before calling this a build failure.
        return agent_refusal_in_transcript(app_dir);
    };
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

/// Recover a refusal the agent stated but could not write down.
///
/// The file is the contract, but an agent does not always get to honour it: a
/// sandboxed agent whose write is refused still SAYS why in its output, and
/// throwing that away turned a clear "Krate has no webcam API" into a generic
/// "that build didn't come together" with no reason at all (K-139). The
/// request was genuinely impossible and the agent diagnosed it correctly --
/// the person deserves to be told that, not shown a failure.
///
/// Kept strict on purpose: only the explicit marker counts. An agent musing
/// that something "cannot be done" mid-run, then finding a way, must not be
/// read as a refusal -- that would fail apps that were about to succeed.
fn agent_refusal_in_transcript(app_dir: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(app_dir).join(".agent-transcript.txt")).ok()?;
    let marker = "KRATE-CANNOT-BUILD:";
    // Last occurrence: an agent that reconsiders states its final position
    // last, and the earlier mention may be it quoting the instructions.
    let at = text.rfind(marker)?;
    let rest = &text[at + marker.len()..];
    // The transcript is JSON events, so the sentence may be followed by escape
    // sequences rather than a real newline. Stop at whichever comes first.
    let end = rest
        .find(['\n', '\r'])
        .into_iter()
        .chain(rest.find("\\n"))
        .chain(rest.find('"'))
        .min()
        .unwrap_or(rest.len());
    let reason = rest[..end].trim().trim_end_matches(['\\', ',']).trim();
    if reason.is_empty() {
        return None;
    }
    Some(reason.to_string())
}

thread_local! {
    /// The stall-retry budget for the current authoring run, on this thread.
    /// `None` means "not yet seeded" -- the first read of a fresh run seeds it
    /// to the starting count; the resume path overwrites it with the decremented
    /// value. Per-thread so the recursive resume counts down without threading a
    /// parameter through every call site.
    static STALL_BUDGET: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// How many stall-restarts remain, seeding the budget on first use.
///
/// Two: a transient API hang usually clears on the first restart; a second
/// covers a run that stalls twice. Beyond that the agent or the network is
/// genuinely unwell, and the honest outcome is the stall error, not an endless
/// restart loop burning the person's quota.
fn stall_retries_remaining() -> Option<u32> {
    STALL_BUDGET.with(|b| {
        if b.get().is_none() {
            b.set(Some(2));
        }
        b.get().filter(|&n| n > 0)
    })
}

/// Re-invoke the agent to continue from the code already written after a stall.
/// The workspace persists, so the same prompt resumes rather than restarts.
fn run_provider_author_resuming(
    provider: &'static dyn agent_provider::AgentProvider,
    app_dir: &str,
    request: &str,
    retries_left: u32,
) -> Result<u8> {
    STALL_BUDGET.with(|b| b.set(Some(retries_left)));
    run_provider_author(provider, app_dir, request)
}

/// Run one provider through Krate's authoring policy.
///
/// Everything here is the same for every provider: the prompt, the transcript,
/// the skeleton snapshot that catches an agent which answered in chat without
/// writing code, the progress reporting, the timeout, and the `check-app`
/// verdict. Only the argument list, the spawn setup, and the progress parsing
/// come from the provider -- which is exactly the split the trait draws.
/// Copy what the agent needs to sign in into its confined home.
///
/// Returns whether the agent can be expected to authenticate from
/// `agent_home`. The caller confines HOME only when this says yes, because
/// a confined agent that cannot log in cannot write anything at all -- and
/// a permission prompt is a smaller harm than a product that does not work
/// (K-179).
///
/// Only the credential and the settings travel. Project history does not:
/// it is a list of the person's own paths, and the whole point of the
/// confined home is that the agent has no map of their disk.
/// Whether the agent's home is confined, given a real home.
///
/// Split out so the rule can be asserted: confinement does NOT depend on
/// seeding, on a credential existing, or on anything about how long the
/// person has used Krate. A fresh account confines exactly like an old one.
fn agent_home_for(real_home: &Path) -> PathBuf {
    real_home.join(".krate").join("agent-home")
}

fn seed_agent_home(real_home: &Path, agent_home: &Path) -> bool {
    // The directory has to exist before anything is copied into it. The
    // caller creates it too, but AFTER this runs, so on a first run every
    // copy here landed in a directory that was not there yet and failed
    // silently. Claude's seeding survived only because it makes its own
    // subdirectory on the way; the per-provider copies below did not.
    let _ = fs::create_dir_all(agent_home);

    // A login keychain INSIDE the confined home. The Security framework
    // resolves the keychain from $HOME, so with HOME rebased here, any tool
    // the agent runs that touches the keychain -- git's osxkeychain helper,
    // a CLI storing a token -- finds no keychain at all, and macOS throws a
    // "Keychain Not Found ... Reset To Defaults" dialog at the person, on
    // every build, over work they did not start. An empty-password keychain
    // in the confined home gives those tools somewhere to write; what they
    // store stays inside the wall, and the person's real keychain is never
    // in the search path.
    #[cfg(target_os = "macos")]
    {
        let keychains = agent_home.join("Library/Keychains");
        let login = keychains.join("login.keychain-db");
        if !login.exists() {
            let _ = fs::create_dir_all(&keychains);
            let created = ProcessCommand::new("/usr/bin/security")
                .args(["create-keychain", "-p", ""])
                .arg(&login)
                .env("HOME", agent_home)
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false);
            if created {
                // Registered as the default within THIS home's preferences,
                // so lookups resolve without asking. The real home's
                // keychain settings are untouched.
                let _ = ProcessCommand::new("/usr/bin/security")
                    .args(["default-keychain", "-s"])
                    .arg(&login)
                    .env("HOME", agent_home)
                    .output();
            }
        }
        if login.exists() {
            // Every seed, not just creation: keychains auto-lock, and a
            // LOCKED confined keychain trades the "Not Found" dialog for a
            // password prompt -- the same interruption wearing a different
            // face (seen live during the first demo builds after the
            // keychain landed). Unlock FIRST: set-keychain-settings itself
            // needs an unlocked keychain, so the other order raises the
            // very dialog it exists to prevent (also seen live). Then no
            // timeout and no lock-on-sleep, so it stays open.
            let _ = ProcessCommand::new("/usr/bin/security")
                .args(["unlock-keychain", "-p", ""])
                .arg(&login)
                .env("HOME", agent_home)
                .output();
            let _ = ProcessCommand::new("/usr/bin/security")
                .args(["set-keychain-settings"])
                .arg(&login)
                .env("HOME", agent_home)
                .output();
            // The search list falls through to the person's REAL login
            // keychain (K-206). Copying Claude's credential into the
            // sandbox forked it, and OAuth refresh tokens ROTATE: the
            // agent's copy refreshed first and the person's own sign-in
            // became the dead twin -- their `claude` broke every time
            // Krate built (seen twice in one night before the cause was
            // found). With the real keychain second in the search list
            // there is ONE token: reads find it, the refresh updates it
            // in place, and new items still land in the sandbox default.
            if let Some(real) = home_dir() {
                let user_login = real.join("Library/Keychains/login.keychain-db");
                if user_login.exists() {
                    let _ = ProcessCommand::new("/usr/bin/security")
                        .args(["list-keychains", "-d", "user", "-s"])
                        .arg(&login)
                        .arg(&user_login)
                        .env("HOME", agent_home)
                        .output();
                }
            }
        }
    }

    // Settings, minus the history of places the person has worked.
    if let Ok(text) = fs::read_to_string(real_home.join(".claude.json")) {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) {
            value["projects"] = serde_json::json!({});
            let _ = fs::write(
                agent_home.join(".claude.json"),
                serde_json::to_string(&value).unwrap_or_default(),
            );
        }
    }

    // The credential itself. The keychain is the macOS home for it; a file
    // is the form everywhere else and the fallback here.
    let config = agent_home.join(".claude");
    let _ = fs::create_dir_all(&config);
    let dest = config.join(".credentials.json");
    #[allow(unused_mut, unused_assignments)]
    let mut claude_ready = false;
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = ProcessCommand::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
        {
            if out.status.success() && !out.stdout.is_empty() {
                {
                    // Detection only. This used to WRITE the token to the
                    // file too; with dest now a link to the person's real
                    // credentials (K-206) that write would stomp their
                    // file, and the search-list keychain already serves
                    // the confined agent.
                    // Deliberately NOT an early return.
                    //
                    // It was one, and that single `return true` is why every
                    // other AI ran signed out: the moment Claude's keychain
                    // credential was written, the function left, and the
                    // per-provider copies below never executed. On a machine
                    // with Claude signed in -- which is every machine we
                    // develop on -- Grok, Codex, Gemini and Copilot silently
                    // got nothing (K-189).
                    claude_ready = true;
                    // K-202 copied the token into the CONFINED keychain here.
                    // That fixed the stale read and created K-206: OAuth
                    // refresh tokens ROTATE, so two live copies means the
                    // agent's refresh kills the person's own sign-in. The
                    // sandbox keychain's search list now falls through to the
                    // real login keychain instead (set where the keychain is
                    // created), so there is exactly one token and Claude
                    // refreshes it in place. Nothing to copy anymore; also
                    // delete any forked copy an older build left behind.
                    let login = agent_home.join("Library/Keychains/login.keychain-db");
                    if login.exists() {
                        let _ = ProcessCommand::new("/usr/bin/security")
                            .args(["delete-generic-password", "-s", "Claude Code-credentials"])
                            .arg(&login)
                            .env("HOME", agent_home)
                            .output();
                    }
                }
            }
        }
    }
    // An older build's COPIED file at dest is exactly the fork K-206
    // kills: replace it with the link.
    #[cfg(unix)]
    if dest.exists() && !dest.is_symlink() {
        let _ = fs::remove_file(&dest);
    }
    if !dest.exists() {
        let source = real_home.join(".claude/.credentials.json");
        // A LINK, not a copy (K-206): OAuth refresh tokens rotate, so a
        // copied credential forks -- whichever copy refreshes first kills
        // the other, and that other was the person's own sign-in. A link
        // means one file, refreshed in place by whoever uses it.
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&source, &dest);
        #[cfg(windows)]
        let _ = fs::hard_link(&source, &dest).or_else(|_| fs::copy(&source, &dest).map(|_| ()));
    }

    // Every OTHER tool's sign-in, for the same reason Claude's is here.
    //
    // Confining the agent's HOME (K-179) stops it asking for the person's
    // Downloads folder in Krate's name. It also hides every credential that
    // lives under the real home -- and only Claude's was ever copied across,
    // so Grok, Codex, Gemini and Copilot were all signed OUT the moment they
    // ran under Krate, on machines where the person had signed in perfectly
    // well.
    //
    // The first outside user to try Grok hit exactly this. Her `grok` TUI
    // worked; her Krate build said "Not signed in". Measured on this
    // machine, same signed-in tool, one variable changed:
    //   HOME=<real>                  -> 0 "not signed in"
    //   HOME=~/.krate/agent-home     -> 2 "not signed in"   (K-189)
    //
    // Copied, never moved or symlinked: the confined copy is the agent's to
    // read, and a symlink would put the real file back inside the sandbox
    // this confinement exists to draw.
    for dir in [".grok", ".codex", ".gemini", ".copilot"] {
        let from = real_home.join(dir);
        if from.is_dir() {
            let _ = copy_dir_shallow(&from, &agent_home.join(dir));
        }
    }
    // Gemini and Copilot also use ~/.config on some installs.
    for dir in ["gemini", "github-copilot"] {
        let from = real_home.join(".config").join(dir);
        if from.is_dir() {
            let to = agent_home.join(".config").join(dir);
            let _ = fs::create_dir_all(agent_home.join(".config"));
            let _ = copy_dir_shallow(&from, &to);
        }
    }

    claude_ready || dest.exists()
}

/// Copy the files directly inside `from` into `to`, creating `to`.
///
/// Shallow on purpose. A credential is a small file at the top of a tool's
/// config directory; the deep contents are caches, logs and session history
/// -- the very material the confined home exists to keep out of reach, and
/// copying it would be both slow and contrary to the point.
fn copy_dir_shallow(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let _ = fs::copy(entry.path(), to.join(entry.file_name()));
        }
    }
    Ok(())
}

fn run_provider_author(
    provider: &'static dyn agent_provider::AgentProvider,
    app_dir: &str,
    request: &str,
) -> Result<u8> {
    // A repair re-enters this function with a CHANGE_MARKER request; only the
    // first, real authoring run opens the build in the trace, so the study row
    // is one build, not one per repair round.
    if !request.starts_with(CHANGE_MARKER) {
        trace::build_start(request, provider.name(), app_dir);
    }
    trace::phase(if request.starts_with(CHANGE_MARKER) {
        "repair-author"
    } else {
        "author"
    });
    // The agent runs `krate check-app .` itself, so it needs this binary on a
    // known path. current_exe is the running krate; hand its absolute path to
    // the prompt so the agent's Bash calls resolve it regardless of PATH.
    let krate_bin = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "krate".to_string());
    // Inline the essentials when the prompt can actually be delivered: via
    // stdin where the provider reads it, or via argv on Unix, whose limit is
    // roomy. Windows argv caps at 32K characters, so a provider without a
    // stdin mode there gets the compact prompt and reads from disk as
    // before. KRATE_INLINE_PACK=0 is the escape hatch.
    let stdin_args = provider.author_args_stdin();
    let inline = std::env::var("KRATE_INLINE_PACK").as_deref() != Ok("0")
        && !request.starts_with(CHANGE_MARKER)
        && (stdin_args.is_some() || !cfg!(windows));
    let prompt = claude_author_prompt_with(app_dir, request, &krate_bin, inline);
    let prompt_via_stdin = inline && stdin_args.is_some() && prompt.len() > 6_000;
    let transcript = Path::new(app_dir).join(".agent-transcript.txt");
    // A snapshot of the skeleton, to detect an agent that answered in chat and
    // never wrote code -- that would leave the blank skeleton, which builds and
    // passes check-app but is not the requested app.
    // The file the agent is expected to CHANGE. In the small-language
    // world (stage 39) the app lives in src/app.rs and lib.rs is fixed
    // glue -- comparing lib.rs there rejected a fully written app as "the
    // agent did nothing" (seen on the first matrix run).
    let watched_file = if Path::new(app_dir).join("src/app.rs").exists() {
        "src/app.rs"
    } else {
        "src/lib.rs"
    };
    let starter_lib = fs::read_to_string(Path::new(app_dir).join(watched_file)).unwrap_or_default();
    let file = fs::File::create(&transcript).ok();

    // Resolve to a full path the same way the readiness probe does, so an
    // agent that lives off PATH (Grok in ~/.grok/bin, launched from a GUI)
    // is found here exactly when the probe said it would be.
    let program = agent_provider::which_on_path(provider.program())
        .unwrap_or_else(|| PathBuf::from(provider.program()));
    let mut command = ProcessCommand::new(program);
    agent_provider::with_tool_path(&mut command);
    // Start the agent IN the app directory. Not a convenience: a sandboxed
    // agent decides what it may write from its own working directory, and
    // codex's `workspace-write` roots exactly there. Without this the agent
    // inherited whatever cwd Krate was launched from (the Studio's, or the
    // repo), every write to the app landed "outside of the project", and
    // codex refused all of them -- so codex could never author anything, and
    // the person saw a generic "that build didn't come together" (K-139).
    command.current_dir(app_dir);
    // And the agent's `~` is OURS, not the person's (K-179).
    //
    // The working directory above decides where the agent starts; HOME
    // decides what `~` means, and an agent running with permissions
    // bypassed will happily `ls ~/Downloads`. On macOS a child's file
    // access is attributed to the PARENT BUNDLE, so that listing became the
    // system demanding the person's Downloads folder in Krate's name --
    // measured: with the real HOME the agent read actual private documents
    // out of ~/Downloads; with this rebased it reports the path does not
    // exist.
    //
    // The Studio sets this too, but the engine must set it as well: `krate
    // create` in a terminal is its own door, and the Studio delegates the
    // build to this very code.
    if let Some(home) = home_dir() {
        let agent_home = agent_home_for(&home);
        // Seed the credential into the confined home, then confine
        // REGARDLESS of whether seeding found one.
        //
        // The first cut rebased HOME without moving the credential and every
        // build died with "Not logged in". The second cut over-corrected:
        // it confined only when seeding SUCCEEDED, which silently handed the
        // real home back to exactly the person who most needs confining --
        // a brand-new user with no credential yet. Measured in a fresh macOS
        // account: the agent was handed HOME=/Users/test, the real one, and
        // the prompts would have returned (K-179).
        //
        // So: always confine. A person who has never signed the agent in is
        // told to sign in either way; that message is the agent's own and
        // arrives whatever HOME says. What must never happen is Krate
        // quietly asking for their Downloads folder.
        let _ = seed_agent_home(&home, &agent_home);
        if std::fs::create_dir_all(&agent_home).is_ok() {
            // Claude on macOS keeps its credential in the KEYCHAIN, which
            // the Security framework resolves from the real user session no
            // matter what $HOME says -- a fake-HOME `security` even reports
            // success while writing nowhere (cfprefsd keys by real user).
            // And a copied token forks: OAuth refresh rotation means the
            // agent's copy killed the person's own sign-in (K-206). So
            // claude alone keeps the real HOME and is confined through its
            // own CLAUDE_CONFIG_DIR instead: config, projects and trust
            // live in the agent home, while the one credential stays in the
            // person's keychain and refreshes in place. Every other agent
            // is file-based and confines the normal K-179 way.
            // Measured, not assumed: CLAUDE_CONFIG_DIR scopes claude's
            // LOGIN too -- with it set, a signed-in machine says "Not
            // logged in" (the credential lookup follows the config dir).
            // So on macOS claude simply keeps the real HOME: the one
            // keychain token, refreshed in place, no fork to rotate dead.
            // Its own permission flags already govern what it may touch.
            let claude_native_keychain =
                cfg!(target_os = "macos") && provider.name() == "claude";
            if !claude_native_keychain {
                command.env("HOME", &agent_home);
            }
            // cargo and rustup resolve their homes from $HOME, and the agent
            // builds the app it writes -- pin them to the real one or
            // confining the agent costs it its compiler.
            if std::env::var_os("CARGO_HOME").is_none() {
                command.env("CARGO_HOME", home.join(".cargo"));
            }
            if std::env::var_os("RUSTUP_HOME").is_none() {
                command.env("RUSTUP_HOME", home.join(".rustup"));
            }
        }
    }
    // Hot sessions (the last piece of the speed study): a repair round or a
    // revise RESUMES the session that wrote the app instead of cold-starting
    // one that must re-read everything -- measured cold, the reading alone
    // was 4 to 11 minutes of every build. The id lives in two places,
    // because changes arrive through two doors: beside the code for repair
    // rounds in this same workspace, and keyed by the app's identity for a
    // revise, whose workspace is a fresh temp unpack every time. Stored ids
    // are taken, not peeked: a resume that fails leaves nothing behind, so
    // the retry cold-starts instead of resuming into the same failure
    // forever. Fresh creates never resume; a new app deserves a clean slate.
    let session_files = agent_session_files(app_dir);
    let resume_args = if request.starts_with(CHANGE_MARKER) {
        take_agent_session(&session_files, provider.name()).and_then(|id| {
            // A revise's workspace is a fresh unpack; the session was made in
            // the previous build's. Move it within reach first.
            provider.adopt_session(&id, app_dir);
            provider.author_args_resuming(&prompt, &id)
        })
    } else {
        // A fresh create resumes ONLY a session deliberately placed in this
        // workspace -- the planning session, seeded via KRATE_PLAN_SESSION.
        // The app-identity file is never consulted here: a new app must not
        // inherit some earlier app's conversation. The planning session was
        // toolless, so it has never seen the essentials -- when the inline
        // prompt can ride stdin it does, resumed; otherwise argv carries it
        // where argv is roomy enough to.
        take_agent_session(&session_files[..1], provider.name()).and_then(|id| {
            // The planning session was made in `krate plan`'s temp dir, not
            // this workspace. Move it within reach first.
            provider.adopt_session(&id, app_dir);
            if prompt_via_stdin {
                provider.author_args_stdin_resuming(&id)
            } else {
                provider.author_args_resuming(&prompt, &id)
            }
        })
    };
    let resumed = resume_args.is_some();
    if resumed {
        if request.starts_with(CHANGE_MARKER) {
            eprintln!("    continuing the session that wrote this app");
        } else {
            eprintln!("    continuing the session that planned this app");
        }
    }
    match resume_args {
        Some(args) => {
            command.args(args);
        }
        None if prompt_via_stdin => {
            command.args(stdin_args.expect("checked by prompt_via_stdin"));
        }
        None => {
            command.args(provider.author_args(&prompt));
        }
    }
    // Provider-specific spawn setup: closing stdin so a headless run never
    // blocks on input, plus anything else that provider needs.
    provider.configure(&mut command);
    // The stdin route re-opens what configure just closed: the prompt is
    // written down the pipe and the pipe is dropped, so the agent still
    // sees EOF and can never hang waiting for more.
    if prompt_via_stdin {
        command.stdin(std::process::Stdio::piped());
    }
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
    // The large prompt goes down the pipe, then the pipe closes so the
    // agent sees EOF -- an open pipe is exactly the hang configure's
    // stdin-null exists to prevent.
    if prompt_via_stdin {
        if let Some(mut sin) = child.stdin.take() {
            use std::io::Write as _;
            let _ = sin.write_all(prompt.as_bytes());
        }
    }

    // Read the agent's streamed events on a worker thread and turn each one
    // into a plain-English progress line. The thread owns the pipe and appends
    // every raw line to the transcript, so the transcript is unchanged while
    // the person watching gets to see real work instead of dots.
    let stdout = child.stdout.take();
    let transcript_for_thread = transcript.clone();
    // The app's own directory, for the trace's outside-workspace check.
    let app_dir_for_thread = app_dir.to_string();
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
                // Any line at all is proof of life, tool call or chatter.
                agent_progress::beat();
                // Trace the RAW tool call for the study -- every read, write,
                // and command, including the Bash exploration that never maps
                // to a progress step. This is what makes "what did it read, and
                // where did it go outside its workspace" answerable. The gap
                // before each event is its think/act time. Guarded so it costs
                // nothing when tracing is off.
                if trace::enabled() {
                    if let Some((tool, target)) = provider.raw_tool_call(&line) {
                        // Flag a target outside the app's own workspace: a read
                        // of the real repo or the SDK cache is dev-machine
                        // material a fresh user would not have, and it inflates
                        // how well a build goes here versus for a real person.
                        let outside = !target.is_empty()
                            && !target.contains(app_dir_for_thread.as_str())
                            && (target.contains("/apps/")
                                || target.contains("/.cache/krate/")
                                || target.contains("/layer6x6/"));
                        let step = if outside {
                            format!("OUTSIDE-WORKSPACE {tool}")
                        } else {
                            tool.clone()
                        };
                        trace::tool_call(&step, &tool, Some(&target));
                    }
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

    agent_progress::begin();
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(timeout_secs);
    // Silence is the real signal of a hung agent, and the total ceiling is a
    // bad proxy for it: a big request legitimately runs for half an hour,
    // while a wedged one produces nothing at all. The reporter stamps this
    // clock on every line it sees; ten quiet minutes ends the run in one,
    // instead of the person watching a spinner to the ceiling (K-127).
    // Silence only means "stalled" for a provider that STREAMS. Grok (and any
    // non-streaming provider) writes one JSON object when the whole run ends,
    // so it is silent on stdout for its entire, healthy run -- the transcript
    // arrives at the finish, not along the way. Applying the silence killer to
    // it kills a working run: a grok request whose single API round-trip took
    // over ten minutes was stopped mid-flight and reported as hung, on a
    // machine where nothing was wrong. For those providers the total deadline
    // is the only honest ceiling; there is no in-run line to time silence from.
    // Keep the kill at 10 minutes: caught LIVE, a real "freeze" was claude's
    // API connection hanging ESTABLISHED at 0% CPU for ~5-6 minutes and then
    // RESUMING on its own -- the build recovered and finished. So a silence is
    // often a transient API stall that self-heals, and killing at 7 min would
    // have thrown away a build that was about to succeed. The kill is the
    // last-resort ceiling for a truly dead agent, not the answer to a slow one.
    //
    // The real bug was never the timeout -- it was that the UI showed no sign
    // of the stall, so the founder saw a silent screen, assumed it was frozen,
    // and retried, again and again. The warning below is the actual fix: it
    // makes the stall VISIBLE so a person knows to wait, not give up.
    let stall = if provider.reports_progress() {
        std::time::Duration::from_secs(600)
    } else {
        // Effectively off: the deadline below is the real bound.
        std::time::Duration::from_secs(u64::MAX / 2)
    };
    // Warn the watching person once the silence passes ~2 minutes -- past the
    // longest healthy mid-build pause (~3 min is the ceiling, but 2 catches a
    // stall early while rarely firing on a working build) -- so a stall is
    // visible long before the kill instead of a silent screen.
    let warn_at = std::time::Duration::from_secs(120);
    let mut warned = false;
    // None means the agent was stopped at the deadline but the app it had
    // already written passes every check -- salvaged, not stalled.
    let status: Option<std::process::ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                let quiet = agent_progress::since_last_line();
                // Reset the warning once the agent speaks again, so a second,
                // later stall warns too rather than only the first.
                if warned && quiet < warn_at {
                    warned = false;
                }
                if !warned && quiet > warn_at {
                    warned = true;
                    report_progress_alive(
                        "the AI has gone quiet -- this can be a slow model reply; still waiting",
                    );
                }
                if quiet > stall {
                    let _ = child.kill();
                    let _ = child.wait();
                    if check_app_verdict(app_dir).is_ok() {
                        eprintln!(
                            "note: the AI went quiet for {} minutes, but the app it wrote already passes every check -- packaging it.",
                            quiet.as_secs() / 60
                        );
                        break None;
                    }
                    // Auto-retry a stalled agent instead of failing the whole
                    // build. A stall is a transient API hang (caught live: the
                    // connection sat ESTABLISHED with no response), and the
                    // workspace persists, so re-invoking the agent RESUMES from
                    // the code it already wrote rather than starting over. This
                    // is what makes the FIRST attempt succeed instead of the
                    // person having to notice the silent screen and retry by
                    // hand. Bounded, so a genuinely dead agent still fails.
                    if let Some(retries_left) = stall_retries_remaining() {
                        eprintln!(
                            "note: the AI went quiet for {} minutes -- restarting it to continue from the code so far ({retries_left} left).",
                            quiet.as_secs() / 60
                        );
                        report_progress_alive(
                            "the AI stalled -- restarting it to pick up where it left off",
                        );
                        if let Some(reporter) = reporter {
                            let _ = reporter.join();
                        }
                        return run_provider_author_resuming(
                            provider,
                            app_dir,
                            request,
                            retries_left - 1,
                        );
                    }
                    return Err(author_stalled_error(app_dir, &transcript, quiet.as_secs()));
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // The old shape errored here unconditionally -- while the
                    // error text itself said "the last check-app run actually
                    // passed, re-running should finish the packaging". A
                    // finished app was being thrown away over the ceremony of
                    // the agent not exiting fast enough (K-084). If the oracle
                    // passes, the app is done: package it.
                    if check_app_verdict(app_dir).is_ok() {
                        eprintln!(
                            "note: the AI overran its {} minute budget, but the app it wrote already passes every check -- packaging it.",
                            timeout_secs / 60
                        );
                        break None;
                    }
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
        // A refusal and a delivered, passing app cannot both be the agent's
        // final position. Grok wrote the refusal marker mid-run, then found a
        // way, finished the app, and reported check-app OK -- and the stale
        // marker still failed the whole create. The artifact outranks the
        // remark: only honor a refusal when there is no working app to hand
        // over.
        let lib_now = fs::read_to_string(Path::new(app_dir).join(watched_file)).unwrap_or_default();
        let delivered = lib_now != starter_lib && check_app_verdict(app_dir).is_ok();
        if delivered {
            eprintln!(
                "note: the AI wondered mid-run whether this was possible (\"{reason}\"), \
                 then built the app anyway -- and it passes every check. Keeping the app."
            );
        } else {
            anyhow::bail!(
                "Krate cannot build that: {reason}\n\n\
                 The AI read the request and Krate's full API reference and stopped rather \
                 than build an app that looks right but cannot do what you asked. If you \
                 think it is wrong, re-run with --force."
            );
        }
    }
    // Remember which session wrote this app, so the next round -- a repair
    // in this same build, or a person's revise next week -- can resume it
    // hot instead of paying the cold start again. Tagged with the provider,
    // because a claude session id handed to grok's --resume is garbage.
    if !status.map(|s| provider.failed(&s)).unwrap_or(true) {
        if let Ok(text) = fs::read_to_string(&transcript) {
            if let Some(id) = provider.session_id_in_transcript(&text) {
                let tagged = format!("{}:{}", provider.name(), id);
                for file in &session_files {
                    let _ = fs::write(file, &tagged);
                }
            }
        }
    }
    // A resume is an optimization and must never cost someone their build.
    // The first live failure was exactly that: claude answered "No
    // conversation found with session ID" (a planning session made in
    // another directory), exited non-zero having done nothing, and the
    // person saw "that build didn't come together". The session id was
    // taken, not peeked, so re-entering cannot resume again -- this retry
    // runs fresh, once.
    if resumed && status.map(|s| provider.failed(&s)).unwrap_or(false) {
        eprintln!("    the earlier session could not be continued -- starting fresh instead");
        return run_provider_author(provider, app_dir, request);
    }
    if status.map(|s| provider.failed(&s)).unwrap_or(false) {
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
            None => {
                // No error event in the transcript, which is the hardest
                // case to diagnose and was arriving with nothing attached:
                // a person saw "did not finish successfully; see <file>",
                // the file stayed on their disk, and a build that died in
                // one second could not be told apart from one that ran for
                // ten minutes (K-186).
                //
                // Everything known here goes into the message: how the tool
                // exited, how long it lasted, and the last lines it wrote.
                // A one-second exit with an empty transcript says "the tool
                // never started"; a long one with output says something
                // else entirely, and neither could be seen before.
                let code = status
                    .and_then(|s| s.code())
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "killed by a signal".to_string());
                let text = fs::read_to_string(&transcript).unwrap_or_default();
                let tail: String = text
                    .lines()
                    .rev()
                    .take(12)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let evidence = if tail.trim().is_empty() {
                    format!(
                        "It wrote nothing at all, which usually means the tool \
                         failed to start rather than failed to work."
                    )
                } else {
                    format!("The last thing it wrote:\n\n{tail}")
                };
                anyhow::bail!(
                    "the {} agent stopped without saying why (exit {code}).\n\n{evidence}\n\n\
                     Check that `{}` runs on its own, then try again. The full \
                     transcript is at {}.",
                    provider.name(),
                    provider.program(),
                    transcript.display()
                )
            }
        }
    }
    let lib_after = fs::read_to_string(Path::new(app_dir).join(watched_file)).unwrap_or_default();
    if lib_after == starter_lib {
        // An untouched app usually means the agent explained instead of
        // writing -- but it is also exactly what an agent leaves behind when
        // every one of its tool calls failed. Codex with a broken Windows
        // sandbox helper does this: it exits 0, writes nothing, and the only
        // trace of the real cause is in its transcript. Name that cause
        // instead of blaming the agent's prose.
        let blob = fs::read_to_string(&transcript).unwrap_or_default();
        if let Some((reason, remedy)) = provider.output_failure(&blob, "") {
            let fix = remedy.map(|r| format!("\n\nFix: {r}.")).unwrap_or_default();
            anyhow::bail!(
                "{} {reason}, so it finished without writing any code.{fix}\n\n\
                 The agent's transcript is at {}.",
                provider.name(),
                transcript.display()
            );
        }
        anyhow::bail!(
            "the agent finished without changing the app: {watched_file} is byte-identical \
             to the starter, so this would package an empty app as if it were \
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
    //
    // And when it does not pass, do not hand the user a failure they have to
    // read, understand, and retry by hand. The agent got most of the way and
    // hit something check-app already explains exactly how to fix -- a leaked
    // wasi import, a build error. Feed that verdict straight back to the agent
    // as its next task, in the SAME workspace, and let it finish the job. This
    // is the deep fix for "grok writes an app that panics/leaks and then stops":
    // the loop closes here, automatically, instead of on a person noticing the
    // error and running create again.
    if let Err(failure) = check_app_verdict(app_dir) {
        // Only a fresh authoring run drives the automatic repair loop. A
        // CHANGE_MARKER request IS a repair or an edit already -- auto-repairing
        // it here would nest run_provider_author inside auto_repair inside
        // run_provider_author, multiplying the rounds. At this level a failing
        // change just reports; the outer loop (auto_repair, or the user's own
        // revise) decides what happens next.
        if request.starts_with(CHANGE_MARKER) {
            anyhow::bail!(
                "the agent finished, but `check-app` does not pass yet:\n\n{failure}\n\n\
                 The agent's transcript is at {}.",
                transcript.display()
            );
        }
        return auto_repair(provider, app_dir, request, &failure, &transcript);
    }
    // A fresh authoring run that passed its final check is a finished build.
    // A CHANGE_MARKER run is a repair/edit step inside a larger flow, so it is
    // not the build's end -- auto_repair (or the caller) owns that.
    if !request.starts_with(CHANGE_MARKER) {
        trace::build_end("ok", None);
    }
    Ok(0)
}

/// The first non-empty line of a message, for a one-line trace field.
fn first_line(s: &str) -> &str {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// Turn a `KRATE_TRACE` JSONL file into the study's one-build review row.
///
/// Reads what the pipeline recorded -- phases with durations, the agent's tool
/// calls with the gap before each (its think/act time), every check-app verdict
/// and where it stopped, repair rounds, and the outcome -- and prints it as a
/// review sheet a person (or the analysis) can read at a glance. Parsing is
/// deliberately forgiving: a malformed line is skipped, never fatal.
fn study_report_command(trace: &Path) -> Result<u8> {
    let text = fs::read_to_string(trace)
        .with_context(|| format!("read the trace file {}", trace.display()))?;
    let events: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if events.is_empty() {
        anyhow::bail!(
            "no events in {} -- was KRATE_TRACE set on the build?",
            trace.display()
        );
    }

    let ms = |e: &serde_json::Value| e.get("t").and_then(|v| v.as_u64()).unwrap_or(0);
    let kind = |e: &serde_json::Value| {
        e.get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let field = |e: &serde_json::Value, k: &str| {
        e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let secs = |a: u64, b: u64| format!("{:.1}s", (b.saturating_sub(a)) as f64 / 1000.0);

    let start = events.first().map(ms).unwrap_or(0);
    let end = events.last().map(ms).unwrap_or(start);

    // Header: the build's identity and total time.
    let start_ev = events.iter().find(|e| kind(e) == "build.start");
    let end_ev = events.iter().rev().find(|e| kind(e) == "build.end");
    println!("========================================================");
    if let Some(e) = start_ev {
        println!("REQUEST   {}", field(e, "request"));
        println!("PROVIDER  {}", field(e, "provider"));
    }
    println!("TOTAL     {}", secs(start, end));
    if let Some(e) = end_ev {
        let outcome = field(e, "outcome");
        let detail = field(e, "detail");
        println!(
            "OUTCOME   {}{}",
            outcome.to_uppercase(),
            if detail.is_empty() {
                String::new()
            } else {
                format!("  ({detail})")
            }
        );
    } else {
        println!("OUTCOME   (no build.end -- run killed or crashed)");
    }

    // Phases and their spans.
    println!("\nPHASES");
    let phase_events: Vec<&serde_json::Value> =
        events.iter().filter(|e| kind(e) == "phase").collect();
    for (i, e) in phase_events.iter().enumerate() {
        let from = ms(e);
        let to = phase_events.get(i + 1).map(|n| ms(n)).unwrap_or(end);
        println!("  {:<16} {}", field(e, "name"), secs(from, to));
    }

    // The agent's tool calls, with the gap before each -- long gaps are where
    // it thought (or waited on the model). Flag any gap over 20s.
    let tools: Vec<&serde_json::Value> = events.iter().filter(|e| kind(e) == "tool").collect();
    println!("\nAGENT STEPS  ({} tool calls)", tools.len());
    let mut prev = start;
    let mut first_write: Option<u64> = None;
    for e in &tools {
        let at = ms(e);
        let gap = at.saturating_sub(prev);
        let step = field(e, "step");
        let tool = field(e, "tool");
        let target = field(e, "detail");
        if first_write.is_none()
            && (step.contains("writing the app") || (tool == "Write" && target.ends_with("lib.rs")))
        {
            first_write = Some(at);
        }
        let mark = if step.starts_with("OUTSIDE-WORKSPACE") {
            "  <== OUTSIDE workspace"
        } else if gap > 20_000 {
            "  <-- long pause"
        } else {
            ""
        };
        // Show the target (file or command) so the read/explore sequence is
        // legible, trimmed to keep the row scannable.
        let target_short: String = target.chars().take(58).collect();
        let label = if tool.is_empty() { step } else { tool };
        println!(
            "  {:>7}  +{:<6} {:<7} {}{}",
            secs(start, at),
            secs(prev, at),
            label,
            target_short,
            mark
        );
        prev = at;
    }
    if let Some(fw) = first_write {
        println!("  first code written at {}", secs(start, fw));
    }

    // Every check-app verdict: the iteration count and where rounds were spent.
    let checks: Vec<&serde_json::Value> =
        events.iter().filter(|e| kind(e) == "check_app").collect();
    println!("\nCHECK-APP  ({} Krate-side verdicts)", checks.len());
    for e in &checks {
        let ok = e.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            println!("  {:>7}  OK", secs(start, ms(e)));
        } else {
            println!(
                "  {:>7}  FAIL at {}: {}",
                secs(start, ms(e)),
                field(e, "stage"),
                field(e, "detail")
            );
        }
    }

    // Repair rounds.
    let repairs: Vec<&serde_json::Value> = events.iter().filter(|e| kind(e) == "repair").collect();
    if !repairs.is_empty() {
        println!("\nAUTO-REPAIR  ({} rounds fired)", repairs.len());
        for e in &repairs {
            println!(
                "  round {} of {}: {}",
                e.get("round").and_then(|v| v.as_u64()).unwrap_or(0),
                e.get("of").and_then(|v| v.as_u64()).unwrap_or(0),
                field(e, "because")
            );
        }
    }

    // Stalls: any gap over 60s anywhere in the event stream.
    let mut stalls = Vec::new();
    let mut prev = start;
    for e in &events {
        let at = ms(e);
        if at.saturating_sub(prev) > 60_000 {
            stalls.push((secs(start, prev), secs(prev, at)));
        }
        prev = at;
    }
    if !stalls.is_empty() {
        println!("\nSTALLS  (dead air over 60s)");
        for (at, dur) in stalls {
            println!("  at {} -- {} of silence", at, dur);
        }
    }
    println!("========================================================");
    Ok(0)
}

/// After the agent stops with a failing check-app, re-invoke it to fix exactly
/// what failed -- up to a small number of rounds -- rather than making the user
/// retry by hand.
///
/// The verdict check-app produces is already a precise fix instruction (the
/// no_std remedy for a wasi leak, the compiler error for a build failure), so
/// the repair task is that verdict verbatim. Bounded rounds, because an agent
/// that cannot fix it in a few tries will not fix it in twenty, and each round
/// costs a real model run; the last failure is surfaced with the same honest
/// message as before so nothing is hidden.
fn auto_repair(
    provider: &'static dyn agent_provider::AgentProvider,
    app_dir: &str,
    request: &str,
    first_failure: &str,
    transcript: &Path,
) -> Result<u8> {
    const MAX_REPAIR_ROUNDS: u32 = 2;
    let mut failure = first_failure.to_string();
    for round in 1..=MAX_REPAIR_ROUNDS {
        trace::repair(round, MAX_REPAIR_ROUNDS, first_line(&failure));
        eprintln!(
            "    the app did not pass yet -- asking the AI to fix it ({round} of {MAX_REPAIR_ROUNDS})"
        );
        let task = format!(
            "{CHANGE_MARKER}The app you just wrote for \"{request}\" does not pass \
             `krate check-app` yet. Here is exactly what failed and how to fix it:\n\n\
             {failure}\n\n\
             Fix only what this names. Do not rewrite the app or start over -- the code \
             you already wrote is in src/lib.rs. Make the change, then run check-app \
             again, and do not stop until it prints OK.",
        );
        // A repair is a change to the existing code, carried as a CHANGE_MARKER
        // task -- run_provider_author dispatches that to the edit prompt, which
        // says "find the one place, change that, leave the rest alone" rather
        // than re-deriving the whole app.
        run_provider_author(provider, app_dir, &task)?;
        match check_app_verdict(app_dir) {
            Ok(()) => {
                eprintln!("    fixed.");
                trace::build_end("ok", Some("fixed by auto-repair"));
                return Ok(0);
            }
            Err(next) => failure = next,
        }
    }
    trace::build_end("failed", Some(first_line(&failure)));
    anyhow::bail!(
        "the agent finished, but `check-app` does not pass yet:\n\n{failure}\n\n\
         The agent's transcript is at {}. Running the command again often gets it \
         the rest of the way.",
        transcript.display()
    );
}

/// Pull the agent's own error sentence out of its transcript.
///
/// Providers stream JSON lines, and a failure is usually one clear sentence
/// buried among hundreds of events -- often nested as a JSON string inside a
/// JSON field. Best-effort by design: an unrecognized shape returns None and
/// the caller falls back to naming the transcript.
fn agent_failure_reason(transcript: &Path) -> Option<String> {
    let text = fs::read_to_string(transcript).ok()?;
    agent_failure_reason_in(&text)
}

/// The same reader, over text already in hand.
///
/// Split out because the plan step has the provider's output in memory and
/// never writes a transcript, and it needs exactly this question answered:
/// did the AI fail, or did it merely answer in a shape we cannot parse? The
/// two were indistinguishable there, and a refusal became a build (K-182).
fn agent_failure_reason_in(text: &str) -> Option<String> {
    let mut last = None;
    // A plain-text error, kept separately and used only if no JSON event is
    // found. Tools write their fatal error to stderr as prose as well as
    // emitting it as an event, and the prose is the copy that survives.
    let mut plain: Option<String> = None;
    let mut plain_wants_more = false;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            // Grok's "Error: Not signed in..." arrives here.
            //
            // The transcript has two writers with no lock between them --
            // stderr is wired to the file at spawn, and the reporter thread
            // appends every stdout line to the same file -- so a long JSON
            // event can be torn in half by a stderr write landing mid-line.
            // That is exactly what happened to the first outside user who
            // tried Grok: her transcript ends with the orphan fragment
            //   achine with a browser."}
            // which is the tail of the error event, cut in two. The JSON was
            // unparseable, this loop skipped every line, and she was told
            // "the grok agent did not finish successfully; see <file>" when
            // the file itself said "Not signed in" in plain English (K-187).
            //
            // Reading the prose too makes the answer independent of whether
            // the JSON survived the race.
            if let Some(rest) = line.strip_prefix("Error:").or_else(|| line.strip_prefix("error:"))
            {
                let rest = rest.trim();
                if !rest.is_empty() && plain.is_none() {
                    plain = Some(rest.to_string());
                    // Keep collecting: the command that fixes it is on the
                    // NEXT line ("  grok login --device-code"), and a reason
                    // that names the problem without the cure sends the
                    // person back to a search engine.
                    plain_wants_more = true;
                    continue;
                }
            }
            // The indented continuation of an error we just started reading.
            if plain_wants_more {
                if line.is_empty() {
                    plain_wants_more = false;
                } else if let Some(existing) = plain.as_mut() {
                    existing.push(' ');
                    existing.push_str(line);
                }
            }
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
            // Claude Code puts the sentence in `result` on its final event,
            // with is_error set. Without this the useful line -- "OAuth
            // session expired and could not be refreshed" -- was skipped and
            // the person got a bare "the agent did not finish successfully",
            // which names neither the cause nor the fix.
            .or_else(|| event.get("result"))
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
            || event.get("error").is_some()
            // Claude's final event is type "result" with this flag, not type
            // "error", so the flag is what has to be read.
            || event.get("is_error").and_then(|v| v.as_bool()) == Some(true);
        if is_error && !unwrapped.trim().is_empty() {
            last = Some(unwrapped.trim().to_string());
        }
    }
    // A JSON event is preferred -- it is the structured, intended channel --
    // but the prose stands in when the event did not survive.
    last.or(plain).map(|reason| {
        // One sentence, not a wall. Long provider errors repeat themselves.
        let trimmed: String = reason.chars().take(300).collect();
        // An expired sign-in is the commonest failure of all and the message
        // for it never says what to do. Add the command.
        let lower = trimmed.to_lowercase();
        if lower.contains("oauth")
            || lower.contains("authenticate")
            || lower.contains("authentication")
            || lower.contains("session expired")
            || lower.contains("not logged in")
        {
            return format!("{trimmed}\n\n  Sign in again, then try once more.");
        }
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

/// The same answer as `ai_status`, as one JSON array.
///
/// Every provider appears, including missing ones -- a frontend deciding
/// what to offer needs the full set, not only the happy rows. States:
/// `working`, `not-ready` (installed but refused, with the reason and the
/// likely fix), `missing`.
fn ai_status_json() -> Result<u8> {
    // Seed before probing, or the probe answers about a home the person's
    // sign-in has not reached yet.
    //
    // The probe runs in the confined home (K-190) so its answer matches what
    // a build will get. But the credential only arrives there when
    // seed_agent_home runs, and that happened at BUILD time -- so someone who
    // signed in to Claude and came straight back to the picker was told the
    // tool was not ready, correctly, about a home nothing had seeded. The
    // sign-in was real; the confined copy did not exist yet. Seeding here
    // costs a few file copies and makes "ready" mean the same thing in the
    // picker as it does in the build.
    if let Some(home) = home_dir() {
        let agent_home = agent_home_for(&home);
        let _ = seed_agent_home(&home, &agent_home);
    }
    let rows: Vec<serde_json::Value> = std::thread::scope(|scope| {
        let handles: Vec<_> = agent_provider::PROVIDERS
            .iter()
            .map(|provider| {
                scope.spawn(move || {
                    let readiness = probe_with_cache(*provider);
                    let (state, detail, remedy) = match &readiness {
                        agent_provider::Readiness::Working => ("working", String::new(), None),
                        agent_provider::Readiness::NotReady { summary, remedy } => {
                            ("not-ready", summary.clone(), remedy.clone())
                        }
                        agent_provider::Readiness::Missing => {
                            ("missing", provider.install_hint().to_string(), None)
                        }
                    };
                    let name = provider.name();
                    let mut label = name.to_string();
                    if let Some(first) = label.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    serde_json::json!({
                        "name": name,
                        "label": label,
                        "state": state,
                        "detail": detail,
                        "remedy": remedy,
                        // The npm package, so a GUI can offer to install it
                        // rather than printing a command and sending someone
                        // to a terminal.
                        "install_package": provider.install_package(),
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });
    // A stored API key is a way to author, so it belongs in the same list
    // the picker reads. No probe: having the key IS the readiness, and a
    // round trip to the vendor to prove it would cost money on every open
    // of the settings sheet (K-218).
    let mut rows = rows;
    for vendor in [api_key::ApiVendor::Anthropic, api_key::ApiVendor::OpenAi] {
        let held = api_key::load(vendor);
        let (state, detail) = match &held {
            Some((_, source)) => ("working", format!("API key, {}", source.describe())),
            None => (
                "missing",
                format!("needs an API key, or set {}", vendor.env_var()),
            ),
        };
        rows.push(serde_json::json!({
            "name": vendor.name(),
            "label": vendor.label(),
            "state": state,
            "detail": detail,
            "remedy": if held.is_some() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(format!("krate api-key set {}", vendor.name()))
            },
            "kind": "api",
        }));
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(0)
}

/// Where one provider's cached probe verdict lives. One file per provider,
/// because the probes run in parallel and a shared file's read-modify-write
/// would let one thread's save drop another's row.
fn probe_cache_file(provider: &str) -> Option<PathBuf> {
    if !provider
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    Some(
        home_dir()?
            .join(".krate")
            .join("cache")
            .join(format!("ai-probe-{provider}.json")),
    )
}

/// A probe, answered from cache when the cached answer still holds.
///
/// The real probe runs each tool (a text-only check misses the failures that
/// matter -- see probe_args), so probing every provider costs seconds and one
/// slow tool holds the whole answer to its 20-second timeout. The Studio asks
/// on every launch and again on window focus, which made opening the app feel
/// broken: the home screen waited twenty seconds on a question whose answer
/// had not changed since the last launch.
///
/// Only WORKING verdicts are cached, for fifteen minutes, keyed to the tool's
/// resolved path and mtime. A working tool rarely stops working inside a
/// quarter hour, and a reinstall or update changes the mtime and re-probes.
/// Not-ready and missing are never cached: those are exactly the states a
/// person is actively fixing, and the recheck existing to notice the fix must
/// not be blinded by its own cache.
fn probe_with_cache(provider: &dyn agent_provider::AgentProvider) -> agent_provider::Readiness {
    const TTL: u64 = 15 * 60;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let resolved = agent_provider::which_on_path(provider.program());
    let key = resolved.as_ref().map(|p| {
        let mtime = std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        (p.display().to_string(), mtime)
    });

    let cache_file = probe_cache_file(provider.name());
    let row: Option<serde_json::Value> = cache_file
        .as_ref()
        .and_then(|f| fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok());
    if let (Some((path, mtime)), Some(row)) = (&key, &row) {
        let fresh = row["when"]
            .as_u64()
            .is_some_and(|w| now.saturating_sub(w) < TTL);
        let same = row["path"].as_str() == Some(path.as_str())
            && row["mtime"].as_u64() == Some(*mtime)
            && row["verdict"].as_str() == Some("working");
        if fresh && same {
            return agent_provider::Readiness::Working;
        }
    }

    let readiness = agent_provider::probe(provider, std::time::Duration::from_secs(20));
    if let (Some((path, mtime)), agent_provider::Readiness::Working, Some(file)) =
        (&key, &readiness, &cache_file)
    {
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")));
        let row = serde_json::json!({
            "path": path,
            "mtime": mtime,
            "verdict": "working",
            "when": now,
        });
        let _ = fs::write(file, row.to_string());
    }
    readiness
}

/// Show which AI coding tools are on this machine.
///
/// This is the "connect your AI" step, and it is deliberately a lookup rather
/// than a login: every one of these tools already has its own sign-in, and
/// Krate holding a copy of someone's credentials would be strictly worse than
/// the tool holding its own. Nothing here reads a key, opens a browser, or
/// talks to a server -- it looks at PATH and tells you what you can use.
fn ai_status(json: bool) -> Result<u8> {
    if json {
        return ai_status_json();
    }
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
    // How far it got, in the only unit that matters to the person: did the
    // AI write a real app, or was it still reading? Three attempts at one
    // game each stopped mid-research, and "did not finish" told the person
    // nothing about whether trying again was worth it (K-129).
    let written = fs::read_to_string(Path::new(app_dir).join("src/lib.rs"))
        .map(|text| text.lines().count())
        .unwrap_or(0);
    let progress = if written > 400 {
        format!("It had written {written} lines of your app, and a retry continues from them.")
    } else if written > 200 {
        format!("It had written {written} lines -- a start, and a retry continues from there.")
    } else {
        "It was still reading Krate's API when the time ran out, so there is \
         little code yet; a retry starts it writing sooner."
            .to_string()
    };
    let verdict = match check_app_verdict(app_dir) {
        Ok(()) => "The last check-app run actually passed -- re-running the command should \
                   finish the packaging."
            .to_string(),
        Err(failure) => format!("The last check-app run reported:\n\n{failure}"),
    };
    anyhow::anyhow!(
        "the AI agent did not finish within {minutes} minutes and was stopped.\n\n\
         {progress}\n\n{verdict}\n\n\
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
/// Whether the interactive loop is bounded by a round or frame count.
///
/// The single most common way a generated app fails a person: it runs for
/// thirty or forty seconds and closes itself while they are reading. The
/// authoring pack says not to, at length, and apps still do it -- a news app
/// bounded its loop at 600 rounds of 50ms and quit after half a minute.
///
/// check-app's usability stage cannot catch this: it watches for five seconds,
/// marks the app as having stayed open, and closes it. Anything with a longer
/// bound passes and still quits on the user.
///
/// Deliberately looks for a bound that is NOT gated on `quick`. A limit on the
/// quick path is correct and required -- that is how a headless check finishes.
fn bounded_interactive_loop(lib: &str) -> Option<String> {
    for (number, line) in lib.lines().enumerate() {
        let trimmed = line.trim();
        // The shape that bites: the loop's limit is picked by `quick`, and the
        // else branch -- the real session -- still gets a finite number.
        //
        //     let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };
        //
        // A news app wrote exactly that and closed itself after thirty
        // seconds, mid-read.
        let Some(rest) = trimmed.split(" else ").nth(1) else {
            continue;
        };
        if !trimmed.contains("if quick") {
            continue;
        }
        let interactive = rest
            .trim()
            .trim_start_matches('{')
            .trim_end_matches(';')
            .trim_end_matches('}')
            .trim();
        // An unbounded interactive branch is the correct shape and common:
        // u32::MAX, usize::MAX, or a literal nobody will reach.
        if interactive.contains("::MAX") {
            continue;
        }
        // A bare identifier in SCREAMING_CASE is a constant round count.
        let is_const = !interactive.is_empty()
            && interactive
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !is_const {
            continue;
        }
        // How long does that bound actually last? A limit measured in hours is
        // a practical "forever" and not the bug -- krate-notes uses 600_000
        // rounds of 50ms, which is 8.3 hours and nobody's session. The bug is
        // a bound somebody reaches while reading: the news app's 600 rounds is
        // thirty seconds.
        // Five minutes. Below it, somebody reading or playing will hit the
        // bound and watch the app vanish -- the news app's thirty seconds is
        // the case this exists for. Above it, the bound is a backstop against
        // a runaway rather than a limit on the session: krate-nova's 100_000
        // frames is about twenty-eight minutes of play, which nobody reaches
        // by accident and which is not what closes an app mid-sentence.
        const A_REAL_SESSION_SECS: u64 = 5 * 60;
        if let Some(seconds) = loop_bound_seconds(lib, interactive) {
            if seconds >= A_REAL_SESSION_SECS {
                continue;
            }
        }
        // Now confirm it actually bounds the loop rather than being read on the
        // quick path only. `while !quick || frames < cap` is correct: the bound
        // applies only when quick, which is what krate-paint does.
        let guarded = lib
            .lines()
            .any(|l| l.contains("while !quick") || l.contains("while quick"));
        if guarded {
            continue;
        }
        return Some(format!(
            "line {}: the interactive loop is bounded by a round count ({interactive}), so \
             the app closes itself while somebody is still using it. A limit on the \
             `quick` path is right and necessary; the same limit on a real session is the \
             commonest way a generated app fails the person using it.",
            number + 1
        ));
    }
    None
}

/// Roughly how many seconds a round-count bound lasts, if both the count and
/// the per-round wait can be read from the source.
///
/// Returns None when either is not a plain literal, in which case the caller
/// treats the bound as suspicious -- a limit nobody can measure is one nobody
/// checked.
fn loop_bound_seconds(lib: &str, bound_name: &str) -> Option<u64> {
    let literal = |name: &str| -> Option<u64> {
        lib.lines()
            .find(|l| l.trim_start().starts_with(&format!("const {name}")))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_end_matches(';').replace('_', ""))
            .and_then(|v| {
                v.trim_end_matches(|c: char| c.is_ascii_alphabetic())
                    .parse::<u64>()
                    .ok()
            })
    };
    let rounds = literal(bound_name)?;
    // The wait is whatever constant ends in MILLIS; apps name it variously.
    let millis = lib
        .lines()
        .filter(|l| l.trim_start().starts_with("const ") && l.contains("MILLIS"))
        .find_map(|l| {
            let name = l.split_whitespace().nth(1)?.trim_end_matches(':');
            literal(name)
        })
        // A game with no wait constant runs flat out and its bound counts
        // frames, not rounds. The runtime paces `present` to 60fps, so 16ms a
        // frame is the honest number -- krate-nova's 100_000 frames is about
        // 28 minutes, not the thirty seconds this check is looking for.
        .unwrap_or(16);
    Some(rounds.saturating_mul(millis) / 1000)
}

fn check_app_verdict(app_dir: &str) -> std::result::Result<(), String> {
    match run_check_app(Path::new(app_dir), None, false) {
        Ok(_) => {
            trace::check_app(true, None, None);
            Ok(())
        }
        Err(failure) => {
            trace::check_app(false, Some(failure.stage.label()), Some(&failure.detail));
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
    // The model-starter seed (K-205): when the request confidently matches
    // an embedded example and wants a GUI, that example becomes the
    // STARTING src/lib.rs and manifest -- a complete app that builds, runs
    // and passes check-app -- and the prompt asks for a transformation
    // instead of authorship. The stamped build put 82% of wall time in the
    // model generating a whole file from silence; starting close means it
    // edits a delta. Same K-078 rule as the skeleton: a retry's work is
    // never overwritten.
    // The SYSTEM: the plan step already read this request with the model's
    // eyes and chose the closest working shape (KRATE_STARTER_SHAPE, set by
    // the studio from the plan's "shape" field). The model is the matcher;
    // keyword matching below survives only as the fallback for a bare
    // `krate create` that never planned.
    let model_starter = match std::env::var("KRATE_STARTER_SHAPE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(shape) if shape.eq_ignore_ascii_case("none") => None,
        Some(shape) => authoring_context::example_by_name(&shape).or_else(|| {
            if matches!(world, krate_author::Skeleton::Gui) {
                authoring_context::closest_example_matched(ctx.request)
            } else {
                None
            }
        }),
        None => {
            if matches!(world, krate_author::Skeleton::Gui) {
                authoring_context::closest_example_matched(ctx.request)
            } else {
                None
            }
        }
    };
    if let Some(example) = model_starter {
        let lib_dest = ctx.app_dir.join("src/lib.rs");
        if !lib_dest.exists() {
            let _ = fs::write(&lib_dest, example.lib);
            let snake = ctx.name.replace('-', "_");
            let manifest: String = example
                .manifest
                .lines()
                .map(|line| {
                    let t = line.trim_start();
                    if t.starts_with("id =") || t.starts_with("id=") {
                        format!("id = \"dev.krate.{snake}\"")
                    } else if t.starts_with("name =") || t.starts_with("name=") {
                        format!("name = \"{}\"", ctx.name)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let _ = fs::write(ctx.app_dir.join("manifest.toml"), manifest);
        }
        let _ = fs::write(
            ctx.app_dir.join(".starter-mode"),
            format!("model:{}: {}", example.name, example.shows),
        );
    }
    if let Ok(app) = skeleton(ctx.name, ctx.sdk_prefix, world) {
        for file in &app.files {
            let dest = ctx.app_dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            // Write only what is not already there. A retry runs over the
            // previous attempt's workspace, and the stall error promises it
            // "resumes from the code already written" -- a promise this loop
            // used to break by putting the starter back over the agent's
            // src/lib.rs before every run (K-078).
            if !dest.exists() {
                let _ = fs::write(&dest, &file.contents);
            }
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
    // The planning session, when the caller carried one over
    // (KRATE_PLAN_SESSION, set by the studio from `krate plan`'s answer).
    // Written into the workspace so the authoring run RESUMES the session
    // that planned: the request and the agreed plan are already in its
    // context instead of being re-sent to a cold start.
    if let Ok(tagged) = std::env::var("KRATE_PLAN_SESSION") {
        let tagged = tagged.trim();
        if !tagged.is_empty() && tagged.len() < 160 {
            let _ = fs::write(ctx.app_dir.join(".agent-session-id"), tagged);
        }
    }
    // The model app for THIS request, picked here rather than hunted by the
    // agent. "Find the closest example" used to cost minutes of exploratory
    // reads on a dev machine and was impossible on an installed Krate, where
    // apps/ does not exist. Written fresh each run (unlike the skeleton):
    // it is reference material, never the agent's work in progress.
    // Skipped in model-starter mode: the starter IS the example, and a
    // duplicate EXAMPLE.rs would cost the agent a 30KB read for nothing.
    if model_starter.is_none() {
        let example = authoring_context::closest_example(ctx.request);
        let _ = fs::write(ctx.app_dir.join("EXAMPLE.rs"), example.lib);
        let _ = fs::write(ctx.app_dir.join("EXAMPLE.manifest.toml"), example.manifest);
    }

    // Warm the dependency cache while the model thinks. The skeleton
    // compiles the whole shared dep graph in the background, so the agent's
    // FIRST check-app -- usually issued a minute into authoring -- meets hot
    // artifacts instead of paying the cold build inside the person's wait.
    {
        let warm_dir = ctx.app_dir.to_path_buf();
        std::thread::spawn(move || {
            // Silent on purpose: build_component prints the compiler's words,
            // and a background warm-up interleaving rustc noise through the
            // authoring progress display would read as chaos.
            let _ = component_build_command(&warm_dir).output();
        });
    }

    // The command Krate builds for a known provider is this binary calling
    // itself -- `'<exe>' author-agent <name>` -- and needs no shell at all.
    // Spawning it directly matters on Windows, where the shell run needs a
    // POSIX bash that plain machines do not have: the probe found the agent,
    // the studio's chip showed green, and create then died with "program not
    // found" trying to start bash. Only a hand-written --author-cmd, which
    // may be an arbitrary pipeline, still goes through a shell.
    let mut command = if let Some(agent) = self_author_agent(ctx.cmd) {
        let exe = std::env::current_exe().context("find this binary to drive the agent")?;
        let mut direct = std::process::Command::new(exe);
        direct.arg("author-agent").arg(agent);
        direct
    } else {
        let mut through_shell = std::process::Command::new(author_shell());
        through_shell.arg("-c").arg(ctx.cmd);
        through_shell
    };
    // The same environment repair the readiness probe uses.
    //
    // Detecting the tool and running it have to agree: an app launched from
    // Finder has neither the PATH these CLIs install into nor, reliably, USER
    // -- and Claude Code needs USER to reach its keychain. Without this the
    // studio could report an AI as working and then fail to author with it.
    agent_provider::with_tool_path(&mut command);
    command
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
                // A shell reports a missing program on stderr and exits 127.
                // Nothing was captured, so the exit code is the only evidence
                // we have -- and "the AI tool is not installed" is a different
                // problem from "the AI tried and failed", with a different fix.
                anyhow::bail!("{}", silent_author_failure(status.code()));
            }
            // A known environment signature beats any guess: seen live when
            // Codex's own Windows sandbox helper was missing and every
            // command the agent ran failed with orchestrator_helper_launch_failed.
            let transcript_text =
                fs::read_to_string(ctx.app_dir.join(".agent-transcript.txt")).unwrap_or_default();
            if transcript_text.contains("orchestrator_helper_launch_failed")
                || tail.contains("orchestrator_helper_launch_failed")
            {
                anyhow::bail!(
                    "the AI's own sandbox is broken on this machine: its helper program is \
                     missing, so every command it ran failed. Reinstalling that AI fixes it; \
                     picking a different AI works right now.\n\nauthor command failed"
                );
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
    if let Some(toolchain) = working_windows_toolchain() {
        // Whichever toolchain can genuinely build here, proven by building.
        // Preferring gnullvm by NAME routed every build on a real machine
        // into the one toolchain that could not link, while the MSVC
        // toolchain beside it worked (K-130).
        let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
        let out = ProcessCommand::new(rustup)
            .args(["run", &toolchain, "rustc", "--print", "sysroot"])
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

/// What to say when the author command failed and wrote nothing we captured.
///
/// The exit code is the whole story here. A shell uses 127 for "command not
/// found" and 126 for "found but not executable", and both mean the AI tool is
/// not really installed -- telling somebody "author command failed" in that
/// case sends them to debug their request when the fix is an install.
fn silent_author_failure(code: Option<i32>) -> String {
    match code {
        Some(127) => "that AI tool is not installed on this machine, or the shell \
cannot find it. Install it, open a new terminal so the new PATH is picked up, \
and try again."
            .to_string(),
        Some(126) => "that AI tool is installed but could not be run -- it may not \
have permission to execute."
            .to_string(),
        Some(code) => format!("the AI tool stopped with error {code} and said nothing."),
        None => "the AI tool was stopped before it finished.".to_string(),
    }
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
    // Make rustup's toolchain win over a Homebrew rust for the child build.
    // cargo-component resolves `cargo` and `rustc` from PATH, and on a
    // machine with both installs Homebrew's can come first -- a rustc with
    // no wasm32-wasip1 target and whatever version brew last shipped, so a
    // perfectly good app fails to build depending on shell setup. rustup's
    // shims (~/.cargo/bin) know the installed targets and honor the
    // person's chosen default, so they go first when they exist. This bit
    // both the CLI and the MCP server on the same machine.
    if let Some(home) = home_dir() {
        let shims = home.join(".cargo").join("bin");
        let rustc = if cfg!(windows) { "rustc.exe" } else { "rustc" };
        if shims.join(rustc).exists() {
            let current = std::env::var_os("PATH").unwrap_or_default();
            let joined =
                std::env::join_paths(std::iter::once(shims).chain(std::env::split_paths(&current)));
            if let Ok(path) = joined {
                command.env("PATH", path);
            }
        }
    }
    // One shared build cache for every generated app, keyed by SDK version.
    //
    // Each app's Cargo.toml depends on the same SDK crate graph, and cargo
    // recompiled the whole graph from zero into every app's own target/ --
    // minutes of a "several minute" create that repeat identically per app.
    // A shared CARGO_TARGET_DIR compiles the dependencies once per machine
    // per SDK version; after the first app, later builds compile only the
    // app's own crate. Cargo takes a lock on the dir, so concurrent builds
    // serialize instead of corrupting. An explicit CARGO_TARGET_DIR from the
    // environment still wins.
    if let Some(shared) = shared_build_dir() {
        let _ = fs::create_dir_all(&shared);
        command.env("CARGO_TARGET_DIR", &shared);
    }

    #[cfg(windows)]
    point_gnullvm_at_its_own_linker(&mut command);

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

/// The shared dependency cache all generated apps build into, keyed by SDK
/// version so a WIT change can never serve stale artifacts. None when the
/// person set CARGO_TARGET_DIR themselves -- their setting wins.
fn shared_build_dir() -> Option<PathBuf> {
    if std::env::var_os("CARGO_TARGET_DIR").is_some() {
        return None;
    }
    home_dir().map(|home| {
        home.join(".cache/krate/build")
            .join(env!("KRATE_SDK_FINGERPRINT"))
    })
}

fn find_built_component(app_dir: &Path) -> Result<PathBuf> {
    let target_root = shared_build_dir().unwrap_or_else(|| app_dir.join("target"));
    let release = target_root.join("wasm32-wasip1/release");
    // The shared cache holds every app's artifact side by side, so the match
    // must be by THIS crate's name, not "any wasm in the directory".
    let wanted = fs::read_to_string(app_dir.join("Cargo.toml"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let line = line.trim();
                line.strip_prefix("name")
                    .and_then(|rest| rest.split('"').nth(1))
                    .map(|name| format!("{}.wasm", name.replace('-', "_")))
            })
        });
    if let Some(name) = &wanted {
        let exact = release.join(name);
        if exact.exists() {
            // The manifest's entry names target/wasm32-wasip1/release/<name>
            // relative to the app, and the permission wall verifies the file
            // RUN is the file DECLARED. The deps stay in the shared cache;
            // the artifact itself always comes home to the declared path.
            if shared_build_dir().is_some() {
                let local = app_dir.join("target/wasm32-wasip1/release");
                fs::create_dir_all(&local)?;
                let home = local.join(name);
                fs::copy(&exact, &home).with_context(|| format!("copying {}", exact.display()))?;
                return Ok(home);
            }
            return Ok(exact);
        }
        // Never fall back to "any wasm" when the name is known: the shared
        // cache holds every app's artifact, and grabbing a neighbour's led
        // check-app to verify the wrong program entirely.
        anyhow::bail!("the build produced no {} in {}", name, release.display());
    }
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
    // Otherwise any capability whose absence would actually stop the app.
    //
    // "Not granted by default" has to be asked of the registry, not guessed
    // from a prefix. The old check excluded `io.` and `ui.window` by name and
    // let `gfx.gpu:basic` through -- which the runtime grants to every app, so
    // withholding it changes nothing, the app runs fine, and the wall check
    // reports "should refuse with exit 5, got 0" and throws the work away.
    // That is what killed a finished edit after it had already built and
    // packed.
    let granted_anyway: std::collections::BTreeSet<String> =
        krate_manifest::supported_capability_specs()
            .iter()
            .filter(|spec| spec.default_granted())
            .map(|spec| spec.name())
            .collect();
    required.into_iter().find(|c| {
        // Withholding the window just closes the app rather than refusing it.
        if c.starts_with("ui.window") {
            return false;
        }
        // Scoped names arrive as `module.action:scope`; the registry knows
        // them by `module.action`.
        let base = c.split(':').next().unwrap_or(c);
        !granted_anyway.contains(base) && !granted_anyway.contains(c.as_str())
    })
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
///
/// Has to clear the usability run's own deadline: the stay-open watch (15s) plus
/// the headless budget's slack (5s) plus build, settle, and per-frame wasm
/// overhead. A flat 60s cleared it on a fast release build and cut a healthy app
/// off on slower execution, reporting "did not finish" for a run about to pass.
/// 90s clears the 20s deadline with wide margin while still bounding a genuinely
/// hung run.
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

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

    // `krate run krate.tech/notes.krate` -- the short command a page prints,
    // retyped without its scheme. Only when nothing on disk has that name
    // AND the first segment reads as a host does the target become https.
    // A real file always wins; this can never shadow one.
    if !path.exists() {
        if let Some(url) = krate_bundle::implied_url(target) {
            let bundle = krate_bundle::fetch(&url, insecure_http)
                .with_context(|| format!("could not open bundle from {url} (from `{target}`)"))?;
            return Ok((
                bundle.component_path().to_path_buf(),
                Some(bundle.manifest_path().to_path_buf()),
                Some(bundle),
            ));
        }
    }

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
    //
    // And *why* it failed is the number that can be acted on. Counting only
    // the bit left us with "one open in eleven fails" and no way to tell a
    // missing file from a crash from the permission wall doing its job
    // (K-100). `classify` sorts a finished run into a closed set of
    // categories, none of which can carry anything about the person.
    let opened = run_component_inner(request);
    let why = usage::OpenFailure::classify(&opened);
    usage::record_with(
        usage::Action::Open,
        usage::Facts {
            ai: None,
            ok: Some(why.is_none()),
            why,
        },
    );
    opened
}

/// Fail early, and in plain words, when the X11 keyboard library is absent.
///
/// winit loads the X11 keyboard bridge with dlopen when it opens a window,
/// and the crate that loads it ends in `.expect(...)` -- so a missing library
/// is a Rust backtrace with a crate path and a line number, for an app that
/// built and packed perfectly. That is the thing Krate promises a
/// non-developer never sees.
///
/// The check is deliberately the same dlopen the loader will do, rather than a
/// filesystem guess: it searches paths we do not want to reimplement, and
/// being wrong in the optimistic direction just restores the panic.
///
/// **Both sonames, in the loader's order.** An earlier version probed only the
/// unversioned `libxkbcommon-x11.so` and then told people to install the `-dev`
/// package. `xkbcommon-dl` actually tries `libxkbcommon-x11.so.0` first
/// (xkbcommon-dl-0.4.2/src/x11.rs:50), so the runtime package alone is enough
/// and always was. The old check refused machines where apps ran fine, and
/// sent a person who only wanted to open an app to install a developer
/// package. Asking exactly what the loader asks is the only way this stays
/// correct.
/// The sonames `xkbcommon-dl` tries, in its order. Kept identical to that
/// crate's list so this asks the same question the loader will.
#[cfg(all(unix, not(target_os = "macos")))]
const XKB_X11_SONAMES: [&[u8]; 2] = [b"libxkbcommon-x11.so.0\0", b"libxkbcommon-x11.so\0"];

#[cfg(all(unix, not(target_os = "macos")))]
fn check_window_libraries() -> Result<()> {
    // No display at all means no window, so nothing to check.
    //
    // Deliberately NOT skipped when WAYLAND_DISPLAY is set. A Wayland session
    // running XWayland sets both variables, and winit may still take the X11
    // path -- skipping there would restore the panic on exactly the setup most
    // modern Linux desktops have. The check is a dlopen that costs microseconds
    // and closes the handle again, so running it needlessly costs nothing while
    // skipping it needlessly costs a Rust backtrace in somebody's face.
    if std::env::var_os("DISPLAY").is_none() {
        return Ok(());
    }

    for library in XKB_X11_SONAMES {
        // SAFETY: null-terminated literals, and the handle is closed on
        // success. This only asks whether the loader can find the name.
        let handle = unsafe { libc::dlopen(library.as_ptr().cast(), libc::RTLD_LAZY) };
        if !handle.is_null() {
            unsafe { libc::dlclose(handle) };
            return Ok(());
        }
    }

    anyhow::bail!(
        "this computer is missing a library apps need to read the keyboard.\n\n\
         Install it with:\n\n    \
         sudo apt install libxkbcommon-x11-0\n\n\
         (on Fedora: sudo dnf install libxkbcommon-x11; on Arch it is part of \
         libxkbcommon.)"
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

    // Wear the app's own name and icon before any window appears.
    //
    // Without this every app someone makes shows up as "Krate" in the dock,
    // because that is the process running it -- a calculator they built should
    // present as their calculator, the way a native app does. One process per
    // opened document already, so the identity is safe to set per process.
    //
    // The icon is the bundle's own `assets/icon.png` when it ships one;
    // otherwise the app keeps the Krate mark, which is the honest fallback.
    #[cfg(target_os = "macos")]
    {
        // Only GUI apps have a dock tile to name; a CLI app has no window and
        // renaming that process would show up as a phantom in the switcher.
        if let Some(manifest) = manifest.filter(|manifest| {
            matches!(
                manifest.app_world(),
                Ok(krate_manifest::AppWorld::Phase3Gui)
            )
        }) {
            let icon = bundle
                .as_ref()
                .and_then(|bundle| bundle.assets_path().map(Path::to_path_buf))
                .map(|assets| assets.join("icon.png"))
                .filter(|path| path.is_file())
                .and_then(|path| std::fs::read(path).ok())
                // No icon of its own: the Krate mark, never the generic
                // exec page. NSImage decodes .icns bytes as readily as PNG.
                .or_else(|| krate_icon_source().and_then(|p| std::fs::read(p).ok()));
            krate_adapter_macos::set_process_identity(&manifest.app.name, icon.as_deref());
        }
    }

    // Linux: export the app's icon for the window that is about to open.
    // WM_CLASS carries the name (the adapter sets it from the title); the
    // icon travels as a temp PNG named in KRATE_WINDOW_ICON, decoded by the
    // adapter into the window's own icon -- task switcher, dock, and titlebar
    // all show the app, not a generic square.
    #[cfg(target_os = "linux")]
    if let Some(manifest) = manifest {
        if let Some(icon) = bundle
            .as_ref()
            .and_then(|bundle| bundle.assets_path().map(Path::to_path_buf))
            .map(|assets| assets.join("icon.png"))
            .filter(|path| path.is_file())
        {
            let dest = std::env::temp_dir().join(format!(
                "krate-icon-{}.png",
                manifest.app.id.replace(['/', ':'], "-")
            ));
            if fs::copy(&icon, &dest).is_ok() {
                std::env::set_var("KRATE_WINDOW_ICON", &dest);
            }
        }
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
        check_layout: request.check_layout,
        app_args: request.app_args,
        max_http_response_bytes: request.max_http_response_bytes,
        default_http_timeout_millis: match request.http_timeout_millis {
            0 => None,
            millis => Some(millis),
        },
        sandbox_root: request.sandbox_root.clone(),
        // An explicit --assets wins; otherwise a packed bundle's own assets
        // are used. Without the flag, an app run from loose source could
        // never see its images at all (K-093).
        bundle_assets_root: request.assets_root.clone().or_else(|| {
            bundle
                .as_ref()
                .and_then(|bundle| bundle.assets_path().map(Path::to_path_buf))
        }),
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
        app_shared: manifest.map(|manifest| (app_shared_path(&manifest.app.id), shared_hub_url())),
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

/// The installed `<App>.app` for this bundle, if `krate install` made one.
///
/// Matched on the app's declared id rather than its name, so an unrelated app
/// that happens to share a name can never be launched in its place. The
/// installed copy's own payload is read to confirm the id, which also means a
/// stale wrapper left behind by a deleted app is simply not matched.
#[cfg(target_os = "macos")]
fn installed_app_for(bundle_path: &Path) -> Option<PathBuf> {
    let manifest = krate_bundle::open(bundle_path).ok()?;
    let name = manifest.manifest().app.name.trim().to_string();
    let id = manifest.manifest().app.id.clone();
    if name.is_empty() {
        return None;
    }

    let roots = [
        home_dir().map(|home| home.join("Applications")),
        Some(PathBuf::from("/Applications")),
    ];
    for root in roots.into_iter().flatten() {
        let candidate = root.join(format!("{name}.app"));
        let payload = candidate.join("Contents/Resources/app.krate");
        if !payload.is_file() {
            continue;
        }
        let matches = krate_bundle::open(&payload)
            .map(|installed| installed.manifest().app.id == id)
            .unwrap_or(false);
        if matches {
            return Some(candidate);
        }
    }
    None
}

/// Wrap and open: the launch path behind double-click and the studio.
/// Resolve a launch target that may be a Krate Cloud URL: download the
/// bundle to a cache file and launch that. Downloading grants nothing --
/// the fetched app meets the same permission wall as any local file.
fn launch_target(bundle: &Path) -> Result<PathBuf> {
    let raw = bundle.to_string_lossy();
    if !krate_bundle::is_url(&raw) {
        return Ok(bundle.to_path_buf());
    }
    let dir = home_dir()
        .map(|home| home.join(".krate/cloud"))
        .context("no home directory")?;
    fs::create_dir_all(&dir)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&raw.as_ref(), &mut hasher);
    let dest = dir.join(format!("{:016x}.krate", std::hash::Hasher::finish(&hasher)));
    let response = ureq::get(&raw)
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .with_context(|| format!("could not download {raw}"))?;
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        // Bounded: a .krate is kilobytes; refuse anything clearly not one.
        response
            .into_reader()
            .take(64 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .context("reading the download")?;
    }
    fs::write(&dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}

#[cfg(target_os = "macos")]
fn launch_app(bundle_path: &Path) -> Result<u8> {
    let bundle_path = &launch_target(bundle_path)?;
    let launchers = home_dir()
        .map(|home| home.join(".krate/launchers"))
        .context("no home directory")?;

    // Reuse the wrapper when the app has not changed, and never rebuild it
    // in place. The first version deleted and recreated the wrapper on every
    // launch; when two opens raced (Finder can deliver a document twice),
    // the second tore the bundle out from under the first while it was still
    // booting, and it died with SIGABRT and nothing on screen -- read
    // straight out of the crash log. Now: identical payload, reuse; changed
    // payload, build beside and swap.
    let name = krate_bundle::open(bundle_path)?
        .manifest()
        .app
        .name
        .trim()
        .to_string();
    let app_dir = launchers.join(format!("{name}.app"));
    let payload = app_dir.join("Contents/Resources/app.krate");
    let same_app = fs::read(&payload)
        .ok()
        .zip(fs::read(bundle_path).ok())
        .map(|(a, b)| a == b)
        .unwrap_or(false);
    // The wrapper embeds a COPY of the engine, so "the app has not changed"
    // is only half the reuse question -- the other half is "has Krate?". A
    // wrapper built before an engine upgrade kept serving the old runtime
    // forever: a scroll-performance fix shipped, `krate run` flew, and the
    // same .krate double-clicked from Finder still lagged (the rc18 trap,
    // one layer deeper). The marker written at wrap time answers it.
    let marker = app_dir.join("Contents/Resources/engine.fingerprint");
    let same_engine = fs::read_to_string(&marker)
        .map(|held| held == engine_fingerprint())
        .unwrap_or(false);
    if !same_app || !same_engine {
        let stage = launchers.join(format!(".stage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage)?;
        let built = install_bundle(bundle_path, &stage)?;
        let old_dir = launchers.join(format!(".old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&old_dir);
        if app_dir.exists() {
            let _ = fs::rename(&app_dir, &old_dir);
        }
        fs::rename(&built, &app_dir)?;
        let _ = fs::remove_dir_all(&old_dir);
        let _ = fs::remove_dir_all(&stage);
    }
    let status = std::process::Command::new("/usr/bin/open")
        .arg("-n")
        .arg(&app_dir)
        .status()
        .context("opening the app")?;
    if !status.success() {
        bail!("macOS refused to open {}", app_dir.display());
    }
    Ok(0)
}

#[cfg(not(target_os = "macos"))]
fn launch_app(bundle_path: &Path) -> Result<u8> {
    let bundle_path = &launch_target(bundle_path)?;
    // Elsewhere a plain windowed run activates fine; the wrapper is a macOS
    // need. Same binary, ordinary run -- but detached, with no console:
    // the engine is a console-subsystem binary on Windows, and spawning it
    // plainly hangs a black terminal behind every app someone opens.
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    // `--consent`, exactly as the macOS path passes `consent: true`. Without
    // it a double-clicked app that declares ANY ask-level capability is
    // refused outright -- "This app needs permission it was not given, so it
    // did not run" -- with no window and no way to say yes. Measured on
    // Windows with a grocery list, whose only ask is `store.kv`: it died
    // instantly, and ran the moment the flag was added (K-157).
    cmd.arg("run").arg("--consent").arg(bundle_path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.spawn()?;
    Ok(0)
}

/// Give an app a real identity on this machine: its own `.app`, its own name
/// in the dock, its own icon.
///
/// Why a wrapper and not a run-time rename: macOS takes the dock name of an
/// unbundled process from the executable's **file name**, not from anything
/// settable while running. Writing `CFBundleName` into the live info
/// dictionary was tried and measured -- the write succeeds and reads back, and
/// the dock still said "krate". A per-app bundle with its own
/// `CFBundleExecutable` is the mechanism that actually works, so that is what
/// this builds. The icon is separate: that one *is* settable at run time, and
/// the runtime already does it from the bundle's `assets/icon.png`.
///
/// The `.krate` is copied inside the wrapper, so the installed app keeps
/// working after the original file is moved, renamed, or thrown away.
#[cfg(target_os = "macos")]
fn install_app(bundle_path: &Path, prefix: Option<&Path>, dry_run: bool) -> Result<u8> {
    let prefix = prefix.map(Path::to_path_buf).unwrap_or_else(|| {
        home_dir()
            .map(|home| home.join("Applications"))
            .unwrap_or_else(|| PathBuf::from("/Applications"))
    });
    if dry_run {
        let opened = krate_bundle::open(bundle_path)?;
        println!(
            "{}",
            prefix
                .join(format!("{}.app", opened.manifest().app.name.trim()))
                .display()
        );
        return Ok(0);
    }
    let app_dir = install_bundle(bundle_path, &prefix)?;
    println!("Installed to {}", app_dir.display());
    println!("It has its own name and icon now; open it from Launchpad like any other app.");
    Ok(0)
}

#[cfg(target_os = "macos")]
/// This engine build's identity, for deciding whether a wrapper's embedded
/// engine copy is current. Version and git sha alone miss dirty rebuilds of
/// the same commit, so the binary's length and mtime join them -- cheap to
/// read, and any rebuild changes at least one of the four.
fn engine_fingerprint() -> String {
    let (len, mtime) = std::env::current_exe()
        .and_then(fs::metadata)
        .map(|m| {
            (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    format!(
        "{}-{}-{len}-{mtime}",
        env!("CARGO_PKG_VERSION"),
        env!("KRATE_GIT_SHA"),
    )
}

/// The wrapper builder `install` and `launch` share: returns the .app it
/// made, prints nothing.
#[cfg(target_os = "macos")]
fn install_bundle(bundle_path: &Path, prefix: &Path) -> Result<PathBuf> {
    let opened = krate_bundle::open(bundle_path)
        .with_context(|| format!("reading {}", bundle_path.display()))?;
    let manifest = opened.manifest();
    let name = manifest.app.name.trim();
    if name.is_empty() {
        bail!("this app has no name in its manifest, so it cannot be installed");
    }

    let app_dir = prefix.join(format!("{name}.app"));

    let macos_dir = app_dir.join("Contents/MacOS");
    let resources = app_dir.join("Contents/Resources");
    // A fresh wrapper each time: leftovers from an older version of the same
    // app would otherwise sit beside the new one inside the bundle.
    if app_dir.exists() {
        fs::remove_dir_all(&app_dir).with_context(|| format!("replacing {}", app_dir.display()))?;
    }
    fs::create_dir_all(&macos_dir).with_context(|| format!("creating {}", macos_dir.display()))?;
    fs::create_dir_all(&resources)?;

    // The app carries its own copy, so it survives the original being moved.
    let payload = resources.join("app.krate");
    fs::copy(bundle_path, &payload)
        .with_context(|| format!("copying the app into {}", payload.display()))?;

    // The engine itself, under the app's name -- not a shell script that runs
    // it. This is the part that makes the identity real, and it was measured
    // both ways: a `#!/bin/sh ... exec krate run ...` shim leaves the DOCK
    // NAME as "krate", because `exec` replaces the shim and macOS reads the
    // name from the executable that is actually running. Copying the engine to
    // `Contents/MacOS/<App Name>` makes the running executable's own name the
    // app's name, and the dock says "Cup Cook".
    //
    // A hard link keeps this from costing 20 MB per installed app; it falls
    // back to a copy across filesystems, where linking cannot work.
    let launcher = macos_dir.join(name);
    let engine = current_engine_path()?;
    if fs::hard_link(&engine, &launcher).is_err() {
        fs::copy(&engine, &launcher)
            .with_context(|| format!("putting the Krate engine in {}", launcher.display()))?;
    }
    // Which engine build this wrapper carries; launch_app compares it
    // against the running engine and rebuilds the wrapper on mismatch.
    fs::write(resources.join("engine.fingerprint"), engine_fingerprint())?;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&launcher)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&launcher, perms)?;

    // The app's own artwork when it ships one, so Finder and the dock show the
    // app rather than the Krate mark.
    //
    // When it ships none, fall back to Krate's own icon rather than leaving
    // CFBundleIconFile unset: an app with no icon gets the blank sheet of
    // paper, which reads as broken. The Krate mark says "this is a Krate app",
    // which is true and looks deliberate.
    let icon_name = opened
        .assets_path()
        .map(|assets| assets.join("icon.png"))
        .filter(|path| path.is_file())
        .and_then(|icon| write_icns(&icon, &resources.join("AppIcon.icns")).ok())
        .map(|()| "AppIcon")
        .or_else(|| {
            krate_icon_source()
                .and_then(|source| fs::copy(source, resources.join("AppIcon.icns")).ok())
                .map(|_| "AppIcon")
        });

    let icon_entry = icon_name
        .map(|icon| format!("    <key>CFBundleIconFile</key>\n    <string>{icon}</string>\n"))
        .unwrap_or_default();
    // Identifier derived from the app's own id, so two different apps are two
    // different apps to Launch Services rather than one overwriting the other.
    let ident = format!("dev.krate.app.{}", manifest.app.id.replace('/', "."));
    fs::write(
        app_dir.join("Contents/Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>{name}</string>
    <key>CFBundleDisplayName</key>
    <string>{name}</string>
    <key>CFBundleExecutable</key>
    <string>{name}</string>
    <key>CFBundleIdentifier</key>
    <string>{ident}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSCameraUsageDescription</key>
    <string>{name} asked for the camera, and you allowed it in Krate's permission window.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>{name} asked for the microphone, and you allowed it in Krate's permission window.</string>
{icon_entry}</dict>
</plist>
"#,
            name = xml_escape(name),
            ident = xml_escape(&ident),
            version = xml_escape(&manifest.app.version),
        ),
    )?;

    // Tell Launch Services, so it appears in Launchpad and Spotlight now
    // rather than whenever the system next rescans.
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/\
                      Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(lsregister)
        .args(["-f".as_ref(), app_dir.as_os_str()])
        .status();

    Ok(app_dir)
}

/// Linux install: a payload under ~/.local/share/krate, a freedesktop
/// launcher entry, and the app's own icon -- so an installed app shows up in
/// the menu with its own name and picture, exactly like the macOS wrapper.
#[cfg(target_os = "linux")]
fn install_app(bundle_path: &Path, prefix: Option<&Path>, dry_run: bool) -> Result<u8> {
    let opened = krate_bundle::open(bundle_path)
        .with_context(|| format!("reading {}", bundle_path.display()))?;
    let manifest = opened.manifest();
    let name = manifest.app.name.trim().to_string();
    if name.is_empty() {
        bail!("this app has no name in its manifest, so it cannot be installed");
    }
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let data = prefix.map(Path::to_path_buf).unwrap_or_else(|| {
        home_dir()
            .map(|h| h.join(".local/share"))
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let app_dir = data.join("krate/apps").join(&slug);
    let desktop_path = data
        .join("applications")
        .join(format!("krate-{slug}.desktop"));
    if dry_run {
        println!("{}", desktop_path.display());
        return Ok(0);
    }
    fs::create_dir_all(&app_dir)?;
    let payload = app_dir.join("app.krate");
    fs::copy(bundle_path, &payload)?;

    // The app's own icon when it ships one, the Krate mark otherwise; the
    // desktop entry takes an absolute path, so no theme install is needed.
    let icon_path = app_dir.join("icon.png");
    let mut have_icon = false;
    if let Some(icon) = opened
        .assets_path()
        .map(|assets| assets.join("icon.png"))
        .filter(|path| path.is_file())
    {
        have_icon = fs::copy(&icon, &icon_path).is_ok();
    }
    if !have_icon {
        if let Some(mark) = krate_icon_png_source() {
            have_icon = fs::copy(mark, &icon_path).is_ok();
        }
    }

    let engine = current_engine_path()?;
    fs::create_dir_all(desktop_path.parent().expect("applications dir"))?;
    fs::write(
        &desktop_path,
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nComment=Made with Krate\nExec=\"{engine}\" launch \"{payload}\"\nTerminal=false\nCategories=Utility;\n{icon}",
            engine = engine.display(),
            payload = payload.display(),
            icon = if have_icon {
                format!("Icon={}\n", icon_path.display())
            } else {
                String::new()
            },
        ),
    )?;
    // Refresh the menu database where the tool exists; missing it only means
    // the entry appears after the next session.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(data.join("applications"))
        .status();

    println!("Installed {name}: it is in your app menu with its own name and icon.");
    Ok(0)
}

/// The Krate mark as a PNG, for Linux launcher entries.
#[cfg(target_os = "linux")]
fn krate_icon_png_source() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    ["../share/krate/krate-app-icon.png", "krate-app-icon.png"]
        .into_iter()
        .map(|rel| dir.join(rel))
        .find(|p| p.is_file())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_app(_bundle_path: &Path, _prefix: Option<&Path>, _dry_run: bool) -> Result<u8> {
    bail!("on Windows the installer sets up file associations; run the .krate directly")
}

/// Krate's own `.icns`, for apps that ship no icon of their own.
///
/// Found relative to the running engine, so it works from an installed
/// Krate.app and from a repo build. `None` when neither is present, and the
/// installed app then simply has no icon rather than failing to install.
#[cfg(target_os = "macos")]
fn krate_icon_source() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        // Inside Krate.app: Contents/MacOS/krate-cli -> Contents/Resources.
        dir.join("../Resources/Krate.icns"),
        // A repo build, where make-macos-app.sh leaves the generated icons.
        dir.join("../../dist/icon/Krate.icns"),
        dir.join("dist/icon/Krate.icns"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// Convert a PNG into an `.icns`, using the system tool that already ships.
#[cfg(target_os = "macos")]
fn write_icns(png: &Path, out: &Path) -> Result<()> {
    let staging = out.with_extension("iconset");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    for size in [16_u32, 32, 128, 256, 512] {
        for (suffix, pixels) in [(String::new(), size), ("@2x".to_string(), size * 2)] {
            let target = staging.join(format!("icon_{size}x{size}{suffix}.png"));
            let status = std::process::Command::new("/usr/bin/sips")
                .args(["-z", &pixels.to_string(), &pixels.to_string()])
                .arg(png)
                .arg("--out")
                .arg(&target)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            if !status.success() {
                bail!("could not resize the app icon");
            }
        }
    }
    let status = std::process::Command::new("/usr/bin/iconutil")
        .args(["-c", "icns"])
        .arg(&staging)
        .arg("-o")
        .arg(out)
        .status()?;
    let _ = fs::remove_dir_all(&staging);
    if !status.success() {
        bail!("could not build the app icon");
    }
    Ok(())
}

/// The engine binary to run installed apps with.
///
/// The running executable, resolved through symlinks: an app installed by a
/// given Krate keeps running against that same Krate rather than whatever
/// later lands on PATH.
fn current_engine_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the Krate engine")?;
    Ok(fs::canonicalize(&exe).unwrap_or(exe))
}

/// Escape text for the generated Info.plist.
#[cfg(target_os = "macos")]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Open Krate Studio, the app-making front end. Returns whether it launched.
///
/// Looked up by bundle identifier rather than by path, so a studio installed
/// anywhere is found; `-b` fails cleanly when it is not installed at all,
/// which is the signal the caller falls back on.
#[cfg(target_os = "macos")]
fn open_studio() -> bool {
    std::process::Command::new("/usr/bin/open")
        .args(["-b", "dev.krate.studio"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The Krate.app entry point (P3-OPEN-03): receive the document Finder asked
/// us to open, then run it through the ordinary consent + native-window flow.
/// The sandbox root is the folder the `.krate` sits in, so an app that writes
/// `./notes/**` keeps its data in a folder next to the document — visible,
/// understandable, and identical to running it from a terminal in that folder.
#[cfg(target_os = "macos")]
/// Build (or refresh) the per-app launcher bundle and return its executable.
///
/// The dock shows an unbundled process under its executable's file name, which
/// is why the engine re-execs itself through a link named after the app. Doing
/// that with a bare file cost the process its Info.plist, and with it every
/// permission macOS gates on a usage description -- the camera prompt never
/// appeared and capture silently returned nothing (K-145).
///
/// So the link lives inside a minimal `.app` that declares those usages. The
/// executable is still a hard link to the engine, so this costs no disk and
/// stays correct when the engine is replaced by an upgrade.
#[cfg(target_os = "macos")]
fn macos_launcher_bundle(dir: &Path, name: &str, engine: &Path) -> std::io::Result<PathBuf> {
    let app = dir.join(format!("{name}.app"));
    let macos = app.join("Contents/MacOS");
    let exe = macos.join(name);
    // Rebuilt every launch: cheap, and it means an upgraded engine is picked
    // up rather than a stale hard link to a deleted binary being re-run.
    let _ = fs::remove_dir_all(&app);
    fs::create_dir_all(&macos)?;
    if fs::hard_link(engine, &exe).is_err() {
        fs::copy(engine, &exe)?;
    }

    // CFBundleIdentifier is per-app on purpose: macOS records camera and
    // microphone decisions against it, so each app the person opens is asked
    // about separately and one app's grant is not another's.
    let id: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \u{20}   <key>CFBundleName</key><string>{name}</string>\n\
         \u{20}   <key>CFBundleDisplayName</key><string>{name}</string>\n\
         \u{20}   <key>CFBundleIdentifier</key><string>tech.krate.app.{id}</string>\n\
         \u{20}   <key>CFBundleExecutable</key><string>{name}</string>\n\
         \u{20}   <key>CFBundlePackageType</key><string>APPL</string>\n\
         \u{20}   <key>CFBundleVersion</key><string>1.0</string>\n\
         \u{20}   <key>NSHighResolutionCapable</key><true/>\n\
         \u{20}   <key>NSCameraUsageDescription</key>\n\
         \u{20}   <string>{name} asked for the camera, and you allowed it in Krate's \
         permission window.</string>\n\
         \u{20}   <key>NSMicrophoneUsageDescription</key>\n\
         \u{20}   <string>{name} asked for the microphone, and you allowed it in Krate's \
         permission window.</string>\n\
         </dict>\n</plist>\n"
    );
    fs::write(app.join("Contents/Info.plist"), plist)?;

    // Ad-hoc sign so the bundle has a stable identity for macOS to record its
    // permission decisions against. Without a signature the decision cannot be
    // remembered and the person is asked again on every launch.
    let _ = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-"])
        .arg(&app)
        .output();
    Ok(exe)
}

// No non-macOS variant of `macos_launcher_bundle`: its only caller is inside
// `open_app`, which is macOS-only, so one on other systems is dead code that
// CI correctly refuses. The dock-name re-exec this supports is a macOS
// concern -- Windows and Linux name a process from its own executable without
// the hop.

/// Gated to macOS, and it has to stay that way: the body uses `std::os::unix`,
/// `Command::exec`, and `krate_adapter_macos`, none of which exist on Windows.
/// The dispatch arm for `Command::OpenApp` carries the same gate.
///
/// This gate was silently dropped after v0.1.51 and `main` stopped compiling
/// for Windows entirely -- nine errors, none of them caught, because nothing
/// builds for Windows outside a release (K-150).
#[cfg(target_os = "macos")]
fn open_app(direct: Option<PathBuf>) -> Result<u8> {
    // A document that arrives while this instance is already running an app
    // (double-click in Finder mid-session) gets its own process, so every
    // opened .krate behaves like its own application.
    let late_open = Box::new(|path: PathBuf| {
        spawn_open_run(&path);
    });
    // A file named on the command line skips the Launch Services wait: the
    // caller already knows what to open. Everything after this point -- the
    // native consent window, the sandbox root, the window itself -- is
    // identical to the Finder path, which is the point.
    // Wear the app's own name in the dock. macOS names an unbundled process
    // after its executable file -- measured; no runtime call changes it -- so
    // the engine re-execs itself through a hard link named after the app.
    // One hop only, guarded by KRATE_AS, and any failure falls through to
    // running under the engine's name rather than not running.
    if let Some(file) = &direct {
        if std::env::var_os("KRATE_AS").is_none() {
            if let Ok(bundle) = krate_bundle::open(file) {
                let name = bundle.manifest().app.name.trim().to_string();
                let exe = std::env::current_exe().ok();
                if !name.is_empty()
                    && exe
                        .as_ref()
                        .and_then(|e| e.file_name())
                        .map(|f| f.to_string_lossy() != *name)
                        .unwrap_or(false)
                {
                    let dir = home_dir()
                        .map(|h| h.join(".krate/launchers"))
                        .unwrap_or_default();
                    let _ = fs::create_dir_all(&dir);
                    let engine = exe.unwrap();
                    // A bundle, not a bare executable. The re-exec is what
                    // puts the app's own name in the dock, but a bare binary
                    // outside any bundle has no Info.plist -- and macOS
                    // refuses camera and microphone access to a process that
                    // cannot say why it wants them, silently, with no prompt
                    // ever shown. Measured: the permission request returned
                    // instantly, the status stayed not-determined forever, and
                    // no dialog appeared on any run (K-145). Wrapping the same
                    // hard link in a minimal .app keeps the dock name and
                    // gives the process the declarations it needs.
                    let link = match macos_launcher_bundle(&dir, &name, &engine) {
                        Ok(link) => link,
                        Err(_) => dir.join(&name),
                    };
                    if link.exists() {
                        use std::os::unix::process::CommandExt;
                        let err = std::process::Command::new(&link)
                            .arg("open-app")
                            .arg(file)
                            .env("KRATE_AS", "1")
                            .exec();
                        eprintln!("re-exec under the app's name failed: {err}");
                    }
                }
            }
        }
    }

    let opened = match direct {
        Some(file) => vec![file],
        None => krate_adapter_macos::wait_for_opened_documents(late_open)
            .map_err(|error| anyhow::anyhow!("waiting for the opened document failed: {error}"))?,
    };
    // AppKit also feeds process arguments through application:openFiles:, so
    // our own subcommand name can arrive as a "document". Only paths that
    // actually exist on disk are documents.
    let opened: Vec<PathBuf> = opened.into_iter().filter(|path| path.exists()).collect();
    // Launched with no document (Krate.app opened directly): that gesture means
    // "I want Krate", not "I want to browse for a file". Hand off to Krate
    // Studio, where a person can make an app rather than only run one. The
    // picker stays as the fallback for machines without the studio installed,
    // because dying silently is not an option -- there is no terminal to print
    // to.
    let picked;
    let target = match opened.first() {
        Some(target) => target,
        None => {
            if open_studio() {
                return Ok(0);
            }
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
    // If this app has been installed, run it as itself.
    //
    // Krate.app's executable is named "krate-cli", so a document opened
    // through it shows "krate-cli" in the dock -- macOS names a process after
    // the executable that is running, which is the same reason `krate install`
    // exists. Handing off to the installed copy means a double-clicked
    // calculator presents as the calculator, exactly as launching it from
    // Launchpad does. Not installed: run it here, under the Krate name, which
    // is still better than not opening at all.
    #[cfg(target_os = "macos")]
    if let Some(app) = installed_app_for(target) {
        let handed_off = ProcessCommand::new("/usr/bin/open")
            .arg("-n")
            .arg(&app)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if handed_off {
            for extra in opened.iter().skip(1) {
                spawn_open_run(extra);
            }
            return Ok(0);
        }
    }

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
        check_layout: false,
        assets_root: None,
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

    // On macOS, go back through the .app bundle rather than spawning the
    // binary directly (K-110).
    //
    // A bare `spawn` produces a process macOS does not consider a GUI
    // application: it has no LaunchServices registration and no activation, so
    // AppKit will not put its window on screen. The runtime creates the window
    // and even prints `opened window "Mdview"`, and the person sees nothing at
    // all. That is every app after the first -- open one app, leave it
    // running, double-click a second, and the second silently never appears.
    //
    // `open -n -a Krate.app <file>` launches a new instance through
    // LaunchServices, which registers it properly and shows its window.
    // Verified both ways on this machine: direct spawn gives a running process
    // with no window; through the bundle the window appears.
    #[cfg(target_os = "macos")]
    if let Some(bundle) = enclosing_app_bundle(&exe) {
        let spawned = ProcessCommand::new("/usr/bin/open")
            .arg("-n")
            .arg("-a")
            .arg(&bundle)
            .arg(path)
            .spawn();
        if spawned.is_ok() {
            return;
        }
        // Fall through to the direct spawn below. Better a window that may
        // not show than no attempt at all.
    }

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

/// The `.app` bundle this executable lives inside, if any.
///
/// A bundled binary sits at `Krate.app/Contents/MacOS/krate-cli`, so the
/// bundle is three levels up. Returns `None` for a plain CLI install, where
/// there is no bundle to launch through.
#[cfg(target_os = "macos")]
fn enclosing_app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos_dir.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    if bundle.extension()? == "app" {
        Some(bundle.to_path_buf())
    } else {
        None
    }
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
        #[cfg(target_os = "windows")]
        {
            // No terminal to answer -- the double-clicked case. Windows was
            // printing this prompt into a console nobody could see (or, with
            // the console hidden, hanging on a read that could never finish).
            // A native dialog is the honest equivalent of [A]ll / [N]one; the
            // per-capability numbers stay a terminal feature.
            let mut body = format!("{} wants to:\n\n", manifest.app.name);
            for cap in &prompt_caps {
                body.push_str(&format!("  \u{2022} {} ({cap})\n", human_label(cap)));
            }
            body.push_str("\nAllow this? Krate enforces exactly this list.");
            let allow = rfd::MessageDialog::new()
                .set_title(&format!("Open {}?", manifest.app.name))
                .set_description(&body)
                .set_buttons(rfd::MessageButtons::OkCancelCustom(
                    "Allow and open".to_string(),
                    "Cancel".to_string(),
                ))
                .show();
            if matches!(allow, rfd::MessageDialogResult::Custom(label) if label == "Allow and open")
            {
                let grants = policy.grants().iter().cloned().chain(prompt_caps);
                return Ok(SessionPolicy::from_grants(grants));
            }
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

    // rfd next: a native dialog with no external tool. On desktops where its
    // backend cannot connect this returns Cancel-ish immediately, which the
    // affirmative-only check below treats as "not answered", falling through
    // to the message -- never a silent grant.
    {
        let mut body = format!("{} wants permission to:\n\n", manifest.app.name);
        for cap in consent_caps {
            body.push_str(&format!("  \u{2022} {}\n", human_label(cap)));
        }
        body.push_str("\nKrate enforces exactly this list.");
        let answer = rfd::MessageDialog::new()
            .set_title(format!("Open {}?", manifest.app.name))
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                "Allow and open".to_string(),
                "Cancel".to_string(),
            ))
            .show();
        match answer {
            rfd::MessageDialogResult::Custom(ref label) if label == "Allow and open" => {
                return Ok(Some(consent_caps.to_vec()));
            }
            // The person saw the dialog and pressed Cancel: a real refusal,
            // not a missing dialog -- do not fall through to the "no dialog"
            // message.
            rfd::MessageDialogResult::Custom(_) => return Ok(None),
            // Anything else is a backend that could not really ask; keep
            // falling so the honest message prints.
            _ => {}
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
    println!("krate   {}", krate_version());
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
    // Which silicon draws app windows. "The game lags" starts here: a
    // missing or software adapter means every frame was rastered on the CPU.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    match krate_presenter_gpu::adapter_summary() {
        Some(gpu) => println!("graphics        {gpu}"),
        None => println!("graphics        no GPU adapter -- app windows draw on the CPU"),
    }
    #[cfg(target_os = "macos")]
    println!("graphics        native macOS pipeline");
    #[cfg(target_os = "windows")]
    print_windows_native_build_status();
    Ok(0)
}

/// The two things a Windows machine needs to build Krate WITH SPEECH.
///
/// `whisper-rs-sys` compiles whisper.cpp through bindgen and cmake, and neither
/// tool is part of the Visual Studio Build Tools VCTools workload -- so a clean
/// Windows 11 with rustup and the Build Tools looked complete, compiled most of
/// the workspace, and then died first on a missing `libclang.dll` and then on a
/// missing `cmake` (K-149).
///
/// Since speech became opt-in these are no longer needed for an ordinary build:
/// `cargo build` works with rustup alone. They are needed for
/// `--features speech`, which is what released binaries ship, so anyone
/// building a release still needs both. Stated as information, never as a
/// fault -- running apps and `krate create` need neither.
#[cfg(target_os = "windows")]
fn print_windows_native_build_status() {
    println!();
    println!("Building Krate with speech (`--features speech`; a plain build needs neither)");

    let clang = std::env::var_os("LIBCLANG_PATH")
        .map(std::path::PathBuf::from)
        .filter(|dir| dir.join("libclang.dll").exists())
        .or_else(|| {
            let default = std::path::PathBuf::from(r"C:\Program Files\LLVM\bin");
            default.join("libclang.dll").exists().then_some(default)
        });
    match clang {
        Some(dir) => println!("libclang        found ({})", dir.display()),
        None => {
            println!("libclang        missing -- `whisper-rs-sys` cannot run bindgen without it");
            println!("                install LLVM, then set LIBCLANG_PATH to its bin folder:");
            println!("                  winget install LLVM.LLVM");
        }
    }

    // PATH first, then the default install location. The MSI installer does
    // not add itself to PATH unless asked, so a machine with cmake genuinely
    // installed was reported as missing -- doctor telling somebody to install
    // what they already have is how a check loses their trust.
    let cmake = agent_provider::which_on_path("cmake").or_else(|| {
        let default = std::path::PathBuf::from(r"C:\Program Files\CMake\bin\cmake.exe");
        default.exists().then_some(default)
    });
    match cmake {
        Some(path) => println!("cmake           found ({})", path.display()),
        None => {
            println!("cmake           missing -- `whisper-rs-sys` builds whisper.cpp with it");
            println!("                  winget install Kitware.CMake");
        }
    }
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
            // Existing is not the same as fitting. A reviewer on Rust 1.85
            // watched doctor print paths, say nothing about versions, and
            // suggest commands that did not include `rustup update` -- then
            // hit the build wall doctor exists to predict (K-080). Say the
            // version next to the path, and say the fix when it is short.
            print_rust_version_fit(&rustup_cargo);
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

/// The minimum Rust the workspace builds with, from Cargo.toml's
/// rust-version. Compiled in so doctor cannot drift from the real bound.
const MIN_RUST: (u32, u32) = (1, 91);

/// Print the toolchain's version beside its path, and `rustup update` when it
/// is older than the workspace needs.
fn print_rust_version_fit(cargo: &Path) {
    let reported = ProcessCommand::new(cargo)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let Some(reported) = reported else {
        println!("  version       could not run cargo --version");
        return;
    };
    println!("  version       {reported}");
    match rust_version_fits(&reported, MIN_RUST) {
        Some(true) => {}
        Some(false) => println!(
            "  warning: Krate needs Rust {}.{} or newer to build apps. Update with: rustup update",
            MIN_RUST.0, MIN_RUST.1
        ),
        // Unparseable is reported, not guessed at.
        None => println!("  note: could not read a version number out of that line"),
    }
}

/// Whether a `cargo --version` line satisfies a minimum. None when no version
/// number can be found in the line at all.
fn rust_version_fits(version_line: &str, min: (u32, u32)) -> Option<bool> {
    let number = version_line.split_whitespace().nth(1)?;
    let mut parts = number.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor) >= min)
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
/// The manifest may not ask beyond the code. Returns the first problem
/// found, or None when the asks and the imports line up.
fn manifest_overreach(manifest: &krate_manifest::Manifest, imports: &[String]) -> Option<String> {
    let has_import = |needle: &str| imports.iter().any(|i| i.contains(needle));

    for request in &manifest.capabilities {
        let cap = request.cap.to_string();
        let (family, scope) = match cap.split_once(':') {
            Some((f, s)) => (f, Some(s)),
            None => (cap.as_str(), None),
        };

        // Rule 1: unscoped fs globs.
        if family.starts_with("fs.") {
            let bare = scope
                .map(|s| s.trim_start_matches("./").trim_start_matches('/'))
                .unwrap_or("");
            if bare == "**" || bare == "*" || bare.is_empty() {
                return Some(format!(
                    "the manifest asks for {cap}, which reads as everything. The sandbox contains it, but a person cannot tell -- and an app wide open on paper fails the permission wall even when the runtime holds"
                ));
            }
        }

        // Rule 2: a family whose interface the component never imports.
        let needed: Option<&str> = match family {
            f if f.starts_with("fs.") => Some(":fs/"),
            "net.connect" => Some(":net/"),
            "store.kv" => Some(":store/kv"),
            "store.sql" => Some(":store/sql"),
            "store.secret" => Some(":store/secret"),
            "store.shared" => Some(":store/shared"),
            "random.bytes" => Some(":random/"),
            "audio.playback" => Some(":audio/playback"),
            "audio.capture" => Some(":audio/capture"),
            "camera.capture" => Some(":camera/capture"),
            "ui.dialog" => Some(":ui/dialog"),
            "ui.clipboard" => Some(":ui/clipboard"),
            // System and always-on families carry no signal here.
            _ => None,
        };
        if let Some(needle) = needed {
            if !imports.is_empty() && !has_import(needle) {
                return Some(format!(
                    "the manifest asks for {cap}, but the component never imports the {needle} interface -- it is asking for something the code cannot even attempt"
                ));
            }
        }

        // Rule 3: a capability the app cannot work without, marked optional.
        //
        // The person is only asked about REQUIRED capabilities. An optional one
        // is never mentioned and never granted, so an app whose whole purpose
        // sits behind it opens without it and without a question -- and looks
        // broken, because from the person's side it is. A generated webcam
        // viewer marked `camera.capture` optional and opened to a permanently
        // empty viewfinder with no prompt and no explanation (K-146).
        //
        // Deliberately narrow: only capabilities that ARE an app's reason for
        // existing whenever they appear at all. A file dialog or the clipboard
        // is genuinely a nice-to-have in most apps; a camera is not something
        // an app reaches for in passing.
        let defining = matches!(family, "camera.capture" | "audio.capture");
        if defining && !request.required {
            return Some(format!(
                "the manifest asks for {cap} but marks it optional. The person is only asked about required capabilities, so this one is never granted and never even mentioned -- the app opens without it, with nothing on screen to say why. If the app still makes sense without {cap}, do not declare it; if it does not, mark it required"
            ));
        }
    }
    None
}

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
    // The pace note. Measured across ten real builds: agents ran the FULL
    // ~20-second check 7 to 33 times each and --no-run far less, despite the
    // prompt teaching the split -- 3 to 10 minutes per build spent
    // re-proving what had not changed. Prose does not change behavior; the
    // tool saying it at the moment of the habit does (K-098). Never a
    // refusal -- an oracle that will not answer is worse than a slow one --
    // just the arithmetic, printed where the agent reads its results.
    let pace_note = if !no_run {
        let stamp = dir.join(".last-full-check");
        let since = fs::metadata(&stamp)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok());
        let _ = fs::write(&stamp, "");
        match since {
            Some(gap) if gap.as_secs() < 120 => Some(gap.as_secs()),
            _ => None,
        }
    } else {
        None
    };
    let outcome = run_check_app(dir, shoot, no_run);
    if let Some(secs) = pace_note {
        if !json {
            println!(
                "  pace: this is your second full check in {secs}s. Iterate with                  `check-app . --no-run` (about 2s) and save the full check for when                  you believe the app is done -- full checks this frequent are the                  single biggest time cost in authoring."
            );
        }
    }
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
    // A round limit on the interactive loop closes the app while somebody is
    // using it. The authoring pack says not to, in strong words, and an AI did
    // it anyway -- so this is checked rather than asked for.
    if let Ok(lib) = fs::read_to_string(dir.join("src/lib.rs")) {
        if let Some(detail) = bounded_interactive_loop(&lib) {
            return Err(CheckFailure {
                stage: CheckStage::Layout,
                detail,
                fix: "Bound the `quick` path, never the interactive one:\n\n                          if quick {\n        // draw one frame, print key:value, exit 0\n                          }\n    // interactive: no round limit at all\n    loop {\n                              match events::wait(None) {\n                                  Some(Event::CloseRequested(_)) => break,\n            _ => {}\n                              }\n    }\n\nA real session ends when the person closes the window,                       and only then."
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
    let imports: Vec<String> = krate_bundle::imports::component_imports(&wasm_bytes)
        .map(|set| set.into_iter().collect())
        .unwrap_or_default();
    passed.push("imports");

    // Stage extension: the manifest must not ask for more than the code can
    // reach. Two rules, both from the review that found a tidier declaring
    // filesystem-wide delete it never called (K-075):
    //
    // 1. An unscoped fs glob (`**`) is refused. The sandbox keeps it
    //    contained, but a manifest line that READS as "everything" fails the
    //    wall's whole purpose -- and the right tool exists now: a folder the
    //    person picks via `ui.dialog:open-folder`, where the pick is the
    //    grant and no fs capability is needed at all.
    // 2. A capability family with no matching interface import is refused:
    //    asking for fs while importing no krate:fs interface means the
    //    manifest asks for something the component cannot even attempt.
    if let Some(problem) = manifest_overreach(&manifest, &imports) {
        // The fix has to match the rule that fired. One combined paragraph
        // told an app whose real mistake was an optional camera to go and
        // rescope its fs globs, which is advice for a different app.
        let fix = if problem.contains("marks it optional") {
            "Set `required = true` on that capability, or remove it from the manifest \
             entirely if the app genuinely works without it. Only required capabilities \
             are put to the person, so an optional one is never granted."
        } else {
            "Scope every fs capability to the narrowest folder the app actually uses (fs.write:./exports/**), or -- for an app that works on a folder the PERSON chooses -- drop the fs capability and use `ui.dialog:open-folder`: the pick is the grant, and files are reached through picked/<token>/... on the ordinary fs calls. Remove any capability whose interface the code never imports."
        };
        return Err(CheckFailure {
            stage: CheckStage::Manifest,
            detail: problem,
            fix: fix.to_string(),
        });
    }

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
    let mut run_args: Vec<String> = vec![
        "run".into(),
        wasm_str.clone(),
        "--manifest".into(),
        manifest_str.clone(),
        "--untrusted".into(),
        "--auto-grant".into(),
        "--headless".into(),
    ];
    // Hand the app its own assets folder. Packed bundles carry assets
    // inside them, but this stage runs loose source, so without this an
    // app that reads an image fails the gate for a reason that has nothing
    // to do with the app -- which is exactly how krate-spriteproof and
    // krate-nova2 failed (K-093).
    let assets_dir = dir.join("assets");
    if assets_dir.is_dir() {
        // Absolute, like the wasm and manifest above: run_self sets the
        // child's cwd, so a relative path would resolve against the wrong
        // place and silently give the app no assets.
        run_args.push("--assets".into());
        run_args.push(
            absolute_from_cwd(&assets_dir)
                .to_string_lossy()
                .into_owned(),
        );
    }
    run_args.push("--".into());
    run_args.push(verify_arg);
    let run_arg_refs: Vec<&str> = run_args.iter().map(String::as_str).collect();
    let exit = run_self(verify_dir.path(), &run_arg_refs).map_err(|error| CheckFailure {
        stage: CheckStage::Run,
        detail: format!("could not run the app: {error:#}"),
        fix: String::new(),
    })?;
    if exit != 0 {
        let hint = match exit {
            4 => " (exit 4 means it exhausted its fuel budget -- either a runaway loop, or honest work that is too expensive per frame: hoist per-pixel math out of inner loops, and draw fewer `quick` frames)",
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
            // Same frame, checked for text drawn over text. Free: the app is
            // already being run and painted here.
            "--check-layout".into(),
        ];
        if is_gui {
            shoot_args.push("--".into());
            shoot_args.push("quick".into());
        }
        // The child's stderr is discarded by run_self, so ask it to write any
        // layout finding where this process can read it back.
        let layout_report = std::env::temp_dir().join(format!(
            "krate-layout-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: single-threaded at this point in check-app.
        unsafe { std::env::set_var("KRATE_LAYOUT_REPORT", &layout_report) };
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
        unsafe { std::env::remove_var("KRATE_LAYOUT_REPORT") };
        // A collision is a note, not a failure. It is a real defect and worth
        // saying out loud, but the app builds, runs and paints -- refusing it
        // here would block work on a judgement call the person can see for
        // themselves in the PNG beside it.
        if let Ok(found) = fs::read_to_string(&layout_report) {
            for line in found.lines() {
                if let Some(rest) = line.strip_prefix("layout: ") {
                    if !rest.starts_with("no text drawn over") {
                        usability_notes.push(rest.to_string());
                    }
                } else if let Some(rest) = line.strip_prefix("layout:   ") {
                    usability_notes.push(format!("  {rest}"));
                }
            }
            let _ = fs::remove_file(&layout_report);
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
        // Extend, never assign: the shoot stage above may already have added
        // layout findings, and this stage runs after it.
        usability_notes.extend(notes);
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
    // Same reason as gnullvm_toolchain_present: a just-installed rustup is in
    // ~/.cargo/bin before it is on this process's PATH.
    // Deliberately the DEFAULT toolchain, not `working_windows_toolchain`.
    //
    // Naming the probed toolchain here looks more precise and made things
    // worse: the probe runs a compile, so this cheap check became expensive
    // and started disagreeing with the fallback in `rustup_toolchain_bin`,
    // which still answers for the default. Two Windows releases shipped
    // broken that way. Ask the same toolchain the fallback will use.
    let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
    let output = ProcessCommand::new(rustup)
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
///
/// One tool at a time, and the missing list is recomputed after every one.
/// The list is a dependency chain -- rustup brings cargo, cargo runs the
/// component build -- so a snapshot taken up front goes stale the moment the
/// first install lands: later commands ran against a PATH from before that
/// install and failed, and the advice was "open a new terminal". Three
/// installs deep, that was three restarts to make one app. Refreshing the
/// process PATH between steps is what a new terminal would have done.
/// Whether an install pass made progress.
///
/// Pulled out of the loop below so the decision can be tested without running
/// winget. The rule that matters: a pass that installed something and left the
/// missing list exactly as long as it found it is stuck, and looping again
/// would repeat the same install forever. That is the shape of the bug that
/// cost three terminal restarts (K-069) -- there the list never shrank because
/// nothing re-read PATH, and each pass "succeeded" while changing nothing.
fn install_made_progress(before: usize, after: usize) -> bool {
    after < before
}

pub(crate) fn install_build_tools() -> Result<()> {
    // Bounded by the full list length: every pass must shrink the list by at
    // least the tool it just installed, and a pass that does not is a failure.
    for _ in 0..=missing_create_tools().len() {
        refresh_process_path();
        let before = missing_create_tools().len();
        let Some(tool) = missing_create_tools().into_iter().next() else {
            return Ok(());
        };
        // Silent on purpose: a progress bar is drawing over this, and rustup
        // and winget both narrate at length. Their output is captured and
        // shown only on failure, where it is the thing worth reading.
        let out = run_install_command_quiet(&tool.install_cmd)
            .with_context(|| format!("install {}", tool.what))?;
        if !out.status.success() {
            let text = String::from_utf8_lossy(&out.stderr);
            let detail = text
                .lines()
                .rfind(|line| !line.trim().is_empty())
                .unwrap_or("no reason given");
            anyhow::bail!("{}: {detail}", tool.what);
        }
        // The install reported success. If the tool is still missing after a
        // fresh PATH read, looping would run the identical command again and
        // again until the bound runs out, then blame the person. Say what is
        // actually true instead.
        refresh_process_path();
        if !install_made_progress(before, missing_create_tools().len()) {
            anyhow::bail!(
                "{} installed without error, but is still not visible to this \
process. Open a new terminal and run `krate` again -- if it is still \
missing there, the install did not land.",
                tool.what
            );
        }
    }
    refresh_process_path();
    let still: Vec<String> = missing_create_tools()
        .into_iter()
        .map(|tool| tool.what.to_string())
        .collect();
    if still.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "installed, but still not usable: {} -- the install reported success \
and the tool is nowhere this process can see",
        still.join(", ")
    );
}

/// Point the gnullvm toolchain at the linker it already ships.
///
/// gnullvm exists so nobody needs Visual Studio, and it carries `rust-lld.exe`
/// to do the linking. But rustc's gnullvm target looks for a linker named
/// `x86_64-w64-mingw32-clang`, which rustup does not install. So a machine with
/// rustup, cargo, cargo-component and the wasm target -- everything -- still
/// fails every build at:
///
///     error: linker `x86_64-w64-mingw32-clang` not found
///
/// and the msvc toolchain beside it fails the same way on `link.exe`. No linker
/// is reachable at all, though a working one is sitting inside the toolchain
/// directory. Naming it is the whole fix: no download, no Build Tools, no
/// mingw. Measured on a real Windows machine that had failed every build for
/// five releases -- with this variable set, both the host build and the
/// wasm32-wasip1 build return 0.
///
/// `rust-lld.exe` specifically, not the `gcc-ld\ld.lld.exe` shim beside it:
/// rustc invokes a gnu-flavored linker as `-flavor gnu`, and the shim rejects
/// that argument ("lld: error: unknown argument: -flavor") while `rust-lld`
/// accepts it. An explicit setting from the environment always wins, so anyone
/// with a real mingw or MSVC setup keeps it.
#[cfg(windows)]
fn point_gnullvm_at_its_own_linker(command: &mut ProcessCommand) {
    const VAR: &str = "CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER";
    if std::env::var_os(VAR).is_some() {
        return;
    }
    let Some(home) = home_dir() else { return };
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64-pc-windows-gnullvm"
    } else {
        "x86_64-pc-windows-gnullvm"
    };
    let lld = home
        .join(".rustup")
        .join("toolchains")
        .join(gnullvm_toolchain_name())
        .join("lib")
        .join("rustlib")
        .join(target)
        .join("bin")
        .join("rust-lld.exe");
    if lld.is_file() {
        let var = if cfg!(target_arch = "aarch64") {
            "CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER"
        } else {
            VAR
        };
        command.env(var, &lld);
    }
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
///
/// Resolved through `resolve_tool` rather than invoked bare. rustup is often
/// installed moments earlier by our own toolchain step, and a process that was
/// already running does not inherit the PATH that install just wrote. Asking
/// PATH alone reported "no linker" on a machine that had one, and the retry
/// happened in the same stale process, so it could never come right -- the
/// only way out was to quit and rerun, which is what people actually did.
#[cfg(windows)]
fn gnullvm_toolchain_present() -> bool {
    let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
    let listed = ProcessCommand::new(rustup)
        .args(["toolchain", "list"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("gnullvm"))
        .unwrap_or(false);
    // Listed is not the same as usable. A real machine had the gnullvm
    // toolchain in `rustup toolchain list` with its self-contained linker
    // directory absent -- a half-finished install -- so every build died at
    // "program not found" linking wit-bindgen-rt's build script, and Krate
    // kept choosing that toolchain because the name was there (K-130).
    listed && gnullvm_can_link()
}

/// Can the gnullvm toolchain link a HOST build script?
///
/// The subtle part, learned from a real machine: gnullvm ships the wasm
/// linkers (rust-lld, wasm-component-ld) but a wasm component build also
/// compiles build scripts FOR THE HOST -- wit-bindgen-rt has one -- and for
/// that gnullvm calls `x86_64-w64-mingw32-clang`, which its installer does
/// NOT provide. On the founder's PC that clang was absent, so every build
/// died at "linker `x86_64-w64-mingw32-clang` not found" while the MSVC
/// toolchain sitting right next to it built the same crate in one second
/// (K-130).
///
/// So the test is the real one: does the mingw clang gnullvm actually
/// invokes exist? Cheap (a PATH lookup and two file checks) and honest --
/// nothing else predicts this failure.
#[cfg(windows)]
fn gnullvm_can_link() -> bool {
    let prefix = if cfg!(target_arch = "aarch64") {
        "aarch64-w64-mingw32-clang"
    } else {
        "x86_64-w64-mingw32-clang"
    };
    if agent_provider::which_on_path(prefix).is_some() {
        return true;
    }
    // rustup can also carry it inside the toolchain; look where it would be.
    if let Some(home) = home_dir() {
        let bin = home
            .join(".rustup")
            .join("toolchains")
            .join(gnullvm_toolchain_name())
            .join("bin");
        if bin.join(format!("{prefix}.exe")).exists() {
            return true;
        }
    }
    // A plain clang or gcc that can target mingw also satisfies rustc.
    agent_provider::which_on_path("clang").is_some()
        || agent_provider::which_on_path("gcc").is_some()
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

/// Can this rustup toolchain actually build a wasm crate on this machine?
///
/// The only test that does not lie. Every proxy we tried -- is the toolchain
/// listed, does link.exe exist, does vswhere report VC tools, does a
/// self-contained directory exist -- said yes or no while the machine
/// disagreed. So compile a two-line crate for wasm32-wasip1 with that
/// toolchain and see. Takes about a second, cached afterwards, and it is
/// what the real build will do (K-130).
#[cfg(windows)]
fn toolchain_builds_wasm(toolchain: &str) -> bool {
    use std::sync::Mutex;
    static SEEN: Mutex<Option<Vec<(String, bool)>>> = Mutex::new(None);
    if let Ok(mut guard) = SEEN.lock() {
        let seen = guard.get_or_insert_with(Vec::new);
        if let Some((_, ok)) = seen.iter().find(|(name, _)| name == toolchain) {
            return *ok;
        }
    }
    let ok = probe_wasm_build(toolchain);
    if let Ok(mut guard) = SEEN.lock() {
        if let Some(seen) = guard.as_mut() {
            seen.push((toolchain.to_string(), ok));
        }
    }
    ok
}

#[cfg(windows)]
fn probe_wasm_build(toolchain: &str) -> bool {
    let dir = std::env::temp_dir().join(format!("krate-linkprobe-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    if fs::create_dir_all(dir.join("src")).is_err() {
        return false;
    }
    // A crate with a build script, because that is the part that needs a HOST
    // linker -- the exact thing wit-bindgen-rt fails on.
    let wrote = fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"krate-linkprobe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .and_then(|_| fs::write(dir.join("build.rs"), "fn main() {}\n"))
    .and_then(|_| fs::write(dir.join("src/lib.rs"), "pub fn probe() -> u32 { 1 }\n"))
    .is_ok();
    if !wrote {
        return false;
    }
    // Build for wasm, exactly as the real app build does.
    //
    // Two rewrites of this line shipped broken Windows binaries, so it is worth
    // saying what it must NOT become. Probing the host only (v0.1.44) accepts a
    // toolchain that links but has no wasm target; on a clean windows-2022 that
    // is the only toolchain there is, and every build then dies at "failed to
    // find the `wasm32-wasip1` target". Probing host-then-wasm (v0.1.45) rejects
    // that toolchain correctly and is no better, because the fallback below
    // selects it anyway and nothing downstream consults the verdict.
    //
    // Building for wasm is the honest test: it exercises the host linker (the
    // build script still links) AND the target in one command, and when the
    // target is merely missing, cargo-component -- which runs under rustup --
    // has rustup install it on demand. That is why this shape worked for three
    // releases while both "improvements" did not.
    let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
    let mut cmd = ProcessCommand::new(rustup);
    cmd.args([
        "run",
        toolchain,
        "cargo",
        "build",
        "--quiet",
        "--target",
        CREATE_WASM_TARGET,
    ])
    .current_dir(&dir);
    // Probe under the same linker the real build gets, so gnullvm is not
    // rejected for a linker that is present and about to be used (K-134).
    point_gnullvm_at_its_own_linker(&mut cmd);
    let ok = cmd
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    let _ = fs::remove_dir_all(&dir);
    ok
}

/// The rustup toolchain that can actually build here, if any.
#[cfg(windows)]
fn working_windows_toolchain() -> Option<String> {
    let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
    let listed = ProcessCommand::new(rustup)
        .args(["toolchain", "list"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&listed.stdout).to_string();
    let names: Vec<String> = text
        .lines()
        .map(|line| line.split_whitespace().next().unwrap_or("").to_string())
        .filter(|name| !name.is_empty())
        .collect();
    // gnullvm first when it works (no Build Tools needed), then anything
    // else installed -- msvc included, which is what a machine with Visual
    // Studio already has.
    let mut order: Vec<String> = names
        .iter()
        .filter(|n| n.contains("gnullvm"))
        .cloned()
        .collect();
    order.extend(names.iter().filter(|n| !n.contains("gnullvm")).cloned());
    order.into_iter().find(|name| toolchain_builds_wasm(name))
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
    if working_windows_toolchain().is_none() && !msvc_linker_present() {
        // Reinstall rather than install: on a machine where a PREVIOUS
        // gnullvm install left the toolchain without its mingw clang,
        // `rustup toolchain install` sees the name already present and does
        // nothing, so the repair repaired nothing. --force-non-host is not
        // needed; the plain force reinstall replaces the incomplete tree.
        missing.push(MissingTool {
            what: "a linker for Windows",
            install_cmd: vec![
                "rustup".into(),
                "toolchain".into(),
                "install".into(),
                "--force".into(),
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
                // The default toolchain, matching has_rust_target and the
                // fallback in rustup_toolchain_bin. Naming a probed toolchain
                // here split the install from the check that reads it, so a
                // machine could install the target and still be told it was
                // missing.
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
            // The probe could not answer but a rustup toolchain exists --
            // usually a freshly installed rustup whose pinned toolchain has
            // not synced yet, so `target list` stalls into a download and
            // fails. This arm used to shrug (`None => {}`), preflight said
            // ready, and the very first build on a brand-new machine died
            // mid-compile on the missing wasm target (K-204). `rustup
            // target add` is idempotent and syncs the toolchain first, so
            // the honest answer to "cannot tell" is: run the add.
            None => missing.push(MissingTool {
                what: CREATE_WASM_TARGET,
                install_cmd: vec![
                    "rustup".into(),
                    "target".into(),
                    "add".into(),
                    CREATE_WASM_TARGET.into(),
                ],
                note: "the WebAssembly target Krate apps compile to",
            }),
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
/// Put `~/.cargo/bin` FIRST on this process's PATH when it exists, so the
/// rustup-managed toolchain wins over a Homebrew rustc for every check and
/// every child build command. The shadow is not hypothetical: a machine
/// with `brew install rust` passed the toolchain check on rustup's
/// presence and then compiled with Homebrew's rustc, which has no
/// wasm32-wasip1 target -- the build died mid-compile with the very
/// message that warns about this trap (K-204, measured on a fresh HOME).
fn prefer_cargo_bin() {
    #[cfg(windows)]
    let home_var = "USERPROFILE";
    #[cfg(not(windows))]
    let home_var = "HOME";
    let Ok(home) = std::env::var(home_var) else {
        return;
    };
    let cargo_bin = Path::new(&home).join(".cargo").join("bin");
    if !cargo_bin.is_dir() {
        return;
    }
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&old).collect();
    if paths.first() == Some(&cargo_bin) {
        return;
    }
    paths.retain(|p| p != &cargo_bin);
    paths.insert(0, cargo_bin);
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

fn preflight_toolchain(assume_yes: bool, no_install: bool) -> Result<()> {
    prefer_cargo_bin();
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
        let installed = run_install_command(&tool.install_cmd)
            .with_context(|| format!("install {}", tool.what));
        #[cfg(windows)]
        let installed = installed.or_else(|winget_err| {
            // winget is Windows 11's package manager, but store policies,
            // enterprise images, and first-run agreement prompts all make it
            // fail on machines that are otherwise fine. rustup publishes a
            // plain installer exe; download it ourselves and run it silently.
            if tool.what != "Rust (cargo)" {
                return Err(winget_err);
            }
            eprintln!("    winget could not install it; fetching rustup directly");
            let dest = std::env::temp_dir().join("krate-rustup-init.exe");
            let response = ureq::get("https://win.rustup.rs/x86_64")
                .timeout(std::time::Duration::from_secs(120))
                .call()
                .context("downloading rustup-init.exe")?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes)?;
            fs::write(&dest, &bytes)?;
            let status = std::process::Command::new(&dest)
                .args(["-y", "--no-modify-path", "--default-toolchain", "stable"])
                .status()
                .context("running rustup-init.exe")?;
            if status.success() {
                Ok(())
            } else {
                bail!("rustup-init.exe exited with {status}")
            }
        });
        installed?;
    }

    // Installing rustup does not put cargo on THIS process's PATH
    // (--no-modify-path, deliberately). Extending our own PATH is enough:
    // every build command is our child and inherits it. Without this, a
    // brand-new machine's very first `create --yes` installed everything
    // perfectly and then told the person to open a new terminal and start
    // over -- measured on a fresh HOME (K-204), and exactly what a Studio
    // first-make would die on.
    prefer_cargo_bin();

    // Second pass: installing rustup UNLOCKS installs the first pass could
    // not see or run (the wasm target, cargo-component -- both need cargo
    // on PATH, which prefer_cargo_bin just provided). Bailing here with
    // "open a new terminal and re-run" left a brand-new machine's very
    // first make dead after a flawless install (K-204); the tools are one
    // command away, so run the command.
    let second = missing_create_tools();
    for tool in &second {
        eprintln!("==> installing {}", tool.what);
        eprintln!("    {}", install_command_line(&tool.install_cmd));
        run_install_command(&tool.install_cmd)
            .with_context(|| format!("install {}", tool.what))?;
    }

    // Re-check: only after two passes is "still missing" a real wall, and
    // by then the guidance is honest.
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
            .or_else(|| resolve_tool(program))
            .unwrap_or_else(|| PathBuf::from(program))
    } else {
        // Same staleness for every other tool in the chain: rustup itself,
        // winget-installed binaries. resolve_tool knows ~/.cargo/bin and the
        // directory beside our own exe, which is where a just-finished
        // install put the thing this command is about to run.
        resolve_tool(program).unwrap_or_else(|| PathBuf::from(program))
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
    let rustup = resolve_tool("rustup").unwrap_or_else(|| PathBuf::from("rustup"));
    let output = ProcessCommand::new(rustup)
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

/// Pick up PATH changes made since this process started, without a restart.
///
/// Installers write the new PATH where *future* processes read it -- the
/// registry on Windows, shell profiles on Unix -- and a process that is
/// already running never sees it. That one fact produced three separate
/// "close this terminal and reopen it" moments during a first `krate` run on
/// Windows: install Rust, restart; install the linker, restart; install an AI
/// tool, restart. A new terminal fixes each one only because a new terminal
/// re-reads the registry. This does the same re-read in place.
///
/// Idempotent and cheap, so callers run it before any probe or retry rather
/// than trying to guess whether something was installed in between.
pub(crate) fn refresh_process_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut merged: Vec<String> = std::env::split_paths(&current)
        .map(|p| p.to_string_lossy().to_string())
        .collect();

    for entry in freshly_visible_path_entries() {
        if !merged.iter().any(|have| same_path_entry(have, &entry)) {
            merged.push(entry);
        }
    }

    if let Ok(joined) = std::env::join_paths(merged.iter().map(std::ffi::OsString::from)) {
        // Set for this process only; nothing here writes anywhere persistent.
        std::env::set_var("PATH", joined);
    }
}

/// PATH entries a *new* terminal would see that this process might not.
#[cfg(windows)]
fn freshly_visible_path_entries() -> Vec<String> {
    // The two places Windows assembles a fresh PATH from. reg.exe ships with
    // every Windows and needs no elevation to read either key.
    let mut entries = Vec::new();
    for (root, key) in [
        ("HKCU", "Environment"),
        (
            "HKLM",
            "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        ),
    ] {
        let mut reg = ProcessCommand::new("reg");
        reg.args(["query", &format!("{root}\\{key}"), "/v", "Path"]);
        // reg.exe is a console program; without this, a GUI-launched engine
        // flashes a terminal window for every registry read.
        agent_provider::hide_child_console(&mut reg);
        let output = reg.output();
        if let Ok(output) = output {
            if let Some(value) = parse_reg_path_value(&String::from_utf8_lossy(&output.stdout)) {
                for entry in value.split(';').filter(|e| !e.trim().is_empty()) {
                    entries.push(expand_windows_env(entry.trim()));
                }
            }
        }
    }
    entries
}

/// On Unix the equivalent staleness is a tool home written into a shell
/// profile this process never sourced. The homes are few and well known.
#[cfg(not(windows))]
fn freshly_visible_path_entries() -> Vec<String> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    [".cargo/bin", ".grok/bin", ".local/bin"]
        .iter()
        .map(|tail| home.join(tail))
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.to_string_lossy().to_string())
        .collect()
}

/// Pull the data out of a `reg query ... /v Path` answer.
///
/// The value line looks like `    Path    REG_EXPAND_SZ    C:\a;C:\b` -- the
/// data is everything after the type token, and it may contain spaces, so
/// splitting on whitespace would truncate it.
// Only the Windows refresh calls these two; they stay compiled (and tested)
// everywhere because they are pure string work and the tests must not be
// Windows-only.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_reg_path_value(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("Path") {
            continue;
        }
        for kind in ["REG_EXPAND_SZ", "REG_SZ"] {
            if let Some(index) = trimmed.find(kind) {
                let value = trimmed[index + kind.len()..].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Expand `%NAME%` references the registry stores unexpanded.
///
/// REG_EXPAND_SZ values routinely say `%USERPROFILE%\.cargo\bin`; a new
/// terminal expands them and so must we, or the entry is a directory that
/// does not exist and the refresh silently does nothing.
#[cfg_attr(not(windows), allow(dead_code))]
fn expand_windows_env(entry: &str) -> String {
    let mut result = String::with_capacity(entry.len());
    let mut rest = entry;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) => result.push_str(&value),
                    // Unknown variable: keep the literal text, matching cmd.
                    Err(_) => {
                        result.push('%');
                        result.push_str(name);
                        result.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                result.push('%');
                rest = after;
            }
        }
    }
    result.push_str(rest);
    result
}

/// Whether two PATH entries name the same directory, the way the installer
/// compares them: case-insensitively on Windows, ignoring a trailing slash.
fn same_path_entry(a: &str, b: &str) -> bool {
    let trim = |s: &str| s.trim_end_matches(['\\', '/']).to_string();
    if cfg!(windows) {
        trim(a).eq_ignore_ascii_case(&trim(b))
    } else {
        trim(a) == trim(b)
    }
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

pub(crate) fn krate_home() -> PathBuf {
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

/// Where one app's shared-store mirror lives, alongside its other storage.
fn app_shared_path(app_id: &str) -> PathBuf {
    app_store_path(app_id).with_extension("shared.json")
}

/// Where the id of the agent session that last wrote this app is kept: beside
/// the code (found by repair rounds in the same workspace) and beside the
/// app's other storage (found by a revise, whose workspace is a fresh temp).
fn agent_session_files(app_dir: &str) -> Vec<PathBuf> {
    let mut files = vec![Path::new(app_dir).join(".agent-session-id")];
    if let Ok(manifest) =
        krate_manifest::Manifest::parse_file(Path::new(app_dir).join("manifest.toml"))
    {
        files.push(app_store_path(&manifest.app.id).with_extension("agent-session"));
    }
    files
}

/// Read-and-remove the stored session id, if one exists FOR THIS PROVIDER.
/// One shot by design: a resume that fails must not be retried forever.
fn take_agent_session(files: &[PathBuf], provider: &str) -> Option<String> {
    let mut found = None;
    for file in files {
        if let Ok(text) = fs::read_to_string(file) {
            let _ = fs::remove_file(file);
            if found.is_none() {
                found = text
                    .trim()
                    .strip_prefix(&format!("{provider}:"))
                    .map(str::to_string)
                    .filter(|id| !id.is_empty());
            }
        }
    }
    found
}

/// The hub shared stores sync against. The same override the publisher and
/// the studio honour, so a local hub serves everything at once.
fn shared_hub_url() -> String {
    std::env::var("KRATE_HUB_URL").unwrap_or_else(|_| "https://hub.krate.tech".to_string())
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
pub(crate) fn machine_key() -> Vec<u8> {
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
mod bounded_loop_tests {
    use super::bounded_interactive_loop;

    /// The news app that closed itself after thirty seconds while somebody
    /// was reading it. The authoring pack forbids this at length and an AI
    /// wrote it anyway, so it is checked rather than asked for.
    #[test]
    fn a_thirty_second_bound_on_a_real_session_is_caught() {
        let lib = r#"
const MAX_ROUNDS: u32 = 600;
const QUICK_ROUNDS: u32 = 20;
const ROUND_MILLIS: u32 = 50;
fn run() -> i32 {
    let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };
    while r < rounds { }
}
"#;
        let found = bounded_interactive_loop(lib);
        assert!(found.is_some(), "600 rounds of 50ms is thirty seconds");
        assert!(found.unwrap().contains("MAX_ROUNDS"));
    }

    /// A bound measured in hours is a backstop against a runaway, not a limit
    /// on the session. krate-notes uses 600_000 rounds of 50ms -- 8.3 hours.
    #[test]
    fn a_bound_nobody_reaches_is_left_alone() {
        let lib = r#"
const MAX_WAIT_ROUNDS: u32 = 600_000;
const QUICK_WAIT_ROUNDS: u32 = 20;
const WAIT_ROUND_MILLIS: u32 = 50;
fn run() -> i32 {
    let rounds = if quick { QUICK_WAIT_ROUNDS } else { MAX_WAIT_ROUNDS };
}
"#;
        assert!(bounded_interactive_loop(lib).is_none());
    }

    /// A game with no wait constant counts frames, and the runtime paces
    /// present to 60fps. krate-nova's 100_000 frames is about 28 minutes.
    #[test]
    fn a_frame_paced_game_is_left_alone() {
        let lib = r#"
const MAX_FRAMES: u32 = 100_000;
const QUICK_FRAMES: u32 = 90;
fn run() -> i32 {
    let frame_cap = if quick { QUICK_FRAMES } else { MAX_FRAMES };
}
"#;
        assert!(bounded_interactive_loop(lib).is_none());
    }

    /// The correct shape: the bound applies only when quick.
    #[test]
    fn a_bound_that_only_applies_to_quick_is_correct() {
        let lib = r#"
const MAX_FRAMES: u32 = 5000;
const QUICK_FRAMES: u32 = 90;
fn run() -> i32 {
    let frame_cap = if quick { QUICK_FRAMES } else { MAX_FRAMES };
    while !quick || frames < frame_cap { }
}
"#;
        assert!(
            bounded_interactive_loop(lib).is_none(),
            "while !quick || ... bounds only the quick path"
        );
    }

    #[test]
    fn an_unbounded_loop_is_correct() {
        let lib = "fn run() -> i32 { loop { match events::wait(None) { _ => {} } } }";
        assert!(bounded_interactive_loop(lib).is_none());
    }
}

#[cfg(test)]
mod card_tests {
    use super::{card_file_stem, card_trust_line, compose_card_face};
    use std::io::{Cursor, Write};

    fn manifest_with(caps: &[&str]) -> krate_manifest::Manifest {
        let mut toml = String::from(
            "[app]\nid = \"dev.krate.card\"\nname = \"Rate card\"\nversion = \"0.1.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n",
        );
        for cap in caps {
            toml.push_str(&format!(
                "\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"t\"\nrequired = true\n"
            ));
        }
        krate_manifest::Manifest::parse(&toml).expect("manifest parses")
    }

    /// The caption uses the same words the consent prompt uses, ending with
    /// the guarantee. This is the line a stranger reads before daring to tap.
    #[test]
    fn trust_line_reads_like_the_consent_prompt() {
        let line = card_trust_line(&manifest_with(&["ui.window:create", "store.kv"]));
        assert!(
            line.starts_with("can "),
            "the line states ability, got {line:?}"
        );
        assert!(line.ends_with("· nothing else"), "got {line:?}");
        assert!(line.contains("window"), "got {line:?}");
    }

    #[test]
    fn file_stem_closes_spaces_and_survives_odd_names() {
        assert_eq!(card_file_stem("Rate card"), "RateCard");
        assert_eq!(card_file_stem("tip / splitter!"), "TipSplitter");
        assert_eq!(card_file_stem("   "), "App");
    }

    /// The one-file-two-programs property, end to end: a composed face with
    /// a real zip bundle behind it decodes as a PNG from the front AND opens
    /// as a bundle from the back. This is K-195's card mechanism as a test,
    /// so nobody can break either half without hearing about it.
    #[test]
    fn a_card_reads_as_both_picture_and_bundle() {
        // A minimal but real bundle: manifest first (as pack writes it), then
        // the component entry.
        let manifest_toml = "[app]\nid = \"dev.krate.card\"\nname = \"Rate card\"\n\
             version = \"0.1.0\"\nentry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n";
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.toml", stored).expect("start manifest");
        zip.write_all(manifest_toml.as_bytes()).expect("write manifest");
        zip.start_file("code.wasm", stored).expect("start component");
        zip.write_all(b"\0asm\x01\0\0\0").expect("write component");
        let bundle_bytes = zip.finish().expect("finish zip").into_inner();

        // A real face over it, composed exactly as `krate card` composes one.
        let shot = krate_adapter_common::ui::ImagePixels::new(
            8,
            8,
            vec![0x80u8; 8 * 8 * 4],
        )
        .expect("shot pixels");
        let face =
            compose_card_face(&shot, "RateCard.krate", "1 KB", "can open a window · nothing else")
                .expect("face composes");

        let mut card = face.clone();
        card.extend_from_slice(&bundle_bytes);

        // Reading from the front: a picture, shot plus caption strip.
        let decoder = png::Decoder::new(Cursor::new(card.as_slice()));
        let reader = decoder.read_info().expect("the card fronts as a PNG");
        let info = reader.info();
        assert_eq!(info.width, 8);
        assert!(info.height > 8, "the caption strip is part of the face");

        // Reading from the back: the app, by the same open the runtime uses.
        let opened =
            krate_bundle::open_reader(Cursor::new(card)).expect("the card backs as a bundle");
        assert_eq!(opened.manifest().app.name, "Rate card");
    }
}

#[cfg(test)]
mod wrap_tests {
    use super::{wrap_prefix_unix, wrap_prefix_windows};
    use std::io::{Cursor, Write};

    fn tiny_bundle() -> Vec<u8> {
        let manifest_toml = "[app]\nid = \"dev.krate.wrap\"\nname = \"Rate card\"\n\
             version = \"0.1.0\"\nentry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n";
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.toml", stored).expect("start manifest");
        zip.write_all(manifest_toml.as_bytes()).expect("write manifest");
        zip.start_file("code.wasm", stored).expect("start component");
        zip.write_all(b"\0asm\x01\0\0\0").expect("write component");
        zip.finish().expect("finish zip").into_inner()
    }

    /// The one-file-two-programs property for the wrap: the shell reads a
    /// script from the front, `krate run` reads the app from the back.
    #[test]
    fn a_unix_wrap_reads_as_script_and_bundle() {
        let mut wrap = wrap_prefix_unix("Rate card", "RateCard").into_bytes();
        wrap.extend_from_slice(&tiny_bundle());
        assert!(wrap.starts_with(b"#!/bin/sh\n"), "the front is a shell script");
        let opened =
            krate_bundle::open_reader(Cursor::new(wrap)).expect("the back is a bundle");
        assert_eq!(opened.manifest().app.name, "Rate card");
    }

    /// The prefix alone must be a valid shell program: `sh -n` parses it
    /// without executing anything, which is exactly the reading a receiver's
    /// shell will do before the exit line stops it.
    #[test]
    #[cfg(unix)]
    fn the_unix_prefix_parses_as_shell() {
        let prefix = wrap_prefix_unix("Rate card", "RateCard");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("prefix.sh");
        std::fs::write(&path, prefix).expect("write prefix");
        let status = std::process::Command::new("sh")
            .arg("-n")
            .arg(&path)
            .status()
            .expect("run sh -n");
        assert!(status.success(), "sh -n rejected the wrap prefix");
    }

    /// cmd.exe wants CRLF, must never fall through into the bundle bytes,
    /// and must not carry the player -- it plants it by download.
    #[test]
    fn the_windows_prefix_is_crlf_and_exits_before_the_bundle() {
        let prefix = wrap_prefix_windows("Rate card", "RateCard");
        assert!(prefix.starts_with("@echo off\r\n"));
        assert!(prefix.contains("exit /b %STATUS%\r\n"), "execution ends before the blob");
        assert!(prefix.contains("install.ps1"), "it plants the player, never bundles it");
        assert!(!prefix.contains('\u{0}'), "text only; the app rides behind");
        let mut wrap = prefix.into_bytes();
        wrap.extend_from_slice(&tiny_bundle());
        let opened =
            krate_bundle::open_reader(Cursor::new(wrap)).expect("the back is a bundle");
        assert_eq!(opened.manifest().app.name, "Rate card");
    }
}

#[cfg(test)]
mod gating_tests {
    use super::gating_capability;

    fn manifest_with(caps: &[(&str, bool)]) -> krate_manifest::Manifest {
        let mut toml = String::from(
            "[app]\nid = \"dev.krate.t\"\nname = \"T\"\nversion = \"0.1.0\"\n\
             entry = \"a.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n",
        );
        for (cap, required) in caps {
            toml.push_str(&format!(
                "\n[[capabilities]]\ncap = \"{cap}\"\nrationale = \"t\"\nrequired = {required}\n"
            ));
        }
        krate_manifest::Manifest::parse(&toml).expect("manifest parses")
    }

    /// The bug that threw away a finished edit.
    ///
    /// A screensaver declared `gfx.gpu:basic` as required. That capability is
    /// granted to every app by default, so withholding it changes nothing --
    /// the app ran fine and exited 0, and the permission-wall check reported
    /// "withholding gfx.gpu:basic should refuse with exit 5, got 0" and
    /// discarded work that had already built and packed.
    #[test]
    fn a_capability_granted_to_everyone_is_never_the_gate() {
        let manifest = manifest_with(&[
            ("ui.window:create", true),
            ("gfx.gpu:basic", true),
            ("io.stdout", true),
        ]);
        assert_eq!(
            gating_capability(&manifest),
            None,
            "nothing here can be withheld, so there is no wall to test"
        );
    }

    #[test]
    fn a_real_capability_is_still_chosen() {
        let manifest = manifest_with(&[
            ("ui.window:create", true),
            ("gfx.gpu:basic", true),
            ("store.kv", true),
        ]);
        assert_eq!(gating_capability(&manifest), Some("store.kv".to_string()));
    }

    #[test]
    fn filesystem_access_is_preferred_when_present() {
        let manifest = manifest_with(&[("store.kv", true), ("fs.write:notes/**", true)]);
        assert_eq!(
            gating_capability(&manifest),
            Some("fs.write:notes/**".to_string()),
            "the clearest thing to show being withheld"
        );
    }
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
mod refusal_tests {
    use super::{agent_failure_reason_in, extract_plan_json};

    /// A provider that REFUSED must be told apart from one that answered in
    /// a shape we cannot parse. Both leave extract_plan_json empty, and for
    /// a while both fell through to "building directly" -- which is how a
    /// person who typed "do not create any app" got an app (K-182).
    #[test]
    fn a_provider_that_refused_is_not_read_as_permission_to_build() {
        // codex exec --json, account out of credit. Real shape, captured
        // from `codex exec --json` on 2026-08-27.
        let refused = concat!(
            r#"{"type":"thread.started","thread_id":"01a03f30"}"#, "\n",
            r#"{"type":"turn.started"}"#, "\n",
            r#"{"type":"error","message":"You've hit your usage limit."}"#, "\n",
            r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#,
        );
        assert!(
            extract_plan_json(refused).is_none(),
            "a refusal carries no plan, which is why it needs its own check"
        );
        let reason = agent_failure_reason_in(refused)
            .expect("a refusal must surface the provider's own words");
        assert!(
            reason.contains("usage limit"),
            "the person must be told what the AI actually said: {reason}"
        );
    }

    /// The other half. An answer that is merely unparseable must NOT be
    /// reported as a failure -- the soft fallback to a direct build is
    /// deliberate and is what keeps a first request from dying on an output
    /// shape nobody has seen yet.
    #[test]
    fn an_unparseable_answer_is_not_mistaken_for_a_refusal() {
        for text in [
            "I think a tip calculator would be lovely. Shall I start?",
            "",
            r#"{"type":"item.completed","item":{"text":"not json at all"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":10}}"#,
        ] {
            assert!(
                agent_failure_reason_in(text).is_none(),
                "this is an unreadable answer, not a refusal: {text:?}"
            );
        }
    }

    /// Her transcript, as it actually arrived: the JSON event torn in half by
    /// a stderr write, leaving an orphan fragment, and the same error present
    /// only as prose. Before this, every line was skipped and she was told
    /// "the grok agent did not finish successfully; see <file>" while the
    /// file said "Not signed in" in plain English (K-187).
    #[test]
    fn a_plain_text_error_is_read_when_the_json_event_was_torn_in_half() {
        let hers = concat!(
            "Error: Not signed in. To authenticate without a browser, run:\n",
            "  grok login --device-code\n",
            "\n",
            "Alternatively, set the XAI_API_KEY environment variable or run ",
            "`grok login` on a machine with a browser.\n",
            // The tail of the JSON event, cut in two by the interleaving.
            "achine with a browser.\"}\n",
        );
        let reason = agent_failure_reason_in(hers)
            .expect("the prose must be read when the JSON did not survive");
        assert!(
            reason.contains("Not signed in"),
            "she must be told what her tool actually said: {reason}"
        );
    }

    /// The JSON event still wins when it survives, because it is the clean
    /// single sentence rather than the first line of a wrapped paragraph.
    #[test]
    fn a_surviving_json_event_is_preferred_over_the_prose() {
        let both = concat!(
            r#"{"type":"error","message":"Not signed in. Run: grok login --device-code"}"#,
            "\n",
            "Error: Not signed in. To authenticate without a browser, run:\n",
        );
        let reason = agent_failure_reason_in(both).expect("an error is present");
        assert!(
            reason.contains("Run: grok login"),
            "the structured event is the better copy: {reason}"
        );
    }

    /// A real plan still parses, so neither check can swallow a good answer.
    #[test]
    fn a_real_answer_is_still_read_as_one() {
        let asked = r#"{"ask":["What should it do?"]}"#;
        let planned = r#"{"plan":"A tip calculator.","needs":[]}"#;
        assert!(extract_plan_json(asked).is_some());
        assert!(extract_plan_json(planned).is_some());
        assert!(agent_failure_reason_in(asked).is_none());
        assert!(agent_failure_reason_in(planned).is_none());
    }
}

#[cfg(test)]
mod wall_tests {
    use super::{wall_in_request, Wall};

    /// The walls the gate is allowed to name are the ones with no capability
    /// at all. Naming them costs ten seconds instead of the forty minutes a
    /// build takes to discover the same thing.
    #[test]
    fn a_request_for_something_krate_cannot_do_is_named_before_building() {
        let cases: &[(&str, Wall)] = &[
            ("a screen recorder that saves an mp4", Wall::ScreenCapture),
            ("an app to record my screen while I talk", Wall::ScreenCapture),
            ("an app that types my signature into whatever field is focused",
             Wall::OtherApps),
            ("something that clicks buttons in other apps for me", Wall::OtherApps),
            ("a MIDI keyboard app that plays my connected piano", Wall::Hardware),
            ("read my bluetooth heart rate device", Wall::Hardware),
            ("a background timer that keeps running after I close the window",
             Wall::Background),
        ];
        for (request, expected) in cases {
            assert_eq!(
                wall_in_request(request),
                Some(*expected),
                "should have been caught as a wall: {request}"
            );
        }
    }

    /// The half that matters more. A false hit refuses work Krate can really
    /// do, which is a worse failure than the slow discovery this gate
    /// replaces -- so ordinary requests, and every request that merely uses
    /// a wall's vocabulary innocently, must pass straight through.
    #[test]
    fn ordinary_requests_are_never_mistaken_for_walls() {
        let buildable = [
            // Plain apps.
            "a tip calculator with a bill field and buttons for 15 and 20 percent",
            "a to-do list I can check off, that remembers my items",
            "a snake game I play with arrow keys",
            "a markdown note editor with live preview",
            "a weather dashboard that fetches the current weather for a city",
            // Words a wall rule uses, in innocent senses.
            "a full screen clock app",
            "a screen saver with bouncing shapes",
            "a game that fills the whole screen",
            "an app that types out my notes as I press keys",
            "a drawing app I control with the mouse",
            "a keyboard trainer that shows which key to press",
            "a piano app I play with the computer keyboard",
            "a timer that runs in its own window",
            // The word "background" is not the wall; background RUNNING is.
            // An earlier cut of the rule refused this, because "background"
            // satisfied both halves of the match on its own.
            "an app with a dark background and big buttons",
            "a note app with a background image",
            "a game with a scrolling background",
            "a photo viewer for images I drop on it",
            // The camera, deliberately. A backend exists on all three desktop
            // systems (K-119 macOS, K-148 Windows and Linux), so refusing it
            // would be false everywhere -- and even where hardware might let
            // it down, "might not work here" is not this gate's question.
            "an app that shows my webcam feed with a photo button",
            "a camera app that takes a picture",
        ];
        for request in buildable {
            assert_eq!(
                wall_in_request(request),
                None,
                "a buildable request was refused as a wall: {request}"
            );
        }
    }
}

#[cfg(test)]
mod create_tests {
    use super::{agent_home_for, change_prompt, extract_plan_json, CHANGE_MARKER};

    /// A brand-new person gets confined exactly like an old one.
    ///
    /// This test exists because the fix nearly shipped broken in the other
    /// direction: an earlier cut confined the agent only when a credential
    /// could be seeded into the confined home, which meant somebody who had
    /// never signed the agent in -- a first-time user, the very person
    /// meeting these prompts -- silently kept the real home. Measured in a
    /// fresh macOS account: the agent was handed HOME=/Users/test (K-179).
    #[test]
    fn the_agents_home_is_confined_for_everyone_including_first_time_users() {
        let real = Path::new("/Users/newcomer");
        let confined = agent_home_for(real);

        assert!(
            confined.starts_with(real.join(".krate")),
            "the agent's home must live under Krate's own directory, got {}",
            confined.display()
        );
        assert_ne!(
            confined, real,
            "the agent must never be handed the person's real home"
        );
    }

    /// A change and a new app are different jobs and must get different
    /// instructions. They used to share one prompt, so an AI asked to move a
    /// button was told to "find the closest example and adapt it" and
    /// "write the app" -- and re-derived a whole Krate app to change one line.
    /// The dominant cost of authoring an app was measured, not guessed.
    ///
    /// An incremental app compile is 0.7s and a clean one 1.7s -- compilation
    /// is under one percent. But `check-app` is 17s, because it also runs the
    /// app twice under a headless budget, resizes its window and clicks it.
    /// The AI ran it five times, and said so in its own transcript: "The
    /// check-app is taking a long time - probably building. Let me wait."
    ///
    /// `--no-run` is 2.0s and still catches what actually breaks: it returns
    /// the build error and the wasi-import error before it stops. So the loop
    /// is iterate with --no-run, prove once with the full check.
    #[test]
    fn both_prompts_teach_the_fast_check_loop() {
        let fresh = claude_author_prompt("/work/app", "a tip calculator", "/k");
        assert!(fresh.contains("check-app . --no-run"), "fresh: fast loop");
        assert!(
            fresh.contains("run the full check once") || fresh.contains("full check"),
            "fresh: full check still required"
        );

        let marked = format!("{CHANGE_MARKER}make the button blue");
        let edit = claude_author_prompt("/work/app", &marked, "/k");
        assert!(edit.contains("check-app . --no-run"), "edit: fast loop");
        assert!(edit.contains("full"), "edit: full check still required");
    }

    #[test]
    fn a_change_is_told_to_edit_not_to_write_an_app() {
        let marked = format!("{CHANGE_MARKER}make the button blue");
        let prompt = claude_author_prompt("/work/app", &marked, "/usr/local/bin/krate");

        assert!(prompt.contains("This is an edit, not a rewrite"));
        assert!(prompt.contains("make the button blue"));
        // The things that make a fresh build slow must NOT be asked for.
        assert!(
            !prompt.contains("Find the closest example"),
            "an edit must not go hunting for an example app"
        );
        assert!(
            prompt.contains("KRATE_AUTHORING.md is in this directory"),
            "the reference stays available, consulted rather than read whole"
        );
        // And the marker itself must never be shown to the model as text.
        assert!(
            !prompt.contains(CHANGE_MARKER),
            "marker leaked into the prompt"
        );
    }

    #[test]
    fn a_new_app_still_gets_the_full_instructions() {
        let prompt = claude_author_prompt("/work/app", "a tip calculator", "/usr/local/bin/krate");
        assert!(prompt.contains("Read KRATE_AUTHORING.md"));
        // The example is pre-picked and written into the workspace; the
        // prompt names it instead of sending the agent hunting.
        assert!(prompt.contains("Your model app is EXAMPLE.rs"));
        assert!(!prompt.contains("This is an edit"));
    }

    /// The example picker: the model app matches the request's shape, and a
    /// request matching nothing falls back to the checklist.
    #[test]
    fn the_closest_example_matches_the_request() {
        use crate::authoring_context::closest_example;
        assert_eq!(
            closest_example("a brick breaker game with a ball").name,
            "krate-bounce"
        );
        assert_eq!(
            closest_example("a habit tracker that saves my streaks").name,
            "krate-checklist"
        );
        assert_eq!(
            closest_example("a contact book with a database").name,
            "krate-contacts"
        );
        assert_eq!(
            closest_example("show me the weather from an api").name,
            "krate-fetch"
        );
        assert_eq!(closest_example("a pomodoro timer").name, "krate-focus");
        assert_eq!(closest_example("something nice").name, "krate-checklist");
    }

    #[test]
    fn the_change_prompt_keeps_the_app_identity() {
        // Renaming the crate or the package breaks the app for somebody who
        // already has a copy, which is the one thing an edit must not do.
        let prompt = change_prompt("/work/app", "add a reset button", "/k");
        assert!(prompt.contains("Keep the same crate name"));
        assert!(prompt.contains("check-app"));
    }

    use super::{
        app_kind_name, author_contract, claude_author_prompt, expand_windows_env, has_tool,
        human_label, install_made_progress, manifest_overreach, name_from_request,
        parse_reg_path_value, rust_version_fits, same_path_entry, silent_author_failure, toml_path,
        validate_create_request, MAX_DERIVED_NAME_WORDS,
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
    fn a_spreadsheet_attachment_becomes_readable_csvs() {
        // The friend's Excel was the whole request, and the agent could not
        // open it: sheets must land beside the original as CSV.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("finances.xlsx");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/finances.xlsx"),
            &path,
        )
        .expect("stage the fixture");
        let mut written = super::spreadsheet_to_csvs(&path);
        written.sort();
        assert_eq!(
            written,
            vec!["finances.Holdings.csv", "finances.Monthly-Budget.csv"]
        );
        let holdings =
            std::fs::read_to_string(dir.path().join("finances.Holdings.csv")).expect("csv");
        assert!(holdings.starts_with("Asset,Amount,Currency\n"));
        assert!(
            holdings.contains("\"Gold, 24k\",50,grams"),
            "comma cell must be quoted: {holdings}"
        );
    }

    #[test]
    fn a_plan_answer_survives_prose_and_fences() {
        // Agents wrap answers no matter what the prompt says; the contract
        // lives in extraction.
        let wrapped =
            "Sure! Here is my answer:\n```json\n{\"ask\": [\"what data?\"]}\n```\nHope that helps.";
        assert_eq!(
            extract_plan_json(wrapped).as_deref(),
            Some("{\"ask\": [\"what data?\"]}")
        );
        // A stray brace inside a string must not break the balance scan.
        let tricky = "{\"plan\": \"draws a { curly } chart\", \"needs\": []}";
        assert_eq!(extract_plan_json(tricky).as_deref(), Some(tricky));
        // JSON without the contract keys is not an answer.
        assert_eq!(extract_plan_json("{\"other\": 1}"), None);
        assert_eq!(extract_plan_json("no json at all"), None);
    }

    #[test]
    fn a_grok_envelope_is_unwrapped() {
        // The exact shape grok's CLI prints for a plan, captured from a real
        // Windows session that failed with "the AI did not answer in the
        // expected shape". The plan is a correct `{"ask": [...]}` buried in the
        // envelope's `text` string; the contract must reach it.
        let grok = r#"{
  "text": "{\"ask\": [\"Should this play your own music files, or just look like Apple Music?\", \"Which screens matter?\"]}",
  "stopReason": "end_turn",
  "sessionId": "01a015c0-deb7-7a13-a3f5-97f46fd8a20b",
  "thought": "The user wants me to decide whether the request is ready."
}"#;
        let extracted = extract_plan_json(grok).expect("grok envelope must be unwrapped");
        let value: serde_json::Value = serde_json::from_str(&extracted).unwrap();
        assert!(value.get("ask").is_some(), "got: {extracted}");

        // A plan-shaped answer inside the envelope works too.
        let planned = r#"{"text":"{\"plan\":\"a music player\",\"needs\":[\"ui.window\"]}","stopReason":"end_turn"}"#;
        let value: serde_json::Value =
            serde_json::from_str(&extract_plan_json(planned).unwrap()).unwrap();
        assert_eq!(
            value.get("plan").and_then(|p| p.as_str()),
            Some("a music player")
        );

        // An envelope whose text is NOT a plan is still not an answer.
        let empty = r#"{"text":"I think this needs more detail.","stopReason":"end_turn"}"#;
        assert_eq!(extract_plan_json(empty), None);
    }

    #[test]
    fn a_codex_stream_is_unwrapped() {
        // Codex streams one JSON object per line, and the plan is nested inside
        // an item.completed event's item.text -- captured verbatim from a real
        // Windows session that failed. The scanner must read every line and
        // descend into the nested escaped string.
        let codex = concat!(
            "{\"type\":\"thread.started\",\"thread_id\":\"01a0\"}\n",
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"id\":\"item_0\",\"type\":\"agent_message\",",
            "\"text\":\"{\\\"plan\\\":\\\"a stopwatch\\\",\\\"needs\\\":[]}\"}}\n",
            "{\"type\":\"turn.completed\"}\n",
        );
        let value: serde_json::Value =
            serde_json::from_str(&extract_plan_json(codex).expect("codex stream unwrapped"))
                .unwrap();
        assert_eq!(
            value.get("plan").and_then(|p| p.as_str()),
            Some("a stopwatch")
        );

        // A stream that only asks questions, nested the same way.
        let asked = concat!(
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"item.completed\",\"item\":{\"text\":\"{\\\"ask\\\":[\\\"which screens?\\\"]}\"}}\n",
        );
        let value: serde_json::Value =
            serde_json::from_str(&extract_plan_json(asked).unwrap()).unwrap();
        assert!(value.get("ask").is_some());
    }

    #[test]
    fn garbage_output_extracts_nothing_rather_than_a_wrong_plan() {
        // The other half of never-failing: extraction must not FALSE-POSITIVE.
        // plan_command falls back to a build when this returns None, so a wrong
        // match here would build the wrong thing while a clean None just builds
        // what was asked. None is the safe answer for anything that is not a
        // real plan.
        assert_eq!(extract_plan_json(""), None);
        assert_eq!(extract_plan_json("connection reset by peer"), None);
        assert_eq!(extract_plan_json("{\"error\":\"rate limited\"}"), None);
        assert_eq!(extract_plan_json("{{{ broken"), None);
        // A model that answers in prose with no JSON at all.
        assert_eq!(
            extract_plan_json("I think you should build a to-do app with reminders."),
            None
        );
        // An envelope whose inner text is prose, not JSON, must not match.
        assert_eq!(
            extract_plan_json("{\"text\":\"sounds good, building now\"}"),
            None
        );
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
        // A pasted chat prompt opens conversationally; the filler must not
        // become the name. The live case: "So i have made..." produced an
        // app (and its permission-wall folder) named "so".
        assert_eq!(
            name_from_request("So i have made a basic excel sheet advanced monthly budget tracker")
                .as_deref(),
            Some("excel-sheet-advanced")
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

    /// A missing AI tool must be named as missing, not reported as a failed
    /// build. Somebody sent to debug their request when the real problem is an
    /// uninstalled binary will not find anything, which is what happened on a
    /// fresh Windows machine.
    #[test]
    fn silent_author_failure_names_a_missing_tool() {
        let not_found = silent_author_failure(Some(127));
        assert!(not_found.contains("not installed"), "{not_found}");
        assert!(not_found.contains("new terminal"), "{not_found}");

        assert!(silent_author_failure(Some(126)).contains("permission"));
        assert!(silent_author_failure(Some(3)).contains("error 3"));
        assert!(silent_author_failure(None).contains("stopped before"));
    }

    /// Doctor must compare versions, not admire paths (K-080): 1.85 on a
    /// 1.91 workspace passed silently and failed at build time.
    #[test]
    fn a_toolchain_older_than_the_workspace_needs_is_called_out() {
        assert_eq!(
            rust_version_fits("cargo 1.91.1 (abc 2026-01-01)", (1, 91)),
            Some(true)
        );
        assert_eq!(
            rust_version_fits("cargo 1.85.1 (abc 2025-01-01)", (1, 91)),
            Some(false)
        );
        assert_eq!(rust_version_fits("cargo 2.0.0", (1, 91)), Some(true));
        assert_eq!(rust_version_fits("garbage with no number", (1, 91)), None);
    }

    /// K-075 step 2: the manifest may not ask beyond the code. Rule one --
    /// an unscoped fs glob reads as "everything" and is refused with the
    /// open-folder path named. Rule two -- a capability whose interface the
    /// component never imports is an ask the code cannot even attempt,
    /// which is exactly the reviewed tidier's fs.remove:** with no remove
    /// call anywhere.
    #[test]
    fn a_manifest_may_not_ask_beyond_the_code() {
        let manifest = |caps: &str| {
            krate_manifest::Manifest::parse(&format!(
                "[app]\nid = \"dev.krate.t\"\nname = \"T\"\nversion = \"0.1.0\"\n\
                 entry = \"code.wasm\"\nworld = \"krate:app/gui@0.2.0\"\n{caps}"
            ))
            .expect("manifest parses")
        };
        let fs_imports = vec!["krate:fs/files@0.1.0".to_string()];

        // Unscoped ** is refused even when fs IS imported.
        let wide = manifest(
            "[[capabilities]]\ncap = \"fs.list:**\"\nrationale = \"r\"\nrequired = true\n",
        );
        let problem = manifest_overreach(&wide, &fs_imports).expect("must refuse **");
        assert!(problem.contains("reads as everything"), "{problem}");

        // A scoped ask with the matching import passes.
        let scoped = manifest(
            "[[capabilities]]\ncap = \"fs.write:./out/**\"\nrationale = \"r\"\nrequired = true\n",
        );
        assert!(manifest_overreach(&scoped, &fs_imports).is_none());

        // Asking for net while importing no net interface is refused.
        let netless = manifest(
            "[[capabilities]]\ncap = \"net.connect:api.example.com:443\"\nrationale = \"r\"\nrequired = true\n",
        );
        let problem =
            manifest_overreach(&netless, &fs_imports).expect("must refuse importless net");
        assert!(problem.contains("never"), "{problem}");

        // The folder-picker path needs no fs capability at all: a manifest
        // asking only for the dialog, with the dialog imported, is clean.
        let picker = manifest(
            "[[capabilities]]\ncap = \"ui.dialog:open-folder\"\nrationale = \"r\"\nrequired = true\n",
        );
        let dialog_imports = vec!["krate:ui/dialog@0.1.0".to_string()];
        assert!(manifest_overreach(&picker, &dialog_imports).is_none());
    }

    /// The install loop must stop when a pass changes nothing.
    ///
    /// K-069: each pass ran an install that reported success while the missing
    /// list stayed the same length, because nothing re-read PATH. Looping on
    /// that repeats one install forever; the loop now treats "no shrink" as
    /// the failure it is.
    #[test]
    fn an_install_pass_that_changes_nothing_is_not_progress() {
        assert!(install_made_progress(3, 2), "one fewer is progress");
        assert!(install_made_progress(1, 0), "reaching zero is progress");
        assert!(!install_made_progress(2, 2), "same length is stuck");
        assert!(
            !install_made_progress(2, 3),
            "growing is stuck, not progress"
        );
    }

    /// The registry answer for PATH has the data after a type token, and the
    /// data itself contains spaces and semicolons. Parsing must take the
    /// whole remainder, not the next whitespace-delimited word.
    #[test]
    fn reg_path_value_survives_spaces_and_expand_type() {
        let output = "\r\nHKEY_CURRENT_USER\\Environment\r\n    \
                      Path    REG_EXPAND_SZ    C:\\Program Files\\Krate\\bin;%USERPROFILE%\\.cargo\\bin\r\n";
        assert_eq!(
            parse_reg_path_value(output).as_deref(),
            Some("C:\\Program Files\\Krate\\bin;%USERPROFILE%\\.cargo\\bin")
        );
        assert_eq!(parse_reg_path_value("no value here"), None);
    }

    /// `%NAME%` must expand from the environment, and an unknown name must
    /// stay literal -- that is what cmd does, and inventing an empty path
    /// entry would be worse than keeping the odd literal.
    #[test]
    fn windows_env_expansion_matches_cmd() {
        std::env::set_var("KRATE_TEST_EXPAND", "C:\\Users\\me");
        assert_eq!(
            expand_windows_env("%KRATE_TEST_EXPAND%\\.cargo\\bin"),
            "C:\\Users\\me\\.cargo\\bin"
        );
        assert_eq!(
            expand_windows_env("%KRATE_TEST_MISSING_VAR%\\bin"),
            "%KRATE_TEST_MISSING_VAR%\\bin"
        );
        assert_eq!(expand_windows_env("plain"), "plain");
        std::env::remove_var("KRATE_TEST_EXPAND");
    }

    /// Refreshing must never duplicate an entry that is already present, or
    /// PATH grows on every call for the life of the process.
    #[test]
    fn path_refresh_is_idempotent_about_known_entries() {
        assert!(same_path_entry("/usr/local/bin", "/usr/local/bin/"));
        if cfg!(windows) {
            assert!(same_path_entry("C:\\Krate\\bin", "c:\\krate\\bin\\"));
        } else {
            assert!(!same_path_entry("/a/b", "/A/B"));
        }
    }

    /// anyhow's `Display` prints only the outermost context, so a failure that
    /// was wrapped with `.context("run --author-cmd")` showed exactly that
    /// string and hid the cause. The TUI must use the alternate form.
    #[test]
    fn error_display_keeps_the_cause() {
        let err = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "program not found",
        ))
        .context("run --author-cmd");

        assert_eq!(err.to_string(), "run --author-cmd");
        let shown = format!("{err:#}");
        assert!(shown.contains("program not found"), "{shown}");
    }

    /// A change must be authored inside the app's own unpacked source, never
    /// in a fresh subdirectory named after the change text -- that is how
    /// "change the controls" once replaced a finished game with a generic
    /// app built from the change sentence alone.
    #[test]
    fn changes_land_in_the_apps_own_source() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A plain workspace: never adopted, change or not.
        assert!(!crate::is_existing_app_workspace(root, true));
        assert!(!crate::is_existing_app_workspace(root, false));
        // An unpacked app: adopted exactly when the request is a change.
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "// the app").unwrap();
        assert!(crate::is_existing_app_workspace(root, true));
        assert!(!crate::is_existing_app_workspace(root, false));
    }

    /// The command built for a provider must be recognized as our own
    /// self-invocation, so authoring can skip the shell -- on Windows the
    /// shell means bash, and plain machines have none.
    #[test]
    fn provider_author_commands_are_recognized_as_self() {
        let provider = crate::agent_provider::resolve("grok").expect("grok is a known provider");
        let cmd = crate::agent_author_command(provider);
        assert_eq!(crate::self_author_agent(&cmd), Some("grok"));
        // A hand-written command must keep going through the shell.
        assert_eq!(crate::self_author_agent("mytool --write-app"), None);
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

    /// A refusal the agent stated but was not allowed to write down (K-139).
    ///
    /// codex's sandbox rejected the write, so a correct "Krate has no camera
    /// API" reached the person as a generic build failure with no reason.
    #[test]
    fn a_refusal_only_in_the_transcript_is_still_a_refusal() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(".agent-transcript.txt"),
            "{\"type\":\"agent_message\",\"text\":\"KRATE-CANNOT-BUILD: Krate has no webcam \
             capability yet\\nBlocked: the sandbox is read-only here.\"}\n",
        )
        .expect("write");
        assert_eq!(
            super::agent_refusal(&dir.path().to_string_lossy()).as_deref(),
            Some("Krate has no webcam capability yet")
        );
    }

    /// The file always wins: it is the contract, and the transcript is only
    /// the fallback for when the agent could not honour it.
    #[test]
    fn the_refusal_file_is_preferred_over_the_transcript() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(super::AGENT_REFUSAL_FILE),
            "KRATE-CANNOT-BUILD: the real reason\n",
        )
        .expect("write");
        std::fs::write(
            dir.path().join(".agent-transcript.txt"),
            "KRATE-CANNOT-BUILD: an earlier thought\n",
        )
        .expect("write");
        assert_eq!(
            super::agent_refusal(&dir.path().to_string_lossy()).as_deref(),
            Some("the real reason")
        );
    }

    /// An agent that considers giving up and then builds the app anyway must
    /// not be read as refusing -- that would fail apps that succeeded.
    #[test]
    fn a_transcript_without_the_marker_is_not_a_refusal() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join(".agent-transcript.txt"),
            "I wondered whether this cannot be built, then found a way.\n",
        )
        .expect("write");
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
