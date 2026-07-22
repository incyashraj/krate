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
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const SIDEBAR_ID: u64 = 2;
const EDITOR_ID: u64 = 3;
const STATUS_ID: u64 = 4;
const NOTE_ROW_BASE_ID: u64 = 10;

/// How many notes the sample manages. Fixed so no allocation is needed.
const NOTE_COUNT: usize = 3;
const NOTE_TITLES: [&str; NOTE_COUNT] = ["first note", "second note", "third note"];
const NOTE_FILES: [&str; NOTE_COUNT] = [
    "./notes/first.txt",
    "./notes/second.txt",
    "./notes/third.txt",
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

struct Component;

/// A fixed-capacity text buffer that never panics and never allocates.
struct NoteBuffer {
    bytes: [u8; NOTE_CAPACITY],
    len: usize,
}

impl NoteBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; NOTE_CAPACITY],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push_str(&mut self, text: &str) {
        for byte in text.as_bytes() {
            self.push(*byte);
        }
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    fn as_str(&self) -> &str {
        let slice = self.bytes.get(..self.len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }
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
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
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
    }
}

/// The note list. Selection lives here, so the host paints the highlight.
fn sidebar(selected: Option<u32>) -> types::WidgetNode {
    types::WidgetNode {
        id: SIDEBAR_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::ListView,
        label: None,
        role: Some(pure_string("listbox")),
        style: types::Style {
            width: Some(220.0),
            height: Some(96.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected,
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
            height: Some(24.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
    }
}

/// The editor. A TextArea wraps and fills from the top, unlike a field.
fn editor(text: &str) -> types::WidgetNode {
    types::WidgetNode {
        id: EDITOR_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::TextArea,
        label: Some(pure_string(text)),
        role: Some(pure_string("textbox")),
        style: types::Style {
            width: Some(660.0),
            height: Some(200.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
    }
}

fn status(text: &str) -> types::WidgetNode {
    types::WidgetNode {
        id: STATUS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string(text)),
        role: Some(pure_string("status")),
        style: types::Style {
            width: Some(660.0),
            height: Some(20.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
    }
}

fn is_note_row(widget: Option<u64>) -> Option<usize> {
    let id = widget?;
    if id < NOTE_ROW_BASE_ID {
        return None;
    }
    let index = (id - NOTE_ROW_BASE_ID) as usize;
    (index < NOTE_COUNT).then_some(index)
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

        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &sidebar(Some(selected))).is_err()
            || tree::upsert_node(win, &editor(buffer.as_str())).is_err()
            || tree::upsert_node(win, &status("ready")).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        let mut row = 0usize;
        while row < NOTE_COUNT {
            if tree::upsert_node(win, &note_row(row)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            row += 1;
        }

        // Raw args are newline terminated, one per argument, so an exact
        // comparison against b"quick" never matches. Comparing the first line
        // is what actually detects the flag.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        let rounds = if quick {
            QUICK_WAIT_ROUNDS
        } else {
            MAX_WAIT_ROUNDS
        };

        let mut saved_any = false;
        let mut close_requested = false;

        for _ in 0..rounds {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
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
                        let _ = tree::upsert_node(win, &editor(buffer.as_str()));
                        let _ = tree::upsert_node(win, &status("loaded"));
                    }
                }
                Some(types::Event::TextInput(text)) => {
                    for byte in text.as_bytes() {
                        let printable = byte.is_ascii_graphic() || *byte == b' ';
                        if printable {
                            buffer.push(*byte);
                        }
                    }
                    let _ = tree::upsert_node(win, &editor(buffer.as_str()));
                }
                Some(types::Event::Key(key)) => {
                    if key.pressed && key.key.as_bytes() == b"Backspace" {
                        buffer.pop();
                        let _ = tree::upsert_node(win, &editor(buffer.as_str()));
                    }
                    // Ctrl or Cmd plus S saves, the shortcut every note app has.
                    if key.pressed
                        && key.key.as_bytes() == b"s"
                        && (key.modifiers.control || key.modifiers.meta)
                    {
                        if save_note(selected as usize, &buffer) {
                            saved_any = true;
                            let _ = tree::upsert_node(win, &status("saved"));
                        } else {
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

        // Save on the way out, so closing the window does not lose work.
        if save_note(selected as usize, &buffer) {
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
