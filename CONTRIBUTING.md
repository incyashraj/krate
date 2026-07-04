# Contributing to Krate

First off: thank you. Krate is a long project built on first principles, and
every contribution — code, docs, design, or ideas — compounds.

---

## Before you start

1. Read the [Code of Conduct](CODE_OF_CONDUCT.md). We enforce it.
2. Read the [Roadmap](Plan/Build-Plan.md) and the [current phase plan](Plan/Phase-2-Plan.md)
   so you know where we are.
3. Check [open issues](https://github.com/incyashraj/krate/issues) — especially
   those labelled `good first issue`.
4. For anything bigger than a typo fix, open an issue or start a
   [GitHub Discussion](https://github.com/incyashraj/krate/discussions) first
   so we can align before you invest time.

---

## Development setup

```bash
# 1. Fork, then clone your fork
git clone https://github.com/<your-handle>/krate.git
cd krate

# 2. Install the right Rust toolchain (rust-toolchain.toml does this automatically)
#    rustup will read the file and install the pinned toolchain.
rustup show

# 3. Build the workspace
cargo build --workspace

# 4. Run tests
cargo test --workspace

# 5. Lint (must pass with zero warnings)
cargo clippy --all-targets --all-features -- -D warnings

# 6. Format check
cargo fmt --all -- --check
```

Everything should work in under 10 minutes on a modern machine.
If it doesn't, [open a bug report](https://github.com/incyashraj/krate/issues/new?template=bug_report.md) — that's a real bug.

---

## Making a change

### Branch naming

```
p{phase}-{area}-{short-description}
```

Examples:
- `p0-docs-fix-typo-readme`
- `p1-runtime-add-wasmtime-embed`
- `p2-uapi-io-write-impl`

### Commit style

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]
[optional footer(s)]
```

Types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `build`, `ci`

Scopes follow the crate name or plan phase, e.g. `runtime`, `cli`, `uapi`, `p0`.

Examples:
```
feat(runtime): embed wasmtime engine
fix(cli): handle missing manifest gracefully
docs(p0): add ADR-0001 rust-for-runtime
chore(ci): pin cargo-deny to v0.14
```

### Pull requests

1. Keep PRs focused. One logical change per PR.
2. Fill in the [PR template](.github/PULL_REQUEST_TEMPLATE.md) completely.
3. Reference the task ID in the PR description (format: `P{N}-{AREA}-{NN}`).
4. All CI checks must pass. Zero clippy warnings.
5. Add an entry to `CHANGELOG.md` under `[Unreleased]`.
6. If you changed the book, run `mdbook build docs/book`.
7. If you changed WIT, follow the
   [WIT style guide](docs/book/src/wit-style.md) and regenerate the UAPI
   reference.

---

## What requires an ADR?

Decisions that affect multiple crates, are hard to reverse, or wouldn't be
obvious from code alone. See [ADR process](docs/adr/README.md) and
[ADR template](docs/adr/template.md).

---

## Licensing of contributions

By submitting a pull request you agree to license your contribution under the
same terms as this project: **MIT OR Apache-2.0** (your choice per file).

There is no CLA. The `SPDX-License-Identifier` header approach is used for
any new source files.

---

## Decision-making

- Small changes: PR author decides, one maintainer approves.
- Large changes: write an ADR, open for discussion, merge with two approvals.
- Breaking changes to UAPI interfaces: require an ADR + two weeks open comment period.

---

## Where to ask for help

- **GitHub Discussions** — best place for design questions and early feedback.
- **Discord** `#help` channel — coming once the community server is live.
- **Issue comments** — on the specific issue you're working on.

Please don't open issues just to ask questions — use Discussions.
