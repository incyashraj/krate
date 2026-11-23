//! Ports diagrams/sankey/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::SankeyDb;
use super::layout::{layout_sankey, Alignment, LayoutOptions, SankeyLayout};
use crate::json::Value;
use crate::pele::common::fit_width::{fit_width, title_room};
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{resolve_style, series_color};
use crate::pele::text::entities::decode_entities;
use crate::pele::text::label::layout_label;
use crate::pele::text::measurer::{style, Measure, METRICS};

const LABEL_GAP: f64 = 6.0;
const VALUE_GAP: f64 = 15.0;
const LABEL_ROOM: f64 = 1.8;
const CAREFUL: usize = 300;

/// `n.toString(36)` for a whole number.
fn radix36(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut digits = Vec::new();
    while n > 0 {
        let d = (n % 36) as u8;
        digits.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        n /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

/// Ports `contentId`: two 32-bit hashes, so different gradients
/// practically never share an id.
fn content_id(text: &str) -> String {
    let mut a: u32 = 0x811c9dc5;
    let mut b: u32 = 5381;
    for c in text.encode_utf16() {
        let c = c as u32;
        a = (a ^ c).wrapping_mul(0x01000193);
        b = b.wrapping_mul(33).wrapping_add(c);
    }
    format!("{}{}", radix36(a), radix36(b))
}

/// Ports `numberOption`.
fn number_option(conf: &Value, key: &str, fallback: f64, min: f64, max: f64) -> f64 {
    match conf.get(key) {
        Some(Value::Num(v)) if *v >= min => jmin(*v, max),
        _ => fallback,
    }
}

/// Ports `textOption`.
fn text_option(conf: &Value, key: &str) -> String {
    match conf.get(key) {
        Some(Value::Str(s)) => s.clone(),
        Some(Value::Num(v)) if v.is_finite() => num_str(*v),
        _ => String::new(),
    }
}

/// Ports `rounded`: two decimals, as Mermaid shows values; numbers too
/// large for that are left as they are.
fn rounded(value: f64) -> f64 {
    if value.abs() < 1e15 {
        jround(value * 100.0) / 100.0
    } else {
        value
    }
}

/// Ports `fillStyle`: a colour from the configuration, as an inline style
/// that overrides the series colour.
fn fill_style(color: Option<&Value>) -> String {
    match color {
        Some(Value::Str(c)) => resolve_style(&[format!("fill:{}", c)]).shape,
        _ => String::new(),
    }
}

/// Ports `renderSankey`.
pub fn render_sankey(
    model: &SankeyDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    let conf = match config.get("sankey") {
        Some(v @ Value::Obj(_)) => v.clone(),
        _ => Value::obj(),
    };
    let natural = number_option(&conf, "width", 600.0, 1.0, 10000.0);
    fit_width(options, natural, 160.0, |chart_width, _| {
        draw(model, &conf, options, palette, chart_width, natural)
    })
}

/// One node's label (`labels[i]`).
struct Placed {
    value: String,
    width: f64,
    after: bool,
    left: f64,
    y: f64,
}

/// `/^[\w-]+$/`.
fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Ports `draw`.
fn draw(
    model: &SankeyDb,
    conf: &Value,
    options: &RenderOptions,
    palette: &Palette,
    chart_width: f64,
    natural: f64,
) -> Result<Rendered, PeleError> {
    let size = options.font_size.unwrap_or(16.0);
    let least = jround(size * 0.625);
    // Labels shrink with a chart narrowed to fit, as they would if the whole picture were scaled down.
    let mut small = jmax(
        least,
        jround(size * 0.875 * jmin(1.0, chart_width / natural)),
    );
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);

    let chart_height = number_option(conf, "height", 400.0, 1.0, 10000.0);
    let node_width = jmin(
        number_option(conf, "nodeWidth", 10.0, 1.0, 200.0),
        chart_width,
    );
    let show_values = conf.get("showValues") != Some(&Value::Bool(false));
    let prefix = decode_entities(&text_option(conf, "prefix"));
    let suffix = decode_entities(&text_option(conf, "suffix"));
    let outlined = conf.get("labelStyle") == Some(&Value::s("outlined"));
    let alignment = match conf.get("nodeAlignment") {
        Some(Value::Str(s)) if s == "left" => Alignment::Left,
        Some(Value::Str(s)) if s == "right" => Alignment::Right,
        Some(Value::Str(s)) if s == "center" => Alignment::Center,
        _ => Alignment::Justify,
    };
    let link_color = match conf.get("linkColor") {
        Some(Value::Str(s)) => s.clone(),
        _ => String::from("gradient"),
    };

    let lay = |label_size: f64| -> Result<SankeyLayout, PeleError> {
        layout_sankey(
            model,
            &LayoutOptions {
                width: chart_width,
                height: chart_height,
                node_width,
                node_padding: number_option(conf, "nodePadding", 12.0, 0.0, 200.0)
                    + if show_values { VALUE_GAP } else { 0.0 },
                label_height: label_size * 1.15,
                alignment,
            },
        )
    };
    let mut layout = lay(small)?;
    // Labels also shrink with the most crowded column, so each keeps about LABEL_ROOM lines.
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut fullest = 1;
    for node in &layout.nodes {
        let count = match rows.iter_mut().find(|(l, _)| *l == node.layer) {
            Some(entry) => {
                entry.1 += 1;
                entry.1
            }
            None => {
                rows.push((node.layer, 1));
                1
            }
        };
        if count > fullest {
            fullest = count;
        }
    }
    let crowded = jmax(least, jround(chart_height / fullest as f64 / LABEL_ROOM));
    if crowded < small {
        small = crowded;
        layout = lay(small)?;
    }
    let SankeyLayout { nodes, links } = layout;
    let line_height = small * 1.5;

    let mut custom: Vec<(String, String)> = Vec::new();
    if let Some(Value::Obj(colors)) = conf.get("nodeColors") {
        for (id, color) in colors {
            let style = fill_style(Some(color));
            if !style.is_empty() {
                match custom.iter_mut().find(|(k, _)| k == id) {
                    Some(e) => e.1 = style,
                    None => custom.push((id.clone(), style)),
                }
            }
        }
    }
    let node_style = |i: usize| -> String {
        custom
            .iter()
            .find(|(k, _)| *k == model.nodes[i].id)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    // Match the whole fill to preserve semicolons in escaped entities.
    let node_color = |i: usize| -> String {
        let s = node_style(i);
        if let Some(inner) = s
            .strip_prefix(" style=\"fill:")
            .and_then(|r| r.strip_suffix(";\""))
        {
            if !inner.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                return inner.into();
            }
        }
        series_color(i as i64)
    };

    // Labels sit beside the bars, on the side that faces the middle of the chart unless that
    // side is crowded.
    let mut central = 0;
    let mut most = 0.0;
    for node in &nodes {
        if node.value > most {
            most = node.value;
            central = node.layer;
        }
    }
    let mut min_x: f64 = 0.0;
    let mut max_x: f64 = chart_width;
    let mut min_y: f64 = 0.0;
    let mut max_y: f64 = chart_height;
    let mut labels: Vec<Placed> = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let name = decode_entities(&model.nodes[i].id);
        let value = if show_values {
            format!("{}{}{}", prefix, num_str(rounded(node.value)), suffix)
        } else {
            String::new()
        };
        let width = measurer.width(
            &if !value.is_empty() {
                format!("{} {}", name, value)
            } else {
                name
            },
            small,
            0,
        );
        let after = if outlined {
            node.layer >= central
        } else {
            node.x < chart_width / 2.0
        };
        let x = if after {
            node.x + node_width + LABEL_GAP
        } else {
            node.x - LABEL_GAP
        };
        let y = node.y + node.height / 2.0;
        min_x = jmin(min_x, if after { x } else { x - width });
        max_x = jmax(max_x, if after { x + width } else { x });
        min_y = jmin(min_y, y - small * 0.75);
        max_y = jmax(max_y, y + small * 0.75);
        labels.push(Placed {
            value,
            width,
            after,
            left: if after { x } else { x - width },
            y,
        });
    }
    // A label moves at most one line; retain two bands on each side.
    let mut bands: alloc::collections::BTreeMap<i64, Vec<usize>> =
        alloc::collections::BTreeMap::new();
    if nodes.len() <= CAREFUL {
        for (k, label) in labels.iter().enumerate() {
            let band = (label.y / line_height).floor() as i64;
            for key in [band - 2, band - 1, band, band + 1, band + 2] {
                bands.entry(key).or_default().push(k);
            }
        }
    }
    let mut labels_out = String::new();
    let mut nodes_out = String::new();
    for (i, node) in nodes.iter().enumerate() {
        let id = &model.nodes[i].id;
        let name = decode_entities(id);
        let height = jmax(node.height, 1.0);
        nodes_out.push_str(&format!(
            "<rect class=\"pele-sankey-node\" data-id=\"{}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{}\"{}/>",
            esc_text(id),
            num(node.x),
            num(node.y),
            num(node_width),
            num(height),
            num(jmin(2.0, height / 2.0)),
            series_color(i as i64),
            node_style(i)
        ));

        let overlaps_label = |labels: &[Placed], left: f64, y: f64| -> bool {
            let key = (y / line_height).floor();
            let me = &labels[i];
            let near = if key.is_finite() {
                bands.get(&(key as i64))
            } else {
                None
            };
            near.is_some_and(|list| {
                list.iter().any(|&o| {
                    let other = &labels[o];
                    o != i
                        && (other.y - y).abs() < line_height
                        && left < other.left + other.width + LABEL_GAP
                        && left + me.width > other.left - LABEL_GAP
                })
            })
        };
        // Try the other side, then an adjacent line when both sides are crowded.
        if nodes.len() <= CAREFUL && overlaps_label(&labels, labels[i].left, labels[i].y) {
            'place: for dy in [0.0, -line_height, line_height] {
                let first = labels[i].after;
                for after in [first, !first] {
                    let lw = labels[i].width;
                    let left = if after {
                        node.x + node_width + LABEL_GAP
                    } else {
                        node.x - LABEL_GAP - lw
                    };
                    let y = labels[i].y + dy;
                    if overlaps_label(&labels, left, y)
                        || left < min_x
                        || left + lw > max_x
                        || y - small * 0.75 < min_y
                        || y + small * 0.75 > max_y
                    {
                        continue;
                    }
                    if nodes.iter().enumerate().any(|(j, bar)| {
                        j != i
                            && left < bar.x + node_width + LABEL_GAP
                            && left + lw > bar.x - LABEL_GAP
                            && y - small * 0.75 < bar.y + bar.height
                            && y + small * 0.75 > bar.y
                    }) {
                        continue;
                    }
                    // A label stays nearer its own bar than any other in the column, so it can't name that one.
                    let own = jmax(jmax(0.0, node.y - y), y - node.y - node.height);
                    if nodes.iter().enumerate().any(|(j, bar)| {
                        j != i
                            && bar.x == node.x
                            && jmax(jmax(0.0, bar.y - y), y - bar.y - bar.height) <= own
                    }) {
                        continue;
                    }
                    labels[i].left = left;
                    labels[i].y = y;
                    labels[i].after = after;
                    break 'place;
                }
            }
        }
        let label = &labels[i];
        let (value, after, y) = (&label.value, label.after, label.y);
        let x = if after {
            node.x + node_width + LABEL_GAP
        } else {
            node.x - LABEL_GAP
        };
        let position = format!(
            " x=\"{}\" y=\"{}\" text-anchor=\"{}\"",
            num(x),
            num(y + small * 0.35),
            if after { "start" } else { "end" }
        );
        let mut text = esc(&name);
        if !value.is_empty() {
            text.push_str(&format!(
                " <tspan class=\"pele-sankey-value\" fill=\"var(--_m)\">{}</tspan>",
                esc(value)
            ));
        }
        labels_out.push_str(&format!(
            "<g class=\"pele-label\" data-id=\"{}\">",
            esc_text(id)
        ));
        if outlined {
            labels_out.push_str(&format!(
                "<text{} fill=\"none\" stroke=\"var(--_bg)\" stroke-width=\"3\" stroke-linejoin=\"round\">{}{}</text>",
                position,
                esc(&name),
                if !value.is_empty() {
                    format!(" {}", esc(value))
                } else {
                    String::new()
                }
            ));
        }
        labels_out.push_str(&format!("<text{}>{}</text></g>", position, text));
    }

    let fixed = if link_color == "gradient" || link_color == "source" || link_color == "target" {
        String::new()
    } else {
        fill_style(Some(&Value::s(&link_color)))
    };
    // Without an idPrefix diagrams share gradient ids, and Chrome then uses a hidden or differently
    // themed copy's gradient. Other characters would end the url() and void the whole fill.
    let id_prefix = options.id_prefix.clone().unwrap_or_default();
    let gradient = link_color == "gradient" && is_id(&id_prefix);
    let mut gradients: Vec<(String, String)> = Vec::new();
    let mut links_out = String::new();
    for link in &links {
        let end = if link_color == "target" {
            link.target
        } else {
            link.source
        };
        let half = jmax(link.thickness, 1.0) / 2.0;
        let x0 = nodes[link.source].x + node_width;
        let x1 = nodes[link.target].x;
        let mut paint = format!(
            "fill=\"{}\"{}",
            series_color(end as i64),
            if !fixed.is_empty() {
                fixed.clone()
            } else {
                node_style(end)
            }
        );
        if gradient {
            // Ids come from the gradient itself, so diagrams sharing a prefix agree on them. The colour
            // after the url() is the fallback.
            let stops = format!(
                "x1=\"{}\" x2=\"{}\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/>",
                num(x0),
                num(x1),
                node_color(link.source),
                node_color(link.target)
            );
            let id = format!("{}-sankey-{}", id_prefix, content_id(&stops));
            let def = format!(
                "<linearGradient id=\"{}\" gradientUnits=\"userSpaceOnUse\" {}</linearGradient>",
                id, stops
            );
            match gradients.iter_mut().find(|(k, _)| *k == id) {
                Some(e) => e.1 = def,
                None => gradients.push((id.clone(), def)),
            }
            paint = format!("fill=\"url(#{}) {}\"", id, node_color(link.source));
        }
        let mid = num((x0 + x1) / 2.0);
        let from = esc_text(&model.nodes[link.source].id);
        let to = esc_text(&model.nodes[link.target].id);
        links_out.push_str(&format!(
            "<path class=\"pele-sankey-link\" data-id=\"{from}-&gt;{to}\" d=\"M{},{}C{mid},{} {mid},{} {},{}L{},{}C{mid},{} {mid},{} {},{}Z\" {paint}><title>{from} \u{2192} {to}: {}{}{}</title></path>",
            num(x0),
            num(link.source_y - half),
            num(link.source_y - half),
            num(link.target_y - half),
            num(x1),
            num(link.target_y - half),
            num(x1),
            num(link.target_y + half),
            num(link.target_y + half),
            num(link.source_y + half),
            num(x0),
            num(link.source_y + half),
            esc(&prefix),
            num_str(rounded(link.value)),
            esc(&suffix),
        ));
    }

    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        size,
        title_room(options),
        style::BOLD,
        options.icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(max_x - min_x, title.width);
    let ox = pad - min_x + (inner - (max_x - min_x)) / 2.0;
    let oy = pad + title_height - min_y;
    let width = (inner + 2.0 * pad).ceil();
    let height = (max_y - min_y + title_height + 2.0 * pad).ceil();

    let mut content = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    content.push_str(&format!(
        "<g transform=\"translate({},{})\">",
        num(ox),
        num(oy)
    ));
    if !gradients.is_empty() {
        content.push_str("<defs>");
        for (_, def) in &gradients {
            content.push_str(def);
        }
        content.push_str("</defs>");
    }
    content.push_str(&format!(
        "<g class=\"pele-sankey-links\" fill-opacity=\"0.4\">{}</g><g class=\"pele-sankey-nodes\">{}</g><g class=\"pele-labels\" font-size=\"{}\">{}</g></g>",
        links_out,
        nodes_out,
        num_str(small),
        labels_out
    ));
    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };
    let svg = svg_document(
        "sankey",
        width,
        height,
        size,
        options,
        &palette.style,
        &acc,
        &content,
    );
    Ok(Rendered {
        svg,
        width,
        height,
        links: Vec::new(),
    })
}
