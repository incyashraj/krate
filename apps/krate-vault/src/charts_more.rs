//! The newer Mermaid kinds Obsidian 1.15 draws: gitGraph, quadrantChart,
//! xychart-beta, sankey-beta, block-beta, requirementDiagram and C4's
//! context diagrams, laid out into the same [`Item`]s as the others
//! (src/charts.rs). Sizes are Mermaid's, measured in Obsidian, at its 16 px
//! text scaled to `size`.

use crate::charts::{
    bx, graph_chart, line, lines_of, text, unquote, Chart, GEdge, GNode, Head, Item, Role,
};
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

type Measure<'a> = &'a dyn Fn(&str, f32) -> f32;

/// The value of `key: "value"` or `key "value"` after a keyword.
fn after<'a>(l: &'a str, key: &str) -> Option<&'a str> {
    let r = l.strip_prefix(key)?;
    Some(r.trim().trim_start_matches(':').trim().trim_matches('"'))
}

fn quoted_args(s: &str) -> Vec<String> {
    // Comma separated, commas inside quotes kept.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for ch in s.chars() {
        match ch {
            '"' => q = !q,
            ',' if !q => out.push(core::mem::take(&mut cur).trim().into()),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(String::from(cur.trim()));
    }
    out
}

// ---- gitGraph -------------------------------------------------------------------------------

pub fn git(src: &str, width: Measure, size: f32) -> Option<Chart> {
    struct Commit {
        lane: usize,
        parents: Vec<usize>,
        merge: bool,
        id: String,
        tag: String,
    }
    let mut lanes: Vec<String> = alloc::vec![String::from("main")];
    let mut head: Vec<Option<usize>> = alloc::vec![None];
    let mut cur = 0usize;
    let mut commits: Vec<Commit> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        let mut w = l.split_whitespace();
        let cmd = w.next().unwrap_or("");
        let rest = l.get(cmd.len()..).unwrap_or("").trim();
        let opt = |k: &str| -> String {
            rest.find(&alloc::format!("{k}:"))
                .map(|i| {
                    let v = rest.get(i + k.len() + 1..).unwrap_or("").trim_start();
                    let v = v.strip_prefix('"').unwrap_or(v);
                    String::from(v.split(['"', ' ']).next().unwrap_or(""))
                })
                .unwrap_or_default()
        };
        match cmd {
            "commit" => {
                commits.push(Commit {
                    lane: cur,
                    parents: head[cur].into_iter().collect(),
                    merge: false,
                    id: opt("id"),
                    tag: opt("tag"),
                });
                head[cur] = Some(commits.len() - 1);
            }
            "branch" => {
                let name = String::from(w.next().unwrap_or("branch").trim_matches('"'));
                let from = head[cur];
                lanes.push(name);
                head.push(from);
                cur = lanes.len() - 1;
            }
            "checkout" | "switch" => {
                let name = w.next().unwrap_or("").trim_matches('"');
                if let Some(i) = lanes.iter().position(|b| b == name) {
                    cur = i;
                }
            }
            "merge" => {
                let name = w.next().unwrap_or("").trim_matches('"');
                if let Some(i) = lanes.iter().position(|b| b == name) {
                    let mut parents: Vec<usize> = head[cur].into_iter().collect();
                    parents.extend(head[i]);
                    commits.push(Commit {
                        lane: cur,
                        parents,
                        merge: true,
                        id: opt("id"),
                        tag: opt("tag"),
                    });
                    head[cur] = Some(commits.len() - 1);
                }
            }
            "cherry-pick" => {}
            _ => {}
        }
    }
    if commits.is_empty() {
        return None;
    }
    // Measured on Obsidian's: 36 px between commits, 72 between branches,
    // 6 px dots, the branch names in coloured boxes at the left.
    let step = size * 2.7;
    let lane_gap = size * 5.6;
    let label_w = lanes.iter().map(|b| width(b, size)).fold(0.0, f32::max) + size;
    let x0 = label_w + size * 1.5;
    let cx = |i: usize| x0 + i as f32 * step;
    let cy = |lane: usize| size + lane as f32 * lane_gap;
    let end = cx(commits.len() - 1) + step;
    let mut items = Vec::new();
    for (k, name) in lanes.iter().enumerate() {
        let y = cy(k);
        items.push(Item::Line {
            pts: alloc::vec![(label_w, y), (end, y)],
            role: Role::Series(k as u8),
            width: 1.0,
            dashed: true,
            head: Head::None,
            tail: Head::None,
        });
        let w = width(name, size) + size;
        items.push(Item::Box {
            x: label_w - w,
            y: y - size * 0.8,
            w,
            h: size * 1.6,
            r: 3.0,
            fill: Role::Series(k as u8),
            stroke: Role::None,
            dashed: false,
        });
        items.push(text(
            label_w - w * 0.5,
            y + size * 0.35,
            name,
            size,
            1,
            false,
            Role::Ink,
        ));
    }
    for (i, c) in commits.iter().enumerate() {
        for &p in &c.parents {
            let (a, b) = ((cx(p), cy(commits[p].lane)), (cx(i), cy(c.lane)));
            let role = Role::Series(commits[p].lane.max(c.lane) as u8);
            let pts = if (a.1 - b.1).abs() < 0.5 {
                alloc::vec![a, b]
            } else {
                // An S from one lane to the other, as Mermaid curves it.
                let mid = (a.0 + b.0) * 0.5;
                crate::layered::basis(&[a, (mid, a.1), (mid, b.1), b])
            };
            items.push(Item::Line {
                pts,
                role,
                width: 2.0,
                dashed: false,
                head: Head::None,
                tail: Head::None,
            });
        }
    }
    for (i, c) in commits.iter().enumerate() {
        let (x, y) = (cx(i), cy(c.lane));
        let r = size * 0.45;
        if c.merge {
            items.push(Item::Circle {
                cx: x,
                cy: y,
                r,
                fill: Role::Paper,
                stroke: Role::Series(c.lane as u8),
            });
            items.push(Item::Circle {
                cx: x,
                cy: y,
                r: r * 0.45,
                fill: Role::Series(c.lane as u8),
                stroke: Role::None,
            });
        } else {
            items.push(Item::Circle {
                cx: x,
                cy: y,
                r,
                fill: Role::Series(c.lane as u8),
                stroke: Role::None,
            });
        }
        // Mermaid writes a made-up id for each commit, turned on its side;
        // Krate cannot turn text (K-1341), so only ids and tags written in
        // the diagram are shown, under the dot.
        let mut lbl = c.id.clone();
        if !c.tag.is_empty() {
            if !lbl.is_empty() {
                lbl.push(' ');
            }
            lbl.push_str(&c.tag);
        }
        if !lbl.is_empty() {
            items.push(text(
                x,
                y + size * 1.6,
                &lbl,
                size * 0.75,
                1,
                false,
                Role::Text,
            ));
        }
    }
    Some(Chart {
        w: end,
        h: cy(lanes.len() - 1) + size * 2.5,
        items,
    })
}

