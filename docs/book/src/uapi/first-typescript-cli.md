# Your First UAPI App In TypeScript

This walkthrough gets you started with the TypeScript Layer36 SDK in the
current Phase 2 state.

Right now, TypeScript is in scaffold mode: SDK shape is live, and hosted full
CI now runs the TypeScript runtime fixture lane by default.

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

## 4. Build Runtime Variant Fixtures

From repo root:

```bash
scripts/build-phase2-language-variant-fixtures.sh
```

This script now tries to build the TypeScript variant fixtures automatically
from `test/integration/language-variants-src/` when `jco` is available.
Outputs go to:

```text
test/integration/language-variants/
```

If `jco` is missing, it exits cleanly in default mode and tells you what is
missing. For hosted full CI, Layer36 now allows `npx`-driven jco installation
for this step, so the TypeScript lane can stay active by default.

## 5. Optional Runtime Variant Test Hook

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
When all three are present, the script also runs the component import-purity
check before runtime assertions.
You can force stricter CI behavior with `LAYER36_LANGUAGE_VARIANTS_MODE`.
Useful values are `optional` (default), `go`, `ts`, `any`, and `both`.

## 6. Where This Fits In Phase 2

TypeScript is now at "SDK, harness, and first fixture-build path ready" stage.

Still pending:

- cross-host stability evidence for restricted-runner curl fixture behavior

So this tutorial is intentionally honest: strong SDK structure today, full build
lane in progress, and no blocker for continuing core runtime work.
