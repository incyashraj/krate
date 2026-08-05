# GOALS.md — where we are going, broken down until each piece fits one agent

**This file is the plan of record.** It goes long-term to short-term, so no
piece of work exists without a line back to why. Updated whenever a workstation
lands or a goal changes. `BUGS.md` holds defects; this holds direction and
progress.

Last updated: 2026-08-05.

---

## Long term: a person can make any app, and it works everywhere

The one sentence: **anyone describes an app in their own words, gets a real
working app, runs it on macOS, Windows and Linux, and publishes it to Krate
Cloud — and the runtime underneath is boring and reliable.**

Not templates. Not a demo. Any reasonable app, asked for in any words.

Held to four properties:

| | What it means | Where we actually are |
|---|---|---|
| **Any app** | An arbitrary request produces a working app, not the nearest template | Unknown, being measured (W16) |
| **Works properly** | Clicks land, lists scroll, resizing works, the window stays open | Failing today — K-001, K-003 |
| **Everywhere** | The same file runs on three OSes | Working, 6 targets, rc20 |
| **Shareable** | Publish to Krate Cloud, someone else opens it | Publish works; Cloud is a static shelf |
| **Reliable runtime** | No leaks, no crashes, no surprises | Good — 90 GB leak fixed, 956 tests |

We are strong on the last three and unproven on the first two. That is the
honest position, and it is the right thing to fix first.

---

## Mid term: version 1 — a stable release for real users

**What "done" means:** someone who has never seen Krate installs it, connects
their AI, describes a mid-complexity app, gets something that works, and sends
it to a friend who opens it.

Mid-complexity means: a list that scrolls, a form that validates, a chart from
real data, a small game, a text tool. Not a spreadsheet, not a browser.

The gates before we say version 1:

- **G1. The benchmark exists and the number is public.** A fixed corpus, a
  stated pass bar, an honest score including failures. *Corpus and harness done
  (W16). The first run scored 0 of 5 authored apps, caused by K-015, now fixed.
  Needs a re-run before any number is published.*
- **G2. Nothing on the board is a blocker.** Today: K-001 (no scroll), K-007
  (environment). *In progress — W12.*
- **G3. Usability is enforced, not hoped for.** check-app fails an app that
  cannot be clicked, resized, or stays closed. *In progress — W14.*
- **G4. The outsider path works cold.** Someone with only the public install and
  the website gets a working app. *Run once (W17): 8 built, 0 usable. The
  headline cause is fixed; needs a re-run to confirm.*
- **G5. Real users have done it.** Ten people outside this machine have made an
  app and sent it to someone.

G5 is the only one that cannot be faked, and it is what the raise needs.

---

## Short term: the live workstations

Each is one agent, one worktree, one deliverable. `BUGS.md` says who owns which
defect.

| WS | Owns | Serves | Status |
|---|---|---|---|
| **W12** | Wheel/scroll event: WIT, host, three adapters, scrolling checklist | G2, K-001 | **done, parked on its branch, not merged** |
| **W13** | Canvas apps lay out from canvas_size and handle resize | G2, K-003 | **landed and merged** |
| **W14** | Usability stage in check-app | G3, K-006 | **done, parked on its branch, not merged** |
| **W15** | Text measurement, delete the guess from seven apps | G2, K-002 | **landed** -- 11 apps converted, measurement matches drawn pixels |
| **W16** | The benchmark: corpus, harness, honest score | G1 | **landed** -- 42 requests, harness self-checks against 17 bundles, first score 0/5 authored |
| **W17** | Outsider testing with Grok, from a clean machine's point of view | G4 | **landed** -- 8 built, 0 usable; five defects filed |

### What each is actually trying to prove

- **W12** — that a list longer than the window is reachable. Today it is not, at
  all, for any app.
- **W13** — that a click lands where the pixel is after the window is resized.
- **W14** — that we find the next unusable app before a person does.
- **W15** — that text stops colliding, because the app can ask how wide it is
  instead of guessing `chars * size * 0.52`.
- **W16** — the number. Everything else is opinion until this exists.
- **W17** — that it works for someone who is not us, with only what we published.

---

## How this is run

