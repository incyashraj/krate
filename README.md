<p align="center">
  <img src="docs/landing/krate-app-icon.png" width="128" alt="Krate logo">
</p>

<h1 align="center">Krate</h1>

<p align="center">
  <strong>Make apps with AI. Share them like documents.</strong>
</p>

<p align="center">
  One app file that opens on Mac, Windows, and Linux.<br>
  The user sees what it wants to access before it runs.
</p>

<p align="center">
  <a href="https://github.com/incyashraj/krate/actions/workflows/ci.yml">
    <img src="https://github.com/incyashraj/krate/actions/workflows/ci.yml/badge.svg" alt="CI status">
  </a>
  <a href="https://github.com/incyashraj/krate/releases">
    <img src="https://img.shields.io/github/v/release/incyashraj/krate?include_prereleases&sort=semver" alt="Latest release">
  </a>
  <a href="LICENSE-MIT">
    <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2563eb" alt="MIT or Apache 2.0 license">
  </a>
</p>

<p align="center">
  <a href="https://incyashraj.github.io/krate/">Website</a>
  ·
  <a href="https://incyashraj.github.io/krate/docs/try-krate-notes.html">Try Notes</a>
  ·
  <a href="https://incyashraj.github.io/krate/docs/pages/make-an-app-with-ai.html">Make an app</a>
  ·
  <a href="https://incyashraj.github.io/krate/docs/quickstart.html">Docs</a>
  ·
  <a href="https://github.com/incyashraj/krate/releases">Releases</a>
  ·
  <a href="https://github.com/incyashraj/krate/discussions">Discussions</a>
</p>

<p align="center">
  <a href="https://incyashraj.github.io/krate/">
    <img src="docs/landing/og.png" width="900" alt="Krate turns an app into one file that opens on Mac, Windows, and Linux">
  </a>
</p>

## What is Krate?

AI can make useful small apps for one person or one team. Sharing those apps is
still hard.

A browser link cannot handle every local use case. A normal desktop app can,
but it is tied to an operating system and may reach more of the computer than
the user expects.

Krate uses a simpler app format:

1. The app and the access it requests go into one `.krate` file.
2. The same file opens on Mac, Windows, and Linux.
3. Krate shows the requested access before the app runs.
4. The app receives only the access the user allows.

```text
request
   ↓
checklist.krate
   ↓
review access
   ↓
open on Mac, Windows, or Linux
```

Krate is open source and currently in pre-alpha.

## Try it

Install the command line tool on macOS or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/incyashraj/krate/main/scripts/install.sh | sh
```

Check the installation:

```bash
krate --version
```

Run the public Notes app:

```bash
krate run \
  https://github.com/incyashraj/krate/releases/download/notes-v0.1.0/notes.krate \
  --native-window \
  --prompt
```

Krate fetches the file, shows what it requests, and runs it with the access you
approve.

For a guided walkthrough, use
[Try Krate Notes](https://incyashraj.github.io/krate/docs/try-krate-notes.html).

### macOS without a terminal

Download the `krate-app` zip for your Mac from the
[current release](https://github.com/incyashraj/krate/releases/tag/v0.1.0-rc3).
Unzip `Krate.app`, then download
[`notes.krate`](https://github.com/incyashraj/krate/releases/download/notes-v0.1.0/notes.krate).

The current app is not signed. On first open:

1. Right-click `Krate.app`.
2. Choose **Open**.
3. Double-click `notes.krate`.

The macOS permission window appears before the Notes app runs.

### Windows

Download the newest `x86_64-pc-windows-msvc.zip` from
[Releases](https://github.com/incyashraj/krate/releases), extract
`krate.exe`, and place its folder on your `PATH`.

Then:

```powershell
krate --version
krate run https://github.com/incyashraj/krate/releases/download/notes-v0.1.0/notes.krate --native-window --prompt
```

Windows file association support is available through
[`scripts/install-krate-desktop.ps1`](scripts/install-krate-desktop.ps1).

### Linux double-click support

After installing `krate`, register `.krate` files for your current user:

```bash
git clone https://github.com/incyashraj/krate
cd krate
scripts/install-krate-desktop.sh
```

You can then open a `.krate` file from your file manager.

## Make an app

The current release includes `krate create`. It turns a supported request into
a complete `.krate` file:

```bash
krate create \
  "Make a checklist app that saves locally" \
  --output checklist.krate
