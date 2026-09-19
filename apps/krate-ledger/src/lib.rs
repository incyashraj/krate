//! Ledger -- a finance tracker built by hand to stress the database.
//!
//! CP-C of Plan/Proof-Checkpoints-2026-09-16.md. CP-A proved the runtime can
//! carry a game and CP-B an editor; this asks whether it can carry the shape
//! most business software takes: thousands of rows, filters, sorting, charts,
//! and a store that survives being killed.
//!
//! `store.sql` had two example apps and no stress test before this, so the
//! numbers below are the first ones anybody has.
//!
//! The app: transactions in a real table, a category breakdown, a monthly bar
//! chart, filter-as-you-type, sortable columns, running balances, and CSV
//! export. `--stress` builds 100,000 rows and times insert, query, filtered
//! query, aggregate and sort.
//!
//! `#![no_std]`, and here that is not style but necessity: `store.sql`'s
//! `query` lifts a list, and in a std-linked guest the generated glue reaches
//! std's allocation-error handler, which drags the entire `wasi:*` import set
//! in and fails the import check. krate-contacts is the probe that established
//! this; Ledger follows it.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::sql::{self, Value};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 940.0;
const HEIGHT: f32 = 600.0;

const ROW_H: f32 = 22.0;
const TOP: f32 = 96.0;
const STATUS_H: f32 = 24.0;

/// How many rows the table shows at once. The query asks for exactly this
/// many with LIMIT/OFFSET rather than reading everything and slicing -- which
/// is the difference between a list that works at a hundred rows and one that
/// works at a hundred thousand.
const PAGE: usize = 18;

const CATEGORIES: [&str; 8] = [
    "Groceries",
    "Rent",
    "Transport",
    "Utilities",
    "Dining",
    "Health",
    "Software",
    "Income",
];

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    DateDesc,
    DateAsc,
    AmountDesc,
    AmountAsc,
    Category,
}

impl Sort {
    fn order_by(self) -> &'static str {
        match self {
            Sort::DateDesc => "day DESC, id DESC",
            Sort::DateAsc => "day ASC, id ASC",
            Sort::AmountDesc => "cents DESC",
            Sort::AmountAsc => "cents ASC",
            Sort::Category => "category ASC, day DESC",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Sort::DateDesc => "newest",
            Sort::DateAsc => "oldest",
            Sort::AmountDesc => "largest",
            Sort::AmountAsc => "smallest",
            Sort::Category => "category",
        }
    }

    fn next(self) -> Sort {
        match self {
            Sort::DateDesc => Sort::DateAsc,
            Sort::DateAsc => Sort::AmountDesc,
            Sort::AmountDesc => Sort::AmountAsc,
            Sort::AmountAsc => Sort::Category,
            Sort::Category => Sort::DateDesc,
        }
    }
}

struct Row {
    id: i64,
    day: i64,
    cents: i64,
    category: String,
    note: String,
}

struct App {
    rows: Vec<Row>,
    total_rows: i64,
    matching: i64,
    page: usize,
    sort: Sort,
    filter: String,
    filtering: bool,
    /// Per-category totals for the breakdown, largest first.
    breakdown: Vec<(String, i64)>,
    /// Twelve monthly totals for the bar chart.
    months: [i64; 12],
    balance: i64,
    status: String,
    last_query_us: u64,
}

// ------------------------------------------------------------------ schema

