# The build service

What happens when someone types a sentence on krate.tech and presses send.

It runs `krate create` -- the same command the CLI and Studio run, not a
reimplementation -- so an app made in a browser and an app made on a
desktop cannot come out different.

## Run it

```bash
KRATE_BIN=/path/to/krate KRATE_AGENT=claude node src/server.js
```

| Variable | What it is |
|---|---|
| `KRATE_BIN` | The engine. Use an absolute path: a bare `krate` may resolve to an older installed release |
| `KRATE_AGENT` | Which AI writes the app (`claude`, `codex`, ...) |
| `KRATE_HUB` | The hub the wall is checked against |
| `KRATE_ORIGIN` | The site allowed to call this, for CORS |
| `KRATE_BUILD_TIMEOUT_MS` | How long one build may take. Default 15 minutes |
| `KRATE_BUILDER_DEV` | `1` skips the wall, for developing the service. **Never set in production** |

## The doors

```
POST /build            { request }   -> { id }
GET  /build/<id>                     -> { state, stage, line, shot, error, result }
GET  /build/<id>/file                -> the .krate itself
POST /build/<id>/stop                -> { ok }
GET  /health                         -> { ok, building }
```

## The two guards that matter

Every build is real inference we pay for, so:

- **The wall is asked of the hub, never of the browser.** A counter the
  page owns is a counter anyone can edit.
- **One build at a time per account.** Not politeness -- the difference
  between a bill and a bankruptcy.

Only a build that produced a file counts against the free three. A
failure the person did not cause never costs them one.
