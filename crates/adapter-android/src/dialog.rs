//! A message and a yes-or-no question (krate:ui/dialog message and confirm)
//! as the phone's own dialog, through the player activity's Dialog.java.
//!
//! The runtime answers both without asking on a phone today: a message is
//! "shown and closed" and every question is "no" (phase3_gui_host.rs,
//! `message` and `confirm`). So "Delete this note?" could never be answered
//! yes on Android and a notes app could not delete. These are the calls for
//! the runtime to make there instead, the way it already calls
//! `phone::notify` for a notification.

/// What the activity's krateDialogAnswer says: still up, or how it closed.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn answer(code: i32) -> Option<bool> {
    match code {
        -1 => None,
        0 => Some(true),
        // No, Back, a tap outside, or a dialog that could not show: the
        // question's own rule is that anything but yes is no.
        _ => Some(false),
    }
}

#[cfg(target_os = "android")]
mod real {
    use crate::java::{wait_for, with_activity};
    use jni::objects::JValue;
    use std::time::Duration;

    fn ask(title: &str, body: &str, question: bool) -> bool {
        let shown = with_activity("showing a dialog", |env, activity| {
            let title = env.new_string(title)?;
            let body = env.new_string(body)?;
            env.call_method(
                activity,
                "krateDialog",
                "(Ljava/lang/String;Ljava/lang/String;Z)V",
                &[
                    JValue::Object(&title),
                    JValue::Object(&body),
                    JValue::Bool(question.into()),
                ],
            )?
            .v()
        });
        if shown.is_none() {
            return false;
        }
        // The person may take their time; the app waits, as it does for a
        // desktop's dialog, while Krate keeps the phone's events answered.
        wait_for(Duration::from_secs(24 * 60 * 60), || {
            with_activity("reading the dialog", |env, activity| {
                env.call_method(activity, "krateDialogAnswer", "()I", &[])?
                    .i()
            })
            .map_or(Some(false), super::answer)
        })
        .unwrap_or(false)
    }

    /// Show a message with one button and wait until the person closes it.
    pub fn message(title: &str, body: &str) {
        let _ = ask(title, body, false);
    }

    /// Ask a yes-or-no question and wait for the answer; anything but Yes
    /// is no.
    pub fn confirm(title: &str, body: &str) -> bool {
        ask(title, body, true)
    }
}

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(test)]
mod tests {
    use super::answer;

    #[test]
    fn only_yes_is_yes_and_an_open_dialog_is_no_answer_yet() {
        assert_eq!(answer(-1), None);
        assert_eq!(answer(0), Some(true));
        assert_eq!(answer(1), Some(false));
        assert_eq!(answer(2), Some(false));
    }
}
