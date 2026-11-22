# Vault versus Obsidian: feature parity

What the Krate replica (`apps/krate-vault`, "Vault") does of Obsidian's core,
checked 2026-11-22 on commit-time source. **Done** means it works and was
seen working; **Partial** says what is missing; **Missing** is not there. How
each was verified is on its line: *unit* is a native `cargo test` in
`apps/krate-vault` (55 tests), *task* is a headless `krate run --task` run
against a generated vault, *shot* is a picture in `screens/`, *check* is
`krate check-app apps/krate-vault` (prints OK; its task switches, links and
searches). Nothing was clicked in a real window: every run was headless.
The Obsidian side was not run in this pass; the comparison is against
Obsidian's documented default behaviour.

## The features IC-999 names

| Feature | State | How verified |
|---|---|---|
| Vault is one folder of `.md` files and subfolders | Done | task: medium (1,001 notes, 9 folders) and heavy (10,001 notes, 41 folders) listed and indexed; unit `opening_lists_notes_and_folders_but_not_hidden_ones` (dot folders hidden) |
| Open a folder the person picks | Done, not clicked | the `ui.dialog:open-folder` path (pick is the grant, `picked/<token>/`) is Krate Code's proven path; check withholds and grants it; a headless run cannot show a real picker |
| Vault path given at start (benchmark) | Done | `vault=./vault` with `--sandbox-root` and the optional `fs.*:./vault/**` grants; README section 3 |
| Reopen the last vault next launch | Missing (runtime hole) | a picked folder's grant ends with the run (K-1336); Vault shows its picker each launch |
| File tree: expand and collapse folders | Done | unit `the_tree_expands_and_opens_notes`; shot 01 (chevrons, indent guides); keyboard arrows in the tree |
| Create note | Done | unit `new_note_asks_for_its_name` (Cmd+N makes `Untitled.md` in the picked folder and starts naming it); ribbon, tree button, palette, context menu |
| Create folder | Done | unit `create_rename_move_and_trash`; tree button and context menu |
| Rename (and keep links working) | Done | unit `renaming_rewrites_links_and_follows_tabs`: inline rename in the tree, open tabs follow, `[[old]]`, `[[old|alias]]`, `[[old#h]]` rewritten in open tabs (undoable) and on disk |
| Move | Done | unit `move_and_drag_into_folders`: "Move file to..." folder chooser and drag a row onto a folder; drag not exercised in a real window |
| Delete with confirm | Done | unit `delete_asks_first_and_moves_to_trash`: confirm modal, Enter/Delete or Cancel/Esc; goes to the vault's `.trash` (Obsidian's default is the system trash; `.trash` is its other setting) |
| Tabs across the top of the editor | Partial | new, close, switch, Ctrl+Tab, Cmd+W, Cmd+T, Cmd-click opens in a new tab, back and forward per tab (unit `backlinks_follow_the_open_note`); no drag to reorder, no pinning, no split panes |
| Right sidebar: backlinks | Partial | Linked mentions with each linking line (unit, shots 02 and 10, heavy: 29 backlinks); no "Unlinked mentions" section |
| Right sidebar: outline | Done | headings by level, click jumps (unit `words_and_headings`, shot 06) |
| Right sidebar: outgoing links, tags | Done | outgoing links (unresolved dimmed); tags with counts, click searches the tag (shot 08, unit `search_runs_as_a_job`) |
| Live preview: headings, bold, italic, strike, highlight, inline code | Done | unit `styles_and_syntax_are_found`; syntax hidden except on the line being edited, as Obsidian does (shots 01, 02) |
| Live preview: lists, nested lists, numbered lists | Done | bullets drawn as dots with indent guides; Enter continues a list, Enter on an empty item outdents or ends it, Tab and Shift+Tab indent (unit `lists_continue_and_end_on_enter`, `tab_indents_list_items_only`) |
| Live preview: checkboxes | Done | drawn boxes, click ticks, Cmd+L toggles, done items struck (unit `tasks_toggle_and_bold_wraps`, shot 10) |
| Live preview: code blocks | Done | fenced blocks shaded in a mono face, language label on the fence; the fence index follows edits without rescanning (unit `fences_follow_typing`) |
| Live preview: links | Done | `[[x]]`, `[[x|alias]]` (alias shown), `[text](url)` (text shown), bare `https://` links; unresolved wiki links dimmed |
| Live preview: tables, callouts, embeds, images, math, properties | Missing | tables show as plain text rows; `![[embed]]` shows as a link; front matter shows as text |
| Reading view toggle | Partial | Cmd+E and the header button (unit `palette_runs_commands`, shot 07); it is the same line renderer with all syntax hidden and no caret, not a separate document renderer: blank lines keep their height, nothing is transcluded |
| Source mode | Done | palette "Toggle live preview and source mode": every character shown |
| `[[wikilinks]]` open the target | Done | click (Cmd-click for a new tab) opens it; a link to a missing note creates it, as Obsidian does (unit `following_a_missing_link_makes_the_note`); `[[note#Heading]]` opens the note but does not scroll to the heading (Partial) |
| Completion while typing `[[` | Partial | fuzzy over all note names, arrows and Enter or Tab or click, reuses the auto-closed `]]` (unit `brackets_pair_and_links_complete`, shot 06, check task); no heading (`[[note#`) or block completion |
| `#tags` | Done | styled as pills, indexed, tag pane, `tag:` search including nested tags (unit `tags_match_their_children`) |
| Vault search (Cmd+Shift+F) with matching lines | Done | runs between frames with results as they come, words AND'ed, `"phrases"`, `tag:`, `path:`, `file:`, `-exclude`; results grouped by note with each matching line and the match highlighted; click jumps to it (unit, shot 04: 580 notes, 2,371 matches); no regex, no case-sensitive toggle, no sort choice |
| Quick switcher (Cmd+O) | Done | fuzzy over note names then paths, recent notes first when empty, Enter opens, Cmd+Enter new tab, Shift+Enter or no match creates (unit `switcher_finds_notes_fuzzily_and_creates_missing_ones`, shot 03 at 10,001 notes) |
| Command palette (Cmd+P) | Done | 37 commands with their keys, fuzzy (unit `palette_runs_commands`, shot 05); no hotkey editing |
| Status bar: word and character counts | Done | counts kept per edit from the touched lines only (unit `counts_follow_every_edit`, 200,000-line note: 2,354,639 words, shot 09), plus backlink count and indexing progress |
| Dark theme by default | Done | dark by default; palette switches to light |
| Same density and structure | Done (approximate) | ribbon 44 px, sidebars 260 and 280 px (dragged to resize), 26 px tree rows at 13 px, 40 px tab strip, 38 px view header, 16 px editor text at a 700 px reading width, 12 px status bar; inline title above the note; no pixel comparison against Obsidian was made |
| No Obsidian name, logo or icons | Done | app is "Vault", its own accent (blue), its own icons drawn from shapes; `grep -rnil obsidian apps/krate-vault --exclude-dir=target` finds nothing |

