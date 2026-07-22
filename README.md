# Krate (formerly Layer36)

> Share software the way you share a document, without handing over your machine.

**Krate** — by Krate Labs — packs an application and the permissions it is
asking for into a single file. Send someone a link to it. It opens on their
Mac, Windows, or Linux machine as a real desktop app, and it cannot read a
file, reach the network, or touch anything else unless they allow it.

```console
$ krate pack app.wasm --manifest app.toml -o hello.krate
wrote hello.krate (11080 bytes)

$ krate run https://example.com/hello.krate
```

**Why this matters now:** AI writes most new software, and nobody reads it.
Software gets generated constantly and shared constantly, between people who
did not write it and cannot audit it. So the thing that travels between people
has to carry its own permissions. Running it on your own machine is how it stays
useful — that is where your files and your work already are — and the capability
model is what keeps that safe.

Underneath, a program compiles once to a WebAssembly component and runs
natively on Linux, macOS, and Windows through one standard library (UAPI), with
a capability system (UCap) enforcing every grant. That portability is how the
sharing property is achieved, not the point of it.

## Install

macOS and Linux, one command:

```bash
curl -fsSL https://raw.githubusercontent.com/incyashraj/krate/main/scripts/install.sh | sh
```

Then try the note-taking sample, which is one 12 KB file:

```bash
krate run https://github.com/incyashraj/krate/releases/download/notes-v0.1.0/notes.krate --native-window --prompt
```

