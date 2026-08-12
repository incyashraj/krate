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