/// Create the table if it is not there, and index what the app sorts and
/// filters on.
///
/// The indexes are the difference between a query that is instant at 100,000
/// rows and one that is a full scan. Written here rather than assumed, because
/// this is the thing CP-C is actually testing.
fn migrate() -> Result<(), sql::SqlError> {
    // A table from an older shape of this app has no `month` column. Rather
    // than fail on the first insert, drop it: this is a stress harness and
    // its data is generated. A real app migrates instead, which is why the
    // choice is stated here rather than assumed.
    let stale = sql::query("SELECT month FROM entries LIMIT 1", &[]).is_err()
        && sql::query("SELECT id FROM entries LIMIT 1", &[]).is_ok();
    if stale {
        let _ = sql::execute("DROP TABLE entries", &[]);
    }
    sql::transaction(&[
        "CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY,
            day INTEGER NOT NULL,
            month INTEGER NOT NULL,
            cents INTEGER NOT NULL,
            category TEXT NOT NULL,
            note TEXT NOT NULL
         )"
        .to_string(),
        // BOTH columns of the ORDER BY, in the same direction. Indexing only
        // `day DESC` while sorting `day DESC, id DESC` made the engine build a
        // temp B-tree for the second term -- "USE TEMP B-TREE FOR LAST TERM OF
        // ORDER BY" in the plan -- which is what made a deep OFFSET cost 27 ms.
        "CREATE INDEX IF NOT EXISTS entries_day_id ON entries(day DESC, id DESC)".to_string(),
        "CREATE INDEX IF NOT EXISTS entries_cents ON entries(cents)".to_string(),
        // GROUP BY category SUM(cents) has to read `cents` for every row. With
        // an index on (category) alone the engine walks the index and then
        // fetches each row from the table -- 100,000 random reads. Adding
        // `cents` to the index makes it COVERING: the aggregate never touches
        // the table at all.
        "CREATE INDEX IF NOT EXISTS entries_cat_cents ON entries(category, cents)".to_string(),
        "CREATE INDEX IF NOT EXISTS entries_month ON entries(month, cents)".to_string(),
        "CREATE INDEX IF NOT EXISTS entries_category ON entries(category, day DESC)".to_string(),
    ])
}

fn count_all() -> i64 {
    sql::query("SELECT COUNT(*) FROM entries", &[])
        .ok()
        .and_then(|r| first_int(&r))
        .unwrap_or(0)
}

fn first_int(r: &sql::QueryResult) -> Option<i64> {
    match r.rows.first()?.values.first()? {
        Value::Integer(v) => Some(*v),
        _ => None,
    }
}

fn int_at(row: &sql::Row, i: usize) -> i64 {
    match row.values.get(i) {
        Some(Value::Integer(v)) => *v,
        Some(Value::Real(v)) => *v as i64,
        _ => 0,
    }
}

fn text_at(row: &sql::Row, i: usize) -> String {
    match row.values.get(i) {
        Some(Value::Text(t)) => t.clone(),
        _ => String::new(),
    }
}

// ------------------------------------------------------------------ loading

impl App {
    /// Fetch one page, the matching count, the breakdown and the chart.
    ///
    /// Five queries rather than one big read: the table asks for PAGE rows,
    /// and the totals are computed by the database rather than by summing in
    /// the app. At 100,000 rows the difference is the whole point.
    fn reload(&mut self) {
        let t0 = clock::monotonic_nanos();

        let like = {
            let mut s = String::from("%");
            s.push_str(&self.filter);
            s.push('%');
            s
        };
        let filtered = !self.filter.is_empty();

        // 1. How many match.
        self.matching = if filtered {
            sql::query(
                "SELECT COUNT(*) FROM entries WHERE category LIKE ?1 OR note LIKE ?1",
                &[Value::Text(like.clone())],
            )
            .ok()
            .and_then(|r| first_int(&r))
            .unwrap_or(0)
        } else {
            self.total_rows
        };

        // 2. One page of rows.
        let offset = (self.page * PAGE) as i64;
        let mut stmt = String::from("SELECT id, day, cents, category, note FROM entries");
        if filtered {
            stmt.push_str(" WHERE category LIKE ?1 OR note LIKE ?1");
        }
        stmt.push_str(" ORDER BY ");
        stmt.push_str(self.sort.order_by());
        stmt.push_str(" LIMIT ");
        stmt.push_str(&i64_str(PAGE as i64));
        stmt.push_str(" OFFSET ");
        stmt.push_str(&i64_str(offset));

        let params: Vec<Value> = if filtered {
            alloc::vec![Value::Text(like.clone())]
        } else {
            Vec::new()
        };
        self.rows.clear();
        if let Ok(r) = sql::query(&stmt, &params) {
            for row in &r.rows {
                self.rows.push(Row {
                    id: int_at(row, 0),
                    day: int_at(row, 1),
                    cents: int_at(row, 2),
                    category: text_at(row, 3),
                    note: text_at(row, 4),
                });
            }
        }

        // 3. Category breakdown, computed by the database.
        self.breakdown.clear();
        let bstmt = if filtered {
            "SELECT category, SUM(cents) FROM entries
             WHERE category LIKE ?1 OR note LIKE ?1
             GROUP BY category ORDER BY SUM(cents) ASC"
        } else {
            "SELECT category, SUM(cents) FROM entries
             GROUP BY category ORDER BY SUM(cents) ASC"
        };
        if let Ok(r) = sql::query(bstmt, &params) {
            for row in &r.rows {
                self.breakdown.push((text_at(row, 0), int_at(row, 1)));
            }
        }

        // 4. Twelve months of totals for the chart.
        //
        // Grouping by `day / 30 % 12` computes an expression per row, so no
        // index can help and the whole table is read -- measured at 27 ms of
        // a 27 ms reload. A stored `month` column with its own index turns
        // the same chart into an indexed group. The lesson generalises: an
        // expression in GROUP BY is a full scan wearing a disguise.
        self.months = [0; 12];
        if let Ok(r) = sql::query(
            "SELECT month, SUM(cents) FROM entries GROUP BY month",
            &[],
        ) {
            for row in &r.rows {
                let m = int_at(row, 0).clamp(0, 11) as usize;
                self.months[m] = int_at(row, 1);
            }
        }

        // 5. The balance.
        self.balance = sql::query("SELECT COALESCE(SUM(cents), 0) FROM entries", &[])
            .ok()
            .and_then(|r| first_int(&r))
            .unwrap_or(0);

        self.last_query_us = clock::monotonic_nanos().saturating_sub(t0) / 1_000;
    }
}