// ---- quadrantChart --------------------------------------------------------------------------

pub fn quadrant(src: &str, width: Measure, size: f32) -> Option<Chart> {
    let mut title = String::new();
    let mut xa = (String::new(), String::new());
    let mut ya = (String::new(), String::new());
    let mut quads: [String; 4] = Default::default();
    let mut points: Vec<(String, f32, f32)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        let axis = |r: &str| -> (String, String) {
            let (a, b) = r.split_once("-->").unwrap_or((r, ""));
            (String::from(unquote(a)), String::from(unquote(b)))
        };
        if let Some(t) = after(l, "title") {
            title = String::from(t);
        } else if let Some(r) = l.strip_prefix("x-axis") {
            xa = axis(r);
        } else if let Some(r) = l.strip_prefix("y-axis") {
            ya = axis(r);
        } else if let Some(r) = l.strip_prefix("quadrant-") {
            let (n, t) = r.split_at(1);
            if let Some(i) = n.parse::<usize>().ok().filter(|i| (1..=4).contains(i)) {
                quads[i - 1] = String::from(unquote(t));
            }
        } else if let Some((name, at)) = l.split_once(':') {
            let v: Vec<f32> = at
                .trim()
                .trim_matches(['[', ']'])
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect();
            if v.len() == 2 {
                points.push((
                    String::from(unquote(name)),
                    v[0].clamp(0.0, 1.0),
                    v[1].clamp(0.0, 1.0),
                ));
            }
        }
    }
    // Obsidian's: 213 by 197 px quadrants with a 3 px gap, labels at
    // the top of each, axis names outside.
    let (q, qh) = (size * 13.3, size * 12.3);
    let gap = size * 0.2;
    let left = size * 1.2
        + [&ya.0, &ya.1]
            .iter()
            .map(|s| width(s, size * 0.8))
            .fold(0.0, f32::max);
    let top = if title.is_empty() {
        size * 0.5
    } else {
        size * 3.0
    };
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            left + q + gap * 0.5,
            size * 1.6,
            &title,
            size * 1.05,
            1,
            true,
            Role::Text,
        ));
    }
    // quadrant-1 top right, 2 top left, 3 bottom left, 4 bottom right.
    let at = [(1.0, 0.0), (0.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    for (k, (qx, qy)) in at.iter().enumerate() {
        let (x, y) = (left + qx * (q + gap), top + qy * (qh + gap));
        items.push(bx(x, y, q, qh, 3.0, Role::Node));
        if let Some(Item::Box { stroke, .. }) = items.last_mut() {
            *stroke = Role::None;
        }
        if !quads[k].is_empty() {
            items.push(text(
                x + q * 0.5,
                y + size * 1.4,
                &quads[k],
                size,
                1,
                false,
                Role::Text,
            ));
        }
    }
    let total = q * 2.0 + gap;
    let total_h = qh * 2.0 + gap;
    for (name, px, py) in &points {
        let x = left + px * total;
        let y = top + (1.0 - py) * total_h;
        items.push(Item::Circle {
            cx: x,
            cy: y,
            r: size * 0.32,
            fill: Role::Series(0),
            stroke: Role::None,
        });
        items.push(text(
            x,
            y + size * 1.5,
            name,
            size * 0.85,
            1,
            false,
            Role::Text,
        ));
    }
    // Axis names: x under each half, y beside each half (Mermaid turns
    // these on their side; Krate cannot turn text, K-1341).
    let by = top + total_h + size * 1.5;
    items.push(text(
        left + q * 0.5,
        by,
        &xa.0,
        size * 0.8,
        1,
        false,
        Role::Text,
    ));
    items.push(text(
        left + q * 1.5 + gap,
        by,
        &xa.1,
        size * 0.8,
        1,
        false,
        Role::Text,
    ));
    items.push(text(
        left - size * 0.5,
        top + qh * 0.5,
        &ya.1,
        size * 0.8,
        2,
        false,
        Role::Text,
    ));
    items.push(text(
        left - size * 0.5,
        top + qh * 1.5 + gap,
        &ya.0,
        size * 0.8,
        2,
        false,
        Role::Text,
    ));
    Some(Chart {
        w: left + total + size,
        h: by + size,
        items,
    })
}

// ---- xychart-beta ---------------------------------------------------------------------------

fn nice_step(range: f32) -> f32 {
    let raw = (range / 7.0).max(1e-6);
    let mut p = 1.0f32;
    while p * 10.0 <= raw {
        p *= 10.0;
    }
    while p > raw {
        p /= 10.0;
    }
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if p * m >= raw {
            return p * m;
        }
    }
    p * 10.0
}

