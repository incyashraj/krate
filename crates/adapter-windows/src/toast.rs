//! Desktop notifications on Windows (IC-271): a toast whose sender line is
//! the app's own name.
//!
//! An unpackaged program may post toasts under an AppUserModelID it registers
//! at `HKCU\Software\Classes\AppUserModelId\<id>`; Windows shows that key's
//! `DisplayName` as the sender, the line every other notification uses for
//! the program it came from. The id and the name both come from the name the
//! host holds for the app, never from the guest's title or body, so an app
//! cannot post as somebody else.
//!
//! Before showing, the notifier is asked whether this app's notifications are
//! switched on. Windows answers that exactly, so a toast the person has turned
//! off is reported as not shown instead of as success -- the acknowledgement
//! macOS cannot give.

/// The AppUserModelID an app's toasts are posted under.
///
/// Stable for a name, so the person's per-app setting in Windows (on, off,
/// banners or not) survives across runs. A hash rather than the name itself:
/// an id may not hold spaces, and any name is a valid app name.
pub fn app_user_model_id(app_name: &str) -> String {
    // FNV-1a, 64-bit: no dependency, and the id only has to be stable, not
    // secret.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in app_name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("Krate.App.{hash:016x}")
}

/// The toast's XML: the guest's title, then its body, as plain text lines.
pub fn toast_xml(title: &str, body: &str) -> String {
    let mut xml = String::from("<toast><visual><binding template=\"ToastGeneric\">");
    for line in [title, body] {
        if line.is_empty() {
            continue;
        }
        xml.push_str("<text>");
        xml.push_str(&escape_xml(line));
        xml.push_str("</text>");
    }
    xml.push_str("</binding></visual></toast>");
    xml
}

/// Why Windows will not show this app's toasts, from the notifier's
/// `NotificationSetting`, or `None` when they are switched on.
pub fn setting_refusal(setting: i32) -> Option<&'static str> {
    match setting {
        0 => None,
        1 => Some("notifications for this app are turned off in Windows settings"),
        2 => Some("notifications are turned off on this computer"),
        3 => Some("notifications are turned off by this computer's administrator"),
        _ => Some("Windows will not show notifications for this app"),
    }
}

fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Show one toast attributed to `app_name`.
///
/// Ok means Windows accepted it with this app's notifications switched on.
/// The person may still have Focus Assist on; Windows then files the toast in
/// the notification centre instead of showing a banner, which is the
/// platform's own promise and not one Krate can make stronger.
#[cfg(windows)]
pub fn show(app_name: &str, title: &str, body: &str) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let id = app_user_model_id(app_name);
    register_sender(&id, app_name)?;

    fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> String {
        move |err| format!("Windows could not show it ({step}: {err})")
    }
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&id))
        .map_err(failed("notifier"))?;
    // A definite "off" is reported as not shown. No answer is not "off": a
    // toast on CI's windows-2022 runner failed with Element not found
    // (0x80070490), the answer Windows is known to give from Setting for a
    // program with no Start menu entry, so a missing answer goes on to Show,
    // whose own error is the one that counts.
    if let Ok(setting) = notifier.Setting() {
        if let Some(why) = setting_refusal(setting.0) {
            return Err(why.to_string());
        }
    }
    let doc = XmlDocument::new().map_err(failed("document"))?;
    doc.LoadXml(&HSTRING::from(toast_xml(title, body)))
        .map_err(failed("content"))?;
    let toast = ToastNotification::CreateToastNotification(&doc).map_err(failed("toast"))?;
    notifier.Show(&toast).map_err(failed("show"))
}

/// Days a daily reminder is set ahead on Windows, which has no repeating
/// toast for a desktop program: a year, the longest any reminder may wait,
/// renewed each time the app sets it again.
pub const DAILY_DAYS: u64 = 365;

/// When each toast of a reminder comes, in seconds from now: once, or once
/// a day from the first for [`DAILY_DAYS`].
pub fn deliveries(after_seconds: u64, daily: bool) -> Vec<u64> {
    if !daily {
        return vec![after_seconds];
    }
    (0..DAILY_DAYS)
        .map(|day| after_seconds + day * 24 * 3600)
        .collect()
}

/// A WinRT DateTime (100 ns since 1601) `after_seconds` past `now_unix_ms`.
pub fn ticks_after(now_unix_ms: u64, after_seconds: u64) -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    UNIX_EPOCH_TICKS + (now_unix_ms as i64 + after_seconds as i64 * 1000) * 10_000
}

