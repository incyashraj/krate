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
