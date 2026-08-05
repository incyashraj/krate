# The Krate App Benchmark -- first run

Date: 2026-08-05. Machine: M-series Mac.
Binary: `target/release/krate` at commit `778768c`, copied to a scratch path and
invoked by absolute path for every request, so a mid-run rebuild by another
workstation could not change what was being measured.
Agent: `--agent grok` (`agent --single`, grok 0.2.14).

## The number

**4 of 9 attempted requests produced a usable app. 44%.**

Read that as a first calibration, not a product verdict: 9 of 42 is a small
sample, and the four passes are all in the `refuse` tier, which costs no
authoring at all. **Of the five requests that were actually authored, zero
passed.** That is the honest headline and it is worse than 44% suggests.

| Tier | Score | Notes |
|---|---|---|
| refuse | 4/4 | correct fast refusals, 0 seconds each |
| easy | 0/5 | all five authored, built, ran, painted -- and none was checkable |
| medium | not run | machine contention, see below |
| hard | not run | expected to be mostly red; K-001/K-002/K-003 |

The old measure would have scored these same five apps **5 of 5**. Every one of
them builds, imports only `krate:*`, runs headless, and paints a frame. That gap
-- 5/5 against 0/5 on the same artifacts -- is the entire reason this benchmark
exists.

## Why the run is 9 requests and not 42

Not quota, and not the product. Between three and five other workstations were
running `krate create` on this machine throughout, against the same grok
account and the same build cache. The harness refuses to start beside another
authoring run for exactly this reason, and it fired repeatedly. The nine
requests here were taken in windows between other runs, with
`ALLOW_CONCURRENT_RUNS=1` set deliberately and knowingly.

This is recorded rather than hidden because a contended machine inflates wall
times (mean 104s here) and could plausibly cause a spurious failure. It did not
cause the failures below -- every one of them is a specific, reproducible
property of the app that was written, visible in its source.

## The failures, ranked by cause

### One cause explains all five: K-013, teaching-hole

Every authored request failed gate 3, "does what was asked", and every one
failed it the same way: **the app works and cannot prove it.**

| # | Request | What it printed | Why it failed |
|---|---|---|---|
| 1 | a tip calculator | `bill:60 tip%:18 people:2 total_cents:7080` | correct maths, all on one line, keys named `tip%` and `total_cents` |
| 2 | a to-do list I can check things off in | `items:5` / `saved:yes` | never reported whether anything was checked |
| 5 | a click counter | `count:0` | the self-exercise run never clicked before printing |
| 6 | a dice roller that rolls two dice | *(nothing)* | never calls stdout at all; draws dice to canvas only |
| 9 | a stopwatch with lap times | `stopwatch:ok` | the "I ran" pattern, carrying no state |

Request 1 is the clearest case. The app is **right**: a 60 bill, 18%, split 2
ways, 7080 cents. A person would be happy with it. It scored 1/4 because it put
three keys on one line and invented its own names.

Request 6 is the most serious. It painted a valid 15KB frame and its source
contains no stdout call anywhere -- so its state is invisible to this benchmark,
to the K-006 usability stage, and to CI. It could be rolling two dice or zero
and nothing automated could tell.

**The cause, from the pack itself.** `krate krate-mode` lines 52-57 are the
entire specification of what the verification run should print:

> do the app's real work once against a small built-in sample, **print
> something**, and exit 0

"Print something" is the whole contract. The pack's own worked example does the
right thing -- `write_pair(&stdout, "timezone", &timezone)`, one `key:value` per
line -- and 17 of 17 shipped bundles follow it. But the rule is only ever
*demonstrated*, never *stated*, so an agent is free not to copy it, and five out
of five did not.

Filed as **K-013**, class `teaching-hole`, unclaimed. It is one paragraph in the
pack and it is the highest-leverage entry on the board right now: it does not
need runtime work, and until it lands, no automated check can distinguish an app
that works from one that does not.

### What was NOT the cause

Worth stating, because these were the predicted failure modes and none of them
fired in this sample:

- **Not the runtime.** Zero failures at the imports gate. Zero `wasi:*` leaks.
- **Not stability.** All five painted a frame and exited cleanly. Nothing hung,
  nothing crashed, nothing closed itself (K-009 stays fixed).
- **Not authoring reliability.** 5 of 5 requests produced a building, running
  app. The reliability corpus's 14/14 is not contradicted -- it is confirmed and
  shown to be measuring a lower bar.
- **Not the known blockers.** K-001 (scroll), K-002 (text measurement) and
  K-003 (resize) are real and will show up in the `hard` tier, but no request in
  this run reached them.

