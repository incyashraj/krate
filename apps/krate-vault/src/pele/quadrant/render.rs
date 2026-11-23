//! Ports diagrams/quadrant/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::{QuadrantDb, QuadrantText};
use crate::json::Value;
use crate::pele::common::fit_width::cap_width;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str, parse_int};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{class_names, resolve_style, series_color, ROUNDED};
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

const GAP: f64 = 4.0;
const CAREFUL: usize = 300;

/// Ports `interface Dot`.
struct Dot {
    cx: f64,
    cy: f64,
    reach: f64,
    text: Label,
}

/// Ports `setting`.
fn setting<'a>(config: &'a Config, key: &str) -> Option<&'a Value> {
    config.get("quadrantChart").and_then(|q| q.get(key))
}

/// Ports `length`.
fn length(config: &Config, key: &str, fallback: f64, max: f64) -> f64 {
    match setting(config, key) {
        Some(Value::Num(v)) if *v >= 0.0 => jmin(*v, max),
        _ => fallback,
    }
}

/// Ports `textSize`: Mermaid's quadrantChart font size keys; without them
/// the sizes follow fontSize.
fn text_size(config: &Config, key: &str, fallback: f64) -> f64 {
    match setting(config, key) {
        Some(Value::Num(v)) if v.is_finite() => jmin(72.0, jmax(4.0, *v)),
        _ => fallback,
    }
}

/// Ports `hex`.
fn hex(color: &str) -> String {
    if color.starts_with('#') {
        color.into()
    } else {
        format!("#{}", color)
    }
}

