# Krate — Phase 0 Detailed Plan: Foundation

> **Phase:** 0 of 8
> **Duration:** Weeks 1–4 (20 calendar days, ~5 engineering days of actual work)
> **Phase sentence:** *Get the project bones set up so real work can start.*
> **Prerequisite:** Commitment to ship. Nothing else.
> **Supersedes:** nothing. **Superseded by:** nothing.

---

## Table of Contents

0. [How to Use This Document](#0-how-to-use-this-document)
1. [Phase Objective](#1-phase-objective)
2. [Success Criteria](#2-success-criteria)
3. [What Phase 0 Is and Is Not](#3-what-phase-0-is-and-is-not)
4. [The Repo as Architecture](#4-the-repo-as-architecture)
5. [Technology Decisions](#5-technology-decisions)
6. [Week-by-Week Breakdown](#6-week-by-week-breakdown)
7. [Task Details](#7-task-details)
8. [Document Templates](#8-document-templates)
9. [CI Setup](#9-ci-setup)
10. [Community Setup](#10-community-setup)
11. [Legal & IP](#11-legal--ip)
12. [ADR-0001 in Full](#12-adr-0001-in-full)
13. [Exit Criteria Checklist](#13-exit-criteria-checklist)
14. [Phase 0 Risks](#14-phase-0-risks)
15. [Handoff to Phase 1](#15-handoff-to-phase-1)
16. [Appendices](#16-appendices)

---

## 0. How to Use This Document

Phase 0 is short but easy to get wrong in ways that compound. The mistakes here — a sloppy license choice, a brittle CI, a non-welcoming CONTRIBUTING — don't surface until month 6 when you can't change them without breaking something.

- Every task has an ID (`P0-REPO-01`, etc.) matching §7 of the main Build Plan.
- The document templates in §8 are deliberately complete. Paste, don't paraphrase. If you feel like "improving" them mid-phase, open an ADR instead.
- Phase 0 has no code. Every deliverable is a file in the repo or an account on a service.
- If you're tempted to start Phase 1 before finishing §13, stop. Every hour spent polishing Phase 0 saves a day in Phase 1.

---

## 1. Phase Objective

### 1.1 One-sentence objective

**A contributor can `git clone`, open their editor, and be productive in under an hour — without asking anyone a question.**

### 1.2 Why this matters

Every serious OSS project has a moment where someone who isn't the founder shows up, reads the repo, and decides whether to contribute or close the tab. That decision happens in the first five minutes. The README, the build system, the CI, the CONTRIBUTING guide — together they answer "is this real?" If the answer is no, the contributor leaves and doesn't come back. Krate needs at least 30 external contributors over 24 months. Phase 0 is where we earn the right to attract them.

### 1.3 The four outputs of Phase 0

1. **A repo that passes the smell test** — clean README, working CI, clear contributing path.
2. **A documentation site** — empty but scaffolded, so Phase 1's docs have a home.
3. **A community channel** — Discord server set up and staffed (by you, for now).
4. **A record of the first decision** — ADR-0001 explaining the language choice, setting the precedent that decisions are written down.

---

## 2. Success Criteria

Phase 0 is **done** when, and only when, every row below is true.

| # | Criterion | Measured How |
|---|-----------|--------------|
| 1 | `git clone && cargo build` succeeds in ≤ 10 minutes on a fresh macOS/Linux/Windows laptop | Time a volunteer or fresh VM |
| 2 | CI green on `main` for ≥ 5 consecutive days with no human intervention | GitHub Actions history |
| 3 | README renders cleanly on GitHub, explains the project in ≤ 90 seconds of reading | External reader test |
| 4 | CONTRIBUTING.md walks a new contributor from zero to merged PR | Walkthrough test |
| 5 | ADR-0001 merged; ADR template exists for future decisions | `docs/adr/` |
| 6 | mdBook site live at a URL (GitHub Pages OK) | Published link |
| 7 | Discord server active with announcement channel and at least 10 members | Member count |
| 8 | Public Twitter/X or equivalent announcement thread published | Post link |
| 9 | Trademark search completed for "Krate" (filing deferred) | Written summary in `docs/legal/` |
| 10 | At least one external contributor has opened and merged a PR | Git log |

---

## 3. What Phase 0 Is and Is Not

### 3.1 Phase 0 IS

- Repo scaffolding, licensing, CI.
- Community and documentation infrastructure.
- A trademark search and a public name.
- A tone of voice for the project — welcoming, technical, honest.
- The first ADR and the precedent that decisions are written.
- Five days of actual work spread across four calendar weeks.

### 3.2 Phase 0 is NOT

- Writing runtime code. Not one line. That's Phase 1.
- Designing the UAPI. That's Phase 2.
- Hiring. That's post-Phase-1.
- Fundraising. That's post-Phase-2.
- Trademark filing. Only the *search*. Filing costs money and requires decisions better made at Phase 6.
- Marketing a product. We're announcing a project. There is no product yet.
- Sprinting. The work is deliberately slow to let the community gestate.

### 3.3 The "why four weeks for five days of work" question

Three reasons the calendar is longer than the engineering budget:

1. **Trademark search** involves waiting: for attorney responses or for USPTO search results if you DIY.
2. **Community seeding** takes time. Discord with zero members for two weeks before Phase 1 is fine; Discord with zero members going into Phase 1 is not.
3. **You will realize things.** Writing a README out loud clarifies the project in ways you can't anticipate. Drafts improve across multiple sittings, not within one.

If you compress Phase 0 to one week, you will rewrite half of it in Phase 2 when reality has taught you something.

---

## 4. The Repo as Architecture

Phase 0 has almost no running code, so its "architecture" is the repository structure. Get this right now and it scales through v1.0.

### 4.1 End-of-Phase-0 folder layout

```
krate/
├── .github/
│   ├── workflows/
│   │   └── ci.yml                        # fmt + clippy on empty workspace
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.md
│   │   ├── feature_request.md
│   │   └── config.yml
│   └── PULL_REQUEST_TEMPLATE.md
├── .gitattributes
├── .gitignore
├── Cargo.toml                            # empty workspace (no members yet)
├── rust-toolchain.toml
├── deny.toml
├── LICENSE-MIT
├── LICENSE-APACHE
├── README.md
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md
├── SECURITY.md
├── CHANGELOG.md
├── docs/
│   ├── adr/
│   │   ├── README.md                     # explains what an ADR is
│   │   ├── template.md
│   │   └── 0001-rust-for-runtime.md
│   ├── book/
│   │   ├── book.toml
│   │   └── src/
│   │       ├── SUMMARY.md
│   │       ├── introduction.md
│   │       ├── vision.md
│   │       └── roadmap.md
│   └── legal/
│       └── trademark-search.md
└── scripts/
    └── setup.sh                          # optional: one-shot dev env setup
```

Note what's absent: no `crates/`, no `wit/`, no `test/`. Those appear in Phase 1. A contributor who looks at this layout should see "serious, empty" — not "half-built." The emptiness is a feature.

### 4.2 Repo discoverability

The directory structure matters, but the *metadata* matters more. By end of Phase 0 the repo's GitHub "About" panel must have:

- A one-line description.
- A website link (docs site).
- Topics: `webassembly`, `wasm`, `cross-platform`, `runtime`, `rust`.
- A social preview image (logo or title card — can be simple).

Most OSS discovery happens through GitHub search and topic pages. Setting these five fields costs five minutes and pays off for the life of the project.

---

## 5. Technology Decisions

Phase 0 makes the tooling decisions that Phase 1 will inherit. Get these right and you rarely revisit them.

### 5.1 License: **MIT OR Apache-2.0 dual-license**

- Matches the Rust ecosystem convention (`cargo` itself, most major Rust crates).
- Apache-2.0 provides an explicit patent grant; MIT is maximally permissive.
- Users pick the license most convenient to them.
- Contributors are asked to dual-license their contributions via a short note in `CONTRIBUTING.md` (no CLA, no DCO unless needed later).

### 5.2 Source host: **GitHub**

- Widest reach for OSS discovery.
- Actions, Pages, Discussions, Releases all in one place.
- The alternative (self-hosted Gitea, Codeberg) wins on ideology and loses on contributor reach by a factor of ten.

### 5.3 CI: **GitHub Actions**

- Free matrix runners across Linux, macOS, Windows.
- No separate account to manage.
- Handoff path to self-hosted runners later (Phase 6+) when we need arm64 Linux.

### 5.4 Docs site: **mdBook**

- Rust-ecosystem-native.
- Fast, simple, produces a clean static site.
- Hosts fine on GitHub Pages for free.
- Alternatives considered: Docusaurus (too JS-heavy for a systems project), Zola (fine, less Rust-affinity).

### 5.5 Community: **Discord**

- Where developers actually are in 2026.
- Matrix is philosophically preferable but has a fraction of the reach.
- Plan to mirror critical announcements to GitHub Discussions (async, searchable).

### 5.6 No CLA (Contributor License Agreement)

- Dual MIT/Apache license + a simple in-file "contributions are dual-licensed" notice is enough for now.
- CLAs deter first-time contributors. We cannot afford that in Phase 0.
- Revisit only if Anthropic-scale legal counsel says we must.

### 5.7 No DCO (Developer Certificate of Origin) yet

- DCO requires sign-off on every commit, which breaks GitHub's web UI merge flow.
- Adopt only if a specific threat (copyleft contamination, enterprise lawyers) makes it necessary.

### 5.8 Versioning: **Semantic Versioning 2.0**

- Pre-1.0: breaking changes allowed in minor releases.
- Post-1.0 (end of Phase 7): strict semver.

### 5.9 Commit style: **Conventional Commits (loose)**

- `type(scope): subject` format: `feat(runtime): add fuel metering`.
- Enforced by PR title lint, not per-commit hook (preserves web UI workflow).
- Types used: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `build`, `ci`.

### 5.10 Branch model: **Trunk-based with short-lived feature branches**

- `main` is always green.
- Feature branches named `p{N}-{area}-{task}` (e.g., `p1-rt-01-runtime-crate`).
- Squash merge to `main`.
- No `develop` branch, no release branches until Phase 6.

---

## 6. Week-by-Week Breakdown

Sized for a founder working ~10–15 h/week on Krate alongside ParkSure.

### Week 1 — Repo skeleton

**Goal:** A clean, professional-looking public repo exists.

- Create `krate` GitHub organization.
- Create `krate/krate` public repo.
- Commit: licenses, README draft, CODE_OF_CONDUCT, SECURITY, .gitignore, .gitattributes.
- Initialize empty Cargo workspace.
- Push. Make sure GitHub renders it sensibly.
- Don't announce publicly yet.

### Week 2 — CI + Docs scaffolding

**Goal:** CI is green and the docs site is live.

- Write `ci.yml` that runs fmt + clippy on the empty workspace.
- Enable branch protection on `main` (requires CI green + 1 review).
- Scaffold mdBook under `docs/book/`.
- Set up GitHub Pages deployment from the `gh-pages` branch.
- Write the introduction + vision chapters (copy from main Build Plan §1–§2).
- Do a dry-run clone on a fresh VM: does `cargo build` work in under 10 minutes?

### Week 3 — Documents that matter

**Goal:** Anyone who lands on the repo knows what it is, how to help, and why it's safe to engage.

- Finalize README (see §8.1 template — not a draft, the real thing).
- Write CONTRIBUTING.md (see §8.3).
- Write SECURITY.md (see §8.4).
- Write ADR-0001 in full (see §12).
- Create issue + PR templates.
- Create labels.
- Create the first issues: 3–5 "good first issue" tickets covering docs polish, CI improvements, or scaffolding work that a newcomer could realistically finish in ≤ 2 hours.

### Week 4 — Community + legal + announcement

**Goal:** The project exists in public.

- Create Discord server (channels per §10).
- Write contributor welcome message (see §10.4).
- Complete trademark search (see §11.1).
- Draft and publish the announcement blog post + Twitter/X thread (see §8.9, §8.10).
- Invite first five contributors personally — people you already know — and get one of them to file a real PR.
- Retrospective: what took longer than planned; update this document.

---

## 7. Task Details

### P0-REPO-01 — Initialize monorepo, licenses, README

**Estimate:** 0.5 day.
**Branch:** direct to `main` (bootstrap commits only; future work uses branches).

**Acceptance:**
- Repo exists at `github.com/krate/krate` (or your chosen org), public.
- `LICENSE-MIT` and `LICENSE-APACHE` committed at root.
- README.md committed with the template from §8.1.
- `.gitignore` and `.gitattributes` present.
- At least one commit signed (GPG or Sigstore).

**Gotchas:**
- `.gitattributes` must set `* text eol=lf` to avoid Windows line-ending CI hell later.
- Choose the org name carefully — you cannot rename it painlessly later.

### P0-REPO-02 — rust-toolchain, Cargo workspace, cargo-deny

**Estimate:** 0.5 day.
**Branch:** `p0-repo-02-workspace`.

**Acceptance:**
- `rust-toolchain.toml` pins a specific stable version.
- `Cargo.toml` at root is a valid empty workspace.
- `deny.toml` configures advisories and license rules.
- `cargo build` succeeds (produces nothing — workspace is empty — but doesn't error).

### P0-REPO-03 — GitHub Actions: fmt, clippy, test

**Estimate:** 1 day.
**Branch:** `p0-repo-03-ci`.

**Acceptance:**
- `.github/workflows/ci.yml` runs on PR and push to `main`.
- Jobs: `fmt`, `clippy -- -D warnings`, `test`, `deny`.
- All four green on the empty workspace.
- Branch protection enabled on `main`: require CI pass, require 1 review, no force-push.

### P0-DOCS-01 — mdBook site with first chapter

**Estimate:** 1 day.
**Branch:** `p0-docs-01-mdbook`.

**Acceptance:**
- `docs/book/book.toml` configured with project title, author, repo URL.
- `docs/book/src/SUMMARY.md` lists: Introduction, Vision, Roadmap, Contributing.
- Chapters have content (not `Coming soon`).
- GitHub Pages deploys from a workflow; visible URL.

### P0-DOCS-02 — ADR template + ADR-0001

**Estimate:** 0.5 day.
**Branch:** `p0-docs-02-adr`.

**Acceptance:**
- `docs/adr/README.md` explains what ADRs are and how to write one.
- `docs/adr/template.md` is the copy-paste template.
- `docs/adr/0001-rust-for-runtime.md` contains the full ADR from §12.

### P0-DOCS-03 — CONTRIBUTING.md

**Estimate:** 0.5 day.
**Branch:** `p0-docs-03-contributing`.

**Acceptance:**
- CONTRIBUTING.md covers: build from source, run tests, commit style, PR flow, code of conduct link, "good first issue" pointer.
- Includes a short note on dual-licensing of contributions.
- Ends with "questions? join #help on Discord."

### P0-COMM-01 — Discord server

**Estimate:** 0.5 day.

**Acceptance:**
- Discord server created.
- Channels: `#welcome`, `#announcements`, `#general`, `#dev`, `#rfc`, `#help`, `#off-topic`.
- `#welcome` has pinned rules and role self-assign.
- Invite link in README.
- Server has a recognizable icon.

### P0-COMM-02 — Announcement thread draft

**Estimate:** 0.5 day.

**Acceptance:**
- Twitter/X thread drafted in `docs/legal/launch/twitter-thread.md` (kept private until Week 4).
- Announcement blog post drafted in `docs/book/src/blog/0001-announcing-krate.md`.
- Reviewed by at least one trusted outsider before publication.

### P0-LEGAL-01 — Trademark search

**Estimate:** 0.5 day (active; elapsed may be longer).

**Acceptance:**
- USPTO TESS search for "Krate" and close variants — written results in `docs/legal/trademark-search.md`.
- EUIPO (European Union Intellectual Property Office) search — same file.
- Japan, China, India — same file (these are Krate's likely early non-US markets).
- Decision recorded: proceed with name, pick alternate, defer until Phase 6.
- **No filing.** Search only.

### P0-HIRE-01 — First contributor guide

**Estimate:** 0.5 day.

**Acceptance:**
- `docs/book/src/contributing/first-pr.md` walks a reader from "I cloned the repo" through "my PR merged."
- Includes screenshots.
- Lists 3–5 "good first issue" tickets by link.
- Cross-linked from README and CONTRIBUTING.

---

## 8. Document Templates

These are not drafts. Paste them and adjust only the bracketed placeholders.

### 8.1 README.md

```markdown
# Krate

> Write once. Run on everything. Natively.

Krate is a universal application platform — a portable runtime, a universal
standard library, and a capability-based permission model — that lets you ship
one binary and run it natively on Windows, macOS, Linux, iOS, Android, and the
web.

It is built on WebAssembly and its Component Model, with a thin per-OS adapter
layer that translates calls to native APIs.

**Status:** Pre-alpha. Phase 0 (Foundation). Not yet usable.
See [the roadmap](https://krate.github.io/krate/roadmap.html).

## Why

Every app today is written six times: once for each operating system it runs
on. That is a tax on every developer and a ceiling on every idea. Krate removes
the tax by making the developer's target a portable runtime, not any particular
OS.

Read [the full vision](https://krate.github.io/krate/vision.html).

## Current phase

**Phase 0 — Foundation.** Repo scaffolding, CI, community, first decisions.
No runnable code yet. Phase 1 begins when the checklist in
[`docs/book/src/roadmap.html`](https://krate.github.io/krate/roadmap.html) is
green.

## Quickstart

Nothing to run yet. When Phase 1 ships (ETA: ~8 weeks):

```bash
# Install krate
# (instructions here in Phase 1)

# Run your first component
krate run hello.wasm
```

## Project structure

- `crates/` — Rust crates (appears in Phase 1).
- `wit/` — WebAssembly Interface Types definitions (appears in Phase 1).
- `docs/` — documentation, ADRs, and the mdBook site source.
- `test/` — integration tests (appears in Phase 1).

## Contributing

We want you. Read [CONTRIBUTING.md](CONTRIBUTING.md) and drop by
[Discord](https://discord.gg/REPLACE). Good first issues are labeled
[`good first issue`](https://github.com/krate/krate/labels/good%20first%20issue).

## License

Dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

Contributions are dual-licensed under the same terms.

## Acknowledgements

Krate stands on the shoulders of the Bytecode Alliance, the Rust Foundation,
and everyone else building the open WebAssembly ecosystem.
```

### 8.2 LICENSE-MIT / LICENSE-APACHE

Copy verbatim from:

- <https://opensource.org/license/mit/>
- <https://www.apache.org/licenses/LICENSE-2.0.txt>

Change only the copyright line in MIT:

```
Copyright (c) 2026 Krate contributors
```

### 8.3 CONTRIBUTING.md

```markdown
# Contributing to Krate

Thanks for your interest. Krate is young and every contribution compounds.

## Before you start

1. Read the [Code of Conduct](CODE_OF_CONDUCT.md). We enforce it.
2. Skim the [vision and roadmap](https://krate.github.io/krate/).
3. Find a `good first issue` if you're new:
   https://github.com/krate/krate/labels/good%20first%20issue

## Development setup

Install prerequisites:

- Rust stable (via [rustup](https://rustup.rs/))
- Git

Then:

```bash
git clone https://github.com/krate/krate.git
cd krate
cargo build
cargo test
```

That should work in under 10 minutes on a fresh laptop. If it doesn't,
open an issue — that's a bug.

## Making a change

1. Fork the repo.
2. Create a branch named `p{phase}-{area}-{short-description}` — for
   example `p1-rt-02-component-loader`. The phase and area come from the
   task ID in the Build Plan.
3. Write your change. Keep PRs small and focused.
4. Run `cargo fmt && cargo clippy -- -D warnings && cargo test`.
5. Commit with a [Conventional Commits](https://www.conventionalcommits.org/)
   style message: `feat(runtime): add fuel metering`.
6. Open a PR. Reference the task ID (e.g. "Closes P1-RT-04") in the
   description.

Every PR needs:

- Green CI.
- At least one maintainer approval.
- A clear description of what changed and why.

## Commit style

- Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `build`, `ci`.
- Imperative mood: "add" not "added."
- Lower-case subject.
- Body explains *why* more than *what* (the diff shows what).

## Licensing of contributions

By contributing, you agree that your contributions will be dual-licensed
under the [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE) licenses, the
same as the rest of the project. No separate CLA is required.

## Decision-making (ADRs)

Significant technical decisions are recorded as Architecture Decision
Records in `docs/adr/`. If your change makes a decision that affects
multiple crates or future phases, open an ADR alongside your PR.

See `docs/adr/README.md` for the process.

## Where to ask questions

- `#help` on Discord: https://discord.gg/REPLACE
- GitHub Discussions for long-form topics.

Don't be shy. The maintainers were all new once, and we want to keep
that pipeline alive.
```

### 8.4 SECURITY.md

```markdown
# Security Policy

## Supported versions

Krate is pre-alpha (Phase 0). No versions are supported for production use.

When v1.0 ships, we will maintain the latest minor version with
security patches. Older minor versions will receive patches for 12
months after the next minor release.

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Email `security@krate.example` (REPLACE with real address once set up).

If you prefer encrypted communication, the PGP key fingerprint is:

```
REPLACE with fingerprint
```

Full key at `.github/security-pgp.asc`.

We will acknowledge receipt within 72 hours and provide an initial
assessment within 7 days.

## Disclosure timeline

We follow coordinated disclosure. Default timeline:

- Day 0: report received, acknowledged.
- Day 7: initial assessment shared with reporter.
- Day 30: fix in development if confirmed.
- Day 90: public disclosure, with or without fix (whichever comes first,
  unless extenuating circumstances).

Credit is given to reporters unless they request otherwise.

## Bug bounty

No monetary bounty yet. We will add one when we have the funding to do
so responsibly (estimated: Phase 6).

## Scope

Phase 0 has no runnable code. The threat surface is repo metadata
(licenses, workflows, dependency declarations). Phase 1 will publish
the first meaningful threat model.
```

### 8.5 CODE_OF_CONDUCT.md

Use [Contributor Covenant 2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) verbatim. Set the enforcement contact to the same address as SECURITY.md plus a separate `conduct@` mailbox for conduct-only reports.

### 8.6 CHANGELOG.md (starter)

```markdown
# Changelog

All notable changes to Krate are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Initial repo scaffolding (Phase 0).
- Build plan and phase documents.
- mdBook documentation site.
- CI workflow.
- Dual MIT / Apache 2.0 license.

[Unreleased]: https://github.com/krate/krate/compare/main...HEAD
```

### 8.7 `.gitattributes`

```
* text=auto eol=lf

*.png binary
*.jpg binary
*.pdf binary
*.wasm binary
*.ico binary
```

### 8.8 `.gitignore`

```
/target
/dist
Cargo.lock.bak

# IDE
.vscode/
.idea/
*.swp
*.swo

# OS
.DS_Store
Thumbs.db

# mdBook
/docs/book/book/

# Benchmarks
/criterion/
```

### 8.9 Announcement blog post (template)

Publish as `docs/book/src/blog/0001-announcing-krate.md` at the end of Week 4.

```markdown
# Announcing Krate

An app today is written six times. Once for Windows, once for macOS, once for
Linux, once for iOS, once for Android, once for the web. Every time a
developer has an idea, the tax of shipping it to everyone who might use it
turns that idea into a small company's worth of work.

I've paid this tax. You've paid this tax. The industry pays a cumulative
trillion-dollar tax every year, and the result is that a huge fraction of
good ideas never reach users because rewriting them five times wasn't
worth it.

Krate is my attempt to stop paying that tax.

## What it is

Krate is a universal application platform — one bytecode, one standard
library, one permission model — that lets a developer write an app once
and have it run natively on every operating system.

It's built on WebAssembly and its Component Model. It is not a browser.
It is not Electron. It is not a cross-compiler. It's a runtime you
install on each host, and a binary format every host's runtime can
execute.

## Where it is

Phase 0. Foundation. A repo, a CI pipeline, a documentation site, a
Discord, and a plan. No runnable code yet — Phase 1 starts in ~4 weeks
and the first `krate run hello.wasm` follows soon after.

I'm posting this because the best time to find collaborators is before
the code exists, not after.

## What I'm asking for

Stars, for visibility:
https://github.com/krate/krate

Eyes on the 24-month plan:
https://krate.github.io/krate/roadmap

People who want to build systems-level software with an actual shot
at reshaping how apps get deployed:
https://discord.gg/REPLACE

If this resonates, or if you think I'm wrong, I want to hear from you.

— [Your name], founder
```

### 8.10 Twitter / X announcement thread (template)

```
1/ Launching Krate.

Apps today are written 6 times — once per OS. That's a trillion-dollar
tax on the software industry every year.

Krate is a universal platform that lets you write an app once and run it
natively everywhere.

2/ Not Electron. Not a browser. Not a cross-compiler.

A real runtime you install per-OS + a bytecode format every runtime can
execute. Built on WebAssembly + Component Model.

3/ Status: Phase 0 of 8. Foundation.
Repo, CI, docs, Discord — up now.

Phase 1 starts in 4 weeks. First `krate run hello.wasm` shortly after.

4/ Target v1.0: 24 months.

By then: one .l36app runs on Windows, macOS, Linux, iOS, Android, and
the web. Natively.

First anchor tenant: @ParkSure — 6 client apps → 1 codebase.

5/ Why share before there's code?

Because platforms succeed when they accrete people, and people need
time to learn a thing exists.

Also: if I'm wrong, I'd rather hear it now than in month 18.

6/ Roadmap: [link]
Repo: [link]
Discord: [link]

Would love feedback, contributors, and people who want to build real
systems software for a while.
```

### 8.11 GitHub issue templates

`.github/ISSUE_TEMPLATE/bug_report.md`:

```markdown
---
name: Bug report
about: Something is broken
labels: bug
---

## What happened

A clear description of what you observed.

## What you expected

What you expected instead.

## Steps to reproduce

1.
2.
3.

## Environment

- OS:
- `krate --version`:
- `rustc --version`:
```

`.github/ISSUE_TEMPLATE/feature_request.md`:

```markdown
---
name: Feature request
about: Suggest an idea
labels: enhancement
---

## Problem

The underlying problem, not the solution.

## Proposed solution

What you'd like to see.

## Alternatives considered

Other approaches and why they're worse.

## Additional context

Links, prior art, related issues.
```

`.github/ISSUE_TEMPLATE/config.yml`:

```yaml
blank_issues_enabled: false
contact_links:
  - name: Discord
    url: https://discord.gg/REPLACE
    about: Questions and general discussion
  - name: GitHub Discussions
    url: https://github.com/krate/krate/discussions
    about: Long-form design topics
```

### 8.12 Pull request template

`.github/PULL_REQUEST_TEMPLATE.md`:

```markdown
## Summary

<!-- 1–3 sentences describing the change. -->

## Related

<!-- Link to the task ID in the Build Plan (e.g., P1-RT-02)
     or to any relevant issue. -->

Closes #

## Testing

<!-- How did you test this? What did you run? -->

## Checklist

- [ ] `cargo fmt` applied
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Documentation updated (if user-facing change)
- [ ] CHANGELOG.md updated under `[Unreleased]` (if user-facing change)
- [ ] New public API has doc comments with examples
```

### 8.13 Labels

Create these labels in the GitHub UI at the start of Phase 0 and use them consistently from the first issue onward:

| Label | Color | Purpose |
|---|---|---|
| `bug` | red | Defect in existing behavior |
| `enhancement` | blue | New feature or improvement |
| `docs` | light blue | Documentation only |
| `good first issue` | green | Suitable for newcomers |
| `help wanted` | dark green | We would like assistance |
| `phase:0` .. `phase:7` | purple shades | Which phase this belongs to |
| `area:runtime` | yellow | The runtime crate |
| `area:cli` | yellow | The CLI |
| `area:uapi` | yellow | UAPI design |
| `area:docs` | yellow | Docs site + ADRs |
| `area:ci` | yellow | CI/CD |
| `area:community` | yellow | Community / governance |
| `priority:high` | orange | Should be addressed soon |
| `priority:low` | gray | Can wait |
| `blocked` | black | Waiting on something external |
| `question` | white | Not an actionable issue |

### 8.14 rust-toolchain.toml

```toml
[toolchain]
channel    = "1.83.0"
components = ["rustfmt", "clippy"]
profile    = "minimal"
```

Update the channel when you deliberately bump MSRV. Never floating.

### 8.15 Root Cargo.toml (empty workspace)

```toml
[workspace]
resolver = "2"
members  = []

[workspace.package]
edition      = "2021"
version      = "0.0.0"
license      = "MIT OR Apache-2.0"
repository   = "https://github.com/krate/krate"
rust-version = "1.83"
```

### 8.16 deny.toml

```toml
[advisories]
vulnerability = "deny"
unmaintained  = "warn"
yanked        = "warn"
notice        = "warn"
ignore        = []

[licenses]
allow = [
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-DFS-2016",
]
copyleft    = "deny"
unlicensed  = "deny"

[bans]
multiple-versions = "warn"
wildcards         = "deny"

[sources]
unknown-registry = "deny"
unknown-git      = "warn"
```

### 8.17 book.toml (mdBook config)

```toml
[book]
title    = "Krate"
authors  = ["Krate contributors"]
language = "en"
src      = "src"

[output.html]
git-repository-url = "https://github.com/krate/krate"
edit-url-template  = "https://github.com/krate/krate/edit/main/docs/book/{path}"
site-url           = "/krate/"
default-theme      = "ayu"
preferred-dark-theme = "ayu"
```

### 8.18 SUMMARY.md (mdBook table of contents, Phase 0 version)

```markdown
# Summary

[Introduction](./introduction.md)

# Foundation

- [Vision](./vision.md)
- [Roadmap](./roadmap.md)

# Contributing

- [How to contribute](./contributing/index.md)
- [Your first PR](./contributing/first-pr.md)
- [Code of Conduct](./contributing/conduct.md)

# Decisions

- [About ADRs](./decisions/index.md)

# Blog

- [Announcing Krate](./blog/0001-announcing-krate.md)
```

---

## 9. CI Setup

### 9.1 `.github/workflows/ci.yml`

```yaml
name: CI

on:
  pull_request:
  push:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -D warnings

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt
      - run: cargo fmt --all -- --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace

  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v1

  book:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install mdBook
        run: cargo install mdbook --version 0.4.40 --locked
      - name: Build book
        run: mdbook build docs/book
```

Phase 1 expands this matrix to all three operating systems. Phase 0 only needs Linux — the workspace is empty, so cross-platform builds add noise without signal.

### 9.2 `.github/workflows/pages.yml`

```yaml
name: Deploy mdBook to Pages

on:
  push:
    branches: [main]
    paths: ['docs/book/**']
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: false

jobs:
  deploy:
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install mdBook
        run: cargo install mdbook --version 0.4.40 --locked
      - run: mdbook build docs/book
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: docs/book/book
      - id: deployment
        uses: actions/deploy-pages@v4
```

### 9.3 Branch protection rules

In GitHub → Settings → Branches → Add rule for `main`:

- Require a pull request before merging — yes.
- Require approvals — 1.
- Dismiss stale approvals when new commits are pushed — yes.
- Require status checks to pass: `fmt`, `clippy`, `test`, `deny`, `book`.
- Require branches to be up to date before merging — yes.
- Include administrators — yes (keeps you honest).
- Allow force pushes — no.
- Allow deletions — no.

---

## 10. Community Setup

### 10.1 Discord server structure

Seven channels is the right number for Phase 0. More and the server looks empty; fewer and conversations get muddled.

| Channel | Purpose |
|---|---|
| `#welcome` | Rules, role self-assign, one-line intro expectation |
| `#announcements` | Read-only. New releases, milestones, talks |
| `#general` | Anything Krate-related not covered below |
| `#dev` | Technical discussion, code, design |
| `#rfc` | Long-form proposal discussion (mirror to GitHub Discussions) |
| `#help` | Questions from users and new contributors |
| `#off-topic` | Everything else |

### 10.2 Roles

- `@maintainer` — has merge authority. You, plus anyone you trust to merge.
- `@contributor` — self-assign after first merged PR.
- `@friend` — self-assign for anyone interested but not yet contributing.

### 10.3 Rules (pinned in `#welcome`)

```
1. Be respectful. The Contributor Covenant applies in every channel.
2. Don't ask to ask. Paste the error, paste the command, paste the code.
3. Use code blocks for code. Plain text is painful to read.
4. Keep threads on topic per channel.
5. No spam, no self-promo unless it's directly Krate-related.
6. English is the primary working language, but don't let that stop you —
   translators exist and we'll help.
7. If something makes you uncomfortable, DM a @maintainer. We follow up.
```

### 10.4 Welcome message

Automate with a bot (Carl-bot, MEE6, or custom). Fires on new member join:

```
Welcome to Krate, {user}. Glad you're here.

A few things to know:
• Read the pinned rules in #welcome.
• Skim the vision: https://krate.github.io/krate/vision
• Drop by #general and say hi — one line is enough.
• Questions go in #help. Code and design in #dev.

Everything here is still early. If you want to build the first cross-platform
runtime since the browser, you're in the right place.
```

### 10.5 Mirroring rule

Every substantive technical decision lives on GitHub (issue, PR, ADR, Discussion), not in Discord chat logs. Discord is synchronous and ephemeral; code and decisions need durable homes.

---

## 11. Legal & IP

Per the main plan, legal is mostly deferred. Phase 0 does the minimum necessary to not paint into corners later.

### 11.1 Trademark search

In `docs/legal/trademark-search.md`, record results of searches in at least:

- **USPTO (United States):** <https://tsdr.uspto.gov/>.
- **EUIPO (European Union):** <https://www.tmdn.org/tmview/>.
- **JPO (Japan):** <https://www.j-platpat.inpit.go.jp/>.
- **CNIPA (China):** <https://sbj.cnipa.gov.cn/>.
- **IP India:** <https://tmrsearch.ipindia.gov.in/>.

For each jurisdiction record: search date, exact query, any conflicting marks, class (software is usually Class 9 for goods, 42 for services). If "Krate" is taken in a way that conflicts (same or related class), decide: rename, qualify (e.g., "Krate Runtime"), or proceed knowing a dispute is possible.

**Filing is Phase 6, not Phase 0.** Filing too early wastes money if the project pivots on name; filing too late risks being blocked by a squatter. Phase 6 is when the marketplace launches and the name becomes an economic asset worth defending.

### 11.2 Domain names

Acquire at minimum `krate.dev` (primary) and `krate.com` if available within reasonable cost. Also register on common Indian and Singapore registrars (`.sg`, `.in`) since your business base is there.

Do NOT register 20 variant domains. Trademark rights cover variants without registration in most cases, and a forest of defensive domains is a tax forever.

### 11.3 License compatibility

- MIT OR Apache-2.0 is compatible with virtually every open-source license.
- Incompatible: strict GPL dependencies (no GPL in your crate graph).
- `cargo-deny`'s `copyleft = "deny"` catches this in CI.

### 11.4 Export control

WASM runtimes include cryptographic libraries. Under US export law (EAR), most open-source published crypto is exempt under License Exception TSU, but the exemption requires a notification email to BIS and Commerce Department at or before time of export. This is Phase 6 paperwork when crypto UAPI ships — noted here only so it's not forgotten.

### 11.5 Privacy

Phase 0 has no users and collects no data. No privacy policy needed. Re-evaluate at Phase 6 when the marketplace and identity system handle personal data.

### 11.6 Founder IP assignment

If you work on Krate from a device or account that your employer or academic institution (e.g., NTU) has IP claims on, resolve that in writing *before* the public announcement. Common paths:

- Personal-time + personal-device carve-out letter from employer.
- University "hobby project" disclosure form.
- Explicit written assignment to yourself from any entity that could claim rights.

Do this in Week 1. Retroactive fixes are expensive; preventive fixes are an email.

---

## 12. ADR-0001 in Full

Copy this verbatim into `docs/adr/0001-rust-for-runtime.md` and commit.

```markdown
# ADR-0001: Rust for the Krate runtime

**Status:** Accepted
**Date:** 2026-05-XX
**Authors:** @founder

## Context

The Krate runtime is a native binary that ships on every host operating
system we support (Windows, macOS, Linux, iOS, Android, browsers via
WASM). Its job is to load a WebAssembly component, enforce a capability
model, and dispatch UAPI calls to per-OS adapters.

The language we write it in is one of the most consequential early
decisions:

- It will never be rewritten at scale. We choose once.
- It shapes the contributor pool for the life of the project.
- It determines which WebAssembly runtime we can embed naturally.
- It affects binary size, startup time, and memory footprint — all of
  which matter for Phase 4 (mobile) and Phase 7 (v1.0).

## Decision

We write the Krate runtime in **Rust**.

Specifically:

- `crates/runtime/` — the core runtime library.
- `crates/cli/` — the `krate` command-line binary.
- `crates/host-adapter/*` — per-OS adapter crates.
- `crates/bundle/`, `crates/policy/`, and future crates.

Apps themselves are compiled from any language to WebAssembly; Rust is
the runtime's implementation language, not a constraint on apps.

## Alternatives considered

### C++

**Rejected.**

- Mature ecosystem, wide OS support.
- But: memory safety is manual. A runtime with a single memory-safety
  CVE undermines the trust the entire platform depends on.
- Build systems are a collective weakness (CMake, bazel, meson) —
  contributor friction is higher than Cargo.

### Zig

**Rejected (for now).**

- Compelling language: small, fast, simple.
- But: pre-1.0, stdlib churn is heavy, ecosystem for WebAssembly
  runtimes is immature.
- Revisit only if Rust becomes a limiting factor (no plausible path
  to that today).

### Go

**Rejected.**

- Excellent tooling, great concurrency.
- But: GC introduces latency spikes incompatible with 60fps UI on
  mobile (Phase 4).
- CGo overhead for embedding a WASM engine in native UIs is material.
- Binary size is larger than Rust for equivalent functionality.

### Kotlin / Swift

**Rejected.**

- Platform-biased (JVM, Apple). A truly cross-platform runtime
  shouldn't itself be bound to one ecosystem.
- Cross-compilation story for iOS and Android from anything but the
  respective first-party toolchain is fragile.

## Consequences

### Positive

- Memory safety without GC — a rare combination that exactly fits
  runtime requirements.
- Wasmtime, our chosen WebAssembly engine (see ADR-0002), is
  itself Rust. Embedding is native.
- `cargo` gives us a reproducible build system from day one.
- Contributor pool intersects strongly with systems-minded developers
  who want to build this kind of project.

### Negative

- Rust learning curve deters contributors without prior experience.
  Mitigation: strong onboarding docs, generous code review, "good
  first issue" pipeline.
- Compile times are non-trivial at scale. Mitigation: workspace
  structure, `sccache`, incremental compilation, careful dependency
  hygiene (`deny.toml`).
- Unsafe Rust will be necessary at adapter boundaries (FFI to Cocoa,
  Win32, JNI, Objective-C). This is a known cost; scoped and audited.

### Neutral

- Rust's ecosystem churn (toolchain releases every 6 weeks) requires
  discipline. We pin MSRV and bump deliberately.
- Some platforms (iOS, Android) require additional toolchain setup.
  Addressed phase-by-phase.

## Revisiting

This decision is revisitable only if:

1. A fundamental blocker emerges (e.g., Rust toolchain cannot target
   a platform we need to support).
2. A newer language offers meaningful advantages AND has the
   ecosystem maturity to compete with Rust for a 10-year project.

Neither condition is foreseeable in 2026. Expected lifetime of
this decision: decade-scale.

## References

- [Rust language](https://www.rust-lang.org/)
- [Wasmtime (Rust-embedded WASM runtime)](https://wasmtime.dev/)
- [ADR template](template.md)
```

---

## 13. Exit Criteria Checklist

### Repo
- [ ] `krate/krate` repository exists, public, MIT + Apache-2.0 dual-licensed.
- [x] Root Cargo workspace compiles.
- [x] `rust-toolchain.toml` pins stable.
- [x] `deny.toml` in place and passing.
- [x] `.gitattributes` set for LF line endings.
- [x] `.gitignore` complete.

### Documents
- [ ] README.md reviewed by one external reader and revised.
- [x] CONTRIBUTING.md walks a newcomer from zero to merged PR.
- [x] SECURITY.md published with contact.
- [x] CODE_OF_CONDUCT.md (Contributor Covenant 2.1) present.
- [x] CHANGELOG.md initialized.
- [x] ADR template in `docs/adr/template.md`.
- [x] ADR-0001 merged.

### CI
- [x] `ci.yml` runs fmt, clippy, test, deny, book on every PR.
- [ ] Branch protection active on `main` requiring CI green + 1 review.
- [ ] CI has been green on `main` for ≥ 5 consecutive days.

### Docs site
- [x] mdBook scaffolded with SUMMARY, Introduction, Vision, Roadmap chapters.
- [x] GitHub Pages deployed from workflow.
- [x] Live URL linked from README.

### Community
- [ ] Discord server live with all 7 channels.
- [ ] Welcome bot or pinned welcome message present.
- [ ] At least 10 members, including at least 3 people who are not you.
- [ ] Twitter/X or equivalent announcement thread published.

### Legal
- [x] Trademark search completed and written up in `docs/legal/`.
- [ ] Domain `krate.dev` (or chosen equivalent) secured.
- [x] Founder IP assignment status confirmed in writing.

### External signals
- [ ] At least one external contributor has opened a PR.
- [ ] At least one external contributor's PR has been merged.
- [x] First 5 "good first issues" created and labeled.

### Governance
- [x] Retrospective written (`docs/book/src/phase0/retro.md`).
- [x] Phase 1 kickoff issue opened referencing `Plan/Phase-1-Plan.md`.

---

## 14. Phase 0 Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| README or announcement lands flat — no one cares | Medium | Low | Phase 0 doesn't need to succeed virally. Ten engaged contacts > ten thousand indifferent impressions. |
| Name conflict discovered mid-Phase-0 | Medium | Medium | Do trademark search Week 1, not Week 4. Alternate names already in hand. |
| Discord becomes noisy before there's substance | Low | Medium | Limit promotion until Phase 1 has something runnable. Quality of early members > quantity. |
| Branch protection locks you out of your own repo during setup | Medium | Low | Enable branch protection *after* initial bootstrap commits. Keep "include administrators" off until the team grows. |
| CI fails in a way that's hard to debug because there's no code | Low | Low | Test on a throwaway fork first. |
| You spend three weeks perfecting the README | High | Medium | Time-box: README draft day 1, final day 5. Revisit after first external PR merges, not before. |
| Scope creep — "maybe Phase 0 should also have…" | High | Medium | Every Phase 0 PR cites a P0-* task ID. No task ID → defer to Phase 1 planning. |
| Trademark attorney fee ambush | Low | Medium | You are not filing. Search is free via public databases. Do not engage counsel until Phase 6 unless a specific conflict demands it. |
| Founder employer / university IP claim surfaces late | Low | High | Confirm status Week 1. Get it in writing. |
| ParkSure crunch eats Phase 0 entirely | High | High | Phase 0 is 5 engineering days. You can fit it into evenings over 4 weeks. If even that is impossible, Krate is not yet the right moment — defer, don't half-ship. |

---

## 15. Handoff to Phase 1

### 15.1 What Phase 1 inherits

- Working repo with CI green.
- License, contributor docs, code of conduct.
- mdBook site and ADR pipeline.
- Discord + basic community.
- The decision record discipline — every Phase 1 major call gets an ADR.

### 15.2 What Phase 1 replaces

| Thing | Why replaced |
|---|---|
| Empty `Cargo.toml` workspace | Will gain `crates/runtime/` and `crates/cli/` as members |
| Linux-only CI matrix | Expands to macOS and Windows |
| "Phase 0" labels on issues | Rotated to "phase:1" for all active work |
| "No code yet" disclaimers in README | Replaced with real quickstart |

### 15.3 What Phase 1 must NOT touch

- License terms — no changing MIT/Apache mid-flight.
- The ADR process itself.
- Existing ADRs (supersede via new ADRs if needed; never edit in place).
- The CONTRIBUTING flow once contributors have relied on it.

### 15.4 Phase 1 kickoff procedure

At the end of Week 4:

1. Open an issue titled "Phase 1 kickoff: POC Runtime" referencing `META-OS-Phase-1-Plan.md`.
2. Create all Phase 1 task issues (P1-RT-01 through P1-SEC-01) with the `phase:1` label.
3. Post in `#announcements` and on the blog: "Phase 0 complete; Phase 1 starting."
4. Walk through the Phase 1 plan doc with any collaborators who've arrived during Phase 0.
5. Begin Week 1 of Phase 1 (Wasmtime spike) on schedule.

---

## 16. Appendices

### Appendix A — First-week daily checklist

**Monday (Day 1)**
- [ ] Create `krate` GitHub org.
- [ ] Create `krate/krate` public repo.
- [ ] Push `LICENSE-MIT`, `LICENSE-APACHE`, initial README, .gitignore, .gitattributes.
- [ ] Confirm employer / university IP status.

**Tuesday (Day 2)**
- [x] Write `rust-toolchain.toml`, root `Cargo.toml`, `deny.toml`.
- [x] `cargo build` works on empty workspace.
- [x] Start trademark search.

**Wednesday (Day 3)**
- [x] Write SECURITY.md, CODE_OF_CONDUCT.md, CHANGELOG.md.
- [ ] GitHub Actions CI (`ci.yml`) green.
- [ ] Branch protection on `main`.

**Thursday (Day 4)**
- [x] Issue + PR templates.
- [x] Labels created.
- [x] 5 "good first issue" tickets drafted.

**Friday (Day 5)**
- [x] Retrospective on Week 1.
- [x] Plan Week 2 (CI + Docs scaffolding).

Weeks 2, 3, 4 follow the pattern in §6. The daily cadence exists only in Week 1 because Week 1 establishes the habits that carry the project.

### Appendix B — Contributor welcome PR (your own)

Your *first* contributor — before anyone external shows up — is you, acting as a contributor rather than as a founder. Make one PR in Week 2 that:

- Uses a feature branch (not direct to main).
- Has a Conventional Commit title.
- References a task ID.
- Gets self-merged after CI green.

This seeds the git log with the pattern future contributors imitate. Every PR after this one will look similar because the first one did.

### Appendix C — What to announce vs what to defer

| Announce in Phase 0 | Defer |
|---|---|
| The vision and the plan | Specific API commitments |
| The problem you're solving | Who the enterprise customers will be |
| The roadmap | ETAs for phases beyond Phase 1 |
| The license | Pricing (there is none) |
| The team (you) | Hiring plans |
| The Discord | How to "get access" (nothing to gate yet) |

Under-promise hard in Phase 0. There are no enterprise customers. There is no fundraise. There is no team. Claiming any of those forfeits trust when week 12 arrives and they still don't exist.

### Appendix D — When to bend these rules

Every item in this document has a reason, but the reasons aren't absolute. Situations where you can deviate:

- **Your employer's IP policy forces GPL.** Then the whole project structure changes; open an ADR, not a workaround.
- **The name "Krate" is clearly blocked everywhere.** Pick a new name in Week 1. The sooner, the cheaper.
- **A major contributor wants to join on day 3.** Great — expand Phase 0 to include making them a maintainer properly (roles, trust, onboarding). Don't just wave them in.
- **ParkSure becomes unexpectedly funded in Week 2.** Pause Krate; do ParkSure. Come back when the commitment level is sustainable. Krate is a decade project; a two-month pause is nothing.

### Appendix E — Retrospective template

Save as `docs/book/src/phase0/retro.md` at the end of Phase 0.

```markdown
# Phase 0 Retrospective

**Planned:** 4 weeks / **Actual:** <X> weeks
**Written:** YYYY-MM-DD
**Author:** @handle

## What shipped
- …

## What didn't ship and why
- …

## External signals
- GitHub stars: X
- Discord members: X
- PRs merged from non-founder: X
- First external contributor's story (1 paragraph): …

## What I'd do differently
- …

## What I'm glad I did
- …

## Concrete changes to the main Build Plan
- …

## Concrete changes to the Phase 1 plan before starting it
- …
```

---

---

## Development Log

> **Phase Status:** Mostly done; external gates pending
> **Started:** 2026-05-01
> **Completed:** pending external gates
> **Last Updated:** 2026-05-02

### Progress Summary

_Phase 0 is underway. Repo scaffolding, CI, documentation, ADR-0001, local verification, GitHub bootstrap, Pages publication, and the Krate naming pivot are now recorded. The development repository is `incyashraj/krate`; Krate is the product name that the 6x6 matrix grows into. Remaining Phase 0 work is mostly external: public visibility/social preview, branch protection setup now that CI is green, Discord, official trademark/domain work, and first external contributor PR._

---

### Exit Criteria Status

Full criteria in [§2 Success Criteria](#2-success-criteria). Check off as each criterion is met.

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `git clone && cargo build` succeeds in ≤ 10 min on a fresh machine | Initial workspace pushed to GitHub; fresh-machine clone timing pending |
| 2 | CI green on `main` for ≥ 5 consecutive days without human intervention | First green `main` CI confirmed on 2026-05-03; 5-day window pending |
| 3 | README renders cleanly on GitHub; project explained in ≤ 90 seconds | Draft complete; external reader pending |
| 4 | CONTRIBUTING.md walks a contributor from zero to merged PR | Draft complete |
| 5 | ADR-0001 merged; ADR template exists in `docs/adr/` | Present on `main` |
| 6 | mdBook site live at a public URL (GitHub Pages OK) | Live at `https://incyashraj.github.io/krate/` |
| 7 | Discord server active with ≥ 10 members | Not done |
| 8 | Public Twitter/X announcement thread published | Not done |
| 9 | Trademark search completed for “Krate” (no filing, search only) | Preliminary screen done; official searches pending |
| 10 | At least one external contributor PR merged | Not done |

---

### Completed Tasks

| Task ID | Task | Completed | Notes |
|---------|------|-----------|-------|
| P0-REPO-01 | Initialize monorepo, licenses, README | 2026-05-02 | Done locally. |
| P0-REPO-02 | Rust toolchain, Cargo workspace, cargo-deny | 2026-05-02 | Added root sentinel crate so baseline Cargo commands pass before runtime crates exist. |
| P0-REPO-03 | GitHub Actions: fmt, clippy, test, deny | 2026-05-02 | Configured CI and added mdBook job. |
| P0-DOCS-01 | mdBook site scaffold | 2026-05-02 | Builds locally with `mdbook build docs/book`. |
| P0-DOCS-02 | ADR template + ADR-0001 | 2026-05-02 | Present in `docs/adr/`. |
| P0-DOCS-03 | CONTRIBUTING.md | 2026-05-02 | Draft complete with setup, PR flow, ADRs, and licensing. |
| P0-LEGAL-01 | Trademark search | 2026-05-02 | OneOS rejected due conflicts; Krate selected pending official clearance. |
| P0-HIRE-01 | First contributor guide and starter issues | 2026-05-02 | Five good-first issues opened as GitHub issues `#2`, `#3`, `#4`, `#5`, and `#7`. |

---

### In Progress

| Task ID | Task | Started | Blockers |
|---------|------|---------|----------|
| P0-COMM-01 | Discord server | 2026-05-02 | Requires account setup. Checklist drafted in `docs/community/discord-setup.md`. |
| P0-COMM-02 | Announcement draft | 2026-05-02 | Draft exists; publish after official naming/docs/community readiness. |
| P0-HIRE-01 | First contributor guide | 2026-05-02 | Guide drafted; screenshots pending live GitHub/Pages. |

---

### ADRs Filed This Phase

| ADR | Title | Status | Merged |
|-----|-------|--------|--------|
| ADR-0001 | Rust for the runtime | Accepted locally | — |

---

### Blockers & Open Questions

- Official trademark searches for Krate remain pending.
- GitHub branch protection, public visibility, and social preview still require deliberate repository owner setup.
- Discord creation and public launch should wait until official name/domain decisions are comfortable.
- First external contributor PR cannot be completed locally.

---

### Notes & Learnings

- 2026-05-02: `OneOS` was rejected as final project name after preliminary search surfaced exact software/mobile OS conflicts. `Krate` selected as active name; plans/docs/CLI placeholders/WIT examples/bundle extension renamed accordingly.
- 2026-05-02: Empty Cargo workspace failed baseline commands, so Phase 0 now uses a root sentinel crate until Phase 1 runtime crates exist.
- 2026-05-02: mdBook `SUMMARY.md` structure failed local build and was fixed; docs build is now part of CI.
- 2026-05-02: Initial Krate workspace pushed to GitHub at `incyashraj/krate` with commit `fe41db4` (`chore: bring Krate foundation online`), authored and committed as `incyashraj`.
- 2026-05-02: GitHub CLI authenticated as `incyashraj`; repo homepage/topics set, labels synced, Pages verified at `https://incyashraj.github.io/krate/`, five good-first issues opened, and Phase 1 kickoff opened as issue `#6`.
- 2026-05-03: GitHub CI is green on `main` after the cargo-deny and cross-host Phase 1 fixes. The 5-day Phase 0 stability window starts from this point, subject to future pushes staying green.

---

## Closing

Phase 0 is the phase that looks like nothing happened. No code. No product. No metrics. The work is invisible and the temptation to skip it is huge.

Do not skip it. Every OSS project I've seen that lasted a decade had a Phase 0 like this one. Every project that died before year two either skipped it or hacked through it. The repo, the docs, the CI, the Discord, the first ADR — they are not preparation for the real work. They are the conditions that make the real work possible.

Take the four weeks. Write the README you would want to land on. Set up the CI you would want to inherit. Invite the community you would want to contribute to. Then, with the bones in place, begin Phase 1 — and do not look back.

— end of document —
