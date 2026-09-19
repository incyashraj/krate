# Your first PR

This guide walks from a fresh clone to a small merged pull request.

## 1. Pick a small issue

Start with the `good first issue` label:

<https://github.com/incyashraj/krate/labels/good%20first%20issue>

Read the issue's current discussion before starting. Documentation fixes,
broken-link fixes and small test improvements can be useful first changes.
If the scope is unclear, leave a comment and ask what needs to be covered.

See the [contribution guide](https://github.com/incyashraj/krate/blob/main/CONTRIBUTING.md)
for review conventions and the licenses that apply to different parts of Krate.

## 2. Fork and clone

Use GitHub's **Fork** button, then clone your fork:

```bash
git clone https://github.com/<your-handle>/krate.git
cd krate
git remote add upstream https://github.com/incyashraj/krate.git
```

## 3. Create a branch

Give the branch a name that describes the change:

```bash
git checkout -b docs/improve-first-pr-guide
```

No internal plan or phase task ID is needed.

## 4. Run the baseline checks

For a Rust change, first read the
[platform build requirements](https://github.com/incyashraj/krate/blob/main/docs/build.md).
The repository pins Rust in `rust-toolchain.toml`. Then run:

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

For a book change, install the version of mdBook used by CI and build the book:

```bash
cargo install mdbook --locked --version 0.4.40
mdbook build docs/book
```

## 5. Make the change

Keep the pull request focused on one idea. If you discover a second problem,
open a separate issue or mention it in the PR notes instead of expanding the
diff.

## 6. Commit

Use a Conventional Commit subject:

```bash
git add docs/book/src/contributing/first-pr.md
git commit -m "docs: clarify the first-PR guide"
```

Replace the example path with the files you changed. Check `git diff --staged`
before committing so unrelated work stays out of the PR.

## 7. Open the PR

Push your branch and open a pull request against `main`:

```bash
git push origin docs/improve-first-pr-guide
```

Fill out the PR template, link the public issue or discussion if applicable,
and include the checks you ran. If the PR changes visible documentation, add
a screenshot or a short note describing the rendered page.

## What happens next

A maintainer will review the PR, ask questions if needed, and merge once CI is
green and the scope is clear. Review is a conversation, not an exam.
