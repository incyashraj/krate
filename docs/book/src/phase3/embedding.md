# Embedding and Machine-Readable Runs

Krate's wedge is safe execution of software that programs — including AI
agents — produce and run on a user's behalf. That needs two surfaces beyond
the interactive CLI: a library API other programs can embed, and a
machine-readable form of `krate run`.

## The embedding API

`krate_runtime::embed` runs a component inside the capability sandbox with
no terminal: grants are supplied programmatically through `SessionPolicy`,
nothing prompts, and the app's stdout comes back captured next to a
classified exit.

```rust,no_run
use krate_policy::SessionPolicy;
use krate_runtime::{embed, Config};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let component = std::fs::read("app.wasm")?;
    let mut config = Config::default();
    config.session_policy = SessionPolicy::from_cli_grants(&[
        "fs.read:./data/**".to_string(),
    ])?;

    let outcome = embed::run_component(&component, &config)?;
    println!("class: {}", outcome.exit_class().as_str());
    println!("stdout: {}", outcome.stdout_lossy());
    Ok(())
}
```

`EmbedOutcome` reports the exit code, an `EmbedExitClass`
(`success`, `permission-denied`, `app-error`, `limit-exceeded`), the captured
stdout bytes, and the run duration. Runtime-level failures (invalid
component, trap) surface as errors; a capability denial inside the app is a
classified outcome, not an error — policy-aware callers decide what to do
with it.

Grants never come from prompts here. `SessionPolicy::from_cli_grants` parses
explicit capability strings, and `SessionPolicy::allow_all_declared` grants
everything a manifest declares — the embedding caller owns that decision the
way a human owns the terminal prompt.

## `krate run --json` (schema `krate.run.v1`)

With `--json`, the CLI prints exactly one JSON object on stdout describing
the run, and the app's own stdout is captured into that object instead of
streaming. Process exit codes stay identical to the normal mode, so existing
scripts keep working.

```json
{
  "schema": "krate.run.v1",
  "app": {
    "id": "dev.krate.clock",
    "name": "krate-clock",
    "version": "0.1.0-dev",
    "world": "krate:app/cli@0.1.0"
  },
  "capabilities": {
    "granted": ["io.stdout", "time.clock"],
    "denied": []
  },
  "exit": {
    "code": 0,
    "class": "success",
    "message": null
  },
  "duration_ms": 87,
  "stdout": "app=krate-clock\n..."
}
```

Field notes:

- `app` is `null` when no manifest was provided.
- `capabilities.granted` lists the effective session grants, with boundaries
  (`fs.read:data/**`, `net.connect:host:port`).
- `capabilities.denied` is non-empty only when required capabilities were
  missing and the run was refused before the component started; the process
  exits `5` in that case, matching the interactive flow.
- `exit.class` is one of `success`, `permission-denied` (app exit code 5 by
  Krate convention, or a refusal before the run), `app-error`,
  `limit-exceeded`, `invalid-component`, or `trap`. `exit.code` is `null`
  when the runtime stopped the component (`limit-exceeded`,
  `invalid-component`, `trap`).
- `duration_ms` is `null` when the run was refused before starting.
- `stdout` holds the app's captured stdout as lossy UTF-8.

## Current status

Done now:

- `Runtime::run_bytes_captured` / `run_file_captured` capture app stdout for
  embedding callers.
- `krate_runtime::embed::run_component` with `EmbedOutcome` and
  `EmbedExitClass`, doc-tested.
- `krate run --json` emitting `krate.run.v1`, covered by CLI integration
  tests for the success, denied-before-run, and invalid-component paths.

## The MCP server

`krate-mcp-server` (a `crates/tools` binary) exposes one MCP tool,
`run_component`, over newline-delimited JSON-RPC on stdio. Any MCP-capable
agent framework can execute components inside the sandbox without linking
Rust:

```bash
cargo build -p krate-tools --bin krate-mcp-server
claude mcp add krate -- target/debug/krate-mcp-server
```

The tool takes `component_path`, optional `manifest_path`, `grants`,
`auto_grant`, `app_args`, and `sandbox_root`, and returns the
`krate.run.v1` report as the tool result (with `isError` set for anything
but `success`). Denials surface as data: calling without grants returns
`permission-denied` plus the exact missing capabilities, and the same call
with `auto_grant` succeeds — the agent observes the capability wall
directly.

Still pending on this track:

- richer per-call deny logs inside a run (today the denial signal is the
  app's own exit code plus the refusal-before-run path).
