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
    Config, RunOutcome, Runtime, RuntimeError, DEFAULT_HTTP_TIMEOUT_MILLIS,
    DEFAULT_MAX_HTTP_RESPONSE_BYTES,
};
use serde::Serialize;

mod mcp;
mod sdk;

const MAX_PHASE2_ARGS_RAW_BYTES: usize = 64 * 1024;
const MAX_PHASE2_ARG_COUNT: usize = 1024;
const PHASE3_GUI_UNIMPLEMENTED_EXIT: u8 = 6;

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

        /// A command that writes the app source instead of the built-in
        /// generator — this is where an AI agent plugs in. It is handed
        /// `KRATE_APP_DIR`, `KRATE_APP_NAME`, and `KRATE_REQUEST` and must
        /// write Cargo.toml, src/lib.rs, and manifest.toml into the app dir.
        #[arg(long)]
        author_cmd: Option<String>,

        /// Which built-in template to use when no --author-cmd is given.
        /// Inferred from the request when omitted.
        #[arg(long, value_enum)]
        kind: Option<CreateKind>,

        /// Kebab-case name for the generated app. Defaults per kind.
        #[arg(long)]
        name: Option<String>,

        /// Where to write the authoring transcript (JSON). Defaults to the
        /// output path with a `.transcript.json` suffix.
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

    /// Run the Krate MCP server so an AI agent can execute components under the
    /// capability sandbox. Speaks JSON-RPC 2.0 over stdio; wire an agent at it
    /// with e.g. `claude mcp add krate -- krate mcp`.
    Mcp,
}

/// The built-in app templates `krate create` can generate.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CreateKind {
    /// A CLI app: read a file and print its most frequent words.
    WordFrequency,
    /// A GUI app: a checklist with checkboxes that saves locally.
    Checklist,
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
            author_cmd,
            kind,
            name,
            transcript,
            work_dir,
            yes,
            no_install,
            json,
        }),
        Command::Mcp => mcp::serve().map(|()| 0),
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
    let size = krate_bundle::pack(manifest, file, output)
        .with_context(|| format!("could not pack {}", output.display()))?;
    println!("wrote {} ({size} bytes)", output.display());
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
        None => AppKind::infer(&req.request),
    };
    let default_name = match kind {
        AppKind::Checklist => "checklist",
        AppKind::WordFrequency => "word-count",
    };
    let name = req.name.clone().unwrap_or_else(|| default_name.to_string());

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
    let size = krate_bundle::pack(&packed_manifest, &code, &req.output)
        .with_context(|| format!("pack {}", req.output.display()))?;
    steps.push(serde_json::json!({"step": "pack", "detail": format!("{} bytes", size)}));

    // Step 5: verify the permission wall by running the packed bundle with all
    // grants (must succeed) and without the gating capability (must refuse).
    if !req.json {
        println!("==> verifying the permission wall");
    }
    let gating = gating_capability(&manifest);
    let verify_dir = tempfile::tempdir().context("verify dir")?;
    prepare_verify_dir(verify_dir.path(), &manifest)?;
    let bundle_abs = fs::canonicalize(&req.output)?;

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
            "quick",
        ],
    )?;
    if allow_exit != 0 {
        anyhow::bail!(
            "the packed app failed to run with all grants (exit {allow_exit}); \
             exit 4 means it exhausted its fuel budget (a runaway or infinite loop)"
        );
    }

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
    deny_args.push("quick".to_string());
    let deny_arg_refs: Vec<&str> = deny_args.iter().map(String::as_str).collect();
    let deny_exit = run_self(verify_dir.path(), &deny_arg_refs)?;
    if deny_exit != 5 {
        anyhow::bail!("withholding {gating} should refuse with exit 5, got {deny_exit}");
    }
    steps.push(serde_json::json!({
        "step": "verify",
        "detail": format!("runs with all grants (exit 0), refuses without {gating} (exit 5)")
    }));

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

