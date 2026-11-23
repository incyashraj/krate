//! Ports svg/cluster.ts: where a cluster's title sits along its top edge
//! so that edges entering there do not cross it, and the title drawn over
//! the edges when one does.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::builder::{label_svg, num};
use crate::pele::js::{jmin, jround};
use crate::pele::layout::CNode;
use crate::pele::text::label::{Icons, Label};

/// Ports `type Crossings`: the x of every route point, by its rounded y.
/// The keys are whole numbers, held as i64.
pub type Crossings = BTreeMap<i64, Vec<f64>>;

fn key(y: f64) -> i64 {
    jround(y) as i64
}

/// Ports `markCrossings`.
pub fn mark_crossings(crossings: &mut Crossings, route: &[f64]) {
    let mut k = 0;
    while k + 1 < route.len() {
        crossings
            .entry(key(route[k + 1]))
            .or_default()
            .push(route[k]);
        k += 3;
    }
}

/// Ports `clusterTitleX`: picks where a cluster title sits along the top
/// edge so that edges entering there do not cross it.
pub fn cluster_title_x(c: &CNode, width: f64, crossings: &Crossings, pad: f64) -> f64 {
    if width == 0.0 {
        return c.x;
    }
    let top = c.y - c.h / 2.0;
    let left = c.x - c.w / 2.0;
    let half = width / 2.0;
    let candidates = [left + pad - 6.0 + half, c.x, left + c.w - pad + 6.0 - half];
    let Some(xs) = crossings.get(&key(top)) else {
        return candidates[0];
    };
    let mut best = candidates[0];
    let mut best_gap = -1.0f64;
    for x in candidates {
        let mut gap = f64::INFINITY;
        for &cx in xs {
            if cx > left && cx < left + c.w {
                gap = jmin(gap, (cx - x).abs() - half);
            }
        }
        if gap >= 6.0 {
            return x;
        }
        if gap > best_gap + 0.5 {
            best_gap = gap;
            best = x;
        }
    }
    best
}

/// Ports `titleCrossed`: whether an edge that enters at the top runs
/// through a title placed at `x`.
pub fn title_crossed(c: &CNode, width: f64, x: f64, crossings: &Crossings) -> bool {
    if width <= 0.0 {
        return false;
    }
    let Some(xs) = crossings.get(&key(c.y - c.h / 2.0)) else {
        return false;
    };
    let left = c.x - c.w / 2.0;
    for &cx in xs {
        if cx > left && cx < left + c.w && (cx - x).abs() < width / 2.0 + 4.0 {
            return true;
        }
    }
    false
}

/// Ports `struckTitle`: a title that an edge runs through, drawn over the
/// edges on an outline in the colour of the cluster behind it. `parents`
/// gives each node's parent (-1 for none), to count how deep it is.
#[allow(clippy::too_many_arguments)]
pub fn struck_title(
    label: &Label,
    x: f64,
    y: f64,
    attrs: &str,
    id: &str,
    parents: &[i32],
    index: usize,
    icons: Icons,
) -> String {
    let mut depth = 1;
    let mut p = parents[index];
    while p >= 0 && depth < 8 {
        depth += 1;
        p = parents[p as usize];
    }
    // Each cluster is half its fill over what is behind it.
    let fill = format!(
        "color-mix(in srgb,var(--_a) {}%,var(--_bg))",
        num(100.0 * (1.0 - 0.5f64.powi(depth)))
    );
    format!(
        "<g class=\"pele-cluster-title\" data-id=\"{}\">{}{}</g>",
        id,
        label_svg(
            label,
            x,
            y,
            &format!(
                " fill=\"none\" stroke=\"{}\" stroke-width=\"5\" stroke-linejoin=\"round\"",
                fill
            ),
            icons
        ),
        label_svg(label, x, y, attrs, icons)
    )
}
