//! Ports diagrams/sequence/db.ts, types.ts and colors.ts: the model the
//! statements build.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::json::Value;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{is_space, is_word, trim, trim_start};
use crate::pele::preprocess::{to_js_string, truthy};
use crate::pele::quadrant::db::tighten_lines;
use crate::pele::util::yaml::parse_yaml;

/// Ports `const enum LINETYPE`.
pub mod line {
    pub const SOLID: i32 = 0;
    pub const DOTTED: i32 = 1;
    pub const NOTE: i32 = 2;
    pub const SOLID_CROSS: i32 = 3;
    pub const DOTTED_CROSS: i32 = 4;
    pub const SOLID_OPEN: i32 = 5;
    pub const DOTTED_OPEN: i32 = 6;
    pub const LOOP_START: i32 = 10;
    pub const LOOP_END: i32 = 11;
    pub const ALT_START: i32 = 12;
    pub const ALT_ELSE: i32 = 13;
    pub const ALT_END: i32 = 14;
    pub const OPT_START: i32 = 15;
    pub const OPT_END: i32 = 16;
    pub const ACTIVE_START: i32 = 17;
    pub const ACTIVE_END: i32 = 18;
    pub const PAR_START: i32 = 19;
    pub const PAR_AND: i32 = 20;
    pub const PAR_END: i32 = 21;
    pub const RECT_START: i32 = 22;
    pub const RECT_END: i32 = 23;
    pub const SOLID_POINT: i32 = 24;
    pub const DOTTED_POINT: i32 = 25;
    pub const AUTONUMBER: i32 = 26;
    pub const CRITICAL_START: i32 = 27;
    pub const CRITICAL_OPTION: i32 = 28;
    pub const CRITICAL_END: i32 = 29;
    pub const BREAK_START: i32 = 30;
    pub const BREAK_END: i32 = 31;
    pub const PAR_OVER_START: i32 = 32;
    pub const BIDIRECTIONAL_SOLID: i32 = 33;
    pub const BIDIRECTIONAL_DOTTED: i32 = 34;
    pub const SOLID_TOP: i32 = 41;
    pub const SOLID_BOTTOM: i32 = 42;
    pub const STICK_TOP: i32 = 43;
    pub const STICK_BOTTOM: i32 = 44;
    pub const SOLID_ARROW_TOP_REVERSE: i32 = 45;
    pub const SOLID_ARROW_BOTTOM_REVERSE: i32 = 46;
    pub const STICK_ARROW_TOP_REVERSE: i32 = 47;
    pub const STICK_ARROW_BOTTOM_REVERSE: i32 = 48;
    pub const SOLID_TOP_DOTTED: i32 = 51;
    pub const SOLID_BOTTOM_DOTTED: i32 = 52;
    pub const STICK_TOP_DOTTED: i32 = 53;
    pub const STICK_BOTTOM_DOTTED: i32 = 54;
    pub const SOLID_ARROW_TOP_REVERSE_DOTTED: i32 = 55;
    pub const SOLID_ARROW_BOTTOM_REVERSE_DOTTED: i32 = 56;
    pub const STICK_ARROW_TOP_REVERSE_DOTTED: i32 = 57;
    pub const STICK_ARROW_BOTTOM_REVERSE_DOTTED: i32 = 58;
    pub const CENTRAL_CONNECTION: i32 = 59;
    pub const CENTRAL_CONNECTION_REVERSE: i32 = 60;
    pub const CENTRAL_CONNECTION_DUAL: i32 = 61;
}

/// Ports `const enum PLACEMENT`.
pub const LEFTOF: i32 = 0;
pub const RIGHTOF: i32 = 1;
pub const OVER: i32 = 2;

/// Ports `interface SeqText`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeqText {
    pub text: String,
    pub wrap: Option<bool>,
}

/// Ports `interface SeqBoxData`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeqBoxData {
    pub text: Option<String>,
    pub color: String,
    pub wrap: Option<bool>,
}

