# BUGS.md — the one bug board

**This is the only bug file in this repository.** Do not create another. Do not
open a second list in a plan doc, an evidence file, or a comment. If it is a
defect, it goes here.

Anyone working on Krate — the lead session or any agent — reads this file before
starting work and updates it the moment they find or fix something.

---

## How to use it

### If you find a bug

1. **Search this file first.** If it is already listed, do not add it again. Add
   evidence to the existing entry instead.
2. **Add an entry** at the top of Open, using the format below. Give it the next
   free `K-` number.
3. **Decide honestly whether you are fixing it now.**
   - Fixing it as part of your current task: set `Owner:` to your workstation
     and `Status: claimed`.
   - Not fixing it: leave `Owner: unclaimed`. Say so in your report so it can be
     assigned. **Do not detour from your assigned task to fix an unrelated bug**
     — file it and move on. That is the whole point of this board.

### Before you start fixing anything

**Check `Owner:` first.** If it names a workstation that is not you, do not
touch it. Two agents fixing one bug in two worktrees produces a merge conflict
and two half-solutions. If you think the owner is stuck or gone, say so in your
report — do not take it over silently.

### When you fix it

Move the entry to Fixed, set `Status: fixed`, and record the commit. Keep it in
the file. **A fixed bug with its evidence is how the next person knows not to
reintroduce it**, and several bugs here came back precisely because nobody wrote
them down the first time.

---

## Entry format

```
### K-007 — one line, what breaks, in plain words
Status:   open | claimed | fixed
Owner:    unclaimed | W13 | lead
Severity: blocker | serious | annoyance
Class:    runtime-hole | teaching-hole | example-bug | our-code | environment
Found:    2026-08-05, by whom, how
Evidence: the command and the output, or file:line. Not a description -- proof.
Fix:      what needs to happen, or the commit that did it.
```

**Severity**
- `blocker` — a class of app cannot be built at all
- `serious` — apps build but are wrong or unusable in a person's hands
- `annoyance` — real, survivable

**Class** — this is the useful part, because it says who can fix it:
- `runtime-hole` — the runtime cannot do it. Only we can fix it. No prompt helps.
- `teaching-hole` — the runtime can do it and we never told the AI. Fix the pack.
- `example-bug` — our reference apps teach the bug, so every generated app
  inherits it. Highest leverage per line changed.
- `our-code` — an ordinary defect in Krate itself.
- `environment` — this machine, not the product. Record it so it is not
  rediscovered, but do not let it distort a measurement.

---

## Open

### K-001 — Canvas apps cannot scroll: there is no scroll event
Status:   claimed
Owner:    W12
Severity: blocker
Class:    runtime-hole
Found:    2026-08-05, lead, auditing the WIT against a user report
Evidence: `wit/krate/phase3/deps/ui/ui.wit` defines ten event variants
          (close-requested, resized, redraw-requested, pointer, key, text-input,
          text-changed, action, focus-changed, theme-changed). No wheel, no
          scroll delta. `scroll` appears once as a widget kind, unrelated.
          A user's habit tracker held 32 items, showed 6, and the rest were
          permanently unreachable behind a "+ N more" label.
Fix:      Add a wheel event to the WIT, plumb through host and all three
          adapters, use it in krate-checklist.

### K-002 — No text measurement, so every app guesses text width
Status:   claimed
Owner:    W15
Severity: serious
Class:    runtime-hole
Found:    2026-08-05, lead, capability audit
Evidence: `gfx.wit` has draw-text and zero matches for
          `measure|text-width|text-extent`. Seven shipped apps carry
          `(s.chars().count() as f32) * size * 0.52` -- an invented constant on
          a proportional font where `i` and `W` differ ~4x. The runtime computes
          the true width through parley (`adapter-common/src/vector_text.rs`)
          and does not expose it.
Fix:      canvas2d::measure-text through the same parley path draw-text uses.