fn num_label(v: f32) -> String {
    if (v - v.round()).abs() < 1e-3 {
        alloc::format!("{}", v.round() as i64)
    } else {
        alloc::format!("{v:.1}")
    }
}

pub fn xy(src: &str, width: Measure, size: f32) -> Option<Chart> {
    let mut title = String::new();
    let mut cats: Vec<String> = Vec::new();
    let mut y_name = String::new();
    let mut y_range: Option<(f32, f32)> = None;
    let mut bars: Vec<Vec<f32>> = Vec::new();
    let mut lines: Vec<Vec<f32>> = Vec::new();
    let nums = |s: &str| -> Vec<f32> {
        let inner = s
            .get(s.find('[').map_or(0, |i| i + 1)..s.rfind(']').unwrap_or(s.len()))
            .unwrap_or("");
        inner
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect()
    };
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(t) = after(l, "title") {
            title = String::from(t);
        } else if let Some(r) = l.strip_prefix("x-axis") {
            if let (Some(a), Some(b)) = (r.find('['), r.rfind(']')) {
                cats = r
                    .get(a + 1..b)
                    .unwrap_or("")
                    .split(',')
                    .map(|c| String::from(unquote(c)))
                    .collect();
            }
        } else if let Some(r) = l.strip_prefix("y-axis") {
            let r = r.trim();
            let (name, rest) = if let Some(q) = r.strip_prefix('"') {
                let e = q.find('"').unwrap_or(q.len());
                (q.get(..e).unwrap_or(""), q.get(e + 1..).unwrap_or(""))
            } else {
                ("", r)
            };
            y_name = String::from(name);
            if let Some((a, b)) = rest.split_once("-->") {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
                    y_range = Some((a, b));
                }
            }
        } else if l.starts_with("bar") {
            bars.push(nums(l));
        } else if l.starts_with("line") {
            lines.push(nums(l));
        }
    }
    let n = bars
        .iter()
        .chain(lines.iter())
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(cats.len());
    if n == 0 {
        return None;
    }
    let all: Vec<f32> = bars.iter().chain(lines.iter()).flatten().copied().collect();
    let (lo, hi) = y_range.unwrap_or_else(|| {
        let lo = all.iter().copied().fold(f32::MAX, f32::min).min(0.0);
        let hi = all.iter().copied().fold(f32::MIN, f32::max);
        (lo, hi)
    });
    let step = nice_step(hi - lo);
    let ticks: Vec<f32> = (0..64)
        .map(|k| (lo / step).ceil() * step + k as f32 * step)
        .take_while(|v| *v <= hi + step * 0.01)
        .collect();
    // Obsidian's: about 580 by 390 at 16 px, ticks every step.
    let tick_w = ticks
        .iter()
        .map(|t| width(&num_label(*t), size * 0.85))
        .fold(0.0, f32::max);
    let left = size * 2.5 + tick_w + if y_name.is_empty() { 0.0 } else { size * 1.0 };
    let top = if title.is_empty() { size } else { size * 3.0 };
    let (pw, ph) = (size * 36.0, size * 24.0);
    let mut items = Vec::new();
    if !title.is_empty() {
        items.push(text(
            left + pw * 0.5,
            size * 1.6,
            &title,
            size * 1.0,
            1,
            true,
            Role::Text,
        ));
    }
    let y_of = |v: f32| top + ph - (v - lo) / (hi - lo).max(1e-6) * ph;
    for t in &ticks {
        let y = y_of(*t);
        items.push(Item::Line {
            pts: alloc::vec![(left, y), (left + pw, y)],
            role: Role::Grid,
            width: 1.0,
            dashed: false,
            head: Head::None,
            tail: Head::None,
        });
        items.push(text(
            left - size * 0.5,
            y + size * 0.3,
            &num_label(*t),
            size * 0.85,
            2,
            false,
            Role::Text,
        ));
    }
    items.push(line(
        alloc::vec![(left, top), (left, top + ph), (left + pw, top + ph)],
        false,
        Head::None,
    ));
    if !y_name.is_empty() {
        // Mermaid turns this on its side; Krate cannot turn text (K-1341).
        items.push(text(
            size * 0.2,
            top + ph * 0.5,
            &y_name,
            size * 0.8,
            0,
            false,
            Role::Text,
        ));
    }
    let slot = pw / n as f32;
    for (k, c) in cats.iter().enumerate() {
        items.push(text(
            left + slot * (k as f32 + 0.5),
            top + ph + size * 1.3,
            c,
            size * 0.85,
            1,
            false,
            Role::Text,
        ));
    }
    let nb = bars.len().max(1) as f32;
    for (bi, b) in bars.iter().enumerate() {
        let bw = slot * 0.7 / nb;
        for (k, v) in b.iter().enumerate() {
            let x = left + slot * k as f32 + slot * 0.15 + bw * bi as f32;
            let y = y_of(*v);
            items.push(bx(x, y, bw, top + ph - y, 0.0, Role::Series(bi as u8)));
            if let Some(Item::Box { stroke, .. }) = items.last_mut() {
                *stroke = Role::None;
            }
        }
    }
    for (li, l) in lines.iter().enumerate() {
        let role = Role::Series((bars.len() + li) as u8);
        let pts: Vec<(f32, f32)> = l
            .iter()
            .enumerate()
            .map(|(k, v)| (left + slot * (k as f32 + 0.5), y_of(*v)))
            .collect();
        for p in &pts {
            items.push(Item::Circle {
                cx: p.0,
                cy: p.1,
                r: size * 0.18,
                fill: role,
                stroke: Role::None,
            });
        }
        items.push(Item::Line {
            pts,
            role,
            width: 1.6,
            dashed: false,
            head: Head::None,
            tail: Head::None,
        });
    }
    Some(Chart {
        w: left + pw + size,
        h: top + ph + size * 2.5,
        items,
    })
}

