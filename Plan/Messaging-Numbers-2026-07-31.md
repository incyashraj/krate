# The numbers, in words people already understand

Every line here is backed by `Plan/Native-Comparison-2026-07-31.md`. Nothing is
rounded in our favour. If a number changes, change it here too rather than
letting the good version outlive the measurement.

The rule this follows: say the fact in a unit the reader already cares about.
"1000 songs in your pocket" is not a smaller number than "5 GB" -- it is the
same number, translated.

## The four facts, translated

| Measured | Say this instead |
|---|---|
| 6.5 KB bundle vs 320 KB native | **An app that fits in a text message** |
| 6 ms startup | **Opens in a third of one video frame** |
| 1.00x on sustained compute | **Full speed once it is running** |
| 0.6 µs per capability check | **Safety that costs six ten-thousandths of a millisecond** |

## The line that lands

> **246 Krate apps fit inside one photo on your phone.**

This is the one to lead with. A photo cannot argue back. Compare against an app
and a fair reader says "but that app does more"; compare against a photo and
there is nothing to dispute -- and everyone already resents how much space their
photos take. Same move as "1000 songs in your pocket": the fact is unchanged,
the unit is one they already feel.

### The app comparison, and why Reminders beats Obsidian

> **A Krate checklist app takes 925 times less space than the Reminders app on
> your Mac.** 12,759 bytes against 11.2 MB, both measured.

The instinct was to say "reminder apps are over 100 MB". They are not -- Apple's
Reminders is 11.2 MB, checkable in ten seconds by anyone with a Mac, and one
wrong number makes every right number suspect.

But the honest version is *better*, not worse. Reminders is one of the leanest
apps on a Mac: Apple ships it against system frameworks rather than bundling a
browser inside it. Beating the strong case by 925x is a stronger claim than
beating a weak one by 39,000x, and it cannot be dismissed as cherry-picking.

Keep Obsidian (482 MB) as the follow-up, not the headline: *that* is what the
common way of building costs, and it is 39,000x ours. Leading with it invites
"you picked the fattest app you could find", which is exactly what we did.

## What to lead with

**Not speed.** Speed is a trap as a headline: 1.00x is real and verified, but
the first technical question back is "what is the worst case", and that is 5x.
Lead with speed and the meeting ends on the worst case. Lead with something
else and the honest 5x answer makes you look rigorous.

**Lead with the thing nobody else has:**

> Every app tells you what it can do before it runs, and cannot do anything else.

Nobody is excited about 6 KB. People are tired of not knowing what software
does on their computer.

## Email lines that are true

Short, for a cold email where you get three sentences:

> Krate makes an app one file that runs on Mac, Windows, and Linux, and carries
> its own permissions on the outside. The file is about 6 KB -- small enough to
> send in a message -- and it runs at native speed once open. We ported hexyl,
> a 2,400-line hex viewer, end to end last week.

If they asked a technical question and want detail:

> The overhead is a fixed startup cost, not a per-operation tax. 6 ms to open,
> then native speed: over 300 million calculations the Krate build finished in
> the same time as the native one. The permission check costs 0.6 microseconds
> per call. Worst case is a program that does nothing but ask the system for
> things, where we are about 5x slower end to end -- full numbers and what we
> have not tested are in the repo.

That last sentence is doing real work. Volunteering the worst case is what
makes the rest believable.

## Deck slides

**Slide: the problem.** Software does not move between computers, and you
cannot tell what it does before you run it. Two problems, one file solves both.

**Slide: the numbers.** The four figures above, nothing else on the slide.

**Slide: the proof.** hexyl, ported: 2,400 lines of someone else's Rust, now a
14.8 KB `.krate` producing byte-identical hex output, with zero non-Krate
imports. Not a demo we wrote -- a real program off GitHub.

**Slide: what we have not done.** Windows and Linux are unmeasured. One real
port, not a hundred. This slide wins more meetings than it loses, because every
investor has sat through the version without it.

## Claims not to make

- **"Any app."** One real port is proven. The third person to try "any" will
  disprove it in public and never come back. Say "a 2,400-line hex viewer" --
  it is more specific, more credible, and more impressive.
- **"Faster than native."** An early version of the benchmark said this. It was
  a harness bug. It is not true and it is the kind of claim that ends
  credibility permanently.
- **"Runs everywhere"** without saying the runtime is 17.5 MB and installed
  once. For a single app, a native binary is a smaller download. Say the
  crossover -- about 59 apps -- before someone else does the arithmetic.
- **Anything about TAM.** "$600B software market" is slide 3 of every AI
  coding deck. It signals nothing. The narrow claim is the strong one.
