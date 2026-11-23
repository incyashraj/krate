//! Ports diagrams/gantt/db.ts and types.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::date::Date;
use super::dates::{add, add_days, format, iso_date, parse_strict, Unit, WEEKDAYS};
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{self, is_space, is_word, trim, trim_start};
use crate::pele::quadrant::db::tighten_lines;
use crate::pele::util::url::safe_url;

/// Ports `interface GanttTask`.
#[derive(Debug, Clone, PartialEq)]
pub struct GanttTask {
    pub id: String,
    /// The task's label, under the name Mermaid gives it.
    pub task: String,
    pub section: String,
    /// Unset until `get_tasks` has scheduled the task, and still unset when what it refers to never resolves.
    pub start_time: Option<Date>,
    pub end_time: Option<Date>,
    /// Where the bar ends when excluded days pushed the task's end out.
    pub render_end_time: Option<Date>,
    pub manual_end_time: bool,
    pub processed: bool,
    pub active: bool,
    pub done: bool,
    pub crit: bool,
    pub milestone: bool,
    pub vert: bool,
    /// Row index in source order; -1 for vertical markers, which take no row.
    pub order: i64,
    pub classes: Vec<String>,
    pub prev_task_id: Option<String>,
    /// `raw`: the data as written, its start, and its end.
    pub raw_start: Option<String>,
    pub raw_end: String,
}

const WORK_LIMIT: f64 = 4e8;
const DAY_STEP: f64 = 400.0;
const DATED_STEP: f64 = 200.0;
const FORMAT_STEP: f64 = 400.0;

/// Ports `clock`: the time a render takes as now. Pele reads the current
/// time when none is given; the port has no clock, and takes no time
/// (NaN), so that nothing is drawn at today.
pub fn clock(now: Option<f64>) -> f64 {
    match now {
        Some(t) if t.is_finite() => t,
        _ => f64::NAN,
    }
}

/// Ports `parseDuration`: `/^(\d+(?:\.\d+)?)([Mdhmswy]|ms)$/` on the trimmed text.
pub fn parse_duration(s: &str) -> (f64, Unit) {
    let t = trim(s);
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return (f64::NAN, Unit::Ms);
    }
    let mut num_end = i;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 {
            num_end = j;
        }
    }
    let unit = match &t[num_end..] {
        "M" => Unit::Month,
        "d" => Unit::D,
        "h" => Unit::H,
        "m" => Unit::M,
        "s" => Unit::S,
        "w" => Unit::W,
        "y" => Unit::Y,
        "ms" => Unit::Ms,
        _ => return (f64::NAN, Unit::Ms),
    };
    (js::parse_float(&t[..num_end]), unit)
}

/// `/[\s,]+/` splits of the lowercased text, without empty tokens,
/// merged into a list in insertion order (`mergeTokens`).
fn merge_tokens(existing: &[String], txt: &str) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for e in existing {
        if !merged.contains(e) {
            merged.push(e.clone());
        }
    }
    for token in txt
        .to_lowercase()
        .split(|c: char| is_space(c) || c == ',')
        .filter(|t| !t.is_empty())
    {
        if !merged.iter().any(|m| m == token) {
            merged.push(token.into());
        }
    }
    merged
}

/// `/^\s*(active|done|crit|milestone|vert)\s*$/`.
fn tag_of(s: &str) -> Option<&'static str> {
    ["active", "done", "crit", "milestone", "vert"]
        .into_iter()
        .find(|t| trim(s) == *t)
}

/// `/^after\s+([\d\w- ]+)/` (or `until`): the group.
fn reference<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(keyword)?;
    let t = rest.trim_start_matches(is_space);
    if t.len() == rest.len() {
        return None;
    }
    // \s+ is greedy, then the group needs one character of its class; a
    // space it took back belongs to the group.
    let end = t
        .find(|c: char| !(is_word(c) || c == '-' || c == ' '))
        .unwrap_or(t.len());
    if end > 0 {
        return Some(&t[..end]);
    }
    // Backtrack: the last whitespace of the run may be a space.
    let run = &rest[..rest.len() - t.len()];
    if run.ends_with(' ') && run.chars().count() >= 2 {
        let start = rest.len() - t.len() - 1;
        let tail = &rest[start..];
        let e = tail
            .find(|c: char| !(is_word(c) || c == '-' || c == ' '))
            .unwrap_or(tail.len());
        return Some(&tail[..e]);
    }
    None
}

