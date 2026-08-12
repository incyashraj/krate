# Failure classification, recorded live as each result landed

Written while the run was in progress, so the reading is not a
reinterpretation after seeing the final score. Each line quotes what the app
actually printed.

## Genuine failures

  req 2  to-do list      printed NOTHING. No stdout call anywhere.

## Working apps scored as failures (K-103)

  req 5  click counter   wanted count>=1
                         printed  clicks:60 frames:61 counts:yes
                         -- clicked sixty times; key named `clicks`

  req 7  password gen    wanted password?
                         printed  length:32 chars:32 bits:191 distinct:21
                         -- works, never prints the password. Arguable:
                            a generator that hides its output is odd.

  req 8  BMI calculator  wanted height? weight?
                         printed  bmi:22.8 category:Normal
                                  height_cm:178 weight_kg:72.5
                         -- entirely correct; units in the key names

  req 9  stopwatch       wanted elapsed>=0
                         printed  elapsed:2:20.18 elapsed_ms:140180 laps:4
                         -- correct; formatted as time, so the numeric
                            compare cannot read it

  req 10 case converter  wanted upper~ABC
                         printed  upper:HELLO WORLD. IT'S A FINE DAY!
                         -- CORPUS BUG: `~` is a literal substring match,
                            so this only passes if the sample text happens
                            to contain "ABC"

  req 13 habit tracker   wanted marked>=1
                         printed  habits:4 days:7 done-today:2
                                  best-streak:5 saved:yes
                         -- 4 habits, 7 days, 2 marked; key named
                            `done-today`

  req 15 expense tracker wanted entries>=3 total>=1
                         printed  expenses:9 total:$289.93 month:$289.93
                                  categories:4 added:yes saved:yes
                         -- 9 entries and a real total; `expenses` not
                            `entries`, and the `$` makes total non-numeric

## A third kind, found at request 16

  req 16 shopping list   wanted items>=2 added>=1 removed>=1
                         printed  items:5 got:3 remaining:2 saved:yes
                         -- a snapshot, with no evidence it can add or
                            remove. Not a naming problem and not a broken
                            app: the pack told it to "print what the app is
                            holding", while the benchmark assumes `quick`
                            "drives the app through its own interactions".
                            The app obeyed the pack. The two documents
                            disagreed.

                            This matters beyond one request: with no
                            scripted-input path, self-exercise is the ONLY
                            evidence that an app's controls respond at all.

## Running tally

  genuine product failures: 1
  vocabulary / format:      7  (one of which is a corpus bug)
  contract disagreement:    1

The teaching fix for this shipped mid-run (K-103) but the run in flight was
already past those requests, so it cannot show here. That is the point of
recording it: the next run tests the fix, this one measures what the fix was
for.

## Two more of the self-exercise kind (req 17, 18)

  req 17 kanban board    wanted columns==3 cards>=3 moved>=1
                         printed  cards:6 todo:2 doing:2 done:2
                                  columns:3 saved:yes
                         -- a correct three-column board with six cards.
                            Missing only `moved`: it never demonstrates
                            moving a card between columns.

  req 18 calculator      wanted display? result? ops>=4
                         printed  display:8 evaluated:2 add:14 subtract:4
                                  multiply:45 divide:2.25 divzero:reported
                         -- it demonstrably performs all four operations
                            AND handles divide-by-zero. It failed for not
                            printing literal keys named `result` and `ops`.
                            This app proved MORE than was asked and still
                            scored zero on the request.

## A second measure, less binary than pass/fail

At 18 requests recorded:

  asserts held, all requests:   32 of 48   (67%)
  asserts held, failures only:  11 of 27   (41%)
  failures missing exactly one key: 6 of 11

The pass rate is the headline and should stay the headline -- an app that
misses one assert did not do what was asked. But "67% of observable
properties held" says something the binary number hides: these are mostly
near misses on reporting, not apps that do nothing.

## Why the medium tier scores worse, measured

At 19 requests the tiers look very different:

  easy    6/12 pass    24 of 32 asserts held (75%)
  medium  1/7  pass     9 of 19 asserts held (47%)