#[cfg(windows)]
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The group every Krate reminder is filed under, beside its id as tag.
#[cfg(windows)]
const REMINDER_GROUP: &str = "krate-reminders";

/// Set reminder `id` for `app_name`, delivered by Windows with the app
/// closed (IC-921, IC-982): one scheduled toast, or one a day for a year,
/// replacing any it had under that id.
#[cfg(windows)]
pub fn schedule(
    app_name: &str,
    id: &str,
    title: &str,
    body: &str,
    after_seconds: u64,
    daily: bool,
) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::DateTime;
    use windows::UI::Notifications::{ScheduledToastNotification, ToastNotificationManager};

    let aumid = app_user_model_id(app_name);
    register_sender(&aumid, app_name)?;
    fn failed(step: &'static str) -> impl Fn(windows::core::Error) -> String {
        move |err| format!("Windows could not set the reminder ({step}: {err})")
    }
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&aumid))
        .map_err(failed("notifier"))?;
    if let Ok(setting) = notifier.Setting() {
        if let Some(why) = setting_refusal(setting.0) {
            return Err(why.to_string());
        }
    }
    cancel(app_name, id);
    let now = now_unix_ms();
    for after in deliveries(after_seconds, daily) {
        let doc = XmlDocument::new().map_err(failed("document"))?;
        doc.LoadXml(&HSTRING::from(toast_xml(title, body)))
            .map_err(failed("content"))?;
        let toast = ScheduledToastNotification::CreateScheduledToastNotification(
            &doc,
            DateTime {
                UniversalTime: ticks_after(now, after),
            },
        )
        .map_err(failed("toast"))?;
        toast.SetTag(&HSTRING::from(id)).map_err(failed("id"))?;
        toast
            .SetGroup(&HSTRING::from(REMINDER_GROUP))
            .map_err(failed("group"))?;
        notifier.AddToSchedule(&toast).map_err(failed("schedule"))?;
    }
    Ok(())
}

/// Every scheduled toast of `app_name`'s reminders, with its tag.
#[cfg(windows)]
fn scheduled(
    app_name: &str,
) -> Vec<(
    windows::UI::Notifications::ToastNotifier,
    windows::UI::Notifications::ScheduledToastNotification,
    String,
)> {
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;
    let Ok(notifier) = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
        app_user_model_id(app_name),
    )) else {
        return Vec::new();
    };
    let Ok(list) = notifier.GetScheduledToastNotifications() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for i in 0..list.Size().unwrap_or(0) {
        let Ok(toast) = list.GetAt(i) else {
            continue;
        };
        let group = toast.Group().map(|g| g.to_string()).unwrap_or_default();
        if group != REMINDER_GROUP {
            continue;
        }
        let tag = toast.Tag().map(|t| t.to_string()).unwrap_or_default();
        found.push((notifier.clone(), toast, tag));
    }
    found
}

/// Take back every toast still to come of reminder `id`.
#[cfg(windows)]
pub fn cancel(app_name: &str, id: &str) {
    for (notifier, toast, tag) in scheduled(app_name) {
        if tag == id {
            let _ = notifier.RemoveFromSchedule(&toast);
        }
    }
}