Full walkthrough: [Try Krate Notes](https://incyashraj.github.io/krate/try-krate-notes.html).

**The long-term arc** is a universal application platform: the same portable
file running natively on desktop, mobile, and the web, with distribution and
identity built in. See [the full vision](https://incyashraj.github.io/krate/vision.html),
[the roadmap](https://incyashraj.github.io/krate/roadmap.html), and
[follow the build](https://incyashraj.github.io/krate/build-log.html).

**Naming note:** The project was renamed from **Layer36** to **Krate** in
July 2026 (company: Krate Labs). During the transition, the repository,
code, commands, crate names, and `krate:*` API namespaces keep the legacy
name — the code-level rename is a scheduled slice that lands before the
UAPI freeze. Everything you run below therefore still says `krate`; the
behavior is Krate.

**Status:** Pre-alpha. The Krate runtime runs real CLI components — clock, cat,
curl — from a single `.wasm` on Linux, macOS, and Windows, with
manifest-declared capabilities, launch grants, and runtime permission checks,
proven by cross-host CI. The first GUI component works too: one portable file
opens a real native window (real `NSButton`, real `NSTextField`) on macOS and
runs headless on the other hosts — the full CI matrix executes the
byte-identical GUI artifact on all three OSes. Agents can drive all of it:
an embedding API, `krate run --json`, and an MCP server
(`krate-mcp-server`) expose sandboxed execution with permission decisions
returned as data. Formal Phase 2 exit still needs final cross-host evidence,
a UAPI freeze review, and an outside developer walkthrough.

---

## Current phase

**Phase 3 — desktop UI foundation** (with Phase 2 closeout tracked separately).
Current work covers:

- **done:** the P3-VS-01 vertical slice — one portable WASM component opens
  a real native macOS window with native controls, and a human click flows
  back into the component as a portable event
  (`sh scripts/demo-hello-gui.sh` to see it). Keyboard input is not wired on
  macOS yet, so an AppKit window is clickable but not typable; Linux and
  Windows deliver keys and modifiers today.
- **done:** the agent-embedding track — `krate_runtime::embed`,
  `krate run --json` (schema `krate.run.v1`), and the `krate-mcp-server`
  MCP tool for agent frameworks
- **done:** cross-OS artifact proof — full CI runs the byte-identical GUI
  component headless on Linux, macOS, and Windows
- next milestone: real winit windows on Linux and Windows, so the same file
  becomes *visible* everywhere (widgets drawn per ADR-0015 on Linux)

Phase 2's CLI path stays fully supported: UAPI modules for `io`, `fs`, `net`,
`time`, and `locale`, the sample apps, and the evidence harness are unchanged.

## Quickstart

### Pack an app into one file and share it

A `.krate` is a single file holding the component and the permissions it asks
for. Packing changes how an app is delivered and nothing about what it may do:
a bundle fetched from a URL gets exactly the authority a local one does, which
is none until it is granted.

```bash
cargo build -p krate-cli
scripts/build-krate-cat-component.sh

mkdir -p /tmp/share/fixtures && cd /tmp/share
# a bundle always runs code.wasm, so the manifest names that
cp "$OLDPWD/apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm" code.wasm
sed 's|^entry.*|entry = "code.wasm"|' "$OLDPWD/apps/krate-cat/manifest.toml" > manifest.toml
printf 'hello from a shared krate\n' > fixtures/hello.txt

"$OLDPWD/target/debug/krate" pack code.wasm --manifest manifest.toml -o cat.krate
"$OLDPWD/target/debug/krate" run cat.krate --auto-grant -- ./fixtures/hello.txt
```

```text
hello from a shared krate
```

The app declared `fs.read:./fixtures/**`, so that is all it can read. Ask it for
anything else and it is refused, even with `--auto-grant`:

```console
$ krate run cat.krate --auto-grant -- /etc/hosts
krate-cat: permission denied: /etc/hosts
```

Put `cat.krate` behind any URL and it runs from there, with the same limits:

```bash
krate run https://example.com/cat.krate -- ./fixtures/hello.txt
```

Plain `http://` is refused unless you pass `--insecure-http`, which exists for
local test servers.

### The rest

Build the CLI and the Phase 2 sample components:

```bash
cargo build -p krate-cli
scripts/build-krate-clock-component.sh
scripts/build-krate-cat-component.sh
scripts/build-krate-curl-component.sh
```

Explain the permissions for the file-reading sample:

```bash
target/debug/krate manifest explain apps/krate-cat/manifest.toml
```

Run a deterministic clock component through the Phase 2 UAPI path:

```bash
target/debug/krate run \
  --auto-grant \
  --manifest apps/krate-clock/manifest.toml \
  --test-time 1234567890 \
  --test-locale en-US \
  --test-timezone UTC \
  apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm
```

Run the file sample with an explicit grant:

```bash
mkdir -p apps/krate-cat/fixtures
printf 'hello from Krate\n' > apps/krate-cat/fixtures/hello.txt
cd apps/krate-cat
../../target/debug/krate run \
  --manifest manifest.toml \
  --auto-grant \
  target/wasm32-wasip1/release/krate_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

Expected file sample output:

```text
hello from Krate
```

Check the current Phase 2 exit status:

```bash
scripts/phase2-exit-readiness.sh
```

See it with a window (macOS shows a real native window; other hosts run the
same file headless):

```bash
sh scripts/demo-hello-gui.sh
```

Exit codes are the assertions: `0` you clicked the native button, `1` clean
run without a click, `2` window closed early. Full test manual:
[Hello GUI Demo & Testing](https://incyashraj.github.io/krate/phase3/hello-gui-demo.html).

Get a machine-readable run report (what agents consume):

```bash
target/debug/krate run --json --auto-grant \
  --manifest apps/krate-clock/manifest.toml \
  apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm
```

For the full walkthrough, read the
[Quickstart](https://incyashraj.github.io/krate/quickstart.html).

## Security

Krate is pre-alpha. Do not run untrusted WebAssembly through `krate` yet.
Phase 2 has real capability checks for the current UAPI slice, but the platform
is not adversarially hardened and should still be treated as a developer proof.

See the [Phase 2 threat model](docs/book/src/phase2/threat-model-v0-2.md).

## Project structure

```
crates/         # Runtime, CLI, policy, manifest, adapters, SDK helpers
wit/            # WebAssembly Interface Types definitions
apps/           # Sample and dogfood apps
docs/           # Documentation, ADRs, mdBook site source
  adr/          # Architecture Decision Records
  book/         # mdBook source
  legal/        # Trademark, legal notes
  rfc/          # Proposals
Plan/           # Phase-by-phase build plans (living documents)
src/            # Workspace sentinel
test/           # Integration tests and component fixtures
scripts/        # Dev tooling scripts
```

## Contributing

We want you. Read [CONTRIBUTING.md](CONTRIBUTING.md) and start with
[GitHub Discussions](https://github.com/incyashraj/krate/discussions).
The Discord invite will be added once the Phase 0 community server is live.

Good first issues are labeled
[`good first issue`](https://github.com/incyashraj/krate/labels/good%20first%20issue).

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option. Contributions are dual-licensed under the same terms.

## Acknowledgements

Krate stands on the shoulders of the
[Bytecode Alliance](https://bytecodealliance.org/), the
[Rust Foundation](https://foundation.rust-lang.org/), and everyone else
building the open WebAssembly ecosystem.
