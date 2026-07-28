//! Krate Notes — the flagship sample (Phase-3-Plan §17).
//!
//! A note taking app that is one shareable file. A list of notes on the left,
//! an editor on the right, and saving writes to a directory the user granted.
//! Nothing else on the machine is reachable.
//!
//! This exists because a widget gallery proves a mechanism and a real app
//! proves a product. Someone can be sent a link to this, open it, see exactly
//! what it wants, allow one folder, and keep using it.
//!
//! Panic-free discipline, inherited from hello-gui: indexed slice operations
//! pull std's panic machinery (and with it WASI imports) into the component,
//! so every buffer here is fixed capacity and every access non-panicking.

#[allow(warnings)]
mod bindings;

use bindings::krate::fs::files::{self, OpenMode};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{clipboard, events, tree, types, window};

const ROOT_ID: u64 = 1;
/// The left column wraps a header label and the note list. Child order is
/// BTreeMap id order, so the left column's id must sort before the editor
/// column's for the sidebar to sit on the left.
const LEFT_COLUMN_ID: u64 = 2;
const EDITOR_ID: u64 = 3;
const STATUS_ID: u64 = 4;
const EDITOR_COLUMN_ID: u64 = 5;
const SIDEBAR_HEADER_ID: u64 = 6;
const SIDEBAR_ID: u64 = 7;
const NOTE_ROW_BASE_ID: u64 = 10;
/// The "+ New note" row lives just past the last possible note slot, so its id
/// never collides with a note row and `is_note_row` can reject it cleanly.
const NEW_NOTE_ID: u64 = NOTE_ROW_BASE_ID + NOTE_CAPACITY_SLOTS as u64;

/// Total note slots the sample can ever hold. Fixed so no allocation is needed;
/// only the first `live` of them are shown, and "+ New note" reveals the next.
const NOTE_CAPACITY_SLOTS: usize = 8;
/// How many notes exist when the app first opens.
const INITIAL_NOTE_COUNT: usize = 3;
const NOTE_TITLES: [&str; NOTE_CAPACITY_SLOTS] = [
    "first note",
    "second note",
    "third note",
    "fourth note",
    "fifth note",
    "sixth note",
    "seventh note",
    "eighth note",
];
const NOTE_FILES: [&str; NOTE_CAPACITY_SLOTS] = [
    "./notes/first.txt",
    "./notes/second.txt",
    "./notes/third.txt",
    "./notes/fourth.txt",
    "./notes/fifth.txt",
    "./notes/sixth.txt",
    "./notes/seventh.txt",
    "./notes/eighth.txt",
];

/// Bytes of note text the editor holds. A real editor would grow; a sample
/// that must not pull panic machinery into the component does not.
const NOTE_CAPACITY: usize = 512;

/// Rounds the interactive session runs for. A note taking app should stay
/// open until the person closes it, not time out while they are thinking, so
/// this is a very high ceiling rather than a demo budget: about eight hours at
/// the round length below. `hello-gui` uses a short bound because it is a CI
/// fixture that must never hang; this is an app someone uses.
const MAX_WAIT_ROUNDS: u32 = 600_000;
/// Automated runs pass `quick` and exit promptly.
const QUICK_WAIT_ROUNDS: u32 = 40;
const WAIT_ROUND_MILLIS: u32 = 50;

/// Consecutive quiet rounds before the app stops waiting, about ten seconds.
///
/// The ceiling above assumes a window someone can come back to and click. With
/// no window there is nothing to come back to and every round is guaranteed to
/// stay quiet, so waiting the ceiling out sits there for eight hours looking
/// like a hang. Any click or keystroke resets this, so someone using the window
/// never reaches it.
const MAX_IDLE_ROUNDS: u32 = 200;

struct Component;

/// How many edit states undo/redo can walk back through. Fixed so the editor
/// allocates nothing: the two stacks below are inline arrays. A note is small
/// and a demo session is short, so a shallow history is plenty; the point is
/// that undo works at all on the drawn path, not that it is unbounded.
const HISTORY_DEPTH: usize = 32;

/// One captured edit state: the full text plus where the cursor and selection
/// sat. Undo restores all three so the caret lands back where the person was.
#[derive(Clone, Copy)]
struct Snapshot {
    bytes: [u8; NOTE_CAPACITY],
    len: usize,
    cursor: usize,
    anchor: usize,
}

