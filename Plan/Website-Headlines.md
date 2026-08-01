# Website headlines

Ready-to-paste copy. Every number here is measured — `sh scripts/check-claims.sh`
reprints them all, and CI prints them on every push. Sources are in
`Plan/Claims-We-Can-Make.md`.

Written for two readers at once: a founder deciding whether to build on Krate,
and an investor deciding whether this is a company. They want the same thing —
proof that the hard part is done.

---

## The hero. Pick one.

**A. The one I would ship**

# A 2D game. 11 kilobytes. Runs on Mac, Windows, and Linux.
### No installer. No runtime to download. No permission it didn't ask for.

Why it works: it is a fact, not an adjective. "2D game" tells a developer the
ceiling is high; "11 kilobytes" tells them how. Nobody expects those two things
in one sentence.

**B. For the security-minded room**

# The app can't read your files.
### Not *shouldn't* — can't. We attack our own sandbox to prove it.

**C. For the Electron-weary**

# Electron ships a browser with every app.
### We ship eleven kilobytes.

---

## The number section

### 318 apps fit inside one photo.

A complete 2D game — gravity, collision, sprites — is 11 KB. Not 11 megabytes.
The same file opens on Mac, Windows, and Linux, because nothing is bundled
inside it that your computer already has.

| | Size |
|---|---|
| Apple's Reminders app | 11.2 MB |
| Everything Krate has ever shipped — ten complete apps | **283 KB** |

*One app that makes lists is 41× larger than every app we have ever built,
combined.*

---

## The speed section

### Apps open in 16 milliseconds.

Median cold start, measured five times each on an Apple Silicon Mac:

| App | Opens in |
|---|---|
| A budget splitter | 16 ms |
| A 2D game | 18 ms |
| A markdown viewer (4,863 lines ported) | 28 ms |
| An image viewer (2,677 lines ported) | 30 ms |

*A single frame of 60 fps video is 16.7 milliseconds. Most Krate apps are fully
open before your screen has finished drawing one frame.*

### And the sandbox is free.

The question every engineer asks: what does the safety cost?

| Work | Native | Krate | Difference |
|---|---|---|---|
| 300 million integer operations | 703.9 ms | 706.3 ms | **1.00×** |
| 100 million | 220.0 ms | 236.6 ms | 1.08× |

**Sandboxed code running at native speed.** Not "close to" — at 300 million
operations the difference is inside measurement noise.

The honest caveat, which we publish: an app that crosses the sandbox boundary
constantly and computes almost nothing in between pays up to 5.14×. That is
the cost of checking permissions, and it is the trade we chose.

*Verified:* output was checked identical before every timing. The first version
of this benchmark reported Krate as **faster than native**, which was a bug in
the harness — the output check is what caught it. Full method:
`Plan/Native-Comparison-2026-07-31.md`.

---

## The proof section

### We publish the receipts.

**Nine complete apps re-run every night on Mac, Windows, and Linux.** Not "it
starts" — we check the numbers each one prints, and a wrong answer fails the
build. The results are public, in the open repository, every night.

### We attack our own sandbox.

We give an app permission to read `/etc/` — where a Unix system keeps its
account list — and point it at the password file.

```
00000000  73 61 6e 64 62 6f 78 20  63 6f 70 79
```

It reads a copy inside its own cage. Those bytes spell **"sandbox copy"**. The
app believes it succeeded. The real file was never reachable.

### When you pick a file, the app gets the file — not the folder.

A Krate app never learns where your file lives. It gets a token for that one
file, and the token stops working when the app closes. **Your click is the
permission.**

---

## The two ways in

### Bring software that already exists.

Eight real programs from GitHub now run on Krate — an image viewer, a markdown
reader, a 5,396-line regex tool — turned into single files that run everywhere.

### Or describe the app you want.

Eight plain-English requests, six working apps. A tip calculator that gets the
maths right. A metronome that clicks through your speakers. An expense tracker
that remembers between runs.

**Describe an app in a sentence. Get a file that runs everywhere, inside a
sandbox it cannot escape.**

---

## For early adopters: the "will this still exist next year" section

### The whole thing is open, and the tests are the proof.

- **795 tests** run on every change
- **23 separate checks** on every push — build, lint, security audit, and the
  nine apps re-run on all three operating systems
- **64,837 lines of Rust**, in the open, readable today

### Nothing here is a demo that only works on the demo machine.

The tables on our documentation site are **generated from the runtime itself**,
not written by hand, and CI fails if they drift. When we say 17 of 17 interface
elements work everywhere, that sentence is produced by the code it describes.

### We publish what does not work.

Our own repository contains the list of things Krate cannot do, the bugs we
shipped and caught, and the one measurement that embarrassed us. Early adopters
can read it before committing a line of code.

### One install, shared by every app.

Krate is a 17.5 MB install, once. Every app after that is kilobytes. Compare
with the current norm, where each app brings its own copy of a browser.

---

## For the investor deck

Three slides, in this order:

**1. The problem, in one line.**
> Every desktop app today ships a copy of a web browser to draw a button.

**2. The proof, in one number.**
> A 2D game with physics and sprites: 11 KB, three operating systems, one file.
> Apple's Reminders app is 1,074× larger and does one thing.

**3. The moat, in one demonstration.**
> Run the escape test live. It takes eight seconds and it ends the "but is it
> really secure" conversation, because the audience watches the app fail.

### If they ask what stops someone copying this

The answer is not the idea — it is the surface. Krate is a contract between an
app and three operating systems, and every one of the following had to work
identically on all three before any of it counted:

> windows · 17 widget kinds · layout · file dialogs that hand over tokens
> instead of paths · images · a drawing surface · sound in and out · HTTPS ·
> SQL · secure storage · speech-to-text · a permission wall that is tested by
> attacking it

Each one is easy alone. The value is that no app has to care which computer it
is running on — and that is only true once *all* of them are done, on *all*
three systems, and stay done. Nine apps re-run nightly to prove they stay done.

The second moat is subtler and shows up in our own history: **we keep catching
ourselves.** A change that would have broken every existing app was caught the
day it was made, by a test that runs real apps rather than checking a
signature. That discipline is what a buyer is actually purchasing.

### The slide that earns trust

Put the honest limits on a slide of their own. It converts the room:

> **What Krate cannot do yet:** 3D graphics. Video. Live connections
> (multiplayer, streaming feeds).
>
> **And the one we tell people before they find it:** when an AI builds an app
> for you, Krate guarantees it cannot exceed the permissions you granted. It
> does not guarantee the AI made good choices inside them. We caught our own
> generated password manager storing passwords in ordinary storage, and fixed
> the instructions. We measure this. We publish it.

A founder who volunteers their weakest result is telling you the other numbers
are real.

---

## Words to avoid

- **"Blazing fast", "revolutionary", "next-generation".** We have real numbers;
  adjectives make them look like decoration.
- **"Secure by design".** Everyone says it. Show the escape test instead.
- **"Any app".** Not yet true — 3D, video, and live connections are missing.
  "Any small or medium app" is defensible and still remarkable.
- **"Blazing 12,000 FPS".** True in the draw path, and it invites a benchmark
  argument we do not need. "Fast enough that drawing is not the limit" is the
  honest version.

---

## Maintenance

These numbers change as the code improves — usually downward, which is good
news that makes the website wrong.

```bash
sh scripts/check-claims.sh
```

Run it before any pitch. The live site currently says "246 apps fit inside one
photo"; it is 253 today for that app, and 318 for the game. Small drift, and
exactly the kind that gets noticed by the one person in the room checking.
