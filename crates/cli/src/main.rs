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

mod mcp;
mod port_report;
mod sdk;
mod sdk_reference;
mod speech_model;

const MAX_PHASE2_ARGS_RAW_BYTES: usize = 64 * 1024;
const MAX_PHASE2_ARG_COUNT: usize = 1024;
/// Fuel budget applied to an untrusted run (`run --untrusted`, and the run
/// Krate makes when it verifies an app it just authored). Large enough that a
/// real app finishing its work never trips it, small enough that a runaway or
/// infinite loop is stopped in well under a second instead of hanging. An
/// explicit `--fuel` always overrides this.
const UNTRUSTED_FUEL_BUDGET: u64 = 5_000_000_000;

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
    #[command(subcommand)]
    command: Command,
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

        /// Use the opt-in native window prototype for Phase 3 GUI apps
        /// (macOS AppKit today). The default GUI path stays headless.
        #[arg(long)]
        native_window: bool,

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

        /// Arguments passed to the Krate app. Put them after `--`.
        #[arg(last = true, value_name = "ARG")]
        app_args: Vec<String>,
    },
    /// Print version information.
    Version,
    /// Check the local development environment.
    Doctor,
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
        /// lower-level escape hatch for any other tool.
        #[arg(long, value_enum)]
        agent: Option<AgentKind>,

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
        #[arg(long, value_enum)]
        agent: Option<AgentKind>,

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

    /// Run the Krate MCP server so an AI agent can execute components under the
    /// capability sandbox. Speaks JSON-RPC 2.0 over stdio; wire an agent at it
    /// with e.g. `claude mcp add krate -- krate mcp`.
    Mcp,

    /// Internal: drive a supported AI agent to author the app. `krate create
    /// --agent claude` runs this; it reads KRATE_REQUEST / KRATE_APP_DIR from
    /// the environment create sets. Hidden because it is not a user entry point.
    #[command(hide = true)]
    AuthorAgent {
        #[arg(value_enum)]
        agent: AgentKind,
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

/// An AI coding agent Krate knows how to drive for `--agent`.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum AgentKind {
    /// Claude Code (the `claude` CLI), driven headlessly.
    Claude,
}

impl AgentKind {
    /// The `--author-cmd` string that drives this agent. Krate builds the
    /// headless prompt and passes the request through the environment, so the
    /// user never has to write agent glue — `--agent claude` just works.
    fn author_command(self) -> &'static str {
        match self {
            // `krate author-agent claude` is a hidden subcommand that runs the
            // agent with the right prompt and flags, reading KRATE_REQUEST etc.
            // from the environment create already sets. Invoking our own binary
            // keeps the prompt versioned with the tool instead of in a script.
            AgentKind::Claude => "krate author-agent claude",
        }
    }
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
    }
    format!("{err:#}")
}