/// A fixed-capacity, allocation-free, panic-free text editor.
///
/// The drawn path (Linux, Windows) paints its own text, so unlike the macOS
/// native control, every editing behavior — caret movement, selection, cut,
/// copy, paste, undo, redo — is implemented here in the guest against the raw
/// byte buffer. Text is treated as bytes: `TextInput` on the drawn path is
/// filtered to ASCII, so a byte index is always a character boundary and the
/// caret arithmetic below is exact.
struct NoteBuffer {
    bytes: [u8; NOTE_CAPACITY],
    len: usize,
    /// Caret position as a byte offset in `0..=len`.
    cursor: usize,
    /// The other end of the selection. Equal to `cursor` means no selection;
    /// otherwise the selection spans `[min, max)` of the two.
    anchor: usize,
    /// States to restore on undo, and the states redo can return to. Both are
    /// bounded ring-free stacks: pushing past the top drops the oldest state.
    undo: [Snapshot; HISTORY_DEPTH],
    undo_len: usize,
    redo: [Snapshot; HISTORY_DEPTH],
    redo_len: usize,
}

impl NoteBuffer {
    const fn new() -> Self {
        let empty = Snapshot {
            bytes: [0; NOTE_CAPACITY],
            len: 0,
            cursor: 0,
            anchor: 0,
        };
        Self {
            bytes: [0; NOTE_CAPACITY],
            len: 0,
            cursor: 0,
            anchor: 0,
            undo: [empty; HISTORY_DEPTH],
            undo_len: 0,
            redo: [empty; HISTORY_DEPTH],
            redo_len: 0,
        }
    }

    // ---- state used by load/save/quick-seed (unchanged call sites) ----

    /// Reset to an empty note. Used when loading a file or making a new note,
    /// which is a fresh document, so history is cleared too.
    fn clear(&mut self) {
        self.len = 0;
        self.cursor = 0;
        self.anchor = 0;
        self.undo_len = 0;
        self.redo_len = 0;
    }

    fn push_str(&mut self, text: &str) {
        for byte in text.as_bytes() {
            self.push(*byte);
        }
    }

