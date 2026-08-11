# The plan of work — written 2026-08-12

Every capability claim below was checked against the source today. Where a
cost is estimated, the thing that makes it expensive is named, so the estimate
can be argued with rather than believed.

---

## What this plan is answering

Not "what should we build next". The question a tester actually asked:

> "This is not that amazing considering what the world has seen in the AI era.
> People are used to better, nicer apps with faster response and modern,
> classy, elite UI. So why Krate?"

That decomposes into three separate problems, and conflating them is why the
answer has been unsatisfying:

| What they said | What it really is | Fixable? |
|---|---|---|
| "not amazing" | **Positioning.** We demo app types that already exist, inviting a comparison we lose by definition | Yes — change the demo, not the product |
| "modern, classy UI" | **Real capability gaps**, now enumerated below | Yes — 3 of 5 are cheap |
| "faster response" | **K-101**, fixed 2026-08-12 | Done |

**The positioning problem is the big one and it is free to fix.** A better
note app cannot answer "why Krate?", because a better note app already exists
and syncs to their phone. Krate's answer is the app that *cannot exist any
other way*: the one whose market is one person.

---

## Part 1 — The visual gap, measured

The complaint was vague; the gap is not. Checked against
`wit/krate/phase3/deps/gfx/gfx.wit` today.

### What we have (34 canvas2d functions)

Rounded rects with per-corner radii, drop shadows with real blur, linear and
radial gradients, gradients with angle + stops, text with weight/italic/letter
spacing, text measurement, circles, arcs, clipping, sprites with rotation,
raw pixels, images with rounded corners, and a full 3D scene path.

That is more than enough for a good-looking app. **The showcase apps prove it**
— `krate-pulse` and `krate-savings` look current.

### What is missing, in the order it hurts

| Gap | Why it shows | Cost | Note |
|---|---|---|---|
| **Emoji** | An app with no emoji in 2026 reads as *broken*, not minimal | Medium | Needs a colour-glyph path (COLR/CBDT) in the font stack |
| **Arbitrary paths** | No chart line, no custom icon, no logo, no non-rectangular anything | **Large** | See the architecture note below |
| **Transform stack** | No rotate/scale of a group; every animation is position-only | Medium | |
| **Opacity layer** | Cannot fade a whole panel in or out — the commonest transition there is | Small | |
| **Backdrop blur** | The "frosted glass" look every modern OS uses for sheets and modals | Medium | Shadow blur already exists to borrow from |

### The architecture note that decides the cost

`crates/runtime/src/canvas_raster.rs` rasterises **each shape with its own
hand-written signed-distance function** — `round_rect_sdf` and friends, one
per shape, blending coverage per pixel. It is not a general path renderer.

Desktop draws through this. Only iOS goes through vello, which *is* a path
renderer.

So:

- **Opacity layers and transforms are cheap** — they compose over the existing
  SDF calls without changing how shapes are filled. Checked rather than
  assumed: every shape funnels through one `blend_coverage`, which already
  multiplies the colour's alpha by coverage, so a group opacity is one more
  multiply in one function. Transforms have the same shape — `map_point` and
  `map_len` already exist for the design-size transform and would take a
  matrix instead of a scale.
- **Arbitrary paths are expensive** — they need either a scanline rasteriser
  written from scratch, or moving desktop onto vello (which iOS already
  proves works, and would bring paths, transforms and opacity in one go).

**That second option is the real decision in this plan**, and it should be
made deliberately rather than drifted into. It is weeks, not days, and it
touches the most-used code path in the product.

---

## Part 2 — The order of work

### Now — the demo that answers "why Krate?" (days)

Build **one app that cannot exist any other way**, in front of someone.

Not a category clone. A tool for one real person's real workflow — the kind
nobody will ever ship because the market is one. The pitch stops being *"look
at this app"* (answerable with "I already use something better") and becomes
*"tell me a tool you wish existed"*, which has no answer but yes.

**This needs one input we do not have: a real person and their real
workflow.** Any friend, any job. Without it we are guessing again.

Success is not "it looks nice". It is that person saying *"can I keep this?"*

### Next — the three cheap visual wins (about a week)

In this order, because it is the order they are noticed:

1. **Emoji.** Highest ratio of "looks broken" to effort.
2. **Opacity layers.** Unlocks every fade; smallest change on the list.
3. **Transform stack.** Unlocks rotate/scale animation.

Each lands the K-098 way or it reaches nobody: painter implementation,
`--shoot` pixel test, **a line in the authoring pack**, and **an example app
that uses it**.

### Then — the decision about paths

Two roads, and I would not pick one without deciding what Krate is for:

| | Write an SDF/scanline path renderer | Move desktop to vello |
|---|---|---|
| Cost | Large | Larger |
| Gets us | Paths only | Paths, transforms, opacity, blur — all at once |
| Risk | New rasteriser, new bugs, all self-inflicted | Regressions in the most-used path; iOS already de-risks it |
| Verdict | Cheaper now, dead end later | **The one I would choose**, but not this month |

### Deferred, deliberately

- **K-099** (nothing measures wasted space in a generated app). Needs a region
  measure; my edge-band attempt was thrown away because it scored every real
  app 3–6% including the obviously bad one.
- **The compatibility programme** (`Plan/krate-compatibility-2026-08.md`) —
  as *research on paper*, not as clones. Extracting each app's capability
  requirements takes days; building seventeen of them teaches nothing the
  extraction does not.
- **Streaming bodies.** The natural follow-on to K-101, and it lifts the
  in-memory ceiling on large downloads. Not urgent until an app needs it.

---

## Part 3 — The measurements we still cannot quote

This is the uncomfortable section and it should stay in the plan until it is
empty. Four numbers decide whether "a person makes an app and sends a link"
actually works. **Two are unmeasured and one is bad and stale.**

| Question | Today |
|---|---|
| How long from idea to file? | 5–12 min, sometimes a retry |
| How often does authoring work first time? | **0/5 on 2026-08-05. Never re-run.** |
| How long from link to running app? | **Never measured** |
| How often does opening fail? | 9.2% — now instrumented (K-100), needs a week of data |

Re-running the authoring benchmark is worth more than any single feature on
this plan, because every other claim depends on it and we currently cannot
answer the first question anyone technical will ask.

---

## What I would do in the next two weeks

1. **The impossible-app demo** — blocked on one real person's workflow.
2. **Emoji, opacity, transforms** — a week, lands the K-098 way.
3. **Re-run the authoring benchmark** and publish whatever it says.
4. **Read the K-100 reason codes** once v0.1.12 has been out a week, and fix
   whatever dominates once `refused` is excluded.

Not on this list, on purpose: building Notion, Slack, or Dropbox clones. The
first two are answerable by capability extraction on paper, and the third is
answered by one line of `world.wit` — an app exports only `run()`, so it
cannot sync while closed.
