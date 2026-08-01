# Claims we can make

Website, deck, and investor copy draw from here. **Nothing goes in this file
that has not been run.** Each claim carries how it was checked, so anyone can
reproduce it and nobody has to trust a memory of a demo.

Last verified: 2026-08-02.

> Rule for adding a claim: run the thing, paste the command, paste the output.
> If it cannot be re-run on a fresh machine, it is a story, not a claim — put
> it under "Not yet claimable" instead.

---

## The headline

**One file. Three operating systems. No installer, no runtime to download, no
permission it did not ask for.**

Everything below is a way of making that concrete.

---

## Size: the number people feel

| What it is | Size | Compared to Apple's Reminders (11.2 MB) |
|---|---|---|
| A 2D game with physics and sprites | **11 KB** | **1,074× smaller** |
| A markdown viewer (ported from 4,863 lines) | **51 KB** | 227× smaller |
| An image viewer (ported from 2,677 lines) | **77 KB** | 152× smaller |

Ways to say it:

- **"A complete 2D game, smaller than the icon of most apps."**
- **"One photo from your phone holds 318 copies of our game."**
- **"Everything Krate has ever shipped — ten complete apps — is 283 KB
  together. Apple's Reminders app, which does one thing, is 41× that."**
- **"Nine of those ten are re-run every night on all three operating systems,
  and we check the answers they print, not just that they started."**

*Verified:* `ls -la evidence/ported/*.krate`, and `du -sk` on
`/System/Applications/Reminders.app` on macOS 26 (11,520 KB = 11.2 MB). The
site's existing comparison uses the same figure, deliberately: two pages of
ours disagreeing about Apple's app size is the detail a careful reader
catches.

---

## What Krate can build (each proven by a running app)

- **2D games.** Real game loop, gravity, collision, sprites with transparency.
  Measured at **over 12,000 frames per second** in the draw path.
- **Animation.** Time-based, so the same file runs at the same speed on a
  gaming desktop and a tired laptop.
- **Drawing and charts.** Apps draw their own graphics — bars, strokes, text,
  images — with one rasterizer shared by all three systems.
- **Sound.** Real audio through the speakers, verified by a test that plays a
  440 Hz tone on actual hardware.
- **Photo and document viewers.** File dialogs, image decoding, scrolling
  text, search.
- **Internet apps.** HTTPS scoped to a single host and port, not "the
  internet".
- **Databases, secure storage, speech-to-text.** All real, all sandboxed.

*Verified:* nine of the ten bundles replay every night on macOS, Windows, and
Linux (`scripts/replay-ported-apps.sh`), each checked against its real output,
not just an exit code. The tenth (`rssfwd`) is excluded on purpose: it reaches
the internet, and a nightly test that depends on someone else's server tells
you about their uptime, not our runtime.

---

## Security: the claim we can defend under attack

**"An app can only touch what you allowed. We test this by trying to escape."**

The demonstration: a hex viewer is granted permission to read `/etc/**` and
asked to open `/etc/passwd` — the file holding a Unix system's account list.
It reads a copy inside its own sandbox. The real file is unreachable.

```
$ krate run --grant "fs.read:/etc/**" hexyl.krate -- /etc/passwd
00000000  73 61 6e 64 62 6f 78 20  63 6f 70 79     sandbox copy
```

Those bytes spell "sandbox copy". The app believes it read `/etc/passwd`.

Supporting claims, all enforced by code:

- **A file picker hands over a token, never a path.** The app learns the file's
  name and can read that one file — it never learns where the file lives, and
  cannot walk to its neighbours. The click *is* the permission.
- **A token dies with the run.** An app cannot save one and come back later.
- **Image decoding happens inside the sandbox.** A malformed photo attacks the
  app, never the operating system. Pixels cross the boundary; parsers do not.
- **Home directories are unreachable by design.** An app that asks for
  `~/Pictures` is refused at packaging time, with a message explaining that the
  person can grant a folder by choosing it.

---

## Portability: the hard part nobody sees

**"17 of 17 interface elements work identically on all three systems. There is
no 'supported on Mac only' footnote."**

*Verified:* `docs/book/src/reference/widget-parity.md`, generated from the
runtime rather than written by hand, and checked in CI so it cannot go stale.

Second claim, for a technical audience:

**"We ship the proof. Every app we have built or ported runs on all three
operating systems every night, and we check its real output — not that it
started, but that the numbers it printed are still right."**

---

## Porting: taking software that already exists

Eight real programs from GitHub now run on Krate, including a 5,396-line regex
tool, a 4,863-line markdown viewer, and a 2,677-line image viewer.

**"An image viewer built for one operating system became a file that runs on
all three, in under an hour, without its author's involvement."**

The honest detail that makes it credible: the number of repair attempts needed
fell from **5 to 1** as we fixed our own tooling. Every one of those five was
our documentation being wrong, not the porting being hard.

---

## Generating: describing an app instead of building one

Eight plain-English requests, **six working apps**. Verified by running them
and checking the answers by hand: a tip calculator ($42.50 at 20% → $8.50), a
unit converter (100 kg → 220.46 lb), an expense tracker with a running total,
a metronome that clicks through the speakers.

**"Describe an app in a sentence. Get a file that runs everywhere, inside a
sandbox it cannot escape."**

Both failures were Krate refusing to ship something inconsistent — not bad code
getting through.

---

## Lines founders respond to

- **"Electron ships a browser with every app. We ship 11 kilobytes."**
- **"The app you write can't read your files. Not 'shouldn't' — can't. We
  attack it to prove it."**
- **"Write it once. It runs on Mac, Windows, and Linux. We test all three
  every night, and we publish the results."**
- **"Your users don't install a runtime. They open a file."**

---

## Not yet claimable

Kept here so nobody accidentally promises it:

- **3D graphics.** The interface is declared and does nothing.
- **Video playback.** No decoder, no frame clock.
- **WebSockets / live connections.** HTTPS works; streaming does not. No
  multiplayer, no live feeds.
- **System menu bars.** Apps degrade to in-window buttons.
- **"The AI always makes good choices."** It does not. A generated password
  keeper stored passwords in ordinary app storage until we told the AI which
  store meant what. Krate guarantees an app cannot exceed its permissions; it
  does not guarantee good judgment inside them. Say this before someone finds
  it.

---

## For Krate Cloud (sample apps and screenshots)

Ready to show, each with a bundle already in `evidence/ported/`:

| App | Why it demonstrates something |
|---|---|
| `bounce` | 11 KB game: physics, sprites, animation |
| `chart` | an app drawing its own graphics |
| `eo2` | a real image viewer, ported from GitHub |
| `mdview` | a real markdown viewer, ported from GitHub |
| `savings` | the everyday case: a form and a list |
| `hexyl` | for developers: identical output to the original |

Screenshots need a person at a real machine on each operating system — the
nightly runs are headless. That is the next thing to capture.
