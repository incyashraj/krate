# UAPI Overview

UAPI means Universal API.

It is the standard app API every Layer36 app will call. Instead of calling
Windows files, macOS files, Linux files, Android files, or iOS files directly,
an app calls the Layer36 file API. The host adapter then does the native work.

Phase 2 starts this layer.

## Planned Modules

```text
layer36:
  io/              stdio, pipes, stdout, stderr
  fs/              files, paths, metadata
  net/             HTTP first, more network APIs later
  time/            clocks and timers
  locale/          language, region, formatting
  ui/              windows, widgets, layout, input
  gfx/             2D drawing and GPU work
  audio/           playback and capture
  sensors/         motion, location, camera, mic
  storage/         key-value, SQL, object storage
  crypto/          hashes, signing, encryption, random
  identity/        user identity and signing
  notify/          system notifications
  accessibility/   screen readers and reduced-motion settings
  platform/        device info and host capabilities
```

## Phase 2 Scope

Phase 2 only covers:

- `io`
- `fs`
- `net`
- `time`
- `locale`

That is enough to build the first useful CLI apps without pretending the whole
platform is ready.

The first Phase 2 draft is checked into `wit/layer36/phase2`. It is a review
draft, not a frozen compatibility promise yet.
