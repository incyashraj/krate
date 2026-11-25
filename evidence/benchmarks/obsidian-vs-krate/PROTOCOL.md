# Protocol: Vault (Krate) against Obsidian on macOS (IC-999)

Written 2026-11-22, before any measured run, and committed on its own so
its date and text can be checked against every run that cites it. A run
records the commit and sha256 of this file; `scripts/audit.py` refuses a
run made while this file had uncommitted changes. Changing the protocol
after a run means a new protocol commit and new runs, never an edited old
run.

## 1. Scope

- **What is compared.** Krate's "Vault" (`apps/krate-vault`, packed as a
  `.krate`, run by the Krate build named in `config.env`: a release-profile
  build of the `next` branch at a recorded commit, the work that ships as
  the next release, named as that build in every result) against
  Obsidian 1.15.1 (the official universal DMG, sha256
  `e05c7e3e2262b14c965bb246dcb74a30fc9098bd669e42957c4d9f788d04fdf4`,
  run as arm64), opening byte-identical vaults made by
  `scripts/make-bench-vault.py`.
- **Vault is not Obsidian.** It is a measured replica of Obsidian's core,
  with the gaps listed feature by feature in [parity.md](parity.md) (no
  graph, canvas, plugins, sync, tables, embeds, properties, split panes,
  among others). Every result is an equivalent-workload comparison on the
  workloads named here, not a claim of feature parity, and is quoted with
  that sentence.
- **Machine.** macOS on Apple silicon (arm64) first. Windows and Linux
  are later runs under a later protocol revision; nothing here may be
  quoted for them.
- **Who measures.** One measured session on the founder's Mac, then the
  same command run by someone other than the author (IC-999 close-with).

## 2. Hypotheses

Stated before the data, with the direction we expect. Each is tested on
the numbers the kit records; a hypothesis that fails is reported as
failed, beside the ones that hold.

- H1 size: the Vault `.krate` is smaller than Obsidian's installed app by
  more than 1,000 times, and Krate's installer plus installed runtime plus
  Vault is smaller than Obsidian's DMG and installed app.
- H2 start: median time from launch to the note's own text on screen is
  lower for Vault at every load, warm and cold.
- H3 memory: whole-process-tree `phys_footprint` at T+30 s and T+120 s is
  lower for Vault at every load.
- H4 processes: Vault runs as fewer processes and threads.
- H5 idle: idle CPU and idle wakeups are not assumed better. The
  2026-08-25 MarkText run found idle CPU was not an honest Krate advantage
  (Vault blinks its caret and polls its folder every 3 s). This is
  measured, with no expected direction.
- H6 network: Vault contacts no remote address in its first 60 s other
  than Krate's own usage report, if that is on; Obsidian contacts its
  update servers.
- H7 growth: neither app's footprint grows by more than 10 % over 60 idle
  minutes with the heavy vault open.
- H8 interaction: quick switcher, vault search, opening a note and typing
  echo are no slower in Vault at the light and heavy loads. No expected
  direction for scroll pacing on the 50,000-line note.

## 3. Inputs

| Input | Fixed how |
|---|---|
| Obsidian 1.15.1 DMG | sha256 above; architecture checked with `lipo -archs` headlessly; installed by `scripts/install-obsidian.sh` into `~/krate-bench/apps/`, never `/Applications` |
| Krate | `cargo build --release -p krate-cli --features speech` (the macOS release build's own command and profile) of the commit in `config.env` (`KRATE_COMMIT`); `krate version` must report the version in `config.env`; binary sha256 recorded; the installer for the size leg is that binary packed as the release packs it (`krate-<version>-aarch64-apple-darwin.tar.gz`) |
| Vault | one `.krate` file; sha256, size and the manifest inside it recorded, and the file copied into the run |
| Vaults | `make-bench-vault.py` default seed `0x1C999`: light 100 notes, medium 1,000, heavy 10,000 (each plus `Home.md`), big-50000 and big-200000 single notes; each `tree_sha256` must equal the README table |
| Target notes | `scripts/note-phrases.py`: the note opened at start is the generated note numbered 00001 (the busiest hub, most backlinks) in light/medium/heavy, and the single note in big-*; switcher and search targets are the last-numbered note |

## 4. How each app is started

Both apps are started from their executables by the same instrument
(`build/start-probe`), with stdin from `/dev/null` and stdout/stderr to a
log file, so neither runs attached to a terminal.

- **Obsidian.** `Obsidian.app/Contents/MacOS/Obsidian
  --user-data-dir=<profile>/user-data`. `scripts/obsidian-profile.py`
  writes, before launch: `<user-data>/obsidian.json` with one vault
  registered (`path`, `ts`, `open: true`), so Obsidian opens that vault
  instead of its vault chooser; `<user-data>/<vault id>.json` with the
  window size (1280 x 800 points, the size Vault is told to open at); and
  `<vault>/.obsidian/workspace.json`: the layout Obsidian 1.15.1 itself
  wrote on its first launch, with the main pane's empty "New tab" leaf
  replaced by a markdown leaf on the target note and `lastOpenFiles` and
  `active` set to match. Nothing else in Obsidian's settings is changed:
  updates as shipped (on), theme as shipped, no plugins, right sidebar
  collapsed as shipped.
- **Vault.** `KRATE_CACHE_DIR=<profile>/krate-cache krate run <vault.krate>
  --profile <bench name> --sandbox-root <profile>/root --grant ... --
  vault=./vault open=<target note> trace`, the vault copied to
  `<profile>/root/vault`. Krate's usage report is as shipped (on unless the
  machine sets `KRATE_NO_USAGE`; recorded).
