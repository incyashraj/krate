//! What a desktop app takes for granted, on an Android phone (IC-918):
//! links, notifications, the microphone's prompt and speaking aloud, through
//! the player activity's Phone.java. The runtime calls these where a desktop
//! would run a helper process; each answers a plain sentence when it cannot.

/// What the phone's own permission prompt answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    Granted,
    Refused,
}

#[cfg(target_os = "android")]
mod real {
    use super::Asked;
    use crate::java::{string, wait_for, with_activity};
    use jni::objects::{JObjectArray, JValue};
    use std::time::Duration;

    /// Ask one of the activity's krate*Ask prompts and wait for the person.
    pub(crate) fn ask(method: &str) -> Asked {
        let answer = wait_for(Duration::from_secs(10 * 60), || {
            match with_activity("asking the phone", |env, activity| {
                env.call_method(activity, method, "()I", &[])?.i()
            }) {
                Some(2) => None,
                Some(0) => Some(Asked::Granted),
                _ => Some(Asked::Refused),
            }
        });
        answer.unwrap_or(Asked::Refused)
    }

    /// A Java method of (String...) -> String where null means done.
    fn done_or_why(what: &str, method: &str, sig: &str, args: &[&str]) -> Result<(), String> {
        let why = with_activity(what, |env, activity| {
            let mut strings = Vec::new();
            for a in args {
                strings.push(env.new_string(a)?);
            }
            let values: Vec<JValue> = strings.iter().map(|s| JValue::Object(s)).collect();
            let why = env.call_method(activity, method, sig, &values)?.l()?;
            string(env, why)
        })
        .ok_or_else(|| format!("{what} failed inside Krate"))?;
        match why {
            None => Ok(()),
            Some(why) => Err(why),
        }
    }

    /// Pixels the status bar, gesture bar and cutout cover: top, right,
    /// bottom, left.
    pub fn covered_edges() -> (f32, f32, f32, f32) {
        with_activity("reading the covered edges", |env, activity| {
            let all = env
                .call_method(activity, "krateCovered", "()[I", &[])?
                .l()?;
            let all = jni::objects::JIntArray::from(all);
            let mut edges = [0i32; 4];
            if env.get_array_length(&all)? == 4 {
                env.get_int_array_region(&all, 0, &mut edges)?;
            }
            Ok((
                edges[0] as f32,
                edges[1] as f32,
                edges[2] as f32,
                edges[3] as f32,
            ))
        })
        .unwrap_or((0.0, 0.0, 0.0, 0.0))
    }

