# What happens when you let an AI write eight desktop apps

**Published:** 2026-08-05

We gave an AI the public Krate documentation and asked for eight apps nobody
had built before: a tip calculator, a colour picker, an expense tracker,
flashcards, a countdown timer, a markdown scratchpad, a chess board, and a
maze.

No templates. No hints beyond what anyone can read on the website. Then we
tried to actually use each one.

**Eight of eight built. Zero of eight were usable.**

This is what that gap was made of, because the failures were more interesting
than the successes.

## Every app quit itself while you were using it

All eight bounded their event loop. Four used the same constant:

```rust
const MAX_ROUNDS: u32 = 800;
const ROUND_MILLIS: u32 = 50;
```

Eight hundred rounds at fifty milliseconds is forty seconds. Timed three times
on the same app: 45s, 46s, 43s. A flashcard app that closes itself during
revision is not a working app, however well the rest is written.

Eight out of eight making the identical mistake is not eight coincidences. It
is one missing sentence. Our examples all carried a loop bound so automated
tests could not hang, and nothing anywhere said that bound belongs only to the
test path. The AI copied what it saw.

## Nothing could be clicked

Buttons did nothing. We clicked a colour picker's "+" seven times across two
runs and the app's own state stayed at its untouched default.

That one was entirely ours. On macOS, AppKit delivers mouse events down a
responder chain, and an app that draws its own interface is an image view that
answers nothing. Every press on every drawn button was silently dropped. The
apps were hit-testing correctly and never receiving anything to test.

Scroll had the same hole, found the same way a week earlier.

## Layout collapsed past four buttons

The fifth button in a row wrapped and drew on top of the next row's text. In
the expense tracker, "Add expense" -- the app's most important control -- ended
up underneath the "Other" category button.

Underneath that was a smaller and more embarrassing bug: there was no way to
measure text. Apps were multiplying character counts by a made-up constant.
Eleven shipped apps did this, several with comments confidently stating the
host font was monospace. It is not. At 32 points, `iiii` is 28 pixels wide and
`WWWW` is 121 -- four times the difference, from an API that returned the same
number for both.

## What this cost, and what it bought

Every one of those defects is now fixed. But the useful part is not the fixes,
it is the shape of the failures:

- **Two were missing sentences in documentation**, not missing features. The
  runtime could already do the right thing.
- **Two were holes in our own code** that no amount of prompting could route
  around.
- **None were the AI writing bad code.** The Rust was correct: integer-cents
  arithmetic in the expense tracker, correct calendar maths in the countdown,
  guarded division everywhere. It compiled, it passed every check we had, and
  it did what it was asked.

That is the lesson worth taking. When an AI produces something that does not
work, the interesting question is usually not "why is the model bad at this".
It is "what did we fail to tell it, and what did we fail to build".

## Checking is different from building

Our checks passed all eight apps. They confirmed each one compiled, imported
only permitted interfaces, ran, and painted a frame. All true. All useless for
the question that mattered.

So we added a stage that drives the app and asks three things: does the window
stay open, does resizing change the app's own canvas, does pressing a control
do anything. On its first sweep it caught a shipped app closing itself after
1.5 seconds -- one we had been running for weeks.

"It builds" and "it works" are different claims, and only one of them is worth
making.

You can [read what runs today](https://krate.tech/progress), including what
still does not, or [try an app](https://krate.tech/cloud) and judge for
yourself.
