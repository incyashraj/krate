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

**The number: 17 of 17 widgets and 9 of 14 interfaces fully implemented on all
three systems** (the generated parity table is the authority; two more are
partial). No platform-only widgets, for the first time.

| Still hollow | What it blocks |
|---|---|
| `gfx.canvas2d` | drawing apps, charts drawn by the app itself |
| `gfx.gpu3d` | 3D anything |
| `ui.menu` | apps that expect a system menu bar (degrades to buttons) |

Everything else — windows, 17 widget kinds, images, file picker with
token-based grants, clipboard, notifications, HTTPS, SQL, KV store, secrets,
random, speech-to-text, and as of today audio playback (a real tone through
real speakers, verified by an ignored-by-default device test) — is real and
tested nightly by replaying committed
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

**The number: 5 of 8 AI-authored apps came out valid in the July experiment;
the 3 failures shared one cause (std linkage) that is now fixed SDK-side.**
Not re-measured since — that re-run is the next real datum here.

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
| Runtime | 9/14 interfaces full, 17/17 widgets | 14/14, or the hollow ones removed from the contract |
| Porting | 8 apps, 2 for non-programmers | a stranger's app ports with 0–1 repairs, unattended |
| Creating | 5/8 valid (pre-fix) | 8/8 on re-run, then novel requests |
| Trust | sandbox verified by test | an external person fails to escape it |

The honest one-sentence status: **the shape of the dream is built — picker,
pictures, sandbox, three OSes — and the work now is making the pipelines
boringly reliable instead of heroically debuggable.**
