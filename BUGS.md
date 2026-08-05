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

### K-014 — This machine is out of disk, and cargo cannot finish a test run
Status:   open
Owner:    unclaimed
Severity: serious
Class:    environment
Found:    2026-08-05, W12, running cargo test at the end of the K-001 work
Evidence: `df -h /` reports 159Mi available of 460Gi (99% full). Tool calls
          start failing with "ENOSPC: no space left on device". The bulk is
          build output: `/Users/yashrajpardeshi/Projects/layer6x6/target` is
          87G, and each agent worktree adds its own (mine is 7.8G).
Fix:      Not a product defect. `cargo clean` the shared checkout and the
          finished worktrees. Recorded so a later ENOSPC failure is not
          mistaken for a Krate bug, and because several agents building in
          parallel worktrees is what fills the disk -- the cost is structural,
          not a one-off.

### K-013 — apps/krate-bigscroll has no manifest, so it is not a runnable app
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-05, W12, running check-app across apps while verifying K-001
Evidence: `krate check-app apps/krate-bigscroll` prints
          "FAILED at layout / apps/krate-bigscroll is not an app directory:
          manifest.toml is missing". The directory holds Cargo.toml, Cargo.lock
          and src/ and has never had a manifest (`git log -- apps/krate-bigscroll`
          shows only 1d799c1, a workspace-root change). Pre-existing, unrelated
          to the scroll work.
Fix:      Add a manifest.toml so it is a real app, or delete the directory. As
          it stands it is a half-app that fails any sweep over `apps/`, and its
          name makes it look like the scrolling reference when it is not --
          `apps/krate-checklist` is.

### K-002 — No text measurement, so every app guesses text width
Status:   fixed-pending-merge
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

### K-003 — Canvas apps break when the window is resized
Status:   fixed-pending-merge
Owner:    W13
Severity: serious
Class:    example-bug
Found:    2026-08-05, user report, first real MCP session
Evidence: `hit_row` in apps/krate-checklist compares against `const WIDTH`. No
          app reads canvas_size on resize; only 3 of 34 call it at all. Resize
          and the canvas stretches while hit-boxes stay put, so clicks land in
          the wrong row. Reported as "cannot click on anything" and "graphics
          seem broken" -- one bug, two symptoms.
Fix:      Lay out from canvas_size, handle Event::Resized, keep layout in one
          place so drawing and hit-testing cannot disagree.

### K-004 — No clipping, so a scrolling list would draw over its own header
Status:   open
Owner:    unclaimed
Severity: serious
Class:    runtime-hole
Found:    2026-08-05, lead, capability audit
Evidence: one mention of "clip" in the whole of `gfx.wit`, not a clip rect.
          Confirmed while fixing K-001: with the wheel event working, a row
          scrolled above the list region paints straight over the title, and a
          row at the bottom edge shows its rounded corners beside the narrower
          text field. `apps/krate-checklist` works around both by hand -- it
          skips any row above the region and repaints a background band across
          the input strip before drawing it. That works and it is not a fix:
          every scrolling app has to reinvent it, and an app that draws rows of
          varying height cannot.
Fix:      A clip rectangle on canvas2d. Left unclaimed by W12: the scroll work
          shipped without it by working around it in the app, so this is a real
          remaining hole rather than something already covered. When it lands,
          delete the hand-rolled band and the strict top-edge test in
          `apps/krate-checklist` (`row_visible`, and the `fill` above the input
          strip in `draw`) -- both exist only because there is no clip rect.

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
Status:   fixed-pending-merge
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

### K-007 — Two of this machine's three AI accounts are unusable
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    environment
Found:    2026-08-05, lead, trying to verify Krate Mode end to end
Evidence: `claude -p` returns "OAuth session expired and could not be
          refreshed". `codex exec` returns "The 'gpt-5.6-sol' model requires a
          newer version of Codex".
          **Grok works.** `krate create --agent grok` authored a 584-line chess
          board in 237s and a real tip calculator in 229s, both from scratch --
          verified by timing and by reading the source. So authoring can still
          be measured end to end; it was a blocker and is not any more.
Fix:      Not ours. Yashraj re-authenticates Claude and updates Codex. Recorded
          because it once turned a 14/14 pass rate into a reported 23% and must
          not be mistaken for a product failure again.

### K-013 — Our development history leaks into every app a user makes
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-05, W17, outsider testing -- it read the Cargo.toml of an app
          it had just made and found notes addressed to us, not to it
Evidence: `crates/author/src/lib.rs:1045` writes this into every generated
          Cargo.toml: "It was missing here, so every windowed app the agent
          wrote had to discover the missing dep through a failed build and add
          it back by hand." Another names an image viewer that pulled four wasi
          imports. These are our bug-fix notes from past sessions, shipped to
          strangers inside their own app.
          W17 reasonably concluded from them that it had been handed a
          pre-built template rather than a fresh authoring run, and stopped.
