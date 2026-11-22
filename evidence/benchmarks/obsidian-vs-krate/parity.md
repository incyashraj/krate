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
| Delete with confirm | Done | the "Deleted files" setting is acted on: system trash, the vault's `.trash`, or for good, with Obsidian's confirm wording for each (units `delete_asks_first_and_moves_to_trash`, `extract_heading_attachments_and_deleting`, `vault::opening_lists...`); the system trash goes through one hook (`guest/disk.rs` `trash_file`) that falls back to `.trash` until K-1337's Krate call is merged |
| Tabs across the top of the editor | Done | new, close, switch, Ctrl+Tab, Cmd+1..9, Cmd+W, Cmd+T, Cmd+Shift+T (undo close), Cmd-click opens in a new tab, back and forward per tab; drag to reorder, into another group, or to a group's right or bottom edge (a new group); pin (links open beside, Cmd+W leaves it); right-click menu (close others, pin, split, link) (unit `panes_split_pin_and_close`, shot parity/08) |
| Split panes | Done | Split right / Split down (palette, tab menu, Cmd+Shift+N for a new note to the right), any depth, dividers drag, a group with no tabs closes (unit `panes::splits_place_and_close`, `panes_split_pin_and_close`) |
| Linked panes | Done | "Link with tab..." links the active tab with any tab in another group (palette, tab menu); linked tabs follow each other's note; "Open backlinks for the current note", "Open outline of the current file", "Open outgoing links for the current file" open those views as tabs beside the note, linked to it (unit `stacked_tabs_side_views_and_linking`, shot parity/14); pop-out windows are not possible: one window per app (K-1344) |
| Stacked tabs | Done | "Toggle stacked tabs" (palette, tab menu): the tab bar keeps only + and the list, each tab not showing is a spine with its title, before the showing tab on the left and after it on the right, a press on a spine shows that tab (unit `stacked_tabs_side_views_and_linking`, shot parity/10); the title runs a letter under the other, not turned on its side: canvas2d draws no turned text (K-1341) |
| Sidebars collapse, resize | Done | both sidebars toggle (top-row buttons, palette), drag their edges; the right sidebar starts closed, as Obsidian's does |
| Right sidebar: backlinks | Done | Linked mentions (lines as written, the link marked) and Unlinked mentions counted as Obsidian counts them, mentions not notes, canvas files' text included: 5 and 9 on the bench vault, the same rows in the same order (shots parity/02, 03); Link shows on the row under the pointer |
| Right sidebar: outline | Done | headings by level, click jumps (unit `words_and_headings`, shot 06) |
| Right sidebar: outgoing links, tags, all properties | Done | outgoing links (unresolved dimmed); tags nested (`a/b` under `a`, counts summed, fold), click searches the tag; all properties with counts, click searches `[name]` (unit `bookmarks_unlinked_mentions_tags_and_properties`) |
| Bookmarks | Done | notes, headings, folders, searches, groups, in `.obsidian/bookmarks.json` in its format; left sidebar pane, open, fold, remove (unit `bookmarks::the_apps_bookmarks_file_round_trips`) |
| Live preview: headings, bold, italic, strike, highlight, inline code | Done | unit `styles_and_syntax_are_found`; syntax hidden except on the line being edited, as Obsidian does (shots 01, 02) |
| Live preview: lists, nested lists, numbered lists | Done | bullets drawn as dots with indent guides; Enter continues a list, Enter on an empty item outdents or ends it, Tab and Shift+Tab indent (unit `lists_continue_and_end_on_enter`, `tab_indents_list_items_only`) |
| Live preview: checkboxes | Done | drawn boxes, click ticks, Cmd+L toggles, done items struck (unit `tasks_toggle_and_bold_wraps`, shot 10) |
| Live preview: code blocks | Done | fenced blocks shaded in a mono face, language label on the fence; the fence index follows edits without rescanning (unit `fences_follow_typing`) |
| Live preview: links | Done | `[[x]]`, `[[x|alias]]` (alias shown), `[text](url)` (text shown), bare `https://` links; unresolved wiki links dimmed |
| Live preview: tables, callouts, embeds, images, math, properties | Done | each drawn whole; tables and properties are edited inside the drawn block, the others show their source with the caret in them, as live preview does; see "Editor and rendering" below |
| Reading view toggle | Done | Cmd+E and the header button (unit `palette_runs_commands`); reading view lays lines out by its own rules: blank lines collapse into paragraph gaps, comments, footnote definitions and syntax-only lines go, footnotes are numbered and listed at the end, every block is drawn whole, embeds transclude (screens/parity/*-reading.png) |
| Source mode | Done | palette "Toggle live preview and source mode": every character shown |
| `[[wikilinks]]` open the target | Done | click (Cmd-click for a new tab) opens it; a link to a missing note creates it, as Obsidian does (unit `following_a_missing_link_makes_the_note`); `[[note#Heading]]` and `[[note#^block]]` open the note with the heading or block at the top and the caret on it, `[[#Heading]]` stays in the note (unit `links_go_to_their_heading_and_block`); shown as "Note > Heading" (shot parity/links-live) |
| Completion while typing `[[` | Done | fuzzy over all note names, arrows and Enter or Tab or click, reuses the auto-closed `]]` (unit `brackets_pair_and_links_complete`, shot 06, check task); after `[[Note#` the note's headings with their level, after `[[Note#^` every block of the note with its text, and choosing one without an id writes a new `^id` into it, as Obsidian does (unit `headings_and_blocks_complete_after_a_hash`) |
| `#tags` | Done | styled as pills, indexed, tag pane, `tag:` search including nested tags (unit `tags_match_their_children`) |
| Vault search (Cmd+Shift+F) with matching lines | Done | runs between frames with results as they come, words AND'ed, `"phrases"`, `tag:`, `path:`, `file:`, `-exclude`; results grouped by note with each matching line and the match highlighted; click jumps to it (unit, shot 04: 580 notes, 2,371 matches); no regex, no case-sensitive toggle, no sort choice |
| Quick switcher (Cmd+O) | Done | fuzzy over note names then paths, recent notes first when empty, Enter opens, Cmd+Enter new tab, Shift+Enter or no match creates (unit `switcher_finds_notes_fuzzily_and_creates_missing_ones`, shot 03 at 10,001 notes) |
| Command palette (Cmd+P) | Done | 68 commands by Obsidian's names and ids ("Graph view: Open graph view", "Daily notes: Open today's daily note"...), a plugin switched off takes its commands away; default hotkeys are Obsidian's (Cmd+G graph, Cmd+, settings, Cmd+Shift+T, Cmd+1..9...), and ones set in `.obsidian/hotkeys.json` override them (unit `hotkeys_default_and_own`); hotkeys are listed in settings, not yet recorded there |
| Status bar: word and character counts | Done | counts kept per edit from the touched lines only (unit `counts_follow_every_edit`, 200,000-line note: 2,354,639 words, shot 09), plus backlink count and indexing progress |
| Dark theme by default | Done | Obsidian's default theme value for value from its CSS (`--color-base-00` #1C1C1C ... `-100` #DADADA, accent hsl(258 88% 66%), light `moonstone` too); `appearance.json` `theme` obsidian / moonstone / system read and written (shots parity/02, 03) |
| Same density and structure | Done (measured) | measured from Obsidian 1.15.1 window captures on this Mac: full-bleed top row with the sidebar tabs (Files, Search, Bookmarks) and toggles, 300 px sidebars, 27 px tree rows at 13 px, CANVAS/BASE tags, 40 px tab bar with 200 px tabs (rounded top, outlined), 40 px view header, file-explorer action row, vault switcher with help and settings at the bottom left, floating status bar ("6 backlinks", pencil, words, characters), the New tab page's round buttons; ribbon off by default (`showRibbon`), on in settings; side by side in screens/parity/ |
| Settings window | Done | General, Editor, Files and links, Appearance, Hotkeys, Core plugins, and the Daily notes, File recovery and Templates pages; settings that change behaviour work (default view mode, live preview, readable line length, confirm deletion, deleted files, new note location and folder, attachment folder, detect all file extensions, theme, font size, ribbon, view header, plugins on/off...), written to `.obsidian/*.json` in its format, unknown keys kept. Hotkeys: press a command's row and then the keys to bind it, Backspace leaves it blank, the restore button puts the default back, a clash is shown in red with the other command's name; written to `hotkeys.json` as Obsidian writes it (unit `hotkeys_are_recorded_reset_and_clash`, shot parity/15) |
| No Obsidian name, logo or app icon | Done | app and window are "Vault" with Vault's own welcome mark and app icon; the icons are Lucide-style line icons drawn by Vault; the word appears only where the file format needs it (the `.obsidian` settings folder, theme values `obsidian`/`moonstone`) and in comments |

## Core plugins beyond IC-999's list (branch vault-workspace)

| Feature | State | How verified |
|---|---|---|
| Graph view (global) | Done | every note and canvas a dot sized by links, unresolved links faint, tags green when shown; force layout (links, Barnes-Hut push, centre pull) that cools and stops; zoom at the pointer, pan, drag a dot, hover lights it and its neighbours, click opens; Filters (search, tags, attachments, existing files only, orphans), Groups (colour by search), Display (arrows, text fade, node size, link thickness), Forces (centre, repel, link force, distance) saved in `.obsidian/graph.json` in its format (units `graph::*`, shot parity/04); "Animate" and the attachments toggle (Vault lists no attachments) are not built |
| Graph at 10,000 notes | Done | the 10,001-note vault lays out and draws (shot parity/09); a layout step plus a full picture of 10,000 dots and 49,959 links is 9.7 ms and 26.3 ms natively in release (`cargo test --release graph::bench -- --ignored --nocapture`); the picture is kept by the host and moved and scaled while panning, redrawn after |
| Graph view (local) | Done | opened to the right, linked to the note's tab and following it, centred on the note drawn in the accent; its own panel: depth (1-5), incoming, outgoing and neighbour links (off by default, as Obsidian), the filters, groups, display and forces (unit `graph::local_graph_keeps_the_neighbourhood`, shot parity/11); Animate replays the graph growing in both graphs |
| Canvas | Done | `.canvas` (JSON Canvas) read and written as Obsidian writes it (tab indents, a card per line, key order and unknown keys kept, numbers rewritten only when moved: unit `canvas::a_board_reads_and_writes_back_byte_for_byte`); text, note, link and group cards with the six preset colours or hex, arrows as curves from card sides with ends and labels; pan (wheel or drag), zoom (Cmd+wheel, controls), zoom to fit; double-click makes a card, type in it, drag to move (a group moves what is in it), corner resizes, drag from a side handle to another card to connect, Delete removes, Cmd+Z/Cmd+Shift+Z, right-click colour/remove/open; toolbar adds a card, a note, a web page (units `canvas_cards_are_added_typed_saved_and_undone`, shot parity/05). A link card reads the page (optional `net.connect:*:443`) and shows its title above the card and its title, description and site in it, as a preview (unit `canvas::a_link_card_reads_the_pages_title`); Obsidian shows the live page there; note cards show the note's text, not a live editor |
| Bases | Done | `.base` YAML read in Obsidian's format: global and per-view `filters`, `formulas` (if, arithmetic, `today()`/`now()`/`date()`, date differences shown as "9 days", `.format()`, `.year`/`.month`/`.day`, `.relative()`, text methods, `min`/`max`/`abs`/`concat`, `.toFixed()`), `order`, display names, `sort`, `limit`, `groupBy` (headed groups); table and cards views; dates with a calendar mark in the locale's day-first order; a cell pressed asks for a new value and writes it into the note's properties (units `bases::*`, `base_cells_set_properties`, shots parity/06, 13). List and map views draw as the table; summaries are not built |
| Attachments | Done | pictures, PDF, sound and video listed in the tree with their extension tag (PNG, PDF...), every file with "Detect all file extensions"; a picture opens in a tab fitted to it (shot parity/12); "Insert attachment" copies a picked file where "Default location for new attachments" says (`/`, `./`, `./sub`, a folder) and embeds it (unit `extract_heading_attachments_and_deleting`). A PDF cannot be shown (no PDF renderer in Krate: K-1342) and sound or video in the vault cannot be played (K-1343): the tab says so |
| Daily notes | Done | open today's, previous, next; date format (moment tokens), folder, template, open on startup, from `daily-notes.json` (unit `daily_notes_open_today_with_the_template`, `dates::*`) |
| Templates | Done | insert template from the template folder, `{{title}}`, `{{date}}`, `{{time}}`, `{{date:FORMAT}}`; insert current date or time, formats from `templates.json` (unit `templates_insert_at_the_cursor`) |
| Note composer | Done | merge the current note into another, extract the selection, and Extract this heading (the section under the cursor's heading moves to a new note named after it, an embed left in its place) (units `note_composer_merges_and_extracts`, `extract_heading_attachments_and_deleting`) |
| File recovery | Done | a snapshot of a note on save at most every `intervalMinutes`, kept `keepDays`, in the app's own store (as Obsidian keeps them outside the vault); "Open local history" lists them, shows one, Restore (keeping the current text as a snapshot); Clear history (units `recovery::*`, `recovery_keeps_copies_and_restores`) |
| Page preview (hover a link) | Done | see "Editor and rendering" below |
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
plugins, sync, publish, PDF export.

## Runtime holes found while building it

Filed in BUGS.md, unclaimed:

- **K-1334** (runtime-hole): no folder of more than 4,096 entries can be listed.
- **K-1335** (our-code): `fs::write` fails over 8 MiB; `read_to_end` reads 8 KiB a call.
- **K-1336** (runtime-hole): a picked folder is forgotten when the app closes (fixed since).
- **K-1337** (runtime-hole): no call moves a file to the computer's trash (the lead's call is on `next`; Vault's `trash_file` hook switches to it at merge).
- **K-1341** (runtime-hole): canvas2d draws no turned text (stacked tab spines).
- **K-1342** (runtime-hole): no way to show a PDF.
- **K-1343** (runtime-hole): sound and video in a picked folder cannot be played (fixed since: they play, see Embeds: sound and video).
- **K-1344** (runtime-hole): one window per app, so no pop-out windows.
- **K-1346** (runtime-hole): `video.open` cannot play a file the app reads in its own sandbox (only https, a picked folder or bundled assets).

## Editor and rendering, against Obsidian 1.15.1 side by side

Checked 2026-11-22 against Obsidian 1.15.1 itself (not its documentation):
the same notes (`render-vault/`, copied into a generated light vault) opened
in an isolated Obsidian profile and captured with `screencapture -l`, and in
Vault headless with `krate run --task` steps that wait and `shoot`. Each
picture in `screens/parity/` is Obsidian's note pane on the left and
Vault's on the right. *unit* names a native test in `apps/krate-vault`
(152 tests in all).

| Feature | State | How verified / what differs |
|---|---|---|
| Tables (GFM pipes) | Done | drawn with 1 px borders, 8 by 3 px cell padding, bold header, alignment, inline Markdown and links in cells, columns sized to content (parity/table-live, table-reading); edited inside the drawn table as Obsidian 1.15 does: the caret in a cell, Tab and Shift+Tab cell to cell (past the last cell adds a row), Enter down a row (past the last adds one), Up and Down keep the column, a typed `\|` is escaped, deleting stops at a cell's edge; a right-click on a cell opens the table menu (add, move, duplicate, delete rows and columns; align; sort) (unit `tables::tests`; headless task: arrows into the table, typed into cells, Tab added a row: screens/parity/vault-table-edit.png) |
| Callouts | Done | every Obsidian type and alias, its colour, a tenth of the colour behind, title or the type's name, `+`/`-` folding, Markdown and lists inside (unit `callouts_read_their_head`; parity/callouts-live, callouts-reading); the icons are Lucide's own shapes (ISC, LUCIDE-LICENSE.txt), as Obsidian's are |
| Embeds: notes | Done | `![[Note]]` (headed by its name), `![[Note#Heading]]` (the section), `![[Note#^block]]` (the block), with the accent bar, their own tables, callouts, maths and pictures drawn; an embed inside an embed shows as its link (unit `sections_and_blocks_are_found`, `embeds_name_notes_pictures_and_sizes`; parity/embeds-notes-live). Obsidian's open-in-place icon on hover is not drawn |
| Embeds: pictures | Done | `![[pic.png]]`, `|300`, `|300x200`, `![alt](path)`, found beside the note, at the root or in any folder; PNG, JPEG, BMP, GIF and WebP (lossy, lossless, animated: image-webp's decoder ported to no_std) and SVG (the app's own reader and rasteriser: paths, shapes, transforms, gradients; text drawn with the canvas's fonts) decoded in the app; GIFs and animated WebPs play, each frame at its own time; `![alt](https://...)` is fetched (optional `net.connect:*:443`) (unit `image::tests`, `webp::tests`, `svg::tests`; parity/pictures-live). AVIF cannot be decoded in an app (K-1345). SVG filters, masks, clip paths, `<use>` and CSS sheets are not read. Some hosts refuse Krate's request (Wikimedia answers 400 to any client without a contact address in its User-Agent) |
| Embeds: sound and video | Done | `![[clip.mp4]]` and `![[song.mp3]]` (mp4, webm, ogv, mov, mkv; mp3, wav, m4a, ogg, flac, 3gp: Obsidian's formats, and aac and m4v) play in the note through `krate:gfx/video` with the controls Obsidian shows: play and pause, "0:00 / 0:03", a track to jump along, mute, the three dots; a video at its own size up to the column with the bar over its foot, a sound as a bar across the column; a press on the picture plays or pauses (unit `embeds_name_notes_pictures_and_sizes`; headless task in a picked folder: pressed "Play or pause video", the frame and clock moved to 0:02, then muted and played the sound: parity/media-live, screens/parity/vault-media-playing.png). Obsidian's video bar also has a full-screen button and its menu (speed, download, picture in picture); Vault's three dots draw but open nothing. Plays only in a folder the person picked: a vault given by path in the sandbox (`vault=./vault`) cannot reach `video.open`, which answers "the app does not carry bundled assets", and the note says Krate could not open it (K-1346). Mac and phones only, as `krate:gfx/video` is |
| Math | Done | `$...$` inline and `$$...$$` blocks set in a serif face by the app's own TeX layout: fractions (`\frac`, `\dfrac`, `\binom`), sub- and superscripts, Greek, relations and operators, `\sum`/`\prod`/`\lim` with limits in display and beside inline, integrals, `\sqrt` and `\sqrt[n]`, `pmatrix`/`bmatrix`/`vmatrix`/`cases`/`aligned`, growing `\left( \right)`, accents, `\text`, `\mathbb`, spacing; what it cannot read shows in red, as KaTeX does (unit `math::tests`, 6 tests; parity/math-live). Glyph shapes are the system serif, not KaTeX's fonts |
| Properties | Done | the Properties block for YAML front matter: text, list, number, checkbox, date, date and time, tags, aliases, with Lucide's icons and the 148 px key column, edited inside the block as Obsidian does: a value or name opens a field where it is drawn (Enter or a press elsewhere keeps it, Tab goes on), lists are chips with a cross and a field to add more, a date opens a month to pick from, the icon opens the type menu (Text, List, Number, Checkbox, Date, Date & time, Remove), Add property opens the new name; written back as YAML, one undo each (unit `propedit::tests`, `values_write_back_as_yaml`; headless task: changed a status, added an alias, opened the date picker, added a property: parity/properties-live, screens/parity/vault-properties-edit.png). Obsidian keeps the type a person picks in .obsidian/types.json; Vault infers it from the value |
| Footnotes | Done | `[^1]` and `^[inline]` small and raised while editing; reading view numbers them by first reference and lists them after the note with the way back (unit `footnotes_number_in_reference_order`; parity/inline-live, inline-reading). The way-back arrow is not clickable |
| Highlights, strikethrough, inline HTML | Done | `==x==`, `~~x~~`, `<u>`, `<mark>`, `<sub>`, `<sup>`, `<kbd>`, `<b>`/`<strong>`, `<i>`/`<em>`, `<s>`/`<del>`, `<br>` and `<span>` (tags hidden, text styled) (unit `math_footnotes_comments_html_and_block_ids`). Other HTML (tables, `<div>`, `<img>`, `<details>`) shows as text |
| `%%comments%%` | Done | dimmed while editing, gone in reading view, inline and as `%%` blocks (unit same, `spans_are_found_from_any_of_their_lines`) |
| Horizontal rules, blockquotes, nested and task lists | Done | task states `[ ]`, `[x]`, `[/]`, `[-]` and any other mark drawn in the box; Obsidian's list geometry (32 px a level, text 31 px in, numbers right-aligned in the gutter, 0.075em spacing above and below each item, guides under nested items) measured side by side (parity/inline-live); reading view strikes the children of a done task, as Obsidian does |
| Block ids `^id` | Done | small and dimmed while editing, hidden in reading view |
| Code blocks | Done | coloured by language with Obsidian's dark and light code colours: Rust, JavaScript/TypeScript, Python, shell, C/C++/Java/Go/C#/Swift/Kotlin, SQL, JSON/YAML/TOML, CSS; the language's name in the corner (unit `code_lines_are_coloured_by_language`; parity/code-mermaid-live) |
| Mermaid | Done (layout differs) | every kind Obsidian draws, by the app: flowcharts, sequence (lifelines, actors, notes, loop/alt frames), class (compartments, UML heads), state (start and end dots), ER (cardinalities), Gantt (dates, sections, done/active/crit), pie (legend), journey (scored), mindmap (both sides of the root), timeline (unit `charts::tests`, `flowcharts_rank_their_boxes`; parity/diagrams-1, -2, -3). The placement is the app's own ranked layout, not Mermaid's dagre, so a large graph is arranged differently; gitGraph, quadrant, sankey, XY and block diagrams stay a code block |
| Reading view | Done | its own layout (see the row above); typography spacing measured against Obsidian's: paragraph gaps of 1 em, headings 1.618/1.462/1.318/1.188/1.076 em |
| Heading links show as "Note > Heading" | Done | parity/links-live |
| Page preview | Done | a link hovered with Cmd held in editing view, or plainly in reading view (Obsidian's defaults), shows the note with its blocks drawn in a popover under the link (`preview=Table` start argument for a headless picture: screens/parity/vault-page-preview.png). Obsidian also previews search results; Vault only links in the note |
| Tag click opens search | Done | unit `search_runs_as_a_job` (tag: query) |
| External links | Done | open in the browser through `ui.open-url` |
| Find and replace in the note | Done | Cmd+F opens the bar with the selection, matches highlighted, "n / total", Enter/Shift+Enter and the arrows step, Escape closes; Cmd+Option+F or the bar's button adds the replace row, Replace and Replace all (one undo); match case, whole word and regular expression toggles (Aa, ab, .*, or Option+C/W/R in the bar) with a JavaScript-flavoured regex engine and `$1`/`$&` in replacements (unit `find::tests`, `regex::tests`; screens/parity/vault-find-replace.png) |
| Several carets | Done | Option-click adds or removes a caret, Cmd+D selects the word then each next occurrence, Cmd+Option+Up/Down adds a caret above or below; typing, Backspace, Delete, Enter, the arrows (Up and Down too), Home and End act at every caret as one undo; Escape keeps one (unit `several_carets_type_together`) |
| Folding | Done | headings (to the next heading at their level) and list items (their children) fold from a chevron in the margin, shown on hover and while folded, with "..." after a folded line; Cmd+Option+[ and ] fold and unfold, with Shift all at once; folds follow edits (unit `headings_and_lists_fold`) |
| Smart lists | Done | (earlier: `lists_continue_and_end_on_enter`) |
| Drag text | Done | a press inside the selection drags the text and drops it where released, one undo (unit `selected_text_moves_by_dragging`); not dragged in a real window |
| Spell check | Done | the computer's own checker (`krate:ui/spelling`, K-1340 fixed) marks unknown words with a red wavy underline in live and source mode, not in reading view; code, wiki link targets, web addresses, tags, maths, HTML tags, block ids, footnote labels and front matter keys are never marked, a link's shown text and front matter values are; only lines on screen are checked, each cached by its text and checked again when it changes; right-click on a marked word lists up to five suggestions, then Add to dictionary (the checker learns it) and Ignore (for the session); app.json `"spellcheck": false` turns it off (unit `only_prose_is_checked`, `front_matter_checks_values_not_keys`, `the_menu_replaces_or_queues`; headless task: Teh, jumpd, alais and tsak underlined once the note is clicked, nothing before: parity/spelling-live). Values inside the Properties block are not marked (Obsidian marks them while one is being edited). Our isolated Obsidian draws no marks in a capture even focused and after typing (its window is never the key window), so the mark's look is matched to Chromium's, not measured |

A note opens with the editor unfocused, as Obsidian opens it: no caret and
no line showing its syntax until it is clicked or a key is pressed (the
heading of a fresh note shows no `#`). Settings > Show inline title and
Show line numbers are followed (the gutter's numbers at each line's first
row).

Performance with all of this on: the 200,000-line note (debug runtime,
`krate run ... --task` that waits 1.5 s, Cmd+Down, types, saves):

| | before (b689eb95f) | round 1 | round 2 | round 3 (eb4e38b79 / a17a1d458, same load) |
|---|---|---|---|---|
| note read and parsed (`t_note_open_ms`) | 160, 163 | 171, 199 | 176, 175 | 209, 238 / 187, 208 |
| first frame (`t_first_frame_ms`) | 766, 777 | 763, 810 | 726, 719 | 757, 788 / 735, 757 |
| lines laid out in the whole run | 44 | 45 | 44 | 44 / 44 |
| saved after typing | yes | yes | yes | yes / yes |

Round 3 ran with the machine's load average near 4.5 (other builds), so
it was measured beside eb4e38b79 built the same way, alternating runs;
spell check adds nothing at open (nothing is checked until the note is
clicked) and checks only the lines laid out after that.

The extra time at open is finding the `$$`, `%%` and front matter lines
once; nothing else scans the note.

Not checked here: anything in a real window (every run above was
headless), Obsidian's light theme side by side, and the light-theme code
colours.
