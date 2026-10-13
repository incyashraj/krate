# Publishing and opening big apps

A `.krate` is one file, and most of them are small. A game with all its
assets inside is not. The same `krate publish` and `krate run` commands take
a big app too; what changes past a few megabytes is how the bytes
travel. This page is the whole path: the size tiers, what publishing looks
like, what happens when an upload is cut off, how a big app opens from a
link, and where debugging info goes.

Every number here is the number the code enforces. The ceilings live in
`crates/bundle/src/lib.rs` (`LOCAL_LIMITS`, `HUB_LIMITS`, `HUB_BIG_LIMITS`)
and the publish flow in `crates/cli/src/main.rs` (search for `publish/big`).
If this page and the code ever disagree, the code is right.

## The size tiers

"The size" is the `.krate` file as it sits on disk.

| What | Size | Decided by |
|---|---|---|
| Opening a file on a desktop | up to 2 GiB | `LOCAL_LIMITS` |
| One ordinary publish request | up to 5 MiB | the publish door (`MAX_UPLOAD_BYTES`) |
| Krate Cloud's big-file path | up to 1 GiB | `HUB_BIG_LIMITS` |

- **A file on a desktop opens up to 2 GiB.** `krate run app.krate`, a
  double-click, the Player: one game with everything packed inside opens
  from the file in your hand. This is the tier a big app downloaded from a
  link also opens under.
- **One ordinary publish request takes up to 5 MiB.** Up to that,
  `krate publish app.krate` sends the bundle in a single request and the
  hub answers at once.
- **Past 5 MiB, `krate publish` switches to the big-file path on its own**
  and takes up to 1 GiB. The bundle goes up in 64 MiB parts and is checked
  as a whole before anything is listed.

What is refused:

- **Past 1 GiB, Krate Cloud will not take the app**, and says so: "this app
  is N bytes; Krate Cloud takes apps up to 1073741824 bytes. It still opens
  from the file on a desktop."
- **Past 2 GiB, no desktop opens it either.**
- Each door also bounds what is inside the file -- one entry, one asset,
  the totals, the file counts -- so a bundle that unpacks into far more
  than it arrived as is refused everywhere. `krate judge` at the end of
  this page gives the hub's own answer for any file.

A big app is a desktop app. `krate pack` says it plainly when a bundle
passes the big tier: "it opens from the file on a desktop ... It does not
open on a phone."

### While the big-file path is new

The big-file path needs more than a file:

