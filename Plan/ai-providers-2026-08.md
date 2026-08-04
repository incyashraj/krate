# Connect your AI: the provider seam

Date: 2026-08-04
Workstation: W3 (Connect your AI)
Status: design agreed, stage 1 refactor landed

## Why this exists

Krate can already have an AI write an app for you. It can only be Claude.
`--agent` takes exactly one value, `claude`, and every layer under it is
Claude-shaped: the flags, the streamed event format, the failure text.

That is a product problem before it is a code problem. "Krate works with
Claude" asks a person to have the same AI subscription we happen to have used.
"Krate works with your AI" asks nothing. The second sentence is the one worth
being able to say, and today the code cannot back it up.

This document is the plan to make it true, and the record of what shipped.

## What the code looks like today

All references are to `crates/cli/src/main.rs` before this change:

- `enum AgentKind` (line 459) has one variant, `Claude`. It is a clap
  `ValueEnum`, so the accepted spelling of `--agent` and the set of supported
  providers are the same thing. Adding a provider means editing the enum, and
  an unknown value gets clap's generic "invalid value" error.
- `AgentKind::author_command()` (line 474) turns the choice into an
  `--author-cmd` string. It invokes this binary by absolute path -- see the
  comment at line 475 and the test named below -- and appends the hidden
  `author-agent claude` subcommand.
- `run_author_agent()` (line 2803) reads `KRATE_APP_DIR` and `KRATE_REQUEST`
  from the environment and matches on the agent.
- `run_claude_author()` (line 2865) is the real driver: it builds the prompt,
  spawns `claude` with `-p`, `--allowed-tools`, `--output-format stream-json`,
  `--verbose`, and `--permission-mode bypassPermissions`, closes stdin, strips
  inherited `CLAUDE_CODE_*` variables, streams stdout, enforces a timeout, and
  then re-runs the `check-app` oracle itself.
- `author_progress_line()` (line 3044) parses one line of Claude Code's
  streamed JSON into a plain-English sentence.
- The port path repeats the pattern: `PortAuthor` (line 893),
  `run_claude_port()` (line 1936), `run_claude_port_repair()` (line 1980).

Two things in that list are Claude-specific and two are not. The prompt, the
timeout, the stdin handling, the empty-skeleton check, and the `check-app`
verdict are Krate's authoring policy and should be identical for every
provider. Only the argument list and the event parsing actually differ. The
current code does not draw that line, so every provider added by copy-paste
would duplicate the policy and let it drift.

## The provider trait

The seam splits exactly where the differences are. A provider supplies its
name, how to find it, how to invoke it, and how to read what it says. Krate
keeps everything else.

```rust
/// One AI coding agent Krate knows how to drive.
///
/// A provider answers four questions and nothing more: what it is called, how
/// to check it is installed, how to invoke it headlessly, and how to turn its
/// output into progress the person watching can read. The authoring policy --
/// the prompt, the timeout, the empty-skeleton guard, the `check-app` verdict
/// -- belongs to Krate and is identical for every provider.
trait AgentProvider {
    /// The value accepted after `--agent`, e.g. "claude". Lowercase, stable.
    fn name(&self) -> &'static str;

    /// One line for the provider listing in `--help` and in errors.
    fn description(&self) -> &'static str;

    /// The executable Krate spawns, looked up on PATH.
    fn program(&self) -> &'static str;

    /// What to tell someone whose machine does not have this CLI.
    fn install_hint(&self) -> &'static str;

    /// Arguments for one headless authoring run of `prompt`.
    fn author_args(&self, prompt: &str) -> Vec<String>;

    /// Provider-specific spawn setup: stdin, environment, working directory.
    /// The default closes stdin, which every provider needs.
    fn configure(&self, command: &mut std::process::Command) {
        command.stdin(std::process::Stdio::null());
    }

    /// Turn one line of streamed output into a plain-English progress line,
    /// or `None` for lines a person does not care about. Best-effort: an
    /// unrecognized shape prints nothing and the transcript keeps the raw line.
    fn progress_line(&self, line: &str) -> Option<String>;

    /// Whether the run failed, given the exit status. Exit code alone is the
    /// default; a provider that reports failure in its final event overrides.
    fn failed(&self, status: &std::process::ExitStatus) -> bool {
        !status.success()
    }
}
```