/// Ports `class GanttDb`.
#[derive(Debug, Clone, Default)]
pub struct GanttDb {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub date_format: String,
    pub axis_format: String,
    pub tick_interval: Option<String>,
    pub today_marker: String,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub inclusive_end_dates: bool,
    pub top_axis: bool,
    pub display_mode: String,
    pub weekday: Option<String>,
    pub weekend: String,
    pub sections: Vec<String>,
    /// Task id to link, in insertion order.
    pub links: Vec<(String, String)>,
    pub tasks: Vec<GanttTask>,

    current_section: String,
    task_count: i64,
    last_order: i64,
    last_task_id: Option<String>,
    now: f64,
    work: f64,
    excluded_days: [bool; 7],
    dated_tokens: bool,
}

impl GanttDb {
    pub fn new() -> GanttDb {
        GanttDb {
            weekend: "saturday".into(),
            ..GanttDb::default()
        }
    }

    fn by_id(&self, id: &str) -> Option<usize> {
        // A Map keeps the last task set under an id.
        self.tasks.iter().rposition(|t| t.id == id)
    }

    pub fn set_date_format(&mut self, txt: &str) {
        self.date_format = txt.into();
    }

    pub fn set_includes(&mut self, txt: &str) {
        self.includes = merge_tokens(&self.includes, txt);
    }

    pub fn set_excludes(&mut self, txt: &str) {
        self.excludes = merge_tokens(&self.excludes, txt);
    }

    pub fn set_acc_title(&mut self, txt: &str) {
        self.acc_title = Some(trim_start(txt).into());
    }

    pub fn set_acc_description(&mut self, txt: &str) {
        self.acc_descr = Some(tighten_lines(txt));
    }

    pub fn add_section(&mut self, txt: &str) {
        self.current_section = txt.into();
        self.sections.push(txt.into());
    }

    /// Ports `addTask`.
    pub fn add_task(&mut self, descr: &str, data_str: &str) -> Result<(), PeleError> {
        let body = data_str.strip_prefix(':').unwrap_or(data_str);
        let mut data: Vec<String> = body.split(',').map(String::from).collect();
        let mut task = GanttTask {
            id: String::new(),
            task: descr.into(),
            section: self.current_section.clone(),
            start_time: None,
            end_time: None,
            render_end_time: None,
            manual_end_time: false,
            processed: false,
            active: false,
            done: false,
            crit: false,
            milestone: false,
            vert: false,
            order: -1,
            classes: Vec::new(),
            prev_task_id: self.last_task_id.clone(),
            raw_start: None,
            raw_end: String::new(),
        };
        while let Some(tag) = data.first().and_then(|d| tag_of(d)) {
            match tag {
                "active" => task.active = true,
                "done" => task.done = true,
                "crit" => task.crit = true,
                "milestone" => task.milestone = true,
                _ => task.vert = true,
            }
            data.remove(0);
        }
        // Mermaid fails with a TypeError on both of these.
        if data.is_empty() {
            return Err(PeleError::of(
                format!("Task \"{}\" needs a duration or an end date.", trim(descr)),
                PeleErrorCode::Semantic,
                "gantt",
            ));
        }
        if data.len() > 3 {
            return Err(PeleError::of(
                format!(
                    "Task \"{}\" has too many fields. A task takes an id, a start, and an end or duration.",
                    trim(descr)
                ),
                PeleErrorCode::Semantic,
                "gantt",
            ));
        }
        for d in data.iter_mut() {
            *d = trim(d).into();
        }
        if data.len() == 3 {
            task.id = data[0].clone();
        } else {
            self.task_count += 1;
            task.id = format!("task{}", self.task_count);
        }
        if data.len() > 1 {
            task.raw_start = Some(data[data.len() - 2].clone());
        }
        task.raw_end = data[data.len() - 1].clone();
        if !task.vert {
            task.order = self.last_order;
            self.last_order += 1;
        }
        self.last_task_id = Some(task.id.clone());
        self.tasks.push(task);
        Ok(())
    }

    /// Ports `setLink`.
    pub fn set_link(&mut self, ids: &str, link: &str) {
        let href = safe_url(link);
        for id in ids.split(',') {
            if self.by_id(id).is_some() {
                match self.links.iter_mut().find(|(k, _)| k == id) {
                    Some(e) => e.1 = href.clone(),
                    None => self.links.push((id.into(), href.clone())),
                }
            }
        }
        self.set_class(ids, "clickable");
    }

    /// Ports `setClass`.
    pub fn set_class(&mut self, ids: &str, class_name: &str) {
        for id in ids.split(',') {
            if let Some(k) = self.by_id(id) {
                self.tasks[k].classes.push(class_name.into());
            }
        }
    }

