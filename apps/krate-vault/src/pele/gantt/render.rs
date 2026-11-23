//! Ports diagrams/gantt/render.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::axis::{auto_ticks, interval_ticks, time_format};
use super::date::Date;
use super::dates::{add, add_days, Unit};
use super::db::{clock, GanttDb, GanttTask};
use crate::json::Value;
use crate::pele::common::fit_width::cap_width;
use crate::pele::core::{LinkInfo, RenderOptions, Rendered};
use crate::pele::errors::PeleError;
use crate::pele::js::{jmax, jmin, jround, num_str};
use crate::pele::preprocess::Config;
use crate::pele::svg::builder::{esc, esc_text, label_svg, num};
use crate::pele::svg::palette::Palette;
use crate::pele::svg::root::{svg_document, Accessible};
use crate::pele::svg::theme::{resolve_style, CRITICAL, ROUNDED};
use crate::pele::text::label::{layout_label, Label};
use crate::pele::text::measurer::{style, Measure, METRICS};
use crate::pele::util::url::linked;

const WIDTH: f64 = 800.0;
const MIN_PLOT: f64 = 240.0;
const LABEL_GAP: f64 = 8.0;

/// Ports `setting`.
fn setting(config: &Config, key: &str, min: f64, max: f64) -> Option<f64> {
    match config.get("gantt").and_then(|g| g.get(key)) {
        Some(Value::Num(v)) if v.is_finite() => Some(jmin(max, jmax(min, *v))),
        _ => None,
    }
}

