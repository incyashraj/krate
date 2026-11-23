//! Ports svg/shapes.ts: every node shape Pele draws, its size around a
//! label, how far its outline sits inside its box, and how much of a side
//! edges may spread along.

// The shape tables keep the TypeScript's one function per shape and side.
#![allow(clippy::collapsible_match)]

use alloc::format;
use alloc::string::String;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::builder::num;
use super::theme::rounded;
use crate::pele::js::{jmax, jmin};
use crate::pele::layout::CNode;

/// Ports `interface ShapeSize`. `dy` is the vertical shift of the label
/// from the shape's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeSize {
    pub w: f64,
    pub h: f64,
    pub dy: f64,
}

pub const PAD_X: f64 = 18.0;
pub const PAD_Y: f64 = 12.0;

/// Kept clear at each end of a straight side, for the corner.
const CORNER: f64 = 12.0;

/// The shapes `SHAPES` names.
const NAMES: [&str; 57] = [
    "rect",
    "rounded",
    "stadium",
    "fr-rect",
    "cyl",
    "circle",
    "dbl-circ",
    "ellipse",
    "diam",
    "hex",
    "odd",
    "lean-r",
    "lean-l",
    "trap-b",
    "trap-t",
    "text",
    "sm-circ",
    "f-circ",
    "fr-circ",
    "fork",
    "notch-rect",
    "lin-rect",
    "div-rect",
    "win-pane",
    "tri",
    "flip-tri",
    "sl-rect",
    "notch-pent",
    "hourglass",
    "delay",
    "h-cyl",
    "cross-circ",
    "st-rect",
    "doc",
    "flag",
    "datastore",
    "folder",
    "bucket",
    "console",
    "browser",
    "person",
    "bang",
    "cloud",
    "brace",
    "brace-r",
    "braces",
    "bolt",
    "lin-cyl",
    "curv-trap",
    "docs",
    "bow-rect",
    "tag-rect",
    "tag-doc",
    "lin-doc",
    "collapsedGroup",
    "rect",
    "rect",
];

fn known(shape: &str) -> bool {
    NAMES.contains(&shape)
}

/// Ports `flat`.
fn flat(w: f64, h: f64, sideways: bool) -> f64 {
    (if sideways { h } else { w }) - 2.0 * CORNER
}

/// Ports `flatAcross(cut)`.
fn flat_across(w: f64, _h: f64, sideways: bool, cut: f64) -> f64 {
    if sideways {
        0.0
    } else {
        w - 2.0 * cut - 2.0 * CORNER
    }
}

/// Ports `bulge`: how far an ellipse's outline has fallen away from its
/// bounding box, `t` from the middle of a side `half` long.
fn bulge(t: f64, half: f64, depth: f64) -> f64 {
    let k = jmin(1.0, t.abs() / half);
    depth * (1.0 - (1.0 - k * k).sqrt())
}

/// Ports `roundInset`.
fn round_inset(w: f64, h: f64, side: u8, t: f64) -> f64 {
    if side & 1 != 0 {
        bulge(t, h / 2.0, w / 2.0)
    } else {
        bulge(t, w / 2.0, h / 2.0)
    }
}

/// Ports `roundSpan`.
fn round_span(w: f64, h: f64, sideways: bool) -> f64 {
    (if sideways { h } else { w }) * 0.6
}

/// Ports `capInset`.
fn cap_inset(w: f64, side: u8, t: f64) -> f64 {
    if side & 1 != 0 {
        0.0
    } else {
        bulge(t, w / 2.0, jmin(12.0, jmax(6.0, w / 14.0)))
    }
}

/// Ports `triSize`.
fn tri_size(tw: f64, th: f64, flip: bool) -> ShapeSize {
    let w = jmax(1.6 * tw + 40.0, 80.0);
    let h = (th + 10.0) / (1.0 - (tw + 8.0) / w);
    let dy = h / 2.0 - 10.0 - th / 2.0;
    ShapeSize {
        w,
        h,
        dy: if flip { -dy } else { dy },
    }
}

/// Ports `path`.
fn path(d: &str, a: &str) -> String {
    format!("<path d=\"{}\"{}/>", d, a)
}

/// Ports `leanInset`.
fn lean_inset(h: f64, side: u8) -> f64 {
    if side & 1 != 0 {
        h / 8.0
    } else {
        0.0
    }
}

/// Ports `box`.
fn box_size(tw: f64, th: f64) -> ShapeSize {
    ShapeSize {
        w: jmax(tw + 2.0 * PAD_X, 2.0 * PAD_X + 12.0),
        h: th + 2.0 * PAD_Y,
        dy: 0.0,
    }
}

