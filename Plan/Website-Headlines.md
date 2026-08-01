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