/// Ports `text`.
fn text(config: &Config, key: &str) -> Option<String> {
    match config.get("gantt").and_then(|g| g.get(key)) {
        Some(Value::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Ports `flag`.
fn flag(config: &Config, key: &str) -> bool {
    config.get("gantt").and_then(|g| g.get(key)) == Some(&Value::Bool(true))
}

/// `raw.replace(/<br\s*\/?>/gi, ' ')`.
fn breaks_to_spaces(raw: &str) -> String {
    let c: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '<'
            && c.get(i + 1).is_some_and(|x| x.eq_ignore_ascii_case(&'b'))
            && c.get(i + 2).is_some_and(|x| x.eq_ignore_ascii_case(&'r'))
        {
            let mut j = i + 3;
            while j < c.len() && crate::pele::js::is_space(c[j]) {
                j += 1;
            }
            if c.get(j) == Some(&'/') {
                j += 1;
            }
            if c.get(j) == Some(&'>') {
                out.push(' ');
                i = j + 1;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Ports `oneLine`: a task label sits on one row, so only its first line is kept.
fn one_line(raw: &str, measurer: &dyn Measure, size: f64, options: &RenderOptions) -> Label {
    let mut label = layout_label(
        Some(&breaks_to_spaces(raw)),
        false,
        measurer,
        size,
        4000.0,
        0,
        options.icons,
    );
    if label.lines.len() <= 1 {
        return label;
    }
    label.lines.truncate(1);
    label.widths.truncate(1);
    label.width = label.widths[0];
    label.height = label.line_height;
    label
}

/// Ports `scheduled`: whether scheduling gave the task a start and an end.
fn scheduled(task: &GanttTask) -> bool {
    task.start_time.is_some_and(|d| d.time().is_finite())
        && task.end_time.is_some_and(|d| d.time().is_finite())
}

fn tv(d: Option<Date>) -> f64 {
    d.map(|d| d.time()).unwrap_or(f64::NAN)
}

/// Ports `packRows`: Mermaid's compact mode; within a section, a task goes
/// on the first row that is free when it starts.
fn pack_rows(tasks: &[usize], all: &[GanttTask], offset: usize, row_of: &mut [usize]) -> usize {
    let mut sorted: Vec<usize> = tasks
        .iter()
        .copied()
        .filter(|&k| scheduled(&all[k]))
        .collect();
    sorted.sort_by(|&a, &b| {
        let d = tv(all[a].start_time) - tv(all[b].start_time);
        let d = if d != 0.0 && !d.is_nan() {
            d
        } else {
            (all[a].order - all[b].order) as f64
        };
        d.partial_cmp(&0.0).unwrap_or(core::cmp::Ordering::Equal)
    });
    let mut free: Vec<f64> = Vec::new();
    let mut set = alloc::vec![false; all.len()];
    for &k in &sorted {
        let start = tv(all[k].start_time);
        let mut row = 0;
        while row < free.len() && start < free[row] {
            row += 1;
        }
        if row == free.len() {
            free.push(0.0);
        }
        free[row] = tv(all[k].end_time);
        row_of[k] = offset + row;
        set[k] = true;
    }
    let mut rows = free.len();
    for &k in tasks {
        if !set[k] {
            row_of[k] = offset + rows;
            set[k] = true;
            rows += 1;
        }
    }
    rows.max(1)
}

/// Ports `interface Bar`.
struct Bar {
    task: usize,
    label: Label,
    row: usize,
    x0: f64,
    x1: f64,
    label_x: f64,
    inside: bool,
}

/// Ports `renderGantt`.
pub fn render_gantt(
    model: &GanttDb,
    config: &Config,
    options: &RenderOptions,
    palette: &Palette,
) -> Result<Rendered, PeleError> {
    let mut model = model.clone();
    let size = options.font_size.unwrap_or(16.0);
    let measurer: &dyn Measure = options.measurer.unwrap_or(&METRICS);
    let pad = options.padding.unwrap_or(8.0);
    let now = clock(options.now);
    let acc = Accessible {
        acc_title: model.acc_title.clone(),
        acc_descr: model.acc_descr.clone(),
    };

    let label_size = setting(config, "fontSize", 4.0, 72.0).unwrap_or(jround(size * 0.8125));
    let section_size = setting(config, "sectionFontSize", 4.0, 72.0).unwrap_or(label_size);
    let tick_size = jround(size * 0.75);
    let bar_height = setting(config, "barHeight", 4.0, 400.0).unwrap_or(jround(label_size * 1.85));
    let bar_gap = setting(config, "barGap", 0.0, 400.0).unwrap_or(jround(bar_height / 4.0));
    let row_height = bar_height + bar_gap;
    let section_styles = jround(setting(config, "numberSectionStyles", 1.0, 1000.0).unwrap_or(4.0));
    let compact = model.display_mode == "compact";
    let top_axis = model.top_axis || flag(config, "topAxis");

    model.get_tasks(now)?;
    let tasks = model.tasks.clone();
    let row_tasks: Vec<usize> = (0..tasks.len()).filter(|&k| !tasks[k].vert).collect();
    let verts: Vec<usize> = (0..tasks.len())
        .filter(|&k| tasks[k].vert && scheduled(&tasks[k]))
        .collect();

    let mut categories: Vec<String> = Vec::new();
    for &k in &row_tasks {
        if !categories.contains(&tasks[k].section) {
            categories.push(tasks[k].section.clone());
        }
    }
    let category = |s: &str| categories.iter().position(|c| c == s).unwrap_or(0);

    let mut row_of = alloc::vec![0usize; tasks.len()];
    let mut row_section: Vec<String> = Vec::new();
    if compact {
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for &k in &row_tasks {
            match groups.iter_mut().find(|(s, _)| *s == tasks[k].section) {
                Some(g) => g.1.push(k),
                None => groups.push((tasks[k].section.clone(), alloc::vec![k])),
            }
        }
        for (section, group) in &groups {
            let rows = pack_rows(group, &tasks, row_section.len(), &mut row_of);
            for _ in 0..rows {
                row_section.push(section.clone());
            }
        }
    } else {
        for &k in &row_tasks {
            row_of[k] = row_section.len();
            row_section.push(tasks[k].section.clone());
        }
    }
    let row_count = row_section.len();

    let mut start = f64::INFINITY;
    let mut stop = f64::NEG_INFINITY;
    for task in &tasks {
        if !scheduled(task) {
            continue;
        }
        start = jmin(start, tv(task.start_time));
        stop = jmax(stop, tv(task.end_time));
    }
    let has_plot = start <= stop;

    let title = layout_label(
        model.title.as_deref(),
        false,
        measurer,
        size,
        4000.0,
        style::BOLD,
        options.icons,
    );
    let title_height = if title.height > 0.0 {
        title.height + 16.0
    } else {
        0.0
    };

    if !has_plot {
        let width = (jmax(title.width, 1.0) + 2.0 * pad).ceil();
        let height = (jmax(title.height, 1.0) + 2.0 * pad).ceil();
        let content = label_svg(
            &title,
            width / 2.0,
            pad + title.height / 2.0,
            " class=\"pele-title\" font-weight=\"var(--_tw)\"",
            None,
        );
        return Ok(Rendered {
            svg: svg_document(
                "gantt",
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
        });
    }

    let mut section_labels: Vec<Label> = Vec::new();
    let mut section_width: f64 = 0.0;
    for section in &categories {
        let label = layout_label(
            Some(section),
            false,
            measurer,
            section_size,
            180.0,
            0,
            options.icons,
        );
        section_width = jmax(section_width, label.width);
        section_labels.push(label);
    }

    let axis_format = if !model.axis_format.is_empty() {
        model.axis_format.clone()
    } else if model.date_format == "D" {
        String::from("%d")
    } else {
        text(config, "axisFormat").unwrap_or_else(|| String::from("%Y-%m-%d"))
    };
    let tick_label = |t: f64| -> String { time_format(&axis_format, &Date::new(t)) };
    let sample_width = jmax(
        measurer.width(&tick_label(start), tick_size, 0),
        measurer.width(&tick_label(stop), tick_size, 0),
    );

    let left = pad
        + setting(config, "leftPadding", 0.0, 2000.0).unwrap_or(jmax(
            if section_width > 0.0 {
                section_width + 24.0
            } else {
                0.0
            },
            sample_width / 2.0,
        ));
    let right_padding =
        setting(config, "rightPadding", 0.0, 2000.0).unwrap_or(jmax(16.0, sample_width / 2.0));
    let target = cap_width(
        options,
        setting(config, "useWidth", 100.0, 20000.0).unwrap_or(WIDTH),
        280.0,
        0.0,
    );
    let plot_width = jmax(MIN_PLOT, target - left - right_padding - pad);
    let right = left + plot_width;
    let scale = if stop > start {
        plot_width / (stop - start)
    } else {
        0.0
    };
    let x = |t: f64| -> f64 {
        if scale == 0.0 {
            left + plot_width / 2.0
        } else {
            left + (t - start) * scale
        }
    };

    let tick_height = jround(tick_size * 1.75);
    let top_padding = setting(config, "topPadding", 0.0, 2000.0);
    let grid_start = setting(config, "gridLineStartPadding", 0.0, 2000.0);
    let head_height = pad + title_height + if top_axis { tick_height } else { 0.0 };
    let plot_top = jmax(head_height, top_padding.unwrap_or(0.0));
    // Mermaid starts its grid lines above the first row by the difference between these two settings.
    let grid_lead = match (top_padding, grid_start) {
        (Some(tp), Some(gs)) => jmax(0.0, jmin(tp - gs, plot_top - head_height)),
        _ => 0.0,
    };
    let plot_height = jmax(row_count as f64, 1.0) * row_height;
    let plot_bottom = plot_top + plot_height;

    // Bars and where their labels go: inside when they fit, otherwise beside the bar on the side with room.
    let mut bars: Vec<Bar> = Vec::new();
    let mut extent = right + right_padding;
    for &k in &row_tasks {
        let task = &tasks[k];
        let label = one_line(&task.task, measurer, label_size, options);
        let row = row_of[k];
        if !scheduled(task) {
            let label_x = left + LABEL_GAP + label.width / 2.0;
            extent = jmax(extent, label_x + label.width / 2.0);
            bars.push(Bar {
                task: k,
                label,
                row,
                x0: f64::NAN,
                x1: f64::NAN,
                label_x,
                inside: false,
            });
            continue;
        }
        let mut x0 = x(tv(task.start_time));
        let mut x1 = jmax(
            x(tv(task.render_end_time.or(task.end_time))),
            x0 + if task.milestone { 0.0 } else { 2.0 },
        );
        if task.milestone {
            let mid = x0 + (x(tv(task.end_time)) - x0) / 2.0;
            x0 = mid - bar_height / 2.0;
            x1 = mid + bar_height / 2.0;
        }
        let inside = !task.milestone && label.width + 2.0 * LABEL_GAP <= x1 - x0;
        let mut label_x = (x0 + x1) / 2.0;
        if !inside {
            let fits_right = x1 + LABEL_GAP + label.width <= right + right_padding;
            let fits_left = x0 - LABEL_GAP - label.width >= left;
            label_x = if fits_right || !fits_left {
                x1 + LABEL_GAP + label.width / 2.0
            } else {
                x0 - LABEL_GAP - label.width / 2.0
            };
            extent = jmax(extent, label_x + label.width / 2.0);
        }
        bars.push(Bar {
            task: k,
            label,
            row,
            x0,
            x1,
            label_x,
            inside,
        });
    }

    let interval = match model.tick_interval.as_deref() {
        Some(t) if !t.is_empty() => String::from(t),
        _ => text(config, "tickInterval")
            .filter(|t| !t.is_empty())
            .unwrap_or_default(),
    };
    let weekday = model
        .weekday
        .clone()
        .or_else(|| text(config, "weekday"))
        .unwrap_or_else(|| String::from("sunday"));
    let count = jmax(
        2.0,
        jmin(10.0, (plot_width / (sample_width + 24.0)).floor()),
    );
    let ticks = interval_ticks(&interval, &weekday, start, stop, (plot_width / 3.0).ceil())
        .unwrap_or_else(|| auto_ticks(start, stop, count));

    let mut grid = String::new();
    let mut axis_bottom = String::new();
    let mut axis_top = String::new();
    let mut last_right = f64::NEG_INFINITY;
    let mut last_label = String::new();
    let bottom_y = plot_bottom + tick_height / 2.0 + 2.0;
    let top_y = plot_top - grid_lead - tick_height / 2.0 - 2.0;
    for &t in &ticks {
        let tx = x(t);
        grid.push_str(&format!(
            "M{},{}V{}",
            num(tx),
            num(plot_top - grid_lead),
            num(plot_bottom)
        ));
        let label = tick_label(t);
        let width = measurer.width(&label, tick_size, 0);
        // A label is skipped when it would run into the one before it or only repeat it; its grid line stays.
        if tx - width / 2.0 < last_right + 8.0 || label == last_label {
            continue;
        }
        last_right = tx + width / 2.0;
        last_label = label.clone();
        extent = jmax(extent, last_right);
        let body = format!("text-anchor=\"middle\">{}</text>", esc(&label));
        axis_bottom.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" {}",
            num(tx),
            num(bottom_y + tick_size * 0.35),
            body
        ));
        if top_axis {
            axis_top.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" {}",
                num(tx),
                num(top_y + tick_size * 0.35),
                body
            ));
        }
    }

    // Vertical markers take no row. Their labels go under the axis, on as many lines as it takes to keep them apart.
    let mut vert_lines = String::new();
    let mut vert_labels = String::new();
    let mut vert_ends: Vec<f64> = Vec::new();
    let vert_top = plot_bottom + tick_height + 4.0;
    let vert_line = jround(label_size * 1.5);
    let mut sorted_verts = verts.clone();
    sorted_verts.sort_by(|&a, &b| {
        (tv(tasks[a].start_time) - tv(tasks[b].start_time))
            .partial_cmp(&0.0)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    for &k in &sorted_verts {
        let task = &tasks[k];
        let vx = x(tv(task.start_time));
        let label = one_line(&task.task, measurer, label_size, options);
        let cx = jmax(pad + label.width / 2.0, vx);
        let mut line = 0;
        while line < vert_ends.len() && cx - label.width / 2.0 < vert_ends[line] + 8.0 {
            line += 1;
        }
        if line == vert_ends.len() {
            vert_ends.push(0.0);
        }
        vert_ends[line] = cx + label.width / 2.0;
        extent = jmax(extent, vert_ends[line]);
        vert_lines.push_str(&format!(
            "<path class=\"pele-vert\" data-id=\"{}\" d=\"M{},{}V{}\"/>",
            esc_text(&task.id),
            num(vx),
            num(plot_top - grid_lead),
            num(plot_bottom)
        ));
        vert_labels.push_str(&label_svg(
            &label,
            cx,
            vert_top + line as f64 * vert_line + vert_line / 2.0,
            &format!(
                " class=\"pele-vert-label\" data-id=\"{}\"",
                esc_text(&task.id)
            ),
            None,
        ));
    }

    let width = (jmax(extent, pad + title.width) + pad).ceil();
    let height = (plot_bottom
        + tick_height
        + if !vert_ends.is_empty() {
            4.0 + vert_ends.len() as f64 * vert_line
        } else {
            0.0
        }
        + pad)
        .ceil();

    // Section bands, one per run of rows that share a section, shaded on alternate sections.
    let mut bands = String::new();
    let mut section_out = String::new();
    let mut row = 0;
    while row < row_count {
        let section = &row_section[row];
        let mut end = row + 1;
        while end < row_count && row_section[end] == *section {
            end += 1;
        }
        let index = category(section) as f64 % section_styles;
        let y = plot_top + row as f64 * row_height;
        let h = (end - row) as f64 * row_height;
        if categories.len() > 1 || !section.is_empty() {
            bands.push_str(&format!(
                "<rect class=\"pele-section pele-section-{}\" x=\"0\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
                num_str(index),
                num(y),
                num_str(width),
                num(h),
                if index % 2.0 == 0.0 { "var(--_s)" } else { "none" }
            ));
        }
        let label = &section_labels[category(section)];
        section_out.push_str(&label_svg(
            label,
            pad + 8.0 + label.width / 2.0,
            y + h / 2.0,
            &format!(
                " class=\"pele-section-label\" data-id=\"{}\"",
                esc_text(section)
            ),
            options.icons,
        ));
        row = end;
    }

    // Excluded days, as Mermaid marks them: a day at a time from the first task's start, and not at all
    // when the chart spans more than five years.
    let mut excluded = String::new();
    if (!model.excludes.is_empty() || !model.includes.is_empty())
        && add(&Date::new(start), 6.0, Unit::Y).time() > stop
    {
        let mut from: Option<Date> = None;
        let mut to: Option<Date> = None;
        let flush = |from: &mut Option<Date>, to: &mut Option<Date>, excluded: &mut String| {
            let (Some(f), Some(t)) = (*from, *to) else {
                return;
            };
            let mut day_start = f;
            day_start.set_hours(0.0, 0.0, 0.0, 0.0);
            let mut day_end = t;
            day_end.set_hours(23.0, 59.0, 59.0, 999.0);
            let ex0 = jmax(left, x(day_start.time()));
            let ex1 = jmin(right, x(day_end.time()));
            if ex1 > ex0 {
                excluded.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                    num(ex0),
                    num(plot_top),
                    num(ex1 - ex0),
                    num(plot_height)
                ));
            }
            *from = None;
            *to = None;
        };
        let mut day = Date::new(start);
        while day.time() <= stop {
            if model.is_invalid_date(&day)? {
                if from.is_none() {
                    from = Some(day);
                }
                to = Some(day);
            } else {
                flush(&mut from, &mut to, &mut excluded);
            }
            day = add_days(&day, 1.0);
        }
        flush(&mut from, &mut to, &mut excluded);
    }

    let mut links: Vec<LinkInfo> = Vec::new();
    let mut tasks_out = String::new();
    for bar in &bars {
        let task = &tasks[bar.task];
        let cy = plot_top + bar.row as f64 * row_height + row_height / 2.0;
        let state = if task.active {
            "active"
        } else if task.done {
            "done"
        } else {
            ""
        };
        let mut classes = String::from("pele-task");
        if !state.is_empty() {
            classes.push_str(" pele-");
            classes.push_str(state);
        }
        if task.crit {
            classes.push_str(" pele-crit");
        }
        if task.milestone {
            classes.push_str(" pele-milestone");
        }
        if task.classes.iter().any(|c| c == "clickable") {
            classes.push_str(" pele-clickable");
        }

        let mut shape = String::new();
        let mut text_fill = "";
        if bar.x0.is_nan() {
            classes.push_str(" pele-unscheduled");
            text_fill = " fill=\"var(--_m)\"";
        } else {
            // A critical task takes the critical color in place of the accent. Done, it keeps the fill
            // of finished work and only its border is in that color.
            let color = if task.crit { CRITICAL } else { "var(--_c)" };
            let paint = if state == "done" {
                String::from("fill=\"var(--_a)\"")
            } else if state == "active" {
                format!("fill=\"{}\" fill-opacity=\"0.25\"", color)
            } else {
                format!("fill=\"{}\"", color)
            };
            let stroke = format!(
                "stroke=\"{}\"",
                if state == "done" && !task.crit {
                    "var(--_b)"
                } else {
                    color
                }
            );
            if task.milestone {
                let cx = (bar.x0 + bar.x1) / 2.0;
                let r = bar_height / 2.0;
                shape = format!(
                    "<path d=\"M{},{}L{},{}L{},{}L{},{}Z\" {} {} stroke-linejoin=\"round\"/>",
                    num(cx),
                    num(cy - r),
                    num(cx + r),
                    num(cy),
                    num(cx),
                    num(cy + r),
                    num(cx - r),
                    num(cy),
                    paint,
                    stroke
                );
            } else {
                shape = format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {} {}{}/>",
                    num(bar.x0),
                    num(cy - bar_height / 2.0),
                    num(bar.x1 - bar.x0),
                    num(bar_height),
                    paint,
                    stroke,
                    ROUNDED
                );
            }
            if bar.inside {
                text_fill = if state.is_empty() {
                    " fill=\"var(--_bg)\""
                } else {
                    ""
                };
            } else if state == "done" {
                text_fill = " fill=\"var(--_m)\"";
            }
        }

        let mut body = shape
            + &label_svg(
                &bar.label,
                bar.label_x,
                cy,
                &format!(" class=\"pele-label\"{}", text_fill),
                None,
            );
        if let Some((_, url)) = model.links.iter().find(|(id, _)| *id == task.id) {
            body = linked(&body, url, &task.id, &mut links, options, "");
        }
        tasks_out.push_str(&format!(
            "<g class=\"{}\" data-id=\"{}\">{}</g>",
            classes,
            esc_text(&task.id),
            body
        ));
    }

    let mut today = String::new();
    if model.today_marker != "off" && now >= start && now <= stop && stop > start {
        let tx = x(now);
        let top = plot_top - grid_lead;
        let style = if !model.today_marker.is_empty() {
            let parts: Vec<&str> = model.today_marker.split(',').collect();
            resolve_style(&parts).line
        } else {
            String::new()
        };
        today = format!(
            "<g class=\"pele-today\"><path d=\"M{},{}V{}\" fill=\"none\" stroke=\"var(--_c)\" stroke-width=\"2\"{}/>",
            num(tx),
            num(top),
            num(plot_bottom),
            style
        );
        if style.is_empty() {
            today.push_str(&format!(
                "<path d=\"M{},{}H{}L{},{}Z\" fill=\"var(--_c)\"/>",
                num(tx - 4.0),
                num(top - 5.0),
                num(tx + 4.0),
                num(tx),
                num(top + 1.0)
            ));
        }
        today.push_str("</g>");
    }

    let mut axis_line = format!("M{},{}H{}", num(left), num(plot_bottom), num(right));
    if top_axis {
        axis_line.push_str(&format!(
            "M{},{}H{}",
            num(left),
            num(plot_top - grid_lead),
            num(right)
        ));
    }
    let mut content = label_svg(
        &title,
        width / 2.0,
        pad + title.height / 2.0,
        " class=\"pele-title\" font-weight=\"var(--_tw)\"",
        None,
    );
    if !bands.is_empty() {
        content.push_str(&format!("<g class=\"pele-sections\">{}</g>", bands));
    }
    if !excluded.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-excluded\" fill=\"var(--_a)\">{}</g>",
            excluded
        ));
    }
    content.push_str(&format!(
        "<g class=\"pele-grid\" fill=\"none\"><path d=\"{}\" stroke=\"var(--_b)\" stroke-dasharray=\"1 3\"/><path d=\"{}\" stroke=\"var(--_b)\"/></g>",
        grid, axis_line
    ));
    if !vert_lines.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-verts\" fill=\"none\" stroke=\"var(--_l)\" stroke-width=\"1.5\" stroke-dasharray=\"4 3\">{}</g>",
            vert_lines
        ));
    }
    content.push_str(&today);
    content.push_str(&format!(
        "<g class=\"pele-tasks\" font-size=\"{}\">{}</g>",
        num(label_size),
        tasks_out
    ));
    content.push_str(&format!(
        "<g class=\"pele-section-labels\" font-size=\"{}\">{}</g>",
        num(section_size),
        section_out
    ));
    content.push_str(&format!(
        "<g class=\"pele-axis\" font-size=\"{}\" fill=\"var(--_m)\">{}{}</g>",
        num_str(tick_size),
        axis_bottom,
        axis_top
    ));
    if !vert_labels.is_empty() {
        content.push_str(&format!(
            "<g class=\"pele-vert-labels\" font-size=\"{}\">{}</g>",
            num(label_size),
            vert_labels
        ));
    }

    Ok(Rendered {
        svg: svg_document(
            "gantt",
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
        links,
    })
}