## The refuse tier: 4/4, and it is fast

All four impossible requests were refused in **0 seconds** with a named limit:

| # | Request | Limit named |
|---|---|---|
| 39 | download my email and show me the unread ones | `host-app` |
| 40 | a chat app so I can message my friends | `another-device` |
| 41 | sync my files to my phone | `another-device` |
| 42 | an app that posts to my twitter account | `third-party-account` |

None produced a `.krate`. This is the one part of the system measured here that
is working exactly as intended, and it is a real improvement: the same request
39 once spent 673 seconds building a 1511-line mail client over invented data.

## What this benchmark does not measure yet

Stated plainly rather than shown as a green tick that checks nothing.

- **Resize is not enforced.** The stated pass bar includes "survives a window
  resize". The runtime has no scripted-input path, so nothing can tell an app
  the window changed size and observe what it does. K-003 is the known defect
  and W13 owns it. Until a resize can be injected, this gate is declared, not
  checked.
- **Click is carried, not isolated.** The `quick` path is the app operating its
  own controls, so a broken click surfaces as unchanged state (exactly what
  request 5 shows). That is real evidence, but it is not the same as a synthetic
  click at a coordinate.
- **The `medium` and `hard` tiers are unrun.** 33 of 42 requests have no result.
  The score above must not be quoted as if it covered them.

## Corrections and harness defects found this run

Recorded because a benchmark that hides its own bugs is not worth trusting.

1. **The first draft required every app to print `ready:1` and `quick:done`.**
   Nothing teaches those keys, so every app would have failed them and the score
   would have measured an invention of the harness. Dropped before any number
   was published; "stays open" is now evidenced behaviourally.
2. **A dead agent account was scored as a product failure.** `--agent claude`
   failed a request in 4 seconds with "OAuth session expired" in the transcript.
   Four seconds is far too fast to have authored anything. Now recorded as
   `skipped`, like a quota rejection -- the same lesson that once turned 14/14
   into a reported 23%, arriving through a different door.
3. **Four asserts tested spelling, not capability.** Rows 14, 30, 33, 34 wanted
   `saved==1`; real apps write `saved:yes`. An app that genuinely saved would
   have failed on the name. Relaxed to `!=no` and recorded in the corpus change
   log. Caught against the shipped bundles *before* the run, not after failing
   one.
4. **The concurrency guard had false positives**, matching its own wrapper shell
   and any grep containing the phrase, and it blocked `--dry-run`, which authors
   nothing. A guard with false positives gets disabled and then protects
   nothing.
5. **An early alarm about cross-contamination was wrong.** Another workstation
   was authoring a similarly-named app at the same moment, and I briefly
   concluded my result had been polluted. Re-running my own `.krate` directly
   proved the output was mine. The wrong conclusion is recorded alongside the
   right one because "I suspected contamination and disproved it" is a different
   claim from "there was none".

## Correction to a board entry

**K-007 said this machine's AI accounts are unusable and called it a blocker.**
That is now too strong, and it matters because it is the difference between "we
cannot measure" and "we can". Checking all four providers:

- `claude -p` -- "OAuth session expired and could not be refreshed"
- `codex exec` -- "requires a newer version of Codex"
- `copilot -p ... --allow-all-tools` -- exits 1 with **empty stdout and empty
  stderr**, which is worse than the other two because nothing says why
- **`agent --single "Reply with exactly: ALIVE" --output-format json` -- exits 0
  and returns `{"text":"ALIVE","stopReason":"end_turn",...}`**

Grok works, `krate ai` lists it as ready, and
`crates/cli/src/agent_provider.rs:519` registers it. This entire run was
authored with it. K-007 has been downgraded to `serious` with that evidence
added, and a note to reach for `--agent grok` before reporting that authoring
cannot run.

## Next

1. **Fix K-013.** One paragraph in the authoring pack: on `quick`, print one
   `key:value` per line, bare values, no units or symbols. `write_pair` already
   does this and is already in the pack -- promote it from example to rule. Then
   re-run these same nine requests. Nothing else on this list is worth doing
   first, because until it lands every score is measuring output formatting
   rather than capability.
2. **Run the medium and hard tiers** on an uncontended machine. Budget roughly
   3 minutes per request, so about 100 minutes for the remaining 33.
3. **Do not publish 44%.** Publish "0 of 5 authored apps were machine-checkable,
   and here is the one-paragraph fix". The 44% is arithmetically true and
   rhetorically useless, because the four passes cost no authoring.
4. **Wire the resize gate** once a scripted-input path exists (K-003, W13), and
   delete the caveat above rather than leaving it to rot.