// ---- sankey-beta ----------------------------------------------------------------------------

pub fn sankey(src: &str, width: Measure, size: f32) -> Option<Chart> {
    let mut names: Vec<String> = Vec::new();
    let mut flows: Vec<(usize, usize, f32)> = Vec::new();
    let id = |names: &mut Vec<String>, s: &str| -> usize {
        let s = unquote(s);
        if let Some(i) = names.iter().position(|n| n == s) {
            return i;
        }
        names.push(String::from(s));
        names.len() - 1
    };
    for l in lines_of(src).into_iter().skip(1) {
        let f = quoted_args(l);
        if f.len() == 3 {
            if let Ok(v) = f[2].trim().parse::<f32>() {
                let a = id(&mut names, &f[0]);
                let b = id(&mut names, &f[1]);
                flows.push((a, b, v));
            }
        }
    }
    if flows.is_empty() {
        return None;
    }
    let n = names.len();
    // Columns: the longest path in; the ends pushed to the last column.
    let mut col = alloc::vec![0usize; n];
    for _ in 0..n {
        for &(a, b, _) in &flows {
            if col[b] < col[a] + 1 {
                col[b] = col[a] + 1;
            }
        }
    }
    let last = col.iter().copied().max().unwrap_or(0);
    for (i, c) in col.iter_mut().enumerate() {
        if !flows.iter().any(|f| f.0 == i) {
            *c = last;
        }
    }
    let value: Vec<f32> = (0..n)
        .map(|i| {
            let inn: f32 = flows.iter().filter(|f| f.1 == i).map(|f| f.2).sum();
            let out: f32 = flows.iter().filter(|f| f.0 == i).map(|f| f.2).sum();
            inn.max(out)
        })
        .collect();
    // Obsidian's: 600 by 400, 10 px nodes, 10 px between them.
    let (cw, ch) = (size * 37.5, size * 25.0);
    let node_w = size * 0.6;
    let pad = size * 0.6;
    let mut per_col: Vec<Vec<usize>> = alloc::vec![Vec::new(); last + 1];
    for i in 0..n {
        per_col[col[i]].push(i);
    }
    let k = per_col
        .iter()
        .map(|c| {
            let tot: f32 = c.iter().map(|i| value[*i]).sum();
            (ch - pad * (c.len().saturating_sub(1)) as f32) / tot.max(1e-6)
        })
        .fold(f32::MAX, f32::min);
    let mut ny = alloc::vec![0.0f32; n];
    for c in &per_col {
        let tot: f32 =
            c.iter().map(|i| value[*i] * k).sum::<f32>() + pad * (c.len().saturating_sub(1)) as f32;
        let mut y = (ch - tot) * 0.5;
        for &i in c {
            ny[i] = y;
            y += value[i] * k + pad;
        }
    }
    // Relax, as d3-sankey does: each node toward where its flows meet the
    // nodes they join, then the column kept apart and inside the chart.
    for _ in 0..6 {
        let mut out_at = ny.clone();
        let mut in_at = ny.clone();
        let mut ports: Vec<(f32, f32)> = Vec::new();
        for &(a, b, v) in &flows {
            ports.push((out_at[a], in_at[b]));
            out_at[a] += v * k;
            in_at[b] += v * k;
        }
        let mut want = ny.clone();
        for i in 0..n {
            let (mut sum, mut wsum) = (0.0f32, 0.0f32);
            for (fi, &(a, b, v)) in flows.iter().enumerate() {
                let (pa, pb) = ports[fi];
                if a == i {
                    sum += (ny[i] + (pb - (pa - ny[i])) - ny[i]) * v;
                    wsum += v;
                } else if b == i {
                    sum += (ny[i] + (pa - (pb - ny[i])) - ny[i]) * v;
                    wsum += v;
                }
            }
            if wsum > 0.0 {
                want[i] = ny[i] + (sum / wsum - ny[i]) * 0.5;
            }
        }
        for c in &per_col {
            let mut order: Vec<usize> = c.clone();
            order.sort_by(|a, b| {
                want[*a]
                    .partial_cmp(&want[*b])
                    .unwrap_or(core::cmp::Ordering::Equal)
            });
            let mut y = 0.0f32;
            for &i in &order {
                ny[i] = want[i].max(y);
                y = ny[i] + value[i] * k + pad;
            }
            let over = (y - pad) - ch;
            if over > 0.0 {
                let mut bottom = ch;
                for &i in order.iter().rev() {
                    ny[i] = ny[i].min(bottom - value[i] * k);
                    bottom = ny[i] - pad;
                }
            }
        }
    }
    let nx = |i: usize| (cw - node_w) * col[i] as f32 / last.max(1) as f32;
    let mut items = Vec::new();
    let mut out_at = ny.clone();
    let mut in_at = ny.clone();
    for &(a, b, v) in &flows {
        let t = v * k;
        let (x0, x1) = (nx(a) + node_w, nx(b));
        let (ya, yb) = (out_at[a], in_at[b]);
        out_at[a] += t;
        in_at[b] += t;
        let mx = (x0 + x1) * 0.5;
        let top = crate::layered::basis(&[(x0, ya), (mx, ya), (mx, yb), (x1, yb)]);
        let mut bot =
            crate::layered::basis(&[(x0, ya + t), (mx, ya + t), (mx, yb + t), (x1, yb + t)]);
        bot.reverse();
        let mut pts = top;
        pts.extend(bot);
        if let Some(f) = pts.first().copied() {
            pts.push(f);
        }
        items.push(Item::Poly {
            pts,
            fill: Role::Band(a as u8),
            stroke: Role::None,
        });
    }
    for i in 0..n {
        let h = value[i] * k;
        items.push(bx(
            nx(i),
            ny[i],
            node_w,
            h.max(1.0),
            0.0,
            Role::Series(i as u8),
        ));
        if let Some(Item::Box { stroke, .. }) = items.last_mut() {
            *stroke = Role::None;
        }
        let label = alloc::format!("{} {}", names[i], num_label(value[i]));
        let (lx, anchor) = if col[i] == last {
            (nx(i) - size * 0.4, 2)
        } else {
            (nx(i) + node_w + size * 0.4, 0)
        };
        let _ = width;
        items.push(text(
            lx,
            ny[i] + h * 0.5 + size * 0.35,
            &label,
            size * 0.9,
            anchor,
            false,
            Role::Text,
        ));
    }
    Some(Chart {
        w: cw,
        h: ch,
        items,
    })
}

