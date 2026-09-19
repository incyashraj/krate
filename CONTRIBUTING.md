# Contributing to Krate

Thanks for helping improve Krate. Code, documentation, examples and reports
from trying the software are all useful contributions.

---

## Before you start

1. Read the [Code of Conduct](CODE_OF_CONDUCT.md). We enforce it.
2. Read the public [roadmap](docs/book/src/roadmap.md) and
   [current capability limits](docs/book/src/limits.md).
3. Check [open issues](https://github.com/incyashraj/krate/issues) -- especially
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

# 2. Check the pinned Rust toolchain
rustup show
```

Before building Rust code, install the platform prerequisites in
[Building Krate from source](docs/build.md). The toolchain version and targets
are pinned in [rust-toolchain.toml](rust-toolchain.toml). Building guest apps
also needs `cargo-component`; opening a packaged app does not.

For Rust changes, run the workspace checks from the repository root:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

For book changes, use the mdBook version used by CI:

```bash
cargo install mdbook --locked --version 0.4.40
mdbook build docs/book
```

A cold Rust build can take time and depends on your machine. If setup fails,
include your OS, architecture, command and error in a
[bug report](https://github.com/incyashraj/krate/issues/new?template=bug_report.md).
Remove credentials and private paths from logs before sharing them.

---

## Making a change

### Branch naming

Use a descriptive name, such as `docs/improve-quickstart` or
`fix/manifest-error`. You do not need an internal plan or phase task ID.

### Commit style

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

[optional body]
[optional footer(s)]
```

Types: `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `build`, `ci`

Scopes can name the crate or area, for example `runtime`, `cli` or `docs`.

Examples:
```
feat(runtime): embed wasmtime engine
fix(cli): handle missing manifest gracefully
docs: clarify the runtime installation steps
chore(ci): pin cargo-deny to v0.14
```

### Pull requests

1. Keep PRs focused. One logical change per PR.
2. Fill in the [PR template](.github/PULL_REQUEST_TEMPLATE.md) completely.
3. Link the public issue or discussion, if there is one, and explain the change.
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

Contributions use the existing license terms of the code they change:

- The player, `.krate` format, CLI and runtime use
  [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
- Krate Studio and the hub worker have separate Business Source License 1.1
  terms in [studio/LICENSE](studio/LICENSE) and
  [cloud/worker/LICENSE](cloud/worker/LICENSE). Those files specify their use
  grants and change to Apache-2.0 on August 30, 2030.

Read the applicable license before contributing. This guide does not replace
or change those terms.

There is no CLA. The `SPDX-License-Identifier` header approach is used for
any new source files.

---

## Decision-making

- Small changes: PR author decides, one maintainer approves.
- Large changes: write an ADR, open for discussion, merge with two approvals.
  Founder stage, while `docs/governance/maintainers.json` lists one active
  maintainer: that maintainer records their own review in the PR, and the
  two-approval rule resumes when a second is registered.
- Breaking changes to UAPI interfaces: require an ADR + two weeks open comment period.

---

## Where to ask for help

- **GitHub Discussions** -- best place for design questions and early feedback.
- **Discord** `#help` channel -- coming once the community server is live.
- **Issue comments** -- on the specific issue you're working on.

Please don't open issues just to ask questions -- use Discussions.
