//! The modal engine: keys in, edits and effects out.
//!
//! Normal, insert, visual (by character and by line), select (what a mouse
//! drag or Shift with an arrow makes: typing replaces it, the way any editor
//! behaves), and the command line for `:` and `/`. A command in normal mode
//! is gathered key by key and parsed whole -- `[count]["x][count]op[count]
//! motion` -- so `3dw`, `d3w`, `"ayy` and `2d2w` all mean what they mean in
//! Vim, and `.` repeats a change by feeding its keys through the same parser
//! again.
//!
//! The engine does not know about files, panes or the screen beyond what a
//! command needs (the visible lines, for `H`, `M`, `L` and half-page
//! scrolls). Anything bigger -- write this file, split, open the palette --
//! comes back as an [`Effect`] for the workspace to carry out.
//!
//! Search and `:s` match literal text, not regular expressions; lowercase
//! patterns match either case (Vim's `smartcase`).

use crate::buffer::{Buffer, Pos};
use crate::text::{self, big_class, class, Class};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

/// One thing a person did at the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A typed character (from text input, never from a key name).
    Char(char),
    Key(Key),
    /// An arrow, Home or End with Shift held: extends a selection.
    Shift(Key),
    /// Ctrl (or Cmd) with a letter.
    Ctrl(char),
    /// Text the person pasted.
    Paste(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    /// A selection made with the mouse or Shift: typing replaces it.
    Select,
    /// The `:` line.
    Command,
    /// The `/` (forward) or `?` (backward) line.
    Search { forward: bool },
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::Select => "SELECT",
            Mode::Command => "COMMAND",
            Mode::Search { .. } => "SEARCH",
        }
    }

    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::Select)
    }
}

/// Where a pane's cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub pos: Pos,
    /// Where a visual or select range began.
    pub anchor: Pos,
    /// The screen column `j` and `k` aim for; `usize::MAX` after `$`.
    pub want: usize,
}

impl Cursor {
    pub fn at(pos: Pos) -> Self {
        Cursor {
            pos,
            anchor: pos,
            want: 0,
        }
    }
}

/// What a command needs to know about the screen.
pub struct Ctx<'a> {
    pub buf: &'a mut Buffer,
    pub cur: &'a mut Cursor,
    /// How many lines the pane shows.
    pub page: usize,
    /// The first line the pane shows.
    pub top: usize,
}

