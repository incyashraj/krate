use std::fs::OpenOptions;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use layer36_manifest::{
    supported_capability_specs, App, Capability, CapabilityRequest, Manifest, PHASE2_CLI_WORLD,
};
use layer36_policy::{resolve_session_policy, SessionPolicy};
use layer36_runtime::{
    Config, RunOutcome, Runtime, RuntimeError, DEFAULT_HTTP_TIMEOUT_MILLIS,
    DEFAULT_MAX_HTTP_RESPONSE_BYTES,
};
use serde::Serialize;

const MAX_PHASE2_ARGS_RAW_BYTES: usize = 64 * 1024;
const MAX_PHASE2_ARG_COUNT: usize = 1024;

#[derive(Debug, Parser)]
#[command(
    name = "layer36",
    version,
    about = "Layer36: write once, run on everything."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a WebAssembly component through the Layer36 runtime.
    Run {
        /// Path to the .wasm component.
        file: PathBuf,

        /// Max fuel units to allow. Omit for unlimited.
        #[arg(long)]
        fuel: Option<u64>,

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

        /// Path to a Phase 2 manifest.toml. If omitted, Layer36 checks next to the .wasm file.
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

        /// Arguments passed to the Layer36 app. Put them after `--`.
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
            tracing_subscriber::EnvFilter::try_from_env("LAYER36_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .without_time()
        .init();

    match run() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            file,
            fuel,
            mem_limit,
            max_http_response_bytes,
            http_timeout_millis,
            sandbox_root,
            manifest,
            grant,
            auto_grant,
            prompt,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time,
            test_locale,
            test_timezone,
            app_args,
        } => run_component(RunRequest {
            file,
            fuel,
            mem_limit,
            max_http_response_bytes,
            http_timeout_millis,
            sandbox_root,
            manifest_path: manifest,
            grants: grant,
            auto_grant,
            prompt,
            dump_caps,
            dump_caps_format,
            log_grants,
            log_grants_format,
            test_time_millis: test_time,
            test_locale,
            test_timezone,
            app_args,
        }),
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

fn run_component(request: RunRequest) -> Result<u8> {
    if !request.file.exists() {
        anyhow::bail!("input file does not exist: {}", request.file.display());
    }

    validate_app_args(&request.app_args)?;

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
        let can_prompt = request.prompt || io::stdin().is_terminal();
        let missing = policy.missing_required_for_manifest(manifest)?;
        if !missing.is_empty() && can_prompt && !request.auto_grant {
            policy = prompt_for_session_grants(manifest, &policy)?;
        }

        let missing = policy.missing_required_for_manifest(manifest)?;
        if !missing.is_empty() {
            eprintln!("permission denied: missing required capabilities");
            for cap in missing {
                eprintln!("  - {cap}");
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
    };
    let runtime = Runtime::new(&config)?;

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
    eprintln!("Requests the following capabilities:");
    for (index, cap) in prompt_caps.iter().enumerate() {
        eprintln!("  [{}] {cap}", index + 1);
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
            event: "layer36.grants",
            wasm: wasm_file.display().to_string(),
            app: manifest.map(RunCapsApp::from_manifest),
            capabilities: policy.grants().iter().map(ToString::to_string).collect(),
        };
        serde_json::to_writer(&mut file, &record)?;
        writeln!(file)?;
        return Ok(());
    }

    writeln!(file, "Layer36 grant log")?;
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
    println!("layer36   {}", env!("CARGO_PKG_VERSION"));
    println!("wasmtime  43.0.2");
    println!("rustc     {}", env!("LAYER36_RUSTC_VERSION"));
    println!("commit    {}", env!("LAYER36_GIT_SHA"));
}

fn doctor() -> Result<u8> {
    println!("Layer36 doctor");
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
    println!("state dir       {}", layer36_home().display());
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
}

impl ManifestAppExplanation {
    fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            id: manifest.app.id.clone(),
            name: manifest.app.name.clone(),
            version: manifest.app.version.clone(),
            entry: manifest.app.entry.display().to_string(),
            world: manifest.app.world.clone(),
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
        .collect::<layer36_manifest::Result<Vec<_>>>()?;

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

    println!("Phase 2 capabilities");
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

fn layer36_home() -> PathBuf {
    home_dir()
        .map(|home| home.join(".layer36"))
        .unwrap_or_else(|| PathBuf::from(".layer36"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
