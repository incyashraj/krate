# Your First UAPI App In TypeScript

This walkthrough gets you started with the TypeScript Layer36 SDK in the
current Phase 2 state.

Right now, TypeScript is in scaffold mode: SDK shape is live, while full
component build and always-on runtime fixture proof are still being finished.

## What You Build Today

- a small TypeScript app shape using `@layer36/sdk`
- a validated package structure
- a practical map of what works now vs what is still pending

## 1. Check Tooling

From repo root:

```bash
cargo run -p layer36-cli -- doctor
```

Look for:

- `node`
- `npm`
- `jco`

If `jco` is missing, SDK shape work can continue, but component build proof is
not ready on that machine yet.

## 2. Start From The TypeScript Cat Example

Use:

```text
packages/sdk-ts/examples/layer36-cat.ts
```

The sample path is intentionally simple:

- read args through Layer36 SDK
- read files through Layer36 SDK
- print through Layer36 SDK

No direct Node filesystem or socket APIs are used in the app path.

## 3. Run The Shape Check

From repo root:

```bash
npm --prefix packages/sdk-ts run check:shape
```

This confirms package metadata, helper exports, and import declarations still
match the current UAPI-facing SDK contract.

## 4. Optional Runtime Variant Test Hook

If TypeScript variant WASM fixtures exist under:

```text
test/integration/language-variants/
```

run:

```bash
scripts/test-phase2-language-variants.sh
```

If fixtures are not present, the script exits with a skip message and no error.
If you provide TypeScript fixture env vars, provide all three (`clock`, `cat`,
and `curl`) so the runtime lane runs as one complete set.

## 5. Where This Fits In Phase 2

TypeScript is now at "SDK and harness ready" stage.

Still pending:

- stable componentize build path in CI
- routine fixture generation for TypeScript variants
- always-on runtime fixture gate for TypeScript variants

So this tutorial is intentionally honest: strong SDK structure today, full build
lane in progress, and no blocker for continuing core runtime work.