fn run() -> Result<u8> {
    let cli = Cli::parse();

    match cli.command {
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
            insecure_http,
            json,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time,
            test_locale,
            test_timezone,
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
            native_window,
            json,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time_millis: test_time,
            test_locale,
            test_timezone,
            app_args,
        }),
        #[cfg(target_os = "macos")]
        Command::OpenApp => open_app(),
        Command::Pack {
            file,
            manifest,
            output,
        } => pack_bundle(&file, &manifest, &output),
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
        } => create_krate(CreateRequest {
            request,
            output,
            // --agent is the clean front door; it resolves to the command that
            // drives that agent. An explicit --author-cmd still wins for any
            // other tool.
            author_cmd: author_cmd.or_else(|| agent.map(|a| a.author_command().to_string())),
            kind,
            name,
            transcript,
            work_dir,
            yes,
            no_install,
            json,
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
        Command::AuthorAgent { agent } => run_author_agent(agent),
        Command::Version => {
            print_version();
            Ok(0)
        }
        Command::Doctor => doctor(),
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
    agent: Option<AgentKind>,
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
        let command = match (req.agent, req.author_cmd.as_deref()) {
            (Some(AgentKind::Claude), None) => PortAuthor::Claude,
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
        return Err(format!(
            "the component imports unsupported host APIs: {}\n\
             \n\
             These come from something linked into the component, not from a call \
             the code makes directly. Check the dependencies in candidate/Cargo.toml \
             first: a crate that needs `std` brings all of this with it, and no \
             rewriting of the app's own code removes it. `image` is the usual \
             culprit -- `zune-png` and `zune-jpeg` (with `default-features = false`, \
             plus `zune-core` for `ZCursor`) decode the same formats under `no_std`. \
             If every dependency is clean, it is the app's own code reaching the \
             operating system through std rather than through Krate: `std::fs`, \
             `std::io` (including `println!` and `dbg!`), `std::time`, `std::env`, \
             `std::process`, `std::net`, `std::thread`. In-memory std is fine -- \
             `String`, `format!`, `Vec` and `HashMap` do not leak. Do not reach for \
             `#![no_std]` in a windowed app: the generated bindings need std, so it \
             cannot compile, and the error looks like your own code is at fault.",
            bad.join(", ")
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
    println!("Krate will not upload it for you. To send it:");
    println!("  1. Copy the text above.");
    println!("  2. Open https://github.com/incyashraj/krate/issues/new");
    println!("  3. Paste it, edit out anything you would rather not share, and post.");
    println!();
    println!("Anything you leave in is public on that page, so read it once more first.");

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
            &["run", bundle_str, "--untrusted", "--auto-grant", "--", arg],
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
    native_window: bool,
    insecure_http: bool,
    json: bool,
    dump_caps: bool,
    dump_caps_format: OutputFormat,
    log_grants: Option<PathBuf>,
    log_grants_format: GrantLogFormat,
    test_time_millis: Option<u64>,
    test_locale: Option<String>,
    test_timezone: Option<String>,
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

/// Write a `.krate` bundle from a component and its manifest.
fn pack_bundle(file: &Path, manifest: &Path, output: &Path) -> Result<u8> {
    let assets = manifest
        .parent()
        .map(|parent| parent.join("assets"))
        .filter(|path| path.is_dir());
    let size = krate_bundle::pack_with_assets(manifest, file, assets.as_deref(), output)
        .with_context(|| format!("could not pack {}", output.display()))?;
    println!("wrote {} ({size} bytes)", output.display());
    if let Some(assets) = assets {
        println!("included portable assets from {}", assets.display());
    }
    Ok(0)
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
    let size = krate_bundle::pack_with_assets(
        &packed_manifest,
        &code,
        assets.is_dir().then_some(assets.as_path()),
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
    let transcript = serde_json::json!({
        "schema": "krate.author.v1",
        "request": req.request,
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
/// prompt here — versioned with the tool, not in an external script — and runs
/// the agent headless. Today the one supported agent is Claude Code.
fn run_author_agent(agent: AgentKind) -> Result<u8> {
    let app_dir = std::env::var("KRATE_APP_DIR")
        .context("KRATE_APP_DIR is not set; run this through `krate create --agent`")?;
    let request =
        std::env::var("KRATE_REQUEST").unwrap_or_else(|_| "a small useful app".to_string());
    let kind = std::env::var("KRATE_APP_KIND").unwrap_or_else(|_| "checklist".to_string());

    match agent {
        AgentKind::Claude => run_claude_author(&app_dir, &request, &kind),
    }
}

/// The prompt handed to Claude Code. It points the model at the compiling
/// starter `create` already dropped in the app dir and states the rules, then
/// asks it to make the app match the request. Editing a known-good, rendering,
/// non-hanging base is what makes AI authoring dependable rather than a coin
/// flip: the model adapts real working code instead of writing the strict
/// no_std/`krate:*` discipline from a blank page.
fn claude_author_prompt(app_dir: &str, request: &str, kind: &str) -> String {
    let starter = match kind {
        "voice-prompter" => {
            "a voice prompter GUI that opens a window, displays a script, requests \
microphone access, transcribes each spoken phrase locally with its bundled model, and \
advances when the words match the current line. Preserve its audio.capture calls, \
streaming local speech match, visible listening state, manual controls, and quick \
verification path"
        }
        "word-frequency" => {
            "a command-line word-frequency app that reads only its granted input file \
and prints a report. Preserve its bounded input handling and quick verification path"
        }
        _ => {
            "a checklist GUI that opens a window, shows checkbox rows, lets the user \
add and toggle items, and saves them to its granted folder. Preserve its window, event, \
save, and quick verification paths"
        }
    };
    format!(
        "You are writing a Krate desktop app in Rust, from the user's request.\n\
\n\
A COMPILING, WORKING starter is already in {app_dir} (Cargo.toml, src/lib.rs,\n\
manifest.toml): {starter}. Read it first. It already follows every rule below\n\
and works, so it is the safest base to build from.\n\
\n\
Your job: make the app match the request. Adapt the starter's title, content,\n\
controls, and behavior while keeping every capability the resulting app really\n\
needs in manifest.toml. If the request is genuinely different, rewrite\n\
src/lib.rs following the same structure and rules.\n\
\n\
Request: {request}\n\
\n\
HARD RULES (the starter obeys all of these; do not break them):\n\
- The app is no_std plus alloc. Do not add any std usage.\n\
- Import only from the Krate bindings modules already used by the starter.\n\
  Never import wasi interfaces or std io.\n\
- Build strings with the starter's pure_string and number_string helpers,\n\
  never with the format macro.\n\
- Keep the starter's working host calls and event structure unless the request\n\
  genuinely requires a change.\n\
- The app must still exit promptly when its first argument is the literal\n\
  word quick. The starter already does this; keep it.\n\
\n\
After editing, the app must build, match the request, request only the access it\n\
uses, and exit on quick. Use the Read and Edit (or Write) tools. Do not explain;\n\
just make the app."
    )
}

fn run_claude_author(app_dir: &str, request: &str, kind: &str) -> Result<u8> {
    let prompt = claude_author_prompt(app_dir, request, kind);
    let transcript = Path::new(app_dir).join(".agent-transcript.txt");
    // The template compiles and passes every downstream check by design, so an
    // agent that answers in chat and edits nothing would hand the person a
    // checklist app wearing their request's name. The port pipeline shipped
    // exactly that once; the same snapshot-compare closes it here.
    let starter_lib = fs::read_to_string(Path::new(app_dir).join("src/lib.rs")).unwrap_or_default();
    let file = fs::File::create(&transcript).ok();

    let mut command = ProcessCommand::new("claude");
    command
        .arg("-p")
        .arg(&prompt)
        .arg("--allowed-tools")
        .arg("Read,Edit,Write")
        .arg("--permission-mode")
        .arg("acceptEdits");
    // Send the model's own chatter to the transcript, not the create output.
    if let Some(file) = &file {
        if let Ok(clone) = file.try_clone() {
            command.stdout(std::process::Stdio::from(file.try_clone().unwrap_or(clone)));
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
            "the Claude agent did not finish successfully; see {}",
            transcript.display()
        );
    }
    let lib_after = fs::read_to_string(Path::new(app_dir).join("src/lib.rs")).unwrap_or_default();
    if lib_after == starter_lib {
        anyhow::bail!(
            "the agent finished without changing the app: src/lib.rs is byte-identical \
             to the starter template, so this would package the template as if it were \
             \"{request}\". The agent's transcript is at {} -- it usually means the \
             agent explained the app instead of writing it. Even a request the template \
             already satisfies needs its titles and labels made real.",
            transcript.display()
        );
    }
    Ok(0)
}

fn run_author_command(ctx: AuthorContext<'_>) -> Result<()> {
    use krate_author::{generate, AppKind, AppRequest};

    fs::create_dir_all(ctx.app_dir.join("src"))?;

    // Give the agent a running start rather than a blank page: a compiling
    // starter (the built-in template for this kind) it can edit, and a CONTRACT
    // stating the one hard rule. Both may be overwritten by the agent.
    let mut request = match ctx.kind {
        AppKind::Checklist => AppRequest::checklist(ctx.name),
        AppKind::WordFrequency => AppRequest::word_frequency(ctx.name),
        AppKind::VoicePrompter => AppRequest::voice_prompter(ctx.name),
    };
    request.description = ctx.request.to_string();
    if let Ok(app) = generate(&request, ctx.sdk_prefix) {
        for file in &app.files {
            let dest = ctx.app_dir.join(&file.path);
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&dest, &file.contents);
        }
    }
    fs::write(ctx.app_dir.join("CONTRACT.md"), author_contract(ctx.name))?;

    let shell = author_shell();
    let status = std::process::Command::new(shell)
        .arg("-c")
        .arg(ctx.cmd)
        .env("KRATE_APP_DIR", ctx.app_dir)
        .env("KRATE_APP_NAME", ctx.name)
        .env("KRATE_REQUEST", ctx.request)
        .env("KRATE_APP_KIND", app_kind_name(ctx.kind))
        // The materialized SDK: the agent resolves WIT/bindings from here.
        .env("KRATE_SDK_DIR", ctx.sdk_dir)
        .status()
        .context("run --author-cmd")?;
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
iterators do not reach the operating system and do not leak. Keep\n\
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
    let out = ProcessCommand::new("rustup")
        .args(["which", "cargo"])
        .output()
        .ok()?;
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
        phase3_ui_mode: if request.native_window {
            krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype
        } else {
            krate_runtime::phase3_ui::Phase3HostUiMode::HeadlessDraft
        },
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
        native_window: true,
        json: false,
        dump_caps: false,
        dump_caps_format: OutputFormat::Text,
        log_grants: None,
        log_grants_format: GrantLogFormat::Text,
        test_time_millis: None,
        test_locale: None,
        test_timezone: None,
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
    println!("state dir       {}", krate_home().display());
    Ok(0)
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
fn missing_create_tools() -> Vec<MissingTool> {
    let mut missing = Vec::new();

    let have_cargo = has_tool("cargo", &["--version"]);
    if !have_cargo {
        missing.push(MissingTool {
            what: "Rust (cargo)",
            // rustup is the supported installer; we print its official command
            // and only run it with consent.
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
        format!("{joined} | sh")
    } else {
        joined
    }
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
        let sh = ProcessCommand::new("sh")
            .args(["-s", "--", "-y"])
            .stdin(curl.stdout.context("curl produced no output")?)
            .status()
            .context("run the rustup installer")?;
        sh
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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
        assert!(super::install_command_line(&curl).ends_with("| sh"));

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
    fn claude_gets_the_starter_for_the_requested_app_kind() {
        let voice = claude_author_prompt(
            "/tmp/app",
            "make a voice prompter that follows me",
            "voice-prompter",
        );
        assert!(voice.contains("microphone access"));
        assert!(voice.contains("audio.capture"));
        assert!(voice.contains("streaming local speech match"));
        assert!(!voice.contains("a checklist GUI"));

        let checklist = claude_author_prompt("/tmp/app", "make a grocery list", "checklist");
        assert!(checklist.contains("a checklist GUI"));
        assert!(!checklist.contains("streaming local speech match"));
    }

    #[test]
    fn app_kinds_have_stable_agent_environment_names() {
        assert_eq!(app_kind_name(AppKind::Checklist), "checklist");
        assert_eq!(app_kind_name(AppKind::WordFrequency), "word-frequency");
        assert_eq!(app_kind_name(AppKind::VoicePrompter), "voice-prompter");
    }
}
