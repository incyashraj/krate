# Is the teaching reaching the apps? Recorded as run 3 lands.

Written per request, before the final score, so it cannot be assembled to
suit the number.

## req 1 -- tip calculator (pass 4/4)

    bill:84.75 percent:18 tip:15.26 total:100.01
    people:4 each:25.01 per_person:25.01 saved:yes

`each` and `per_person` carry the same value under two names. That is the
"print both when a name is ambiguous" rule, which was written after req 20
of run 2 failed on `search` versus `query`.

## req 2 -- to-do list (pass 2/2)

    items:40 entries:40 done:3 remaining:37
    added:1 checked:2 removed:1 scrolled:240 saved:yes

Four of the five fixes visible in one output:
  - operate your own controls   added / checked / removed / scrolled
  - print both when ambiguous   items and entries
  - seed at the scale asked     40 items, not 3
  - print the count, not `yes`  checked:2

In run 2 this same request held 1 of 2 asserts and never reported anything
checked.

## req 3 -- countdown timer (pass 3/3)

    minutes:25 duration:1500 remaining:1104 remaining_text:18:24
    elapsed:1 counted:1673 ticks:1 started:1 paused:1
    adjusted:30 restored:25 reset:1 sessions:0 running:0

The strongest signal so far, and it is generalisation rather than copying.
The pack's example says print `elapsed:140` with a formatted `elapsed_text`
beside it. This app produced `remaining:1104` and `remaining_text:18:24` --
the same pattern applied to a different quantity, which is what
understanding a rule looks like as opposed to pattern-matching one.

It also drives every verb the request implies -- started, paused, adjusted,
restored, reset, ticked -- and reports each as a count.

Run 2's stopwatch printed `elapsed:2:20.18` and failed `elapsed>=0` for
exactly the lack of this.

## Caveat

Three requests, all easy tier, all passes. This is a signal that the teaching
is being applied, not evidence that the pass rate has moved. The number is
the number, and it is not in yet.

## req 5 -- click counter (pass 1/1), and which fix is doing the work

    count:6 clicks:6 pressed:7 decremented:1 saved:yes

In run 2 this printed `clicks:60 frames:61 counts:yes` and failed `count>=1`.
It now prints **both** `count` and `clicks`, drives the button seven times,
and even decrements once.

Two fixes could explain the pass -- the teaching (prefer the ordinary word,
print both when ambiguous) and the harness (`count|clicks>=1`). It matters
which, because one is the product improving and the other is the measure
being more forgiving.

Tested rather than assumed. `strict-check.sh` re-scores each recorded pass
with the alternatives stripped out, so only the corpus's original key counts:

    req 1  passes on the original key alone
    req 2  passes on the original key alone
    req 3  passes on the original key alone
    req 4  passes on the original key alone
    req 5  passes on the original key alone

**All five passes hold without the alternatives operator.** So far the
teaching is doing the work and the harness change has not been needed once.
That is the more honest reading and the less flattering one for the fix I
spent longest on.

(First attempt at this check ran under zsh, where `read -a` is not supported,
and reported the exact opposite for every request. The uniformity is what
gave it away -- five different apps cannot all need the same thing.)

## req 6 and 7 -- the clearest before-and-after pair so far

### req 6 -- dice roller (pass 5/5)

    die1:6 die2:6 total:12 rolls:40 doubles:9 double:yes

Worth noting because `die1`/`die2` are pure convention -- they appear nowhere
in "a dice roller that rolls two dice", and the corpus gives them no
alternatives. So nothing lenient was available here; the app simply guessed
the conventional names and rolled forty times.

### req 7 -- password generator (pass 2/2)

    run 2:  length:32 chars:32 generated:3 history:2 bits:191
            sets:3 copied:yes distinct:21
            -- eight facts about a password, and no password

    run 3:  password:lwRU+$i}ogA3dyQ$CNF]
            length:20 characters:20 generated:6 bits:128 entropy:128
            pool:86 strength:Excellent unique:yes sets:4 copied:yes

The rule written after run 2's failure says, verbatim: "If the app generates
something -- a password, a colour, an id -- print the thing itself, not only
facts about it." The app now prints the password.

That is a complete causal chain that can be checked by anyone: the failure is
in run 2's archived output, the rule is in the pack, the changed behaviour is
in run 3's archived output.