Fix:      Comments in generated files should explain the code to the person who
          now owns it. The history belongs in our repo, not in their app.

### K-014 — A debug build shadows the real release on PATH
Status:   open
Owner:    unclaimed
Severity: serious
Class:    environment
Found:    2026-08-05, W17, checking what `krate` actually resolves to
Evidence: `which krate` gives
          `/Users/yashrajpardeshi/Projects/layer6x6/target/debug/krate`
          (`krate 0.1.0-dev`). The installed release at `~/.local/bin/krate`
          (rc20) is shadowed. Anything measured through the dev binary is
          contaminated: it is not the code a user runs.
Fix:      Not a product defect, but it silently invalidates measurements and has
          already made a fixed bug appear to come back twice. Every command in
          this repo must use an absolute path, and outsider testing must use
          ~/.local/bin/krate explicitly.

### K-015 — The `quick` run says "print something", so nothing can read what an app printed
Status:   open
Owner:    unclaimed
Severity: serious
Class:    teaching-hole
Found:    2026-08-05, W16, first Krate App Benchmark run
Evidence: `krate krate-mode` line 52-57 is the whole specification of the
          verification run's output: "do the app's real work once against a
          small built-in sample, **print something**, and exit 0". That is the
          only instruction about stdout anywhere in the pack.
          The pack's own worked example does the right thing --
          `write_pair(&stdout, "timezone", &timezone)`, one `key:value` per
          line -- and 17 of 17 shipped bundles follow it (`items:5`,
          `income:6500`, `index:0`). But the rule is never stated, only
          demonstrated, so an agent is free not to copy it.
          Benchmark request 1, "a tip calculator", authored via
          `--agent grok`. The app is CORRECT: bill $48.50, tip 18%, total
          $57.23. It printed all three on one line with currency symbols:
              $ krate run app.krate --auto-grant --shoot f.png -- quick
              bill:$48.50 tip:18% total:$57.23
          Only `bill?` is machine-readable. `tip>=0` and `total>=0` cannot be
          evaluated: `tip` and `total` are not at the start of a line, and the
          values carry `$` and `%`.
          Cost: a working app scored 1/6 on observable properties. Anything
          that reads app state -- this benchmark, the K-006 usability stage,
          CI -- is blind to an app that works.
Fix:      State the contract in the pack, do not just demonstrate it: on
          `quick`, print one `key:value` per line, bare values, no symbols or
          units, last line `quick:done`. `write_pair` already does exactly this
          and is already in the pack; promote it from example to rule.
          Highest leverage per line changed on the board -- it is one paragraph
          and it makes every future app readable by machine.

### K-016 — Generated apps bound their interactive loop and quit mid-use
Status:   claimed
Owner:    lead
Severity: blocker
Class:    teaching-hole
Found:    2026-08-05, W17, outsider testing -- it timed the same app three times
Evidence: Eight apps written by Grok from the public pack, and **all eight**
          bounded the interactive loop. tip/colour/expense/flashcards used
          `MAX_ROUNDS = 800` at `ROUND_MILLIS = 50` = exactly 40 seconds, timed
          at 45s/46s/43s. countdown and scratchpad 60s, chess 180s, maze 360s.
          The apps did separate `QUICK_ROUNDS` correctly; they simply believed
          the interactive loop should also end. A flashcard app that closes
          during revision is not a working app.
Fix:      The pack now says plainly that the interactive loop takes no bound at
          all, with the wrong pattern named and the right one shown. The bound
          belongs only on the `quick` path.

### K-017 — Nothing anyone can click reliably works
Status:   open
Owner:    unclaimed
Severity: blocker
Class:    unknown -- needs diagnosis
Found:    2026-08-05, W17, outsider testing
Evidence: The colour picker's Red "+" was clicked seven times across two runs
          and the app's own final state stayed at the untouched default
          `r:64 g:128 b:200`. One earlier run did end at `#FFFFFF`, so input
          CAN land -- which makes it unreliable rather than unwired, and that is
          worse. 0 of 8 apps were usable despite 8 of 8 building.
Fix:      Diagnose first. Could be K-003 (layout and hit-testing disagree), a
          pointer-event delivery bug in the macOS adapter, or generated apps
          hit-testing wrongly. Do not guess -- instrument one app and find out.

### K-018 — Layout collapses past four controls in a row
Status:   open
Owner:    unclaimed
Severity: serious
Class:    example-bug
Found:    2026-08-05, W17, outsider testing, via `--shoot`
Evidence: The 5th button in a row wraps and draws on top of the next row's
          text. In the expense tracker the "Add expense" button -- the app's
          most important control -- is covered by the "Other" category button.
          The chess board renders 7 columns instead of 8, sheared. The three
          apps that look right all use rows of 2-3 buttons; the maze escapes by
          drawing to a canvas.
