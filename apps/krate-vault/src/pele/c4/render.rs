//! Ports diagrams/c4/render.ts: lays out and draws a parsed C4 diagram.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::C4Db;
use super::layout::{layout_c4, HEAD_LEFT, HEAD_TOP};
use super::route::{crosses, cyl_ry, queue_rx, route, Bx, Lanes, Route, HEAD, LABEL_PAD, LANE};
use crate::json::Value;
use crate::pele::common::fit_width::{tighten, title_room};
use crate::pele::common::literal_label::left_label_svg;
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{self, jmax, jmin, jround};
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::edges::{
    casings_svg, edge_casing, marker, marker_trim, over_fills, EdgePath, Fill, ARROW,
};
use crate::pele::svg::ink::{ink_fill, paints};
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::shapes::draw_shape;
use crate::pele::svg::theme::{resolve_style, rounded, ResolvedStyle};
use crate::pele::text::label::layout_label;
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::{linked, sanitize_url};

const PAIR_SHIFT: f64 = 7.0;
const NARROW_GAP: f64 = 36.0;
const WIDEN: f64 = 24.0;
const MAX_OFFSET: f64 = 2000.0;
/// A label may cover this much of a shape's edge, where there is no text.
const EDGE: f64 = 8.0;
/// Places along a relation to try for its label, nearest the middle first.
const SLIDE: [f64; 7] = [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8];
/// Above this many box comparisons, labels stay at the middle and lines
/// are not steered around shapes.
const CARE_BUDGET: f64 = 2000000.0;

/// Ports `userStyle`.
fn user_style(fill: Option<&str>, stroke: Option<&str>, color: Option<&str>) -> ResolvedStyle {
    let mut declarations: Vec<String> = Vec::new();
    if let Some(f) = fill.filter(|f| !f.is_empty()) {
        declarations.push(format!("fill:{}", f));
    }
    if let Some(s) = stroke.filter(|s| !s.is_empty()) {
        declarations.push(format!("stroke:{}", s));
    }
    if let Some(c) = color.filter(|c| !c.is_empty()) {
        declarations.push(format!("color:{}", c));
    }
    resolve_style(&declarations)
}

/// `url.replace(/ﬂ°°?/g, '#').replace(/¶ß/g, ';')`.
fn unhide(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(k) = rest.find("\u{fb02}\u{b0}") {
        out.push_str(&rest[..k]);
        out.push('#');
        rest = &rest[k + "\u{fb02}\u{b0}".len()..];
        if let Some(r) = rest.strip_prefix('\u{b0}') {
            rest = r;
        }
    }
    out.push_str(rest);
    out.replace("\u{b6}\u{df}", ";")
}

/// Ports `link`.
fn link(
    body: &str,
    url: Option<&str>,
    id: &str,
    links: &mut Vec<LinkInfo>,
    options: &RenderOptions,
) -> String {
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        return body.into();
    };
    // Entity encoding has hidden any `#name;` in the address. Put it back, so that the check sees what a browser would.
    let shown = if url.contains("\u{b6}\u{df}") {
        unhide(url)
    } else {
        String::from(url)
    };
    linked(body, &sanitize_url(&shown), id, links, options, "")
}

/// Ports `renderC4`.
pub fn render_c4(
    db: &C4Db,
    config: &Value,
    options: &RenderOptions,
    palette_style: &str,
) -> Result<Rendered, PeleError> {
    tighten(options, |tight| {
        if !tight {
            return Ok(draw(db, config, options, palette_style, false, NARROW_GAP)?.0);
        }
        let (mut best, mut squeezed) = draw(db, config, options, palette_style, true, NARROW_GAP)?;
        let mut gap = NARROW_GAP + WIDEN;
        while squeezed > 0 && gap <= NARROW_GAP + 3.0 * WIDEN {
            let (wider, left) = draw(db, config, options, palette_style, true, gap)?;
            if left < squeezed {
                best = wider;
                squeezed = left;
            }
            gap += WIDEN;
        }
        Ok(best)
    })
}

/// A route as the path edge casings take.
fn edge_path(r: &Route) -> EdgePath {
    EdgePath {
        d: r.d.clone(),
        tokens: None,
        sx: r.sx,
        sy: r.sy,
        sdx: r.sdx,
        sdy: r.sdy,
        ex: r.ex,
        ey: r.ey,
        edx: r.edx,
        edy: r.edy,
    }
}

