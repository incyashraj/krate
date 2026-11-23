//! Ports diagrams/git/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::db::{GitCommit, GitModel, CHERRY_PICK, HIGHLIGHT, MERGE, REVERSE};
use crate::json::Value;
use crate::pele::common::fit::plain_label;
use crate::pele::common::fit_width::turn_to_fit;
use crate::pele::core::{RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{series_color, ROUNDED};
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};

const DOT: f64 = 7.0;
const STEP: f64 = 44.0;
const LANE: f64 = 44.0;
/// Shorter than a step, so a curve between lanes never runs through a commit on a lane it crosses.
const CURVE: f64 = 36.0;
const GAP: f64 = 8.0;
const LABEL_GAP: f64 = 20.0;
const TAIL: f64 = 20.0;
const NOTCH: f64 = 7.0;
const TAG_GAP: f64 = 6.0;
const HALF: f64 = core::f64::consts::FRAC_1_SQRT_2;
const CLASSES: [&str; 5] = ["normal", "reverse", "highlight", "merge", "cherry-pick"];

/// Ports `interface View`.
struct View<'a> {
    commit: &'a GitCommit,
    index: usize,
    lane: usize,
    m: f64,
    label: Option<Label>,
    tags: Vec<Label>,
}

/// Ports `flag`.
fn flag(config: &Config, key: &str, fallback: bool) -> bool {
    match config.get("gitGraph").and_then(|g| g.get(key)) {
        Some(Value::Bool(b)) => *b,
        _ => fallback,
    }
}

/// Ports `tagWidth`.
fn tag_width(tag: &Label) -> f64 {
    NOTCH + 4.0 + tag.width + 6.0
}

/// Ports `symbol`.
fn symbol(kind: u8, x: f64, y: f64, color: &str) -> String {
    let at = format!("cx=\"{}\" cy=\"{}\"", num(x), num(y));
    if kind == HIGHLIGHT {
        return format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\" fill=\"{}\"/>",
            num(x - DOT - 1.0),
            num(y - DOT - 1.0),
            num_str(2.0 * DOT + 2.0),
            num_str(2.0 * DOT + 2.0),
            color
        );
    }
    if kind == MERGE {
        return format!(
            "<circle {} r=\"{}\" fill=\"var(--_bg)\" stroke=\"{}\" stroke-width=\"2\"/><circle {} r=\"2.5\" fill=\"{}\"/>",
            at,
            num_str(DOT - 1.0),
            color,
            at,
            color
        );
    }
    let dot = format!("<circle {} r=\"{}\" fill=\"{}\"/>", at, num_str(DOT), color);
    if kind == REVERSE {
        return dot
            + &format!(
                "<path d=\"M{},{}l6,6M{},{}l6,-6\" fill=\"none\" stroke=\"var(--_bg)\" stroke-width=\"1.5\" stroke-linecap=\"round\"/>",
                num(x - 3.0),
                num(y - 3.0),
                num(x - 3.0),
                num(y + 3.0)
            );
    }
    if kind == CHERRY_PICK {
        return dot
            + &format!(
                "<path d=\"M{},{}L{},{}L{},{}\" fill=\"none\" stroke=\"var(--_bg)\" stroke-linejoin=\"round\"/><circle cx=\"{}\" cy=\"{}\" r=\"1.8\" fill=\"var(--_bg)\"/><circle cx=\"{}\" cy=\"{}\" r=\"1.8\" fill=\"var(--_bg)\"/>",
                num(x - 2.6),
                num(y + 1.0),
                num(x + 0.8),
                num(y - 4.2),
                num(x + 2.6),
                num(y + 1.0),
                num(x - 2.6),
                num(y + 2.2),
                num(x + 2.6),
                num(y + 2.2)
            );
    }
    dot
}

/// Ports `renderGit`.
pub fn render_git(
    model: &GitModel,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    if model.direction != "LR" {
        return Ok(draw(model, config, options, palette, model.direction));
    }
    turn_to_fit(options, |down| -> Result<Rendered, PeleError> {
        Ok(draw(
            model,
            config,
            options,
            palette,
            if down { "TB" } else { "LR" },
        ))
    })
}

