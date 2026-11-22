# Obsidian versus Krate: the Krate side (IC-999)

This directory holds the Obsidian benchmark: the replica app, the vaults
both apps open, how to run the replica against them, the protocol written
before any run ([PROTOCOL.md](PROTOCOL.md)) and the kit that measures both
apps and seals the result (section 4). Nothing in this directory is a
benchmark result yet.

- The app: `apps/krate-vault` ("Vault"), a Krate app that does the work the
  Electron notes app does on the same vault. What it does and does not do is
  in [parity.md](parity.md), feature by feature.
- The vaults: `scripts/make-bench-vault.py`, deterministic, stdlib only.
- Pictures of every state: [screens/](screens/), taken headless with
  `krate run --task` steps against the generated vaults; [screens/parity/](screens/parity/)
  puts Obsidian 1.15.1 (window captures, isolated profile) and Vault side by
  side on the same vault.
- The rendering notes: [render-vault/](render-vault/) -- tables, callouts,
  math, properties, embeds, footnotes, code and Mermaid, a note each,
  with two pictures. Copy them into a generated vault (`Render/`) and open
  the same note in both apps; [screens/parity/](screens/parity/) holds
  each side by side (Obsidian 1.15.1 left, Vault right).

## 1. Make the vaults

```sh
python3 scripts/make-bench-vault.py --self-test      # byte-identical twice, counts, links resolve
python3 scripts/make-bench-vault.py --out /tmp/bench --all
```

| vault | what | files | bytes | tree_sha256 |
|---|---|---|---|---|
| light | 100 notes + Home | 101 | 152,052 | `de15775a8f33f793618de644d4a9e505784f191b50f144ee60756ffe29dbff57` |
| medium | 1,000 notes + Home | 1,001 | 1,366,268 | `89e0119e6f93311fb32e2cc925386bed47e2038e5c02239dce3b0779768ce97b` |
| heavy | 10,000 notes + Home | 10,001 | 13,733,784 | `42209ae7fc0cd817d9ffdde36d0d97bf594a57c192ba340b3b32adf3d9402040` |
| big-1000 | one note, 1,000 lines | 1 | 70,958 | `656754a544229c5e9693e673cb4a2b4d6713aabb2c0d45df7b9bb933581e4e08` |
| big-50000 | one note, 50,000 lines | 1 | 3,527,672 | `08504f9b4ad1858d962a99574cbc3c7eed222716650475b4f399b906301f1610` |
| big-200000 | one note, 200,000 lines | 1 | 14,119,307 | `1e711196337de5c2c367bcee5d90a7086706281dee27886f5a739d1d0ef0de44` |

Default seed `0x1C999`. Each vault gets `<out>/<name>.manifest.json` beside
it: every file's path, size and sha256, and `tree_sha256` over all of them.
Point both apps at the same folder; check the manifest before and after a
run (a run that types changes the vault, so copy it fresh per run).

Notes hold headings, paragraphs with bold, italic and inline code, bullet,
nested, numbered and task lists, quotes, fenced code in five languages,
`[[links]]` (plain, `|alias`, `#heading`) with a skew so some notes are hubs
with dozens of backlinks, and `#tags`, some nested. Folders: Projects,
Areas, Resources, Archive, Journal/2025, Journal/2026, People, Meetings; the
heavy vault splits each into numbered subfolders so no folder holds more
than 400 entries (Krate cannot list a folder of more than 4,096 entries:
K-1334, see parity.md).

## 2. Build the app

```sh
cargo build -p krate-cli                    # or --release for measuring
cd apps/krate-vault && cargo-component build --release && cd ../..
target/debug/krate check-app apps/krate-vault     # prints OK
```

Measure with a release runtime (`cargo build --release -p krate-cli`, or the
installed Krate): the debug runtime is many times slower at text shaping
and is only good for proving behaviour.

## 3. Run it against a vault

A person opens Vault and picks a folder (`ui.dialog:open-folder`; the pick
is the grant, for that run). A benchmark cannot answer a picker, so the app
also opens a folder named `vault` inside its sandbox root, through optional
`fs.*:./vault/**` capabilities a run grants by name:

```sh
mkdir -p /tmp/run-medium && cp -R /tmp/bench/medium /tmp/run-medium/vault
target/debug/krate run apps/krate-vault/target/wasm32-wasip1/release/krate_vault.wasm \
  --manifest apps/krate-vault/manifest.toml \
  --sandbox-root /tmp/run-medium \
  --grant ui.dialog:open-folder \
  --grant 'fs.read:./vault/**' --grant 'fs.list:./vault/**' --grant 'fs.write:./vault/**' \
  --grant 'fs.mkdir:./vault/**' --grant 'fs.remove:./vault/**' \
  -- vault=./vault open=Home.md trace
```

Quote the grants in zsh (`**`). `ui.dialog:open-folder` is required by the
manifest, so a run must grant it even though it never opens the picker. Add
`--shoot frame.png` for one headless frame, or `--task steps.toml` to drive it.

Arguments after `--`, one per word:

