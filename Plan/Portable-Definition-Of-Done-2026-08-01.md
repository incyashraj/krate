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
| Real third-party ports | **2** — hexyl (CLI, 2,400 lines), savings (GUI, 490 lines) |
| Capability keys | 34 |
| Capabilities the analyzer can suggest | 10 |
| Widgets working on all three systems | 13 of 17 |
| Languages the pipeline can build | 1 (Rust) |
| **Bundles built on one OS and opened on another** | **0** |
| Three-OS lanes green | **no** — Windows failing as of today |

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

**Status: FAILING.** Windows fails `cargo test --workspace` as of 2026-08-01.
The same commit passes on macOS locally, so it is genuinely platform-specific.

Nothing else on this list means anything until this is green. A portability
claim with a red lane is not a claim, and every gate below is measured *per
OS*, so a broken lane invalidates all of them at once.

### Gate 2 — One file, three computers, proven

**Check:** a `.krate` built on Linux is downloaded by the macOS and Windows
lanes, opened there, and produces byte-identical output. Same for a bundle
built on each of the other two. Nine runs, three bundles.

**Status: NEVER TESTED.** Every lane today builds a bundle on its own OS and
runs it on that same OS. The single most-repeated sentence about this product
— one file, any computer — has no test behind it.

The mechanism already exists: `phase-component-fixtures` shares artifacts
between jobs. This is a day of work, not a project, and it is the highest-value
missing test in the repository.

### Gate 3 — Ports keep working, not just worked once

**Check:** a nightly job ports a fixed set of real third-party programs from
source and asserts each one builds, packs, runs, and produces expected output.
A regression in the SDK, the analyzer, or the runtime fails that job.

**Status: 2 ports, both by hand, neither repeatable.**

Both current ports were run manually and their results written down. Nothing
re-runs them. If a change broke porting tomorrow, we would find out from a
user.

The set should start at the two we have and grow by one every time a new shape
is proven — never shrink.

### Gate 4 — Enough shapes to generalise

**Check:** at least **six** third-party programs ported, covering:

- [x] command line, byte-oriented (hexyl)
- [x] GUI, form and list (savings)
- [ ] something that talks to the network
- [ ] something with a real database
- [ ] something that reads and writes many files
- [ ] something genuinely medium — **5,000+ lines**

**Status: 2 of 6.**

Six is not magic. It is the smallest number where "it works on the shape I
tried" stops being the likely explanation. Two ports of two shapes is a
promising signal and nothing more, and saying "any app" on it would be
disproved by the third person who tries.

Note the last row: "medium sized" is currently an untested word. hexyl at 2,400
lines is our largest, and nobody has tried 5,000.

### Gate 5 — The tooling can see what it needs to

**Check:** for every capability the runtime supports, the analyzer can detect
the source pattern that implies it, asserted by a test that enumerates the
capability list rather than sampling it.

**Status: 10 of 34.**

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

**Status: partly.** The installer is tested; the full cold path from
`krate.tech` on a machine that has never seen Krate is not.

This is the gate people actually hit first, and it is the one most easily
broken by something outside the repository.

## What "stable" means here

Not "no bugs". It means:

1. **Gates 1 and 2 are green nightly.** Three systems pass, and one file
   demonstrably runs on all three.
2. **Gate 3 runs nightly and has not regressed** for two weeks.
3. **Gate 4 is at six shapes**, including one over 5,000 lines.
4. **Gate 5 is at 100%** — every capability the runtime has is one the tooling
   can spot.
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
5. **Close the analyzer coverage gap to 34.** Gate 5.

Nothing on this list is a new capability. That is deliberate: the evidence from
today says the runtime is not where the limit is.
