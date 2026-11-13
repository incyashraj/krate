// The smallest app that uses an organisation's team data (store.org,
// IC-951), so the capability is proved through the binary a person runs
// rather than only in the host.
//
//   krate run app.krate -- put tasks t1 title=Order done?=true n#=3 note!
//   krate run app.krate -- get tasks t1 then sync then who
//
// Several steps run in order, separated by `then`. Each prints one line
// naming the step and its outcome, so a test can tell a refusal from an
// absence from a value. A field is `name=text`, `name?=true|false`,
// `name#=<number>` or `name!` (remove it); `fill <c> <id> <field> <bytes>`
// writes that many bytes of text, for the limits.
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    io::{
        args, stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    org::{self, OrgError, Value},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let words: Vec<&str> = raw.split('\n').filter(|w| !w.is_empty()).collect();
        let stdout = stdio::stdout();
        for step in words.split(|w| *w == "then") {
            let line = run_step(step);
            if !write_line(&stdout, &line) {
                return 20;
            }
        }
        0
    }
}

fn run_step(step: &[&str]) -> String {
    match step {
        ["put", collection, id, fields @ ..] => {
            let fields: Vec<(&str, Value)> = fields.iter().map(|f| field_of(f)).collect();
            match org::put(collection, id, &fields) {
                Ok(()) => format!("put {id} ok"),
                Err(err) => format!("put {id} {}", describe(err)),
            }
        }
        ["fill", collection, id, field, bytes] => {
            let n: usize = bytes.parse().unwrap_or(0);
            let text: String = core::iter::repeat_n('x', n).collect();
            match org::put(collection, id, &[(field, Value::Text(text))]) {
                Ok(()) => format!("fill {id} ok"),
                Err(err) => format!("fill {id} {}", describe(err)),
            }
        }
        ["get", collection, id] => match org::get(collection, id) {
            Ok(Some(entry)) => {
                let fields: Vec<String> = entry
                    .fields
                    .iter()
                    .map(|(name, value)| format!("{name}={}", shown(value)))
                    .collect();
                format!("get {id} {} by={}", fields.join(";"), entry.changed_by)
            }
            Ok(None) => format!("get {id} none"),
            Err(err) => format!("get {id} {}", describe(err)),
        },
        ["delete", collection, id] => match org::delete(collection, id) {
            Ok(()) => format!("delete {id} ok"),
            Err(err) => format!("delete {id} {}", describe(err)),
        },
        ["list", collection] => match org::all(collection) {
            Ok(entries) => {
                let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
                format!("list {}", ids.join(","))
            }
            Err(err) => format!("list {}", describe(err)),
        },
        ["sync"] => match org::sync() {
            Ok(state) => format!(
                "sync online={} changed={} pending={}",
                state.online, state.changed, state.pending
            ),
            Err(err) => format!("sync {}", describe(err)),
        },
        ["who"] => match org::who() {
            Ok(Some(me)) => format!("who {} {} {}", me.name, me.role, me.org),
            Ok(None) => String::from("who none"),
            Err(err) => format!("who {}", describe(err)),
        },
        ["status"] => match org::status() {
            Ok(s) => format!(
                "status {} pending={} reached={} org={}",
                match s.link {
                    org::Link::Synced => "synced",
                    org::Link::Syncing => "syncing",
                    org::Link::Offline => "offline",
                    org::Link::WaitingForKey => "waiting-for-key",
                },
                s.pending,
                s.reached_at.is_some(),
                s.org
            ),
            Err(err) => format!("status {}", describe(err)),
        },
        _ => String::from("usage: put|fill|get|delete|list|sync|who|status, steps joined by `then`"),
    }
}

fn field_of(spec: &str) -> (&str, Value) {
    if let Some(name) = spec.strip_suffix('!') {
        return (name, Value::Null);
    }
    let (name, value) = spec.split_once('=').unwrap_or((spec, ""));
    if let Some(name) = name.strip_suffix('?') {
        return (name, Value::Boolean(value == "true"));
    }
    if let Some(name) = name.strip_suffix('#') {
        return (name, Value::Number(value.parse().unwrap_or(0.0)));
    }
    (name, Value::Text(String::from(value)))
}

fn shown(value: &Value) -> String {
    match value {
        Value::Null => String::from("null"),
        Value::Text(text) => text.clone(),
        Value::Number(n) => format!("{n}"),
        Value::Boolean(b) => format!("{b}"),
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
    }
}

fn describe(err: OrgError) -> String {
    match err {
        OrgError::Denied => String::from("error=denied"),
        OrgError::NotAnOrgApp => String::from("error=not-an-org-app"),
        OrgError::Invalid(words) => format!("error=invalid {words}"),
        OrgError::TooLarge => String::from("error=too-large"),
        OrgError::OverQuota => String::from("error=over-quota"),
        OrgError::Refused(refusal) => format!("error=refused reason={}", refusal.reason),
        OrgError::Unsupported => String::from("error=unsupported"),
        OrgError::Io(words) => format!("error=io {words}"),
    }
}

fn write_line(stream: &OutputStream, value: &str) -> bool {
    stream.write_line(value).is_ok() && stream.flush().is_ok()
}

krate::export!(Component);
