//! The phone's on-screen keyboard (K-1024's Android half).
//!
//! Before this, nothing on Android ever asked for the keyboard: a to-do
//! list's text field took a tap, showed a caret, and could not be typed
//! into, and a game's self-drawn name box (krate:ui/text-entry) answered
//! "no keyboard". A NativeActivity has no text editor for the keyboard to
//! talk to, so the player's activity carries one invisible view
//! (Typing.java) whose input connection writes down what the keyboard did;
//! this module reads that down and turns it into the key presses and typed
//! text every other Krate host already delivers.
//!
//! Keyboards do not type letter by letter. Gboard holds the word being typed
//! as "composing" text and rewrites it as suggestions change; Chinese and
//! Japanese keyboards compose a whole phrase before it becomes characters.
//! [`Keystrokes`] types composing text the moment it appears and takes back
//! (with Backspace) whatever a later rewrite changes, so the app's field
//! shows the word as it grows and ends with exactly what the keyboard
//! committed.

// Off Android the keyboard glue is not compiled; the rules are, for tests.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use krate_adapter_common::ui::{Modifiers, RawKeySample, WidgetKind, WidgetPlacement, WindowId};

/// One thing the keyboard did, as Typing.java heard it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ImeEvent {
    /// Text the keyboard committed (commitText).
    Commit(String),
    /// The word being composed, as it stands now (setComposingText).
    Compose(String),
    /// Text already in the field became the word being composed again
    /// (setComposingRegion): it is on screen, nothing is typed.
    Recompose(String),
    /// The composing word is final as it stands (finishComposingText).
    FinishCompose,
    /// Characters removed before and after the caret (deleteSurroundingText).
    Delete { before: u32, after: u32 },
    /// A named key the keyboard pressed: Enter, Backspace, an arrow.
    Key(String),
}

/// Read one line Typing.java wrote: a one-letter kind, a colon, the rest.
pub(crate) fn parse(line: &str) -> Option<ImeEvent> {
    let (kind, rest) = line.split_once(':')?;
    Some(match kind {
        "c" => ImeEvent::Commit(rest.to_string()),
        "m" => ImeEvent::Compose(rest.to_string()),
        "r" => ImeEvent::Recompose(rest.to_string()),
        "f" => ImeEvent::FinishCompose,
        "d" => {
            let (before, after) = rest.split_once(',')?;
            ImeEvent::Delete {
                // A keyboard asking to delete a million characters is asking
                // to clear the field; a field's worth is plenty.
                before: before.trim().parse::<u32>().ok()?.min(MAX_DELETE),
                after: after.trim().parse::<u32>().ok()?.min(MAX_DELETE),
            }
        }
        "k" if !rest.is_empty() => ImeEvent::Key(rest.to_string()),
        _ => return None,
    })
}

/// The most characters one keyboard request may remove.
const MAX_DELETE: u32 = 4096;

/// One key press the app hears: a character typed, or a named key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stroke {
    Char(char),
    Key(String),
}

/// Turns what the keyboard did into key presses, composing included.
#[derive(Debug, Default)]
pub(crate) struct Keystrokes {
    /// The composing word as the app's field now shows it.
    composing: String,
}

impl Keystrokes {
    /// A new field: nothing is being composed in it.
    pub(crate) fn reset(&mut self) {
        self.composing.clear();
    }

    pub(crate) fn feed(&mut self, event: ImeEvent, out: &mut Vec<Stroke>) {
        match event {
            ImeEvent::Commit(text) => {
                let shown = std::mem::take(&mut self.composing);
                rewrite(&shown, &text, out);
            }
            ImeEvent::Compose(text) => {
                let shown = std::mem::take(&mut self.composing);
                rewrite(&shown, &text, out);
                self.composing = text;
            }
            ImeEvent::Recompose(text) => self.composing = text,
            ImeEvent::FinishCompose => self.composing.clear(),
            ImeEvent::Delete { before, after } => {
                // What is deleted is out of the composing word too.
                let kept = self
                    .composing
                    .chars()
                    .count()
                    .saturating_sub(before as usize);
                self.composing = self.composing.chars().take(kept).collect();
                out.extend((0..before).map(|_| Stroke::Key("Backspace".to_string())));
                out.extend((0..after).map(|_| Stroke::Key("Delete".to_string())));
            }
            ImeEvent::Key(name) => {
                // A key moves the caret or ends the line: whatever was being
                // composed stays as typed.
                self.composing.clear();
                out.push(Stroke::Key(name));
            }
        }
    }
}

