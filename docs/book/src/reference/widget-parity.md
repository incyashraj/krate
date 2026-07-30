# Widget parity

Generated from the code that renders, not written by hand. Run
`cargo run -p krate-tools --bin check-widget-parity -- --write` to refresh it.

**13 of 17 declared widgets work on all three systems.**

A widget that renders on one system and not another is the failure this
table exists to make visible: an app built on the machine that supports it
will not work when it is shared with someone on another.

| Widget | macOS | Windows | Linux | Everywhere |
| --- | --- | --- | --- | --- |
| `button` | yes | yes | yes | **yes** |
| `canvas` | yes | no | no | no |
| `checkbox` | yes | yes | yes | **yes** |
| `grid` | no | no | no | no |
| `image` | no | no | no | no |
| `list-view` | yes | yes | yes | **yes** |
| `progress` | yes | yes | yes | **yes** |
| `radio` | yes | yes | yes | **yes** |
| `scroll` | yes | yes | yes | **yes** |
| `slider` | yes | yes | yes | **yes** |
| `stack` | yes | yes | yes | **yes** |
| `switch` | yes | yes | yes | **yes** |
| `tabs` | no | no | no | no |
| `text` | yes | yes | yes | **yes** |
| `text-area` | yes | yes | yes | **yes** |
| `text-field` | yes | yes | yes | **yes** |
| `tree-view` | yes | yes | yes | **yes** |

## Gaps

- macOS only: `canvas`
- Windows and Linux only: none
- Not implemented anywhere: `grid`, `image`, `tabs`
