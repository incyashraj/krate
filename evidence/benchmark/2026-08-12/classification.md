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
