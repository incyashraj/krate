<p align="center">
  <img src="docs/landing/krate-logo.png" width="128" alt="Krate logo">
</p>

<h1 align="center">Krate</h1>

<p align="center">
  <strong>Build once. Ship one file. It runs on Mac, Windows, and Linux.</strong>
</p>

<p align="center">
  No per-app installer, no per-OS port, no signing dance. A Krate app is a<br>
  few hundred kilobytes, carries its own source, and reaches nothing it did not declare.<br>
  The <strong>player</strong> is open source and installs once (~11 MB on macOS).
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
  <a href="https://krate.tech/">Website</a>
  ·
  <a href="https://krate.tech/studio/">Krate Studio</a>
  ·
  <a href="https://krate.tech/docs/quickstart.html">Docs</a>
  ·
  <a href="https://github.com/incyashraj/krate/releases">Releases</a>
</p>

<p align="center">
  <a href="https://krate.tech/">
    <img src="docs/landing/og-v3.png" width="900" alt="Krate turns an app into one file that opens on Mac, Windows, and Linux">
  </a>
</p>

## Start

```sh
curl -fsSL https://krate.tech/install.sh | sh     # macOS and Linux
irm https://krate.tech/install.ps1 | iex          # Windows
```

```sh
krate create "a tip calculator" --output tip.krate   # write one
krate port ./my-existing-app                         # or bring one you have
krate run tip.krate --dump-caps                      # read what it asks for
```

`krate check-app` is the oracle: it compiles the crate, confirms it imports
only `krate:*`, runs it once headless and paints a frame, then names the
stage and the fix when something fails.

