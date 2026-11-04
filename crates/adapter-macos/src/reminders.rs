//! Reminders that arrive with the app closed, on a Mac (IC-921), the half a
//! phone already had. Each installed app is its own bundle, so macOS keeps
//! its reminders, its notification setting and its name apart from every
//! other app's; a reminder tapped opens that app. A Krate that is not an
//! installed app (`krate run`, a bundle shared by many apps) has no such
//! place, and says so.

use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool};
use objc2::{define_class, msg_send, ClassType, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};

use crate::share_photos::wait;

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

/// Every reminder's identifier starts here; the bundle already belongs to
/// one app.
const PREFIX: &str = "krate.";

/// The bundle an installed app runs as, or why reminders cannot come.
fn installed_app() -> Result<(), String> {
    // SAFETY: the main bundle's identifier; nil outside a bundle.
    let ident: Option<String> = unsafe {
        let bundle: Retained<AnyObject> = msg_send![objc2::class!(NSBundle), mainBundle];
        let ident: Option<Retained<NSString>> = msg_send![&*bundle, bundleIdentifier];
        ident.map(|i| i.to_string())
    };
    let per_app = ident.as_deref().is_some_and(|i| {
        ["tech.krate.app.", "dev.krate.app."]
            .iter()
            .any(|p| i.strip_prefix(p).is_some_and(|rest| !rest.is_empty()))
    });
    if per_app {
        Ok(())
    } else {
        Err("reminders come to installed apps: install this one with `krate install`, then open it from Applications".to_string())
    }
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements for a notification
    //   center delegate.
    // - macOS calls it on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct KrateMacNoticeDelegate;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateMacNoticeDelegate {}

    impl KrateMacNoticeDelegate {
        /// Show the reminder while the app is in front: by default macOS
        /// shows nothing for the app on screen, and it is the one that set
        /// the reminder.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &AnyObject,
            _notification: &AnyObject,
            done: &block2::DynBlock<dyn Fn(usize)>,
        ) {
            // Banner (16) | list (8) | sound (2).
            done.call((16 | 8 | 2,));
        }
    }
);

thread_local! {
    static DELEGATE: std::cell::OnceCell<Retained<KrateMacNoticeDelegate>> =
        const { std::cell::OnceCell::new() };
}

fn center() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"UNUserNotificationCenter")?;
    // SAFETY: the bundle's notification center.
    let center: Retained<AnyObject> = unsafe { msg_send![class, currentNotificationCenter] };
    if let Some(mtm) = MainThreadMarker::new() {
        DELEGATE.with(|cell| {
            let delegate = cell.get_or_init(|| {
                let this = KrateMacNoticeDelegate::alloc(mtm).set_ivars(());
                // SAFETY: NSObject's init on a fresh allocation.
                unsafe { msg_send![super(this), init] }
            });
            // SAFETY: the protocol added once for the delegate's class; the
            // delegate is held for the life of the process.
            unsafe {
                if let Some(protocol) = AnyProtocol::get(c"UNUserNotificationCenterDelegate") {
                    objc2::ffi::class_addProtocol(
                        KrateMacNoticeDelegate::class() as *const AnyClass as *mut AnyClass,
                        protocol,
                    );
                }
                let _: () = msg_send![&*center, setDelegate: &**delegate];
            }
        });
    }
    Some(center)
}