/// Ports `poly`.
fn poly(points: &[f64], a: &str) -> String {
    let mut d = String::new();
    let mut i = 0;
    while i + 1 < points.len() {
        if i > 0 {
            d.push(' ');
        }
        d.push_str(&num(points[i]));
        d.push(',');
        d.push_str(&num(points[i + 1]));
        i += 2;
    }
    format!("<polygon points=\"{}\"{}/>", d, a)
}

/// Ports `rect`.
fn rect(w: f64, h: f64, a: &str, rx: &str) -> String {
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}{}/>",
        num(-w / 2.0),
        num(-h / 2.0),
        num(w),
        num(h),
        rx,
        a
    )
}

fn sz(w: f64, h: f64, dy: f64) -> ShapeSize {
    ShapeSize { w, h, dy }
}

/// Ports `brace`.
fn brace(x: f64, h: f64, dir: f64, line: &str) -> String {
    let y = h / 2.0;
    let k = 6.0 * dir;
    format!(
        "<path d=\"M{},{}q{},0 {},6V-6q0,6 {},6q{},0 {},6V{}q0,6 {},6\" fill=\"none\"{}/>",
        num(x),
        num(-y),
        num(k),
        num(k),
        num(k),
        num(-k),
        num(-k),
        num(y - 6.0),
        num(-k),
        line
    )
}

/// Ports `wave`.
fn wave(w: f64, h: f64, a: &str) -> String {
    let x = w / 2.0;
    let top = -h / 2.0;
    let base = h / 2.0 - 10.0;
    let q = w / 4.0;
    format!(
        "<path d=\"M{},{}H{}V{}q{},-12 {},0t{},0Z\"{}/>",
        num(-x),
        num(top),
        num(x),
        num(base),
        num(-q),
        num(-2.0 * q),
        num(-2.0 * q),
        a
    )
}