/// What the engine asks the rest of the editor to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// `:w` (with an optional new name, relative to the project).
    Write { path: Option<String> },
    /// `:q`; `force` drops unsaved changes.
    Quit { force: bool },
    /// `:wq`, `:x`, `ZZ`.
    WriteQuit,
    /// `:wa`.
    WriteAll,
    /// `:qa`.
    QuitAll { force: bool },
    /// `:e <file>`; with no file, reload this one (`force` for `:e!`).
    Edit { path: Option<String>, force: bool },
    /// `:vsplit [file]`, `Ctrl-W v`.
    Split { path: Option<String> },
    /// `Ctrl-W w`, `Ctrl-W l`: the next pane (or previous).
    FocusPane { forward: bool },
    /// `:close`, `Ctrl-W c`.
    ClosePane,
    /// `:only`, `Ctrl-W o`.
    OnlyPane,
    /// `:bn`, `:bp`.
    NextBuffer { forward: bool },
    /// `:bd`.
    CloseBuffer { force: bool },
    /// Scroll so the cursor's line sits at the top, middle or bottom.
    Scroll(ScrollTo),
    /// Scroll by this many lines (half-page and page commands).
    ScrollLines(isize),
    /// An ex command the engine does not own (`:theme`, `:cargo`, ...),
    /// handed to the workspace as typed.
    Command(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTo {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Dedent,
    Lower,
    Upper,
    ToggleCase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindKind {
    /// `f`: onto the character.
    To,
    /// `t`: just before it.
    Till,
    /// `F`.
    BackTo,
    /// `T`.
    BackTill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    WordEndBack(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    /// `gg` (no count: first line) and `G` (no count: last line).
    GotoLine { last: bool },
    MatchPair,
    Find(FindKind, char),
    RepeatFind { reverse: bool },
    ParaFwd,
    ParaBack,
    ScreenTop,
    ScreenMid,
    ScreenBottom,
    SearchNext { reverse: bool },
    SearchWord { forward: bool },
    NextLineStart,
    PrevLineStart,
    HalfDown,
    HalfUp,
    PageDown,
    PageUp,
    /// `gd`: the place this name is defined in the file.
    Definition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MKind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Obj {
    Word { big: bool, around: bool },
    Pair { open: char, close: char, around: bool },
    Quote { q: char, around: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    Insert,
    Append,
    InsertLineStart,
    AppendLineEnd,
    OpenBelow,
    OpenAbove,
    DeleteChar,
    DeleteCharBack,
    DeleteToEnd,
    ChangeToEnd,
    SubstChar,
    SubstLine,
    YankLine,
    Replace(char),
    Join,
    PutAfter,
    PutBefore,
    Undo,
    Redo,
    Repeat,
    Visual,
    VisualLine,
    CmdLine,
    Search { forward: bool },
    ToggleCaseChar,
    ScrollTo(ScrollTo),
    SaveQuit,
    QuitForce,
    Window(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Motion(Motion),
    /// The operator doubled: `dd`, `yy`, `>>`.
    Lines,
    Object(Obj),
    Act(Act),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cmd {
    count: Option<usize>,
    reg: Option<char>,
    op: Option<Op>,
    target: Target,
    /// How many leading inputs were the count (stripped when recording `.`).
    count_len: usize,
}

enum Parse {
    More,
    Bad,
    Done(Cmd),
}

/// The whole modal state, shared by every pane the way Vim shares it:
/// one mode, one set of registers, one last search.
#[derive(Debug, Clone)]
pub struct Vim {
    pub mode: Mode,
    pending: Vec<Input>,
    /// The `:` or `/` line being typed.
    pub cmdline: String,
    history: Vec<String>,
    history_at: Option<usize>,
    /// The mode `:` was typed from (visual makes the range `'<,'>`).
    cmd_from: Mode,
    pub registers: BTreeMap<char, Register>,
    pub last_search: Option<(String, bool)>,
    /// Matches are highlighted until `:noh` or a new search.
    pub highlight: bool,
    last_find: Option<(FindKind, char)>,
    last_change: Option<(Option<usize>, Vec<Input>)>,
    recording: Option<(Option<usize>, Vec<Input>)>,
    /// `3ihi<Esc>`: how many times the insert repeats, and where it began.
    insert_count: usize,
    insert_from: Pos,
    /// The last visual range, for `'<,'>`.
    visual_marks: Option<(Pos, Pos, bool)>,
    /// A line to show in the status area.
    pub message: Option<(String, bool)>,
    pub tab: usize,
    pub shiftwidth: usize,
    pub expandtab: bool,
    pub relative_numbers: bool,
    /// Which mode Select falls back to when its selection is gone.
    select_return: Mode,
    /// Keys typed in normal mode, shown at the right of the status line.
    pub showcmd: String,
}

impl Default for Vim {
    fn default() -> Self {
        Vim::new()
    }
}

fn digit(i: &Input) -> Option<usize> {
    match i {
        Input::Char(c) if c.is_ascii_digit() => Some(*c as usize - '0' as usize),
        _ => None,
    }
}

fn ch(i: Option<&Input>) -> Option<char> {
    match i {
        Some(Input::Char(c)) => Some(*c),
        _ => None,
    }
}

/// Parse `[count]` from `inputs[at..]`; a leading `0` is the motion, not a count.
fn read_count(inputs: &[Input], mut at: usize) -> (Option<usize>, usize) {
    let start = at;
    let mut n: usize = 0;
    while let Some(d) = inputs.get(at).and_then(digit) {
        if at == start && d == 0 {
            break;
        }
        n = n.saturating_mul(10).saturating_add(d);
        at += 1;
    }
    if at == start {
        (None, at)
    } else {
        (Some(n.max(1)), at)
    }
}

/// A motion from `inputs[at..]`: `More` when it needs another key.
fn parse_motion(inputs: &[Input], at: usize) -> Result<Option<(Motion, usize)>, ()> {
    let Some(first) = inputs.get(at) else {
        return Ok(None);
    };
    let m = match first {
        Input::Key(Key::Left) | Input::Key(Key::Backspace) => Motion::Left,
        Input::Key(Key::Right) => Motion::Right,
        Input::Key(Key::Up) => Motion::Up,
        Input::Key(Key::Down) => Motion::Down,
        Input::Key(Key::Home) => Motion::LineStart,
        Input::Key(Key::End) => Motion::LineEnd,
        Input::Key(Key::Enter) => Motion::NextLineStart,
        Input::Key(Key::PageDown) | Input::Ctrl('f') => Motion::PageDown,
        Input::Key(Key::PageUp) | Input::Ctrl('b') => Motion::PageUp,
        Input::Ctrl('d') => Motion::HalfDown,
        Input::Ctrl('u') => Motion::HalfUp,
        Input::Char(c) => match c {
            'h' => Motion::Left,
            'l' | ' ' => Motion::Right,
            'j' => Motion::Down,
            'k' => Motion::Up,
            'w' => Motion::WordFwd(false),
            'W' => Motion::WordFwd(true),
            'b' => Motion::WordBack(false),
            'B' => Motion::WordBack(true),
            'e' => Motion::WordEnd(false),
            'E' => Motion::WordEnd(true),
            '0' => Motion::LineStart,
            '^' => Motion::FirstNonBlank,
            '$' => Motion::LineEnd,
            'G' => Motion::GotoLine { last: true },
            '%' => Motion::MatchPair,
            ';' => Motion::RepeatFind { reverse: false },
            ',' => Motion::RepeatFind { reverse: true },
            '}' => Motion::ParaFwd,
            '{' => Motion::ParaBack,
            'H' => Motion::ScreenTop,
            'M' => Motion::ScreenMid,
            'L' => Motion::ScreenBottom,
            'n' => Motion::SearchNext { reverse: false },
            'N' => Motion::SearchNext { reverse: true },
            '*' => Motion::SearchWord { forward: true },
            '#' => Motion::SearchWord { forward: false },
            '+' => Motion::NextLineStart,
            '-' => Motion::PrevLineStart,
            'f' | 't' | 'F' | 'T' => {
                let kind = match c {
                    'f' => FindKind::To,
                    't' => FindKind::Till,
                    'F' => FindKind::BackTo,
                    _ => FindKind::BackTill,
                };
                return match inputs.get(at + 1) {
                    None => Err(()),
                    Some(Input::Char(t)) => Ok(Some((Motion::Find(kind, *t), at + 2))),
                    Some(_) => Ok(None),
                };
            }
            'g' => {
                return match ch(inputs.get(at + 1)) {
                    None if inputs.get(at + 1).is_none() => Err(()),
                    Some('g') => Ok(Some((Motion::GotoLine { last: false }, at + 2))),
                    Some('e') => Ok(Some((Motion::WordEndBack(false), at + 2))),
                    Some('E') => Ok(Some((Motion::WordEndBack(true), at + 2))),
                    Some('_') => Ok(Some((Motion::LastNonBlank, at + 2))),
                    Some('d') => Ok(Some((Motion::Definition, at + 2))),
                    _ => Ok(None),
                };
            }
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };
    Ok(Some((m, at + 1)))
}

/// `Err(())` means "needs more keys".
fn parse_object(inputs: &[Input], at: usize) -> Result<Option<(Obj, usize)>, ()> {
    let around = match ch(inputs.get(at)) {
        Some('i') => false,
        Some('a') => true,
        _ => return Ok(None),
    };
    let Some(next) = inputs.get(at + 1) else {
        return Err(());
    };
    let obj = match next {
        Input::Char('w') => Obj::Word { big: false, around },
        Input::Char('W') => Obj::Word { big: true, around },
        Input::Char('(') | Input::Char(')') | Input::Char('b') => Obj::Pair { open: '(', close: ')', around },
        Input::Char('{') | Input::Char('}') | Input::Char('B') => Obj::Pair { open: '{', close: '}', around },
        Input::Char('[') | Input::Char(']') => Obj::Pair { open: '[', close: ']', around },
        Input::Char('<') | Input::Char('>') => Obj::Pair { open: '<', close: '>', around },
        Input::Char(q @ ('"' | '\'' | '`')) => Obj::Quote { q: *q, around },
        _ => return Ok(None),
    };
    Ok(Some((obj, at + 2)))
}

fn parse(inputs: &[Input]) -> Parse {
    let (count1, mut at) = read_count(inputs, 0);
    let mut reg = None;
    if ch(inputs.get(at)) == Some('"') {
        match ch(inputs.get(at + 1)) {
            None if inputs.get(at + 1).is_none() => return Parse::More,
            Some(r) => {
                reg = Some(r);
                at += 2;
            }
            None => return Parse::Bad,
        }
    }
    let (count2, after) = read_count(inputs, at);
    at = after;
    let count = match (count1, count2) {
        (Some(a), Some(b)) => Some(a.saturating_mul(b)),
        (a, b) => a.or(b),
    };
    let count_len = if count1.is_some() && reg.is_none() && count2.is_none() {
        // Only a leading count can be swapped for `.`'s own.
        let (_, end) = read_count(inputs, 0);
        end
    } else {
        0
    };
    let Some(first) = inputs.get(at) else {
        return Parse::More;
    };
    let done = |op, target| {
        Parse::Done(Cmd {
            count,
            reg,
            op,
            target,
            count_len,
        })
    };

    // Operators.
    let op = match first {
        Input::Char('d') => Some(Op::Delete),
        Input::Char('c') => Some(Op::Change),
        Input::Char('y') => Some(Op::Yank),
        Input::Char('>') => Some(Op::Indent),
        Input::Char('<') => Some(Op::Dedent),
        Input::Char('g') => match ch(inputs.get(at + 1)) {
            Some('u') => Some(Op::Lower),
            Some('U') => Some(Op::Upper),
            Some('~') => Some(Op::ToggleCase),
            _ => None,
        },
        _ => None,
    };
    if let Some(op) = op {
        let op_len = if matches!(first, Input::Char('g')) { 2 } else { 1 };
        let op_char = ch(inputs.get(at + op_len - 1));
        let mut k = at + op_len;
        let Some(next) = inputs.get(k) else {
            return Parse::More;
        };
        // Doubled: dd, cc, yy, >>, <<, guu / gUU / g~~.
        if ch(Some(next)) == op_char {
            return done(Some(op), Target::Lines);
        }
        let (count3, after) = read_count(inputs, k);
        k = after;
        let count = match (count, count3) {
            (Some(a), Some(b)) => Some(a.saturating_mul(b)),
            (a, b) => a.or(b),
        };
        let done = |target| {
            Parse::Done(Cmd {
                count,
                reg,
                op: Some(op),
                target,
                count_len: if count3.is_some() { 0 } else { count_len },
            })
        };
        if inputs.get(k).is_none() {
            return Parse::More;
        }
        match parse_object(inputs, k) {
            Err(()) => return Parse::More,
            Ok(Some((obj, _))) => return done(Target::Object(obj)),
            Ok(None) => {}
        }
        return match parse_motion(inputs, k) {
            Err(()) => Parse::More,
            Ok(Some((m, _))) => done(Target::Motion(m)),
            Ok(None) => Parse::Bad,
        };
    }

    // Actions.
    let two = |n: Option<&Input>| ch(n);
    let act = match first {
        Input::Char('i') => Some(Act::Insert),
        Input::Char('a') => Some(Act::Append),
        Input::Char('I') => Some(Act::InsertLineStart),
        Input::Char('A') => Some(Act::AppendLineEnd),
        Input::Char('o') => Some(Act::OpenBelow),
        Input::Char('O') => Some(Act::OpenAbove),
        Input::Char('x') | Input::Key(Key::Delete) => Some(Act::DeleteChar),
        Input::Char('X') => Some(Act::DeleteCharBack),
        Input::Char('D') => Some(Act::DeleteToEnd),
        Input::Char('C') => Some(Act::ChangeToEnd),
        Input::Char('s') => Some(Act::SubstChar),
        Input::Char('S') => Some(Act::SubstLine),
        Input::Char('Y') => Some(Act::YankLine),
        Input::Char('J') => Some(Act::Join),
        Input::Char('p') => Some(Act::PutAfter),
        Input::Char('P') => Some(Act::PutBefore),
        Input::Char('u') => Some(Act::Undo),
        Input::Ctrl('r') => Some(Act::Redo),
        Input::Char('.') => Some(Act::Repeat),
        Input::Char('v') => Some(Act::Visual),
        Input::Char('V') => Some(Act::VisualLine),
        Input::Char(':') => Some(Act::CmdLine),
        Input::Char('/') => Some(Act::Search { forward: true }),
        Input::Char('?') => Some(Act::Search { forward: false }),
        Input::Char('~') => Some(Act::ToggleCaseChar),
        Input::Char('r') => match inputs.get(at + 1) {
            None => return Parse::More,
            Some(Input::Char(c)) => Some(Act::Replace(*c)),
            Some(_) => return Parse::Bad,
        },
        Input::Char('z') => match inputs.get(at + 1) {
            None => return Parse::More,
            Some(n) => match two(Some(n)) {
                Some('z') | Some('.') => Some(Act::ScrollTo(ScrollTo::Center)),
                Some('t') => Some(Act::ScrollTo(ScrollTo::Top)),
                Some('b') | Some('-') => Some(Act::ScrollTo(ScrollTo::Bottom)),
                _ => return Parse::Bad,
            },
        },
        Input::Char('Z') => match inputs.get(at + 1) {
            None => return Parse::More,
            Some(n) => match two(Some(n)) {
                Some('Z') => Some(Act::SaveQuit),
                Some('Q') => Some(Act::QuitForce),
                _ => return Parse::Bad,
            },
        },
        Input::Ctrl('w') => match inputs.get(at + 1) {
            None => return Parse::More,
            Some(Input::Char(c)) => Some(Act::Window(*c)),
            Some(Input::Ctrl(c)) => Some(Act::Window(*c)),
            Some(_) => return Parse::Bad,
        },
        _ => None,
    };
    if let Some(act) = act {
        return done(None, Target::Act(act));
    }
    match parse_motion(inputs, at) {
        Err(()) => Parse::More,
        Ok(Some((m, _))) => done(None, Target::Motion(m)),
        Ok(None) => Parse::Bad,
    }
}

/// Find `pat` in `line` from byte `from`, honouring smartcase.
fn find_in(line: &str, pat: &str, from: usize, fold: bool) -> Option<usize> {
    if pat.is_empty() || from > line.len() {
        return None;
    }
    if fold {
        let l = line.to_ascii_lowercase();
        let p = pat.to_ascii_lowercase();
        l.get(from..)?.find(&p).map(|i| i + from)
    } else {
        line.get(from..)?.find(pat).map(|i| i + from)
    }
}

fn rfind_in(line: &str, pat: &str, before: usize, fold: bool) -> Option<usize> {
    if pat.is_empty() || before == 0 {
        return None;
    }
    let mut end = (before + pat.len() - 1).min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    let hay = line.get(..end)?;
    let found = if fold {
        hay.to_ascii_lowercase().rfind(&pat.to_ascii_lowercase())
    } else {
        hay.rfind(pat)
    };
    found.filter(|&i| i < before)
}

pub fn smartcase(pat: &str) -> bool {
    !pat.chars().any(|c| c.is_uppercase())
}

/// Every match of `pat` on one line, as byte ranges, for highlighting.
pub fn matches_on_line(line: &str, pat: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let fold = smartcase(pat);
    let mut from = 0;
    while let Some(i) = find_in(line, pat, from, fold) {
        out.push((i, i + pat.len()));
        from = i + pat.len().max(1);
        if out.len() > 500 {
            break;
        }
    }
    out
}

impl Vim {
    pub fn new() -> Self {
        Vim {
            mode: Mode::Normal,
            pending: Vec::new(),
            cmdline: String::new(),
            history: Vec::new(),
            history_at: None,
            cmd_from: Mode::Normal,
            registers: BTreeMap::new(),
            last_search: None,
            highlight: false,
            last_find: None,
            last_change: None,
            recording: None,
            insert_count: 1,
            insert_from: Pos::default(),
            visual_marks: None,
            message: None,
            tab: 4,
            shiftwidth: 4,
            expandtab: true,
            relative_numbers: false,
            select_return: Mode::Normal,
            showcmd: String::new(),
        }
    }

    pub fn register(&self, name: char) -> Option<&Register> {
        self.registers.get(&name)
    }

    fn set_register(&mut self, name: Option<char>, text: String, linewise: bool) {
        let r = Register { text, linewise };
        match name {
            Some('_') => {}
            Some(c) if c.is_ascii_alphabetic() => {
                let c = c.to_ascii_lowercase();
                self.registers.insert(c, r.clone());
                self.registers.insert('"', r);
            }
            _ => {
                self.registers.insert('"', r);
            }
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
    }

    /// The selection as `(start, end)` with `end` exclusive, and whether it
    /// is whole lines. `None` outside visual and select modes.
    pub fn selection(&self, buf: &Buffer, cur: &Cursor) -> Option<(Pos, Pos, bool)> {
        let (a, b) = if cur.anchor <= cur.pos {
            (cur.anchor, cur.pos)
        } else {
            (cur.pos, cur.anchor)
        };
        match self.mode {
            Mode::Visual => Some((a, Pos::new(b.line, buf.next_col(b.line, b.col).max(b.col)), false)),
            Mode::VisualLine => Some((Pos::new(a.line, 0), Pos::new(b.line, buf.line_len(b.line)), true)),
            Mode::Select if a != b => Some((a, b, false)),
            _ => None,
        }
    }

    /// Enter insert mode at the cursor without a command (a click into the
    /// text, a mouse user starting to type).
    pub fn begin_insert(&mut self, buf: &mut Buffer, cur: &mut Cursor) {
        self.pending.clear();
        self.showcmd.clear();
        self.mode = Mode::Insert;
        self.insert_count = 1;
        self.insert_from = cur.pos;
        self.recording = None;
        buf.end_group(cur.pos);
        buf.begin_group(cur.pos);
    }

    /// After the cursor was placed from outside (a click), aim `j` and `k`
    /// at its column.
    pub fn sync_want(&self, buf: &Buffer, cur: &mut Cursor) {
        self.set_want(buf, cur);
    }

    /// Start a selection from `anchor` to the cursor (a mouse drag).
    pub fn begin_select(&mut self, buf: &mut Buffer, cur: &mut Cursor, anchor: Pos) {
        if self.mode == Mode::Insert {
            buf.end_group(cur.pos);
            self.select_return = Mode::Insert;
        } else if !self.mode.is_visual() {
            self.select_return = self.mode;
        }
        self.pending.clear();
        self.showcmd.clear();
        cur.anchor = anchor;
        self.mode = Mode::Select;
    }

    /// Drop out of whatever mode to normal, closing any undo group.
    pub fn to_normal(&mut self, buf: &mut Buffer, cur: &mut Cursor) {
        if self.mode == Mode::Insert {
            buf.end_group(cur.pos);
        }
        self.mode = Mode::Normal;
        self.pending.clear();
        self.showcmd.clear();
        self.clamp_normal(buf, cur);
    }

    fn clamp_normal(&self, buf: &Buffer, cur: &mut Cursor) {
        cur.pos = buf.clamp(cur.pos);
        if !matches!(self.mode, Mode::Insert | Mode::Select) {
            let max = buf.last_char_col(cur.pos.line);
            if cur.pos.col > max {
                cur.pos.col = max;
            }
        }
    }

    fn set_want(&self, buf: &Buffer, cur: &mut Cursor) {
        cur.want = text::display_col(buf.line(cur.pos.line), cur.pos.col, self.tab);
    }

    /// Feed one input. Effects come back for the workspace.
    pub fn feed(&mut self, ctx: &mut Ctx, input: Input) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Some((_, rec)) = self.recording.as_mut() {
            rec.push(input.clone());
        }
        match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal(ctx, input, &mut fx),
            Mode::Insert => self.insert(ctx, input, &mut fx),
            Mode::Select => self.select(ctx, input, &mut fx),
            Mode::Command | Mode::Search { .. } => self.cmdline_key(ctx, input, &mut fx),
        }
        fx
    }

    // ---- normal and visual -------------------------------------------------

    fn normal(&mut self, ctx: &mut Ctx, input: Input, fx: &mut Vec<Effect>) {
        if let Input::Paste(text) = &input {
            let at = ctx.cur.pos;
            ctx.buf.begin_group(at);
            let end = ctx.buf.insert(at, text);
            ctx.buf.end_group(end);
            ctx.cur.pos = end;
            self.clamp_normal(ctx.buf, ctx.cur);
            return;
        }
        if input == Input::Key(Key::Esc) {
            if !self.pending.is_empty() {
                self.pending.clear();
                self.showcmd.clear();
                self.recording = None;
                return;
            }
            if self.mode.is_visual() {
                self.leave_visual(ctx);
            }
            self.recording = None;
            return;
        }
        // Shift with an arrow starts a selection a mouse user expects.
        if let Input::Shift(k) = input {
            let anchor = if self.mode.is_visual() { ctx.cur.anchor } else { ctx.cur.pos };
            self.begin_select(ctx.buf, ctx.cur, anchor);
            self.select_return = Mode::Normal;
            self.shift_move(ctx, k);
            return;
        }
        if self.mode.is_visual() {
            if let Some(done) = self.visual_key(ctx, &input, fx) {
                if done {
                    return;
                }
            }
        }
        self.pending.push(input);
        let parsed = parse(&self.pending);
        match parsed {
            Parse::More => {
                self.showcmd = self.pending_text();
            }
            Parse::Bad => {
                self.pending.clear();
                self.showcmd.clear();
            }
            Parse::Done(cmd) => {
                let keys = core::mem::take(&mut self.pending);
                self.showcmd.clear();
                self.run(ctx, cmd, &keys, fx);
            }
        }
    }

    fn pending_text(&self) -> String {
        let mut s = String::new();
        for i in &self.pending {
            match i {
                Input::Char(c) => s.push(*c),
                Input::Ctrl(c) => {
                    s.push('^');
                    s.push(c.to_ascii_uppercase());
                }
                _ => {}
            }
        }
        s
    }

    fn leave_visual(&mut self, ctx: &mut Ctx) {
        let (a, b) = if ctx.cur.anchor <= ctx.cur.pos {
            (ctx.cur.anchor, ctx.cur.pos)
        } else {
            (ctx.cur.pos, ctx.cur.anchor)
        };
        self.visual_marks = Some((a, b, self.mode == Mode::VisualLine));
        self.mode = Mode::Normal;
        self.clamp_normal(ctx.buf, ctx.cur);
    }

    /// Keys that mean something different with a selection. `Some(true)`
    /// when handled.
    fn visual_key(&mut self, ctx: &mut Ctx, input: &Input, fx: &mut Vec<Effect>) -> Option<bool> {
        if !self.pending.is_empty() {
            // Mid-command (`f` waiting for its char, `g` waiting...): only
            // text objects and motions continue, handled by the parser.
            if let Some(Input::Char(c @ ('i' | 'a'))) = self.pending.last() {
                let around = *c == 'a';
                let mut probe = alloc::vec![Input::Char(if around { 'a' } else { 'i' })];
                probe.push(input.clone());
                if let Ok(Some((obj, _))) = parse_object(&probe, 0) {
                    self.pending.clear();
                    if let Some((a, b, _)) = self.object_range(ctx.buf, ctx.cur.pos, obj, 1) {
                        ctx.cur.anchor = a;
                        ctx.cur.pos = ctx.buf.prev_pos(b).unwrap_or(b);
                        if self.mode == Mode::VisualLine {
                            self.mode = Mode::Visual;
                        }
                    }
                    return Some(true);
                }
            }
            return None;
        }
        if matches!(input, Input::Char('i') | Input::Char('a')) {
            self.pending.push(input.clone());
            return Some(true);
        }
        let sel = self.selection(ctx.buf, ctx.cur)?;
        // Capitals act on whole lines, whatever the selection.
        let whole = matches!(input, Input::Char('D' | 'X' | 'Y' | 'C' | 'S' | 'R'));
        let sel = if whole { (Pos::new(sel.0.line, 0), Pos::new(sel.1.line, ctx.buf.line_len(sel.1.line)), true) } else { sel };
        let op = match input {
            Input::Char('D') | Input::Char('X') => Some(Op::Delete),
            Input::Char('Y') => Some(Op::Yank),
            Input::Char('C') | Input::Char('S') | Input::Char('R') => Some(Op::Change),
            Input::Char('d') | Input::Char('x') | Input::Key(Key::Delete) => Some(Op::Delete),
            Input::Char('c') | Input::Char('s') => Some(Op::Change),
            Input::Char('y') => Some(Op::Yank),
            Input::Char('>') => Some(Op::Indent),
            Input::Char('<') => Some(Op::Dedent),
            Input::Char('u') => Some(Op::Lower),
            Input::Char('U') => Some(Op::Upper),
            Input::Char('~') => Some(Op::ToggleCase),
            _ => None,
        };
        if let Some(op) = op {
            let (a, b, linewise) = sel;
            self.visual_marks = Some((a, b, linewise));
            let kind = if linewise { MKind::Linewise } else { MKind::Exclusive };
            self.mode = Mode::Normal;
            ctx.buf.begin_group(ctx.cur.pos);
            self.apply_op(ctx, op, a, b, kind, None, fx);
            if self.mode != Mode::Insert {
                ctx.buf.end_group(ctx.cur.pos);
                self.clamp_normal(ctx.buf, ctx.cur);
            }
            return Some(true);
        }
        match input {
            Input::Char('o') => {
                core::mem::swap(&mut ctx.cur.pos, &mut ctx.cur.anchor);
                Some(true)
            }
            Input::Char('v') => {
                if self.mode == Mode::Visual {
                    self.leave_visual(ctx);
                } else {
                    self.mode = Mode::Visual;
                }
                Some(true)
            }
            Input::Char('V') => {
                if self.mode == Mode::VisualLine {
                    self.leave_visual(ctx);
                } else {
                    self.mode = Mode::VisualLine;
                }
                Some(true)
            }
            Input::Char('J') => {
                let (a, b, _) = sel;
                let n = (b.line - a.line + 1).max(2);
                self.mode = Mode::Normal;
                ctx.cur.pos = Pos::new(a.line, 0);
                ctx.buf.begin_group(ctx.cur.pos);
                self.join(ctx, n);
                ctx.buf.end_group(ctx.cur.pos);
                Some(true)
            }
            Input::Char('p') | Input::Char('P') => {
                let (a, b, linewise) = sel;
                let reg = self.registers.get(&'"').cloned().unwrap_or_default();
                self.mode = Mode::Normal;
                ctx.buf.begin_group(ctx.cur.pos);
                let (from, to) = if linewise { (Pos::new(a.line, 0), Pos::new(b.line, ctx.buf.line_len(b.line))) } else { (a, b) };
                let mut removed = ctx.buf.slice(from, to);
                if linewise {
                    removed.push('\n');
                }
                let mut text = reg.text.clone();
                if reg.linewise {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    if !linewise {
                        text.insert(0, '\n');
                        text.push('\n');
                    }
                }
                let end = ctx.buf.replace(from, to, &text);
                self.set_register(None, removed, linewise);
                ctx.cur.pos = if reg.linewise { Pos::new(from.line, ctx.buf.first_non_blank(from.line)) } else { ctx.buf.prev_pos(end).unwrap_or(from) };
                ctx.buf.end_group(ctx.cur.pos);
                self.clamp_normal(ctx.buf, ctx.cur);
                Some(true)
            }
            Input::Char(':') => {
                self.visual_marks = Some((sel.0, sel.1, sel.2));
                self.mode = Mode::Normal;
                self.clamp_normal(ctx.buf, ctx.cur);
                self.cmd_from = Mode::Visual;
                self.open_cmdline(Mode::Command, "'<,'>");
                Some(true)
            }
            Input::Char('r') => None,
            _ => {
                let _ = fx;
                None
            }
        }
    }

    fn open_cmdline(&mut self, mode: Mode, seed: &str) {
        self.mode = mode;
        self.cmdline = String::from(seed);
        self.history_at = None;
    }

    fn is_change(cmd: &Cmd) -> bool {
        match cmd.target {
            Target::Act(a) => !matches!(
                a,
                Act::Undo
                    | Act::Redo
                    | Act::Repeat
                    | Act::Visual
                    | Act::VisualLine
                    | Act::CmdLine
                    | Act::Search { .. }
                    | Act::YankLine
                    | Act::ScrollTo(_)
                    | Act::SaveQuit
                    | Act::QuitForce
                    | Act::Window(_)
            ),
            _ => matches!(cmd.op, Some(op) if op != Op::Yank),
        }
    }

    fn run(&mut self, ctx: &mut Ctx, cmd: Cmd, keys: &[Input], fx: &mut Vec<Effect>) {
        let change = Self::is_change(&cmd) && !self.mode.is_visual();
        if change {
            let rest: Vec<Input> = keys.iter().skip(cmd.count_len).cloned().collect();
            let count = if cmd.count_len > 0 { cmd.count } else { None };
            self.recording = Some((count, rest));
            ctx.buf.begin_group(ctx.cur.pos);
        }
        self.exec(ctx, cmd, fx);
        if change && self.mode != Mode::Insert {
            ctx.buf.end_group(ctx.cur.pos);
            if let Some(rec) = self.recording.take() {
                self.last_change = Some(rec);
            }
        }
        if !matches!(self.mode, Mode::Insert | Mode::Command | Mode::Search { .. }) {
            self.clamp_normal(ctx.buf, ctx.cur);
        }
    }

    fn exec(&mut self, ctx: &mut Ctx, cmd: Cmd, fx: &mut Vec<Effect>) {
        let n = cmd.count.unwrap_or(1);
        match (cmd.op, cmd.target) {
            (None, Target::Motion(m)) => {
                if let Some((p, _)) = self.motion(ctx, m, cmd.count) {
                    ctx.cur.pos = p;
                    match m {
                        Motion::Up | Motion::Down => {}
                        Motion::LineEnd => ctx.cur.want = usize::MAX,
                        _ => {
                            self.clamp_normal(ctx.buf, ctx.cur);
                            self.set_want(ctx.buf, ctx.cur);
                        }
                    }
                    match m {
                        Motion::HalfDown | Motion::PageDown => {
                            let step = if m == Motion::HalfDown { ctx.page / 2 } else { ctx.page.saturating_sub(2) };
                            fx.push(Effect::ScrollLines((step.max(1) * n) as isize));
                        }
                        Motion::HalfUp | Motion::PageUp => {
                            let step = if m == Motion::HalfUp { ctx.page / 2 } else { ctx.page.saturating_sub(2) };
                            fx.push(Effect::ScrollLines(-((step.max(1) * n) as isize)));
                        }
                        _ => {}
                    }
                }
            }
            (Some(op), Target::Lines) => {
                let a = ctx.cur.pos.line;
                let b = (a + n - 1).min(ctx.buf.last_line());
                self.apply_op(ctx, op, Pos::new(a, 0), Pos::new(b, 0), MKind::Linewise, cmd.reg, fx);
            }
            (Some(op), Target::Motion(m)) => {
                let start = ctx.cur.pos;
                // `cw` changes to the end of the word, as Vim does.
                let m = match (op, m) {
                    (Op::Change, Motion::WordFwd(big))
                        if class(ctx.buf.char_at(start)) != Class::Blank =>
                    {
                        Motion::WordEnd(big)
                    }
                    _ => m,
                };
                let at_word_end = matches!(m, Motion::WordEnd(_)) && op == Op::Change;
                let Some((mut end, mut kind)) = (if at_word_end {
                    self.word_end_from(ctx.buf, start, n, matches!(m, Motion::WordEnd(true)))
                        .map(|p| (p, MKind::Inclusive))
                } else {
                    self.motion(ctx, m, cmd.count)
                }) else {
                    return;
                };
                // `dw` on the last word of a line stops at the line's end.
                if matches!(m, Motion::WordFwd(_)) && end.line > start.line {
                    let fnb = ctx.buf.first_non_blank(end.line);
                    if end.col <= fnb {
                        let l = end.line - 1;
                        end = Pos::new(l, ctx.buf.line_len(l));
                        if end < start {
                            end = start;
                        }
                    }
                }
                if kind == MKind::Exclusive && end > start && end.col == 0 && end.line > start.line {
                    // An exclusive motion ending at a line's start ends at
                    // the end of the line before (`d}`).
                    let l = end.line - 1;
                    end = Pos::new(l, ctx.buf.line_len(l));
                    if start.col <= ctx.buf.first_non_blank(start.line) && end.col >= ctx.buf.line_len(end.line) && matches!(m, Motion::ParaFwd) {
                        kind = MKind::Linewise;
                    }
                }
                let (a, b) = if end < start { (end, start) } else { (start, end) };
                let b = if kind == MKind::Inclusive {
                    let nc = ctx.buf.next_col(b.line, b.col);
                    Pos::new(b.line, nc.max(b.col))
                } else {
                    b
                };
                if matches!(m, Motion::Left | Motion::Right) && a == b {
                    return;
                }
                self.apply_op(ctx, op, a, b, kind, cmd.reg, fx);
                if op == Op::Yank && end < start {
                    ctx.cur.pos = end;
                }
            }
            (Some(op), Target::Object(obj)) => {
                if let Some((a, b, linewise)) = self.object_range(ctx.buf, ctx.cur.pos, obj, n) {
                    let kind = if linewise { MKind::Linewise } else { MKind::Exclusive };
                    let (a, b) = if linewise { (Pos::new(a.line, 0), Pos::new(b.line, 0)) } else { (a, b) };
                    self.apply_op(ctx, op, a, b, kind, cmd.reg, fx);
                }
            }
            (None, Target::Act(act)) => self.act(ctx, act, n, cmd.count, cmd.reg, fx),
            _ => {}
        }
    }

    /// The span of whole lines `a..=b`, including the line end that joins
    /// them to the rest (the one before, at the end of the file).
    fn line_span(&self, buf: &Buffer, a: usize, b: usize) -> (Pos, Pos) {
        if b < buf.last_line() {
            (Pos::new(a, 0), Pos::new(b + 1, 0))
        } else if a > 0 {
            (Pos::new(a - 1, buf.line_len(a - 1)), Pos::new(b, buf.line_len(b)))
        } else {
            (Pos::new(0, 0), Pos::new(b, buf.line_len(b)))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_op(&mut self, ctx: &mut Ctx, op: Op, a: Pos, b: Pos, kind: MKind, reg: Option<char>, fx: &mut Vec<Effect>) {
        let _ = fx;
        let linewise = kind == MKind::Linewise;
        let (la, lb) = (a.line.min(b.line), a.line.max(b.line));
        match op {
            Op::Yank => {
                let text = if linewise {
                    let mut t = ctx.buf.slice(Pos::new(la, 0), Pos::new(lb, ctx.buf.line_len(lb)));
                    t.push('\n');
                    t
                } else {
                    ctx.buf.slice(a, b)
                };
                let lines = lb - la + 1;
                self.set_register(reg, text, linewise);
                if linewise && lines > 2 {
                    self.info(alloc::format!("{lines} lines yanked"));
                }
                if linewise {
                    ctx.cur.pos = Pos::new(la, ctx.cur.pos.col);
                } else {
                    ctx.cur.pos = a.min(ctx.cur.pos);
                }
            }
            Op::Delete => {
                if linewise {
                    let mut t = ctx.buf.slice(Pos::new(la, 0), Pos::new(lb, ctx.buf.line_len(lb)));
                    t.push('\n');
                    let (from, to) = self.line_span(ctx.buf, la, lb);
                    ctx.buf.replace(from, to, "");
                    self.set_register(reg, t, true);
                    let l = la.min(ctx.buf.last_line());
                    ctx.cur.pos = Pos::new(l, ctx.buf.first_non_blank(l));
                    let lines = lb - la + 1;
                    if lines > 2 {
                        self.info(alloc::format!("{lines} fewer lines"));
                    }
                } else {
                    let t = ctx.buf.delete(a, b);
                    self.set_register(reg, t, false);
                    ctx.cur.pos = a;
                }
            }
            Op::Change => {
                if linewise {
                    let mut t = ctx.buf.slice(Pos::new(la, 0), Pos::new(lb, ctx.buf.line_len(lb)));
                    t.push('\n');
                    let indent = String::from(text::indent_of(ctx.buf.line(la)));
                    let end = Pos::new(lb, ctx.buf.line_len(lb));
                    ctx.buf.replace(Pos::new(la, 0), end, &indent);
                    self.set_register(reg, t, true);
                    ctx.cur.pos = Pos::new(la, indent.len());
                } else {
                    let t = ctx.buf.delete(a, b);
                    self.set_register(reg, t, false);
                    ctx.cur.pos = a;
                }
                self.mode = Mode::Insert;
                self.insert_count = 1;
                self.insert_from = ctx.cur.pos;
            }
            Op::Indent | Op::Dedent => {
                let lb = if !linewise && b.col == 0 && b.line > la { b.line - 1 } else { lb };
                for l in la..=lb {
                    self.shift_line(ctx.buf, l, op == Op::Indent);
                }
                ctx.cur.pos = Pos::new(la, ctx.buf.first_non_blank(la));
                let n = lb - la + 1;
                if n > 2 {
                    self.info(alloc::format!("{n} lines {}ed 1 time", if op == Op::Indent { ">" } else { "<" }));
                }
            }
            Op::Lower | Op::Upper | Op::ToggleCase => {
                let (from, to) = if linewise {
                    (Pos::new(la, 0), Pos::new(lb, ctx.buf.line_len(lb)))
                } else {
                    (a, b)
                };
                let t = ctx.buf.slice(from, to);
                let changed: String = t
                    .chars()
                    .map(|c| match op {
                        Op::Lower => c.to_ascii_lowercase(),
                        Op::Upper => c.to_ascii_uppercase(),
                        _ => {
                            if c.is_ascii_uppercase() {
                                c.to_ascii_lowercase()
                            } else {
                                c.to_ascii_uppercase()
                            }
                        }
                    })
                    .collect();
                if changed != t {
                    ctx.buf.replace(from, to, &changed);
                }
                ctx.cur.pos = from;
            }
        }
    }

    fn shift_line(&self, buf: &mut Buffer, line: usize, right: bool) {
        let s = buf.line(line);
        if s.is_empty() && right {
            return;
        }
        let indent = text::indent_of(s);
        let width = text::display_width(indent, self.tab);
        let target = if right {
            (width / self.shiftwidth + 1) * self.shiftwidth
        } else {
            if width == 0 {
                return;
            }
            ((width - 1) / self.shiftwidth) * self.shiftwidth
        };
        let new_indent = self.indent_string(target);
        let len = indent.len();
        buf.replace(Pos::new(line, 0), Pos::new(line, len), &new_indent);
    }

    fn indent_string(&self, width: usize) -> String {
        let mut s = String::new();
        if self.expandtab {
            for _ in 0..width {
                s.push(' ');
            }
        } else {
            for _ in 0..width / self.tab {
                s.push('\t');
            }
            for _ in 0..width % self.tab {
                s.push(' ');
            }
        }
        s
    }

    fn enter_insert(&mut self, ctx: &mut Ctx, at: Pos, count: usize) {
        ctx.cur.pos = at;
        self.mode = Mode::Insert;
        self.insert_count = count.max(1);
        self.insert_from = at;
        if !ctx.buf.group_open() {
            ctx.buf.begin_group(at);
        }
    }

    fn act(&mut self, ctx: &mut Ctx, act: Act, n: usize, count: Option<usize>, reg: Option<char>, fx: &mut Vec<Effect>) {
        let p = ctx.cur.pos;
        match act {
            Act::Insert => self.enter_insert(ctx, p, n),
            Act::Append => {
                let col = ctx.buf.next_col(p.line, p.col);
                self.enter_insert(ctx, Pos::new(p.line, col), n)
            }
            Act::InsertLineStart => {
                let col = ctx.buf.first_non_blank(p.line);
                self.enter_insert(ctx, Pos::new(p.line, col), n)
            }
            Act::AppendLineEnd => {
                let col = ctx.buf.line_len(p.line);
                self.enter_insert(ctx, Pos::new(p.line, col), n)
            }
            Act::OpenBelow | Act::OpenAbove => {
                let below = act == Act::OpenBelow;
                let indent = self.auto_indent_for(ctx.buf, p.line, below);
                let at = if below {
                    let end = Pos::new(p.line, ctx.buf.line_len(p.line));
                    let mut t = String::from("\n");
                    t.push_str(&indent);
                    ctx.buf.insert(end, &t)
                } else {
                    let mut t = indent.clone();
                    t.push('\n');
                    ctx.buf.insert(Pos::new(p.line, 0), &t);
                    Pos::new(p.line, indent.len())
                };
                self.enter_insert(ctx, at, 1);
                self.insert_count = 1;
            }
            Act::DeleteChar => {
                let len = ctx.buf.line_len(p.line);
                if len == 0 {
                    return;
                }
                let mut end = p.col;
                for _ in 0..n {
                    end = ctx.buf.next_col(p.line, end);
                }
                let t = ctx.buf.delete(p, Pos::new(p.line, end.min(len)));
                self.set_register(reg, t, false);
            }
            Act::DeleteCharBack => {
                let mut start = p.col;
                for _ in 0..n {
                    start = ctx.buf.prev_col(p.line, start);
                }
                if start < p.col {
                    let t = ctx.buf.delete(Pos::new(p.line, start), p);
                    self.set_register(reg, t, false);
                    ctx.cur.pos = Pos::new(p.line, start);
                }
            }
            Act::DeleteToEnd | Act::ChangeToEnd => {
                let last = (p.line + n - 1).min(ctx.buf.last_line());
                let end = Pos::new(last, ctx.buf.line_len(last));
                let t = ctx.buf.delete(p, end);
                self.set_register(reg, t, false);
                if act == Act::ChangeToEnd {
                    self.enter_insert(ctx, p, 1);
                }
            }
            Act::SubstChar => {
                let len = ctx.buf.line_len(p.line);
                let mut end = p.col;
                for _ in 0..n {
                    end = ctx.buf.next_col(p.line, end);
                }
                if len > 0 {
                    let t = ctx.buf.delete(p, Pos::new(p.line, end.min(len)));
                    self.set_register(reg, t, false);
                }
                self.enter_insert(ctx, p, 1);
            }
            Act::SubstLine => {
                let b = (p.line + n - 1).min(ctx.buf.last_line());
                self.apply_op(ctx, Op::Change, Pos::new(p.line, 0), Pos::new(b, 0), MKind::Linewise, reg, fx);
            }
            Act::YankLine => {
                let b = (p.line + n - 1).min(ctx.buf.last_line());
                self.apply_op(ctx, Op::Yank, Pos::new(p.line, 0), Pos::new(b, 0), MKind::Linewise, reg, fx);
            }
            Act::Replace(c) => {
                let len = ctx.buf.line_len(p.line);
                let mut end = p.col;
                let mut k = 0;
                while k < n && end < len {
                    end = ctx.buf.next_col(p.line, end);
                    k += 1;
                }
                if k < n {
                    return;
                }
                let mut s = String::new();
                for _ in 0..n {
                    s.push(c);
                }
                ctx.buf.replace(p, Pos::new(p.line, end), &s);
                ctx.cur.pos = Pos::new(p.line, p.col + s.len() - c.len_utf8());
            }
            Act::Join => self.join(ctx, n.max(2)),
            Act::PutAfter | Act::PutBefore => {
                let name = reg.unwrap_or('"').to_ascii_lowercase();
                let Some(r) = self.registers.get(&name).cloned() else {
                    self.error("Nothing in the register yet: yank something with y first");
                    return;
                };
                let mut t = String::new();
                for _ in 0..n {
                    t.push_str(&r.text);
                }
                let after = act == Act::PutAfter;
                if r.linewise {
                    let at = if after {
                        let l = p.line;
                        if l == ctx.buf.last_line() {
                            // Put below the last line: a line end comes first.
                            let end = Pos::new(l, ctx.buf.line_len(l));
                            let mut body = String::from("\n");
                            body.push_str(t.strip_suffix('\n').unwrap_or(&t));
                            ctx.buf.insert(end, &body);
                            ctx.cur.pos = Pos::new(l + 1, ctx.buf.first_non_blank(l + 1));
                            return;
                        }
                        Pos::new(l + 1, 0)
                    } else {
                        Pos::new(p.line, 0)
                    };
                    ctx.buf.insert(at, &t);
                    ctx.cur.pos = Pos::new(at.line, ctx.buf.first_non_blank(at.line));
                } else {
                    let at = if after && ctx.buf.line_len(p.line) > 0 {
                        Pos::new(p.line, ctx.buf.next_col(p.line, p.col))
                    } else {
                        p
                    };
                    let end = ctx.buf.insert(at, &t);
                    ctx.cur.pos = if t.contains('\n') { at } else { ctx.buf.prev_pos(end).unwrap_or(at) };
                }
            }
            Act::Undo => {
                let mut last = None;
                for _ in 0..n {
                    match ctx.buf.undo() {
                        Some(at) => last = Some(at),
                        None => break,
                    }
                }
                match last {
                    Some(at) => ctx.cur.pos = at,
                    None => self.info("Already at oldest change"),
                }
            }
            Act::Redo => {
                let mut last = None;
                for _ in 0..n {
                    match ctx.buf.redo() {
                        Some(at) => last = Some(at),
                        None => break,
                    }
                }
                match last {
                    Some(at) => ctx.cur.pos = at,
                    None => self.info("Already at newest change"),
                }
            }
            Act::Repeat => {
                let Some((old, keys)) = self.last_change.clone() else {
                    return;
                };
                let count = count.or(old);
                let mut seq: Vec<Input> = Vec::new();
                if let Some(c) = count {
                    for d in text::num(c).chars() {
                        seq.push(Input::Char(d));
                    }
                }
                seq.extend(keys);
                for i in seq {
                    let more = self.feed(ctx, i);
                    fx.extend(more);
                }
                if self.mode == Mode::Insert {
                    // The recorded change always ends its insert; be safe.
                    let more = self.feed(ctx, Input::Key(Key::Esc));
                    fx.extend(more);
                }
            }
            Act::Visual => {
                ctx.cur.anchor = p;
                self.mode = Mode::Visual;
            }
            Act::VisualLine => {
                ctx.cur.anchor = p;
                self.mode = Mode::VisualLine;
            }
            Act::CmdLine => {
                self.cmd_from = Mode::Normal;
                let seed = if count.is_some() {
                    let mut s = String::from(".,.+");
                    s.push_str(&text::num(n - 1));
                    s
                } else {
                    String::new()
                };
                self.open_cmdline(Mode::Command, &seed);
            }
            Act::Search { forward } => self.open_cmdline(Mode::Search { forward }, ""),
            Act::ToggleCaseChar => {
                let len = ctx.buf.line_len(p.line);
                let mut end = p.col;
                for _ in 0..n {
                    end = ctx.buf.next_col(p.line, end);
                }
                let end = end.min(len);
                let t = ctx.buf.slice(p, Pos::new(p.line, end));
                let flipped: String = t
                    .chars()
                    .map(|c| if c.is_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() })
                    .collect();
                if flipped != t {
                    ctx.buf.replace(p, Pos::new(p.line, end), &flipped);
                }
                ctx.cur.pos = Pos::new(p.line, end);
            }
            Act::ScrollTo(to) => fx.push(Effect::Scroll(to)),
            Act::SaveQuit => fx.push(Effect::WriteQuit),
            Act::QuitForce => fx.push(Effect::Quit { force: true }),
            Act::Window(c) => match c {
                'v' | 's' | 'V' | 'S' => fx.push(Effect::Split { path: None }),
                'w' | 'l' | 'L' => fx.push(Effect::FocusPane { forward: true }),
                'W' | 'h' | 'H' | 'p' => fx.push(Effect::FocusPane { forward: false }),
                'c' | 'q' => fx.push(Effect::ClosePane),
                'o' => fx.push(Effect::OnlyPane),
                _ => {}
            },
        }
    }

    fn join(&mut self, ctx: &mut Ctx, lines: usize) {
        let l = ctx.cur.pos.line;
        let mut col = ctx.buf.line_len(l);
        for _ in 1..lines {
            if l >= ctx.buf.last_line() {
                break;
            }
            let cur_len = ctx.buf.line_len(l);
            let next = ctx.buf.line(l + 1);
            let lead = next.len() - next.trim_start().len();
            let next_trim_starts_close = next.trim_start().starts_with(')');
            let cur_ends_space = ctx.buf.line(l).ends_with(' ');
            let sep = if next.trim().is_empty() || next_trim_starts_close || cur_ends_space || cur_len == 0 { "" } else { " " };
            ctx.buf.replace(Pos::new(l, cur_len), Pos::new(l + 1, lead), sep);
            col = cur_len;
        }
        ctx.cur.pos = Pos::new(l, col);
    }

    /// The indent a new line next to `line` should start with: the line's
    /// own, one level deeper after an opening bracket.
    fn auto_indent_for(&self, buf: &Buffer, line: usize, below: bool) -> String {
        let s = buf.line(line);
        let mut indent = String::from(text::indent_of(s));
        if below {
            let t = s.trim_end();
            if t.ends_with('{') || t.ends_with('(') || t.ends_with('[') {
                indent.push_str(&self.indent_string(self.shiftwidth));
            }
        }
        indent
    }

    // ---- motions -------------------------------------------------------------

    fn motion(&mut self, ctx: &mut Ctx, m: Motion, count: Option<usize>) -> Option<(Pos, MKind)> {
        let buf = &*ctx.buf;
        let p = ctx.cur.pos;
        let n = count.unwrap_or(1);
        let tab = self.tab;
        let vertical = |line: usize, want: usize| {
            let s = buf.line(line);
            Pos::new(line, text::col_for_display(s, want, tab))
        };
        let r = match m {
            Motion::Left => {
                let mut col = p.col;
                for _ in 0..n {
                    col = buf.prev_col(p.line, col);
                }
                (Pos::new(p.line, col), MKind::Exclusive)
            }
            Motion::Right => {
                let mut col = p.col;
                let len = buf.line_len(p.line);
                for _ in 0..n {
                    col = buf.next_col(p.line, col).min(len);
                }
                (Pos::new(p.line, col), MKind::Exclusive)
            }
            Motion::Down | Motion::Up => {
                let line = if m == Motion::Down {
                    (p.line + n).min(buf.last_line())
                } else {
                    p.line.saturating_sub(n)
                };
                if line == p.line && count.is_none() {
                    return None;
                }
                let want = if ctx.cur.want == usize::MAX { usize::MAX } else { ctx.cur.want };
                let mut to = vertical(line, want);
                if want == usize::MAX {
                    to.col = buf.line_len(line);
                }
                (to, MKind::Linewise)
            }
            Motion::WordFwd(big) => {
                let mut q = p;
                for _ in 0..n {
                    q = self.word_fwd(buf, q, big);
                }
                (q, MKind::Exclusive)
            }
            Motion::WordBack(big) => {
                let mut q = p;
                for _ in 0..n {
                    q = self.word_back(buf, q, big);
                }
                (q, MKind::Exclusive)
            }
            Motion::WordEnd(big) => (self.word_end_from(buf, p, n, big)?, MKind::Inclusive),
            Motion::WordEndBack(big) => {
                let mut q = p;
                for _ in 0..n {
                    q = self.word_end_back(buf, q, big);
                }
                (q, MKind::Inclusive)
            }
            Motion::LineStart => (Pos::new(p.line, 0), MKind::Exclusive),
            Motion::FirstNonBlank => (Pos::new(p.line, buf.first_non_blank(p.line)), MKind::Exclusive),
            Motion::LineEnd => {
                let line = (p.line + n - 1).min(buf.last_line());
                (Pos::new(line, buf.last_char_col(line)), MKind::Inclusive)
            }
            Motion::LastNonBlank => {
                let line = (p.line + n - 1).min(buf.last_line());
                let s = buf.line(line);
                let t = s.trim_end().len();
                (Pos::new(line, buf.prev_col(line, t)), MKind::Inclusive)
            }
            Motion::GotoLine { last } => {
                let line = match count {
                    Some(c) => (c - 1).min(buf.last_line()),
                    None if last => buf.last_line(),
                    None => 0,
                };
                (Pos::new(line, buf.first_non_blank(line)), MKind::Linewise)
            }
            Motion::NextLineStart | Motion::PrevLineStart => {
                let line = if m == Motion::NextLineStart {
                    (p.line + n).min(buf.last_line())
                } else {
                    p.line.saturating_sub(n)
                };
                (Pos::new(line, buf.first_non_blank(line)), MKind::Linewise)
            }
            Motion::MatchPair => (self.match_pair(buf, p)?, MKind::Inclusive),
            Motion::Find(kind, c) => {
                self.last_find = Some((kind, c));
                (self.find_char(buf, p, kind, c, n, false)?, find_kind(kind))
            }
            Motion::RepeatFind { reverse } => {
                let (kind, c) = self.last_find?;
                let kind = if reverse {
                    match kind {
                        FindKind::To => FindKind::BackTo,
                        FindKind::Till => FindKind::BackTill,
                        FindKind::BackTo => FindKind::To,
                        FindKind::BackTill => FindKind::Till,
                    }
                } else {
                    kind
                };
                (self.find_char(buf, p, kind, c, n, true)?, find_kind(kind))
            }
            Motion::ParaFwd => {
                let mut l = p.line;
                for _ in 0..n {
                    while l < buf.last_line() && buf.line(l).trim().is_empty() {
                        l += 1;
                    }
                    while l < buf.last_line() && !buf.line(l).trim().is_empty() {
                        l += 1;
                    }
                }
                let col = if l == buf.last_line() && !buf.line(l).trim().is_empty() { buf.line_len(l) } else { 0 };
                (Pos::new(l, col), MKind::Exclusive)
            }
            Motion::ParaBack => {
                let mut l = p.line;
                for _ in 0..n {
                    while l > 0 && buf.line(l).trim().is_empty() {
                        l -= 1;
                    }
                    while l > 0 && !buf.line(l).trim().is_empty() {
                        l -= 1;
                    }
                }
                (Pos::new(l, 0), MKind::Exclusive)
            }
            Motion::ScreenTop => {
                let l = (ctx.top + count.unwrap_or(1) - 1).min(buf.last_line());
                (Pos::new(l, buf.first_non_blank(l)), MKind::Linewise)
            }
            Motion::ScreenMid => {
                let bottom = (ctx.top + ctx.page.max(1) - 1).min(buf.last_line());
                let l = (ctx.top + bottom) / 2;
                (Pos::new(l, buf.first_non_blank(l)), MKind::Linewise)
            }
            Motion::ScreenBottom => {
                let bottom = (ctx.top + ctx.page.max(1) - 1).min(buf.last_line());
                let l = bottom.saturating_sub(count.unwrap_or(1) - 1).max(ctx.top.min(bottom));
                (Pos::new(l, buf.first_non_blank(l)), MKind::Linewise)
            }
            Motion::HalfDown | Motion::HalfUp | Motion::PageDown | Motion::PageUp => {
                let step = match m {
                    Motion::HalfDown | Motion::HalfUp => (ctx.page / 2).max(1),
                    _ => ctx.page.saturating_sub(2).max(1),
                } * n;
                let line = if matches!(m, Motion::HalfDown | Motion::PageDown) {
                    (p.line + step).min(buf.last_line())
                } else {
                    p.line.saturating_sub(step)
                };
                (Pos::new(line, buf.first_non_blank(line)), MKind::Linewise)
            }
            Motion::SearchNext { reverse } => {
                let (pat, fwd) = self.last_search.clone()?;
                self.highlight = true;
                let forward = fwd != reverse;
                let mut q = p;
                for _ in 0..n {
                    q = self.search_from(buf, q, &pat, forward)?;
                }
                (q, MKind::Exclusive)
            }
            Motion::SearchWord { forward } => {
                let (a, b) = self.word_under(buf, p)?;
                let word = String::from(buf.line(p.line).get(a..b)?);
                self.last_search = Some((word.clone(), forward));
                self.highlight = true;
                let mut q = Pos::new(p.line, a);
                for _ in 0..n {
                    q = self.search_from(buf, q, &word, forward)?;
                }
                (q, MKind::Exclusive)
            }
            Motion::Definition => {
                let (a, b) = self.word_under(buf, p)?;
                let word = String::from(buf.line(p.line).get(a..b)?);
                (self.definition_of(buf, &word).unwrap_or(p), MKind::Exclusive)
            }
        };
        Some(r)
    }

    fn word_fwd(&self, buf: &Buffer, mut p: Pos, big: bool) -> Pos {
        let cls = |c| if big { big_class(c) } else { class(c) };
        let start = cls(buf.char_at(p));
        if start != Class::Blank {
            while cls(buf.char_at(p)) == start {
                match buf.next_pos(p) {
                    Some(n) => p = n,
                    None => return buf.end(),
                }
            }
        }
        loop {
            match buf.char_at(p) {
                None => return p,
                Some('\n') => {
                    let Some(n) = buf.next_pos(p) else { return p };
                    p = n;
                    if buf.line_len(p.line) == 0 {
                        return p;
                    }
                }
                Some(c) if c.is_whitespace() => match buf.next_pos(p) {
                    Some(n) => p = n,
                    None => return p,
                },
                Some(_) => return p,
            }
        }
    }

    fn word_back(&self, buf: &Buffer, p: Pos, big: bool) -> Pos {
        let cls = |c| if big { big_class(c) } else { class(c) };
        let Some(mut q) = buf.prev_pos(p) else { return p };
        // Skip blanks back, stopping on an empty line.
        loop {
            if buf.line_len(q.line) == 0 && q.col == 0 && q != p {
                return q;
            }
            match buf.char_at(q) {
                Some(c) if c.is_whitespace() => match buf.prev_pos(q) {
                    Some(n) => q = n,
                    None => return q,
                },
                _ => break,
            }
        }
        let c = cls(buf.char_at(q));
        while let Some(prev) = buf.prev_pos(q) {
            if prev.line != q.line || cls(buf.char_at(prev)) != c {
                break;
            }
            q = prev;
        }
        q
    }

    fn word_end_from(&self, buf: &Buffer, p: Pos, n: usize, big: bool) -> Option<Pos> {
        let mut q = p;
        for _ in 0..n {
            q = self.word_end(buf, q, big)?;
        }
        Some(q)
    }

    fn word_end(&self, buf: &Buffer, p: Pos, big: bool) -> Option<Pos> {
        let cls = |c| if big { big_class(c) } else { class(c) };
        let mut q = buf.next_pos(p)?;
        while cls(buf.char_at(q)) == Class::Blank {
            q = buf.next_pos(q)?;
        }
        let c = cls(buf.char_at(q));
        while let Some(next) = buf.next_pos(q) {
            if next.line != q.line || cls(buf.char_at(next)) != c {
                break;
            }
            q = next;
        }
        Some(q)
    }

    fn word_end_back(&self, buf: &Buffer, p: Pos, big: bool) -> Pos {
        let cls = |c| if big { big_class(c) } else { class(c) };
        let start = cls(buf.char_at(p));
        let mut q = p;
        // Leave the current word.
        while let Some(prev) = buf.prev_pos(q) {
            q = prev;
            if cls(buf.char_at(q)) != start || start == Class::Blank {
                break;
            }
        }
        while cls(buf.char_at(q)) == Class::Blank {
            match buf.prev_pos(q) {
                Some(prev) => q = prev,
                None => return q,
            }
        }
        q
    }

    /// The word (letters, digits, `_`) at or after the cursor on its line.
    fn word_under(&self, buf: &Buffer, p: Pos) -> Option<(usize, usize)> {
        let s = buf.line(p.line);
        let mut a = p.col;
        // Step forward to a word if not on one.
        while a < s.len() && !s.get(a..)?.chars().next().is_some_and(text::is_word_char) {
            a = buf.next_col(p.line, a);
        }
        if a >= s.len() {
            return None;
        }
        while a > 0 {
            let prev = buf.prev_col(p.line, a);
            if !s.get(prev..)?.chars().next().is_some_and(text::is_word_char) {
                break;
            }
            a = prev;
        }
        let mut b = a;
        while b < s.len() && s.get(b..)?.chars().next().is_some_and(text::is_word_char) {
            b = buf.next_col(p.line, b);
        }
        Some((a, b))
    }

    fn definition_of(&self, buf: &Buffer, word: &str) -> Option<Pos> {
        const INTRO: [&str; 12] = ["fn ", "struct ", "enum ", "trait ", "type ", "mod ", "const ", "static ", "let ", "let mut ", "union ", "macro_rules! "];
        for l in 0..buf.line_count() {
            let s = buf.line(l);
            for intro in INTRO {
                let mut from = 0;
                while let Some(i) = s.get(from..).and_then(|r| r.find(intro)) {
                    let at = from + i + intro.len();
                    let before_ok = from + i == 0 || !s.as_bytes().get(from + i - 1).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
                    if before_ok && s.get(at..).is_some_and(|r| r.starts_with(word)) {
                        let end = at + word.len();
                        let after_ok = !s.as_bytes().get(end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
                        if after_ok {
                            return Some(Pos::new(l, at));
                        }
                    }
                    from = at;
                }
            }
        }
        None
    }

    fn match_pair(&self, buf: &Buffer, p: Pos) -> Option<Pos> {
        let s = buf.line(p.line);
        let mut col = p.col;
        // The first bracket at or after the cursor on this line.
        let found = loop {
            let c = s.get(col..)?.chars().next()?;
            if "(){}[]".contains(c) {
                break c;
            }
            col = buf.next_col(p.line, col);
            if col >= s.len() {
                return None;
            }
        };
        let (open, close, fwd) = match found {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            _ => ('{', '}', false),
        };
        let mut depth = 0i32;
        let mut q = Pos::new(p.line, col);
        loop {
            match buf.char_at(q) {
                Some(c) if c == open => depth += if fwd { 1 } else { -1 },
                Some(c) if c == close => depth += if fwd { -1 } else { 1 },
                _ => {}
            }
            if depth == 0 {
                return Some(q);
            }
            q = if fwd { buf.next_pos(q)? } else { buf.prev_pos(q)? };
        }
    }

    fn find_char(&self, buf: &Buffer, p: Pos, kind: FindKind, c: char, n: usize, repeat: bool) -> Option<Pos> {
        let s = buf.line(p.line);
        let mut col = p.col;
        let mut hits = 0;
        match kind {
            FindKind::To | FindKind::Till => {
                // `;` after `t` must step past the char it stopped before.
                if repeat && kind == FindKind::Till {
                    col = buf.next_col(p.line, col);
                }
                loop {
                    let next = buf.next_col(p.line, col);
                    if next >= s.len() || next == col {
                        return None;
                    }
                    col = next;
                    if s.get(col..)?.starts_with(c) {
                        hits += 1;
                        if hits == n {
                            break;
                        }
                    }
                }
                if kind == FindKind::Till {
                    col = buf.prev_col(p.line, col);
                }
            }
            FindKind::BackTo | FindKind::BackTill => {
                if repeat && kind == FindKind::BackTill {
                    col = buf.prev_col(p.line, col);
                }
                loop {
                    if col == 0 {
                        return None;
                    }
                    col = buf.prev_col(p.line, col);
                    if s.get(col..)?.starts_with(c) {
                        hits += 1;
                        if hits == n {
                            break;
                        }
                    }
                }
                if kind == FindKind::BackTill {
                    col = buf.next_col(p.line, col);
                }
            }
        }
        Some(Pos::new(p.line, col))
    }

    /// The next match of `pat` after (or before) `p`, wrapping round the
    /// file once.
    fn search_from(&mut self, buf: &Buffer, p: Pos, pat: &str, forward: bool) -> Option<Pos> {
        let fold = smartcase(pat);
        let lines = buf.line_count();
        if forward {
            let from = buf.next_col(p.line, p.col);
            if from > p.col {
                if let Some(i) = find_in(buf.line(p.line), pat, from, fold) {
                    return Some(Pos::new(p.line, i));
                }
            }
            for k in 1..=lines {
                let l = (p.line + k) % lines;
                if let Some(i) = find_in(buf.line(l), pat, 0, fold) {
                    if l <= p.line {
                        self.info("search hit BOTTOM, continuing at TOP");
                    }
                    return Some(Pos::new(l, i));
                }
            }
        } else {
            if let Some(i) = rfind_in(buf.line(p.line), pat, p.col, fold) {
                return Some(Pos::new(p.line, i));
            }
            for k in 1..=lines {
                let l = (p.line + lines - k) % lines;
                let s = buf.line(l);
                if let Some(i) = rfind_in(s, pat, s.len() + 1, fold) {
                    if l >= p.line {
                        self.info("search hit TOP, continuing at BOTTOM");
                    }
                    return Some(Pos::new(l, i));
                }
            }
        }
        None
    }

    fn object_range(&self, buf: &Buffer, p: Pos, obj: Obj, n: usize) -> Option<(Pos, Pos, bool)> {
        match obj {
            Obj::Word { big, around } => {
                let s = buf.line(p.line);
                if s.is_empty() {
                    return None;
                }
                let cls = |c: Option<char>| if big { big_class(c) } else { class(c) };
                let at = |col: usize| s.get(col..).and_then(|r| r.chars().next());
                let c = cls(at(p.col));
                let mut a = p.col;
                while a > 0 && cls(at(buf.prev_col(p.line, a))) == c {
                    a = buf.prev_col(p.line, a);
                }
                let mut b = p.col;
                for k in 0..n {
                    let ck = if k == 0 { c } else { cls(at(b)) };
                    while b < s.len() && cls(at(b)) == ck {
                        b = buf.next_col(p.line, b);
                    }
                    if around {
                        if b < s.len() && cls(at(b)) == Class::Blank {
                            while b < s.len() && cls(at(b)) == Class::Blank {
                                b = buf.next_col(p.line, b);
                            }
                        } else {
                            while a > 0 && cls(at(buf.prev_col(p.line, a))) == Class::Blank {
                                a = buf.prev_col(p.line, a);
                            }
                        }
                    }
                }
                Some((Pos::new(p.line, a), Pos::new(p.line, b), false))
            }
            Obj::Quote { q, around } => {
                let s = buf.line(p.line);
                let quotes: Vec<usize> = s.char_indices().filter(|(i, c)| *c == q && (*i == 0 || s.as_bytes().get(i - 1) != Some(&b'\\'))).map(|(i, _)| i).collect();
                let mut k = 0;
                while k + 1 < quotes.len() {
                    let (a, b) = (quotes[k], quotes[k + 1]);
                    if p.col <= b {
                        if p.col >= a || k == 0 {
                            return if around {
                                Some((Pos::new(p.line, a), Pos::new(p.line, b + 1), false))
                            } else {
                                Some((Pos::new(p.line, a + 1), Pos::new(p.line, b), false))
                            };
                        }
                    }
                    k += 2;
                }
                None
            }
            Obj::Pair { open, close, around } => {
                // Walk out from the cursor to the enclosing open bracket.
                let mut depth = 0i32;
                let mut q = p;
                let mut opened = None;
                if buf.char_at(p) == Some(open) {
                    opened = Some(p);
                } else {
                    while let Some(prev) = buf.prev_pos(q) {
                        q = prev;
                        match buf.char_at(q) {
                            Some(c) if c == close => depth += 1,
                            Some(c) if c == open => {
                                if depth == 0 {
                                    opened = Some(q);
                                    break;
                                }
                                depth -= 1;
                            }
                            _ => {}
                        }
                    }
                }
                let mut a = opened?;
                for _ in 1..n {
                    // Count more levels out.
                    let mut d = 0i32;
                    let mut r = a;
                    let mut found = None;
                    while let Some(prev) = buf.prev_pos(r) {
                        r = prev;
                        match buf.char_at(r) {
                            Some(c) if c == close => d += 1,
                            Some(c) if c == open => {
                                if d == 0 {
                                    found = Some(r);
                                    break;
                                }
                                d -= 1;
                            }
                            _ => {}
                        }
                    }
                    a = found?;
                }
                let b = self.match_pair(buf, a)?;
                if around {
                    let after = Pos::new(b.line, buf.next_col(b.line, b.col));
                    return Some((a, after, false));
                }
                let mut start = Pos::new(a.line, buf.next_col(a.line, a.col));
                let mut end = b;
                // A block whose brackets sit on their own lines is its lines.
                if start.col >= buf.line_len(start.line) && start.line < b.line && buf.line(b.line).get(..b.col).is_some_and(|h| h.trim().is_empty()) {
                    if start.line + 1 > b.line - 1 {
                        return Some((Pos::new(b.line, 0), Pos::new(b.line, 0), false));
                    }
                    start = Pos::new(start.line + 1, 0);
                    end = Pos::new(b.line - 1, 0);
                    return Some((start, end, true));
                }
                if end < start {
                    end = start;
                }
                Some((start, end, false))
            }
        }
    }

    // ---- insert --------------------------------------------------------------

    fn insert(&mut self, ctx: &mut Ctx, input: Input, fx: &mut Vec<Effect>) {
        let p = ctx.cur.pos;
        match input {
            Input::Key(Key::Esc) => {
                // `3ihi<Esc>` types the text three times.
                if self.insert_count > 1 && self.insert_from.line == p.line && p.col >= self.insert_from.col {
                    let typed = String::from(ctx.buf.line(p.line).get(self.insert_from.col..p.col).unwrap_or(""));
                    let mut at = p;
                    for _ in 1..self.insert_count {
                        at = ctx.buf.insert(at, &typed);
                    }
                    ctx.cur.pos = at;
                }
                self.insert_count = 1;
                ctx.buf.end_group(ctx.cur.pos);
                self.mode = Mode::Normal;
                ctx.cur.pos.col = ctx.buf.prev_col(ctx.cur.pos.line, ctx.cur.pos.col);
                self.clamp_normal(ctx.buf, ctx.cur);
                self.set_want(ctx.buf, ctx.cur);
                if let Some(rec) = self.recording.take() {
                    self.last_change = Some(rec);
                }
            }
            Input::Char(c) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                // A closing brace on a blank line steps back a level.
                if c == '}' || c == ')' || c == ']' {
                    let s = ctx.buf.line(p.line);
                    if !s.is_empty() && s.get(..p.col).is_some_and(|h| h.trim().is_empty()) && p.col == s.len() {
                        let width = text::display_width(s, self.tab);
                        let target = width.saturating_sub(self.shiftwidth) / self.shiftwidth * self.shiftwidth;
                        let ind = self.indent_string(target);
                        ctx.buf.replace(Pos::new(p.line, 0), p, &ind);
                        ctx.cur.pos = Pos::new(p.line, ind.len());
                    }
                }
                let mut s = [0u8; 4];
                let at = ctx.cur.pos;
                ctx.cur.pos = ctx.buf.insert(at, c.encode_utf8(&mut s));
                self.set_want(ctx.buf, ctx.cur);
            }
            Input::Paste(text) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                ctx.cur.pos = ctx.buf.insert(p, &text);
                self.set_want(ctx.buf, ctx.cur);
            }
            Input::Key(Key::Enter) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                let s = ctx.buf.line(p.line);
                let before = s.get(..p.col).unwrap_or("").trim_end();
                let after = s.get(p.col..).unwrap_or("");
                let mut indent = String::from(text::indent_of(s));
                let opens = before.ends_with('{') || before.ends_with('(') || before.ends_with('[');
                let closes_next = after.trim_start().starts_with('}') || after.trim_start().starts_with(')') || after.trim_start().starts_with(']');
                // Trailing spaces before the cursor go, as every editor does.
                let trailing = s.get(..p.col).unwrap_or("").len() - s.get(..p.col).unwrap_or("").trim_end().len();
                let from = Pos::new(p.line, p.col - trailing.min(p.col));
                let lead = after.len() - after.trim_start().len();
                let to = Pos::new(p.line, p.col + lead);
                let mut t = String::from("\n");
                if opens {
                    let outer = indent.clone();
                    indent.push_str(&self.indent_string(self.shiftwidth));
                    t.push_str(&indent);
                    if closes_next {
                        t.push('\n');
                        t.push_str(&outer);
                        ctx.buf.replace(from, to, &t);
                        ctx.cur.pos = Pos::new(p.line + 1, indent.len());
                        self.set_want(ctx.buf, ctx.cur);
                        return;
                    }
                } else {
                    t.push_str(&indent);
                }
                ctx.cur.pos = ctx.buf.replace(from, to, &t);
                self.set_want(ctx.buf, ctx.cur);
            }
            Input::Key(Key::Tab) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                if self.expandtab {
                    let d = text::display_col(ctx.buf.line(p.line), p.col, self.tab);
                    let next = (d / self.tab + 1) * self.tab;
                    let mut s = String::new();
                    for _ in d..next {
                        s.push(' ');
                    }
                    ctx.cur.pos = ctx.buf.insert(p, &s);
                } else {
                    ctx.cur.pos = ctx.buf.insert(p, "\t");
                }
                self.set_want(ctx.buf, ctx.cur);
            }
            Input::Key(Key::Backspace) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                if p.col == 0 {
                    if p.line > 0 {
                        let l = p.line - 1;
                        let at = Pos::new(l, ctx.buf.line_len(l));
                        ctx.buf.delete(at, p);
                        ctx.cur.pos = at;
                    }
                } else {
                    let s = ctx.buf.line(p.line);
                    let head = s.get(..p.col).unwrap_or("");
                    // In leading spaces, step back to the previous indent stop.
                    let from = if self.expandtab && !head.is_empty() && head.bytes().all(|b| b == b' ') {
                        let w = head.len();
                        let stop = (w - 1) / self.shiftwidth * self.shiftwidth;
                        stop
                    } else {
                        ctx.buf.prev_col(p.line, p.col)
                    };
                    ctx.buf.delete(Pos::new(p.line, from), p);
                    ctx.cur.pos = Pos::new(p.line, from);
                }
                self.set_want(ctx.buf, ctx.cur);
            }
            Input::Key(Key::Delete) => {
                if !ctx.buf.group_open() {
                    ctx.buf.begin_group(p);
                }
                if let Some(n) = ctx.buf.next_pos(p) {
                    ctx.buf.delete(p, n);
                }
            }
            Input::Ctrl('w') => {
                // Delete the word before the cursor.
                let start = self.word_back(ctx.buf, p, false);
                let start = if start.line < p.line { Pos::new(p.line, 0) } else { start };
                if start < p {
                    if !ctx.buf.group_open() {
                        ctx.buf.begin_group(p);
                    }
                    ctx.buf.delete(start, p);
                    ctx.cur.pos = start;
                }
            }
            Input::Ctrl('u') => {
                let start = Pos::new(p.line, 0);
                if start < p {
                    if !ctx.buf.group_open() {
                        ctx.buf.begin_group(p);
                    }
                    ctx.buf.delete(start, p);
                    ctx.cur.pos = start;
                }
            }
            Input::Shift(k) => {
                self.begin_select(ctx.buf, ctx.cur, p);
                self.shift_move(ctx, k);
            }
            Input::Key(k) => {
                // Moving with the arrows starts a new undo step, as in Vim.
                ctx.buf.end_group(p);
                self.recording = None;
                self.move_key(ctx, k);
                ctx.buf.begin_group(ctx.cur.pos);
                self.insert_from = ctx.cur.pos;
                self.insert_count = 1;
            }
            Input::Ctrl(_) => {
                let _ = fx;
            }
        }
    }

    /// An arrow, Home, End or page key moving the cursor in any mode.
    fn move_key(&mut self, ctx: &mut Ctx, k: Key) {
        let p = ctx.cur.pos;
        let buf = &*ctx.buf;
        match k {
            Key::Left => ctx.cur.pos = Pos::new(p.line, buf.prev_col(p.line, p.col)),
            Key::Right => ctx.cur.pos = Pos::new(p.line, buf.next_col(p.line, p.col)),
            Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                let step = if matches!(k, Key::PageUp | Key::PageDown) { ctx.page.saturating_sub(2).max(1) } else { 1 };
                let line = if matches!(k, Key::Down | Key::PageDown) {
                    (p.line + step).min(buf.last_line())
                } else {
                    p.line.saturating_sub(step)
                };
                let col = if ctx.cur.want == usize::MAX { buf.line_len(line) } else { text::col_for_display(buf.line(line), ctx.cur.want, self.tab) };
                ctx.cur.pos = Pos::new(line, col);
                return;
            }
            Key::Home => {
                let fnb = buf.first_non_blank(p.line);
                ctx.cur.pos = Pos::new(p.line, if p.col == fnb { 0 } else { fnb });
            }
            Key::End => ctx.cur.pos = Pos::new(p.line, buf.line_len(p.line)),
            _ => {}
        }
        self.set_want(ctx.buf, ctx.cur);
    }

    fn shift_move(&mut self, ctx: &mut Ctx, k: Key) {
        self.move_key(ctx, k);
        if ctx.cur.pos == ctx.cur.anchor {
            self.mode = self.select_return;
            if self.mode == Mode::Insert {
                ctx.buf.begin_group(ctx.cur.pos);
            }
        }
    }

    // ---- select (mouse and Shift selections) -----------------------------------

    fn select(&mut self, ctx: &mut Ctx, input: Input, fx: &mut Vec<Effect>) {
        let (a, b) = if ctx.cur.anchor <= ctx.cur.pos {
            (ctx.cur.anchor, ctx.cur.pos)
        } else {
            (ctx.cur.pos, ctx.cur.anchor)
        };
        match input {
            Input::Key(Key::Esc) => {
                self.mode = Mode::Normal;
                self.clamp_normal(ctx.buf, ctx.cur);
            }
            Input::Shift(k) => self.shift_move(ctx, k),
            Input::Key(k @ (Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown)) => {
                ctx.cur.pos = if matches!(k, Key::Left | Key::Up) { a } else { b };
                self.mode = self.select_return;
                if self.mode == Mode::Insert {
                    ctx.buf.begin_group(ctx.cur.pos);
                }
                if !matches!(k, Key::Left | Key::Right) {
                    self.move_key(ctx, k);
                }
                if self.mode != Mode::Insert {
                    self.clamp_normal(ctx.buf, ctx.cur);
                }
            }
            Input::Key(Key::Backspace) | Input::Key(Key::Delete) => {
                ctx.buf.begin_group(ctx.cur.pos);
                let t = ctx.buf.delete(a, b);
                self.set_register(None, t, false);
                ctx.cur.pos = a;
                self.mode = Mode::Insert;
                self.insert_count = 1;
                self.insert_from = a;
            }
            Input::Char(_) | Input::Paste(_) | Input::Key(Key::Enter) | Input::Key(Key::Tab) => {
                ctx.buf.begin_group(ctx.cur.pos);
                let t = ctx.buf.delete(a, b);
                self.set_register(None, t, false);
                ctx.cur.pos = a;
                self.mode = Mode::Insert;
                self.insert_count = 1;
                self.insert_from = a;
                self.insert(ctx, input, fx);
            }
            Input::Ctrl('c') | Input::Ctrl('x') => {
                let t = ctx.buf.slice(a, b);
                self.set_register(None, t, false);
                if input == Input::Ctrl('x') {
                    ctx.buf.begin_group(ctx.cur.pos);
                    ctx.buf.delete(a, b);
                    ctx.buf.end_group(a);
                    ctx.cur.pos = a;
                    self.mode = self.select_return;
                    if self.mode == Mode::Insert {
                        ctx.buf.begin_group(a);
                    }
                } else {
                    self.info("Copied the selection (paste it with p)");
                }
            }
            _ => {}
        }
    }

    /// Cmd/Ctrl+C or X with a visual selection: copy or cut it.
    pub fn copy_selection(&mut self, ctx: &mut Ctx, cut: bool) -> bool {
        let Some((a, b, linewise)) = self.selection(ctx.buf, ctx.cur) else {
            return false;
        };
        let (from, to) = if linewise { self.line_span(ctx.buf, a.line, b.line) } else { (a, b) };
        let mut t = ctx.buf.slice(a, b);
        if linewise {
            t.push('\n');
        }
        self.set_register(None, t, linewise);
        if cut {
            ctx.buf.begin_group(ctx.cur.pos);
            ctx.buf.delete(from, to);
            ctx.buf.end_group(from);
            ctx.cur.pos = ctx.buf.clamp(from);
        }
        if self.mode == Mode::Select {
            self.mode = self.select_return;
            if self.mode == Mode::Insert {
                ctx.buf.begin_group(ctx.cur.pos);
            }
        } else {
            self.mode = Mode::Normal;
        }
        if self.mode != Mode::Insert {
            self.clamp_normal(ctx.buf, ctx.cur);
        }
        true
    }

    // ---- the command line ------------------------------------------------------

    fn cmdline_key(&mut self, ctx: &mut Ctx, input: Input, fx: &mut Vec<Effect>) {
        match input {
            Input::Key(Key::Esc) => {
                self.mode = Mode::Normal;
                self.cmdline.clear();
            }
            Input::Key(Key::Backspace) => {
                if self.cmdline.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            Input::Ctrl('u') => self.cmdline.clear(),
            Input::Char(c) => self.cmdline.push(c),
            Input::Paste(t) => {
                for c in t.chars() {
                    if c != '\n' && c != '\r' {
                        self.cmdline.push(c);
                    }
                }
            }
            Input::Key(Key::Up) | Input::Key(Key::Down) => {
                if self.history.is_empty() {
                    return;
                }
                let up = input == Input::Key(Key::Up);
                let at = match (self.history_at, up) {
                    (None, true) => self.history.len() - 1,
                    (None, false) => return,
                    (Some(i), true) => i.saturating_sub(1),
                    (Some(i), false) => {
                        if i + 1 >= self.history.len() {
                            self.history_at = None;
                            self.cmdline.clear();
                            return;
                        }
                        i + 1
                    }
                };
                self.history_at = Some(at);
                self.cmdline = self.history.get(at).cloned().unwrap_or_default();
            }
            Input::Key(Key::Enter) => {
                let line = core::mem::take(&mut self.cmdline);
                let mode = self.mode;
                self.mode = Mode::Normal;
                match mode {
                    Mode::Search { forward } => {
                        let pat = if line.is_empty() {
                            match &self.last_search {
                                Some((p, _)) => p.clone(),
                                None => return,
                            }
                        } else {
                            line
                        };
                        self.last_search = Some((pat.clone(), forward));
                        self.highlight = true;
                        let buf = &*ctx.buf;
                        match self.search_from(buf, ctx.cur.pos, &pat, forward) {
                            Some(p) => {
                                ctx.cur.pos = p;
                                self.set_want(ctx.buf, ctx.cur);
                            }
                            None => {
                                let mut m = String::from("Pattern not found: ");
                                m.push_str(&pat);
                                self.error(m);
                            }
                        }
                    }
                    _ => {
                        if !line.trim().is_empty() {
                            self.history.retain(|h| h != &line);
                            self.history.push(line.clone());
                        }
                        self.ex(ctx, line.trim(), fx);
                        self.clamp_normal(ctx.buf, ctx.cur);
                    }
                }
            }
            _ => {}
        }
    }

    /// Run one `:` command.
    pub fn ex(&mut self, ctx: &mut Ctx, line: &str, fx: &mut Vec<Effect>) {
        let line = line.trim_start_matches(':').trim();
        if line.is_empty() {
            return;
        }
        // A line range in front: %, N, N,M, '<,'>, ., $, .,.+N
        let (range, rest) = self.parse_range(ctx, line);
        let rest = rest.trim_start();
        if rest.is_empty() {
            if let Some((_, b)) = range {
                ctx.cur.pos = Pos::new(b, ctx.buf.first_non_blank(b));
                self.set_want(ctx.buf, ctx.cur);
            }
            return;
        }
        // The name is the leading letters (one sign for `>` and `<`), with
        // a `!` kept on it; what follows is the argument, separator and all
        // (`s#a#b#` uses `#`).
        let letters = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let mut name_end = if letters == 0 {
            rest.chars().next().map(char::len_utf8).unwrap_or(0)
        } else {
            letters
        };
        if rest.get(name_end..).is_some_and(|r| r.starts_with('!')) {
            name_end += 1;
        }
        let name = rest.get(..name_end).unwrap_or("");
        let arg = rest.get(name_end..).unwrap_or("").trim_start_matches(' ');
        let arg_opt = if arg.trim().is_empty() { None } else { Some(String::from(arg.trim())) };
        match name {
            "w" | "write" => fx.push(Effect::Write { path: arg_opt }),
            "w!" => fx.push(Effect::Write { path: arg_opt }),
            "q" | "quit" => fx.push(Effect::Quit { force: false }),
            "q!" | "quit!" => fx.push(Effect::Quit { force: true }),
            "wq" | "x" | "xit" | "wq!" | "x!" => fx.push(Effect::WriteQuit),
            "wa" | "wall" => fx.push(Effect::WriteAll),
            "qa" | "qall" => fx.push(Effect::QuitAll { force: false }),
            "qa!" | "qall!" => fx.push(Effect::QuitAll { force: true }),
            "wqa" | "xa" => {
                fx.push(Effect::WriteAll);
                fx.push(Effect::QuitAll { force: false });
            }
            "e" | "edit" => fx.push(Effect::Edit { path: arg_opt, force: false }),
            "e!" | "edit!" => fx.push(Effect::Edit { path: arg_opt, force: true }),
            "vs" | "vsp" | "vsplit" | "sp" | "split" | "vnew" | "new" => fx.push(Effect::Split { path: arg_opt }),
            "clo" | "close" => fx.push(Effect::ClosePane),
            "on" | "only" => fx.push(Effect::OnlyPane),
            "bn" | "bnext" => fx.push(Effect::NextBuffer { forward: true }),
            "bp" | "bprev" | "bprevious" | "bN" => fx.push(Effect::NextBuffer { forward: false }),
            "bd" | "bdelete" => fx.push(Effect::CloseBuffer { force: false }),
            "bd!" | "bdelete!" => fx.push(Effect::CloseBuffer { force: true }),
            "noh" | "nohlsearch" => self.highlight = false,
            "s" | "substitute" | "%s" => {
                let range = if name == "%s" { Some((0, ctx.buf.last_line())) } else { range };
                self.substitute(ctx, range, arg);
            }
            "set" | "se" => self.set_option(arg),
            "d" | "delete" => {
                let (a, b) = range.unwrap_or((ctx.cur.pos.line, ctx.cur.pos.line));
                ctx.buf.begin_group(ctx.cur.pos);
                self.apply_op(ctx, Op::Delete, Pos::new(a, 0), Pos::new(b, 0), MKind::Linewise, None, fx);
                ctx.buf.end_group(ctx.cur.pos);
            }
            "y" | "yank" => {
                let (a, b) = range.unwrap_or((ctx.cur.pos.line, ctx.cur.pos.line));
                self.apply_op(ctx, Op::Yank, Pos::new(a, 0), Pos::new(b, 0), MKind::Linewise, None, fx);
            }
            ">" | "<" => {
                let (a, b) = range.unwrap_or((ctx.cur.pos.line, ctx.cur.pos.line));
                ctx.buf.begin_group(ctx.cur.pos);
                for l in a..=b {
                    self.shift_line(ctx.buf, l, name == ">");
                }
                ctx.buf.end_group(ctx.cur.pos);
            }
            _ => {
                // Not the engine's: the workspace knows the rest (and says so
                // when nobody does).
                let mut whole = String::from(name);
                if !arg.is_empty() {
                    whole.push(' ');
                    whole.push_str(arg.trim());
                }
                fx.push(Effect::Command(whole));
            }
        }
    }

    fn parse_range<'s>(&self, ctx: &Ctx, line: &'s str) -> (Option<(usize, usize)>, &'s str) {
        let last = ctx.buf.last_line();
        let here = ctx.cur.pos.line;
        if let Some(rest) = line.strip_prefix('%') {
            return (Some((0, last)), rest);
        }
        if let Some(rest) = line.strip_prefix("'<,'>") {
            let r = self.visual_marks.map(|(a, b, _)| (a.line, b.line)).unwrap_or((here, here));
            return (Some(r), rest);
        }
        let addr = |s: &'s str| -> (Option<usize>, &'s str) {
            let base_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
            let (mut v, mut rest) = if base_len > 0 {
                let n: usize = s[..base_len].parse().unwrap_or(1);
                (Some(n.saturating_sub(1)), &s[base_len..])
            } else if let Some(r) = s.strip_prefix('.') {
                (Some(here), r)
            } else if let Some(r) = s.strip_prefix('$') {
                (Some(last), r)
            } else {
                (None, s)
            };
            if let Some(r) = rest.strip_prefix('+') {
                let k = r.chars().take_while(|c| c.is_ascii_digit()).count();
                let n: usize = if k == 0 { 1 } else { r[..k].parse().unwrap_or(1) };
                v = Some(v.unwrap_or(here) + n);
                rest = &r[k..];
            } else if let Some(r) = rest.strip_prefix('-') {
                let k = r.chars().take_while(|c| c.is_ascii_digit()).count();
                let n: usize = if k == 0 { 1 } else { r[..k].parse().unwrap_or(1) };
                v = Some(v.unwrap_or(here).saturating_sub(n));
                rest = &r[k..];
            }
            (v.map(|x| x.min(last)), rest)
        };
        let (a, rest) = addr(line);
        let Some(a) = a else {
            return (None, line);
        };
        if let Some(r) = rest.strip_prefix(',') {
            let (b, rest) = addr(r);
            let b = b.unwrap_or(a);
            return (Some((a.min(b), a.max(b))), rest);
        }
        (Some((a, a)), rest)
    }

    fn set_option(&mut self, arg: &str) {
        for opt in arg.split_whitespace() {
            let (name, value) = match opt.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (opt, None),
            };
            let num = value.and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=16).contains(n));
            match (name, num) {
                ("rnu" | "relativenumber", _) => self.relative_numbers = true,
                ("nornu" | "norelativenumber", _) => self.relative_numbers = false,
                ("ts" | "tabstop", Some(n)) => self.tab = n,
                ("sw" | "shiftwidth", Some(n)) => self.shiftwidth = n,
                ("et" | "expandtab", _) => self.expandtab = true,
                ("noet" | "noexpandtab", _) => self.expandtab = false,
                ("hls" | "hlsearch", _) => self.highlight = true,
                ("nohls" | "nohlsearch", _) => self.highlight = false,
                _ => {
                    let mut m = String::from("Unknown option: ");
                    m.push_str(opt);
                    self.error(m);
                    return;
                }
            }
        }
    }

    /// `:s/old/new/[g]` over a range of lines, literal text.
    fn substitute(&mut self, ctx: &mut Ctx, range: Option<(usize, usize)>, arg: &str) {
        let mut chars = arg.chars();
        let Some(sep) = chars.next() else {
            self.error("Usage: :s/old/new/ or :%s/old/new/g");
            return;
        };
        if sep.is_alphanumeric() || sep == ' ' {
            self.error("Usage: :s/old/new/ or :%s/old/new/g");
            return;
        }
        let mut parts: Vec<String> = alloc::vec![String::new()];
        let mut escaped = false;
        for c in chars {
            if escaped {
                let last = parts.last_mut();
                if let Some(s) = last {
                    if c == sep || c == '\\' {
                        s.push(c);
                    } else if c == 'n' {
                        s.push('\n');
                    } else if c == 't' {
                        s.push('\t');
                    } else {
                        s.push('\\');
                        s.push(c);
                    }
                }
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == sep && parts.len() < 3 {
                parts.push(String::new());
            } else if let Some(s) = parts.last_mut() {
                s.push(c);
            }
        }
        let pat = parts.first().cloned().unwrap_or_default();
        let rep = parts.get(1).cloned().unwrap_or_default();
        let flags = parts.get(2).cloned().unwrap_or_default();
        let pat = if pat.is_empty() {
            match &self.last_search {
                Some((p, _)) => p.clone(),
                None => {
                    self.error("No previous pattern");
                    return;
                }
            }
        } else {
            pat
        };
        let global = flags.contains('g');
        let fold = flags.contains('i') || (smartcase(&pat) && !flags.contains('I'));
        let (a, b) = range.unwrap_or((ctx.cur.pos.line, ctx.cur.pos.line));
        let mut count = 0usize;
        let mut lines_changed = 0usize;
        let mut last_line = None;
        ctx.buf.begin_group(ctx.cur.pos);
        let mut l = a;
        let mut end_line = b;
        while l <= end_line && l <= ctx.buf.last_line() {
            let s = String::from(ctx.buf.line(l));
            let mut out = String::new();
            let mut from = 0;
            let mut n = 0;
            while let Some(i) = find_in(&s, &pat, from, fold) {
                out.push_str(s.get(from..i).unwrap_or(""));
                // `&` in the replacement is the matched text.
                for c in rep.chars() {
                    if c == '&' {
                        out.push_str(s.get(i..i + pat.len()).unwrap_or(""));
                    } else {
                        out.push(c);
                    }
                }
                from = i + pat.len();
                n += 1;
                if !global {
                    break;
                }
            }
            if n > 0 {
                out.push_str(s.get(from..).unwrap_or(""));
                let added = out.matches('\n').count();
                ctx.buf.replace(Pos::new(l, 0), Pos::new(l, s.len()), &out);
                count += n;
                lines_changed += 1;
                last_line = Some(l + added);
                end_line += added;
                l += added;
            }
            l += 1;
        }
        ctx.buf.end_group(ctx.cur.pos);
        match last_line {
            Some(l) => {
                ctx.cur.pos = Pos::new(l, ctx.buf.first_non_blank(l));
                self.last_search = Some((pat, true));
                let mut m = text::num(count);
                m.push_str(if count == 1 { " substitution on " } else { " substitutions on " });
                m.push_str(&text::num(lines_changed));
                m.push_str(if lines_changed == 1 { " line" } else { " lines" });
                self.info(m);
            }
            None => {
                let mut m = String::from("Pattern not found: ");
                m.push_str(&pat);
                self.error(m);
            }
        }
    }

    /// Type a whole string of inputs (tests and scripted runs).
    pub fn keys(&mut self, ctx: &mut Ctx, keys: &str) -> Vec<Effect> {
        let mut fx = Vec::new();
        for i in parse_keys(keys) {
            fx.extend(self.feed(ctx, i));
        }
        fx
    }
}