/// Schedule a reminder: identifier `krate.<id>`, delivered by macOS with the
/// app closed; `daily` repeats at that time of day. The bundle's own name
/// is the sender, so the reminder's title is the app's.
pub(crate) fn schedule(
    id: &str,
    title: &str,
    body: &str,
    after: u64,
    daily: bool,
) -> Result<(), String> {
    installed_app()?;
    let classes = (
        center(),
        AnyClass::get(c"UNMutableNotificationContent"),
        AnyClass::get(c"UNNotificationRequest"),
        AnyClass::get(c"UNTimeIntervalNotificationTrigger"),
        AnyClass::get(c"UNCalendarNotificationTrigger"),
        AnyClass::get(c"NSDate"),
        AnyClass::get(c"NSCalendar"),
    );
    let (
        Some(center),
        Some(content_class),
        Some(request_class),
        Some(interval_class),
        Some(calendar_trigger),
        Some(date_class),
        Some(calendar_class),
    ) = classes
    else {
        return Err("this Mac has no reminders".to_string());
    };
    let full = format!("{PREFIX}{id}");
    let (title, body) = (title.to_string(), body.to_string());
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    // SAFETY: authorization, then a content, a trigger (a time interval, or
    // the time of day for a daily one) and a request added to the bundle's
    // center; the completions answer on the channel.
    unsafe {
        let center_for_add = center.clone();
        let asked = block2::RcBlock::new(move |ok: Bool, _error: *mut AnyObject| {
            if !ok.as_bool() {
                let _ = tx.send(Err(
                    "notifications are off for this app: System Settings > Notifications"
                        .to_string(),
                ));
                return;
            }
            let ns = |t: &str| NSString::from_str(t);
            let content: Retained<AnyObject> = msg_send![content_class, new];
            let _: () = msg_send![&*content, setTitle: &*ns(&title)];
            let _: () = msg_send![&*content, setBody: &*ns(&body)];
            let sound: Option<Retained<AnyObject>> = AnyClass::get(c"UNNotificationSound")
                .and_then(|class| msg_send![class, defaultSound]);
            if let Some(sound) = sound {
                let _: () = msg_send![&*content, setSound: &*sound];
            }
            let trigger: Retained<AnyObject> = if daily {
                // The time of day it first comes, every day.
                let date: Retained<AnyObject> =
                    msg_send![date_class, dateWithTimeIntervalSinceNow: after as f64];
                let calendar: Retained<AnyObject> = msg_send![calendar_class, currentCalendar];
                // Hour (32) | minute (64) | second (128).
                let parts: Retained<AnyObject> =
                    msg_send![&*calendar, components: (32usize | 64 | 128), fromDate: &*date];
                msg_send![calendar_trigger, triggerWithDateMatchingComponents: &*parts, repeats: true]
            } else {
                msg_send![interval_class, triggerWithTimeInterval: after as f64, repeats: false]
            };
            let request: Retained<AnyObject> = msg_send![
                request_class,
                requestWithIdentifier: &*ns(&full),
                content: &*content,
                trigger: &*trigger
            ];
            let tx_add = tx.clone();
            let added = block2::RcBlock::new(move |error: *mut AnyObject| {
                let _ = tx_add.send(if error.is_null() {
                    Ok(())
                } else {
                    let text: Option<Retained<NSString>> = msg_send![&*error, localizedDescription];
                    Err(format!(
                        "macOS refused the reminder: {}",
                        text.map(|t| t.to_string()).unwrap_or_default()
                    ))
                });
            });
            let _: () = msg_send![&*center_for_add, addNotificationRequest: &*request, withCompletionHandler: &*added];
        });
        // Badge (1) | sound (2) | alert (4).
        let _: () = msg_send![&*center, requestAuthorizationWithOptions: 7usize, completionHandler: &*asked];
    }
    wait(&rx, Duration::from_secs(120)).unwrap_or(Err("macOS did not answer".to_string()))
}