Strict check still holds at seven of seven -- every pass survives with the
alternatives stripped out, so none of this is the harness being lenient.

## req 8 -- BMI calculator (pass 3/3), the units rule

    run 2:  bmi:22.8 category:Normal height_cm:178 weight_kg:72.5
            units:metric
            -- correct arithmetic, failed height? and weight? because the
               units were baked into the key names

    run 3:  bmi:23.51 category:Healthy status:Healthy
            height:178.0 weight:74.5 healthy_min:58.6 healthy_max:79.2
            units:metric typed:2 stepped:3 dragged:137.5 toggled:1

The rule -- "put units in the value or leave them out, never in the key --
`height:178`, not `height_cm`" -- is applied exactly, with `units:metric`
kept as its own key. The app also drives its own controls now (typed,
stepped, dragged, toggled), which run 2's version did not.

Four flips so far: req 2, 5, 7, 8. No regressions.

## A measurable signal, with its caveat

    run 3, requests 1-8:    9.8 keys per app
    run 2, requests 14-38:  6.5 keys per app

**These are different request sets**, because run 2 preserved no output before
request 14. So this is suggestive, not a controlled comparison -- the honest
version arrives when run 3 reaches request 14 and the same requests can be
compared directly.

## req 9 -- stopwatch (pass 2/2), the most literal rule application yet

    run 2:  elapsed:2:20.18 elapsed_ms:140180 laps:4 fastest_lap:3
            -- failed elapsed>=0, because "2:20.18" is not a number

    run 3:  elapsed:580 elapsed_text:0:00.58 laps:10 lap_count:10
            fastest:33 slowest:94 scrolled:276

The pack says, verbatim: "print `elapsed:140` and add a formatted
`elapsed_text` beside it". The app printed `elapsed:580` and
`elapsed_text:0:00.58` -- the same two key names, including the `_text`
suffix convention.

**A note against my own case.** This is a very literal match, and `elapsed`
is one of the few keys the pack names explicitly. So req 9 is weaker evidence
than req 3, where the same pattern was applied to `remaining` -- a quantity
the pack never mentions. Copying an example is not the same as understanding
a rule, and only the second generalises to requests nobody anticipated.

Both kinds are present in this run, which is the encouraging part. But if
someone wanted to argue the improvement is memorisation of the pack's worked
examples rather than better reporting in general, req 9 is the exhibit they
would use, and req 3 is the answer to it.

Five flips: req 2, 5, 7, 8, 9. Strict check nine of nine.

## Attribution: how much of the easy-tier gain is the app, and how much is me?

Six easy-tier requests flipped from fail to pass. The obvious objection is
that I also edited the corpus and the harness between runs, so some of the
gain could be me moving the target. Checked against the original corpus at
commit 7ef2c85, before any of my edits:

  req 2   corpus UNCHANGED   the flip is the app
  req 5   corpus edited      but `count>=1` -- the ORIGINAL assert -- passes
                             on run 3's output anyway, so the edit was not
                             needed
  req 7   corpus UNCHANGED   the flip is the app
  req 8   corpus UNCHANGED   the flip is the app
  req 9   corpus UNCHANGED   the flip is the app
  req 10  corpus edited      and the original `upper~ABC` still fails, because
                             it was unmeetable by design -- a literal
                             substring match that only passed if the app's
                             sample text happened to contain "ABC"

**So five of six flips are the app improving against an unchanged bar.** The
sixth is a corpus bug that no correct app could ever have satisfied.

Combined with the strict check -- all ten passes hold with the alternatives
operator stripped out -- the easy-tier result is not the measure being
softened. That is the claim I most wanted to be able to falsify, and it
survived the two tests I could think of.

## The easy tier is complete

              run 2              run 3
  pass        6/12               12/12
  asserts     24/32  (75%)       32/32  (100%)

Six flips, no regressions, and every pass survives the strict check with the
alternatives operator stripped out. Five of the six flips ran against a
corpus row I never touched; the sixth was an assert no correct app could
satisfy.

**What this does and does not show.** It shows the five teaching fixes reach
the apps and change what they report. It does not show the apps are better
*apps* -- the same request produced working software in run 2 too, and mostly
failed on how it described itself. The honest sentence is that the reporting
gap is closed on the easy tier, not that the product got twelve times better.

The medium tier is the test that matters. Run 2 scored 2/18 there with 51% of
asserts held, and I claimed nearly all of that was reporting rather than
broken apps. If that reading was right, medium should move a long way. If it
was rationalisation, it will not.

