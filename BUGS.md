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

### K-103 -- the benchmark scores correct apps as failures over key names
Status:   open
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-12, mid-run of the benchmark re-run, when five consecutive
          failures all turned out to be the same shape.
Evidence: Four of the first five failures are apps that **work**, failing on
          the name or format of a key rather than on behaviour:

            req-5  click counter   wants count>=1
                   printed: clicks:60 frames:61 counts:yes
                   -- it clicked sixty times; the key is `clicks`

            req-8  BMI calculator  wants height? weight?
                   printed: bmi:22.8 category:Normal
                            height_cm:178 weight_kg:72.5 units:metric
                   -- entirely correct; the keys carry their units

            req-9  stopwatch       wants elapsed>=0
                   printed: elapsed:2:20.18 elapsed_ms:140180 laps:4
                   -- `elapsed` is there, formatted as time, so the
                      numeric compare cannot read it

            req-7  password gen    wants password?
                   printed: length:32 chars:32 bits:191 distinct:21 ...
                   -- eight keys, none of them the password itself.
                      This one is arguably a real gap.

          Only req-2 (to-do list) is a plain failure: it printed nothing.

          A sixth landed after this entry was first written, and it is a
          different bug -- in the corpus rather than in the apps:

            req-10 case converter  wants upper~ABC;lower~abc
                   printed: upper:HELLO WORLD. IT'S A FINE DAY!
                            lower:hello world. it's a fine day!
                            title:Hello World. It's A Fine Day!
                   -- `~` is a literal substring match, so `upper~ABC`
                      only passes if the app's own sample text happens to
                      contain the letters ABC. The author meant "some
                      uppercase text"; the assert says something narrower
                      and unmeetable.

          Swept the rest of the corpus for the same shape: three asserts use
          `~`, and the other two (`hex~#`, `output~{`) match structural
          characters any correct app emits. **Request 10 is the only one
          that assumes particular sample data.** Fixing it is a one-line
          corpus edit, and it must be made before the next run rather than
          after seeing this one's score.
Impact:   **The headline number understates the product.** A benchmark that
          fails a correct BMI calculator because it wrote `height_cm`
          instead of `height` is measuring vocabulary agreement, not
          usability -- and the whole reason this harness exists is to stop
          measuring the wrong thing.
Fix:      Not by loosening the bar. Two honest options:
          1. Teach the contract to include the key names, so the app and
             the corpus share a vocabulary -- the same move that fixed the
             "print something" gap (K-102).
          2. Let an assert accept alternatives (`count|clicks>=1`) and a
             numeric compare fall back to a `<key>_ms` sibling.
          Option 1 is better: it makes generated apps more legible to
          everything, not just to this harness.
Note:     Do not quietly rescore the run. The number stands as measured;
          this entry is what it means. Both figures belong in RESULTS.md --
          the raw pass rate, and how much of the gap is vocabulary.

### K-102 -- krate-mode still says "print something", the exact contract that scored 0/5
Status:   fixed 2026-08-12
Owner:    lead
Severity: serious
Class:    teaching-hole
Found:    2026-08-12, reading the old benchmark results before re-running it.
Evidence: The 2026-08-05 benchmark scored 0 of 5 on authored apps, and every
          one failed the same gate for the same reason: the app worked and
          could not prove it. A tip calculator computed the right answer and
          printed `bill:60 tip%:18 people:2 total_cents:7080` -- three keys
          on one line with invented names. A dice roller printed nothing at
          all. RESULTS.md traced the cause to the pack's own words:

            "do the app's real work once against a small built-in sample,
             print something, and exit 0"

          The **authoring pack** (`krate authoring-context`, what `create`
          uses) was since fixed and now says it properly:

            "Print one `key:value` per line, and make the keys mean
             something ... 'print something' is not enough: an app that
             prints `ok` builds, runs, paints a frame, and proves nothing"

          **`krate krate-mode` was not.** Line 54 of the published prompt
          still reads "print something, and exit 0":

            $ krate krate-mode | sed -n '54p'
            once against a small built-in sample, print something, and exit 0

          So the two authoring paths teach different contracts. Anyone who
          pastes Krate Mode into a chat model gets the 2026-08-05 behaviour
          that scored zero.
Fix:      The prompt now carries the same contract as the pack: one
          `key:value` per line, a worked example, the formatting rules, and
          the 2026-08-05 evidence for why it matters (a tip calculator that
          computed the right answer and still scored zero).

          `the_quick_contract_matches_what_the_authoring_pack_teaches` pins
          it, and asserts on substance rather than phrasing: the prompt must
          name the format, must show an example, and **must not contain the
          string "print something, and exit 0"**. It also checks the pack
          still agrees, so a future edit to either one cannot split them
          again silently.
Note:     The first attempt did not compile -- quoting the old wording inside
          the prompt's Rust string literal ended the literal early. Same
          class of mistake as the earlier one where a friend's quoted words
          broke the pack: prose that talks about prose needs its quotes
          checked.
Note:     Found while re-running the benchmark, and it predicts the shape of
          the result: `create` uses the fixed pack, so the number should be
          better than 0/5 -- but that improvement would not reach anyone
          using the paste-in path.

### K-101 -- a network fetch freezes the app for its whole duration
Status:   fixed 2026-08-12 -- `begin`/`poll`/`cancel` ship alongside the
          blocking `fetch`, which stays for one-shot CLI tools
Owner:    lead
Severity: serious
Class:    runtime-hole
Found:    2026-08-12, checking what Krate can actually build before starting
          the compatibility programme (Plan/krate-compatibility-2026-08.md).
Evidence: A guest is single-threaded -- no thread or concurrency primitive
          exists anywhere in the WIT -- and the host's HTTP client is `ureq`,
          which is synchronous (`Cargo.toml:86`). So `http_client::get`
          blocks the guest's event loop until the response is complete.

          Measured against a local server that stalls 3 seconds:

            $ krate run krate_fetch.wasm --auto-grant --headless \
                -- http://127.0.0.1:8799/
            fetch:ok:356
            ELAPSED: 5.37s

          The app could not draw, animate, or answer a click for the whole
          stall.
Impact:   **A progress bar during a download is impossible, and so is a
          cancel button.** Every networked Krate app is currently either
          unresponsive while it works or too trivial to notice. This is the
          single limit most likely to be hit by an outside developer
          building something real.
Fix:      Three new calls on `krate:net/http-client`: `begin` returns a
          handle at once, `poll` answers immediately with
          pending/ready/failed/unknown-handle, and `cancel` retires a
          handle (what a cancel button calls). The work runs on an OS
          thread; the guest keeps its own loop.

          **The grant is checked at `begin`, on the calling thread, before
          any worker exists** -- a handle is only ever issued for a host the
          person allowed. Pinned by
          `the_async_path_refuses_a_host_that_was_never_granted`.

          The adapter hands over a `FetchJob` (a `Send` closure holding only
          plain data) rather than being sent itself, because host state is
          `Rc<RefCell<_>>`. Blocking and async run the *same*
          `perform_http_request`, so there is no second HTTP path to drift.
Verified: `apps/zz-asyncproof` against a server that stalls 3 seconds --
          the same server, both paths:

            blocking  krate-fetch:      8.36s wall, 0 turns of guest work
            async     zz-asyncproof:    begin returned in 1 ms
                                        turns_while_waiting: 258
                                        status 200, total 3017 ms

          258 turns is 258 frames the app could have drawn, or clicks it
          could have answered, while the network was slow. Seven unit tests
          cover the handle table, including that a retired handle does not
          leak and that a double-cancel does not panic.
Note:     `fetch` is unchanged and still right for a one-shot CLI tool where
          nobody is watching a window. Streaming bodies (chunks rather than
          one buffered `list<u8>`) remain the follow-on, and would lift the
          in-memory ceiling on large downloads -- not filed yet.

### K-100 -- roughly one in eleven app opens fails, and nobody knows why
Status:   fixed 2026-08-12 -- reason codes ship in v0.1.12; the cause of the
          remaining real failures needs a week of data to name
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-12, reading hub.krate.tech/stats while documenting real
          traction numbers for the guide. Not reported by anyone -- the
          telemetry has been carrying this the whole time.
Evidence: Five days of recorded actions from the live stats endpoint:

            $ curl -s https://hub.krate.tech/stats
            open          4187
            open-failed    425
            install        235

          425 of 4612 open attempts failed: a 90.8% success rate. The
          telemetry records that an open failed but not why, so there is
          no way from here to tell a missing runtime from a bad bundle
          from an app that crashed on startup.
Fix:      `usage::OpenFailure`, a closed enum of eight reasons, classified
          from the finished run and sent as a `why` field. Closed rather
          than free-form for the same reason `Action` is: a string would
          eventually carry a path or an app name. The worker validates
          against the same list and drops anything else.

          Verified end to end against a local collector, not just in unit
          tests -- each line below is a real run of the release binary:

            $ krate run /nope/missing.krate
            {"action":"open","ok":false,"why":"not-found"}

            $ krate run seating3.krate --auto-grant
            {"action":"open","ok":true}

            $ krate run krate_tidy.wasm --manifest ... </dev/null
            exit=5
            {"action":"open","ok":false,"why":"refused"}

          Twelve tests cover the classifier, including that the whole error
          chain is read (the CLI wraps almost everything in context, so a
          cause under a context line must still classify) and that every
          reason is a fixed lowercase word.
Finding:  **`refused` is not a failure.** The permission wall turning an
          app away exits 5, and exit 5 was being counted as a failed open.
          Some unknown share of the 425 was the product working correctly,
          so the true defect rate is lower than 9.2% -- by how much, the
          next week of data will say. This is why the fix had to be a
          reason code and not a guess at a cause.
Next:     Read blob7 of the krate_usage dataset once v0.1.12 has been out a
          week, then fix whatever dominates after `refused` is excluded.
          `/stats` carries the query to run.
Note:     This is the highest-value signal we have that is not a guess --
          it is real people, on real machines, failing to open real apps.
          Worth more than another install-count push: fixing a real failure
          rate helps every future user, and G5 needs opens that work.

### K-099 -- nothing measures wasted space inside a generated app's window
Status:   open
Owner:    unclaimed
Severity: moderate
Class:    runtime-hole
Found:    2026-08-12, after K-098's teaching fix. Generated apps now use the
          right primitives, but an arbitrary new app still spends a third of
          its window on nothing, and no gate can see it.
Evidence: Wrote an edge-band check (largest fraction of the window, measured
          in from an edge, containing no content) and measured it against
          six real screenshots. It does not work:

            seating.png    2240x1440  band=0.061
            seating2.png   2360x1520  band=0.034
            seating3.png   2240x1440  band=0.044   <- visibly wasteful
            krate-savings  920x1120   band=0.041
            krate-pulse    2160x1400  band=0.060

          Every app scores 3-6%, including the one with an obvious dead
          region, because real apps put a full-width header or footer near
          each edge, so no complete row or column is ever empty. The check
          only catches content pinned away from an edge, which is K-096's
          resize bug and already covered.
          Reverted rather than shipped: a gate that detects nothing while
          claiming to is worse than no gate.
Fix:      Needs a region measure, not an edge measure -- e.g. divide the
          frame into a coarse grid and report the largest connected run of
          cells that carry no content, ignoring cells the background
          gradient alone fills. Must keep the false-positive guard: a
          deliberately airy layout is not a defect, so the bar has to be
          "nothing was drawn here at all", not "this looks sparse".
Note:     The teaching half is done (see the density rules in
          `authoring_context.rs`) and measurably helps, but the AI trades
          density off against overlap between runs, which is exactly the
          kind of regression only a machine check holds.

### K-098 -- the pack taught a rounded-corner hack for a primitive that shipped
Status:   fixed 2026-08-11 (commit e6df2bf)
Owner:    lead
Severity: serious
Class:    teaching-hole + example-bug
Found:    2026-08-11, asking why a first-time user's app of an unfamiliar
          kind looks dated when the showcase apps look current.