/// Ports `draw`: the drawing, and how many routes found no lane of their own.
fn draw(
    db: &C4Db,
    _config: &Value,
    options: &RenderOptions,
    palette_style: &str,
    tight: bool,
    gap: f64,
) -> Result<(Rendered, usize), PeleError> {
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.8125);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let icons = options.icons;
    let mut links: Vec<LinkInfo> = Vec::new();
    let room = if tight {
        Some(jmax(80.0, options.max_width.unwrap_or(320.0) - 2.0 * pad))
    } else {
        None
    };
    let mut layout = layout_c4(db, measurer, size, small, room, gap, icons);
    let root = layout.order[0];

    let mut min_x = 0.0;
    let mut min_y = 0.0;
    let mut max_x = layout.groups[root].bx.w;
    let mut max_y = layout.groups[root].bx.h;
    let mut grow = |x0: f64, y0: f64, x1: f64, y1: f64| {
        min_x = jmin(min_x, x0);
        min_y = jmin(min_y, y0);
        max_x = jmax(max_x, x1);
        max_y = jmax(max_y, y1);
    };

    let mut ends: Vec<Bx> = Vec::new();
    let mut pairs: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for rel in &db.rels {
        let from = if rel.from_attr.is_some() {
            None
        } else {
            db.get_c4_shape(&rel.from)
        };
        let to = if rel.to_attr.is_some() {
            None
        } else {
            db.get_c4_shape(&rel.to)
        };
        let a = from.and_then(|f| layout.boxes.get(&f)).copied();
        let b = to.and_then(|t| layout.boxes.get(&t)).copied();
        let (Some(a), Some(b)) = (a, b) else {
            return Err(PeleError::of(
                format!(
                    "C4 rel \"{}\" -> \"{}\" references an unknown shape or boundary",
                    rel.from, rel.to
                ),
                PeleErrorCode::Semantic,
                "c4",
            ));
        };
        ends.push(a);
        ends.push(b);
        pairs.entry(a.id).or_default().insert(b.id);
    }

    // What a relation's label should not cover: shapes, arrowheads, boundary headings, and the labels placed before it.
    let mut nodes: Vec<Bx> = Vec::new();
    let mut taken: Vec<f64> = Vec::new();
    for &g in &layout.order {
        for node in &layout.groups[g].nodes {
            let b = node.bx;
            nodes.push(b);
            taken.extend_from_slice(&[b.x + EDGE, b.y + EDGE, b.x + b.w - EDGE, b.y + b.h - EDGE]);
        }
    }
    let rels_n = db.rels.len() as f64;
    let careful =
        rels_n * (nodes.len() as f64 + layout.order.len() as f64 + 3.0 * rels_n) <= CARE_BUDGET;
    let covered = |taken: &[f64], x0: f64, y0: f64, x1: f64, y1: f64| -> f64 {
        let mut area = 0.0;
        let mut i = 0;
        while i < taken.len() {
            let w = jmin(x1, taken[i + 2]) - jmax(x0, taken[i]);
            if w > 0.0 {
                let h = jmin(y1, taken[i + 3]) - jmax(y0, taken[i + 1]);
                if h > 0.0 {
                    area += w * h;
                }
            }
            i += 4;
        }
        area
    };
    let offset = |value: Option<f64>| -> f64 {
        match value {
            Some(v) if v != 0.0 && !v.is_nan() => jmax(-MAX_OFFSET, jmin(MAX_OFFSET, v)),
            _ => 0.0,
        }
    };

    // Every line is drawn before any label is placed, so that labels can keep off the arrowheads.
    let mut casings = String::new();
    let mut edges_out = String::new();
    let trim = marker_trim("arrow_point");
    let mut fills: Vec<Fill> = Vec::new();
    for &g in &layout.order {
        if g == root {
            continue;
        }
        let group = &layout.groups[g];
        let shape = user_style(
            db.boundaries[group.boundary].bg_color.as_deref(),
            None,
            None,
        )
        .shape;
        if paints(&shape) {
            let b = group.bx;
            fills.push(Fill {
                bx: [b.x, b.y, b.x + b.w, b.y + b.h],
                shape,
            });
        }
    }
    let mut lanes = Lanes::new();
    for &g in &layout.order {
        if g != root {
            lanes.frame(&layout.groups[g].bx);
        }
    }
    let mut routes: Vec<Route> = Vec::new();
    for (index, rel) in db.rels.iter().enumerate() {
        let a = ends[index * 2];
        let b = ends[index * 2 + 1];
        let st = user_style(None, rel.line_color.as_deref(), None);
        let start_arrow = rel.kind == "birel" || rel.kind == "rel_b";
        let end_arrow = rel.kind != "rel_b";
        let reverse = a.id != b.id && pairs.get(&b.id).is_some_and(|s| s.contains(&a.id));
        let r = route(
            &a,
            &b,
            if reverse { PAIR_SHIFT } else { 0.0 },
            if careful { Some(&nodes) } else { None },
            &mut lanes,
            if start_arrow { trim } else { 0.0 },
            if end_arrow { trim } else { 0.0 },
        );
        grow(
            jmin(r.x0, r.x1),
            jmin(r.y0, r.y1),
            jmax(r.x0, r.x1),
            jmax(r.y0, r.y1),
        );
        if start_arrow && careful {
            taken.extend_from_slice(&[r.sx - ARROW, r.sy - ARROW, r.sx + ARROW, r.sy + ARROW]);
        }
        if end_arrow && careful {
            taken.extend_from_slice(&[r.ex - ARROW, r.ey - ARROW, r.ex + ARROW, r.ey + ARROW]);
        }
        let over = over_fills(&r.d, &[r.sx, r.sy, r.ex, r.ey], &fills, &st.line, None);
        let over_s = over.as_deref().unwrap_or("");
        let color = if !over_s.is_empty() {
            String::from(over_s)
        } else {
            match &st.stroke {
                Some(s) if !s.is_empty() => esc(s),
                _ => String::from("var(--_l)"),
            }
        };
        if over.as_deref() == Some("") {
            casings.push_str(&edge_casing(
                &edge_path(&r),
                if start_arrow { "arrow_point" } else { "none" },
                if end_arrow { "arrow_point" } else { "none" },
                &st.line,
                false,
            ));
        }
        edges_out.push_str(&format!(
            "<g class=\"pele-edge\" data-id=\"{}\"{}{}><path d=\"{}\"/>{}{}</g>",
            esc_text(&format!("{}-{}", rel.from, rel.to)),
            if over_s.is_empty() {
                String::new()
            } else {
                format!(" stroke=\"{}\"", over_s)
            },
            st.line,
            r.d,
            if start_arrow {
                marker("arrow_point", r.sx, r.sy, r.sdx, r.sdy, &color, "none")
            } else {
                String::new()
            },
            if end_arrow {
                marker("arrow_point", r.ex, r.ey, r.edx, r.edy, &color, "none")
            } else {
                String::new()
            }
        ));
        routes.push(r);
    }

    // A boundary's heading goes at the left, the middle or the right of its top edge: the first of them no line crosses.
    let order = layout.order.clone();
    for &g in &order {
        let group = &mut layout.groups[g];
        if !careful || group.head_w == 0.0 {
            continue;
        }
        let top = group.bx.y + HEAD_TOP;
        let bottom = group.bx.y + group.head - LANE;
        let mut fewest = usize::MAX;
        for x in [
            HEAD_LEFT,
            (group.bx.w - group.head_w) / 2.0,
            group.bx.w - HEAD_LEFT - group.head_w,
        ] {
            let mut count = 0;
            for r in &routes {
                if crosses(
                    &r.points,
                    group.bx.x + x - 4.0,
                    top,
                    group.bx.x + x + group.head_w + 4.0,
                    bottom,
                ) {
                    count += 1;
                }
            }
            if count < fewest {
                fewest = count;
                group.head_x = x;
            }
            if count == 0 {
                break;
            }
        }
        taken.extend_from_slice(&[
            group.bx.x + group.head_x - 4.0,
            top,
            group.bx.x + group.head_x + group.head_w + 4.0,
            bottom,
        ]);
    }

    let mut labels_out = String::new();
    for (index, rel) in db.rels.iter().enumerate() {
        let r = &routes[index];
        let id = format!("{}-{}", rel.from, rel.to);
        let st = user_style(None, None, rel.text_color.as_deref());
        let text = format!(
            "{}{}",
            if db.c4_type.as_deref() == Some("C4Dynamic") {
                format!("{}: ", index + 1)
            } else {
                String::new()
            },
            rel.label
        );
        let label = layout_label(Some(&text), false, measurer, small, r.room, 0, icons);
        let techn_text = rel
            .techn
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| format!("[{}]", t));
        let techn = layout_label(
            techn_text.as_deref(),
            false,
            measurer,
            small,
            r.room,
            style::ITALIC,
            icons,
        );
        let lw = jmax(label.width, techn.width) + LABEL_PAD;
        let lh = label.height + techn.height;
        if lh == 0.0 {
            continue;
        }

        // The label sits on the line, or just beside it when another relation runs the opposite way.
        let beside = r.nx.abs() * (lw / 2.0) + r.ny.abs() * (lh / 2.0) + 2.0;
        let aside = if r.lean != 0.0 && !r.lean.is_nan() {
            jmax(0.0, jmin(r.lean, beside - 14.0))
        } else {
            beside
        };
        // Manual offsets are tuned to the original spacing. Reposition compact labels beside
        // their routed arrows so an offset cannot leave them floating outside a boundary.
        let mut dx = r.nx * aside + if tight { 0.0 } else { offset(rel.offset_x) };
        let mut dy = r.ny * aside + if tight { 0.0 } else { offset(rel.offset_y) };
        let mut at = 0.5;
        if careful {
            // Slide along the line to the spot that covers least.
            let mut least = f64::INFINITY;
            let moved = dx != r.nx * aside || dy != r.ny * aside;
            let passes = if moved { 2 } else { 1 };
            let mut pass = 0;
            while pass < passes && least > 0.0 {
                let px = if pass > 0 { r.nx * aside } else { dx };
                let py = if pass > 0 { r.ny * aside } else { dy };
                for t in SLIDE {
                    let cx = r.x0 + (r.x1 - r.x0) * t + px;
                    let cy = r.y0 + (r.y1 - r.y0) * t + py;
                    let area = covered(
                        &taken,
                        cx - lw / 2.0,
                        cy - lh / 2.0,
                        cx + lw / 2.0,
                        cy + lh / 2.0,
                    );
                    if area < least {
                        least = area;
                        at = t;
                        dx = px;
                        dy = py;
                    }
                    if area == 0.0 {
                        break;
                    }
                }
                pass += 1;
            }
        }
        let cx = r.x0 + (r.x1 - r.x0) * at + dx;
        let top = r.y0 + (r.y1 - r.y0) * at + dy - lh / 2.0;
        if careful {
            taken.extend_from_slice(&[cx - lw / 2.0, top, cx + lw / 2.0, top + lh]);
        }
        grow(cx - lw / 2.0, top, cx + lw / 2.0, top + lh);
        let body = label_svg(&label, cx, top + label.height / 2.0, &st.text, None)
            + &label_svg(
                &techn,
                cx,
                top + label.height + techn.height / 2.0,
                &format!(" class=\"pele-c4-techn\" fill=\"var(--_m)\"{}", st.text),
                None,
            );
        labels_out.push_str(&format!(
            "<g class=\"pele-edge-label\" data-id=\"{}\">{}<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"var(--_bg)\"/>{}</g>",
            esc_text(&id),
            match rel.descr.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => format!("<title>{}</title>", esc_text(d)),
                None => String::new(),
            },
            num(cx - lw / 2.0),
            num(top),
            num(lw),
            num(lh),
            link(&body, rel.link.as_deref(), &id, &mut links, options)
        ));
    }

    let mut clusters = String::new();
    let mut nodes_out = String::new();
    for &g in &layout.order {
        let group = &layout.groups[g];
        if g != root {
            let boundary = &db.boundaries[group.boundary];
            let st = user_style(
                boundary.bg_color.as_deref(),
                boundary.border_color.as_deref(),
                boundary.font_color.as_deref(),
            );
            let x = group.bx.x + group.head_x;
            let mut y = group.bx.y + HEAD_TOP;
            let color = ink_fill(&st.shape, &st.text, None) + &st.text;
            let muted = ink_fill(&st.shape, &st.text, Some("var(--_m)")) + &st.text;
            let mut text = left_label_svg(
                &group.labels[0],
                x,
                y,
                &format!(
                    " class=\"pele-cluster-label\" font-weight=\"var(--_hw)\"{}",
                    color
                ),
                None,
            );
            y += group.labels[0].height;
            text.push_str(&left_label_svg(
                &group.labels[1],
                x,
                y,
                &format!(
                    " class=\"pele-c4-type\" font-size=\"{}\"{}",
                    js::num_str(small),
                    muted
                ),
                None,
            ));
            y += group.labels[1].height;
            text.push_str(&left_label_svg(
                &group.labels[2],
                x,
                y,
                &format!(
                    " class=\"pele-c4-descr\" font-size=\"{}\"{}",
                    js::num_str(small),
                    muted
                ),
                None,
            ));
            clusters.push_str(&format!(
                "<g class=\"pele-cluster pele-c4-{}\" data-id=\"{}\"><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"none\" stroke=\"var(--_b)\"{}{}/>{}</g>",
                if boundary.node_type.is_some() {
                    "node"
                } else {
                    "boundary"
                },
                esc_text(&boundary.alias),
                num(group.bx.x),
                num(group.bx.y),
                num(group.bx.w),
                num(group.bx.h),
                if boundary.node_type.is_some() {
                    ""
                } else {
                    " stroke-dasharray=\"6 4\""
                },
                rounded(&st.shape),
                link(&text, boundary.link.as_deref(), &boundary.alias, &mut links, options)
            ));
        }
        for node in &group.nodes {
            let shape = &db.shapes[node.shape];
            let external = shape.kind.starts_with("external");
            let st = user_style(
                shape.bg_color.as_deref(),
                shape.border_color.as_deref(),
                shape.font_color.as_deref(),
            );
            let stroke = String::from(" stroke=\"var(--_b)\"")
                + if external {
                    " stroke-dasharray=\"4 3\""
                } else {
                    ""
                };
            let (w, h, form, labels) = (node.bx.w, node.bx.h, node.bx.form, &node.labels);
            let dx = if form == "h-cyl" {
                -queue_rx(h) / 2.0
            } else {
                0.0
            };
            let dy = if form == "person" {
                HEAD / 2.0
            } else if form == "cyl" {
                cyl_ry(w) / 2.0
            } else {
                0.0
            };
            let color = ink_fill(&st.shape, &st.text, None) + &st.text;
            let muted = ink_fill(&st.shape, &st.text, Some("var(--_m)")) + &st.text;
            let mut y = dy - node.text_h / 2.0;
            let mut text = label_svg(
                &labels[0],
                dx,
                y + labels[0].height / 2.0,
                &format!(
                    " class=\"pele-c4-type\" font-size=\"{}\"{}",
                    js::num_str(small - 1.0),
                    muted
                ),
                None,
            );
            y += labels[0].height;
            text.push_str(&label_svg(
                &labels[1],
                dx,
                y + labels[1].height / 2.0,
                &format!(" class=\"pele-label\" font-weight=\"var(--_hw)\"{}", color),
                None,
            ));
            y += labels[1].height;
            text.push_str(&label_svg(
                &labels[2],
                dx,
                y + labels[2].height / 2.0,
                &format!(
                    " class=\"pele-c4-techn\" font-size=\"{}\"{}",
                    js::num_str(small),
                    muted
                ),
                None,
            ));
            y += labels[2].height + 4.0;
            text.push_str(&label_svg(
                &labels[3],
                dx,
                y + labels[3].height / 2.0,
                &format!(
                    " class=\"pele-c4-descr\" font-size=\"{}\"{}",
                    js::num_str(small),
                    color
                ),
                None,
            ));
            let body = draw_shape(
                form,
                w,
                h,
                &format!(
                    " fill=\"var({})\"{}{}",
                    if external { "--_bg" } else { "--_s" },
                    stroke,
                    st.shape
                ),
                &format!("{}{}", stroke, st.line),
            ) + &text;
            nodes_out.push_str(&format!(
                "<g class=\"pele-node pele-shape-{} pele-c4-{}{}\" data-id=\"{}\" transform=\"translate({},{})\">{}</g>",
                form,
                shape.kind,
                if external { " pele-c4-external" } else { "" },
                esc_text(&shape.alias),
                num(node.bx.x + w / 2.0),
                num(node.bx.y + h / 2.0),
                link(&body, shape.link.as_deref(), &shape.alias, &mut links, options)
            ));
        }
    }

    let title = layout_label(
        db.title.as_deref(),
        false,
        measurer,
        size,
        if tight { title_room(options) } else { 4000.0 },
        style::BOLD,
        icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 12.0
    } else {
        0.0
    };
    let inner = jmax(max_x - min_x, title.width);
    let width = (inner + 2.0 * pad).ceil();
    let height = (max_y - min_y + title_height + 2.0 * pad).ceil();
    let ox = pad - min_x + (inner - (max_x - min_x)) / 2.0;
    let oy = pad + title_height - min_y;

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
    if !clusters.is_empty() {
        content.push_str(&format!("<g class=\"pele-clusters\">{}</g>", clusters));
    }
    content.push_str(&casings_svg(&casings));
    if !edges_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke=\"var(--_l)\" stroke-linecap=\"round\">{}</g>",
            edges_out
        ));
    }
    if !nodes_out.is_empty() {
        content.push_str(&format!("<g class=\"pele-nodes\">{}</g>", nodes_out));
    }
    if !labels_out.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edge-labels\" font-size=\"{}\">{}</g>",
            js::num_str(small),
            labels_out
        ));
    }
    content.push_str("</g>");
    let svg = svg_document(
        "c4",
        width,
        height,
        size,
        options,
        palette_style,
        &Accessible {
            acc_title: db.acc_title.clone(),
            acc_descr: db.acc_descr.clone(),
        },
        &content,
    );
    Ok((
        Rendered {
            svg,
            width,
            height,
            links,
        },
        lanes.squeezed,
    ))
}