The tempting reading is "medium apps are harder and the AI is worse at
them". The corpus says otherwise. Counting asserts that name an ACTION --
added, removed, moved, toggled, marked, checked, selected, evaluated,
result, count -- rather than a piece of state:

  easy:    2 of 32 asserts name an action   (6%)
  medium:  7 of 47 asserts name an action   (15%)

Medium requests demand proof-of-operation at roughly three times the rate,
and proof-of-operation is precisely what the under-specified `quick`
contract did not produce (see req 16, 17, 18, 19). The apps themselves are
not obviously worse: the kanban board had three columns and six cards
correctly distributed, and the settings panel had five switches with three
on and a theme saved.

So the tier gap is mostly the same reporting gap, concentrated where the
corpus asks for it. That is a claim the next run can falsify: if the
self-exercise fix is the cause, medium should close most of the distance to
easy. If it does not, the difficulty reading was right after all.

## The correction that matters most (found at req 20)

  req 20 contact book    wanted contacts>=3 matches>=1 query?
                         printed  contacts:9 seeded:8 search:gra matches:1
                                  added:1 selected:1 stored:sqlite
                         -- it searched, matched, added, selected, and used
                            SQLite. It failed one assert: it called the
                            search term `search`, the corpus wanted `query`.

This one disproves the fix written for the others. The request is "a contact
book I can search". The rule shipped earlier said "use the request's own
word", so `search` was CORRECT by our own guidance and the corpus was the
thing out of step.

Swept the corpus to see how often its keys can be inferred from the request:

  corpus keys not present in their own request text: 79 of 105 (75%)

`bill` is not in "a tip calculator". `die1` is not in "a dice roller that
rolls two dice". `remaining` is not in "a countdown timer". Three quarters
of the expected names cannot be derived from what the app was asked for.

So the earlier prediction -- that the naming fix would close the tier gap --
was wrong before it was tested, and the tier-gap note above inherits the
flaw. The teaching now says what an app can actually do: prefer the ordinary
word, keep units and currency symbols out of numbers, and **when a name is
ambiguous print both** (`query` and `search`), since extra lines cost
nothing and a missing one is invisible. The rest belongs on the harness
side: alternatives in an assert, or publishing the expected names.

## Halfway audit (21 of 42), and a check on my own reading

Medium stands at 1 pass in 9. Before attributing that to the contract again,
here is every medium failure's actual output, so the claim can be checked
rather than believed:

  13  habits:4 days:7 done-today:2 best-streak:5 saved:yes
  15  expenses:9 total:$289.93 month:$289.93 categories:4 added:yes saved:yes
  16  items:5 got:3 remaining:2 saved:yes
  17  cards:6 todo:2 doing:2 done:2 columns:3 saved:yes
  18  display:8 evaluated:2 add:14 subtract:4 multiply:45 divide:2.25
      divzero:reported
  19  switches:5 on:3 theme:Solarized choices:4 textsize:140 saved:yes
  20  contacts:9 seeded:8 search:gra matches:1 added:1 selected:1 stored:sqlite
  21  rows:16 columns:5 quoted_fields:ok header:ok selected_row:3

  21 CSV viewer      wanted rows>=3 cols>=2
                     printed rows:16 columns:5 ...
                     -- 16 rows and 5 columns of parsed CSV, with quoted
                        fields and a header. Failed on `columns` vs `cols`.

Not one of these is an app that does nothing. Every one holds real state,
and several do more than asked (the calculator handles divide-by-zero; the
contact book uses SQLite).

**The caveat I owe this reading.** I have now explained twelve failures as
problems with the contract or the corpus, and a story in which nothing is
ever the product's fault should be distrusted. Two things keep it honest:
the corpus sweep is a number (75% of keys unguessable), not an opinion; and
the one prediction I made from this reading -- that the naming fix would
close the tier gap -- was disproved at req 20 and is recorded as wrong
above.

What would change my mind: an app that prints nothing, crashes, or reports
state that is plainly incorrect. Req 2 was the first kind. Nothing so far
has been the third.

