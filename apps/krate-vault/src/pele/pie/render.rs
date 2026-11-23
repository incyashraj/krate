//! Ports diagrams/pie/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::f64::consts::PI;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::model::PieModel;
use crate::json::Value;
use crate::pele::common::fit_width::{fit_width, title_room};
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jround, num_str, to_fixed};
use crate::pele::preprocess::{to_js_string, Config};
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::series_color;
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

const RADIUS: f64 = 120.0;
const SWATCH: f64 = 12.0;
const ROW: f64 = 26.0;
const GAP: f64 = 32.0;
const OUTSIDE: f64 = 28.0;

/// Ports `option`.
fn option<'a>(config: &'a Config, key: &str) -> Option<&'a Value> {
    config.get("pie").and_then(|p| p.get(key))
}

/// Ports `point`.
fn point(angle: f64, radius: f64) -> String {
    format!(
        "{},{}",
        num(angle.sin() * radius),
        num(-angle.cos() * radius)
    )
}

/// Ports `slice`.
fn slice(start: f64, end: f64, outer: f64, inner: f64) -> String {
    if end - start >= PI * 2.0 - 1e-9 {
        let ring = |r: f64, sweep: u8| -> String {
            format!(
                "M0,{}A{},{} 0 1 {} 0,{}A{},{} 0 1 {} 0,{}Z",
                num(-r),
                num(r),
                num(r),
                sweep,
                num(r),
                num(r),
                num(r),
                sweep,
                num(-r)
            )
        };
        let mut out = ring(outer, 1);
        if inner > 0.0 {
            out.push_str(&ring(inner, 0));
        }
        return out;
    }
    let large = if end - start > PI { 1 } else { 0 };
    if inner <= 0.0 {
        return format!(
            "M0,0L{}A{},{} 0 {} 1 {}Z",
            point(start, outer),
            num(outer),
            num(outer),
            large,
            point(end, outer)
        );
    }
    format!(
        "M{}A{},{} 0 {} 1 {}L{}A{},{} 0 {} 0 {}Z",
        point(start, outer),
        num(outer),
        num(outer),
        large,
        point(end, outer),
        point(end, inner),
        num(inner),
        num(inner),
        large,
        point(start, inner)
    )
}

/// Ports `renderPie`.
pub fn render_pie(
    model: &PieModel,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    fit_width(options, RADIUS * 2.0, 140.0, |diameter, tight| {
        Ok(draw(model, config, options, palette, diameter / 2.0, tight))
    })
}

/// A number option (`typeof v === 'number'`).
fn number_of(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Num(n)) => Some(*n),
        _ => None,
    }
}

