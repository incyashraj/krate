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
  stated pass bar, an honest score including failures. *In progress — W16.*
- **G2. Nothing on the board is a blocker.** Today: K-001 (no scroll), K-007
  (environment). *In progress — W12.*
- **G3. Usability is enforced, not hoped for.** check-app fails an app that
  cannot be clicked, resized, or stays closed. *In progress — W14.*
- **G4. The outsider path works cold.** Someone with only the public install and
  the website gets a working app. *Harness built; needs a real run.*
- **G5. Real users have done it.** Ten people outside this machine have made an
  app and sent it to someone.

G5 is the only one that cannot be faked, and it is what the raise needs.

---

## Short term: the live workstations

Each is one agent, one worktree, one deliverable. `BUGS.md` says who owns which
defect.

| WS | Owns | Serves | Status |
|---|---|---|---|
| **W12** | Wheel/scroll event: WIT, host, three adapters, scrolling checklist | G2, K-001 | running |
| **W13** | Canvas apps lay out from canvas_size and handle resize | G2, K-003 | running |
| **W14** | Usability stage in check-app | G3, K-006 | running |
| **W15** | Text measurement, delete the guess from seven apps | G2, K-002 | running |
| **W16** | The benchmark: corpus, harness, honest score | G1 | running |
| **W17** | Outsider testing with Grok, from a clean machine's point of view | G4 | starting |

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

## Progress log

Newest first. One line per landing.

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
