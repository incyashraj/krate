# When can we say "any small or medium app ports to Krate and runs everywhere"?

No system is ever stable. So the question is not "is it finished" — it is
**what would have to be true for the claim to survive contact with strangers**,
and how do we know when it is.

This is that list. Every gate is a thing that either passes or does not, on a
machine, without anyone's judgement involved. The point of writing them down is
that we cannot move the goalposts later, in either direction.

## The claim, stated exactly

> A person takes a small or medium program they did not write, runs one
> command, and gets a single file. That file opens on macOS, Windows, and
> Linux, does what the original did, and can only do what it says on the
> outside.

Six testable parts: **someone else's program**, **one command**, **one file**,
**three systems**, **same behaviour**, **declared limits**.

## Where we actually are, 2026-08-01

Measured, not estimated:

| | |
|---|---|
| Real third-party ports | **5** — hexyl (CLI), savings (GUI), ddh (filesystem), rssfwd (network), envelope (database) |
| Capability keys | 34 |
| Capabilities the analyzer can suggest | 18 of 18 requestable (13 more are default-granted) |
| Widgets working on all three systems | 13 of 17 |
| Languages the pipeline can build | 1 (Rust) |
| **Bundles built on one OS and opened on another** | **9 of 9** |
| Three-OS lanes green | **yes** — all three pass |

The last two rows are the ones that matter. Everything else is progress; those
two are the claim itself.

## The gates

Ordered by what blocks what. A gate is not "done" until its check runs in CI on
every push or nightly, because a check nobody runs is not a check — that is not
a theory, it is what happened here: nine jobs sat behind a commit tag typed once
in the repo's history, and in the four weeks they did not run, seven projects
broke and a security advisory in the wasm engine went unnoticed.

### Gate 1 — Three systems are actually green

**Check:** the full matrix passes on macOS, Windows, and Linux, nightly.

**Status: GREEN as of 2026-08-01.** All three lanes pass. The Windows failure
that surfaced on the first nightly run was a test fixture handing a Windows
temp path to bash, which eats backslashes as escapes; fixed the same day.

Nothing else on this list means anything until this is green. A portability
claim with a red lane is not a claim, and every gate below is measured *per
OS*, so a broken lane invalidates all of them at once.

### Gate 2 — One file, three computers, proven

**Check:** a `.krate` built on Linux is downloaded by the macOS and Windows
lanes, opened there, and produces byte-identical output. Same for a bundle
built on each of the other two. Nine runs, three bundles.

**Status: PROVEN, all nine openings, 2026-08-01.** Every bundle opens on every
system, from the run's own logs rather than a claim:

```
bundle from macos-latest   opened and saved on Linux
bundle from ubuntu-latest  opened and saved on Linux
bundle from windows-latest opened and saved on Linux
all 3 bundles opened on Linux, each with its own identity
```

and the same three on macOS and on Windows. A `.krate` authored on Windows runs
on Linux and saves its data — a sentence that had never been true in a test
before today.

Nine of nine. "One file, any computer" is now a checked claim rather than a
description.

### The nightly itself is not yet proven

The schedule (`cron: "0 3 * * *"`) is live on main and the workflow is active,
but **no scheduled run has fired yet** -- it landed a few hours before the first
window and GitHub commonly delays scheduled runs. Every full run so far has been
a manual dispatch.

That matters for the two-week clean-nightly condition: the clock cannot start
until a scheduled run has actually happened on its own. Check for one before
counting any days.

### Gate 3 — Ports keep working, not just worked once

**Check:** a nightly job ports a fixed set of real third-party programs from
source and asserts each one builds, packs, runs, and produces expected output.
A regression in the SDK, the analyzer, or the runtime fails that job.

**Status: script written, wired nightly, and it skips on CI.**

`scripts/port-regression.sh` clones three proven programs at pinned commits,
ports each with the real pipeline, and checks the output still matches. It is
wired into the nightly run.

But the transform step drives an AI agent, and a GitHub runner does not have
one, so the nightly job prints this and exits 0:

```
no 'claude' on PATH, so the transform step cannot run.
Skipping without failing; run it where the agent is installed.
```

That is honest -- it says exactly what it did not do -- and it means **this
gate is currently green without verifying anything**. Skipping loudly beats
failing every night for a reason that is not a regression, but neither is
coverage.

**Half of it is now closed.** `scripts/replay-ported-apps.sh` runs every ported
bundle -- they are committed under `evidence/ported/`, 64 KB for four -- and
checks each still produces its real answer. No agent involved, so it runs on
every push, on all three systems.

That splits the gate cleanly:

- **Does a ported app still work?** Covered, everywhere, every push. A runtime
  or bundle-format change that would break every existing app is caught the day
  it lands.
- **Does porting still produce one?** Not covered. That needs the agent, and it
  is the half that skips.

The remaining half needs an agent available to CI, a recorded transcript to
replay, or a scheduled run on a machine that has one.

The set should start at the two we have and grow by one every time a new shape
is proven — never shrink.

### Gate 4 — Enough shapes to generalise

**Check:** at least **six** third-party programs ported, covering:

- [x] command line, byte-oriented (hexyl)
- [x] GUI, form and list (savings)
- [x] something that talks to the network (rss-forwarder, per-host HTTPS)
- [x] something with a real database (envelope, SQL + secrets + random)
- [x] something that reads and writes many files (ddh, duplicate finder)
- [ ] something genuinely medium — **5,000+ lines**