/// Ports `draw`: `tight` is set when the chart does not fit the width it
/// has with its legend beside it.
fn draw(
    model: &PieModel,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
    radius: f64,
    tight: bool,
) -> Rendered {
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.875);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;

    let text_position = match number_of(option(config, "textPosition")) {
        Some(p) if (0.0..=1.0).contains(&p) => p,
        _ => 0.75,
    };
    let inner = match number_of(option(config, "donutHole")) {
        Some(h) if h > 0.0 && h <= 0.9 => h * radius,
        _ => 0.0,
    };
    let placed = match option(config, "legendPosition") {
        None | Some(Value::Null) => String::from("right"),
        Some(v) => to_js_string(v),
    };
    let legend_position: &str = if tight && (placed == "left" || placed == "right") {
        "bottom"
    } else {
        &placed
    };

    let entries = &model.sections;
    let mut sum = 0.0;
    for (_, value) in entries {
        sum += value;
    }

    let legend: Vec<Label> = entries
        .iter()
        .map(|(label, value)| {
            let text = if model.show_data {
                format!("{} [{}]", label, num_str(*value))
            } else {
                label.clone()
            };
            layout_label(Some(&text), false, measurer, small, 4000.0, 0, icons)
        })
        .collect();
    let mut legend_width: f64 = 0.0;
    for label in &legend {
        legend_width = jmax(legend_width, label.width);
    }
    legend_width += if !legend.is_empty() {
        SWATCH + 8.0
    } else {
        0.0
    };
    let legend_height = legend.len() as f64 * ROW;

    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        size,
        title_room(options),
        style::BOLD,
        icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 16.0
    } else {
        0.0
    };

    // A slice too thin for its percentage gets the label outside the rim, which needs a margin.
    let shares: Vec<f64> = entries
        .iter()
        .map(|(_, value)| {
            let share = if sum > 0.0 { value / sum } else { 0.0 };
            if share * 100.0 >= 1.0 && to_fixed(share * 100.0, 0) != "0" {
                share
            } else {
                0.0
            }
        })
        .collect();
    let narrow: Vec<bool> = shares
        .iter()
        .map(|&share| {
            if share == 0.0 || share >= 0.5 {
                return false;
            }
            let chord = 2.0 * radius * text_position * (share * PI).sin();
            chord < measurer.width(&(to_fixed(share * 100.0, 0) + "%"), small, 0) + 6.0
        })
        .collect();
    let margin = if narrow.contains(&true) { OUTSIDE } else { 0.0 };

    let side = if matches!(legend_position, "left" | "right" | "center") {
        legend_position
    } else {
        ""
    };
    let stacked = legend_position == "top" || legend_position == "bottom";
    let diameter = (radius + margin) * 2.0;
    let mut width = diameter;
    let mut height = diameter;
    if side == "left" || side == "right" {
        width += if legend_width > 0.0 {
            GAP + legend_width
        } else {
            0.0
        };
        height = jmax(height, legend_height);
    } else if stacked {
        width = jmax(width, legend_width);
        height += if legend_height > 0.0 {
            20.0 + legend_height
        } else {
            0.0
        };
    }
    width = jmax(width, title.width);

    let cx = pad
        + if side == "left" {
            legend_width + GAP + diameter / 2.0
        } else if side == "right" {
            diameter / 2.0
        } else {
            width / 2.0
        };
    let body_top = pad + title_height;
    let cy = body_top
        + if legend_position == "top" {
            legend_height + 20.0 + diameter / 2.0
        } else if stacked {
            diameter / 2.0
        } else {
            height / 2.0
        };

    let mut slices = String::new();
    let mut labels = String::new();
    let mut outside = String::new();
    let mut angle = 0.0;
    let rim = radius + OUTSIDE / 2.0;
    let mut placed_outside: Vec<(f64, f64, f64)> = Vec::new();
    let clear = |placed: &[(f64, f64, f64)], x: f64, y: f64, width: f64| -> bool {
        placed.iter().all(|&(ox, oy, ow)| {
            (x - ox).abs() >= (width + ow) / 2.0 + 2.0 || (y - oy).abs() >= small + 2.0
        })
    };
    for (index, (label, _)) in entries.iter().enumerate() {
        let share = shares[index];
        if share == 0.0 {
            continue;
        }
        let end = angle + share * PI * 2.0;
        slices.push_str(&format!(
            "<path class=\"pele-slice\" data-id=\"{}\" d=\"{}\" fill=\"{}\"/>",
            esc_text(label),
            slice(angle, end, radius, inner),
            series_color(index as i64)
        ));
        let mid = (angle + end) / 2.0;
        let text = to_fixed(share * 100.0, 0) + "%";
        if narrow[index] {
            let width = measurer.width(&text, small, 0);
            let mut at = mid;
            let mut step = 0;
            while step < 360 && !clear(&placed_outside, at.sin() * rim, -at.cos() * rim, width) {
                at += 2.0 / rim;
                step += 1;
            }
            let x = at.sin() * rim;
            let y = -at.cos() * rim;
            placed_outside.push((x, y, width));
            outside.push_str(&format!(
                "<text x=\"{}\" y=\"{}\">{}</text>",
                num(x),
                num(y + small * 0.35),
                text
            ));
        } else {
            let r = radius * text_position;
            labels.push_str(&format!(
                "<text x=\"{}\" y=\"{}\"{}>{}</text>",
                num(mid.sin() * r),
                num(-mid.cos() * r + small * 0.35),
                palette.series_ink_fill(index as i64),
                text
            ));
        }
        angle = end;
    }
    if slices.is_empty() {
        slices = format!(
            "<circle class=\"pele-slice\" r=\"{}\" fill=\"none\" stroke=\"var(--_b)\"/>",
            num_str(radius)
        );
    }

    let mut legend_out = String::new();
    let (lx, ly);
    if side == "right" {
        lx = cx + diameter / 2.0 + GAP;
        ly = body_top + (height - legend_height) / 2.0;
    } else if side == "left" {
        lx = pad;
        ly = body_top + (height - legend_height) / 2.0;
    } else if side == "center" {
        lx = cx - legend_width / 2.0;
        ly = cy - legend_height / 2.0;
    } else {
        lx = pad + (width - legend_width) / 2.0;
        ly = if legend_position == "top" {
            body_top
        } else {
            body_top + diameter + 20.0
        };
    }
    for (index, label) in legend.iter().enumerate() {
        let y = ly + index as f64 * ROW + ROW / 2.0;
        legend_out.push_str(&format!(
            "<g class=\"pele-legend-item\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"{}\"/>{}</g>",
            esc_text(&entries[index].0),
            num(lx),
            num(y - SWATCH / 2.0),
            num_str(SWATCH),
            num_str(SWATCH),
            series_color(index as i64),
            label_svg(label, lx + SWATCH + 8.0 + label.width / 2.0, y, "", None)
        ));
    }

    let total_width = (width + 2.0 * pad).ceil();
    let total_height = (height + title_height + 2.0 * pad).ceil();
    let mut content = label_svg(
        &title,
        total_width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    content.push_str(&format!(
        "<g class=\"pele-slices\" transform=\"translate({},{})\" stroke=\"var(--_bg)\" stroke-width=\"1\" stroke-linejoin=\"round\">{}</g>",
        num(cx),
        num(cy),
        slices
    ));
    if !labels.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-slice-labels\" transform=\"translate({},{})\" font-size=\"{}\" text-anchor=\"middle\" fill=\"var(--_bg)\">{}</g>",
            num(cx),
            num(cy),
            num_str(small),
            labels
        ));
    }
    if !outside.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-slice-labels\" transform=\"translate({},{})\" font-size=\"{}\" text-anchor=\"middle\">{}</g>",
            num(cx),
            num(cy),
            num_str(small),
            outside
        ));
    }
    if !legend_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-legend\" font-size=\"{}\">{}</g>",
            num_str(small),
            legend_out
        ));
    }

    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };
    Rendered {
        svg: svg_document(
            "pie",
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
