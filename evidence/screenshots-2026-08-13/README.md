# App screenshots, 2026-08-13

Shot with `krate run <file> --auto-grant --shoot <png>`, which renders the
app's real window headlessly. Sizes are the actual `.krate` files.

| File | Size | Verdict |
|---|---|---|
| seating.png | 29,762 B | **Use this one.** Real data, seat dots showing occupancy, capacity pills, serif heading. |
| tictactoe.png | 27,539 B | Good UI, but the board is empty and the "New round" button overlaps the "Draws" label (K-106). |
| memory.png | 30,169 B | Good cards, but the hint text runs under the bottom row (K-106). |

Two of the three have overlapping text. That is filed as K-106 and it is why
only the seating chart should go in a post as-is.

The empty board in tictactoe is a separate problem for marketing: `--shoot`
captures the opening frame, and a game screenshot wants a game in progress.
Seeding state before the shot would fix it.
