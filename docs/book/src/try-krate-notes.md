# Try Krate Notes

Krate Notes is a small note taking app. It is one file, about 12 kilobytes,
and it can only touch the folder you allow it to.

This page is written for someone who has never used Krate before.

## What you need

The Krate runtime. Until signed releases land you build it once from source,
which takes a few minutes:

```bash
git clone https://github.com/incyashraj/krate
cd krate
cargo build -p krate-cli
```

That produces `target/debug/krate`. Everything below uses it.

## 1. Get the app

The quickest path is to skip downloading entirely and run it straight from its
release URL:

```bash
krate run https://github.com/incyashraj/krate/releases/download/notes-v0.1.0/notes.krate --native-window --prompt
```

Nothing lands on your disk first. The bundle is fetched, you are shown what it
wants, and it runs with only what you allow. Downloading grants nothing on its
own.

Or build it yourself from the repository:

```bash
sh scripts/build-krate-notes-component.sh
mkdir -p ~/notes-demo/notes && cd ~/notes-demo
cp <repo>/apps/krate-notes/target/wasm32-wasip1/release/krate_notes.wasm code.wasm
cp <repo>/apps/krate-notes/manifest.toml manifest.toml
<repo>/target/debug/krate pack code.wasm --manifest manifest.toml -o notes.krate
```

You now have `notes.krate`, one file containing the app and the permissions it
is asking for.

## 2. See what it wants, before running it

```bash
krate run notes.krate --dump-caps
```

Nothing executes. You get the list of capabilities the app declared, so you can
decide whether to run it at all.

## 3. Run it

```bash
krate run notes.krate --native-window --prompt
```

`--prompt` asks you about each capability first:

```text
App: Krate Notes (dev.krate.notes)
Requests the following capabilities:
  [1] fs.read:notes/**
      Load your saved notes
  [2] fs.write:notes/**
      Save the note you are editing
Grant [A]ll / [N]one / numbers (for example 1,2):
```

Type `A` and a window opens: your notes on the left, an editor on the right.
Click a note to switch to it. Your work is saved when you switch notes and
again when you close the window.

Then look at what it wrote:

```bash
cat ~/notes-demo/notes/first.txt
```

## 4. Watch the wall hold

This is the part worth trying. Allow everything except writing:

```bash
krate run notes.krate \
  --grant "fs.read:./notes/**" \
  --grant ui.window:create \
  --grant io.stdout \
  --grant io.args
```

The app refuses to start and tells you exactly which permission is missing. It
cannot save, cannot read anything outside `./notes/`, and cannot reach the
network at all, because none of that was granted.

## Running it from a link

A `.krate` is a normal file, so any web host serves it:

```bash
krate run https://example.com/notes.krate --native-window --prompt
```

Nothing is on your disk beforehand. The app is fetched, you are shown what it
wants, and it runs with only what you allow. Downloading grants nothing on its
own.

Plain `http://` is refused unless you pass `--insecure-http`, which exists for
local test servers.

## What works where, honestly

| | Linux | Windows | macOS |
|---|---|---|---|
| Opens a real window | yes | yes | yes |
| Note list and selection | yes | yes | yes |
| Typing into the editor | yes | yes | see below |
| Saving to a granted folder | yes | yes | yes |
| Permission wall | yes | yes | yes |

Widgets are drawn rather than native on Linux and Windows; on macOS they are
real AppKit controls. Keyboard input on macOS is newer than the other two
hosts, so if typing does not behave, that is the reason, and Linux is the most
complete experience today.

## If something goes wrong

**`permission denied: missing required capabilities`** — the app asked for
something you did not grant. The message names the exact flag to add.

**`refusing to fetch over plain HTTP`** — use an `https` URL, or add
`--insecure-http` if you are serving it locally on purpose.

**The window opens and closes immediately** — you probably passed `quick`,
which is the flag automated tests use to exit straight away. Leave it off.
