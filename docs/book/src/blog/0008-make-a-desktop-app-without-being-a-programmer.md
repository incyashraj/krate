# How to make a desktop app without being a programmer

**Published:** 2026-08-05

You can describe a small desktop tool in one sentence and have a working
version of it in a few minutes. This is genuinely new, and it is worth
explaining exactly what is and is not required, because most explanations
either overstate it or bury it in setup instructions.

## What you need

Two things:

- **Krate installed.** One command:
  `curl -fsSL https://krate.tech/install.sh | sh`
- **An AI coding tool you already have.** Claude Code, Codex, Gemini, Copilot,
  or Grok — any one of them. If you already use one for anything, you are done.

You do not need to know Rust. You do not need to install build tools by hand;
Krate checks what is missing the first time and offers to set it up.

## The whole process

Type this:

```
krate
```

It opens a short menu. Pick **Make an app**, then say what you want in ordinary
words:

```
a habit tracker with a weekly grid
```

Krate checks which of your AI tools actually work right now — one that is
installed but not signed in is listed with the command that fixes it, so you
cannot pick one that will fail — and asks which should write it.

Then it builds, showing each stage:

```
✓ read Krate's API reference        4s
✓ wrote the app's code           1m 12s
✓ compiling it                     52s
▸ checking it runs and paints a frame
```

Two to five minutes later you have a file, typically 15 to 40 KB, and Krate
offers to open it, change it, or publish it.

## Changing it

This is the part that makes it a tool rather than a vending machine. Pick
**Make a change** and say what should be different:

```
make the grid monthly instead of weekly
```

The app carries its own source inside the file, so the AI edits what exists
rather than starting over. That also means anyone you send the file to can
change it — not just you, and not just on the machine that made it.

## Sending it to someone

The file is the app. Email it, message it, put it on a USB stick. The person
who receives it installs Krate once and double-clicks the file, the same as a
document.

Before it opens, Krate shows them what it wants, in the app's own words: "open
a window", "save its own list". Anything not on that list is unavailable to the
app. They can open something a stranger made without reading a line of code.

Or publish it:

```
krate publish habit-tracker.krate
```

That signs you in with GitHub the first time, uploads the file, and hands back
a link anyone can run. It also appears on
[Krate Cloud](https://krate.tech/cloud) with your name on it. There is a
[web page](https://krate.tech/publish) that does the same thing if you would
rather drag the file into a browser.

## What it will not do

Being straight about the edges:

- **Small apps, not large ones.** Calculators, timers, trackers, viewers, small
  games. Not a video editor.
- **Two to five minutes each time.** It is compiling real code, not filling in
  a template. Nothing removes that wait.
- **It is early.** Krate is pre-alpha. Things break, and the
  [list of what does not work yet](https://krate.tech/progress) is public.
- **The AI sometimes gets it wrong.** Krate checks the app builds, runs, and
  responds before handing it to you, so what reaches you usually works — but
  "usually" is the honest word.

## Try it

```
curl -fsSL https://krate.tech/install.sh | sh
krate
```

If something is confusing or broken, [tell
us](https://krate.tech/contact) — the confusing parts are the ones we most need
to hear about.
