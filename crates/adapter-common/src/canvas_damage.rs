//! What part of a canvas changed since the screen last showed it (K-1373).
//!
//! A canvas app that redraws a caret, a clock or a progress bar changes a
//! few hundred pixels, and every present used to copy and upload all of a
//! Retina window's 16 MB anyway: idle Vault spent most of its frame moving
//! pixels nobody had touched. The host knows which part was drawn since the
//! last present (every draw is trimmed to the app's clip); it notes that
//! here, by window and widget, and the adapter that puts the canvas on the
//! screen takes it and uploads only that part.
//!
//! A side table rather than a field on the placement, the same way
//! `krate_presenter_gpu::shared::scene_frames` hands a 3D frame over: a
//! placement is built in a dozen places and only the presenting adapter
//! cares. The contract is conservative in both directions: no entry means
//! "assume all of it changed", and notes made before the adapter took them
//! are merged, so a frame the adapter skipped is never lost.

use std::collections::HashMap;
use std::sync::Mutex;

/// A changed region of a canvas's picture, in that picture's pixels:
/// `(left, top, right, bottom)`, right and bottom exclusive. An empty
/// region (`left == right` or `top == bottom`) means nothing changed.
pub type Damage = (u32, u32, u32, u32);

/// What is known about one canvas's next upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Only this part changed (empty: nothing did).
    Region(Damage),
    /// All of it may have changed.
    All,
}

static NOTES: Mutex<Option<HashMap<(u64, u64), Change>>> = Mutex::new(None);

/// Whether a region holds no pixels.
pub fn is_empty(d: Damage) -> bool {
    d.0 >= d.2 || d.1 >= d.3
}

/// The smallest region holding both.
pub fn union(a: Damage, b: Damage) -> Damage {
    if is_empty(a) {
        return b;
    }
    if is_empty(b) {
        return a;
    }
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// Merge two notes about the same canvas.
pub fn merge(a: Change, b: Change) -> Change {
    match (a, b) {
        (Change::Region(x), Change::Region(y)) => Change::Region(union(x, y)),
        _ => Change::All,
    }
}

/// Note that the picture `window`'s canvas `widget` is about to show
/// changed only where `change` says, merged with any note not yet taken.
pub fn note(window: u64, widget: u64, change: Change) {
    let mut notes = NOTES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let map = notes.get_or_insert_with(HashMap::new);
    let merged = match map.get(&(window, widget)) {
        Some(old) => merge(*old, change),
        None => change,
    };
    map.insert((window, widget), merged);
}

/// Take what is known about the canvas's change: `All` when nothing was
/// noted, which is always a safe answer.
pub fn take(window: u64, widget: u64) -> Change {
    let mut notes = NOTES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    notes
        .as_mut()
        .and_then(|map| map.remove(&(window, widget)))
        .unwrap_or(Change::All)
}

/// Forget a canvas's notes (its window closed).
pub fn forget_window(window: u64) {
    let mut notes = NOTES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(map) = notes.as_mut() {
        map.retain(|(w, _), _| *w != window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_merge_until_taken_and_default_to_all() {
        // A window id no other test uses: the table is process-wide.
        let (w, c) = (0xD0_0001, 7);
        assert_eq!(take(w, c), Change::All, "nothing noted: assume all");
        note(w, c, Change::Region((10, 10, 20, 20)));
        note(w, c, Change::Region((30, 5, 40, 12)));
        assert_eq!(take(w, c), Change::Region((10, 5, 40, 20)));
        assert_eq!(take(w, c), Change::All, "taking empties it");
        note(w, c, Change::Region((0, 0, 0, 0)));
        assert_eq!(take(w, c), Change::Region((0, 0, 0, 0)), "nothing changed");
        note(w, c, Change::Region((1, 1, 2, 2)));
        note(w, c, Change::All);
        note(w, c, Change::Region((1, 1, 2, 2)));
        assert_eq!(take(w, c), Change::All, "all wins");
        note(w, c, Change::Region((1, 1, 2, 2)));
        forget_window(w);
        assert_eq!(take(w, c), Change::All);
    }

    #[test]
    fn an_empty_region_adds_nothing() {
        assert_eq!(union((0, 0, 0, 0), (3, 4, 5, 6)), (3, 4, 5, 6));
        assert_eq!(union((3, 4, 5, 6), (9, 9, 9, 9)), (3, 4, 5, 6));
        assert!(is_empty((5, 5, 5, 9)) && !is_empty((5, 5, 6, 6)));
    }
}
