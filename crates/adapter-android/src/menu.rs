//! Krate's menu over a running app (IC-934), drawn by the player activity's
//! Menu.java. Nothing sits over an app: a half-second hold on the strip
//! under the clock (its top inset plus [`MENU_BAND`] points Krate keeps apps
//! out of), or the phone's back gesture, opens it. A short tap there still
//! reaches the app.

use std::sync::atomic::{AtomicBool, Ordering};

/// The band under the status bar Krate keeps apps out of, for the hold.
pub const MENU_BAND: f32 = 20.0;

/// How long a finger stays on the strip before the menu opens.
pub const HOLD_MS: u128 = 500;

/// Whether an app (not Krate's own sheet) is running: the menu, the band
/// and back-opens-the-menu apply only then.
static CHROME: AtomicBool = AtomicBool::new(false);

pub fn chrome_on() -> bool {
    CHROME.load(Ordering::Relaxed)
}

/// Whether a touch at `y` points down the window starts in the strip whose
/// covered top is `top` points.
pub fn in_strip(y: f32, top: f32) -> bool {
    y >= 0.0 && y < top.max(20.0) + MENU_BAND
}

#[cfg(target_os = "android")]
mod real {
    use super::CHROME;
    use crate::java::with_activity;
    use jni::objects::{JObject, JValue};
    use std::sync::atomic::Ordering;

    /// Say which app runs (its name, id, what it may use in words, and what
    /// opens it again), or that none does (None).
    pub fn set_app(app: Option<(&str, &str, &[String], &str)>) {
        CHROME.store(app.is_some(), Ordering::Relaxed);
        let _ = with_activity("telling the menu the app", |env, activity| {
            let (name, id, uses, opens) = app.unwrap_or(("", "", &[], ""));
            let name = if app.is_some() {
                JObject::from(env.new_string(name)?)
            } else {
                JObject::null()
            };
            let id = env.new_string(id)?;
            let opens = env.new_string(opens)?;
            let list =
                env.new_object_array(uses.len() as i32, "java/lang/String", JObject::null())?;
            for (i, use_) in uses.iter().enumerate() {
                let one = env.new_string(use_)?;
                env.set_object_array_element(&list, i as i32, one)?;
            }
            env.call_method(
                activity,
                "krateSetApp",
                "(Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)V",
                &[
                    JValue::Object(&name),
                    JValue::Object(&id),
                    JValue::Object(&list),
                    JValue::Object(&opens),
                ],
            )?
            .v()
        });
    }

    /// Open the menu (it is drawn on Android's UI thread).
    pub fn show() {
        let _ = with_activity("opening Krate's menu", |env, activity| {
            env.call_method(activity, "krateShowMenu", "()V", &[])?.v()
        });
    }

    /// Whether the menu asked the app to close since last asked.
    pub fn take_close() -> bool {
        with_activity("reading Krate's menu", |env, activity| {
            env.call_method(activity, "krateMenuAction", "()I", &[])?
                .i()
        }) == Some(1)
    }

    /// Whether a link or file was tapped since last asked while Krate's own
    /// screen (its home, the permission sheet) shows: that screen closes so
    /// the player can open what was tapped.
    pub fn take_opened_meanwhile() -> bool {
        with_activity("reading what was opened", |env, activity| {
            env.call_method(activity, "krateOpenedMeanwhile", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    /// Whether the menu asked for the app to open again once it has closed.
    pub fn restart_asked() -> bool {
        with_activity("reading Krate's menu", |env, activity| {
            env.call_method(activity, "krateRestartAsked", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    /// Open Krate again on the same app, in a fresh process; does not return.
    pub fn relaunch() {
        let _ = with_activity("restarting the app", |env, activity| {
            env.call_method(activity, "krateRelaunch", "()V", &[])?.v()
        });
    }
}

#[cfg(target_os = "android")]
pub use real::*;

#[cfg(test)]
mod tests {
    use super::in_strip;

    #[test]
    fn the_strip_is_the_status_bar_and_the_band_under_it() {
        // A Pixel 7's status bar is about 45 points: the strip runs to 65.
        assert!(in_strip(10.0, 45.0));
        assert!(in_strip(64.0, 45.0));
        assert!(!in_strip(66.0, 45.0));
        // No inset reported yet: still a usable strip.
        assert!(in_strip(30.0, 0.0));
        assert!(!in_strip(-1.0, 45.0));
    }
}