**Status: 5 of 6.**

Six is not magic. It is the smallest number where "it works on the shape I
tried" stops being the likely explanation. Two ports of two shapes is a
promising signal and nothing more, and saying "any app" on it would be
disproved by the third person who tries.

Note the last row: "medium sized" is currently an untested word. hexyl at 2,392
lines is our largest, and nobody has tried 5,000.

What the two ports cost, measured:

| | Source | Ported | Ratio |
|---|---|---|---|
| hexyl (CLI) | 2,392 | 1,133 | **0.47x** |
| savings (GUI) | 490 | 810 | **1.65x** |

A command-line program shrinks: argument parsing, colour crates, and terminal
detection all fall away because Krate supplies them or the app no longer needs
them. A GUI grows: an immediate-mode draw loop becomes an explicit widget tree,
which is more code saying the same thing.

That matters for the 5,000-line gate. A 5,000-line CLI is plausibly a
2,400-line port, which is a size already proven. A 5,000-line GUI is plausibly
8,000 lines of output, which is a different question and probably the harder
half of "medium sized".

### Gate 5 — The tooling can see what it needs to

**Check:** for every capability the runtime supports, the analyzer can detect
the source pattern that implies it, asserted by a test that enumerates the
capability list rather than sampling it.

**Status: DONE — 18 of 18.**

Measuring this properly changed what it meant. 13 of the 34 capabilities are
granted to every app by default, so suggesting `io.stdout` or `time.clock`
would be noise. The real target is the 18 an app has to ask for, and all 18 are
now detectable. The test enumerates the runtime's own list rather than sampling
it, so a capability added without a detector fails there instead of in
someone's port.

This is the gate that keeps paying out. Six defects found in one day, and every
single one was the tooling unable to *see* something rather than the runtime
unable to *do* it:

1. `stdio::write` — the contract listed rules but not one function
2. `random.bytes` — the #3 crate in Rust, entirely missing
3. Rust GUI toolkits — knew Qt and WPF, not egui or iced
4. `ui.window:create` — identified a GUI app, suggested no capabilities
5. random, clipboard, secrets — supported, undetectable
6. non-Rust languages — reported "needs changes" and prepared a Rust scaffold

Six for six. Treat it as a rule: **when a port fails, look at what the tools
could perceive before adding anything to the runtime.**

### Gate 6 — Failures are honest and cheap to report

**Check:** every port failure is classified, and each class carries a promise
we can keep. A test asserts no class can carry a deadline we cannot meet.

**Status: DONE.** Classifier ships with five classes; reporting is opt-in, shows
the full file, and never transmits. Worth keeping listed because it is what
makes Gate 4 self-feeding: real failures from real users are a better roadmap
than our guesses, and this is the only path by which they reach us.

### Gate 7 — The install path works cold

**Check:** on a clean machine of each OS, the published install command
followed by the published first-run command works, verified nightly against the
real published artifacts rather than a local build.

**Status: two of three systems.** `scripts/test-cold-install.sh` walks the
published path nightly on Linux and macOS: fetch the installer from krate.tech,
run it, confirm it still suggests a command, run that command and require the
permission wall to refuse with exit 5 in plain words, then grant and require
the app to work. Verified by hand on macOS: install, refuse, grant, run,
`note:first note`.

**Windows is now covered too**, by its own PowerShell walk -- a shell script
cannot test a PowerShell one. Same journey: fetch install.ps1 from krate.tech,
run it, require exit 5 without grants with a refusal in plain words, then grant
and require the app to work.

The Windows installer had the same GitHub rate-limit hole the Unix one did: it
queried the same API and died with the same unhelpful message. It now falls
back to reading the releases page and, failing that, says what happened and
gives a pinned command.

This is the gate people actually hit first, and the one most easily broken by
something outside the repository -- the release assets, the domain, the GitHub
API, the app file on the site. Any of those can break with no commit, which is
why the test uses the published URLs rather than a local build.

## What "stable" means here

Not "no bugs". It means:

1. **Gates 1 and 2 are green nightly.** Three systems pass, and one file
   demonstrably runs on all three.
2. **Gate 3 runs nightly and has not regressed** for two weeks.
3. **Gate 4 is at six shapes**, including one over 5,000 lines.
4. **Gate 5 stays at 100%** — every capability an app must request is one the
   tooling can spot, enforced by enumeration rather than by remembering.
5. **Gate 7 passes from the published artifacts**, not from a local build.

At that point the sentence at the top can be said out loud, with the caveat
that it means "the shapes we have proven", and the proof is a link to a nightly
run rather than a claim.

## What to do next, in order

1. **Fix Windows.** Gate 1. Nothing else counts while a lane is red.
2. **Build the cross-OS bundle test.** Gate 2. Highest value per hour of any
   work on this list, and the mechanism is already there.
3. **Turn the two ports into a nightly job.** Gate 3. Makes every later gate
   measurable instead of anecdotal.
4. **Port a network app and a database app.** Gate 4, and the fastest way to
   find the next three tooling gaps.
5. ~~Close the analyzer coverage gap.~~ Done: 18 of 18 requestable capabilities.

Nothing on this list is a new capability. That is deliberate: the evidence from
today says the runtime is not where the limit is.
