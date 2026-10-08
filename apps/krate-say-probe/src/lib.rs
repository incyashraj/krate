// The smallest app that speaks (krate:speech/synthesis), so the capability
// is proved through the binary a person runs rather than only in the host.
//
//   krate run say-probe.krate -- voices
//   krate run say-probe.krate -- say <text>
//   krate run say-probe.krate -- rate <x>
//
// `say` starts the line, waits until it is heard to start, then stops it:
// the whole contract -- start, speaking, stop -- in well under a second of
// sound. One line of output names the outcome, so a test can tell
// "not granted" from "no voice on this computer" from speech.
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use krate::{
    io::{
        args, stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    speech::synthesis::{self, SayError},
    time::sleep::sleep_millis,
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let mut words = raw.split('\n').filter(|word| !word.is_empty());
        let line = match (words.next(), words.next()) {
            (Some("voices"), None) => match synthesis::voices() {
                Ok(voices) => format!("voices={}", voices.len()),
                Err(err) => describe(err),
            },
            (Some("say"), Some(text)) => say_and_stop(text),
            // krate:audio/playback-queue (K-1011), on a stream that does not
            // exist: the interface links and answers rather than trapping.
            (Some("queue"), None) => match krate::audio::playback_queue::queued(999) {
                Ok(frames) => format!("queue={frames}"),
                Err(krate::audio::types::AudioError::InvalidStream) => String::from("queue=invalid-stream"),
                Err(other) => format!("queue=error {other:?}"),
            },
            (Some("rate"), Some(rate)) => match rate.parse::<f32>() {
                Ok(rate) => match synthesis::say("rate", None, Some(rate)) {
                    Ok(()) => {
                        let _ = synthesis::stop();
                        String::from("rate=ok")
                    }
                    Err(err) => describe(err),
                },
                Err(_) => String::from("usage: rate <number>"),
            },
            _ => String::from("usage: voices | say <text> | rate <x> | queue"),
        };
        if !write_line(&stdio::stdout(), &line) {
            return 20;
        }
        0
    }
}

fn say_and_stop(text: &str) -> String {
    if let Err(err) = synthesis::say(text, None, None) {
        return describe(err);
    }
    let mut waited = 0;
    loop {
        match synthesis::speaking() {
            Ok(true) => break,
            Ok(false) if waited < 3_000 => {
                sleep_millis(10);
                waited += 10;
            }
            Ok(false) => return String::from("said=never-started"),
            Err(err) => return describe(err),
        }
    }
    if let Err(err) = synthesis::stop() {
        return describe(err);
    }
    match synthesis::speaking() {
        Ok(false) => String::from("said=started,stopped"),
        Ok(true) => String::from("said=started,still-speaking"),
        Err(err) => describe(err),
    }
}

fn describe(err: SayError) -> String {
    match err {
        SayError::PermissionDenied => String::from("error=permission-denied"),
        SayError::InvalidRequest(why) => ["error=invalid-request ", &why].concat(),
        SayError::Unsupported(why) => ["error=unsupported ", &why].concat(),
        SayError::Platform(why) => ["error=platform ", &why].concat(),
    }
}

fn write_line(stream: &OutputStream, value: &str) -> bool {
    stream.write_line(value).is_ok() && stream.flush().is_ok()
}

krate::export!(Component);
