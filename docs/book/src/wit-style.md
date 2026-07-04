# WIT Style Guide

Krate UAPI is written in WIT because WIT is the contract between an app and
every host we support. Once a module is frozen, app authors build against it and
host adapters must keep honoring it. This guide keeps that contract boring,
small, and portable.

Phase 2 uses this guide for `io`, `fs`, `net`, `time`, and `locale`.

## The Rule

Every WIT change should pass this sentence:

> A Rust, Go, or TypeScript app can call this API, and Linux, macOS, and Windows
> can implement it without guessing what the words mean.

If a type only makes sense on one host, it is not ready for UAPI. Put it behind
an adapter detail, defer it, or write an ADR.

## Package Names

Use one package per UAPI module:

```wit
package krate:fs@0.1.0;
package krate:net@0.1.0;
```

Rules:

- Use `krate:<module>@<semver>`.
- Use short nouns for modules: `fs`, `net`, `time`, `locale`.
- Do not put host names in package names.
- Do not use marketing names in WIT. WIT is an ABI, not a product page.

The app world imports versioned module interfaces:

```wit
package krate:app@0.1.0;

world cli {
  import krate:io/stdio@0.1.0;
  import krate:fs/files@0.1.0;

  export run: func() -> s32;
}
```

## Interface Names

Use interface names for one clear job:

```wit
interface files
interface http-client
interface clock
interface format
```

Rules:

- Use kebab-case.
- Use nouns for groups of stateful operations: `files`, `streams`.
- Use a noun phrase for a service: `http-client`.
- Avoid buckets like `utils`, `common`, `helpers`, or `misc`.

If an interface needs three unrelated paragraphs to explain it, split it.

## Function Names

Use small verbs or verb phrases:

```wit
open: func(path: string, mode: open-mode) -> result<file, fs-error>;
format-date: func(millis: u64, timezone: string, style: date-style, locale: locale-id) -> string;
```

Rules:

- Use kebab-case.
- Prefer verbs: `open`, `read`, `write`, `flush`, `rename`.
- Include the object only when it removes ambiguity: `remove-file`,
  `remove-dir`, `format-date`.
- Do not expose host syscall names.
- Do not encode permission checks in names. The capability system handles that.

Bad:

```wit
win32-create-file-w: func(path: string) -> result<u32, fs-error>;
```

Good:

```wit
open: func(path: string, mode: open-mode) -> result<file, fs-error>;
```

## Records, Enums, And Variants

Use records for data with named fields:

```wit
record file-stat {
  size: u64,
  modified-millis: u64,
  is-dir: bool,
}
```

Use enums for closed sets with no attached data:

```wit
enum http-method {
  get,
  post,
  put,
}
```

Use variants when at least one case needs data:

```wit
variant net-error {
  invalid-url,
  dns-failure(string),
  permission-denied,
  other(string),
}
```

Rules:

- Use kebab-case for type names, fields, and cases.
- Prefer explicit units in field names: `modified-millis`,
  `timeout-millis`.
- Keep records flat unless nesting gives a real domain boundary.
- Do not add an enum case unless every host can produce or consume it.
- Use `other(string)` only as a temporary escape hatch. If it becomes common,
  promote the case to a real variant.

## Resources Vs Functions

Use a resource when the host owns state across calls:

```wit
resource file {
  read: func(n: u32) -> result<list<u8>, fs-error>;
  write: func(bytes: list<u8>) -> result<u32, fs-error>;
  stat: func() -> result<file-stat, fs-error>;
}
```

Use a plain function when the call has no lasting host-owned handle:

```wit
stat: func(path: string) -> result<file-stat, fs-error>;
get: func(url: string) -> result<list<u8>, net-error>;
```

Choose a resource when:

- The app needs repeated operations on the same host object.
- The host must control lifetime.
- The value cannot be copied safely into app memory.
- Future calls need a cursor, buffer, stream, socket, window, or device session.

Choose a function when:

- The call is request-response.
- All data can be copied in and out.
- There is no cleanup beyond returning the result.

## Error Style

Most UAPI calls should return `result<T, module-error>`.

```wit
read: func(n: u32) -> result<list<u8>, io-error>;
open: func(path: string, mode: open-mode) -> result<file, fs-error>;
```

Use typed errors so apps can handle normal failures:

```wit
variant fs-error {
  not-found,
  permission-denied,
  invalid-path,
  io(string),
}
```

Rules:

- Every capability-protected module must have `permission-denied`.
- Use module-specific errors: `fs-error`, `net-error`, `io-error`.
- Use variants for expected failures.
- Trap only for runtime bugs, invalid ABI state, or exhausted runtime resources.
- Do not return raw OS error numbers. Translate them at the adapter boundary.

For example, a missing file is not a trap. It is `fs-error.not-found`.

## Capability Names

Capabilities are not WIT syntax, but every protected WIT call needs a matching
capability shape.

Current Phase 2 examples:

| WIT call | Capability |
|---|---|
| `fs.files.open(path, read)` | `fs.read:<path-or-glob>` |
| `fs.files.open(path, write)` | `fs.write:<path-or-glob>` |
| `net.http-client.get(url)` | `net.connect:<host>:<port>` |
| `time.clock.now-millis()` | `time.now` |
| `io.stdio.stdout()` | `io.stdout` |

Rules:

- The capability name should describe the permission, not the function name.
- Resource scopes should be human-readable: paths, host:port pairs, device IDs.
- Default grants must stay low risk: stdout, stderr, args, locale, and clock are
  acceptable in Phase 2; filesystem and network are not.
- If a WIT call reaches the host OS, decide its capability before merging it.

## Comments

Use WIT comments for anything that will help an app author:

```wit
/// Return the current wall-clock time in milliseconds since Unix epoch.
now-millis: func() -> u64;
```

Rules:

- Write comments for public interfaces, records, resources, and non-obvious
  functions.
- Say what the API means, not how the current Rust adapter happens to work.
- Mention units, ordering, encoding, and limits.
- Keep comments stable. The generated UAPI reference publishes them.

## Versioning

Phase 2 is still draft, but the versioning rule is already strict:

- Additive changes can stay in the same minor draft while the module is not
  frozen.
- Removing a function, changing a field type, or renaming a case is breaking.
- After v0.1.0 is frozen, breaking changes need a new module version.
- A frozen app world should import exact module versions.

Before freezing a module, run through every sample app and ask whether the names
would still make sense in Phase 3 GUI, Phase 4 mobile, and Phase 6 packaged
apps. If not, fix the name before the freeze.

## The Native Three Test

Before adding a UAPI type, write down how it maps to Linux, macOS, and Windows.

For Phase 4 mobile-facing types, use the native three of five test: the type
must map naturally to at least three of Windows, Linux, macOS, Android, and iOS.
If it only maps cleanly to one platform, it is probably an adapter detail or a
higher-level convenience API.

This test is not about lowest common denominator design. It is about avoiding
fake portability.

## Review Checklist

Before a WIT PR merges:

- Package and interface names are kebab-case and versioned.
- Every function has a clear host behavior.
- Every protected call has a capability shape.
- Errors are typed and include `permission-denied` where needed.
- Resources are used only for host-owned state.
- Units are named in fields and docs.
- The generated UAPI reference is updated.
- The relevant sample app still reads naturally through the Rust SDK.
- Any hard-to-reverse choice has an ADR.

Small WIT is good WIT. Add the narrow thing we can support everywhere, then grow
it after real apps prove the missing shape.