/// Ports `interface SeqBox`.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqBox {
    pub name: Option<String>,
    pub wrap: bool,
    pub fill: String,
    pub actor_keys: Vec<String>,
}

/// Ports `interface SeqActor`. `bx` indexes `boxes`.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqActor {
    pub bx: Option<usize>,
    pub name: String,
    pub description: String,
    pub wrap: bool,
    pub prev_actor: Option<String>,
    pub next_actor: Option<String>,
    pub links: Vec<(String, Value)>,
    pub properties: Vec<(String, Value)>,
    pub details: Option<String>,
    pub kind: String,
}

/// Ports `interface SeqNumbering`.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqNumbering {
    pub start: Option<f64>,
    pub step: Option<f64>,
    pub visible: bool,
}

/// Ports `interface SeqMessage`: `message` is text, or numbering.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqMessage {
    pub id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: Option<String>,
    pub numbering: Option<SeqNumbering>,
    pub wrap: bool,
    pub kind: Option<i32>,
    pub activate: bool,
    pub placement: Option<i32>,
    pub central: i32,
}

/// Who a note is on (`string | string[]`).
#[derive(Debug, Clone, PartialEq)]
pub enum NoteActor {
    One(String),
    Two(String, String),
}

/// A menu statement's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Links,
    ALink,
    Properties,
    Details,
}

/// The statements the parser hands to `apply` (`SeqStatement`), flattened
/// in the order Mermaid's recursive walk meets them.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `addParticipant`, `createParticipant` (create) or `destroyParticipant` (destroy).
    Participant {
        create: bool,
        destroy: bool,
        actor: String,
        draw: Option<&'static str>,
        description: Option<SeqText>,
        config: Option<String>,
    },
    Message {
        from: String,
        to: String,
        signal_type: i32,
        msg: SeqText,
        activate: bool,
        central: Option<i32>,
    },
    /// `activeStart`, `activeEnd`, `centralConnection`, `centralConnectionReverse`.
    Signal {
        signal_type: i32,
        actor: String,
    },
    Note {
        actor: NoteActor,
        placement: i32,
        text: SeqText,
    },
    Menu {
        kind: Menu,
        actor: String,
        text: SeqText,
    },
    Autonumber {
        start: Option<f64>,
        step: Option<f64>,
        visible: bool,
    },
    BoxStart(SeqBoxData),
    BoxEnd,
    /// A block's start, divider or end: `addSignal(undefined, undefined, text, type)`.
    Block {
        signal_type: i32,
        text: Option<SeqText>,
    },
}

fn fail(message: String) -> PeleError {
    PeleError::of(message, PeleErrorCode::Semantic, "sequence")
}

/// Ports `extractWrap`.
pub fn extract_wrap(raw: &str) -> SeqText {
    let text = trim(raw);
    let after = |s: &str, word: &str| -> Option<usize> {
        let t = s.strip_prefix(':').unwrap_or(s);
        t.strip_prefix(word).map(|r| s.len() - r.len())
    };
    let wrap = if after(text, "wrap:").is_some() {
        Some(true)
    } else if after(text, "nowrap:").is_some() {
        Some(false)
    } else {
        None
    };
    let rest = match wrap {
        None => text,
        Some(_) => {
            let k = after(text, "wrap:")
                .or_else(|| after(text, "nowrap:"))
                .unwrap_or(0);
            &text[k..]
        }
    };
    SeqText {
        text: trim(rest).into(),
        wrap,
    }
}

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\r' | '\u{2028}' | '\u{2029}')
}