/// Change what the field shows from `shown` to `wanted` with the fewest
/// key presses: keep the shared start, Backspace the rest, type the new end.
fn rewrite(shown: &str, wanted: &str, out: &mut Vec<Stroke>) {
    let same = shown
        .chars()
        .zip(wanted.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let erase = shown.chars().count() - same;
    out.extend((0..erase).map(|_| Stroke::Key("Backspace".to_string())));
    for c in wanted.chars().skip(same) {
        out.push(match c {
            // A keyboard's Enter on a many-line field commits "\n"; the app
            // hears the Enter key, as from any keyboard.
            '\n' | '\r' => Stroke::Key("Enter".to_string()),
            '\t' => Stroke::Key("Tab".to_string()),
            c if c.is_control() => continue,
            c => Stroke::Char(c),
        });
    }
}

/// The key presses as the samples a desktop keyboard would have made: each
/// stroke pressed and let go, a character carrying itself as typed text.
/// Let go as well as pressed, so a game asking which keys are held never
/// sees a keyboard letter stuck down.
pub(crate) fn samples(window: WindowId, strokes: &[Stroke]) -> Vec<RawKeySample> {
    let mut out = Vec::with_capacity(strokes.len() * 2);
    for stroke in strokes {
        let (key, text) = match stroke {
            Stroke::Char(' ') => ("Space".to_string(), Some(" ".to_string())),
            Stroke::Char(c) => (c.to_string(), Some(c.to_string())),
            Stroke::Key(name) => (name.clone(), None),
        };
        out.push(RawKeySample {
            window,
            key: key.clone(),
            pressed: true,
            modifiers: Modifiers::default(),
            text,
        });
        out.push(RawKeySample {
            window,
            key,
            pressed: false,
            modifiers: Modifiers::default(),
            text: None,
        });
    }
    out
}

/// Whether a widget of this kind is typed into: a press on one brings the
/// keyboard up, focus moving off it puts the keyboard away.
pub(crate) fn takes_typing(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::TextField | WidgetKind::TextArea)
}

/// The text field (if any) a finger landing at (x, y) is on, and whether it
/// holds many lines -- a field the app edits itself counts too.
pub(crate) fn field_at(placements: &[WidgetPlacement], x: f32, y: f32) -> Option<bool> {
    placements
        .iter()
        .rev()
        .filter(|p| x >= p.x && x < p.x + p.width && y >= p.y && y < p.y + p.height)
        .filter(|p| match p.clip {
            Some((cx, cy, cw, ch)) => x >= cx && x < cx + cw && y >= cy && y < cy + ch,
            None => true,
        })
        .find(|p| takes_typing(p.kind) || p.text_cursor.is_some())
        .map(|p| p.kind == WidgetKind::TextArea)
}

/// Watches the edges the phone covers (its bars, its cutout, and the
/// keyboard while it is up) and says when they change, so a window is laid
/// out again above a keyboard that just slid up.
#[derive(Debug, Default)]
pub(crate) struct EdgeWatch {
    last: Option<(f32, f32, f32, f32)>,
}

impl EdgeWatch {
    /// True when `now` differs from the last edges seen; the first reading
    /// is not a change.
    pub(crate) fn changed(&mut self, now: (f32, f32, f32, f32)) -> bool {
        let before = self.last.replace(now);
        before.is_some_and(|before| before != now)
    }
}

#[cfg(target_os = "android")]
mod real {
    use super::*;
    use crate::java::{string, with_activity};
    use jni::objects::{JObjectArray, JValue};
    use std::sync::Mutex;