    /// Ports `getTasks`: resolves every task's start and end, from scratch
    /// on each call; up to eleven passes, as in Mermaid.
    pub fn get_tasks(&mut self, now: f64) -> Result<(), PeleError> {
        self.now = now;
        self.work = 0.0;
        let weekend_start = match self.weekend.as_str() {
            "friday" => 5.0,
            "saturday" => 6.0,
            _ => f64::NAN,
        };
        let weekends = self.excludes.iter().any(|e| e == "weekends");
        for (day, name) in WEEKDAYS.iter().enumerate() {
            let iso = if day == 0 { 7.0 } else { day as f64 };
            self.excluded_days[day] = (weekends
                && (iso == weekend_start || iso == weekend_start + 1.0))
                || self.excludes.iter().any(|e| *e == name.to_lowercase());
        }
        // A date written with only these tokens and separators can never read as a weekday name.
        self.dated_tokens = !self.includes.is_empty()
            || !trim(&self.date_format)
                .chars()
                .all(|c| "YMDHhmsSZXx-/.:, T".contains(c));
        for token in &self.excludes {
            if token != "weekends" && !WEEKDAYS.iter().any(|d| d.to_lowercase() == *token) {
                self.dated_tokens = true;
            }
        }
        for task in self.tasks.iter_mut() {
            task.start_time = None;
            task.end_time = None;
            task.render_end_time = None;
            task.processed = false;
            task.manual_end_time = false;
        }
        let mut all = self.compile_tasks()?;
        let mut i = 0;
        while !all && i < 10 {
            all = self.compile_tasks()?;
            i += 1;
        }
        Ok(())
    }

    /// Ports `isInvalidDate`: whether a day is excluded.
    pub fn is_invalid_date(&mut self, date: &Date) -> Result<bool, PeleError> {
        let day = date.weekday();
        let weekday = !day.is_nan() && self.excluded_days[day as usize];
        if !self.dated_tokens {
            return Ok(weekday);
        }
        let template = String::from(trim(&self.date_format));
        let date_only = iso_date(date);
        let mut formatted = Some(date_only.clone());
        self.spend(DATED_STEP)?;
        if template != "YYYY-MM-DD" {
            self.spend(FORMAT_STEP + js::len16(&template) as f64 * 8.0)?;
            formatted = format(date, &template);
        }
        let listed = |set: &[String]| -> bool {
            set.contains(&date_only) || formatted.as_ref().is_some_and(|f| set.contains(f))
        };
        if listed(&self.includes) {
            return Ok(false);
        }
        Ok(weekday || listed(&self.excludes))
    }

    fn spend(&mut self, units: f64) -> Result<(), PeleError> {
        self.work += units;
        if self.work > WORK_LIMIT {
            return Err(PeleError::of(
                "The gantt chart takes too much work to schedule.",
                PeleErrorCode::Limit,
                "gantt",
            ));
        }
        Ok(())
    }

    fn parse(&mut self, s: &str, template: &str) -> Result<Option<Date>, PeleError> {
        self.spend((js::len16(template) as f64 + 1.0) * (js::len16(s) as f64 + 1.0))?;
        Ok(parse_strict(s, template, &Date::new(self.now)))
    }

    fn today(&self) -> Date {
        let mut today = Date::new(self.now);
        today.set_hours(0.0, 0.0, 0.0, 0.0);
        today
    }

    fn compile_tasks(&mut self) -> Result<bool, PeleError> {
        let mut all = true;
        for k in 0..self.tasks.len() {
            self.compile_task(k)?;
            all = all && self.tasks[k].processed;
        }
        Ok(all)
    }

    fn compile_task(&mut self, k: usize) -> Result<(), PeleError> {
        match self.tasks[k].raw_start.clone() {
            None => {
                // Mermaid fails with a TypeError when the first task has no start. It starts today here.
                let previous = self.tasks[k]
                    .prev_task_id
                    .clone()
                    .and_then(|id| self.by_id(&id));
                self.tasks[k].start_time = match previous {
                    Some(p) => self.tasks[p].end_time,
                    None => Some(self.today()),
                };
            }
            Some(start) => {
                if let Some(t) = self.get_start_date(&start)? {
                    self.tasks[k].start_time = Some(t);
                }
            }
        }
        let Some(start) = self.tasks[k].start_time else {
            return Ok(());
        };
        let raw_end = self.tasks[k].raw_end.clone();
        self.tasks[k].end_time = self.get_end_date(&start, &raw_end)?;
        if self.tasks[k].end_time.is_none() {
            return Ok(());
        }
        self.tasks[k].processed = true;
        self.tasks[k].manual_end_time = self.parse(&raw_end, "YYYY-MM-DD")?.is_some();
        self.check_task_dates(k)
    }

