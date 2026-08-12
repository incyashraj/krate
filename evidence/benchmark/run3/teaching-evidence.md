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
