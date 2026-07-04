# Vision

Krate exists because app portability is still broken.

A text file can move from one device to another. A photo can move. A web page
can move. A serious native app usually cannot. It has to be rebuilt for each
platform, then tested, packaged, signed, distributed, and maintained again.

Krate is a plan to put one runtime layer between apps and operating systems.

## The 6 x 6 Problem

The dream is a full matrix:

| App origin | Runs on |
|------------|---------|
| Windows app | Windows, Linux, ChromeOS, Android, macOS, iOS |
| Linux app | Windows, Linux, ChromeOS, Android, macOS, iOS |
| Web app | Windows, Linux, ChromeOS, Android, macOS, iOS |
| Android app | Windows, Linux, ChromeOS, Android, macOS, iOS |
| macOS app | Windows, Linux, ChromeOS, Android, macOS, iOS |
| iOS app | Windows, Linux, ChromeOS, Android, macOS, iOS |

Krate does not magically run every existing native app today. The path is more
practical: define a new portable app target that can become good enough for new
apps, then build bridges and tooling over time.

## The Bet

The same pattern has worked before:

| Old problem | Middle layer | What changed |
|-------------|--------------|--------------|
| Many CPUs | LLVM IR | One compiler front end can reach many chips. |
| Many servers | JVM and .NET bytecode | One backend app can run on many OSes. |
| Many browsers | HTML, CSS, and JS | One site can reach almost every device. |

Krate tries to bring that pattern to native apps:

1. WebAssembly is the portable program format.
2. UAPI is the standard app API.
3. UCap is the permission model.
4. Host adapters translate Krate calls into native OS calls.
5. A bundle format and marketplace make apps installable.

## Why This Might Work Now

- WebAssembly is stable and widely understood.
- The Component Model makes host APIs cleaner than raw WASM imports.
- Rust, Go, TypeScript, C, and other languages can target WASM.
- Desktop and mobile hardware are closer than they used to be.
- Developers are tired of maintaining the same product in many stacks.

## What Success Looks Like

At v1.0, a developer should be able to build one Krate app and ship it to the
main desktop and mobile platforms with platform specific adapters doing the
native work.

The hard requirements are:

| Area | Target |
|------|--------|
| Hosts | Windows, macOS, Linux, iOS, Android, and web where possible |
| App format | `.l36app` bundle |
| Runtime | Fast cold start and predictable memory use |
| APIs | Files, network, time, locale, UI, graphics, sensors, identity |
| Permissions | Clear grants instead of silent host access |
| Developer flow | New project to running app in about a minute |
| Real proof | ParkSure or another real product running on Krate |
