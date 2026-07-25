# krate-author — the AI authoring loop

This crate and `scripts/author-krate.sh` prove the step Krate was missing: not
just *running* and *protecting* apps, but showing that an agent can **create**
the shareable artifact directly. A request goes in; a working, permission-gated
`.krate` comes out, and the whole run is recorded as evidence.

## The loop

```
request ─▶ author ─▶ build ─▶ pack ─▶ verify (allow, then deny)
```

1. **author** — generate a complete Krate guest crate (`Cargo.toml`,
   `src/lib.rs`, `manifest.toml`) from the request. Today's one app kind is a
   word-frequency reporter that reads a file and prints its most common words.
2. **build** — `cargo-component` compiles it to a wasm component. The component
   must import only `krate:*`; a `wasi:*` import fails the loop.
3. **pack** — `krate pack` bundles code + manifest into one `.krate`.
4. **verify** — run it **with** the `fs.read` grant (works, exit 0) and
   **without** it (refuses before running, exit 5). That wall is the point.

Run it:

```sh
scripts/author-krate.sh
```

It writes, under `evidence/authoring/`:

- `transcript.json` — schema `krate.author.v1`: the request, every step's
  command and exit, the `code.wasm` sha256, and a verdict.
- `report.csv` — the app's output on a fixed input, so the evidence is stable.
- `<name>.krate` — the packaged artifact (gitignored; CI uploads it).

## Where the AI plugs in

The **author** step is the seam. By default it runs the deterministic
`krate-author` generator — that is what CI gates on, so the loop is always
reproducibly green. To have a real model write the code instead:

```sh
scripts/author-krate.sh --author-cmd '<command that writes the app>'
```

The command is handed `KRATE_APP_DIR`, `KRATE_APP_NAME`, and `KRATE_REQUEST` in
the environment and is responsible for writing `Cargo.toml`, `src/lib.rs`, and
`manifest.toml` into `KRATE_APP_DIR`. Everything downstream — build, pack,
verify — is unchanged. This is how Claude Code or an API-driven agent produces a
genuine transcript. The LLM path is a demo hook and never gates CI.

## The one hard constraint the generator encodes

A Krate component may import only `krate:*`. Ordinary std code breaks this: a
growable `Vec`'s reallocation, `HashMap`'s hasher, `format!`, and the
`args::first`/`fs::read_to_string` SDK helpers all pull the `wasi:*` import set
in, and link-time optimization cannot strip it. So the generated app follows the
same fixed-capacity, panic-free discipline as the in-tree samples: fixed `[u8;
N]` buffers, a fixed word table, `.get()`/`.get_mut()` only, and output formatted
by hand. This is the non-obvious knowledge an agent authoring for Krate needs,
and encoding it in the generator is most of the value here.
