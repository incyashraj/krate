# Layer36 Status

Last updated: 2026-05-06
Repo: `incyashraj/layer6x6`
Branch: `main`
Latest checked completed push before this slice: `73dddb9`
Working tree at this status update: sample-evidence comparator host-label and same-commit hardening validated locally

## 1) Project size today

- Commits after this slice lands: 247
- Tracked files after this slice lands: about 246
- Total tracked lines after this slice lands: about 72,492
- Rust lines (`.rs`) after this slice lands: about 36,445
- Docs lines (`.md`) after this slice lands: about 26,514

## 2) Latest CI and Pages state

Latest completed push (`fdda72e`) checks:

- CI: success (run `25436832524`)
- Deploy docs to GitHub Pages: success from latest docs push (run `25436647881`)

Recent pushes before that are also green.

## 3) What this version can do now

Layer36 already runs real Phase 2 CLI components through the runtime:

- `layer36-clock`
- `layer36-cat`
- `layer36-curl`

Current capability set includes:

- Manifest parsing and capability checks
- Launch grant flow (`--grant`, `--auto-grant`, prompt flow)
- Runtime UAPI policy checks before host calls
- Cross language fixture and parity coverage for Rust, TypeScript, and available Go paths
- Published docs on GitHub Pages with Phase tracking

## 4) Progress read for Phase 2

Practical engineering progress is strong and close to completion for the first useful slice.

- Core engineering slice: around 89% to 91%
- Formal Phase 2 exit gates: around 74% to 79%

Main reason formal completion is lower than engineering completion:
the remaining work is mostly evidence and gate closure, not missing base architecture.

## 5) What remains to close Phase 2 fully

Top pending items:

1. Final UAPI v0.1 freeze review for WIT contracts
2. Stronger Go runtime proof path with import pure fixture promotion across runners
3. Broader cross host evidence for language variant behavior
4. Formal gate evidence:
   - multi day CI stability window
   - long fuzz soak pass
   - benchmark and dependency audit sign off
5. Phase 2 retrospective and Phase 3 kickoff issue

## 6) Recent completed development highlights

- TypeScript curl error behavior aligned with Rust:
  - permission denied returns exit code `5`
  - invalid URL returns exit code `20`
- Added parity tests for Rust vs TypeScript on curl denial and invalid URL paths
- Go curl classifier hardened with stable mapping and unit tests
- Hosted workflows moved to Node 24 ready action versions
- WIT contract comments added across Phase 2 UAPI and enforced by `check-uapi`
- Generated UAPI reference now includes those WIT contract comments
- Rust SDK package smoke now verifies packaged README, SDK root, and generated bindings files
- Go TinyGo smoke artifacts build locally, but promotion correctly blocks them because they still import WASI host APIs
- Component import checker now reports all failing artifacts in one run, which improves Go runtime-proof triage
- Added a Phase 2 UAPI freeze-review page with checklist and commands
- Added a repeatable UAPI freeze-evidence snapshot page generated from `check-uapi`
- Wired hosted and self-hosted CI to fail when that freeze-evidence page is stale
- Added a runtime adapter-boundary guard for 34 host wrappers across Linux, macOS, and Windows adapter crates
- Added a Phase 2 exit-evidence ledger that tracks all 15 exit gates with status, proof source, and next step
- Added a Phase 2 Rust sample evidence recorder for clock, cat, and curl stdout/hash proof across hosts
- Added a Phase 2 cross-host sample evidence comparator for Linux/macOS/Windows report parity checks
- Hardened sample-evidence comparison with host-label validation so `--linux`, `--macos`, and `--windows` cannot silently point to the wrong host report
- Hardened sample-evidence comparison with same-commit validation so cross-host reports must come from one code revision

## 7) Source of truth files

- Plan: `Plan/Phase-2-Plan.md`
- Phase docs page: `docs/book/src/phases/phase-2.md`
- Progress page for non technical readers: `docs/book/src/progress-for-everyone.md`

## 8) Resume prompt for a new GPT session

Use this exact prompt in a new session:

`Continue Layer36 on main. Start with STATUS.md and Plan/Phase-2-Plan.md. Keep pushing Phase 2 closure, update plan/docs after each chunk, keep GitHub Pages in sync, and check CI after every push.`
