//! Reminders that arrive with the app closed (krate:ui/reminders, IC-921):
//! AlarmManager and a "Reminders" notification channel, through the player
//! activity's Reminder.java. The runtime has already checked `ui.notify`,
//! the id, the text and the delay.
//!
//! Each app sees only its own: the system's id is `krate.<app id>.<id>`. The
//! player says which app is running and how to open it again, so a reminder
//! tapped later opens the app that set it.

use std::sync::Mutex;

/// The running app's id and what opens it again ("" for the app the player
/// carries; otherwise a `file://` URI to the copy Krate kept).
static CURRENT: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Say which app is running (Some) or that none is (None).
pub fn set_current_app(app: Option<(String, String)>) {
    *CURRENT.lock().expect("current app lock") = app;
}

/// The prefix of the running app's reminders' system ids.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn prefix_for(app_id: &str) -> String {
    format!("krate.{app_id}.")
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn current() -> Option<(String, String)> {
    CURRENT.lock().expect("current app lock").clone()
}

#[cfg(target_os = "android")]
mod real {
    use super::{current, prefix_for};
    use crate::java::{string, wait_for, with_activity};
    use crate::phone::{ask, Asked};
    use jni::objects::{JObjectArray, JValue};
    use std::time::Duration;

    pub fn schedule(
        id: &str,
        sender: &str,
        title: &str,
        body: &str,
        after_seconds: u64,
        daily: bool,
    ) -> Result<(), String> {
        let (app, opens) = current().ok_or_else(|| "no app is running".to_string())?;
        if ask("krateNotifyAsk") == Asked::Refused {
            return Err("notifications were not allowed for Krate".to_string());
        }
        // On time needs "Alarms & reminders" on Android 12 and up: asked
        // once per app, on its first reminder, by Krate's own sheet. A no
        // still sets the reminder, near the time.
        let app_for_ask = app.clone();
        let on_time = wait_for(Duration::from_secs(10 * 60), || {
            match with_activity("asking about on-time reminders", |env, activity| {
                let id = env.new_string(&app_for_ask)?;
                let name = env.new_string(sender)?;
                env.call_method(
                    activity,
                    "krateExactAsk",
                    "(Ljava/lang/String;Ljava/lang/String;)I",
                    &[JValue::Object(&id), JValue::Object(&name)],
                )?
                .i()
            }) {
                Some(2) => None,
                Some(answer) => Some(answer == 0),
                None => Some(false),
            }
        })
        .unwrap_or(false);
        log::info!(
            "reminder {id}: {}",
            if on_time { "on time" } else { "may be late" }
        );
        let system_id = format!("{}{id}", prefix_for(&app));
        let why = with_activity("setting a reminder", |env, activity| {
            let strings = [
                env.new_string(&system_id)?,
                env.new_string(sender)?,
                env.new_string(title)?,
                env.new_string(body)?,
                env.new_string(&opens)?,
            ];
            let why = env
                .call_method(
                    activity,
                    "krateRemind",
                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;JZLjava/lang/String;)Ljava/lang/String;",
                    &[
                        JValue::Object(&strings[0]),
                        JValue::Object(&strings[1]),
                        JValue::Object(&strings[2]),
                        JValue::Object(&strings[3]),
                        JValue::Long(after_seconds as i64),
                        JValue::Bool(u8::from(daily)),
                        JValue::Object(&strings[4]),
                    ],
                )?
                .l()?;
            string(env, why)
        })
        .ok_or_else(|| "the reminder could not be set".to_string())?;
        why.map_or(Ok(()), Err)
    }

    pub fn cancel(id: &str) {
        let Some((app, _)) = current() else {
            return;
        };
        let system_id = format!("{}{id}", prefix_for(&app));
        let _ = with_activity("cancelling a reminder", |env, activity| {
            let id = env.new_string(&system_id)?;
            env.call_method(
                activity,
                "krateRemindCancel",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&id)],
            )?
            .v()
        });
    }

    pub fn pending() -> Vec<String> {
        let Some((app, _)) = current() else {
            return Vec::new();
        };
        let prefix = prefix_for(&app);
        with_activity("listing reminders", |env, activity| {
            let p = env.new_string(&prefix)?;
            let all = env
                .call_method(
                    activity,
                    "krateRemindPending",
                    "(Ljava/lang/String;)[Ljava/lang/String;",
                    &[JValue::Object(&p)],
                )?
                .l()?;
            let all = JObjectArray::from(all);
            let mut out = Vec::new();
            for i in 0..env.get_array_length(&all)? {
                let one = env.get_object_array_element(&all, i)?;
                if let Some(id) = string(env, one)? {
                    out.extend(id.strip_prefix(&prefix).map(str::to_string));
                }
            }
            out.sort();
            Ok(out)
        })
        .unwrap_or_default()
    }
}

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(test)]
mod tests {
    use super::prefix_for;

    #[test]
    fn an_apps_reminders_are_named_under_its_own_id() {
        assert_eq!(
            prefix_for("dev.krate.phone-check"),
            "krate.dev.krate.phone-check."
        );
        // One app's prefix is never another's: the dot closes it.
        assert!(!format!("{}x", prefix_for("dev.a.bc")).starts_with(&prefix_for("dev.a.b")));
    }
}
