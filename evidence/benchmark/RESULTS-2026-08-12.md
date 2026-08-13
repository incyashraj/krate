# The Krate App Benchmark -- second run

Date: 2026-08-12. Machine: M-series Mac.
Binary: `krate 0.1.12` at commit `aabab14`, copied to a scratch path and
invoked by absolute path, so nothing could rebuild underneath the run.
Agent: `--agent claude`.

The first run was 2026-08-05 and is preserved in `results-2026-08-05.tsv`.

## The number

**14 of 42 requests passed. 33%.**

**Excluding the four `refuse` requests, which cost no authoring: 10 of 38.
26%.**

Quote the second one. The refusals are correct fast rejections that write no
code, and counting them inflates the rate -- that is how the first run's
"44%" was mostly four freebies.

| Tier | Pass | Assert level |
|---|---|---|
| easy | 6/12 | 24/32 (75%) |
| medium | 2/18 | 24/47 (51%) |
| hard | 2/8 | 10/22 (45%) |
| refuse | 4/4 | -- |
| **total** | **14/42** | **62/105 (59%)** |

## The comparison with 2026-08-05

The old run attempted 9 requests, four of them refusals, and scored **0 of 5
on authored apps**. Five requests were attempted by both runs, under the same
pass bar:

| Request | 5 Aug | 12 Aug |
|---|---|---|
| tip calculator | fail | **pass** |
| dice roller | fail | **pass** |
| to-do list | fail | fail |
| click counter | fail | fail |
| stopwatch | fail | fail |

**0/5 to 2/5** on the like-for-like set, and 0/5 to 10/38 on a corpus four
times larger. Not a clean comparison -- the old run used `--agent grok` on a
contended machine -- so the honest statement is that the number moved up from
a floor, on a much bigger sample.

## Read the second number too

**59% of observable properties held** (62 of 105), and **14 of 28 failures
missed by exactly one assert**.

Both numbers are true and they say different things. The pass rate is what
matters to a person: an app that misses one property did not do what was
asked. But an all-or-nothing bar over ~2.7 asserts per request turns a 51%
assert rate into an 11% pass rate in the medium tier, and reading 11% as
"medium apps barely work" would be wrong.

## What the 28 failures actually were

Classified as each result landed, not reinterpreted afterwards. Every raw
output is in `2026-08-12/outputs/`, and `2026-08-12/show.sh <id>` prints the
request, the asserts and the app's text side by side.

| Cause | Count | Whose |
|---|---|---|
| a key name -- synonym, abbreviation, prefix | 10 | the measure's |
| self-exercise: described itself, never operated | 4 | ours, fixed |
| printed facts about the output, not the output | 2 | ours, fixed |
| under-seeded: too little data to show the behaviour | 1 | ours, fixed |
| a boolean where a count was wanted | 1 | ours, fixed |
| summary vs detail (asked for a tally, got the values) | 1 | the measure's |
| inverse (`over:yes` for `alive?`) | 1 | the measure's |
| corpus bug (`upper~ABC` needs literal "ABC") | 1 | the measure's, fixed |
| under-reported: printed a count, never the second property | 1 | ours |
| remainder, mixed/multiple causes | 6 | -- |

**Almost none of these apps were broken.** A base64 encoder printed real
base64, verified a round trip, tested known vectors and rejected bad input --
and failed on `round_trip` versus `roundtrip`, one underscore. A table app
measured five column widths in pixels and lost on `columns` versus `cols`. A
calculator proved all four operations and divide-by-zero handling, and failed
for not printing keys called `result` and `ops`.

**Ten of twenty-eight failures turn on a key name.** That is the single
clearest finding of this run, and it is filed as K-105.

\newpage
**Correction, made during run 3.** This document first recorded request 2 as
a plain product failure that "printed nothing at all". That was wrong. The
output preserver did not start until request 14, so requests 1-13 have no
archived stdout, and an absent file was read as an empty one. The TSV shows
request 2 held 1 of 2 asserts -- `items>=3` passed -- which is impossible for
an app that printed nothing. It printed an item count and never reported
anything checked.

**So this run contained zero plain product failures.** Every one of the 28
was a reporting gap or a measurement problem. That reads as better news for
the product than the original text, which is exactly why it is flagged rather
than quietly amended.

## What was fixed while the run was in flight

Deliberately none of it could flatter this run: the harness was pinned to the
2026-08-12 binary and the corpus was frozen. These are what the *next* run
tests.

