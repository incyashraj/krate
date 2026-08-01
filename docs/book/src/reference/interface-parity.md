# Interface parity

Generated from the runtime host, not written by hand. Run
`cargo run -p krate-tools --bin check-interface-parity -- --write` to refresh it.

**8 of 14 declared interfaces are fully implemented. 4 are declared and do nothing yet.**

An interface that is declared but not implemented refuses every call with
`Unsupported`. That is the honest failure -- nothing pretends to work -- but a
person only finds out after building on it, which is why this table exists.

Read it alongside the widget table. They answer different questions: `canvas`
lays out on all three systems, so the widget table says it works, and
`gfx.canvas2d` refuses every call, so nothing can draw into it.

| Interface | Functions | State |
| --- | --- | --- |
| `audio.capture` | 4 | **works** |
| `audio.playback` | 4 | **not implemented** |
| `gfx.canvas2d` | 3 | **not implemented** |
| `gfx.gpu3d` | 2 | **not implemented** |
| `speech.transcription` | 20 | **works** |
| `ui.clipboard` | 2 | **works** |
| `ui.dialog` | 3 | **works** |
| `ui.events` | 2 | **works** |
| `ui.image` | 2 | **works** |
| `ui.launcher` | 1 | **works** |
| `ui.menu` | 1 | **not implemented** |
| `ui.notify` | 1 | **works** |
| `ui.tree` | 5 | partly — 1 of 5 refuse |
| `ui.window` | 7 | partly — 1 of 7 refuse |

