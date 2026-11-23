//! Ports diagrams/sequence/render.ts.
//!
//! Pele's `draw` is one function with closures over its state; the port
//! keeps that state in [`Draw`], and the closures are its methods.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::actors::{actor_kind, actor_size, draw_actor, is_figure, ActorSize, PAD_X};
use super::db::{line, SeqActor, SeqDb, SeqMessage, LEFTOF, RIGHTOF};
use crate::json::Value;
use crate::pele::common::fit_width::{tighten, title_room};
use crate::pele::common::literal_label::break_label;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::edges::{marker, ARROW};
use crate::pele::svg::ink::ink;
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{class_names, resolve_style, rounded, FONT_MONO, ROUNDED};
use crate::pele::text::label::{empty_label, layout_label, Icons, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

// `const enum Head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    None,
    Arrow,
    Cross,
    Open,
    HalfSolid,
    HalfStick,
}

/// Ports `interface LineStyle`.
#[derive(Debug, Clone, Copy)]
struct LineStyle {
    dotted: bool,
    head: Head,
    /// Where the heads are: at the receiver, at the sender only (the reverse arrows), or at both.
    at_start: bool,
    at_end: bool,
    bottom: bool,
}

/// Ports `STYLES[type]`.
fn style_of(kind: i32) -> Option<LineStyle> {
    let s = |dotted: bool, head: Head, ends: u8, bottom: bool| LineStyle {
        dotted,
        head,
        at_start: ends != 1,
        at_end: ends != 0,
        bottom,
    };
    Some(match kind {
        line::SOLID => s(false, Head::Arrow, 1, false),
        line::DOTTED => s(true, Head::Arrow, 1, false),
        line::SOLID_CROSS => s(false, Head::Cross, 1, false),
        line::DOTTED_CROSS => s(true, Head::Cross, 1, false),
        line::SOLID_OPEN => s(false, Head::None, 1, false),
        line::DOTTED_OPEN => s(true, Head::None, 1, false),
        line::SOLID_POINT => s(false, Head::Open, 1, false),
        line::DOTTED_POINT => s(true, Head::Open, 1, false),
        line::BIDIRECTIONAL_SOLID => s(false, Head::Arrow, 2, false),
        line::BIDIRECTIONAL_DOTTED => s(true, Head::Arrow, 2, false),
        _ => {
            let (dotted, base) = if (51..=58).contains(&kind) {
                (true, kind - 10)
            } else {
                (false, kind)
            };
            match base {
                line::SOLID_TOP => s(dotted, Head::HalfSolid, 1, false),
                line::SOLID_BOTTOM => s(dotted, Head::HalfSolid, 1, true),
                line::STICK_TOP => s(dotted, Head::HalfStick, 1, false),
                line::STICK_BOTTOM => s(dotted, Head::HalfStick, 1, true),
                line::SOLID_ARROW_TOP_REVERSE => s(dotted, Head::HalfSolid, 0, false),
                line::SOLID_ARROW_BOTTOM_REVERSE => s(dotted, Head::HalfSolid, 0, true),
                line::STICK_ARROW_TOP_REVERSE => s(dotted, Head::HalfStick, 0, false),
                line::STICK_ARROW_BOTTOM_REVERSE => s(dotted, Head::HalfStick, 0, true),
                _ => return None,
            }
        }
    })
}

/// Ports `FRAME_KIND`.
fn frame_kind(kind: i32) -> Option<&'static str> {
    Some(match kind {
        line::LOOP_START => "loop",
        line::ALT_START => "alt",
        line::OPT_START => "opt",
        line::PAR_START | line::PAR_OVER_START => "par",
        line::CRITICAL_START => "critical",
        line::BREAK_START => "break",
        line::RECT_START => "rect",
        _ => return None,
    })
}

const MSG_PAD: f64 = 12.0;
const SELF_W: f64 = 28.0;
const SELF_H: f64 = 22.0;
const ACT_W: f64 = 10.0;
const ACT_STEP: f64 = 4.0;
const ACT_MIN: f64 = 14.0;
const NOTE_GAP: f64 = 10.0;
const NOTE_OVER: f64 = 16.0;
const FRAME_PAD: f64 = 20.0;
const FRAME_NEST: f64 = 10.0;
const TAB_H: f64 = 20.0;
const FRAME_TEXT: f64 = 240.0;
const BADGE_H: f64 = 16.0;
const CENTRAL_R: f64 = 4.0;
const DASH: &str = " stroke-dasharray=\"4 4\"";

/// Ports `isLineMessage`.
pub fn is_line_message(kind: Option<i32>) -> bool {
    kind.is_some_and(|k| style_of(k).is_some())
}

/// Ports `interface Numbering`.
pub struct Numbering {
    pub index: Vec<f64>,
    pub shown: Vec<bool>,
    pub visible: bool,
}

/// `x || y` for a number.
fn or(v: Option<f64>, y: f64) -> f64 {
    match v {
        Some(x) if x != 0.0 && !x.is_nan() => x,
        _ => y,
    }
}

/// Ports `numberMessages`: counts message lines the way Mermaid's
/// renderer does while it draws.
pub fn number_messages(messages: &[SeqMessage], always: bool) -> Numbering {
    let mut index = alloc::vec![f64::NAN; messages.len()];
    let mut shown = alloc::vec![false; messages.len()];
    let mut current = 1.0;
    let mut step = 1.0;
    let mut visible = false;
    for (i, m) in messages.iter().enumerate() {
        if m.kind == Some(line::AUTONUMBER) {
            if let Some(spec) = &m.numbering {
                current = or(spec.start, current);
                step = or(spec.step, step);
                visible = spec.visible;
            }
        } else if is_line_message(m.kind) {
            index[i] = current;
            shown[i] = visible || always;
            current = jround((current + step) * 100.0) / 100.0;
        }
    }
    Numbering {
        index,
        shown,
        visible,
    }
}

/// Ports `interface Column`.
#[derive(Clone)]
struct Column {
    id: String,
    actor: usize,
    kind: &'static str,
    label: Label,
    size: ActorSize,
    x: f64,
    create_at: isize,
    destroy_at: isize,
    top: f64,
    life_start: f64,
    life_end: f64,
    activations: Vec<f64>,
}

// `const enum V`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V {
    Line,
    SelfLoop,
    Note,
}

/// Ports `interface View`.
#[derive(Clone)]
struct View {
    kind: V,
    a: usize,
    b: usize,
    label: Label,
    x: f64,
    w: f64,
}

/// Ports `interface Frame`.
#[derive(Clone)]
struct Frame {
    kind: &'static str,
    x0: f64,
    x1: f64,
    top: f64,
    title: Label,
    tab_w: f64,
    fill: String,
    dividers: Vec<usize>,
    divider_y: Vec<f64>,
    divider_labels: Vec<Label>,
}

/// Ports `interface BoxRun`.
#[derive(Clone)]
struct BoxRun {
    bx: usize,
    first: usize,
    last: usize,
    title: Label,
    grow: f64,
}

/// Ports `option`.
fn option(conf: &Value, key: &str, fallback: f64) -> f64 {
    match conf.get(key) {
        Some(Value::Num(v)) if *v >= 0.0 && *v <= 2000.0 => *v,
        _ => fallback,
    }
}

/// Ports `half`.
fn half(x: f64, y: f64, dir: f64, up: f64, stick: bool, color: &str) -> String {
    let bx = num(x - dir * ARROW);
    if stick {
        return format!(
            "<path class=\"pele-marker\" d=\"M{},{}L{},{}\"/>",
            num(x),
            num(y),
            bx,
            num(y + up * 5.0)
        );
    }
    format!(
        "<path class=\"pele-marker\" d=\"M{},{}L{},{}V{}Z\" fill=\"{}\" stroke=\"none\"/>",
        num(x),
        num(y),
        bx,
        num(y + up * 4.5),
        num(y),
        color
    )
}

