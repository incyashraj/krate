# Widget parity

Generated from the code that renders, not written by hand. Run
`cargo run -p krate-tools --bin check-widget-parity -- --write` to refresh it.

**8 of 17 declared widgets work on all three systems.**

A widget that renders on one system and not another is the failure this
table exists to make visible: an app built on the machine that supports it
will not work when it is shared with someone on another.

| Widget | macOS | Windows | Linux | Everywhere |
| --- | --- | --- | --- | --- |
| `button` | yes | yes | yes | **yes** |
| `canvas` | no | yes | yes | no |
| `checkbox` | yes | yes | yes | **yes** |
| `grid` | no | no | no | no |
| `image` | no | no | no | no |
| `list-view` | yes | yes | yes | **yes** |
| `progress` | no | yes | yes | no |
| `radio` | no | yes | yes | no |
| `scroll` | yes | no | no | no |
| `slider` | yes | yes | yes | **yes** |
| `stack` | yes | yes | yes | **yes** |
| `switch` | no | yes | yes | no |
| `tabs` | no | no | no | no |
| `text` | yes | yes | yes | **yes** |
| `text-area` | yes | yes | yes | **yes** |
| `text-field` | yes | yes | yes | **yes** |
| `tree-view` | no | yes | yes | no |

## Gaps

- macOS only: `scroll`
- Windows and Linux only: `canvas`, `progress`, `radio`, `switch`, `tree-view`
- Not implemented anywhere: `grid`, `image`, `tabs`
