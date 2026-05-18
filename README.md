# Layer36 (from layer6x6)

> Write once. Run on everything. Natively.

**Naming note:** This repository lives at `incyashraj/layer6x6` while the
project is still proving the 6x6 portability matrix. The product name is
**Layer36**: layer6x6 becomes Layer36 once the matrix is solved.

Layer36 is a universal application platform — a portable runtime, a universal
standard library (UAPI), and a capability-based permission model (UCap) — that
lets you ship one binary and run it natively on Windows, macOS, Linux, iOS,
Android, and the web.

It is built on WebAssembly and its Component Model, with a thin per-OS adapter
layer that translates UAPI calls to native OS APIs.

**Status:** Pre-alpha. Phase 2 is active: the CLI can run Phase 2
WebAssembly components through UAPI, manifest-declared capabilities, launch
grants, and sample apps for clock, cat, and curl. Formal Phase 2 exit still
needs final cross-host evidence, UAPI freeze review, and an outside developer
walkthrough.
See [the roadmap](https://incyashraj.github.io/layer6x6/roadmap.html).

---

## Why

Every app today is written six times: once for each operating system it runs
on. That is a tax on every developer and a ceiling on every idea. Layer36 removes
the tax by making the developer's target a portable runtime, not any particular
OS.

Read [the full vision](https://incyashraj.github.io/layer6x6/vision.html).

## Current phase

**Phase 2 — UAPI v0.1.** Layer36 is moving from runtime proof to a useful CLI
app platform slice. Current work covers:

- UAPI modules for `io`, `fs`, `net`, `time`, and `locale`
- UCap capability parsing, manifests, launch grants, and runtime checks
- sample apps: `layer36-clock`, `layer36-cat`, and `layer36-curl`
- Rust SDK direction, TypeScript fixture coverage, and experimental Go tracking
- evidence scripts for samples, UCap enforcement, adapters, benchmarks, and CI

Phase 1 is still supported for the original hello-world proof, but new app work
should use the Phase 2 UAPI path.

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
