//! Ports diagrams/timeline/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::TimelineDb;
use crate::json::Value;
use crate::pele::common::fit::{fit_label, plain_label};
use crate::pele::common::fit_width::turn_to_fit;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str, trim};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::edges::{marker, ARROW};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{rounded, series_color};
use crate::pele::text::label::Label;
use crate::pele::text::measurer::{style, Measure, METRICS};

const PAD_X: f64 = 12.0;
const PAD_Y: f64 = 8.0;
const MIN_TEXT: f64 = 152.0;
const MAX_TEXT: f64 = 216.0;
const EVENT_TEXT_TD: f64 = 264.0;
const GAP: f64 = 20.0;
const AXIS: f64 = 20.0;
const SPINE: f64 = 14.0;
const EVENT_GAP: f64 = 8.0;
const SECTION_GAP: f64 = 8.0;
const TAIL: f64 = 12.0;
const ON_COLOR: &str = " fill=\"var(--_bg)\"";
const LINE: &str = " fill=\"none\" stroke=\"var(--_b)\"";

/// Ports `interface Group`.
struct Group {
    section: i64,
    periods: Vec<usize>,
}

/// Ports `box`.
fn rect(x: f64, y: f64, w: f64, h: f64, attrs: &str) -> String {
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
        num(x),
        num(y),
        num(w),
        num(h),
        rounded(attrs)
    )
}

/// Ports `renderTimeline`.
pub fn render_timeline(
    model: &TimelineDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    if model.td {
        return Ok(draw(model, config, options, palette, true));
    }
    turn_to_fit(options, |down| -> Result<Rendered, PeleError> {
        Ok(draw(model, config, options, palette, down))
    })
}