// --------------------------------------------------------------- formatting

fn i64_str(v: i64) -> String {
    let mut s = String::new();
    if v < 0 {
        s.push('-');
    }
    let mut u = v.unsigned_abs();
    let mut digits = [0u8; 24];
    let mut n = 0;
    if u == 0 {
        s.push('0');
        return s;
    }
    while u > 0 {
        digits[n] = b'0' + (u % 10) as u8;
        u /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(digits[n] as char);
    }
    s
}

/// Cents as money, with a thousands separator, because a column of raw
/// integers is unreadable at a glance.
fn money(cents: i64) -> String {
    let neg = cents < 0;
    let a = cents.unsigned_abs();
    let whole = a / 100;
    let frac = a % 100;
    let w = i64_str(whole as i64);
    let mut grouped = String::new();
    let bytes = w.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*b as char);
    }
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&grouped);
    s.push('.');
    if frac < 10 {
        s.push('0');
    }
    s.push_str(&i64_str(frac as i64));
    s
}

/// Day number as `YYYY-MM-DD`, counting from 2024-01-01. Deliberately naive:
/// this is a stress harness, not a calendar library.
fn day_str(day: i64) -> String {
    let year = 2024 + day / 365;
    let rest = day % 365;
    let month = rest / 30 + 1;
    let dom = rest % 30 + 1;
    let mut s = i64_str(year);
    s.push('-');
    if month < 10 {
        s.push('0');
    }
    s.push_str(&i64_str(month));
    s.push('-');
    if dom < 10 {
        s.push('0');
    }
    s.push_str(&i64_str(dom));
    s
}

// ------------------------------------------------------------------- colour

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

// --------------------------------------------------------------------- draw

