# Krate TypeScript SDK

This is the first TypeScript shape for the Phase 2 UAPI. It is intentionally
small: the package gives app authors stable import names, TypeScript types, and
light helpers for the same `io`, `fs`, `net`, `time`, and `locale` modules used
by the Rust samples.

The runtime proof is still pending until `jco` is wired into the sample build.
For now, treat this package as an SDK contract draft.

```ts
import { io, net } from "@krate/sdk";

const url = io.args()[0] ?? "http://127.0.0.1/";
const body = net.getText(url);
io.print(body);
```

Expected toolchain:

```bash
npm install -D @bytecodealliance/jco typescript
npx jco componentize ./src/main.ts --wit ../../wit/krate/phase2 --world-name cli --out app.wasm
```

The exact `jco componentize` command may change while Phase 2 bindings settle.

Current sample sources:

- `examples/krate-clock.ts`
- `examples/krate-cat.ts`
- `examples/krate-curl.ts`