/// Ports `shapeSize`: the shape's box around a label `tw` by `th`.
pub fn shape_size(shape: &str, tw: f64, th: f64) -> ShapeSize {
    let row = th + 2.0 * PAD_Y;
    match shape {
        "stadium" => sz(tw + row * 0.5 + 2.0 * PAD_X - 8.0, row, 0.0),
        "fr-rect" => sz(tw + 2.0 * PAD_X + 16.0, row, 0.0),
        "cyl" => {
            let w = jmax(tw + 2.0 * PAD_X, 56.0);
            let ry = jmin(12.0, jmax(6.0, w / 14.0));
            sz(w, th + 2.0 * PAD_Y + 2.0 * ry, ry / 2.0)
        }
        "circle" => {
            let d = jmax((tw * tw + th * th).sqrt() + 16.0, 44.0);
            sz(d, d, 0.0)
        }
        "dbl-circ" => {
            let d = jmax((tw * tw + th * th).sqrt() + 26.0, 54.0);
            sz(d, d, 0.0)
        }
        "ellipse" => sz(tw * 1.3 + 2.0 * PAD_X, th * 1.2 + 2.0 * PAD_Y, 0.0),
        "diam" => {
            let a = tw + 16.0;
            let b = th + 8.0;
            sz(a + 1.6 * b, a / 1.6 + b, 0.0)
        }
        "hex" | "lean-r" | "lean-l" | "trap-b" | "trap-t" => {
            sz(tw + 2.0 * PAD_X + row / 2.0, row, 0.0)
        }
        "odd" | "delay" => sz(tw + 2.0 * PAD_X + row / 4.0, row, 0.0),
        "text" => sz(tw + 8.0, th + 8.0, 0.0),
        "sm-circ" | "f-circ" => sz(14.0, 14.0, 0.0),
        "fr-circ" => sz(18.0, 18.0, 0.0),
        "fork" => sz(72.0, 8.0, 0.0),
        "lin-rect" => sz(tw + 2.0 * PAD_X + 8.0, row, 0.0),
        "div-rect" => sz(tw + 2.0 * PAD_X, th + 2.0 * PAD_Y + 8.0, 4.0),
        "win-pane" => sz(tw + 2.0 * PAD_X + 8.0, th + 2.0 * PAD_Y + 8.0, 4.0),
        "tri" => tri_size(tw, th, false),
        "flip-tri" => tri_size(tw, th, true),
        "sl-rect" => sz(tw + 2.0 * PAD_X, th + 2.0 * PAD_Y + 10.0, 5.0),
        "notch-pent" => sz(tw + 2.0 * PAD_X, row, 0.0),
        "hourglass" => sz(32.0, 40.0, 0.0),
        "h-cyl" => {
            let rx = jmin(12.0, row / 4.0);
            sz(tw + 2.0 * PAD_X + 2.0 * rx, row, 0.0)
        }
        "cross-circ" => sz(44.0, 44.0, 0.0),
        "st-rect" => sz(tw + 2.0 * PAD_X + 8.0, th + 2.0 * PAD_Y + 8.0, 4.0),
        "doc" | "tag-doc" => sz(tw + 2.0 * PAD_X, th + 2.0 * PAD_Y + 10.0, -5.0),
        "flag" => sz(tw + 2.0 * PAD_X, th + 2.0 * PAD_Y + 16.0, 0.0),
        "folder" => sz(jmax(tw + 2.0 * PAD_X, 64.0), th + 2.0 * PAD_Y + 8.0, 4.0),
        "bucket" => sz(tw + 2.0 * PAD_X + 20.0, th + 2.0 * PAD_Y + 10.0, 3.0),
        "console" | "browser" => sz(jmax(tw + 2.0 * PAD_X, 72.0), th + 2.0 * PAD_Y + 12.0, 6.0),
        "person" => sz(jmax(tw + 2.0 * PAD_X, 56.0), th + 2.0 * PAD_Y + 26.0, 13.0),
        "bang" => sz(tw + 2.0 * PAD_X + 28.0, th + 2.0 * PAD_Y + 24.0, 0.0),
        "cloud" => sz(tw + 2.0 * PAD_X + 32.0, th + 2.0 * PAD_Y + 24.0, 0.0),
        "brace" | "brace-r" => sz(tw + 2.0 * PAD_X + 12.0, row, 0.0),
        "braces" => sz(tw + 2.0 * PAD_X + 24.0, row, 0.0),
        "bolt" => sz(36.0, 56.0, 0.0),
        "lin-cyl" => {
            let w = jmax(tw + 2.0 * PAD_X, 56.0);
            let ry = jmin(12.0, jmax(6.0, w / 14.0));
            sz(w, th + 2.0 * PAD_Y + 2.0 * ry + 8.0, ry / 2.0 + 4.0)
        }
        "curv-trap" => sz(tw + 2.0 * PAD_X + row * 0.6, row, 0.0),
        "docs" => sz(tw + 2.0 * PAD_X + 8.0, th + 2.0 * PAD_Y + 18.0, -1.0),
        "bow-rect" => sz(tw + 2.0 * PAD_X + row * 0.4, row, 0.0),
        "lin-doc" => sz(tw + 2.0 * PAD_X + 8.0, th + 2.0 * PAD_Y + 10.0, -5.0),
        "collapsedGroup" => sz(tw + 2.0 * PAD_X + 8.0, th + 2.0 * PAD_Y + 8.0, -2.0),
        // rect, rounded, notch-rect, datastore, tag-rect and anything unknown.
        _ => box_size(tw, th),
    }
}

