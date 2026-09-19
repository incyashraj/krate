# Quickstart

Install the runtime, inspect an app, then choose how you want to build.
The same `.krate` artifact runs through compatible native Krate runtimes on
macOS, Windows and Linux. The recipient needs the runtime, not your build
tools. Existing OS-specific applications need a port to Krate's APIs.

## Get Krate

Download the runtime archive for your OS and CPU from
[the latest release](https://github.com/incyashraj/krate/releases/latest),
or use the published installer. The following commands execute a downloaded
script: inspect [install.sh](https://github.com/incyashraj/krate/blob/main/scripts/install.sh)
or [install.ps1](https://github.com/incyashraj/krate/blob/main/scripts/install.ps1)
first if you want to review it.

macOS and Linux:

```sh
curl -fsSL https://krate.tech/install.sh | sh
```

Windows **PowerShell**, not Command Prompt:

```powershell
irm https://krate.tech/install.ps1 | iex
```

Open a new terminal if the installer updated your PATH, then check:

```sh
krate --version
krate doctor
```

`doctor` reports authoring tools as well as the runtime environment. Missing
Rust or AI tooling is relevant to building, not to opening an already packed
app. On Linux X11 desktops, the keyboard bridge library may also be needed;
see the [platform requirements](https://github.com/incyashraj/krate/blob/main/docs/build.md).

## Open an app before building one

### Try the Chart sample

Download [chart.krate](https://raw.githubusercontent.com/incyashraj/krate/c46d29500f2b894c89285b04e1b5e2f75184e972/evidence/ported/chart.krate),
a rainfall-chart example from this repository. No authoring tools or AI account
are needed. Its [Rust source](https://github.com/incyashraj/krate/tree/c46d29500f2b894c89285b04e1b5e2f75184e972/apps/krate-chart)
is available separately; this particular sample bundle contains the manifest
and compiled component, not embedded source.

The complete file's SHA-256 is:

```text
3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3
```

Check it with `shasum -a 256 chart.krate` on macOS,
`sha256sum chart.krate` on Linux, or
`Get-FileHash chart.krate -Algorithm SHA256` in Windows PowerShell.
Then, from the folder containing the download:

```sh
krate run chart.krate --dump-caps
krate run chart.krate --prompt
```

Chart requests window creation, standard output and command-line arguments.
It requests no file or network access. The inspection's app-identity digest
is different from the complete-file download checksum above.

For a terminal-only functional check:

```sh
krate run chart.krate --headless --profile first-app -- quick
```

Expected output includes:

```text
bars:7
commands:10
drawn:yes
```

This check passed with the published v0.5.0 macOS ARM64 runtime. It checks the
chart's drawing commands, not an interactive session. The same sample also
passes the [source-built runtime's Linux, macOS and Windows CI replay](https://github.com/incyashraj/krate/actions/runs/35454102875).

### Open another app

Use a `.krate` file from a source you trust, such as a project you have
reviewed. Replace `app.krate` with the local filename:

```sh
krate run app.krate --dump-caps
krate run app.krate --prompt
```

The first command reports capabilities without executing the component.
The second asks about missing permissions before running. Do not grant a
capability just to dismiss a prompt: check why the app needs it. A permission
list describes access, not every behavior of the app.

## Make something

**With AI:** the CLI path below needs Rust/Cargo, the WebAssembly build
target, component tooling, and an installed, authenticated Claude Code CLI.
`krate ai` shows detected providers. Krate can offer to install missing build
tools; add `--no-install` if you want missing tools to stop the command instead.
Your AI provider's costs and terms still apply.

```sh
krate ai
krate create "a regex tester with a pattern box and live matches" \
  --agent claude --output regex.krate
```

**Without AI:** use the [Rust SDK](uapi/rust-sdk.md) and the from-source
walkthrough below. Prefer a graphical authoring interface? See
[Krate Studio](https://krate.tech/studio/).

The authoring checks cover the build, accepted imports, basic execution and
rendering. They do not prove every interaction works. Use the generated app,
test invalid input and saved data, and verify it on your target operating
systems before distributing it.

## Run it, and read what it may touch

```sh
krate run regex.krate --dump-caps
krate run regex.krate --prompt
```

Inspect first, then review the permission prompt. Use
`krate run --help` for the flags supported by the installed version.

## Send it

Send the `.krate` file by email, chat or a shared folder. The normal authoring
path includes editable source; inspect what is inside before sharing and do
not include secrets or private inputs. The recipient installs a compatible
Krate runtime once and runs the same file. Application data stored separately
on your machine is not automatically copied or synchronized.

Publishing to a hub is optional and uploads your bundle. Check the selected
hub, authentication and listing settings first:

```sh
krate publish --help
krate publish regex.krate
```

You do not need a hosted service to pass the file directly to another person.

## Where next

- **[Porting](porting.md)** to bring a project you already have.
- **[Current capability limits](limits.md)** before choosing APIs for a project.
- **[Distribution models](https://krate.tech/desktop-app-distribution.html)**
  to compare a shared artifact with Electron/Tauri packaging.
- **[The Rust SDK](uapi/rust-sdk.md)** for writing the code by hand.

---

The rest of this page is the from-source path for contributors. Some commands
exercise historical Phase 2 fixtures and evidence helpers rather than the
normal released-app workflow above. Their phase names are not the current
release version.

## Prerequisites

Install:

- Git
- Rust via `rustup`
- `cargo-component`

Krate pins its Rust toolchain in `rust-toolchain.toml`, so entering the repo
lets `rustup` install the right compiler and WASM targets.

Install the component tooling:

```bash
cargo install cargo-component --locked --version 0.21.1
```

## Get The Source

```bash
git clone https://github.com/incyashraj/krate.git
cd krate
```

## Build Krate

```bash
cargo build -p krate-cli
```

Check the local environment:

```bash
target/debug/krate doctor
```

`doctor` reports core Rust tools first, then Phase 2 language tools such as
`wasm-tools`, `tinygo`, `go`, `node`, `npm`, and `jco` when they are available.

## Build The Phase 2 Samples

```bash
scripts/build-krate-clock-component.sh
scripts/build-krate-cat-component.sh
scripts/build-krate-curl-component.sh
```

The scripts print component paths like:

```text
apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm
```

## Inspect A Manifest

Phase 2 apps carry a `manifest.toml` for app identity and capability requests.
Before running the file sample, inspect what it asks for:

```bash
target/debug/krate manifest explain apps/krate-cat/manifest.toml
```

You should see:

- `io.args`, `io.stdout`, and `io.stderr` as default-granted app plumbing
- `fs.read:fixtures/**` as a non-default launch grant

That is the current permission model in simple form:

```text
low-risk app plumbing -> default grant
host file/network access -> explicit launch grant
```

## Run The Clock Sample

Run a deterministic clock sample through the Phase 2 UAPI path:

```bash
target/debug/krate run \
  --auto-grant \
  --manifest apps/krate-clock/manifest.toml \
  --test-time 1234567890 \
  --test-locale en-US \
  --test-timezone UTC \
  apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm
```

This exercises time, locale, timezone, and stdout. The test flags make output
stable enough for evidence runs.

## Run The File Sample

Create a test file:

```bash
mkdir -p apps/krate-cat/fixtures
printf 'hello from Krate\n' > apps/krate-cat/fixtures/hello.txt
```

Run with the sample manifest and grant approval:

```bash
cd apps/krate-cat
../../target/debug/krate run \
  --manifest manifest.toml \
  --auto-grant \
  target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

Expected output:

```text
hello from Krate
```

## See The Denial Path

Run the same app without granting the file capability:

```bash
cd apps/krate-cat
printf '' | ../../target/debug/krate run \
  --manifest manifest.toml \
  target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

In a non-interactive shell, Krate exits before starting the component and
prints the missing required capability. That is intentional: host file access
should be explicit.

## Optional Network Sample

Start a local HTTP server in one terminal:

```bash
mkdir -p /tmp/krate-demo-http
printf 'portable runtime response\n' > /tmp/krate-demo-http/demo.txt
cd /tmp/krate-demo-http
python3 -m http.server 8765
```

In another terminal, run the curl sample with an explicit network grant:

```bash
cd /path/to/layer6x6
target/debug/krate run \
  --grant net.connect:127.0.0.1:8765 \
  apps/krate-curl/target/wasm32-wasip1/release/krate_curl.wasm \
  -- http://127.0.0.1:8765/demo.txt
```

If localhost sockets are restricted in your environment, skip this sample and
use the file and clock samples first.

## Check Phase 2 Readiness

```bash
scripts/phase2-exit-readiness.sh
```

This command reads the Phase 2 exit ledger and prints how many gates are done,
partial, pending, or blocked. It does not declare Phase 2 complete. It gives a
repeatable status snapshot.

## Run Evidence Helpers

For a local evidence packet:

```bash
scripts/record-phase2-exit-bundle.sh --strict
scripts/check-phase2-exit-evidence.sh
scripts/phase2-exit-readiness.sh
```

For sample output evidence:

```bash
scripts/record-phase2-sample-evidence.sh
```

For permission enforcement evidence:

```bash
scripts/record-phase2-ucap-evidence.sh --strict
```

## Historical Phase 1 Proof

The original hello-world proof still exists and is useful for understanding the
runtime base:

```bash
scripts/build-hello-component.sh
target/debug/krate run test/integration/hello-world/target/wasm32-wasip1/release/hello_world.wasm
```

Expected output:

```text
Hello, Krate!
```

For new app work, use the Phase 2 UAPI path instead:

- [Your First UAPI App In Rust](uapi/first-rust-cli.md)
- [Migrating From Phase 1 To Phase 2](phase2/migrating-from-phase1.md)

## See It With a Window

The GUI vertical slice has a one-command demo. On macOS it opens a real
native window (click the button within 30 seconds and watch the text field
change); on Linux and Windows the same portable file runs headless -- and the
full CI matrix proves the identical bytes open real windows there too.

```bash
sh scripts/demo-hello-gui.sh
```

Exit codes are the assertions: `0` = native click observed, `1` = clean
bounded run without a click, `2` = window closed early. The full test
manual, including manual commands and troubleshooting, is on the
[Hello GUI Demo & Testing](phase3/hello-gui-demo.md) page.

## Machine-Readable Runs

Add `--json` to any run to get one `krate.run.v1` object describing it --
app identity, granted capabilities with boundaries, denials, exit class,
duration, and captured output. This is the same report AI agents receive
through `krate mcp`; see
[Embedding & JSON Runs](phase3/embedding.md).
