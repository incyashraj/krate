# App screenshots, 2026-08-13

Shot with `krate run <file> --auto-grant --shoot <png>`, which renders the
app's real window headlessly. Sizes are the live gallery figures, checkable
at krate.tech/cloud.

## Use these

| Image | Size | Why |
|---|---|---|
| krate-pulse.png | 26,962 B | **The hero.** Money dashboard: balance, 30-day spending chart, category bars, transactions, an insight card. Nothing overlaps. |
| krate-journal.png | 29,878 B | Chat-style journal. "Only you can see this, entries stay on this device" is a ready-made privacy line. |
| seating.png | 29,762 B | Wedding seating planner. Real data, seat dots, capacity pills. |
| krate-notes.png | 29,137 B | Notes that stay local. |
| krate-focus.png | 23,790 B | Focus timer with a ring. |
| krate-savings.png | 24,712 B | Budget splitter. |
| krate-clocks.png | 24,261 B | Six world clock faces. |

## Do not post these

| Image | Problem |
|---|---|
| tictactoe.png | "New round" button overlaps the "Draws" label (K-106). Also an empty board, and nobody shares a tic tac toe game. |
| memory.png | Hint text runs under the bottom row of cards (K-106). |

Both games pass check-app and both look wrong at a glance. That is K-106.

## Note on game screenshots

`--shoot` captures the opening frame, so a game appears unplayed. Seed state
before shooting if a game image is ever needed.
