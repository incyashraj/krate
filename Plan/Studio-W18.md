# W18: Krate Studio — the full requirement list

Written 2026-08-14 from Yashraj's spec, so nothing on it gets lost. Each item
carries its status and, where done, how it was verified. BUGS.md stays the
bug list; this is the studio's build sheet.

## Must-have features

| # | Requirement | Status |
|---|---|---|
| 1 | **Dev history**: return to any past session, resume, change the app after feedback | **building** — sessions stored as JSON per conversation (id, title, messages, bundle path); home screen lists them; opening one restores the thread and the done card, and the next message revises that bundle |
| 2 | **Attachments**: any file format, used by the AI to shape the app | **building** — engine grows `--attach` on create and revise (the revise machinery already copied files in and told the agent to read them; create gets the same); studio gets a paperclip |
| 3 | **No full rebuild for small changes** | **done, measured** — `krate revise`: bundles carry their own source, the AI edits in place. One-line change: 1.6 min vs ~6 from scratch, before/after frames byte-identical |
| 4 | **Own AI after funding — possible?** | **yes, by design** — the engine's `AgentProvider` trait + `--author-cmd` seam means "Krate AI" is one more provider: a hosted endpoint (server runs the model, streams edits) or a bundled local model. Zero studio changes; the chip gains one entry. Cost sits server-side, which is exactly what funding buys. Recorded here as the answer of record |
| 5 | **Account required + browser login that returns to the app** | **building** — GitHub device flow already in the engine (opens browser, polls, stores identity). Studio gates on it at launch: sign-in screen shows the code, browser opens, the poll completing flips the app in — no manual step back |
| 6 | **Connect any AI, easiest possible** | **building** — `krate ai --json` probes every provider with reason + remedy; the connect panel shows each with its one fix (install command / sign-in) and a working one is one click to switch. The `--author-cmd` seam remains the "any other AI" door |
| 7 | **Live steps with a details view** | **done** — staged progress with elapsed time; every raw engine line streams into a collapsed details log |
| 8 | **Design: modern, intentional, "magical"** | **ongoing** — site's Geist/dark language; reveal moment when the app finishes; every placement gets a reason or gets cut |
| 9 | Stop a running session | **building** — the spawned engine child is tracked and killable from the UI |
| 10 | Choose the working directory | **building** — settings; default stays ~/Documents/Krate Apps |
| 11 | Easy AI switching | **building** — the agent chip opens the connect panel; pick = switch |
| 12 | Ask to open the app when done | **done** — the done card leads with Open it; consider a gentle auto-focus, never an auto-open |
| 13 | Krate Cloud inside the studio | **partial** — v1 shows your local session history as "your apps"; a true per-account published list needs the hub to record identity per upload (it records a name today, not a queryable account id) — hub work, filed below |
| 14 | **Short share links, old links stay valid** | **building** — hub mints an alias at publish (`/a/<12-hex>`); the full-hash URL keeps working forever because it is the content address itself. Investor links unaffected |

## Standing rules for the studio

- The shell stays thin: every operation is the same `krate` engine the
  terminal uses. If the studio and the CLI ever disagree, one is lying.
- Plain words at eye level, raw lines one click away. A person in this app
  never meets a compiler error.
- Honest time: builds say minutes and show elapsed; changes say "the AI
  reads before it edits".

## Filed hub work (separate from the studio)

- Per-account published-apps listing: publish carries the signed-in identity
  today as a display name; listing "mine" needs the account id stored per
  app and an authed `/apps?mine=1`. Small worker change, needs care around
  anonymous publishes staying anonymous.