/// Ports `head`: the head whose tip is at (x, y) and points along dir.
fn head(kind: Head, x: f64, y: f64, dir: f64, up: f64, color: &str) -> String {
    match kind {
        Head::Arrow => marker("arrow_point", x, y, dir, 0.0, color, "none"),
        Head::Cross => marker("arrow_cross", x, y, dir, 0.0, color, "none"),
        Head::Open => format!(
            "<path class=\"pele-marker\" d=\"M{},{}L{},{}L{},{}\" stroke-linejoin=\"round\"/>",
            num(x - dir * ARROW),
            num(y - 4.5),
            num(x),
            num(y),
            num(x - dir * ARROW),
            num(y + 4.5)
        ),
        Head::HalfSolid => half(x, y, dir, up, false, color),
        Head::HalfStick => half(x, y, dir, up, true, color),
        Head::None => String::new(),
    }
}

/// Ports `trim`.
fn trim_of(kind: Head) -> f64 {
    match kind {
        Head::Arrow => ARROW - 1.0,
        Head::Cross => 4.0,
        _ => 0.0,
    }
}

/// Measures in the monospace style (`messageMeasurer` with `monospace` set).
struct Mono<'a>(&'a dyn Measure);

impl Measure for Mono<'_> {
    fn width(&self, text: &str, size: f64, s: u32) -> f64 {
        self.0.width(text, size, s | style::MONO)
    }
}

/// Ports `renderSequence`.
pub fn render_sequence(
    db: &SeqDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    tighten(options, |tight| -> Result<Rendered, PeleError> {
        Ok(draw(db, config, options, palette, tight, 0.0, true))
    })
}

/// The state of one `draw`.
struct Draw<'a> {
    tight: bool,
    small: f64,
    measurer: &'a dyn Measure,
    message_measurer: &'a dyn Measure,
    icons: Icons<'a>,
    wrap_w: f64,
    note_pad: f64,
    box_pad: f64,
    msg_pad: f64,
    note_over: f64,
    frame_pad: f64,
    frame_nest: f64,
    tiny: f64,
    messages: &'a [SeqMessage],
    count: usize,
    cols: Vec<Column>,
    runs: Vec<BoxRun>,
    gaps: Vec<f64>,
    views: Vec<Option<View>>,
    sized: Vec<Option<Vec<(f64, Label)>>>,
    frames: Vec<Frame>,
    frame_at: Vec<Option<usize>>,
    min_x: f64,
    max_x: f64,
    numbering: Numbering,
}

impl Draw<'_> {
    /// Ports `text`.
    fn text(
        &self,
        raw: Option<&str>,
        font_size: f64,
        wrap: bool,
        width: f64,
        by: &dyn Measure,
    ) -> Label {
        let label = layout_label(
            raw,
            false,
            by,
            font_size,
            if self.tight || wrap {
                width
            } else {
                f64::INFINITY
            },
            0,
            self.icons,
        );
        if self.tight {
            break_label(&label, by, jmax(120.0, width))
        } else {
            label
        }
    }

    /// Ports `messageText`.
    fn message_text(&self, raw: &str, wrap: bool, width: f64) -> Label {
        self.text(Some(raw), self.small, wrap, width, self.message_measurer)
    }

    /// Ports `badgeWidth`.
    fn badge_width(&self, i: usize) -> f64 {
        if self.numbering.shown[i] {
            jmax(
                BADGE_H,
                self.measurer
                    .width(&num_str(self.numbering.index[i]), self.tiny, 0)
                    + 8.0,
            )
        } else {
            0.0
        }
    }

    /// Ports `sizedText`.
    fn sized_text(&mut self, i: usize, room: f64) -> Label {
        if self.sized[i].is_none() {
            let current = self.views[i]
                .as_ref()
                .map(|v| v.label.clone())
                .unwrap_or_else(empty_label);
            if room == self.wrap_w {
                return current;
            }
            self.sized[i] = Some(alloc::vec![(self.wrap_w, current)]);
        }
        if let Some(found) = self.sized[i]
            .as_ref()
            .and_then(|k| k.iter().find(|(r, _)| *r == room))
        {
            return found.1.clone();
        }
        let label = self.message_text(self.messages[i].text.as_deref().unwrap_or(""), true, room);
        if let Some(k) = self.sized[i].as_mut() {
            k.push((room, label.clone()));
        }
        label
    }

    /// Ports `place`.
    fn place(&mut self) {
        for i in 1..self.cols.len() {
            self.cols[i].x = self.cols[i - 1].x + self.gaps[i - 1];
        }
    }

    /// Ports `extend`.
    fn extend(&mut self, x0: f64, x1: f64) {
        if x0 < self.min_x {
            self.min_x = x0;
        }
        if x1 > self.max_x {
            self.max_x = x1;
        }
    }

    /// Ports `boxEdges`.
    fn box_edges(&self, run: &BoxRun) -> (f64, f64) {
        let f = &self.cols[run.first];
        let l = &self.cols[run.last];
        (
            f.x - f.size.w / 2.0 - self.box_pad - run.grow,
            l.x + l.size.w / 2.0 + self.box_pad + run.grow,
        )
    }

    /// Ports `layout`: horizontal extents of every block, from what it holds.
    fn layout(&mut self) {
        self.place();
        self.min_x = f64::INFINITY;
        self.max_x = f64::NEG_INFINITY;
        for k in 0..self.cols.len() {
            let c = &self.cols[k];
            let (x0, x1) = (c.x - c.size.w / 2.0, c.x + c.size.w / 2.0);
            self.extend(x0, x1);
        }
        for k in 0..self.runs.len() {
            let (x0, x1) = self.box_edges(&self.runs[k].clone());
            self.extend(x0, x1);
        }
        self.frames.clear();
        for f in self.frame_at.iter_mut() {
            *f = None;
        }

        let last = self.cols.len() as isize - 1;
        let mut open: Vec<usize> = Vec::new();
        for i in 0..self.count {
            let m = &self.messages[i];
            let kind = m.kind;
            let fk = kind.and_then(frame_kind);
            if let Some(fk) = fk {
                let frame = Frame {
                    kind: fk,
                    x0: f64::INFINITY,
                    x1: f64::NEG_INFINITY,
                    top: 0.0,
                    title: empty_label(),
                    tab_w: 0.0,
                    fill: if fk == "rect" {
                        m.text.clone().unwrap_or_default()
                    } else {
                        String::new()
                    },
                    dividers: alloc::vec![i],
                    divider_y: Vec::new(),
                    divider_labels: Vec::new(),
                };
                self.frames.push(frame);
                let f = self.frames.len() - 1;
                self.frame_at[i] = Some(f);
                open.push(f);
            } else if matches!(
                kind,
                Some(line::ALT_ELSE) | Some(line::PAR_AND) | Some(line::CRITICAL_OPTION)
            ) {
                if let Some(&f) = open.last() {
                    self.frames[f].dividers.push(i);
                }
            } else if matches!(
                kind,
                Some(line::LOOP_END)
                    | Some(line::ALT_END)
                    | Some(line::OPT_END)
                    | Some(line::PAR_END)
                    | Some(line::CRITICAL_END)
                    | Some(line::BREAK_END)
                    | Some(line::RECT_END)
            ) {
                let Some(f) = open.pop() else {
                    continue;
                };
                self.frame_at[i] = Some(f);
                if self.frames[f].x0 > self.frames[f].x1 {
                    let (a, b) = if last < 0 {
                        (0.0, 120.0)
                    } else {
                        (
                            self.cols[0].x - self.frame_pad,
                            self.cols[last as usize].x + self.frame_pad,
                        )
                    };
                    self.frames[f].x0 = a;
                    self.frames[f].x1 = b;
                }
                if self.frames[f].kind != "rect" {
                    let tab_w = self
                        .measurer
                        .width(self.frames[f].kind, self.tiny, style::BOLD)
                        + 14.0;
                    self.frames[f].tab_w = tab_w;
                    // A block grows to the right to give its texts up to FRAME_TEXT of width before they wrap.
                    let dividers = self.frames[f].dividers.clone();
                    for (k, &d) in dividers.iter().enumerate() {
                        let raw = self.messages[d].text.clone().unwrap_or_default();
                        let tab = if k == 0 { tab_w + 8.0 } else { 0.0 };
                        let room = self.frames[f].x1 - self.frames[f].x0 - 16.0 - tab;
                        let label = if !raw.is_empty() {
                            self.text(
                                Some(&format!("[{}]", raw)),
                                self.small,
                                true,
                                jmax(
                                    room,
                                    if self.tight {
                                        jmin(FRAME_TEXT, self.wrap_w)
                                    } else {
                                        FRAME_TEXT
                                    },
                                ),
                                self.measurer,
                            )
                        } else {
                            empty_label()
                        };
                        if label.width > room {
                            self.frames[f].x1 += label.width - room;
                        }
                        if k == 0 {
                            self.frames[f].title = label;
                        } else {
                            self.frames[f].divider_labels.push(label);
                        }
                    }
                }
                let (x0, x1) = (self.frames[f].x0, self.frames[f].x1);
                self.extend(x0, x1);
                if let Some(&p) = open.last() {
                    self.frames[p].x0 = jmin(self.frames[p].x0, x0 - self.frame_nest);
                    self.frames[p].x1 = jmax(self.frames[p].x1, x1 + self.frame_nest);
                }
            } else if let Some(mut view) = self.views[i].clone() {
                let a = self.cols[view.a].clone();
                let b = self.cols[view.b].clone();
                let x0;
                let mut x1;
                if view.kind == V::Note {
                    if m.placement == Some(LEFTOF) {
                        view.x = a.x - NOTE_GAP - view.w;
                    } else if m.placement == Some(RIGHTOF) {
                        view.x = a.x + NOTE_GAP;
                    } else if view.a == view.b {
                        view.x = a.x - view.w / 2.0;
                    } else {
                        let lo = jmin(a.x, b.x) - self.note_over;
                        let hi = jmax(a.x, b.x) + self.note_over;
                        if self.tight || m.wrap {
                            view.label = self.text(
                                m.text.as_deref(),
                                self.small,
                                true,
                                jmax(self.wrap_w, hi - lo - 2.0 * self.note_pad),
                                self.measurer,
                            );
                        }
                        view.w = jmax(hi - lo, view.label.width + 2.0 * self.note_pad);
                        view.x = (lo + hi - view.w) / 2.0;
                    }
                    x0 = view.x;
                    x1 = view.x + view.w;
                } else if view.kind == V::SelfLoop {
                    x0 = a.x;
                    x1 = a.x + ACT_W + SELF_W + 8.0 + view.label.width + self.badge_width(i);
                } else {
                    let mut lo = jmin(a.x, b.x);
                    x1 = jmax(a.x, b.x);
                    if self.tight || m.wrap {
                        self.views[i] = Some(view.clone());
                        view.label = self
                            .sized_text(i, jmax(self.wrap_w, x1 - lo - 2.0 * self.msg_pad - ACT_W));
                    }
                    if b.create_at == i as isize {
                        if b.x > a.x {
                            x1 += b.size.w / 2.0;
                        } else {
                            lo -= b.size.w / 2.0;
                        }
                    }
                    x0 = lo;
                }
                self.views[i] = Some(view);
                self.extend(x0, x1);
                if let Some(&f) = open.last() {
                    self.frames[f].x0 = jmin(self.frames[f].x0, x0 - self.frame_pad);
                    self.frames[f].x1 = jmax(self.frames[f].x1, x1 + self.frame_pad);
                }
            }
        }
    }
}

