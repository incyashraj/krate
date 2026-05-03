# Phase 2: UAPI v0.1

**Status:** Started
**Estimate:** est. 4 to 8 weeks
**Goal:** Make Layer36 useful for small command line apps.

Phase 2 replaces the temporary Phase 1 host interface with real APIs:

- `io`
- `fs`
- `net`
- `time`
- `locale`

The first draft of those WIT contracts now lives at `wit/layer36/phase2`.
It is not frozen yet, but it is real source code and CI parses it so syntax
mistakes are caught early.

The capability layer has also started. Layer36 can parse a sidecar
`manifest.toml`, check launch-time grants, and carry the session policy into the
runtime. The newest piece is a runtime UAPI guard: it translates calls like
`fs.read ./data/file.txt` or `net.connect api.example.com:443` into the exact
capability check that must pass before an adapter talks to the host OS.

There is also a first dispatcher scaffold now. In simple terms: we have the
place where generated UAPI calls will enter the runtime, get checked by policy,
and then move to the host adapter. The current tests prove denied file and
network calls stop before any adapter code runs.

The Phase 2 WIT also has a Rust host-binding checkpoint now. That means CI asks
Wasmtime to generate bindings for the new `cli` world and checks a few important
names. So far the shape is usable: `run` returns an `i32`, `OpenMode::Read`
exists, and `HttpMethod::Get` exists.

The latest runtime piece is the generated type bridge. It maps WIT records and
errors into the dispatcher's Rust types, then back again. Put simply: the
runtime now understands the words that generated WIT code will use when it asks
for files, network, time, locale, and logs.

The proof apps are:

- `layer36-curl`
- `layer36-cat`
- `layer36-clock`

If Phase 2 works, those apps should produce the same output on Linux, macOS, and
Windows while running through the same Layer36 runtime model.

See [`Plan/Phase-2-Plan.md`](https://github.com/incyashraj/layer6x6/blob/main/Plan/Phase-2-Plan.md).
