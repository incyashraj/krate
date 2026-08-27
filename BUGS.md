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

### K-194 -- some animating apps grow ~400 MB/sec until the machine dies; others are flat

Status:   unclaimed
Owner:    unclaimed
Severity: blocker
Class:    our-code
Found:    2026-08-27, by Yashraj: Aurora makes his MacBook hot, laggy, then
          crashes and restarts it. Reproduced and measured; the crash is real
          and this entry cost him a restart to produce, so DO NOT reproduce it
          without a hard memory ceiling and a guaranteed kill.
Evidence: Sampled once a second, killed at a 2.5 GB ceiling:
            Aurora        136 -> 2620 MB in 8s   100% CPU   +363 MB/s
            Aurora 3      376 -> 2554 MB in 7s   100% CPU   +363 MB/s
            Track Dash    156 -> 2704 MB in 6s   100% CPU   +510 MB/s
          Against apps that do not:
            Flow Field    182 -> 182 MB over 12s            flat
            Bounce        107 -> 110 MB over 20s   21% CPU  flat
            Calculator    123 -> 123 MB over 10s    1% CPU  flat
          Unbounded and linear, not the K-151 sawtooth. Extrapolated, Aurora
          reaches 16 GB in 45 seconds.
Not the   Several plausible causes were tested and ruled OUT:
cause:    - "animating apps leak" (K-151): Bounce animates and is flat.
          - "draw_pixels leaks": Flow Field uses draw_pixels and is flat.
          - "the guest allocates per frame": the leakers allocate LESS. Track
            Dash has zero allocation calls in its source and leaks fastest;
            Flow Field has eleven and is flat.
          - "the app forgot to pace its frames": Aurora's own loop paces to
            60fps and its comments show the author understood the
            request_redraw pinning trap. It pins a core anyway.
Scale:    Aurora's two draw_pixels calls move 300,000 bytes per frame, or
          17 MB/s at 60fps. Memory grows at 363 MB/s -- 21x the payload. The
          data itself is not the leak; something retains roughly twenty
          copies of each frame and never frees them.
Lead      The canvas_lists theory is WRONG and is recorded here so nobody
ruled     spends time on it again. supports_canvas_lists defaults to false
out:      (adapter-common/src/ui.rs:1260) and only iOS overrides it, so on
          macOS lists_enabled() is false, record_op returns before pushing,
          and canvas_lists is never filled. It cannot be the leak.
Found     phase3_gui_host.rs:3832 -- draw_pixels calls record_op FIRST, and
instead:  builds `rgba.clone()` to do it, then checks lists_enabled() and
          returns. On macOS that clone is a full copy of every frame's
          pixels, constructed and immediately discarded: 300,000 bytes per
          Aurora frame, 17 MB/s at 60fps. Real waste, and the argument order
          is plainly wrong -- the clone should happen inside the branch that
          uses it. But 17 MB/s is not 363 MB/s, so this is A leak, not THE
          leak. Something else holds the other twenty copies.
Best      publish_canvas (phase3_gui_host.rs:1972) copies the WHOLE window
lead:     surface every frame -- `surface.to_image()` -- and inserts it as an
          Arc into self.images. Aurora's window is 900x600, which on a retina
          display is 1800x1200 rgba = 8,640,000 bytes per frame, or 494 MB/s
          at 60fps. Measured growth is 363 MB/s: the same order, consistent
          with a real frame rate somewhat under 60. Nothing else in the path
          is near that scale.
          The insert REPLACES on the same (window, widget) key and the only
          other holder takes an Arc clone, so by construction the old buffer
          should drop. It evidently does not: the per-second deltas are
          297, 363, 371, 363, 363, 363, 364 -- flat and linear, with no
          plateau. Churn against a reusing allocator would flatten; this does
          not, so something retains every frame's surface.
Does not  2026-08-28, after a restart: the SAME Aurora 3 bundle that grew to
reproduce 2554 MB now runs flat at 130 MB for six seconds, and drops to 110.
now:      Repeated three times, release build and debug build, both flat. So
          the leak is real -- it was measured twice, and it restarted the
          machine -- but it is CONDITIONAL on something that was true earlier
          and is not true now.
          What differed, and none of it is yet ruled in or out:
            - the machine had been up ~14 hours and was under load from a
              build matrix; now it is 20 minutes from a cold boot
            - other krate apps had been run and killed beforehand
            - the earlier runs used hand-passed --grant flags, the later ones
              derive the same set from --dump-caps (verified identical)
          The instrumented run also showed the presenter line for the first
          time: "canvas presents on Apple M4 (Metal, IntegratedGpu)". The
          surface-copy path in publish_canvas is CPU-raster; if Metal was
          active in the flat runs and not in the leaking ones, that is the
          difference and the whole earlier analysis points at the right code
          for the wrong reason.
Next:     Do not fix anything yet. Reproduce first, deliberately: run the
          matrix or several apps to put the machine in the earlier state,
          then measure Aurora again with stderr captured so the presenter
          line is on the record for BOTH a leaking and a flat run. Only then
          is there something to fix. Four theories have now been wrong and
          every one of them looked right on paper.
Reach:    Eleven shipped apps use draw_pixels, including Ice Climber (the
          multiplayer demo), Super Mario Bros., Track Dash and Weather. At
          least one of them (Track Dash) leaks. The blast radius is unknown
          until each is measured.
Safety:   /tmp/memtest/probe.sh measures one app under a hard time limit AND
          a memory ceiling, and kills the process tree on both paths. Use it.
          A bare `krate run` on one of these will take the machine down.


### K-193 -- macOS asks for Apple Music and the media library in Krate's name, mid-build

Status:   unclaimed
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-27, on v0.2.1, by Aanchala while an app was being built with
          Grok. The dialog appeared over the live build view, at the "Testing
          it" stage:
            "Krate" would like to access Apple Music, your music and video
            activity, and your media library.
          The build itself worked -- she made her app -- so this is a trust
          problem rather than a functional one, and that is what makes it
          urgent: it is the same shape as K-179, which was reported as
          bombardment by a stranger.