fn find_kind(k: FindKind) -> MKind {
    match k {
        FindKind::To | FindKind::Till => MKind::Inclusive,
        _ => MKind::Exclusive,
    }
}

/// Keys in Vim's own notation: `<Esc>`, `<CR>`, `<BS>`, `<Tab>`, `<C-r>`,
/// `<Up>`... everything else typed as is.
pub fn parse_keys(s: &str) -> Vec<Input> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest.find('>') {
                let name = &rest[1..end];
                let key = match name {
                    "Esc" => Some(Input::Key(Key::Esc)),
                    "CR" | "Enter" => Some(Input::Key(Key::Enter)),
                    "BS" => Some(Input::Key(Key::Backspace)),
                    "Del" => Some(Input::Key(Key::Delete)),
                    "Tab" => Some(Input::Key(Key::Tab)),
                    "Left" => Some(Input::Key(Key::Left)),
                    "Right" => Some(Input::Key(Key::Right)),
                    "Up" => Some(Input::Key(Key::Up)),
                    "Down" => Some(Input::Key(Key::Down)),
                    "Home" => Some(Input::Key(Key::Home)),
                    "End" => Some(Input::Key(Key::End)),
                    "S-Left" => Some(Input::Shift(Key::Left)),
                    "S-Right" => Some(Input::Shift(Key::Right)),
                    "S-Down" => Some(Input::Shift(Key::Down)),
                    "S-Up" => Some(Input::Shift(Key::Up)),
                    "lt" => Some(Input::Char('<')),
                    _ => name.strip_prefix("C-").and_then(|r| r.chars().next()).map(|c| Input::Ctrl(c.to_ascii_lowercase())),
                };
                if let Some(k) = key {
                    out.push(k);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        out.push(Input::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

impl Vim {
    /// What the status line should say about pending keys and counts.
    pub fn pending_keys(&self) -> &str {
        &self.showcmd
    }

    pub fn last_change_len(&self) -> usize {
        self.last_change.as_ref().map(|(_, k)| k.len()).unwrap_or(0)
    }
}