    /// Open a page in the in-app browser, a note shown first; false when the
    /// phone has no browser.
    pub fn open_tab(url: &str, note: &str) -> bool {
        with_activity("opening an in-app tab", |env, activity| {
            let url = env.new_string(url)?;
            let note = env.new_string(note)?;
            env.call_method(
                activity,
                "krateOpenTab",
                "(Ljava/lang/String;Ljava/lang/String;)Z",
                &[JValue::Object(&url), JValue::Object(&note)],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    /// Whether the in-app tab is still over Krate.
    pub fn tab_open() -> bool {
        with_activity("asking about the in-app tab", |env, activity| {
            env.call_method(activity, "krateTabOpen", "()Z", &[])?.z()
        })
        .unwrap_or(false)
    }

    /// Close the in-app tab: Krate comes back to the front.
    pub fn close_tab() {
        let _ = with_activity("closing the in-app tab", |env, activity| {
            env.call_method(activity, "krateCloseTab", "()V", &[])?.v()
        });
    }

    /// The phone's name as its owner sees it, for Krate's profile screen.
    pub fn device_name() -> String {
        with_activity("reading the phone's name", |env, activity| {
            let name = env
                .call_method(activity, "krateDeviceName", "()Ljava/lang/String;", &[])?
                .l()?;
            string(env, name)
        })
        .flatten()
        .unwrap_or_else(|| "This phone".to_string())
    }

    /// Ask for dark bar icons over a light top or bottom, light over dark.
    pub fn light_bars(top: bool, bottom: bool) {
        let _ = with_activity("colouring the system bars", |env, activity| {
            env.call_method(
                activity,
                "krateLightBars",
                "(ZZ)V",
                &[
                    jni::objects::JValue::Bool(top.into()),
                    jni::objects::JValue::Bool(bottom.into()),
                ],
            )?
            .v()
        });
    }

    /// Whether a screen reader that explores by touch (TalkBack) is on.
    pub fn screen_reader_on() -> bool {
        with_activity("asking about the screen reader", |env, activity| {
            env.call_method(activity, "krateScreenReaderOn", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    pub fn read_clipboard() -> Result<String, String> {
        with_activity("reading the clipboard", |env, activity| {
            let text = env
                .call_method(activity, "krateClipRead", "()Ljava/lang/String;", &[])?
                .l()?;
            string(env, text)
        })
        .map(Option::unwrap_or_default)
        .ok_or_else(|| "the clipboard could not be read".to_string())
    }

    pub fn write_clipboard(text: &str) -> Result<(), String> {
        let wrote = with_activity("writing the clipboard", |env, activity| {
            let text = env.new_string(text)?;
            env.call_method(
                activity,
                "krateClipWrite",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&text)],
            )?
            .z()
        });
        match wrote {
            Some(true) => Ok(()),
            _ => Err("the clipboard could not be written".to_string()),
        }
    }

    /// Hand a link to the app that opens it (a browser, Mail, the owner).
    pub fn open_url(url: &str) -> Result<(), String> {
        done_or_why(
            "opening a link",
            "krateOpenUrl",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[url],
        )
    }

    /// Post a notification, the app's name as its sender line, asking the
    /// phone first on Android 13 and up.
    pub fn notify(app: &str, title: &str, body: &str) -> Result<(), String> {
        if ask("krateNotifyAsk") == Asked::Refused {
            return Err("notifications were not allowed for Krate".to_string());
        }
        done_or_why(
            "posting a notification",
            "krateNotify",
            "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
            &[app, title, body],
        )
    }

    /// The phone's microphone question, asked before the first recording.
    pub fn ask_microphone() -> Asked {
        ask("krateMicAsk")
    }

    /// Start the phone's speech engine and wait for it: Ok once it can
    /// speak. Called from the speech worker's thread.
    pub fn speech_ready() -> Result<(), String> {
        for _ in 0..200 {
            match with_activity("starting speech", |env, activity| {
                env.call_method(activity, "krateSpeechReady", "()I", &[])?
                    .i()
            }) {
                Some(1) => return Ok(()),
                Some(0) => std::thread::sleep(Duration::from_millis(50)),
                _ => break,
            }
        }
        Err("this phone has no speech engine".to_string())
    }

    /// Each voice as (id, name, language).
    pub fn voices() -> Vec<(String, String, String)> {
        with_activity("listing voices", |env, activity| {
            let all = env
                .call_method(activity, "krateVoices", "()[Ljava/lang/String;", &[])?
                .l()?;
            let all = JObjectArray::from(all);
            let n = env.get_array_length(&all)?;
            let mut out = Vec::new();
            for i in 0..n {
                let one = env.get_object_array_element(&all, i)?;
                if let Some(line) = string(env, one)? {
                    let mut parts = line.splitn(3, '\t');
                    if let (Some(id), Some(name), Some(lang)) =
                        (parts.next(), parts.next(), parts.next())
                    {
                        out.push((id.to_string(), name.to_string(), lang.to_string()));
                    }
                }
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn say(text: &str, voice: Option<&str>, rate: f32) -> Result<(), String> {
        let why = with_activity("speaking", |env, activity| {
            let text = env.new_string(text)?;
            let voice = env.new_string(voice.unwrap_or(""))?;
            let why = env
                .call_method(
                    activity,
                    "krateSay",
                    "(Ljava/lang/String;Ljava/lang/String;F)Ljava/lang/String;",
                    &[
                        JValue::Object(&text),
                        JValue::Object(&voice),
                        JValue::Float(rate),
                    ],
                )?
                .l()?;
            string(env, why)
        })
        .ok_or_else(|| "speaking failed inside Krate".to_string())?;
        why.map_or(Ok(()), Err)
    }

    pub fn stop_speaking() {
        let _ = with_activity("stopping speech", |env, activity| {
            env.call_method(activity, "krateStopSpeaking", "()V", &[])?
                .v()
        });
    }

    pub fn speaking() -> bool {
        with_activity("asking if speaking", |env, activity| {
            env.call_method(activity, "krateSpeaking", "()Z", &[])?.z()
        })
        .unwrap_or(false)
    }
}

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(not(target_os = "android"))]
mod stub {
    /// No phone, nothing covering the edges.
    pub fn covered_edges() -> (f32, f32, f32, f32) {
        (0.0, 0.0, 0.0, 0.0)
    }

    /// No bars off a phone.
    pub fn light_bars(_top: bool, _bottom: bool) {}

    pub fn open_tab(_url: &str, _note: &str) -> bool {
        false
    }

    pub fn tab_open() -> bool {
        false
    }

    pub fn close_tab() {}

    /// No phone, no name.
    pub fn device_name() -> String {
        "This phone".to_string()
    }

    /// No screen reader off a phone.
    pub fn screen_reader_on() -> bool {
        false
    }

    pub fn read_clipboard() -> Result<String, String> {
        Err("the clipboard is Android's only on Android".to_string())
    }

    pub fn write_clipboard(_text: &str) -> Result<(), String> {
        Err("the clipboard is Android's only on Android".to_string())
    }
}

#[cfg(not(target_os = "android"))]
pub use stub::*;
