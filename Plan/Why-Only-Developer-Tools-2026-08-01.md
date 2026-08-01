# Why every ported app is a developer utility

Six ports proven: a hex viewer, a budget splitter, a duplicate finder, an RSS
forwarder, an environment-variable manager, a regex generator.

Every one is a tool for programmers. That is not a coincidence and it is not
about which repositories were picked. It is what Krate can currently express.

## The actual blocker

**A Krate app cannot ask a person to choose a file.**

Look at how the ports get their input:

```
hexyl      fs.read:input/**
ddh        fs.list:input/**
grex       fs.read:input/**
```

Every one reads from a hardcoded `input/` folder. Not because the agent was
lazy -- because there is no other way to get a file into a Krate app.

A real desktop app opens with "choose a photo", "open your spreadsheet",
"select the folder to organise". Ours says: *first make a folder called
`input`, put the file in it, then run this.* That is a shape only a programmer
tolerates, and it is why every port that succeeded was a programmer's tool.

## Three things are wrong at once

1. **The WIT has no file picker.** `interface dialog` declares `message` and
   `confirm` and nothing else. There is no `open-file`, no `choose-folder`. An
   app cannot ask, because the question does not exist in the contract.

2. **The capability exists anyway.** `ui.dialog:file-open` is in the capability
   registry, default-granted, gating an operation that was never declared. Every
   app's permission list says it can open a file picker. None can.

3. **macOS has a working picker, unused.** `crates/adapter-macos/src/open_document.rs`
   is 197 lines of real `NSOpenPanel`. Windows and Linux have nothing.

## Why this has not been fixed

Wiring up the macOS one alone would rebuild the exact trap the `canvas` widget
was in this morning: works on the machine it was built on, fails when the app is
shared. That is the failure Krate exists to remove, so a one-host picker is
worse than none.

`rfd` (0.17.2) does native file dialogs on all three systems and would remove
that objection. It is one dependency.

## The part that needs deciding, not coding

A file picker is not a normal capability. Every other grant is decided **before**
the app runs, from a list a person reads. A picker decides **during** the run,
from a click.

That inverts the model, and the inversion is the whole point: choosing a file in
a native dialog *is* the person granting access to that file, more directly and
more comprehensibly than any manifest line. The design question is how the
runtime represents that:

- **The dialog returns a path the app can then read.** Simple, and wrong: the
  app now holds a path outside every granted glob, so either the read is
  refused (the picker is useless) or the glob is widened (the wall has a hole).
- **The dialog returns a handle, not a path.** The app can read and write that
  one file and never learns where it lives. The grant is the click. This fits
  the capability model rather than fighting it, and it is what `store.kv`
  already does for storage -- the app addresses a thing it cannot name.
- **The dialog widens the session policy for exactly the chosen path.** Honest
  and inspectable: `--log-grants` would show the file the person picked. More
  machinery, but it keeps one representation of authority.

The second is the one that matches everything else in the system. It also means
`fs.read` and the picker are genuinely different capabilities rather than one
being a way around the other.

## What this is worth

This is the difference between "ports developer utilities" and "ports software
people want". Not a capability count -- a shape. Until an app can say *open
your file*, the honest description of Krate is a runtime for command-line tools
that happen to run everywhere.

Recommended as the next real piece of work, ahead of any remaining widget or
interface, because it is the one that changes which apps are possible rather
than which ones are prettier.
