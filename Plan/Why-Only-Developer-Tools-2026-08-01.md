# Why every ported app was a developer utility

Six ports proven: a hex viewer, a budget splitter, a duplicate finder, an RSS
forwarder, an environment-variable manager, a regex generator.

Every one is a tool for programmers. That was not a coincidence and not about
which repositories were picked. It was what Krate could express.

Two things were missing, and both are fixed now. This is the record of what
they were, because the diagnosis is worth more than the fix.

## Blocker one: an app could not ask a person to choose a file

Every port read from a hardcoded `input/` folder:

```
hexyl      fs.read:input/**
ddh        fs.list:input/**
grex       fs.read:input/**
```

Not because the agent was lazy -- there was no other way to get a file into a
Krate app. A real desktop app opens with "choose a photo". Ours said: *first
make a folder called `input`, put the file in it, then run this.* That is a
shape only a programmer tolerates.

**Fixed.** `ui.dialog.open-file` opens the system's own dialog and returns a
name and a token, never a path. `fs.files.open-chosen` takes the token. The
click is the grant, which fits the capability model rather than fighting it:
the app never learns where the file lives, and the token dies with the run.

## Blocker two: a window could not show a picture

Once apps could open files, the honest candidates were all viewers -- of
photos, of documents, of anything. None could be ported.

`image` was in the contract and passed through the runtime, and `widget-node`
had no field for image data. An app could ask for an image widget and had no
way to say which picture. It drew an empty box on every host.

This was not "unimplemented". It was declared, plumbed, and hollow, which is
worse: an agent reading the contract would use it and get nothing.

**Fixed.** `krate:ui/image` carries decoded RGBA to a widget by id. All three
hosts draw it, and the widget parity table now reports 17 of 17 on macOS,
Windows, and Linux with no platform-only entries -- the first time that has
been true.

Two decisions inside that are worth keeping:

- **Pixels cross the boundary, not PNG bytes.** Decoding untrusted images is
  one of the most attacked pieces of code there is. A decoder in the runtime
  would mean every app's malformed download runs against the host. Each app
  carries its own decoder inside its own sandbox instead. More work for the
  app, and the right side to put the cost on.

- **A separate interface, not a field.** Adding `pixels` to `widget-node`
  changed that record's type, and the Component Model matches structurally --
  so `savings`, already built and shipped, stopped instantiating. Not
  degraded: refusing to open. Every GUI app would have broken on release.
  `scripts/replay-ported-apps.sh` caught it, which is the first time that
  script has earned its keep. Adding to a contract is not automatically
  additive.

## Blocker three, found on the way: the agent was told widgets do not exist

CONTRACT.md says *"this is the whole guest API -- if something you want is not
here, it does not exist, do not invent a call"*, and listed every file and
network call and zero widgets. The reference generates from the Rust guest SDK,
which is phase2 and has no UI.

An agent following that instruction correctly would conclude a windowed app
cannot be ported at all. Seventeen widget kinds are listed now, generated from
the same enum the hosts match on.

## What is still true

The analyzer learned two things from candidates that failed:

- **A disk cleaner with 357 stars was correctly refused** for spawning
  processes to move folders. That is the sandbox working, not a gap.
- **A C library binding is a change, not a blocker** -- but only after getting
  it wrong twice. Matching the `-sys` suffix marked all six proven ports
  unsupported (`js-sys` and `web-sys` are WebAssembly's own bindings). Keeping
  it a blocker marked three unsupported, because they carry real bindings under
  dependencies the port replaces anyway.

  A false blocker is worse than a miss. It tells someone not to try something
  that would have worked, and they never find out it was wrong.

## What this was worth

The difference between "ports developer utilities" and "ports software people
want" was never a capability count. It was two shapes: *open your file*, and
*here is what it looks like*. Both exist now.

The honest test is not that the pieces exist -- it is an app somebody outside
this repository would open on purpose. That is the port in flight.
