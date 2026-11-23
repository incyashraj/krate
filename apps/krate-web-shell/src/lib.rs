// Web Shell: the Krate side of a wrapped web app (IC-1004). `krate port
// <folder> --web` packs a built web folder (Vite, React, plain HTML) into
// assets/web with a krate-web.json beside it, and this component runs it:
// one window the size the wrap asked for, the web app filling it through
// krate:ui/app-view, and its localStorage kept in the app's own Krate store
// -- loaded before the page's scripts run, saved as the page changes it --
// so an unmodified web app keeps its data across runs, on every computer,
// behind Krate's permission wall.
//
// The page may also send `{"krate":"call","id":N,"op":"store.get","key":K}`
// (and store.set, store.delete, store.keys) and gets
// `{"krate":"reply","id":N,"ok":...}` or `{..."error":"..."}` back.
#![no_std]
extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use krate::ui::app_view;
use krate::{
    ui::{
        events,
        types::{self, Event},
        window,
    },
    Guest,
};
use serde_json::{json, Map, Value};

/// Where the page's localStorage lives in the app's store.
const LS: &str = "ls:";

struct Config {
    title: String,
    width: u32,
    height: u32,
    folder: String,
    start: String,
}

fn config() -> Config {
    let text = krate::assets::read_text("krate-web.json").unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let s = |k: &str, d: &str| v.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
    let n = |k: &str, d: u32| {
        v.get(k)
            .and_then(Value::as_u64)
            .map(|x| x.clamp(320, 4096) as u32)
            .unwrap_or(d)
    };
    Config {
        title: s("title", "Web App"),
        width: n("width", 1100),
        height: n("height", 760),
        folder: s("folder", "web"),
        start: s("start", "index.html"),
    }
}

/// The page's saved localStorage, as the preload's `localStorage` object.
fn preload() -> String {
    let mut saved = Map::new();
    for key in krate::store::keys().unwrap_or_default() {
        if let Some(name) = key.strip_prefix(LS) {
            if let Ok(Some(value)) = krate::store::get_text(&key) {
                saved.insert(name.to_string(), Value::String(value));
            }
        }
    }
    json!({ "localStorage": saved }).to_string()
}

fn clear_saved() {
    for key in krate::store::keys().unwrap_or_default() {
        if key.starts_with(LS) {
            let _ = krate::store::delete(&key);
        }
    }
}

/// One request the page made of the app; the reply, if it wants one.
fn handle(message: &str) -> Option<String> {
    let v: Value = serde_json::from_str(message).ok()?;
    let key = |name: &str| v.get(name).and_then(Value::as_str).unwrap_or("").to_string();
    match v.get("krate").and_then(Value::as_str)? {
        "ls" => {
            match v.get("op").and_then(Value::as_str)? {
                "set" => {
                    let _ = krate::store::set_text(&(String::from(LS) + &key("key")), &key("value"));
                }
                "remove" => {
                    let _ = krate::store::delete(&(String::from(LS) + &key("key")));
                }
                "clear" => clear_saved(),
                _ => {}
            }
            None
        }
        "call" => {
            let id = v.get("id").cloned().unwrap_or(Value::Null);
            let k = key("key");
            let answer: Result<Value, String> = match v.get("op").and_then(Value::as_str) {
                Some("store.get") => krate::store::get_text(&k)
                    .map(|o| o.map(Value::String).unwrap_or(Value::Null))
                    .map_err(|e| alloc::format!("{e:?}")),
                Some("store.set") => krate::store::set_text(&k, &key("value"))
                    .map(|_| Value::Null)
                    .map_err(|e| alloc::format!("{e:?}")),
                Some("store.delete") => krate::store::delete(&k)
                    .map(|_| Value::Null)
                    .map_err(|e| alloc::format!("{e:?}")),
                Some("store.keys") => krate::store::keys()
                    .map(|ks| {
                        Value::Array(
                            ks.into_iter()
                                .filter(|k| !k.starts_with(LS))
                                .map(Value::String)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .map_err(|e| alloc::format!("{e:?}")),
                other => Err(alloc::format!("no such call: {other:?}")),
            };
            Some(
                match answer {
                    Ok(ok) => json!({ "krate": "reply", "id": id, "ok": ok }),
                    Err(error) => json!({ "krate": "reply", "id": id, "error": error }),
                }
                .to_string(),
            )
        }
        _ => None,
    }
}

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let c = config();
        let Ok(win) = window::create(
            &c.title,
            types::WindowSize {
                width: c.width,
                height: c.height,
            },
        ) else {
            return 1;
        };
        let view = match app_view::open(win, &c.folder, &c.start, &preload()) {
            Ok(view) => view,
            Err(e) => {
                let _ = krate::ui::dialog::message(
                    win,
                    "This app cannot open here",
                    &alloc::format!("{e:?}"),
                );
                return 1;
            }
        };
        loop {
            match events::wait(None) {
                Some(Event::CloseRequested(_)) => return 0,
                Some(Event::Action(id)) if id == view => {
                    for message in app_view::receive(view) {
                        if let Some(reply) = handle(&message) {
                            let _ = app_view::post(view, &reply);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

krate::export!(Component);