    /// Ports `references`.
    fn references(&self, list: &str) -> Vec<usize> {
        list.split(' ')
            .filter(|id| !id.is_empty())
            .filter_map(|id| self.by_id(id))
            .collect()
    }

    /// `+date` of an optional date (`undefined` is NaN).
    fn tv(d: Option<Date>) -> f64 {
        d.map(|d| d.time()).unwrap_or(f64::NAN)
    }

    /// Ports `getStartDate`. Ok(None) is `undefined`.
    fn get_start_date(&mut self, raw: &str) -> Result<Option<Date>, PeleError> {
        let s = String::from(trim(raw));
        let template = String::from(trim(&self.date_format));

        if (template == "x" || template == "X")
            && !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
        {
            let date = Date::new(js::number(&s));
            if !date.is_valid() {
                return Err(invalid_date(&s));
            }
            return Ok(Some(date));
        }

        if let Some(list) = reference(&s, "after") {
            let tasks = self.references(list);
            let mut latest: Option<usize> = None;
            for t in tasks {
                if latest.is_none()
                    || Self::tv(self.tasks[t].end_time)
                        > Self::tv(self.tasks[latest.unwrap_or(0)].end_time)
                {
                    latest = Some(t);
                }
            }
            return Ok(match latest {
                Some(l) => self.tasks[l].end_time,
                None => Some(self.today()),
            });
        }

        if let Some(parsed) = self.parse(&s, &template)? {
            return Ok(Some(parsed));
        }

        // What does not fit the format gets a second chance with the JavaScript date parser, as in Mermaid.
        let date = Date::parse(&s);
        if !date.is_valid() || date.full_year() < -10000.0 || date.full_year() > 10000.0 {
            return Err(invalid_date(&s));
        }
        Ok(Some(date))
    }

    /// Ports `getEndDate`.
    fn get_end_date(&mut self, start_time: &Date, raw: &str) -> Result<Option<Date>, PeleError> {
        let s = String::from(trim(raw));

        if let Some(list) = reference(&s, "until") {
            let tasks = self.references(list);
            let mut earliest: Option<usize> = None;
            for t in tasks {
                if earliest.is_none()
                    || Self::tv(self.tasks[t].start_time)
                        < Self::tv(self.tasks[earliest.unwrap_or(0)].start_time)
                {
                    earliest = Some(t);
                }
            }
            return Ok(match earliest {
                Some(e) => self.tasks[e].start_time,
                None => Some(self.today()),
            });
        }

        let template = String::from(trim(&self.date_format));
        if let Some(parsed) = self.parse(&s, &template)? {
            return Ok(Some(if self.inclusive_end_dates {
                add_days(&parsed, 1.0)
            } else {
                parsed
            }));
        }

        let (value, unit) = parse_duration(&s);
        if value.is_nan() {
            return Ok(Some(*start_time));
        }
        let end = add(start_time, value, unit);
        Ok(Some(if end.is_valid() { end } else { *start_time }))
    }

    /// Ports `checkTaskDates`: pushes the end of a task out by one day for
    /// every excluded day it covers.
    fn check_task_dates(&mut self, k: usize) -> Result<(), PeleError> {
        if self.excludes.is_empty() || self.tasks[k].manual_end_time {
            return Ok(());
        }
        let (Some(start), Some(end)) = (self.tasks[k].start_time, self.tasks[k].end_time) else {
            return Ok(());
        };
        let mut cursor = add_days(&start, 1.0);
        let mut end_time = end;
        let max_end_time = add_days(&end_time, 10000.0).time();
        let mut invalid = false;
        let mut render_end_time: Option<f64> = None;
        while cursor.time() <= end_time.time() {
            self.spend(DAY_STEP)?;
            if !invalid {
                render_end_time = Some(end_time.time());
            }
            invalid = self.is_invalid_date(&cursor)?;
            if invalid {
                end_time.set_date(end_time.date() + 1.0);
                if end_time.time() > max_end_time {
                    return Err(PeleError::of(
                        "Failed to find a valid date that was not excluded by `excludes` after 10,000 iterations.",
                        PeleErrorCode::Semantic,
                        "gantt",
                    ));
                }
            }
            cursor.set_date(cursor.date() + 1.0);
        }
        self.tasks[k].end_time = Some(end_time);
        self.tasks[k].render_end_time = render_end_time.map(Date::new);
        Ok(())
    }
}

fn invalid_date(s: &str) -> PeleError {
    PeleError::of(
        format!("Invalid date:{}", s),
        PeleErrorCode::Semantic,
        "gantt",
    )
}
