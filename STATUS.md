# Layer36 Status

Last updated: 2026-05-06
Repo: `incyashraj/layer6x6`
Branch: `main`
Head commit at handoff start: `3dde154`
Working tree at handoff start: clean

## 1) Project size today

- Commits: 237
- Tracked files: 231
- Total tracked lines: 69,492
- Rust lines (`.rs`): 34,507
- Docs lines (`.md`): 25,673

## 2) Latest CI and Pages state

Latest push (`3dde154`) checks:

- CI: success (run `25432973813`)
- Deploy docs to GitHub Pages: success (run `25432973788`)

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

## 7) Source of truth files

- Plan: `Plan/Phase-2-Plan.md`
- Phase docs page: `docs/book/src/phases/phase-2.md`
- Progress page for non technical readers: `docs/book/src/progress-for-everyone.md`

## 8) Resume prompt for a new GPT session

Use this exact prompt in a new session:

`Continue Layer36 from commit 3dde154 on main. Use Plan/Phase-2-Plan.md (Completed Tasks and In Progress) as source of truth. Keep pushing Phase 2 closure, update plan/docs after each chunk, and keep GitHub Pages docs in sync.`
