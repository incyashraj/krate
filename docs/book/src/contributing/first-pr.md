# Your first PR

This guide walks from a fresh clone to a small merged pull request.

## 1. Pick a small issue

Start with the `good first issue` label:

<https://github.com/incyashraj/krate/labels/good%20first%20issue>

Good Phase 0 starter tasks are documentation fixes, broken-link fixes, CI
polish, and small scaffolding improvements. If an issue looks bigger than two
hours, leave a comment and ask for help trimming the scope.

## 2. Fork and clone

Use GitHub's **Fork** button, then clone your fork:

```bash
git clone https://github.com/<your-handle>/krate.git
cd krate
git remote add upstream https://github.com/incyashraj/krate.git
```

## 3. Create a branch

Branch names follow the task phase:

```bash
git checkout -b p0-docs-fix-first-pr-guide
```

Use `p0-` for Phase 0 work, `p1-` for Phase 1 work, and so on.

## 4. Run the baseline checks

Before changing anything, confirm the repo works on your machine:

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

For documentation-only work, also build the book:

```bash
mdbook build docs/book
```

## 5. Make the change

Keep the pull request focused on one idea. If you discover a second problem,
open a separate issue or mention it in the PR notes instead of expanding the
diff.

## 6. Commit

Use a Conventional Commit subject:

```bash
git add .
git commit -m "docs(p0): fix first-pr guide"
```

## 7. Open the PR

Push your branch and open a pull request against `main`:

```bash
git push origin p0-docs-fix-first-pr-guide
```

Fill out the PR template, link the issue or task ID, and include the checks you
ran. If the PR changes visible documentation, add a screenshot or a short note
describing the rendered page.

## What happens next

A maintainer will review the PR, ask questions if needed, and merge once CI is
green and the scope is clear. Review is a conversation, not an exam.

## Screenshot checklist

When GitHub Pages is live, the guide should include screenshots of:

- The `good first issue` label page.
- The GitHub fork button.
- The pull request form with the template filled in.

Until then, the commands above are the source of truth for the local workflow.