## req 13 (medium) and the strictest test available

    run 2:  habits:4 days:7 done-today:2 best-streak:5 saved:yes
            -- failed marked>=1, because the app said `done-today`

    run 3:  habits:5 days:7 marked:2 done:2 week:8 streak:55
            toggled:2 unmarked:1 added:1 removed:1 saved:yes

The app now prints `marked` -- the ordinary word -- and self-exercises.

This row's corpus assert WAS edited by me (I added `marked|done-today|logged`),
so it is exactly the case where my changes could be flattering the result.
Tested against the unedited original `habits>=3;days>=7;marked>=1`: all three
hold. The alternative was not needed.

Generalised into `score-against-original-corpus.sh`, which re-scores every
recorded result against the corpus as it stood at commit 7ef2c85, before any
edit of mine:

    against the ORIGINAL unedited corpus: 12 of 13 recorded requests pass

The one exception is req 10, whose original assert `upper~ABC` was unmeetable
by design.

**That is the strictest reading available and the one to quote.** Not "12/13
after I fixed the corpus", but "12/13 against the corpus exactly as it was
when it scored 0/5 in August".

## req 14, a preserver race, and a corrected baseline

  req 14 note taking (pass 2/2)

    run 2:  notes:3 characters:753 open:Groceries saved:yes
    run 3:  notes:5 items:5 characters:825 added:1 removed:1
            saved:yes selected:4 title:Grocery run
            note:Grocery run Oats, olive oil, the small tin of tomatoes...
            started:5 restored:5

**Two process failures caught here, both mine.**

First, the archived copy of req 14 was 0 bytes. The preserver used `cp -n`,
which raced the app: it copied the file while the app was still writing, and
`-n` then never overwrote the empty version. The contradiction that caught it
is the same one as before -- 2 of 2 asserts held, which no silent app can do.
The preserver now skips empty files and re-copies whenever the source has
grown. Audited every other archived output: all intact.

Second, `score-against-original-corpus.sh` was baselined at commit 7ef2c85,
the corpus's first version. But req 14's assert changed from `saved==1` to
`saved!=no` in commit ed20f59 -- **before this session, not by me**. So the
script was scoring run 3 against asserts that were not the ones run 2 faced,
and it reported req 14 as a failure that run 2's own corpus would have
passed.

Rebaselined to eea8dd3, the corpus exactly as it stood when run 2 scored
10/38:

    against run 2 corpus (pre-my-edits): 14 of 14 recorded requests pass

That is the honest comparison, and it is stronger than the one I published an
hour ago. The earlier "12 of 13" was correct about my own edits but wrong
about the baseline.

## req 15 -- the first pass that genuinely needed the harness fix

  req 15 expense tracker (pass 2/2)

    run 2:  expenses:9 total:$289.93 month:$289.93 categories:4
            -- failed entries>=3 (said `expenses`) and total>=1 (the $)

    run 3:  expenses:40 items:40 total:1878.40 running_total:1878.40
            average:46.96 largest:240.00 added:1 removed:1
            scrolled:2360 seeded:40 seeded_total:1851.65 saved:yes

Its two run-2 failures were fixed by different things, and the split is worth
being exact about:

    total:$289.93 -> total:1878.40   TEACHING (no currency symbol in a number)
    9 entries     -> 40 entries      TEACHING (seed at the scale asked)
    expenses      -> expenses        UNCHANGED -- needed the alternatives
                                     operator, because the app still does not
                                     say `entries`

So this is the first recorded pass that would NOT hold against run 2's exact
corpus:

    against run 2 corpus (pre-my-edits): 14 of 15 recorded requests pass

**The streak breaks here, and it should.** Fourteen passes stood on the app
improving; this one stands half on the app and half on me making the measure
accept a synonym. Both numbers get reported: the raw pass rate, and the
count that survives against the corpus as it was.

## req 16 -- the request that produced a rule now satisfies it

  req 16 shopping list (pass 3/3)

    run 2:  items:5 got:3 remaining:2 saved:yes
            -- a correct snapshot with no evidence it can add or remove

    run 3:  items:20 entries:20 added:1 removed:1 remaining:17
            got:3 saved:yes

