# Roadmap: any small/medium app, ported or generated, on all three systems

The dream, stated as something checkable: **a person names a small or medium
app — existing or imagined — and gets one .krate file that runs identically on
macOS, Windows, and Linux.**

That is two pipelines sharing one runtime:

```
  PORT      existing source ──▶ analyzer ──▶ agent ──▶ .krate
  CREATE    plain words ──────▶ starter ───▶ agent ──▶ .krate
                                                          │
                              one runtime, three OSes ◀───┘
```

The detailed gates live in `Portable-Definition-Of-Done-2026-08-01.md`. This
page is the scoreboard: what each track can do today, what blocks it, and the
one number per track that says whether we moved.

## Track 1 — The runtime (both pipelines land here)

**The number: 17 of 17 widgets and 10 of 14 interfaces fully implemented on
all three systems** (the generated parity table is the authority; two more are
partial). No platform-only widgets.

| Still hollow | What it blocks |
|---|---|
| `gfx.gpu3d` | 3D anything |
| `ui.menu` | apps that expect a system menu bar (degrades to buttons) |

**Animation works.** `krate-bounce` runs a real game loop — measure elapsed
time, advance physics, draw, request the next frame — and the quick run
measured over twelve thousand frames a second. Whatever eventually limits
games here, it is not the draw path.

Everything else — windows, 17 widget kinds, images, file picker with
token-based grants, clipboard, notifications, HTTPS, SQL, KV store, secrets,
random, speech-to-text, audio playback (a real tone through real speakers),
and 2D drawing (a bar chart drawn by a guest through the WIT boundary) — is
real and tested nightly by replaying committed
bundles on all three OSes.

## Track 2 — Porting existing apps

**The number: 8 proven ports, all replaying green nightly.** hexyl, savings,
ddh, rssfwd, envelope, grex, eo2, mdview — and the last two (an image viewer
and a markdown viewer) are apps whose audience is not programmers.

Repair attempts on the same app, as causes were found and fixed: **5 → 3 → in
progress**. Every cause so far was wrong guidance handed to the agent, not a
runtime limit. Each one now has a test:

- recommended a decoder that cannot build for the target → CI builds every
  recommended crate and inspects its imports
- false rule that `format!`/`Vec` leak wasi → verified both directions, rule
  rewritten
- documented an API path without its module → test walks every documented path
  against the WIT
- accepted `~/Pictures` grants that can never match → refused at pack time
- analyzer read generated packaging files as app source (twice) → excluded,
  tested

**Generalization datum:** md-viewer (4,863 lines, an app none of the fixes
were shaped around) ported with **zero repair attempts** under the honest
pipeline — after its first "zero-repair success" was exposed as the untouched
scaffold. The trend on the app that drove the fixes: 5 → 3 → 1.

## Track 3 — Generating new apps

**The number: 6 of 8, re-measured 2026-08-02** (`Create-Batch-2026-08-02.md`).
Both failures were Krate's own checks refusing to ship something inconsistent,
not the AI writing bad code, and both causes are fixed. Every pass was run and
its output checked by hand.

The finding that matters is not the score: a password keeper came out working
and storing passwords in ordinary app data rather than the OS keychain. The
sandbox guarantees an app cannot exceed its permissions; it cannot yet
guarantee good judgment inside them. The contract now spells out which store
means what.

`krate create` works two ways: built-in templates (checklist, voice prompter)
with no AI needed, and `--agent claude` for arbitrary requests. The GUI
scaffold now carries `std_feature = true`, so generated windowed apps can use
real dependencies without tripping the import wall.

## Track 4 — Trust (what makes the claim safe to say out loud)

- Capability sandbox: an app sees only what it is granted; verified by
  withholding grants at pack time and by direct escape testing (`/etc/passwd`
  reads the sandbox copy — bytes checked)
- A malformed image cannot exploit the host: decoding happens inside the
  sandbox, pixels cross the boundary
- Shipped bundles keep working: the replay caught the one change that would
  have broken every GUI app, the day it was made

## How to read progress (the short version)

| Track | Today | "Done" looks like |
|---|---|---|
| Runtime | 10/14 interfaces full, 17/17 widgets | 14/14, or the hollow ones removed from the contract |
| Porting | 8 apps, 2 for non-programmers | a stranger's app ports with 0–1 repairs, unattended |
| Creating | 6/8, both failures now fixed | 8/8 on re-run, and sound judgment inside the sandbox |
| Trust | sandbox verified by test | an external person fails to escape it |

## The longer horizon, with honest distances

What "everything you can think of" breaks into, nearest first:

| Ambition | Distance | What is actually missing |
|---|---|---|
| 2D games | **close** | Canvas and a frame loop both work at speed. Needs sprites (images into a canvas), and key-repeat/held-key state for smooth control. |
| Modern UI feel | **close** | Layout, widgets, and per-frame redraw exist. Needs an animation curve helper and rounded/translucent fills — the canvas has square edges and no gradients today. |
| Sound in games | **done for playback** | Tones and streams work. Mixing several sounds at once is app-side today; a mixer could move host-side later. |
| Web APIs / online apps | **partly there** | HTTPS with per-host scoping works and is proven by a ported RSS forwarder and a generated quote fetcher. Missing: streaming responses and WebSockets, so live feeds and multiplayer are out. |
| 3D | **far** | `gfx.gpu3d` is declared and hollow. This is a real GPU abstraction across three systems — weeks, and the one place the "weeks not days" warning is honest. |
| Video | **far** | No decode interface. Same shape as images (decode in the sandbox, pixels to the host) but needs a frame clock and audio sync. |

The nearest three share a property worth noticing: none needs a new
capability or a new host adapter. Sprites, rounded fills, and key-state all
extend surfaces that already reach all three systems.

The honest one-sentence status: **the shape of the dream is built — picker,
pictures, sound, drawing, animation, sandbox, three OSes — and the work now is
making the pipelines boringly reliable instead of heroically debuggable.**