/// The ids of `app_name`'s reminders still to come, each once.
#[cfg(windows)]
pub fn pending(app_name: &str) -> Vec<String> {
    let mut ids: Vec<String> = scheduled(app_name)
        .into_iter()
        .map(|(_, _, tag)| tag)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Register the sender Windows shows for `id`: the app's name.
///
/// Per user, so no administrator is needed, and rewritten on every toast so
/// an app that is renamed is shown under its new name.
#[cfg(windows)]
fn register_sender(id: &str, app_name: &str) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let path = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{id}"));
    let mut key = HKEY::default();
    // SAFETY: `path` outlives the call and `key` is a valid out pointer.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &path,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if created.is_err() {
        return Err(format!(
            "Windows would not record who the notification is from ({created:?})"
        ));
    }
    // REG_SZ wants UTF-16 with its terminating NUL, as bytes.
    let name: Vec<u8> = app_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: `key` was opened above with KEY_SET_VALUE and is closed below.
    let set = unsafe {
        RegSetValueExW(
            key,
            &HSTRING::from("DisplayName"),
            None,
            REG_SZ,
            Some(&name),
        )
    };
    // SAFETY: `key` is open and closed exactly once.
    let _ = unsafe { RegCloseKey(key) };
    if set.is_err() {
        return Err(format!(
            "Windows would not record who the notification is from ({set:?})"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_keeps_one_sender_id_and_two_apps_do_not_share_one() {
        let notes = app_user_model_id("Notes");
        assert_eq!(notes, app_user_model_id("Notes"), "stable across runs");
        assert_ne!(notes, app_user_model_id("Notes 2"));
        // An AppUserModelID may not hold spaces and is at most 128 characters.
        let long = app_user_model_id(&"A very long app name ".repeat(40));
        for id in [&notes, &long] {
            assert!(id.starts_with("Krate.App."), "{id}");
            assert!(!id.contains(' ') && id.len() <= 128, "{id}");
        }
    }

    #[test]
    fn the_guest_cannot_write_markup_into_its_toast() {
        // A title that tries to close the text element and add its own
        // binding, image or action stays text.
        let xml = toast_xml(
            "Done</text><image src=\"file:///C:/x.png\"/><text>",
            "a & b <action/> 'q' \"x\"",
        );
        assert_eq!(
            xml,
            "<toast><visual><binding template=\"ToastGeneric\">\
             <text>Done&lt;/text&gt;&lt;image src=&quot;file:///C:/x.png&quot;/&gt;&lt;text&gt;</text>\
             <text>a &amp; b &lt;action/&gt; &apos;q&apos; &quot;x&quot;</text>\
             </binding></visual></toast>"
        );
        assert_eq!(xml.matches("<text>").count(), 2);
        assert!(!xml.contains("<image") && !xml.contains("<action"));
    }

    #[test]
    fn an_empty_body_is_one_line_not_an_empty_one() {
        assert_eq!(
            toast_xml("Timer", ""),
            "<toast><visual><binding template=\"ToastGeneric\"><text>Timer</text></binding></visual></toast>"
        );
    }

    /// On a real Windows desktop (CI's windows-2022 runner has one): the
    /// toast is accepted, or refused only because notifications are off --
    /// never a WinRT error, which is what a malformed toast or a wrong call
    /// would give -- and the sender Windows shows is the app's name.
    #[cfg(windows)]
    #[test]
    fn windows_takes_the_toast_and_names_the_app_as_its_sender() {
        use windows::core::HSTRING;
        use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

        let app = "Krate toast test";
        match show(app, "Build finished", "3 tests passed & 0 failed <ok>") {
            Ok(()) => {}
            Err(why) => assert!(
                why.contains("turned off") || why.contains("will not"),
                "Windows refused the toast itself: {why}"
            ),
        }

        let path = HSTRING::from(format!(
            "Software\\Classes\\AppUserModelId\\{}",
            app_user_model_id(app)
        ));
        let mut buf = [0u16; 128];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: `buf` and `len` describe one writable buffer.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &path,
                &HSTRING::from("DisplayName"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        assert!(read.is_ok(), "the sender was not registered: {read:?}");
        let chars = (len as usize / 2).saturating_sub(1);
        assert_eq!(String::from_utf16_lossy(&buf[..chars]), app);
    }

    #[test]
    fn a_reminder_comes_once_or_every_day_for_a_year() {
        assert_eq!(deliveries(240, false), [240]);
        let daily = deliveries(60, true);
        assert_eq!(daily.len() as u64, DAILY_DAYS);
        assert_eq!(daily[0], 60);
        assert_eq!(daily[1] - daily[0], 24 * 3600);
        // The last is within the year a reminder may wait.
        assert!(*daily.last().unwrap() <= 366 * 24 * 3600);
    }

    #[test]
    fn a_reminder_s_time_is_counted_from_1601_as_windows_counts() {
        // 1970-01-01 plus four minutes.
        assert_eq!(
            ticks_after(0, 240),
            116_444_736_000_000_000 + 240 * 10_000_000
        );
        assert_eq!(
            ticks_after(1_500, 0) - ticks_after(0, 0),
            1_500 * 10_000,
            "milliseconds are 10,000 ticks"
        );
    }

    #[test]
    fn a_toast_the_person_switched_off_is_not_reported_as_shown() {
        assert_eq!(setting_refusal(0), None);
        for setting in [1, 2, 3, 4, 99] {
            let why = setting_refusal(setting).expect("refused");
            assert!(
                why.contains("turned off") || why.contains("will not"),
                "{why}"
            );
        }
        assert!(setting_refusal(1).unwrap().contains("this app"));
    }
}