/// Ports `drawShape`: the shape's SVG, centred on the origin. `a` is the
/// fill and stroke, `line` the stroke of its inner lines.
pub fn draw_shape(shape: &str, w: f64, h: f64, a: &str, line: &str) -> String {
    match shape {
        "rounded" => rect(w, h, a, &format!(" rx=\"{}\"", num(jmin(12.0, h / 2.0)))),
        "stadium" => rect(w, h, a, &format!(" rx=\"{}\"", num(h / 2.0))),
        "fr-rect" => {
            let x = w / 2.0 - 8.0;
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}V{}M{},{}V{}\" fill=\"none\"{}/>",
                    num(-x),
                    num(-h / 2.0),
                    num(h / 2.0),
                    num(x),
                    num(-h / 2.0),
                    num(h / 2.0),
                    line
                )
        }
        "cyl" => {
            let rx = w / 2.0;
            let ry = jmin(12.0, jmax(6.0, w / 14.0));
            let top = -h / 2.0 + ry;
            let bottom = h / 2.0 - ry;
            format!(
                "<path d=\"M{},{}A{},{} 0 0 1 {},{}V{}A{},{} 0 0 1 {},{}Z\"{}/>",
                num(-rx),
                num(top),
                num(rx),
                num(ry),
                num(rx),
                num(top),
                num(bottom),
                num(rx),
                num(ry),
                num(-rx),
                num(bottom),
                a
            ) + &format!(
                "<path d=\"M{},{}A{},{} 0 0 0 {},{}\" fill=\"none\"{}/>",
                num(-rx),
                num(top),
                num(rx),
                num(ry),
                num(rx),
                num(top),
                line
            )
        }
        "circle" | "sm-circ" => format!("<circle r=\"{}\"{}/>", num(w / 2.0), a),
        "dbl-circ" => format!(
            "<circle r=\"{}\"{}/><circle r=\"{}\" fill=\"none\"{}/>",
            num(w / 2.0),
            a,
            num(w / 2.0 - 5.0),
            line
        ),
        "ellipse" => format!(
            "<ellipse rx=\"{}\" ry=\"{}\"{}/>",
            num(w / 2.0),
            num(h / 2.0),
            a
        ),
        "diam" => poly(
            &[0.0, -h / 2.0, w / 2.0, 0.0, 0.0, h / 2.0, -w / 2.0, 0.0],
            a,
        ),
        "hex" => {
            let m = h / 4.0;
            poly(
                &[
                    -w / 2.0 + m,
                    -h / 2.0,
                    w / 2.0 - m,
                    -h / 2.0,
                    w / 2.0,
                    0.0,
                    w / 2.0 - m,
                    h / 2.0,
                    -w / 2.0 + m,
                    h / 2.0,
                    -w / 2.0,
                    0.0,
                ],
                a,
            )
        }
        "odd" => {
            let notch = h / 4.0;
            poly(
                &[
                    -w / 2.0,
                    -h / 2.0,
                    w / 2.0,
                    -h / 2.0,
                    w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    h / 2.0,
                    -w / 2.0 + notch,
                    0.0,
                ],
                a,
            )
        }
        "lean-r" => {
            let s = h / 4.0;
            poly(
                &[
                    -w / 2.0 + s,
                    -h / 2.0,
                    w / 2.0,
                    -h / 2.0,
                    w / 2.0 - s,
                    h / 2.0,
                    -w / 2.0,
                    h / 2.0,
                ],
                a,
            )
        }
        "lean-l" => {
            let s = h / 4.0;
            poly(
                &[
                    -w / 2.0,
                    -h / 2.0,
                    w / 2.0 - s,
                    -h / 2.0,
                    w / 2.0,
                    h / 2.0,
                    -w / 2.0 + s,
                    h / 2.0,
                ],
                a,
            )
        }
        "trap-b" => {
            let s = h / 4.0;
            poly(
                &[
                    -w / 2.0 + s,
                    -h / 2.0,
                    w / 2.0 - s,
                    -h / 2.0,
                    w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    h / 2.0,
                ],
                a,
            )
        }
        "trap-t" => {
            let s = h / 4.0;
            poly(
                &[
                    -w / 2.0,
                    -h / 2.0,
                    w / 2.0,
                    -h / 2.0,
                    w / 2.0 - s,
                    h / 2.0,
                    -w / 2.0 + s,
                    h / 2.0,
                ],
                a,
            )
        }
        "text" => rect(w, h, " fill=\"none\" stroke=\"none\"", ""),
        "f-circ" => format!(
            "<circle r=\"{}\" fill=\"var(--_l)\"{}/>",
            num(w / 2.0),
            line
        ),
        "fr-circ" => format!(
            "<circle r=\"{}\"{}/><circle r=\"{}\" fill=\"var(--_l)\"{}/>",
            num(w / 2.0),
            a,
            num(w / 2.0 - 4.0),
            line
        ),
        "fork" => rect(w, h, &format!(" fill=\"var(--_l)\"{}", line), " rx=\"2\""),
        "notch-rect" => {
            let c = 10.0;
            poly(
                &[
                    -w / 2.0 + c,
                    -h / 2.0,
                    w / 2.0,
                    -h / 2.0,
                    w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    -h / 2.0 + c,
                ],
                a,
            )
        }
        "lin-rect" => {
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}V{}\" fill=\"none\"{}/>",
                    num(-w / 2.0 + 8.0),
                    num(-h / 2.0),
                    num(h / 2.0),
                    line
                )
        }
        "div-rect" => {
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}H{}\" fill=\"none\"{}/>",
                    num(-w / 2.0),
                    num(-h / 2.0 + 8.0),
                    num(w / 2.0),
                    line
                )
        }
        "win-pane" => {
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}V{}M{},{}H{}\" fill=\"none\"{}/>",
                    num(-w / 2.0 + 8.0),
                    num(-h / 2.0),
                    num(h / 2.0),
                    num(-w / 2.0),
                    num(-h / 2.0 + 8.0),
                    num(w / 2.0),
                    line
                )
        }
        "tri" => poly(&[0.0, -h / 2.0, w / 2.0, h / 2.0, -w / 2.0, h / 2.0], a),
        "flip-tri" => poly(&[-w / 2.0, -h / 2.0, w / 2.0, -h / 2.0, 0.0, h / 2.0], a),
        "sl-rect" => poly(
            &[
                -w / 2.0,
                -h / 2.0 + 10.0,
                w / 2.0,
                -h / 2.0,
                w / 2.0,
                h / 2.0,
                -w / 2.0,
                h / 2.0,
            ],
            a,
        ),
        "notch-pent" => {
            let c = 10.0;
            poly(
                &[
                    -w / 2.0 + c,
                    -h / 2.0,
                    w / 2.0 - c,
                    -h / 2.0,
                    w / 2.0,
                    -h / 2.0 + c,
                    w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    h / 2.0,
                    -w / 2.0,
                    -h / 2.0 + c,
                ],
                a,
            )
        }
        "hourglass" => poly(
            &[
                -w / 2.0,
                -h / 2.0,
                w / 2.0,
                -h / 2.0,
                -w / 2.0,
                h / 2.0,
                w / 2.0,
                h / 2.0,
            ],
            a,
        ),
        "delay" => {
            let r = h / 2.0;
            format!(
                "<path d=\"M{},{}H{}A{},{} 0 0 1 {},{}H{}Z\"{}/>",
                num(-w / 2.0),
                num(-r),
                num(w / 2.0 - r),
                num(r),
                num(r),
                num(w / 2.0 - r),
                num(r),
                num(-w / 2.0),
                a
            )
        }
        "h-cyl" => {
            let ry = h / 2.0;
            let rx = jmin(12.0, h / 4.0);
            let l = -w / 2.0 + rx;
            let r = w / 2.0 - rx;
            format!(
                "<path d=\"M{},{}H{}A{},{} 0 0 1 {},{}H{}A{},{} 0 0 1 {},{}Z\"{}/>",
                num(l),
                num(-ry),
                num(r),
                num(rx),
                num(ry),
                num(r),
                num(ry),
                num(l),
                num(rx),
                num(ry),
                num(l),
                num(-ry),
                a
            ) + &format!(
                "<path d=\"M{},{}A{},{} 0 0 0 {},{}\" fill=\"none\"{}/>",
                num(r),
                num(-ry),
                num(rx),
                num(ry),
                num(r),
                num(ry),
                line
            )
        }
        "cross-circ" => {
            let r = w / 2.0;
            let k = r * core::f64::consts::FRAC_1_SQRT_2;
            format!("<circle r=\"{}\"{}/>", num(r), a)
                + &format!(
                    "<path d=\"M{},{}L{},{}M{},{}L{},{}\" fill=\"none\"{}/>",
                    num(-k),
                    num(-k),
                    num(k),
                    num(k),
                    num(k),
                    num(-k),
                    num(-k),
                    num(k),
                    line
                )
        }
        "st-rect" => {
            let bw = w - 8.0;
            let bh = h - 8.0;
            let mut out = String::new();
            for i in 0..3 {
                let x = -w / 2.0 + (2 - i) as f64 * 4.0;
                let y = -h / 2.0 + i as f64 * 4.0;
                out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                    num(x),
                    num(y),
                    num(bw),
                    num(bh),
                    rounded(a)
                ));
            }
            out
        }
        "doc" => wave(w, h, a),
        "flag" => {
            let x = w / 2.0;
            let y = h / 2.0 - 8.0;
            let q = w / 4.0;
            format!(
                "<path d=\"M{},{}q{},-10 {},0t{},0V{}q{},10 {},0t{},0Z\"{}/>",
                num(-x),
                num(-y),
                num(q),
                num(2.0 * q),
                num(2.0 * q),
                num(y),
                num(-q),
                num(-2.0 * q),
                num(-2.0 * q),
                a
            )
        }
        "datastore" => {
            rect(
                w,
                h,
                &a.replacen("stroke=\"var(--_b)\"", "stroke=\"none\"", 1),
                "",
            ) + &format!(
                "<path d=\"M{},{}H{}M{},{}H{}\" fill=\"none\"{}/>",
                num(-w / 2.0),
                num(-h / 2.0),
                num(w / 2.0),
                num(-w / 2.0),
                num(h / 2.0),
                num(w / 2.0),
                line
            )
        }
        "folder" => {
            let x = w / 2.0;
            let y = h / 2.0;
            let tab = jmin(w * 0.4, 48.0);
            path(
                &format!(
                    "M{},{}H{}L{},{}H{}V{}H{}Z",
                    num(-x),
                    num(-y),
                    num(-x + tab),
                    num(-x + tab + 8.0),
                    num(-y + 8.0),
                    num(x),
                    num(y),
                    num(-x)
                ),
                a,
            )
        }
        "bucket" => {
            let x = w / 2.0;
            let y = h / 2.0;
            let ry = 5.0;
            let taper = h / 6.0;
            path(
                &format!(
                    "M{},{}A{},{} 0 0 1 {},{}L{},{}A{},{} 0 0 1 {},{}Z",
                    num(-x),
                    num(-y + ry),
                    num(x),
                    num(ry),
                    num(x),
                    num(-y + ry),
                    num(x - taper),
                    num(y - ry),
                    num(x - taper),
                    num(ry),
                    num(-x + taper),
                    num(y - ry)
                ),
                a,
            ) + &format!(
                "<path d=\"M{},{}A{},{} 0 0 0 {},{}\" fill=\"none\"{}/>",
                num(-x),
                num(-y + ry),
                num(x),
                num(ry),
                num(x),
                num(-y + ry),
                line
            )
        }
        "console" => {
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}H{}M{},{}l3,2.5l-3,2.5M{},{}h4\" fill=\"none\"{}/>",
                    num(-w / 2.0),
                    num(-h / 2.0 + 12.0),
                    num(w / 2.0),
                    num(-w / 2.0 + 7.0),
                    num(-h / 2.0 + 4.0),
                    num(-w / 2.0 + 13.0),
                    num(-h / 2.0 + 9.0),
                    line
                )
        }
        "browser" => {
            let mut dots = String::new();
            for i in 0..3 {
                dots.push_str(&format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"1.5\" fill=\"none\"{}/>",
                    num(-w / 2.0 + 8.0 + i as f64 * 7.0),
                    num(-h / 2.0 + 6.0),
                    line
                ));
            }
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}H{}\" fill=\"none\"{}/>",
                    num(-w / 2.0),
                    num(-h / 2.0 + 12.0),
                    num(w / 2.0),
                    line
                )
                + &dots
        }
        "person" => {
            let top = -h / 2.0;
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"12\"{}/>",
                num(-w / 2.0),
                num(top + 26.0),
                num(w),
                num(h - 26.0),
                a
            ) + &format!("<circle cy=\"{}\" r=\"13\"{}/>", num(top + 14.0), a)
        }
        "bang" => {
            let spikes = 14;
            let mut pts = alloc::vec::Vec::new();
            for i in 0..spikes * 2 {
                let angle = (core::f64::consts::PI * i as f64) / spikes as f64
                    - core::f64::consts::PI / 2.0;
                let k = if i % 2 == 0 { 1.0 } else { 0.82 };
                pts.push(angle.cos() * (w / 2.0) * k);
                pts.push(angle.sin() * (h / 2.0) * k);
            }
            poly(&pts, a)
        }
        "cloud" => {
            let x = w / 2.0;
            let y = h / 2.0;
            let r = y * 0.55;
            path(
                &format!(
                    "M{},{}A{},{} 0 0 1 {},{}A{},{} 0 0 1 {},{}A{},{} 0 0 1 {},{}A{},{} 0 0 1 {},{}Z",
                    num(-x + r),
                    num(y),
                    num(r),
                    num(r),
                    num(-x + r * 0.6),
                    num(-y * 0.1),
                    num(x * 0.42),
                    num(y * 0.7),
                    num(-x * 0.1),
                    num(-y * 0.72),
                    num(x * 0.4),
                    num(y * 0.62),
                    num(x * 0.62),
                    num(-y * 0.42),
                    num(r * 1.05),
                    num(r * 1.05),
                    num(x - r),
                    num(y)
                ),
                a,
            )
        }
        "brace" => {
            rect(w, h, " fill=\"none\" stroke=\"none\"", "")
                + &brace(-w / 2.0 + 12.0, h, -1.0, line)
        }
        "brace-r" => {
            rect(w, h, " fill=\"none\" stroke=\"none\"", "") + &brace(w / 2.0 - 12.0, h, 1.0, line)
        }
        "braces" => {
            rect(w, h, " fill=\"none\" stroke=\"none\"", "")
                + &brace(-w / 2.0 + 12.0, h, -1.0, line)
                + &brace(w / 2.0 - 12.0, h, 1.0, line)
        }
        "bolt" => poly(
            &[
                w * 0.15,
                -h / 2.0,
                -w / 2.0,
                h * 0.08,
                -w * 0.05,
                h * 0.08,
                -w * 0.15,
                h / 2.0,
                w / 2.0,
                -h * 0.08,
                w * 0.05,
                -h * 0.08,
            ],
            a,
        ),
        "lin-cyl" => {
            let rx = w / 2.0;
            let ry = jmin(12.0, jmax(6.0, w / 14.0));
            let top = -h / 2.0 + ry;
            let bottom = h / 2.0 - ry;
            let arc = |y: f64| {
                format!(
                    "M{},{}A{},{} 0 0 0 {},{}",
                    num(-rx),
                    num(y),
                    num(rx),
                    num(ry),
                    num(rx),
                    num(y)
                )
            };
            path(
                &format!(
                    "M{},{}A{},{} 0 0 1 {},{}V{}A{},{} 0 0 1 {},{}Z",
                    num(-rx),
                    num(top),
                    num(rx),
                    num(ry),
                    num(rx),
                    num(top),
                    num(bottom),
                    num(rx),
                    num(ry),
                    num(-rx),
                    num(bottom)
                ),
                a,
            ) + &format!(
                "<path d=\"{}{}\" fill=\"none\"{}/>",
                arc(top),
                arc(top + 8.0),
                line
            )
        }
        "curv-trap" => {
            let x = w / 2.0;
            let y = h / 2.0;
            let k = h * 0.3;
            path(
                &format!(
                    "M{},{}H{}A{},{} 0 0 1 {},{}H{}L{},0Z",
                    num(-x + k),
                    num(-y),
                    num(x - k),
                    num(k),
                    num(y),
                    num(x - k),
                    num(y),
                    num(-x + k),
                    num(-x)
                ),
                a,
            )
        }
        "docs" => {
            let mut out = String::new();
            for i in 0..3i32 {
                out.push_str(&format!(
                    "<g transform=\"translate({},{})\">{}</g>",
                    (1 - i) * 4,
                    (i - 1) * 4,
                    wave(w - 8.0, h - 8.0, a)
                ));
            }
            out
        }
        "bow-rect" => {
            let x = w / 2.0;
            let y = h / 2.0;
            let k = h * 0.2;
            path(
                &format!(
                    "M{},{}H{}A{},{} 0 0 0 {},{}H{}A{},{} 0 0 1 {},{}Z",
                    num(-x + k),
                    num(-y),
                    num(x),
                    num(k),
                    num(y),
                    num(x),
                    num(y),
                    num(-x + k),
                    num(k),
                    num(y),
                    num(-x + k),
                    num(-y)
                ),
                a,
            )
        }
        "tag-rect" => {
            rect(w, h, &rounded(a), "")
                + &format!(
                    "<path d=\"M{},{}L{},{}\" fill=\"none\"{}/>",
                    num(w / 2.0 - 12.0),
                    num(h / 2.0),
                    num(w / 2.0),
                    num(h / 2.0 - 12.0),
                    line
                )
        }
        "tag-doc" => {
            wave(w, h, a)
                + &format!(
                    "<path d=\"M{},{}L{},{}\" fill=\"none\"{}/>",
                    num(w / 2.0 - 12.0),
                    num(h / 2.0 - 14.0),
                    num(w / 2.0),
                    num(h / 2.0 - 26.0),
                    line
                )
        }
        "lin-doc" => {
            wave(w, h, a)
                + &format!(
                    "<path d=\"M{},{}V{}\" fill=\"none\"{}/>",
                    num(-w / 2.0 + 8.0),
                    num(-h / 2.0),
                    num(h / 2.0 - 6.0),
                    line
                )
        }
        "collapsedGroup" => {
            format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                num(-w / 2.0 + 6.0),
                num(-h / 2.0 + 8.0),
                num(w - 12.0),
                num(h - 8.0),
                rounded(a)
            ) + &format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"{}/>",
                num(-w / 2.0),
                num(-h / 2.0),
                num(w),
                num(h - 4.0),
                rounded(a)
            )
        }
        // rect and anything unknown.
        _ => rect(w, h, &rounded(a), ""),
    }
}