| argument | does |
|---|---|
| `vault=./vault` | open the folder `vault` under `--sandbox-root` |
| `open=<path in vault>` | open that note (`open=Big note 200000.md`, `open=Projects/01/Tundra brief 00001.md`) |
| `search=<query>` | run a vault search at start and show the search pane |
| `show=a,b,...` | at start: `switcher`, `palette`, `outline`, `tags`, `outgoing`, `backlinks`, `unlinked`, `properties`, `bookmarks`, `graph`, `localgraph`, `settings`, `plugins`, `appearance`, `splitright`, `splitdown`, `daily`, `recovery`, `ribbon`, `newtab`, `pin`, `stack`, `outlinehere`, `backlinkshere`, `hotkeys`, `files` |
| `also=<path>` | open another file in a new tab (repeatable) |
| `right` / `light` | open the right sidebar / draw in the light theme |
| `settle` | before the first frame, finish indexing, let a graph's layout settle and read canvas link pages (for a still picture) |
| `reading` | start the note in reading view |
| `preview=<link>` | show the page preview for that link at start (Cmd-hover cannot be done headless) |
| `sample` | open the built-in sample vault (memory only) |
| `trace` | print timing marks as they happen (below) |
| `quick` | the self-check run: sample vault, every feature once, exit 0 |

`trace` prints `key:ms` lines, milliseconds of the runtime's monotonic clock
since the app's `run()` began: `t_window_ms` (window created), `t_vault_listed_ms`
(every folder listed, the tree known), `t_note_open_ms` (the `open=` note read
and parsed), `t_first_frame_ms` (first frame presented), `t_index_done_ms`
(links and tags of every note read; backlinks complete). On exit it prints
the state as `key:value` (notes, folders, indexed, the open note's lines,
words, characters, search counts, saves, frames, lines_laid_out).

These marks are the app's own view. A sealed benchmark must time from
outside (process start to a window showing the note's text), as the
MarkText kit does; the marks are for cross-checking that.

## What the headless proof runs showed (debug runtime, not results)

Debug `krate` on the founder's Mac, 2026-11-22, only to show the app does
the work at each load:

- medium: tree listed 16 ms after the window, 1,001 notes, all indexed.
- heavy: 10,001 notes, 41 folders, all indexed; the quick switcher finds a
  note among 10,001 and its 29 backlinks show with context (screens/10).
- big-200000: the 200,001-line note opens and draws having laid out 44
  lines in total (`lines_laid_out:44`) -- only the lines shown; Cmd+Down to
  the end and typing worked, and the 14 MB note saved (`saves:1`).
- a flat folder of 4,100 notes: cannot be listed (screens/11, K-1334).

## 4. Measure both apps (the kit)

[PROTOCOL.md](PROTOCOL.md) says what is measured, how, how often and what
counts; it was committed before any run and a run is held to it.

```sh
cd evidence/benchmarks/obsidian-vs-krate
./scripts/self-test.sh                 # headless proof of the kit, about 30 s
cp config.example.env config.env       # set KRATE_BIN, KRATE_VAULT_BUNDLE, KRATE_INSTALLER
./scripts/install-obsidian.sh          # verify the DMG, copy Obsidian to ~/krate-bench/apps (no launch)
./scripts/prepare.sh                   # vaults, targets, Swift instruments, input check
./scripts/run-suite.sh                 # the session; ends with analysis.md, seal.sha256, audit.txt
```

Before `run-suite.sh`: the checklist in PROTOCOL.md section 9, and Terminal
holding Screen Recording (every leg: the note must be read off the window)
and Accessibility (the interaction leg posts keys and wheel events). The kit
checks both and never changes them. The full session is about 4.5 hours
(start 20 min, resources and idle 80 min, network 5 min, interaction 35 min,
growth 2 h); without `growth` in `BENCH_LEGS`, about 2.5 hours.

| File | Does |
|---|---|
| `scripts/install-obsidian.sh`, `fetch-obsidian.sh` | the pinned DMG, verified; installed by copy, never opened |
| `scripts/obsidian-profile.py` | an isolated Obsidian profile that opens the vault on the target note, and the check that it stayed isolated |
| `scripts/note-phrases.py` | the target note of each vault, its phrases, the switcher and search targets |
| `scripts/swift/start_probe.swift` | launch to window and launch to the note's text, from outside, the same for both apps |
| `scripts/swift/tree_sample.swift` | footprint, RSS, threads, files, CPU, wakeups of the whole process tree, from the kernel |
| `scripts/swift/interact.swift` | switcher, search, open, typing echo and scroll pacing, from frame timestamps |
| `scripts/net-poll.py` | remote addresses held after launch |
| `scripts/measure-*-leg.sh`, `run-suite.sh` | one leg each, ABBA order, every raw sample kept |
| `scripts/record.py` | one sample's raw output into one row, accepted or rejected by the protocol's rules |
| `scripts/analyze.py`, `seal.py`, `audit.py` | median/p90/n from raw only; sha256 of every file; the fail-closed verdict |
| `scripts/synth-run.py` | a synthetic run for the self-test, refused by the audit anywhere else |