/// Ports `draw`.
#[allow(clippy::too_many_arguments)]
fn draw(
    db: &SeqDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
    tight: bool,
    hold: f64,
    by_need: bool,
) -> Rendered {
    let empty = Value::obj();
    let conf = match config.get("sequence") {
        Some(v) => v,
        None => &empty,
    };
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let tiny = jround(size * 0.75);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let monospaced = options
        .monospace
        .as_ref()
        .is_some_and(|m| m.iter().any(|p| p == "messages"));
    let mono = Mono(measurer);
    let message_measurer: &dyn Measure = if monospaced { &mono } else { measurer };
    let icons = options.icons;
    let pad = options.padding.unwrap_or(8.0);
    let mirror = conf.get("mirrorActors") != Some(&Value::Bool(false));
    let right_angles = conf.get("rightAngles") == Some(&Value::Bool(true));
    let actor_margin = jmin(
        option(conf, "actorMargin", 40.0),
        if tight { 8.0 } else { f64::INFINITY },
    );
    let min_w = jmin(
        option(conf, "width", 64.0),
        if tight { 48.0 } else { f64::INFINITY },
    );
    let min_h = option(conf, "height", jround(size * 1.5) + 20.0);
    let wrap_w = jmin(
        option(conf, "width", 160.0),
        if tight {
            jmax(
                32.0,
                (options.max_width.unwrap_or(320.0) - 2.0 * pad)
                    / jmax(1.0, db.actors.len() as f64)
                    - actor_margin
                    - 24.0,
            )
        } else {
            f64::INFINITY
        },
    );
    // Mermaid's default of 35 corresponds to Pele's default gap. Other values move it by the difference.
    let msg_gap = jmax(4.0, 14.0 + option(conf, "messageMargin", 35.0) - 35.0);
    let note_pad = jmin(
        option(conf, "noteMargin", 10.0),
        if tight { 6.0 } else { f64::INFINITY },
    );
    let box_pad = jmin(
        option(conf, "boxMargin", 10.0),
        if tight { 6.0 } else { f64::INFINITY },
    );
    let align = match conf.get("messageAlign") {
        Some(Value::Str(s)) if s == "left" => -1,
        Some(Value::Str(s)) if s == "right" => 1,
        _ => 0,
    };

    let msg_pad = if tight { 6.0 } else { MSG_PAD };
    let note_over = if tight { 8.0 } else { NOTE_OVER };
    let frame_pad = if tight { 12.0 } else { FRAME_PAD };
    let frame_nest = if tight { 6.0 } else { FRAME_NEST };
    let messages = &db.messages[..];
    let count = messages.len();
    let numbering = number_messages(
        messages,
        conf.get("showSequenceNumbers") == Some(&Value::Bool(true)),
    );

    let mut d = Draw {
        tight,
        small,
        measurer,
        message_measurer,
        icons,
        wrap_w,
        note_pad,
        box_pad,
        msg_pad,
        note_over,
        frame_pad,
        frame_nest,
        tiny,
        messages,
        count,
        cols: Vec::new(),
        runs: Vec::new(),
        gaps: Vec::new(),
        views: alloc::vec![None; count],
        sized: alloc::vec![None; count],
        frames: Vec::new(),
        frame_at: alloc::vec![None; count],
        min_x: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        numbering,
    };

    let used: Option<Vec<Option<&str>>> =
        if conf.get("hideUnusedParticipants") == Some(&Value::Bool(true)) {
            let mut u = Vec::new();
            for m in messages {
                u.push(m.from.as_deref());
                u.push(m.to.as_deref());
            }
            Some(u)
        } else {
            None
        };

    let mut next_line = alloc::vec![-1isize; count + 1];
    for i in (0..count).rev() {
        next_line[i] = if is_line_message(messages[i].kind) {
            i as isize
        } else {
            next_line[i + 1]
        };
    }

    let col_of =
        |cols: &[Column], id: &str| -> Option<usize> { cols.iter().position(|c| c.id == id) };
    for (k, (id, actor)) in db.actors.iter().enumerate() {
        if let Some(u) = &used {
            if !u.contains(&Some(id.as_str())) {
                continue;
            }
        }
        let kind = actor_kind(&actor.kind);
        let label = d.text(Some(&actor.description), size, actor.wrap, wrap_w, measurer);
        let created = db
            .created_actors
            .iter()
            .find(|(a, _)| a == id)
            .map(|(_, v)| *v);
        let destroyed = db
            .destroyed_actors
            .iter()
            .find(|(a, _)| a == id)
            .map(|(_, v)| *v);
        let mut create_at = match created {
            None => -1,
            Some(c) => next_line[c.min(count)],
        };
        if create_at >= 0 && messages[create_at as usize].to.as_deref() != Some(id) {
            create_at = -1;
        }
        let mut destroy_at = match destroyed {
            None => -1,
            Some(c) => next_line[c.min(count)],
        };
        if destroy_at >= 0 {
            let m = &messages[destroy_at as usize];
            if m.to.as_deref() != Some(id) && m.from.as_deref() != Some(id) {
                destroy_at = -1;
            }
        }
        let size_of = actor_size(kind, &label, min_w, min_h, if tight { 8.0 } else { PAD_X });
        d.cols.push(Column {
            id: id.clone(),
            actor: k,
            kind,
            label,
            size: size_of,
            x: 0.0,
            create_at,
            destroy_at,
            top: 0.0,
            life_start: 0.0,
            life_end: -1.0,
            activations: Vec::new(),
        });
    }
    let actor_of = |c: &Column| -> &SeqActor { &db.actors[c.actor].1 };
    let ncols = d.cols.len();
    let last = ncols as isize - 1;

    // Widths between lifelines. Every message and note says how far apart the lifelines it touches must be.
    let mut needs: Vec<(usize, f64)> = Vec::new();
    let need = |needs: &mut Vec<(usize, f64)>, lo: isize, hi: isize, width: f64| {
        if lo < 0 || hi > last {
            return;
        }
        let key = lo as usize * ncols + hi as usize;
        match needs.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => {
                if e.1 < width {
                    e.1 = width;
                }
            }
            None => {
                if 0.0 < width {
                    needs.push((key, width));
                }
            }
        }
    };

    for (i, m) in messages.iter().enumerate() {
        let a = m.from.as_deref().and_then(|f| col_of(&d.cols, f));
        let b = m.to.as_deref().and_then(|t| col_of(&d.cols, t));
        let (Some(a), Some(b), Some(text)) = (a, b, m.text.as_deref()) else {
            continue;
        };
        let (ai, bi) = (a as isize, b as isize);
        if m.kind == Some(line::NOTE) {
            let label = d.text(Some(text), small, m.wrap, wrap_w, measurer);
            let w = label.width + 2.0 * note_pad;
            if m.placement == Some(LEFTOF) {
                need(&mut needs, ai - 1, ai, w + 2.0 * NOTE_GAP + ACT_W);
            } else if m.placement == Some(RIGHTOF) {
                need(&mut needs, ai, ai + 1, w + 2.0 * NOTE_GAP + ACT_W);
            } else if a == b {
                need(&mut needs, ai - 1, ai, w / 2.0 + NOTE_GAP);
                need(&mut needs, ai, ai + 1, w / 2.0 + NOTE_GAP);
            } else {
                need(&mut needs, ai.min(bi), ai.max(bi), w - 2.0 * note_over);
            }
            d.views[i] = Some(View {
                kind: V::Note,
                a,
                b,
                label,
                x: 0.0,
                w,
            });
        } else if is_line_message(m.kind) {
            let label = d.message_text(text, m.wrap, wrap_w);
            let badge = d.badge_width(i);
            if a == b {
                need(
                    &mut needs,
                    ai,
                    ai + 1,
                    SELF_W + 8.0 + label.width + 16.0 + ACT_W + badge,
                );
                d.views[i] = Some(View {
                    kind: V::SelfLoop,
                    a,
                    b,
                    label,
                    x: 0.0,
                    w: 0.0,
                });
            } else {
                let created = if d.cols[b].create_at == i as isize {
                    d.cols[b].size.w / 2.0
                } else {
                    0.0
                };
                let central = if m.central != 0 { 4.0 * CENTRAL_R } else { 0.0 };
                let lw = if tight {
                    jmin(label.width, jmax(16.0, wrap_w - 16.0))
                } else {
                    label.width
                };
                need(
                    &mut needs,
                    ai.min(bi),
                    ai.max(bi),
                    lw + 2.0 * msg_pad + ACT_W + badge + created + central,
                );
                d.views[i] = Some(View {
                    kind: V::Line,
                    a,
                    b,
                    label,
                    x: 0.0,
                    w: 0.0,
                });
            }
        }
    }

    for i in 0..ncols {
        let bx = actor_of(&d.cols[i]).bx;
        let Some(bx) = bx else {
            continue;
        };
        match d.runs.last_mut() {
            Some(run) if run.bx == bx && run.last + 1 == i => run.last = i,
            _ => {
                let b = &db.boxes[bx];
                let title = d.text(b.name.as_deref(), small, b.wrap, wrap_w, measurer);
                d.runs.push(BoxRun {
                    bx,
                    first: i,
                    last: i,
                    title,
                    grow: 0.0,
                });
            }
        }
    }

    d.gaps = alloc::vec![0.0; ncols.saturating_sub(1)];
    for i in 0..ncols.saturating_sub(1) {
        let left = &d.cols[i];
        let right = &d.cols[i + 1];
        let (lb, rb) = (actor_of(left).bx, actor_of(right).bx);
        let boxed = if lb != rb {
            (if lb.is_some() { box_pad } else { 0.0 }) + (if rb.is_some() { box_pad } else { 0.0 })
        } else {
            0.0
        };
        let key = i * ncols + i + 1;
        let n = needs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, w)| *w)
            .unwrap_or(0.0);
        d.gaps[i] = jmax(
            left.size.w / 2.0 + right.size.w / 2.0 + actor_margin + boxed,
            n,
        );
    }
    // A message across several lifelines widens each gap it crosses by an equal share of what is missing.
    let mut spans: Vec<(usize, usize, f64)> = Vec::new();
    for &(key, width) in &needs {
        let lo = key / ncols;
        let hi = key % ncols;
        if hi as isize - lo as isize > 1 {
            spans.push((lo, hi, width));
        }
    }
    spans.sort_by(|p, q| {
        let a = (p.1 - p.0) as isize - (q.1 - q.0) as isize;
        let a = if a != 0 {
            a
        } else {
            p.0 as isize - q.0 as isize
        };
        a.cmp(&0)
    });
    for &(lo, hi, width) in &spans {
        let mut total = 0.0;
        for g in lo..hi {
            total += d.gaps[g];
        }
        if total >= width {
            continue;
        }
        let share = (width - total) / (hi - lo) as f64;
        for g in lo..hi {
            d.gaps[g] += share;
        }
    }
    d.place();
    for k in 0..d.runs.len() {
        let run = &d.runs[k];
        let inner = d.cols[run.last].x + d.cols[run.last].size.w / 2.0
            - (d.cols[run.first].x - d.cols[run.first].size.w / 2.0);
        let grow = jmax(0.0, (run.title.width - inner) / 2.0);
        let (first, lst) = (run.first, run.last);
        d.runs[k].grow = grow;
        if grow == 0.0 {
            continue;
        }
        if first > 0 {
            d.gaps[first - 1] += grow;
        }
        if (lst as isize) < last {
            d.gaps[lst] += grow;
        }
    }
    d.layout();

    // Widen gaps only if it reduces message wrapping.
    let mut grown = false;
    let mut uneven = false;
    if let (true, Some(max_width)) = (tight, options.max_width) {
        let spare = max_width.floor() - (d.max_x - d.min_x + 2.0 * pad).ceil() - hold;
        let mut stretch = alloc::vec![0i32; ncols];
        let mut want = alloc::vec![0.0f64; d.gaps.len()];
        let mut wrapped: Vec<usize> = Vec::new();
        let mut lines = 0usize;
        let mut i = 0;
        while spare > 0.0 && i < d.views.len() {
            let Some(view) = d.views[i].clone().filter(|v| v.kind == V::Line) else {
                i += 1;
                continue;
            };
            let natural = d.message_text(
                messages[i].text.as_deref().unwrap_or(""),
                false,
                f64::INFINITY,
            );
            if view.label.lines.len() <= natural.lines.len() {
                i += 1;
                continue;
            }
            wrapped.push(i);
            lines += view.label.lines.len();
            let lo = view.a.min(view.b);
            let hi = view.a.max(view.b);
            stretch[lo] += 1;
            stretch[hi] -= 1;
            let short = (natural.width - (d.cols[hi].x - d.cols[lo].x - 2.0 * msg_pad - ACT_W))
                / (hi - lo) as f64;
            for w in &mut want[lo..hi] {
                *w = jmax(*w, short);
            }
            i += 1;
        }
        let mut active = 0;
        let mut crossed = 0.0;
        for st in stretch.iter_mut().take(d.gaps.len()) {
            active += *st;
            *st = if active > 0 { 1 } else { 0 };
            crossed += *st as f64;
        }
        if crossed > 0.0 {
            let before = d.gaps.clone();
            let spread = |d: &mut Draw, share: &dyn Fn(usize) -> f64| -> Vec<usize> {
                for (g, gap) in d.gaps.iter_mut().enumerate() {
                    *gap = before[g] + share(g);
                }
                d.layout();
                wrapped
                    .iter()
                    .map(|&i| {
                        d.views[i]
                            .as_ref()
                            .map(|v| v.label.lines.len())
                            .unwrap_or(0)
                    })
                    .collect()
            };
            let sum = |counts: &[usize]| -> usize { counts.iter().sum() };
            let even = |g: usize| -> f64 { (stretch[g] as f64 * spare) / crossed };
            let mut counts = spread(&mut d, &even);
            let missing: f64 = want.iter().sum();
            // Allow for rounding error.
            let needs_of = |g: usize| -> f64 { (want[g] * (spare - 0.01)) / missing };
            // Reducing wrapping requires at least one gap wider than the even split.
            if by_need
                && crossed > 1.0
                && missing > 0.0
                && (0..d.gaps.len()).any(|g| needs_of(g) > even(g))
            {
                let first = counts.clone();
                counts = spread(&mut d, &needs_of);
                uneven = sum(&counts) < sum(&first)
                    && counts.iter().zip(first.iter()).all(|(n, f)| n <= f);
                if !uneven {
                    counts = spread(&mut d, &even);
                }
            }
            grown = sum(&counts) < lines;
            if !grown {
                d.gaps = before.clone();
                d.layout();
            }
        }
    }

    // Vertical placement, top to bottom, drawing as it goes.
    let mut box_title_h: f64 = 0.0;
    for run in &d.runs {
        box_title_h = jmax(box_title_h, run.title.height);
    }
    let box_top = if !d.runs.is_empty() {
        box_pad
            + if box_title_h > 0.0 {
                box_title_h + 4.0
            } else {
                0.0
            }
    } else {
        0.0
    };
    let mut row_h: f64 = 0.0;
    for col in &d.cols {
        if col.create_at < 0 {
            row_h = jmax(row_h, col.size.h);
        }
    }
    let head_bottom = box_top + row_h;
    for col in d.cols.iter_mut() {
        col.top = head_bottom - col.size.h;
        col.life_start = head_bottom;
    }

    let xs: Vec<f64> = d.cols.iter().map(|c| c.x).collect();
    // Whether a lifeline runs between two x positions, where text would be struck through.
    let crosses = |x0: f64, x1: f64| -> bool {
        let mut lo = 0;
        let mut hi = xs.len();
        while lo < hi {
            let mid = (lo + hi) >> 1;
            if xs[mid] <= x0 {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo < xs.len() && xs[lo] < x1
    };
    let mut stack: Vec<usize> = Vec::new();
    let ink_of = |fill: Option<&str>| -> Option<&'static str> {
        let fill = fill.filter(|f| !f.is_empty())?;
        ink(&resolve_style(&[format!("fill:{}", fill)]).shape, "")
    };
    let ink_on = |fill: Option<&str>| -> String {
        match ink_of(fill) {
            Some(c) => format!(" fill=\"{}\"", c),
            None => String::new(),
        }
    };
    let runs = d.runs.clone();
    let run_edges: Vec<(f64, f64)> = runs.iter().map(|r| d.box_edges(r)).collect();
    // Prefer the innermost rect's fill, then a containing box's.
    let fill_under = |stack: &[usize], frames: &[Frame], l: f64, r: f64| -> Option<String> {
        for &k in stack.iter().rev() {
            if frames[k].kind == "rect" {
                return Some(frames[k].fill.clone());
            }
        }
        for (k, run) in runs.iter().enumerate() {
            let (x0, x1) = run_edges[k];
            if l >= x0 && r <= x1 {
                return Some(db.boxes[run.bx].fill.clone());
            }
        }
        None
    };
    let under = |stack: &[usize], frames: &[Frame], l: f64, r: f64| -> String {
        ink_on(fill_under(stack, frames, l, r).as_deref())
    };
    let backed = |stack: &[usize],
                  frames: &[Frame],
                  label: &Label,
                  cx: f64,
                  cy: f64,
                  frame: bool|
     -> String {
        if label.width == 0.0 {
            return String::new();
        }
        let l = cx - label.width / 2.0;
        let plate = crosses(l, l + label.width);
        let back = if plate {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"var(--_bg)\" stroke=\"none\"/>",
                num(l - 3.0),
                num(cy - label.height / 2.0 + 2.0),
                num(label.width + 6.0),
                num(label.height - 4.0)
            )
        } else {
            String::new()
        };
        let fill = if plate {
            String::new()
        } else {
            under(stack, frames, l, l + label.width)
        };
        let attrs = if frame {
            format!(
                " class=\"pele-frame-label\"{} stroke=\"none\"",
                if fill.is_empty() {
                    " fill=\"var(--_m)\""
                } else {
                    &fill
                }
            )
        } else {
            fill
        };
        back + &label_svg(label, cx, cy, &attrs, icons)
    };
    let left_edge = |col: &Column, extra: usize| -> f64 {
        if col.activations.len() + extra > 0 {
            col.x - ACT_W / 2.0
        } else {
            col.x
        }
    };
    let right_edge = |col: &Column, extra: usize| -> f64 {
        let depth = col.activations.len() + extra;
        if depth > 0 {
            col.x + ACT_W / 2.0 + (depth - 1) as f64 * ACT_STEP
        } else {
            col.x
        }
    };

    // Shaded blocks and activations close from the inside out. They are written in reverse, so that
    // the inner ones end up on top.
    let mut rects_out: Vec<String> = Vec::new();
    let mut activations_out: Vec<String> = Vec::new();
    let mut frames_out = String::new();
    let mut edges_out = String::new();
    let mut labels_out = String::new();
    let mut notes_out = String::new();
    let mut actors_out = String::new();
    let mut crosses_out = String::new();
    let end_activation = |d: &mut Draw,
                          activations_out: &mut Vec<String>,
                          c: usize,
                          end: f64|
     -> f64 {
        let start = d.cols[c].activations.pop().unwrap_or(0.0);
        let bottom = jmax(end, start + ACT_MIN);
        let x = d.cols[c].x - ACT_W / 2.0 + d.cols[c].activations.len() as f64 * ACT_STEP;
        d.extend(x, x + ACT_W);
        activations_out.push(format!(
            "<rect class=\"pele-activation\" data-id=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\"/>",
            esc_text(&d.cols[c].id),
            num(x),
            num(start),
            num_str(ACT_W),
            num(bottom - start)
        ));
        bottom
    };
    // The lifeline of a destroyed participant ends in a cross a little below the message that ends it.
    let destroy = |d: &mut Draw, crosses_out: &mut String, c: usize, line_y: f64| -> f64 {
        let cx = d.cols[c].x;
        let cy = line_y + 16.0;
        d.cols[c].life_end = cy;
        crosses_out.push_str(&format!(
            "<path class=\"pele-destroy\" data-id=\"{}\" d=\"M{},{}L{},{}M{},{}L{},{}\"/>",
            esc_text(&d.cols[c].id),
            num(cx - 6.0),
            num(cy - 6.0),
            num(cx + 6.0),
            num(cy + 6.0),
            num(cx + 6.0),
            num(cy - 6.0),
            num(cx - 6.0),
            num(cy + 6.0)
        ));
        cy + 6.0
    };
    let actor_svg = |col: &Column, top: f64, footer: bool| -> String {
        let actor = actor_of(col);
        let cls = actor
            .properties
            .iter()
            .find(|(k, _)| k == "class")
            .and_then(|(_, v)| match v {
                Value::Str(s) => Some(class_names(s)),
                _ => None,
            })
            .unwrap_or_default();
        let ink_attr = if is_figure(col.kind) {
            ink_on(actor.bx.map(|b| db.boxes[b].fill.as_str()))
        } else {
            String::new()
        };
        format!(
            "<g class=\"pele-node pele-actor pele-actor-{}{}{}\" data-id=\"{}\">{}</g>",
            col.kind,
            if footer { " pele-actor-footer" } else { "" },
            cls,
            esc_text(&col.id),
            draw_actor(col.kind, col.x, top, col.size, &col.label, icons, &ink_attr)
        )
    };
    let badge = |d: &Draw, i: usize, cx: f64, cy: f64, w: f64| -> String {
        format!(
            "<g class=\"pele-sequence-number\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"var(--_l)\" stroke=\"none\"/><text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\" fill=\"var(--_bg)\" stroke=\"none\">{}</text></g>",
            num(cx - w / 2.0),
            num(cy - BADGE_H / 2.0),
            num(w),
            num_str(BADGE_H),
            num_str(BADGE_H / 2.0),
            num(cx),
            num(cy + tiny * 0.35),
            num_str(tiny),
            esc(&num_str(d.numbering.index[i]))
        )
    };

    let mut y = head_bottom + 6.0;
    for i in 0..count {
        let m = &messages[i];
        let kind = m.kind;
        let view = d.views[i].clone();
        let frame = d.frame_at[i];
        let col_from = m.from.as_deref().and_then(|f| col_of(&d.cols, f));

        if kind == Some(line::ACTIVE_START) {
            if let Some(c) = col_from {
                d.cols[c].activations.push(y);
            }
        } else if kind == Some(line::ACTIVE_END) {
            if let Some(c) = col_from {
                if !d.cols[c].activations.is_empty() {
                    y = jmax(y, end_activation(&mut d, &mut activations_out, c, y));
                }
            }
        } else if let Some(f) = frame.filter(|&f| d.frames[f].dividers[0] == i) {
            stack.push(f);
            if d.frames[f].kind == "rect" {
                d.frames[f].top = y + 8.0;
                y = d.frames[f].top;
            } else {
                d.frames[f].top = y + 12.0;
                y = d.frames[f].top + jmax(TAB_H, d.frames[f].title.height + 2.0);
            }
        } else if let Some(f) = frame {
            stack.pop();
            let bottom = y + 12.0;
            let fr = d.frames[f].clone();
            let x = num(fr.x0);
            let w = num(fr.x1 - fr.x0);
            if fr.kind == "rect" {
                let fill = if !fr.fill.is_empty() {
                    resolve_style(&[format!("fill:{}", fr.fill)]).shape
                } else {
                    String::new()
                };
                rects_out.push(format!(
                    "<rect class=\"pele-rect\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                    x,
                    num(fr.top),
                    w,
                    num(bottom - fr.top),
                    rounded(&format!(
                        "{}{}",
                        if fill.is_empty() {
                            " fill-opacity=\"0.5\""
                        } else {
                            ""
                        },
                        fill
                    ))
                ));
            } else {
                let mut inner = format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\"{}/>",
                    x,
                    num(fr.top),
                    w,
                    num(bottom - fr.top),
                    ROUNDED
                );
                for k in 0..fr.divider_y.len() {
                    let dy = fr.divider_y[k];
                    let label = &fr.divider_labels[k];
                    inner.push_str(&format!(
                        "<path d=\"M{},{}H{}\"{}/>",
                        x,
                        num(dy),
                        num(fr.x1),
                        DASH
                    ));
                    inner.push_str(&backed(
                        &stack,
                        &d.frames,
                        label,
                        fr.x0 + 8.0 + label.width / 2.0,
                        dy + 2.0 + label.height / 2.0,
                        true,
                    ));
                }
                inner.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"var(--_s)\"{}/><text class=\"pele-frame-kind\" x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"{}\" font-weight=\"var(--_hw)\" fill=\"var(--_m)\" stroke=\"none\">{}</text>",
                    x,
                    num(fr.top),
                    num(fr.tab_w),
                    num_str(TAB_H),
                    ROUNDED,
                    num(fr.x0 + fr.tab_w / 2.0),
                    num(fr.top + TAB_H / 2.0 + tiny * 0.35),
                    num_str(tiny),
                    fr.kind
                ));
                inner.push_str(&backed(
                    &stack,
                    &d.frames,
                    &fr.title,
                    fr.x0 + fr.tab_w + 8.0 + fr.title.width / 2.0,
                    fr.top + jmax(TAB_H, fr.title.height) / 2.0,
                    true,
                ));
                frames_out.push_str(&format!(
                    "<g class=\"pele-frame pele-frame-{}\">{}</g>",
                    fr.kind, inner
                ));
            }
            y = bottom;
        } else if matches!(
            kind,
            Some(line::ALT_ELSE) | Some(line::PAR_AND) | Some(line::CRITICAL_OPTION)
        ) {
            if let Some(&cur) = stack.last() {
                let k = d.frames[cur].divider_y.len();
                let lh = d.frames[cur]
                    .divider_labels
                    .get(k)
                    .map(|l| l.height)
                    .unwrap_or(0.0);
                d.frames[cur].divider_y.push(y + 12.0);
                y += 12.0 + if lh > 0.0 { lh } else { 0.0 };
            }
        } else if let Some(view) = view.as_ref().filter(|v| v.kind == V::Note) {
            let a = &d.cols[view.a];
            let h = view.label.height + 2.0 * jmax(4.0, note_pad - 4.0);
            let top = y + 10.0;
            let mut x = view.x;
            if m.placement == Some(RIGHTOF) {
                x += right_edge(a, 0) - a.x;
            } else if m.placement == Some(LEFTOF) {
                x += left_edge(a, 0) - a.x;
            }
            d.extend(x, x + view.w);
            notes_out.push_str(&format!(
                "<g class=\"pele-note\" data-id=\"i{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>{}</g>",
                esc_text(&m.id),
                num(x),
                num(top),
                num(view.w),
                num(h),
                ROUNDED,
                label_svg(
                    &view.label,
                    x + view.w / 2.0,
                    top + h / 2.0,
                    " class=\"pele-label\" stroke=\"none\" fill=\"var(--_fg)\"",
                    icons
                )
            ));
            y = top + h;
        } else if let Some(view) = view {
            let Some(st) = kind.and_then(style_of) else {
                continue;
            };
            let label = view.label.clone();
            let bw = d.badge_width(i);
            // The activation a `+` starts belongs to this message, so the arrow stops at its edge.
            let mut starting = 0usize;
            for mk in messages.iter().skip(i + 1) {
                let t = mk.kind;
                if t == Some(line::ACTIVE_START) {
                    if mk.from == m.to {
                        starting += 1;
                    }
                } else if t != Some(line::ACTIVE_END)
                    && t != Some(line::CENTRAL_CONNECTION)
                    && t != Some(line::CENTRAL_CONNECTION_REVERSE)
                {
                    break;
                }
            }
            let up = if st.bottom { 1.0 } else { -1.0 };
            let attrs = if st.dotted { DASH } else { "" };
            let cc = m.central;
            let circle_to = cc == line::CENTRAL_CONNECTION || cc == line::CENTRAL_CONNECTION_DUAL;
            let circle_from =
                cc == line::CENTRAL_CONNECTION_REVERSE || cc == line::CENTRAL_CONNECTION_DUAL;
            let mut color = String::from("var(--_l)");
            let circle = |color: &str, cx: f64, cy: f64| -> String {
                format!(
                    "<circle class=\"pele-central\" cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"none\"/>",
                    num(cx),
                    num(cy),
                    num_str(CENTRAL_R),
                    color
                )
            };
            let tail_at_end = st.at_start && !st.at_end;
            let head_trim = trim_of(st.head);
            let mut inner = String::new();
            let line_y: f64;
            let line_ink: Option<&'static str>;
            let (a, b) = (view.a, view.b);

            if view.kind == V::SelfLoop {
                let ac = d.cols[a].clone();
                let top = y + msg_gap + jmax(0.0, (label.height - SELF_H) / 2.0);
                let bottom = top + SELF_H;
                let mut x0 = if circle_from {
                    ac.x + CENTRAL_R
                } else {
                    right_edge(&ac, 0)
                };
                let mut x1 = if circle_to {
                    ac.x + CENTRAL_R
                } else {
                    right_edge(&ac, starting)
                };
                let out = jmax(x0, x1) + SELF_W + bw / 2.0;
                line_ink = ink_of(fill_under(&stack, &d.frames, ac.x, out).as_deref());
                if let Some(c) = line_ink {
                    color = c.into();
                }
                if circle_from {
                    inner.push_str(&circle(&color, ac.x, top));
                }
                if circle_to {
                    inner.push_str(&circle(&color, ac.x, bottom));
                }
                if bw > 0.0 {
                    inner.push_str(&badge(
                        &d,
                        i,
                        if tail_at_end { x1 } else { x0 },
                        if tail_at_end { bottom } else { top },
                        bw,
                    ));
                    if tail_at_end {
                        x1 += bw / 2.0;
                    } else {
                        x0 += bw / 2.0;
                    }
                }
                // The loop leaves to the right and comes back heading left, which turns a half head over.
                let heads = (if st.at_start {
                    head(st.head, x0, top, -1.0, up, &color)
                } else {
                    String::new()
                }) + &(if st.at_end {
                    head(st.head, x1, bottom, -1.0, -up, &color)
                } else {
                    String::new()
                });
                let from = num(x0 + if st.at_start { head_trim } else { 0.0 });
                let to = num(x1 + if st.at_end { head_trim } else { 0.0 });
                let path = if right_angles {
                    format!("M{},{}H{}V{}H{}", from, num(top), num(out), num(bottom), to)
                } else {
                    format!(
                        "M{},{}H{}A6,6 0 0 1 {},{}V{}A6,6 0 0 1 {},{}H{}",
                        from,
                        num(top),
                        num(out - 6.0),
                        num(out),
                        num(top + 6.0),
                        num(bottom - 6.0),
                        num(out - 6.0),
                        num(bottom),
                        to
                    )
                };
                inner = format!("<path d=\"{}\"{}/>", path, attrs) + &heads + &inner;
                d.extend(ac.x, out + 8.0 + label.width);
                if label.width > 0.0 {
                    labels_out.push_str(&format!(
                        "<g class=\"pele-edge-label\" data-id=\"i{}\">{}</g>",
                        esc_text(&m.id),
                        label_svg(
                            &label,
                            out + 8.0 + label.width / 2.0,
                            (top + bottom) / 2.0,
                            &under(&stack, &d.frames, out + 8.0, out + 8.0 + label.width),
                            icons
                        )
                    ));
                }
                // A label taller than the loop reaches below it, so the next row starts under the label.
                line_y = jmax(bottom, (top + bottom) / 2.0 + label.height / 2.0);
            } else {
                let (ac, bc) = (d.cols[a].clone(), d.cols[b].clone());
                let dir = if bc.x > ac.x { 1.0 } else { -1.0 };
                let creating = bc.create_at == i as isize;
                line_ink = ink_of(
                    fill_under(&stack, &d.frames, jmin(ac.x, bc.x), jmax(ac.x, bc.x)).as_deref(),
                );
                if let Some(c) = line_ink {
                    color = c.into();
                }
                line_y = y
                    + msg_gap
                    + jmax(
                        if label.height > 0.0 {
                            label.height + 2.0
                        } else {
                            6.0
                        },
                        if creating {
                            bc.size.h / 2.0 - msg_gap + 6.0
                        } else {
                            0.0
                        },
                    );
                let mut x0 = if dir > 0.0 {
                    right_edge(&ac, 0)
                } else {
                    left_edge(&ac, 0)
                };
                let mut x1 = if dir > 0.0 {
                    left_edge(&bc, starting)
                } else {
                    right_edge(&bc, starting)
                };
                if creating {
                    d.cols[b].top = line_y - bc.size.h / 2.0;
                    d.cols[b].life_start = d.cols[b].top + bc.size.h;
                    x1 = bc.x
                        - dir
                            * if is_figure(bc.kind) {
                                18.0
                            } else {
                                bc.size.w / 2.0 + 1.0
                            };
                }
                if circle_from {
                    inner.push_str(&circle(&color, ac.x, line_y));
                    x0 = ac.x + dir * CENTRAL_R;
                }
                if circle_to {
                    inner.push_str(&circle(&color, bc.x, line_y));
                    x1 = bc.x - dir * CENTRAL_R;
                }
                if bw > 0.0 {
                    // The number sits on the lifeline at the tail of the arrow, past a connection circle if there is one.
                    if tail_at_end {
                        let cx = if circle_to {
                            x1 - dir * (bw / 2.0 + 2.0)
                        } else {
                            x1
                        };
                        inner.push_str(&badge(&d, i, cx, line_y, bw));
                        x1 = cx - (dir * bw) / 2.0;
                    } else {
                        let cx = if circle_from {
                            x0 + dir * (bw / 2.0 + 2.0)
                        } else {
                            x0
                        };
                        inner.push_str(&badge(&d, i, cx, line_y, bw));
                        x0 = cx + (dir * bw) / 2.0;
                    }
                }
                let heads = (if st.at_start {
                    head(st.head, x0, line_y, -dir, up * dir, &color)
                } else {
                    String::new()
                }) + &(if st.at_end {
                    head(st.head, x1, line_y, dir, up * dir, &color)
                } else {
                    String::new()
                });
                let from = x0 + if st.at_start { dir * head_trim } else { 0.0 };
                let to = x1 - if st.at_end { dir * head_trim } else { 0.0 };
                inner = format!(
                    "<path d=\"M{},{}H{}\"{}/>",
                    num(from),
                    num(line_y),
                    num(to),
                    attrs
                ) + &heads
                    + &inner;
                if label.width > 0.0 {
                    let l = jmin(x0, x1);
                    let r = jmax(x0, x1);
                    let cx = if align < 0 {
                        l + msg_pad + label.width / 2.0
                    } else if align > 0 {
                        r - msg_pad - label.width / 2.0
                    } else {
                        (l + r) / 2.0
                    };
                    d.extend(cx - label.width / 2.0, cx + label.width / 2.0);
                    labels_out.push_str(&format!(
                        "<g class=\"pele-edge-label\" data-id=\"i{}\">{}</g>",
                        esc_text(&m.id),
                        backed(
                            &stack,
                            &d.frames,
                            &label,
                            cx,
                            line_y - 4.0 - label.height / 2.0,
                            false
                        )
                    ));
                }
                if creating {
                    y = line_y + bc.size.h / 2.0 - 6.0;
                }
            }
            edges_out.push_str(&format!(
                "<g class=\"pele-edge pele-message\" data-id=\"i{}\"{}>{}</g>",
                esc_text(&m.id),
                match line_ink {
                    Some(l) => format!(" stroke=\"{}\"", l),
                    None => String::new(),
                },
                inner
            ));
            y = jmax(y, line_y);
            if d.cols[a].destroy_at == i as isize {
                y = jmax(y, destroy(&mut d, &mut crosses_out, a, line_y));
            }
            if d.cols[b].destroy_at == i as isize && b != a {
                y = jmax(y, destroy(&mut d, &mut crosses_out, b, line_y));
            }
        }
    }

    y += 20.0;
    for c in 0..d.cols.len() {
        while !d.cols[c].activations.is_empty() {
            end_activation(&mut d, &mut activations_out, c, y);
        }
    }
    let mut bottom = y;
    if mirror {
        let mut foot_h: f64 = 0.0;
        for col in &d.cols {
            if col.life_end >= 0.0 {
                continue;
            }
            foot_h = jmax(foot_h, col.size.h);
            actors_out.push_str(&actor_svg(col, y, true));
        }
        bottom = y + foot_h;
    }
    let mut lifelines = String::new();
    for col in &d.cols {
        actors_out.push_str(&actor_svg(col, col.top, false));
        let end = if col.life_end >= 0.0 { col.life_end } else { y };
        if end > col.life_start {
            lifelines.push_str(&format!(
                "<path class=\"pele-lifeline\" data-id=\"{}\" d=\"M{},{}V{}\"/>",
                esc_text(&col.id),
                num(col.x),
                num(col.life_start),
                num(end)
            ));
        }
    }

    let mut boxes_out = String::new();
    for (k, run) in runs.iter().enumerate() {
        let (x0, x1) = run_edges[k];
        let bx = &db.boxes[run.bx];
        let fill = if bx.fill == "transparent" {
            String::new()
        } else {
            resolve_style(&[format!("fill:{}", bx.fill)]).shape
        };
        let ink_attr = ink_on(Some(&bx.fill));
        boxes_out.push_str(&format!(
            "<g class=\"pele-cluster pele-box\"{}><rect x=\"{}\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"var(--_a)\" stroke=\"var(--_b)\"{}/>{}</g>",
            match bx.name.as_deref().filter(|n| !n.is_empty()) {
                Some(n) => format!(" data-id=\"{}\"", esc_text(n)),
                None => String::new(),
            },
            num(x0),
            num(x1 - x0),
            num(bottom + box_pad),
            rounded(&format!(
                "{}{}",
                if fill.is_empty() {
                    " fill-opacity=\"0.5\""
                } else {
                    ""
                },
                fill
            )),
            label_svg(
                &run.title,
                (x0 + x1) / 2.0,
                box_pad / 2.0 + 2.0 + run.title.height / 2.0,
                &format!(
                    " class=\"pele-cluster-label\"{}",
                    if ink_attr.is_empty() {
                        " fill=\"var(--_m)\""
                    } else {
                        &ink_attr
                    }
                ),
                icons
            )
        ));
    }
    if !runs.is_empty() {
        bottom += box_pad;
    }

    let mut min_y = 0.0;
    let mut title_out = String::new();
    let room = title_room(options);
    // Ports `titleLabel`, with the host's icons as Pele's layoutLabel has them.
    let plain_title = layout_label(
        db.title.as_deref(),
        false,
        measurer,
        size,
        if tight { room } else { f64::INFINITY },
        style::BOLD,
        icons,
    );
    let title = if tight {
        break_label(&plain_title, measurer, jmax(120.0, room))
    } else {
        plain_title
    };
    if d.min_x > d.max_x {
        d.extend(0.0, jmax(title.width, 40.0));
    }
    if title.width > 0.0 {
        let cx = (d.min_x + d.max_x) / 2.0;
        d.extend(cx - title.width / 2.0, cx + title.width / 2.0);
        min_y = -title.height - 12.0;
        title_out = label_svg(
            &title,
            cx,
            min_y + title.height / 2.0,
            " class=\"pele-title\" font-weight=\"var(--_tw)\"",
            icons,
        );
    }

    let width = (d.max_x - d.min_x + 2.0 * pad).ceil();
    // Retry with equal shares or room for activation notes and rounding.
    if let Some(max_width) = options.max_width {
        if grown && width > max_width {
            let next_hold = if uneven {
                hold
            } else if hold > 0.0 {
                f64::INFINITY
            } else {
                width - max_width.floor()
            };
            return draw(db, config, options, palette, tight, next_hold, false);
        }
    }
    let height = (bottom - min_y + 2.0 * pad).ceil();
    let mut content = format!(
        "<g transform=\"translate({},{})\">",
        num(pad - d.min_x),
        num(pad - min_y)
    );
    content.push_str(&title_out);
    if !boxes_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-boxes\" font-size=\"{}\">{}</g>",
            num_str(small),
            boxes_out
        ));
    }
    if !rects_out.is_empty() {
        rects_out.reverse();
        content.push_str(&format!(
            "<g class=\"pele-rects\" fill=\"var(--_a)\">{}</g>",
            rects_out.concat()
        ));
    }
    if !lifelines.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-lifelines\" fill=\"none\" stroke=\"var(--_b)\">{}</g>",
            lifelines
        ));
    }
    if !activations_out.is_empty() {
        activations_out.reverse();
        content.push_str(&format!(
            "<g class=\"pele-activations\" fill=\"var(--_s)\" stroke=\"var(--_b)\">{}</g>",
            activations_out.concat()
        ));
    }
    if !frames_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-frames\" font-size=\"{}\" stroke=\"var(--_b)\">{}</g>",
            num_str(small),
            frames_out
        ));
    }
    if !edges_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>",
            edges_out
        ));
    }
    if !crosses_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-destroys\" fill=\"none\" stroke=\"var(--_l)\" stroke-width=\"1.5\" stroke-linecap=\"round\">{}</g>",
            crosses_out
        ));
    }
    if !labels_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\"{}>{}</g>",
            num_str(small),
            if monospaced {
                format!(" font-family=\"{}\"", FONT_MONO)
            } else {
                String::new()
            },
            labels_out
        ));
    }
    if !notes_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-notes\" font-size=\"{}\" fill=\"var(--_a)\" stroke=\"var(--_b)\">{}</g>",
            num_str(small),
            notes_out
        ));
    }
    if !actors_out.is_empty() {
        content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", actors_out));
    }
    content.push_str("</g>");

    let acc = Accessible {
        acc_title: db.acc_title.clone(),
        acc_descr: db.acc_descr.clone(),
    };
    Rendered {
        svg: svg_document(
            "sequence",
            width,
            height,
            size,
            options,
            &palette.style,
            &acc,
            &content,
        ),
        width,
        height,
        links: Vec::new(),
    }
}