// ---- block-beta -----------------------------------------------------------------------------

pub fn block(src: &str, width: Measure, size: f32) -> Option<Chart> {
    let mut cols = 0usize;
    let mut blocks: Vec<(String, String, usize)> = Vec::new();
    let mut arrows: Vec<(String, String)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(c) = l.strip_prefix("columns") {
            cols = c.trim().parse().unwrap_or(0);
            continue;
        }
        if let Some((a, b)) = l.split_once("-->") {
            arrows.push((String::from(a.trim()), String::from(b.trim())));
            continue;
        }
        // Blocks on a line: id, id["label"], id:2, id["label"]:2.
        let mut rest = l;
        while !rest.trim().is_empty() {
            rest = rest.trim_start();
            let id_end = rest
                .find(|c: char| c.is_whitespace() || c == '[' || c == '(' || c == ':')
                .unwrap_or(rest.len());
            let id = String::from(rest.get(..id_end).unwrap_or(""));
            rest = rest.get(id_end..).unwrap_or("");
            let mut label = id.clone();
            if let Some(r) = rest.strip_prefix('[').or_else(|| rest.strip_prefix('(')) {
                let r = r.trim_start_matches(['[', '(']);
                let e = r.find([']', ')']).unwrap_or(r.len());
                label = String::from(unquote(r.get(..e).unwrap_or("")));
                rest = r.get(e..).unwrap_or("").trim_start_matches([']', ')']);
            }
            let mut span = 1;
            if let Some(r) = rest.strip_prefix(':') {
                let e = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
                span = r.get(..e).and_then(|d| d.parse().ok()).unwrap_or(1).max(1);
                rest = r.get(e..).unwrap_or("");
            }
            if !id.is_empty() && id != "space" {
                blocks.push((id, label, span));
            } else if id == "space" {
                blocks.push((String::new(), String::new(), span));
            }
        }
    }
    if blocks.is_empty() {
        return None;
    }
    let cols = if cols == 0 {
        blocks.iter().map(|b| b.2).sum()
    } else {
        cols
    };
    // Obsidian's: 77 px cells at 16 px, 10 px apart, 40 high.
    let cell = blocks
        .iter()
        .filter(|b| b.2 == 1)
        .map(|b| width(&b.1, size) + size * 2.5)
        .fold(size * 4.8, f32::max);
    let gap = size * 0.6;
    let h = size * 2.5;
    let mut items = Vec::new();
    let (mut c, mut r) = (0usize, 0usize);
    let mut at: Vec<(String, f32, f32, f32)> = Vec::new();
    for (id, label, span) in &blocks {
        let span = (*span).min(cols);
        if c + span > cols {
            c = 0;
            r += 1;
        }
        let x = c as f32 * (cell + gap);
        let y = r as f32 * (h + gap);
        let w = span as f32 * cell + (span - 1) as f32 * gap;
        if !id.is_empty() {
            items.push(bx(x, y, w, h, 3.0, Role::Node));
            items.push(text(
                x + w * 0.5,
                y + h * 0.5 + size * 0.36,
                label,
                size,
                1,
                false,
                Role::Text,
            ));
            at.push((id.clone(), x + w * 0.5, y + h * 0.5, w));
        }
        c += span;
        if c >= cols {
            c = 0;
            r += 1;
        }
    }
    for (a, b) in arrows {
        let (Some(p), Some(q)) = (at.iter().find(|t| t.0 == a), at.iter().find(|t| t.0 == b))
        else {
            continue;
        };
        items.push(line(
            alloc::vec![(p.1, p.2), (q.1, q.2)],
            false,
            Head::Arrow,
        ));
    }
    let rows = if c == 0 { r } else { r + 1 };
    Some(Chart {
        w: cols as f32 * (cell + gap) - gap,
        h: rows as f32 * (h + gap) - gap,
        items,
    })
}