fn run_author_command(ctx: AuthorContext<'_>) -> Result<()> {
    use krate_author::{generate, AppKind, AppRequest};

    fs::create_dir_all(ctx.app_dir.join("src"))?;

    // Give the agent a running start rather than a blank page: a compiling
    // starter (the built-in template for this kind) it can edit, and a CONTRACT
    // stating the one hard rule. Both may be overwritten by the agent.
    let mut request = match ctx.kind {
        AppKind::Checklist => AppRequest::checklist(ctx.name),
        AppKind::WordFrequency => AppRequest::word_frequency(ctx.name),
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

    let shell = if cfg!(windows) { "bash" } else { "sh" };
    let status = std::process::Command::new(shell)
        .arg("-c")
        .arg(ctx.cmd)
        .env("KRATE_APP_DIR", ctx.app_dir)
        .env("KRATE_APP_NAME", ctx.name)
        .env("KRATE_REQUEST", ctx.request)
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

/// The briefing dropped into the app dir for an agent. States the one hard rule
/// and how the app is checked, so the agent gets it right the first time.
fn author_contract(name: &str) -> String {
    format!(
        "# Krate app contract for `{name}`\n\
\n\
You are writing a Krate guest app in Rust. Three files must exist in this\n\
directory when you finish: `Cargo.toml`, `src/lib.rs`, `manifest.toml`.\n\
A compiling starter for each is already here — edit it to fit the request in\n\
the `KRATE_REQUEST` environment variable.\n\
\n\
## The one hard rule\n\
A Krate component may import ONLY `krate:*` interfaces. Ordinary std code\n\
breaks this and the app will be rejected: a growable `Vec`'s reallocation,\n\
`HashMap`, `format!`, and the `args::first` / `read_to_string` SDK helpers all\n\
pull `wasi:*` imports in, which cannot be stripped. So:\n\
- use fixed-capacity `[u8; N]` buffers and `.get()` / `.get_mut()` (never index),\n\
- read args with `args::raw()` and split by hand,\n\
- build strings by hand instead of `format!`.\n\
The in-repo samples under `$KRATE_SDK_DIR` follow this discipline; copy it.\n\
\n\
## The manifest\n\
Declare only the capabilities the app uses. Mark the one that gates it\n\
(`fs.write` for a saving app) `required = true`.\n\
\n\
## What happens next\n\
`krate create` builds what you write, checks it imports only `krate:*`, packs\n\
it, and verifies its permission wall. If you reach for something unsafe, the\n\
import check stops it here — it never ships. The SDK (WIT + Rust bindings) is\n\
at `$KRATE_SDK_DIR`.\n"
    )
}

/// Build the app dir to a wasm component with cargo-component, returning the
/// path to the produced wasm.
fn build_component(app_dir: &Path) -> Result<PathBuf> {
    let status = std::process::Command::new("cargo-component")
        .arg("build")
        .arg("--release")
        .current_dir(app_dir)
        .status()
        .context("run cargo-component (is it installed? `cargo install cargo-component`)")?;
    if !status.success() {
        anyhow::bail!("cargo-component build failed");
    }
    let release = app_dir.join("target/wasm32-wasip1/release");
    for entry in fs::read_dir(&release).with_context(|| format!("read {}", release.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("wasm") {
            return Ok(path);
        }
    }
    anyhow::bail!("no wasm produced in {}", release.display())
}

/// The capability whose grant gates the app — the required one the verify step
/// withholds to prove the wall. Prefers fs.write, then fs.read, else the first
/// required capability.
fn gating_capability(manifest: &krate_manifest::Manifest) -> String {
    let required: Vec<String> = manifest
        .capabilities
        .iter()
        .filter(|c| c.required)
        .map(|c| c.cap.clone())
        .collect();
    for prefer in ["fs.write", "fs.read"] {
        if let Some(cap) = required.iter().find(|c| c.starts_with(prefer)) {
            return cap.clone();
        }
    }
    required
        .into_iter()
        .find(|c| !c.starts_with("io.") && !c.starts_with("ui.window"))
        .unwrap_or_else(|| "fs.write".to_string())
}

/// Create the data directories the app expects under the verify dir, so a
/// granted run has somewhere to write.
fn prepare_verify_dir(dir: &Path, manifest: &krate_manifest::Manifest) -> Result<()> {
    for cap in manifest.capabilities.iter() {
        let name = cap.cap.clone();
        if let Some(rest) = name
            .strip_prefix("fs.read:")
            .or_else(|| name.strip_prefix("fs.write:"))
        {
            // Turn "./checklist/**" into the directory "checklist".
            let trimmed = rest.trim_start_matches("./");
            if let Some(first) = trimmed.split('/').next() {
                if !first.is_empty() && first != "**" {
                    let _ = fs::create_dir_all(dir.join(first));
                }
            }
        }
    }
    Ok(())
}

/// Re-invoke this same `krate` binary in `dir` with `args`, returning its exit
/// code. Used to verify a packed bundle in isolation.
fn run_self(dir: &Path, args: &[&str]) -> Result<i32> {
    let exe = std::env::current_exe().context("locate self")?;
    // Capture the child's output rather than inherit it: the verified app's own
    // stdout (e.g. its "saved" line) is noise to the create caller, and would
    // corrupt the single-object stream under --json. Only the exit code matters.
    let output = std::process::Command::new(exe)
        .args(args)
        .current_dir(dir)
        .output()
        .context("re-invoke krate for verification")?;
    Ok(output.status.code().unwrap_or(-1))
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
    let (file, bundle_manifest, _bundle) =
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

    if request.dump_caps {
        print_effective_capabilities(&request.file, manifest, &policy, request.dump_caps_format)?;
        return Ok(0);
    }

    if let Some(manifest) = manifest {
        let world = manifest.app_world()?;
        if !world.is_runnable() && !matches!(world, AppWorld::Phase3Gui) {
            eprintln!("unsupported app world for run: {}", world.world_name());
            return Ok(PHASE3_GUI_UNIMPLEMENTED_EXIT);
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
        app_args: request.app_args,
        max_http_response_bytes: request.max_http_response_bytes,
        default_http_timeout_millis: match request.http_timeout_millis {
            0 => None,
            millis => Some(millis),
        },
        sandbox_root: request.sandbox_root,
        phase3_ui_mode: if request.native_window {
            krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype
        } else {
            krate_runtime::phase3_ui::Phase3HostUiMode::HeadlessDraft
        },
    };
    let runtime = Runtime::new(&config)?;

    if request.json {
        let started = std::time::Instant::now();
        let (exit, stdout, cli_code) = match runtime.run_file_captured(&request.file, &config) {
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

    match runtime.run_file(&request.file, &config) {
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
    io::stdin().read_line(&mut input)?;
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
        // No native window on this platform. If there is a terminal to answer
        // in, ask there. Otherwise (a double-clicked bundle on Linux with no
        // controlling terminal) try a graphical dialog, and if none is present
        // leave a clear message instead of a silent failure.
        ConsentOutcome::Unsupported => {
            if io::stdin().is_terminal() {
                prompt_for_session_grants(manifest, policy)
            } else {
                consent_without_terminal(manifest, policy, &consent_caps)
            }
        }
    }
}

/// The consent path when `--consent` is asked for but there is no terminal to
/// prompt in — a double-clicked `.krate`. On Linux this tries a graphical
/// dialog; on other platforms it falls back to the terminal prompt (which is a
/// no-op read that refuses cleanly if nothing answers).
fn consent_without_terminal(
    manifest: &Manifest,
    policy: &SessionPolicy,
    consent_caps: &[Capability],
) -> Result<SessionPolicy> {
    #[cfg(target_os = "linux")]
    {
        match linux_graphical_consent(manifest, consent_caps)? {
            Some(selected) => {
                let grants = policy.grants().iter().cloned().chain(selected);
                Ok(SessionPolicy::from_grants(grants))
            }
            // A dialog ran and the user declined, or none was available: leave
            // the policy unchanged so the run is refused downstream.
            None => Ok(policy.clone()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = consent_caps;
        prompt_for_session_grants(manifest, policy)
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
) -> Result<()> {
    if format == OutputFormat::Json {
        let dump = RunCapsDump {
            wasm: wasm_file.display().to_string(),
            app: manifest.map(RunCapsApp::from_manifest),
            capabilities: policy.grants().iter().map(ToString::to_string).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&dump)?);
        return Ok(());
    }

    println!("Effective capabilities");
    for cap in policy.grants() {
        println!("  - {cap}");
    }

    Ok(())
}

#[derive(Debug, Serialize)]
struct RunCapsDump {
    wasm: String,
    app: Option<RunCapsApp>,
    capabilities: Vec<String>,
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
    println!("krate   {}", env!("CARGO_PKG_VERSION"));
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

    // Only meaningful when rustup is present; if rustup is missing the Rust row
    // above already covers it.
    if have_cargo && has_rust_target(CREATE_WASM_TARGET) == Some(false) {
        missing.push(MissingTool {
            what: CREATE_WASM_TARGET,
            install_cmd: vec![
                "rustup".into(),
                "target".into(),
                "add".into(),
                CREATE_WASM_TARGET.into(),
            ],
            note: "the WebAssembly target Krate apps compile to",
        });
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

    eprintln!("Krate needs a few build tools to create an app, and some are missing:");
    for tool in &missing {
        eprintln!("  - {} ({})", tool.what, tool.note);
    }
    eprintln!();

    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let may_install = !no_install && (assume_yes || interactive);

    if !may_install {
        eprintln!("Install them, then run `krate create` again:");
        for tool in &missing {
            eprintln!("  {}", tool.install_cmd.join(" "));
        }
        eprintln!();
        eprintln!("Or check your setup any time with `krate doctor`.");
        anyhow::bail!("missing build tools; see the commands above");
    }

    if !assume_yes {
        eprint!("Install them now? [Y/n] ");
        io::stderr().flush().ok();
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_lowercase();
        if !(answer.is_empty() || answer == "y" || answer == "yes") {
            eprintln!("Not installing. To do it yourself:");
            for tool in &missing {
                eprintln!("  {}", tool.install_cmd.join(" "));
            }
            anyhow::bail!("build tools are required to create an app");
        }
    }

    for tool in &missing {
        eprintln!("==> installing {}", tool.what);
        eprintln!("    {}", tool.install_cmd.join(" "));
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

fn krate_home() -> PathBuf {
    home_dir()
        .map(|home| home.join(".krate"))
        .unwrap_or_else(|| PathBuf::from(".krate"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod create_tests {
    use super::{has_tool, human_label, toml_path, validate_create_request};
    use krate_manifest::Capability;
    use std::path::Path;

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
}