/// Ports `splitBox`: a box line split into a leading color and the rest.
fn split_box(s: &str) -> Option<(String, String)> {
    // RE_BOX_FUNCTION = /^(?:rgba?|hsla?)\s*\(/
    let head = ["rgba", "rgb", "hsla", "hsl"]
        .iter()
        .find(|f| s.starts_with(**f))
        .and_then(|f| {
            let rest = &s[f.len()..];
            let t = rest.trim_start_matches(is_space);
            t.strip_prefix('(').map(|_| s.len() - t.len() + 1)
        });
    if let Some(fn_len) = head {
        if let Some(close) = s.rfind(')') {
            if close >= fn_len {
                if s[fn_len..].contains(is_line_terminator) {
                    return None;
                }
                return Some((s[..close + 1].into(), s[close + 1..].into()));
            }
        }
    }
    if s.contains(is_line_terminator) {
        return None;
    }
    let end = s
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    Some((s[..end].into(), s[end..].into()))
}

const KEYWORDS: &str = "aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen transparent currentcolor inherit initial unset revert accentcolor accentcolortext activetext buttonborder buttonface buttontext canvas canvastext field fieldtext graytext highlight highlighttext linktext mark marktext selecteditem selecteditemtext visitedtext activeborder activecaption appworkspace background buttonhighlight buttonshadow captiontext inactiveborder inactivecaption inactivecaptiontext infobackground infotext menu menutext scrollbar threeddarkshadow threedface threedhighlight threedlightshadow threedshadow window windowframe windowtext";

// colors.ts `const enum K`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum K {
    Bad,
    Number,
    Percent,
    Angle,
    None,
}

/// Ports `kind`: RE_TERM = /^[+-]?(?:\d+(?:\.\d+)?|\.\d+)(?:e[+-]?\d+)?(%|deg|grad|rad|turn)?$/i.
fn kind(term: &str) -> K {
    if term.to_lowercase() == "none" {
        return K::None;
    }
    let b = term.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let s = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > s {
        if i < b.len() && b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    } else if i < b.len() && b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return K::Bad;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let d = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > d {
            i = j;
        }
    }
    let unit = term[i..].to_ascii_lowercase();
    match unit.as_str() {
        "" => K::Number,
        "%" => K::Percent,
        "deg" | "grad" | "rad" | "turn" => K::Angle,
        _ => K::Bad,
    }
}

/// Ports `isAlpha`.
fn is_alpha(term: &str, modern: bool) -> bool {
    let k = kind(term);
    k == K::Number || k == K::Percent || (modern && k == K::None)
}

/// JS `split(/\s+/)`.
fn split_space(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_run = false;
    for (i, c) in s.char_indices() {
        if is_space(c) {
            if !in_run {
                out.push(&s[start..i]);
                in_run = true;
            }
        } else if in_run {
            start = i;
            in_run = false;
        }
    }
    if in_run {
        out.push("");
    } else {
        out.push(&s[start..]);
    }
    out
}

/// Ports `isFunction`.
fn is_function(color: &str) -> bool {
    let hsl = color.starts_with("hsl");
    let open = color.find('(').unwrap_or(0);
    let name = &color[..open];
    if name != "rgb" && name != "rgba" && name != "hsl" && name != "hsla" {
        return false;
    }
    if crate::pele::js::len16(color) > 120 || color.find(')') != Some(color.len() - 1) {
        return false;
    }
    let inner = trim(&color[open + 1..color.len() - 1]);
    let legacy = inner.contains(',');
    let mut parts: Vec<String>;
    let mut alpha: Option<String> = None;
    if legacy {
        parts = inner.split(',').map(|s| String::from(trim(s))).collect();
        if parts.len() == 4 {
            alpha = parts.pop();
        }
    } else {
        let slash: Vec<&str> = inner.split('/').collect();
        if slash.len() > 2 {
            return false;
        }
        parts = split_space(trim(slash[0]))
            .into_iter()
            .map(String::from)
            .collect();
        if slash.len() == 2 {
            alpha = Some(trim(slash[1]).into());
        }
    }
    if parts.len() != 3 || alpha.as_deref().is_some_and(|a| !is_alpha(a, !legacy)) {
        return false;
    }
    let kinds: Vec<K> = parts.iter().map(|p| kind(p)).collect();
    if legacy {
        if hsl {
            return (kinds[0] == K::Number || kinds[0] == K::Angle)
                && kinds[1] == K::Percent
                && kinds[2] == K::Percent;
        }
        return (kinds[0] == K::Number || kinds[0] == K::Percent)
            && kinds[1] == kinds[0]
            && kinds[2] == kinds[0];
    }
    for (i, &k) in kinds.iter().enumerate() {
        let ok = k == K::Number
            || k == K::None
            || if hsl && i == 0 {
                k == K::Angle
            } else {
                k == K::Percent
            };
        if !ok {
            return false;
        }
    }
    true
}

