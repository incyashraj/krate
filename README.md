# Layer36 (from layer6x6)

> Software should run like a PDF opens: exactly the same on any device — and
> never touch anything without permission.

Layer36 is a safe runtime for portable software. A program compiles once to a
WebAssembly component; the Layer36 runtime runs that same file natively on
Linux, macOS, and Windows through one standard library (UAPI), and a
capability system (UCap) means the program touches nothing — not a file, not
the network — without an explicit grant.

**Why now:** software is being generated faster than it can be ported, audited,
or sandboxed. Machine-generated tools need one safe, universal place to run.
Layer36 is being built to be that place — starting with developer tools and
local utilities, growing toward the full platform.

**The long-term arc** is a universal application platform: the same portable
file running natively on desktop, mobile, and the web, with distribution and
identity built in. See [the full vision](https://incyashraj.github.io/layer6x6/vision.html),
[the roadmap](https://incyashraj.github.io/layer6x6/roadmap.html), and
[follow the build](https://incyashraj.github.io/layer6x6/build-log.html).

**Naming note:** This repository lives at `incyashraj/layer6x6` while the
project is still proving the 6x6 portability matrix. The product name is
**Layer36**: layer6x6 becomes Layer36 once the matrix is solved.

**Status:** Pre-alpha. The runtime runs real CLI components — clock, cat,
curl — from a single `.wasm` on Linux, macOS, and Windows, with
manifest-declared capabilities, launch grants, and runtime permission checks,
proven by cross-host CI. The first GUI component works too: one portable file
opens a real native window (real `NSButton`, real `NSTextField`) on macOS and
runs headless on the other hosts — the full CI matrix executes the
byte-identical GUI artifact on all three OSes. Agents can drive all of it:
an embedding API, `layer36 run --json`, and an MCP server
(`layer36-mcp-server`) expose sandboxed execution with permission decisions
returned as data. Formal Phase 2 exit still needs final cross-host evidence,
a UAPI freeze review, and an outside developer walkthrough.

---

## Current phase

**Phase 3 — desktop UI foundation** (with Phase 2 closeout tracked separately).
Current work covers:

- **done:** the P3-VS-01 vertical slice — one portable WASM component opens
  a real native macOS window with native controls, and a human click flows
  back into the component as a portable event
  (`sh scripts/demo-hello-gui.sh` to see it)
- **done:** the agent-embedding track — `layer36_runtime::embed`,
  `layer36 run --json` (schema `layer36.run.v1`), and the `layer36-mcp-server`
  MCP tool for agent frameworks
- **done:** cross-OS artifact proof — full CI runs the byte-identical GUI
  component headless on Linux, macOS, and Windows
- next milestone: real winit windows on Linux and Windows, so the same file
  becomes *visible* everywhere (widgets drawn per ADR-0015 on Linux)

Phase 2's CLI path stays fully supported: UAPI modules for `io`, `fs`, `net`,
`time`, and `locale`, the sample apps, and the evidence harness are unchanged.

## Quickstart

Build the CLI and the Phase 2 sample components:

```bash
cargo build -p layer36-cli
scripts/build-layer36-clock-component.sh
scripts/build-layer36-cat-component.sh
scripts/build-layer36-curl-component.sh
```

Explain the permissions for the file-reading sample:

```bash
target/debug/layer36 manifest explain apps/layer36-cat/manifest.toml
```

Run a deterministic clock component through the Phase 2 UAPI path:

```bash
target/debug/layer36 run \
  --auto-grant \
  --manifest apps/layer36-clock/manifest.toml \
  --test-time 1234567890 \
  --test-locale en-US \
  --test-timezone UTC \
  apps/layer36-clock/target/wasm32-wasip1/release/layer36_clock.wasm
```

Run the file sample with an explicit grant:

```bash
mkdir -p apps/layer36-cat/fixtures
printf 'hello from Layer36\n' > apps/layer36-cat/fixtures/hello.txt
cd apps/layer36-cat
../../target/debug/layer36 run \
  --manifest manifest.toml \
  --auto-grant \
  target/wasm32-wasip1/release/layer36_cat.wasm \
  -- ./fixtures/hello.txt
cd ../..
```

Expected file sample output:

```text
hello from Layer36
```

Check the current Phase 2 exit status:

```bash
scripts/phase2-exit-readiness.sh
```

For the full walkthrough, read the
[Quickstart](https://incyashraj.github.io/layer6x6/quickstart.html).

## Security

Layer36 is pre-alpha. Do not run untrusted WebAssembly through `layer36` yet.
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
[GitHub Discussions](https://github.com/incyashraj/layer6x6/discussions).
The Discord invite will be added once the Phase 0 community server is live.

Good first issues are labeled
[`good first issue`](https://github.com/incyashraj/layer6x6/labels/good%20first%20issue).

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option. Contributions are dual-licensed under the same terms.

## Acknowledgements

Layer36 stands on the shoulders of the
[Bytecode Alliance](https://bytecodealliance.org/), the
[Rust Foundation](https://foundation.rust-lang.org/), and everyone else
building the open WebAssembly ecosystem.