Availability is deliberately not a trait method with custom logic. It is one
shared function -- look up `program()` on PATH -- because "is this CLI
installed" has exactly one correct answer and a provider that got creative
about it would only be able to get it wrong.

`failed()` has a default because for three of the four providers the exit code
is the truth. Cursor is the reason it is overridable at all: its terminal
`result` event carries `is_error`, and a provider that wants to read that
should be able to.

## Providers

Every flag below was checked against the vendor's own documentation in August
2026. Where I could not confirm something I say so rather than inventing it.

### Claude Code -- supported today

- Binary: `claude`
- Invocation: `claude -p <prompt> --allowed-tools Read,Edit,Write,Bash
  --output-format stream-json --verbose --permission-mode bypassPermissions`
- Progress: streamed JSON, one object per line;
  `message.content[].type == "tool_use"` carries `name` and `input`.
- Auth: `claude` is already signed in, or `ANTHROPIC_API_KEY`.
- Note: Krate strips inherited `CLAUDE_CODE_*` variables so a nested run starts
  clean. This is the one genuinely provider-specific spawn quirk we have found,
  and it is why `configure()` exists on the trait.

This is the code that already works. It moved behind the trait unchanged.

### OpenAI Codex CLI -- verified, not yet implemented

- Binary: `codex`, subcommand `exec`
- Invocation: `codex exec --json --sandbox workspace-write <prompt>`
- `--sandbox workspace-write` is what lets the agent edit files and run the
  build. `--full-auto` is deprecated in favour of it and now prints a warning.
  `--sandbox danger-full-access` exists and we should not use it.
- Progress: `--json` streams JSON Lines. Events carry a `type`:
  `thread.started`, `turn.started`, `turn.completed`, `item.started`,
  `item.completed`, `error`. Command executions and file changes arrive as
  `item.*` events. This is a different shape from Claude's, which is the entire
  reason `progress_line()` is per-provider.
- Auth: saved credentials from `codex login`, or `CODEX_API_KEY` set inline.
  Note it is `CODEX_API_KEY`, not `OPENAI_API_KEY`.
- Caveat: `codex exec` expects to run inside a git repository unless given
  `--skip-git-repo-check`. Krate authors into a temp directory that is not a
  repo, so this flag is very likely required. I have not run it to confirm.

### Gemini CLI -- verified, not yet implemented

- Binary: `gemini`
- Invocation: `gemini --prompt <prompt> --approval-mode yolo --output-format
  stream-json`
- Headless is triggered by `-p`/`--prompt` or by a non-TTY. Tool calls must be
  auto-approved or a headless run blocks forever with nobody to answer the
  prompt -- the same trap documented for Claude at line 2897. The modern
  spelling is `--approval-mode yolo`; the older `--yolo` is equivalent.
- Progress: `--output-format json` returns a single object with `response`,
  `stats`, and optional `error`. Streaming JSONL emits `init`, `message`,
  `tool_use`, `tool_result`, `error`, `result`. We want the streaming form, so
  progress lines appear during the run rather than all at the end.
- Auth: Google sign-in, or `GEMINI_API_KEY` from AI Studio.

### Cursor CLI -- verified, with a caveat

- Binary: `cursor-agent`. The docs show it as `agent` in examples; I could not
  confirm from the documentation alone which name the installer puts on PATH.
  Resolve this by installing it before implementing, not by guessing.
- Invocation: `cursor-agent -p <prompt> --output-format stream-json --force
  --trust`
- `--trust` is documented as required in headless mode. `--force` (also spelled
  `--yolo`) allows commands unless explicitly denied.
- Progress: `stream-json` is the closest of any provider to Claude's shape --
  `system`/`init`, `user`, `assistant`, `tool_call` with `subtype`
  `started`/`completed`, and a terminal `result` carrying `is_error`. Plain
  `--output-format json` emits one object at the end with no tool events, so it
  is useless for progress.
- Auth: `cursor-agent login`, `CURSOR_API_KEY`, or `--api-key`.
- Caveat: there is an open community report of `-p` headless mode hanging and
  never returning. Krate's timeout already bounds that, but it means Cursor
  should be marked experimental until someone runs it end to end.

### Generic `--author-cmd` -- supported today, stays

