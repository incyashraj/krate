# The compatibility programme: can Krate build the software people use?

Written 2026-08-12. **Every capability claim here was checked against the WIT
and the runtime source today**, not remembered. Where a limit is stated, the
check that found it is beside it.

The goal, in one sentence: **find out what Krate can and cannot deliver by
rebuilding the software people actually use, as complete working apps, from
the position of an outside developer** -- and then fix what stops us.

---

## Why the obvious version of this plan fails, and what to do instead

The tempting plan is: list the top apps since 2010, document each one, rebuild
each one. That plan fails, and it is worth being precise about why, because
the failure mode is expensive.

**Rebuilding Dropbox teaches you almost nothing about Krate.** Ninety percent
of Dropbox is a server, an account system, a sync protocol, and a business.
The part that touches Krate -- watch a folder, diff it, upload changes, show
progress -- is maybe five percent of it, and it is the *only* five percent
that tells us anything. Spend three weeks on the other ninety-five and you
learn nothing you could not have learned in an afternoon.

Worse: the answer to "can Krate do Dropbox?" is **no, and not for an
interesting reason**. It is no because an app exports exactly one function,
`run()`, so it cannot sync while its window is closed. That is one sentence
from `wit/krate/phase3/world.wit`, and it did not need a rebuild to find.

So the programme is restructured around the thing that actually generalises:

> **An app is a bundle of capabilities. Rebuild the capability, not the brand.**

Slack, Discord, WhatsApp and Teams are one question: *can a Krate app hold a
live connection and receive a message it did not ask for?* Answer that once
and you have answered it for every chat app that will ever be requested.

The app list still matters -- it is how we find the capabilities, and how we
stay honest about what people really use rather than what is convenient to
build. It is the input, not the output.

---

## What Krate can do today: the honest ledger

Checked 2026-08-12 against `wit/krate/phase3/`, `crates/runtime/`, and
`crates/bundle/`.

### The world an app lives in

```
world gui {
  import ... 12 interfaces, 125 functions ...
  export run: func() -> s32;
}
```

**One exported function.** An app starts, runs, returns. This single line
decides more about what is buildable than everything else combined.

### Solid

| Capability | Evidence | What it unlocks |
|---|---|---|
| Real SQL, parameterised, transactions | `store/sql`: `query`, `execute`, `transaction` | Anything data-shaped: notes, tasks, finance, catalogues |
| Key-value + encrypted secrets | `store/kv`, `store/secret` | Sessions, tokens, preferences |
| HTTP GET/POST with headers + body | `net/http-client`: `get`, `fetch` | Any REST API |
| Full 2D drawing + text shaping | `gfx/canvas2d`, 35 funcs | Any UI you can draw |
| Files, sandboxed; picker for the rest | `fs/files` + `ui/dialog:open-folder` | Local documents, imports, exports |
| Audio in and out | `audio` 11 funcs | Players, recorders |
| Clipboard, notifications, menus | `ui/*` | Desktop-native feel |

### The four walls

These are the limits that decide the whole programme. Each is a fact, not an
impression.

**Wall 1 -- No background execution.**
`export run: func() -> s32` is the entire lifecycle. No wake-on-event, no
launch-at-login, no scheduled task. *An app cannot do anything while it is
closed.*
Kills: file-sync daemons, backup tools, "new mail" notifications, alarms.

**Wall 2 -- No streaming or server-push.**
```
$ grep -rowE "websocket|server-sent-events|event-stream|tcp-socket|udp" wit/
(no matches)
```
Only `get` and `fetch`, both with **buffered** `list<u8>` bodies -- no chunked
transfer in either direction.
Kills: live chat, collaborative editing, video/audio streaming, live feeds.
Forces: polling, which is a real but degraded answer.

**Wall 3 -- Single-threaded, blocking I/O.**
No thread or concurrency primitive exists in the WIT, and the HTTP client is
`ureq` (synchronous, `Cargo.toml:86`). **Every network call blocks the event
loop**, so a large download freezes the window.

Measured, not inferred. A local server that stalls 3 seconds before
responding, fetched by `apps/krate-fetch`:

```
$ krate run krate_fetch.wasm --auto-grant --headless -- http://127.0.0.1:8799/
fetch:ok:356
ELAPSED: 5.37s   (server stalls 3s)
```

The guest sat blocked for the whole stall. It could not draw, animate, or
answer a click. **A progress spinner during a download is currently
impossible**, and so is a cancel button.

Kills: responsive apps that fetch anything substantial.
This is the most fixable of the four and probably the highest value.

**Wall 4 -- No embedded browser, no video codec.**
Kills: anything whose product is rendering the web, and all video playback.

### The size ceiling (not a wall, but worth knowing)

`MAX_BUNDLE_BYTES` 256 MB, `MAX_ASSET_BYTES` 96 MB per asset. Generous. Not
a constraint for anything in this programme.

---

## The app list, and what each one actually asks of Krate

Chosen for *capability coverage*, not popularity. The question beside each is
the one that generalises.

### Tier A -- buildable today, complete and honest

These need nothing new. If we cannot build these, the problem is us.

| App | The generalising question | Stands in for |
|---|---|---|
| **Notion / Bear** | Rich text editing + local DB | every note, wiki, doc tool |
| **Things / Todoist** | Recurring rules, dates, projects | every task manager |
| **Mint / YNAB** | Import CSV, categorise, chart | every finance tool |
| **Anki** | Spaced repetition, media, scheduling | every learning tool |
| **Pocket / Instapaper** | Fetch a URL, extract text, store offline | every read-later tool |
| **Sublime / VS Code (editor core)** | Large text buffer, find/replace, syntax | every editor |
| **Photos (library)** | Thumbnail grid, EXIF, albums, edits | every media manager |
| **Spotify (local player)** | Audio playback, queue, library, scrubbing | every player |