This is a closed loop worth stating exactly, because every step is in the
repository:

  1. run 2's shopping list printed a snapshot and failed `added` and
     `removed`
  2. that failure produced the self-exercise rule, whose worked example
     reads: "A shopping list asked for 'add and remove items' should add
     one and remove one, then report both"
  3. run 3's shopping list adds one, removes one, and reports both

Its corpus row was never edited, so it passes against run 2's exact asserts.

## The medium tier, four requests in

    req 13 habit tracker    run 2 fail  ->  run 3 pass
    req 14 note taking      run 2 fail  ->  run 3 pass   (timed out in run 2)
    req 15 expense tracker  run 2 fail  ->  run 3 pass
    req 16 shopping list    run 2 fail  ->  run 3 pass

Four for four, against four failures at the same point in run 2. Against run
2's own corpus the count is 15 of 16, the exception being req 15's
`entries`/`expenses`.

Too early to call the tier -- run 2's medium tier had 18 requests and this is
four of them. But the claim under test was that those failures were reporting
rather than broken apps, and so far every one that has been retried reports
better and passes.

## req 17 -- kanban, and the medium tier at 5/5

    run 2:  cards:6 todo:2 doing:2 done:2 columns:3 saved:yes
            -- failed moved>=1: a correct board that never moved a card

    run 3:  columns:3 cards:17 items:17 todo:7 doing:6 done:4
            added:1 moved:3 scrolled:72 saved:yes

`moved:3` is the exact assert that failed. Seeding also went from 6 cards to
17.

Standing at 17 recorded:

    run 3        17/42 recorded, 17 passing, 45/45 asserts (100%)
    medium        5/5   (run 2: 2/18)
    vs run 2's corpus  16 of 17

**What is still to come is harder for my reading, not easier.** Twelve of the
thirteen remaining medium requests failed in run 2, and five of those failed
on pure naming -- `query`, `cols`, `bullets`, `max`, `recorded` -- where the
teaching has no reliable purchase, because 75% of corpus keys cannot be
derived from the request. Those five are where the alternatives operator will
carry the pass rather than the app, and the "against run 2's corpus" number
is what will expose it.

If that second number stays close to the raw one through requests 20-28, the
apps really did learn to name things better. If it diverges sharply, the
harness is doing the work and I should say so.

## req 18 -- calculator, and the split gets finer

    run 2:  display:8 evaluated:2 add:14 subtract:4 multiply:45
            divide:2.25 divzero:reported
            -- proved all four operations AND divide-by-zero, and failed
               `result?` and `ops>=4` on the key names

    run 3:  sum:42 difference:25 product:96 quotient:11.25 decimal:10
            result:42 display:42 operations:4 buttons:19
            calculations:6 divide_by_zero:refused

Against run 2's exact asserts:

    display?   holds
    result?    holds     <- TEACHING: the app learned the ordinary word
    ops>=4     FAILS     <- it says `operations`; needed the alternative

Second divergence, and finer than req 15's. There the app fixed two problems
and left one; here it fixed one of the two failing asserts on its own.

    against run 2 corpus: 16 of 18

`ops` is an abbreviation of a word the app did use. That is exactly the class
I said teaching could not reliably reach -- an app has no way to know the
reader wants the short form. It is also the class the alternatives operator
exists for, and it is doing its job.

## req 20 -- the first failure of run 3, and it exposes my own corpus work

  req 20 contact book (FAIL 2/3)

    run 2:  contacts:9 seeded:8 search:gra matches:1 added:1 selected:1
            -- failed `query?`, because the app said `search`

    run 3:  contacts:24 entries:24 query:ma search:ma matched:3
            results:3 selected:Hedy Lamarr cleared:24 saved:yes
            -- `query?` now HOLDS. It fails `matches>=1` instead.

Against run 2's exact asserts:

    contacts>=3  holds
    query?       holds   <- the run-2 failure, FIXED by teaching
    matches>=1   FAILS   <- a NEW failure, on a different key

The app applied the print-both rule to `query`/`search` and not to
`matches`, where it chose `matched` and `results`.

**This is evidence against my corpus work, not for it.** I added alternatives
only where run 2 failed. `matches` passed in run 2, so I never touched it --
and a differently-phrased app broke it. My edits were fitted to one run's
failures rather than to the space of reasonable phrasings.

Twenty single-name asserts remain in the requests still to run -- `rows`,
`input`, `output`, `headings`, `bars`, `board`, `turn`, `cards`, `books`,
`panels`, `encoded`, `decoded`, `scrolled` and more. Any of them can break
the same way.

