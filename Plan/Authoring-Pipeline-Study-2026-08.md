# The Authoring Pipeline: a first-principles study

A structured investigation of "AI makes a Krate app," run across multiple AIs,
to decide what in the process is essential, what is ceremony, and what we would
build differently if we started today with no legacy.

**Owner of the builds:** Yashraj (runs each test, captures the transcript).
**Owner of the analysis:** Claude (fills in Part 4 from the captured data).

---

## Part 0 — The pipeline as it actually is (measured, not assumed)

Every number below was read out of the repo on 2026-08-19, not estimated.

### What every AI reads before writing one line

`KRATE_AUTHORING.md`, the pack, generated fresh into each app dir:

- **1,087 lines / ~11,765 words / ~78 KB.**
- 6 top-level sections, ~40 subsections. Section map: the full SDK surface,
  the capability catalog, "two patterns that decide how good the app feels,"
  no_std/panic discipline, the GUI world, a complete worked example (~200 lines
  of `src/lib.rs` inline), and an index of the shipped examples.

Then the prompt tells it to **read "the closest example" in full**. Those range
from 84 lines (krate-cat) to 933 (krate-notes); 41 example apps exist.

So a typical cold build has the AI ingest **~78 KB of pack + one 300–900 line
example** before it writes anything. That is the single biggest fixed cost of
every build, and the first thing to interrogate from first principles.

### The phases of a build

1. `plan` — one JSON round: ask ≤3 questions, or state a plan. Target < 30s.
2. `authoring` — the agent loops: read pack → find example → write `src/lib.rs`
   + `manifest.toml` → run `krate check-app` → fix → repeat until `OK`.
   Budget: `AGENT_AUTHOR_TIMEOUT_SECS = 2400` (40 min), `STALL_SECS` heartbeat.
3. `check-app` (the agent runs it, and Krate runs it again at the end) —
   build → import-check → run headless → usability drive (15s stay-open watch).
4. `auto_repair` — if the final check fails, Krate re-invokes the agent with the
   verdict, up to 2 rounds. (New in v0.1.50.)
5. `pack` + `verify the permission wall`.

### The known limits (what a user can ask for that we cannot yet do)

Capabilities that EXIST: `fs` (read/write/list/mkdir), `io`, `net:connect`,
`store` (kv/sql/secret), `time`, `random`, `locale`, `motion`, `ui`
(window/dialog/menu/notify/clipboard), `gfx` (gpu/canvas2d/scene3d),
`audio` (playback + capture/mic), `speech` (transcription).

Capabilities that DO NOT exist — the walls:
- ~~**Camera / webcam video**~~ — **CORRECTED 2026-08-27. This is no longer a
  wall and the line below it was wrong when written.** `camera.capture` is a
  real declared capability with a backend on all three desktop systems:
  AVFoundation on macOS (K-119), and nokhwa over Media Foundation (Windows)
  and V4L2 (Linux), shipped in 021f19e01 (K-148). It is NOT the 3D scene
  camera. What is still true: no run against a physical webcam on Windows or
  Linux has been recorded, so "implemented" and "works on your machine" remain
  separate claims. B1 below is therefore a real build test, not a wall test.
- **Screen capture / recording.**
- **Bluetooth / USB / serial / MIDI hardware.**
- **Background / persistent processes** (an app is one window, one run).
- **Multi-window** (one window per app today).
- **GPU compute** (`gfx.gpu:compute` declared, not implemented).
- **Sending keystrokes/input to OTHER apps** (by design — refused).

An honest test suite has to hit these on purpose (Tier B below), because the
most damaging user experience is asking for something we cannot do and getting
a confusing failure instead of an honest "Krate cannot do X yet."

---

## Part 1 — The test matrix (two tiers)

Run each with **grok** and **codex** at minimum; **claude** where available.
Same request text verbatim across AIs, so differences are the AI, not the ask.

### Tier A — the common path (everyday friction)

The apps a real user actually types. Measures the cost of the normal flow.

| # | Request (type verbatim) | What it exercises |
|---|---|---|
| A1 | a tip calculator with a bill field and buttons for 15 and 20 percent | simplest GUI, no_std, one screen |
| A2 | a to-do list I can check off, that remembers my items | `store.kv` persistence |
| A3 | a snake game I play with arrow keys | held input, game loop, canvas |
| A4 | a music player with a few demo tracks and play/pause | audio synthesis + playback |
| A5 | a markdown note editor with live preview | text input, layout, scroll |
| A6 | a weather dashboard that fetches the current weather for a city | `net:connect` (live HTTP) |

### Tier B — the limits (deliberate walls)

Chosen to hit what we cannot do. The *right* outcome for most of these is a
fast, honest refusal — "Krate cannot do X yet" — NOT a 40-minute failure or a
fake app that pretends. How each AI handles the wall is the finding.

