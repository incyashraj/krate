# TypeScript SDK

The TypeScript SDK is now started at `packages/sdk-ts`. It is not the final
binding proof yet. Think of it as the first clean shape for TypeScript app code:
stable import names, clear types, and small helpers over the Phase 2 UAPI.

## Current Status

What exists now:

- `@layer36/sdk` package metadata.
- Type declarations for the Layer36 WIT import modules.
- Helpers for arguments, stdout, stderr, file reads and writes, HTTP GET, time,
  and locale calls.
- Example source files for TypeScript clock, cat, and curl-style CLI apps.
- A dependency-free shape check that guards the package layout and import names.
- Runtime fixture auto-build support through
  `scripts/build-phase2-language-variant-fixtures.sh`.
- Local runtime fixture proof for TypeScript clock and cat.

What still needs proof:

- Keep full runtime fixture proof stable in hosted CI.
- Complete and stabilize the TinyGo fixture lane so language-variant checks can
  move from optional to strict by default.
- Keep curl fixture evidence stable on restricted runners where local socket
  bind policy may differ.

## Example

```typescript
import { io, net } from "@layer36/sdk";

const url = io.args()[0];

if (!url) {
  io.eprintln("usage: layer36-ts-curl <url>");
  throw new Error("missing url");
}

io.print(net.getText(url));
```

The longer examples live here:

- `packages/sdk-ts/examples/layer36-clock.ts`
- `packages/sdk-ts/examples/layer36-cat.ts`
- `packages/sdk-ts/examples/layer36-curl.ts`

This code is meant to compile into a WebAssembly component with `jco`, then run
inside Layer36. It should not call Node filesystem or network APIs directly.
All real access must go through Layer36 UAPI imports so the manifest and UCap
checks stay in charge.

## Tooling

Run:

```bash
layer36 doctor
```

For this track, these lines should be present:

```text
node            v...
npm             ...
jco             ... (or "... (via npx)")
```

If `jco` is missing, install it as a local project dependency:

```bash
npm install -D @bytecodealliance/jco typescript
```

The CLI doctor command now reports `jco` from either the direct binary path or
`npx --no-install jco`, so local Node-based installs are visible immediately.

## Binding Shape Note

WIT `variant` values are represented as tagged objects in this binding path.
For example, filesystem open mode is passed as `{ tag: "read" }` instead of the
plain string `"read"`. The SDK helper exports the correct values so app code
can stay simple.

## Current Check

The normal CI path runs a small package shape check:

```bash
npm --prefix packages/sdk-ts run check:shape
```

This does not compile a component. It catches simple mistakes such as missing
helper files, wrong package metadata, accidental `wasi:*` imports, or missing
Layer36 import declarations.

For runtime fixture builds, use:

```bash
scripts/build-phase2-language-variant-fixtures.sh
```

When `jco` is available, that script now builds the TypeScript fixture trio in
`test/integration/language-variants/` and lets the existing runtime-variant
tests run without manually setting fixture env vars.
