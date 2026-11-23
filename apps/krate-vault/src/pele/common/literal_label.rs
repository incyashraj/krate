//! Ports diagrams/common/literal-label.ts: labels drawn as written.
//!
//! Math is not ported (see text/label.rs), so no span is math here and a
//! label has no `heights`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{is_space, jmax, jround, trim_end};
use crate::pele::svg::builder::{esc, label_svg, num, shared_style, span_style, text_style};
use crate::pele::text::label::{Icons, Label, Span};
use crate::pele::text::measurer::Measure;

/// Ports `literalLabel`: literal labels, including filenames and protocol
/// fields, retain every character but the spaces a line breaks at. Long
/// words can break between code points.
pub fn literal_label(
    text: &str,
    measurer: &dyn Measure,
    size: f64,
    room: f64,
    style: u32,
) -> Label {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let chars: Vec<char> = raw.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        }
        let mut from = 0usize;
        while from < chars.len() {
            let fits = |length: usize| -> bool {
                let s: String = chars[from..(from + length).min(chars.len())]
                    .iter()
                    .collect();
                measurer.width(&s, size, style) <= room
            };
            let mut hi = 1usize;
            while hi < chars.len() - from && fits(hi) {
                hi = (chars.len() - from).min(hi * 2);
            }
            let mut lo = 1usize;
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let mut take = lo;
            let wraps = from + take < chars.len();
            if wraps && !is_space(chars[from + take]) {
                for i in (1..take).rev() {
                    if is_space(chars[from + i]) {
                        take = i + 1;
                        break;
                    }
                }
            }
            let line: String = chars[from..from + take].iter().collect();
            if wraps {
                let t = trim_end(&line);
                lines.push(if t.is_empty() { line.clone() } else { t.into() });
            } else {
                lines.push(line);
            }
            from += take;
            while wraps && from < chars.len() && is_space(chars[from]) {
                from += 1;
            }
        }
    }
    if text.is_empty() {
        lines.clear();
    }
    let widths: Vec<f64> = lines
        .iter()
        .map(|line| measurer.width(line, size, style))
        .collect();
    let mut width = 0.0;
    for &w in &widths {
        width = jmax(width, w);
    }
    let line_height = jround(size * 1.5);
    let height = lines.len() as f64 * line_height;
    Label {
        lines: lines
            .into_iter()
            .zip(widths.iter())
            .map(|(text, &width)| {
                alloc::vec![Span {
                    text,
                    style,
                    icon: None,
                    width,
                }]
            })
            .collect(),
        widths,
        width,
        height,
        size,
        line_height,
        heights: None,
    }
}

/// Ports `breakLabel`: breaks a laid-out label's lines that are wider than
/// `room` between characters.
pub fn break_label(label: &Label, measurer: &dyn Measure, room: f64) -> Label {
    if label.width <= room {
        return label.clone();
    }
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    for i in 0..label.lines.len() {
        if label.widths[i] <= room {
            lines.push(label.lines[i].clone());
            widths.push(label.widths[i]);
            continue;
        }
        let mut current: Vec<Span> = Vec::new();
        let mut width = 0.0;
        let commit = |current: &mut Vec<Span>,
                      width: &mut f64,
                      lines: &mut Vec<Vec<Span>>,
                      widths: &mut Vec<f64>| {
            lines.push(core::mem::take(current));
            widths.push(*width);
            *width = 0.0;
        };
        for span in &label.lines[i] {
            if span.icon.is_some() {
                if !current.is_empty() && width + span.width > room {
                    commit(&mut current, &mut width, &mut lines, &mut widths);
                }
                current.push(span.clone());
                width += span.width;
                continue;
            }
            for ch in span.text.chars() {
                let joined = current
                    .last()
                    .is_some_and(|l| l.icon.is_none() && l.style == span.style);
                let next_width = if joined {
                    let l = current.last().unwrap();
                    let mut s = l.text.clone();
                    s.push(ch);
                    measurer.width(&s, label.size, span.style)
                } else {
                    measurer.width(ch.encode_utf8(&mut [0; 4]), label.size, span.style)
                };
                let mut added = next_width
                    - if joined {
                        current.last().unwrap().width
                    } else {
                        0.0
                    };
                let mut has_last = !current.is_empty();
                if !current.is_empty() && width + added > room {
                    commit(&mut current, &mut width, &mut lines, &mut widths);
                    has_last = false;
                    added = measurer.width(ch.encode_utf8(&mut [0; 4]), label.size, span.style);
                }
                if has_last && joined {
                    let l = current.last_mut().unwrap();
                    l.text.push(ch);
                    l.width += added;
                } else {
                    current.push(Span {
                        text: ch.into(),
                        style: span.style,
                        icon: None,
                        width: added,
                    });
                }
                width += added;
            }
        }
        if !current.is_empty() || label.lines[i].is_empty() {
            commit(&mut current, &mut width, &mut lines, &mut widths);
        }
    }
    let mut width = 0.0;
    for &w in &widths {
        width = jmax(width, w);
    }
    let height = lines.len() as f64 * label.line_height;
    Label {
        lines,
        widths,
        width,
        height,
        ..label.clone()
    }
}

/// Ports `leftLabelSvg`: writes a label with every line starting at x. A
/// line is anchored at its start and keeps its spaces, so that where it
/// begins does not depend on how its text was measured.
pub fn left_label_svg(label: &Label, x: f64, top: f64, attrs: &str, icons: Icons) -> String {
    let shared = shared_style(label);
    let open = String::from("<text") + &text_style(attrs, shared);
    let mut body = String::new();
    for i in 0..label.lines.len() {
        let spans = &label.lines[i];
        let cy = top + (i as f64 + 0.5) * label.line_height;
        if spans.iter().any(|s| s.icon.is_some()) {
            let width = label.widths[i];
            let one = Label {
                lines: alloc::vec![spans.clone()],
                widths: alloc::vec![width],
                width,
                height: label.line_height,
                ..label.clone()
            };
            body.push_str(&label_svg(&one, x + width / 2.0, cy, attrs, icons));
            continue;
        }
        let mut text = String::new();
        for span in spans {
            let own = span_style(span.style & !shared);
            if !own.is_empty() {
                text.push_str(&format!("<tspan{}>{}</tspan>", own, esc(&span.text)));
            } else {
                text.push_str(&esc(&span.text));
            }
        }
        if !text.is_empty() {
            body.push_str(&format!(
                "{} x=\"{}\" y=\"{}\" text-anchor=\"start\" xml:space=\"preserve\">{}</text>",
                open,
                num(x),
                num(cy + label.size * 0.35),
                text
            ));
        }
    }
    body
}
