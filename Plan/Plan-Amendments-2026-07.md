# Krate Plan Amendments — July 2026 (Change Order)

> **Status:** Approved by the founder 2026-07-02, based on a full direction review of the repo, plans, exit ledger, and CI state (review notes kept founder-private).
> **Purpose:** The single instruction set for updating every plan document before development moves forward. Apply all amendments below, in order, before starting new Phase 3 implementation work.
> **Audience:** Anyone (or any session) doing planning or development work in this repo.
> **Rule:** If any future work conflicts with this document, this document wins until it is superseded by a newer approved amendment.

---

## Table of Contents

1. [Verified current state (baseline facts)](#1-verified-current-state)
2. [Guardrails — what must NOT change](#2-guardrails)
3. [Amendment A1 — Linux widget strategy (resolves GTK4/winit conflict)](#a1)
4. [Amendment A2 — Phase 3 re-sequencing (vertical slice first)](#a2)
5. [Amendment A3 — Agent-embedding surface (new scope)](#a3)
6. [Amendment A4 — Phase 2 closeout timebox + evidence tooling freeze](#a4)
7. [Amendment A5 — GTM Stage 1 activation + narrative alignment](#a5)
8. [Amendment A6 — Co-founder/first-hire tracked lane](#a6)
9. [Amendment A7 — Operational fixes](#a7)
10. [Amendment A8 — STATUS.md resume prompt update](#a8)
11. [New task definitions (full specs)](#11-new-task-definitions)
12. [Order of application + acceptance checklist](#12-order-of-application)

---

## 1. Verified current state

Facts verified against the repo, CI, and ledger on 2026-07-02. These are the baseline for every amendment:

- HEAD is `8bc9a15` (2026-06-23). Push CI and Pages deploy are green through HEAD. STATUS.md is accurate and in sync.
- **Phase 2**: engineering ~90–92% complete. Exit ledger (`docs/book/src/phase2/exit-evidence.md`): P2E-13/14/15 Done; P2E-01 strong draft; P2E-02..11 Partial; P2E-12 (outside timed walkthrough) Pending. All remaining work is evidence/gate closure, not architecture.
- **Phase 3**: started under the documented waiver. Landed: WIT drafts (ui/gfx/audio), shared widget tree, Taffy layout crate + prepared-layout path, UCap-gated `runtime::phase3_ui` dispatcher, headless adapters ×3 OSes, opt-in AppKit native window prototype (NSWindow, delegate, draw view, event-loop step, smoke command), Winit session/callback scaffolding for Linux+Windows (no real Winit OS windows yet).
- Known recorded deficits: 10k-node **cold** layout rebuild above Phase 3 budget (prepared path under budget); Go runtime parity experimental.
- **Operational**: self-hosted runner `krate-local` offline since 2026-06-24 — nightly fuzz runs queue 24h and cancel, every night. `Krate-book.pdf` untracked in repo root since May 23.
- **ADR numbering**: highest existing ADR in `docs/adr/` is `0014-layout-engine-taffy.md`. Next free number is **0015**. (Note: Phase-3-Plan.md *text* cites different ADR numbers for some decisions than the files actually use — when applying A1, follow the repo's file numbering, and fix plan-text citations where touched.)
- **External context** (drives priority, not architecture): the project needs, in the near term, a demonstrable native-window milestone, first outside-developer validation, and a clean CI history. These needs affect sequencing only; the founder tracks them outside this repo.

## 2. Guardrails

These are working well. **Do not change them under any amendment:**

1. **Phase 2 UAPI freeze discipline** — no Phase 3 work may modify Phase 2 WIT contracts. The freeze-lock checker stays enforced.
2. **The honest ledger habit** — STATUS.md and the exit ledgers record misses and unfinished work as plainly as wins. Never soften this; it is a diligence asset.
3. **Phase discipline** (Phase-3-Plan §4.3) — no tray icons, themes, plugins, or other Phase-N temptations. A3 below is the *only* approved scope addition, and it is bounded.
4. **Answer D** (ADR-0013, native widgets + drawn fallback) — remains the architecture bet. A1 narrows *where* it applies in v0.1; it does not reverse it.
5. **CI green rule** — check CI after every push; keep GitHub Pages in sync with docs changes.
6. **Existing plan documents are amended, not rewritten.** Apply the minimal edits specified below; preserve all other content and history.

---

<a name="a1"></a>
## 3. Amendment A1 — Linux widget strategy

### The problem (reason)

Phase-3-Plan §7.2 chooses `winit` for Linux windowing. §7.3 and §15.1 choose `gtk4-rs` for native Linux widgets **in those same windows**. These are mutually incompatible: GTK4 removed foreign-window embedding — GTK4 widgets can only live inside a GTK-owned window running GTK's own event loop. A `GtkButton` cannot be hosted inside a winit-created Wayland/X11 surface. Continuing on the current path means weeks of Winit work that dead-ends when widget lowering starts.

### The decision (solution)

For Phase 3 / v0.1: **Linux keeps winit windowing, and ALL Linux widgets use the drawn fallback (vello)**. Native GTK widget lowering is removed from Phase 3 scope (revisit post-v1.0, or if demand appears, via GTK-owned windows as a separate backend). Windows keeps native lowering (Win32 common controls as child HWNDs of a winit window — supported). macOS keeps AppKit native lowering. XAML Islands on Windows is dropped from v0.1 (defer; plain Win32 controls only).

Why this resolution and not "GTK owns the windows": Linux is the host where "native look" is least defined (GNOME vs KDE vs others), so drawn widgets cost the least user-facing credibility there; keeping winit preserves one shared windowing path for Linux+Windows; and the Answer-D bet still gets proven fully on macOS and Windows.

### Changes to make

| File | Section | Change |
|---|---|---|
| `docs/adr/0015-linux-widget-strategy.md` | new file | Write ADR-0015 recording the decision above, with the GTK4 embedding rationale. Follow `docs/adr/template.md`. |
| `Plan/Phase-3-Plan.md` | §7.3 table | Linux row: change library to "`vello` drawn fallback (no native GTK lowering in v0.1 — see ADR-0015)". Windows row: note "Win32 common controls; XAML Islands deferred past v0.1". |
| `Plan/Phase-3-Plan.md` | §15.1 (Linux adapter) | "Native widgets: gtk4-rs" → "Widgets: drawn fallback via vello (ADR-0015). GTK theme-pack tokens for visual fit." Remove/annotate the GTK theme mismatch pain item accordingly. |
| `Plan/Phase-3-Plan.md` | §5.5 / §9.1 | Add one clarifying line: the "native three of five" test governs *protocol inclusion*; per-host lowering may still be drawn where the host backend doesn't support embedding (Linux v0.1). |
| `Plan/Phase-3-Plan.md` | §3 Success Criteria row 4 | Annotate: on Linux, "feels native" is measured against the drawn-widget rubric (correct scroll physics, focus, shortcuts, dark mode), not GTK widget identity. |
| `Plan/Phase-3-Plan.md` | §19 P3-UI-07 (Linux widget bridge) | Redefine task: "Linux drawn-widget backend" — implement the drawn fallback path for the v0.1 widget set inside winit windows, using per-host theme tokens. |
| `Plan/Build-Plan.md` | §5 tech stack (if GTK named) + §14.1 risks | Update Linux widget row to match; add closed-risk note: "GTK4-in-winit embedding conflict — resolved by ADR-0015 before implementation." |
| `docs/book/src/phase3/widget-protocol.md` | lowering table | Reflect Linux = drawn in v0.1. Keep Pages in sync. |

### Acceptance

ADR-0015 exists and is linked from Phase-3-Plan §7.3; no plan document any longer instructs embedding GTK4 widgets in winit windows; `check-phase3-design-docs.sh` (extend if needed) passes.

---

<a name="a2"></a>
## 4. Amendment A2 — Phase 3 re-sequencing: vertical slice first

### The problem (reason)

Phase 3 work so far is horizontal: every event route, every adapter boundary, session scaffolds across all hosts — but **zero native widgets proven end-to-end**. Phase-3-Plan §0 itself names this the riskiest phase and §5 names native lowering the core bet. The current sequence discovers a fatal flaw in that bet last instead of first. Separately, the project needs a public, demonstrable native milestone in the near term — the vertical slice doubles as that demo.

### The decision (solution)

Insert one milestone, **P3-VS-01 (vertical slice)**, as the immediate next implementation work, ahead of the previously planned "first real Linux/Windows Winit window" step. Winit broadening resumes after P3-VS-01 passes (and after A1's ADR lands, which unblocks it correctly). Full task spec in §11 below.

### Changes to make

| File | Section | Change |
|---|---|---|
| `Plan/Phase-3-Plan.md` | §18 week breakdown | Add a note at top of §18: "Amended 2026-07: P3-VS-01 (macOS vertical slice) executes before Winit window broadening — see Plan-Amendments-2026-07.md A2." |
| `Plan/Phase-3-Plan.md` | §19 | Add task P3-VS-01 (copy spec from §11 of this document). Note P3-UI-05 (macOS widget bridge) starts via P3-VS-01's two widgets. |
| `STATUS.md` | "next step" statements + resume prompt | Replace "Next Phase 3 work should create the first real Linux/Windows Winit window…" with the A8 text below. |

### Acceptance

STATUS.md and Phase-3-Plan agree the next milestone is P3-VS-01; no session resumes Winit window creation before P3-VS-01 is complete.

---

<a name="a3"></a>
## 5. Amendment A3 — Agent-embedding surface

### The problem (reason)

The company's stated wedge is "safe runtime for portable AI-generated software," but no phase before 5–6 ships anything an AI-agent builder can actually use. The buyer in the wedge story (agent-framework developers) exists **now**; reaching them requires only exposing what the runtime already does. This is also the earliest realistic source of external adoption proof, which the project needs most.

### The decision (solution)

Add three bounded tasks (specs in §11): **P3-EMB-01** stable embedding API on `crates/runtime` (execute a component under a policy object, structured results); **P3-EMB-02** `krate run --json` machine-readable output (run result, grants used, denials with capability names, exit classification); **P3-EMB-03** a thin MCP server wrapping the runtime so agent frameworks can execute components under UCap. Scheduled **after** P3-VS-01 — the slice comes first.

Scope bound (guardrail 3 applies): no agent orchestration, no model calls, no tool registry — Krate executes artifacts safely; the agent ecosystem does the rest.

### Changes to make

| File | Section | Change |
|---|---|---|
| `Plan/Phase-3-Plan.md` | §19 | Add P3-EMB-01/02/03 task entries (from §11). Mark as "parallel track, non-blocking for Phase 3 exit criteria." |
| `Plan/Build-Plan.md` | §6 roadmap + §15 GTM | Add one paragraph: agent-embedding is the Phase-3-era expression of the AI wedge; hosted/marketplace expressions remain Phase 5–6. |
| `docs/book` | new page `phase3/embedding.md` (when work starts) | Document the embedding API and `--json` contract. |

### Acceptance

Tasks exist in the plan with the stated scope bound; Phase 3 exit criteria are NOT expanded (embedding is additive, not a new gate).

---

<a name="a4"></a>
## 6. Amendment A4 — Phase 2 closeout timebox + evidence tooling freeze

### The problem (reason)

Phase 2's remaining items are entirely evidence/process (freeze review, bundles, walkthrough, stability windows). The credibility this machinery buys is already banked; each additional hour on it is taken from the vertical slice and external validation. ~40 of 57 scripts are already evidence infrastructure, and Phase 3 has begun adding its own.

### The decision (solution)

1. **Timebox**: Phase 2 closeout gets **one focused week** — run the freeze review, fill the decision packet, record the final bundle, schedule (not necessarily complete) the P2E-12 outside walkthrough. Whatever isn't closed in that week is tracked in the ledger but no longer blocks or interleaves with Phase 3 work.
2. **Freeze rule**: **no new evidence recorder/comparator/checker scripts are added until the team has a second engineer.** Phase 3 verification reuses existing harnesses (`check-phase3-uapi.sh`, `check-phase3-design-docs.sh`, `check-phase3-layout.sh`, standard tests/benches). Exception: a checker explicitly required by an existing exit gate may be modified, not multiplied.

### Changes to make

| File | Section | Change |
|---|---|---|
| `Plan/Phase-2-Plan.md` | status header | Add: "Closeout timeboxed per Plan-Amendments-2026-07.md A4; unfinished formal gates tracked in the exit ledger without blocking Phase 3." |
| `Plan/Build-Plan.md` | §21 development status | Record the evidence-tooling freeze rule. |
| `STATUS.md` | §5 (what remains for Phase 2) | Add the timebox note so future sessions don't re-expand closeout work. |

### Acceptance

A future session picking up "Phase 2 remaining work" finds the timebox rule before finding the gate list.

---

<a name="a5"></a>
## 7. Amendment A5 — GTM Stage 1 activation + narrative alignment

### The problem (reason)

Build-Plan §15.2 schedules OSS credibility (public build log, community, talks) for months 0–12. None of it has started, and Phase 0's external items are still pending — meaning the project is entering its outward-facing period with zero external signal. Separately, the repo README leads with the grand universal-platform story while the sharper public wedge is safe execution of AI-generated software; readers who evaluate the repo meet a mismatched narrative.

### The decision (solution)

1. Start the minimum GTM loop (~2–3 h/week): one public build-log post per milestone (source material = the STATUS.md updates already being written), published to the existing GitHub Pages site; a "follow the build" link in the README.
2. Align `README.md`: open with the wedge (safe portable runtime; AI-generated software as why-now), then the platform vision as the long-term arc. Same product, same phases — only the order of the story changes.
3. Defer Discord until the vertical-slice demo exists (a community needs something to watch).

### Changes to make

| File | Change |
|---|---|
| `README.md` | Rewrite the opening (first ~30 lines) per above. Keep everything factual; no capability claims beyond STATUS.md. |
| `Plan/Build-Plan.md` §15.2 | Mark Stage 1 as active; note the build-log mechanism. |
| `docs/book` | Add a build-log section/page fed per milestone. |

### Acceptance

README first screen tells the wedge story; first build-log post published with the P3-VS-01 milestone.

---

<a name="a6"></a>
## 8. Amendment A6 — Co-founder/first-hire tracked lane

### The problem (reason)

Build-Plan §14.3 marks "recruit co-founder or first key systems contributor by end of Phase 1" as High likelihood / **Critical** impact. Phase 3 has started; the milestone silently lapsed and is tracked nowhere. It is also the first question any outside evaluator asks a solo-founder project.

### The decision (solution)

Track it like an exit gate: a **Hiring** section in STATUS.md with (a) the first-hire profile — recommended: systems engineer owning the Windows + Linux adapter lanes; (b) pipeline status (people contacted / in conversation); (c) a concise outside-ready answer describing the role and search status. Update it with every STATUS.md refresh, like any other lane.

### Changes to make

| File | Change |
|---|---|
| `STATUS.md` | Add "## Hiring lane" section with the three items above. |
| `Plan/Build-Plan.md` §14.3 | Update mitigation: "tracked as a standing STATUS.md lane per Plan-Amendments-2026-07.md A6." |

### Acceptance

STATUS.md contains the lane; it is non-empty (at minimum the profile is written).

---

<a name="a7"></a>
## 9. Amendment A7 — Operational fixes

Three small items with outsized diligence/exit impact. Reasons inline:

1. **Self-hosted runner `krate-local`**: bring it back online, or disable the nightly fuzz schedule until it is. *Reason:* nine consecutive cancelled runs pollute the public Actions history, and the Phase 2 fuzz-soak gate cannot close while it's down. Verify the queued run `28562509376` resolves.
2. **`Krate-book.pdf`** (untracked in repo root since May 23): decide — move it out of the repo (recommended if it's a personal artifact), or gitignore it, or commit it deliberately. *Reason:* a stray 224 KB unexplained PDF in the root of a repo that prides itself on discipline is noise at best, a question mark in diligence at worst.
3. **Founder-private files check**: confirm the private `/Invest/` folder and any personal material remain fully gitignored. *Reason:* the repo is public; private operating files must never land in history.

### Acceptance

Actions tab shows no new cancelled nightly runs; `git status` clean at root; private folder absent from git history (`git log --all -- Invest/` empty).

---

<a name="a8"></a>
## 10. Amendment A8 — STATUS.md resume prompt update

### The problem (reason)

STATUS.md §8 contains the exact prompt used to resume development in new sessions. It currently instructs: "Next Phase 3 work should create the first real Linux/Windows Winit window…". After A1+A2, that instruction sends development in the wrong direction.

### The change

Replace the "next work" sentence in the §8 resume prompt (and the equivalent statements in §4A) with:

> "Next Phase 3 work is P3-VS-01, the macOS vertical slice: one WASM component drives a real AppKit window containing a native NSButton and NSTextField, and receives the click event back — end-to-end through UCap and the Phase 3 dispatcher. Read Plan/Plan-Amendments-2026-07.md first: Linux widget lowering is drawn-fallback-only per ADR-0015, Winit window broadening resumes only after P3-VS-01 passes, evidence tooling is frozen per A4, and agent-embedding tasks (P3-EMB-01..03) follow the slice."

### Acceptance

A fresh session following STATUS.md §8 lands on P3-VS-01, not on Winit windows.

---

## 11. New task definitions

### P3-VS-01 — macOS vertical slice (native widget end-to-end)

- **What**: A demo WASM component (new `apps/` or `examples/` entry) that: opens a window via the Phase 3 dispatcher → the AppKit prototype creates the real NSWindow → the component submits a widget tree containing one `Button` and one `TextField` → the macOS adapter lowers them to a real `NSButton`/`NSTextField` placed by the Taffy layout → a physical click on the NSButton flows back through the delegate/event bridge → dequeued by the component via the event path → component updates the label text.
- **Builds on**: `AppKitWindowSession`, delegate bridge, draw view surface, shared widget tree, `Phase3UiDispatcher` layout + event routes — all already landed.
- **Explicitly out of scope**: reconciler diffing beyond naive re-submit; drawn fallback rendering; any Linux/Windows work; styling beyond defaults.
- **Done when**: a recorded local run (extend `smoke-phase3-appkit-runtime.sh` pattern) shows the click round-trip; STATUS.md updated; this recorded run is the centerpiece of the public demo.
- **Failure escalation**: if native lowering hits a structural wall (event routing, layout-to-AppKit coordinate mismatch, ownership), STOP and write the findings into an ADR draft before writing workaround code — this is the bet-validation milestone; discovering a flaw is a success condition of the task.

### P3-EMB-01 — Runtime embedding API

- **What**: a documented public API on `crates/runtime`: load component + manifest, supply grants programmatically (no interactive prompt), run, receive a structured result (exit class, stdout/stderr handles, per-capability grant/deny log).
- **Done when**: an external Rust program (doc-tested example) embeds Krate in <30 lines; no interactive TTY required.

### P3-EMB-02 — `krate run --json`

- **What**: machine-readable run output on stdout/stderr-safe channel: run status, exit code + classification, capabilities requested/granted/denied (names + boundaries), timing. Schema documented in the book.
- **Done when**: `krate run --json` output parses with a documented schema for success, permission-denied, and invalid-input paths of the three sample apps.

### P3-EMB-03 — MCP server wrapper

- **What**: minimal MCP server exposing `run_component` (artifact path/bytes, manifest, grants) backed by P3-EMB-01, returning P3-EMB-02-shaped results. Ships as a `tools/` binary.
- **Done when**: an MCP-capable agent client can execute `krate-cat` with and without grants and observe the deny/allow difference.

---

<a name="a9"></a>
## Amendment A9 — Rename: Krate becomes Krate (added 2026-07-04)

**Decision**: the product is **Krate**; the company is **Krate Labs**. The
`.ai` naming neighborhood is crowded (krater.ai, krateo.ai, krates.ai), the
product is a runtime rather than a model, and "Krate" carries the shipping-
crate and Rust-crate meanings natively.

**Phase A (done with this amendment)**: outward surfaces — README, docs
title, introduction naming note, build log, STATUS, Build Plan name field,
the product book, and founder-private materials — carry the new name with a
"formerly Krate" transition note. Code, commands, crate names, and WIT
namespaces intentionally keep `krate`.

**Phase B (scheduled slice — land BEFORE the UAPI v0.1 freeze decision)**:
1. WIT namespaces `krate:*` to `krate:*` across phase1/phase2/phase3
   packages, with freeze-lock regeneration, generated-binding regeneration,
   import-purity checker updates, fixture rebuilds, and manifest world
   strings.
2. Crate renames (`krate-*` to `krate-*`), CLI binary `krate` to
   `krate`, script names, env var prefixes (`KRATE_` to `KRATE_`), JSON
   schema id (`krate.run.v1` to `krate.run.v1` with the old id accepted
   during transition), MCP server name.
3. Repository rename and docs URL migration; GitHub redirects cover old
   links.
Rationale for the pre-freeze deadline: renaming a frozen namespace later
would be a breaking version bump; renaming a draft namespace now is cheap.

**Inventory (measured 2026-07-04)**: ~4,900 occurrences in ~300 files —
wit/ 47 (the decision core), crates/ 1,285, apps/ 715 (mostly regenerable
bindings), scripts/ 393, docs 846, Plan 1,286 (prose), .github 151,
Cargo.lock 31 (regenerates). Execution order for the slice: (1) rename WIT
packages and worlds; (2) rename crates and the CLI binary; (3) regenerate
bindings, lockfiles, and fixtures; (4) sweep scripts/workflows/env vars;
(5) let clippy + fast CI + the full matrix flush every missed reference;
(6) prose sweep across Plan/docs/book last, once commands are real;
(7) repo rename + Pages URL last of all (GitHub redirects cover old
links). Keep `krate.run.v1` accepted as a schema alias during
transition. One focused session with the CI loop as the verifier.

## 12. Order of application

Apply in this order (dependencies flow downward):

1. **A7** — runner + repo hygiene (independent; unblocks fuzz gate; cleans diligence surface).
2. **A4** — Phase 2 timebox week (closes the paperwork era; edits Phase-2-Plan, Build-Plan, STATUS.md).
3. **A1** — ADR-0015 + all Linux-widget plan edits (a writing task; unblocks correct Winit/widget work).
4. **A2 + A8** — re-sequencing edits + resume-prompt update (Phase-3-Plan §18/§19, STATUS.md).
5. **A6** — hiring lane in STATUS.md (one edit, do with #4's STATUS.md touch).
6. **A3** — embedding tasks added to plans (paper only; implementation after P3-VS-01).
7. **A5** — README + Build-Plan GTM edits; first build-log post ships with the P3-VS-01 milestone.
8. Begin **P3-VS-01** implementation.

### Final acceptance checklist (all boxes before new implementation work)

- [ ] `docs/adr/0015-linux-widget-strategy.md` merged; Phase-3-Plan §7.3/§15.1/§19 and widget-protocol docs page updated; Pages in sync
- [ ] Phase-3-Plan §18/§19 carry the amendment note and P3-VS-01 / P3-EMB-01..03 task specs
- [ ] STATUS.md: next-work statements + §8 resume prompt per A8; Hiring lane present; Phase 2 timebox note present
- [ ] Phase-2-Plan header carries the timebox note; Build-Plan carries the tooling-freeze rule, GTM Stage 1 activation, updated §14 rows
- [ ] README opens with the wedge narrative
- [ ] Runner online or nightly disabled; root `git status` clean; tracker verified out of git
- [ ] CI green on the docs/plan amendment push

### What this change order does NOT do

It does not alter Phase 3's destination (GUI on three OSes, `krate-notes`, the §3 exit criteria except the Linux annotation), does not touch Phase 2's UAPI, does not add platform scope, and does not change the architecture bet. It re-orders work so the riskiest assumption is tested first, removes one impossible integration, bounds process overhead, and makes the project's stated wedge buildable — so that development, documentation, and the outward story all point in the same direction.
