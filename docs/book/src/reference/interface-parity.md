# Interface parity

Generated from the runtime host, not written by hand. Run
`cargo run -p krate-tools --bin check-interface-parity -- --write` to refresh it.

**11 of 14 declared interfaces are fully implemented. 1 are declared and do nothing yet.**

An interface that is declared but not implemented refuses every call with
`Unsupported`. That is the honest failure -- nothing pretends to work -- but a
person only finds out after building on it, which is why this table exists.

Read it alongside the widget table. They answer different questions: the
widget table says a kind lays out and draws, this one says whether the calls
behind an interface do anything. For a while `canvas` laid out everywhere
while `gfx.canvas2d` refused every call -- a widget that existed and could
not be drawn into. That pair reads `works` on both tables now.

| Interface | Functions | State |
| --- | --- | --- |
| `audio.capture` | 4 | **works** |
| `audio.playback` | 7 | **works** |
| `gfx.canvas2d` | 7 | **works** |
| `gfx.scene3d` | 10 | **works** |
| `speech.transcription` | 25 | **works** |
| `ui.clipboard` | 2 | **works** |
| `ui.dialog` | 3 | **works** |
| `ui.events` | 3 | **works** |
| `ui.image` | 2 | **works** |
| `ui.launcher` | 1 | **works** |
| `ui.menu` | 1 | **not implemented** |
| `ui.notify` | 1 | **works** |
| `ui.tree` | 5 | partly — 1 of 5 refuse |
| `ui.window` | 7 | partly — 1 of 7 refuse |

