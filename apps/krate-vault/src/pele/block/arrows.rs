//! Ports diagrams/block/arrows.ts: block arrows, `<["label"]>(dir)`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use crate::pele::js::{jmax, jmin, trim};
use crate::pele::svg::builder::num;

pub const RIGHT: u32 = 1;
pub const LEFT: u32 = 2;
pub const UP: u32 = 4;
pub const DOWN: u32 = 8;

/// How far a head reaches past each side of its shaft.
pub const OVERHANG: f64 = 7.0;
const MAX_DEPTH: f64 = 20.0;
const NUB: f64 = 12.0;

/// Ports `interface ArrowSize`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrowSize {
    pub w: f64,
    pub h: f64,
    /// Shift of the label from the center, away from the heads.
    pub dx: f64,
    pub dy: f64,
}

/// Ports `arrowMask`: Mermaid compares directions exactly, so `( up )`
/// draws no arrow there. Here spaces are ignored.
pub fn arrow_mask(directions: Option<&[String]>) -> u32 {
    let mut mask = 0;
    for direction in directions.unwrap_or(&[]) {
        match trim(direction) {
            "right" => mask |= RIGHT,
            "left" => mask |= LEFT,
            "up" => mask |= UP,
            "down" => mask |= DOWN,
            "x" => mask |= RIGHT | LEFT,
            "y" => mask |= UP | DOWN,
            _ => {}
        }
    }
    mask
}

/// Ports `bit`.
fn bit(mask: u32, flag: u32) -> f64 {
    if mask & flag != 0 {
        1.0
    } else {
        0.0
    }
}

/// Ports `arrowAxis`: an arrow along one axis has a shaft and heads wider
/// than it; one that points along both axes is a box with a small point on
/// each side it names.
pub fn arrow_axis(mask: u32) -> &'static str {
    if mask != 0 && mask & (UP | DOWN) == 0 {
        return "x";
    }
    if mask != 0 && mask & (RIGHT | LEFT) == 0 {
        return "y";
    }
    ""
}

/// Ports `arrowSize`.
pub fn arrow_size(mask: u32, tw: f64, th: f64) -> ArrowSize {
    let axis = arrow_axis(mask);
    let across = bit(mask, LEFT) - bit(mask, RIGHT);
    let along = bit(mask, UP) - bit(mask, DOWN);
    if axis == "x" {
        let h = jmax(th + 8.0, 14.0) + 2.0 * OVERHANG;
        let depth = jmin(h / 2.0, MAX_DEPTH);
        return ArrowSize {
            w: jmax(tw + 12.0, 16.0) + depth * if mask == (RIGHT | LEFT) { 2.0 } else { 1.0 },
            h,
            dx: (across * depth) / 2.0,
            dy: 0.0,
        };
    }
    if axis == "y" {
        let w = jmax(tw + 16.0, 14.0) + 2.0 * OVERHANG;
        let depth = jmin(w / 2.0, MAX_DEPTH);
        return ArrowSize {
            w,
            h: jmax(th + 8.0, 16.0) + depth * if mask == (UP | DOWN) { 2.0 } else { 1.0 },
            dx: 0.0,
            dy: (along * depth) / 2.0,
        };
    }
    ArrowSize {
        w: jmax(tw + 16.0, 24.0) + NUB * (bit(mask, LEFT) + bit(mask, RIGHT)),
        h: jmax(th + 8.0, 24.0) + NUB * (bit(mask, UP) + bit(mask, DOWN)),
        dx: (across * NUB) / 2.0,
        dy: (along * NUB) / 2.0,
    }
}

/// Ports `drawArrow`.
pub fn draw_arrow(mask: u32, w: f64, h: f64, attrs: &str) -> String {
    let axis = arrow_axis(mask);
    let x = w / 2.0;
    let y = h / 2.0;
    let mut p: Vec<f64>;
    if axis == "x" {
        let depth = jmin(y, MAX_DEPTH);
        let s = y - OVERHANG;
        let x0 = if mask & LEFT != 0 { -x + depth } else { -x };
        let x1 = if mask & RIGHT != 0 { x - depth } else { x };
        p = alloc::vec![x0, -s, x1, -s];
        if mask & RIGHT != 0 {
            p.extend_from_slice(&[x1, -y, x, 0.0, x1, y]);
        }
        p.extend_from_slice(&[x1, s, x0, s]);
        if mask & LEFT != 0 {
            p.extend_from_slice(&[x0, y, -x, 0.0, x0, -y]);
        }
    } else if axis == "y" {
        let depth = jmin(x, MAX_DEPTH);
        let s = x - OVERHANG;
        let y0 = if mask & UP != 0 { -y + depth } else { -y };
        let y1 = if mask & DOWN != 0 { y - depth } else { y };
        p = alloc::vec![s, y0, s, y1];
        if mask & DOWN != 0 {
            p.extend_from_slice(&[x, y1, 0.0, y, -x, y1]);
        }
        p.extend_from_slice(&[-s, y1, -s, y0]);
        if mask & UP != 0 {
            p.extend_from_slice(&[-x, y0, 0.0, -y, x, y0]);
        }
    } else {
        let x0 = if mask & LEFT != 0 { -x + NUB } else { -x };
        let x1 = if mask & RIGHT != 0 { x - NUB } else { x };
        let y0 = if mask & UP != 0 { -y + NUB } else { -y };
        let y1 = if mask & DOWN != 0 { y - NUB } else { y };
        let cx = (x0 + x1) / 2.0;
        let cy = (y0 + y1) / 2.0;
        let kx = jmin(10.0, (x1 - x0) / 2.0);
        let ky = jmin(10.0, (y1 - y0) / 2.0);
        p = alloc::vec![x0, y0];
        if mask & UP != 0 {
            p.extend_from_slice(&[cx - kx, y0, cx, -y, cx + kx, y0]);
        }
        p.extend_from_slice(&[x1, y0]);
        if mask & RIGHT != 0 {
            p.extend_from_slice(&[x1, cy - ky, x, cy, x1, cy + ky]);
        }
        p.extend_from_slice(&[x1, y1]);
        if mask & DOWN != 0 {
            p.extend_from_slice(&[cx + kx, y1, cx, y, cx - kx, y1]);
        }
        p.extend_from_slice(&[x0, y1]);
        if mask & LEFT != 0 {
            p.extend_from_slice(&[x0, cy + ky, -x, cy, x0, cy - ky]);
        }
    }
    let mut points = String::new();
    let mut i = 0;
    while i < p.len() {
        if i > 0 {
            points.push(' ');
        }
        points.push_str(&format!("{},{}", num(p[i]), num(p[i + 1])));
        i += 2;
    }
    format!(
        "<polygon points=\"{}\" stroke-linejoin=\"round\"{}/>",
        points, attrs
    )
}