    /// Append one byte at the end and keep the caret trailing it. Used by the
    /// file loader and the quick-run seed, which fill the buffer before the
    /// person starts editing, so this never touches history.
    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
            self.cursor = self.len;
            self.anchor = self.len;
        }
    }

    fn as_str(&self) -> &str {
        let slice = self.bytes.get(..self.len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    // ---- selection ----

    /// Selection bounds as `[start, end)` in byte offsets, start <= end.
    fn selection(&self) -> (usize, usize) {
        if self.cursor <= self.anchor {
            (self.cursor, self.anchor)
        } else {
            (self.anchor, self.cursor)
        }
    }

    fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// The selected text, or the empty string when nothing is selected. Copy
    /// and Cut hand this to the clipboard.
    fn selected_text(&self) -> &str {
        let (start, end) = self.selection();
        let slice = self.bytes.get(start..end).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.len;
    }

    // ---- caret movement. `extend` keeps the anchor to grow a selection. ----

    fn move_left(&mut self, extend: bool) {
        // Moving without extending collapses an existing selection to its left
        // edge rather than stepping past it — the behavior every editor has.
        if !extend && self.has_selection() {
            let (start, _) = self.selection();
            self.cursor = start;
        } else {
            self.cursor = self.cursor.saturating_sub(1);
        }
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_right(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let (_, end) = self.selection();
            self.cursor = end;
        } else {
            self.cursor = (self.cursor + 1).min(self.len);
        }
        if !extend {
            self.anchor = self.cursor;
        }
    }

    fn move_home(&mut self, extend: bool) {
        self.cursor = 0;
        if !extend {
            self.anchor = 0;
        }
    }

    fn move_end(&mut self, extend: bool) {
        self.cursor = self.len;
        if !extend {
            self.anchor = self.len;
        }
    }

    // ---- history ----

    /// Capture the current state onto the undo stack before a mutation, and
    /// drop any redo future — editing after an undo forks history, so the old
    /// redo path no longer applies.
    fn record(&mut self) {
        let snap = Snapshot {
            bytes: self.bytes,
            len: self.len,
            cursor: self.cursor,
            anchor: self.anchor,
        };
        push_snapshot(&mut self.undo, &mut self.undo_len, snap);
        self.redo_len = 0;
    }

    fn restore(&mut self, snap: Snapshot) {
        self.bytes = snap.bytes;
        self.len = snap.len;
        self.cursor = snap.cursor.min(snap.len);
        self.anchor = snap.anchor.min(snap.len);
    }

    /// Step back one edit. The current state is pushed onto redo first so it
    /// can be reached again. Returns whether anything changed.
    fn undo(&mut self) -> bool {
        let Some(prev) = pop_snapshot(&self.undo, &mut self.undo_len) else {
            return false;
        };
        let current = Snapshot {
            bytes: self.bytes,
            len: self.len,
            cursor: self.cursor,
            anchor: self.anchor,
        };
        push_snapshot(&mut self.redo, &mut self.redo_len, current);
        self.restore(prev);
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = pop_snapshot(&self.redo, &mut self.redo_len) else {
            return false;
        };
        let current = Snapshot {
            bytes: self.bytes,
            len: self.len,
            cursor: self.cursor,
            anchor: self.anchor,
        };
        push_snapshot(&mut self.undo, &mut self.undo_len, current);
        self.restore(next);
        true
    }

    // ---- editing ops (each records history first) ----

    /// Remove the current selection in place, leaving the caret where it began.
    /// Returns false when there was nothing selected.
    fn delete_selection_raw(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (start, end) = self.selection();
        let removed = end - start;
        // Shift the tail after the selection left over the hole it leaves.
        let mut i = end;
        while i < self.len {
            if let Some(src) = self.bytes.get(i).copied() {
                if let Some(slot) = self.bytes.get_mut(start + (i - end)) {
                    *slot = src;
                }
            }
            i += 1;
        }
        self.len = self.len.saturating_sub(removed);
        self.cursor = start;
        self.anchor = start;
        true
    }

    /// Insert text at the caret, replacing any selection first. Silently stops
    /// at capacity rather than growing or panicking.
    fn insert_str(&mut self, text: &str) {
        self.record();
        self.delete_selection_raw();
        for byte in text.as_bytes() {
            // The drawn host only forwards ASCII text input, but guard anyway:
            // a non-ASCII byte would break the byte-equals-char assumption.
            if !byte.is_ascii() {
                continue;
            }
            self.insert_byte(*byte);
        }
    }

    /// Insert one byte at the caret, shifting the tail right. No-op at capacity.
    fn insert_byte(&mut self, byte: u8) {
        if self.len >= NOTE_CAPACITY {
            return;
        }
        // Shift everything from the caret onward one slot to the right.
        let mut i = self.len;
        while i > self.cursor {
            if let Some(src) = self.bytes.get(i - 1).copied() {
                if let Some(slot) = self.bytes.get_mut(i) {
                    *slot = src;
                }
            }
            i -= 1;
        }
        if let Some(slot) = self.bytes.get_mut(self.cursor) {
            *slot = byte;
            self.len += 1;
            self.cursor += 1;
            self.anchor = self.cursor;
        }
    }

    /// Backspace: delete the selection if any, otherwise the byte before the
    /// caret.
    fn backspace(&mut self) {
        self.record();
        if self.delete_selection_raw() {
            return;
        }
        if self.cursor == 0 {
            // Nothing to delete; drop the snapshot we just took so a no-op
            // backspace does not consume an undo step.
            self.undo_len = self.undo_len.saturating_sub(1);
            return;
        }
        self.cursor -= 1;
        let mut i = self.cursor;
        while i + 1 < self.len {
            if let Some(src) = self.bytes.get(i + 1).copied() {
                if let Some(slot) = self.bytes.get_mut(i) {
                    *slot = src;
                }
            }
            i += 1;
        }
        self.len = self.len.saturating_sub(1);
        self.anchor = self.cursor;
    }

    /// Forward delete (the Delete key): the selection if any, else the byte at
    /// the caret.
    fn delete_forward(&mut self) {
        self.record();
        if self.delete_selection_raw() {
            return;
        }
        if self.cursor >= self.len {
            self.undo_len = self.undo_len.saturating_sub(1);
            return;
        }
        let mut i = self.cursor;
        while i + 1 < self.len {
            if let Some(src) = self.bytes.get(i + 1).copied() {
                if let Some(slot) = self.bytes.get_mut(i) {
                    *slot = src;
                }
            }
            i += 1;
        }
        self.len = self.len.saturating_sub(1);
        self.anchor = self.cursor;
    }

    /// Delete the current selection as one undoable step. Cut is Copy (which
    /// reads `selected_text` first) followed by this. No-op with no selection,
    /// and then it does not consume an undo step.
    fn delete_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        self.record();
        self.delete_selection_raw();
    }
}

/// Push a snapshot onto a bounded stack. At capacity the oldest state slides
/// out, so history stays shallow without ever allocating or panicking.
fn push_snapshot(stack: &mut [Snapshot; HISTORY_DEPTH], len: &mut usize, snap: Snapshot) {
    if *len < HISTORY_DEPTH {
        if let Some(slot) = stack.get_mut(*len) {
            *slot = snap;
            *len += 1;
        }
        return;
    }
    // Full: drop index 0 by shifting everything down, then write the newest.
    let mut i = 1;
    while i < HISTORY_DEPTH {
        if let Some(src) = stack.get(i).copied() {
            if let Some(dst) = stack.get_mut(i - 1) {
                *dst = src;
            }
        }
        i += 1;
    }
    if let Some(slot) = stack.get_mut(HISTORY_DEPTH - 1) {
        *slot = snap;
    }
}

fn pop_snapshot(stack: &[Snapshot; HISTORY_DEPTH], len: &mut usize) -> Option<Snapshot> {
    if *len == 0 {
        return None;
    }
    *len -= 1;
    stack.get(*len).copied()
}