fn draw(a: &App, canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, rgb(0.08, 0.09, 0.12))?;

    // Header.
    canvas2d::fill_rect(canvas, rect(0.0, 0.0, WIDTH, 58.0), rgb(0.12, 0.14, 0.18))?;
    canvas2d::draw_text(
        canvas,
        "Ledger",
        gfx::Point { x: 16.0, y: 26.0 },
        18.0,
        rgb(0.90, 0.93, 0.98),
    )?;

    let mut sub = String::new();
    sub.push_str(&i64_str(a.total_rows));
    sub.push_str(" entries   balance ");
    sub.push_str(&money(a.balance));
    canvas2d::draw_text(
        canvas,
        &sub,
        gfx::Point { x: 16.0, y: 46.0 },
        12.0,
        rgb(0.58, 0.66, 0.78),
    )?;

    // The monthly chart, top right: twelve bars scaled to the largest.
    let peak = a.months.iter().copied().map(|v| v.abs()).max().unwrap_or(1).max(1);
    let cx = WIDTH - 300.0;
    for m in 0..12 {
        let v = a.months[m];
        let h = (v.abs() as f32 / peak as f32) * 34.0;
        let x = cx + m as f32 * 23.0;
        let col = if v < 0 {
            rgb(0.85, 0.42, 0.45)
        } else {
            rgb(0.40, 0.80, 0.60)
        };
        canvas2d::fill_rect(canvas, rect(x, 44.0 - h, 16.0, h.max(1.0)), col)?;
    }
    canvas2d::draw_text(
        canvas,
        "by month",
        gfx::Point { x: cx, y: 56.0 },
        10.0,
        rgb(0.45, 0.52, 0.62),
    )?;

    // Column headers.
    canvas2d::fill_rect(canvas, rect(0.0, 58.0, WIDTH, TOP - 58.0), rgb(0.10, 0.12, 0.15))?;
    let mut head = String::from("sorted by ");
    head.push_str(a.sort.label());
    head.push_str("   [s] change   [f] filter   [n/p] page   [e] export");
    canvas2d::draw_text(
        canvas,
        &head,
        gfx::Point { x: 16.0, y: 74.0 },
        11.0,
        rgb(0.52, 0.60, 0.72),
    )?;

    for (label, x) in [
        ("DATE", 16.0),
        ("CATEGORY", 110.0),
        ("NOTE", 240.0),
        ("AMOUNT", 560.0),
    ] {
        canvas2d::draw_text(
            canvas,
            label,
            gfx::Point { x, y: 92.0 },
            10.0,
            rgb(0.42, 0.48, 0.58),
        )?;
    }

    // Rows.
    for (i, r) in a.rows.iter().enumerate() {
        let y = TOP + i as f32 * ROW_H;
        if i % 2 == 0 {
            canvas2d::fill_rect(
                canvas,
                rect(0.0, y, 660.0, ROW_H),
                rgba(1.0, 1.0, 1.0, 0.018),
            )?;
        }
        let ty = y + 15.0;
        canvas2d::draw_text(
            canvas,
            &day_str(r.day),
            gfx::Point { x: 16.0, y: ty },
            11.5,
            rgb(0.60, 0.66, 0.76),
        )?;
        canvas2d::draw_text(
            canvas,
            &r.category,
            gfx::Point { x: 110.0, y: ty },
            11.5,
            rgb(0.55, 0.78, 0.92),
        )?;
        canvas2d::draw_text(
            canvas,
            &r.note,
            gfx::Point { x: 240.0, y: ty },
            11.5,
            rgb(0.78, 0.82, 0.88),
        )?;
        let amount = money(r.cents);
        let m = canvas2d::measure_text(canvas, &amount, 11.5)?;
        let ink = if r.cents < 0 {
            rgb(0.90, 0.50, 0.52)
        } else {
            rgb(0.48, 0.85, 0.62)
        };
        canvas2d::draw_text(
            canvas,
            &amount,
            gfx::Point {
                x: 640.0 - m.width,
                y: ty,
            },
            11.5,
            ink,
        )?;
    }

    // Category breakdown down the right side.
    let bx = 690.0;
    canvas2d::fill_rect(
        canvas,
        rect(bx - 12.0, TOP - 12.0, WIDTH - bx + 12.0, HEIGHT - TOP - STATUS_H + 12.0),
        rgba(1.0, 1.0, 1.0, 0.025),
    )?;
    canvas2d::draw_text(
        canvas,
        "WHERE IT GOES",
        gfx::Point { x: bx, y: TOP + 6.0 },
        10.0,
        rgb(0.42, 0.48, 0.58),
    )?;
    let widest = a
        .breakdown
        .iter()
        .map(|(_, v)| v.abs())
        .max()
        .unwrap_or(1)
        .max(1);
    for (i, (cat, total)) in a.breakdown.iter().enumerate() {
        let y = TOP + 26.0 + i as f32 * 30.0;
        if y > HEIGHT - STATUS_H - 24.0 {
            break;
        }
        canvas2d::draw_text(
            canvas,
            cat,
            gfx::Point { x: bx, y },
            11.0,
            rgb(0.76, 0.82, 0.90),
        )?;
        let amt = money(*total);
        let m = canvas2d::measure_text(canvas, &amt, 10.5)?;
        canvas2d::draw_text(
            canvas,
            &amt,
            gfx::Point {
                x: WIDTH - 16.0 - m.width,
                y,
            },
            10.5,
            if *total < 0 {
                rgb(0.85, 0.50, 0.52)
            } else {
                rgb(0.48, 0.85, 0.62)
            },
        )?;
        let w = (total.abs() as f32 / widest as f32) * (WIDTH - bx - 16.0);
        canvas2d::fill_rect(
            canvas,
            rect(bx, y + 5.0, w.max(2.0), 5.0),
            if *total < 0 {
                rgba(0.85, 0.45, 0.48, 0.55)
            } else {
                rgba(0.40, 0.80, 0.58, 0.55)
            },
        )?;
    }

    // Status bar.
    let sy = HEIGHT - STATUS_H;
    canvas2d::fill_rect(canvas, rect(0.0, sy, WIDTH, STATUS_H), rgb(0.12, 0.14, 0.18))?;
    let mut left = String::new();
    if a.filtering {
        left.push_str("filter: ");
        left.push_str(&a.filter);
        left.push_str("_   ");
    }
    left.push_str(&i64_str(a.matching));
    left.push_str(" matching   page ");
    left.push_str(&i64_str(a.page as i64 + 1));
    left.push_str("   query ");
    left.push_str(&i64_str(a.last_query_us as i64));
    left.push_str("us");
    canvas2d::draw_text(
        canvas,
        &left,
        gfx::Point { x: 16.0, y: sy + 16.0 },
        11.0,
        rgb(0.58, 0.66, 0.78),
    )?;

    if !a.status.is_empty() {
        let m = canvas2d::measure_text(canvas, &a.status, 11.0)?;
        canvas2d::draw_text(
            canvas,
            &a.status,
            gfx::Point {
                x: WIDTH - 16.0 - m.width,
                y: sy + 16.0,
            },
            11.0,
            rgb(0.50, 0.85, 0.60),
        )?;
    }

    canvas2d::present(canvas)
}