/// Ports `isCssColor`.
pub fn is_css_color(color: &str) -> bool {
    if color.contains('(') {
        return is_function(color);
    }
    let lower = color.to_lowercase();
    KEYWORDS.split(' ').any(|k| k == lower)
}

/// Ports `entries`.
fn entries(value: &Value) -> Vec<(String, Value)> {
    match value {
        Value::Str(s) => s
            .chars()
            .enumerate()
            .map(|(i, c)| (alloc::format!("{}", i), Value::Str(c.into())))
            .collect(),
        Value::Obj(map) => {
            // Object.entries: integer keys first, ascending, then the rest in order.
            let mut ints: Vec<(u32, String, Value)> = Vec::new();
            let mut rest: Vec<(String, Value)> = Vec::new();
            for (k, v) in map {
                let canonical = k
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n != u32::MAX && alloc::format!("{}", n) == *k);
                match canonical {
                    Some(n) => ints.push((n, k.clone(), v.clone())),
                    None => rest.push((k.clone(), v.clone())),
                }
            }
            ints.sort_by_key(|e| e.0);
            ints.into_iter()
                .map(|(_, k, v)| (k, v))
                .chain(rest)
                .collect()
        }
        Value::Arr(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (alloc::format!("{}", i), v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// A Map's `set`: a key keeps its first place.
fn set<V>(map: &mut Vec<(String, V)>, key: &str, value: V) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = value,
        None => map.push((key.into(), value)),
    }
}

/// Ports `class SeqDb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeqDb {
    /// In insertion order, as a Map.
    pub actors: Vec<(String, SeqActor)>,
    pub created_actors: Vec<(String, usize)>,
    pub destroyed_actors: Vec<(String, usize)>,
    pub boxes: Vec<SeqBox>,
    pub messages: Vec<SeqMessage>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    prev_actor: Option<String>,
    current_box: Option<usize>,
    last_created: Option<String>,
    last_destroyed: Option<String>,
    wrap_enabled: Option<bool>,
    sequence_wrap: bool,
    active: Vec<(String, i64)>,
}

impl SeqDb {
    pub fn new(wrap: Option<bool>, sequence_wrap: bool) -> SeqDb {
        SeqDb {
            wrap_enabled: wrap,
            sequence_wrap,
            ..SeqDb::default()
        }
    }

    pub fn actor(&self, id: &str) -> Option<&SeqActor> {
        self.actors.iter().find(|(k, _)| k == id).map(|(_, a)| a)
    }

    fn actor_mut(&mut self, id: &str) -> Option<&mut SeqActor> {
        self.actors
            .iter_mut()
            .find(|(k, _)| k == id)
            .map(|(_, a)| a)
    }