Prefer a window? [Krate Studio](https://krate.tech/studio/) does the same
things with the same engine underneath.

Sent a `.krate` and just want to open it?
[krate.tech/open](https://krate.tech/open/).

## Nothing installed? Start in the browser

[krate.tech/app](https://krate.tech/app/) is Krate Studio in a tab. Describe
the app you want and **Krate AI** writes it, builds it on our machines and
hands back a `.krate` you can download and run. No install, no toolchain, no
API key of your own.

Your first app is on us, and so is your first change to it. After that,
Krate Studio on your own machine is free and unlimited when you point it at
an AI you already have (Claude, Codex, Gemini, or an API key you hold). The
session follows your account, so anything you start in the browser opens on
the desktop ready to edit.

## 285 MB became a file you can email

The same 50,000-line notes workload, same Apple M4 Mac, head to head against
MarkText 0.17.1 (Electron). Both builds ARM64. Every number below comes from
one reproducible run whose raw samples, input digests and machine state are
committed beside it and covered by a seal.

| | MarkText | Krate |
|---|---:|---:|
| The app payload | 284.6 MiB installed | **36.6 KiB** |
| Memory, 50,000 lines | 2,299.4 MiB across four processes | **178.5 MiB, one process** |
| Opens in, 50,000 lines | 611.5 ms median of 10 | **237.1 ms median of 10** |
| CPU, document open | 7.38% of a core | **1.97% of a core** |

Krate does not put another browser inside every app. The 36.6 KiB is the
per-app payload: the shared player is installed once at 88.6 MiB, so the
first app you ship costs 3.21x less disk than the Electron build, and every
app after that costs almost nothing.

The `.krate` file you actually send is bigger than its payload, because it
carries the app's own source and the SDK interfaces beside the code: about
105 KB to 360 KB for the apps in this repository, and more for one that
embeds large assets. That is the honest number to compare against an
installer, and it is still roughly a thousandth of the Electron build.

Method, raw samples and seal: the
[reproducible benchmark kit](evidence/benchmarks/marktext-vs-krate/README.md)
and [this run](evidence/benchmarks/marktext-vs-krate/runs/20260825T085644Z/analysis.md).
Energy was not measured and no battery-life claim is made from it.

## Why a file

Shipping a desktop app means a build per operating system, an installer to
maintain, and a code-signing certificate for each platform. A web app
trades that for hosting you keep paying for and a machine you do not
control.

1. The app and the access it asks for go into one `.krate` file.
2. That same file opens on Mac, Windows, and Linux. The bytes do not change.
3. Krate shows the person what it wants before it runs.
4. The app gets only what they allow, and nothing else.

A Krate app is a WebAssembly component compiled from ordinary Rust. It
carries no browser and no per-app runtime: the compiled code for a playable
game is **13 KB** and for the notes editor in the benchmark **37 KB**. The
file you send wraps that in its own source and the SDK interfaces, so a
`.krate` is typically **105 KB to 360 KB**.

## Krate Studio

<p align="center">
  <img src="docs/landing/app-shots/studio-home.png" width="900" alt="Krate Studio">
</p>

Studio runs the same engine the CLI does: build, import-check, run and
pack, with the engine's real output on screen, not a summary of it. It uses
the coding AI you already have installed and pay for, and stays out of the
way if you would rather write the code yourself. Every `.krate` carries its
own source, so a build you shipped a year ago opens as a project you can
edit.

It runs in two places, and they are the same interface rather than two
lookalikes:

- **In a browser**, at [krate.tech/app](https://krate.tech/app/). Krate AI
  does the writing and the build happens on our machines. Nothing to
  install, and your first app and first change are free.
- **On your machine**, from [krate.tech/studio](https://krate.tech/studio/).
  Signed `.dmg` on macOS, installer on Windows (unsigned for now, so
  SmartScreen asks once), AppImage on Linux. Free and unlimited with your
  own AI.

Sessions live on your account, so an app you start in the browser opens on
the desktop with its conversation intact and editable.

## What it costs

Nothing, right now. The player is MIT and Apache licensed and always will
be. The CLI and Krate Studio on your own machine are free and uncapped,
and we will say so long before that changes.

The one thing with a limit is the inference we pay for. Krate AI in the
browser writes your first app and your first change to it on us; after
that, making is unlimited on your own machine with an AI you already have.

If you point Krate at your own coding AI, that stays your subscription and
Krate never holds its keys.

## The permission wall

A `.krate` holds a WebAssembly component, the app's name, the access it
requests, and a reason for each request. Opening the file grants nothing on
its own; Krate connects only the operations the person approved. Look
inside any app without running it:

```bash
krate run app.krate --dump-caps
```

- `fs.read:notes/**` reads only inside the `notes` folder;
- `store.kv` / `store.sql` give an app private storage, so remembering
  things needs no access to your folders at all;
- no network call works unless network access was declared and granted, and
  a redirect to a host you did not allow is not followed;
- an app downloaded from a URL gets no extra access for having come from one;
- the camera, the microphone, speech and sound sit behind the same wall, so
  an app that wants to see or hear you has to say so first;
- a file you pick or drag onto a window is handed over as a name and a
  token, never a path, so the app opens that one file and cannot walk to its
  folder or come back for it on a later run;
- `store.shared` and `net.ws` are the multiplayer pair: a bucket two
  machines share through an invite code, and a live two-way connection.

The strong version of the claim is mechanical, not a promise: these apps
import **zero** `wasi:*` interfaces. There is no ambient access to leak
because the door was never built into the app. Check any app yourself with
`wasm-tools component wit`.

---

## Work on it

Everything above is what Krate does. Everything below is the machinery:
the player, the `.krate` format, and `krate check-app` live in this
repository; Krate Studio and the hub are the product layer (`studio/`
and `cloud/`).

## A terminal or your own AI tools

Studio is one front end for the engine, not the only one. Whichever you
pick, every path ends at the same `.krate` file:

- **MCP**: `krate mcp` is a server Claude Desktop or Cursor can call; add
  `{"mcpServers": {"krate": {"command": "krate", "args": ["mcp"]}}}` to the
  app's MCP config and ask for the app in chat. Full setup:
  [docs/mcp-setup.md](docs/mcp-setup.md).
- **Krate Mode**: one paste-in prompt that teaches any AI chat to write
  correct Krate code, generated from the real interface definitions.
  `krate krate-mode` prints it;
  [read it online](https://krate.tech/docs/pages/krate-mode.html).
- **One word**: `krate` asks what you want and builds it. In one line:
  `krate create "a habit tracker" --output habit.krate --agent claude`.
  Five agents are supported (`claude`, `codex`, `gemini`, `copilot`,
  `grok`); `krate ai` says which are ready, and `--author-cmd` is the seam
  for anything else.

Ask for something a sandboxed app cannot be, like "download my email", and
`krate create` refuses in about a second with the reason and a suggestion,
instead of spending five minutes building something convincing that could
never work.

## `krate check-app`: the oracle behind AI authoring

An AI writing code needs a truthful, fast answer to "is this actually
correct?" `check-app` runs six stages and prints one verdict; because the
failure text names the fix, an agent can loop on it without a human in the
middle.

| Stage | Exit | What it proves |
| --- | --- | --- |
| layout | 10 | The directory has the files an app needs |
| manifest | 11 | The manifest is valid |
| build | 12 | It compiles, with the right toolchain |
| imports | 13 | It imports only `krate:*`, no `wasi:*` leak |
| run | 14 | It actually runs, headless |
| shoot | 15 | A GUI app paints a real frame |

`--json` gives the same verdict as a machine-readable object; `--shoot
frame.png` writes the app's first frame to a PNG so output can be looked at
rather than guessed about.

## Where things stand

One `.krate` runs on Mac, Windows, and Linux, GPU-rendered with a CPU
fallback; files, network, storage, and clipboard sit behind the permission
wall; apps run from a file or straight from an HTTPS URL; publishing a
link works from Studio, `krate publish`, or
[krate.tech/publish](https://krate.tech/publish). Exact evidence:
[STATUS.md](STATUS.md).

Known limits, stated plainly:

- Windows and Linux studio builds are unsigned for now; macOS is signed and
  notarised by Apple.
- An AI has to write against the current Krate APIs, which are still changing.
- Permission review and desktop polish differ between operating systems.
- File formats and interfaces will change before 1.0.
- This is a young product, not a frozen API. Use it for your own apps and
  the published examples, not as a shield against hostile third-party code.

## Build from source

```bash
git clone https://github.com/incyashraj/krate
cd krate
cargo build --workspace && cargo test --workspace
```

The workspace has over 1,600 Rust tests. You need the Rust toolchain named in
`rust-toolchain.toml` and `cargo-component`; check your machine with
`krate doctor`. Platform packages, the two Windows traps, and the
one-package Linux receiver note live in [docs/build.md](docs/build.md).

## Repository map

```text
apps/       Sample apps and apps used to test Krate
crates/     Runtime, CLI, policy, adapters, MCP server, authoring, and SDK
docs/       Website, book, design records, and technical documentation
Plan/       Current and future implementation plans
scripts/    Build, install, test, evidence, and release tools
test/       Integration fixtures and cross-language tests
wit/        Krate interface definitions
```

## Documentation

| Start here | Link |
| --- | --- |
| Connect Krate to Claude Desktop or Cursor | [docs/mcp-setup.md](docs/mcp-setup.md) |
| The paste-in prompt for any AI chat | [Krate Mode](https://krate.tech/docs/pages/krate-mode.html) |
| Plain guide for making an app | [Make an app with AI](https://krate.tech/docs/pages/make-an-app-with-ai.html) |
| Developer quickstart | [Quickstart](https://krate.tech/docs/quickstart.html) |
| Build from source | [docs/build.md](docs/build.md) |
| Exact current evidence | [Status](STATUS.md) |

## Contributing

Contributions are welcome across code, documentation, design, examples, and
testing. Start with [CONTRIBUTING.md](CONTRIBUTING.md), then
[good first issues](https://github.com/incyashraj/krate/labels/good%20first%20issue)
and [Discussions](https://github.com/incyashraj/krate/discussions). For a
larger change, open an issue before writing the full implementation.
Everyone is held to the [Code of Conduct](CODE_OF_CONDUCT.md).

## What Krate counts

A random id made on your machine, the Krate version, the OS name, one of
`install` / `make` / `open` / `publish`, whether an AI wrote the app, and
whether it worked. Never: app names, prompts, paths, or anything about you.
`krate telemetry off` turns it off (`DO_NOT_TRACK=1` is honoured too), and
it says so on first run rather than hiding here.

## Project status

- Stage: public beta, works end to end, API not yet frozen
- Current release: [the latest on the releases page](https://github.com/incyashraj/krate/releases/latest)
- Company: Krate Labs
- Maintainer: [Yashraj Pardeshi](https://github.com/incyashraj)
- License: MIT OR Apache-2.0

Krate was previously named Layer36. The rename is complete.

## License

Open core, split by what each piece is for:

| Piece | License |
| --- | --- |
| Player, `.krate` format, CLI, runtime: everything a receiver trusts | [MIT](LICENSE-MIT) OR [Apache 2.0](LICENSE-APACHE) |
| Krate Studio (`studio/`) and the hub worker (`cloud/worker/`) | [Business Source License 1.1](studio/LICENSE), converts to Apache 2.0 in 2030 |

The BSL lets you read, build, and use Studio; it stops a competing hosted
Studio. Versions published before the split remain under their original
MIT OR Apache terms. Contributions to the player use the dual license.

## Acknowledgements

Krate builds on work from the
[Bytecode Alliance](https://bytecodealliance.org/),
[Wasmtime](https://wasmtime.dev/), the Rust community, and the wider
WebAssembly community.
