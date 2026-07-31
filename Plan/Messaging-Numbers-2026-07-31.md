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

Measured on this machine, both real files:

> **Obsidian, a note-taking app, is 482 MB. A Krate checklist app is 12 KB.
> You could fit 39,000 of ours inside one of theirs.**

This works because it is checkable in ten seconds and the reader already
resents how much space their apps take.

One warning that matters: the first version of this comparison was going to be
"reminder apps are over 100 MB". Apple's Reminders is 12 MB. The claim would
have been wrong and someone would have checked. **Electron apps are the honest
target** -- Obsidian 482 MB, Discord 431 MB, Signal 394 MB, VS Code 1.4 GB --
because most of that size is a copy of a web browser shipped inside the app,
which is exactly the problem Krate does not have.

Say the caveat before someone else does: Obsidian does far more than keep a
checklist. The comparison is still fair, because the 482 MB is not what makes
it a better notes app.

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
