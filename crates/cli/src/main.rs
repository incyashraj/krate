use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use layer36_manifest::Manifest;
use layer36_policy::resolve_session_policy;
use layer36_runtime::{Config, RunOutcome, Runtime, RuntimeError};

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

        /// Path to a Phase 2 manifest.toml. If omitted, Layer36 checks next to the .wasm file.
        #[arg(long)]
        manifest: Option<PathBuf>,

        /// Grant a capability for this run session. Repeat for multiple grants.
        #[arg(long, value_name = "CAP")]
        grant: Vec<String>,

        /// Grant every capability declared in the manifest for this run session.
        #[arg(long)]
        auto_grant: bool,
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
    },
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
            manifest,
            grant,
            auto_grant,
        } => run_component(file, fuel, mem_limit, manifest, grant, auto_grant),
        Command::Version => {
            print_version();
            Ok(0)
        }
        Command::Doctor => doctor(),
        Command::Manifest { command } => match command {
            ManifestCommand::Check { file } => check_manifest(&file),
        },
    }
}

fn run_component(
    file: PathBuf,
    fuel: Option<u64>,
    mem_limit: u64,
    manifest_path: Option<PathBuf>,
    grants: Vec<String>,
    auto_grant: bool,
) -> Result<u8> {
    if !file.exists() {
        anyhow::bail!("input file does not exist: {}", file.display());
    }

    let manifest = load_run_manifest(&file, manifest_path.as_deref())?;
    let policy = resolve_session_policy(manifest.as_ref(), &grants, auto_grant)?;

    if let Some(manifest) = &manifest {
        let missing = policy.missing_required_for_manifest(manifest)?;
        if !missing.is_empty() {
            eprintln!("permission denied: missing required capabilities");
            for cap in missing {
                eprintln!("  - {cap}");
            }
            return Ok(5);
        }
    }

    let config = Config {
        fuel,
        memory_bytes: mem_limit
            .checked_mul(1024 * 1024)
            .context("memory limit is too large")?,
        session_policy: policy,
    };
    let runtime = Runtime::new(&config)?;

    match runtime.run_file(&file, &config) {
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

fn load_run_manifest(file: &Path, manifest_path: Option<&Path>) -> Result<Option<Manifest>> {
    if let Some(path) = manifest_path {
        return Ok(Some(Manifest::parse_file(path)?));
    }

    let Some(parent) = file.parent() else {
        return Ok(None);
    };

    let candidate = parent.join("manifest.toml");
    if candidate.exists() {
        Ok(Some(Manifest::parse_file(candidate)?))
    } else {
        Ok(None)
    }
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
    print_tool_status("cargo-component", &["--version"]);
    print_target_status("wasm32-wasip1")?;
    print_target_status("wasm32-wasip2")?;
    println!("state dir       {}", layer36_home().display());
    Ok(0)
}

fn check_manifest(file: &Path) -> Result<u8> {
    let manifest = Manifest::parse_file(file)?;
    let declared_caps = manifest.declared_capabilities()?;
    let required_caps = manifest.required_capabilities()?;

    println!("Manifest OK");
    println!("app id          {}", manifest.app.id);
    println!("app name        {}", manifest.app.name);
    println!("entry           {}", manifest.app.entry.display());
    println!("world           {}", manifest.app.world);
    println!("capabilities    {}", declared_caps.len());
    println!("required caps   {}", required_caps.len());

    Ok(0)
}

fn print_tool_status(program: &str, args: &[&str]) {
    let command = resolve_tool(program).unwrap_or_else(|| PathBuf::from(program));
    match ProcessCommand::new(command).args(args).output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("{program:<15} {}", version.trim());
        }
        _ => println!("{program:<15} missing"),
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