/// Build an owned `String` without touching std's allocation-error handler.
///
/// `String::from` and `push_str` reference std's OOM handler, which drags the
/// whole `wasi:cli`/`wasi:io` import set into an otherwise pure component and
/// makes it unloadable by a runtime that only provides `krate:*`. This mirrors
/// the raw-allocation path the generated bindings use, trapping on allocation
/// failure instead.
fn pure_string(text: &str) -> String {
    pure_string_from_bytes(text.as_bytes())
}

fn pure_string_from_bytes(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            // On the wasm target, trap without pulling in std's allocation-error
            // handler (which drags the wasi import set into the component). Under
            // a native test build there is no such constraint, so abort plainly.
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            std::process::abort();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

/// Root is a Grid, which lays out as a row: sidebar on the left, editor
/// column on the right, the shape every note app has.
fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Grid,
        label: None,
        role: None,
        style: types::Style {
            width: Some(720.0),
            height: Some(480.0),
            grow: 0.0,
            padding: 16.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The right-hand column: editor above, status line below.
fn editor_column() -> types::WidgetNode {
    types::WidgetNode {
        id: EDITOR_COLUMN_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style {
            width: Some(460.0),
            height: Some(448.0),
            grow: 1.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The left column: a header label above the note list.
fn left_column() -> types::WidgetNode {
    types::WidgetNode {
        id: LEFT_COLUMN_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style {
            width: Some(200.0),
            height: Some(448.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The list's header. A plain label: its parent is the Stack, not the
/// ListView, so the host does not lower it as a clickable row.
fn sidebar_header() -> types::WidgetNode {
    types::WidgetNode {
        id: SIDEBAR_HEADER_ID,
        parent: Some(LEFT_COLUMN_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string("Notes")),
        role: Some(pure_string("heading")),
        style: types::Style {
            width: Some(200.0),
            height: Some(30.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The note list. Selection lives here, so the host paints the highlight.
fn sidebar(selected: Option<u32>) -> types::WidgetNode {
    types::WidgetNode {
        id: SIDEBAR_ID,
        parent: Some(LEFT_COLUMN_ID),
        kind: types::WidgetKind::ListView,
        label: None,
        role: Some(pure_string("listbox")),
        style: types::Style {
            width: Some(200.0),
            height: Some(418.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected,
        text_cursor: None,
    }
}

fn note_row(index: usize) -> types::WidgetNode {
    types::WidgetNode {
        id: NOTE_ROW_BASE_ID + index as u64,
        parent: Some(SIDEBAR_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string(
            NOTE_TITLES.get(index).copied().unwrap_or("note"),
        )),
        role: Some(pure_string("option")),
        style: types::Style {
            width: Some(200.0),
            height: Some(28.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The editor. A TextArea wraps and fills from the top, unlike a field.
///
/// `cursor` and `anchor` are byte offsets the drawn hosts use to paint the
/// caret and selection. The macOS native control owns its own caret and
/// ignores them, so passing them always is harmless there.
fn editor(buffer: &NoteBuffer) -> types::WidgetNode {
    types::WidgetNode {
        id: EDITOR_ID,
        parent: Some(EDITOR_COLUMN_ID),
        kind: types::WidgetKind::TextArea,
        label: Some(pure_string(buffer.as_str())),
        role: Some(pure_string("textbox")),
        style: types::Style {
            width: Some(460.0),
            height: Some(392.0),
            grow: 1.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: Some(types::TextCursor {
            cursor: buffer.cursor as u32,
            anchor: buffer.anchor as u32,
        }),
    }
}

fn status(text: &str) -> types::WidgetNode {
    types::WidgetNode {
        id: STATUS_ID,
        parent: Some(EDITOR_COLUMN_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string(text)),
        role: Some(pure_string("status")),
        style: types::Style {
            width: Some(460.0),
            height: Some(28.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The "+ New note" affordance at the bottom of the list. A clickable row, not
/// a note: selecting it reveals the next slot instead of loading a file.
fn new_note_row() -> types::WidgetNode {
    types::WidgetNode {
        id: NEW_NOTE_ID,
        parent: Some(SIDEBAR_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string("+ New note")),
        role: Some(pure_string("button")),
        style: types::Style {
            width: Some(200.0),
            height: Some(28.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn is_note_row(widget: Option<u64>) -> Option<usize> {
    let id = widget?;
    if id < NOTE_ROW_BASE_ID || id >= NEW_NOTE_ID {
        return None;
    }
    let index = (id - NOTE_ROW_BASE_ID) as usize;
    (index < NOTE_CAPACITY_SLOTS).then_some(index)
}

fn is_new_note_row(widget: Option<u64>) -> bool {
    widget == Some(NEW_NOTE_ID)
}

/// Load a note from the granted directory. A missing file is an empty note,
/// not an error: the first run of a fresh install has nothing saved yet.
fn load_note(index: usize, buffer: &mut NoteBuffer) -> bool {
    buffer.clear();
    let Some(path) = NOTE_FILES.get(index) else {
        return false;
    };
    let Ok(file) = files::open(path, OpenMode::Read) else {
        return false;
    };
    while let Ok(chunk) = file.read(NOTE_CAPACITY as u32) {
        if chunk.is_empty() {
            break;
        }
        for byte in &chunk {
            buffer.push(*byte);
        }
    }
    true
}

/// What a drawn-path key press did, so the run loop knows what to re-lower.
enum DrawnKeyOutcome {
    /// The key was not one this editor handles.
    Ignored,
    /// Text changed; re-lower the editor and mark the note dirty.
    Edited,
    /// Only the caret or selection moved; re-lower to repaint it.
    Moved,
    /// The note was saved to disk.
    Saved,
    /// A save was attempted but the write was refused.
    SaveDenied,
}

/// The command a drawn-path key press maps to, before any side effect runs.
///
/// Splitting translation from execution keeps the shortcut table — the part
/// that must behave the same on every host — a pure function of the key and
/// modifiers, with no clipboard, file, or binding calls in the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyAction {
    None,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Save,
    Backspace,
    DeleteForward,
    MoveLeft { extend: bool },
    MoveRight { extend: bool },
    MoveHome { extend: bool },
    MoveEnd { extend: bool },
}

/// Translate a key name and modifier state into an editing command.
///
/// This is the whole keyboard surface for the drawn hosts, and the one place
/// platform shortcuts are decided. Editing shortcuts key off Control (Linux,
/// Windows) or Meta/Command (macOS) via `chord`, so the byte-identical guest
/// honors each platform's native modifier without knowing which host it runs
/// on. Navigation keys (arrows, Home, End, Backspace, Delete) need no modifier;
/// Shift on them extends the selection.
fn classify_key(key: &str, control: bool, meta: bool, shift: bool) -> KeyAction {
    let chord = control || meta;
    if chord {
        return match key {
            "a" | "A" => KeyAction::SelectAll,
            "c" | "C" => KeyAction::Copy,
            "x" | "X" => KeyAction::Cut,
            "v" | "V" => KeyAction::Paste,
            // Redo is Cmd/Ctrl+Shift+Z; undo is the same chord without Shift.
            // Ctrl+Y is the Windows habit for redo. Check Shift first so Z does
            // not fall through to undo.
            "z" | "Z" if shift => KeyAction::Redo,
            "z" | "Z" => KeyAction::Undo,
            "y" | "Y" => KeyAction::Redo,
            "s" | "S" => KeyAction::Save,
            _ => KeyAction::None,
        };
    }
    match key {
        "Backspace" => KeyAction::Backspace,
        "Delete" => KeyAction::DeleteForward,
        "ArrowLeft" => KeyAction::MoveLeft { extend: shift },
        "ArrowRight" => KeyAction::MoveRight { extend: shift },
        "Home" => KeyAction::MoveHome { extend: shift },
        "End" => KeyAction::MoveEnd { extend: shift },
        _ => KeyAction::None,
    }
}

/// Run one drawn-path key press: translate it, then apply it to the buffer,
/// performing the clipboard and file side effects the command needs.
fn apply_drawn_key(buffer: &mut NoteBuffer, key: &types::KeyEvent, selected: usize) -> DrawnKeyOutcome {
    let action = classify_key(
        &key.key,
        key.modifiers.control,
        key.modifiers.meta,
        key.modifiers.shift,
    );
    match action {
        KeyAction::None => DrawnKeyOutcome::Ignored,
        KeyAction::SelectAll => {
            buffer.select_all();
            DrawnKeyOutcome::Moved
        }
        // Cut/Copy/Paste round-trip through the host clipboard. A note denied
        // `ui.clipboard` still types and saves — the calls just fail closed.
        KeyAction::Copy => {
            if buffer.has_selection() {
                let _ = clipboard::write_text(buffer.selected_text());
            }
            DrawnKeyOutcome::Ignored
        }
        KeyAction::Cut => {
            if buffer.has_selection() {
                let _ = clipboard::write_text(buffer.selected_text());
                buffer.delete_selection();
                DrawnKeyOutcome::Edited
            } else {
                DrawnKeyOutcome::Ignored
            }
        }
        KeyAction::Paste => {
            if let Ok(text) = clipboard::read_text() {
                if !text.is_empty() {
                    buffer.insert_str(&text);
                    return DrawnKeyOutcome::Edited;
                }
            }
            DrawnKeyOutcome::Ignored
        }
        KeyAction::Undo => {
            if buffer.undo() {
                DrawnKeyOutcome::Edited
            } else {
                DrawnKeyOutcome::Ignored
            }
        }
        KeyAction::Redo => {
            if buffer.redo() {
                DrawnKeyOutcome::Edited
            } else {
                DrawnKeyOutcome::Ignored
            }
        }
        KeyAction::Save => {
            if save_note(selected, buffer) {
                DrawnKeyOutcome::Saved
            } else {
                DrawnKeyOutcome::SaveDenied
            }
        }
        KeyAction::Backspace => {
            buffer.backspace();
            DrawnKeyOutcome::Edited
        }
        KeyAction::DeleteForward => {
            buffer.delete_forward();
            DrawnKeyOutcome::Edited
        }
        KeyAction::MoveLeft { extend } => {
            buffer.move_left(extend);
            DrawnKeyOutcome::Moved
        }
        KeyAction::MoveRight { extend } => {
            buffer.move_right(extend);
            DrawnKeyOutcome::Moved
        }
        KeyAction::MoveHome { extend } => {
            buffer.move_home(extend);
            DrawnKeyOutcome::Moved
        }
        KeyAction::MoveEnd { extend } => {
            buffer.move_end(extend);
            DrawnKeyOutcome::Moved
        }
    }
}

/// Save the editor buffer back to the granted directory.
fn save_note(index: usize, buffer: &NoteBuffer) -> bool {
    let Some(path) = NOTE_FILES.get(index) else {
        return false;
    };
    let Ok(file) = files::open(path, OpenMode::Write) else {
        return false;
    };
    let bytes = buffer.bytes.get(..buffer.len).unwrap_or(&[]);
    file.write(bytes).is_ok()
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: 720,
            height: 480,
        };
        let Ok(win) = window::create("Krate Notes", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        let mut selected: u32 = 0;
        let mut buffer = NoteBuffer::new();
        // A hint belongs on screen, not in the buffer: text seeded here would
        // be saved to the file as though the person had typed it.
        load_note(0, &mut buffer);

        // Detect the automation flag before building the tree so a quick run
        // can seed deterministic content and exit having provably saved it.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        if quick && buffer.as_str().is_empty() {
            buffer.push_str("saved by quick run");
        }

        // The editor column must exist before its children (editor, status).
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &left_column()).is_err()
            || tree::upsert_node(win, &sidebar_header()).is_err()
            || tree::upsert_node(win, &sidebar(Some(selected))).is_err()
            || tree::upsert_node(win, &editor_column()).is_err()
            || tree::upsert_node(win, &editor(&buffer)).is_err()
            || tree::upsert_node(win, &status("Cmd+S to save")).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // How many note slots are currently in use. Starts at the seeded
        // count, grows when "+ New note" is chosen, and re-discovers notes
        // created in earlier sessions by probing which files exist — a note
        // added yesterday must still be in the list today.
        let mut live: usize = INITIAL_NOTE_COUNT;
        while live < NOTE_CAPACITY_SLOTS {
            let Some(path) = NOTE_FILES.get(live) else {
                break;
            };
            if files::open(path, OpenMode::Read).is_err() {
                break;
            }
            live += 1;
        }
        let mut row = 0usize;
        while row < live {
            if tree::upsert_node(win, &note_row(row)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            row += 1;
        }
        if tree::upsert_node(win, &new_note_row()).is_err() {
            let _ = window::close(win);
            return 32;
        }

        let rounds = if quick {
            QUICK_WAIT_ROUNDS
        } else {
            MAX_WAIT_ROUNDS
        };

        let mut saved_any = false;
        let mut close_requested = false;
        // A quick run seeds content above and must save it on exit to prove
        // the write path in CI, so it starts dirty.
        let mut dirty = quick;

        let mut idle_rounds = 0u32;
        for _ in 0..rounds {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            // Quiet rounds are normal while someone reads or thinks, but an
            // unbroken run of them means nobody is there to type at all.
            if event.is_none() {
                idle_rounds += 1;
                if idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            match event {
                // Selecting a note saves the one being edited, then loads the
                // new one. Losing edits on click would be the first thing a
                // real user noticed.
                Some(types::Event::Pointer(pointer))
                    if pointer.pressed && is_note_row(pointer.widget).is_some() =>
                {
                    if let Some(index) = is_note_row(pointer.widget) {
                        if save_note(selected as usize, &buffer) {
                            saved_any = true;
                        }
                        selected = index as u32;
                        load_note(index, &mut buffer);
                        let _ = tree::upsert_node(win, &sidebar(Some(selected)));
                        let _ = tree::upsert_node(win, &editor(&buffer));
                        let _ = tree::upsert_node(win, &status("loaded"));
                        dirty = false;
                    }
                }
                // "+ New note" reveals the next unused slot, up to capacity. It
                // saves the note being edited first, then switches to a fresh
                // empty note so nothing in progress is lost.
                Some(types::Event::Pointer(pointer))
                    if pointer.pressed && is_new_note_row(pointer.widget) =>
                {
                    if live < NOTE_CAPACITY_SLOTS {
                        if save_note(selected as usize, &buffer) {
                            saved_any = true;
                        }
                        let index = live;
                        live += 1;
                        selected = index as u32;
                        buffer.clear();
                        let _ = tree::upsert_node(win, &note_row(index));
                        let _ = tree::upsert_node(win, &sidebar(Some(selected)));
                        let _ = tree::upsert_node(win, &editor(&buffer));
                        let _ = tree::upsert_node(win, &status("new note"));
                        dirty = false;
                    } else {
                        let _ = tree::upsert_node(win, &status("note list full"));
                    }
                }
                // A native control (macOS) owns its text and reports the whole
                // value after any edit, including deletes and pastes. Replace,
                // do not append, and do not re-lower the editor: the control
                // already shows the truth, and re-lowering would fight it.
                Some(types::Event::TextChanged(changed)) if changed.widget == EDITOR_ID => {
                    // Mirror the control's authoritative text and touch nothing
                    // else. Any upsert here would re-lower the whole tree,
                    // rebuilding the control being typed into and dropping
                    // characters. The editor shows the truth already; the status
                    // updates only on save.
                    buffer.clear();
                    for byte in changed.text.as_bytes() {
                        buffer.push(*byte);
                    }
                    dirty = true;
                }
                // A drawn host (Linux, Windows) sends committed characters and
                // relies on the guest to render them. Insert at the caret,
                // replacing any selection, and re-lower. Control combinations
                // (Ctrl+C and friends) arrive as Key events with no text, so
                // this only ever carries real typed characters.
                Some(types::Event::TextInput(text)) => {
                    let only_printable = text
                        .as_bytes()
                        .iter()
                        .all(|byte| byte.is_ascii_graphic() || *byte == b' ');
                    if only_printable && !text.is_empty() {
                        buffer.insert_str(&text);
                        dirty = true;
                        let _ = tree::upsert_node(win, &editor(&buffer));
                        let _ = tree::upsert_node(win, &status("editing"));
                    }
                }
                // The drawn path implements every editing behavior here, since
                // it paints its own text and has no native control to defer to.
                // Shortcuts fire on Control (Linux, Windows) or Meta/Command
                // (macOS) so the same guest works on every host.
                Some(types::Event::Key(key)) if key.pressed => {
                    match apply_drawn_key(&mut buffer, &key, selected as usize) {
                        DrawnKeyOutcome::Ignored => {}
                        DrawnKeyOutcome::Edited => {
                            dirty = true;
                            let _ = tree::upsert_node(win, &editor(&buffer));
                        }
                        DrawnKeyOutcome::Moved => {
                            // Caret and selection live in the guest; re-lower so
                            // the painter can show the new selection highlight.
                            let _ = tree::upsert_node(win, &editor(&buffer));
                        }
                        DrawnKeyOutcome::Saved => {
                            saved_any = true;
                            dirty = false;
                            let _ = tree::upsert_node(win, &status("saved"));
                        }
                        DrawnKeyOutcome::SaveDenied => {
                            let _ = tree::upsert_node(win, &status("save denied"));
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

        // Save on the way out only when there are unsaved edits, so closing
        // never loses work but a view-only session does not rewrite the file.
        // The empty-buffer guard still stands: never erase a note by saving
        // nothing over it.
        if dirty && !buffer.as_str().is_empty() && save_note(selected as usize, &buffer) {
            saved_any = true;
        }

        let _ = window::close(win);

        // Report for automation, matching the hello-gui convention.
        let out = stdio::stdout();
        let _ = out.write(b"note:");
        let _ = out.write(
            NOTE_TITLES
                .get(selected as usize)
                .copied()
                .unwrap_or("")
                .as_bytes(),
        );
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

bindings::export!(Component with_types_in bindings);

#[cfg(test)]
mod tests {
    //! Editor-logic tests that run natively — no host, no wasm, no clipboard.
    //! They cover the parts of the YC demo interaction the guest owns on the
    //! drawn path: shortcut translation, selection, editing, and undo/redo.
    //! Persistence (save/load across close/reopen) rides the real file host and
    //! is proven by the cross-platform matrix run, not here.
    use super::*;

    fn typed(buffer: &mut NoteBuffer, text: &str) {
        buffer.insert_str(text);
    }

    // ---- shortcut translation ----

    #[test]
    fn control_and_meta_both_trigger_editing_shortcuts() {
        // Linux/Windows use Control, macOS uses Meta; the same guest must honor
        // either, so a chord is control OR meta.
        assert_eq!(classify_key("c", true, false, false), KeyAction::Copy);
        assert_eq!(classify_key("c", false, true, false), KeyAction::Copy);
        assert_eq!(classify_key("a", true, false, false), KeyAction::SelectAll);
        assert_eq!(classify_key("v", false, true, false), KeyAction::Paste);
        assert_eq!(classify_key("x", true, false, false), KeyAction::Cut);
        assert_eq!(classify_key("s", true, false, false), KeyAction::Save);
    }

    #[test]
    fn undo_redo_shortcuts_distinguish_shift_and_y() {
        assert_eq!(classify_key("z", true, false, false), KeyAction::Undo);
        assert_eq!(classify_key("z", true, false, true), KeyAction::Redo);
        assert_eq!(classify_key("Z", true, false, true), KeyAction::Redo);
        // Ctrl+Y is the Windows redo habit.
        assert_eq!(classify_key("y", true, false, false), KeyAction::Redo);
    }

    #[test]
    fn navigation_keys_need_no_modifier_and_shift_extends() {
        assert_eq!(classify_key("Backspace", false, false, false), KeyAction::Backspace);
        assert_eq!(classify_key("Delete", false, false, false), KeyAction::DeleteForward);
        assert_eq!(
            classify_key("ArrowLeft", false, false, false),
            KeyAction::MoveLeft { extend: false }
        );
        assert_eq!(
            classify_key("ArrowRight", false, false, true),
            KeyAction::MoveRight { extend: true }
        );
        assert_eq!(
            classify_key("Home", false, false, true),
            KeyAction::MoveHome { extend: true }
        );
        assert_eq!(classify_key("End", false, false, false), KeyAction::MoveEnd { extend: false });
    }

    #[test]
    fn a_plain_letter_is_not_a_shortcut() {
        // Without a chord, a letter is text, handled by the TextInput path, so
        // the key handler must not claim it.
        assert_eq!(classify_key("a", false, false, false), KeyAction::None);
        assert_eq!(classify_key("z", false, false, false), KeyAction::None);
    }

    // ---- editing ----

    #[test]
    fn typing_inserts_at_the_caret_not_only_the_end() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "helo");
        b.move_left(false); // caret between "hel" and "o"
        typed(&mut b, "l"); // "hello"
        assert_eq!(b.as_str(), "hello");
        assert_eq!(b.cursor, 4);
    }

    #[test]
    fn backspace_and_delete_remove_around_the_caret() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "abcd");
        b.backspace(); // "abc"
        assert_eq!(b.as_str(), "abc");
        b.move_home(false);
        b.delete_forward(); // "bc"
        assert_eq!(b.as_str(), "bc");
        assert_eq!(b.cursor, 0);
    }

    #[test]
    fn insert_at_capacity_never_grows_or_panics() {
        let mut b = NoteBuffer::new();
        // Fill past capacity; the editor must clamp, not panic.
        for _ in 0..(NOTE_CAPACITY + 50) {
            typed(&mut b, "x");
        }
        assert_eq!(b.len, NOTE_CAPACITY);
        assert_eq!(b.as_str().len(), NOTE_CAPACITY);
    }

    // ---- selection ----

    #[test]
    fn shift_arrows_build_a_selection_and_typing_replaces_it() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "hello");
        b.move_home(false);
        b.move_right(true); // select "h"
        b.move_right(true); // select "he"
        assert!(b.has_selection());
        assert_eq!(b.selected_text(), "he");
        typed(&mut b, "HE"); // replaces selection
        assert_eq!(b.as_str(), "HEllo");
        assert!(!b.has_selection());
    }

    #[test]
    fn select_all_spans_the_whole_buffer() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "note body");
        b.select_all();
        assert_eq!(b.selected_text(), "note body");
        b.delete_selection();
        assert_eq!(b.as_str(), "");
    }

    #[test]
    fn moving_without_shift_collapses_the_selection() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "abcde");
        b.select_all();
        b.move_left(false); // collapse to left edge
        assert!(!b.has_selection());
        assert_eq!(b.cursor, 0);
    }

    // ---- undo / redo ----

    #[test]
    fn undo_then_redo_walks_edit_history() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "one");
        typed(&mut b, " two");
        assert_eq!(b.as_str(), "one two");
        assert!(b.undo());
        assert_eq!(b.as_str(), "one");
        assert!(b.redo());
        assert_eq!(b.as_str(), "one two");
    }

    #[test]
    fn undo_restores_caret_and_selection_position() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "abc");
        let before = (b.cursor, b.anchor);
        typed(&mut b, "d");
        assert!(b.undo());
        assert_eq!((b.cursor, b.anchor), before);
    }

    #[test]
    fn editing_after_undo_drops_the_redo_future() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "a");
        typed(&mut b, "b");
        assert!(b.undo()); // back to "a"
        typed(&mut b, "c"); // forks history
        assert_eq!(b.as_str(), "ac");
        // The old "ab" future is gone.
        assert!(!b.redo());
        assert_eq!(b.as_str(), "ac");
    }

    #[test]
    fn a_noop_backspace_does_not_consume_an_undo_step() {
        let mut b = NoteBuffer::new();
        typed(&mut b, "x");
        b.move_home(false);
        b.backspace(); // caret at 0, nothing to delete
        assert!(b.undo()); // undoes the typing, not a phantom step
        assert_eq!(b.as_str(), "");
    }
}