    fn auto_wrap(&self) -> bool {
        self.wrap_enabled.unwrap_or(self.sequence_wrap)
    }

    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(trim_start(text).into());
    }

    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(tighten_lines(text));
    }

    pub fn set_diagram_title(&mut self, text: &str) {
        self.title = Some(text.into());
    }

    /// Ports `parseBoxData`.
    pub fn parse_box_data(&self, s: &str) -> SeqBoxData {
        let m = split_box(s);
        let mut color = match &m {
            Some((c, _)) if !c.is_empty() => String::from(trim(c)),
            _ => String::from("transparent"),
        };
        let mut title = match &m {
            Some((_, t)) if !t.is_empty() => Some(String::from(trim(t))),
            _ => None,
        };
        if !is_css_color(&color) {
            color = "transparent".into();
            title = Some(trim(s).into());
        }
        let Some(title) = title else {
            return SeqBoxData {
                text: None,
                color,
                wrap: None,
            };
        };
        let SeqText { text, wrap } = extract_wrap(&title);
        SeqBoxData {
            text: if text.is_empty() { None } else { Some(text) },
            color,
            wrap,
        }
    }

    fn add_box(&mut self, data: &SeqBoxData) {
        let wrap = data.wrap.unwrap_or(self.auto_wrap());
        self.boxes.push(SeqBox {
            name: data.text.clone(),
            wrap,
            fill: data.color.clone(),
            actor_keys: Vec::new(),
        });
        self.current_box = Some(self.boxes.len() - 1);
    }

    /// Ports `addActor`.
    fn add_actor(
        &mut self,
        id: &str,
        name: &str,
        mut description: Option<SeqText>,
        mut kind: Option<String>,
        metadata: Option<&str>,
    ) -> Result<(), PeleError> {
        let mut assigned_box = self.current_box;
        let mut doc: Option<Value> = None;
        if let Some(metadata) = metadata {
            let src = if metadata.contains('\n') {
                format!("{}\n", metadata)
            } else {
                format!("{{\n{}\n}}", metadata)
            };
            let parsed = parse_yaml(&src).map_err(|message| {
                PeleError::of(
                    format!(
                        "Config of participant \"{}\" is not valid YAML. {}",
                        id, message
                    ),
                    PeleErrorCode::Syntax,
                    "sequence",
                )
            })?;
            if matches!(parsed, Value::Obj(_)) {
                doc = Some(parsed);
            }
        }
        let doc_type = doc.as_ref().and_then(|d| d.get("type")).cloned();
        let doc_alias = doc.as_ref().and_then(|d| d.get("alias")).cloned();
        if let Some(t) = doc_type.filter(|t| !matches!(t, Value::Null)) {
            kind = Some(to_js_string(&t));
        }
        if truthy(doc_alias.as_ref()) && description.as_ref().is_none_or(|d| d.text == name) {
            description = Some(SeqText {
                text: to_js_string(doc_alias.as_ref().unwrap_or(&Value::Null)),
                wrap: description.as_ref().and_then(|d| d.wrap),
            });
        }

        let current_box = self.current_box;
        if let Some(old) = self.actor(id).cloned() {
            if let (Some(cb), Some(ob)) = (current_box, old.bx) {
                if cb != ob {
                    let name_of = |b: usize| {
                        self.boxes[b]
                            .name
                            .clone()
                            .unwrap_or_else(|| "undefined".into())
                    };
                    return Err(fail(format!(
                        "A same participant should only be defined in one Box: {} can't be in '{}' and in '{}' at the same time.",
                        old.name,
                        name_of(ob),
                        name_of(cb)
                    )));
                }
            }
            assigned_box = if old.bx.is_some() {
                old.bx
            } else {
                current_box
            };
            if let Some(a) = self.actor_mut(id) {
                a.bx = assigned_box;
            }
            if name == old.name && description.is_none() {
                return Ok(());
            }
        }
        // A participant that is only mentioned in a message has no kind and takes its name as its label.
        if description.is_none() || kind.is_none() {
            description = Some(SeqText {
                text: name.into(),
                wrap: None,
            });
        }
        let description = description.unwrap_or_default();
        let actor = SeqActor {
            bx: assigned_box,
            name: name.into(),
            description: description.text,
            wrap: description.wrap.unwrap_or(self.auto_wrap()),
            prev_actor: self.prev_actor.clone(),
            next_actor: None,
            links: Vec::new(),
            properties: Vec::new(),
            details: None,
            kind: kind.unwrap_or_else(|| "participant".into()),
        };
        set(&mut self.actors, id, actor);
        if let Some(prev) = self.prev_actor.clone().filter(|p| !p.is_empty()) {
            if let Some(p) = self.actor_mut(&prev) {
                p.next_actor = Some(id.into());
            }
        }
        if let Some(b) = self.current_box {
            self.boxes[b].actor_keys.push(id.into());
        }
        self.prev_actor = Some(id.into());
        Ok(())
    }

    /// Ports `addSignal`.
    fn add_signal(
        &mut self,
        from: Option<&str>,
        to: Option<&str>,
        message: Option<&SeqText>,
        kind: i32,
        activate: bool,
        central: Option<i32>,
    ) -> Result<(), PeleError> {
        if kind == line::ACTIVE_END {
            let count = match from {
                Some(f) => self
                    .active
                    .iter()
                    .find(|(k, _)| k == f)
                    .map(|(_, c)| *c)
                    .unwrap_or(0),
                None => 0,
            };
            if count < 1 {
                return Err(fail(format!(
                    "Trying to inactivate an inactive participant ({})",
                    from.unwrap_or("undefined")
                )));
            }
            set(&mut self.active, from.unwrap_or(""), count - 1);
        } else if kind == line::ACTIVE_START {
            if let Some(f) = from.filter(|f| !f.is_empty()) {
                let c = self
                    .active
                    .iter()
                    .find(|(k, _)| k == f)
                    .map(|(_, c)| *c)
                    .unwrap_or(0);
                set(&mut self.active, f, c + 1);
            }
        }
        let wrap = message.and_then(|m| m.wrap).unwrap_or(self.auto_wrap());
        self.messages.push(SeqMessage {
            id: format!("{}", self.messages.len()),
            from: from.map(String::from),
            to: to.map(String::from),
            text: Some(message.map(|m| m.text.clone()).unwrap_or_default()),
            numbering: None,
            wrap,
            kind: Some(kind),
            activate,
            placement: None,
            central: central.unwrap_or(0),
        });
        Ok(())
    }

    /// Ports `addNote`.
    fn add_note(&mut self, actor: &NoteActor, placement: i32, message: &SeqText) {
        let wrap = message.wrap.unwrap_or(self.auto_wrap());
        let (a, b) = match actor {
            NoteActor::One(a) => (a.clone(), a.clone()),
            NoteActor::Two(a, b) => (a.clone(), b.clone()),
        };
        self.messages.push(SeqMessage {
            id: format!("{}", self.messages.len()),
            from: Some(a),
            to: Some(b),
            text: Some(message.text.clone()),
            numbering: None,
            wrap,
            kind: Some(line::NOTE),
            activate: false,
            placement: Some(placement),
            central: 0,
        });
    }

    /// Ports `addLinks`, `addALink`, `addProperties` and `addDetails`.
    fn add_menu(&mut self, kind: Menu, actor_id: &str, text: &SeqText) {
        let Some(actor) = self.actor_mut(actor_id) else {
            return;
        };
        match kind {
            Menu::Links | Menu::Properties => {
                let Some(parsed) = crate::json::parse(&text.text) else {
                    return;
                };
                for (k, v) in entries(&parsed) {
                    if kind == Menu::Links {
                        set(&mut actor.links, &k, v);
                    } else {
                        set(&mut actor.properties, &k, v);
                    }
                }
            }
            Menu::ALink => {
                // text.slice(0, sep - 1) and text.slice(sep + 1), in UTF-16 units.
                let t = &text.text;
                let units: Vec<u16> = t.encode_utf16().collect();
                let sep = units
                    .iter()
                    .position(|&c| c == 64)
                    .map(|p| p as isize)
                    .unwrap_or(-1);
                let n = units.len() as isize;
                let clamp = |i: isize| -> usize {
                    if i < 0 {
                        (n + i).max(0) as usize
                    } else {
                        i.min(n) as usize
                    }
                };
                let a = clamp(sep - 1);
                let key = String::from_utf16_lossy(&units[..a]);
                let b = clamp(sep + 1);
                let value = String::from_utf16_lossy(&units[b..]);
                set(
                    &mut actor.links,
                    trim(&key),
                    Value::Str(trim(&value).into()),
                );
            }
            Menu::Details => actor.details = Some(text.text.clone()),
        }
    }

    /// Ports `apply` and `applyOne`.
    pub fn apply(&mut self, statements: &[Stmt]) -> Result<(), PeleError> {
        for s in statements {
            self.apply_one(s)?;
        }
        Ok(())
    }

    fn apply_one(&mut self, param: &Stmt) -> Result<(), PeleError> {
        match param {
            Stmt::Autonumber {
                start,
                step,
                visible,
            } => {
                self.messages.push(SeqMessage {
                    id: format!("{}", self.messages.len()),
                    from: None,
                    to: None,
                    text: None,
                    numbering: Some(SeqNumbering {
                        start: *start,
                        step: *step,
                        visible: *visible,
                    }),
                    wrap: false,
                    kind: Some(line::AUTONUMBER),
                    activate: false,
                    placement: None,
                    central: 0,
                });
            }
            Stmt::Participant {
                create,
                destroy,
                actor,
                draw,
                description,
                config,
            } => {
                if *create {
                    if self.actor(actor).is_some() {
                        return Err(fail("It is not possible to have actors with the same id, even if one is destroyed before the next is created. Use 'AS' aliases to simulate the behavior".into()));
                    }
                    self.last_created = Some(actor.clone());
                    self.add_actor(
                        actor,
                        actor,
                        description.clone(),
                        draw.map(String::from),
                        config.as_deref(),
                    )?;
                    let at = self.messages.len();
                    set(&mut self.created_actors, actor, at);
                } else if *destroy {
                    self.last_destroyed = Some(actor.clone());
                    let at = self.messages.len();
                    set(&mut self.destroyed_actors, actor, at);
                } else {
                    self.add_actor(
                        actor,
                        actor,
                        description.clone(),
                        draw.map(String::from),
                        config.as_deref(),
                    )?;
                }
            }
            Stmt::Signal { signal_type, actor } => {
                self.add_signal(Some(actor), None, None, *signal_type, false, None)?;
            }
            Stmt::Note {
                actor,
                placement,
                text,
            } => self.add_note(actor, *placement, text),
            Stmt::Menu { kind, actor, text } => self.add_menu(*kind, actor, text),
            Stmt::Message {
                from,
                to,
                signal_type,
                msg,
                activate,
                central,
            } => {
                if let Some(created) = self.last_created.clone().filter(|c| !c.is_empty()) {
                    if *to != created {
                        return Err(fail(format!("The created participant {} does not have an associated creating message after its declaration. Please check the sequence diagram.", created)));
                    }
                    self.last_created = None;
                } else if let Some(destroyed) =
                    self.last_destroyed.clone().filter(|d| !d.is_empty())
                {
                    if *to != destroyed && *from != destroyed {
                        return Err(fail(format!("The destroyed participant {} does not have an associated destroying message after its declaration. Please check the sequence diagram.", destroyed)));
                    }
                    self.last_destroyed = None;
                }
                self.add_signal(
                    Some(from),
                    Some(to),
                    Some(msg),
                    *signal_type,
                    *activate,
                    *central,
                )?;
            }
            Stmt::BoxStart(data) => self.add_box(data),
            Stmt::BoxEnd => self.current_box = None,
            Stmt::Block { signal_type, text } => {
                self.add_signal(None, None, text.as_ref(), *signal_type, false, None)?;
            }
        }
        Ok(())
    }
}