Evidence: `fill-round-rect`, `stroke-round-rect`, `drop-shadow-round-rect`,
          `linear-gradient-stops` and `draw-text-styled` are all in
          gfx.wit. The hand-written design section of the authoring pack
          mentioned none of them, and instead prescribed:

            "a rounded rect for every card and button (fill the middle
             rectangle, then the four corner circles with `fill_circle`)"

          So the pack's own prose contradicted the function list two
          hundred lines below it, and prose is what a model follows.

          Nine example apps still carried the hand-built helper, which is
          the example-bug half: the AI copies these files.

            $ grep -rl "corner disc\|cross of two rects" apps/*/src/*.rs
            krate-checklist krate-clip krate-contacts krate-eo2
            krate-fetch krate-journal krate-mdview krate-notes krate-pulse

          Not cosmetic. krate-checklist's `stroke_rounded` drew corners as
          1px discs, so every empty checkbox and the "Add an item" field
          rendered as a broken dotted box -- confirmed by
          `krate run --shoot` before and after.
Fix:      Rewrote the pack's design section around the primitives that
          exist (including shadows and font weight, neither taught before)
          and converted all nine apps. Each helper is now one host call.
          Per-corner radii replaced krate-journal's bubble tail and
          krate-mdview's stacked strips; krate-pulse's offset opaque rect
          became a real blurred shadow. Fleet 32/0 after.
Lesson:   When a primitive ships, the teaching is not done until the pack's
          *prose* changes and the examples stop demonstrating the old way.
          A function list the model never reads is not teaching.

### K-097 -- a scripted fix gave seventeen apps a square design size
Status:   fixed 2026-08-11, same day it shipped
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-11, screenshotting the example apps after people said
          Krate needed "better UI and overall experience". krate-checklist
          had a white band across its top with the title nearly invisible
          against it, and its "Add an item" row was cut off entirely.
Evidence: The K-096 conversion script matched each app's size constants
          with a regex, and `const (\w*H(?:EIGHT)?)` matched WIDTH as
          happily as HEIGHT -- so seventeen apps were told their design
          size was WIDTH x WIDTH. A 440x440 design area inside a 440x620
          window leaves 180 rows outside the app's coordinate system,
          showing the surface's initial white.
          It shipped in v0.1.10 and the gate did not catch it: a square
          design size is still a valid design size, so every resize check
          passed. Only looking at the pixels found it.
Fix:      All seventeen corrected to their real height constant, verified
          by screenshot (the white band is gone and the input row is back)
          and by the fleet staying 32 pass / 0 fail.
Lesson:   A scripted edit across many files needs its output READ, not
          just its exit code. And an app's appearance needs an eye on it;
          a gate that checks behaviour will pass something that looks
          broken.

### K-096 -- 21 of 32 apps ignore the window size, and the gate could not see it
Status:   fixed -- gate, runtime, all 21 apps, and the authoring pack
Owner:    lead
Severity: blocker
Class:    example-bug
Found:    2026-08-11, by Yashraj's friend on Windows, trying an
          AI-generated game: "the character and ground is out of screen
          even after changing ratio and everything". The header and hint
          text drew correctly; the game world did not.
Evidence: 21 of 32 apps never call `canvas2d::canvas_size` -- they draw
          from constants. The host then SCALES that fixed-size picture to
          fill the window (adapter-common/painter.rs draw_image, "fit
          inside, preserving proportions"), which is why a hardcoded app
          looks stretched and blurry, and why a game with a camera puts
          its world off the edge.
          Worse, check-app passed every one of them. Its resize check
          only asked whether the canvas RECT grew -- and it always does,
          because the layout engine resizes it without the app's help.
          Two further reasons the check never even ran: with the `quick`
          argument most apps exit before opening a window, and the driver
          runs in a child process, so its own diagnostics never reach the
          parent's terminal.
Fix:      The resize check now asks what resolution the app is ACTUALLY
          drawing at (`CanvasSurface::dimensions`) and fails when the
          window grew but the app kept rendering at its old size, naming
          the consequence in the message. Verified both directions:
          krate-bounce (hardcoded 320x240) -> broke, krate-gram (reads
          canvas-size) -> held. A unit test locks the margin logic, and
          that test caught a real bug in my first implementation -- it
          used the top-left pixel as "background", which in the very case
          this exists to catch is painted content.
          The apps were fixed by giving the runtime a better answer than
          "rewrite every constant". `canvas2d.set-design-size` is new in
          the WIT: an app declares the coordinate system it was designed
          for, keeps drawing in those numbers, and the host scales them
          UNIFORMLY to any window and centres the remainder -- letterboxed
          like a console on a widescreen TV, never distorted. Pointer and
          wheel events are converted back into design space on the way to
          the guest, so hit-testing needs no change at all. Eighteen apps
          took one call each; krate-chart and krate-weather already read
          canvas-size and only needed to redraw on Event::Resized (their
          own comment said "redraw is not needed, the canvas keeps its
          raster" -- true until a resize refits the canvas and the old
          picture is gone).
          Two host bugs surfaced while proving it: a resize replaced the
          whole surface and silently dropped the design size, so a correct
          app looked wrong one frame later; and canvases only refitted when
          an app called canvas-size, so an app that never asked kept
          drawing at its opening size forever. Both fixed.
          The gate now also exempts what it cannot judge: an app with no
          canvas2d surface (krate-cubes draws through scene3d) is not
          asked the render-size question, and a design-space app is
          recognised as adapting rather than ignoring the window.
          Fleet: 11 pass / 21 fail -> 32 pass / 0 fail. 1141 tests pass.
          A regression guard confirms the gate still fails an app with the
          fix removed. The pack teaches both honest strategies -- lay out
          from canvas-size, or fix a design size -- so generated apps
          inherit a choice instead of a bug.

### K-095 -- an interactive app handled one input event per frame
Status:   fixed in the fleet and taught in the pack; the device verdict
          on iOS responsiveness is still pending
Owner:    lead
Severity: blocker
Class:    example-bug
Found:    2026-08-11, on Yashraj's iPhone. His words named the shape
          before the numbers did: "first swipe seems okayish, later ones
          scroll with delay" -- the signature of a queue that grows
          rather than a pipeline that is slow.
Evidence: The device trace showed the pipeline was healthy where it was
          measured -- scene 0.4 ms, render 3.7 ms, touch->wheel 0.9 ms --
          while frames locked to exactly 33.3 ms and the app presented
          precisely 50% of the vsyncs it was told about. Two causes
          compounded:
          1. The guest consumed ONE event per `events.wait` call and then
             spent a whole frame drawing. A drag reports at up to 120 Hz,
             so the backlog grows for as long as the finger moves; the
             first swipe starts empty and every later swipe inherits it.
          2. The runtime paced `present` with its own timer on top of a
             GPU adapter that already blocks for the panel inside
             get_current_texture. An earlier attempt to remove that made
             the guest free-run instead -- visible in the trace as
             gpu-present-done at 279.0 ms followed by gpu-present-start
             at 280.2 -- so FIFO absorbed the wait and every frame landed
             one refresh late.
Fix:      Apps drain the queue with `events::poll()` before drawing, then
          block once (krate-gram and krate-wall both did one-per-frame;
          the wall is the sheet every app shows first, so its scroll is
          the first thing anyone touches). The runtime no longer paces
          present at all when the adapter blocks for vsync -- the display
          is the only clock. The authoring pack now teaches the drain
          pattern with the measurement behind it, so generated apps do
          not inherit the bug.

### K-093 -- an app run from loose source can never see its own assets
Status:   fixed 2026-08-11
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-11, clearing the last of K-092's fleet failures:
          krate-spriteproof and krate-nova2 both exited 40
          ("asset:bg-missing") under check-app while running perfectly
          when packed
Evidence: `bundle_assets_root` was only ever filled from a packed
          `.krate`, and `krate run` had no way to say where assets live.
          check-app's run stage runs loose source, so any app that reads
          an image failed the gate for a reason that had nothing to do
          with the app. Every AI-authored app that uses a picture would
          have hit this.
Fix:      `krate run --assets <dir>` now exists and wins over a bundle's
          own assets; check-app passes the app's `assets/` folder when
          one is present, as an ABSOLUTE path -- run_self sets the
          child's cwd, so a relative path silently resolved elsewhere and
          handed the app nothing (the same trap the wasm and manifest
          paths already documented). Both apps pass.

### K-094 -- the usability driver stalls on an app that is behaving correctly
Status:   fixed 2026-08-11, verified in both directions
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-11, the last failure in the fleet sweep
Evidence: apps/zz-example-check fails check-app with "the app did not
          finish its verification run within 60 seconds and was stopped".
          The app itself is fine and does exactly what the pack teaches:
          `events::wait(None)` in a real session, a bounded quick path.
          Run directly it exits 0 in 0.4 s:
            krate run <wasm> --manifest ... --auto-grant --headless -- quick
            -> "counter: window ran", exit 0, 0.4s
          So the driver, not the app, is what cannot finish.
Fix:      DONE, in the driver, with the app untouched. The cause: the
          script only advanced where `events.wait` is ENTERED, but an app
          blocked in `wait(None)` on an empty queue never enters it again
          -- so the driver took one step and the wait loop slept until the
          harness killed the run at 60 s. The loop now steps the driver
          from inside an unbounded wait as well, delivering the script's
          clicks and resizes as ordinary events, which is how a person's
          input arrives anyway.
          Verified both directions, because a gate that stops failing is
          worthless if it also stops catching: zz-example-check now
          passes in 16.6 s, and a deliberately planted self-closer (a
          real session bounded at 40 rounds x 50 ms) is still caught --
          "closed it by itself after 2.3s". Fleet: 32 pass, 0 fail. 1140
          tests pass.
          Worth recording: my first regression attempt planted a bound of
          400 rounds, which is 20 seconds -- longer than the 15-second
          stay-open watch, so the gate passed it correctly and I briefly
          believed I had broken the gate. The test was wrong, not the
          code.

### K-092 -- eight fleet apps close their own window mid-session
Status:   fixed for the whole self-closing class 2026-08-11; the six
          remaining failures are unrelated causes, listed below
Owner:    unclaimed
Severity: serious
Class:    example-bug
Found:    2026-08-11, sweeping check-app across the whole fleet while
          verifying a work queue -- the board said "four older apps"
          (K-025) and the truth was thirteen, with one cause under most
          of them
Evidence: `krate check-app` over every app in apps/ with a manifest:
          19 pass, 13 fail.
            layout stage (7): krate-clip, krate-contacts, krate-fractal,
              krate-keyvault, krate-nova2, krate-spriteproof,
              krate-weather -- all seven report the same thing, e.g.
              "line 96: the interactive loop is bounded by a round count
              (MAX_ROUNDS), so the app closes itself while somebody is
              still using it"
            usability (1): krate-notes -- "opened a window and then
              closed it by itself after 12.6s, with nobody asking"
            manifest (2): krate-eo2, krate-mdview -- ask for a capability
              whose interface the component never imports
            run (2): krate-curl, krate-hello-gui -- fail headless with
              all grants, exit 1
Fix:      DONE. Fifteen apps carried a bounded interactive loop; each now
          runs unbounded in a real session and keeps its bound only on the
          `quick` path. Four shapes existed and all four are fixed:
          `MAX_ROUNDS`, `MAX_FRAMES`, a bare literal (400), and
          krate-clocks' hour-long cap (which loops plainly now, since its
          quick path returns earlier).
          Two more of the same family were found while verifying, one of
          them by Yashraj clicking the app by hand:
            - krate-hello-gui closed itself a moment after the button was
              clicked (LINGER_ROUNDS_AFTER_CLICK, then break). A demo
              script shipped as an app, and the first thing an AI copies.
            - krate-notes closed after ~12 seconds of quiet
              (MAX_IDLE_ROUNDS) -- a person reading a note is idle.
          hello-gui also reported exit 2 for "closed before clicking" and
          1 for a bounded run; closing a window is how a session ends, so
          it is 0 now.
          Fleet: 19 pass / 13 fail -> 26 pass / 6 fail. 1140 tests pass.
          The headless side needs no counter: the wall-clock budget in
          phase3_gui_host already ends a run that feeds itself redraws,
          verified at 5.1s (a counter cannot work there, because
          request-redraw resets any idle count).
Left:     krate-curl (exit 20), krate-nova2 + krate-spriteproof (exit 40,
          missing packaged assets bg.rgba/sprite.rgba), krate-eo2 +
          krate-mdview (manifest asks for an interface the component
          never imports), zz-example-check (a fixture, not an app).
          None is a self-closing window; each is its own small bug. Then re-run the sweep and take the manifest and
          run failures individually. This is example-bug class and the
          highest leverage left in the fleet: the authoring pack points
          AIs at these files, and krate-hello-gui is indexed as "the
          smallest GUI app" -- the first thing a generated app copies.

### K-091 -- every app launch blocks ~68 ms on a telemetry round-trip
Status:   fixed 2026-08-11, verified by measurement and by delivery
Owner:    unclaimed
Severity: blocker
Class:    our-code
Found:    2026-08-11, decomposing the "is Krate slower than native"
          question for outreach claims -- the answer was yes, and this
          was 91% of the reason
Evidence: Median wall time, `krate run` on an 18 KB app, this Mac:
            normal                       73.9 ms
            KRATE_NO_USAGE=1              6.4 ms
            krate --version (load only)   5.9 ms
            runtime compile+run           3.3 ms   (criterion)
            steady state                    61 us  (criterion)
          The runtime is fast. The product is not, because
          crates/cli/src/usage.rs:250 joins the reporting thread against
          a 600 ms deadline on the way out -- the right fix for a lost
          event (a detached thread loses the race with process exit) put
          in the wrong place: the user's launch path.
Fix:      Two parts, because the obvious half was not enough. Events are
          spooled to ~/.krate/usage-spool.jsonl (one JSON object per line,
          capped at 200) before anything touches the network, so nothing
          is lost even if the process dies immediately. Then a DETACHED
          HELPER PROCESS (`krate usage-flush`, hidden, this same binary)
          drains the spool and outlives its parent.
          A background thread was tried first and provably does not work:
          every command exits in single-digit milliseconds while a round
          trip takes hundreds, so the thread is killed by process exit
          every time -- observed as a spool that grew to 21 events and
          never drained. That is the same race the original blocking join
          existed to win; spooling first means it no longer has to be won.
          Measured after: 73.9 ms -> 7.8 ms with telemetry on, identical
          to having it off. A dead hub costs 9.9 ms instead of up to 600,
          and its events survive on disk and are delivered on the next
          launch (verified offline, then reconnected). The helper sets
          KRATE_USAGE_HELPER so it can never record its own run or spawn
          another helper. 1140 tests pass.

### K-090 -- the iOS player ran hot, died in the background, and felt late
Status:   fixed in main (three cuts, each from a crash log or a measured
          probe); device re-verification with the founder's thumb pending
Owner:    lead (M2 workstream)
Severity: blocker (a stranger's first iPhone impression)
Class:    our-code
Found:    2026-08-10, by Yashraj on his iPhone 13 Pro: "slow, laggy and
          late in response" -- against a simulator that felt fine
Evidence: The device's own crash logs, pulled over devicectl. One:
          0x8BADF00D, "scene-update watchdog transgression ... exhausted
          real time allowance of 10.00 seconds", WatchdogVisibility:
          Background -- every backgrounding silently killed the app,
          because the player's main never returns to UIKit and iOS could
          not suspend it. Two: a cpu_resource violation with "Thermal
          Level 4 / Thermal State serious" -- the 100 Hz idle poll churn
          (park 10 ms, wake, poll, repeat, forever) heated the phone and
          the throttled silicon made everything feel slow. Three: the
          blind 10 ms sleep in events.wait meant a touch could land
          mid-nap before anything noticed it.
Fix:      A UiAdapter::park_for_events contract: an adapter that can wait
          inside its native event loop does, and the host skips its blind
          sleep -- iOS parks in NSRunLoop, which wakes the instant input
          arrives. Idle parks are 250 ms (waking is event-driven; short
          slices only burned battery); frame pacing parks instead of
          sleeping so lifecycle traffic flows mid-animation; and
          backgrounding exits the process cleanly -- the same visible
          effect as the watchdog kill, minus the kill, until M3's real
          suspend/resume. Desktop untouched: park defaults to false and
          the old sleep remains.
          Second wave, each cut from an on-device number after the thumb
          said "still the same". The guest moved to its own thread (a
          nested run loop never drains the main dispatch queue, and iOS
          routes parts of its own touch pipeline through it). Touch-scroll
          deltas coalesce (120 Hz reports vs one frame per event queued a
          two-second replay). The window joined its UIWindowScene (an
          unattached window rides a legacy event path: 116 touch callouts
          in a whole session became a steady 60 Hz stream, gap p50 16.7
          ms, measured on-device). The guest thread runs user-interactive
          QoS. Canvas text layouts cache behind an Rc; rounded photo
          blits skip the SDF outside the corner bands.
          What the numbers say remains: blit 0.5 ms, event turnaround
          near zero, frames at ~37 ms -- CPU raster at phone resolution
          IS the frame. A 120 Hz-native feel is not reachable by CPU
          cuts; it is the wgpu backend's job, now the mobile plan of
          record for rendering.

### K-089 -- phone-resolution CPU raster spent the whole frame budget
Status:   fixed (three cuts, measured before and after)
Owner:    lead (M2 workstream)
Severity: serious
Class:    our-code
Found:    2026-08-10, by Yashraj using the simulator player: "lagging,
          like very slow frames, clearly not the experience I or the
          user want" -- the outside eye caught what screenshots cannot
Evidence: A probe in the iOS adapter's blit: 15-21 ms per frame for the
          blit ALONE, before the guest drew anything, at the iPhone's
          native 3x (1206x2622 = 3.2M pixels). Three separate costs: a
          fresh 12 MB buffer allocated and zeroed every frame; the
          placement paint bilinear-resampling 3.2M pixels that needed no
          resampling (the canvas was already at physical resolution);
          and every pixel of that surface rasterized at 3x on the CPU.
Fix:      Three cuts, blit measured 15-21 ms -> 5.8-6.7 ms. The paint
          buffer lives in the host and is reused. The shared draw_image
          gained an identity fast path -- 1:1 scale, whole-pixel origin,
          opaque source is a row copy, not 4-tap sampling -- locked into
          every platform's blit. And the iOS adapter caps its raster
          scale at 2x, letting UIImageView's GPU compositor do the final
          2x->3x stretch for free; the wgpu backend (mobile plan phase 3)
          is the eventual full-density answer.
          The Android leg, audited 2026-08-10: 129.8 ms per paint on the
          emulator, three measured causes. The identity fast path missed
          on float dust (2.625 x a rounded logical size = scale 1.000004;
          strict equality sent 2.6M pixels through bilinear -- now snaps
          within half a pixel). The painter wrote row-by-row into the
          ANativeWindow's uncached write-combined memory (now paints a
          cached staging Vec, bulk-copies once). And redraw_all repainted
          every pump, dirty or not (both mobile adapters carry a dirty
          flag now; a still image costs nothing, and iOS stops
          double-blitting). Sections before: acquire 9.1, paint 41.8,
          present 17.8; after: 2.7, 12.8, 10.6 -- 26-31 ms per paint,
          ~5x, real devices expected faster. Full-density 60 fps remains
          the wgpu backend's job.

### K-088 -- the Android blit ignores display scale, so every app is pixelated
Status:   fixed
Owner:    lead (M1 workstream)
Severity: serious
Class:    our-code
Found:    2026-08-09, first light on the Android emulator: krate-gram drew
          correctly but visibly pixelated
Evidence: The first-light screenshot. The Pixel 7 surface is 1080x2400
          physical at scale ~2.6; the painter rasterized at logical 390x720
          and the blit stretched it. Desktop learned this exact lesson as
          K-067 (logical pixels, host owns scale) -- the Android adapter has
          the snapshot's scale_factor and does not yet paint at physical
          resolution. Also visible: a white band where the aspect-mismatched
          remainder of the surface goes unpainted.
Fix:      Two halves, both landed. The Android adapter now converts every
          size at its boundary (create snapshot and Resized events divide
          by scale_factor), so the app and layout live in logical pixels
          like the input path already did. And CanvasSurface rasters at
          physical resolution: new_scaled keeps logical dimensions for the
          app while the buffer is logical x scale, every public draw call
          converts once at entry, and the three delegating calls (text,
          full-sweep arc, one-stop gradient) delegate on raw arguments so
          nothing scales twice -- locked by
          a_scaled_surface_rasters_physical_and_reports_logical. The
          placement blit is then 1:1. Desktop is bit-identical at scale 1:
          the whole suite passed untouched, and the gram desktop shot
          matches. Verified on the emulator: full-bleed, crisp.

### K-087 -- krate-gram hardcodes its size instead of reading canvas-size
Status:   fixed
Owner:    lead (M1 workstream)
Severity: serious
Class:    example-bug
Found:    2026-08-09, first light on Android: the phone's surface is not
          390x720, and the flagship modern-UI example cannot adapt
Evidence: apps/krate-gram/src/lib.rs WIDTH/HEIGHT constants drive every
          coordinate; canvas2d::canvas_size is never called. W13 exists
          because of exactly this class -- and the example apps are what
          every generated app learns from. On a phone the feed letterboxes
          into the wrong aspect instead of filling the screen.
Fix:      A Layout struct computed from canvas-size at the top of every
          frame drives every coordinate: the feed column caps at 480 and
          centers on wide surfaces, the photo rect derives from the column,
          scroll extents and hit boxes from the same struct. The window
          size request is explicitly labeled a request, not the truth. The
          pack's index line for krate-gram now names the canvas-size
          pattern so generated apps inherit the fix. Verified: fills a
          Pixel 7 edge to edge, desktop shot unchanged.

### K-086 -- the dialog wall was hollow at every layer below the words
Status:   fixed (three layers, each locked by a test)
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-09, building the tidier example: an end-to-end guard test
          written before the app exposed the first layer, and pulling that
          thread exposed two more
Evidence: Layer by layer, worst last:
          1. The default-granted `ui.dialog:*` wildcard silently covered
             file-open, file-save and open-folder at the policy layer -- so
             K-076's "explicit ask" promotion changed the wall's words and
             changed nothing real: every app had the privileged dialogs
             without declaring anything.
          2. `picked/...` paths were checked as fs.* capabilities, so the
             pick-is-the-grant design would have required the very fs scopes
             it exists to remove (caught only because the guard was tested
             instead of just the adapter).
          3. UiDialogResource existed and was referenced by NO host code:
             message, confirm, open-file and open-folder ran with no
             capability check at all. message/confirm would also block a
             headless run forever on a dialog nobody can dismiss.
Fix:      Defaults now grant exactly the harmless pair (message, confirm);
          the wildcard is an explicit ask meaning "all dialogs" and walls as
          one. `picked/` paths ride the ui.dialog:open-folder grant -- the
          pick converts dialog authority into scoped fs authority, and fs
          scopes can NOT substitute (locked both directions). All four
          dialog host functions check their capability before revealing
          anything, and all four are headless-safe (file/folder cancel,
          message no-ops, confirm answers no -- its own dismissed-means-no
          rule). Policy, guard and host layers each carry their own test.

### K-085 -- publish died on a cache write when the KV quota was spent
Status:   fixed and deployed (hub); CLI note-surfacing rides the next release
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-09, Yashraj's TUI "Share it" -- HTTP 500 mid-campaign-prep
Evidence: A temporary debug deploy captured the truth in one reproduction:

            DBG Error: KV put() limit exceeded for the day.
                at verifyGitHub (index.js:314)

          The GitHub identity CACHE write threw on the exhausted quota and
          took the whole publish down -- an optimization killing the
          product, K-082's disease in a second spot. The gallery metadata
          put had the same fragility one screen later.
Fix:      Deployed: the identity cache write is best-effort (a miss costs
          one GitHub round trip); the gallery metadata write degrades
          instead of dying -- the bundle is already safe in R2, the URL
          works, and the response carries a note naming the deferred
          listing. The CLI now surfaces that note instead of swallowing it
          into a clean-looking success. Verified end to end on the live
          hub: publish returns the URL, run-by-URL paints the app, and the
          degraded case prints its note.

          The rule, now twice-earned: no optimization and no side-write may
          sit between a person and the thing they asked for.

### K-084 -- a finished app was thrown away because the AI did not exit fast enough
Status:   fixed in main; rides the next release
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-10, Yashraj's own TUI run, minutes before the outreach push
Evidence: The error convicts itself, verbatim:

            error: the AI agent did not finish within 15 minutes and was stopped.
            The last check-app run actually passed -- re-running the command
            should finish the packaging.

          The stall path ran check_app_verdict, learned the app was DONE,
          printed that fact, and errored anyway. And the retry the message
          recommends could not resume in the TUI: author_app_for_tui handed
          create a fresh tempdir every attempt (work_dir None / a per-attempt
          staging dir), so "resumes from the code already written" -- true in
          the CLI since K-078 -- was structurally false at the front door.
          Fifteen minutes of written, PASSING code abandoned per attempt.
Fix:      Two halves. At the deadline, if the oracle passes, the run is
          salvaged: the agent is stopped and the pipeline continues to
          packaging, with one note line -- an app that passes every check is
          done regardless of whether its author said goodbye. And the TUI
          now builds in a stable workspace (~/.krate/builds/<name>), so a
          retry genuinely resumes and attachments survive attempts; the
          workspace is cleared per-app on success (target/ is hundreds of
          MB), and only when the name is derivable -- a guessed name might
          delete a different app's resumable state.

### K-083 -- the dialog host and the fs host each had their own token registry
Status:   fixed (mutation-locked)
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-10, tracing K-075's fix through the file-token lane it was
          meant to reuse
Evidence: phase2_host.rs:71 and phase3_gui_host.rs:245 each built their own
          ChosenFiles with Default::default(), and nothing wired them
          together. The picker issued tokens into one map; fs.open-chosen
          resolved against the other, which was always empty -- so every
          dialog-picked file in a GUI app has answered NotFound since the
          feature existed. The comment on the picker said "the picker writes
          here and fs.open-chosen reads": the sharing it described did not
          exist. Nothing noticed because the dialogs were default-granted
          decoration until two days ago, and no test walked pick-then-open.
Fix:      One registry, created in HostState::new and handed to both hosts
          (and the fs adapter) by Rc. The regression lock builds the state
          the way production does and proves a picker-issued token resolves
          in the fs host's registry; commenting out the wiring makes it fail
          with "one registry, both hosts", verified by mutation.

### K-082 -- telemetry shared a KV budget with publishing, and spent it
Status:   fixed in main; NEEDS DEPLOY (cloud/deploy.sh with the Cloudflare token)
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-10, Cloudflare email: "KV requests are temporarily blocked",
          free tier's 1,000 puts/day exceeded, reset 00:00 UTC
Evidence: cloud/worker/src/index.js wrote TWO KV puts per usage ping (a
          seen:day:id marker plus a read-modify-write count: bump), into the
          same APPS namespace that holds publish metadata and the auth
          identity cache. 1,000 puts / 2 = 500 pings a day, and one busy day
          -- CI replay matrices, cold-install walks, release verification,
          plus ordinary development -- crossed it. Until the daily reset,
          every `krate publish` and GitHub sign-in gets a 429: counting took
          the product down. The client is unharmed by design (usage posts are
          detached with a short timeout), so only the hub-side operations
          broke.
Fix:      Telemetry moved to a Cloudflare Analytics Engine dataset
          (krate_usage): one writeDataPoint per ping, no KV involvement,
          uniques via the index, unmetered at this scale. KV keeps only what
          it should have held all along -- apps and auth. The stats endpoint
          keeps reading the 90 days of legacy KV history until its TTLs
          retire it, and names the cutover date. CI workflows now set
          KRATE_NO_USAGE=1: a replay matrix is not a user, for the metrics
          as much as the quota.

          The principle worth keeping: counting must never sit on the same
          budget as the thing being counted.

### K-081 -- telemetry is on by default, against the product's own principle
Status:   fixed in main -- first interactive run asks [Y/n] before the first
          count; `n` writes the same marker `krate telemetry off` writes;
          non-interactive runs are never prompted
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-09, Denis's second review, finding 7
Evidence: crates/cli/src/usage.rs:18 -- "It is opt-out and says so." One line on
          first run, but for a tool whose pitch is asking before taking, taking
          first and explaining is the wrong order.
Fix:      Ask once on first interactive run, remember the answer. Headless and
          CI stay silent and off.

### K-080 -- doctor checks that tools exist, not that they fit
Status:   fixed in main -- doctor prints the toolchain version beside the
          path, compares against the workspace minimum (compiled in), and
          says `rustup update` when older; version parsing is tested
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-09, Denis's second review, finding 6: pinned Rust 1.91.1,
          his 1.85.1, doctor printed the rustup path and no version
Evidence: No doctor line compares the installed toolchain against the
          workspace's rust-version (1.91); the printed fix list never includes
          `rustup update`.
Fix:      Print the toolchain version beside the path, compare against the
          minimum, and say `rustup update` when it is older.

### K-079 -- port never checks the one wall that blocks most ports: std
Status:   fixed in main -- every Cargo.toml's direct dependencies are
          collected; known-std crates raise a Blocker naming them, everything
          unverified raises a Change saying to check no_std support first.
          Denis's exact shape (lopdf) is the regression test
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-09, Denis's second review, finding 3: his PDF tool got
          "needs changes, one finding" when the true answer is "cannot port:
          the PDF crate needs std"
Evidence: crates/cli/src/port_report.rs mentions no_std exactly once, as an
          aside. No per-dependency check exists. Same shape as the doctor bug
          fixed last review: confident advice about a wall never checked for.
Fix:      For every dependency in the analyzed Cargo.toml, say plainly that it
          must work without std, mark the ones not known to, and lead the
          report with that when any are unverified. Also say prominently in
          the docs that diceroll works because rand supports no_std -- most
          document/PDF/image crates do not.

### K-078 -- create deletes the evidence its own error points at
Status:   fixed in main -- a Drop-armed keeper persists the temp workspace on
          any failure and prints where; a workspace holding a previous
          attempt's src/lib.rs is no longer wiped; the skeleton writes only
          missing files, so "resumes from the code already written" is now
          true instead of promised
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-09, Denis's second review, finding 5
Evidence: Two mechanisms, either sufficient:
          - crates/cli/src/main.rs:3461: `let _ = fs::remove_dir_all(&app_dir)`
            at the START of every create run, so a retry wipes the previous
            attempt before "resuming" -- while the failure text promises the
            run will resume from the code already written.
          - The no-output-dir branch holds the workspace in a tempdir whose
            guard drops when create returns, so the .agent-transcript.txt path
            printed in the failure message is deleted before the person can
            open it.
Fix:      On failure, persist the workspace (tempdir into_path) and print the
          real path; stop wiping a previous attempt that the error told the
          person to inspect, or stop promising resume.

### K-077 -- the authoring pack teaches every AI to build a GUI, whatever was asked
Status:   fixed in main -- the pack now says to keep the starter's world,
          names both worlds, and warns that a CLI request built as a window
          app fails the checks over and over
Owner:    lead
Severity: serious
Class:    teaching-hole
Found:    2026-08-09, Denis's second review, finding 4: asked for a
          command-line tidier, got a window app, 18 rounds and 15 minutes lost
Evidence: The world inference exists and is right -- "command-line" is in
          wants_gui's CLI signals (crates/author/src/lib.rs:58) and the
          skeleton is written with the inferred world (main.rs:4757). But the
          pack's only manifest example hardcodes
          `world = "krate:app/gui@0.2.0"` (authoring_context.rs:843), so the
          AI "corrects" the CLI skeleton back to GUI by copying the example.
          Then the checks fight the mismatch for the rest of the run.
Fix:      The pack shows both worlds, says to keep the skeleton's choice, and
          KRATE_APP_KIND is stated in the pack rather than only in the env.

### K-076 -- the permission list describes capabilities that do not work, in stale words
Status:   fixed in main -- file-open/file-save promoted to explicit asks (the
          stale comment's own stated condition: rfd serves all three hosts),
          both rotten comments rewritten to today's truth, and the port
          analyzer taught to detect dialog use so the promotion is coherent
          end to end (its own cross-check test caught that gap)
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-09, Denis's second review, finding 2, quoting our own comment
Evidence: crates/manifest/src/lib.rs:96 still says canvas2d "refuses every
          call" -- false since v0.1.0-rc5; the bounce game ships on it. Lines
          88-92 keep dialogs default-granted "until dialogs land on all three
          systems" -- but choose_file_on_host is implemented through rfd
          (phase3_gui_host.rs:3026), which serves macOS, Windows and Linux.
          The comments froze while the code moved, and the wall inherits the
          confusion: a reader cannot tell which lines are real.
Fix:      Promote ui.dialog file-open/file-save to explicit asks now that they
          are implemented (the comment's own stated condition), correct the
          stale comments, and never let the wall name a capability the host
          cannot honor without saying so.

### K-075 -- a "tidy my folder" app is impossible, so the generator reaches for **
Status:   fixed in main, ships in v0.1.8 -- all three steps done, worked
          example in apps/krate-tidy; only Denis's grant-lifetime answer
          remains, tracked below.
          Step 1: ui.dialog:open-folder exists end to end.
          The pick is the grant: the app gets a token, reaches the subtree
          through picked/<token>/... on the ordinary fs calls, and the mount
          runs through the same resolver choke point as the sandbox, so
          symlink refusal and containment come free (security tests pin the
          forged token, the traversal, the symlink escape, and non-aliasing
          into the sandbox). Grant dies with the run -- revoke-on-exit chosen
          as the safer boundary; Denis may argue for persist-per-app and the
          registry can grow that later. Headless runs auto-cancel every
          dialog so CI can never hang. Step 2 SHIPPED in main: check-app
          refuses an unscoped fs glob (naming the open-folder path as the
          fix) and any capability whose interface the component never
          imports -- both rules tested against the reviewed tidier's exact
          shapes. The pack teaches the picker pattern and the motion module
          in a new design-patterns section; the fleet was swept (no wide
          scopes existed). Step 3 SHIPPED: apps/krate-tidy is the worked
          tidier -- manifest with zero fs capabilities (ui.dialog:open-folder
          is the only privileged line), list/stat/mkdir/rename all under
          picked/<token>/..., all six check-app stages green, quick mode
          proves the classifier with no dialog (planned:6). The app Denis
          proved impossible now exists on honest rails. Remaining: Denis's
          answer on grant lifetime.
Owner:    lead
Severity: blocker
Class:    runtime-hole
Found:    2026-08-09, Denis's second review, finding 1 -- the deepest one
Evidence: Four connected facts, each verified:
          1. Every fs path resolves inside the app's sandbox root with
             symlink and containment checks (runtime/src/lib.rs:859-877), so
             `fs.list:**` reaches the app's own folder and nothing else.
             Denis's "asks for my whole disk" is factually wrong -- but it is
             the only reading the wall's words allow, because no fs line says
             the boundary ("see what is in a folder you choose", tui.rs:1277).
             An app that scares reviewers while being safe is still a failed
             wall.
          2. Host folders cannot be named by design (absolute paths refused),
             and there is no folder dialog -- only file-open. So a folder
             tidier has no correct manifest at all; ** is merely the one that
             LOOKS like it might work. The generator is not careless; the
             runtime has a hole.
          3. Grants cannot narrow: manifest fs.list:** is satisfied only by a
             grant at least as wide, so the person's only choices are
             everything or nothing.
          4. The tidier's manifest asks fs.remove:** and the code never calls
             remove -- check-app proves imports stay inside krate:* but never
             that the manifest asks no more than the code uses.
Fix:      In Denis's order, which is right: (1) ship ui.dialog:open-folder --
          a person picks a real host folder and the pick IS a scoped grant,
          same trust shape as file-open's token; (2) then check-app rejects an
          unscoped fs.*:** and a capability the component's imports never
          justify, the same way it rejects a stray wasi:* import. Doing (2)
          first would only make the generator fail instead of overreach.
          The wording half is in main: every fs line on the wall now names
          the boundary ("inside its own private folder -- never your files"),
          locked by tests including `fs.list:**` specifically. The dialog and
          the check-app tightening remain, in that order.

### K-074 -- every Windows zip has used backslash separators, which the spec forbids
Status:   fixed, shipped and verified in v0.1.4
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-08, by K-073's new release-verification gate on its first run
Evidence: The published v0.1.3 archive, read with a spec-compliant tool:

            $ python3 -c "import zipfile; print(zipfile.ZipFile(
                'krate-0.1.3-x86_64-pc-windows-msvc.zip').namelist()[:1])"
            ['krate-0.1.3-x86_64-pc-windows-msvc\\cargo-component.exe']

            $ unzip krate-0.1.3-x86_64-pc-windows-msvc.zip
            warning: appears to use backslashes as path separators

          ZIP requires forward slashes (APPNOTE 4.4.17). scripts/package.sh
          prefers `zip`, but the Windows runner has none, so it falls back to
          PowerShell's Compress-Archive -- which writes backslashes. Windows
          Explorer copes, which is why this shipped unnoticed in every Windows
          release. Nothing else does: extracting with Python, WSL, Git Bash or
          CI produces seven files with literal backslashes in their names and
          no folder at all.
Fix:      package.sh now builds the archive through .NET's ZipArchive, naming
          each entry explicitly with forward slashes, and then refuses to ship
          any zip whose entries contain a backslash. The release gate checks
          the same property on the published asset, so the two are independent.

          Verified by running the new guard against the real published v0.1.3
          zip: it exits 1 and names the offending entries. Then verified on
          the published v0.1.4 asset -- entries use forward slashes, Python's
          zipfile creates a real folder, and unzip exits 0 with no warning
          where the same asset one release earlier produced a backslash mess.

          One trap the gate hit on itself, worth keeping: unzip exits 1 on the
          backslash *warning* even when extraction fully succeeded, so with
          set -e the verification step died and hid every check after it --
          including the one meant to report this defect by name. A gate that
          fails for the wrong reason is a gate that teaches you nothing.

### K-073 -- nothing verified the published release, so three defects shipped
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-08, reading the board as data rather than as a list
Evidence: Counting how each entry was discovered:

            found by a person on a machine:  14
            found by CI before shipping:      0

          1041 tests, and not one has ever caught a shipping bug. The reason
          is structural, not lazy: everything verified the build, and nothing
          verified the download. Three defects reached users through that gap
          and were each found by hand afterwards --

            K-058  the Windows zip shipped with no document icon
            K-063  Krate.app signed without allow-jit, so every double-click
                   was killed by the kernel; passed signing, notarization and
                   spctl on the way out
            K-068  the tarball was packed before signing ran, so the signed
                   binary was never the one shipped

          All three are checkable in seconds against the published asset, and
          CI never looked at a published asset. The cold-install walk, the one
          gate that behaves like a person, ran only on a nightly schedule or a
          hand-typed [full-ci] tag -- never on a release.
Fix:      A `verify` job in release.yml, needs: publish, on macOS/Linux/Windows.
          It downloads what the public downloads and behaves like somebody who
          just got it: checksum, unpack, `--version` must match the tag (content,
          never timestamps), cargo-component present, the Windows icon and
          console-less opener present, macOS signed non-adhoc with allow-jit on
          both the CLI and the bundle plus spctl, then builds a real app with
          the published binary and confirms the permission wall still refuses
          without grants. A bad release is now loud before anyone is told to
          install it.

          Also closed the regression gap behind K-069 and K-070: the install
          loop's termination rule is now a pure function with a test (a pass
          that changes nothing is not progress -- the exact shape of the
          three-restart bug), and the alternate-screen contract is a tested
          property (enter must be undone, restore must live in Drop). Both
          tests were mutation-checked: reintroducing each bug makes them fail.

### K-072 -- the Windows install command fails in Command Prompt, and nothing says so
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-08, a friend's Windows PC, first command they ever typed
Evidence: 'irm' is not recognized as an internal or external command,
          operable program or batch file.

          `irm` is a PowerShell alias for Invoke-RestMethod; in cmd.exe it
          does not exist. The site published it under a tab labelled only
          "Windows" in four places, and auto-selects that tab for every
          Windows visitor -- so a first-time user in Command Prompt was
          *handed* a command that cannot work, with nothing naming the shell.
          This is the first command a Windows user ever runs, so the failure
          rate here is the adoption rate.
Fix:      The toggle now reads "Windows (PowerShell)", and a cmd.exe line
          appears with it: run `powershell` first, then paste. README says
          the same.

          Considered and rejected: publishing
          `powershell -NoProfile -Command "irm ... | iex"`, which works
          verbatim in both shells. It runs the installer in a CHILD process,
          so the session PATH patch dies with it and the current window
          cannot see `krate` afterwards -- reintroducing exactly the
          "open a new terminal" friction K-069 removed. Naming the shell
          keeps the zero-restart property.

### K-071 -- whisper-rs stopped linking on the windows-latest CI image
Status:   open
Owner:    unclaimed
Severity: serious
Class:    environment
Found:    2026-08-07, CI run 31162680943, on a commit touching only a workflow
          file and BUGS.md -- so the code did not cause it
Evidence: Library tests (windows-latest) and Test (ubuntu? no -- windows only):

            libwhisper_rs_sys-...rlib(ops.obj) : error LNK2019: unresolved
              external symbol __imp_fminf ...
            fatal error LNK1120: 20 unresolved externals

          Twenty unresolved CRT math symbols (__imp_fmaxf, __imp_erff,
          __imp_lroundf...) linking krate-runtime's test binary against
          whisper-rs/ggml. The same code linked fine in earlier runs; the
          runner image updated (MSBuild/MSVC), so this is the machine
          changing under us, not a regression in Krate. Every push now fails
          the Windows library-tests job until it is addressed (ggml needs
          the UCRT import lib the new image stopped providing implicitly, or
          whisper-rs needs a version bump built against it).
Fix:      Mitigated 2026-08-07: release and runtime-linking CI jobs pinned
          to windows-2022, which still links ggml correctly; v0.1.3 built
          green on it. The real fix (whisper-rs/ggml against the new UCRT
          arrangement, or an upstream bump) is still open before the pin can
          come off -- windows-2022 will be retired eventually.
Update:   2026-08-12. Still failing, now on the pinned windows-2022 image
          too, with the same class of symbol (`__imp_fgetc`, `__imp_fputs`,
          `__imp_fgetpos` -- stdio rather than math this time). CI run
          31528953170: **10 of 11 jobs green, this is the only red one.**

          Why releases keep shipping anyway, which was not written down and
          should have been: the release workflow builds Windows with
          `no-speech: true`, so it never links whisper at all. Only this CI
          job builds the full feature set. That is the whole reason v0.1.12
          published six platforms green while this stayed red.

          The cost of leaving it: main has been red for five days, and a
          permanently red board stops being read. The next failure that is a
          real defect will look exactly like this one. That is the argument
          for fixing it, not the speech feature itself.

### K-070 -- typing a request with no AI connected throws the request away
Status:   fixed, shipped in v0.1.3
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, first run on a stranger's Windows PC, next to Grok's CLI
Evidence: crates/cli/src/tui.rs:766 -- choose_provider() returns Ok(None) when
          nothing probes as working, make_named_app_with unwinds, and the
          sentence the person just typed is gone. They install an AI, come
          back, and type it again. For a first-time user this is the worst
          moment in the product, and it sits directly on the first thing they
          try.
Fix:      The request is held. The no-AI gate shows what to install, waits on
          Enter, re-reads PATH (see K-069), re-probes, and starts the build
          with the held request the moment a provider works. Backing out says
          the request is one up-arrow away (it is in prompt history).

### K-069 -- first-run setup on Windows demanded three terminal restarts
Status:   fixed, shipped in v0.1.3. The trimmed installers ARE live (they deploy from the
          site, not from a release).
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, first run on a stranger's Windows PC
Evidence: install_build_tools (crates/cli/src/main.rs:7817 before the fix)
          computed the missing-tool list once and ran the installs in order.
          The list is a dependency chain -- winget installs rustup, the next
          command runs `rustup`, the next needs `cargo` -- and each command
          was executed by bare name against the PATH this process captured at
          startup. Installs write PATH to the registry, which running
          processes never re-read, so step N+1 could not see step N's work,
          failed, and tui.rs:670 said "open a new terminal and try again".
          Three dependent steps, three restarts. K-066 fixed one symptom
          (rustup lookups); this is the mechanism behind the whole class.
Fix:      refresh_process_path() re-reads HKCU/HKLM PATH from the registry
          (reg.exe, no elevation) and merges it into the process -- what a
          new terminal does, minus the terminal. Called between install
          steps, and before every toolchain and AI probe. The missing list is
          recomputed after every step. The "open a new terminal" advice is
          deleted because the condition it described can no longer occur.
          Plan/krate-front-door-2026-08.md holds the full trace.

### K-068 -- the CLI tarball has never contained the binary the workflow signs
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, verifying the published v0.1.2 assets
Evidence: The workflow signs the bare CLI in the "Sign and notarize" step, but
          Package -- which builds the tarball -- runs before it. The signed
          binary in target/ is never read again. The published asset proves it:

            $ codesign -dvv krate-0.1.2-aarch64-apple-darwin/krate
            flags=0x20002(adhoc,linker-signed)

          Two defects hid each other here. The signing step also passed
          `--options runtime` with no entitlements -- the K-063 combination the
          kernel kills -- so if the signed binary HAD reached the tarball,
          `krate run` would have died on every launch. Shipping the unsigned
          one is why it worked. First filed as "signed hardened with no
          entitlements, shipped in v0.1.2"; both halves of that were wrong,
          which is what verifying the published asset rather than the workflow
          diff is for.
Fix:      Entitlements on the codesign call, and the tarball repacked after
          signing so it carries what was signed. Shipped and verified in
          v0.1.3: the published tarball's krate is hardened, Developer-ID
          signed, carries allow-jit, and ran a component to a painted frame.

### K-067 -- a window is created at the wrong size on any display that is not 100%
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, on a friend's Windows PC, running krate-nova
Evidence: The window is created with a *logical* size while every other size in
          the adapter is *physical*:

            crates/adapter-windows/src/winit_native.rs:138  with_inner_size(LogicalSize::new(..))
            crates/adapter-windows/src/winit_native.rs:327  tracked.window.inner_size()   // physical
            crates/adapter-windows/src/winit_native.rs:466  tracked.window.inner_size()   // physical

          At 150% scaling an app asking for 800x600 gets a 1200x900 window and
          paints 800x600 of it. The remaining band stays blank and the picture
          looks squashed into a corner -- it reads as "the graphics are worse on
          Windows" but no drawing code is involved. Invisible at 100%, and
          nearly invisible on a 2x Retina Mac, which is why it survived.
          adapter-linux carried the identical defect at line 135.
Fix:      Create with PhysicalSize so the requested size means what every other
          call site already means. Both adapters. NOT yet compile-checked on a
          real Windows/Linux host -- the module is behind
          #[cfg(target_os = "windows")] so a macOS `cargo check` skips it.

### K-066 -- a just-installed toolchain is invisible, so the linker check never passes
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, on a friend's Windows PC, first run
Evidence: Two tabs of the same session disagreed: one reported cargo ready, the
          other demanded "install a linker for Windows". has_tool() resolves via
          resolve_tool(), which falls back to ~/.cargo/bin; the rustup checks
          called the binary bare and consulted PATH only:

            crates/cli/src/main.rs:7824  ProcessCommand::new("rustup")   // gnullvm_toolchain_present
            crates/cli/src/main.rs:7746  ProcessCommand::new("rustup")   // has_rust_target
            crates/cli/src/main.rs:5089  ProcessCommand::new("rustup")   // on the build path

          rustup is installed by our own toolchain step moments earlier, and a
          running process does not inherit the PATH that install wrote. So the
          check fails, and the retry runs in the same stale process and fails
          again. The only escape was to quit and rerun the installer, which is
          what the person actually did.
Fix:      Route every rustup invocation through resolve_tool(), same as cargo.

### K-065 -- every TUI failure hides its own cause
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, on a friend's Windows PC, "Make an app" with grok
Evidence: The whole failure shown to the person was the string `run --author-cmd`.
          anyhow's Display prints only the outermost context; six call sites used
          it:

            $ cargo test -p krate-cli --bins error_display_keeps_the_cause
            DISPLAY:   run --author-cmd
            ALTERNATE: run --author-cmd: program not found

          Separately, when the author command failed having written nothing we
          captured, the message was the bare "author command failed" -- which
          sends somebody to debug their request when the real problem is that
          the AI tool is not installed (a shell exits 127 for that).
Fix:      Print `{err:#}` at all six sites, and name exit 127/126 as a missing
          or non-executable tool rather than a failed build.

### K-064 -- "needs a newer version of Krate" is a guess, and usually the wrong one
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, macOS, opening an app from Downloads through the menu
Evidence: Opening a .krate built on 4 August with a runtime built today:

              error: this app needs a newer version of Krate than you have
              installed. Update Krate and try again:
                curl -fsSL https://krate.tech/install.sh | sh

          Exactly backwards. The app is older, not newer; the interface grew
          under it (`krate:ui/events@0.1.0` gained events, and canvas2d has
          gone from 7 functions to 26 since). Updating Krate cannot fix it,
          so the one instruction given was the one that does not work.

          K-035 recorded this same backwards message on 5 August and fixed the
          bundles rather than the wording, so it was still there today.
Impact:   Somebody follows the instruction, reinstalls, sees the identical
          error, and concludes the product is broken. It also hides the real
          fix, which is to rebuild the app.
Fix:      Stop guessing which side is older. The message now names the actual
          condition -- the app and this Krate were built against different
          versions of the interface -- and gives both moves: update Krate if
          the app arrived recently, or rebuild it through "Make a change" if
          it is one you made a while ago. The wasmtime line is appended as
          Details, so the missing interface is visible rather than swallowed.


### K-063 -- Double-clicking a .krate on macOS killed the app instantly
Status:   fixed (shipped v0.1.2; verified on the published bundle: entitlements
          present, LaunchServices launch survives, app draws)
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, macOS 27, double-clicking an app in Finder
Evidence: Nothing happened. No window, no error, no message -- and a fresh
          crash report every time:

              signal: SIGKILL (Code Signature Invalid)
              termination: CODESIGNING / Invalid Page

          The bundle is signed and notarized and passes every check:

              spctl -a -vv Krate.app
              accepted   source=Notarized Developer ID

          But it carries NO entitlements:

              codesign -d --entitlements - .../krate-cli
              (empty)

          Hardened runtime (`--options runtime`, required for notarization)
          forbids writable-executable memory. Wasmtime JIT-compiles every
          component it runs, so the kernel kills the process the moment an app
          is opened. Silently, because a double-clicked app has no terminal.

          Proven on one machine, same binary, same app:
            unentitled  -- crash reports 6 -> 7, nothing rendered
            entitled    -- crash reports 6 -> 6, app ran and drew a frame
Impact:   Double-clicking a .krate has never worked on a signed macOS build.
          It is the headline promise -- "somebody sends you an app and you
          double-click it" -- and every notarized release has shipped with it
          broken. It survived because every check we had passes: the signature
          is valid, notarization succeeds, spctl accepts it. Only running it
          fails.
Fix:      scripts/krate.entitlements, with allow-jit and
          allow-unsigned-executable-memory, passed to codesign for the inner
          binary and the bundle.

          Note for anyone editing that file: AMFI rejects XML comments inside
          the dict. Two signing attempts failed with "AMFIUnserializeXML:
          syntax error" before the comments came out.

          Found alongside a second problem on this machine: three Krate.app
          copies were registered as .krate handlers, two of them unsigned
          (~/Applications from an old install, and one in /private/tmp).
          macOS picked among them unpredictably. Removed, and the surviving
          one re-registered.


### K-061 -- A generated app closed itself after thirty seconds, mid-read
Status:   fixed
Owner:    lead
Severity: blocker
Class:    example-bug
Found:    2026-08-07, Windows 11, reading a news app made with the TUI
Evidence: The app bounded its interactive loop:

              const MAX_ROUNDS: u32 = 600;      // 50ms a round
              let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };

          600 x 50ms is thirty seconds, and then the window vanishes while
          somebody is still reading the news.

          The authoring pack already forbids this, at length, in its own
          section headed "Do not close yourself. This is the most common way a
          generated app fails." The AI wrote it anyway.

          check-app cannot catch it: the usability driver watches for five
          seconds (HEADLESS_RUN_BUDGET), records the app as having stayed
          open, then closes it itself. Any bound longer than five seconds
          passes the check and still quits on the person.
Impact:   The commonest way a finished, correct-looking app fails its user,
          and nothing in six stages saw it.
Fix:      A layout-stage check that reads the source and fails when the
          interactive branch of a `if quick { A } else { B }` bound is a
          finite round count. It runs in 0.1s, before any build.

          Deliberately narrow, verified against eleven shipped apps with zero
          false positives:
            - a bound only reachable when quick (`while !quick || frames < n`)
              is the correct shape and passes -- krate-paint
            - a bound measured in hours is a runaway backstop, not a session
              limit -- krate-notes at 600_000 rounds is 8.3 hours
            - a frame-paced game with no wait constant is paced by the
              runtime's 60fps present -- krate-nova's 100_000 frames is 28
              minutes
          The bar is five minutes: below it somebody reaches the bound while
          using the app, above it they do not.

          Teaching was already there and was not enough. This is the
          difference between asking and checking.

### K-062 -- Double-clicking a .krate flashed a console and opened nothing
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, Windows 11, double-clicking an app in Explorer
Evidence: The registered handler on that machine:

              "…\AppData\Local\Krate\bin\krate.exe" "%1"

          No `run` subcommand. Running it by hand reproduces what the person
          sees for an instant:

              error: unrecognized subcommand '…\make-a-newspaper-app.krate'
              Usage: krate.exe [COMMAND]

          The console appears, prints that, and closes -- too fast to read.

          An older release registered the association this way. The current
          installer registers krate-open.exe correctly, but machines carrying
          the old association are already out there and no new installer can
          reach the association a previous one wrote.
Impact:   "Somebody sends you an app and you double-click it" is the whole
          pitch, and it did nothing at all with no explanation.
Fix:      `krate <file>.krate` now means `krate run <file>.krate`. The binary
          understands what it was asked for regardless of what the association
          says, so every machine with the old registration is fixed by the
          next update rather than needing the association repaired.


### K-059 -- Opening an app from the menu never said what it could reach
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, user question: "we say our app asks for permission on
          what they'll be accessing -- then any app I made, opening it in the
          TUI by clicking open it now?"
Evidence: The whole of it was:

              opening i-want-roadrash-like.krate
              close its window, or press Ctrl-C, to come back here

          `run_bundle_for_tui` passes `--auto-grant`, so every capability in
          the manifest is granted with no prompt and no listing.

          The grant itself is defensible: it is the person's own app, and a
          yes/no per capability is friction with no decision behind it. Saying
          nothing is a different thing. "An app tells you what it wants before
          it starts" is the sentence on the front page, and the front door was
          the one place that did not.
Impact:   The capability wall is the product's whole differentiator and it was
          invisible at the moment somebody was actually using it.
Fix:      The manifest is read and stated in plain words before the app runs:

              ✓ this app can draw its window and nothing else

          or, for one that asks for more:

              this app can:
                ✓ save files in a folder called notes
                ✗ nothing else -- not your files, not the network

          Deliberately not the developer wording from the authoring pack --
          that says "write files under a folder", which is right for somebody
          writing an app and wrong for somebody deciding whether to open one.
          Path globs are turned into folder names for the same reason:
          `./notes/**` is exact and unreadable.

          Capabilities every app gets (stdout, args, the clock, the window)
          are not listed, or the one line that matters is buried under five
          that never vary.

### K-060 -- The runtime's launch line duplicated what the menu just said
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, macOS, opening an app from the menu
Evidence: krate: opened window "Krate Road Rash" (close it or press Ctrl-C to
          quit)

          printed directly under the menu's own "opening ... close its window,
          or press Ctrl-C, to come back here". Two sentences saying the same
          thing, the second one blunter and naming a window title the person
          can already see.

          It goes to stderr, which the front door inherits deliberately -- a
          real error must not be swallowed -- so capturing stdout did not
          silence it.
Fix:      KRATE_QUIET_LAUNCH, set by the front door only. A bare `krate run`
          in a terminal still prints it, which is where it is useful.


### K-058 -- v0.1.0 shipped without the Windows document icon
Status:   fixed (shipped v0.1.1)
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, verifying the published v0.1.0 assets
Evidence: The Windows archive carries krate.exe, krate-open.exe and
          cargo-component.exe, but no KrateDoc.ico -- so a .krate still shows
          the blank-page icon, which is the whole of K-048.

          The release log said so on both Windows targets:

              warning: no KrateDoc.ico; .krate files will show a blank icon

          Cause: the icon generator needs Pillow, and the only `pip install
          pillow` in the workflow lives in the macOS-only Krate.app step --
          which runs AFTER packaging and never on Windows at all.
Impact:   K-048 was fixed in the repository and not in the product. The fix
          was verified by generating an icon locally, which is exactly the
          check that could not catch this.
Fix:      Pillow is installed before the Package step on Windows targets.

          The bigger fix is that package.sh now FAILS instead of warning. A
          warning printed twice in a release log and shipped anyway is not a
          safeguard. Same change for a missing krate-open.exe, which would put
          a console window back beside every double-clicked app.

          Needs the next release to reach anyone. v0.1.0's Windows archive
          keeps the blank icon.


### K-057 -- A two-line prompt reprinted itself once per keystroke
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, macOS, publishing an app from the menu
Evidence: Typing a one-line description produced sixteen copies of the
          question marching down the screen:

              One line about it (or press enter to skip)
              One line about it (or press enter to skip)
              ... x16
              > DVD Screensaver

          The line editor redraws with `\r\x1b[K`, which returns to the start
          of the CURRENT line and clears it -- exactly one line. Two prompts
          embed a newline in their label ("One line about it...\n  > " and
          "Path to the .krate file\n  > "), so every keystroke cleared the
          `> ` line and reprinted the whole label under it.
Impact:   The publish flow, which is the one thing a .krate exists for, looked
          broken at the moment somebody was trying to share their app.
Fix:      The label is printed once when the prompt opens; only its LAST line
          is repainted while typing. Verified through a pty on both prompts:
          the question now appears zero further times as characters arrive.


### K-053 -- A finished edit was thrown away by the permission-wall check
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, macOS, adding a DVD screensaver to a working app
Evidence: The AI made the change, it compiled, it packed, and then:

              ✗ that change did not work -- your app is untouched
              withholding gfx.gpu:basic should refuse with exit 5, got 0

          `gating_capability` picks a required capability to withhold, to
          prove the permission wall refuses without it. It excluded `io.` and
          `ui.window` by prefix and let `gfx.gpu:basic` through -- which the
          runtime grants to EVERY app by default. Withholding it changes
          nothing, so the app ran fine and exited 0, and that correct
          behaviour was read as a failure.
Impact:   Ten minutes of work discarded after everything real had succeeded.
          It hits any app declaring a default-granted capability as required.
Fix:      Ask the registry which capabilities are default-granted instead of
          guessing from prefixes. An app with nothing withholdable now
          correctly has no wall to test rather than a fake one.

          Two more things that made it worse, also fixed: the message said
          "your app is untouched" while the source directory HAD been edited
          (only the bundle was not replaced), and a failed change dropped to
          the top-level menu, so the app looked gone. It now stays with the
          app so trying again is one keypress.

### K-054 -- An app's own output leaked into the menu
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, macOS, closing a screensaver
Evidence: Closing an app left this under the menu:

              screensavers:4
              current:Starfield
              frames:782

          Apps print machine-readable lines for check-app to assert on. The
          TUI ran the app with stdout inherited, so those landed on screen.
Fix:      stdout is captured and kept for the failure message -- an app that
          exits non-zero usually explains itself there, which is exactly when
          the lines are worth showing. stderr stays inherited, since that is
          where a real error goes.

### K-055 -- Changing an app showed raw cargo output; making one did not
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, macOS, comparing the make and change flows in one log
Evidence: A fresh build showed clean named stages. A change showed
          "warning: function `pure_string` is never used" and a full cargo
          dump through the middle of the display.

          The change flow never started a Progress display. With no sink
          installed, `drawing` is false, so the authoring child inherits the
          terminal instead of being piped -- the same mechanism that was fixed
          for the make flow, on a path that never got it.
Fix:      `revise_app_for_tui_watched`, mirroring the make flow. The display
          is what causes the child's output to be captured, so this fixes the
          stage reporting and the noise together.

### K-056 -- Nothing said Grok cannot report progress
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, from the transcript evidence behind K-045
Evidence: Grok writes one JSON object when the whole run ends, so a progress
          display has nothing to show in between. Somebody watching a still
          screen for ten minutes concludes it has hung -- which has already
          happened once, on an eight-minute Windows run that was killed by
          hand while working normally.
Fix:      The picker recommends Claude Code and says why. Choosing a provider
          that cannot stream now prints, before the wait starts: "grok does
          not report progress while it works, so the steps below will not move
          until it finishes. It is not stuck." A `reports_progress` method on
          the provider trait carries it, so a new provider declares its own
          behaviour rather than the menu hardcoding names.


### K-052 -- No stroke-circle, so round things got square outlines
Status:   fixed
Owner:    lead
Severity: serious
Class:    runtime-hole
Found:    2026-08-07, comparing two AIs given the same request
Evidence: Grok's screensaver draws every bubble inside a visible thin square
          box, and the DVD logo inside a hard red rectangle. Its own comment
          says what it wanted:

              // Thin rim.
              canvas2d::stroke_rect(...)

          `gfx.wit` has `fill-circle` and no `stroke-circle`. Told to put a rim
          on a round bubble, the nearest available call was `stroke-rect`, so
          that is what it used. The model did the best it could with what was
          exposed; the gap was ours.
Impact:   Every rim, ring, dial and unfilled dot -- ordinary UI, not an edge
          case. And the failure is silent: the app builds, passes all six
          stages, and just looks wrong, so no check catches it.
Fix:      `canvas2d::stroke-circle(canvas, center, radius, width, stroke)`,
          anti-aliased by coverage the same way fill-circle is, so a thin rim
          reads as a smooth curve. Tested for the three things that make it a
          ring: the edge is drawn, the centre stays empty, and nothing appears
          where a rect's corner would be. The pack now says to use it and not
          to reach for stroke-rect.


### K-051 -- An expired AI sign-in reported nothing useful
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, macOS, generating a test app with an expired login
Evidence: The whole message was:

              error: the claude agent did not finish successfully;
              see .../.agent-transcript.txt
              error: author command failed

          The transcript held the real answer all along:

              "Failed to authenticate: OAuth session expired and could
               not be refreshed"

          `agent_failure_reason` looked in `/error/message`, `/item/message`
          and `message`. Claude Code puts the sentence in `result` on its final
          event, flagged with `is_error: true` and typed "result", not "error".
          So the one useful line was skipped every time.

          Worse, on failure the temp work directory is cleaned up -- so the
          error named a transcript that had already been deleted.
Impact:   The commonest failure of all (a sign-in that quietly lapsed) looked
          like Krate being broken, and the file it pointed at was gone.
Fix:      Read `result` and honour `is_error`. An expired sign-in also gets
          "Sign in again, then try once more" appended, since the provider's
          own wording never says what to do about it.

          Verified against the real transcript from the failed run rather than
          a constructed one.


### K-050 -- Authoring waits on check-app, not on the AI reading or thinking
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, measured on macOS after a six-minute Windows edit
Evidence: Measured rather than assumed, in this order:

            incremental app compile        0.7s
            clean app compile, 6 crates    1.7s
            check-app --no-run             2.0s   (4 stages)
            check-app full                17.1s   (6 stages)

          The AI's own transcript, for a one-line change:

              "The check-app is taking a long time - probably building.
               Let me wait for it."

          and it ran check-app five times. Five times seventeen seconds is
          most of the six minutes, plus a model round trip between each.

          The 43 KB context pack was the suspected cause and is not: the whole
          run held only 3,354 characters of reasoning. Reading is seconds.

          The seventeen seconds is not the build. It is the run stage and the
          usability stage, which each start the app under a five-second
          headless budget, then resize its window and click it.
Impact:   Every app and every edit pays it. It is also why an edit costs about
          as much as a first build, which reads as the tool being slow at the
          one thing that should be quick.
Fix:      `--no-run` already existed and stops after imports. Both prompts now
          teach the loop: iterate with `check-app . --no-run` (2s), prove once
          with the full `check-app .` (17s).

          Verified that --no-run still catches what matters -- it returns the
          build failure and the wasi-import failure before the no_run branch,
          confirmed by reading the code path and by breaking an app on
          purpose (compile error caught in 0.1s).

          Quality cannot regress: `create` runs its own full check-app after
          the agent finishes (`check_app_verdict`, no_run: false) and refuses
          to package an app that fails. The fast loop is the AI's inner loop;
          the gate is still ours.


### K-048 -- .krate files have no icon on Windows, and could not have one
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, Windows 11, looking at a .krate in Explorer
Evidence: Every .krate shows the generic blank-page icon.

          The association script looks for `dist\icon\KrateDoc.ico`. No .ico
          exists anywhere in the repository, so `$icon` stayed empty and the
          DefaultIcon key was never written.

          The generator that draws the icons only emitted .icns, and it called
          iconutil unconditionally -- a macOS-only tool. So the Windows icon
          could not be produced on the Windows runner even in principle.
Impact:   A .krate is meant to look like a thing you double-click. A blank page
          icon says "unknown file type", which is the opposite.
Fix:      The generator writes .ico with all seven sizes Explorer picks from,
          before the .icns and unconditionally; iconutil is now skipped rather
          than assumed. package.sh puts KrateDoc.ico in the Windows archives
          and the association script looks beside the binary first, which is
          where a real install has it.

### K-049 -- "2-5 minutes" was wrong; it is 5-12
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, Windows 11, timing real runs
Evidence: The menu promised "2-5 minutes". Measured runs took 5-12, and a
          small change to an existing app took six -- against a line promising
          it would be "usually quicker than the first build".
Impact:   Worse than a cosmetic error. Somebody told five minutes, watching a
          silent screen at minute nine, concludes it has hung and kills it --
          which is exactly what happened on an earlier eight-minute run.
Fix:      The menu says 5-12 minutes. The change flow says it is still a few
          minutes and explains why: the compile is incremental but the AI
          still reads the whole app to find where the edit goes, and that is
          the long part. Also corrected in the MCP docs and source comments.


### K-045 -- The progress display shows nothing for AIs that do not stream
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, Windows 11, making an app with Grok
Evidence: "working out what to build" for the whole run, then straight to
          completed. No intermediate progress at all.

          The display was driven entirely by parsing the AI's streamed output.
          Grok does not stream: its transcript is ONE JSON object written when
          the run finishes, holding the whole session. So there is nothing to
          parse until it is over, and nothing to show. Claude streams per-tool
          events, which is why this looked fixed on macOS.
Impact:   Reads as a hang for minutes. In the reporter's words, "that's where
          almost everyone will close the process because they'll think its
          stuck" -- which is exactly what happened on an earlier eight-minute
          Windows run.
Fix:      Drive the display from our own pipeline, which knows the real phases
          regardless of the AI: building, verifying, packaging each report as
          they start. The authoring phase now also says plainly that some AI
          tools report nothing until they finish, so silence is explained
          rather than mysterious. Parsed agent output still refines the detail
          line when a provider does stream.

### K-046 -- An app named "krate" collides with the SDK and cannot build
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, Windows 11, reading a Grok transcript
Evidence: The app package takes its name from the request; the SDK dependency
          is always named `krate`. Ask for something that derives the name
          "krate" and cargo sees two packages with that name.

          From the transcript, the AI spent most of its run on it and wrote:

              KRATE-CANNOT-BUILD: ... the skeleton already contains a package
              collision between the app and the SDK (both named "krate" with
              identical version)

          It eventually worked around it by changing the version, but the run
          cost several minutes and nearly failed outright.
Impact:   A whole authoring run wasted, and the error names neither the app nor
          the SDK -- so nobody reading it would know what to change.
Fix:      An app that would be called `krate` is called `krate-app` instead.
          Renaming rather than erroring: the request was buildable, the clash
          is our naming.

### K-047 -- Double-clicking a .krate opens a console window beside the app
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-07, Windows 11, double-clicking a .krate in Explorer
Evidence: A black console window appears next to the app and stays for the
          whole session.

          The file association ran `krate.exe run "%1" --consent`, and
          krate.exe is a console application -- so Windows allocates a console
          for it. Correct for a terminal, wrong for Explorer.
Impact:   Undermines the thing Krate sells. "Someone sends you an app and you
          double-click it" is the whole pitch, and a stray terminal makes it
          look like a developer tool.
Fix:      krate-open.exe, built for the "windows" subsystem, which is the only
          way to avoid the console. It hands the file to the same
          `krate run --consent`, so there is one runner rather than two that
          drift. Failures go to a message box, because with no console there
          is nowhere else for them to go. Shipped in the Windows archives and
          registered by the association script, which falls back to krate.exe
          when it is missing.


### K-044 -- Building Krate on a clean Linux or Windows machine is undocumented
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    environment
Found:    2026-08-07, Ubuntu 24.04 and Windows 11, both fresh Azure VMs
Evidence: Neither README.md nor docs/ names a single build dependency. Found by
          hitting them one at a time on machines with nothing installed:

          Ubuntu 24.04, needed before `cargo build -p krate-cli` gets through.
          Found one at a time, each as a separate failed build:
            build-essential pkg-config libssl-dev cmake
            libwayland-dev          (wayland-sys build script panics without it)
            libxkbcommon-dev        (also what K-036 needs at runtime)
            libasound2-dev          (alsa-sys, for audio)
            libudev-dev             (gamepads)

          Windows 11:
            Visual Studio Build Tools with the VC++ workload
            libclang, for the default feature set -- whisper's bindgen needs
            it. Without it, build with --no-default-features.
            A pagefile. 16 GB of RAM and no pagefile kills the LTO link with
            no message and an empty log; three builds died silently.

          None of this affects somebody INSTALLING Krate -- the released
          binaries carry what they need. It is only the from-source path.
Impact:   Anybody who clones the repo to try a change hits these one at a time,
          each as an unexplained failure. The wayland one is the worst: a
          panic inside a dependency's build script, which reads as a broken
          crate rather than a missing apt package.
Fix:      README's "Build from source" now names all of them per platform,
          with what each is for and what its absence looks like -- the wayland
          one especially, since a panic inside a build script reads as a broken
          crate. The Windows pagefile note is there too.

          The apt line was checked against the Linux VM where the build now
          succeeds: all eight packages present and accounted for. Documenting
          an untested command would have been worse than documenting none.


### K-043 -- Five shipped apps closed their own window after ten seconds
Status:   fixed
Owner:    lead
Severity: serious
Class:    example-bug
Found:    2026-08-07, macOS, running check-app while verifying the release
Evidence: `krate check-app apps/krate-savings`:

              FAILED at usability
              the app opened a window and then closed it by itself after
              11.7s, with nobody asking it to

          Every one of these apps counts quiet rounds and gives up after
          MAX_IDLE_ROUNDS, so a headless check cannot hang. The count was not
          gated on the `quick` argument, so it ran in a real session too: open
          the app, think for ten seconds, and the window closes itself.

          krate-savings, krate-checklist, krate-journal, krate-focus,
          krate-pulse. krate-notes and krate-timer already gated it, which is
          what the correct shape looks like.
Impact:   Worse than it sounds, because these are the example apps. Every
          generated app copies their loop, so the pattern spreads: this is
          exactly the class the bug board calls example-bug, highest leverage
          per line changed.
Fix:      `if quick && idle_rounds >= MAX_IDLE_ROUNDS`. Four of the five now
          pass all six stages. krate-pulse still fails, on an unrelated resize
          bug that is K-003 -- filed, not detoured into.


### K-042 -- Every 3D scene was mirrored, so steering went the wrong way
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, Windows 11 (Azure), playing an AI-written racing game
Evidence: Pressing right steered left and pressing left steered right. The
          game's own code was correct -- right increments its x, and it draws
          the world relative to that -- so the fault was in ours.

          `Scene::project` built the camera basis with

              let right = forward.cross(world_up).normalized();

          With up at +Y, `forward x up` points at -X. Worked through by hand
          for a camera at the origin looking down +Z:

              forward x up = (-1, 0, 0)     a point at +X projects to -1: LEFT
              up x forward = ( 1, 0, 0)     the same point projects to +1: RIGHT

          So an object to the player's right was drawn on their left, and the
          whole scene was mirrored horizontally.

          `up` comes out correct with either order, which is why nothing looked
          upside down and a road -- roughly symmetric -- still looked right.
          The only visible symptom was inverted controls.
Impact:   Every 3D app on every platform. Not a Windows bug; it was found
          there because that is where a 3D game was first played.
Fix:      `up.cross(forward)`, with `up = forward.cross(right)` to match.

          Culling had to flip with it: mirroring reversed screen-space winding,
          so `area > 0.0` preserved the documented
          counter-clockwise-from-outside rule only because two errors
          cancelled. It is now `area < 0.0`, computed rather than guessed.

          Two existing tests encoded the mirrored world and were corrected --
          they had been passing on the strength of the bug. Test
          `something_on_the_right_is_drawn_on_the_right` pins the handedness,
          and also asserts up stays up so a future fix cannot trade one axis
          for the other.


### K-041 -- Krate has no memory on Windows: HOME is not set there
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, Windows 11 Pro (Azure), reopening the menu after
          building an app
Evidence: Build an app successfully, reopen the front door, choose "My apps":

              No apps yet. Pick 1 from the menu and make one.

          The app existed and was on the Desktop. On that machine:

              $env:HOME         -> empty
              $env:USERPROFILE  -> C:\Users\krateuser.krate-win

          `recent_apps_file()` read `HOME` and returned None, so `remember_app`
          saved nothing and `recent_apps` found nothing.

          Ten call sites read `HOME` directly while a correct `home_dir()` with
          a USERPROFILE fallback already existed a few hundred lines away.
          Four of them are user-facing: "My apps" (recent-apps), History
          (history.tsv), the Desktop default (dirs_desktop), and GitHub
          sign-in (github.json). Telemetry and two cache paths as well.
Impact:   Krate appears to have no memory on Windows. Every one failed by
          returning None rather than erroring, so nothing was ever logged and
          it read as a product that simply does not remember anything.
Fix:      `home_dir()` is now `pub(crate)` and every site goes through it. It
          also treats an empty HOME as unset, which is what Windows actually
          presents. A test walks crates/cli/src and fails on any read of HOME
          without a USERPROFILE fallback, so the next one cannot be added
          silently.


### K-039 -- Reading a key destroyed the window's close request
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-07, macOS and Linux, closing an AI-written game
Evidence: A generated game handled `Event::CloseRequested(_) => break` at the
          top of its loop, and clicking the window's close button still did
          nothing. Ctrl-C was the only way out, on both systems.

          `key-held` pumps the platform queue on purpose, so a game that reads
          input every frame and never calls `poll` still sees live keys. It
          then threw the pumped event away:

              let _ = self.poll_one_event();

          That game reads ten keys per frame. Ten pumps per frame, and a
          CloseRequested surfacing during any of them was discarded before the
          game's own `poll` could match on it. The app was written correctly
          and could not be closed.
Impact:   Every canvas game is unclosable by its own close button, which is the
          first thing anyone tries. It also masked K-032: the two-press
          backstop never fired, because the first press was being eaten rather
          than ignored.
Fix:      Events pumped by `key-held` are held in `pending_events` and handed
          over by the next `poll` or `wait`, in arrival order. Test
          `reading_a_key_does_not_swallow_the_close_request` covers it.

          NOT yet confirmed by clicking a real close button -- verified by test
          and by reading the path. The click needs a machine where automation
          has accessibility permission.

### K-040 -- The TUI asks which AI before every single change
Status:   fixed
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-07, macOS, changing a game twice in one session
Evidence: Build an app, pick grok. Choose "Make a change" -- asked again, with
          a full five-tool probe first. Change it again -- asked again. Making
          an app is build, look, change, look again, and each loop re-asked a
          question already answered.
Impact:   Nobody switches AI halfway through changing one game. The probe also
          costs a round trip against every installed tool each time.
Fix:      The choice is remembered for the session and shown as a reminder
          ("using grok -- press a to use a different AI"), so it stays visible
          and reversible without being asked again.


### K-036 -- A GUI app panics on stock Ubuntu: libxkbcommon-x11.so is missing
Status:   open
Owner:    unclaimed
Severity: blocker
Class:    our-code
Found:    2026-08-06, Azure Ubuntu 24.04 + XFCE, x86_64, by running a
          Grok-authored app from the front door
Evidence: The app built, packed, and was written to the Desktop. Opening it:

              thread 'main' (3992) panicked at
              xkbcommon-dl-0.4.2/src/x11.rs:59:28:
              Library libxkbcommon-x11.so could not be loaded.

          On that machine `libxkbcommon.so.0` IS present but the X11 bridge is
          not -- they are separate Ubuntu packages (libxkbcommon0 vs
          libxkbcommon-x11-0), and Ubuntu Desktop installs only the first.

          crates/adapter-linux builds winit with the `x11` feature, which
          dlopens libxkbcommon-x11.so at window creation.
Impact:   Two failures, not one. The app cannot open at all, and the person is
          shown a Rust panic with a crate path and a line number -- exactly the
          thing Krate promises a non-developer never sees.
Fix:      Not started. Needs both halves: name the dependency so it is
          installed, and catch the load failure so a missing library reads as
          one plain sentence naming the package to install, never a panic.

### K-037 -- `krate` exits silently on Windows: no menu, no error
Status:   fixed
Owner:    unclaimed
Severity: blocker
Class:    our-code
Found:    2026-08-06, Windows 11 Pro 25H2 x86_64 (Azure), fresh install
Evidence: In PowerShell:

              PS C:\Users\krateuser.krate-win> krate
              PS C:\Users\krateuser.krate-win>

          Returns immediately. No menu, no error, no exit message. The same
          binary opens the menu on Linux.
Impact:   The front door does not open. Everything Krate offers a Windows user
          is behind this, so on Windows there is currently no product.
Root cause found 2026-08-06. The exit code is the whole answer:

              & krate.exe --version
              EXIT=-1073741515

          -1073741515 is 0xC0000135, STATUS_DLL_NOT_FOUND. Windows refuses to
          start the process, so nothing runs -- not even `--version`, which is
          why there is no output and no error to show.

          The missing DLL is the Visual C++ runtime. On that fresh machine:

              Test-Path C:\Windows\System32\vcruntime140.dll   -> False
              Test-Path C:\Windows\System32\msvcp140.dll       -> False

          Only the .NET-bundled variants (vcruntime140_clr0400.dll) are there,
          and those do not satisfy the import. The x64 VC++ Redistributable is
          not installed, and Windows 11 does not ship it by default.

          The MSVC toolchain links these dynamically. We neither ship them, nor
          install them, nor mention them anywhere.
Proven 2026-08-06 by changing one thing on that machine: installing the
          x64 VC++ Redistributable and nothing else.

              & krate.exe --version
              krate v0.1.0-rc24
              EXIT=0

          Same binary, same shell, same session. The missing DLL was the whole
          of it.
Fix:      Static CRT, .cargo/config.toml sets
          `-C target-feature=+crt-static` for both Windows targets.

          VERIFIED 2026-08-07 by building on Windows 11 and inspecting the
          binary: it imports none of VCRUNTIME140.dll, MSVCP140.dll, or
          api-ms-win-crt. The dependency is gone rather than satisfied, so a
          clean machine that never had the redistributable can run it.

          Two things the build itself taught, both worth keeping:

          - The test VM could not build the default feature set: whisper's
            bindgen needs libclang, which a bare Windows install does not have.
            Built with --no-default-features there. NOT a workflow bug -- the
            GitHub runner ships libclang, and rc24's
            x86_64-pc-windows-msvc.zip was built with speech and shipped
            fine. It is a note about what a Windows machine needs to build
            Krate from source, not about the release.
          - The VM had 16 GB of RAM and NO pagefile. The LTO link needs more
            than the free physical memory, and Windows kills the process with
            no message and an empty log. Three builds died silently before
            that was found. Enabling an automatic pagefile fixed it. Three candidates were considered: build the
          Windows target with a static CRT (`-C target-feature=+crt-static`),
          which removes the dependency entirely and keeps the install one file;
          or have the installer install the redistributable; or ship the DLLs
          beside krate.exe. The static build is the one that cannot regress on
          a machine we never see. Whatever is chosen, the loader failure must
          also stop being silent -- an installed program that exits with no
          output is indistinguishable from one that does nothing.

### K-038 -- The progress display freezes at stage one for the whole run
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-06, Ubuntu 24.04, authoring a 2D game with Grok
Evidence: "reading Krate's API reference 5:31" stayed on screen for the entire
          five and a half minutes, while cargo warnings scrolled through the
          display's own redraw region, and the finished app was announced
          before the display admitted stage one was done.

          Cause: the front door re-invokes this binary as a child
          (`krate author-agent <name>`) to do the authoring. PROGRESS_SINK is a
          process-local static, so the child's report_progress always found
          nothing and fell through to eprintln!, printing into the terminal the
          parent's display was redrawing. Two writers, one terminal.

          The comment at the fallback said the display "owns the terminal" --
          true, and impossible for the child to honour across a process
          boundary.
Impact:   Reads as a hang. It is why an eight-minute Windows run was killed by
          hand: nothing on screen distinguished a working run from a dead one.
Fix:      The child now reports over stdout behind KRATE_PROGRESS_CHANNEL,
          tagged with a control-character prefix; the parent captures that pipe
          and drives the one real display. cargo's stderr is drained and kept
          for the failure message rather than shown.


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
Status:   fixed
Owner:    lead
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
Status:   fixed
Owner:    lead
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

          The last app still failing this (krate-pulse) turned out to have a
          different cause than the entry assumed, and a much smaller fix. It
          set `Style { width: Some(WIDTH), height: Some(HEIGHT) }` on both its
          root and its canvas, so the canvas was pinned to 1080x700 and could
          not grow. The layout engine was obeying the app; the app was asking
          for the wrong thing. `width: None, height: None, grow: 1.0` on both
          nodes, and it passes all six stages -- verified by screenshot too,
          since a fill can stretch a layout that a check cannot see.

### K-004 — No clipping, so a scrolling list would draw over its own header
Status:   fixed (f5820f0)
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
Status:   fixed
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

### K-028 — Two of this machine's three AI accounts are unusable
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

### K-029 — Our development history leaks into every app a user makes
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

### K-030 — A debug build shadows the real release on PATH
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
Status:   fixed (5478426)
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
Status:   fixed (5478426)
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
Status:   fixed (c85dec7)
Owner:    lead
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
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, W17, outsider testing, cold start
Evidence: `krate ai` lists Claude and Codex under "Ready to use" when both fail
          on invocation -- Claude's auth is expired, Codex needs a newer CLI.
          It only checks whether the binary is on PATH. A newcomer follows that
          advice, picks Claude, and gets a failure that looks like Krate's.
Fix:      Fixed by actually running each tool rather than looking at PATH.
          `agent_provider::probe` spawns a cheap round trip, bounded by a
          timeout, and reads the failure into one actionable line: not signed
          in (with the login command), needs a git repo, hit a usage limit, or
          -- the Copilot case -- "fails to start, and prints no reason why".
          All providers are probed in parallel, so the listing costs about six
          seconds rather than four sequential round trips.

          On this machine it now prints exactly what W17 found by hand:
          codex and grok ready, claude not signed in, copilot failing silently.

          Two things found while building it. Codex prints a stdin notice on
          every `exec`, so it gets `login status` as its probe instead. And
          Codex reports a healthy login on *stderr*, so the first version
          demanded stdout and marked a working tool broken.

### K-020 — Double-clicking a .krate opened a file picker, not the app
Status:   not reproducible -- closing
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, W17, outsider testing
Evidence: Double-clicking a `.krate` produced an off-screen file picker rather
          than opening the app. Double-click is the headline promise on the
          website and the simplest path we advertise.
Fix:      Retested by Yashraj on 2026-08-05: double-click works. W17 most
          likely hit the stale /Applications/Krate.app trap, as suspected.
          Closing rather than leaving an unreproducible entry on the board.

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

### K-025 — Four older apps fail check-app at the run stage
Status:   superseded by K-092 -- verified 2026-08-11: the real count is
          thirteen, and seven share one root cause (a round-limited
          interactive loop). Tracking it there.
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-05, lead, sweeping every app after the W12/W13/W14 merges
Evidence: After a clean rebuild, 23 of 27 apps pass every stage. These four
          fail at `run` with "failed to run headless with all grants (exit 1)":
          krate-hello-gui, krate-curl, krate-nova2, krate-spriteproof.
          Pre-existing, not caused by the merges -- they were failing before.
          krate-curl needs a live server, so its failure may be expected rather
          than a defect.
Fix:      Diagnose each. None is a reference app the authoring pack recommends,
          so none blocks a user making an app.

### K-026 — An app's only route to live data is a button, and buttons do not work
Status:   fixed (c85dec7 + the pack fix here)
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-05, Yashraj, first real app made through the new front door
Evidence: Asked for "a news app giving me all open source news". The app is
          correct: it declares `net.connect:hnrss.org:443`, imports
          `krate:net/http-client`, has a real `try_live_fetch`, and carries
          three states -- `mode:sample`, `mode:live`, `mode:live-failed`.
          The run printed `mode:sample`, NOT `live-failed`, so the fetch was
          never attempted. The live fetch is behind a "Refresh" button, and
          pointer input does not reliably reach apps (K-017).

          Verified separately that the feed and the permission are both fine:
          `curl https://hnrss.org/newest?q=open+source` returns 200, and
          `--log-grants` shows `net.connect:hnrss.org:443` was granted.

          So a user asks for live news, gets an app that CAN fetch live news,
          and sees hardcoded sample articles with no way to reach the real
          ones. It reads as "Krate makes fake apps", which is the worst
          possible misreading of a working sandbox.
Fix:      Two parts. Fix pointer delivery (K-017) so the button works. And
          teach the pack that an app should attempt live data on startup, not
          only behind a control -- sample data is a fallback, never the
          default state.

### K-027 — Bundles made by the installed release carry no source
Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    environment
Found:    2026-08-05, lead, while diagnosing K-026
Evidence: `unzip -l ~/Desktop/a-news-app-giving.krate` lists only
          manifest.toml and code.wasm. Source shipping landed in 27f4609 but
          the installed `krate` on PATH is rc20, which predates it. Anyone
          testing with the public install gets none of today's fixes --
          scroll, resize, text measurement, self-close -- and reports bugs
          that are already fixed here.
Fix:      Cut a release. Until then, say plainly which binary a test used.

### K-031 — Krate.app shipped with no icons, so every .krate looks corrupt
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, Yashraj, from the Dock
Evidence: `plutil -p /Applications/Krate.app/Contents/Info.plist` names
          CFBundleIconFile "Krate" and CFBundleTypeIconFile "KrateDoc", but
          Contents/Resources contains no .icns at all, so macOS draws the
          broken-document page on every .krate a user owns.
          make-macos-app.sh regenerated the icons behind `|| true` and copied
          them only `if [ -f ]`, so a build machine without PIL shipped a
          release whose every file looked corrupt, silently.
Fix:      The copy is now required: generated icons first, the committed
          dist/icon/ copies as a fallback, and a hard failure naming the fix
          if neither exists. Verified byte-identical to the source logos.

### K-032 — A window sometimes will not close from its own close button
Status:   open
Owner:    unclaimed
Severity: serious
Class:    unknown -- needs diagnosis
Found:    2026-08-05, Yashraj, using a generated app
Evidence: Clicking the native close button sometimes leaves the window open
          with the pointer in the spinning-wait state. The app can be closed
          from the terminal instead. Read the code: windowShouldClose returns
          false and defers to the app, the callback is queued and drained, and
          the host's wait loop pumps native windows -- so the wiring is
          present and the fault is elsewhere. Not yet reproduced under a
          debugger.
Fix:      Reproduce first with a minimal app. Suspect the app is inside a
          long-running call when the callback arrives, or a redraw loop that
          never yields, rather than a missing event path.

### K-033 — The usage notice printed into a pipe and broke the site build
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, lead, from a failed Pages deploy
Evidence: `bounce.krate: krate did not return listing JSON. got: Krate counts
          how many people use it...` -- scripts/store-listing.py parses krate's
          output as JSON, and the build helper merges stderr into stdout, so a
          one-time notice became the answer the script read.
Fix:      The notice prints only when both streams are a terminal. Counting is
          unaffected; only the notice waits for someone to read it. Verified
          silent when piped and shown under a pty.

### K-034 — The hub dropped every install event
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, lead, checking /stats against what the CLI sent
Evidence: The CLI sent install/open/open; /stats showed installs rising and
          `open` stuck at 1. The Worker's allow-list was
          `["make","open","publish"]`, so install -- the top-of-funnel number
          the whole feature exists to answer -- was silently discarded.
Fix:      install added to the list. Failures and AI-authored runs are counted
          separately too, since a failed make folded into one total is
          invisible.

### K-035 — Evidence bundles predate the WIT and fail replay
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-05, CI, on all three systems
Evidence: `savings: FAILED to run (exit 1) -- this app needs a newer version of
          Krate than you have installed`. The message is backwards: the apps
          are older. Their bundles were built before W12 added the wheel event
          to the WIT.
Fix:      Six rebuilt from source in apps/; five have no source and were
          already passing. Three expectations were also stale -- eo2 and mdview
          asserted strings the apps stopped printing when they moved to the
          key:value quick shape (K-015), and savings asserted a widget caption
          rather than its arithmetic.

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