**Top down.** Long-term goal → mid-term gate → one workstation → one agent. No
task exists without a line back up to a goal in this file.

**Fail fast, on real requests.** Templates prove nothing: a user asks for
something random, so the tests ask for something random. W17 exists to break
things the way a stranger would, from outside our context.

**One board.** Anything found goes to `BUGS.md`, claimed before it is fixed, so
two agents never fix one bug twice.

**Recorded as it happens.** This file and the board are updated when a
workstation lands, so a session that dies mid-flight loses nothing.

---

## Parked work — not merged, do not lose

Two workstations finished but are NOT merged. Their branches hold real,
building work. Merge these before starting anything new.

| Branch | What it holds | Why parked |
|---|---|---|
| `worktree-agent-a568654ac09818ebe` | **W12: the wheel/scroll event.** Wheel event in the WIT, host, adapters, and a scrolling checklist. Builds clean. Commit `fd3b5e2`. | Never committed itself; I committed it as WIP. Fixes K-001, the last blocker. |
| `worktree-agent-a078a3c9a5a425de9` | **W14: the usability stage.** Seventh check-app stage, exit code 16. Proven to catch K-009 and K-003 by reverting each fix. Commit `e140712`. | Ready to merge. Full-workspace clippy not run against it. |

Expect a BUGS.md K-number collision on both -- they branched before K-013
through K-023 existed. Renumber theirs into free slots, as done for W13 and
W16.

## Progress log

Newest first. One line per landing.

- **2026-08-05** — W13 landed and merged. Found the runtime half of the resize
  bug: a bound canvas never learned its window had been resized, so canvas_size
  reported the opening size forever -- which is likely why only 3 of 34 apps
  ever called it. Seven apps now lay out from the real size.
- **2026-08-05** — W12 and W14 finished but stalled without committing. Their
  work is preserved on their branches and listed above. **Merging W12 clears
  K-001, the last blocker.**
- **2026-08-05** — W17 (outsider, Grok, public install only) delivered the
  number that matters: **8 of 8 apps built, 0 of 8 usable.** Every one bounded
  its interactive loop and quit itself mid-use -- four at exactly 40 seconds,
  timed three times. All eight made the same mistake, which makes it a teaching
  hole, now fixed. Also: buttons that work only sometimes (K-017), layout
  collapsing past four controls (K-018), `krate ai` calling broken providers
  ready (K-019), double-click opening a file picker (K-020).
- **2026-08-05** — W15 landed text measurement. `iiii` and `WWWW` now measure
  4.25x apart where the old constant returned the same number for both. Found
  the bug hiding under five different names across **11** apps, not the 7 we
  knew about, with several comments claiming the host font is monospace.
- **2026-08-05** — W16 landed the benchmark and the number is bad in the useful
  way: **0 of 5 authored apps passed**, while the old measure scores those same
  five 5/5. Every one builds, imports only krate:*, runs, and paints a frame --
  and none can prove it did what was asked, because the authoring pack's entire
  spec for the verification run was "print something". One paragraph in the pack
  (K-015), now fixed. Do not publish a score until it is re-run.
- **2026-08-05** — W17 (outsider) stopped mid-run rather than report eight
  working apps on a path it suspected was serving templates. Two of its three
  findings hold and are filed (K-013, K-014); the central one -- that `--agent
  grok` does not really call Grok -- is wrong, disproved by timing a chess board
  at 237s and 584 lines. Grok authoring works end to end, which also downgrades
  K-007 from blocker to annoyance.
- **2026-08-05** — Workstations W12-W16 launched against the capability audit.
  BUGS.md and the claim protocol established.
- **2026-08-05** — rc20 shipped: MCP server with seven authoring tools, five AI
  providers, `krate connect`, the refusal path.
- **2026-08-05** — First real MCP session by a user found five failures. Two
  fixed (silent wrong app, apps closing after 10s), three filed.
- **2026-08-05** — Capability audit: no text measurement, no scroll event, no
  clipping, no frame timing. All four are the runtime knowing something and not
  exposing it.
- **2026-08-04** — `krate create` could not build any app it made. Fixed.
- **2026-08-04** — Reliability run: 14 of 14 requests that reached the AI
  produced a working app. 47 more were quota rejections, not failures.
