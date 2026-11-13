//! Records Check -- the browser Player's database proof (K-1149,
//! crates/adapter-web/player/sql-proof.mjs).
//!
//! Every path of krate:store/sql an app with records uses, in one window,
//! each outcome a line on stdout (the proof reads it) and on the canvas:
//!
//! - a table made if it is missing and one row added per open, so the
//!   count says whether the database survived the tab being reloaded;
//! - an integer past 2^53 and a blob, read back exactly;
//! - `ATTACH` refused as forbidden, as on desktop;
//! - a batch whose last statement fails leaves nothing behind;
//! - clearing the app's key-value settings leaves its database alone.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::io::stdio;
use krate::sql::{self, SqlError, Value};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 220.0;
const BIG: i64 = -9_007_199_254_740_993;

struct Component;

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

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
            grow: 1.0,
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

fn digits(mut n: u64) -> String {
    let mut d = Vec::new();
    loop {
        d.push(b'0' + (n % 10) as u8);
        n /= 10;
        if n == 0 {
            break;
        }
    }
    d.reverse();
    String::from_utf8(d).unwrap_or_default()
}

fn error_name(e: &SqlError) -> &'static str {
    match e {
        SqlError::Denied => "denied",
        SqlError::InvalidStatement(_) => "invalid-statement",
        SqlError::Forbidden(_) => "forbidden",
        SqlError::TooLarge => "too-large",
        SqlError::Io(_) => "io",
    }
}

fn say(lines: &mut Vec<String>, text: String) {
    let _ = stdio::println(&text);
    lines.push(text);
}

fn checks(lines: &mut Vec<String>) {
    let s = String::from;
    match sql::execute("CREATE TABLE IF NOT EXISTS opens (id INTEGER PRIMARY KEY, at TEXT)", &[]) {
        Ok(_) => {}
        Err(e) => return say(lines, s("create: ") + error_name(&e)),
    }
    let _ = sql::execute("INSERT INTO opens (at) VALUES (?1)", &[Value::Text(s("now"))]);
    match sql::query("SELECT count(*) FROM opens", &[]) {
        Ok(r) => match r.rows.first().and_then(|row| row.values.first()) {
            Some(Value::Integer(n)) => say(lines, s("opens: ") + &digits(*n as u64)),
            _ => say(lines, s("opens: no answer")),
        },
        Err(e) => say(lines, s("opens: ") + error_name(&e)),
    }

    let _ = sql::execute("CREATE TABLE IF NOT EXISTS kinds (i INTEGER, b BLOB)", &[]);
    let _ = sql::execute("DELETE FROM kinds", &[]);
    let _ = sql::execute(
        "INSERT INTO kinds VALUES (?1, ?2)",
        &[Value::Integer(BIG), Value::Blob(vec![0, 255, 16, 7])],
    );
    match sql::query("SELECT i, b FROM kinds", &[]) {
        Ok(r) => {
            let row = r.rows.first().map(|row| row.values.clone()).unwrap_or_default();
            let exact = matches!(row.first(), Some(Value::Integer(n)) if *n == BIG)
                && matches!(row.get(1), Some(Value::Blob(b)) if b.as_slice() == [0, 255, 16, 7]);
            say(lines, s(if exact { "values: exact" } else { "values: changed" }));
        }
        Err(e) => say(lines, s("values: ") + error_name(&e)),
    }

    match sql::execute("ATTACH DATABASE 'other.db' AS other", &[]) {
        Ok(_) => say(lines, s("attach: allowed")),
        Err(e) => say(lines, s("attach: ") + error_name(&e)),
    }

    let batch = vec![
        s("CREATE TABLE IF NOT EXISTS batch (x INTEGER)"),
        s("INSERT INTO batch VALUES (1)"),
        s("INSERT INTO missing VALUES (1)"),
    ];
    let failed = sql::transaction(&batch).err().map(|e| error_name(&e)).unwrap_or("ok");
    let left = match sql::query("SELECT count(*) FROM batch", &[]) {
        Ok(_) => "kept",
        Err(_) => "nothing",
    };
    say(lines, s("batch: ") + failed + ", " + left + " left");

    let _ = krate::store::set("theme", b"dark");
    let _ = krate::store::clear();
    match sql::query("SELECT count(*) FROM opens", &[]) {
        Ok(_) => say(lines, s("after kv clear: database kept")),
        Err(e) => say(lines, s("after kv clear: ") + error_name(&e)),
    }
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Records Check",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) else {
            return 1;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return 2;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            return 3;
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut lines: Vec<String> = Vec::new();
        checks(&mut lines);
        say(&mut lines, String::from("done"));

        loop {
            let _ = canvas2d::clear(canvas, color(0.07, 0.09, 0.14));
            let ink = color(0.92, 0.94, 0.98);
            let _ = canvas2d::draw_text(canvas, "Records Check", gfx::Point { x: 16.0, y: 30.0 }, 20.0, ink);
            for (i, l) in lines.iter().enumerate() {
                let _ = canvas2d::draw_text(canvas, l, gfx::Point { x: 16.0, y: 56.0 + 18.0 * i as f32 }, 12.0, ink);
            }
            let _ = canvas2d::present(canvas);

            let mut next = events::wait(Some(16));
            let mut closed = false;
            while let Some(event) = next {
                if let types::Event::CloseRequested(_) = event {
                    closed = true;
                }
                next = events::poll();
            }
            if closed {
                break;
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
