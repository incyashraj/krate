//! Where the Mac is, for `krate:ui/location` (IC-917, IC-982): CoreLocation,
//! the framework the iPhone path already drives, read the same way -- no
//! delegate, the manager's latest fix asked for each time.
//!
//! The first ask makes the manager on the main thread, asks to use location
//! while the app is open (macOS shows its own prompt, naming the app's
//! bundle), starts it, and answers `finding`; a moment later the fix.
//!
//! macOS shows that prompt only for an app bundle that says why it wants
//! location in its Info.plist. A bare `krate run` from a terminal has no such
//! bundle, and the request would be dropped without a word, leaving the app
//! on `finding` for ever. So an undeclared bundle answers `unavailable` at
//! once and says why on stderr.

use krate_adapter_common::ui::{LocationAnswer, Position};

/// The keys macOS reads for the location prompt's sentence.
pub(crate) const USAGE_KEYS: [&str; 2] = [
    "NSLocationWhenInUseUsageDescription",
    "NSLocationUsageDescription",
];

/// One fix as CoreLocation reports it, before it becomes a `Position`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    pub horizontal_accuracy: f64,
    pub altitude: f64,
    pub vertical_accuracy: f64,
    pub speed: f64,
    pub course: f64,
    pub age_seconds: f64,
}

