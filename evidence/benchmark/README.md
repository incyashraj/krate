# The Krate App Benchmark

A public, reproducible measure of whether an app Krate authors is **usable**,
not merely valid.

## Why this is not the reliability corpus

`evidence/reliability/` asks "did authoring succeed" -- did something build,
import only `krate:*`, and run. That measure once reported 14 of 14, and on the
same day a real person was handed an app they could not click.

Both numbers were true. Neither was a measurement of the thing that matters.

This benchmark asks the harder question: would the person who typed the request
get an app that does the thing, survives a resize, answers a click, and stays
open.

## The pass bar

A request passes only if the app:

1. **builds and imports only `krate:*`**
2. **does what was asked** -- the per-request observable properties in
   `corpus.tsv` all hold
3. **survives a window resize**
4. **responds to a click**
5. **stays open** rather than closing itself

Bars 1, 2 and 5 are enforced today. Bar 4 is carried by bar 2, because the
`quick` self-exercise path is the app operating its own controls -- an app whose
click handling is broken cannot report changed state. **Bar 3 is not enforced
today** and the harness says so rather than showing a green tick that checks
nothing: the runtime has no scripted-input path, so nothing can tell an app the
window changed size and watch what it does. See K-003 on the bug board.

## How "does what was asked" is machine-checkable

This is the hard part and the reason the corpus is a TSV rather than a list of
sentences.

Each request carries semicolon-separated **asserts** over `key:value` lines the
app prints during `krate run <app> -- quick`. A human writes them once, reading
the request; they are then locked in `corpus.tsv` and re-run forever with no
human in the loop.

    1  easy  a tip calculator  bill?;tip>=0;total>=0;total!=0

Five operators, deliberately tiny: `key>=N`, `key<=N`, `key==V`, `key!=V`,
`key~text`, `key?`. Anything richer needs a parser, and a parser nobody trusts
is worse than a bar nobody can game.

`quick` is not invented here. 27 of the 34 apps in `apps/` already implement it
as a self-exercise path that drives the app through its own interactions and
prints the resulting state. The benchmark makes the house convention a
requirement.

### What was rejected, and why

- **A locked reference screenshot.** Fails on any font, scale, or theme change,
  so it needs re-judging constantly and rots into a rubber stamp within a month.
  Worse, a pixel diff cannot tell "the counter incremented" from "a counter is
  painted on screen" -- which is exactly the distinction between a working app
  and a picture of one.
- **A model judging the screenshot.** Not reproducible, so the number stops
  being something anyone can be held to.
- **Exit code only.** That is what the reliability harness already does, and it
  is what let a 1511-line mail client over invented data score as a pass.

## Files

    corpus.tsv          the fixed request set. Append only; ids never change.
    results-<date>.tsv  one row per request: pass/fail, the gate, the reason.
    selfcheck-<date>.tsv  the harness run against already-shipped bundles.
    RESULTS.md          the score, the failures, and what each one needs.

    scripts/benchmark-run.sh        the harness. Resumable.
    scripts/benchmark-selfcheck.sh  validates the gates without an AI account.

## Running it

    cargo build --release -p krate-cli
    KRATE_BIN=/absolute/path/to/target/release/krate \
      AGENT=grok scripts/benchmark-run.sh

Resumable by design -- a full run is hours. Any id already in the results file
is skipped, so stopping and resuming is the normal way to use it.

Do not rebuild the binary mid-run. Half the results would measure one build and
half another, and the score would then be a number about nothing.

## The honesty rule

**Do not tune the corpus to make the number look good.** The `hard` tier exists
because it is currently mostly red. A benchmark that only contains what we pass
is marketing and will be found out.

Rows may be added. A row may only be **removed** if it proves ambiguous or
unjudgeable, and the removal is recorded in `RESULTS.md` with the reason. An
assert may only be **changed** with a note saying what changed and why -- never
silently, and never because we failed it.

The first honest number will be low. That is the point. A number that only moves
up when the product genuinely improves is the asset.
