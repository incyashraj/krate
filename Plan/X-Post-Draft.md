# X post drafts

**Status, 2026-08-02.** `v0.1.0-rc5` is tagged and building. It is the first
release that can run the apps below — rc4 predated drawing, animation and
sound, so it refused the 2D game outright.

`krate.tech/reports` is live and verified: styled, three charts, and the limits
section. Post the build-in-public version now. Before posting the "install it"
version, download rc5 on a machine you have not built on and open the game
yourself. The value of the first post is that the second one can be trusted.

---

## Post now — the build-in-public version

**Option A. The one I would send.**

> A 2D game with gravity, collision and sprites.
>
> 11 kilobytes.
>
> One file, runs on Mac, Windows and Linux. No installer, no runtime download,
> no permission it didn't ask for.
>
> Every number, and everything that doesn't work yet:
> krate.tech/reports

*Why it works: two facts nobody expects in one sentence, then a link that
proves it rather than a link that sells.*

**Option B. The security angle.**

> We give one of our apps permission to read /etc, then ask it for the system
> password file.
>
> It reads this:
>
> 73 61 6e 64 62 6f 78 20 63 6f 70 79
>
> That spells "sandbox copy". The app thinks it succeeded. The real file was
> never reachable.
>
> krate.tech/reports

**Option C. The comparison.**

> Apple's Reminders app is 11.2 MB and keeps lists.
>
> Every app we have ever built — an image viewer, a markdown reader, a regex
> tool, a 2D game, six more — is 283 KB together.
>
> One file each. Mac, Windows, Linux.
>
> krate.tech/reports

---

## Post after tagging a release

Same openers, with the last line changed to:

> Install it, open the game, and try to make it read a file it wasn't given:
> krate.tech

Only post this once a release newer than rc4 exists and you have installed it
yourself on a clean machine. The whole value of the first post is that the
second one can be trusted.

---

## The thread, if you want more than one post

**1/**
> A 2D game with gravity, collision and sprites. 11 kilobytes. One file that
> runs on Mac, Windows and Linux.

**2/**
> It is that small because nothing is bundled inside it that your computer
> already has. No browser, no runtime, no framework. Krate installs once —
> 17.5 MB — and every app after that is kilobytes.

**3/**
> Speed was the thing we expected to lose. 300 million integer operations:
> 703.9 ms native, 706.3 ms inside the sandbox. Inside measurement noise.
>
> Apps open in about 16 ms — one frame of 60 fps video is 16.7.

**4/**
> The part we care most about: an app can only touch what you allowed.
>
> We grant one permission to read /etc, ask for the password file, and it gets
> a copy inside its own cage. Those bytes spell "sandbox copy".

**5/**
> What it cannot do yet: 3D, video, live connections. And when an AI writes the
> app, we guarantee it cannot exceed the permissions you granted — not that it
> chose well inside them. We caught our own generated password manager storing
> passwords in the wrong place.
>
> All of it: krate.tech/reports

*Post 5 is the one that earns the follow. Everyone claims the first four.*

---

## Rules for replies

- **"Is this just Electron/Tauri/WASM?"** — Tauri still uses the system
  webview and ships per-app. The nearest honest answer: Krate is a capability
  contract over WebAssembly, and the work is that every capability behaves the
  same on three operating systems. Link `/reports`.
- **"11 KB is cheating, the runtime is huge."** — Agree immediately and give
  the number: 17.5 MB once, shared by every app. It is on the site.
- **"Benchmarks are always fake."** — The best answer we have: our first
  benchmark said Krate was *faster than native*, we found the harness bug, and
  we published both the fix and the 5.14× worst case. Point at the methodology
  paragraph.
- **Someone says it does not work.** Ask which release and which app. Anyone
  on rc4 or older cannot run the game, the chart, or anything with sound —
  those interfaces landed in rc5. Thank them, name the version, do not argue.
  A person who tried it and reported back is worth more than one who scrolled.

---

## What not to say

- Anything with "revolutionary", "blazing", or "game-changing". The numbers do
  that work, and adjectives make readers assume the numbers are decoration.
- **"Any app."** Not true yet. "Any small or medium app" is defensible.
- **"12,000 FPS."** True in the draw path, and it starts a benchmark argument
  we gain nothing from winning.
- A screenshot of a terminal with no context. If the post shows output, say
  what it is in the same image or the first reply.
