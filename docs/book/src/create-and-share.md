# Create and share an app

This guide takes you from a plain-English request to a `.krate` file you can
send to someone, who opens it by double-clicking and reviews exactly what it
can access before it runs. You do not need to know the Krate internals, and you
do not need to have written the app yourself — an AI agent can write it for you.

The example builds a checklist app that saves its items to a local file.

> Want the plain-English, no-jargon version first? See
> [Make an app by asking](pages/make-an-app-with-ai.html) — the same flow
> written for anyone, with big step-by-step pictures of what happens.

## What you need

- **Rust**, via [`rustup`](https://rustup.rs). Krate pins its toolchain, so the
  right compiler and the `wasm32-wasip1` target install on demand.
- **`cargo-component`**, the tool that builds a Rust app into a Krate component:

  ```bash
  cargo install cargo-component --locked --version 0.21.1
  ```

- **The `krate` CLI.** Build it once and put it on your `PATH`:

  ```bash
  git clone https://github.com/incyashraj/krate
  cd krate
  cargo build --release -p krate-cli
  # the binary is target/release/krate — copy it anywhere on your PATH
  ```

  The binary carries the Krate SDK inside it, so once you have `krate` you do
  not need the checkout anymore. `krate create` writes the SDK it needs to a
  cache directory the first time it runs.

> Krate is pre-alpha. This flow is for building and sharing your own apps, not
> for running untrusted third-party code yet.

## Create the app

One command authors the app, builds it, checks it, packages it, and proves its
permission wall before writing the file:

```bash
krate create "Make a checklist app that saves locally" \
  --output checklist.krate
```

You will see it work through the steps, and finish with:

```text
Created checklist.krate
  transcript: checklist.krate.transcript.json
  requested access:
    - ui.window:create
    - io.stdout
    - io.args
    - fs.read:./checklist/**
    - fs.write:./checklist/**

Send checklist.krate to someone; they can double-click it to open it.
```

Two files are written:

- **`checklist.krate`** — the whole app in one file. This is what you share.
- **`checklist.krate.transcript.json`** — a record of the request, the app, the
  permissions it asks for, and the verification that it runs with those grants
  and refuses without the one that gates it.

### Letting an AI agent write the app

The command above uses a built-in template. To have a coding agent write the
app instead, pass `--author-cmd`. Krate hands your command three environment
variables and expects it to write `Cargo.toml`, `src/lib.rs`, and
`manifest.toml` into the app directory:

| Variable         | Meaning                                             |
| ---------------- | --------------------------------------------------- |
| `KRATE_APP_DIR`  | Directory to write the generated crate into         |
| `KRATE_APP_NAME` | The app's kebab-case name                           |
| `KRATE_REQUEST`  | The plain-English request you passed                |

```bash
krate create "Make a checklist app that saves locally" \
  --author-cmd "<your agent command>" \
  --output checklist.krate
```

Everything after the agent writes the source — build, import check, packaging,
and the allow/deny verification — is identical. If the agent produces an app
that reaches for anything outside Krate's capabilities, `krate create` stops
before packaging it and tells you what it tried to import.

## Share it

`checklist.krate` is a single file. Send it however you send any file — a
message, an email attachment, a shared drive. The person receiving it does not
need to trust you or read any code.

## Open it (on the other side)

The recipient needs Krate installed the same way.

**Turn on double-click (one time).** On macOS this is set up automatically by
`Krate.app`. On Linux and Windows, run the small registration script once so the
file manager knows to open `.krate` files with Krate:

- **Linux:** `scripts/install-krate-desktop.sh`
- **Windows:** `powershell -ExecutionPolicy Bypass -File scripts\install-krate-desktop.ps1`

Both need no administrator rights, register only for the current user, and can
be undone with `--uninstall` (`-Uninstall` on Windows). After that, a
double-clicked `.krate` opens through the same permission review as
`krate run <file> --consent`.

Then, step by step:

1. **Double-click `checklist.krate`.** Before anything runs, Krate shows a
   window listing exactly what the app is asking for — a window, and read/write
   access to its own `checklist` folder. Nothing else on the machine is
   reachable.
2. **Review and allow.** The requested access is right there to read. Allow it,
   and the checklist window opens.
3. **Use it.** Type a new item into the field and click **Add item** (or press
   Enter) — it appears in the list. Click a checkbox to mark an item done.
4. **Close the window.** Every change was saved to the app's folder as you made
   it.
5. **Reopen it** (double-click the file again, allow again). Your items are
   still there, exactly as you left them — checked ones checked, added ones
   present.
6. **Try denying.** Open it once more and decline the folder-write access. The
   app does not start at all — it never opens half-working, because the wall
   comes before the app's code runs.

From a terminal the same file runs with:

```bash
krate run checklist.krate
```

Add `--consent` to get the permission review even without a double-click, or
`--auto-grant` to grant everything for a quick local try. The whole round trip —
create, open, edit, persist across a reopen, and refuse without its write
grant — is also checked headlessly by `scripts/checklist-roundtrip.sh`.

## What just happened

- You described an app in one sentence.
- It was written, built, checked, and packaged into one `.krate` — and its
  permission wall was verified before the file existed.
- You sent that one file to someone.
- They opened it, saw exactly what it could touch, allowed it, and used it.

The same file behaves identically on macOS, Linux, and Windows.