- **K-102** -- `krate-mode` still told models to "print something", the exact
  contract under which five apps scored zero in August. The pack that
  `create` uses had been fixed; the paste-in prompt had not.
- **K-103** -- the pack said how to format a key and never what to name it.
  Corrected twice: the first version told apps to "use the request's own
  word", which request 20 disproved -- 75% of corpus keys do not appear in
  their request at all.
- **self-exercise** -- the pack said "print what the app is holding" while
  the benchmark assumes `quick` "drives the app through its own
  interactions". Two of our own documents disagreed, and the apps obeyed the
  one they were given.
- **seeding** -- "seed enough state that the numbers are interesting" was too
  vague to tell an app that a *long* list has to actually be long.
- **counts** -- print the number you have, not `yes`.

## Filed, not fixed

- **K-105** -- an assert cannot accept a synonym. Ten failures. The fix is
  `count|clicks>=1` in the corpus, plus a rule for the prefix case
  (`widest_column` for `widest`).
- **K-104** -- the 900 s budget is a ceiling on variance, not on work. The
  same request took 902 s and failed, then 378 s and passed. A timeout is
  recorded as `fail`, indistinguishable from a bad app.
- **K-103 (remainder)** -- the corpus needs a sweep, and the harness has no
  key-to-key comparison.

## Two predictions, both recorded before the evidence, both wrong

1. *"The naming fix will close the tier gap."* Disproved at request 20: 75%
   of corpus keys cannot be derived from the request, so teaching cannot
   reach them.
2. *"The hard tier will show that `quick` cannot demonstrate behaviour that
   only exists over time -- scrolling, ticking, bouncing."* Disproved three
   times: request 31 scrolled 11,896 pixels, request 36 ran 85 game ticks,
   request 38 bounced 4 times across 90 frames.

The second is the more valuable failure. Two suspected runtime limits came
off the list by evidence rather than argument.

## Method notes, so the number can be judged

- Requests 1-13 ran with `TIMEOUT_SECS=900`; 14-42 with `1800`, after
  request 14 was cut off at 902 s having completed 41 productive authoring
  steps. Request 14 was re-run at the higher budget rather than carried over,
  so no row mixes the two. See `2026-08-12/budget-note.txt`.
- An earlier attempt lost 13 requests to `API Error: Connection closed
  mid-response`. Those rows were deleted and re-attempted rather than counted
  as failures -- no code was written, so there was nothing to judge.
- Mean authoring time, 38 authored requests: **385 s**.
- Single-shot, one attempt per request. The same request has been observed
  taking 378 s and >900 s, so **this process has at least 2.4x run-to-run
  variance and the pass rate carries more noise than has been quantified.**
  Measure the spread before reading precision into 26%.

---

# Addendum, 2026-08-12: what the harness fixes recover

**This is a replay, not a new measurement.** The 26% headline above stands as
the measured result. What follows is what the same archived app outputs score
against the fixed harness and corpus -- it says how much of the gap was the
measure's, and it is not a new pass rate.

Replaying the 25 requests with archived output:

| | Passing |
|---|---|
| as measured on 2026-08-12 | 4 |
| against the fixed harness and corpus | **15** |

Eleven apps recovered, every one of which already reported the property under
a different name or shape. Audited individually:

    20  contact book   search:gra          for query
    21  CSV viewer     columns:5           for cols
    23  markdown       list_items:11       for bullets
    24  bar chart      largest:31.5        for max
    26  memory game    moves:3             for flipped
    28  mood tracker   logged:12           for recorded
    29  dashboard      panels:3            for a `values` tally
    34  table          columns / widest_column
    35  text wrap      lines:18            for wrapped
    36  snake          over:yes            for alive
    38  bouncing ball  balls:9 bounces:4   for x/y

Two of those alternatives are judgement calls rather than plain synonyms and
are flagged in the commit: `flipped|moves` and `wrapped|lines`.

**The bar did not move.** Ten tests pin it (`test-assert-alternatives.sh`): an
absent key still fails, `items:0` still fails `>=1`, and `total:$289.93` is
still not a number. A do-nothing app fails every edited assert.

What this means, stated carefully: **roughly two thirds of this run's
failures were the measure disagreeing with the app about a word, not the app
failing to work.** The next full run is what turns that into a real number,
and it will also be the first to test the five teaching fixes, which could
not affect the run they were found in.
