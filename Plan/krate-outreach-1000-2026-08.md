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

**Title (pick one, ≤80 chars):**
> Show HN: Krate – AI writes a desktop app, you get one 16 KB file that runs anywhere
> Show HN: I made AI-written desktop apps shareable as one small sandboxed file

**Text:**
> AI can write a useful little desktop app in minutes. Sharing it is still
> the broken part: a repo, a toolchain, "works on my machine."
>
> Krate packs the app and its permissions into one file (typically 15–60
> KB — real WebAssembly compiled from Rust the AI writes, no Electron, no
> bundled browser). The same file opens on macOS, Windows, and Linux by
> double-click. Before it runs, it shows what it wants in plain words —
> "save files in its own private folder, never your files" — and the
> runtime enforces the answer. Deny something and the app still opens,
> just without that power.
>
> You bring your own AI CLI (Claude, Codex, Gemini, Copilot, Grok — it
> drives whichever you have). Type `krate`, describe the app, ~5–12 min
> later you have a file you can email to someone.
>
> Honest limits: the permission boundary works but I'm not yet claiming
> hardening against hostile code; heavy 3D and video aren't there; apps
> are Rust-compiled components so the AI writes real code, which sometimes
> takes a retry. An outside reviewer's findings and my fixes are all on
> the public bug board in the repo.
>
> Install: curl -fsSL https://krate.tech/install.sh | sh (Windows in
> PowerShell: irm https://krate.tech/install.ps1 | iex)
> Site: https://krate.tech — Source: https://github.com/incyashraj/krate

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