| # | Request (type verbatim) | Wall it hits |
|---|---|---|
| B1 | an app that shows my webcam feed with a photo button | NOT a wall -- camera ships on all three (K-119, K-148). Run it as the first real-hardware test: does a physical webcam actually deliver frames on Windows and Linux? |
| B2 | a screen recorder that saves an mp4 | screen capture |
| B3 | a voice memo app that records me and plays it back | mic capture (exists!) — does the AI find it? |
| B4 | a two-window app: controls in one, a big preview in the other | multi-window |
| B5 | a background timer that keeps running after I close the window | persistent process |
| B6 | an app that types my signature into whatever field is focused | input to other apps (refused by design) |
| B7 | a MIDI keyboard that plays my connected piano | hardware device |

---

## Part 2 — What to capture per build (the review sheet)

For **each** (request × AI), capture into a row. This is the raw data of the
study. The instrumentation that makes most of this free is in Part 3.

1. **Request + AI + Krate version.**
2. **Plan phase:** did it ask questions or state a plan? How many? Were the
   questions useful or ceremony? Wall-time.
3. **What it read:** every file the agent opened, in order, from the transcript
   (the pack, which example(s), re-reads). Did it read the pack front-to-back
   or jump? Did it read examples it did not need?
4. **Where it thought longest:** the gaps between tool calls — where did it
   pause > 20s? (Model latency vs genuine reasoning.)
5. **First code at:** wall-time from start to the first `write src/lib.rs`.
6. **check-app loop:** how many times did the agent run check-app? What failed
   each round (build? imports/wasi-leak? usability? logic)? How did it recover?
7. **Stalls / dead air:** any period > 60s with no output, and what it was
   doing (network wait vs stuck).
8. **auto_repair:** did Krate's post-agent repair fire? Did it succeed?
9. **Total wall-time** and **outcome:** OK / failed / refused / wrong-app.
10. **The app itself:** does it work? Does it match the request? Screenshot.
11. **For Tier B:** did it refuse honestly and fast, waste time, or fake it?

---

## Part 3 — Instrumentation to make the data free (Claude builds, opt-in)

Rather than have Yashraj hand-transcribe, add a single env-gated trace so a
build writes its own review sheet. Proposed (not yet built — confirm before I
add it):

- `KRATE_TRACE=<path>`: the create pipeline appends a JSONL line per event —
  phase start/end with timestamps, each provider tool_call (already parsed for
  progress), each check-app run + its verdict, each repair round. The agent's
  own transcript already exists per session; this adds the *timing spine* and
  the *check-app outcomes* the transcript lacks.
- A tiny `krate study-report <trace.jsonl>` that prints the Part-2 row.

This turns every future build into a data point automatically, and is reusable
long after this study. **This is the one piece of code the study needs; the
rest is running builds and reading transcripts.**

---

## Part 4 — First-principles analysis (Claude fills from the data)

The questions the study exists to answer. I will answer each from the captured
rows, not from opinion.

### 4a. What is essential vs ceremony in the current flow?
- Is the 78 KB pack the right size, or is the AI ignoring 80% of it? (Measure:
  what fraction of pack sections does a build's reads/actions actually touch?)
- Is reading a full 300–900 line example necessary, or would a 30-line minimal
  scaffold + targeted snippets be faster with equal success?
- Is the plan/question round earning its keep, or is it latency the user hates?
  (Codex asks questions for *everything* — measured. Is that value or noise?)

### 4b. Where does the time actually go?
- Split total wall-time into: pack/example reading, thinking (model latency),
  writing, check-app compile, usability watch, repair. Which dominates? Which
  is in our control vs the model's?

### 4c. The from-scratch question.
> "If we had zero knowledge of how this is done today, would we build the flow
> like this? If we had all modern research and resources, where is this
> traditional and where could it be modern/efficient?"

Candidate rethinks to evaluate against the data, not assert:
- **Retrieval over dump.** Instead of handing the AI 78 KB up front, give it a
  tool to *query* the pack ("how do I play a sound?") and fetch only what it
  needs. Modern agent practice; would cut the fixed reading cost.
- **A compile-in-the-loop that is instant.** check-app compile is the slow gate.
  Could a cached/warm build or a type-check-only fast path cut the iteration
  time 5–10x?
- **Refuse before building.** For Tier B walls, the *plan* step should say
  "Krate cannot do X" in 10 seconds instead of the agent discovering it 30
  minutes in. A capability-aware plan gate is a first-principles win.
- **Templates as starting geometry, not prose.** Instead of "read this example
  and adapt," start the AI *from* the closest working app's code as the scaffold
  and let it diff. (Revise already works this way; create does not.)
- **Skip the question round when the request is unambiguous.** Measure how often
  the questions changed the outcome; cut them where they did not.

### 4d. The capability roadmap the tests expose.
Rank the Tier B walls by how often a real user would hit them, and what each
would take to close (camera device wiring, multi-window, etc.). Camera first if
the data says users ask for it most.

---

## How to run it (Yashraj)

1. Cut/confirm the current release is installed.
2. For each row in Tier A then Tier B, in the Studio (or `krate create` CLI):
   type the request, pick the AI, let it run to completion or failure.
3. Keep the session — the transcript is at
   `~/.krate/studio/sessions/<id>.json` (and the agent transcript in the build
   workspace). Note the wall-time and outcome.
4. Hand Claude the session ids (or the transcripts); Claude fills Parts 2 and 4.

If the instrumentation in Part 3 is built first, steps 3–4 become automatic.
