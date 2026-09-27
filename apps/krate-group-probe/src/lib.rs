// The smallest app that uses a shared group (IC-738), so the capability is
// proved through the binary a person runs rather than only in the host.
//
//   krate run krate-group-probe.krate -- set <key> <value>
//   krate run krate-group-probe.krate -- get <key>
//
// It prints exactly one line naming the outcome, so a test can tell
// "refused, not a member" from "refused, not granted" from a real value.
#![no_std]
extern crate alloc;

use alloc::string::String;
use krate::{
    group::{self, GroupError},
    io::{
        args, stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    Guest,
};

const GROUP: &str = "family-budget";

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let mut words = raw.split('\n').filter(|word| !word.is_empty());
        let stdout = stdio::stdout();
        let line = match (words.next(), words.next(), words.next()) {
            (Some("set"), Some(key), Some(value)) => match group::set_text(GROUP, key, value) {
                Ok(()) => String::from("set=ok"),
                Err(err) => describe(err),
            },
            (Some("get"), Some(key), None) => match group::get_text(GROUP, key) {
                Ok(Some(value)) => ["get=", &value].concat(),
                Ok(None) => String::from("get=none"),
                Err(err) => describe(err),
            },
            _ => String::from("usage: set <key> <value> | get <key>"),
        };
        if !write_line(&stdout, &line) {
            return 20;
        }
        0
    }
}

fn describe(err: GroupError) -> String {
    match err {
        GroupError::Denied => String::from("error=denied"),
        GroupError::NotAMember => String::from("error=not-a-member"),
        GroupError::InvalidKey => String::from("error=invalid-key"),
        GroupError::TooLarge => String::from("error=too-large"),
        GroupError::Io(message) => ["error=io ", &message].concat(),
    }
}

fn write_line(stream: &OutputStream, value: &str) -> bool {
    stream.write_line(value).is_ok() && stream.flush().is_ok()
}

krate::export!(Component);