/// Show a notification now, from an installed app (IC-271): `None` when this
/// Krate is not an installed app, so the caller uses its fallback.
///
/// The bundle is the sender, so macOS draws the app's own name and icon on
/// the notification, as for every other program -- the osascript fallback
/// can only say it came from Script Editor. And macOS answers whether this
/// app's notifications are on, so one the person switched off is reported as
/// not shown instead of as success.
pub fn notify_now(title: &str, body: &str) -> Option<Result<(), String>> {
    installed_app().ok()?;
    let (Some(center), Some(content_class), Some(request_class)) = (
        center(),
        AnyClass::get(c"UNMutableNotificationContent"),
        AnyClass::get(c"UNNotificationRequest"),
    ) else {
        return Some(Err("this Mac has no notification service".to_string()));
    };
    let (title, body) = (title.to_string(), body.to_string());
    // One identifier per notification, so two in a row are both shown.
    let id = format!(
        "{PREFIX}now.{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    // SAFETY: authorization, then a content and a request with no trigger
    // (deliver now) added to the bundle's center; the completions answer on
    // the channel.
    unsafe {
        let center_for_add = center.clone();
        let asked = block2::RcBlock::new(move |ok: Bool, _error: *mut AnyObject| {
            if !ok.as_bool() {
                let _ = tx.send(Err(
                    "notifications are off for this app: System Settings > Notifications"
                        .to_string(),
                ));
                return;
            }
            let ns = |t: &str| NSString::from_str(t);
            let content: Retained<AnyObject> = msg_send![content_class, new];
            let _: () = msg_send![&*content, setTitle: &*ns(&title)];
            let _: () = msg_send![&*content, setBody: &*ns(&body)];
            let none: *const AnyObject = std::ptr::null();
            let request: Retained<AnyObject> = msg_send![
                request_class,
                requestWithIdentifier: &*ns(&id),
                content: &*content,
                trigger: none
            ];
            let tx_add = tx.clone();
            let added = block2::RcBlock::new(move |error: *mut AnyObject| {
                let _ = tx_add.send(if error.is_null() {
                    Ok(())
                } else {
                    let text: Option<Retained<NSString>> = msg_send![&*error, localizedDescription];
                    Err(format!(
                        "macOS refused the notification: {}",
                        text.map(|t| t.to_string()).unwrap_or_default()
                    ))
                });
            });
            let _: () = msg_send![&*center_for_add, addNotificationRequest: &*request, withCompletionHandler: &*added];
        });
        // Badge (1) | sound (2) | alert (4).
        let _: () = msg_send![&*center, requestAuthorizationWithOptions: 7usize, completionHandler: &*asked];
    }
    Some(wait(&rx, Duration::from_secs(120)).unwrap_or(Err("macOS did not answer".to_string())))
}

/// Take back one of the app's reminders.
pub(crate) fn cancel(id: &str) {
    if installed_app().is_err() {
        return;
    }
    let Some(center) = center() else {
        return;
    };
    // SAFETY: remove one pending request by its identifier.
    unsafe {
        let ids = NSArray::from_retained_slice(&[NSString::from_str(&format!("{PREFIX}{id}"))]);
        let _: () = msg_send![&*center, removePendingNotificationRequestsWithIdentifiers: &*ids];
    }
}

/// The app's reminders still to come, by the app's own ids.
pub(crate) fn pending() -> Vec<String> {
    if installed_app().is_err() {
        return Vec::new();
    }
    let Some(center) = center() else {
        return Vec::new();
    };
    let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
    // SAFETY: the pending requests, read for their identifiers.
    unsafe {
        let got = block2::RcBlock::new(move |requests: *mut AnyObject| {
            let mut ids = Vec::new();
            if !requests.is_null() {
                let count: usize = msg_send![&*requests, count];
                for i in 0..count {
                    let request: Retained<AnyObject> = msg_send![&*requests, objectAtIndex: i];
                    let id: Option<Retained<NSString>> = msg_send![&*request, identifier];
                    if let Some(rest) = id
                        .map(|s| s.to_string())
                        .and_then(|s| s.strip_prefix(PREFIX).map(str::to_string))
                    {
                        ids.push(rest);
                    }
                }
            }
            let _ = tx.send(ids);
        });
        let _: () = msg_send![&*center, getPendingNotificationRequestsWithCompletionHandler: &*got];
    }
    wait(&rx, Duration::from_secs(5)).unwrap_or_default()
}