### K-014 — Running an app from its source dir writes its data into the repo
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-05, W13, running krate-notes headless while fixing K-003
Evidence: `krate run apps/krate-notes/target/.../krate_notes.wasm --manifest
          manifest.toml --auto-grant --headless` created
          `apps/krate-notes/notes/{first,second,third}.txt` as untracked files
          in the repo:
            $ git status --short
            ?? apps/krate-notes/notes/
          The app's fs paths are sandbox-relative and the sandbox root is the
          cwd, so running from the app's own directory drops its save files
          into the source tree. `.gitignore` has `/notes/` -- anchored at the
          repo root, so it does not match this path. Harmless but it means
          anyone verifying a GUI app dirties their working tree and may commit
          an app's test data by accident.
Fix:      Either put a per-app sandbox data dir outside the source tree, or
          add the app-relative pattern to .gitignore. Not urgent.

### K-004 — No clipping, so a scrolling list would draw over its own header
Status:   open
Owner:    unclaimed
Severity: serious
Class:    runtime-hole
Found:    2026-08-05, lead, capability audit
Evidence: one mention of "clip" in the whole of `gfx.wit`, not a clip rect.
Fix:      A clip rectangle on canvas2d. Travels with K-001; scrolling is not
          usable without it.

### K-005 — No frame timing, so animation polls a timeout and hopes
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    runtime-hole
Found:    2026-08-05, lead, capability audit
Evidence: `redraw-requested` and `request-redraw` exist; zero matches for
          `vsync|frame-time|animation`. Games poll `events::wait(Some(16))`.
Fix:      A frame/tick event carrying elapsed time, so animation is time-based.

### K-006 — Nothing checks whether an app is usable, only whether it is valid
Status:   claimed
Owner:    W14
Severity: serious
Class:    our-code
Found:    2026-08-05, lead, after a user got a green-checked unusable app
Evidence: check-app has six stages -- layout, manifest, build, imports, run,
          shoot. Every one asks whether the app compiles and paints. None asks
          whether a click lands, whether it survives a resize, or whether it
          stays open. An app with all three defects passed every stage.
Fix:      A usability stage. Must not produce false failures -- a flaky gate
          gets skipped and then protects nothing.

### K-007 — This machine's AI accounts are unusable
Status:   open
Owner:    unclaimed
Severity: blocker
Class:    environment
Found:    2026-08-05, lead, trying to verify Krate Mode end to end
Evidence: `claude -p` returns "OAuth session expired and could not be
          refreshed". `codex exec` returns "The 'gpt-5.6-sol' model requires a
          newer version of Codex".
Fix:      Not ours. Yashraj re-authenticates Claude and updates Codex. Recorded
          because it blocks every end-to-end authoring measurement and must not
          be mistaken for a product failure -- it already turned a 14/14 pass
          rate into a reported 23% once.

---

## Fixed

### K-003 — Canvas apps break when the window is resized
Status:   fixed
Owner:    W13
Severity: serious
Class:    example-bug
Found:    2026-08-05, user report, first real MCP session
Evidence: `hit_row` in apps/krate-checklist compared against `const WIDTH`. No
          app read canvas_size on resize; only 3 of 34 called it at all. Resize
          and the canvas stretches while hit-boxes stay put, so clicks land in
          the wrong row. Reported as "cannot click on anything" and "graphics
          seem broken" -- one bug, two symptoms.

          Turned out to be two bugs stacked, and fixing only the apps would
          have fixed nothing. See K-013 for the host half.

          Proof, from the app driving its own window through three shapes and
          hit-testing the row it just drew:
            $ krate run apps/krate-checklist/...wasm --manifest manifest.toml \
                --auto-grant --headless -- resize-check
            size:440x620 hit:ok
            size:900x500 hit:ok
            size:320x760 hit:ok
            resize:ok
          The same component against the pre-fix binary reports `440x620`
          three times -- the canvas never learned it had been resized.
