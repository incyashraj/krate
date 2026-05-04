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

What still needs proof:

- Install `jco`.
- Build a real TypeScript component.
- Run that component through `layer36 run`.
- Add a fixture-backed test like the Rust samples.

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
jco             ...
```

If `jco` is missing, install it as a local project dependency when we wire the
sample build:

```bash
npm install -D @bytecodealliance/jco typescript
```

We are keeping that install out of the normal CI path for now so push checks stay
cheap.

## Current Check

The normal CI path runs a small package shape check:

```bash
npm --prefix packages/sdk-ts run check:shape
```

This does not compile a component. It catches simple mistakes such as missing
helper files, wrong package metadata, accidental `wasi:*` imports, or missing
Layer36 import declarations.