// ---- requirementDiagram ---------------------------------------------------------------------

pub fn requirement(src: &str, width: Measure, size: f32) -> Option<Chart> {
    let mut nodes: Vec<GNode> = Vec::new();
    let mut edges: Vec<GEdge> = Vec::new();
    let mut open: Option<usize> = None;
    let kinds = [
        ("functionalRequirement", "«Functional Requirement»"),
        ("interfaceRequirement", "«Interface Requirement»"),
        ("performanceRequirement", "«Performance Requirement»"),
        ("physicalRequirement", "«Physical Requirement»"),
        ("designConstraint", "«Design Constraint»"),
        ("requirement", "«Requirement»"),
        ("element", "«Element»"),
    ];
    let field = |k: &str| -> &'static str {
        match k {
            "id" => "ID",
            "text" => "Text",
            "risk" => "Risk",
            "verifymethod" => "Verification",
            "type" => "Type",
            "docref" => "Doc Ref",
            _ => "",
        }
    };
    let title_case = |v: &str| -> String {
        let mut c = v.chars();
        match c.next() {
            Some(f) => {
                let mut s: String = f.to_uppercase().collect();
                s.push_str(c.as_str());
                s
            }
            None => String::new(),
        }
    };
    for l in lines_of(src).into_iter().skip(1) {
        if l == "}" {
            open = None;
            continue;
        }
        if let Some(i) = open {
            if let Some((k, v)) = l.split_once(':') {
                let name = field(k.trim());
                if !name.is_empty() {
                    let v = unquote(v);
                    let v = if matches!(k.trim(), "risk" | "verifymethod") {
                        title_case(v)
                    } else {
                        String::from(v)
                    };
                    nodes[i].rows.push(alloc::format!("{name}: {v}"));
                }
            }
            continue;
        }
        if let Some((kw, stereo)) = kinds.iter().find(|(kw, _)| l.starts_with(kw)) {
            let name = l
                .get(kw.len()..)
                .unwrap_or("")
                .trim()
                .trim_end_matches('{')
                .trim();
            let i = crate::charts::find_or_add(&mut nodes, unquote(name));
            nodes[i].stereo = String::from(*stereo);
            if l.ends_with('{') {
                open = Some(i);
            }
            continue;
        }
        // a - satisfies -> b, or b <- copies - a
        if let Some((left, b)) = l.split_once("->") {
            if let Some((a, rel)) = left.split_once(" - ") {
                let ia = crate::charts::find_or_add(&mut nodes, a.trim());
                let ib = crate::charts::find_or_add(&mut nodes, b.trim());
                edges.push(GEdge {
                    a: ia,
                    b: ib,
                    label: alloc::format!("«{}»", rel.trim()),
                    dashed: true,
                    head: Head::Arrow,
                    tail: Head::None,
                    a_card: String::new(),
                    b_card: String::new(),
                });
            }
        } else if let Some((b, right)) = l.split_once("<-") {
            if let Some((rel, a)) = right.split_once(" - ") {
                let ia = crate::charts::find_or_add(&mut nodes, a.trim());
                let ib = crate::charts::find_or_add(&mut nodes, b.trim());
                edges.push(GEdge {
                    a: ia,
                    b: ib,
                    label: alloc::format!("«{}»", rel.trim()),
                    dashed: true,
                    head: Head::Arrow,
                    tail: Head::None,
                    a_card: String::new(),
                    b_card: String::new(),
                });
            }
        }
    }
    if nodes.is_empty() {
        return None;
    }
    Some(graph_chart(&nodes, &edges, width, size, false))
}