    struct State {
        /// The window the keyboard types into.
        window: Option<WindowId>,
        /// Asked up; Typing.java is read while it is, and once after.
        up: bool,
        strokes: Keystrokes,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        window: None,
        up: false,
        strokes: Keystrokes {
            composing: String::new(),
        },
    });

    /// Bring the keyboard up for `window`, or put it away. True when it was
    /// asked up. `many_lines` makes its Enter key a new line, not Done.
    pub fn set(window: WindowId, on: bool, many_lines: bool) -> bool {
        let asked = with_activity("the keyboard", |env, activity| {
            env.call_method(
                activity,
                "krateTextEntry",
                "(ZZ)Z",
                &[JValue::Bool(on.into()), JValue::Bool(many_lines.into())],
            )?
            .z()
        })
        .unwrap_or(false);
        let mut state = STATE.lock().expect("typing lock");
        if on {
            // A tap into a field starts from what that field shows.
            state.strokes.reset();
            state.window = Some(window);
        }
        state.up = on && asked;
        on && asked
    }

    /// What the keyboard typed since the last ask, as key samples.
    pub fn drain() -> Vec<RawKeySample> {
        let window = {
            let state = STATE.lock().expect("typing lock");
            match state.window {
                Some(window) => window,
                None => return Vec::new(),
            }
        };
        let lines = with_activity("reading the keyboard", |env, activity| {
            let all = env
                .call_method(activity, "krateTyped", "()[Ljava/lang/String;", &[])?
                .l()?;
            if all.is_null() {
                return Ok(Vec::new());
            }
            let all = JObjectArray::from(all);
            let mut out = Vec::new();
            for i in 0..env.get_array_length(&all)? {
                let one = env.get_object_array_element(&all, i)?;
                if let Some(line) = string(env, one)? {
                    out.push(line);
                }
            }
            Ok(out)
        })
        .unwrap_or_default();
        if lines.is_empty() {
            return Vec::new();
        }
        let mut state = STATE.lock().expect("typing lock");
        let mut strokes = Vec::new();
        for event in lines.iter().filter_map(|line| parse(line)) {
            state.strokes.feed(event, &mut strokes);
        }
        samples(window, &strokes)
    }
}

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: &[&str]) -> Vec<Stroke> {
        let mut keys = Keystrokes::default();
        let mut out = Vec::new();
        for line in events {
            keys.feed(parse(line).expect("a line Typing.java writes"), &mut out);
        }
        out
    }

    fn typed(strokes: &[Stroke]) -> String {
        // What a plain text field shows after these strokes.
        let mut field = String::new();
        for stroke in strokes {
            match stroke {
                Stroke::Char(c) => field.push(*c),
                Stroke::Key(k) if k == "Backspace" => {
                    field.pop();
                }
                Stroke::Key(k) if k == "Enter" => field.push('\n'),
                Stroke::Key(_) => {}
            }
        }
        field
    }

    #[test]
    fn a_committed_word_is_typed_letter_by_letter() {
        let strokes = run(&["c:hi"]);
        assert_eq!(strokes, vec![Stroke::Char('h'), Stroke::Char('i')]);
    }

    #[test]
    fn a_composing_word_shows_as_it_grows_and_ends_as_committed() {
        // Gboard typing "hello" with autocorrect to "Hello" and a space.
        let strokes = run(&[
            "m:h", "m:he", "m:hel", "m:hell", "m:hello", "c:Hello", "c: ",
        ]);
        assert_eq!(typed(&strokes), "Hello ");
        // Each letter appeared as it was typed, before any commit.
        assert_eq!(strokes[0], Stroke::Char('h'));
        assert_eq!(strokes[1], Stroke::Char('e'));
    }

    #[test]
    fn a_suggestion_replaces_only_what_changed() {
        let strokes = run(&["m:teh", "c:the"]);
        // "t" kept, "eh" erased, "he" typed.
        assert_eq!(
            strokes,
            vec![
                Stroke::Char('t'),
                Stroke::Char('e'),
                Stroke::Char('h'),
                Stroke::Key("Backspace".into()),
                Stroke::Key("Backspace".into()),
                Stroke::Char('h'),
                Stroke::Char('e'),
            ]
        );
        assert_eq!(typed(&strokes), "the");
    }

    #[test]
    fn a_chinese_keyboard_ends_with_the_characters_not_the_pinyin() {
        let strokes = run(&["m:n", "m:ni", "m:nih", "m:niha", "m:nihao", "c:你好"]);
        assert_eq!(typed(&strokes), "你好");
    }

    #[test]
    fn backspacing_into_a_word_does_not_type_it_twice() {
        // "cat " typed, then Backspace twice: Gboard deletes the space, takes
        // "cat" back as the composing word, then shortens it.
        let strokes = run(&["c:cat ", "d:1,0", "r:cat", "m:ca"]);
        assert_eq!(typed(&strokes), "ca");
    }

    #[test]
    fn finishing_a_word_keeps_it_and_the_next_starts_fresh() {
        let strokes = run(&["m:ok", "f:", "m:go", "f:"]);
        assert_eq!(typed(&strokes), "okgo");
    }

    #[test]
    fn enter_and_named_keys_reach_the_app_as_keys() {
        let strokes = run(&["c:a\nb", "k:Enter", "k:Backspace"]);
        assert_eq!(
            strokes,
            vec![
                Stroke::Char('a'),
                Stroke::Key("Enter".into()),
                Stroke::Char('b'),
                Stroke::Key("Enter".into()),
                Stroke::Key("Backspace".into()),
            ]
        );
    }

    #[test]
    fn deleting_inside_a_composing_word_shortens_it() {
        let strokes = run(&["m:word", "d:2,0", "m:wo!"]);
        assert_eq!(typed(&strokes), "wo!");
    }

    #[test]
    fn unknown_or_broken_lines_are_ignored() {
        assert_eq!(parse("x:what"), None);
        assert_eq!(parse("no colon"), None);
        assert_eq!(parse("d:1"), None);
        assert_eq!(parse("k:"), None);
        assert_eq!(
            parse("d:99999999,0"),
            Some(ImeEvent::Delete {
                before: MAX_DELETE,
                after: 0
            })
        );
        // A committed colon is text, not a separator.
        assert_eq!(parse("c::"), Some(ImeEvent::Commit(":".into())));
    }

    #[test]
    fn samples_press_and_let_go_and_space_is_named() {
        use krate_adapter_common::ui::{WindowAdapter, WindowOptions, WindowSize};
        let host = krate_adapter_common::ui::DraftUiAdapter::default();
        let window = host
            .create_window(
                WindowOptions::new("typing", WindowSize::new(390, 844).expect("size"))
                    .expect("options"),
            )
            .expect("window");
        let got = samples(window, &[Stroke::Char(' '), Stroke::Char('é')]);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].key, "Space");
        assert_eq!(got[0].text.as_deref(), Some(" "));
        assert!(got[0].pressed && !got[1].pressed);
        assert_eq!(got[1].text, None);
        assert_eq!(got[2].key, "é");
        assert_eq!(got[2].text.as_deref(), Some("é"));
    }

    fn placement(kind: WidgetKind, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
        WidgetPlacement {
            kind,
            x,
            y,
            width: w,
            height: h,
            ..WidgetPlacement::default()
        }
    }

    #[test]
    fn a_tap_on_a_field_finds_it_and_a_tap_beside_it_does_not() {
        let placements = vec![
            placement(WidgetKind::Stack, 0.0, 0.0, 400.0, 800.0),
            placement(WidgetKind::TextField, 10.0, 10.0, 200.0, 40.0),
            placement(WidgetKind::TextArea, 10.0, 100.0, 200.0, 200.0),
            placement(WidgetKind::Button, 10.0, 400.0, 100.0, 40.0),
        ];
        assert_eq!(field_at(&placements, 50.0, 30.0), Some(false));
        assert_eq!(field_at(&placements, 50.0, 150.0), Some(true));
        assert_eq!(field_at(&placements, 50.0, 420.0), None);
        assert_eq!(field_at(&placements, 300.0, 30.0), None);
        // A field scrolled out of its list is not under the finger.
        let mut clipped = placement(WidgetKind::TextField, 10.0, 500.0, 200.0, 40.0);
        clipped.clip = Some((0.0, 0.0, 400.0, 450.0));
        assert_eq!(field_at(&[clipped.clone()], 50.0, 520.0), None);
        // A text the app edits itself is typed into too.
        let mut own = placement(WidgetKind::Text, 0.0, 0.0, 100.0, 20.0);
        own.text_cursor = Some((0, 0));
        assert_eq!(field_at(&[own], 5.0, 5.0), Some(false));
    }

    #[test]
    fn the_edges_changing_is_noticed_once_and_the_first_look_is_not_a_change() {
        let mut watch = EdgeWatch::default();
        assert!(!watch.changed((24.0, 0.0, 48.0, 0.0)));
        assert!(!watch.changed((24.0, 0.0, 48.0, 0.0)));
        // The keyboard slid up over the bottom.
        assert!(watch.changed((24.0, 0.0, 900.0, 0.0)));
        assert!(!watch.changed((24.0, 0.0, 900.0, 0.0)));
        assert!(watch.changed((24.0, 0.0, 48.0, 0.0)));
    }
}