### Tier B -- buildable only in a degraded form; the degradation is the finding

| App | What breaks | The honest Krate version |
|---|---|---|
| **Slack / Discord** | Wall 2: no live socket | Polls every N seconds. Works. Feels wrong. |
| **Gmail** | Wall 1 + 2: no push, no background | Fetches on open. No new-mail notification. |
| **Dropbox** | Wall 1: cannot sync while closed | Manual "sync now" while open |
| **Figma** | Wall 2: no multiplayer | Single-player vector editor -- still substantial |
| **Trello** | Wall 2 for realtime, else fine | Fine solo, stale with a team |

### Tier C -- not buildable, and we should say so plainly

| App | Why | Would need |
|---|---|---|
| **Chrome** | No embedded engine | To be a browser |
| **YouTube / Netflix** | No video decode | Video pipeline + DRM |
| **Zoom** | No realtime media | WebRTC |
| **Photoshop / Blender** | Scale, GPU compute | Years |

---

## What gets built, in what order

Not "one app per week for a year". Each phase answers a question and produces
a decision.

### Phase 1 -- Prove the ceiling (2 weeks)

Build **three complete Tier A apps**, as an outside developer would: only the
public install, only krate.tech, no repo source. Pick the three that stress
different subsystems:

1. **A note-taking app** -- text editing, SQL, search. The most-requested
   shape in existence.
2. **A finance tracker** -- file import, parsing, charts, dates.
3. **A media library** -- images, thumbnails, metadata, a grid that scrolls.

"Complete" means: it persists, it survives a resize, it handles an empty
state, it handles a thousand rows, it can be closed and reopened without loss,
and someone else can open the `.krate` and use it without instructions.

**The deliverable is not the apps. It is the defect list.** Every place the
authoring loop, the runtime, or the pack failed an outside developer, filed
with evidence.

### Phase 2 -- Break the walls, in value order (4-6 weeks)

Fix what Phase 1 proves is worth fixing. My prediction, to be tested rather
than assumed:

1. **Non-blocking fetch** (Wall 3). Highest value, smallest change. Every
   networked app is currently either unresponsive or trivial. Likely shape: a
   request handle the app polls, so the event loop keeps turning.
2. **A frame/timer event** so apps stop guessing at time with `wait(16)`.
   Already identified as a gap; every animation needs it.
3. **Streaming bodies** (part of Wall 2). Unlocks large downloads and
   progress bars without holding a file in memory.
4. **A live connection primitive** (the rest of Wall 2). The big one. Decides
   whether chat, collaboration and live data are ever possible.

Each lands with: the runtime change, a `--shoot` pixel test, a line in the
authoring pack, and **an example app that uses it** -- the K-098 rule, or the
capability does not reach anyone.

### Phase 3 -- Prove the loop, not the apps (2 weeks)

The real target you named: *a person at a big company builds something, sends
a link, and the other person just opens it.*

That is not an app problem. It is four measurements:

| Question | Measure | Today |
|---|---|---|
| How long from idea to file? | minutes | 5-12, one retry sometimes |
| How often does it work first time? | % | **unmeasured since 2026-08-05 (0/5)** |
| How long from link to running? | seconds | unmeasured |
| How often does opening fail? | % | 9.2%, cause now instrumented (K-100) |

**Two of those four are unmeasured and one is a bad number we have not
re-run.** Fixing that is worth more than another ten apps, and it is the
honest prerequisite to telling anyone this works.

---

## How the details get gathered (the part you asked for)

For each app on the list, one page, and only these fields -- because
everything else is decoration:

1. **What it is for**, in one sentence a stranger would recognise.
2. **The three things people actually do in it.** Not the feature list. A
   note app is: capture fast, find later, never lose anything.
3. **The data it holds**, as a schema. This is the part that ports directly.
4. **What it does when it is not being used.** *This is the Wall 1 test, and
   it decides tier membership more than anything else.*
5. **What it needs from the network, and when.** Poll or push? Big or small?
   *The Wall 2 and 3 test.*
6. **The one interaction that would be embarrassing to get wrong.** Dragging
   a card in Trello. Scrubbing in Spotify. Typing in a note.
7. **Krate verdict:** Tier A / B / C, with the specific interface that
   decides it.

Field 4 alone would have sorted the entire list before any code was written.

---

## What "efficient for users" actually means

You asked how someone at a big company gets from idea to a link someone else
just opens. The path exists; these are its measured frictions:

| Friction | Today | What would fix it |
|---|---|---|
| Build takes 5-12 min | Rust compile dominates | Warm toolchain; prebuilt deps |
| Sometimes needs a retry | check-app loops | Better first-shot rate; measure first |
| Windows shows SmartScreen | No OV/EV cert | A purchase, not a fix |
| 1 in 11 opens fails | Cause now recorded | Read the data in a week |
| Publishing needs the CLI | No GUI path | `krate connect` covers the AI path |

The two that matter most are the two we cannot currently quote: the first-shot
success rate, and the time from link to running app.

---

## What I recommend

Do Phase 1 -- but **start with one app, not three**, and only after the
current release settles. Pick the note-taking app: it is the most requested
shape, it needs no new capability, and it will expose the authoring loop's
real quality in a way our 39 examples cannot, because none of them were built
by a stranger's rules.

If that one app comes out complete and usable, the programme is worth
running. If it does not, the defect list it produces is worth more than the
next nine apps would have been.

**What I would not do:** document eighty apps before building one. The
document would be stale before it was finished, and the four walls above --
found in an hour of reading the WIT -- already sort most of the list.