Fix:      Likely downstream of K-002 (no text measurement, now fixed) since
          button widths were guessed. Re-test after K-002 merges before
          treating it as separate.

### K-019 — `krate ai` reports broken providers as ready
Status:   open
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-05, W17, outsider testing, cold start
Evidence: `krate ai` lists Claude and Codex under "Ready to use" when both fail
          on invocation -- Claude's auth is expired, Codex needs a newer CLI.
          It only checks whether the binary is on PATH. A newcomer follows that
          advice, picks Claude, and gets a failure that looks like Krate's.
Fix:      Either probe the provider cheaply, or soften the wording from "Ready
          to use" to "installed" and say a sign-in may still be needed.

### K-020 — Double-clicking a .krate opened a file picker, not the app
Status:   open
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-05, W17, outsider testing
Evidence: Double-clicking a `.krate` produced an off-screen file picker rather
          than opening the app. Double-click is the headline promise on the
          website and the simplest path we advertise.
Fix:      Reproduce on a clean machine first -- this may be the stale
          /Applications/Krate.app trap that has bitten before.

### K-021 — An absolute path to target/release is the WRONG binary in a worktree
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    environment
Found:    2026-08-05, W15, after losing significant time to it
Evidence: CLAUDE.md says to invoke
          `/Users/yashrajpardeshi/Projects/layer6x6/target/release/krate` by
          absolute path. From a worktree that is the MAIN repo's binary, built
          by another workstation before the caller's WIT existed. It reported
          "this app needs a newer version of Krate", which reads as a broken
          app rather than a stale tool.
Fix:      The rule is "absolute path to YOUR OWN worktree's target". CLAUDE.md
          needs correcting.

### K-022 — A bound canvas never learned its window had been resized
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

### K-023 — Running an app from its source dir writes its data into the repo
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

### K-024 — krate-pulse pins its canvas to constants, so it ignores a resize
Status:   open
Owner:    unclaimed
Severity: serious
Class:    example-bug
Found:    2026-08-05, W14, first run of the new usability stage across apps/
Evidence: Found by the usability stage, not by a person -- which is what the
          stage is for:

              $ ./target/release/krate check-app apps/krate-pulse
              FAILED at usability
              the window was resized to 1300x840 and the app's canvas stayed
              1080x700, so its layout is not following the window
              EXIT=16

          `apps/krate-pulse/src/lib.rs:503` and `:518` set
          `width: Some(WIDTH), height: Some(HEIGHT)` on the canvas node, with
          `const WIDTH: f32 = 1080.0` at :33. The file has zero matches for
          both `canvas_size` and `Resized`, so nothing re-lays it out.
          It is the only shipped app that pins its canvas this way
          (`grep -ln "width: Some(WIDTH)" apps/*/src/lib.rs`).
Fix:      Same shape as K-003: drop the fixed style on the canvas node, lay out
          from `canvas2d::canvas_size`, and handle `Event::Resized`. Left
          unclaimed rather than fixed here, because K-003 is W13's and this is
          the same repair on a second app -- it should go with that work.

---

## Fixed

### K-001 — Canvas apps cannot scroll: there is no scroll event
Status:   fixed
Owner:    W12
Severity: blocker
Class:    runtime-hole
Found:    2026-08-05, lead, auditing the WIT against a user report
Evidence: `wit/krate/phase3/deps/ui/ui.wit` defined ten event variants
          (close-requested, resized, redraw-requested, pointer, key, text-input,
          text-changed, action, focus-changed, theme-changed). No wheel, no
          scroll delta. `scroll` appeared once as a widget kind, unrelated.
          A user's habit tracker held 32 items, showed 6, and the rest were
          permanently unreachable behind a "+ N more" label.
          The runtime already captured wheel input on Linux and Windows and
          spent it entirely host-side on `scroll` widget containers -- a canvas
          app has no such container, so every scroll was silently dropped.
          macOS captured none at all.
Fix:      COMMIT_PLACEHOLDER. An eleventh event variant, `wheel(wheel-event)`,
          shaped like `pointer-event` (window, widget, x, y, modifiers) plus
          `dx`/`dy` in logical pixels, positive-down and positive-right, with a
          notch normalized to ~20px so one gesture moves a list the same
          distance on all three systems.
Proof:    `krate check-app apps/krate-checklist` prints OK, and the app's
          verification run prints `items:20 / saved:yes / scroll:ok 120` --
          `scroll:ok` means a wheel delta moved the list and clamped at both
          ends. `krate run --shoot` renders the list starting at item 3 with a
          proportional scrollbar, where it used to render six items and
          "+ 14 more".
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