Evidence: Krate does not ask for this. Verified two ways on the shipped
          v0.2.1 bundle:
            - no source in crates/*/src or studio/src references
              AppleMusic, MPMediaLibrary or NSAppleMusicUsageDescription
            - Info.plist declares exactly two usage strings, camera and
              microphone, and no media entry at all
          So the request comes from a CHILD process. macOS attributes a
          child's access to the parent bundle's identity -- the mechanism
          behind K-179 -- so whatever the agent or the app under test touched,
          the prompt carries Krate's name.
Likely:   The build was at the usability stage, which RUNS the app being
          written. An app that declares audio.playback opens the system audio
          path, and on macOS the media-library door sits close to it. Worth
          confirming before fixing: reproduce by building an app that plays
          sound, and watch whether the prompt appears at the run step.
Why it    A person is told an app they are making wants their music library.
matters:  Nothing in Krate's promise explains that, and the permission wall is
          the product's central claim. A prompt nobody can account for costs
          more trust than the feature that triggered it is worth.
Next:     Reproduce first, deliberately, then decide. If it is the audio path,
          the fix is to open only what the app actually declared rather than
          the general media session. Do NOT guess at an entitlement change: an
          Info.plist edit that silences a prompt without understanding it
          would hide the evidence rather than fix the cause.


### K-192 -- cutting any non-rc tag silently repoints the website, overriding a deliberate rollback

Status:   unclaimed
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-27. Yashraj decided to serve v0.1.57 from the website while
          the 0.2.x line was unstable, and it was set. Twenty minutes later
          the v0.2.1 build finished, its promote job ran, and the website was
          pointing at v0.2.1 again. Nobody chose that.
Evidence: release.yml's promote job runs for every tag without "-rc" and
          does:
            gh release edit "$TAG" --prerelease=false --latest
          There is no check for whether someone has deliberately pinned an
          older release. The pin is a GitHub flag with no record in the repo,
          so the pipeline cannot see it and overwrites it.
Why it    A rollback is the thing you do when users are hitting a bug. The one
matters:  moment it must hold is exactly when a fix is being built -- and that
          is precisely when the pipeline undoes it. It is silent: the run is
          green, nothing warns, and the only way to notice is to check the
          flag by hand afterwards.
Next:     Make the pin visible to the pipeline. A CHANNEL file in the repo
          naming the tag the website should serve, with promote refusing to
          move latest when the file names a different tag, keeps the decision
          in version control where it can be reviewed -- rather than in a
          GitHub flag that a later build silently wins.


### K-191 -- the Studio has its OWN credential seeding, and it was Claude-only too

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27. Aanchala updated to v0.2.1-rc.1 -- the release carrying
          the K-189 fix -- and her build still failed. Diagnosed entirely from
          her report, with no round trip, which is K-185 working as intended.
Evidence: about.txt from the rc:
            krate: v0.2.1-rc.1
            agent codex: working
            agent grok: working
          and the transcript the report now carries:
            401 Unauthorized: Missing bearer or basic authentication in
            header, url: https://api.openai.com/v1/responses
          "Missing bearer" is a Codex that found no credential at all, not one
          that was refused -- so the confined home was still empty for it.
Cause:    There are TWO seeding functions, and K-189 fixed only one.
            crates/cli/src/main.rs::seed_agent_home   -- fixed in K-189
            studio/src/main.rs::seed_agent_config     -- still Claude-only
          The Studio sets HOME to the confined directory (agent_home_env,
          line 1081) BEFORE it runs the engine, so by the time the engine's
          own seeding runs the agent is already pointed at a home the Studio
          prepared. For a Studio build, the Studio's copy is the one that
          decides, and it copied .claude.json and .claude/.credentials.json
          and nothing else.
Fixed:    seed_agent_config now copies ~/.grok, ~/.codex, ~/.gemini,
          ~/.copilot and the ~/.config variants, shallowly, exactly as the
          engine does.
Lesson:   Two implementations of one rule is the bug. The engine and the
          Studio each confine the agent's home, and each seed it, and fixing
          the rule in one place left the other wrong in a way that looked
          identical from outside -- the user updated to the fix and saw no
          change. Worth collapsing into one shared function; recorded rather
          than done, because the release should not wait on a refactor.
Test:     a_credential_travels_but_the_history_under_it_does_not pins both
          halves: the credential at the top of the config dir travels, the
          session history in its subdirectories does not.


### K-190 -- the readiness probe runs in a different home from the build, so the chip is green while every build fails

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27, chasing why Aanchala's Grok said "Not signed in" while
          her `grok` TUI worked and her Studio chip was GREEN.
Evidence: Authoring confines HOME to ~/.krate/agent-home (K-179). The probe in
          agent_provider::probe did not -- it inherited the real home. So the
          two asked different machines the same question:
            probe     (real HOME)     -> working, green dot
            authoring (confined HOME) -> "Not signed in. Run: grok login ..."
          Both measured here on one machine with one signed-in Grok. The chip
          was not merely unhelpful, it was actively wrong: it reported the
          state of a home the build never uses.
          Not Grok-specific. Under the confined home, codex fails the same way
          -- a real run produced 11 error events and 19 auth-failure lines.
Fixed:    probe now runs under the same home authoring will use, when that
          directory exists. It returns None rather than creating it, because a
          probe must observe and never set the machine up; on a first run
          there is nothing to confine to, authoring creates it, and from then
          on the two agree.
          Verified: with no seeded credential, grok now probes as
          "not-ready -- is installed but not signed in" instead of "working".
          Her chip would have been red, with the reason, before she typed a
          word.
Note:     This exposed how wide K-189 is. With the probe telling the truth,
          claude, codex, copilot and grok ALL report not-ready on this machine
          -- because only Claude's credential is copied into the confined
          home. The green dots were hiding it. K-189 is the fix; this entry is
          why it was invisible.
          One test changed with it: ai_lists_what_this_machine_can_author_with
          asserted the output always contains "krate create", which silently
          required the machine running the suite to be signed in to an AI. It
          now accepts either the next command or the fix, because what matters
          is that the person is never left at a dead end.


### K-189 -- the confined agent home carries only Claude's sign-in, so every other AI runs signed out

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27. Aanchala's Grok builds failed with "Not signed in",
          twice. Yashraj then had her run `grok` in a terminal: the TUI opened
          and she could talk to it, so she WAS signed in and the transcript
          was telling the truth about a state Krate had created.
Evidence: Krate rebases the agent's HOME to ~/.krate/agent-home (K-179, so the
          agent cannot ask for the person's Downloads folder in Krate's name),
          and seed_agent_home copies exactly four paths across:
            .claude.json, .claude, .claude/.credentials.json, .credentials.json
          All Claude. Nothing for Grok, Codex, Gemini or Copilot, whose
          credentials live in ~/.grok/auth.json and friends.
          Measured on this machine, same signed-in Grok, one variable changed:
            HOME=<real>               -> 0 "not signed in"
            HOME=~/.krate/agent-home  -> 2 "not signed in", identical text
          So the confinement that protects the person's files also signs them
          out of every AI except Claude.
Fix       seed_agent_home now shallow-copies ~/.grok, ~/.codex, ~/.gemini,
written:  ~/.copilot and the ~/.config variants into the confined home, and
          creates the directory first (the caller created it AFTER seeding, so
          on a first run every copy landed nowhere -- Claude's survived only
          because it makes its own subdirectory on the way).
Real      The copies were unreachable. seed_agent_home ends with a macOS
cause:    branch that reads Claude's credential out of the keychain and, on
          success, did `return true` -- leaving the function before any other
          provider was touched. On any machine with Claude signed in (which
          is every machine we develop on) Grok, Codex, Gemini and Copilot
          silently got nothing. Found by instrumenting the function and
          watching one real run: the entry trace fired, the loop trace never
          did.
Verified: with the early return replaced by a flag,
            ~/.krate/agent-home/.grok/auth.json   SEEDED
            ~/.krate/agent-home/.codex/auth.json  SEEDED
          the probe reports grok and codex "working" instead of "not signed
          in", and a real `krate create --agent grok` gets past authentication
          and into authoring -- reading Krate's API and writing code.


### K-187 -- two writers tear the transcript in half, so a signed-out AI reports nothing

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27. Aanchala, a first-time user on her own MacBook, tried to
          make a photo editor and then a calendar with Grok. Both died in one
          second and said only "the grok agent did not finish successfully;
          see <file>". She sent the file, and it said, in plain English:
            Error: Not signed in. To authenticate without a browser, run:
              grok login --device-code
          Krate had the answer on disk the whole time and showed her none of it.
Evidence: Reproduced exactly: a signed-out grok exits 1 and writes the SAME
          error twice -- once as a JSON event on stdout, once as prose on
          stderr:
            {"type":"error","message":"Not signed in. To authenticate ..."}
            Error: Not signed in. To authenticate without a browser, run:
          The JSON event parses correctly (type=error + message, both of which
          agent_failure_reason_in already reads), so it should have been
          quoted. It was not, and her file shows why. It ends with the orphan
          fragment:
            achine with a browser."}
          which is the TAIL of that JSON event, cut in two. The transcript has
          two independent writers and no lock between them: stderr is wired
          straight to the file at spawn (main.rs:5882), while the reporter
          thread opens its own append handle and writes every stdout line
          (main.rs:5929). A stderr write landed in the middle of the JSON
          line, so it no longer began with `{`, the parser skipped it, and
          agent_failure_reason returned None.
Fixed:    agent_failure_reason_in now also reads a plain-text "Error:" line
          and the indented continuation under it, and prefers a surviving
          JSON event when there is one. The reason is now independent of
          whether the JSON won the race. Verified against her real transcript,
          reconstructed with the tear in the same place:
            REASON: "Not signed in. To authenticate without a browser, run:
                     grok login --device-code"
          The continuation matters: the first line names the problem and the
          second carries the cure, and a reason without the cure sends the
          person to a search engine.
Still     The interleaving itself is NOT fixed -- stderr and the reporter
open:     still share the file with no coordination, so any long JSON line can
          still be torn. The parser no longer depends on it, so this is now a
          cosmetic corruption rather than a silent failure, but the real fix
          is to pipe stderr through the same thread that owns the file. Left
          out of this change deliberately: it is a spawn-path change with a
          real risk of losing stderr entirely, and it should not ride along
          with a fix people are waiting on.

### K-188 -- `krate` is not on PATH after installing Krate Studio

Status:   fixed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-27, Nakshatra's MacBook, while trying to run a diagnostic:
            $ krate ai --json
            zsh: command not found: krate
            $ krate --version
            zsh: command not found: krate
          He has Krate Studio installed and working -- he opened apps from the
          cloud successfully in the same session -- so this is not a broken
          install.
Cause:    The .dmg installs Krate.app, and the engine lives inside the bundle
          at Contents/Resources/bin/krate. engine() finds it there, which is
          why the Studio works. Nothing ever puts it on the shell PATH; only
          the CLI tarball from krate.tech does that.
Why it    Every support instruction we give starts with a `krate` command, and
matters:  for a Studio-only user none of them work. It also makes a Studio
          user unable to answer the one question that would diagnose their own
          problem, which is how K-183 stalled.
Real      first_run_setup DID try to symlink into /usr/local/bin. It was
cause:    guarded by `if dir.is_dir()`, and the comment claimed the directory
          "is writable by the admin user on most machines". Measured here, as
          an admin:
            /usr/local/bin  ->  root:wheel, NOT writable
          So the symlink failed silently on every machine, `setup-done` was
          written regardless, and it never tried again.
Fixed:    Two halves.
          - first_run_setup still never asks for a password, but now probes
            writability by attempting a file rather than assuming ownership,
            and replaces a stale link that points at a moved engine.
          - Settings gains a "Terminal" row: one button, one password, and
            `krate` is on PATH. A button rather than a prompt on launch --
            being asked for a password by an app you just dragged in is worse
            than not having the shortcut.
          /usr/local/bin is the right target: it is the FIRST entry in
          /etc/paths on a stock Mac, so a link there is on PATH for every
          shell without touching anyone's dotfiles.
Verified: a symlink to the in-bundle engine resolves correctly --
            /tmp/linktest/krate --version   -> krate v0.2.0
            /tmp/linktest/krate ai --json   -> 5 agents
          so the engine finds its own resources through the link.


### K-185 -- every support report sent from the Studio arrives without the evidence

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27, trying to diagnose Aanchala's Grok failure from her
          report and finding nothing in it to diagnose. Three reports had
          already arrived this way without anyone noticing, because a report
          that contains a transcript of the CHAT looks complete until you
          need the transcript of the BUILD.
Evidence: Her report (hub fb12c719bde2af0b) held exactly two files:
            session.json   the on-screen conversation
            about.txt      the machine
          and neither can explain a build failure. The card on her screen
          named the file that could -- .agent-transcript.txt -- and it stayed
          on her disk.
Cause:    report_command found the workspace only by searching the session
          text for the phrase "the workspace is kept at ", which is printed
          by WorkspaceKeeper's Drop when a TEMP workspace is kept after a
          failure (main.rs:4750). The Studio never uses a temp workspace: it
          passes --work-dir so a retry resumes from the code already written
          (studio/src/main.rs:742, K-129). So the phrase is never printed,
          the search always fails, and the collection loop never runs.
          Verified on a real session, before and after:
            before: 2 files (session.json, about.txt)
            after:  7 files, including a 1.3 MB .agent-transcript.txt and
                    trace.jsonl
Fixed:    The Studio's workspace is derived rather than parsed -- it is
          studio/builds/<session>, built from the same session id the command
          already has -- and the old phrase search stays as the fallback for
          CLI builds. Both levels are collected: trace.jsonl sits at the top
          and the app's transcript and source one folder down, which was
          checked against a real build directory rather than assumed, because
          reading only the top would have collected the trace and missed the
          transcript.
          trace.jsonl is now collected too. Every Studio build already writes
          one (KRATE_TRACE, studio/src/main.rs:751) and nothing had ever read
          it; it carries the timing spine the authoring study needs.
Lesson:   A support channel is not working because reports arrive. Send one
          from a real machine and open it. Three did arrive, and all three
          were hollow.

### K-184 -- Krate's own advice is read back as the diagnosis, so a signed-in AI is told to sign in

Status:   fixed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-27, by Yashraj's brother, v0.2.0, on a Mac whose Codex chip
          was GREEN and in use. The failure card said "Your AI needs signing
          in. Click its name at the top for the fix." He was signed in.
Evidence: The chain is entirely inside our own code:
          1. crates/cli/src/main.rs:6500 -- when a provider error mentions
             authentication, the engine APPENDS its own sentence:
               "Sign in again, then try once more."
          2. That whole string, Krate's prose included, becomes the error the
             Studio receives.
          3. studio/ui/app.js plainWords() classifies by matching words in
             that text, and its rule
               /sign ?in|\bauth(?!or)|logged/
             matches OUR sentence, not the provider's error.
          So any failure whose underlying error merely MENTIONED
          authentication came back as "needs signing in", with total
          confidence, under a green dot.
          Second offence of the same kind: the (?!or) guard on that very line
          is the scar from K-124, where "author command failed" -- also our
          own words -- sent every user to sign in.
Fix:      Strip Krate's own prose BEFORE classifying (KRATE_OWN_PROSE +
          providerWords in app.js), so the guess is made on what the provider
          said and nothing else. Then classify on the provider's real signal:
          a usage limit is checked before auth (telling someone to sign in
          when they are already signed in is the most confusing thing this
          card can say), and a genuine auth failure is matched on HTTP 401/403
          and "unauthorized" as well as the words.
          Verified against his exact error text plus both K-124 regressions:
          8 cases, 0 failures.

### K-182 -- an AI that refuses to answer is reported as "building directly", so a request is built after the person said not to

Status:   fixed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-27, by Yashraj's brother on a cold MacBook, v0.2.0. He typed
          "do not create any app" and Krate built one anyway. He is the first
          person to use Krate who did not build it, which is why this is the
          most valuable report we have had.
Evidence: The submitted report (hub id 4a1ab58e1cc1d747) carries the whole
          proof. about.txt:
            krate: v0.2.0        <- version stamping works
            agent claude: missing
            agent codex: installed
          session.json, all four messages, with timestamps:
            1787765607 YOU   do not create any app
            1787765607 KRATE On it. I'll show you each step as it happens.
            1787765607 KRATE While I work, I'll open your app a few times ...
            1787765629 KRATE that build didn't come together
          "Looking at your request..." is ABSENT and the first two lines share
          a timestamp to the second. The plan step did not run slowly; it did
          not visibly run at all.
Cause:    Reproduced locally on 2026-08-27 with the same agent:
            $ krate plan "do not create any app" --agent codex
            note: could not read a plan from the AI; building directly.
            {"needs":[],"plan":""}
          versus the same request with a working agent, which is correct:
            $ krate plan "do not create any app" --agent claude
            {"ask":["You said not to create an app -- do you want me to stop
             entirely, or did you mean something else by that?", ...]}
          The chain: codex exec --json emits a line-per-event stream. When the
          account cannot answer, the stream carries
            {"type":"error","message":"You've hit your usage limit..."}
            {"type":"turn.failed","error":{...}}
          and no event carries an `ask` or `plan` key. extract_plan_json
          (crates/cli/src/main.rs:4426) searches for exactly those keys, finds
          none, and plan_command falls through to its deliberate soft
          fallback at main.rs:4393, emitting {"plan":""}. The Studio reads an
          empty plan as "the AI decided this is ready to build" -- identical
          to a genuine go-ahead -- and builds.
          Confirmed the same shape by hand:
            codex exec --json --skip-git-repo-check --sandbox workspace-write '<prompt>'
            => {"type":"error","message":"You've hit your usage limit..."}
Why it    The fallback is deliberate and its reasoning is sound for a
matters:  MALFORMED answer: "we could not pre-plan, so we are just building
          it" is better than failing a first request. But it cannot be right
          for a REFUSED one. An AI that never answered is not an AI that said
          yes, and the two are currently indistinguishable to the Studio. The
          result is the worst thing an app-maker can do: build something after
          being told not to, then charge the person fifteen minutes for it.
Fixed:    plan_command now asks, before falling back, whether the provider
          REPORTED AN ERROR rather than merely answered unreadably. It reuses
          the authoring path's own reader, split into agent_failure_reason_in
          so both callers share one understanding of each provider's shape.
          On a refusal it bails with the provider's own words and the line
          that matters: "Nothing was built."
          Verified end to end with the account that caused it:
            $ krate plan "do not create any app" --agent codex
            error: codex could not look at your request:
              You've hit your usage limit. Visit ... or try again at Sep 1st.
            Nothing was built. This is a problem with the AI tool, not with
            Krate or your request.
          and unchanged for a working agent, which still asks its question.
          Three tests pin both directions: a refusal must not read as
          permission, and an unparseable answer must NOT be reported as a
          refusal -- the soft fallback is deliberate and still protects a
          first request from dying on an output shape nobody has seen.

### K-183 -- the AI picker names five tools and installs none of them

Status:   unclaimed
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-27, same session as K-182. He could not get an AI connected
          and gave up on the Mac; on the Windows machine the build started
          with no AI connected at all and only then said claude was missing.
Evidence: His screenshot of "Your AI": five rows, four of them dead ends that
          hand him a command to run somewhere else --
            Claude   "Install Claude Code from https://claude.com/claude-code,
                      then run `claude` once to sign in."
            Gemini   "npm install -g @google/gemini-cli@latest, then run ..."
            Copilot  "npm install -g @github/copilot@latest, then run ..."
            Grok     "install the Grok CLI from xAI (docs.x.ai), then run ..."
          Only codex showed "ready - in use", and codex was the one that then
          failed (K-182).
Note:     The machinery is all there, which makes this a wiring bug rather
          than a missing feature -- verified 2026-08-27, not assumed:
            - install_agent exists (studio/src/main.rs:1740) and streams
              progress to the UI.
            - The UI already builds an "Install" button for
              state === "missing" && install_package (app.js:1801).
            - `krate ai --json` on this machine returns install_package for
              FOUR of the five: claude @anthropic-ai/claude-code, codex
              @openai/codex, gemini @google/gemini-cli, copilot @github/copilot.
              Only grok is None (no npm package).
SETTLED 2026-08-27 by his own output. He ran the engine from inside the
          bundle and `krate ai --json` returned, on HIS machine:
            claude  state=missing  install_package=@anthropic-ai/claude-code
            codex   state=working  install_package=@openai/codex
            gemini  state=missing  install_package=@google/gemini-cli
            copilot state=missing  install_package=@github/copilot
            grok    state=missing  install_package=null
          So the data was right: three rows (claude, gemini, copilot) were
          "missing" WITH a package, which is exactly the branch that draws an
          Install button, and only grok legitimately has none. The engine is
          not at fault and neither is the row logic.
          What remains unexplained is only the screenshot, and it is no longer
          worth chasing from here: the picker he saw may predate the build he
          now has, or the buttons may have been below his fold. Re-check on
          the next release with a fresh screenshot before spending more on it.

Earlier testing, kept because it rules things out: Driving the
          REAL openAiSheet() in a browser with HIS exact rows -- claude
          missing, codex working, gemini/copilot/grok missing -- produces:
            Claude  -> Install (82px, visible)
            Codex   -> no button (in use, correct)
            Gemini  -> Install (82px)
            Copilot -> Install (82px)
            Grok    -> no button   <- only row with no install_package
          So four rows DO get a button, and the code that draws them shipped
          in v0.2.0 (verified: `git show v0.2.0:studio/ui/app.js` contains
          both the Install button and the install_agent call). A flex-shrink
          theory was tested and DISPROVED -- the buttons are not squeezed.
          What that leaves: his screenshot shows prose and no buttons, and
          the reason is still not established. Do not fix this by guessing.
          Get `krate ai --json` from HIS machine; if a row comes back with a
          state outside working/not-ready/missing, or with install_package
          null where mine has one, that is the answer and it is one line.
Fixed     Two real defects found while testing this, both shipped:
here:     - The onboarding picker labelled EVERY non-working agent "not
            installed", including one that is installed and only needs a
            sign-in. Telling someone to install what they already have is
            how a picker becomes a dead end. Now says "needs signing in".
          - .ai-row's button had no flex-shrink guard. Not the cause of his
            screenshot, but a genuine hazard: the row's text is a full
            sentence with a URL in it, and nothing stopped it taking the
            button's width. Pinned with flex: none.
Still     Grok has no npm package, so it can never show an Install button --
open:     it is the one row that is honestly a dead end, and it needs its own
          route (a link that opens docs.x.ai would beat prose).

### K-181 -- the notes.krate published on krate.tech exits 2 while working correctly

Status:   fixed in source, awaiting republish
Owner:    claude
Severity: serious
Class:    example-bug
Found:    2026-08-26, claude, reading the nightly CI failure before cutting
          v0.2.0. Three jobs failed -- ubuntu, macOS and windows cold-install
          -- on a commit that had passed the day before, which is what made
          it worth chasing rather than dismissing as flake.
Evidence: The cold-install walk runs the demo app the website hands a new
          person, and asserts it exits 0:
            krate run --headless --grant 'fs.read:notes/**' \
              --grant 'fs.write:notes/**' https://krate.tech/notes.krate
            => exit 2
            stdout: note:Ship the demo
                    saved:yes
            stderr: (empty)
          Reproduced locally on macOS with the current engine. The app does
          its job -- it reads its note and saves -- and then reports failure.
          Exit 2 out of the runtime is InvalidComponent, but that path prints
          to stderr and stderr is empty, so this is RunOutcome::Exited(2):
          the guest's own return value. The bug is in the published bundle,
          not in the engine.
Why it   A person following the website runs this app first. Anything that
matters:  wraps it -- a script, a launcher, CI, `&&` in a shell -- sees a
          failed program. It also silently disarms the cold-path gate: the
          test that exists to prove a stranger's first run works has been
          red on every platform since it started failing.
Cause:    apps/krate-notes/src/lib.rs ended run() with
            if close_requested { 2 } else { 0 }
          and close_requested is set by types::Event::CloseRequested -- the
          event a person generates by clicking the X. So the app reported
          failure for the ordinary way of quitting it, not only headless.
          Headless just makes it deterministic: the host closes the window,
          so the demo failed on every CI run on all three platforms.
Fixed:    return 0. Verified with the exact commands the cold-install gate
          runs, both halves:
            no grants  -> exit 5, "needs permission"   (gate wants 5)
            with grants-> exit 0, note:Ship the demo   (gate wants 0)
          check-app passes every stage.
Next:     Republish notes.krate to the notes-v0.1.0 release asset that
          pages.yml pulls from (release.yml does not build this app), then
          confirm the three cold-install jobs go green. Blocked on the
          GitHub Actions outage that began 2026-08-26 15:11 UTC.

### K-180 -- a dev Studio silently loses the planning conversation, because studio/bin/krate goes stale

Status:   fixed
Owner:    claude
Severity: medium
Class:    environment
Found:    2026-08-26, by Yashraj: "the stable one goes to the app making and
          asks me question before making an app, but the one you made just
          starts making, and this thing makes me feel that something is
          really wrong". Found comparing a dev build against installed
          v0.1.58 -- the exact comparison that catches this class of fault.
Evidence: The engine Studio runs is resolved by engine() in studio/src/main.rs,
          which checks a sibling of the executable and then studio/bin/. On
          this machine studio/bin/krate was built 2026-08-16 and predates the
          `plan` subcommand entirely:
            $ ./studio/bin/krate plan "can you make an app?"
            error: unrecognized subcommand 'plan'
            tip: a similar subcommand exists: 'launch'
          plan_request in studio/src/main.rs turns a non-zero exit into Err,
          runPlan's catch in studio/ui/app.js says "I'll skip the questions
          this time and build right away" and calls finishPlanningAndBuild().
          So a stale engine does not read as a broken engine -- it reads as
          a product that stopped asking questions. With the current engine
          the same request answers correctly:
            {"ask":["What should the app do -- what's the one task it helps
            you finish?", ...]}
Fix:      cp target/release/krate studio/bin/krate. Not shipped-broken: the
          release workflow rebuilds studio/bin/krate from the matrix target
          on every platform (release.yml:227,272,294,322) and the path is
          untracked, so only local dev builds can drift.
Lesson:   The engine beside the Studio is a build artifact with no version
          check. A dev Studio can be arbitrarily newer than the engine it
          drives, and every failure surfaces as changed PRODUCT behaviour
          rather than as a tooling error. Same family as "the binary on PATH
          is not the one you built" -- when dev and stable behave differently,
          suspect the pair of binaries before suspecting the diff.

### K-179 -- macOS asks for Downloads, Documents and Music in Krate's name, because the AI agent inherits our identity

Status:   claimed
Owner:    claude
Severity: blocker
Class:    our-code
Found:    2026-08-26, by Yashraj: "whenever someone installs and opens krate,
          it bombards them with allow access to downloads, documents, music".
          Found while approaching content creators -- a stranger reads these
          prompts as Krate demanding their whole home folder, which is the
          exact opposite of what the permission wall promises.
Evidence: NOT the permission wall and NOT the app-running path. A live launch
          of the installed /Applications/Krate.app holds ZERO handles under
          Desktop/Documents/Downloads/Music/Pictures:
            lsof -p <studio pid> | grep -icE 'Desktop|Documents|Downloads|Music|Pictures'
            => 0
          The prompts come from the CHILD we spawn. studio/src/main.rs spawns
          the agent with cmd.current_dir(work) and, in agent_provider.rs,
          `--permission-mode bypassPermissions`. On macOS, TCC attributes a
          child process's file access to the PARENT BUNDLE's identity, so
          every folder Claude Code explores is asked for in Krate's name.
          Second, independent source: crates/cli/src/tui.rs:1618 dirs_desktop()
          calls desktop.is_dir() and the CLI writes finished apps to ~/Desktop;
          stat-ing Desktop is itself a TCC trigger. The Studio's default was
          moved to ~/Krate Apps for this exact reason (studio/src/main.rs:252
          comment) and the CLI never got the same fix.
Fix:      Two changes, both proven:
          1. agent_home_env() rebases the agent's HOME onto
             ~/.krate/studio/agent, so `~` inside the agent is Krate's own
             scratch dir. PROVEN: run the agent under that environment and
             ask it to `ls ~/Downloads` -- it answers "No such file or
             directory ... ~ resolves to .krate/studio/agent, not your real
             home", and the system is never asked. CARGO_HOME and RUSTUP_HOME
             are pinned to the REAL home in the same function, because cargo
             resolves them from $HOME and the agent builds what it writes;
             without that pin, confining the agent would cost it its
             compiler. Sign-in verified working under the rebased HOME (a
             401 during testing was a day-old seeded credential, not the
             change: re-seeding from the keychain, which seed_agent_config
             does every spawn, authenticates).
          2. The CLI's default output moved from ~/Desktop to ~/Krate Apps,
             where the Studio already writes. dirs_desktop() called
             desktop.is_dir(), and stat-ing a TCC-guarded folder is itself
             the trigger; the new default_app_dir() creates ~/Krate Apps
             instead and never probes a guarded path.
          3. The ENGINE confines too, not just the Studio. `krate create` in
             a terminal is its own door and the Studio delegates builds to
             the engine, so the Studio-only fix leaked. Found by testing,
             not by reading.
          Two near-misses caught in a FRESH macOS account (user "test",
          created for this), both of which would have shipped:
            - Rebasing HOME without moving the credential: every build died
              "Not logged in · Please run /login". The engine now seeds the
              credential into the confined home first.
            - Then confining only when seeding SUCCEEDED, which silently
              handed the real home back to anyone with no credential yet --
              a first-time user, exactly the person meeting these prompts.
              Measured in the fresh account: AGENT_SEES_HOME=/Users/test.
              Confinement is now unconditional; after the fix the same probe
              reports AGENT_SEES_HOME=/Users/test/.krate/agent-home with
              CARGO_HOME=/Users/test/.cargo intact.
          Cold-room evidence: a fresh macOS account installed Krate from
          krate.tech and ran an app (115 KB screenshot) with ZERO entries in
          the TCC log. A bundle given an identity macOS had never seen
          launched with zero protected-folder handles and no dialogs.
          Regression tests: the_agent_never_inherits_the_persons_home (studio),
          the_agents_home_is_confined_for_everyone_including_first_time_users
          (engine), finished_apps_never_land_in_a_folder_macos_guards (cli).

### K-178 -- A double-clicked game on Windows shows consent, gets approved, and never opens

Status:   open
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-25, by Yashraj on the Windows PC with the multiplayer Ice
          Climber (v0.1.57 engine confirmed on the machine).
Evidence: the same .krate runs and paints on that PC when launched with
          --grant flags over SSH (C:\krate\mp-pc.png, 49KB), so the app,
          the runtime, and net.ws all work there. The failure is specific to
          the consent path: dialog appears, Allow clicked, no window. Remote
          reproduction blocked by the machine's locked interactive session;
          next signal is stderr from a terminal run:
          %LOCALAPPDATA%\Krate\bin\krate.exe run <file> --consent
          2> C:\krate\err.txt
Fix:      pending that stderr. Suspects: the post-consent continuation in
          the association context, or security software (see K-177 -- the
          same machine shows dialogs about taskkill.exe).

### K-177 -- Stopping a build could make security software pop a dialog about taskkill.exe

Status:   fixed
Owner:    claude
Severity: annoyance
Class:    our-code
Found:    2026-08-25, by Yashraj on the Windows PC: "sudden dialogue box
          saying taskkill.exe not working".
Evidence: the Studio's kill_tree and the CLI's agent-probe timeout both
          shelled out to taskkill /PID x /T /F. Both suppressed its OUTPUT
          (K-159), but the spawn itself is what endpoint security watches:
          an unsigned app launching a system kill tool reads as suspicious,
          and the watchdog's dialog is outside our control.
Fix:      no external process at all: kill_process_tree walks
          CreateToolhelp32Snapshot and TerminateProcess-es the tree,
          children first, in-process -- nothing for a watchdog to flag, no
          console to flash, same semantics. crates/cli/src/winproc.rs and
          the studio twin; compile-proven for the msvc target.

### K-176 -- Windows consent is all-or-nothing; the per-capability choice is macOS-only

Status:   claimed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-25, by Yashraj comparing platforms: "I cannot agree or
          disagree to selected terms rather than agreeing everything".
Evidence: consent_for_session_grants: the rich per-capability window is
          macOS-only by decision (2026-07-23, "a later P3-OPEN slice");
          Windows double-click falls back to one rfd::MessageDialog with
          Allow-all/Cancel (main.rs ~9390). The permission wall's whole
          pitch is informed, granular consent -- on the OS most users run,
          it is a yes/no box.
Fix:      a native Win32 dialog built in-process (DialogBoxIndirect or a
          drawn window through the adapter): one checkbox per capability
          with the same plain-words rationale the terminal prompt prints,
          optional capabilities pre-unchecked exactly like macOS. The
          macOS invisible-checkbox lesson applies: verify by pixels on the
          real PC before shipping.

### K-175 -- ui.dropzone is declared, consent-worded, and recommended -- and does not exist

Status:   claimed
Owner:    claude
Severity: serious
Class:    runtime-hole
Found:    2026-08-25, claude, building the capability coverage matrix
          (Plan/Capability-Coverage-2026-08.md).
Evidence: `grep -rn dropzone crates/ wit/` -- hits in manifest validation
          (mime resource), consent wordings (authoring_context.rs:451 "accept
          dragged files", tui.rs:1315 "accept files you drag onto it"), and
          the PORT ANALYZER which actively recommends declaring it
          (port/src/lib.rs:1060). Zero hits in wit/, zero host functions,
          zero DroppedFile/HoveredFile handling in any adapter. An app that
          declares it puts a promise on the consent sheet the runtime cannot
          keep -- the exact hollow-permission shape K-086 was about.
Fix:      implement it: winit DroppedFile/HoveredFile -> a phase3 event apps
          can poll/receive, mime-filtered by the declared scope, behind the
          existing wall; six gates + pack teaching + an example. Until it
          lands, the port analyzer must stop recommending a capability that
          does not work.

### K-174 -- The Studio setup shows NSIS's raw "Error opening file for writing" when the engine is running

Status:   open
Owner:    unclaimed
Severity: annoyance
Class:    our-code
Found:    2026-08-25, by Yashraj running the v0.1.56 setup on the Windows PC
          while a build (driven over SSH) had krate.exe open.
Evidence: screenshot of the PC: "Krate Setup -- Installing... Extract:
          bin\..." with the dialog "Error opening file for writing:" over
          bin\krate.exe; tasklist showed two running krate.exe processes
          holding the file. Windows locks a running exe, so any open Krate
          app or in-flight build makes the setup fail with NSIS's rawest
          error and no hint of the fix.
Fix:      the same lesson K-165 taught the CLI installer, applied to the
          NSIS setup: before extracting, detect running krate.exe /
          krate-studio.exe and say in plain words "close your Krate apps
          first", with a retry -- a Tauri NSIS installer hook can run the
          check. Unclaimed.

### K-173 -- A failed session resume killed the whole build instead of falling back

Status:   claimed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-25, by Yashraj in the Studio: "make a NES Ice Climber game"
          planned fine, then v1 failed with "that build didn't come together".
Evidence: ~/.krate/studio/builds/s-1787590860642/nes-ice-climber/
          .agent-transcript.txt: `No conversation found with session ID:
          f9985b9f-...` and a result JSON with duration_ms 0, num_turns 0.
          The planning session's transcript WAS on disk and the same id
          resumed fine minutes later from a foreign directory -- the Studio
          fires the build so soon after planning that claude's just-written
          session was not yet visible to the resuming process. A race, so it
          will recur; the defect is that the resume failure was FATAL.
Fix:      a resume is an optimization and must never cost someone their
          build: run_provider_author now retries fresh, once, when a resumed
          run fails (the session id was taken, not peeked, so the retry
          cannot loop). Plus adopt_session(): claude's per-directory session
          store gets the planning/previous-workspace transcript copied into
          the new workspace's project dir before the resume, best effort.
          Proven by a create with a deliberately bogus KRATE_PLAN_SESSION
          building to completion through the fallback.

### K-172 -- The Studio dmg shipped carrying an app the notary REJECTED, under a log line saying it was notarized

Status:   claimed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, by Yashraj downloading the Studio on a Mac: Gatekeeper's
          "Apple could not verify" malware dialog with Move to Bin.
Evidence: v0.1.55 release job 97487260897: the Player's notarization printed
          `status: Accepted` and stapled; the Studio's printed
          `status: Invalid` followed by `The staple and validate action
          failed! Error 65.` and then, one line later, `studio notarized and
          stapled` -- and the job concluded success and published the dmg.
          Two causes stacked: (1) signing is a hand-kept file list, and
          6a53e3cf added cargo-component into Resources/bin without anyone
          adding a codesign line, so the notary found an ad-hoc Mach-O;
          (2) `notarytool submit --wait` exits 0 for a REJECTED submission,
          and the workflow judged by exit code.
Fix:      release.yml: sign_bundle() discovers and signs every Mach-O in a
          bundle (plus everything in Contents/MacOS, where the notary checks
          scripts too) and re-runs the ad-hoc check over the whole bundle;
          notarize() judges by the `status:` verdict, prints the notary's own
          log on a rejection, and a rejected Player zip or Studio bundle now
          FAILS the release instead of shipping. Verified by re-dispatching
          the release for v0.1.55.

### K-171 -- A newer Studio can sit on top of a years-old engine in Krate\bin

Status:   open
Owner:    unclaimed
Severity: serious
Class:    our-code
Found:    2026-08-24, claude, surveying the Windows PC before updating it to
          v0.1.54
Evidence: on the PC, the uninstall registry said `Krate 0.1.53` installed at
          `C:\Users\user\AppData\Local\Krate`, but
          `C:\Users\user\AppData\Local\Krate\bin\krate.exe --version` printed
          `krate 0.1.28`. The Studio resolves its engine by sibling-then-bin,
          so a 0.1.53 Studio on that machine could have been driving a 0.1.28
          engine -- the rc18 stale-engine failure shape, but produced by our
          own installer leaving `bin\krate.exe` behind. Running
          `irm krate.tech/install.ps1 | iex` printed "Updating krate 0.1.28 ->
          v0.1.54", confirming bin held the old binary until the CLI installer
          replaced it.
Fix:      two candidate truths, both defects. Either (a) the Studio installer
          left an old engine in `bin\`, or (b) -- more likely -- that
          `krate.exe` was a SOURCE build copied there during testing: source
          builds report the workspace placeholder `0.1.28` because only
          release CI stamps KRATE_RELEASE_VERSION from the tag. Under (b) the
          bug is that a source-built engine misreports its identity, so
          nobody -- installer, Studio, or human -- can tell a fresh build
          from an ancient one by version string. Stamp source builds with
          the git describe output, and make the Studio refuse or warn when
          its engine's version does not match its own. Unclaimed -- found in
          passing while updating the PC.

### K-169 -- Windows and Linux rendered every canvas at 1x and stretched it: the "broken low quality pixels"

Status:   fixed (proven: the desktop screenshot after the fix is visibly
          native-density -- text edges and piece shading match the Mac)
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, the founder, comparing the same app on both machines: "graphics I see in macos are really good, but these are like broken low quality pixels"
Evidence: the K-088 contract rasters every canvas at the display's density
          (window_scale) while apps keep logical units. macOS, iOS and
          Android override window_scale; the Windows and Linux adapters
          never did, so the trait default of 1.0 stood in -- every canvas
          rasterized at logical resolution and was linearly stretched 1.5x
          across the founder's 150% display. The provider-shape trap again:
          wired on the platform we develop on, silently defaulted on the
          twins, and invisible to every headless check because shoots pick
          their own scale.
Fix:      both winit adapters override window_scale from the live window's
          scale_factor (native_window_scale in winit_native). The canvas
          then rasters at physical resolution and the placement blit is 1:1,
          exactly as K-088 intended everywhere.

### K-170 -- A full-bleed window on Windows could not be moved or maximized at all

Status:   fixed (proven by scripted input on the real desktop: a band drag
          moved the window; a double-press maximized it to the full work
          area. Two rounds of refinement were needed and are worth keeping:
          the drag must start on the first cursor MOVE, not on the press --
          drag-on-press ate the second click and maximize could never fire --
          and the K-167 resize clamp must exempt maximized/fullscreen
          windows, because it un-maximized every maximize one resize later.)
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, the founder: "I cannot drag that window to full screen"
Evidence: undecorated windows have no title bar to grab; K-168 restored the
          close and minimize buttons but the window itself stayed nailed to
          wherever it opened, at whatever size. macOS full-bleed windows keep
          a transparent title band that drags and double-clicks to zoom; the
          winit platforms had nothing.
Fix:      the top 36 logical pixels of an undecorated window act as the
          title bar: press starts an OS window drag, double-press toggles
          maximize, and a consumed press swallows its own release so the app
          never sees half a click. The overlay control cluster keeps
          priority. Also: the K-167 size clamp now re-applies on every
          Resized, because winit's DPI settling regrew the window past the
          screen edge after the creation-time clamp had fired.

### K-168 -- A full-bleed app on Windows has no close or minimize button at all

Status:   fixed (proven on the PC's real desktop: the overlay buttons render
          top-right of the frameless chess window, and a synthetic click on
          the drawn close button exited the app -- "clicked close at 1912,46
          exited=True")
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, the founder, double-clicking the chess app: "opens without window like close and minimize buttons"
Evidence: full-bleed on Windows and Linux is an undecorated window
          (set_decorations(false), the K-117 decision) -- which removes the
          entire title bar, buttons included. macOS overlays its traffic
          lights on the app's drawing, so a full-bleed Mac app stays
          closable; the same app on Windows was a bare rectangle a person
          could only leave through Alt-F4. The pack teaches full-bleed, so
          every polished generated app inherits the trap.
Fix:      the adapters now draw their own minimize and close controls over
          the top-right corner of any undecorated window, and eat clicks in
          that cluster before the app sees them (close feeds the normal
          CloseRequested path, K-121 two-strike included). One geometry
          source (adapter-common::overlay) feeds the hit test, the CPU
          painter's sprite blend, the vello scene path, and the canvas
          fast path's corner composite, so what is drawn is what clicks.

### K-167 -- Windows creates app windows bigger than the screen, bottom hanging off it

Status:   fixed (proven on the PC's real desktop: the same chess bundle now
          opens at 1918x992 physical at (24,24) -- fully on a 1080p screen --
          with every rank, file, and panel visible)
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, the founder's chess app on the Windows PC ("weird aspect ratio, not responding"), reproduced on its real desktop by scheduled-task probe
Evidence: KRATE_EVENT_TRACE on the desktop:
          "created physical=1920x1260 scale=1.5 logical=1280x840
          requested=1280x840" -- the app asked for 1280x840 logical, the
          display is 1080p at 150%, and Windows created a 1920x1260-physical
          window without complaint: 240 pixels of it below the bottom of the
          monitor. The board's lower ranks were literally off screen, which
          read as a wrong aspect ratio and dead clicks. macOS constrains
          windows to the screen's visible frame by itself, which is why the
          same bundle was perfect there; headless never has a screen to
          overflow. A first diagnosis (silent OS clamp with no Resized) was
          WRONG -- an early probe's GetWindowRect came back DPI-virtualized
          from a non-DPI-aware PowerShell and mimicked a clamp. The trace is
          the authority.
Fix:      both winit adapters constrain a new window to the current monitor
          (less a conservative title-bar/taskbar allowance, since winit
          exposes no work area), pull it to the top-left when clamped, and
          synthesize the same Resized event a drag produces so the host
          relayouts and the app refits. crates/adapter-windows and
          crates/adapter-linux, winit_native.rs. The create/resize trace
          stays, behind KRATE_EVENT_TRACE.

### K-165 -- Updating Krate on Windows while an app is open dies with a raw stack trace

Status:   fixed
Owner:    claude
Severity: annoyance
Class:    our-code
Found:    2026-08-24, claude, running install.ps1 on the Windows PC while a Krate app was open
Evidence: C:\krate\install-cold.log: "Copy-Item : The process cannot access the
          file 'C:\...\Krate\bin\krate.exe' because it is being used" plus a
          PowerShell position stack -- the least helpful possible words for
          "close your Krate apps first". Windows locks a running exe, so any
          update attempted while an app (or the Studio's engine) runs hits it.
Fix:      the copy catches IOException and says plainly: close any open Krate
          apps and Krate Studio, then run the installer again.
          scripts/install.ps1.

### K-166 -- The CLI installer and the Studio fight over who owns .krate

Status:   fixed (and the damage class is now self-healing: the founder's
          double-click kept launching a DELETED cold-test install's
          krate.exe -- oversized window, no studio -- because the studio's
          registration was marker-gated to run once and could never
          re-assert. The Windows setup now runs on EVERY launch (a dozen
          cheap reg writes; UserChoice still outranks it), repairs a
          Krate.Bundle whose target exe no longer exists, and broadcasts
          SHChangeNotify so the live Explorer drops its cached association
          -- without that broadcast the shell kept using the dead command
          even after the registry was corrected. Proven on the PC: with the
          extension deliberately hijacked to a dead ProgId, one studio
          launch healed both keys, and the double-click chain is
          studio -> bin\krate.exe run --consent again.)
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, claude, cold-installing v0.1.53 on the PC where the Studio was live
Evidence: after install.ps1, reg query HKCU\...\.krate showed Krate.Bundle
          pointing at the CLI's krate.exe -- the Studio's Krate.App association
          silently stolen. The Studio's own first-run does the same theft in
          reverse (its K-158 comment even celebrates it). A person with both
          installed gets whichever registered last, and a CLI installed to a
          temp dir leaves double-click pointing at a path that may be deleted.
Fix:      install-krate-desktop.ps1 keeps an existing Krate.App association
          when its opener actually exists on disk (Krate.Bundle still lands in
          OpenWithProgids so "Open with" always offers it), and uninstall only
          removes the extension key when it points at Krate.Bundle. The PC's
          association was restored to the Studio by hand.

### K-164 -- Every store download failed on the released runtime for days, and nothing said so

Status:   fixed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, claude, chasing the nightly cold-install failure
Evidence: v0.1.52 run of krate.tech/notes.krate and every evidence/store
          bundle: "this app and this copy of Krate were built against
          different versions of the app interface". The bundles were packed
          before the camera WIT change (08513eb8); the notes-v0.1.0 release
          asset dated 2026-07-28. The nightly cold-install had been red for
          five straight days. Three separate holes let it happen and stay:
          (1) nothing repacks the served bundles when the WIT changes,
          (2) pages.yml did not list evidence/** in its trigger paths, so even
          a repack commit would not redeploy the site,
          (3) the release gate ran notes.krate only WITHOUT grants -- the
          refusal happens before instantiation, so the gate passed while the
          app could not actually start.
Fix:      all bundles repacked with scripts/pack-store-apps.sh and verified to
          run on the v0.1.52 binary before committing; notes-v0.1.0 asset
          re-uploaded; evidence/ported/** and evidence/store/** added to the
          pages triggers; release gate gained a with-grants run of the site
          sample so drift now fails the release with instructions to repack.

### K-160 -- A stale mid-run refusal outranks the working app the agent delivered

Status:   fixed (proven: the v0.1.53 PC ladder ran four grok builds -- tip
          calculator, habit tracker, brick breaker, photo booth -- and grok
          typed the marker mid-run in ALL FOUR; every app was delivered,
          passed check-app, and was kept, with the note quoting the doubt.
          Without the fix the entire ladder would have been thrown away.
          The prompt now also forbids the marker outside the refusal file
          and corrects the capabilities grok kept doubting.)
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, claude, grok create "a unit converter for cooking measurements" on the Windows PC
Evidence: C:\krate\grok-test.log ends "error: Krate cannot build that: ...cannot
          access external data sources like unit conversion tables" while the
          transcript's final message says the app is finished and "check-app
          printed **OK**". Grok wrote the KRATE-CANNOT-BUILD marker mid-run,
          reconsidered, built a passing app -- and the rfind over the
          transcript still failed the create and threw the app away.
Fix:      agent_refusal is only honored when there is no delivered app:
          if src/lib.rs differs from the starter AND check_app_verdict passes,
          the run keeps the app and prints a note quoting the stale refusal.
          crates/cli/src/main.rs (create flow, refusal check).

### K-161 -- Codex's broken sandbox helper reads as "the agent explained instead of writing"

Status:   fixed
Owner:    claude
Severity: annoyance
Class:    our-code
Found:    2026-08-24, claude, codex create on the Windows PC
Evidence: codex 0.147.0's install has no codex-windows-sandbox-setup.exe, so
          every tool call fails ("orchestrator_helper_launch_failed ... program
          not found") and codex exits 0 with nothing written. The probe
          (`krate ai --json`) diagnoses it correctly ("not-ready ... reinstall
          the Codex CLI"), but an explicit `--agent codex` create bypasses the
          probe and died with the generic "byte-identical to the blank
          skeleton ... the agent explained the app instead of writing it".
Fix:      when the app comes back untouched, the transcript is scanned with the
          provider's output_failure first, so the create names the sandbox
          breakage and the reinstall remedy instead of blaming the agent's
          prose. crates/cli/src/main.rs.

### K-162 -- The Studio's PATH append could mangle or duplicate the user's PATH

Status:   fixed
Owner:    claude
Severity: serious
Class:    our-code
Found:    2026-08-24, claude, HKCU\Environment on the Windows PC
Evidence: reg query HKCU\Environment /v Path showed
          "...\Microsoft\WindowsAppsC:\...\Krate\bin;C:\...\Krate\bin" -- one
          copy fused onto the previous entry with no semicolon (breaking that
          entry) and a second appended after it. The check was a raw substring
          `contains`, wrong in both directions: it sees the dir inside a
          mangled entry and skips, or misses over a case difference and
          appends again. The reg query also ran without CREATE_NO_WINDOW
          (K-159 class).
Fix:      element-wise, case-insensitive comparison of semicolon-split PATH
          entries; append joins with an explicit semicolon and trims a
          trailing one; query goes through silent_cmd. Verified on the PC:
          clean PATH gains exactly one well-formed entry, a forced second
          setup run adds nothing. studio/src/main.rs.

### K-163 -- --shoot can only see frame one, so nothing that develops later can be verified

Status:   fixed
Owner:    claude
Severity: annoyance
Class:    our-code
Found:    2026-08-24, claude, verifying the webcam app on the Windows PC
Evidence: krate run --shoot of webcam3.krate always captured "WAITING FOR
          CAMERA" -- the capture fires at the app's first wait(), before the
          first camera frame can possibly arrive. The same blindness applies
          to anything fetched, animated, or streamed.
Fix:      KRATE_SHOOT_AFTER_MS holds the capture back that many milliseconds
          (close still captures unconditionally so short runs yield their
          final frame). With 4000, the same app shoots a live webcam frame,
          which is how camera-on-Windows was finally proven end to end.
          crates/runtime/src/phase3_gui_host.rs.

### K-154 -- The resize check judged apps before they had redrawn

Class: our-code
Owner: claude (improved, not eliminated -- see below)

`ios.krate` failed the resize check on roughly one run in three with "the
window grew but the app kept drawing at its old size", and passed the other
two. The app resizes correctly; the check was judging it too early.

The wait was one extra VISIT -- a turn counter, not time -- and a visit can
land in microseconds. Same defect class as K-143, which was the click check
judging the frame before the app had repainted.

Now the verdict waits for `RESIZE_SETTLE` (250ms), a visit from `wait` where
the app's turn is finished, AND for the render surface to have caught up with
the canvas rect. Two false starts on that last condition are worth recording:

- Comparing the canvas rect to the WINDOW never matches, because a canvas is
  inset from its window. Every run fell through to the give-up path, which is
  the race again, just later.
- Comparing the rect to its own old size passes as soon as the LAYOUT reflows,
  which happens before the app redraws. The verdict then read the old render
  size anyway.

Only the render size proves the app itself caught up, because that is the thing
the verdict actually judges.

**Measured honestly: 29 of 30 runs pass, against roughly 2 in 3 before.** One
failure in thirty remains and this entry stays open because of it. The
remaining window is likely between the render surface resizing and the app's
next present; catching that needs a signal from the present path rather than a
poll of the surface. No other app regressed -- weather-dashboard, snake,
nes-game, pulse, mark-replica and the tip calculators all still hold.

### K-151 -- Animating apps hold gigabytes; static ones hold 130 MB

Class: our-code
Owner: unclaimed

Not the K-127 runaway (that reached 46 GB and never came back). This one is
reclaimed -- memory rises and falls -- but the working set is enormous for what
these apps draw, and a demo machine under it will stutter.

Measured 2026-08-21 on macOS, windowed, through the signed app bundle:

    static apps
      tip-calculator 3        127 MB
      todo-list-check-off     131 MB

    continuously animating apps
      pulse                  2274 MB
      nes-game               2423 MB
      snake-game-play        1598-4327 MB, oscillating over 150s

Snake sampled every 15s: 1990, 1765, 1928, 4245, 4327, 3656, 3162, 2434, 2218,
1598. So it is a sawtooth -- allocation outruns reclamation for a while, then
the pool drains. Nothing grows without bound, which is why this is a
performance bug rather than the crash K-127 was.

A snake game holding two gigabytes is wrong on its face. The suspect is the
per-frame allocation in the canvas present path: static apps draw once and
settle at 130 MB, and the only difference in the animating ones is that they
present every frame.

Worth fixing before any performance claim is made in public, and worth knowing
before a live demo: static apps are safe, an animating one may make the machine
work hard.

### K-149 -- Building Krate on Windows needs LLVM, and nothing says so

Class: environment (the machine) / our-code (the silence about it)
Owner: claude (in progress)

A clean Windows 11 machine with rustup and the Visual Studio Build Tools --
everything our own docs ask for -- cannot build Krate. The build runs for
several minutes, compiles most of the workspace, and then dies:

    error: failed to run custom build command for `whisper-rs-sys v0.15.0`
    Unable to find libclang: "couldn't find any valid shared libraries matching:
    ['clang.dll', 'libclang.dll'], set the `LIBCLANG_PATH` environment variable"

`whisper-rs-sys` runs `bindgen`, which needs `libclang.dll`. The VS Build Tools
VCTools workload does not include Clang, so a person following our setup ends
up with a toolchain that looks complete and is not.

This is the same crate already recorded in `crates/runtime/Cargo.toml` as the
reason arm64 Linux was missing from every release ("whisper.cpp needs a C++17
compiler; the arm64 Linux cross container reaches only C++14"). One optional
feature, `speech`, is now the single hardest dependency on two platforms.

It is TWO missing prerequisites, not one. After installing LLVM the build got
further and died again, in the same crate:

    error: failed to run custom build command for `whisper-rs-sys v0.15.0`
    is `cmake` not installed?

So a clean Windows machine needs LLVM *and* CMake beyond what our setup asks
for, and finds out one failure at a time, several minutes apart.

Done: `krate doctor` now has a "Building Krate itself" section on Windows that
checks for `libclang.dll` (honouring `LIBCLANG_PATH`, falling back to the
default LLVM location) and for `cmake` on PATH, and prints the `winget` command
for whichever is missing. Stated as information, not a fault -- running apps
and `krate create` need neither; only building the runtime from source does.

**Now three platforms.** The aarch64 Linux cross-build in CI fails on the same
crate for a third reason -- its Ubuntu 16.04 container ships libclang 3.8,
bindgen needs 9+, and `apt.llvm.org` is unreachable from that image:

    E: Unable to locate package libclang-9-dev
    FATAL: clang 9 did not install; Ubuntu 16.04 ships libclang 3.8,
    which bindgen cannot use.
    ./whisper.cpp/ggml/include/ggml.h:211:10: fatal error: 'stdbool.h' file not found

So `whisper-rs-sys` has now blocked arm64 Linux (C++17), Windows (libclang +
cmake), and aarch64 Linux cross-builds (libclang version). Every one of those
is one optional feature that most apps never touch.

**Decided 2026-08-20: speech is now opt-in.** `default = []` for `krate-cli`
and `default = ["phase2-bindings"]` for `krate-runtime`, so an ordinary build
needs nothing but rustup -- which is what our setup docs have always promised
and, until now, was not true on any of those three platforms.

Released binaries are unaffected: the release workflow passes
`--features speech`, so what a person downloads still transcribes. The logic
inverted rather than disappeared -- a release ADDS speech where the toolchain
allows it, instead of a broken platform SUBTRACTING it. Verified both ways:
`cargo build --release -p krate-cli` and `--features speech` both succeed.

The CI job that built "without speech, the way arm64 Linux ships" now tests
the DEFAULT configuration rather than an odd one out, and says so.

Found on the Azure parity VM (see K-148). Verified fixed by installing
LLVM 18.1.8 and setting `LIBCLANG_PATH`; the build then proceeds.

**A Mac cannot lint the other platforms, and this is why CI matters.** Two
defects in the parity work were invisible here and only CI could see them:

- `default_constructed_unit_structs` on the nokhwa backend. The identical lint
  was fixed on the macOS backend days earlier, but the nokhwa branch sits
  behind `cfg(windows/linux)`, so local clippy never compiles it.
- `interface-parity.md` is generated from the WIT and verified with
  `git diff --exit-code`, so adding `krate:camera` left the committed copy
  stale.

Cross-checking does not substitute: `cargo clippy --target
x86_64-pc-windows-msvc` dies on the C dependencies (ring, zstd) long before
reaching the camera code. So for anything behind a platform `cfg`, CI's runners
are the only place a lint or a test can fire -- which is the same lesson K-150
taught about `--lib` never compiling binaries.

### K-148 -- Windows and Linux are behind macOS, and nothing measures the gap

Class: runtime-hole
Owner: unclaimed

Krate ships six platforms but only macOS is developed against daily. The gap is
now big enough to be a product risk rather than a backlog item, and no check
anywhere reports it -- it is found app by app, on a person's machine.

Measured 2026-08-20, before the work:

    adapter-macos    5800 lines
    adapter-linux    2283 lines
    adapter-windows  2209 lines

Line count turned out to be a poor proxy. Counting the UiAdapter/WindowAdapter
methods each adapter actually implements, after the parity work:

    macos    79 distinct fn
    windows  80 distinct fn
    linux    80 distinct fn

macOS is larger because AppKit needs more code per feature, not because it does
more. The two REAL gaps were camera and full-bleed, and both are now closed.

Confirmed missing on Windows (each degrades honestly, so nothing "breaks" --
apps just quietly do less):

- ~~**camera.capture**~~ -- NO LONGER a gap in code. Corrected 2026-08-27:
  `platform_backend()` returns `NokhwaCameraBackend` for BOTH Windows
  (`input-msmf`, Media Foundation) and Linux (`input-v4l`, V4L2), shipped in
  021f19e01 (K-148) after this line was written. The dependency is declared
  per target in crates/runtime/Cargo.toml, so it compiles into both builds.
  UNVERIFIED AGAINST REAL HARDWARE on either system: there is no evidence run,
  and the code path has only ever been reasoned about, never pointed at a
  physical webcam. Treat "implemented" and "works" as separate claims until a
  device test exists. This entry read "returns None" for six days after it
  stopped being true, which is why a stale gap list is worse than none.
- ~~**ui.clipboard**~~ -- NOT a gap. Corrected 2026-08-20: the Windows adapter
  implements clipboard through `arboard`; the `Unsupported` arms are the
  `#[cfg(not(target_os = "windows"))]` fallbacks, which is the opposite of what
  a grep for "Unsupported" suggests. Counting error messages is not an audit.
- **set-full-bleed** -- no implementation, so the trait default refuses. Every
  full-bleed app (the pack encourages them) gets standard chrome instead.
  Recorded under K-117 as still open for Windows and Linux.

What is NOT platform-split, and should be verified rather than assumed:

- K-146 (check-app refuses an optional defining capability) -- pure Rust in the
  CLI, applies everywhere.
- K-147 (a camera frame carries its own width and height) -- a WIT shape
  change, applies everywhere.
- K-137 (the in-flight fetch cap) -- platform-free runtime code.
- K-140/K-143 (the click check's idle-churn threshold and settle timing) --
  platform-free, but the *input path* it measures is per-adapter, so the check
  passing on macOS says nothing about Windows.

K-141 is the warning here: a full-bleed window's clicks landed a title-bar
height off on macOS because the input path flipped coordinates against the
wrong rect. Windows and Linux have their own input paths and could carry the
same class of bug independently. Nobody has looked.

**First real Windows run, 2026-08-20.** A Windows 11 24H2 VM was stood up on
Azure (see [[krate-azure-winvm]] in memory; resource group `krate-parity`) and
everything below was measured there, not inferred.

What WORKS on Windows, verified:

- `krate.exe` builds (33.4 MB) and runs; `--version` and `doctor` both fine.
- `krate create` builds a real app end to end -- `tip.krate`, 23036 bytes,
  packed and permission-wall verified. It correctly fell back to the checklist
  template with a clear explanation, because no AI agent is installed there.
- The app RUNS: window opens, `items:5 saved:yes`, and the missing GPU
  degrades honestly ("Microsoft Basic Render Driver is a software adapter;
  drawing on the CPU").
- The whole usability script passes on a real full-bleed canvas app:
  stay-open, resize and click all `held`.
- **The design-space mapping is identical to macOS.** Same numbers, same
  letterbox: `canvas=1140x780 design=920x640 k=1.219 off=(9.4,0.0)`, and the
  click check answered `difference=0.016182 idle_churn=0.000068` -- a real
  reaction told apart from the animation. K-140, K-143 and K-147 are
  genuinely cross-platform, not macOS-shaped fixes that happen to compile.

What was still missing, and is now closed (2026-08-20):

- **camera** -- a nokhwa backend now serves BOTH Windows (Media Foundation)
  and Linux (V4L2) behind the same `CameraBackend` trait macOS uses, so one
  file closes both gaps instead of two bodies of unsafe code. Compiles on the
  Windows VM (`EXIT=0`). It cannot be proven end to end there -- an Azure VM
  has no camera hardware, confirmed with `Get-PnpDevice` -- so `unavailable`
  remains the correct answer on that machine. Pinned instead by
  `every_desktop_platform_has_a_camera_backend`, which fails if any desktop
  platform loses its backend.
- **set-full-bleed** -- implemented for Windows and Linux as an undecorated
  winit window, returning the new size so the canvas refits rather than
  leaving stale pixels where the frame was.

  Writing the test found a second, larger hole: `discover_ui_adapter()` returns
  the DRAFT adapter, which had no `set_full_bleed` at all and so inherited the
  trait default -- which refuses. The draft leg accepts on macOS, so the
  refusal was unique to Windows and Linux, and silent, because an app ignores
  the error and carries on. Both now delegate to the draft explicitly.

One difference is deliberate and worth stating: macOS keeps its traffic lights
and overlays them on the app's drawing. Windows has no equivalent overlay, so
an undecorated window loses its buttons; the alternative is extending the
client area into the frame and hit-testing the caption by hand, which is a lot
of surface to get wrong for a cosmetic gain. On Linux the window manager owns
the frame, so this is a request -- most compositors honour it, and a tiling one
that draws no decorations was already full-bleed.

Two blockers found on the way are their own entries: K-150 (main had not
compiled for Windows since v0.1.51) and K-149 (LLVM and CMake are undocumented
build prerequisites).

Cross-checking from a Mac is NOT a substitute: `cargo check --target
x86_64-pc-windows-msvc` works for pure-Rust crates but cannot get past the C
dependencies (zstd, sqlite, whisper) without an MSVC toolchain, so it never
reaches the code that was broken.

### K-144 -- A quick run can print its results as a literal, and nothing notices

Class: teaching-hole
Owner: unclaimed

codex's countdown timer ends its quick run with:

    let _ = out.write(
        b"duration:300\nstarted:yes\nreset:yes\nremaining_at_reset:300\nremaining:297\n",
    );

Every number is a constant. The app never reports what actually happened, and
the quick run passes whatever the app does -- it would print `started:yes`
with the Start button removed entirely.

This is worse than a missing check, because it is a check that reads as passing
while measuring nothing. It cost real time here: the hardcoded output was taken
as proof the timer worked, and the investigation looked at the host for it.

The pack asks for quick-run output and gives the format, but never says the
values have to be read back out of the app's own state at the moment of
printing. Say that, and say why: a literal here is indistinguishable from a
working app until somebody clicks the button by hand.

### K-138 -- Generated apps pick a fixed design size and then cannot be scrolled

Class: teaching-hole
Owner: unclaimed

`weather-dashboard.krate`, authored by claude on 2026-08-20, handles no wheel
events at all -- there is no `Event::Wheel` arm anywhere in its 2279 lines. The
founder's report was "not clickable or scrollable"; the scrolling half is real
and this is why.

The cause is a decision the pack never covers. The app calls
`canvas2d::set_design_size(canvas, 920x640)` and lays every control out at fixed
coordinates inside that box. Having done so, the AI reasoned there was nothing
to scroll -- the design never overflows itself -- and skipped wheel handling
entirely. The host then letterboxes 920x640 into whatever window the person
opened, so on a real screen the content is scaled and a scroll gesture does
nothing at all.

The pack teaches scrolling well (authoring_context.rs:744-772: offsets in
pixels, clamp, subtract from hit-testing, clip the region) but conditions the
whole lesson on "a list that outgrows the window". An app with a fixed design
size never outgrows its own box, so the lesson reads as not applying. Nothing
in the pack says what a design size costs, or that a person will still try to
scroll a window that looks like a dashboard.

Evidence:

    $ grep -n "Wheel" wd-src/source/src/lib.rs
    (no output)

    $ grep -n "Wheel" ios-src/source/src/lib.rs
    2997:                    Some(types::Event::Wheel(w)) => {

Two apps, same author, same session, same prompt shape: one handles the wheel
and one does not. That inconsistency is the teaching hole, not chance.

The fix is pack-side: say when a design size is the right call and when it is
not, and say that a fixed design size does not excuse an app from handling the
wheel.

### K-135 -- The plan gate only read claude's shape, so grok and codex could never plan

Class: our-code
Owner: claude (fixed in 5f9fac32, pending release)

The Studio's first step on any request is `krate plan`, which asks the agent
whether to build or ask questions and expects one JSON object back. The parser
(`extract_plan_json`, born in c9b648e0) understood only bare JSON on stdout --
which is what `claude -p` emits. Every other provider frames its answer:

    bare     {"ask":[...]}                                  claude -p
    envelope {"text":"{\"ask\":[...]}", ...}                grok --output-format json
    stream   {"type":"item.completed","item":{"text":...}}  codex exec --json, one per line

grok and codex both answered CORRECTLY and both failed with "the AI did not
answer in the expected shape", instantly, on every request. Reproduced live on
the Windows PC (v0.1.46) after selecting each:

    krate plan "a simple stopwatch" --agent grok
      -> {"text":"{\"plan\":\"A desktop stopwatch...\",\"needs\":[]}", ...}  rejected
    krate plan "a simple stopwatch" --agent codex
      -> {"type":"item.completed","item":{"text":"{\"plan\":...}"}}          rejected

Only claude overrides plan_args (bare `-p`); grok, codex, gemini, copilot all
fall through to author_args, which uses a JSON output format. So the gate worked
for exactly one of five agents.

Fixed by parsing every balanced object in the output and searching recursively,
descending into strings that are themselves JSON. Tests carry the verbatim grok
envelope and codex stream from the failing sessions.

This is the clearest case of the "built on assumptions" pattern: one provider's
behavior (claude emits bare JSON) was taken as every provider's, and shipped
without testing the others. See the note in DEVELOPMENT once written.

### K-112 -- Windows presents frames on the CPU: visibly slower than the Mac side by side

Class: our-code
Owner: unclaimed

The Windows adapter rasterizes with the shared CPU painter and presents
through softbuffer; macOS runs the native adapter. The same game (krate-nova)
run on both machines at once is visibly smoother on the Mac, and with the
DPI fix the Windows buffer grew scale^2 larger, which raises the CPU cost
further on scaled displays. Reported by real users, 2026-08-15.

Fix shape: the wgpu/vello presenter behind the same placement contract
(plan 7.5 names it). The placement contract is already in place, so this is
a presenter swap, not a redesign. Until then Windows is correct but not
proud.

Update:   2026-08-17, f2e3aad4: the CPU fallback halved. to_image went
          from per-byte pushes to vectorizable chunked writes (4.3ms ->
          0.34ms/frame), draw_image scales opaque sources by packed-row
          memcpy, and wait's park slices clamp to the deadline. Windows
          VM (2 cores, no GPU): 73% of a core on v0.1.30 -> 38% on main,
          50fps sustained. evidence/perf/2026-08-17-canvas-cpu-path.md.
          The GPU presenter remains the real fix for parity with the Mac.

Update:   2026-08-17, live debugging on the founder's Iris Xe PC over
          ssh: three fixes (pump repainted per event check 6f73ece5;
          swapchain double-clocked the publish f426cff7; canvas frames
          re-uploaded through the scene pipeline ae3e4316) took the same
          game from under 2.4fps with three cores pinned to side-by-side
          play parity with the M4 Mac. Publish sync still p50 12-14ms on
          that machine; that remainder is this workstation's real GPU
          presenter work.

### K-108 -- dead space cannot tell an editor from an app that stopped short
Status:   open
Owner:    unclaimed
Severity: minor
Class:    runtime-hole
Found:    2026-08-13, while fixing K-099. Named rather than papered over.
Evidence: krate-notes reports 38.9% of its window empty, the highest of the
          21 apps measured, and it is not a defect: the empty part is the
          editor below a short note, which is where you type.

              $ krate run krate-notes.wasm --shoot n.png --check-layout
              layout: nothing is drawn in 39% of the window ...

Why not fixed with K-099: an editor with room left in it and an app that
          stopped drawing halfway down emit the same draw calls -- content
          at the top, nothing below. Two rules were tried against all 21
          measured apps and neither separated them:

            "content above the region"      also suppressed the real defect
                                            in `the_bottom_half_left_empty`
            "inside a drawn container"      finds no container: krate-notes
                                            draws its editor as bare
                                            background with text on it, and
                                            its only large rect is the
                                            sidebar, which does not contain
                                            the empty region

          A canvas app has no widget kind to ask, so the information is not
          in the draw list at all.
Mitigation: the finding is worded as what was measured rather than as a
          verdict, surfaces as a note and never a failure, and the false
          positive is a test that asserts the wrong answer on purpose
          (`an_editor_with_room_to_type_is_reported_and_should_not_be`).
          If that test starts failing, the limit was fixed and this closes.
Fix:      Probably needs the guest to say so -- a way to mark a region as
          "somewhere a person fills", which the host then excludes. That is
          a WIT change and wants a real second use case before it is worth
          one.

### K-107 -- text drawn over shapes is not detected, only text over text
Status:   open
Owner:    unclaimed
Severity: moderate
Class:    our-code
Found:    2026-08-13, while fixing K-106. Named rather than quietly widened.
Evidence: The memory game from K-106 draws its hint across the bottom row of
          cards, and `--check-layout` says:

              $ krate run req-26/app.krate --shoot mem.png --check-layout
              layout: no text drawn over other text

          Correct for what the check measures, and useless to the person
          looking at the overlap in the screenshot.
Why not fixed with K-106: text over a shape needs a way to tell a background
          apart from content. Text on a card, a panel, a gradient or a
          button is the single most common thing an app draws and is always
          right; text on a card that is content -- a playing card, a photo,
          a chart bar -- is wrong. The draw list alone does not distinguish
          them, so the naive version would fire on nearly every app and the
          check would be turned off.
Fix:      Probably: treat a shape as content when text is drawn over it that
          belongs to something else, i.e. use z-order and containment rather
          than shape kind. A label positioned inside its own panel is fine;
          a string that crosses several sibling shapes of the same size is
          the memory game. Needs calibrating against the same seven clean
          apps used for K-106, where the false-positive rate must stay zero.

### K-105 -- an assert cannot accept a synonym, so eight correct apps failed
Status:   partly fixed 2026-08-12 (commit 7f06969). The operator ships and
          works; **enumerating synonyms by hand does not converge** -- see
          the 2026-08-13 finding below.
Owner:    lead
Finding:  2026-08-13, run 3. I populated the alternatives from run 2's
          observed failures, which is fitting to the data. Run 3 broke two
          requests on words I had not seen and so had not listed:

            req 20  I added `query|search` after run 2 failed on `query`.
                    Run 3 printed `query` AND `search` -- the teaching
                    worked -- then failed `matches>=1` by printing
                    `matched:3` and `results:3`.

            req 23  I added `bullets|list_items` after run 2 failed on
                    `bullets`. Run 3 printed `lists:178` and `items:178`.

          Counting the names apps have chosen for one concept:

            list items:      list_items, lists, items, bullets    = 4
            search results:  matches, matched, results            = 3
            formatted output: output, out                         = 2

          Each run produces new ones. A hand-maintained synonym list chases
          a moving target and will always be one run behind, so the operator
          is necessary and not sufficient.
Better:   Three options, none yet chosen:
          1. **Publish the expected key names to the app.** The corpus
             already knows them; withholding them is what makes this a
             guessing game. It weakens the test slightly -- the app is told
             what to report -- but the test is meant to measure whether the
             app DOES the thing, not whether it guesses vocabulary.
          2. Match on meaning rather than spelling. Needs a judge, which the
             benchmark README rejects for good reason: not reproducible.
          3. Teach apps to print the request's nouns plus common variants.
             Already partly done ("print both when ambiguous") and it is
             what made req 20's `query`/`search` work -- but an app cannot
             enumerate every reader's vocabulary either.
          Option 1 is the only one that converges. Worth doing before the
          next run, and it makes the pass rate mean "did the app do the
          thing", which is what it was always supposed to mean.
Severity: serious
Class:    our-code
Found:    2026-08-12, at request 28 of the re-run, once the pattern was
          countable rather than anecdotal.
Evidence: Eight failures are a single synonym away from passing. The app's
          word and the corpus's word mean the same thing:

            req  wanted      app printed    kind
             5   count       clicks         synonym
            13   marked      done-today     synonym
            15   entries     expenses       domain word
            20   query       search         synonym (and `search` is the
                                            word in the request itself)
            21   cols        columns        abbreviation
            23   bullets     list_items     synonym
            24   max         largest        synonym
            28   recorded    logged         synonym

          Every one of these apps did the work. The mood tracker held 31
          days, 12 entries and an average of 3.5; it failed because it wrote
          `logged` where the corpus wanted `recorded`.
Impact:   Eight of nineteen failures in this run. The benchmark exists to
          stop measuring the wrong thing, and on this axis it is measuring
          whether an app and a corpus author picked the same word from a
          set of equally correct ones.
Fix:      Let an assert name alternatives, and pass if any of them holds:

            count|clicks>=1
            cols|columns>=2
            recorded|logged>=1

          One character of syntax, and the operator table in
          `scripts/benchmark-run.sh` already splits on the operator, so the
          key half just needs splitting on `|` before the lookup. Then sweep
          the corpus and add the obvious alternatives.
Not the fix: loosening the bar in any other way. An app that does not report
          a property still fails; this only stops two names for the same
          reported property counting as a miss. The pass bar itself does not
          move.
Note:     Teaching cannot close this on its own -- see the correction in
          K-103, where 75% of corpus keys cannot be derived from the request
          at all. The pack now says to print both names when a name is
          ambiguous, which helps, but the harness is where this belongs.
Update:   2026-08-12, req 34 adds a second shape. A table app measured the
          pixel width of all five columns, computed the grid width, and
          identified the widest column and the row it came from:

            columns:5  measured:yes  widest_column:1
            width_part:200  width_supplier:266  width_location:151
            grid_width:816  widest_source_row:1

          It failed `cols>=3` (abbreviation, as above) and `widest>=1` --
          not a synonym this time but a **prefix**: it printed
          `widest_column` where the assert wanted `widest`.

          Alternatives alone would not have saved it. The fix needs either
          the corpus to spell out `widest|widest_column`, or a rule that a
          key matches when the assert's name is a prefix of it followed by
          an underscore. The second is more general and riskier: under it
          `width_part` still would not match `width`, which is right, but
          the rule has to be stated deliberately rather than assumed to
          behave.
          Ten of twenty failures now turn on a key name.

### K-103 -- the benchmark scores correct apps as failures over key names
Status:   half fixed 2026-08-12 -- the teaching half shipped; the corpus and
          the missing operator are still open
Owner:    lead
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

          Corpus corrected 2026-08-12: `upper~ABC;lower~abc;title?` becomes
          `upper?;lower?;title?`. **This is weaker and that is worth saying
          out loud** -- it now checks the three cases are reported, not that
          they differ. The obvious stronger assert, `upper!=lower`, does not
          work: `!=` compares a key's value against a *literal string*, so
          it would ask whether `upper` is the text "lower", which is true of
          almost anything. There is no key-to-key comparison in the five
          operators.
          That is the real limitation, and it argues for a sixth operator
          rather than for a cleverer assert. Left unfiled as its own entry
          because it is the same fix as this one.
Fixed:    2026-08-12, the teaching half. By request 13 the count was **six of
          seven failures being working apps with different key names**
          (`clicks` for `count`, `done-today` for `marked`, `height_cm` for
          `height`, `elapsed:2:20.18` for a numeric `elapsed`), so the pack
          now says what to name a key rather than only how to format one:
          use the request's own noun, keep units out of the key, print
          durations bare, and print a generated thing rather than only facts
          about it. Every example in that section is one of the six real
          failures.
          The pack said "lower-case keys, no spaces" and never said what to
          call them, so apps invented reasonable names and the corpus
          expected different reasonable names. Same shape as K-102: the
          contract was under-specified, not disobeyed.
Still open: the corpus needs a sweep for asserts that assume a name no
          request implies, and the harness needs a key-to-key operator. The
          run in flight keeps the old corpus -- changing the measure
          mid-run would invalidate it.
Correction: 2026-08-12, at request 20. **The teaching fix above rests on a
          premise that does not hold, and would not have saved most of the
          failures it was written for.**

          Request 20 ("a contact book I can search") exercised itself
          properly -- `search:gra matches:1 added:1 selected:1
          stored:sqlite` -- and failed one assert: it named the search term
          `search` where the corpus wanted `query`. By the rule I had just
          shipped ("use the request's own word") the app was **right** and
          the corpus was wrong.

          Swept the whole corpus for how often its keys can be inferred
          from the request at all:

            $ awk ... evidence/benchmark/corpus.tsv
            corpus keys not present in their own request text: 79 of 105 (75%)

          `bill` is not in "a tip calculator". `die1` is not in "a dice
          roller that rolls two dice". `remaining` is not in "a countdown
          timer". Three quarters of the corpus expects a name the app has
          no way to guess, so **no amount of teaching can make an app match
          it**, and the rule as written is not wrong so much as
          unachievable.

          What follows:
          - The teaching half is worth keeping for the parts that ARE
            derivable (units out of keys, bare numbers, print the generated
            thing) but it must stop claiming it fixes the naming failures.
          - The real fix is on the harness side: an assert should accept
            alternatives (`count|clicks>=1`), or the corpus should publish
            its expected key names as part of the request the app sees.
          - **Do not expect the next run to improve much on naming.** I
            predicted it would; that prediction was wrong before it was
            tested, and the tier-gap note in the classification inherits the
            same flaw.
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
Update:   2026-08-12. **Two wrong fixes, then the actual cause. Both wrong
          attempts are recorded because each one looked right and each one
          cost a CI cycle to disprove.**

          Attempt 1 -- "CMake builds whisper against the dynamic CRT while
          Rust links static". Wrong: the cmake crate already passes `-MT`,
          the static runtime, so the two sides agreed all along. The
          toolchain file also broke the build in a new way, because a bash
          `$(pwd)` path means nothing to native CMake ("Could not find
          toolchain file: /d/a/krate/..."). Reverted.

          Attempt 2 -- "the cache restores whisper objects built under the
          old image". The failing runs really do invoke cmake zero times, so
          this was plausible. Rotating the cache key on the image label
          worked as designed -- "Cache not found for input keys" -- and
          whisper-rs-sys then **compiled from source and failed on the same
          23 symbols**. So the cache was never the cause. The key change is
          kept as hygiene, relabelled, because it is what produced the clean
          rebuild that disproved it.

          What the link line actually shows (run 31562047659):

            /defaultlib:libcmt          <- Rust asks for the static CRT
            legacy_stdio_definitions.lib
            (no libucrt.lib anywhere)

          `libcmt` does not itself carry the UCRT stdio and math functions;
          those live in `libucrt.lib`, and nothing puts it on the line. That
          is why every unresolved symbol is a UCRT one -- `__imp_fgetc`,
          `__imp_fmaxf`, `__imp__aligned_malloc` -- and why no cache key or
          CMake flag can help: the library simply is not being linked.

          The fix therefore belongs in the link arguments, not the workflow:
          `-C link-arg=/defaultlib:libucrt.lib` for the Windows targets in
          `.cargo/config.toml`, beside the `+crt-static` that is already
          there. **Not attempted yet** -- three speculative fixes in a row on
          a platform I cannot test locally is worse than a red job with an
          accurate diagnosis, and the next attempt should be made by someone
          who can reproduce it on Windows.

          Context that has not changed: CI run 31528953170 was **10 of 11
          jobs green, this the only red one**, and releases ship regardless
          because the release workflow builds Windows with `no-speech`.

          Why releases keep shipping anyway, which was not written down and
          should have been: the release workflow builds Windows with
          `no-speech: true`, so it never links whisper at all. Only this CI
          job builds the full feature set. That is the whole reason v0.1.12
          published six platforms green while this stayed red.

          The cost of leaving it: main has been red for five days, and a
          permanently red board stops being read. The next failure that is a
          real defect will look exactly like this one. That is the argument
          for fixing it, not the speech feature itself.

### K-014 — This machine is out of disk, and cargo cannot finish a test run
Status:   resolved -- rechecked 2026-08-13, the disk is no longer full
Owner:    lead
Severity: serious
Class:    environment
Found:    2026-08-05, W12, running cargo test at the end of the K-001 work
Recheck:  2026-08-13. `df -h /` now reports 34Gi available of 460Gi (26%%
          used). Full workspace test runs complete. Environment class, so
          nothing to fix in the product -- recorded closed so it is not
          rediscovered:

              /dev/disk3s1s1   460Gi    12Gi    33Gi    26%    481k  350M    0%   /

Evidence: `df -h /` reports 159Mi available of 460Gi (99%% full). Tool calls
          start failing with "ENOSPC: no space left on device". The bulk is
          build output: `/Users/yashrajpardeshi/Projects/layer6x6/target` is
          87G, and each agent worktree adds its own (mine is 7.8G).
Fix:      Not a product defect. `cargo clean` the shared checkout and the
          finished worktrees. Recorded so a later ENOSPC failure is not
          mistaken for a Krate bug, and because several agents building in
          parallel worktrees is what fills the disk -- the cost is structural,
          not a one-off.

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

### K-030 — A debug build shadows the real release on PATH
Status:   mitigated 2026-08-13 (7a2defa) -- the shadowing is the machine's
          PATH and stays; the binary now says which one you ran
Owner:    lead
Severity: serious
Class:    environment
Found:    2026-08-05, W17, checking what `krate` actually resolves to
Evidence: `which krate` gives
          `/Users/yashrajpardeshi/Projects/layer6x6/target/debug/krate`
          (`krate 0.1.0-dev`). The installed release at `~/.local/bin/krate`
          (rc20) is shadowed. Anything measured through the dev binary is
          contaminated: it is not the code a user runs.
Fix:      2026-08-13, commit 7a2defa. Rechecked and still live, and worse than
          recorded: the debug build reports the SAME version as release, so
          the version string could not tell them apart at all.

            target/debug/krate      krate 0.1.12
            target/release/krate    krate 0.1.12
            ~/.local/bin/krate      krate v0.1.8

          A debug build now appends a warning:

            krate 0.1.12 (debug build -- not what a user runs)

          Release output is unchanged, verified both ways. The suffix
          deliberately does not reach telemetry -- that version goes into a
          JSON field and a suffix would make every dev run its own "version".

          The rule still stands: every command in this repo uses an absolute
          path, and outsider testing uses ~/.local/bin/krate explicitly. This
          makes breaking that rule visible in one command rather than after an
          afternoon of confusing results.

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
Recheck:  2026-08-13. Still reproduces, and the entry above is wrong about
          what is happening -- which matters, because the fix it proposes
          cannot work.

          The three files are **tracked in the repository**:

              $ git ls-files apps/krate-notes/notes/
              apps/krate-notes/notes/first.txt
              apps/krate-notes/notes/second.txt
              apps/krate-notes/notes/third.txt

          They are the app's seed notes, committed on purpose in 63e8596 so
          krate-notes has content to show. So a run from the app's own
          directory does not create untracked files -- it **overwrites
          checked-in ones**. `git status` looked clean afterwards only
          because the app wrote back byte-identical content; appending one
          character and re-running shows ` M first.txt` immediately.

          That rules out the .gitignore half of the proposed fix outright:
          gitignore has no effect on tracked files. A rule was written,
          tested, found to change nothing, and removed rather than committed.

          The trap is real but milder and differently shaped than filed:
          editing a note in the app while verifying it dirties the working
          tree, and the diff looks like someone edited the seed data by hand.
Fix:      The sandbox-root half still stands and is the only one that works:
          default the sandbox root somewhere outside the source tree, or make
          `check-app` and the verify path run each app in a scratch cwd. The
          second is smaller and covers the case that actually bites, since
          nobody runs an app from its source directory except while verifying
          it. Still not urgent.
Left as is deliberately: changing the default sandbox root moves where every
          app's data lives, which is a bigger change than an annoyance-level
          bug justifies, and doing it carelessly would break anyone relying on
          the current cwd behaviour.

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

### K-120 -- Any continuously animating GUI app burns ~100% of a core
Status:   open
Owner:    lead (via K-112: the GPU presenter is the fix's home)
Severity: serious
Class:    runtime-hole
Found:    2026-08-16, building apps/krate-aurora, measured on macOS (M4).
Evidence: Two animating apps, measured with `ps -o %cpu=` six seconds after
          launch, both windowed and idle with no input:

              $ krate run Glow.krate --auto-grant     # shipped reference app
              Glow: 94.3% CPU
              $ krate run Aurora.krate --auto-grant   # 45,000 px/frame
              Aurora: 101.8% CPU

          The gap between them is the point. krate-glow draws a handful of
          vector cards and does almost no per-pixel work; krate-aurora
          computes 45,000 pixels a frame through three noise fields. Seven
          points separate them, so the cost is not the guest's drawing --
          it is a floor that any app paying to animate at all runs into.

          Ruled out inside the guest, in this order, each re-measured:
          1. `request-redraw` feeding its own event back, so every
             `events::wait(Some(ms))` returns instantly and the loop spins.
             The host already documents this at phase3_gui_host.rs:2290
             ("an animation loop calls request-redraw every frame and
             immediately receives that redraw back"). Removing the call
             entirely: still ~101%.
          2. No frame pacing. Added a monotonic-clock deadline that only
             draws when a frame is due and blocks in `wait` for the
             remainder: still ~101%.
          3. A per-frame `Vec` allocation for the reflection buffer.
             Hoisted to a reused buffer -- real fix, kept, and it took RSS
             from 215 MB to ~140 MB -- but CPU unchanged at ~101%.

          So a guest that paces itself correctly, allocates nothing per
          frame, and never asks for a redraw still cannot idle below a full
          core. That is the runtime's floor, not the app's.
Impact:   This is the demo case, not a corner: an animated app is exactly
          what gets shown to someone. A fan audibly spinning up during a
          demo, and a laptop draining, reads as "this runtime is heavy"
          regardless of how good the app looks.
Fix:      Unknown -- needs a profile of the host's wait/pump path rather
          than another guest-side change; the three obvious guest causes
          above are eliminated. Suspect the manual `nextEventMatchingMask`
          pump (see K-032's diagnosis, which established this runtime turns
          no CFRunLoop and pumps by hand from poll/wait) never actually
          parks: a pump that returns immediately whether or not an event is
          waiting would produce exactly this floor. Worth measuring against
          the Windows and Linux adapters before assuming it is macOS-only.
Update:   2026-08-16, symbolized profile (debug build, macOS M4): with the
          guest spin removed (K-122) glow still holds ~67% of a core, and
          the top of stack is the CPU rasterizer redrawing the full scene
          every frame -- linear_gradient_stops (207 samples),
          drop_shadow_round_rect (172), fill_round_rect (93),
          publish_canvas (83), stroke_round_rect (45). The floor is
          full-scene CPU raster at 60fps. The real fix is the GPU
          presenter workstation (K-112); interim wins would be caching
          shadow masks and gradient ramps keyed by their parameters.

### K-117 -- Apps cannot paint the title bar area, so full-bleed designs are impossible
Status:   open
Owner:    lead
Severity: major
Class:    runtime-hole
Found:    2026-08-16, replicating MarkText pixel-for-pixel as a Krate app
          for the first head-to-head benchmark.
Evidence: MarkText's window is one flat #282828 surface to the very top
          edge, traffic lights overlaid (titleBarStyle overlay). The Krate
          replica gets a standard macOS title bar band above its content --
          the one visible difference no app code can remove, because the
          window API offers no full-bleed or hidden-title style. Every
          modern editor, terminal and browser uses this style; apps that
          cannot will always look one generation older.
Fix:      A window style option in the ui.window interface (full-bleed /
          hidden title with overlay controls), honored by the macOS
          adapter (titlebarAppearsTransparent + fullSizeContentView), the
          Windows adapter (extend client area into the frame), and Linux
          (CSD). Until then the studio's own chrome does on the shell what
          apps cannot do for themselves.
Update:   2026-08-16, e63ef189: shipped on macOS (set-full-bleed in
          ui.window, additive; transparent titlebar + full-size content +
          overlaid lights; sizes follow effective_content_rect so the
          canvas owns the band). Draft accepts so check-app cannot fail an
          app for asking. Still open for the Windows adapter
          (extend-client-area into the frame) and Linux (CSD); both
          currently return honest unsupported and keep standard chrome.

## Fixed

### K-159 -- Studio on Windows flashed console windows through every build

Class: our-code
Owner: claude
Status: fixed

The studio is a `windows_subsystem = "windows"` app, so any console-subsystem
child spawned with a bare `Command` pops a real console window. Three call
sites did:

- `pid_alive` runs `tasklist` on the liveness watchdog, every few seconds of
  every build -- a black box flashing over the screen for the whole time an
  app is being made.
- `kill_tree` runs `taskkill` -- a flash on pressing Stop.
- `shoot` runs the engine to photograph the finished app -- a flash at the
  exact moment of success.

All three now use `silent_cmd` (CREATE_NO_WINDOW), which the rest of the file
already used. Found by auditing every `Command::new` in studio/main.rs while
setting up Windows verification on the physical PC.

### K-158 -- Nothing registers the .krate file type on Windows

Class: our-code
Owner: claude
Status: fixed

On the parity VM with the shipped v0.1.52 installer there was no `.krate`
association at all -- no class, no FileExts entry, no `krate` on PATH.
Double-clicking a `.krate` did nothing, which is the product's whole promise.

The registration code was not missing. `first_run_setup` has a Windows arm
that writes every key correctly: `.krate` -> `Krate.App`, the open command,
the icon, and the `krate://` scheme for the sign-in hop.

**It was never reached.** `main` has two early returns before it -- one for a
`krate://` URI, one for a `.krate` path on argv -- and setup was called after
both. On a machine where the studio had only ever been launched with an
argument, registration never ran once. Open the studio plainly first and it
all registers, which is why this survived: it depends entirely on the order
somebody happens to do things in.

Two changes:

- Setup runs BEFORE the early returns on Windows and Linux, and
  **synchronously** there. Those returns leave `main`, which ends the process
  -- a background thread would be cut off partway through writing the
  registry, which is worse than not starting. Marker-guarded, so later
  launches cost one file check. macOS keeps the background call; its path has
  no early return.
- Windows setup now also puts `krate` on PATH via `HKCU\Environment`, matching
  the `/usr/local/bin` symlink macOS makes. Without it `krate doctor` -- the
  first thing anyone is told to run -- is not a command on Windows even after
  installing. Appended only when absent, so PATH cannot grow on every launch.

**What is proven and what is not.** The ordering fault is proven by reading:
`first_run_setup` sat after two `return`s in `main`, and both fire when the
studio is launched with an argument. The registry keys are proven to WRITE
correctly on the VM. What is NOT yet proven end to end is that a double-click
then opens the app, because Azure Run Command executes as SYSTEM: it writes to
SYSTEM's hive, and `assoc`/`ftype` from a service context do not reflect a
user's HKCU classes. Confirmed that is an artifact rather than a finding by
writing a throwaway `.ktest` class and seeing the same non-result.

So this needs one check on a machine somebody is actually logged into: install,
double-click a `.krate`, confirm it opens. Until then the fix is
argued-and-compiled, not demonstrated.

### K-157 -- A double-clicked app on Windows and Linux is refused, not asked

Class: our-code
Owner: claude
Status: fixed

On macOS, `open_app` runs the bundle with `consent: true`, so an app that
declares an ask-level capability gets a consent window and the person says yes.
The Windows and Linux `launch_app` ran `krate run <path>` with no consent flag
at all, so the same app was refused outright:

    This app needs permission it was not given, so it did not run.
    It needs to:
      - save its own settings and data (store.kv)

No window, no prompt, no way to say yes -- and on a double-click there is no
console to read that message in either, so the app simply does nothing.

Measured on the parity VM with a grocery list whose only ask is `store.kv`: it
died instantly, and ran the moment `--consent` was added. `launch_app` now
passes it, matching what macOS has always done.

Worth noting how narrow the escape was: an app declaring ONLY default-granted
capabilities (a window, stdout) runs fine, which is most of the sample apps.
Anything that saves data, reaches the network, or uses the camera was dead on
arrival.

### K-156 -- The Studio installers ship without cargo-component

Class: our-code
Owner: claude
Status: fixed

Hit on a clean Windows 11 machine with the shipped v0.1.52 installer. Making
the first app failed with:

    That one didn't come together.
    The build tools aren't set up yet. Trying again lets Krate install them.
    error: failed to compile `cargo-component v0.21.1`
    error: install cargo-component: install command failed

The install directory held `krate-studio.exe`, `uninstall.exe` and
`bin\krate.exe` -- and no `cargo-component.exe`. So Studio fell back to
compiling it from source: 378 crates before anything the person asked for
begins, and a hard failure with a "try again" button if that compile trips.

**The CLI archives have shipped it for ages; the Studio installers never did.**
The release workflow builds cargo-component per target into `tooling/bin`, and
a verification step even checks it is in the CLI archive -- with the comment
"It has silently gone missing on Windows before". But the three Studio
packaging steps copy only the engine:

    cp "target/${matrix.target}/release/krate.exe" studio/bin/krate.exe

Fixed: all three (macOS, Windows, Linux) now also copy
`tooling/bin/cargo-component` into `studio/bin`, which `resources: ["bin/*"]`
carries into the install beside the engine -- exactly where `resolve_tool`
looks. The copy is tolerant (the build step is `continue-on-error`, and a
Studio without the tool beats no Studio) but prints a `::warning::`, so a
missing one shows up in the log rather than in somebody's first build.

Worth noting what made this hard to see: the engine installs to `bin\`, not
beside `krate-studio.exe`. Dropping cargo-component next to the Studio
executable did nothing; it has to sit beside `krate.exe`.

Unblocked on the parity VM by hand, and `krate doctor` there now reports
`cargo-component 0.21.1`.

### K-155 -- Codex builds show no progress at all: the parser reads the wrong fields

Class: our-code
Owner: claude
Status: fixed

Reported from real use: a habit-tracker build sat on "Reading Krate's API --
still reading, this is the longest part of a build" for eleven minutes with
nothing under it. It succeeded, but the person had no way to tell it was alive.

The trace says it exactly:

    TOTAL     730.9s
    AGENT STEPS  (0 tool calls)
    STALLS  at 0.0s -- 730.9s of silence

And the transcript for the same build holds **155 events**: 68 command
executions, 37 item starts, 6 file changes. Codex was talking the whole time.

Two faults, both in `CodexProvider::progress_line`:

1. **Wrong fields.** Codex puts the kind of work in `/item/type` and the
   command in `/item/command`. The parser looked for `name`, `tool` and
   `/item/name` -- none of which codex sends -- so every event returned `None`.
   `command_execution` and `file_change` are also codex's own words, and
   `describe_tool_use` matches on substrings like "write" and "read", so they
   are translated before the match.

2. **A shell read looked like nothing.** Codex does its whole pack-read with
   `cat` and `sed`, and the bash branch only recognised `check-app` and
   `cargo`, returning `None` for everything else. So even with the fields
   fixed, the longest phase of the build would still have been silent.
   `read_target` now names the file a reading command points at.

Only `item.started` reports: codex emits started AND completed for the same
work, and reporting both printed every line twice.

Pinned by tests built from the real events out of that transcript. The same
build would now report 37 progress lines instead of none.

### K-153 -- The launcher bundle a double-click builds had no camera key

Class: our-code
Owner: claude
Status: fixed

Shipped in v0.1.52 and found by installing that release and using it, not by
reading the diff. Double-clicking a webcam app said the camera could not be
opened; the trace read:

    usage description missing: Unsupported("this app bundle does not declare
    NSCameraUsageDescription, which macOS requires before any process may
    open a camera")

**There are two launcher-bundle builders in main.rs, and K-145 fixed the wrong
one.** `macos_launcher_bundle` (K-145) serves `Command::OpenApp`;
`install_bundle`, reached through `Command::Launch`, is what a Finder
double-click actually runs -- and its plist template had `CFBundleName`,
`CFBundleIdentifier`, `NSHighResolutionCapable` and nothing else. So every app
opened the ordinary way ran from a bundle that could not ask for the camera.

The give-away was a `Resources` folder in the generated wrapper that the code I
had written never creates. Two builders producing similar bundles is the
underlying defect; for now both carry `NSCameraUsageDescription` and
`NSMicrophoneUsageDescription`, and a follow-up should collapse them into one.

Verified end to end after the fix: the generated `Shows.app` plist carries the
key, the trace reaches "past permission" instead of failing, macOS shows its
prompt, and the live feed appears.

### K-152 -- A live build looks dead after leaving the session and coming back

Class: our-code (UX)
Owner: claude (FIXED, see Fixed)

Reported from real use: "I'm making an app, I go to the cloud section and come
back to the main page and I couldn't see the active session. I clicked that
recent session from history, and I could see the process was active but the
right side progress was blank -- it said you'll see the app preview here."

Two separate faults, both about state that lived only in the DOM.

**The progress pane.** Everything it shows -- the stage list, the log, the live
line, the clock -- was written straight into `#stateBuilding` as it happened,
and nothing kept a copy. `show("building")` only un-hides that pane; it rebuilds
nothing. So re-entering a session showed whatever was left in the DOM, which
after a trip elsewhere is nothing.

Worse, `advanceStage` MOVES `#peekBox` inside `#stages`, so any later wipe of
that list destroys `#peekBox` and `#nowLine` with it -- the same mechanism as
K-136. Rescuing the element is not enough on this path, because by then it does
not exist.

**The "making now" bar on home.** It carries `class="reveal"`, and `revealIn()`
stamps `opacity: 0` on every `.reveal` element when a view appears, then
animates them back. But the bar is still `hidden` at that moment;
`renderBuilding()` un-hides it two awaits later, after the animation has
finished, with the from-state still on it.

**Also answered here: only one build runs at a time.** `run_author` in
studio/src/main.rs holds a single pid slot and refuses a second with "one app
is already being made". The UI already handles that honestly -- a request typed
into the building session queues and says so; one typed into a different
session says which app is holding the slot. That is a deliberate limit, not a
defect, but it is worth stating plainly since the report asked.

### K-152 -- A live build looks dead after leaving the session and coming back

Class: our-code (UX)
Owner: claude
Status: fixed

Build progress now lives in `state.builds`, a Map keyed by session id, holding
the log lines, the stage index, the live line, the header and the start time.
`restoreBuild(sessionId)` replays all of it, and `openSession` calls it on the
branch that reopens a session which is building right now.

Two details that the fix turns on:

- `restoreBuild` RECREATES `#peekBox` when it is missing rather than only
  rescuing it. On this path the element has usually already been destroyed
  along with `#stages`, so a rescue finds nothing (K-136's mechanism, reached
  from a different direction).
- The log is capped at `BUILD_LOG_LINES` (400). The pane only shows the tail,
  and a long build prints thousands of lines; keeping them all would grow
  without bound for no benefit.

`renderBuilding()` clears the reveal from-state off the "making now" bar when
it un-hides it, so a bar un-hidden after the animation ran cannot come back
invisible.

Verified in a browser against the reported flow: with the pane wiped, the
before state is 0 stages and a null `#nowLine`; after the restore it is 4
stages, "Testing it" lit, the live line back, the log intact and the title
correct. A build continues correctly through a restore -- new engine lines land
in the rebuilt DOM, stages still advance, and the peek stays anchored under the
live row.

### K-150 -- main has not compiled for Windows since v0.1.51, and nothing noticed

Class: our-code
Owner: claude (FIXED, see Fixed)

`open_app` lost its `#[cfg(target_os = "macos")]` at some point after v0.1.51.
Its body uses `std::os::unix::process::CommandExt`, `Command::exec`, and
`krate_adapter_macos` -- none of which exist on Windows -- so `main` stopped
compiling there entirely. Nine errors:

    error[E0433]: could not find `unix` in `os`
    error[E0425]: cannot find function `spawn_open_run` in this scope
    error[E0599]: no method named `exec` found for `&mut std::process::Command`
    error[E0433]: unresolved module or unlinked crate `krate_adapter_macos`
    error[E0282]: type annotations needed
    ... 4 more

v0.1.51 had the gate, and still has it -- so the last RELEASE is fine and this
never reached a person. Every commit on `main` since is broken.

The real defect is not the missing attribute; it is that six commits landed
over it without anyone noticing. Windows is only ever compiled during a
release, so `main` can rot for days and the first sign is a failed release --
or, as here, somebody finally standing up a VM.

Fix: gate restored, verified compiling on a real Windows 11 machine. What this
actually argues for is a Windows `cargo check` in ordinary CI, not just in the
release workflow. Cross-checking from a Mac would not have caught it either:
`cargo check --target x86_64-pc-windows-msvc` cannot get past the C
dependencies without an MSVC toolchain, so it never reaches this code.

### K-142 -- The quiet heartbeat contradicts the step it is shown under

Class: our-code (UX)
Owner: claude (FIXED, see Fixed)

The build card's quiet-line rotation was one flat list, cycled regardless of
which step was lit. So "the writing starts once it has read enough" appeared
underneath a lit **Writing the code**, and "reading Krate's API reference"
appeared under it too. The founder caught it: the title says one thing and the
description below says another.

It is small and it is corrosive. The stage list is the only evidence a person
has that a ten-minute build is progressing; a display that contradicts itself
teaches them not to trust any of it.

### K-141 -- On macOS, full-bleed apps ignore every click and scroll

Class: our-code
Owner: claude (FIXED, see Fixed)

**This is the "not clickable, not scrollable" both generated weather apps had,
and it was ours, not theirs.**

`capture_mouse_event` and `capture_wheel_event` in the macOS adapter flipped
AppKit's bottom-up coordinates using `self.window.contentLayoutRect()`.
`contentLayoutRect` is the area *below* the title bar. A full-bleed window owns
the title-bar band as well, so its content is taller -- and flipping against
the shorter rect shifted every click and every scroll up by the title bar
height.

The adapter already had the right answer: `effective_content_rect` exists for
exactly this and is documented against K-117. The two input paths never called
it.

What a person sees: the app animates, so it is clearly alive, but nothing
reacts. The offset lands hardest on the header row -- search fields, buttons,
tabs, city chips -- which is where a person clicks first.

Evidence:

    $ grep -n "set_full_bleed" wd-src/source/src/lib.rs
    379:        let _ = window::set_full_bleed(win, true);
    $ grep -n "set_full_bleed" ios-src/source/src/lib.rs
    2961:        let _ = window::set_full_bleed(win, true);

Both apps full-bleed, both unclickable. The headless check never touches AppKit,
which is why it passed them both (see K-140).

### K-140 -- The click check let a spinner answer for a dead button

Class: our-code
Owner: claude (FIXED, see Fixed)

The usability gate passed both broken weather apps on every stage, including
`click`, while neither could actually be clicked (K-141). That makes the gate
worse than no gate: it certified apps that did not work.

Two holes, both now closed:

1. **The verdict was `difference > 0.0`.** Any animation on screen satisfied it.
   A spinner alone reads as "the app responded" no matter what was pressed --
   measured on the weather app, the idle churn is 0.007% of the frame, and that
   was enough to pass.
2. **The press aimed at the centre of the canvas.** A canvas app draws its own
   controls, so the host does not know where they are, and centre-of-canvas
   lands on empty space as often as not.

Fixed by measuring the app's idle churn first and requiring the press to beat
it by 3x, and by adding `KRATE_PRESS_AT=x,y` so a check can aim at a control
whose position it knows. Measured after the fix, aiming at a city chip:

    krate-check: click difference=0.028558 idle_churn=0.000068 answered=true

2.86% against a 0.007% baseline -- a real reaction, told apart from the
animation for the first time.

### K-139 -- codex can never author: the agent is started in the wrong directory

Class: our-code
Owner: claude (FIXED, see Fixed)

`run_provider_author` never set the child's working directory. The agent
inherited whatever cwd Krate was launched from, while the prompt told it to
write into an absolute path somewhere else.

claude does not care -- it writes anywhere it is told. **codex roots its
`workspace-write` sandbox on its own cwd**, so every write to the app directory
was "outside of the project" and was refused. codex could therefore never
author anything, on any machine. This is the answer to "I already have codex
installed, why does Krate not work with it" -- it was never codex.

Evidence, from the founder's failed webcam build:

    ERROR codex_core::tools::router: error=patch rejected: writing outside of
    the project; rejected by user approval settings

Second defect in the same failure: the agent had correctly decided the app
could not be built (Krate has no camera capability -- K-119) and tried to write
`CANNOT-BUILD.txt` to say so. The sandbox refused that write too, so its clear
explanation existed only in the transcript, and the person was shown a generic
"That one didn't come together" with a stack-trace-looking blob. The refusal
path only ever read the file.

### K-137 -- Concurrent fetches spawn unbounded OS threads, and the host panics when they run out

Class: our-code
Owner: claude (FIXED, see Fixed)

`AsyncFetches::begin` called `std::thread::spawn` once per in-flight request
with no cap. `std::thread::spawn` **panics** when the OS refuses a thread, and a
panic in the host is not a failed request -- it takes the runtime down and the
person gets an operating-system crash dialog. This is the crash the founder saw
opening `ios.krate`.

An app does not have to be hostile to reach it. `cancel` cannot kill a running
thread (there is no safe way to), so a cancelled request keeps its worker until
its own timeout expires -- 9 seconds in these apps. `ios.krate` starts one
request per saved city in `refresh_all`, so a list that is refreshed repeatedly
stacks workers faster than they retire.

Evidence -- the ceiling on this machine, measured:

    $ cat /tmp/thr.rs   # spawn sleeping threads until the OS refuses
    $ rustc -O -o /tmp/thr /tmp/thr.rs && /tmp/thr
    failed after 4095 threads: Resource temporarily unavailable (os error 35)

`Builder::spawn` returns that as `Err`. `thread::spawn` unwraps it and panics.

### K-136 -- A build looks frozen when claude's API stalls (it recovers, but silently)

Class: our-code (UX) / environment (the stall itself)
Owner: claude (FIXED in 7850f3df)

RESOLVED as to root cause, caught live with system evidence. The "freeze" the
founder hit repeatedly -- first attempt of a request appears stuck on
"Understanding what you asked for", the retry builds -- is claude's API
connection stalling: the create process alive at 0% CPU for 11 minutes, its
claude child holding three ESTABLISHED HTTPS connections to the Anthropic API
with no response streaming back. Then, watched further, it RESUMED on its own
after ~5-6 min and the build finished. So the stall is transient and self-
healing; the retry "worked" only because the API answered cleanly next time.

Two things made it read as a hard freeze:
1. claude does its whole pack-read via Bash commands that produce no progress
   step, so the UI stage list sits on "Understanding" for minutes even when
   working (measured: up to 5.5 min of legitimate initial silence).
2. When the API then stalls, the UI shows no sign of it -- a silent screen for
   up to the 10-minute kill.

Mitigation (01714def): warn on the progress channel after 2 min of silence so
the stall is VISIBLE ("the AI has gone quiet ... still waiting") and a person
waits instead of giving up. Kill stays at 10 min -- killing sooner would throw
away a build about to recover.

Not fully closed: the deeper fix is to cut the silent read entirely
(retrieval-over-dump, per the pipeline study) so there is far less dead time to
stall inside, and/or to retry the agent automatically on a hard stall.

ACTUAL ROOT CAUSE, found by reproducing it in a browser rather than reading
logs: advanceStage MOVES #peekBox inside #stages. On the next build,
beginBuild wiped $("stages").innerHTML -- destroying the peek and with it
#nowLine -- and the next line, $("nowLine").textContent = "warming up...",
threw "Cannot set properties of null". That throw was inside buildNow BEFORE
the create_app invoke, so the build was silently lost: no engine, no workspace,
no trace, no error, UI stuck on "building" forever.

That is the whole pattern: the FIRST build of a fresh Studio worked (peek still
in its original home), every build AFTER it died. Every "the retry worked" was
really "the relaunch reset the DOM". The API-stall work above was real but a
different, rarer problem -- this null-deref is what the founder hit for days.

Fixed by rescuing #peekBox before the wipe, making every DOM write in
beginBuild defensive, and wrapping beginBuild in try/catch inside buildNow so
presentation can never again lose a build. Verified: three consecutive
beginBuild calls succeed where the second used to throw.

### K-106 -- generated apps put text on top of other content
Status:   fixed 2026-08-13 (4e60477), text-over-text half; see K-107
Owner:    lead
Severity: moderate
Class:    teaching-hole
Found:    2026-08-13, screenshotting run-3 apps for marketing material. Two
          of the first two games shot had overlapping text, which is a bad
          rate for something a person sees immediately.
Evidence: Both apps pass check-app, and both look wrong at a glance.

          The two failed by different mechanisms, which is the useful part.
          Tic tac toe reserved nothing -- board to y 512, score card 536-612,
          and a button at the constant `y: 556` that was correct in isolation
          and never checked against what came before it. The memory game DID
          reserve a footer correctly, then drew its hint at
          `size.height - FOOTER_H - 6.0` ("just above the footer"), which is
          outside the footer and inside the band already given to the cards.

          That second one matters: the pack's three layout rules are all
          about deciding regions, and the memory game followed them and still
          drew outside its own region.

Fix:      2026-08-13, commit 4e60477, in two parts.

          Teaching: pack rule 4, "draw inside the region you were given, and
          derive every position from it", with both failures as its worked
          examples and a typed-in coordinate named as the smell.

          Detection: `krate run --shoot X.png --check-layout` reads the
          recorded draw calls, not the pixels, so the geometry is the app's
          own numbers. Text against text only -- text over a panel is
          ordinary design and flagging it would bury the real defect.

              $ krate run req-25/app.krate --shoot ttt.png --check-layout
              layout: 2 places where text is drawn over other text
              layout:   "Draws" and "New round" share 30% of the smaller
                        one, around x 211 y 589
              layout:   "0" and "New round" share 19%, around x 223 y 577

          Calibrated against seven real apps that do not have the bug
          (bounce, chart, cubes, eo2, mdview, savings, fa32e9bc): zero false
          positives, several of them text-heavy.

          Two implementation notes worth keeping. The existing display list
          is filled only on adapters that consume it (iOS), so a check
          reading it would find an empty list on desktop and pass everything
          -- worse than no check, because it looks like it worked. And the
          buffer resets on clear, or a repainting app stacks frames and
          appears to collide with itself.

Left open: the memory game reports clean, correctly, because its hint lands
          on cards rather than on text. That is K-107.

            req 25 tic tac toe (27,539 B)
              the "New round" pill is drawn on top of the "Draws" label in
              the score bar. The word Draws is legible through the button.

            req 26 memory game (30,169 B)
              "Turn over two cards to find a pair" is drawn across the
              bottom row of cards, so the hint and the cards share pixels.

          Screenshots: scratchpad/shots-marketing/{tictactoe,memory}.png
Not covered by the existing rule. K-098's follow-up teaches "measure from the
          outermost edge, not the shape's own size", which is about
          decorations sticking out past a shape (chairs around a table).
          This is different: a label or button placed in space that another
          element already occupies. The app has the room -- both windows
          have empty space -- it simply did not reserve any.
Fix:      A layout rule in the pack, roughly: every element gets a rectangle,
          and no two rectangles overlap. Lay the fixed chrome out first
          (title, score bar, buttons), subtract it, and give what is left to
          the content. Say plainly that text drawn over other content is a
          defect even when both are legible.
Why it matters: this is the first thing a person sees, and it is the exact
          complaint that started the visual work -- "not that amazing
          considering what the world has seen". An app can pass every gate
          and still look broken in the screenshot someone posts.
Note:     check-app cannot catch this today. The usability stage measures
          whether the canvas followed the window, not whether two things
          were drawn in the same place. Detecting it needs the region
          measure that K-099 also wants.

### K-110 -- every app after the first opens with no window at all (macOS)
Status:   fixed 2026-08-13 (9901c16), SHIPPED in v0.1.13 2026-08-14
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-13, Yashraj: "double click open is not working here in my pc,
          and not in windows as well ... this breaks so often".
Evidence: Open one .krate and it opens. Leave it running, double-click a
          second, and nothing appears -- no window, no error. Reproduced on
          the RELEASED v0.1.12 with an app made through the full user path:

              installed build   windows: Cubes,              <- second missing
              with the fix      windows: Cubes, Tea Timer

          The process is running and the runtime prints
          `opened window "Tea Timer"`, so from the inside everything worked.

Cause:    The first document arrives through AppKit's open-documents event and
          runs in the process macOS launched -- a registered GUI application.
          Every later document went through `spawn_open_run`, a bare
          `Command::spawn`. That produces a process macOS does not consider a
          GUI app: no LaunchServices registration, no activation, so AppKit
          will not put its window on screen however correctly the runtime
          creates it.

          That is why it read as random. It is not: one app always works, the
          second never does. Which one somebody hits depends only on whether
          they already had an app open.
Fix:      Relaunch through the .app bundle -- `open -n -a Krate.app <file>` --
          so LaunchServices registers the new instance. Direct spawn stays as
          the fallback for a plain CLI install with no bundle.
Windows:  Not affected. Explorer runs `krate-open.exe "%1"`, so each app is
          its own process with the path as an argument and there is no event
          to miss. CI checked only that krate-open.exe was PRESENT in the
          archive, never that it opens anything -- that gap is now closed by a
          cold-install step that registers the association, reads back the
          ProgID command, and runs it on a real file.
Shipped:  v0.1.13, 2026-08-14. Verified the way a person meets it -- a cold
          install from krate.tech, then Finder's own double-click, three
          rounds:

              install exit=0, krate v0.1.13
              round 1: [Cubes, Tea Timer]
              round 2: [Cubes, Tea Timer]
              round 3: [Cubes, Tea Timer]

          Against the same two apps on v0.1.12 the second window never
          appeared at all.

### K-109 -- an app that resets itself last prints a state that looks like it never ran
Status:   fixed 2026-08-13 (b1104d0), untested until benchmark run 5
Owner:    lead
Severity: moderate
Class:    teaching-hole
Found:    2026-08-13, benchmark run 4, request 3 (a countdown timer), failing
          `remaining!=1500` at 2 of 3 asserts.
Evidence: The app is correct. It drove its whole interface on `quick` and
          reset last:

              duration:1500
              remaining:1500
              elapsed:1
              ticks:1
              reset:yes

          `elapsed:1` and `ticks:1` prove it really counted down. But
          `remaining` is back to the starting value, so the one number a
          person would check to see a timer counting says it never ran. Its
          own source comment describes the sequence: "let it count, pause it,
          adjust the length, reset it -- and print what the".
Fix:      2026-08-13, commit b1104d0. The pack already said "operate your own
          controls, do not just print a snapshot", and this app did exactly
          that. What it never said is that an app prints ONCE, at the end, so
          an operation that undoes the others must not be last. Added with
          this failure as the worked example: put reset, clear, cancel and
          close in the middle, or print the telling value where it is true
          (`remaining_at_pause`) as well as at the end.
Not a corpus change, deliberately: `remaining!=1500` is the right assert --
          an app ending on the full duration has not shown a countdown -- and
          widening it to accept 1500 would leave it unable to fail. The rule
          landed after run 4 started, so run 4 cannot test it; run 5 will.

### K-104 -- the benchmark's authoring budget is too small for its own corpus
Status:   fixed 2026-08-12 (commits b1cdeff, 82e440f) -- a timeout and a
          dropped connection are `skipped`, not `fail`; per-tier budget open
Owner:    lead
Regression: 2026-08-12, same day, caused by the fix above and caught four
          requests into run 3. The detection matched
          `KRATE_AUTHOR_TIMEOUT_SECS` and `Raise the budget`, and the agent
          transcript carries an environment dump containing
          `KRATE_AUTHOR_TIMEOUT_SECS=1800` on **every** run. So the rule
          fired on any request that got far enough to write a transcript,
          and the first one to do so halted the benchmark:

            [5] easy skipped account 406s a click counter
            note: authoring hit the 1800s budget while still working

          406 seconds against an 1800 second budget cannot both be true, and
          that contradiction is what prompted the check.

          Now matches only the sentence the CLI prints on a real timeout:
          `did not finish within N minutes and was stopped`. Verified against
          a live transcript from the running benchmark -- the env var appears
          once, the new pattern zero times.

          **The regression was worse than the bug.** The original wrongly
          failed working apps; this wrongly excused them and stopped the run,
          which removes data silently rather than visibly. A skip rule needs
          testing against a healthy run, not only against the failure it was
          written for -- my test covered four failure texts and no success.
Severity: serious
Class:    our-code
Found:    2026-08-12, request 14 of the re-run (a note-taking app).
Evidence: The per-request budget is 900 s (`TIMEOUT_SECS`). Request 14 was
          cut off at 902 s having completed **41 authoring steps** -- read
          the API reference, read the notes example, wrote code, checked it
          built, iterated, wrote again. It was working the whole time. No
          `.krate` was produced, and the row reads `fail / authored`, which
          is indistinguishable from an app that was written badly.

          The budget is not far above the working range:

            req 1  tip calculator     417 s   pass
            req 4  temperature conv   543 s   pass
            req 7  password gen       656 s   fail (naming)
            req 14 note taking       >900 s   cut off

          Easy-tier apps that succeed already take 7-11 minutes. The margin
          above the slowest success is 244 s, and **24 medium and hard
          requests remain**, all of which are larger than the app that just
          ran out of time.
Impact:   Any medium or hard request that needs more than fifteen minutes is
          recorded as a product failure. Run the rest of the corpus at 900 s
          and the headline number measures the timeout, not the authoring
          loop. Same category error as counting a rate-limit rejection as a
          failure, which the harness's own header warns about at length.
Fix:      Two parts.
          1. Raise the budget for the tiers that need it -- the header
             already takes per-tier behaviour for granted elsewhere, so a
             per-tier `TIMEOUT_SECS` is in keeping.
          2. **Record a timeout as its own outcome, not as `fail`.** The
             harness distinguishes `skipped` for quota rejections for
             exactly this reason: no code was written, so there is nothing
             to judge. A timeout is the same situation with a different
             cause.
Note:     Do not raise the budget mid-run and keep the earlier rows. Either
             finish this run at 900 s and mark the timeouts, or restart the
             remaining tiers at a higher budget and say which rows were
             measured under which. Changing the measure halfway and not
             saying so is how a number stops meaning anything.
Correction: 2026-08-12, after the retry. Request 14 passed at the higher
          budget **in 378 seconds, using 12 authoring steps** -- against 41
          steps and a 902 s cutoff on the attempt before it. Same request,
          same corpus, same binary.

          So the original reading was wrong in an important way. A note-taking
          app does **not** need more than 900 s; the first attempt wandered
          down a long path and the second went almost straight there.
          **Authoring time varies by more than 2x on the same request**, and
          the budget is a ceiling on the variance, not on the work.

          That changes what the fix is for. A per-tier budget is still worth
          having, but the reason is to stop an unlucky run being recorded as
          a product failure -- not because larger apps have a higher floor.
          And it makes the case for the second half stronger, not weaker: a
          timeout must be its own outcome, because it now clearly measures
          luck as much as difficulty.

          Worth measuring properly before tuning further: run one request
          several times and record the spread. A benchmark whose per-request
          time varies 2.4x is a benchmark whose single-shot pass rate carries
          more noise than anyone has quantified.
Also:     A restart clears `WORK_ROOT`, so the per-request `run.log` -- the
          actual stdout an app printed -- is lost for every row already
          recorded. The TSV keeps the assert tally and the missing keys,
          which is enough to classify a failure, but not enough to quote
          what the app said. That evidence is exactly what turned "six
          failures" into "six working apps with the wrong key name" (K-103),
          so it is worth keeping: copy `run.log` beside the results row, or
          fold its contents into a column.

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
Status:   fixed 2026-08-13 (fc9bce2), with a known limit -- see K-108
Owner:    lead
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

Fix:      2026-08-13, commit fc9bce2. Measured from the recorded draw calls
          rather than the pixels, which is what made it tractable: the
          background is one op covering the whole canvas, so it is
          recognised by size and excluded, and everything else is content.
          Coverage onto a 24x24 grid, largest empty rectangle by histogram
          sweep.

          Two tries to find the frame boundary. `clear` looked like the
          start of a frame and is not -- ten of fourteen generated apps
          never call it, painting a full-canvas gradient instead, so they
          recorded no canvas size and went unmeasured. That is the same
          "detects nothing while claiming to" failure this bug was reverted
          for the first time, caught only because the calibration run
          printed a measurement for 4 of 14 apps. The boundary is
          `publish_canvas`.

          Calibrated across 21 apps (14 generated, 7 hand-written). All but
          two sit at or under 14.6%, so 16% is above the ordinary band
          rather than a round number picked in advance:

            req-34        17%   a generated table with a 157px unused strip
                                down its right side, full height. Confirmed
                                in the screenshot; its last row is clipped
                                for the same reason.
            krate-notes   39%   a text editor with a short note in it.
                                A false positive -- see K-108.

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
Status:   fixed 2026-08-13 (836d23b)
Owner:    lead
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
Fix:      2026-08-13, commit 836d23b. Both halves, plus a third nobody had
          spotted.

          A check for this already existed in the CLI and was wrong in the
          direction that does damage. It probed only the unversioned
          `libxkbcommon-x11.so` and then told people to install
          `libxkbcommon-x11-dev`. But xkbcommon-dl tries
          `libxkbcommon-x11.so.0` FIRST (xkbcommon-dl-0.4.2/src/x11.rs:50),
          so the runtime package alone is enough and always was:

              50:  &["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"],
              59:  .expect("Library libxkbcommon-x11.so could not be loaded.")

          So on a machine with libxkbcommon-x11-0 installed -- where apps run
          perfectly -- the check refused to start and sent someone who only
          wanted to open an app off to install a developer package. The
          README repeated the same false claim. The panic message names the
          unversioned soname, which is probably what misled it.

          Now: both probes ask exactly what the loader asks, both sonames in
          its order, and name the runtime package `libxkbcommon-x11-0`. A
          second guard sits in the Linux adapter's event-loop setup beside
          the existing no-display guard, so the path that creates a window
          cannot reach the panic even if the CLI check is bypassed. Wayland
          sessions skip it there; the CLI check deliberately does not skip on
          WAYLAND_DISPLAY because XWayland sets both and winit may still take
          the X11 path.

          Verified by type-checking krate-adapter-linux for
          x86_64-unknown-linux-gnu, so the cfg block really compiled rather
          than merely parsed, and by compiling and running the probe
          standalone. Not yet exercised on a real Ubuntu machine without the
          package -- that is the one step this Mac cannot do.
Environment note: `rustc` on this Mac resolves to Homebrew's, which shadows
          rustup, so `rustup target add` reports success while cargo still
          cannot find the target's std. Prefix PATH with
          ~/.rustup/toolchains/<toolchain>/bin to check Linux code here.

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

### K-013 — apps/krate-bigscroll has no manifest, so it is not a runnable app
Status:   fixed 2026-08-13 (ed42658) -- and it was seven apps, not one
Owner:    lead
Severity: annoyance
Class:    our-code
Found:    2026-08-05, W12, running check-app across apps while verifying K-001
Evidence: `krate check-app apps/krate-bigscroll` prints
          "FAILED at layout / apps/krate-bigscroll is not an app directory:
          manifest.toml is missing". The directory holds Cargo.toml, Cargo.lock
          and src/ and has never had a manifest (`git log -- apps/krate-bigscroll`
          shows only 1d799c1, a workspace-root change). Pre-existing, unrelated
          to the scroll work.
Fix:      2026-08-13, commit ed42658. Kept rather than deleted: it is a real
          limitation probe, five hundred rows in one Scroll container, and it
          prints `rows:500` so a script can assert the whole tree was built.
          Verified end to end -- all six stages pass and the run prints
          `rows:500`.

          A sweep found the same defect in six more: krate-calc,
          krate-convert, krate-dashboard, krate-filetree, krate-settings and
          krate-timer, 229 to 531 lines each. All seven now have manifests
          written from what each app actually imports.

          Adding them immediately earned their keep. krate-dashboard failed
          the usability stage the first time it could be checked at all:

              the window grew to 652x396 but the app kept drawing at its
              old size, so the host had to stretch that picture to fit

          Its `draw_chart` already laid out from `canvas2d::canvas_size` and
          was right about the new size the moment it ran -- it just never ran
          again, because the event loop matched only `CloseRequested`. Fixed
          in the same commit by redrawing on `Resized`, the shape
          `krate-chart` already had.

          `apps/` now has no manifest-less directories.
Note:     krate-bigscroll renders on a light background while every other app
          in apps/ is dark. Cosmetic, out of scope here, not filed -- it is a
          probe rather than a shipped example.

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

### K-029 — Our development history leaks into every app a user makes
Status:   fixed 2026-08-13
Owner:    lead
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
Fix:      2026-08-13. Both comments in the generated Cargo.toml rewritten to
          explain the line to the person who now owns the app, with no
          reference to how we found the problem:

            before: "It was missing here, so every windowed app the agent
                     wrote had to discover the missing dep through a failed
                     build and add it back by hand."
            after:  "Your app is `#![no_std]`, so this supplies the pieces
                     Rust would normally take from the standard library...
                     Keep it even if you never call `krate::` yourself,
                     because without it the app will not link."

          Swept the rest of the generator for the same shape. The two other
          past-tense notes are inside tests, where explaining why a check
          exists is correct, and they are not shipped.
Why it mattered more than "annoyance" suggests: a tester read these notes in
          an app they had just made and concluded they had been handed a
          pre-built template, then stopped. A comment addressed to us inside
          a stranger's file does not just read oddly, it makes the product
          look like a fake.

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
Status:   fixed -- retested 2026-08-13 as the entry asked; no longer reproduces
Owner:    lead
Severity: serious
Class:    example-bug
Found:    2026-08-05, W17, outsider testing, via `--shoot`
Evidence: The 5th button in a row wraps and draws on top of the next row's
          text. In the expense tracker the "Add expense" button -- the app's
          most important control -- is covered by the "Other" category button.
          The chess board renders 7 columns instead of 8, sheared. The three
          apps that look right all use rows of 2-3 buttons; the maze escapes by
          drawing to a canvas.
Fix:      Nothing to do. K-002 (text measurement) fixed this, exactly as this
          entry predicted, and the retest it asked for was finally run.

Retest:   2026-08-13. A copy of apps/krate-calc widened from rows of four to
          rows of five, so every row has a fifth button -- the precise
          claimed failure.

            window 420 wide   all four rows of five lay out correctly.
                              No wrap, no overlap, no shear.
            window 300 wide   five 60px buttons need 300px plus gaps, so the
                              fifth wraps to its own line. It wraps cleanly:
                              `--check-layout` reports "no text drawn over
                              other text".

          So the original symptom -- "the 5th button wraps and draws on top of
          the next row's text", "the chess board renders 7 columns instead of
          8, sheared" -- does not happen. What is left is a container too
          narrow for its fixed-width children wrapping them, which is
          ordinary layout, not a collapse.

          Screenshots: scratchpad k018/fivewide.png (300 wide, wraps) and
          k018/wide420.png (420 wide, correct).

Lesson:   This sat open for eight days with "re-test after K-002 merges"
          written on it, and K-002 was already fixed. Same shape as K-024:
          the cost of an unclosed entry is the next person re-deriving it.

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

### K-024 — krate-pulse pins its canvas to constants, so it ignores a resize
Status:   fixed (ee6cfef); board entry was stale, verified 2026-08-13
Owner:    lead
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
Fix:      Done in commit ee6cfef ("Let krate-pulse follow its window: it was
          pinned to 1080x700"). Verified 2026-08-13: the canvas nodes now
          carry `width: None, height: None, grow: 1.0` so they grow with the
          window, and the event loop redraws on resize.

              $ krate check-app apps/krate-pulse
              OK ... usability passed

          The entry sat open because nobody closed it, which is its own
          lesson: a fixed bug left open costs the next person the time to
          re-derive that it is fixed.

Original plan: Same shape as K-003: drop the fixed style on the canvas node,
          lay out from `canvas2d::canvas_size`, and handle `Event::Resized`. Left
          unclaimed rather than fixed here, because K-003 is W13's and this is
          the same repair on a second app -- it should go with that work.

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
Status:   fixed 2026-08-13 (627c66d)
Owner:    lead
Severity: serious
Class:    runtime-hole
Found:    2026-08-05, Yashraj, using a generated app
Evidence: Clicking the native close button sometimes leaves the window open
          with the pointer in the spinning-wait state. The app can be closed
          from the terminal instead. Read the code: windowShouldClose returns
          false and defers to the app, the callback is queued and drained, and
          the host's wait loop pumps native windows -- so the wiring is
          present and the fault is elsewhere. Not yet reproduced under a
          debugger.
Diagnosis: 2026-08-13, from the code. The original suspicion was right, and
          the mechanism is one step earlier than "the callback arrives":

          The guest runs on the MAIN thread, which is the same thread AppKit
          needs in order to deliver `windowShouldClose` at all
          (`MainThreadMarker`, adapter-macos/src/appkit.rs:1271). AppKit is
          pumped from exactly two places, `ui::events::poll` and
          `ui::events::wait` (phase3_gui_host.rs:2086 and :2194) -- both
          inside the guest's own event handling.

          So an app doing heavy work BETWEEN waits starves AppKit completely.
          The click does not sit in a queue undelivered; the delegate never
          runs, so the close is never even observed. That is exactly the
          reported symptom: the window stays open and the pointer shows the
          spinning wait cursor, which is macOS's own "this app is not
          responding" indicator, not a Krate cursor.

          This also explains "sometimes": an ordinary event loop or frame loop
          calls poll or wait every round and pumps constantly, so it never
          shows. It needs an app that computes for a long stretch without
          calling either.

          Ruled out on the way: the event path itself is intact
          (windowShouldClose -> queue -> handle_callback -> CloseRequested),
          every shipped app handles CloseRequested, and the wait loop's blind
          sleep is 10ms rather than the 250ms park, so a close arriving while
          the app IS in `wait` is delivered promptly.
Not reproduced under a debugger: the starvation needs a windowed session, and
          a `quick` run exits before the busy stretch. A probe was built and
          discarded rather than left claiming a measurement it never made.
Fix:      2026-08-13, commit 627c66d, by epoch interruption -- NOT option 1.

          Option 1 was wrong and reading the pump proved it: this runtime
          drives its own manual pump (`nextEventMatchingMask` in a loop)
          instead of `[NSApp run]`, so no CFRunLoop is turning and an observer
          would never fire. Option 2 cannot work either, because a pure
          computation makes no host calls, so there is no boundary to hook.

          Epoch interruption is the only mechanism that does not need the
          guest to cooperate. A background thread ticks the engine epoch every
          8ms; wasmtime runs the callback when the guest next crosses a
          safepoint, it pumps the native loop and returns `Continue` so the
          guest carries straight on. `Continue` and not `Interrupt`: this
          keeps the window answering, it does not end runs. The wall-clock
          budget and fuel are unchanged.

Cost:     Real, and gated because of it. Epoch checks go on every loop
          back-edge, so they cost compile time on every app that carries
          them. One binary built both ways, nothing else changed, six
          alternating runs of krate-savings:

              gated off   0.226s
              forced on   0.480s

          Roughly double. So it is enabled only when the run can actually
          open a window (`Phase3HostUiMode::can_open_a_window`). A headless
          run has no close button; a CLI app can never open a window at all.
          Neither should pay for a mechanism it cannot use.

Evidence: pump counts, via `KRATE_EPOCH_STATS=1`, against a probe whose guest
          runs 400 million iterations making zero host calls:

              busy guest, windowed   60 pumps   <- structurally 0 before
              ordinary GUI app        3 pumps
              CLI app                 gate off entirely
              headless                0 pumps

          The 60 is the fix: sixty chances for AppKit to deliver the close
          during work that previously froze the window solid.
Not verified by hand: reproducing the original symptom needs a person to click
          a close button during a long computation on a real window. The pump
          count shows the mechanism now runs where before nothing could, which
          is the part that was missing.

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

### K-114 — macOS canvas apps scroll like the 90s: line jumps and swap flicker
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-16, founder, scrolling an AI-written notes app side by side
          with MarkText during the first head-to-head benchmark: "it's like
          text on the screen is getting replaced by new text, with some
          vanish and reappear effect" while MarkText was supersmooth.
Evidence: Two mechanisms compound. The macOS canvas path is still the
          CPU-composited NSImageView pipeline with no vsync pairing, so a
          redraw can reach the glass mid-swap -- the vanish/reappear.
          Windows and Linux left this path in v0.1.26 (GPU presenter,
          AutoVsync); macOS is the deferred S5 stage of
          Plan/GPU-Presenter.md. Numbers from the same session: the Krate
          app wins start (156ms vs 1800ms warm), footprint (117MB vs 635MB
          across Electron's five processes), size (210KB vs 107MB) -- and
          loses the scroll feel decisively, which is the row adoption
          actually reads.
Fix:      S5 shipped for canvases: each drawn canvas now presents on a
          vsynced Metal surface (PixelPresenter -- one persistent
          GPU-backed view per canvas, one write_texture + blit per frame,
          atomic swap), with the NSImageView composite kept as the
          no-Metal fallback. Measured on the M4: "canvas presents on
          Apple M4 (Metal, IntegratedGpu)", present interval p50 17.3ms /
          p99 17.7ms -- a 0.4ms spread where the old path had no pacing at
          all. The replica editor renders mid-pixel scroll offsets through
          the same surface. K-115 (teaching) covers the app-side half.

### K-115 — Generated apps scroll by whole lines because the pack never taught pixels
Status:   fixed
Owner:    lead
Severity: major
Class:    teaching-hole
Found:    2026-08-16, same session as K-114, reading the generated notes
          app's source after the founder called out the feel.
Evidence: The app moves its viewport one line per wheel notch. The runtime
          has delivered pixel-precise wheel deltas since K-001, but the
          authoring pack's examples and prose never show pixel-offset
          scrolling, so agents quantize to lines -- every scrolling app
          generated to date has the same 90s feel on every platform.
Fix:      The pack already taught pixel offsets for lists, and the agent
          still quantized -- for TEXT views the natural mental model is a
          line index, and the offset gets rounded on the way in. The pack
          now names that exact anti-pattern ("never round the offset to
          whole lines") and shows the split: first = scroll/line_height,
          within = scroll%line_height, draw from list_y - within. A partly
          visible line at top and bottom IS the smoothness.

### K-116 -- A machine with no real GPU crashed apps instead of falling back
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-16, automated v0.1.30 test on the Azure Windows VM
          (Hyper-V Video, no GPU), first windowed run of a canvas game.
Evidence: wgpu offered WARP -- "Microsoft Basic Render Driver (Dx12, Cpu)"
          -- as an adapter, the presenter took the GPU path, swapchain
          creation failed with DXGI 0x887A0022, and wgpu's DEFAULT
          uncaptured-error handler panicked the process:
          "thread 'main' panicked ... wgpu error: Validation Error".
          The plan's acceptance line "the fallback stays green on machines
          with no usable GPU" is exactly what failed: wgpu reports most
          failures asynchronously, so the render path's Result was never
          the whole story.
Fix:      Two layers in presenter-gpu: a software adapter (DeviceType::Cpu)
          is declined up front -- WARP rasterizes on the CPU behind a DX12
          mask, slower than our own CPU painter -- and every device gets an
          on_uncaptured_error handler that records the failure instead of
          aborting, retiring that window to the CPU painter on the next
          frame.
Status:   fixed

### K-150 -- main has not compiled for Windows since v0.1.51, and nothing noticed

Class: our-code
Owner: claude
Status: fixed

`open_app` lost its `#[cfg(target_os = "macos")]` after v0.1.51. Its body uses
`std::os::unix::process::CommandExt`, `Command::exec` and `krate_adapter_macos`,
so `main` stopped compiling for Windows entirely -- nine errors. v0.1.51 still
has the gate, so no release ever shipped this; every commit on `main` since is
broken.

**Why nothing caught it, which matters more than the attribute.** CI does run a
Windows job, and it runs `cargo test --workspace --lib`. `--lib` builds LIBRARY
targets only, and `krate-cli` has no lib target -- it is a binary. So `main.rs`,
the largest file in the project, was never compiled on Windows by any ordinary
CI run. Six commits landed over the break.

Cross-checking from the Mac would not have caught it either: `cargo check
--target x86_64-pc-windows-msvc` cannot get past the C dependencies (zstd,
sqlite, whisper) without an MSVC toolchain, so it never reaches this code.

Two fixes:

- The gate is restored, with a comment saying why it must stay.
- CI's cross-platform job now also runs `cargo check --workspace --all-targets`,
  which covers binaries, examples, tests and benches. That is the check that
  would have failed on the first bad commit instead of the sixth.

Verified on a real Windows 11 24H2 machine (the Azure parity VM, K-148):
`EXIT=0`, `krate.exe` built at 33.4 MB, `--version` and `doctor` both run.

### K-147 -- A camera frame's size was reported once and then wrong forever

Class: our-code
Owner: claude
Status: fixed

The webcam app opened, the camera light came on, frames flowed -- and the
window was black.

`camera::info` reported the size the app REQUESTED, not the size the device
actually opened at. A Mac asked for 640x480 delivers 1920x1080, so the guest
laid 1080p bytes out as though they were 480p: it read three rows into the
first and painted noise, which reads as black. Nothing errored anywhere, and
the indicator light said the camera was working, because it was.

Two rounds to fix, and the first one was not enough:

1. `info` now prefers the size the device really delivered (a new
   `PlatformStream::observed_size`, learned from the first frame). Still
   black -- because the app called `info` **once**, immediately after `open`,
   before any frame existed. It followed the pack correctly and still got the
   wrong number.

2. So the size moved onto the frame itself: `frame.width` and `frame.height`
   travel with the bytes and cannot disagree with them. That is the real fix.
   An API where the sizes live apart makes this mistake available; one where
   they arrive together does not.

Verified live: feed on screen, correct aspect, photo button working.

The lesson worth keeping: when a generated app uses an API correctly and still
breaks, the API is the defect. The first fix was right about the value and
wrong about the shape, and only the shape change made the mistake unavailable.

### K-146 -- An app's defining capability, marked optional, is never granted or even mentioned

Class: teaching-hole + our-code (check)
Owner: claude
Status: fixed

The generated webcam app marked `camera.capture` **optional**. Only required
capabilities are put to the person, so it was never granted and never even
mentioned: the app opened to a permanently empty viewfinder, no prompt, and
nothing on screen explaining why. The same shape as the music player that
opened silent because `audio.playback` was optional.

The pack already taught this ("Cannot begin without" means the app it CLAIMS to
be) and the AI still got it wrong, so judgment alone is not enough:

- `manifest_overreach` now REFUSES a manifest that marks `camera.capture` or
  `audio.capture` optional. Narrow on purpose -- these are never reached for in
  passing, so their presence at all means the app is about them.
- The failure carries its own fix text. The combined paragraph told an app
  whose real mistake was an optional camera to go rescope its fs globs.
- The pack names the camera in the required-capability list, and says what an
  optional one actually costs.

Evidence -- the exact app, before the manifest was corrected:

    $ krate check-app .
    FAILED at manifest
    the manifest asks for camera.capture but marks it optional. The person is
    only asked about required capabilities, so this one is never granted and
    never even mentioned -- the app opens without it, with nothing on screen
    to say why.

### K-145 -- The dock-name re-exec cost every macOS permission

Class: our-code
Owner: claude
Status: fixed

`open_app` re-execs the engine through a hard link named after the app, so the
dock shows the app's own name rather than "krate". That link was a bare
executable in `~/.krate/launchers`, outside any bundle -- so the running
process had no Info.plist, and macOS refuses camera and microphone access to a
process that cannot say why it wants them. Silently: the permission request
returned instantly, the status stayed `not-determined` forever, and no dialog
was ever shown.

The link now lives inside a minimal generated `.app` carrying
`NSCameraUsageDescription` and `NSMicrophoneUsageDescription`, ad-hoc signed so
macOS has a stable identity to record the decision against. The executable is
still a hard link to the engine, so this costs no disk and stays correct across
upgrades. `CFBundleIdentifier` is per-app, which means each app the person
opens is asked about separately -- one app's camera grant is not another's.

### K-119 -- No camera capability: apps that need one cannot be built yet

Class: runtime-hole
Owner: claude
Status: fixed

Filed 2026-08-16 as a capability audit, with "do it when the first real app
needs it". That app arrived: "an app that shows my webcam feed with a photo
button" is the request the whole authoring pipeline was being measured on, and
it was the one thing Krate could not do at all.

Shipped end to end:

- **`wit/krate/phase3/deps/camera/camera.wit`** -- `devices`, `open`, `info`,
  `start`, `stop`, `read`, `close`. Shaped like `krate:audio/capture` so there
  is one media idiom, with one deliberate difference: frames are **pulled**,
  and only the newest is kept. A queue would mean an app drawing at 30fps from
  a 60fps camera falls further behind every second.
- **`camera.capture` capability**, never granted by default. The camera is the
  most sensitive door Krate opens -- somebody's face and their room, with no
  moment where they choose what it sees.
- **`camera_capture.rs`** -- the shared, platform-free half: the newest-frame
  slot, a 64-stream cap, config validation, and the rule that a stopped stream
  drops the frame it captured while running (otherwise an app that "stopped
  looking" still has a picture taken while the light was on).
- **`camera_macos.rs`** -- AVFoundation behind a `CameraBackend` trait, so a
  second platform implements only what is genuinely different. BGRA sample
  buffers convert to the straight-alpha RGBA `canvas2d::draw_pixels` already
  takes, delivered on a private serial queue that never touches the main
  thread.
- **`NSCameraUsageDescription`** in both `scripts/make-macos-app.sh` and
  `studio/Info.plist`. Without it macOS *terminates the process* the instant
  capture starts, with no catchable error; the backend checks for the key and
  returns a sentence instead.
- **Pack teaching**, plus the WIT itself now rendered into `krate-mode`, so an
  AI can see the API exists.

Two failures worth recording because both cost time and neither was obvious:

1. **macOS never prompts on its own here.** AVFoundation only shows its dialog
   implicitly when an input is created on the main thread with a run loop
   turning, which a Krate app is not doing at that point. Measured: status
   stayed `not-determined` for ten seconds of polling, session running, zero
   frames, no error. The fix is to call
   `requestAccessForMediaType:completionHandler:` explicitly.

2. **Waiting for the answer freezes the app.** The first version blocked
   `open` on the completion block. That is the same "app looks frozen" failure
   a blocking network fetch causes (K-101), and worse here -- the thing the
   person must click sits on top of the window frozen behind it. It now fires
   the request and returns; AVFoundation delivers nothing until access is
   granted, then frames simply begin. The app's loop keeps turning, which is
   why the pack tells it to hold the last frame and paint "waiting for the
   camera" as a state.

Proof it works, from a probe in a signed bundle, after the prompt was allowed:

    auth status before: "not-determined"
      waiting 0, status now "authorized"
    FRAME 8294400 bytes at 780ms

8,294,400 bytes is 1920x1080 RGBA -- a real frame off the MacBook Air camera.

Still open elsewhere: Windows and Linux have no backend, and report
`unsupported` with a sentence naming the system rather than failing silently.

### K-143 -- The click check judged the frame before the app had redrawn

Class: our-code
Owner: claude
Status: fixed

Filed as "a lowered-widget app's buttons do nothing". They do not: **the check
was looking too early.**

An event-loop app handles a press like this: `wait` hands it over, the app
matches it, drains the rest of the queue with `poll`, and only then redraws.
Both `wait` and `poll` step the usability script, so the judging visit arrived
on that drain -- after the app had matched the press, before it had repainted
-- and captured the old frame.

Caught by instrumenting the guest itself. Its own trace, in order:

    DBG pointer arm entered
    DBG widget=7
    krate-frame: labels=["Countdown", "Ready", "05:00", "Start", "Reset"]   <- judged here
    DBG rebuild ok running=yes                                              <- app reacts here

The app was working the whole time. The check called it broken.

Fixed by requiring both conditions before judging: the app has had
`PRESS_SETTLE` (250ms) of wall clock, and the visit comes from `wait` -- the
point where the app has finished this turn. `PRESS_GIVE_UP` (1.5s) judges
anyway for a frame-loop app that never calls `wait`; gating on `wait` alone
stalled the script there and reported a working game as having closed itself,
which is how that second condition was found.

After the fix, the same press on the same app:

    krate-check: click difference=0.001917 idle_churn=0.000000 confident=true answered=true

Two things worth keeping from this. The strengthened check from K-140 is only
trustworthy because it now looks at the right moment -- a stricter threshold
against a stale frame produces confident false failures, which is worse than
the permissive check it replaced. And a "broke" verdict from this gate now
carries real weight, so it must never be believed without reproducing what the
person sees.

### K-142 -- The quiet heartbeat contradicts the step it is shown under

Class: our-code (UX)
Owner: claude
Status: fixed

`THINKING` was one flat array rotated regardless of the lit step, so a person
watching "Writing the code" was told "the writing starts once it has read
enough". Now keyed by step, with lines that are true of each: reading, writing,
testing, packing. `thinkingLine()` reads the lit step and picks from that set.

### K-141 -- On macOS, full-bleed apps ignore every click and scroll

Class: our-code
Owner: claude
Status: fixed

The macOS adapter flipped AppKit's bottom-up pointer and wheel coordinates
against `contentLayoutRect` -- the area below the title bar. A full-bleed
window's content includes the title-bar band, so every click and scroll landed
about a title bar too high. The header row is what that offset moves out of
reach, which is where a person clicks first, so the app read as completely dead
to input while still animating.

`effective_content_rect` already existed for exactly this question (added for
K-117) and is what the drawing path uses. `capture_mouse_event` and
`capture_wheel_event` simply never called it. Both now do.

This was the founder's "not clickable or scrollable" on both generated weather
apps. Both call `window::set_full_bleed(win, true)`, and the pack encourages
full-bleed, so this hit a growing share of generated apps -- and it was ours,
not something those apps did wrong.

### K-140 -- The click check let a spinner answer for a dead button

Class: our-code
Owner: claude
Status: fixed

The gate passed both apps of K-141 on every check while neither could be
clicked. A gate that certifies broken apps is worse than none.

- `usability::press_answered(difference, idle_churn)` replaces the old
  `difference > 0.0`. The driver spends one extra settle visit measuring how
  much the app changes with nobody touching it, and the press must beat that by
  `IDLE_CHURN_MARGIN` (3x). A spinner can no longer answer for a dead button.
- An animating canvas app that does not visibly react now reports *unobserved*
  with the animation named as the reason, instead of silently passing.
- `KRATE_PRESS_AT=x,y` aims the driven press at a control whose position is
  known, instead of guessing at the canvas centre.

Pinned by `a_spinner_cannot_answer_for_a_dead_button`, which uses the real
measured numbers: 0.007% idle churn must not pass, and the 2.86% of a real
chip press must.

### K-139 -- codex can never author: the agent is started in the wrong directory

Class: our-code
Owner: claude
Status: fixed

`run_provider_author` now calls `command.current_dir(app_dir)`. A sandboxed
agent decides what it may write from its own working directory, and codex's
`workspace-write` roots exactly there; without this every write to the app was
rejected as "outside of the project", so codex could never author anything.

Also `agent_refusal` now falls back to `agent_refusal_in_transcript` when the
file is missing. An agent that correctly refuses -- "Krate has no camera API"
-- but is blocked from writing `CANNOT-BUILD.txt` still said so in its output,
and throwing that away turned a clear, correct answer into a generic build
failure. Kept strict: only the explicit `KRATE-CANNOT-BUILD:` marker counts,
and only its last occurrence, so an agent that muses about impossibility and
then finds a way is not read as refusing.

### K-137 -- Concurrent fetches spawn unbounded OS threads, and the host panics when they run out

Class: our-code
Owner: claude
Status: fixed

Found while chasing the founder's report that `ios.krate` "crashed -- a system
crash message appeared on screen". Both weather apps run fine headless and both
fetch live data, so the crash was not the app misbehaving: it was the runtime
dying underneath it.

`AsyncFetches::begin` spawned one OS thread per in-flight request, uncapped,
via `std::thread::spawn` -- which panics rather than returning an error when
the OS is out of threads. Measured ceiling on this machine: 4095 threads, then
`Resource temporarily unavailable (os error 35)`. A panic there kills the whole
runtime, which is what produces an OS crash dialog instead of a failed request.

Ordinary apps can reach it because `cancel` cannot kill a running worker -- it
retires the handle and the thread runs on until the request's own timeout.
`ios.krate` starts one request per saved city on every refresh, so repeated
refreshes stack workers faster than they retire.

Two changes, both in `crates/runtime/src/async_fetch.rs`:

- `MAX_IN_FLIGHT = 64`. Past it, `begin` returns an error the guest can act on
  ("wait for one to finish") instead of starting a 65th worker. Well above what
  a real app needs at once, far below where the OS gives out.
- `Builder::spawn` instead of `thread::spawn`, so a genuine spawn failure is an
  `Err` the caller reports, never a panic that takes the runtime with it.

`begin` now returns `Result<u64, AdapterError>`; `phase2_host.rs` maps the
refusal to a `net-error` the app already knows how to handle.

Pinned by `past_the_cap_a_request_is_refused_instead_of_crashing`, which fills
the cap, asserts the next request is refused, and asserts that freeing one slot
lets the next request through -- so the cap cannot regress into either a crash
or a permanent lockout.

Verified after the fix: both apps still fetch live data.

    $ ./target/release/krate run ~/"Krate Apps"/ios.krate --auto-grant -- quick
    ... hours:24  days:7  frames:8  saved:yes

    $ ./target/release/krate run ~/"Krate Apps"/weather-dashboard.krate --auto-grant -- quick
    ... source:live  observed:2026-08-20T12:00

### K-134 -- No linker is reachable on Windows, and the one that ships is never named

Class: our-code
Owner: claude (fixed in v0.1.46)

Diagnosed over SSH on the machine itself rather than from a support report.
It has rustup, cargo, cargo-component, and wasm32-wasip1 installed for BOTH
toolchains. It has no linker either toolchain will look for:

    gnullvm -> error: linker `x86_64-w64-mingw32-clang` not found
    msvc    -> error: linker `link.exe` not found

No clang, gcc, or Visual Studio anywhere. rustup installs neither linker. So
the probe was right to reject both toolchains and the message was true -- but
the remedy, "reinstall the gnullvm toolchain", could never work, because the
missing piece is not in that toolchain. Five releases of reinstalling changed
nothing, exactly as expected.

gnullvm does ship a working linker, `rust-lld.exe`, in its own directory.
Nothing pointed rustc at it. Naming it is the whole fix. Measured on that
machine, same crate, same toolchain:

    BEFORE (no linker var): error: linker `x86_64-w64-mingw32-clang` not found
                            BEFORE_EXIT=101
    AFTER  (linker named):  AFTER_EXIT=0

`rust-lld.exe`, not the `gcc-ld\ld.lld.exe` shim beside it: rustc invokes a
gnu-flavored linker with `-flavor gnu`, which the shim rejects
("lld: error: unknown argument: -flavor") and rust-lld accepts.

The earlier entry under this number blamed a missing wasm target. That was
wrong -- the target is installed for both toolchains. Corrected above.

K-130's probe compiles a throwaway crate to decide whether a toolchain can
link. It compiled that crate with `--target wasm32-wasip1`. On a machine where
the wasm target is not installed yet, the probe fails with "can't find crate
for `std`" -- not because the linker is missing, but because the target is.
Every toolchain is then judged broken, `missing_create_tools` reports **"a
linker for Windows"**, and the run stops. The step that would install the wasm
target sits further down the same function and never runs. The machine is told
to reinstall a toolchain that was never at fault, and the real gap is never
named. Reinstalling changes nothing, which is why it survived several versions.

What a fresh Windows install reports (support report `3a41d8442fe8f9cb`,
krate v0.1.43, windows x86_64) -- every tool present, and it still fails:

    rustc:           C:\Users\user\.cargo\bin\rustc.exe
    cargo:           C:\Users\user\.cargo\bin\cargo.exe
    cargo-component: C:\Users\user\AppData\Local\Krate\bin\cargo-component.exe
    error: finish the toolchain setup, then re-run

Evidence -- the probe crate, built exactly as K-130 built it, fails on a
machine that builds Krate apps perfectly well:

    $ cargo build --quiet --target wasm32-wasip1
    error[E0463]: can't find crate for `std`
      = note: the `wasm32-wasip1` target may not be installed

Built for the host instead, the same crate succeeds and links its build
script, which is the only thing the probe was ever meant to test:

    $ cargo build --quiet
    exit=0
    $ ls target/debug/build
    krate-linkprobe-221dcb396dd15151

And it still fails when a linker really is absent, so the check keeps its
teeth:

    $ CARGO_TARGET_..._LINKER=/nonexistent-linker cargo build --quiet
    error: could not compile `np` (build script)

Fix: probe the host. A build script is a host artifact; compiling for wasm to
learn whether the host can link was the error.

Fixed by: 2b860920 (in v0.1.46). Two attempts shipped broken before this one,
both from rewriting the toolchain probe rather than leaving it alone:

  v0.1.44 probed the HOST only, so a toolchain that links but has no wasm
  target passes. On a clean windows-2022 that is the only toolchain there is,
  and every build died at "failed to find the `wasm32-wasip1` target".

  v0.1.45 probed host THEN wasm. It rejects that toolchain correctly and is no
  better, because the fallback in `rustup_toolchain_bin` selects it anyway and
  nothing downstream reads the verdict. Measured on the runner: FIXED=FAIL.

Measured on a clean windows-2022 -- exactly one toolchain, and it fails wasm:

    stable-x86_64-pc-windows-msvc   HOST=pass  WASM=fail  has_cargo=yes
    rustup which cargo -> ...\stable\bin\cargo.exe

Building for wasm is the honest probe: the build script still links, so the
host linker is exercised AND the target is, in one command -- and a merely
missing target gets installed on demand by rustup underneath cargo-component.
So the probe went back to what worked for three releases, and the only thing
kept is naming gnullvm's own `rust-lld.exe` as the linker.

Verified on windows-2022 BEFORE tagging this time, with the release verifier's
own command: `check-app apps/krate-bounce --no-run` -> RESULT=PASS, all four
stages (layout, manifest, build, imports).

Lesson worth keeping: the release verifier caught both bad releases and I
treated it as a formality twice. It is the only Windows signal that runs on
every tag.

### K-133 -- The headless run budget skipped every game, so a finished game never exited

Class: our-code
Owner: (fixed)

A headless run is bounded by a five-second wall-clock budget so an app that
waits forever still gives the terminal back. `headless_budget_close_request`
had exactly one caller: `ui::events::wait`. A game does not wait. It drives
itself with `poll` and `key-held` at frame rate, so the budget that was
supposed to bound it was never once consulted, and the bound silently did not
apply to the entire class of app most likely to loop forever.

The failure reads as a broken app. The NES-style game the authoring agent
built is correct and paints a good frame: it wrote its screenshot in about
twenty seconds and then ran until an outer watchdog killed it. Nothing in that
output says the runtime forgot to ask a question on one code path.

Evidence:

    $ krate run "nes-game.krate" --shoot nes.png     # before
    PNG appeared after ~20s
    ... still running at 180s, killed by the harness

    $ krate run "pulse.krate" --shoot pulse.png      # a non-game, for contrast
    pulse: exit=0 elapsed=5s png=yes

Fixed by: asking the budget on the poll path too, and giving it teeth. `poll`
now checks `headless_budget_close_request` before returning an event, and the
budget follows the same two-strike contract as the close button and Ctrl-C
(K-121): the guest is told its window closed, gets a two-second grace period to
save and leave, and if it is still asking for events after that the runtime
exits the process. A budget an app can decline forever is not a budget.

    $ krate run "nes-game.krate" --shoot nes2.png    # after
    game: exit=0 elapsed=6s png=yes

Regression test: `a_spent_budget_ends_a_game_loop_that_only_polls` drives a
window with `poll` alone and asserts a spent budget closes it. The existing
wait-path test still passes; 324 runtime tests green. Windowed runs are
unaffected -- the budget returns early unless the host is headless.

### K-111 -- Painted app UI renders at 1x on Retina displays: everything looks soft

Class: our-code
Owner: unclaimed

Every painted surface (the vello/painter path GUI apps draw with) renders its
bitmap at logical point size, and the macOS adapter displays it scaled up on a
2x display. The result is that a generated app's whole UI -- text, lines,
icons -- looks low-resolution next to any native app. Native-lowered widgets
(NSButton, NSTextField) are sharp; the painted regions around them are not.

Evidence: side-by-side screenshot of the Cup Cook app on a Retina display,
2026-08-15 -- list text and numerals visibly fuzzy at normal viewing size
while the window chrome is crisp. `grep backingScaleFactor
crates/adapter-macos/src/appkit.rs` shows the scale is read (lines ~1467,
1524) but the painted frame is sized in points, not pixels.

Fix shape: render the frame at points x backingScaleFactor and mark the
image rep's logical size in points, so AppKit scales down (sharp) rather than
up (soft). Layout stays in points; only the raster density changes. Needs the
painter, the frame plumbing, and the adapter to agree on the factor, and a
--shoot comparison at 1x and 2x as proof.

Fixed by: K-088's scaled canvas work, verified 2026-08-18 and closed with
evidence: a live window measured 1200x832 points capturing at 2400x1664
pixels -- a true 2.0x, no upscale -- and the markdown replica's text is
sharp at native density in that capture. The headline was stale; the
painted path has rastered at display density since the canvas surface
started taking window_scale.

### K-129 -- Three attempts at one game each started from nothing, and the pack made the AI hunt
Status:   fixed
Owner:    lead
Severity: serious
Class:    teaching-hole
Found:    2026-08-17, founder ran the same Contra-style request three
          times; all three died at the 15-minute ceiling.
Evidence: Every session record shows the same shape: request, plan in
          ~6 seconds, build starts, "that build didn't come together"
          exactly 15 minutes later. The kept transcript of the last
          attempt ends with the agent's own words -- "Now I'll write the
          game." -- after 14 Bash calls spent grepping the WIT and SDK
          for key names, audio, and draw-sprite. Zero lines of game had
          been written when the clock fired, and the studio handed create
          a fresh temp dir every attempt, so the stall message's promise
          ("it resumes from the code already written") was false: each
          retry restarted the research from nothing.
Fix:      (this commit) The pack gains one game section carrying every
          fact a game asks for -- the loop, held vs one-shot input, both
          sprite paths, the complete generated-chiptune recipe (open a
          stream, load_sound, play_sound, with a working square-wave
          blip), pattern-based pixel art with no asset files, and the
          scope rule. Plus K-127's key-name table. And the studio now
          builds each session in its own stable workspace, so a retry
          genuinely resumes.

### K-132 -- The app flashes on screen mid-build and nothing says it is us
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-18, founder watching a game build: the app appeared and
          vanished three or four times and played its sounds, while the
          build card said "16. reading frame.png".
Evidence: The authoring loop runs `check-app --shoot`, which really does
          open the app and render a frame -- that IS the AI looking at its
          work. From outside it is a window flashing and audio playing with
          no explanation, and the one clue on screen was a filename. A
          first-time person reads that as their machine misbehaving.
Fix:      (this commit) Three places now say it plainly. The engine's
          progress words: "opening your app to see it -- a window may
          flash", "running your app to test it", and reading the rendered
          frame becomes "looking at how your app turned out". The rail
          warns BEFORE the first flash, as part of starting a build. And
          the build card marks itself while such a step is live, adding
          "a window may appear for a moment -- that's Krate testing your
          app" exactly when the flash happens.

### K-131 -- A spinner outlived its build, so waiting looked exactly like working
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-18, founder watching his friend's PC: "Understanding what
          you asked for" for ten minutes with, as ssh confirmed, only the
          studio process alive -- no engine, no agent, nothing building.
          Second occurrence of the same shape in two days.
Evidence: The build screen animated from a local flag set when the request
          was sent. Nothing ever verified that a process existed, so any
          build that died unseen (K-128's stale slot refusing the request,
          an engine crash, the machine sleeping) left the screen spinning
          indefinitely. The founder's words: a new user "might think that's
          building their app".
Fix:      (this commit) The studio proves liveness instead of assuming it.
          A new build_alive command answers from the process table, and the
          build screen polls it every four seconds: a dead process ends the
          build through the normal failure path with plain words and a
          retry. A second rule covers the other shape -- if the engine has
          said nothing at all within ninety seconds, the plumbing is broken
          rather than the app, and it says so. Both clear the stale slot on
          the way out. Five rule cases tested (dead, healthy, silent,
          young-and-quiet, too-young-to-judge).

### K-130 -- A half-installed gnullvm toolchain is chosen and then cannot link
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-18, first real support report (69dcfc7c0450e541) from the
          founder's Windows PC: every app build failed within minutes of
          installing v0.1.41.
Evidence: The report's about.txt named the machine, and `krate doctor`
          there showed rustup carrying TWO toolchains, gnullvm and msvc,
          with the build dying at "program not found" compiling
          wit-bindgen-rt's build script -- a missing C linker. gnullvm was
          listed by rustup but its lib/rustlib/<target>/bin/self-contained
          directory did not exist and no clang or gcc was on PATH: a
          half-finished install. Krate chose gnullvm because the NAME
          appeared in `rustup toolchain list`, routing every build into a
          toolchain that cannot link. MSVC was the active default but has
          no Build Tools either.
Fix:      5df2457b. gnullvm_toolchain_present() verifies the toolchain can
          link (its self-contained linker directory exists, or a system
          clang/gcc is reachable) before claiming it, so a broken install
          is treated as absent and the normal install/repair path runs.
          Same commit fixes the report's version stamp (it printed the
          crate version, not the running binary's) and makes the support
          console explain a refused token instead of showing nothing.

### K-127 -- A big request burned its whole budget hunting for key names we never documented
Status:   fixed
Owner:    lead
Severity: serious
Class:    teaching-hole
Found:    2026-08-17, founder asked for a Contra-style NES game in the
          studio; two builds in a row died at the 15-minute ceiling with
          nothing to show, the second after he watched a spinner for 45
          minutes.
Evidence: The kept workspace told the whole story. Eight minutes in, the
          agent had written 124 lines -- the untouched skeleton -- and 11
          of its 16 tool calls were a hunt for ONE fact: the strings
          key-held takes. It grepped the WIT (which says only "the same
          names key-event reports"), then the SDK, then ran `strings` on
          /Applications/Krate.app's binary. The 67 KB authoring pack
          contained the key names zero times.
Fix:      772d77f4. The pack states them in full -- ArrowLeft/Right/Up/
          Down, lowercase characters for letters and digits, Space,
          Enter, Tab, Backspace, Escape, Home, End, PageUp, PageDown,
          Delete -- with the note that getting them wrong is SILENT (the
          app builds, runs, never moves) and the two shipped games named
          as examples. The ceiling also moves 15 -> 40 minutes, because a
          full game stage is a legitimate request rather than a stuck
          agent, and a new stall watchdog ends a genuinely silent agent
          after 10 quiet minutes instead of letting it ride to the
          ceiling.

### K-126 -- Bundled source pins the SDK by absolute local path, so source does not travel
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-17, testing a Windows-built app's bundled source on the
          Mac to verify its interactivity.
Evidence: The .krate's source/Cargo.toml says
          krate = { path = "C:/Users/user/AppData/Local/Krate/sdk/..." }
          and the component.target WIT paths likewise. On any other
          machine (or after that user moves their SDK) check-app and
          revise fail at build with "No such file or directory". The
          bundle SHIPS the SDK precisely so source is editable later;
          the absolute paths defeat it.
Fix:      (this commit) The {KRATE_SDK} placeholder machinery existed on
          both sides all along; the pack-side rewrite matched the Unix
          cache shape /krate/sdk/ lowercase, and Windows materialises
          under AppData/Local/Krate/sdk/ with a capital K, so on Windows
          the rewrite never fired and the author's absolute path shipped.
          sdk_root_in is now case-insensitive and separator-tolerant,
          pinned by tests carrying the exact line from the live bundle.

### K-125 -- A GPU failure during surface configure killed the app instead of falling back
Status:   fixed
Owner:    lead
Severity: critical
Class:    our-code
Found:    2026-08-17, founder's friend's finance dashboard on the Iris Xe
          Windows PC: double-click flashed a terminal and nothing opened,
          while other apps ran fine.
Evidence: Reproduced over ssh: Surface::configure failed (0x887A0022),
          the log printed "GPU device error, retiring to CPU painter" --
          and the process then panicked inside wgpu's
          get_current_texture_view before the retirement could act. With
          panic=abort there is no catching it: one more surface call
          after a failed configure is fatal.
Fix:      283d6b3d. The device-failed flag is re-checked immediately
          before every get_current_texture (scene path, blit path, pixel
          presenter): a failure recorded during configure returns Err and
          the CPU painter takes over. The studio also stops treating "I
          can't open it" as a change request -- it runs the app headless
          itself and reports what the runtime said, with a Fix-it button.

### K-124 -- Every authoring failure said "sign in": /auth/ matched "author command failed"
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-17, founder ran a first-time user's real request (with
          an xlsx attached) on the Windows PC; Codex could not run any
          command because its own sandbox helper was missing, said so in
          its final message, and the failure card said "Your AI needs
          signing in".
Evidence: The card's own small print contradicted its headline. Studio
          plainWords() matched /sign ?in|auth|logged/ against the engine's
          generic "author command failed" -- "author" contains "auth", so
          the sign-in diagnosis fired on every authoring failure that
          reached the generic line. The agent transcript (fetched over
          ssh) showed codex-windows-sandbox-setup.exe missing and every
          exec failing with orchestrator_helper_launch_failed.
Fix:      3c55e91f. \bauth(?!or) so authentication still matches and
          "author" never does; a dedicated plain-words case for the
          broken-agent-sandbox signature in the studio AND the engine
          (which scans the transcript for it); conversational stopwords
          so "So i have made..." cannot name an app "so" (pinned by test
          against the live request). The deeper cure -- surfacing the
          agent's own last words instead of guessing -- rides with K-123's
          conversation work.



### K-123 -- The Studio builds whatever is typed: no questions, no plan, no context intake
Status:   fixed
Owner:    lead
Severity: serious
Class:    our-code
Found:    2026-08-17, founder's friend pasted a ChatGPT prompt that
          depended on Excel files in a folder; the Studio built without
          them and without asking. Founder separately typed "Sadas" and
          got an app named Sadas (live on the store).
Evidence: The authoring flow has no step between "user typed" and "agent
          builds": studio run_author -> krate create, one shot. Every
          chat AI users know asks a question when the request is thin and
          states its plan before acting; we compare to compilers, users
          compare to ChatGPT.
Fix:      Plan/Authoring-Conversation-2026-08.md is the plan of record:
          a `krate plan` engine door (ask-or-plan JSON, no build), the
          Studio conversing before building, attachment nudges when a
          request implies files, and xlsx->csv conversion at authoring
          time. "Sadas" must become a question, never an app.
Update:   2026-08-17, all four stages shipped. S1+S2 (2682a48a,
          6f669989): `krate plan`
          answers ask-or-plan in one JSON object (thin requests get their
          question deterministically, no AI needed; the friend's real
          budget prompt got "attach the Excel" as its first question in
          6.5s), and the Studio holds the conversation in the thread with
          "build it" as the permanent escape hatch. S3: spreadsheet
          attachments land with each sheet converted to CSV beside the
          original (calamine; pinned by test against a two-sheet fixture
          with comma cells). S4: the pack teaches embed-their-data and
          plain-words consent for personal-data apps. Validation left:
          one real first-time session through the shipped Studio.

### K-122 -- krate-glow taught request-redraw-per-frame, so generated animations spin a core
Status:   fixed
Owner:    lead
Severity: serious
Class:    example-bug
Found:    2026-08-16, an outside AI authoring an app from the pack named
          it: the host documents the self-feeding redraw loop at
          phase3_gui_host.rs ("the queue is never empty and the app looks
          busy forever") while the pack's own reference app commits it.
Evidence: apps/krate-glow called window::request_redraw every frame inside
          a loop that already draws on its own schedule, then
          events::wait(Some(16)) -- the redraw event comes straight back,
          the wait returns instantly, and the loop spins. Measured 94.3%
          of a core windowed and idle. As the pack's modern-UI reference,
          every generated animated app inherited the pattern (aurora did,
          at 101.8%).
Fix:      6af5b29d. Glow paces against a monotonic next-frame deadline and
          never calls request-redraw (94% -> ~67%, the remainder is K-120's
          raster floor); its quick path draws 12 synthetic frames, not 90.
          The pack states the rule where motion is taught: request-redraw
          exists to wake an idle loop, and a continuously animating loop is
          never idle.

### K-118 -- wasmtime 43 is out of security support; the upgrade needs rustc 1.94
Status:   fixed
Owner:    lead
Severity: major
Class:    our-code
Found:    2026-08-16, cargo-deny in CI, new advisory RUSTSEC-2026-0222
          (type-index confusion between engines) against wasmtime 43.0.2.
Evidence: Patched trains are >=46.0.2 / >=47.0.3; both pull cranelift
          crates requiring rustc 1.94.0, and the workspace pins 1.91.1, so
          `cargo build` refuses before any API question is reached. The
          runtime creates exactly one Engine per process, so the advisory's
          cross-engine mixing has no practical surface today -- which is
          why a dated deny.toml ignore is honest in the meantime, and why
          this entry exists so the ignore does not quietly become policy.
Fix:      473dbe47. Toolchain pin, workflows and workspace rust-version to
          1.94.1; wasmtime to 46.0.2. No component API drifted between 43
          and 46 -- workspace builds and the full test suite passes
          unchanged. deny.toml's ignore list is empty again. The embedded
          guest SDK still declares rust-version 1.91 so app authors are
          not forced up by a host-side dependency.

### K-121 -- The close button cannot end an app that ignores CloseRequested; the machine paid for it
Status:   fixed
Owner:    lead
Severity: critical
Class:    our-code
Found:    2026-08-16, founder ran an AI-authored Aurora.krate windowed,
          clicked close repeatedly, nothing happened; with the app also
          spinning a core (K-120, since paced) the machine locked up and
          had to be power-cycled.
Evidence: adapter-macos/appkit.rs windowShouldClose always returns false
          and forwards a CloseRequested to the guest -- by design, so an
          app can save on the way out. phase3_gui_host.rs then counts the
          requests and close_ignored_by_guest() fires on the second click,
          but the only consumer (events::poll) just `return Ok(None)`:
          the "close the window ourselves" the doc comment promises was
          never implemented. Worse, only `poll` counts the clicks --
          `wait` and `key-held` deliver CloseRequested without noting it,
          so a game loop reading keys never trips the threshold at all.
          The window has no path to death except Ctrl-C in the launching
          terminal, which a double-click launch does not have.
Fix:      f7486da9. Every real event is counted once, where the dispatcher
          yields it in poll_one_event (so the pending-queue round trip
          cannot double-count a single click), and the second unanswered
          request ends the process from poll, wait, and both presents --
          a game loop lives in present and may never call the others.
          Same two-press contract as Ctrl-C. Threshold pinned by test
          the_second_unanswered_close_request_is_the_runtime_s_to_honour.

### K-113 — "Change my app" replaced it with a stranger
Status:   fixed
Owner:    lead
Severity: blocker
Class:    our-code
Found:    2026-08-16, founder, revising a finished NES-style track game in
          the studio: "change the controls" produced an unrelated generic
          runner app that overwrote the game he loved.
Evidence: The revised bundle fell from 37 KB to 16 KB and drew a plain blue
          "Hold an arrow key to run" screen with none of the game in it.
          Mechanism: revise unpacks the bundle's own source and passes it as
          create's work dir, but create joined `work_dir.join(name)` where
          `name` is inferred from the REQUEST -- for a change, that is the
          change sentence -- so the agent landed in an empty directory
          beside the source, the never-produced-source wipe gave it a fresh
          skeleton, and it built "a runner with arrow keys" from one line.
          The fallback for source-less bundles was worse: with no history
          entry it rebuilt from the change text alone by design.
Fix:      26e3a98. A change adopts the unpacked source directory itself
          (`is_existing_app_workspace`), and a source-less bundle with no
          history refuses in plain words instead of rebuilding from the
          change sentence.
Proof:    `krate revise` on the same track game with a start-card wording
          change edited 8 bytes of 43,665: TRACK DASH / HURDLE / SPRINT all
          still in the source, the new wording present, and a `--shoot` of
          the revised bundle renders the same stadium, crowd and runner.
Status:   fixed

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