/// Ports `shapeInset`: distance from the bounding box to the outline on a
/// side (0 top, 1 right, 2 bottom, 3 left), `t` along the side from its
/// middle.
pub fn shape_inset(shape: &str, w: f64, h: f64, side: u8, t: f64) -> f64 {
    let odd = side & 1 != 0;
    match shape {
        "stadium" => {
            if odd {
                bulge(t, h / 2.0, h / 2.0)
            } else {
                0.0
            }
        }
        "cyl" | "lin-cyl" => cap_inset(w, side, t),
        "circle" | "dbl-circ" | "ellipse" | "sm-circ" | "f-circ" | "fr-circ" | "cross-circ" => {
            round_inset(w, h, side, t)
        }
        "diam" => {
            if odd {
                (t.abs() * w) / h
            } else {
                (t.abs() * h) / w
            }
        }
        "hex" => {
            if odd {
                t.abs() / 2.0
            } else {
                0.0
            }
        }
        "odd" => {
            if side == 3 {
                h / 4.0
            } else {
                0.0
            }
        }
        "lean-r" | "lean-l" | "trap-b" | "trap-t" => lean_inset(h, side),
        "tri" | "flip-tri" | "bolt" => {
            if odd {
                w / 4.0
            } else {
                0.0
            }
        }
        "sl-rect" => {
            if side == 0 {
                5.0
            } else {
                0.0
            }
        }
        "hourglass" => {
            if odd {
                w / 2.0
            } else {
                0.0
            }
        }
        "doc" | "docs" | "tag-doc" | "lin-doc" => {
            if side == 2 {
                10.0
            } else {
                0.0
            }
        }
        "flag" => {
            if odd {
                0.0
            } else {
                8.0
            }
        }
        "bucket" => {
            if odd {
                h / 12.0
            } else {
                0.0
            }
        }
        "bang" => 3.0,
        "cloud" => {
            if odd {
                2.0
            } else {
                1.0
            }
        }
        "bow-rect" => {
            if side == 1 {
                h * 0.2
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// Ports `shapeSpan`: how much of a side of the shape edges may spread
/// along, when the flow runs down or, if `sideways`, across.
pub fn shape_span(shape: &str, w: f64, h: f64, sideways: bool) -> f64 {
    let across = if sideways { h } else { w };
    let v = match shape {
        "rect" | "rounded" | "fr-rect" | "text" | "notch-rect" | "lin-rect" | "div-rect"
        | "win-pane" | "notch-pent" | "datastore" | "console" | "browser" | "tag-rect" => {
            flat(w, h, sideways)
        }
        "stadium" => {
            if sideways {
                h * 0.6
            } else {
                w - h
            }
        }
        "cyl" => {
            if sideways {
                h - 2.0 * CORNER - 12.0
            } else {
                w * 0.6
            }
        }
        "lin-cyl" => {
            if sideways {
                h - 2.0 * CORNER - 20.0
            } else {
                w * 0.6
            }
        }
        "circle" | "dbl-circ" | "ellipse" | "sm-circ" | "f-circ" | "fr-circ" | "cross-circ" => {
            round_span(w, h, sideways)
        }
        "diam" => across * 0.6,
        "hex" => {
            if sideways {
                h * 0.5
            } else {
                w - h / 2.0 - CORNER
            }
        }
        "odd" => flat_across(w, h, sideways, 0.0),
        "lean-r" | "lean-l" | "trap-b" | "trap-t" => flat_across(w, h, sideways, h / 4.0),
        "fork" => across - 8.0,
        "st-rect" => across - 2.0 * CORNER - 8.0,
        _ => 0.0,
    };
    jmax(0.0, v)
}

/// Ports `shapeHasLabel`.
pub fn shape_has_label(shape: &str) -> bool {
    !matches!(
        shape,
        "sm-circ" | "f-circ" | "fr-circ" | "fork" | "hourglass" | "cross-circ" | "bolt"
    )
}

/// Ports `shapeFilled`: false for a shape that ignores style fills.
pub fn shape_filled(shape: &str) -> bool {
    !(known(shape) && matches!(shape, "text" | "brace" | "brace-r" | "braces"))
}

/// Ports `endSide`: the side of a node's layout box that a route end lies
/// on: 0 top, 1 right, 2 bottom, 3 left, or -1 for none.
pub fn end_side(route: &[f64], at: usize, c: &CNode) -> i32 {
    let get = |k: usize| route.get(k).copied().unwrap_or(f64::NAN);
    let dx = get(at) - c.x;
    let dy = get(at + 1) - c.y;
    if (dy.abs() - c.h / 2.0).abs() < 0.5 {
        return if dy < 0.0 { 0 } else { 2 };
    }
    if (dx.abs() - c.w / 2.0).abs() < 0.5 {
        return if dx < 0.0 { 3 } else { 1 };
    }
    -1
}

/// Ports `insetRoute`: moves a route end from a node's layout box onto the
/// outline of the shape drawn in it.
pub fn inset_route(route: &mut [f64], at: usize, shape: &str, w: f64, h: f64, c: &CNode) {
    if c.is_group {
        return;
    }
    let side = end_side(route, at, c);
    if side >= 0 && route.len() < at + 2 {
        return;
    }
    if side == 0 || side == 2 {
        let amount = shape_inset(shape, w, h, side as u8, route[at] - c.x) + (c.h - h) / 2.0;
        route[at + 1] += if side == 0 { amount } else { -amount };
    } else if side >= 0 {
        let amount = shape_inset(shape, w, h, side as u8, route[at + 1] - c.y) + (c.w - w) / 2.0;
        route[at] += if side == 3 { amount } else { -amount };
    }
}
