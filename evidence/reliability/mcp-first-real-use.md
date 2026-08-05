# The first real MCP session, and the five things it broke

A person connected Krate to Claude Desktop with `krate connect`, restarted, and
asked for a habit tracker. They got a file. The app looked reasonable and was
functionally close to useless: nothing could be clicked, the list did not
scroll, resizing broke it, and it closed itself after a few seconds.

Everything below is verified against the code or by running the artifacts from
that session, not inferred from the transcript.

---

## 1. `krate_start_build` reported success for an app nobody asked for

**The worst one, and it is ours.**

`krate_start_build` with no `agent` argument runs the built-in template
generator, which knows three shapes (checklist, word-count, voice-prompter). It
cannot write an arbitrary app. It named the result `habit-tracker` and produced
a checklist.

The tool did warn. It then built the wrong app anyway and reported:

    "status": "succeeded"
    "verdict": "authored a working, permission-gated .krate"

Run that artifact and the window title is literally `Checklist`:

    $ krate run --grant store.kv ~/.krate/mcp/builds/build-1/habit-tracker.krate -- quick
    krate: opened window "Checklist"
    items:5

Every check-app stage passed, because every stage is mechanical. None of them
asks whether the app is what was requested. This is the same class of failure
the refusal path was built to stop, arriving through a different door: an app
that builds, runs, paints a frame, and is not the thing.

The model noticed, abandoned the job, and hand-wrote the app instead -- which
is the only reason the session produced anything at all. A less careful model
would have handed over the checklist.

**Fix:** when no agent is given and the request does not match a template,
`krate_start_build` must refuse rather than warn-and-proceed. `AppKind::
infer_matched` already returns `None` for an unrecognised request; the MCP path
ignores it. A warning the caller can ignore is not a guard.

## 2. Apps close themselves after ten seconds

`MAX_IDLE_ROUNDS = 300` at `WAIT_ROUND_MILLIS = 33` is **9.9 seconds** of no
input, after which the event loop breaks and the window closes.

This is in `apps/krate-checklist`, the app the context pack recommends as "the
simplest GUI+store shape", so the model copied it faithfully. **Seven shipped
apps have it**: checklist, notes, focus, pulse, timer, savings, journal.

The idle timeout exists so a headless verification run cannot hang forever.
That is a real need, and it is only a need in the `quick` path. In an
interactive run it is a bug that makes every app feel broken.

**Fix:** apply the idle timeout only when `quick` is set. An interactive window
closes when the person closes it.

## 3. Nothing scrolls

`krate-checklist` holds `MAX_ITEMS = 32` and draws `VISIBLE_ROWS = 6`. Items 7
to 32 render as a `+ N more` label and cannot be reached. There is no wheel
handling, no scroll offset, no `Scroll` widget -- zero matches for
`scroll|wheel` in the file.

So a person adds a seventh habit, it saves correctly, and disappears.

**Fix:** the canvas examples need a scroll offset driven by wheel events. Until
they have one, every canvas app built from them has an invisible ceiling.

## 4. Resizing the window breaks every click

Hit-testing is written against compile-time constants:

    fn hit_row(list: &Checklist, x: f32, y: f32) -> Option<usize> {
        if x < MARGIN || x > WIDTH - MARGIN {   // WIDTH is a const
            return None;
        }

The app never calls `canvas2d::canvas_size` and never handles a resize event.
Resize the window and the canvas stretches while the hit-boxes stay at their
original coordinates, so clicks land in the wrong row or miss entirely.

This is the single best explanation for "I cannot click on anything" and
"graphics seem broken" arriving together in the same report.

`canvas_size` exists in the host (`phase3_gui_host.rs`). **Only 3 of 34 shipped
apps call it.**

**Fix:** the reference apps must lay out from `canvas_size`, not from
constants, and re-lay out on resize. This is the change that turns these from
fixed-size pictures into real windows.

## 5. `krate pack` rejects the manifest our own docs tell you to write

The context pack and Krate Mode both teach:

    entry = "target/wasm32-wasip1/release/<name>.wasm"

`krate pack` refuses it:

    bundle manifest declares entry `target/wasm32-wasip1/release/krate_habits.wasm`,
    but a bundle always runs `code.wasm`

Both forms are correct in their own place -- the dev manifest points at the
build output, the packed manifest points at the name inside the bundle -- and
we document only the first. Neither `authoring_context.rs` nor `krate-mode.md`
mentions `code.wasm` at all.

The model worked around it with an unexplained `sed`. Anyone else stops here.

**Fix:** `pack` should rewrite the entry itself, since it knows the answer.
Failing that, document the two forms.

---

## What this says about the loop

`check-app` passed everything. The build succeeded. The permission wall held.
Every gate we built worked exactly as specified, and the person still got an
app they could not use.

The gates test whether an app is *valid*. Nothing tests whether it is *usable*
-- whether a click lands, whether a list scrolls, whether the window stays
open. Those are the only properties the person actually experiences.

Ranked by what to fix first:

1. **#1**, because it silently ships the wrong app and undoes the refusal work.
2. **#2 and #4**, because they make every canvas app feel broken, and they are
   in the examples every generated app is copied from.
3. **#5**, a one-line fix that currently stops anyone who follows our docs.
4. **#3**, the largest piece of work and the least immediately visible.

Note that #2, #3, and #4 are all bugs in *our* reference apps. The AI copied
them correctly. Fixing the examples fixes every app generated from them, which
is the highest-leverage repair available.
