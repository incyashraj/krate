# Good First Issue Drafts

These are Phase 0 issue drafts to create in GitHub once repository labels are
configured. Each is intentionally small enough for a new contributor to finish
in one focused sitting.

## 1. docs(p0): proofread the README for first-time readers

**Labels:** `good first issue`, `type:docs`, `phase:0`, `status:ready`

Read the README as if you have never seen Krate before. Fix wording that is
unclear, tighten the project summary, and note any unanswered questions in the
pull request description.

**Acceptance:**

- README still explains Krate in under 90 seconds.
- No broken links are introduced.
- PR includes a short note about what confused you before the edit.

## 2. docs(p0): add screenshots to the first-PR guide

**Labels:** `good first issue`, `type:docs`, `phase:0`, `status:blocked`

Once the GitHub repo and Pages site are public, add screenshots to
`docs/book/src/contributing/first-pr.md`.

**Acceptance:**

- Screenshots cover the fork button, good-first-issue label page, and PR form.
- Images are stored under `docs/book/src/assets/`.
- The book builds locally with `mdbook build docs/book`.

## 3. ci(p0): verify mdBook build on pull requests

**Labels:** `good first issue`, `type:ci`, `phase:0`, `status:ready`

Confirm the `Docs (mdBook)` CI job runs on pull requests and produces useful
failure output when a page is missing.

**Acceptance:**

- Open a test PR that intentionally breaks a book link or page path.
- Confirm CI fails in the docs job.
- Restore the page and confirm CI returns green.

## 4. docs(p0): fill out the trademark search log

**Labels:** `good first issue`, `type:docs`, `phase:0`, `status:ready`

Complete the search log in `docs/legal/trademark-search.md` using the official
databases listed there.

**Acceptance:**

- Each searched database has a dated entry.
- Close matches include owner, class, jurisdiction, and link/reference.
- The decision section is updated to keep the name, change it, or defer.

## 5. docs(p0): review setup on a fresh machine

**Labels:** `good first issue`, `type:docs`, `phase:0`, `status:ready`

Follow `docs/book/src/contributing/setup.md` from a fresh checkout and record
anything that does not work as written.

**Acceptance:**

- `scripts/setup.sh` succeeds or the failure is documented.
- Setup guide is updated with any missing prerequisite.
- PR notes include OS, architecture, Rust version, and elapsed time.