/// Ports `draw`.
fn draw(
    model: &GitModel,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
    direction: &str,
) -> Rendered {
    let size = options.font_size.unwrap_or(16.0);
    let small = jround(size * 0.75);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let icons = options.icons;
    let pad = options.padding.unwrap_or(8.0);

    let show_branches = flag(config, "showBranches", true);
    let show_commit_label = flag(config, "showCommitLabel", true);
    let rotate = flag(config, "rotateCommitLabel", true);
    let parallel = flag(config, "parallelCommits", false);
    let vertical = direction != "LR";
    let flip = direction == "BT";

    let lane_of = |name: &str| -> usize { model.lanes.iter().position(|l| l == name).unwrap_or(0) };
    let lane_count = model.lanes.len();
    let medium = jround(size * 0.875);
    // Mermaid writes branch names, tags, and commit ids as plain text, so tags in them show as written.
    let names: Vec<Label> = model
        .lanes
        .iter()
        .map(|name| {
            plain_label(
                Some(if show_branches { name } else { "" }),
                measurer,
                medium,
                4000.0,
                0,
                icons,
            )
        })
        .collect();

    let mut sorted: Vec<&GitCommit> = model.commits.iter().map(|(_, c)| c).collect();
    sorted.sort_by_key(|c| c.seq);
    let mut views: Vec<View> = Vec::new();
    for commit in sorted {
        let labelled = show_commit_label
            && commit.kind != CHERRY_PICK
            && (commit.kind != MERGE || commit.custom_id == Some(true));
        let mut tags: Vec<Label> = Vec::new();
        for tag in &commit.tags {
            let label = plain_label(Some(tag), measurer, small, 4000.0, 0, icons);
            if !label.lines.is_empty() {
                tags.push(label);
            }
        }
        let index = views.len();
        views.push(View {
            commit,
            index,
            lane: lane_of(&commit.branch),
            m: 0.0,
            label: if labelled {
                Some(plain_label(
                    Some(&commit.id),
                    measurer,
                    small,
                    4000.0,
                    0,
                    icons,
                ))
            } else {
                None
            },
            tags,
        });
    }
    let by_id = |views: &[View], id: &str| -> Option<usize> {
        views.iter().rposition(|v| v.commit.id == id)
    };

    // Room each commit needs: along the main axis on either side, and across it towards the
    // previous lane (`lo`) and the next one (`hi`). Tags sit on one side of a lane and ids on the other.
    let mut half = alloc::vec![0.0f64; views.len()];
    let mut tag_half = alloc::vec![0.0f64; views.len()];
    let mut lo = alloc::vec![DOT; lane_count];
    let mut hi = alloc::vec![DOT; lane_count];
    for (k, view) in views.iter().enumerate() {
        let mut tags_main: f64 = 0.0;
        let mut tags_cross = 0.0;
        for tag in &view.tags {
            let w = tag_width(tag);
            if vertical {
                tags_main = jmax(tags_main, tag.height);
                tags_cross += w + 4.0;
            } else {
                tags_main = jmax(tags_main, w);
                tags_cross += tag.height + 3.0;
            }
        }
        let mut label_main = 0.0;
        let mut label_cross = 0.0;
        if let Some(label) = view.label.as_ref().filter(|l| !l.lines.is_empty()) {
            if rotate {
                label_cross = 3.0 + HALF * (label.width + label.height);
            } else {
                label_main = if vertical { label.height } else { label.width };
                label_cross = 6.0 + if vertical { label.width } else { label.height };
            }
        }
        half[k] = jmax(jmax(DOT, tags_main / 2.0), label_main / 2.0);
        tag_half[k] = tags_main / 2.0;
        let tag_side = DOT
            + if !view.tags.is_empty() {
                TAG_GAP + tags_cross
            } else {
                0.0
            };
        let label_side = DOT + label_cross;
        let lane = view.lane;
        lo[lane] = jmax(lo[lane], if vertical { label_side } else { tag_side });
        hi[lane] = jmax(hi[lane], if vertical { tag_side } else { label_side });
    }

    // In a left-to-right graph the tags stand over their commit. A connector that comes down from
    // a lane above joins the commit's lane this far before the commit, clear of the tags.
    let mut lead = alloc::vec![0.0f64; views.len()];
    if !vertical {
        for (k, view) in views.iter().enumerate() {
            if tag_half[k] == 0.0 {
                continue;
            }
            for id in &view.commit.parents {
                if let Some(p) = by_id(&views, id) {
                    if views[p].lane < view.lane {
                        lead[k] = tag_half[k] + 4.0;
                    }
                }
            }
        }
    }
    let space = |p: usize, k: usize| -> f64 {
        jmax(
            jmax(STEP, half[p] + half[k] + GAP),
            if lead[k] > 0.0 {
                half[p] + CURVE + lead[k]
            } else {
                0.0
            },
        )
    };

    for k in 0..views.len() {
        if !parallel {
            views[k].m = if k == 0 {
                0.0
            } else {
                views[k - 1].m + space(k - 1, k)
            };
            continue;
        }
        // Mermaid's `parallelCommits`: a commit sits one step after its furthest parent.
        let mut m: f64 = 0.0;
        for id in &views[k].commit.parents {
            let Some(p) = by_id(&views, id) else {
                continue;
            };
            if views[p].index >= k {
                continue;
            }
            m = jmax(m, views[p].m + space(views[p].index, k));
        }
        views[k].m = m;
    }
    let mut end: f64 = 0.0;
    for view in &views {
        end = jmax(end, view.m);
    }

    let mut cross = alloc::vec![0.0f64; lane_count];
    for i in 0..lane_count {
        let name = &names[i];
        let reach = if name.lines.is_empty() {
            0.0
        } else if vertical {
            name.width / 2.0 + 8.0
        } else {
            name.height / 2.0 + 2.0
        };
        lo[i] = jmax(lo[i], reach);
        hi[i] = jmax(hi[i], reach);
        if i > 0 {
            cross[i] = cross[i - 1] + jmax(LANE, hi[i - 1] + lo[i] + GAP);
        }
    }

    let px = |m: f64, c: f64| -> f64 {
        if vertical {
            c
        } else {
            m
        }
    };
    let py = |m: f64, c: f64| -> f64 {
        if vertical {
            if flip {
                -m
            } else {
                m
            }
        } else {
            c
        }
    };
    let point = |m: f64, c: f64| -> String { format!("{},{}", num(px(m, c)), num(py(m, c))) };

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut grow = |x0: f64, y0: f64, x1: f64, y1: f64| {
        if x0 < min_x {
            min_x = x0;
        }
        if y0 < min_y {
            min_y = y0;
        }
        if x1 > max_x {
            max_x = x1;
        }
        if y1 > max_y {
            max_y = y1;
        }
    };

    let mut branches = String::new();
    if show_branches {
        for i in 0..lane_count {
            let name = &names[i];
            let color = series_color(i as i64);
            let c = cross[i];
            let w = name.width + 16.0;
            let h = name.height + 4.0;
            let along = if vertical { h } else { w };
            let from = if name.lines.is_empty() {
                -LABEL_GAP
            } else {
                -LABEL_GAP - along
            };
            let cx = px(from + along / 2.0, c);
            let cy = py(from + along / 2.0, c);
            grow(
                px(-LABEL_GAP, c),
                py(-LABEL_GAP, c),
                px(-LABEL_GAP, c),
                py(-LABEL_GAP, c),
            );
            grow(
                px(end + TAIL, c),
                py(end + TAIL, c),
                px(end + TAIL, c),
                py(end + TAIL, c),
            );
            branches.push_str(&format!(
                "<g class=\"pele-branch\" data-id=\"{}\"><path class=\"pele-lane\" d=\"M{}L{}\" stroke=\"{}\" stroke-dasharray=\"2 4\"/>",
                esc_text(&model.lanes[i]),
                point(-LABEL_GAP, c),
                point(end + TAIL, c),
                color
            ));
            if !name.lines.is_empty() {
                grow(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
                let ink = palette.series_ink_fill(i as i64);
                branches.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"{}/>{}",
                    num(cx - w / 2.0),
                    num(cy - h / 2.0),
                    num(w),
                    num(h),
                    color,
                    ROUNDED,
                    label_svg(
                        name,
                        cx,
                        cy,
                        &(String::from(" class=\"pele-label\"")
                            + if ink.is_empty() {
                                " fill=\"var(--_bg)\""
                            } else {
                                &ink
                            }),
                        None
                    )
                ));
            }
            branches.push_str("</g>");
        }
    }

    // Positions of the commits on each lane, to tell whether a connector may run along it.
    let mut taken: Vec<Vec<f64>> = (0..lane_count).map(|_| Vec::new()).collect();
    for view in &views {
        taken[view.lane].push(view.m);
    }
    if parallel {
        for list in taken.iter_mut() {
            list.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        }
    }
    let busy = |lane: usize, from: f64, to: f64| -> bool {
        let list = &taken[lane];
        let mut low = 0;
        let mut high = list.len();
        while low < high {
            let mid = (low + high) >> 1;
            if list[mid] <= from {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low < list.len() && list[low] < to
    };

    let mut curves = String::new();
    let mut lines = String::new();
    for view in &views {
        let commit = view.commit;
        let c2 = cross[view.lane];
        let m2 = view.m;
        for (j, pid) in commit.parents.iter().enumerate() {
            let Some(p) = by_id(&views, pid) else {
                continue;
            };
            if p == view.index {
                continue;
            }
            let parent = &views[p];
            let c1 = cross[parent.lane];
            let m1 = parent.m;
            if parent.lane == view.lane {
                lines.push_str(&format!(
                    "<path class=\"pele-edge\" d=\"M{}L{}\" stroke=\"{}\"/>",
                    point(m1, c1),
                    point(m2, c2),
                    series_color(view.lane as i64)
                ));
                continue;
            }
            let merged = j > 0 && commit.kind == MERGE;
            let picked = j > 0 && !merged;
            let mut d = format!("M{}", point(m1, c1));
            if m2 <= m1 {
                let mid = (m1 + m2) / 2.0;
                d.push_str(&format!(
                    "C{} {} {}",
                    point(mid, c1),
                    point(mid, c2),
                    point(m2, c2)
                ));
            } else if !merged && !busy(view.lane, m1, m2) {
                // Leaves the parent at once and runs along the child's lane.
                let s = jmin(CURVE, m2 - m1);
                d.push_str(&format!(
                    "C{} {} {}L{}",
                    point(m1 + s / 2.0, c1),
                    point(m1 + s / 2.0, c2),
                    point(m1 + s, c2),
                    point(m2, c2)
                ));
            } else {
                // Turns in just before the child, or before its tags when it comes from their side.
                let to = if parent.lane < view.lane && m2 - lead[view.index] > m1 {
                    m2 - lead[view.index]
                } else {
                    m2
                };
                let mut s = jmin(CURVE, to - m1);
                let mut along = c1;
                if !merged || busy(parent.lane, m1, m2) {
                    // Between two lanes, past the ids of one and before the tags of the next, a strip stays empty.
                    along = if c1 > c2 {
                        c2 + hi[view.lane] + GAP / 2.0
                    } else {
                        c2 - lo[view.lane] - GAP / 2.0
                    };
                    s = jmin(CURVE, (to - m1) / 2.0);
                    d.push_str(&format!(
                        "C{} {} {}",
                        point(m1 + s / 2.0, c1),
                        point(m1 + s / 2.0, along),
                        point(m1 + s, along)
                    ));
                }
                d.push_str(&format!(
                    "L{}C{} {} {}",
                    point(to - s, along),
                    point(to - s / 2.0, along),
                    point(to - s / 2.0, c2),
                    point(to, c2)
                ));
                if to < m2 {
                    d.push_str(&format!("L{}", point(m2, c2)));
                }
            }
            curves.push_str(&format!(
                "<path class=\"pele-edge{}\" d=\"{}\" stroke=\"{}\"{}/>",
                if merged {
                    " pele-edge-merge"
                } else if picked {
                    " pele-edge-cherry-pick"
                } else {
                    ""
                },
                d,
                series_color(if merged { parent.lane } else { view.lane } as i64),
                if picked {
                    " stroke-dasharray=\"4 3\""
                } else {
                    ""
                }
            ));
        }
    }

    let mut commits = String::new();
    for view in &views {
        let commit = view.commit;
        let x = px(view.m, cross[view.lane]);
        let y = py(view.m, cross[view.lane]);
        let kind = commit.custom_type.unwrap_or(commit.kind);
        grow(x - DOT - 1.0, y - DOT - 1.0, x + DOT + 1.0, y + DOT + 1.0);
        // Merges and cherry-picks carry a message Mermaid makes up; only a written one is worth showing.
        let written = commit.kind < MERGE && !commit.message.is_empty();
        let mut body = if written {
            format!("<title>{}</title>", esc_text(&commit.message))
        } else {
            String::new()
        };
        body.push_str(&symbol(kind, x, y, &series_color(view.lane as i64)));

        if let Some(label) = view.label.as_ref().filter(|l| !l.lines.is_empty()) {
            let w = label.width;
            let h = label.height;
            let back = |cx: f64, cy: f64| -> String {
                format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"var(--_bg)\"/>",
                    num(cx - w / 2.0 - 2.0),
                    num(cy - h / 2.0 + 2.0),
                    num(w + 4.0),
                    num(h - 4.0)
                )
            };
            if rotate {
                // The text ends beside the commit and trails back the way the history came.
                let k = (HALF * h) / 2.0;
                let ax = if vertical { x - DOT - 3.0 - k } else { x + 3.0 };
                let ay = if vertical { y } else { y + DOT + 3.0 + k };
                let reach = HALF * w + k;
                grow(
                    ax - reach,
                    if flip { ay - reach } else { ay - k },
                    ax + k,
                    if flip { ay + k } else { ay + reach },
                );
                body.push_str(&format!(
                    "<g class=\"pele-commit-label\" transform=\"rotate({} {} {})\">{}{}</g>",
                    if flip { 45 } else { -45 },
                    num(ax),
                    num(ay),
                    back(ax - w / 2.0, ay),
                    label_svg(label, ax - w / 2.0, ay, "", None)
                ));
            } else {
                let cx = if vertical { x - DOT - 6.0 - w / 2.0 } else { x };
                let cy = if vertical { y } else { y + DOT + 6.0 + h / 2.0 };
                grow(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0);
                body.push_str(&format!(
                    "<g class=\"pele-commit-label\">{}{}</g>",
                    back(cx, cy),
                    label_svg(label, cx, cy, "", None)
                ));
            }
        }

        // Beside the lane the tags form a row; above it they stack, the first one furthest out.
        let tags = &view.tags;
        let mut offset = DOT + TAG_GAP;
        let order: Vec<usize> = if vertical {
            (0..tags.len()).collect()
        } else {
            (0..tags.len()).rev().collect()
        };
        for t in order {
            let tag = &tags[t];
            let w = tag_width(tag);
            let h = tag.height;
            let left = if vertical { x + offset } else { x - w / 2.0 };
            let cy = if vertical { y } else { y - offset - h / 2.0 };
            offset += if vertical { w + 4.0 } else { h + 3.0 };
            grow(left, cy - h / 2.0, left + w, cy + h / 2.0);
            body.push_str(&format!(
                "<g class=\"pele-tag\"><path d=\"M{},{}L{},{}H{}V{}H{}Z\" fill=\"var(--_s)\" stroke=\"var(--_b)\" stroke-linejoin=\"round\"/><circle cx=\"{}\" cy=\"{}\" r=\"1.5\" fill=\"var(--_b)\"/>{}</g>",
                num(left),
                num(cy),
                num(left + NOTCH),
                num(cy - h / 2.0),
                num(left + w),
                num(cy + h / 2.0),
                num(left + NOTCH),
                num(left + NOTCH),
                num(cy),
                label_svg(
                    tag,
                    left + NOTCH + 4.0 + tag.width / 2.0,
                    cy,
                    " fill=\"var(--_fg)\"",
                    None
                )
            ));
        }

        let merge = if commit.kind == MERGE && kind != MERGE {
            " pele-commit-merge"
        } else {
            ""
        };
        commits.push_str(&format!(
            "<g class=\"pele-commit pele-commit-{}{}\" data-id=\"{}\">{}</g>",
            CLASSES.get(kind as usize).unwrap_or(&CLASSES[0]),
            merge,
            esc_text(&commit.id),
            body
        ));
    }

    if min_x > max_x {
        // grow(0, 0, 0, 0) on nothing drawn.
        min_x = jmin(min_x, 0.0);
        min_y = jmin(min_y, 0.0);
        max_x = jmax(max_x, 0.0);
        max_y = jmax(max_y, 0.0);
    }
    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        size,
        4000.0,
        style::BOLD,
        icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 16.0
    } else {
        0.0
    };
    let inner = jmax(max_x - min_x, title.width);
    let width = (inner + 2.0 * pad).ceil();
    let height = (max_y - min_y + title_height + 2.0 * pad).ceil();
    let dx = pad - min_x + (inner - (max_x - min_x)) / 2.0;
    let dy = pad + title_height - min_y;

    let mut content = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    content.push_str(&format!(
        "<g transform=\"translate({},{})\">",
        num(dx),
        num(dy)
    ));
    if !branches.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-branches\" fill=\"none\" font-size=\"{}\">{}</g>",
            num_str(medium),
            branches
        ));
    }
    if !curves.is_empty() || !lines.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-edges\" fill=\"none\" stroke-width=\"2\" stroke-linecap=\"round\">{}{}</g>",
            curves, lines
        ));
    }
    if !commits.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-commits\" font-size=\"{}\" fill=\"var(--_m)\">{}</g>",
            num_str(small),
            commits
        ));
    }
    content.push_str("</g>");

    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };
    Rendered {
        svg: svg_document(
            "gitGraph",
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
    }
}