/// Ports `renderQuadrant`.
pub fn render_quadrant(
    model: &QuadrantDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    let size = options.font_size.unwrap_or(16.0);
    let title_size = text_size(config, "titleFontSize", size);
    let quadrant_size = text_size(config, "quadrantLabelFontSize", size);
    let x_axis_size = text_size(config, "xAxisLabelFontSize", jround(size * 0.875));
    let y_axis_size = text_size(config, "yAxisLabelFontSize", jround(size * 0.875));
    let point_size = text_size(config, "pointLabelFontSize", jround(size * 0.75));
    // Sizes that differ from the one a label inherits.
    let size_attr = |own: f64, inherited: f64| -> String {
        if own == inherited {
            String::new()
        } else {
            format!(" font-size=\"{}\"", num_str(own))
        }
    };
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let icons = options.icons;
    let natural = jround(jmax(100.0, length(config, "chartWidth", 500.0, 4000.0)));
    let width = jround(cap_width(options, natural, 280.0, 0.0));
    let height =
        jround((jmax(100.0, length(config, "chartHeight", 500.0, 4000.0)) * width) / natural);
    let outer = length(
        config,
        "quadrantPadding",
        options.padding.unwrap_or(8.0),
        200.0,
    );
    let x_pad = length(config, "xAxisLabelPadding", 5.0, 200.0);
    let y_pad = length(config, "yAxisLabelPadding", 5.0, 200.0);
    let has_points = !model.points.is_empty();

    let label = |text: Option<&QuadrantText>, font_size: f64, max: f64| -> Label {
        layout_label(
            text.map(|t| t.text.as_str()),
            text.is_some_and(|t| t.markdown),
            measurer,
            font_size,
            jmax(max, 40.0),
            0,
            icons,
        )
    };

    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        title_size,
        jmax(width - 2.0 * outer, 40.0),
        style::BOLD,
        icons,
    );
    let title_pad = length(config, "titlePadding", 10.0, 200.0);
    let title_space = if title.height > 0.0 {
        title.height + title_pad
    } else {
        0.0
    };

    let filled = |t: &Option<QuadrantText>| t.as_ref().is_some_and(|t| !t.text.is_empty());
    let has_x = filled(&model.x_axis_left) || filled(&model.x_axis_right);
    let has_y = filled(&model.y_axis_bottom) || filled(&model.y_axis_top);
    // Mermaid moves the x axis labels below the chart as soon as there are points.
    let x_below = has_points || setting(config, "xAxisPosition") == Some(&Value::s("bottom"));
    let y_right = setting(config, "yAxisPosition") == Some(&Value::s("right"));

    let side = width - 2.0 * outer;
    let x_left = label(
        model.x_axis_left.as_ref(),
        x_axis_size,
        if model.x_axis_right.is_some() {
            side / 2.0 - 16.0
        } else {
            side
        },
    );
    let x_right = label(model.x_axis_right.as_ref(), x_axis_size, side / 2.0 - 16.0);
    let x_height = jmax(x_left.height, x_right.height);
    let x_space = if has_x { x_height + x_pad } else { 0.0 };
    let tall = height - 2.0 * outer - title_space - x_space;
    let y_bottom = label(
        model.y_axis_bottom.as_ref(),
        y_axis_size,
        if model.y_axis_top.is_some() {
            tall / 2.0 - 16.0
        } else {
            tall
        },
    );
    let y_top = label(model.y_axis_top.as_ref(), y_axis_size, tall / 2.0 - 16.0);
    let y_height = jmax(y_bottom.height, y_top.height);
    let y_space = if has_y { y_height + y_pad } else { 0.0 };

    let left = outer + if y_right { 0.0 } else { y_space };
    let top = outer + title_space + if x_below { 0.0 } else { x_space };
    let w = jmax(0.0, side - y_space);
    let h = jmax(0.0, tall);
    let tile_w = jmax(0.0, (w - GAP) / 2.0);
    let tile_h = jmax(0.0, (h - GAP) / 2.0);

    // Boxes a point label keeps clear of: the gaps between quadrants, quadrant labels, points,
    // and the labels placed before it.
    let mut taken: Vec<f64> = alloc::vec![
        left + tile_w,
        top,
        left + w - tile_w,
        top + h,
        left,
        top + tile_h,
        left + w,
        top + h - tile_h,
    ];
    let is_clear = |taken: &[f64], from: usize, x0: f64, y0: f64, x1: f64, y1: f64| -> bool {
        let mut k = from;
        while k < taken.len() {
            // Empty quadrant labels must not block point labels.
            if taken[k] >= taken[k + 2] || taken[k + 1] >= taken[k + 3] {
                k += 4;
                continue;
            }
            if x0 < taken[k + 2] && x1 > taken[k] && y0 < taken[k + 3] && y1 > taken[k + 1] {
                return false;
            }
            k += 4;
        }
        true
    };

    let text_top = length(config, "quadrantTextTopPadding", 5.0, 200.0);
    let mut quadrants = String::new();
    for k in 0..4 {
        let x = if k == 0 || k == 3 {
            left + w - tile_w
        } else {
            left
        };
        let y = if k < 2 { top } else { top + h - tile_h };
        let text = label(model.quadrants[k].as_ref(), quadrant_size, tile_w - 16.0);
        let cx = x + tile_w / 2.0;
        let cy = if has_points {
            y + text_top + text.height / 2.0
        } else {
            y + tile_h / 2.0
        };
        taken.extend_from_slice(&[
            cx - text.width / 2.0,
            cy - text.height / 2.0,
            cx + text.width / 2.0,
            cy + text.height / 2.0,
        ]);
        quadrants.push_str(&format!(
            "<g class=\"pele-quadrant\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>{}</g>",
            k + 1,
            num(x),
            num(y),
            num(tile_w),
            num(tile_h),
            ROUNDED,
            label_svg(
                &text,
                cx,
                cy,
                &format!(
                    " class=\"pele-label\"{} fill=\"var(--_fg)\" stroke=\"none\"",
                    size_attr(quadrant_size, size)
                ),
                None
            )
        ));
    }

    let mut axes = String::new();
    if has_x {
        let y = if x_below {
            top + h + x_pad + x_height / 2.0
        } else {
            top - x_pad - x_height / 2.0
        };
        axes.push_str(&label_svg(
            &x_left,
            if x_right.height > 0.0 {
                left + w / 4.0
            } else {
                left + x_left.width / 2.0
            },
            y,
            "",
            None,
        ));
        axes.push_str(&label_svg(&x_right, left + (w * 3.0) / 4.0, y, "", None));
    }
    if has_y {
        let x = if y_right {
            left + w + y_pad + y_height / 2.0
        } else {
            left - y_pad - y_height / 2.0
        };
        let turned = |text: &Label, y: f64| -> String {
            if text.height > 0.0 {
                format!(
                    "<g transform=\"translate({},{}) rotate(-90)\"{}>{}</g>",
                    num(x),
                    num(y),
                    size_attr(y_axis_size, x_axis_size),
                    label_svg(text, 0.0, 0.0, "", None)
                )
            } else {
                String::new()
            }
        };
        axes.push_str(&turned(
            &y_bottom,
            if y_top.height > 0.0 {
                top + (h * 3.0) / 4.0
            } else {
                top + h - y_bottom.width / 2.0
            },
        ));
        axes.push_str(&turned(&y_top, top + h / 4.0));
    }

    let radius = length(config, "pointRadius", 5.0, 200.0);
    let text_pad = length(config, "pointTextPadding", 5.0, 200.0);
    let fill = series_color(0);
    let mut dots: Vec<Dot> = Vec::new();
    let mut circles: Vec<String> = Vec::new();
    for point in &model.points {
        if !(point.x >= 0.0 && point.x <= 1.0 && point.y >= 0.0 && point.y <= 1.0) {
            continue;
        }
        let shared = model
            .classes
            .iter()
            .find(|(k, _)| *k == point.class_name)
            .map(|(_, s)| s);
        let r = jmin(
            point
                .style
                .radius
                .or(shared.and_then(|s| s.radius))
                .unwrap_or(radius),
            2000.0,
        );
        let color = point
            .style
            .color
            .clone()
            .or(shared.and_then(|s| s.color.clone()));
        let stroke_color = point
            .style
            .stroke_color
            .clone()
            .or(shared.and_then(|s| s.stroke_color.clone()));
        let stroke_width = point
            .style
            .stroke_width
            .clone()
            .or(shared.and_then(|s| s.stroke_width.clone()));
        let mut styles: Vec<String> = Vec::new();
        if let Some(c) = color.as_deref().filter(|c| !c.is_empty()) {
            styles.push(format!("fill:{}", hex(c)));
        }
        let sw = stroke_width.as_deref().filter(|s| !s.is_empty());
        let sc = stroke_color.as_deref().filter(|s| !s.is_empty());
        if let Some(sw) = sw {
            if let Some(sc) = sc {
                styles.push(format!("stroke:{}", hex(sc)));
            }
            styles.push(format!("stroke-width:{}", sw));
        }
        let cx = left + point.x * w;
        let cy = top + (1.0 - point.y) * h;
        dots.push(Dot {
            cx,
            cy,
            reach: r + match sw {
                Some(sw) => jmin(parse_int(sw, 10), 400.0) / 2.0,
                None => 0.0,
            },
            // Mermaid keeps point labels on one line; wrap only what is wider than the plot.
            text: label(Some(&point.text), point_size, w),
        });
        circles.push(format!(
            "<circle class=\"pele-point{}\" data-id=\"{}\" cx=\"{}\" cy=\"{}\" r=\"{}\"{}{}/>",
            class_names(&point.class_name),
            esc_text(&point.text.text),
            num(cx),
            num(cy),
            num(r),
            if sw.is_some() && sc.is_none() {
                format!(" stroke=\"{}\"", fill)
            } else {
                String::new()
            },
            resolve_style(&styles).shape
        ));
    }

    // A label goes below its point, or above, right or left of it, or below its quadrant's title,
    // when that is the first place that is inside the chart and free. A second round lets it
    // cross the gaps between quadrants.
    // Large charts skip the search for a free place.
    let careful = dots.len() <= CAREFUL;
    if careful {
        for d in &dots {
            taken.extend_from_slice(&[
                d.cx - d.reach,
                d.cy - d.reach,
                d.cx + d.reach,
                d.cy + d.reach,
            ]);
        }
    }
    let mut labels = String::new();
    for d in &dots {
        let (cx, cy, reach, text) = (d.cx, d.cy, d.reach, &d.text);
        let half_w = text.width / 2.0;
        let half_h = text.size * 0.6 + (text.height - text.line_height) / 2.0;
        let away = reach + text_pad;
        let inset = jmin(away, jmax(0.0, (tile_w - text.width) / 2.0));
        let heading = 8 + 4 * if cx < left + w / 2.0 {
            if cy < top + h / 2.0 {
                1
            } else {
                2
            }
        } else if cy < top + h / 2.0 {
            0
        } else {
            3
        };
        let below = jmax(cy + away, taken[heading + 3] + text_pad) + half_h;
        let spots = [
            cx,
            cy + away + half_h,
            cx,
            cy - away - half_h,
            cx + away + half_w,
            cy,
            cx - away - half_w,
            cy,
            cx,
            below,
        ];
        let mut tx = cx;
        let mut ty = spots[1];
        let mut found = false;
        let mut k = 0;
        while k < 20 {
            let spot = k % 10;
            // A label moved below a title keeps clear of the plot's edge.
            let margin = if spot == 8 { inset } else { 0.0 };
            let x = if (spot < 4 || spot == 8) && text.width < w {
                jmin(
                    jmax(spots[spot], left + half_w + margin),
                    left + w - half_w - margin,
                )
            } else {
                spots[spot]
            };
            let y = spots[spot + 1];
            if x - half_w < left
                || x + half_w > left + w
                || y - half_h < top
                || y + half_h > top + h
            {
                k += 2;
                continue;
            }
            let free = !careful
                || is_clear(
                    &taken,
                    if k < 10 { 0 } else { 8 },
                    x - half_w,
                    y - half_h,
                    x + half_w,
                    y + half_h,
                );
            if !found || free {
                tx = x;
                ty = y;
                found = true;
            }
            if free {
                break;
            }
            k += 2;
        }
        if careful {
            taken.extend_from_slice(&[tx - half_w, ty - half_h, tx + half_w, ty + half_h]);
        }
        labels.push_str(&label_svg(text, tx, ty, " class=\"pele-label\"", None));
    }

    // Mermaid puts each new point under the ones before it.
    let mut content = label_svg(
        &title,
        width / 2.0,
        title_pad / 2.0 + title.height / 2.0,
        &format!(
            " class=\"pele-title\"{} font-weight=\"var(--_tw)\"",
            size_attr(title_size, size)
        ),
        None,
    );
    content.push_str(&format!(
        "<g class=\"pele-quadrants\" fill=\"var(--_s)\" stroke=\"var(--_b)\">{}</g>",
        quadrants
    ));
    if !axes.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-axis-labels\" font-size=\"{}\" fill=\"var(--_m)\">{}</g>",
            num_str(x_axis_size),
            axes
        ));
    }
    if !dots.is_empty() {
        circles.reverse();
        content.push_str(&format!(
            "<g class=\"pele-points\" fill=\"{}\">{}</g><g class=\"pele-point-labels\" font-size=\"{}\">{}</g>",
            fill,
            circles.concat(),
            num_str(point_size),
            labels
        ));
    }

    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };
    Ok(Rendered {
        svg: svg_document(
            "quadrantChart",
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
    })
}
