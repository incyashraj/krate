//! Krate Checklist — a second real GUI sample, model-authorable.
//!
//! A checklist that is one shareable file: a list of items, each with a
//! checkbox, an "Add item" button, and a status line. Toggling an item or
//! adding one saves to a file in the directory the user granted. Nothing else
//! on the machine is reachable.
//!
//! This exists so `krate create` has a real GUI app beyond the word-frequency
//! CLI sample — one that exercises UI, editing state, files, and permissions
//! together. It is deliberately built to the same discipline as krate-notes:
//! a Krate component may import only `krate:*`, so every buffer is fixed
//! capacity and every access non-panicking (a growable `Vec`, `HashMap`, or
//! `format!` would pull the `wasi:*` import set in, and LTO cannot strip it).

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const HEADER_ID: u64 = 2;
const STATUS_ID: u64 = 3;
/// The text field where a new item is typed before it is added.
const NEW_ITEM_FIELD_ID: u64 = 4;
const ITEM_ROW_BASE_ID: u64 = 10;

/// How many checklist items the app can ever hold. Fixed so nothing allocates.
const MAX_ITEMS: usize = 32;
/// The "+ Add item" row sits just past the last possible item slot, so its id
/// never collides with an item row.
const ADD_ITEM_ID: u64 = ITEM_ROW_BASE_ID + MAX_ITEMS as u64;

/// Bytes of text one item can hold. Fixed for the same no-allocation reason.
const ITEM_TEXT_CAP: usize = 128;
/// The file the checklist is saved to, inside the granted directory.
/// The one key this app keeps its items under.
const DATA_KEY: &str = "items";

/// The items seeded on the very first run, so a fresh open is not empty.
const SEED_ITEMS: [&str; 3] = ["Buy milk", "Write the pitch", "Ship the demo"];

/// Interactive runs stay open until the person closes the window; automated
/// runs pass `quick` and exit promptly.
const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 50;

/// Consecutive quiet rounds before the app stops waiting, about ten seconds.
///
/// On a desktop the window itself is the reason to keep waiting: the person can
/// always come back and click. With no window there is nothing to come back to,
/// and every round is guaranteed to stay quiet, so waiting out `MAX_WAIT_ROUNDS`
/// would sit there for over eight hours looking like a hang. A person who is
/// actually using the window resets this on their first click or keystroke, so
/// the bound is only ever reached when nothing is there at all.
const MAX_IDLE_ROUNDS: u32 = 200;

struct Component;

/// One checklist item: its text (fixed capacity), whether it is done, and
/// whether the slot is in use. Copyable so the list is a plain fixed array.
#[derive(Clone, Copy)]
struct Item {
    text: [u8; ITEM_TEXT_CAP],
    text_len: usize,
    done: bool,
    used: bool,
}

impl Item {
    const EMPTY: Item = Item {
        text: [0; ITEM_TEXT_CAP],
        text_len: 0,
        done: false,
        used: false,
    };

