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
