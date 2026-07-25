# Create and share an app

This guide takes you from a plain-English request to a `.krate` file you can
send to someone, who opens it by double-clicking and reviews exactly what it
can access before it runs. You do not need to know the Krate internals, and you
do not need to have written the app yourself — an AI agent can write it for you.

The example builds a checklist app that saves its items to a local file.

## What you need

- **Rust**, via [`rustup`](https://rustup.rs). Krate pins its toolchain, so the
  right compiler and the `wasm32-wasip1` target install on demand.
- **`cargo-component`**, the tool that builds a Rust app into a Krate component:

  ```bash
  cargo install cargo-component --locked --version 0.21.1
  ```

- **The `krate` CLI.** Build it from a Krate checkout and put it on your `PATH`:

  ```bash
  git clone https://github.com/incyashraj/krate
  cd krate
  cargo build --release -p krate-cli
  # the binary is target/release/krate
  ```

- **`KRATE_SDK_ROOT`** pointing at that checkout, so the generated app can build
  against the Krate SDK and interface definitions:

  ```bash
  export KRATE_SDK_ROOT=/path/to/krate
  ```

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

The recipient needs Krate installed the same way. Then:

- **Double-click `checklist.krate`.** Before anything runs, Krate shows what the
  app is asking for — here, a window and read/write access to its own
  `checklist` folder — and nothing else on the machine is reachable.
- **Allow it**, and the checklist opens as a normal app: check items off, add
  new ones, close and reopen it with everything saved.
- **Deny a permission it needs**, and it simply does not start. It never opens
  half-working; the wall comes before the app's code runs.

From a terminal the same file runs with:

```bash
krate run checklist.krate
```

Add `--consent` to get the permission review even without a double-click, or
`--auto-grant` to grant everything for a quick local try.

## What just happened

- You described an app in one sentence.
- It was written, built, checked, and packaged into one `.krate` — and its
  permission wall was verified before the file existed.
- You sent that one file to someone.
- They opened it, saw exactly what it could touch, allowed it, and used it.

The same file behaves identically on macOS, Linux, and Windows.