// --------------------------------------------------------------------- seed

/// Deterministic pseudo-random, so a stress run is reproducible and a
/// screenshot is the same every time.
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 16
}

/// Insert `n` rows in batches inside transactions.
///
/// One statement per row outside a transaction is the naive shape and is
/// pathologically slow in any SQL engine -- each becomes its own commit. This
/// batches, which is what the API's `transaction` is for.
///
/// `batch` must be at most 1,000: `MAX_TRANSACTION_STATEMENTS` in the host
/// bounds one transaction so a batch cannot run unboundedly, and asking for
/// 2,000 comes back `TooLarge`. The limit is right; it is just not
/// discoverable from the SDK, which says only "run several statements as one
/// unit".
fn seed(n: usize, batch: usize) -> Result<u64, sql::SqlError> {
    let t0 = clock::monotonic_nanos();
    let mut state = 0x2026_0916_u64;
    let mut i = 0;
    while i < n {
        let upto = (i + batch).min(n);
        let mut stmts: Vec<String> = Vec::with_capacity(upto - i);
        for k in i..upto {
            let cat = CATEGORIES[(lcg(&mut state) % 8) as usize];
            let income = cat == "Income";
            let mag = (lcg(&mut state) % 40_000) as i64 + 500;
            let cents = if income { mag * 8 } else { -mag };
            let day = (lcg(&mut state) % 720) as i64;
            let mut s = String::from(
                "INSERT INTO entries (id, day, month, cents, category, note) VALUES (",
            );
            s.push_str(&i64_str(k as i64 + 1));
            s.push(',');
            s.push_str(&i64_str(day));
            s.push(',');
            s.push_str(&i64_str(day / 30 % 12));
            s.push(',');
            s.push_str(&i64_str(cents));
            s.push_str(",'");
            s.push_str(cat);
            s.push_str("','");
            s.push_str(match (lcg(&mut state) % 6) as usize {
                0 => "weekly shop",
                1 => "monthly bill",
                2 => "one off",
                3 => "subscription",
                4 => "reimbursed",
                _ => "adjustment",
            });
            s.push_str(" #");
            s.push_str(&i64_str(k as i64 + 1));
            s.push_str("')");
            stmts.push(s);
        }
        sql::transaction(&stmts)?;
        i = upto;
    }
    Ok(clock::monotonic_nanos().saturating_sub(t0) / 1_000_000)
}

