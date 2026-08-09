# 1,000 people trying Krate

Written 2026-08-10, the day counting became honest. Baseline: near zero real
installs/day (the old numbers were our own CI — proven by the arithmetic:
251 daily-active sum vs 246 distinct means machines that lived one day).
Target: 1,000 distinct installs, measured in the krate_usage Analytics
Engine dataset, nothing self-reported.

Division of labor per the standing split: Yashraj posts and talks; this file
arms him with strategy, sequencing, and finished drafts. Every claim below
is true today and gate-verified.

## The one story

**An AI writes you a real desktop app. You get one small file.** It opens on
Mac, Windows, and Linux, tells you what it wants to touch before it runs,
and gets nothing else. The workout-dashboard screenshot is a 16 KB file.
Discord solves the same one-codebase problem at 180 MB.

Three proof points that survive skeptics:
- `curl -fsSL https://krate.tech/install.sh | sh` → type `krate` → describe
  an app. Zero restarts, works with the AI CLI you already have.
- The permission wall in plain words, with the boundary named: "save files
  in its own private folder — never your files."
- Denis's line, used with credit: *"It is the moment you send someone a
  script."*

## Channel sequence (highest leverage first)

### 1. Show HN — the big swing (Tue/Wed/Thu, 8-10am ET)
One shot; don't burn it on a Monday or a weekend. Reply to every comment
for the first 3 hours. HN rewards the honest-limitations paragraph — our
whole brand is already that.

**The selling rule:** sell the suffering, not the product. Open with a
pain every reader has personally felt, in the words they would use for it
themselves. The numbers stay -- but they arrive as relief, not as specs.
Plain words, short sentences, confident, zero hard vocabulary.

**Title (pick one, ≤80 chars):**
> Show HN: Sharing a small app you made is still miserable, so I fixed that
> Show HN: You made a little app. Sending it to someone shouldn't be this hard
> Show HN: Apps you can send like a photo – 13 KB, one file, any computer

**Text:**
> You know this one. You (or your AI) make a genuinely useful little app
> in an afternoon. A folder tidier, a habit tracker, a tool for your mom's
> invoices. And then you try to give it to someone.
>
> Now you're writing install instructions. "You'll need Python 3.11."
> "Clone the repo." "It says the developer can't be verified, click Open
> Anyway." Or you wrap it in Electron and your 200-line tool ships as 200
> megabytes. Half the time the other person gives up. Honestly, half the
> time you don't even bother sending it.
>
> So I built Krate. Your app becomes one small file. Really small -- a
> playable Breakout is 13 KB, and that's the whole app, not the installer.
> The same file opens by double-click on Mac, Windows, and Linux. You send
> it like you'd send a photo. Or publish it with one command and send a
> link that runs directly.
>
> And the trust problem -- "should I really run this thing you sent me?" --
> is handled where it belongs. Before any app runs, it says what it wants
> in plain words: "save files in its own private folder -- never your
> files." Say no to something and the app still opens, just without that
> power. An app that organizes your folders can't even name a folder; it
> asks you to pick one, and your pick is the permission.
>
> Making the app is the easy part now: type `krate`, describe what you
> want, and whichever AI you already use (Claude, Codex, Gemini, Copilot,
> Grok) writes it as real compiled code. No Electron, no bundled browser.
> The generated apps get proper rounded cards, shadows, smooth animation --
> they look like apps from this year, not a science project.
>
> What it doesn't do yet, so you're not surprised: I'm not claiming it's
> hardened against actively hostile code; no heavy 3D or video; and since
> an AI writes real code, a build takes 5-12 minutes and sometimes needs a
> second try. Everything reviewers have found is on a public bug board in
> the repo, with the fixes next to it.
>
> Try it (macOS/Linux): curl -fsSL https://krate.tech/install.sh | sh
> Windows (PowerShell): irm https://krate.tech/install.ps1 | iex
> Then type `krate` and describe something you've been meaning to make.
>
> Site: https://krate.tech -- Source: https://github.com/incyashraj/krate

### 2. r/rust (same week, day after HN)
Angle: the engineering, not the pitch. Rustaceans respect no_std war
stories.

**Title:**
> Krate: shipping AI-written GUI apps as no_std wasm components — what it took

**Body sketch:** the #![no_std]-with-alloc discipline (the Vec::with_capacity
OOM-branch leaking 20 wasi imports story), the capability wall at the
component boundary, SDF-rasterized modern UI on CPU, one binary per OS
hosting everything. Link the repo first, site second. End with "the bug
board is public, reviews like Denis's shaped half of it."

### 3. r/ClaudeAI, r/cursor (angle: your AI can ship now)
> Your Claude Code can now ship real desktop apps people can actually run
Short: MCP config block (five lines), the glow screenshot, "the file it
hands back opens on all three OSes and shows a permission wall first."

### 4. X/Twitter thread (with the screenshots)
1/ This whole app is a 16 KB file. [glow.png]
2/ An AI wrote it. Claude, Codex, Gemini, Copilot or Grok — Krate drives
   whichever you already pay for.
3/ It opens on Mac, Windows and Linux. Same file. Double-click.
4/ Before it runs: [permission wall screenshot] — and the runtime enforces
   it. Deny and the app still opens, minus that power.
5/ No Electron. No bundled browser. Real compiled WebAssembly in a sandbox.
6/ curl -fsSL https://krate.tech/install.sh | sh  → type `krate` → describe
   what you want. That's the product.

### 5. Product Hunt (week 2, after HN copy is validated)
Reuse whatever framing won on HN. Needs: gallery images (glow + wall +
terminal), first-comment from maker telling the story.

### 6. Slow burn
- lobste.rs (needs an invite — worth asking someone from HN comments)
- dev.to repost of the r/rust piece
- The three /answers SEO pages already live; link them in replies rather
  than re-explaining.

## What I watch while you post (division holds)

- AE dataset: distinct installs/day, action mix, failure rates — I report
  a funnel table every day during the campaign.
- The open-failed rate on REAL users for the first time. If strangers'
  apps fail >5%, that's the next bug board entry, found within hours.
- HN/Reddit comment threads for bug reports: filed same-day, fixed fast,
  replied with the commit — that loop IS the marketing for this audience.

## Numbers that define success

- 1,000 distinct installs in the dataset (not sessions, installs).
- Secondary: 100 `make` actions (someone built), 10 `publish` (someone
  shipped). G5 needs ten strangers to make-and-send; this campaign is how
  we find them.
- If HN front-pages, expect 300–600 installs that day alone; if it
  doesn't, the Reddit+X sequence grinds to 1,000 over 3–4 weeks.

## Pre-flight checklist (all green as of v0.1.6)

- [x] install.sh / install.ps1 live, five-line output, shell named
- [x] Zero-restart first run; request survives AI connect
- [x] Double-click verified on published assets, all OSes
- [x] Site on the 2026 shell; Denis's lines credited
- [x] Release gate green on v0.1.6 (verify job, three OSes)
- [x] Counting honest: CI opted out, AE live
- [ ] Yashraj: HN account karma check, PH account, post timing picked
