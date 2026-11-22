# Vault versus Obsidian: feature parity

What the Krate replica (`apps/krate-vault`, "Vault") does of Obsidian's core,
checked 2026-11-22 on commit-time source. **Done** means it works and was
seen working; **Partial** says what is missing; **Missing** is not there. How
each was verified is on its line: *unit* is a native `cargo test` in
`apps/krate-vault` (91 tests), *task* is a headless `krate run --task` run
against a generated vault, *shot* is a picture in `screens/`, *check* is
`krate check-app apps/krate-vault` (prints OK; its task switches, links and
searches). Nothing was clicked in a real Vault window: every run was
headless. The workspace, core-plugin and look rows were compared against
Obsidian 1.15.1 itself: run with its own isolated profile on the same
generated vault, its window captured and measured (sizes in px, colours
read from the pixels and from its own CSS variables), pictures side by side
in `screens/parity/`.

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
| Delete with confirm | Done | unit `delete_asks_first_and_moves_to_trash`: confirm modal (off with "Confirm file deletion"), Enter/Delete or Cancel/Esc; goes to the vault's `.trash` (Obsidian's default is the system trash; Krate has no call for it: K-1337) |
| Tabs across the top of the editor | Done | new, close, switch, Ctrl+Tab, Cmd+1..9, Cmd+W, Cmd+T, Cmd+Shift+T (undo close), Cmd-click opens in a new tab, back and forward per tab; drag to reorder, into another group, or to a group's right or bottom edge (a new group); pin (links open beside, Cmd+W leaves it); right-click menu (close others, pin, split, link) (unit `panes_split_pin_and_close`, shot parity/08) |
| Split panes | Done | Split right / Split down (palette, tab menu, Cmd+Shift+N for a new note to the right), any depth, dividers drag, a group with no tabs closes (unit `panes::splits_place_and_close`, `panes_split_pin_and_close`) |
| Linked panes | Partial | "Link with tab beside": linked tabs follow each other's note; the local graph opens linked to its note (unit `linked_tabs_follow`); Obsidian's linked outline/backlinks views are the sidebar panes here |
| Stacked tabs | Missing | "Toggle stacked tabs" not built |
| Sidebars collapse, resize | Done | both sidebars toggle (top-row buttons, palette), drag their edges; the right sidebar starts closed, as Obsidian's does |
| Right sidebar: backlinks | Done | Linked mentions (lines as written, the link marked) and Unlinked mentions (found a few notes a frame while open, sorted by name, Link turns them into links) (unit `bookmarks_unlinked_mentions_tags_and_properties`, shot parity/02); Obsidian also counts a canvas's JSON as an unlinked mention (9 against Vault's 6 on the bench vault) |
| Right sidebar: outline | Done | headings by level, click jumps (unit `words_and_headings`, shot 06) |
| Right sidebar: outgoing links, tags, all properties | Done | outgoing links (unresolved dimmed); tags nested (`a/b` under `a`, counts summed, fold), click searches the tag; all properties with counts, click searches `[name]` (unit `bookmarks_unlinked_mentions_tags_and_properties`) |
| Bookmarks | Done | notes, headings, folders, searches, groups, in `.obsidian/bookmarks.json` in its format; left sidebar pane, open, fold, remove (unit `bookmarks::the_apps_bookmarks_file_round_trips`) |
| Live preview: headings, bold, italic, strike, highlight, inline code | Done | unit `styles_and_syntax_are_found`; syntax hidden except on the line being edited, as Obsidian does (shots 01, 02) |
| Live preview: lists, nested lists, numbered lists | Done | bullets drawn as dots with indent guides; Enter continues a list, Enter on an empty item outdents or ends it, Tab and Shift+Tab indent (unit `lists_continue_and_end_on_enter`, `tab_indents_list_items_only`) |
| Live preview: checkboxes | Done | drawn boxes, click ticks, Cmd+L toggles, done items struck (unit `tasks_toggle_and_bold_wraps`, shot 10) |
| Live preview: code blocks | Done | fenced blocks shaded in a mono face, language label on the fence; the fence index follows edits without rescanning (unit `fences_follow_typing`) |
| Live preview: links | Done | `[[x]]`, `[[x|alias]]` (alias shown), `[text](url)` (text shown), bare `https://` links; unresolved wiki links dimmed |
| Live preview: tables, callouts, embeds, images, math, properties | Done | each drawn whole while the caret is outside it and shown as source with the caret in it, as live preview does; see "Editor and rendering" below |
| Reading view toggle | Done | Cmd+E and the header button (unit `palette_runs_commands`); reading view lays lines out by its own rules: blank lines collapse into paragraph gaps, comments, footnote definitions and syntax-only lines go, footnotes are numbered and listed at the end, every block is drawn whole, embeds transclude (screens/parity/*-reading.png) |
| Source mode | Done | palette "Toggle live preview and source mode": every character shown |
| `[[wikilinks]]` open the target | Done | click (Cmd-click for a new tab) opens it; a link to a missing note creates it, as Obsidian does (unit `following_a_missing_link_makes_the_note`); `[[note#Heading]]` and `[[note#^block]]` open the note with the heading or block at the top and the caret on it, `[[#Heading]]` stays in the note (unit `links_go_to_their_heading_and_block`); shown as "Note > Heading" (shot parity/links-live) |
| Completion while typing `[[` | Done | fuzzy over all note names, arrows and Enter or Tab or click, reuses the auto-closed `]]` (unit `brackets_pair_and_links_complete`, shot 06, check task); after `[[Note#` the note's headings with their level, after `[[Note#^` its block ids with their text (unit `headings_and_blocks_complete_after_a_hash`); Obsidian also offers paragraphs without an id after `#^` and writes an id into the target -- Vault offers only blocks that have one |
| `#tags` | Done | styled as pills, indexed, tag pane, `tag:` search including nested tags (unit `tags_match_their_children`) |
| Vault search (Cmd+Shift+F) with matching lines | Done | runs between frames with results as they come, words AND'ed, `"phrases"`, `tag:`, `path:`, `file:`, `-exclude`; results grouped by note with each matching line and the match highlighted; click jumps to it (unit, shot 04: 580 notes, 2,371 matches); no regex, no case-sensitive toggle, no sort choice |
| Quick switcher (Cmd+O) | Done | fuzzy over note names then paths, recent notes first when empty, Enter opens, Cmd+Enter new tab, Shift+Enter or no match creates (unit `switcher_finds_notes_fuzzily_and_creates_missing_ones`, shot 03 at 10,001 notes) |
| Command palette (Cmd+P) | Done | 68 commands by Obsidian's names and ids ("Graph view: Open graph view", "Daily notes: Open today's daily note"...), a plugin switched off takes its commands away; default hotkeys are Obsidian's (Cmd+G graph, Cmd+, settings, Cmd+Shift+T, Cmd+1..9...), and ones set in `.obsidian/hotkeys.json` override them (unit `hotkeys_default_and_own`); hotkeys are listed in settings, not yet recorded there |
| Status bar: word and character counts | Done | counts kept per edit from the touched lines only (unit `counts_follow_every_edit`, 200,000-line note: 2,354,639 words, shot 09), plus backlink count and indexing progress |
| Dark theme by default | Done | Obsidian's default theme value for value from its CSS (`--color-base-00` #1C1C1C ... `-100` #DADADA, accent hsl(258 88% 66%), light `moonstone` too); `appearance.json` `theme` obsidian / moonstone / system read and written (shots parity/02, 03) |
| Same density and structure | Done (measured) | measured from Obsidian 1.15.1 window captures on this Mac: full-bleed top row with the sidebar tabs (Files, Search, Bookmarks) and toggles, 300 px sidebars, 27 px tree rows at 13 px, CANVAS/BASE tags, 40 px tab bar with 200 px tabs (rounded top, outlined), 40 px view header, file-explorer action row, vault switcher with help and settings at the bottom left, floating status bar ("6 backlinks", pencil, words, characters), the New tab page's round buttons; ribbon off by default (`showRibbon`), on in settings; side by side in screens/parity/ |
| Settings window | Done | General, Editor, Files and links, Appearance, Hotkeys, Core plugins, and the Daily notes, File recovery and Templates pages; settings that change behaviour work (default view mode, live preview, readable line length, confirm deletion, new note location and folder, theme, font size, ribbon, view header, plugins on/off...), written to `.obsidian/app.json`, `appearance.json`, `core-plugins.json`, `daily-notes.json`, `templates.json`, `file-recovery.json` in its format, unknown keys kept (unit `settings_rows_change_the_files`, `settings::*`, shot parity/07); show line numbers, spellcheck, vim and the inline-title switch are stored but the editor does not act on them yet |
| No Obsidian name, logo or app icon | Done | app and window are "Vault" with Vault's own welcome mark and app icon; the icons are Lucide-style line icons drawn by Vault; the word appears only where the file format needs it (the `.obsidian` settings folder, theme values `obsidian`/`moonstone`) and in comments |

## Core plugins beyond IC-999's list (branch vault-workspace)

| Feature | State | How verified |
|---|---|---|
| Graph view (global) | Done | every note and canvas a dot sized by links, unresolved links faint, tags green when shown; force layout (links, Barnes-Hut push, centre pull) that cools and stops; zoom at the pointer, pan, drag a dot, hover lights it and its neighbours, click opens; Filters (search, tags, attachments, existing files only, orphans), Groups (colour by search), Display (arrows, text fade, node size, link thickness), Forces (centre, repel, link force, distance) saved in `.obsidian/graph.json` in its format (units `graph::*`, shot parity/04); "Animate" and the attachments toggle (Vault lists no attachments) are not built |
| Graph at 10,000 notes | Done | the 10,001-note vault lays out and draws (shot parity/09); a layout step plus a full picture of 10,000 dots and 49,959 links is 9.7 ms and 26.3 ms natively in release (`cargo test --release graph::bench -- --ignored --nocapture`); the picture is kept by the host and moved and scaled while panning, redrawn after |
| Graph view (local) | Done | the note's neighbourhood (depth 1), opened to the right and linked to the note's tab so it follows it (unit `linked_tabs_follow`); depth and its own filters not yet adjustable |
| Canvas | Done | `.canvas` (JSON Canvas) read and written as Obsidian writes it (tab indents, a card per line, key order and unknown keys kept, numbers rewritten only when moved: unit `canvas::a_board_reads_and_writes_back_byte_for_byte`); text, note, link and group cards with the six preset colours or hex, arrows as curves from card sides with ends and labels; pan (wheel or drag), zoom (Cmd+wheel, controls), zoom to fit; double-click makes a card, type in it, drag to move (a group moves what is in it), corner resizes, drag from a side handle to another card to connect, Delete removes, Cmd+Z/Cmd+Shift+Z, right-click colour/remove/open; toolbar adds a card, a note, a web page (units `canvas_cards_are_added_typed_saved_and_undone`, shot parity/05). A link card shows its address, not the page; note cards show the note's text, not a live editor |
| Bases | Done | `.base` YAML read in Obsidian's format: global and per-view `filters` (and/or/not; `file.hasTag`, `file.inFolder`, `file.hasLink`, `file.hasProperty`, comparisons, arithmetic, `.contains`/`.startsWith`/`isEmpty`), `order` columns, `properties` display names, `sort` with direction, `limit`; table with the view switcher and result count, header click sorts (units `bases::*`, shot parity/06: the same three rows in the same order as Obsidian). Cards, list and map views draw as the table; formulas, group by and editing cells are not built; dates show as written (Obsidian shows them in the locale's format) |
| Daily notes | Done | open today's, previous, next; date format (moment tokens), folder, template, open on startup, from `daily-notes.json` (unit `daily_notes_open_today_with_the_template`, `dates::*`) |
| Templates | Done | insert template from the template folder, `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}`; insert current date or time, formats from `templates.json` (unit `templates_insert_at_the_cursor`) |
| Note composer | Done | merge the current note into another (text appended, links to it pointed at the other, it is deleted), extract the selection to a new or existing note leaving a link (unit `note_composer_merges_and_extracts`); "Extract this heading" not built |
| File recovery | Done | a snapshot of a note on save at most every `intervalMinutes`, kept `keepDays`, in the app's own store (as Obsidian keeps them outside the vault); "Open local history" lists them, shows one, Restore (keeping the current text as a snapshot); Clear history (units `recovery::*`, `recovery_keeps_copies_and_restores`) |
| Page preview (hover a link) | Missing | the editor branch's work |
| Random note, slides, audio recorder, workspaces, unique note creator, web viewer, publish | Not built | off by default in Obsidian |
| Sync | Excluded | a paid cloud service; its plugin row says Vault has none |

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

## Not in this replica

Not built: stacked tabs, attachments and images in the file tree,
recording hotkeys in settings, pop-out windows, community themes and
plugins, sync, publish, PDF export. Spell check: Krate has no spell checker
to ask (K-1340).

## Runtime holes found while building it

Filed in BUGS.md, unclaimed:

- **K-1334** (runtime-hole): no folder of more than 4,096 entries can be listed.
- **K-1335** (our-code): `fs::write` fails over 8 MiB; `read_to_end` reads 8 KiB a call.
- **K-1336** (runtime-hole): a picked folder is forgotten when the app closes (fixed since).
- **K-1337** (runtime-hole): no call moves a file to the computer's trash, so Vault deletes into the vault's `.trash`.

## Editor and rendering, against Obsidian 1.15.1 side by side

Checked 2026-11-22 against Obsidian 1.15.1 itself (not its documentation):
the same notes (`render-vault/`, copied into a generated light vault) opened
in an isolated Obsidian profile and captured with `screencapture -l`, and in
Vault headless with `krate run --task` steps that wait and `shoot`. Each
picture in `screens/parity/` is Obsidian's note pane on the left and
Vault's on the right. *unit* names a native test in `apps/krate-vault`
(91 tests in all).

| Feature | State | How verified / what differs |
|---|---|---|
| Tables (GFM pipes) | Done | drawn with 1 px borders, 8 by 3 px cell padding, bold header, `:---`, `:---:`, `---:` alignment, inline Markdown and links in cells, columns sized to content and shared when too wide (unit `table_rows_split_into_cells`, `spans_are_found_from_any_of_their_lines`; parity/table-live, table-reading); the caret in a table shows its rows as source. Obsidian 1.15 edits a cell inside the drawn table; Vault edits the row's source (the task's "editable as source") |
| Callouts | Done | every Obsidian type and alias (note, abstract/summary/tldr, info, todo, tip/hint/important, success/check/done, question/help/faq, warning/caution/attention, failure/fail/missing, danger/error, bug, example, quote/cite; unknown types as note), its icon and RGB, a tenth of the colour behind, title or the type's name, `+`/`-` folding with the chevron, Markdown and lists inside (unit `callouts_read_their_head`; parity/callouts-live, callouts-reading). Icons are drawn from lines, close to but not Lucide's |
| Embeds: notes | Done | `![[Note]]` (headed by its name), `![[Note#Heading]]` (the section), `![[Note#^block]]` (the block), with the accent bar, their own tables, callouts, maths and pictures drawn; an embed inside an embed shows as its link (unit `sections_and_blocks_are_found`, `embeds_name_notes_pictures_and_sizes`; parity/embeds-notes-live). Obsidian's open-in-place icon on hover is not drawn |
| Embeds: pictures | Done | `![[pic.png]]`, `![[pic.png|300]]`, `![[pic.png|300x200]]`, `![alt](path)` and `![alt|64](path)`, found beside the note, at the root or in any folder; PNG, JPEG, GIF (first frame) and BMP decoded in the app, scaled to the column, rounded 4 px (unit `png_decodes_to_rgba`, `gif_first_frame_decodes`, `bmp_and_unknown_formats`; parity/embeds-pictures-live). WebP, AVIF and SVG show "... cannot be shown yet": no `no_std` decoder for them is in the app; animated GIFs stay on their first frame; pictures from the web are not fetched |
| Math | Done | `$...$` inline and `$$...$$` blocks set in a serif face by the app's own TeX layout: fractions (`\frac`, `\dfrac`, `\binom`), sub- and superscripts, Greek, relations and operators, `\sum`/`\prod`/`\lim` with limits in display and beside inline, integrals, `\sqrt` and `\sqrt[n]`, `pmatrix`/`bmatrix`/`vmatrix`/`cases`/`aligned`, growing `\left( \right)`, accents, `\text`, `\mathbb`, spacing; what it cannot read shows in red, as KaTeX does (unit `math::tests`, 6 tests; parity/math-live). Glyph shapes are the system serif, not KaTeX's fonts |
| Properties | Done | the Properties block for YAML front matter: text, list, number, checkbox, date, date and time, tags (pills), aliases, with their icons and the 148 px key column, "Add property" (unit `properties_are_typed_like_the_block_shows_them`, `presses_tick_fold_and_edit_properties`; parity/properties-live). A checkbox ticks in place and Add property adds a key; other values are edited in their YAML line (a click puts the caret there), where Obsidian edits them in the block |
| Footnotes | Done | `[^1]` and `^[inline]` small and raised while editing; reading view numbers them by first reference and lists them after the note with the way back (unit `footnotes_number_in_reference_order`; parity/inline-live, inline-reading). The way-back arrow is not clickable |
| Highlights, strikethrough, inline HTML | Done | `==x==`, `~~x~~`, `<u>`, `<mark>`, `<sub>`, `<sup>`, `<kbd>`, `<b>`/`<strong>`, `<i>`/`<em>`, `<s>`/`<del>`, `<br>` and `<span>` (tags hidden, text styled) (unit `math_footnotes_comments_html_and_block_ids`). Other HTML (tables, `<div>`, `<img>`, `<details>`) shows as text |
| `%%comments%%` | Done | dimmed while editing, gone in reading view, inline and as `%%` blocks (unit same, `spans_are_found_from_any_of_their_lines`) |
| Horizontal rules, blockquotes, nested and task lists | Done | task states `[ ]`, `[x]`, `[/]`, `[-]` and any other mark drawn in the box; ordered numbers muted. Obsidian also strikes the children of a done task; Vault strikes only the task's own line |
| Block ids `^id` | Done | small and dimmed while editing, hidden in reading view |
| Code blocks | Done | coloured by language with Obsidian's dark and light code colours: Rust, JavaScript/TypeScript, Python, shell, C/C++/Java/Go/C#/Swift/Kotlin, SQL, JSON/YAML/TOML, CSS; the language's name in the corner (unit `code_lines_are_coloured_by_language`; parity/code-mermaid-live) |
| Mermaid | Partial | flowcharts (`graph`/`flowchart`, TD/TB/BT/LR/RL, rectangle, round, stadium, circle and diamond nodes, solid, dotted and thick edges, labels, cycles) drawn by the app (unit `flowcharts_rank_their_boxes`, `mermaid_flowcharts_are_blocks_and_other_code_is_not`; parity/code-mermaid-live); the layout is simple ranks, not Mermaid's. Sequence, class, state, Gantt, pie and other diagram kinds stay a styled code block |
| Reading view | Done | its own layout (see the row above); typography spacing measured against Obsidian's: paragraph gaps of 1 em, headings 1.618/1.462/1.318/1.188/1.076 em |
| Heading links show as "Note > Heading" | Done | parity/links-live |
| Page preview | Done | a link hovered with Cmd held shows the note (its blocks drawn) in a popover under the link, gone when the pointer leaves link and popover (`preview=Table` start argument for a headless picture: screens/parity/vault-page-preview.png). Obsidian also previews without Cmd in reading view and on search results; Vault only in the note |
| Tag click opens search | Done | unit `search_runs_as_a_job` (tag: query) |
| External links | Done | open in the browser through `ui.open-url` |
| Find and replace in the note | Done | Cmd+F opens the bar with the selection, matches highlighted, "n / total", Enter/Shift+Enter and the arrows step, Escape closes; Cmd+Option+F or the bar's button adds the replace row, Replace and Replace all (one undo); at most 10,000 matches counted (unit `find::tests`, `keys_drive_several_carets_and_the_find_bar`; screens/parity/vault-find-replace.png). Case-insensitive only; no regex toggle |
| Several carets | Done | Option-click adds or removes a caret, Cmd+D selects the word then each next occurrence, Cmd+Option+Up/Down adds a caret above or below; typing, Backspace, Delete, Enter, Left, Right, Home and End act at every caret as one undo; Escape keeps one (unit `several_carets_type_together`). Up and Down move only the main caret |
| Folding | Done | headings (to the next heading at their level) and list items (their children) fold from a chevron in the margin, shown on hover and while folded, with "..." after a folded line; Cmd+Option+[ and ] fold and unfold, with Shift all at once; folds follow edits (unit `headings_and_lists_fold`) |
| Smart lists | Done | (earlier: `lists_continue_and_end_on_enter`) |
| Drag text | Done | a press inside the selection drags the text and drops it where released, one undo (unit `selected_text_moves_by_dragging`); not dragged in a real window |
| Spell check | Missing (runtime hole) | Krate has no spell checker an app can ask (K-1340) |

Performance with all of this on: the 200,000-line note (debug runtime,
`krate run ... --task` that waits 1.5 s, Cmd+Down, types, saves):

| | before (b689eb95f) | after |
|---|---|---|
| note read and parsed (`t_note_open_ms`) | 160, 163 | 171, 199 |
| first frame (`t_first_frame_ms`) | 766, 777 | 763, 810 |
| lines laid out in the whole run | 44 | 45 |
| saved after typing | yes | yes |

The extra time at open is finding the `$$`, `%%` and front matter lines
once; nothing else scans the note.

Not checked here: anything in a real window (every run above was
headless), Obsidian's light theme side by side, and the light-theme code
colours.