- **Window.** Both 1280 x 800 points: Obsidian through its saved window
  size, Vault through its `size=1280x800` argument (its own default is
  1024 x 800, Obsidian's first-launch size). The window bounds each app
  actually showed are recorded per sample and reported.
- **Profiles and vault copies** live under a `*.noindex` folder so
  Spotlight does not index them during a run.

### Obsidian facts verified 2026-11-22

Seen by the lead on one launch of Obsidian 1.15.1 on this Mac, outside any
measured run (nothing from it is a result):

- The DMG's app is universal (`lipo -archs`: x86_64 arm64), signed by
  Developer ID Team 6JSW4SJWN9 and notarized; the installed `.app`, copied
  with `ditto` from the read-only mounted DMG, is 526 MB (`du -sh`).
  Verified 2026-11-22.
- Launching the executable directly with `--user-data-dir=<profile>` keeps
  every Electron and Obsidian file inside `<profile>` (Cache, GPUCache,
  IndexedDB, Local Storage, `obsidian.json`, `obsidian.log`, `id`, ...).
  Verified 2026-11-22.
- `<profile>/obsidian.json` = `{"vaults":{"<id>":{"path":"<absolute
  vault>","ts":<ms>,"open":true}}}` written before launch opens that vault
  at once: no vault chooser, no "trust author" prompt (the vault has no
  community plugins). Obsidian then writes `<vault>/.obsidian/` (`app.json`,
  `appearance.json`, `core-plugins.json`, `workspace.json`) and logs
  `ENOENT <profile>/<id>.json` when the per-vault window file is absent,
  which is harmless. Verified 2026-11-22.
- With no `workspace.json`, the main pane is an empty "New tab"; the
  target note is opened by the pre-written layout above. Verified
  2026-11-22 (the empty tab; the pre-written layout is assumption 2 below).
- About 8 s after launch on the light vault the tree was 4 processes: main
  164 MB RSS, two Helpers (92 MB, 49 MB) and a Renderer (213 MB). An
  observation, not a result. Verified 2026-11-22.
- At start Obsidian checks for updates (stdout: "Checking for update using
  Github ... App is up to date"), and within 8 s `lsof` showed two TLS
  connections from a Helper, to 2606:4700:20::681a:193 (Cloudflare) and
  2606:50c0:8002::154 (GitHub). This is default behaviour and is measured
  as such, never turned off; every run records it. Verified 2026-11-22.
- Obsidian's stdout and stderr carry its own log; the kit keeps them per
  sample. Verified 2026-11-22.

### Obsidian assumptions to verify on the first measured launch

Read from Obsidian 1.15.1's own `app.asar` and `obsidian.asar`; not yet
seen working. The first launch of the measured session checks each, and
any that is false stops the session until this protocol is revised:

1. The person's own `~/Library/Application Support/obsidian` is never read
   or written. The kit checks it after every Obsidian launch: the profile
   must hold `obsidian.log` and `id`, and the modification time of the
   person's own folder must not change.
2. The pre-written `workspace.json` (Obsidian's own first-launch layout
   with a markdown leaf on the target note) opens that note in the main
   pane in live preview. The correctness gate catches it if not.
3. `<vault id>.json` holding only `width` and `height` sets the window
   size, centred (Obsidian's `main.js` uses both when neither `x` nor `y`
   is given). The recorded window bounds show it.
4. No release-notes tab opens on a fresh profile (in `app.js` it opens
   only when the recorded last-installed version is older than the app).
5. With the person's own Obsidian quit, Obsidian's command-line socket
   (`~/.obsidian-cli.sock`, in the home folder, not the profile) belongs to
   the measured instance only. The kit refuses to start a leg while any
   Obsidian process it did not start is running.
6. If a newer Obsidian is published during the session, the updater
   downloads `obsidian-<version>.asar` into the profile and the next
   launch would run it. A sample whose profile gains such a file is
   rejected (`obsidian_downloaded_update`) and the file is never used.
7. No Gatekeeper first-open dialog: the DMG was fetched with curl and has
   no quarantine flag; `install-obsidian.sh` refuses a quarantined copy
   rather than clearing the flag.
8. Keyboard and wheel events posted by `interact` (CGEvent, US key codes,
   to the frontmost app, which it checks before every event) reach both
   editors as typed text, and Cmd+O and Cmd+Shift+F open the quick
   switcher and search in both. The interaction leg needs Accessibility
   for Terminal, granted by the person; no assistant tool drives either
   app.

## 5. What is measured, and how

Every leg runs both apps in ABBA order (run 1: Vault then Obsidian; run 2:
Obsidian then Vault; run 3: Obsidian then Vault; run 4: Vault then
Obsidian; repeat), keeps every raw sample including rejected ones with a
reason, and waits 2 s between launches after the previous process tree
has gone.

**Correctness gate.** A sample counts only when `start-probe` (Apple
Vision text recognition of the app's own window, `screencapture -l`) read
at least 3 distinct phrases from the target note's own body text on
screen. Phrases are 5 consecutive plain words from one line of the note,
unique within the note, so file names in a sidebar or lines of other notes
in a backlinks pane do not count. A sample whose note never appeared is
kept, marked `rejected`, and contributes no number.

| Leg | Measured | Instrument | Samples |
|---|---|---|---|
| size | Obsidian DMG bytes; installed `.app` `du -sk` and apparent bytes (`du -A`), file count; Krate installer bytes, installed Krate (every path in `KRATE_INSTALLED_PATHS`), Vault `.krate` bytes | `stat`, `du`, `find` | 1 |
| startup warm | launch to first on-screen layer-0 window of the process tree (`window_ms`); launch to the note's text visible (`content_ms`, upper bound, with the previous capture's start as lower bound) | `start-probe`: `CGWindowListCopyWindowInfo` polled every 15 ms; OCR polled as fast as capture allows | 10 per app per load, after 2 unmeasured warm-ups |
| startup cold | same, first launch on a fresh profile and fresh vault copy (Obsidian: new user-data dir; Vault: new `--profile` and empty `KRATE_CACHE_DIR`). The OS file cache is not purged (that needs sudo); cold means a new profile, not a rebooted Mac | `start-probe` | 5 per app per load |
| resources | at T+30 s and T+120 s after launch: `phys_footprint` and RSS summed over the process tree, process count, thread count, open file descriptors; at T+30 s also `/usr/bin/footprint` over the tree as a cross-check | `tree-sample` (`proc_pid_rusage`, `proc_pidinfo`) | 3 per app per load |
| idle | from T+30 s to T+150 s: CPU % of one core (CPU time delta / wall time), package idle wakeups and interrupt wakeups per second, billed energy from rusage; Energy Impact and idle wakeups from `top -stats pid,command,cpu,power,idlew` summed over the tree, 1 s samples | `tree-sample`, `top` | same launches as resources |
| network | every remote address:port any process of the tree (or any process running the app's own executables, so a detached Krate usage sender is included) holds in the first 60 s; DNS is not resolved by the kit | `lsof -i -n -P` every 0.5 s (a connection shorter than 0.5 s can be missed; stated with the result) | 1 cold-profile and 1 warm launch per app, light vault |
| growth | `phys_footprint` of the tree every 60 s for 60 min, idle, heavy vault open | `tree-sample` | 1 per app |
| interaction | quick switcher: Cmd+O to first changed frame, last query key to the target name read on screen; vault search: Cmd+Shift+F, phrase query, last key to the target note's name on screen; open note: Enter in the switcher to the note's phrases on screen; typing echo: key down to the first frame not seen before the key (caret blink frames are learnt first and excluded), 30 keys per sample; scroll pacing on big-50000: 20 s of wheel events at 125 Hz, gaps between changed frames (median, p95, p99, longest), and the note must have moved (OCR before and after) | `interact` (ScreenCaptureKit frames with display timestamps, CGEvent input, Vision OCR of captured frames) | 10 per app per load (light, heavy) for switcher/search/open; 3 x 30 keys for typing at light and big-50000; 3 scroll runs |
| energy (optional) | `powermetrics --show-process-energy` 30 s after settling | needs `sudo -v` by the person first | 1 per app per load |
| battery (optional) | battery charge (`ioreg` AppleSmartBattery CurrentCapacity/MaxCapacity, mAh) before and after 30 min with the app open on the medium vault and a scripted typing pass every 5 min, on battery | `ioreg`, `pmset -g batt` | 1 per app, ABBA over two sessions if repeated |

Loads for startup, resources and idle: light, medium, heavy, big-50000,
big-200000.

## 6. What is reported

For every leg, app, load and mode: **median, p90 and n** of the accepted
samples, the range, and the count and reasons of rejected samples.
Ratios are only ever of medians, computed from raw values, never from
rounded ones. No fastest sample is quoted. `analysis.md` is generated from
`raw/` by `scripts/analyze.py` alone and is checked by regeneration.

## 7. Exclusion rules

A sample is excluded (kept, marked rejected) only for:

1. the target note's text was not read on screen within the leg's timeout
   (60 s; 180 s for a cold heavy or big-200000 start);
2. the app exited, crashed, or showed no window;
3. Rosetta translation of any process in the tree;
4. Obsidian downloaded an update into its profile;
5. focus was not on the app under test when an input event was due (the
   interaction leg checks the frontmost process before every event and
   stops rather than type into another app);
6. a scroll that did not move the note.

No other exclusion is allowed, and none is applied by hand. If one app has
more than 20 % of its samples in a leg rejected, the audit fails that leg
for both apps: the instrument does not work there, and no number from it
is quoted.

## 8. What counts against Krate

Reported in the same tables, never dropped:

- any load or leg where Vault's median is worse than Obsidian's;
- Vault failing a load (the 4,096-entry folder limit, K-1334, is avoided
  by the generator's 400-entry folders and is stated beside every heavy
  result);
- idle CPU and wakeups from Vault's caret blink and 3 s folder poll;
- the size of the shared Krate runtime, always shown beside the Vault
  `.krate` size;
- every parity gap in parity.md, linked from every result;
- Obsidian indexes the vault into its own cache on first open; Vault
  indexes in memory every start. Both are what each app does as shipped
  and are measured as such.

## 9. Machine preparation (checklist, before `run-suite.sh`)

- [ ] Power adapter connected (battery leg: unplugged, charge above 80 %).
- [ ] Low Power Mode off (`pmset -g` shows `lowpowermode 0`; recorded).
- [ ] Display brightness fixed at the same level for the whole session;
      auto-brightness and True Tone off; display sleep and screen saver
      set to Never for the session.
- [ ] Every other app quit, including the person's own Obsidian and any
      Krate app; menu-bar utilities quit where possible.
- [ ] Do Not Disturb on, so no notification takes focus.
- [ ] Time Machine, Spotlight reindexing and software updates not running
      (`tmutil status`, `mdutil -s /`).
- [ ] The Mac idle for 5 minutes before the first leg; lid open.
- [ ] Terminal has Screen Recording (every leg with the correctness gate)
      and Accessibility (the interaction leg only) in System Settings >
      Privacy & Security. The kit only checks; it never changes a setting.
- [ ] Nothing else is recorded or driven on this Mac during the session.

`scripts/capture-machine.sh` records model, chip, memory, macOS version
and build, power source, Low Power Mode, appearance, displays and uptime
at the start and the end of every run; `audit.py` refuses a run whose
machine or OS build differs between start, end and the prepared manifest.

## 10. Recording so it cannot be denied

A run directory holds this protocol's sha256 and commit, the kit's commit,
`config.snapshot.env`, `inputs.tsv` (every input's path, bytes, sha256,
version and architecture, taken at start and again at the end), the
machine at start and end, the Vault `.krate` itself, a copy of every kit
script, and every raw sample. `scripts/seal.py` writes a sha256 for every
one of those files; `analysis.md` and `audit.txt` are derived and checked
by regeneration instead. `audit.py` refuses a run with missing samples,
failed correctness gates beyond the rule in section 7, a different machine
or OS, inputs that changed during the run, an uncommitted protocol, or any
file changed after the seal. The seal's digest is then timestamped in a
public transparency log (IC-999 item 4) as a separate step after the
founder approves publishing it.

Reproduce: `cp config.example.env config.env`, set the paths, then
`./scripts/prepare.sh && ./scripts/run-suite.sh`.