// ---- C4 -------------------------------------------------------------------------------------

pub fn c4(src: &str, width: Measure, size: f32) -> Option<Chart> {
    struct Shape {
        alias: String,
        name: String,
        descr: String,
        stereo: &'static str,
        person: bool,
        ext: bool,
    }
    let mut title = String::new();
    let mut shapes: Vec<Shape> = Vec::new();
    let mut rels: Vec<(String, String, String)> = Vec::new();
    for l in lines_of(src).into_iter().skip(1) {
        if let Some(t) = after(l, "title") {
            title = String::from(t);
            continue;
        }
        let Some(open) = l.find('(') else {
            continue;
        };
        let kw = l.get(..open).unwrap_or("").trim();
        let args = quoted_args(
            l.get(open + 1..l.rfind(')').unwrap_or(l.len()))
                .unwrap_or(""),
        );
        let arg = |i: usize| {
            args.get(i)
                .map(|s| String::from(unquote(s)))
                .unwrap_or_default()
        };
        let shape = match kw {
            "Person" => Some(("«person»", true, false)),
            "Person_Ext" => Some(("«external person»", true, true)),
            "System" => Some(("«system»", false, false)),
            "System_Ext" => Some(("«external system»", false, true)),
            "SystemDb" => Some(("«system_db»", false, false)),
            "SystemDb_Ext" => Some(("«external system_db»", false, true)),
            "Container" => Some(("«container»", false, false)),
            "ContainerDb" => Some(("«container_db»", false, false)),
            "Component" => Some(("«component»", false, false)),
            _ => None,
        };
        if let Some((stereo, person, ext)) = shape {
            shapes.push(Shape {
                alias: arg(0),
                name: arg(1),
                descr: arg(2),
                stereo,
                person,
                ext,
            });
        } else if kw.starts_with("Rel") || kw == "BiRel" {
            rels.push((arg(0), arg(1), arg(2)));
        }
    }
    if shapes.is_empty() {
        return None;
    }
    // Obsidian's: 206 by 110 boxes, two to a row here, 30 px apart, a
    // person with a head over its box.
    let (bw, bh) = (size * 12.9, size * 6.9);
    let gap = size * 2.2;
    let per_row = 2usize;
    let top = if title.is_empty() {
        size * 2.0
    } else {
        size * 4.0
    };
    let mut items = Vec::new();
    if !title.is_empty() {
        let w = per_row as f32 * (bw + gap) - gap;
        items.push(text(
            w * 0.5,
            size * 1.6,
            &title,
            size * 1.05,
            1,
            true,
            Role::Text,
        ));
    }
    let mut at: Vec<(f32, f32)> = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        let (c, r) = (i % per_row, i / per_row);
        let x = c as f32 * (bw + gap);
        let y = top + r as f32 * (bh + gap * 2.5);
        items.push(Item::Box {
            x,
            y,
            w: bw,
            h: bh,
            r: if s.person { size * 0.7 } else { 2.0 },
            fill: if s.ext { Role::None } else { Role::Node },
            stroke: Role::Line,
            dashed: s.ext,
        });
        if s.person {
            items.push(Item::Circle {
                cx: x + bw * 0.5,
                cy: y - size * 0.1,
                r: size * 0.8,
                fill: Role::Node,
                stroke: Role::Line,
            });
        }
        let cx = x + bw * 0.5;
        items.push(text(
            cx,
            y + size * 1.6,
            s.stereo,
            size * 0.8,
            1,
            false,
            Role::Text,
        ));
        items.push(text(
            cx,
            y + size * 3.0,
            &s.name,
            size * 1.05,
            1,
            true,
            Role::Text,
        ));
        if !s.descr.is_empty() {
            items.push(text(
                cx,
                y + size * 4.6,
                &s.descr,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
        }
        at.push((x, y));
    }
    let find = |a: &str| shapes.iter().position(|s| s.alias == a);
    for (a, b, label) in rels {
        let (Some(i), Some(j)) = (find(&a), find(&b)) else {
            continue;
        };
        let ca = (at[i].0 + bw * 0.5, at[i].1 + bh * 0.5);
        let cb = (at[j].0 + bw * 0.5, at[j].1 + bh * 0.5);
        let clip = |c: (f32, f32), to: (f32, f32)| {
            let (dx, dy) = (to.0 - c.0, to.1 - c.1);
            let tx = if dx.abs() > 1e-3 {
                bw * 0.5 / dx.abs()
            } else {
                f32::MAX
            };
            let ty = if dy.abs() > 1e-3 {
                bh * 0.5 / dy.abs()
            } else {
                f32::MAX
            };
            let t = tx.min(ty).min(1.0);
            (c.0 + dx * t, c.1 + dy * t)
        };
        let (p, q) = (clip(ca, cb), clip(cb, ca));
        items.push(line(alloc::vec![p, q], false, Head::Arrow));
        if !label.is_empty() {
            let (mx, my) = ((p.0 + q.0) * 0.5, (p.1 + q.1) * 0.5);
            let w = width(&label, size * 0.85) + size * 0.6;
            items.push(Item::Box {
                x: mx - w * 0.5,
                y: my - size * 0.7,
                w,
                h: size * 1.4,
                r: 0.0,
                fill: Role::Paper,
                stroke: Role::None,
                dashed: false,
            });
            items.push(text(
                mx,
                my + size * 0.3,
                &label,
                size * 0.85,
                1,
                false,
                Role::Text,
            ));
        }
    }
    let rows = shapes.len().div_ceil(per_row);
    Some(Chart {
        w: per_row.min(shapes.len()) as f32 * (bw + gap) - gap,
        h: top + rows as f32 * (bh + gap * 2.5) - gap * 2.5 + size,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str, size: f32) -> f32 {
        s.chars().count() as f32 * size * 0.5
    }

    fn texts(c: &Chart) -> Vec<String> {
        c.items
            .iter()
            .filter_map(|i| match i {
                Item::Text { s, .. } => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_newer_kind_draws() {
        let g = git("gitGraph\ncommit\nbranch develop\ncheckout develop\ncommit id: \"fix\"\ncheckout main\nmerge develop\ncommit tag: \"v1\"", &w, 16.0).unwrap();
        let t = texts(&g);
        assert!(t.contains(&String::from("main")) && t.contains(&String::from("develop")));
        assert!(t.contains(&String::from("fix")) && t.contains(&String::from("v1")));
        let q = quadrant(
            "quadrantChart\ntitle T\nx-axis Low --> High\nquadrant-1 Expand\nA: [0.3, 0.6]",
            &w,
            16.0,
        )
        .unwrap();
        assert!(texts(&q).contains(&String::from("Expand")));
        let x = xy("xychart-beta\ntitle \"Sales\"\nx-axis [jan, feb]\ny-axis \"Revenue\" 0 --> 100\nbar [10, 50]\nline [10, 50]", &w, 16.0).unwrap();
        assert!(texts(&x).contains(&String::from("feb")));
        assert!(texts(&x).contains(&String::from("100")));
        let s = sankey("sankey-beta\nA,B,10\nB,C,4\nB,D,6", &w, 16.0).unwrap();
        assert!(texts(&s).contains(&String::from("B 10")));
        let b = block(
            "block-beta\ncolumns 3\na[\"In\"] b c\nd[\"Logs\"]:3",
            &w,
            16.0,
        )
        .unwrap();
        assert!(texts(&b).contains(&String::from("Logs")));
        let r = requirement("requirementDiagram\nrequirement r {\nid: 1\nrisk: high\n}\nelement e {\ntype: sim\n}\ne - satisfies -> r", &w, 16.0).unwrap();
        let rt = texts(&r);
        assert!(
            rt.contains(&String::from("«Requirement»")) && rt.contains(&String::from("Risk: High"))
        );
        assert!(rt.contains(&String::from("«satisfies»")));
        let c = c4("C4Context\ntitle Ctx\nPerson(c, \"Customer\", \"A user\")\nSystem(s, \"Shop\", \"Sells\")\nRel(c, s, \"Uses\")", &w, 16.0).unwrap();
        let ct = texts(&c);
        assert!(ct.contains(&String::from("«person»")) && ct.contains(&String::from("Uses")));
    }
}