// --------------------------------------------------------------------- main

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"ledger: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let has = |name: &[u8]| raw.as_bytes().split(|b| *b == b'\n').any(|a| a == name);
        let quick = has(b"quick") || has(b"--quick");
        let stress = has(b"stress");

        if migrate().is_err() {
            return fail(b"migrate");
        }

        let mut app = App {
            rows: Vec::new(),
            total_rows: 0,
            matching: 0,
            page: 0,
            sort: Sort::DateDesc,
            filter: String::new(),
            filtering: false,
            breakdown: Vec::new(),
            months: [0; 12],
            balance: 0,
            status: String::new(),
            last_query_us: 0,
        };

        // The stress run answers CP-C's question and exits. No window is
        // needed for it, but one is opened anyway so the run is the same
        // shape as a real one.
        if stress {
            let out = stdio::stdout();
            let existing = count_all();
            if existing < 100_000 {
                let _ = sql::execute("DELETE FROM entries", &[]);
                match seed(100_000, 1_000) {
                    Ok(ms) => {
                        let mut m = String::from("ledger-stress: insert 100000 rows in ");
                        m.push_str(&i64_str(ms as i64));
                        m.push_str("ms\n");
                        let _ = out.write(m.as_bytes());
                    }
                    Err(e) => {
                        let out = stdio::stdout();
                        let _ = out.write(b"seed error: ");
                        let _ = out.write(match e {
                            sql::SqlError::Denied => b"Denied" as &[u8],
                            sql::SqlError::InvalidStatement(_) => b"InvalidStatement",
                            sql::SqlError::Forbidden(_) => b"Forbidden",
                            sql::SqlError::TooLarge => b"TooLarge",
                            sql::SqlError::Io(_) => b"Io",
                            _ => b"other",
                        });
                        let _ = out.write(b"\n");
                        return fail(b"seed");
                    }
                }
            }

            app.total_rows = count_all();

            // Each timing is the whole round trip an app actually pays:
            // statement across the boundary, engine work, rows lifted back.
            let time = |label: &str, f: &dyn Fn()| {
                let t = clock::monotonic_nanos();
                f();
                let us = clock::monotonic_nanos().saturating_sub(t) / 1_000;
                let mut m = String::from("  ");
                m.push_str(label);
                m.push_str(": ");
                m.push_str(&i64_str(us as i64));
                m.push_str("us\n");
                let out = stdio::stdout();
                let _ = out.write(m.as_bytes());
            };

            time("count all", &|| {
                let _ = count_all();
            });
            time("page of 18, sorted by date", &|| {
                let _ = sql::query(
                    "SELECT id, day, cents, category, note FROM entries
                     ORDER BY day DESC, id DESC LIMIT 18",
                    &[],
                );
            });
            time("page 2500 deep (OFFSET 45000)", &|| {
                let _ = sql::query(
                    "SELECT id, day, cents, category, note FROM entries
                     ORDER BY day DESC, id DESC LIMIT 18 OFFSET 45000",
                    &[],
                );
            });
            time("sort by amount, top 18", &|| {
                let _ = sql::query(
                    "SELECT id, day, cents, category, note FROM entries
                     ORDER BY cents ASC LIMIT 18",
                    &[],
                );
            });
            time("filter LIKE across 100k", &|| {
                let _ = sql::query(
                    "SELECT COUNT(*) FROM entries WHERE category LIKE ?1 OR note LIKE ?1",
                    &[Value::Text(String::from("%subscription%"))],
                );
            });
            time("GROUP BY category", &|| {
                let _ = sql::query(
                    "SELECT category, SUM(cents) FROM entries GROUP BY category",
                    &[],
                );
            });
            time("SUM all", &|| {
                let _ = sql::query("SELECT COALESCE(SUM(cents),0) FROM entries", &[]);
            });
            time("lift 1000 rows across the boundary", &|| {
                let _ = sql::query(
                    "SELECT id, day, cents, category, note FROM entries LIMIT 1000",
                    &[],
                );
            });

            let mut m = String::from("ledger-stress: ");
            m.push_str(&i64_str(app.total_rows));
            m.push_str(" rows total\n");
            let _ = out.write(m.as_bytes());
            return 0;
        }

        // A first run with an empty table gets something to look at.
        app.total_rows = count_all();
        if app.total_rows == 0 {
            let _ = seed(400, 400);
            app.total_rows = count_all();
        }

        let win = match window::create(
            "Ledger",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return fail(b"set_root");
        }
        if tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
        {
            return fail(b"upsert canvas");
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => return fail(b"canvas2d::bind"),
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        app.reload();
        let _ = draw(&app, canvas);

        if quick {
            let out = stdio::stdout();
            let mut m = String::from("ledger: ");
            m.push_str(&i64_str(app.total_rows));
            m.push_str(" rows, query ");
            m.push_str(&i64_str(app.last_query_us as i64));
            m.push_str("us\n");
            let _ = out.write(m.as_bytes());
            return 0;
        }

        loop {
            let mut dirty = false;
            while let Some(ev) = events::poll() {
                match ev {
                    types::Event::CloseRequested(id) => {
                        let _ = window::close(id);
                        return 0;
                    }
                    types::Event::TextInput(s) if app.filtering => {
                        for ch in s.chars() {
                            if ch >= ' ' {
                                app.filter.push(ch);
                            }
                        }
                        app.page = 0;
                        dirty = true;
                    }
                    types::Event::Key(k) if k.pressed => match k.key.as_str() {
                        "f" | "F" if !app.filtering => {
                            app.filtering = true;
                            app.filter.clear();
                            dirty = true;
                        }
                        "Escape" => {
                            app.filtering = false;
                            app.filter.clear();
                            app.page = 0;
                            dirty = true;
                        }
                        "Backspace" if app.filtering => {
                            app.filter.pop();
                            app.page = 0;
                            dirty = true;
                        }
                        "s" | "S" if !app.filtering => {
                            app.sort = app.sort.next();
                            dirty = true;
                        }
                        "n" | "N" if !app.filtering => {
                            if ((app.page + 1) * PAGE) < app.matching as usize {
                                app.page += 1;
                                dirty = true;
                            }
                        }
                        "p" | "P" if !app.filtering => {
                            if app.page > 0 {
                                app.page -= 1;
                                dirty = true;
                            }
                        }
                        "e" | "E" if !app.filtering => {
                            // CSV to stdout: the export a data app owes you,
                            // without needing a file capability to prove it.
                            let out = stdio::stdout();
                            let _ = out.write(b"date,category,note,amount\n");
                            if let Ok(r) = sql::query(
                                "SELECT day, category, note, cents FROM entries
                                 ORDER BY day DESC LIMIT 1000",
                                &[],
                            ) {
                                for row in &r.rows {
                                    let mut line = day_str(int_at(row, 0));
                                    line.push(',');
                                    line.push_str(&text_at(row, 1));
                                    line.push_str(",\"");
                                    line.push_str(&text_at(row, 2));
                                    line.push_str("\",");
                                    line.push_str(&money(int_at(row, 3)));
                                    line.push('\n');
                                    let _ = out.write(line.as_bytes());
                                }
                            }
                            app.status.clear();
                            app.status.push_str("exported 1000 rows as CSV");
                            dirty = true;
                        }
                        _ => {}
                    },
                    types::Event::Wheel(w) => {
                        if w.dy < -10.0 && ((app.page + 1) * PAGE) < app.matching as usize {
                            app.page += 1;
                            dirty = true;
                        } else if w.dy > 10.0 && app.page > 0 {
                            app.page -= 1;
                            dirty = true;
                        }
                    }
                    _ => {}
                }
            }

            if dirty {
                app.reload();
            }
            if draw(&app, canvas).is_err() {
                break;
            }
        }
        0
    }
}

krate::export!(Component);