/// Ports `draw`.
fn draw(
    model: &TimelineDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
    vertical: bool,
) -> Rendered {
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let icons = options.icons;
    let pad = options.padding.unwrap_or(8.0);
    let multicolor = config
        .get("timeline")
        .and_then(|t| t.get("disableMulticolor"))
        != Some(&Value::Bool(true));
    let periods = &model.periods;
    let has_sections = !model.sections.is_empty();

    // Mermaid draws nothing for periods that come before the first section; here they lead, in a neutral color.
    let mut groups: Vec<Group> = (0..model.sections.len())
        .map(|section| Group {
            section: section as i64,
            periods: Vec::new(),
        })
        .collect();
    let mut leading = Group {
        section: -1,
        periods: Vec::new(),
    };
    for (i, period) in periods.iter().enumerate() {
        let k = period.section_index;
        if k >= 0 && (k as usize) < groups.len() {
            groups[k as usize].periods.push(i);
        } else {
            leading.periods.push(i);
        }
    }
    if !leading.periods.is_empty() {
        groups.insert(0, leading);
    }

    // Columns widen, up to a point, for a word that would not fit; past that the word is cut.
    // Running down, the two columns share the width the host has.
    let room = match options.max_width {
        Some(m) if vertical && m > 0.0 => m - 2.0 * pad,
        _ => f64::INFINITY,
    };
    let taken = AXIS + SPINE * 2.0 + 4.0 * PAD_X;
    let period_text = jmin(MIN_TEXT, jmax(80.0, jround((room - taken) * 0.38)));
    let event_text = jmin(EVENT_TEXT_TD, jmax(110.0, room - taken - period_text));
    let mut text_w = period_text;
    let event_text_w = |text_w: f64| -> f64 {
        if vertical {
            event_text
        } else {
            text_w
        }
    };
    let mut period_labels: Vec<Label> = periods
        .iter()
        .map(|p| plain_label(Some(&p.text), measurer, size, text_w, 0, icons))
        .collect();
    let mut event_labels: Vec<Vec<Label>> = periods
        .iter()
        .map(|p| {
            p.events
                .iter()
                .map(|e| plain_label(Some(e), measurer, small, event_text_w(text_w), 0, icons))
                .collect()
        })
        .collect();
    let mut widest: f64 = 0.0;
    for label in &period_labels {
        widest = jmax(widest, label.width);
    }
    if !vertical {
        for labels in &event_labels {
            for label in labels {
                widest = jmax(widest, label.width);
            }
        }
    }
    let mut overflow = widest > text_w;
    if vertical {
        for labels in &event_labels {
            for label in labels {
                overflow = overflow || label.width > event_text;
            }
        }
    }
    if overflow {
        text_w = jmin(
            jmax(widest.ceil(), period_text),
            if room == f64::INFINITY {
                MAX_TEXT
            } else {
                period_text
            },
        );
        period_labels = periods
            .iter()
            .map(|p| fit_label(Some(&p.text), measurer, size, text_w, 0, icons))
            .collect();
        event_labels = periods
            .iter()
            .map(|p| {
                p.events
                    .iter()
                    .map(|e| fit_label(Some(e), measurer, small, event_text_w(text_w), 0, icons))
                    .collect()
            })
            .collect();
    }

    let line_height = jround(size * 1.5);
    let col_w = text_w + 2.0 * PAD_X;
    let event_w = event_text_w(text_w) + 2.0 * PAD_X;
    let chip_height = |label: &Label| -> f64 { jmax(label.height, line_height) + 2.0 * PAD_Y };
    let event_height =
        |label: &Label| -> f64 { jmax(label.height, jround(small * 1.5)) + 2.0 * PAD_Y };
    let series_of = |group: &Group, period: usize| -> Option<i64> {
        if group.section >= 0 {
            Some(group.section)
        } else if has_sections {
            None
        } else if multicolor {
            Some(period as i64)
        } else {
            Some(0)
        }
    };
    let color_of = |group: &Group, period: usize| -> String {
        match series_of(group, period) {
            None => String::from("var(--_m)"),
            Some(s) => series_color(s),
        }
    };
    let on_color = |series: Option<i64>| -> String {
        let fill = match series {
            None => String::new(),
            Some(s) => palette.series_ink_fill(s),
        };
        if fill.is_empty() {
            String::from(ON_COLOR)
        } else {
            fill
        }
    };

    let has_events = event_labels.iter().any(|l| !l.is_empty());
    let mut slots = 0usize;
    for group in &groups {
        slots += group.periods.len().max(1);
    }

    let pitch = col_w + GAP;
    let drawn = !periods.is_empty();
    let body_w = if vertical {
        col_w
            + if has_events {
                AXIS + SPINE * 2.0 + event_w
            } else if drawn {
                AXIS + TAIL
            } else {
                0.0
            }
    } else if slots > 0 {
        slots as f64 * pitch - GAP + if drawn { TAIL + ARROW } else { 0.0 }
    } else {
        0.0
    };
    let title = fit_label(
        model.title.as_deref(),
        measurer,
        size,
        jmax(body_w, 320.0),
        style::BOLD,
        icons,
    );
    let title_h = if title.height > 0.0 {
        title.height + 16.0
    } else {
        0.0
    };
    let width = jmax(body_w, title.width);
    let left = pad + (width - body_w) / 2.0;
    let top = pad + title_h;

    let section_label = |group: &Group, w: f64| -> Label {
        fit_label(
            Some(&model.sections[group.section as usize]),
            measurer,
            size,
            w - 2.0 * PAD_X,
            style::BOLD,
            icons,
        )
    };
    let header = |group: &Group, label: &Label, x: f64, y: f64, w: f64, h: f64| -> String {
        rect(
            x,
            y,
            w,
            h,
            &format!(" fill=\"{}\"", series_color(group.section)),
        ) + &label_svg(
            label,
            x + w / 2.0,
            y + h / 2.0,
            &(String::from(" font-weight=\"var(--_hw)\"") + &on_color(Some(group.section))),
            None,
        )
    };
    let period = |i: usize, group: &Group, color: &str, x: f64, y: f64, h: f64| -> String {
        rect(x, y, col_w, h, &format!(" fill=\"{}\"", color))
            + &label_svg(
                &period_labels[i],
                x + col_w / 2.0,
                y + h / 2.0,
                &on_color(series_of(group, i)),
                None,
            )
    };
    let event = |label: &Label, color: &str, x: f64, y: f64, h: f64| -> String {
        format!(
            "<g class=\"pele-node pele-event\">{}{}</g>",
            rect(
                x,
                y,
                event_w,
                h,
                &format!(" fill=\"{}\" fill-opacity=\"0.16\"", color)
            ),
            label_svg(
                label,
                x + event_w / 2.0,
                y + h / 2.0,
                &format!(" font-size=\"{}\"", num_str(small)),
                None
            )
        )
    };
    let dot = |x: f64, y: f64, color: &str| -> String {
        format!(
            "<circle class=\"pele-dot\" cx=\"{}\" cy=\"{}\" r=\"4\" fill=\"{}\" stroke=\"var(--_bg)\" stroke-width=\"2\"/>",
            num(x),
            num(y),
            color
        )
    };
    let open = |group: &Group| -> String {
        if group.section >= 0 {
            format!(
                "<g class=\"pele-cluster pele-section\" data-id=\"{}\">",
                esc_text(trim(&model.sections[group.section as usize]))
            )
        } else {
            String::new()
        }
    };
    let open_period = |i: usize| -> String {
        format!(
            "<g class=\"pele-node pele-period\" data-id=\"{}\">",
            esc_text(trim(&periods[i].text))
        )
    };

    let mut body = String::new();
    let height: f64;

    if vertical {
        let axis_x = left + col_w + AXIS;
        let spine_x = axis_x + SPINE;
        let events_x = spine_x + SPINE;
        let mut y = top;
        for group in &groups {
            body.push_str(&open(group));
            if group.section >= 0 {
                let label = section_label(group, body_w);
                let h = chip_height(&label);
                body.push_str(&header(group, &label, left, y, body_w, h));
                y += h + SECTION_GAP + 4.0;
            }
            for &i in &group.periods {
                let color = color_of(group, i);
                let h = chip_height(&period_labels[i]);
                // A period and its first event share one center line, so the line between them runs straight.
                let first = if !event_labels[i].is_empty() {
                    event_height(&event_labels[i][0])
                } else {
                    h
                };
                let cy = y + jmax(h, first) / 2.0;
                let mut d = format!("M{},{}H{}", num(left + col_w), num(cy), num(axis_x));
                let mut cards = String::new();
                let mut ey = cy - first / 2.0;
                let mut last = cy;
                for label in &event_labels[i] {
                    let eh = event_height(label);
                    last = ey + eh / 2.0;
                    d.push_str(&format!(
                        "M{},{}H{}",
                        num(spine_x),
                        num(last),
                        num(events_x)
                    ));
                    cards.push_str(&event(label, &color, events_x, ey, eh));
                    ey += eh + EVENT_GAP;
                }
                if !cards.is_empty() {
                    d.push_str(&format!(
                        "M{},{}H{}V{}",
                        num(axis_x),
                        num(cy),
                        num(spine_x),
                        num(last)
                    ));
                }
                body.push_str(&open_period(i));
                body.push_str(&period(i, group, &color, left, cy - h / 2.0, h));
                body.push_str(&format!("<path class=\"pele-edge\" d=\"{}\"{}/>", d, LINE));
                body.push_str(&dot(axis_x, cy, &color));
                body.push_str(&cards);
                body.push_str("</g>");
                y = jmax(cy + h / 2.0, ey - EVENT_GAP) + GAP;
            }
            if group.periods.is_empty() {
                y += GAP - SECTION_GAP - 4.0;
            }
            if group.section >= 0 {
                body.push_str("</g>");
            }
        }
        if !groups.is_empty() {
            y -= GAP;
        }
        if drawn {
            let tip = y + TAIL + ARROW;
            body = format!(
                "<path class=\"pele-axis\" d=\"M{},{}V{}\" fill=\"none\" stroke=\"var(--_l)\"/>{}{}",
                num(axis_x),
                num(top),
                num(tip - ARROW + 1.0),
                marker("arrow_point", axis_x, tip, 0.0, 1.0, "var(--_l)", "none"),
                body
            );
            y = tip;
        }
        height = y + pad;
    } else {
        let mut header_h: f64 = 0.0;
        let section_labels: Vec<Option<Label>> = groups
            .iter()
            .map(|group| {
                if group.section < 0 {
                    return None;
                }
                let label = section_label(group, group.periods.len().max(1) as f64 * pitch - GAP);
                header_h = jmax(header_h, chip_height(&label));
                Some(label)
            })
            .collect();
        let mut chip_h: f64 = 0.0;
        for label in &period_labels {
            chip_h = jmax(chip_h, chip_height(label));
        }
        let periods_y = top
            + if has_sections {
                header_h + SECTION_GAP
            } else {
                0.0
            };
        let axis_y = periods_y + chip_h + if chip_h > 0.0 { AXIS } else { 0.0 };
        let events_y = axis_y + AXIS;
        let mut bottom = if drawn { axis_y + TAIL } else { top + header_h };
        let mut slot = 0usize;
        for (g, group) in groups.iter().enumerate() {
            let span = group.periods.len().max(1);
            let gx = left + slot as f64 * pitch;
            body.push_str(&open(group));
            if let Some(label) = &section_labels[g] {
                body.push_str(&header(
                    group,
                    label,
                    gx,
                    top,
                    span as f64 * pitch - GAP,
                    header_h,
                ));
            }
            for (k, &i) in group.periods.iter().enumerate() {
                let color = color_of(group, i);
                let x = gx + k as f64 * pitch;
                let cx = x + col_w / 2.0;
                let mut d = format!(
                    "M{},{}V{}",
                    num(cx),
                    num(periods_y + chip_h),
                    num(if !event_labels[i].is_empty() {
                        events_y
                    } else {
                        axis_y
                    })
                );
                let mut cards = String::new();
                let mut ey = events_y;
                for event_label in &event_labels[i] {
                    let eh = event_height(event_label);
                    if ey > events_y {
                        d.push_str(&format!("M{},{}V{}", num(cx), num(ey - EVENT_GAP), num(ey)));
                    }
                    cards.push_str(&event(event_label, &color, x, ey, eh));
                    ey += eh + EVENT_GAP;
                }
                bottom = jmax(bottom, ey - EVENT_GAP);
                body.push_str(&open_period(i));
                body.push_str(&period(i, group, &color, x, periods_y, chip_h));
                body.push_str(&format!("<path class=\"pele-edge\" d=\"{}\"{}/>", d, LINE));
                body.push_str(&dot(cx, axis_y, &color));
                body.push_str(&cards);
                body.push_str("</g>");
            }
            if group.section >= 0 {
                body.push_str("</g>");
            }
            slot += span;
        }
        if drawn {
            let tip = left + body_w;
            body = format!(
                "<path class=\"pele-axis\" d=\"M{},{}H{}\" fill=\"none\" stroke=\"var(--_l)\"/>{}{}",
                num(left),
                num(axis_y),
                num(tip - ARROW + 1.0),
                marker("arrow_point", tip, axis_y, 1.0, 0.0, "var(--_l)", "none"),
                body
            );
        }
        height = bottom + pad;
    }

    let total_width = (width + 2.0 * pad).ceil();
    let total_height = jmax(height, pad * 2.0).ceil();
    let content = label_svg(
        &title,
        total_width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    ) + &body;
    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };
    Rendered {
        svg: svg_document(
            "timeline",
            total_width,
            total_height,
            size,
            options,
            &palette.style,
            &acc,
            &content,
        ),
        width: total_width,
        height: total_height,
        links: Vec::new(),
    }
}
