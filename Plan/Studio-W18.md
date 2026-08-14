# W18: Krate Studio — the full requirement list

Written 2026-08-14 from Yashraj's spec, so nothing on it gets lost. Each item
carries its status and, where done, how it was verified. BUGS.md stays the
bug list; this is the studio's build sheet.

## Must-have features

| # | Requirement | Status |
|---|---|---|
| 1 | **Dev history**: return to any past session, resume, change the app after feedback | **built** — sessions stored as JSON per conversation (id, title, messages, bundle path); home screen lists them; opening one restores the thread and the done card, and the next message revises that bundle |
| 2 | **Attachments**: any file format, used by the AI to shape the app | **built, then made findable** — engine grows `--attach` on create and revise; the studio's paperclip only existed inside a session, so the home composer (where the first description is typed) now carries a labelled "Add a picture or file" and both rows paint from the same state |
| 3 | **No full rebuild for small changes** | **done, measured** — `krate revise`: bundles carry their own source, the AI edits in place. One-line change: 1.6 min vs ~6 from scratch, before/after frames byte-identical |
| 4 | **Own AI after funding — possible?** | **yes, by design** — the engine's `AgentProvider` trait + `--author-cmd` seam means "Krate AI" is one more provider: a hosted endpoint (server runs the model, streams edits) or a bundled local model. Zero studio changes; the chip gains one entry. Cost sits server-side, which is exactly what funding buys. Recorded here as the answer of record |
| 5 | **Account required + browser login that returns to the app** | **built** — GitHub device flow already in the engine (opens browser, polls, stores identity). Studio gates on it at launch: sign-in screen shows the code, browser opens, the poll completing flips the app in — no manual step back |
| 6 | **Connect any AI, easiest possible** | **built** — `krate ai --json` probes every provider with reason + remedy; the connect panel shows each with its one fix (install command / sign-in) and a working one is one click to switch. The `--author-cmd` seam remains the "any other AI" door |
| 7 | **Live steps with a details view** | **done** — staged progress with elapsed time; every raw engine line streams into a collapsed details log |
| 8 | **Design: modern, intentional, "magical"** | **ongoing** — site's Geist/dark language; reveal moment when the app finishes; every placement gets a reason or gets cut |
| 9 | Stop a running session | **built** (kills the whole process tree, so the agent stops burning quota) — the spawned engine child is tracked and killable from the UI |
| 10 | Choose the working directory | **built** — settings; default stays ~/Documents/Krate Apps |
| 11 | Easy AI switching | **built** — the agent chip opens the connect panel; pick = switch |
| 12 | Ask to open the app when done | **done** — the done card leads with Open it; consider a gentle auto-focus, never an auto-open |
| 13 | Krate Cloud inside the studio | **built** — a Cloud view lists everything published (name, author, size, age) from `hub.krate.tech/apps`, and opening one runs it by URL through the engine's own permission wall. Read in Rust so the webview keeps its CSP. A per-account "mine" filter is still the hub work filed below |
| 14 | **Short share links, old links stay valid** | **done, deployed** (`/a/f2deb8a76496` verified serving; old 64-hex links verified) — hub mints an alias at publish (`/a/<12-hex>`); the full-hash URL keeps working forever because it is the content address itself. Investor links unaffected |

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

## Verified so far / still to drive

Built and verified in the running shell or the design-review browser: gate,
home with history, resume-with-done-card, connect-AI sheet, attach chips,
stop, settings. Verified separately in the engine: create, revise (1.6 min,
one-line diff), account --json, login NDJSON, --attach staging, short links.

**Identity (2026-08-14).** Apps made with Krate now present as themselves.
`krate install` wraps a .krate in its own `.app`: own name in the dock, own
icon, own Launchpad entry. The mechanism was found by measurement -- setting
`CFBundleName` on the running process succeeds and macOS ignores it, and a
shell shim fails because `exec` replaces it; what works is the engine hard
linked to `Contents/MacOS/<App Name>`. Double-clicking an installed .krate
hands off to that wrapper, so it opens as itself rather than as "krate-cli".

**Launch routing (2026-08-14).** Opening Krate.app with no document opens
Krate Studio; opening a .krate runs that app. Verified each route in
isolation.

Still to drive end to end: installers (.msi/AppImage) via CI; the hub's
per-account app listing.
