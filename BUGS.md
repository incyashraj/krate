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
Status:   open
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