The honest reading: the alternatives operator is the right mechanism and my
application of it was incomplete, because I derived the list from failures I
had seen instead of from what an app might reasonably say. That is a
fit-to-the-data mistake and it will keep costing passes until the corpus
lists alternatives systematically.

## req 21, and what the three divergences have in common

  req 21 CSV viewer (pass 2/2)

    run 3:  rows:240 columns:8 header:8 scrolled:900 selected:32 parsed:2
            -- 240 rows parsed and 900 pixels scrolled, but still `columns`

Three passes so far have needed the alternatives operator:

    req 15  expenses  for  entries    the app used the DOMAIN word
    req 18  operations for ops        the app wrote the word out in FULL
    req 21  columns   for  cols       the app wrote the word out in FULL

**Two of the three are the same shape, and the app is arguably right.**
`columns` and `operations` are more readable than `cols` and `ops`. An app
has no way to guess that a reader prefers the abbreviation, and writing the
word out is the better default for a human reading the output.

The pack says nothing about abbreviations, and **it should not start**. The
right conclusion is that the corpus is at fault: `cols>=2` and `ops>=4` are
asking for a shorthand no request implies. Teaching apps to guess a reader's
preferred abbreviation would be fitting the product to the measure, which is
the exact failure this whole exercise has been trying to avoid.

So of the three, one (`expenses`) is a genuine synonym the operator exists
for, and two are corpus rows that should simply be spelled out.

Standing: raw 20/21, against run 2's corpus 17/21.

## The three interruptions, audited

Run 3 stopped three times. Because a skip halts the runner rather than
recording a failure and moving on, every one is visible and none could have
been silently absorbed into the results.

    req 5   FALSE POSITIVE  transcript env dump contained
                            KRATE_AUTHOR_TIMEOUT_SECS
                            -> row deleted, re-run, PASSED

    req 11  GENUINE         "You've hit your session limit"
                            -> row deleted, re-run, PASSED

    req 22  FALSE POSITIVE  the JSON printer's own sample data contained
                            "last_error":"connection reset by peer"
                            -> row deleted, re-running

The results file now contains no skipped rows.

**Both false positives were mine, and both cost time rather than data.** The
common cause is that the skip patterns scan the agent transcript, which
records everything the AI read and wrote -- including the app's source and
the process environment. Matching product-controlled text against
infrastructure-failure patterns was the mistake, made twice.

The guard added after req 22 addresses the class rather than the instance: a
produced `.krate` proves authoring succeeded, so it overrides every skip
reason, all of which mean "no finished app to judge". A future false positive
is now harmless rather than run-stopping.

## req 22 -- the guard works, and the failure has two causes

  req 22 JSON pretty printer (FAIL 2/3)

    run 2:  valid:yes lines:18 objects:2 arrays:2 values:10 depth:2
            bad_json_rejected:yes output_bytes:191
            -- eleven facts about the output, and never the output

    run 3:  input:1 bytes:492 lines:40 formatted:701 valid:yes indent:4
            errors:1 error_line:1 error_column:21
            out:{
            out:  "name": "Krate",
            out:  "version": "0.1.3",
            ...

First: **the guard did its job.** This request was skipped last attempt
because its sample data contained "connection reset by peer". It now scores
normally, and the score is a fail -- which is the correct outcome, not a
convenient one.

Second: the teaching landed. Run 2 printed facts about the output; run 3
prints the formatted JSON itself, which is what the "print the thing, not
facts about it" rule asks for.

It still fails `output~{`, for two separate reasons:

  1. **naming** -- the app called the key `out`, the corpus wants `output`
  2. **a real harness interaction** -- the app emitted its multi-line output
     by repeating the key on every line, and the harness takes the LAST
     matching line. That last line is `out:    "desktop",` which contains no
     brace. Even `out~{` fails.

The second is not the app's fault. The pack says "one pair per line" and says
nothing about what to do when the value is ITSELF multi-line, which is
precisely a pretty-printer's situation. Repeating the key is a reasonable
reading of the rule it was given.

**Not fixing this mid-run.** It is a teaching gap and a harness limitation
worth a considered answer -- probably "emit a multi-line value once, joined,
or as a single key with escaped newlines" -- and changing the pack now would
invalidate the comparison. Recorded for the next run.