## req 22, and the first failure the shipped fix would have caught

  req 22 JSON printer    wanted input? output~{ lines>=3
                         printed  valid:yes lines:18 objects:2 arrays:2
                                  values:10 depth:2 bad_json_rejected:yes
                                  output_bytes:191 style:2 spaces
                         -- eleven facts ABOUT the output, and never the
                            output. It parses, counts, measures depth, and
                            rejects bad JSON, but a pretty printer that
                            never shows pretty-printed text has not
                            demonstrated its one job.

Unlike the naming failures, this is not the corpus being unguessable. The
pack already says, in the section shipped earlier today: "If the app
generates something -- a password, a colour, an id -- print the thing
itself, not only facts about it." That is this case exactly, and the same
rule covers req 7 (a password generator that printed length, bits and
distinct-character counts, but no password).

So of the failures so far:

  genuinely unfixable by teaching (corpus name unguessable):  most
  fixable, and now fixed in the pack:                         req 7, req 22
  fixable, self-exercise:                                     req 16-19
  plain product failure:                                      req 2

That distinction is the useful output of this run. It says which part of the
gap is ours to close and which part is the measure's.

## req 23, and a note about reading the evidence

  req 23 markdown preview  wanted headings>=1 bullets>=1
                           printed  file:sample.md headings:6
                                    list_items:11 blocks:27
                                    source_lines:45 source_bytes:1187
                           -- 6 headings and 11 list items parsed from 45
                              lines. Failed on `list_items` vs `bullets`.

This one nearly went into the record as "printed nothing". The output file
was 95 bytes and my inspection command mangled it to empty. The TSV said 1
of 2 asserts held, which is impossible for an app that printed nothing --
that contradiction is what caught it.

The lesson is small and worth keeping: when a tool and a record disagree,
the record is usually right and the tool is usually the problem. There is
now a `show.sh` beside the results that prints the request, the asserts, and
the raw output without any pipeline in between, and the three earlier
requests read through the old command were re-checked against it. All three
were reported correctly.

## req 24, and a prediction recorded before the hard tier runs

  req 24 bar chart       wanted bars>=3 max>=1
                         printed  bars:6 total:95.75 largest:31.5
                                  typing:9 drawn:yes
                         -- six bars drawn from typed numbers. Failed on
                            `largest` vs `max`, the abbreviation case
                            flagged two requests earlier.

**Prediction, written now so it can be judged rather than adjusted.** The
eight hard-tier requests ask for something the medium ones mostly did not:
proof of dynamic behaviour over time.

  31 scrolled>=1   32 scrolled>=1   33 scrolled>=1  tail!=no
  34 measured!=no  35 wrapped>=2    36 ticks>=1 alive?
  37 strokes>=1 points>=2           38 bounces>=1

If the failures here look like the medium ones -- a working app with a
different key name -- then the reading in this file holds, and the corpus is
most of the gap.

If instead the apps cannot show scrolling, ticking, or bouncing at all, that
is a genuine capability finding: it would mean `quick` cannot demonstrate
behaviour that only exists over time, and no naming fix touches it. That
would be the most useful thing this run produces, and it is the outcome I
would bet on for 36 and 38 specifically.

The four `refuse` requests should pass trivially -- they cost no authoring
and passed 4/4 on 2026-08-05 -- so they will inflate the final rate. State
the number with and without them.

## req 25, and the split that matters for planning

  req 25 tic tac toe     wanted moves>=3 board? turn?
                         printed  players:2 mode:hot-seat moves:5
                                  wins-x:1 wins-o:0 draws:0 rounds:1
                                  winner:x over:yes
                         -- a complete game: five moves, X wins, hot-seat,
                            round tracking. It reported OUTCOMES and never
                            the state a player looks at.

`board` and `turn` are not in the request text, but they are the ordinary
words for tic-tac-toe state -- which is what the shipped rule says to
prefer. So this one the fix would catch.

At 25 requests the failures split three ways:

  Addressable by the guidance shipped today   7   (7,16,17,18,19,22,25)
  Not addressable -- corpus name unguessable  9   (5,8,9,10,13,15,20,21,23,24)
  Plain product failure                       1   (2, printed nothing)

**Where this classification is soft.** The line between "ordinary word the
app should have chosen" and "unguessable corpus name" is a judgement I am
making, and I am the one whose fix is being judged. `board`/`turn` I put in
the first group; `max` vs `largest` I put in the second, and someone could
argue `max` is the ordinary word for a bar chart's tallest bar. If that one
moves, the split is 8/8.

The safest reading: **roughly half the failures are ours to fix and half are
the measure's.** The next run tests the first half, and only the corpus or
an assert-alternatives operator touches the second.

## The medium tier, complete: 1 pass in 14

  req 26 memory game     wanted cards>=8 flipped>=2 matched>=0
                         printed  cards:16 pairs:8 matched:2 moves:3 won:no
                         -- 16 cards, 8 pairs, 2 matched in 3 moves. Failed
                            only on `flipped`: it reports matches, never
                            how many cards were turned over.

Asked the question the other way round -- what single change would have made
each medium failure pass?

  rename one key                 13, 15, 20, 21, 23, 24
  exercise the verb, report it   16, 17, 19, 26
  print state, not just outcome  18, 25
  print the output itself        22

**Every one is a reporting change. Not one requires the app to do something
it could not already do.** No medium failure needed a capability the runtime
lacks, a feature the app got wrong, or logic that did not work.

That is either the most important finding of this run or the clearest sign I
have been reading it the way I wanted to. Both are worth stating, so here is
the check anyone can run: the outputs are in `outputs/`, one file per
request, and `show.sh <id>` prints the request, the asserts and the raw text
side by side. If a reader disagrees with a line above, the evidence to
argue with is right there.

The hard tier is next, and the prediction recorded at req 24 stands
unchanged.

## req 27 passes, and it partly disconfirms my own hypothesis

  req 27 reading list    wanted books>=3 read>=1 unread>=1
                         printed  books:6 read:3 unread:3 saved:yes
                         -- PASS. And notice why: "books", "read" and
                            "unread" are all literally in the request,
                            "a reading list of books with a read/unread
                            mark". The app had nothing to guess.

That invites the obvious test: does key-guessability predict passing? Across
all 27 recorded requests, measuring what fraction of each request's expected
keys appear in its own text:

  passing requests: 47% of keys appear in the request (n=8)
  failing requests: 27% (n=19)

Real, and much weaker than my reading implied. The counter-examples matter
more than the averages:

  req 6  PASSED with 0 of 5 keys in the request -- it guessed `die1`,
         `die2` and `total` correctly, from convention alone
  req 10 FAILED with 3 of 3 keys present (the corpus bug)
  req 5  FAILED with 1 of 1 present

So "the corpus names are unguessable" is a genuine factor and **not the
whole story**. Apps sometimes guess conventional names right, and having the
name in front of them does not guarantee they use it. My earlier framing --
half ours, half the measure's -- survives, but the "measure's half" is
softer than I made it sound: some of those apps could have chosen better
words and did not.

## req 28 and 29: one synonym, one genuinely new kind

  req 28 mood tracker    wanted days>=7 recorded>=1
                         printed  days:31 logged:12 average:3.5
                                  selected:12 selectedmood:Good saved:yes
                         -- 31 days, 12 entries, average 3.5. `logged` for
                            `recorded`. Pure synonym; see K-105.

  req 29 dashboard       wanted panels==3 values>=3
                         printed  panels:3 completion:72 events:1483
                                  session_seconds:2 signal:89
                         -- exactly three panels and four real numbers,
                            each under its own meaningful name.

Request 29 does NOT fit the synonym bucket and should not be forced into
it. There is no synonym relationship between `completion` and `values`: the
corpus asked for a key holding a COUNT of values, and the app printed the
values themselves. Arguably the app's output is more useful than what was
asked for, and the assert still cannot read it.

Call this what it is -- a summary-vs-detail mismatch. An assert that wants
`values>=3` can only be satisfied by an app that thinks to publish a tally
of how many numbers it is showing, which is an odd thing for an app to do
and impossible to infer from "a dashboard with three panels showing
different numbers".

It is a fifth distinct class, after: unguessable names, synonyms,
self-exercise, print-the-thing-not-facts-about-it, and one plain product
failure. The alternatives operator in K-105 does not fix this one; only a
corpus edit does.
