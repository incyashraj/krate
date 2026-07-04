# Phase 0 Status

This page tracks only the important Phase 0 exit work. The detailed checklist
lives in `Plan/Phase-0-Plan.md`.

## Done

| Area | Status |
|------|--------|
| Licenses | MIT and Apache-2.0 files are present. |
| Rust workspace | Cargo build and test pass. |
| Toolchain | Rust `1.91.1` is pinned. |
| Dependency policy | `cargo-deny` passes. |
| CI | Format, lint, tests, docs, and dependency audit run on GitHub Actions. |
| Docs site | mdBook is live at `https://incyashraj.github.io/layer6x6/`. |
| Project docs | README, CONTRIBUTING, SECURITY, code of conduct, ADR template, and first-PR guide exist. |
| Issues and labels | First five good-first issues were created and labeled. |
| Phase 1 kickoff | GitHub issue `#6` exists. |

## Still Important

| Area | Next action |
|------|-------------|
| Public repo settings | Confirm public visibility, social preview, description, homepage, and topics. |
| Branch rules | Keep `main` protected by green CI and pull request review. Owner bypass should stay temporary. |
| CI history | Keep `main` green for five consecutive days. First green runs started on 2026-05-03. |
| README review | Ask one outside reader to follow the README and mark what confused them. |
| Discord or community home | Create channels, rules, and an invite link before public launch. |
| Announcement | Publish only after docs, repo settings, and community home are ready. |
| Domain | Secure `krate.dev` or a chosen equivalent. |
| Naming | Finish official trademark and registry checks. |
| External PR | Get one outside contributor PR opened and merged. |
| Retrospective | Finish the Phase 0 retro after the external work is done. |

## Bottom Line

Phase 0 is ready enough for engineering work to continue. It is not 100% closed
as a public launch phase because the outside-world checks are still open.