Fix:      Apps lay out from `canvas_size` into one `Layout` struct that both
          drawing and hit-testing read, and handle Event::Resized. Converted:
          checklist, notes, pulse, focus, journal, savings, clocks.

          Note for whoever edits a reference app next: `krate-checklist` and
          `krate-clock` are `include_str!`'d into the Krate Mode prompt
          (crates/cli/src/krate_mode.rs:35-38), so changing either makes
          `krate_mode::tests::the_published_prompt_matches_the_generator` fail
          until `krate krate-mode > docs/krate-mode.md` is re-run. That is the
          anti-drift test doing its job -- it is how the fixed example reaches
          every model that pastes the prompt.

### K-013 — A bound canvas never learned its window had been resized
Status:   fixed
Owner:    W13
Severity: serious
Class:    our-code
Found:    2026-08-05, W13, while fixing K-003 -- the app fix did not take
Evidence: `phase3_gui_host.rs` built a `CanvasSurface` once in `bind` from the
          widget rect and never rebuilt it. `canvas_size` returned
          `surface.dimensions()`, so it reported the bind-time size forever.
          An app doing everything right -- reading canvas_size every frame,
          handling Resized -- still laid out to the original size, because the
          host kept telling it nothing had changed.
            $ krate run ...krate_checklist.wasm ... -- resize-check
            size:440x620 hit:ok     <- window was set to 900x500
            size:440x620 hit:ok     <- window was set to 320x760
          This is why only 3 of 34 apps called canvas_size and none looked
          wrong for it: the call could not report anything useful.
Fix:      `CanvasSurface::resize` plus `Phase3GuiHost::refit_canvas`, called
          from `canvas_size` so asking is what re-fits the surface to the
          widget's current rect. Regression test
          `resize_refits_the_buffer_and_reports_the_new_size`.

### K-008 — MCP reported success for an app nobody asked for
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-05, user report, first real MCP session
Evidence: `krate_start_build` without an `agent` ran the built-in template
          generator, produced a checklist named "habit-tracker", passed every
          check-app stage and reported "succeeded". Running that artifact opens
          a window titled "Checklist".
Fix:      e9de97e. Refuses without an agent rather than warning. Unconditional,
          because the matcher is broad enough that "habit tracker" hits the
          checklist rule on the word "track". `allow_builtin: true` is the
          deliberate opt-in.

### K-009 — Apps closed themselves after ten seconds
Status:   fixed
Owner:    lead
Severity: serious
Class:    example-bug
Found:    2026-08-05, user report
Evidence: MAX_IDLE_ROUNDS = 300 at 33ms = 9.9s of no input, then the loop broke
          and the window closed. In six shipped apps, so the AI copied it.
Fix:      e9de97e. Gated on `quick`, so the timeout protects headless
          verification and never a real session.

### K-010 — The pack never taught canvas_size, resize, the event loop, or code.wasm
Status:   fixed
Owner:    lead
Severity: serious
Class:    teaching-hole
Found:    2026-08-05, lead, auditing what the AI is given
Evidence: The runtime implements canvas_size and emits `resized`; the authoring
          pack mentioned each zero times. Five sections and none showed an event
          loop. `krate pack` requires `entry = "code.wasm"` and nothing
          documented it -- a model hit the error and worked around it with an
          unexplained `sed`.
Fix:      c5e9f00. New pack section on making a window that actually works, plus
          a pack error that says what to do rather than only stating the rule.

### K-011 — krate create could not build any app it made
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-04, lead, running the plain create path from a clean directory
Evidence: The GUI template wrote `extern crate krate` into src/lib.rs and never
          the matching dependency, so every app died with three errors that all
          pointed away from the cause. Five requests, five identical failures.
Fix:      9763f5d, plus a regression test that fails when the line is removed.

### K-012 — reports page said 11 apps while 25 shipped
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-05, lead, updating the site
Evidence: The generator globbed only evidence/ported and never evidence/store.
Fix:      481e2a5. Reads both shelves, de-duplicated by name.