    fn text_str(&self) -> &str {
        let slice = self.text.get(..self.text_len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn set_text(&mut self, text: &str) {
        self.text_len = 0;
        for byte in text.as_bytes() {
            if let Some(slot) = self.text.get_mut(self.text_len) {
                *slot = *byte;
                self.text_len += 1;
            }
        }
    }
}

/// The whole checklist: a fixed array of items plus how many slots are live.
struct Checklist {
    items: [Item; MAX_ITEMS],
    len: usize,
}

impl Checklist {
    const fn new() -> Self {
        Self {
            items: [Item::EMPTY; MAX_ITEMS],
            len: 0,
        }
    }

    fn push(&mut self, text: &str, done: bool) {
        if let Some(slot) = self.items.get_mut(self.len) {
            slot.set_text(text);
            slot.done = done;
            slot.used = true;
            self.len += 1;
        }
    }

    fn toggle(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            if item.used {
                item.done = !item.done;
            }
        }
    }
}

/// The text of the new item being typed, before it is added. A fixed buffer so
/// nothing allocates; append and pop only, like the notes editor's buffer.
struct Draft {
    text: [u8; ITEM_TEXT_CAP],
    len: usize,
}

impl Draft {
    const fn new() -> Self {
        Self {
            text: [0; ITEM_TEXT_CAP],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        let slice = self.text.get(..self.len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Append one printable byte; ignored at capacity.
    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.text.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// Replace the whole draft, used when a native control reports its full
    /// text after any edit.
    fn set(&mut self, text: &str) {
        self.len = 0;
        for byte in text.as_bytes() {
            let printable = byte.is_ascii_graphic() || *byte == b' ';
            if printable {
                self.push(*byte);
            }
        }
    }
}

/// Build an owned `String` without touching std's allocation-error handler,
/// which would drag the `wasi:*` import set into the component. Mirrors the
/// raw-allocation path the generated bindings use.
fn pure_string(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            std::process::abort();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

/// Render a small integer as decimal digits without `format!`.
fn number_string(mut value: u32) -> String {
    if value == 0 {
        return pure_string("0");
    }
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    while value > 0 {
        if let Some(slot) = digits.get_mut(len) {
            *slot = b'0' + (value % 10) as u8;
            len += 1;
        }
        value /= 10;
    }
    let mut buf = [0u8; 10];
    let mut buf_len = 0usize;
    for i in (0..len).rev() {
        if let (Some(src), Some(dst)) = (digits.get(i), buf.get_mut(buf_len)) {
            *dst = *src;
            buf_len += 1;
        }
    }
    pure_string(core::str::from_utf8(buf.get(..buf_len).unwrap_or(&[])).unwrap_or("0"))
}

// ---- widget tree ----------------------------------------------------------

fn stack_root() -> types::WidgetNode {
    node(
        ROOT_ID,
        None,
        types::WidgetKind::Stack,
        None,
        None,
        440.0,
        560.0,
        0.0,
        20.0,
    )
}


fn header() -> types::WidgetNode {
    let mut n = node(
        HEADER_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some(pure_string("Checklist")),
        Some(pure_string("heading")),
        388.0,
        30.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

/// One checklist item as a checkbox row: label is the item text, `checked`
/// mirrors whether it is done. Clicking it toggles the item.
fn item_row(index: usize, item: &Item) -> types::WidgetNode {
    let mut n = node(
        ITEM_ROW_BASE_ID + index as u64,
        Some(ROOT_ID),
        types::WidgetKind::Checkbox,
        Some(pure_string(item.text_str())),
        Some(pure_string("checkbox")),
        392.0,
        34.0,
        0.0,
        0.0,
    );
    n.checked = Some(item.done);
    n
}

/// The text field for a new item. Its label is the draft text so far; a native
/// host lowers it to a real editable control, and a drawn host paints it and
/// feeds typed characters back through TextInput/Key events.
fn new_item_field(draft: &Draft) -> types::WidgetNode {
    let mut n = node(
        NEW_ITEM_FIELD_ID,
        Some(ROOT_ID),
        types::WidgetKind::TextField,
        Some(pure_string(draft.as_str())),
        Some(pure_string("textbox")),
        388.0,
        30.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("textbox"));
    n
}

/// The "+ Add item" affordance below the new-item field.
fn add_item_row() -> types::WidgetNode {
    node(
        ADD_ITEM_ID,
        Some(ROOT_ID),
        types::WidgetKind::Button,
        Some(pure_string("+ Add item")),
        Some(pure_string("button")),
        388.0,
        30.0,
        0.0,
        0.0,
    )
}

fn status(text: &str) -> types::WidgetNode {
    let mut n = node(
        STATUS_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some(pure_string(text)),
        Some(pure_string("status")),
        388.0,
        24.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("status"));
    n
}

/// Small constructor so every node above stays one readable line.
#[allow(clippy::too_many_arguments)]
fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    label: Option<String>,
    role: Option<String>,
    width: f32,
    height: f32,
    grow: f32,
    padding: f32,
) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label,
        role,
        style: types::Style {
            width: Some(width),
            height: Some(height),
            grow,
            padding,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn is_item_row(widget: Option<u64>) -> Option<usize> {
    let id = widget?;
    if id < ITEM_ROW_BASE_ID || id >= ADD_ITEM_ID {
        return None;
    }
    let index = (id - ITEM_ROW_BASE_ID) as usize;
    (index < MAX_ITEMS).then_some(index)
}

// ---- persistence ----------------------------------------------------------

/// Load the checklist from the granted file. Each line is `[x] text` (done) or
/// `[ ] text` (not done). A missing file is an empty checklist, not an error.
/// Load the checklist from the app's own store.
///
/// The store, not a file: the app names a key and never a path, so it needs no
/// filesystem grant and the permission prompt says "save its own settings and
/// data" rather than naming a folder. This replaced about a hundred lines of
/// hand-written buffering and parsing that every app would otherwise repeat.
fn load(list: &mut Checklist) -> bool {
    *list = Checklist::new();
    let Ok(Some(data)) = store_kv::get(DATA_KEY) else {
        return false;
    };
    // Split on newlines by hand. `str::lines` and a growable `String` would
    // reach std's allocation-error handler, which drags the whole `wasi:*`
    // import set into an otherwise pure component.
    let mut start = 0usize;
    for i in 0..data.len() {
        if data.get(i).copied() == Some(b'\n') {
            parse_line(data.get(start..i).unwrap_or(&[]), list);
            start = i + 1;
        }
    }
    if start < data.len() {
        parse_line(data.get(start..).unwrap_or(&[]), list);
    }
    true
}

fn parse_line(line: &[u8], list: &mut Checklist) {
    if line.len() < 4 {
        return;
    }
    // Expect "[x] " or "[ ] " prefix.
    let done = line.get(1).copied() == Some(b'x');
    let text = line.get(4..).unwrap_or(&[]);
    if let Ok(text) = core::str::from_utf8(text) {
        if list.len < MAX_ITEMS {
            list.push(text, done);
        }
    }
}

/// Save the checklist, one `[x]/[ ] text` line each.
///
/// The same on-disk shape as before, so a person who looks at their data still
/// sees something readable -- what changed is that the app no longer needs
/// access to a folder to keep it.
fn save(list: &Checklist) -> bool {
    // A fixed buffer for the same no-allocation reason as `load`.
    let mut out = [0u8; MAX_ITEMS * (ITEM_TEXT_CAP + 8)];
    let mut len = 0usize;
    let mut push = |bytes: &[u8], out: &mut [u8], len: &mut usize| {
        for byte in bytes {
            if let Some(slot) = out.get_mut(*len) {
                *slot = *byte;
                *len += 1;
            }
        }
    };
    for i in 0..list.len {
        let Some(item) = list.items.get(i) else {
            continue;
        };
        if !item.used {
            continue;
        }
        push(
            if item.done { b"[x] " } else { b"[ ] " },
            &mut out,
            &mut len,
        );
        push(item.text_str().as_bytes(), &mut out, &mut len);
        push(b"\n", &mut out, &mut len);
    }
    store_kv::set(DATA_KEY, out.get(..len).unwrap_or(&[])).is_ok()
}

// ---- the app --------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: 440,
            height: 560,
        };
        let Ok(win) = window::create("Checklist", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        let mut list = Checklist::new();
        if !load(&mut list) || list.len == 0 {
            for seed in SEED_ITEMS {
                list.push(seed, false);
            }
        }
        let mut draft = Draft::new();

        // A quick automated run types an item, adds it, toggles the first item,
        // and saves — proving the text-entry + edit + save path in CI.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if !rebuild(win, &list, &draft, "Type an item, then Add") {
            let _ = window::close(win);
            return 32;
        }

        let mut saved_any = false;
        let mut close_requested = false;

        // Commit the draft as a new item if there is room and text. Returns
        // whether it saved.
        let commit_draft = |list: &mut Checklist, draft: &mut Draft| -> bool {
            if draft.is_empty() || list.len >= MAX_ITEMS {
                return false;
            }
            list.push(draft.as_str(), false);
            draft.clear();
            save(list)
        };

        if quick {
            // The automated verification path: exercise add + toggle + save,
            // then exit immediately. It must NOT enter the event-wait loop —
            // waiting on window events during a headless/verify run is exactly
            // what makes create's verification hang. Prove the paths and return.
            draft.set("Buy coffee");
            if commit_draft(&mut list, &mut draft) {
                saved_any = true;
            }
            list.toggle(0);
            if save(&list) {
                saved_any = true;
            }
            let _ = rebuild(win, &list, &draft, "saved");
            let _ = window::close(win);
            let out = stdio::stdout();
            let _ = out.write(b"items:");
            let _ = out.write(number_string(list.len as u32).as_bytes());
            let _ = out.write(b"\n");
            if saved_any {
                let _ = out.write(b"saved:yes\n");
            }
            return 0;
        }

        let rounds = MAX_WAIT_ROUNDS;
        let mut idle_rounds = 0u32;
        for _ in 0..rounds {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            // Quiet rounds are normal while someone reads the window, but an
            // unbroken run of them means no one is there to click at all.
            if event.is_none() {
                idle_rounds += 1;
                if idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            match event {
                // Toggling an item flips its done state and saves.
                Some(types::Event::Pointer(pointer))
                    if pointer.pressed && is_item_row(pointer.widget).is_some() =>
                {
                    if let Some(index) = is_item_row(pointer.widget) {
                        list.toggle(index);
                        if save(&list) {
                            saved_any = true;
                        }
                        let _ = rebuild(win, &list, &draft, "saved");
                    }
                }
                // "+ Add item" commits whatever is typed in the field.
                Some(types::Event::Pointer(pointer))
                    if pointer.pressed && pointer.widget == Some(ADD_ITEM_ID) =>
                {
                    if list.len >= MAX_ITEMS {
                        let _ = tree::upsert_node(win, &status("checklist full"));
                    } else if commit_draft(&mut list, &mut draft) {
                        saved_any = true;
                        let _ = rebuild(win, &list, &draft, "added");
                    }
                }
                // A native control (macOS) reports the field's whole text after
                // any edit. Mirror it into the draft without re-lowering, which
                // would fight the control being typed into.
                Some(types::Event::TextChanged(changed)) if changed.widget == NEW_ITEM_FIELD_ID => {
                    draft.set(&changed.text);
                }
                // A drawn host (Linux, Windows) sends typed characters; append
                // the printable ones to the draft and re-lower to show them.
                Some(types::Event::TextInput(text)) => {
                    for byte in text.as_bytes() {
                        let printable = byte.is_ascii_graphic() || *byte == b' ';
                        if printable {
                            draft.push(*byte);
                        }
                    }
                    let _ = rebuild(win, &list, &draft, "typing");
                }
                Some(types::Event::Key(key)) if key.pressed => {
                    if key.key.as_bytes() == b"Backspace" {
                        draft.pop();
                        let _ = rebuild(win, &list, &draft, "typing");
                    } else if key.key.as_bytes() == b"Enter" {
                        if list.len >= MAX_ITEMS {
                            let _ = tree::upsert_node(win, &status("checklist full"));
                        } else if commit_draft(&mut list, &mut draft) {
                            saved_any = true;
                            let _ = rebuild(win, &list, &draft, "added");
                        }
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    close_requested = true;
                    break;
                }
                _ => {}
            }
        }

        let _ = window::close(win);

        let out = stdio::stdout();
        let _ = out.write(b"items:");
        let _ = out.write(number_string(list.len as u32).as_bytes());
        let _ = out.write(b"\n");
        if saved_any {
            let _ = out.write(b"saved:yes\n");
        }

        if close_requested {
            2
        } else {
            0
        }
    }
}

/// Rebuild the whole tree: root, header, one row per live item, the new-item
/// text field, the add button, and the status line. Returns false if any
/// upsert fails.
fn rebuild(win: u64, list: &Checklist, draft: &Draft, status_text: &str) -> bool {
    // Root, then header, then the scrollable list container. Item rows are
    // children of the list container, so a growing list scrolls inside it while
    // the input row and status line below always stay on screen.
    if tree::set_root(win, &stack_root()).is_err() || tree::upsert_node(win, &header()).is_err() {
        return false;
    }
    for i in 0..list.len {
        let Some(item) = list.items.get(i) else {
            continue;
        };
        if !item.used {
            continue;
        }
        if tree::upsert_node(win, &item_row(i, item)).is_err() {
            return false;
        }
    }
    tree::upsert_node(win, &new_item_field(draft)).is_ok()
        && tree::upsert_node(win, &add_item_row()).is_ok()
        && tree::upsert_node(win, &status(status_text)).is_ok()
}

bindings::export!(Component with_types_in bindings);