/// What the app hears, from what CoreLocation says: `status` is
/// CLAuthorizationStatus (0 not asked, 1 restricted, 2 denied, 3 always, 4
/// while in use). Kept apart from the Objective-C calls so it is tested on
/// every machine.
pub(crate) fn answer(status: i32, fix: Option<Fix>) -> LocationAnswer {
    match status {
        1 | 2 => return LocationAnswer::NotAllowed,
        // Not asked yet: the prompt is up.
        0 => return LocationAnswer::Finding,
        _ => {}
    }
    let Some(fix) = fix else {
        return LocationAnswer::Finding;
    };
    // A negative accuracy is CoreLocation's "this fix is not valid".
    if fix.horizontal_accuracy < 0.0 {
        return LocationAnswer::Finding;
    }
    LocationAnswer::Here(Position {
        latitude: fix.latitude,
        longitude: fix.longitude,
        accuracy: fix.horizontal_accuracy as f32,
        altitude: (fix.vertical_accuracy > 0.0).then_some(fix.altitude as f32),
        speed: (fix.speed >= 0.0).then_some(fix.speed as f32),
        course: (fix.course >= 0.0).then_some(fix.course as f32),
        age_ms: (fix.age_seconds.max(0.0) * 1000.0) as u64,
    })
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{answer, Fix, USAGE_KEYS};
    use krate_adapter_common::ui::LocationAnswer;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::MainThreadMarker;
    use objc2_foundation::NSString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[link(name = "CoreLocation", kind = "framework")]
    unsafe extern "C" {}

    /// CLLocationCoordinate2D.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CoordinateC {
        latitude: f64,
        longitude: f64,
    }

    // SAFETY: the C layout of CLLocationCoordinate2D, two doubles.
    unsafe impl objc2::Encode for CoordinateC {
        const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
            "CLLocationCoordinate2D",
            &[objc2::Encoding::Double, objc2::Encoding::Double],
        );
    }

    struct Manager(Retained<AnyObject>);
    // SAFETY: made on the main thread; its properties are read from
    // whichever thread the app runs on, under the lock, which
    // CLLocationManager allows.
    unsafe impl Send for Manager {}

    static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);
    static ASKED: AtomicBool = AtomicBool::new(false);
    static SAID_WHY: AtomicBool = AtomicBool::new(false);

    fn class() -> Option<&'static AnyClass> {
        AnyClass::get(c"CLLocationManager")
    }

    /// Whether this process's bundle says why it wants location.
    fn declared() -> bool {
        // SAFETY: the main bundle's info dictionary lookup; nil when absent.
        unsafe {
            let bundle: Retained<AnyObject> = msg_send![objc2::class!(NSBundle), mainBundle];
            USAGE_KEYS.iter().any(|key| {
                let value: Option<Retained<AnyObject>> =
                    msg_send![&*bundle, objectForInfoDictionaryKey: &*NSString::from_str(key)];
                value.is_some()
            })
        }
    }

    pub(crate) fn available() -> bool {
        if !declared() {
            if !SAID_WHY.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "krate: location is unavailable here: this process is not an app bundle \
                     that says why it wants location (open the app from Krate, not a bare \
                     `krate run`)"
                );
            }
            return false;
        }
        // SAFETY: a class method answering a BOOL: location services on.
        class().is_some_and(|class| unsafe { msg_send![class, locationServicesEnabled] })
    }

    fn start(class: &'static AnyClass) {
        // SAFETY: CLLocationManager's documented setup on the main thread:
        // make it, ask for "while using", start.
        unsafe {
            let manager: Option<Retained<AnyObject>> = msg_send![class, new];
            let Some(manager) = manager else {
                return;
            };
            let best: f64 = -1.0; // kCLLocationAccuracyBest
            let _: () = msg_send![&*manager, setDesiredAccuracy: best];
            let _: () = msg_send![&*manager, requestWhenInUseAuthorization];
            let _: () = msg_send![&*manager, startUpdatingLocation];
            *MANAGER.lock().expect("location lock") = Some(Manager(manager));
        }
    }

    #[cfg(test)]
    pub(crate) fn asked() -> bool {
        ASKED.load(Ordering::Relaxed) || MANAGER.lock().expect("location lock").is_some()
    }

    pub(crate) fn read() -> LocationAnswer {
        let Some(class) = class() else {
            return LocationAnswer::Unavailable;
        };
        if !available() {
            return LocationAnswer::Unavailable;
        }
        if !ASKED.swap(true, Ordering::Relaxed) {
            if MainThreadMarker::new().is_some() {
                start(class);
            } else {
                dispatch2::DispatchQueue::main().exec_async(move || start(class));
            }
            return LocationAnswer::Finding;
        }
        let slot = MANAGER.lock().expect("location lock");
        let Some(manager) = slot.as_ref() else {
            return LocationAnswer::Finding;
        };
        // SAFETY: properties of a live manager and of the CLLocation it
        // holds; nil until the first fix.
        unsafe {
            let status: i32 = msg_send![&*manager.0, authorizationStatus];
            let fix: Option<Retained<AnyObject>> = msg_send![&*manager.0, location];
            let fix = fix.map(|fix| {
                let at: CoordinateC = msg_send![&*fix, coordinate];
                let stamp: Option<Retained<AnyObject>> = msg_send![&*fix, timestamp];
                Fix {
                    latitude: at.latitude,
                    longitude: at.longitude,
                    horizontal_accuracy: msg_send![&*fix, horizontalAccuracy],
                    altitude: msg_send![&*fix, altitude],
                    vertical_accuracy: msg_send![&*fix, verticalAccuracy],
                    speed: msg_send![&*fix, speed],
                    course: msg_send![&*fix, course],
                    age_seconds: stamp.map_or(0.0, |date| {
                        let since: f64 = msg_send![&*date, timeIntervalSinceNow];
                        -since
                    }),
                }
            });
            answer(status, fix)
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use mac::{available, read};

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn asked() -> bool {
    mac::asked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Fix {
        Fix {
            latitude: 51.5007,
            longitude: -0.1246,
            horizontal_accuracy: 12.0,
            altitude: 20.0,
            vertical_accuracy: 3.0,
            speed: -1.0,
            course: -1.0,
            age_seconds: 0.25,
        }
    }

    /// The test binary is a bare process with no Info.plist, as `krate run`
    /// from a terminal is: it must answer unavailable at once and never make
    /// a manager, which is what would otherwise be dropped silently.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_that_does_not_say_why_answers_unavailable_and_never_asks() {
        assert!(!available());
        assert_eq!(read(), LocationAnswer::Unavailable);
        assert_eq!(read(), LocationAnswer::Unavailable);
        assert!(!asked(), "CoreLocation was reached without a usage string");
    }

    #[test]
    fn a_person_who_said_no_is_never_told_finding() {
        for refused in [1, 2] {
            assert_eq!(answer(refused, Some(fix())), LocationAnswer::NotAllowed);
            assert_eq!(answer(refused, None), LocationAnswer::NotAllowed);
        }
    }

    #[test]
    fn while_the_prompt_is_up_or_no_fix_yet_it_is_finding() {
        assert_eq!(answer(0, Some(fix())), LocationAnswer::Finding);
        assert_eq!(answer(4, None), LocationAnswer::Finding);
        let invalid = Fix {
            horizontal_accuracy: -1.0,
            ..fix()
        };
        assert_eq!(answer(3, Some(invalid)), LocationAnswer::Finding);
    }

    #[test]
    fn a_fix_says_only_what_the_mac_knows() {
        let LocationAnswer::Here(p) = answer(3, Some(fix())) else {
            panic!("an allowed fix is a position");
        };
        assert_eq!(
            (p.latitude, p.longitude, p.accuracy),
            (51.5007, -0.1246, 12.0)
        );
        assert_eq!(p.altitude, Some(20.0));
        // A Mac on a desk does not know its speed or course: none, not 0.
        assert_eq!((p.speed, p.course), (None, None));
        assert_eq!(p.age_ms, 250);
        let no_altitude = Fix {
            vertical_accuracy: -1.0,
            ..fix()
        };
        let LocationAnswer::Here(p) = answer(4, Some(no_altitude)) else {
            panic!("while-in-use is allowed too");
        };
        assert_eq!(p.altitude, None);
    }
}