Any command that writes `Cargo.toml`, `src/lib.rs`, and `manifest.toml` into
`KRATE_APP_DIR`. This is the escape hatch for a provider we have not added, a
local model, or a person's own script. It has no progress parsing and no
install hint, by design -- it is the seam one level down from `--agent`, and it
should stay unopinionated. `--author-cmd` keeps winning over `--agent` when
both are given.

### Not covered

GitHub Copilot CLI, Aider, and Amp are plausible additions. I did not research
their headless interfaces for this document, so I make no claim about them.
They cost one file each once stage 1 is in, which is the point.

## Connection models

Three ways a person could connect their AI. They are not equally good.

**(a) Local CLI, already signed in.** What we do today. Krate spawns a binary
that is on PATH and already authenticated. Krate never sees a credential,
never stores one, and never transmits one. Every provider above supports this.

**(b) The person's own API key, stored locally.** Krate would read a key from
the environment and pass it to the provider's CLI. This is worth supporting
only as a pass-through: we read `CODEX_API_KEY` or `GEMINI_API_KEY` from the
environment the person already set and hand it to the child process. Krate
writing a key to its own config file is a step down in security from the
provider's own credential store, and buys nothing.

**(c) OAuth to the provider.** A real "connect your account" button. This
requires being a registered OAuth client with each vendor, handling refresh
tokens, and holding a credential Krate is responsible for. That is a hosted
service with a security burden, and it is not where the value is. Every
provider already ships a working `login` command.

**Recommendation: (a) is the product. (b) is a pass-through we do not own.
(c) is not worth building.**

The line we do not cross, stated plainly so it cannot be quietly eroded:

- Krate must never store a person's API key on a Krate server.
- Krate must never send a person's code, prompt, or app to a Krate server as
  part of authoring.
- The only network traffic authoring causes is between the person's own machine
  and the AI vendor they chose, under their own account.

This is not only ethics. It is the reason "connect your AI" is cheap for us to
ship: we are not in the path, so we have no keys to leak and no bills to pay.

## Where the build runs

**Builds stay on the person's machine. I agree with this and would argue for it
even if asked to do otherwise.**

The reasoning:

Compiling AI-generated Rust is executing untrusted code. `build.rs` runs
arbitrary code at compile time, as do procedural macros. A build service would
be an endpoint where anyone can run whatever they like on our infrastructure by
describing an app -- an attacker would not even need to bypass anything,
because running their code is the advertised feature. Making that safe means
real sandboxing, egress control, and per-tenant isolation. That is a serious
piece of infrastructure to own, and it is on the critical path of every single
`krate create`.

It also contradicts the pitch. Krate's argument is that code runs safely on
your own machine, inside a capability sandbox you can inspect. Shipping a
product whose own authoring step sends your code to our servers to be compiled
undercuts the thing we are selling. The sandbox story and the build story have
to agree.

And the practical version: local builds cost us nothing, scale perfectly, and
work offline once the toolchain is installed. A build farm is a bill that grows
with adoption and an outage that takes authoring down for everyone.

The honest cost of this position is that the person needs a Rust toolchain.
That is a real onboarding cost, and it is W1's problem to make small -- Krate
already offers to install what is missing. It is the right cost to pay.

## Staged plan

**Stage 1 -- the seam. Shipped in this change.**
Introduce the trait, move Claude behind it with no behaviour change, replace
the clap `ValueEnum` with a registry lookup, add missing-CLI detection with an
install hint, and keep `--author-cmd` working. Unlocks: a new provider is a new
file, and the next stage is additive rather than a refactor.

**Stage 2 -- a second provider.** Add Codex, since it is the most-used
alternative and its flags are fully verified. The first non-Claude provider is
what proves the seam is real rather than theoretical; anything the trait got
wrong shows up here, while there is still only one implementation to fix.

**Stage 3 -- Gemini and Cursor.** Two more files. Cursor lands marked
experimental until its headless mode is confirmed working end to end and its
binary name is checked on a real install.

**Stage 4 -- `krate doctor` reports providers.** Show which AI CLIs are
installed and which are signed in, so "connect your AI" is a thing a person can
see the state of rather than discover by failing. Small, and it is the moment
the feature becomes visible in the product.

Stage 1 is not blocked by W1. Stages 2 onward should follow W1's reliability
work, because a second provider multiplies whatever authoring flakiness
already exists rather than fixing it.