- **A GitHub sign-in.** `krate publish` asks for it the first time; past
  5 MiB nothing is sent without it ("not signed in. Sign in first, then
  publish again.").
- **A place on the publisher list.** The path is open to a few publishers
  while it is new, and a refusal says to write to the Krate team to be
  added.
- **Room in the day's quotas:** three big publishes a day per publisher,
  ten across everyone, and five big apps live per publisher ("unpublish
  one to add another"). Only the newest two versions of each app name are
  kept; older ones' bytes are dropped.

None of this applies to an app of 5 MiB or less, which publishes as it
always has.

## What publishing looks like

```bash
krate publish my-game.krate
```

One line per part as the bundle goes up -- this is a 370 MB app, six parts
of 64 MiB:

```text
  sent 67 MB of 370 MB
  sent 134 MB of 370 MB
  sent 201 MB of 370 MB
  ...
  sent 370 MB of 370 MB
  uploaded; Krate Cloud is checking it
Published. Anyone can run it with:
  krate run https://krate.tech/a/0123...cdef
  archive identity 0123...cdef  (the exact file bytes)
```

- **The parts first.** Each part is tried three times before the publish
  stops. The line names how much of the file has landed, not how fast the
  link is.
- **Then the check.** Krate Cloud judges the whole bundle against the big
  tier's limits before anything is listed or served -- the same kind of
  check `krate run` applies on the recipient's machine, at the hub's own
  ceilings. The CLI asks after it every few seconds.
  If the check is still going after twenty minutes it stops waiting and
  says the app will be listed when the check passes; run the same publish
  again to ask. A bundle the check refuses is reported with the reason and
  is not listed.
- **Then the link.** It is printed only after the check passes: the
  `krate run` line anyone can use, and the archive identity -- the digest
  of the exact file bytes you sent, so the file in your hand can be matched
  to the one at the link.

If the exact same bytes are already on Krate Cloud, nothing is uploaded:
you get "these exact bytes are already on Krate Cloud" and the link that
is already there.

## Resuming an interrupted upload

An upload that stops partway does not start over. While a publish is in
flight, `krate publish` keeps its state in `~/.krate/uploads/`, one JSON
file per upload named after the sha256 of the bytes (for an app:
`<sha256>-bundle.json`). It records the hub's upload id, the part count,
and the etag of every part that landed.

To resume, run the same `krate publish` command again. Parts with an etag
are skipped; only the parts still missing are sent. The file's digest is
part of the state's name, so this works only for the same bytes -- a
rebuild is a new upload.

Two things make a resume start over rather than continue:

- **The upload's token expires.** It is good for two days. If the hub
  answers a part with 401, the state file is deleted and the next publish
  starts a fresh upload ("this upload's token expired; publish again to
  start over"). Hub-side, an unfinished upload's record is kept for eight
  days.
- **You publish to a different hub** than the one the state names.

When the hub gives its verdict -- listed or refused -- the state file is
removed.

## Opening a big app from a link

```bash
krate run https://krate.tech/a/0123...cdef
```

An ordinary app is read and opened in one step. A big one is **saved to a
file once, then opened from that file**:

- The download prints its progress -- `downloading: 67 of 370 MB (18%)` --
  about every tenth of the way.
- It lands in Krate's downloads folder: `~/.cache/krate/downloads` on
  Linux, `%LOCALAPPDATA%\Krate\cache\downloads` on Windows
  (`KRATE_CACHE_DIR` moves the root). The file is named after the sha256
  of its bytes.
- **It is kept.** Open the same link again and the saved file is used when
  it is there under the app's address and has the right size; the bytes are
  fetched again only otherwise.
- **The bytes are held to the address in the link** -- the long hex name in
  the URL is the digest of the app. Bytes that do not match it are refused
  and deleted, never run: "the bytes served at this address are not the
  ones it names".
- A download that stops partway keeps nothing and says
  "the download did not finish (X of Y bytes). Try again."

Opening from the saved file is then exactly like opening any local file:
the desktop tier, up to 2 GiB.

## Debugging info in `krate pack`

`krate pack` leaves the component's debugging info out of the bundle by
default -- the `.debug_*` sections, which one real build carried 194 MB of
inside a 234 MB module, and which nobody running the app ever reads. The
command says how much it left out:

```text
wrote my-game.krate (176000000 bytes)
left out 27.0 MB of debugging info nobody running the app reads (KRATE_KEEP_DEBUG=1 keeps it)
```

To keep it, set `KRATE_KEEP_DEBUG=1` when packing:

```bash
KRATE_KEEP_DEBUG=1 krate pack target/wasm32-wasip1/release/my_game.wasm \
  --manifest manifest.toml --output my-game.krate
```

Useful when you are tracing a crash and want the same stack lines the
debugger asks for. Remember the size tier the finished file lands in: the
debugging info counts toward it.

## Check the numbers yourself

```bash
krate judge --tier hub my-game.krate      # the publish door's answer
krate judge --tier hub-big my-game.krate  # the big-file path's answer
```

`krate judge` is the hub's own verdict for a file, by a door's limits. The
size is checked from the file first, so a file past the ceiling is refused
without being read. Exit code 0 means accepted, 1 means refused.

Related: [Create and Share an App](create-and-share.md) for the small-file
flow, and [What Krate cannot do yet](limits.md) for everything else about
where Krate stands today.