## Behaviour that decides what the benchmark measures

| Behaviour | State | How verified / note |
|---|---|---|
| Saves on a pause in typing | Done | 2 s after the last change, on tab switch, close and Cmd+S (unit `typing_saves_after_a_pause_and_reindexes`); the 14 MB note saved in 4 MiB pieces |
| Undo and redo | Done | grouped like typing (buffer tests carried from Krate Code) |
| Copy, cut, paste | Done | Cmd+C/X write the selection, Cmd+V inserts the text the host delivers |
| Typing through an input method | Partial | composed text arrives as `text-input` and is inserted; no in-progress composition underline |
| Caret blink | Done | blinks every 600 ms while the editor has focus, like Obsidian's editor; this costs a frame per blink and counts toward idle CPU on both sides |
| Notices files changed outside the app | Partial | Krate's folder watch is a scan, so Vault polls it every 3 s: changed notes are re-read and an unedited open tab reloads; the poll costs idle CPU that grows with the vault, where Obsidian is told by the OS |
| 10,000 notes | Done | tree listed once at open, index of links and tags filled between frames, 8 ms a frame (task, heavy: all 10,001 indexed) |
| 200,000-line note | Done | only visible lines laid out, cached by their text (task: 44 lines laid out to open, draw, jump to the end and type); edits update counts and fences from the touched lines |
| Memory proportional | Done by design, not yet measured | per note: path, name, links and tags; per open note: its lines; the layout cache holds at most 6,000 lines |
| A folder of more than 4,096 entries | Missing (runtime hole) | `Could not read the vault folder: directory listing exceeds limit (4096 entries)` (screens/11; K-1334); the generated vaults keep folders at 400 or fewer |
| Notes larger than 8 MiB | Done in the app; SDK bug filed | the SDK's `fs::write` sends one request the host refuses over 8 MiB (K-1335); Vault reads and writes in 4 MiB pieces itself |

## Not in this replica (outside IC-999's list)

Graph view, canvas, daily notes, templates, bookmarks, properties, split
panes and stacked tabs, hover previews, in-note find and replace (Cmd+F),
spell check, settings, themes and plugins, sync, publish, PDF export,
attachments and images.

## Runtime holes found while building it

Filed in BUGS.md, unclaimed:

- **K-1334** (runtime-hole): no folder of more than 4,096 entries can be listed.
- **K-1335** (our-code): `fs::write` fails over 8 MiB; `read_to_end` reads 8 KiB a call.
- **K-1336** (runtime-hole): a picked folder is forgotten when the app closes.