```

Krate writes the app, builds it, checks what system features it uses, packages
it, and confirms that denied access is blocked before writing the file.

The public release has two built-in examples:

- a checklist app with local saving;
- a command line word-frequency app.

Creating an app currently needs Rust, the `wasm32-wasip1` target, and
`cargo-component`. Check your machine with:

```bash
krate doctor
```

The full guide is
[Make and share a Krate app](https://incyashraj.github.io/krate/docs/pages/make-an-app-with-ai.html).

## Let an AI coding agent write the app

The built-in examples prove the complete path, but an AI coding agent can
write the app from your request instead. With Claude Code installed and signed
in:

```bash
krate create \
  "A grocery list app called My Groceries" \
  --agent claude \
  --output groceries.krate
```

Krate hands the request to the agent, then builds, checks, packages, and
verifies exactly as with the built-in path — a broken app is caught, not
shipped. For any other tool, `--author-cmd "<your command>"` is the lower-level
seam; it receives:

| Variable | Meaning |
| --- | --- |
| `KRATE_REQUEST` | The user's request |
| `KRATE_APP_NAME` | The generated app name |
| `KRATE_APP_DIR` | The folder where the agent writes the app |

Everything after authoring stays the same. Krate builds the result, refuses
imports outside the Krate interfaces, packages the app, and verifies its
requested access.

The public release can also return output that AI tools and scripts can read:

```bash
krate run app.krate --json --auto-grant
```

The output follows the `krate.run.v1` schema.

## How a `.krate` file works

A `.krate` file contains:

- a WebAssembly component;
- the app name and version;
- the access the app requests;
- the reason for each request.

Opening the file does not grant access by itself. Krate creates a session from
the user's decision and connects only the approved operations to the computer.

Examples:

- `fs.read:notes/**` can read only inside the `notes` folder;
- `fs.write:checklist/**` can write only inside the `checklist` folder;
- no network request works unless network access was declared and granted;
- a downloaded app receives no extra access because it came from a URL.

Inspect an app without running it:

```bash
krate run app.krate --dump-caps
```

Run it and review each request:

```bash
krate run app.krate --prompt
```

## What works today

| Capability | Current state |
| --- | --- |
| One `.krate` file | Working |
| Mac, Windows, and Linux execution | Working |
| Desktop windows on all three systems | Working |
| Text input, editing, selection, and local saving | Working |
| File and network capabilities | Working |
| Permission checks before app execution | Working |
| Run a `.krate` file from an HTTPS URL | Working |
| Create a supported app from a request | Working |
| Let an external AI command author the app | Working |
| JSON output for agents and scripts | Working |
| Convert an existing native app automatically | Not available yet |
| Public app cloud, discovery, identity, and updates | Planned |

The public CI runs the project on Linux, macOS, and Windows. See
[CI](https://github.com/incyashraj/krate/actions/workflows/ci.yml) and the
[current status](STATUS.md) for exact evidence.

## Current platform details

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Runs `.krate` apps | Yes | Yes | Yes |
| Opens desktop windows | Yes | Yes | Yes |
| Notes editing and saving | Yes | Yes | Yes |
| `.krate` file registration | `Krate.app` | Registration script | Registration script |
| Permission review in `v0.1.0-rc3` | Native window | Terminal | Terminal |

The current macOS path uses AppKit controls. Windows and Linux use Krate's
drawn widget path inside a native window.

Development has already improved the Linux no-terminal permission path, but
that change is not part of `v0.1.0-rc3`. Release claims in this README describe
the public release unless a section clearly says otherwise.

## Security

Krate gives an app no access to the user's files or network by default. It can
use only the access approved for the current run.

That boundary is useful, but it does not make every app bug-free or prove that
every author is trustworthy.

Krate is pre-alpha:

- use it for testing your own apps and the published examples;
- do not treat it as ready for hostile third-party code;
- expect file formats and interfaces to change before version 1.0;
- report security problems privately through
  [SECURITY.md](SECURITY.md).

Read the current
[threat model](https://incyashraj.github.io/krate/docs/phase2/threat-model-v0-2.html)
for technical detail.

## Current limitations

- Krate does not convert an existing Swift, Electron, Tauri, or native app.
- The built-in author currently creates a small supported set of applications.
- A coding agent must use the current Krate APIs.
- The public macOS app is not code-signed yet.
- Permission review and desktop polish differ between operating systems.
- Krate Cloud does not exist yet.
- Signing, publisher identity, public discovery, managed updates, and a
  transparency log are planned work.

These limits are part of the product status, not hidden footnotes.

## Why not use a browser link?

A browser is the right answer for many applications.

Some useful local apps need desktop windows, local files, keyboard shortcuts,
devices, or operating system behavior that a browser cannot provide cleanly.
Normal native apps can do those things, but sharing them means packaging for
each system. The person opening one also has to trust code they did not write.

Krate is for the space between those two choices.

## Technical foundation

Krate is written in Rust and uses:

- [Wasmtime](https://wasmtime.dev/) to execute WebAssembly components;
- the [WebAssembly Component Model](https://component-model.bytecodealliance.org/);
- WIT interfaces for portable app behavior;
- host adapters for macOS, Windows, and Linux;
- a capability policy that checks every supported host operation.

The portability is how one file works across operating systems. The capability
model is how the user controls what that file can reach.

## Repository map

```text
apps/       Sample apps and apps used to test Krate
crates/     Runtime, CLI, policy, adapters, authoring, and SDK code
docs/       Website, book, design records, and technical documentation
Plan/       Current and future implementation plans
scripts/    Build, install, test, evidence, and release tools
test/       Integration fixtures and cross-language tests
wit/        Krate interface definitions
```

## Build from source

Requirements:

- the Rust toolchain selected by `rust-toolchain.toml`;
- Git;
- platform build tools;
- `cargo-component` when building Krate apps.

```bash
git clone https://github.com/incyashraj/krate
cd krate

cargo build --workspace
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Build the release CLI:

```bash
cargo build --release -p krate-cli
target/release/krate --version
```

## Documentation

| Start here | Link |
| --- | --- |
| Plain guide for making an app | [Make an app with AI](https://incyashraj.github.io/krate/docs/pages/make-an-app-with-ai.html) |
| First app walkthrough | [Try Krate Notes](https://incyashraj.github.io/krate/docs/try-krate-notes.html) |
| Developer quickstart | [Quickstart](https://incyashraj.github.io/krate/docs/quickstart.html) |
| Product direction | [Vision](https://incyashraj.github.io/krate/docs/vision.html) |
| Planned work | [Roadmap](https://incyashraj.github.io/krate/docs/roadmap.html) |
| Public development history | [Build log](https://incyashraj.github.io/krate/docs/build-log.html) |
| Exact current evidence | [Status](STATUS.md) |

## Roadmap

The immediate priority is external use:

1. watch new users install, create, open, and share an app;
2. repair the points where they get stuck;
3. expand the apps an AI agent can author;
4. make the permission experience clear on every operating system.

Later work includes:

- tools that help port existing source code to Krate interfaces;
- signing and publisher identity;
- Krate Cloud for publishing, finding, sharing, and updating apps;
- organization policies and audit records;
- more operating systems and device types.

See the full [roadmap](https://incyashraj.github.io/krate/docs/roadmap.html).

## Contributing

Contributions are welcome across code, documentation, design, examples, and
testing.

Start with:

1. [CONTRIBUTING.md](CONTRIBUTING.md)
2. [Open issues](https://github.com/incyashraj/krate/issues)
3. [Good first issues](https://github.com/incyashraj/krate/labels/good%20first%20issue)
4. [GitHub Discussions](https://github.com/incyashraj/krate/discussions)
5. [Code of Conduct](CODE_OF_CONDUCT.md)

For a larger change, open an issue or discussion before writing the full
implementation.

## Project status

- Stage: pre-alpha
- Current public release:
  [`v0.1.0-rc3`](https://github.com/incyashraj/krate/releases/tag/v0.1.0-rc3)
- Company: Krate Labs
- Maintainer: [Yashraj Pardeshi](https://github.com/incyashraj)
- License: MIT OR Apache-2.0

Krate was previously named Layer36. The rename is complete in the repository,
commands, interfaces, and documentation.

## License

Choose either:

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

Contributions use the same dual license.

## Acknowledgements

Krate builds on work from the
[Bytecode Alliance](https://bytecodealliance.org/),
[Wasmtime](https://wasmtime.dev/), the Rust community, and the wider
WebAssembly community.
