# Six automated checks passed an app that could not be clicked

**Published:** 2026-08-05

We had six stages of automated checking. An app had to compile, import only
permitted interfaces, run headless, paint a frame, declare its permissions
honestly, and pass a capability audit.

An app passed all six and could not be used at all. Not "was awkward" -- every
button in it was dead.

This is a note about why that happened and what we did instead, because the
failure mode is not specific to us.

## Every check was true and none of them mattered

Each stage measured something real. Compilation is real. Import checking is
real and catches a whole class of sandbox escapes. Painting a frame proves the
graphics path works end to end.

But look at what the set has in common: every check asks whether the software
*ran*. Not one asks whether a person could *use* it. Those are different
questions, and passing the first tells you almost nothing about the second.

The dead buttons were a missing mouse-event path on macOS. The app hit-tested
correctly and never received an event to test. Every automated check
passed — because none of them clicked anything.

## The problem with testing your own product

We knew the app worked because we knew how to use it. We ran it with the flags
we always use, in the directory we always use, after a build we had just done.
Every one of those is a fact about us, not about the product.

Three things that only show up when you stop being yourself:

**The binary on PATH is not the one you built.** We spent hours on "a bug that
came back" that was a stale installed release. Twice.

**Your machine is warm.** Your toolchain is installed, your AI CLI is signed
in, your caches are full. A first-time user has none of that, and the first
thing they see is whatever breaks when it is absent.

**You never do the awkward thing.** You do not resize the window mid-task, or
leave the app alone for a minute, or click the fifth button in a row, because
you know what it does.

## What we changed

**A stranger runs the tests.** We keep an isolated directory with only the
publicly installed release and the public website — no repository, no source,
no internal knowledge. Apps get built there by an AI that has never seen our
code, and then actually used: every control clicked, the window resized, items
added past the bottom of the list, left idle for thirty seconds.

The first run of that produced the sentence this post exists for: **eight of
eight apps built, zero of eight were usable.**

**Checks that use the app, not just run it.** We added a stage that drives a
real app and asks three things: does the window stay open with nobody touching
it, does resizing change the app's own canvas, does pressing a control change
anything. It caught a shipped app closing itself after 1.5 seconds on its first
sweep.

**Screenshots as evidence.** Every app renders its window to a PNG headlessly,
and somebody looks at the picture. That is how we found buttons overlapping
text, a chess board with seven columns, and a control drawn underneath another
control. No automated check was going to report those, and every one is obvious
in an image.

## The rule we ended up with

Do not ask whether it ran. Ask whether someone could use it, and then have
someone who is not you try.

"Building is not passing" is now written at the top of our contributor
documentation, because six stages of green checks taught us that the hard way.

If you want to see how this turned out, [what runs
today](https://krate.tech/progress) lists every app along with what still does
not work. The failures are on the same page as the successes, deliberately.
