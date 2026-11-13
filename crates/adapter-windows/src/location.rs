//! Where the computer is on Windows, for `krate:ui/location` (IC-917,
//! IC-982): `Windows.Devices.Geolocation.Geolocator`, with the answers a
//! phone and a Mac give.
//!
//! The first ask starts a reader on its own thread and answers `finding`.
//! The reader asks Windows for access -- the system's own prompt where
//! Windows shows one, or the person's "let desktop apps access your
//! location" setting -- then reads a fix every two seconds into the latest
//! answer, which each ask returns at once. It stops when the app has not
//! asked for half a minute, and the next ask starts it again.

use krate_adapter_common::ui::{LocationAnswer, Position};

/// The latest thing the reader learned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Latest {
    /// Asked, nothing yet.
    Finding,
    /// The person said no, here or in Settings.
    Refused,
    /// Location is switched off, or this computer has no way to tell.
    Off,
    Here(Position),
}

/// What the app hears.
pub fn answer_of(latest: Latest) -> LocationAnswer {
    match latest {
        Latest::Finding => LocationAnswer::Finding,
        Latest::Refused => LocationAnswer::NotAllowed,
        Latest::Off => LocationAnswer::Unavailable,
        Latest::Here(at) => LocationAnswer::Here(at),
    }
}

/// Whether a `PositionStatus` is a computer that can tell where it is: not
/// Disabled (3, switched off) and not NotAvailable (5, nothing to tell
/// with).
pub fn status_can_tell(status: i32) -> bool {
    !matches!(status, 3 | 5)
}

/// How old a fix stamped `ticks` (a WinRT DateTime: 100 ns since 1601) is
/// at `now_unix_ms`, in milliseconds; never below zero.
pub fn age_ms(ticks: i64, now_unix_ms: u64) -> u64 {
    // 1601-01-01 to 1970-01-01, in 100 ns ticks.
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let stamp_ms = (ticks - UNIX_EPOCH_TICKS) / 10_000;
    now_unix_ms.saturating_sub(stamp_ms.max(0) as u64)
}

/// A fix as Windows gives it; `None` for what it does not know.
#[allow(clippy::too_many_arguments)]
pub fn position(
    latitude: f64,
    longitude: f64,
    accuracy: f64,
    altitude: Option<f64>,
    speed: Option<f64>,
    heading: Option<f64>,
    age_ms: u64,
) -> Position {
    let known = |v: Option<f64>| v.filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as f32);
    Position {
        latitude,
        longitude,
        accuracy: accuracy.max(0.0) as f32,
        altitude: altitude.filter(|v| v.is_finite()).map(|v| v as f32),
        speed: known(speed),
        course: known(heading),
        age_ms,
    }
}

#[cfg(windows)]
mod geolocator {
    use super::{age_ms, position, status_can_tell, Latest};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;
    use windows::Devices::Geolocation::{
        GeolocationAccessStatus, Geolocator, Geoposition, PositionAccuracy,
    };

    /// Stop reading after this long without an ask.
    const IDLE_MS: u64 = 30_000;

    static LATEST: Mutex<Option<Latest>> = Mutex::new(None);
    static LAST_ASK_MS: AtomicU64 = AtomicU64::new(0);

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn set(latest: Latest) {
        *LATEST.lock().expect("location lock") = Some(latest);
    }

    fn fix_of(at: &Geoposition) -> windows::core::Result<Latest> {
        let coordinate = at.Coordinate()?;
        let point = coordinate.Point()?.Position()?;
        let altitude = coordinate
            .AltitudeAccuracy()
            .and_then(|r| r.Value())
            .ok()
            .map(|_| point.Altitude);
        let speed = coordinate.Speed().and_then(|r| r.Value()).ok();
        let heading = coordinate.Heading().and_then(|r| r.Value()).ok();
        let stamp = coordinate.Timestamp()?.UniversalTime;
        Ok(Latest::Here(position(
            point.Latitude,
            point.Longitude,
            coordinate.Accuracy()?,
            altitude,
            speed,
            heading,
            age_ms(stamp, now_ms()),
        )))
    }

    fn read_until_idle() {
        let access = Geolocator::RequestAccessAsync().and_then(|op| op.join());
        if access == Ok(GeolocationAccessStatus::Denied) {
            set(Latest::Refused);
            return;
        }
        let Ok(locator) = Geolocator::new() else {
            set(Latest::Off);
            return;
        };
        let _ = locator.SetDesiredAccuracy(PositionAccuracy::High);
        while now_ms().saturating_sub(LAST_ASK_MS.load(Ordering::Relaxed)) < IDLE_MS {
            match locator.GetGeopositionAsync().and_then(|op| op.join()) {
                Ok(at) => {
                    if let Ok(fix) = fix_of(&at) {
                        set(fix);
                    }
                }
                // Access denied: the person said no in Settings.
                Err(err) if err.code().0 as u32 == 0x8007_0005 => {
                    set(Latest::Refused);
                    return;
                }
                Err(_) => {
                    let status = locator.LocationStatus().map(|s| s.0).unwrap_or(5);
                    if !status_can_tell(status) {
                        set(Latest::Off);
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        // Idle: the next ask starts the reader again.
        let mut latest = LATEST.lock().expect("location lock");
        if !matches!(*latest, Some(Latest::Refused)) {
            *latest = None;
        }
    }

    pub fn available() -> bool {
        Geolocator::new()
            .and_then(|g| g.LocationStatus())
            .map(|s| status_can_tell(s.0))
            .unwrap_or(false)
    }

    pub fn read() -> krate_adapter_common::ui::LocationAnswer {
        LAST_ASK_MS.store(now_ms(), Ordering::Relaxed);
        let latest = {
            let mut slot = LATEST.lock().expect("location lock");
            match *slot {
                Some(latest) => latest,
                None => {
                    *slot = Some(Latest::Finding);
                    std::thread::spawn(read_until_idle);
                    Latest::Finding
                }
            }
        };
        super::answer_of(latest)
    }
}

#[cfg(windows)]
pub use geolocator::{available, read};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thing_the_reader_learns_is_the_answer_a_phone_gives() {
        assert_eq!(answer_of(Latest::Finding), LocationAnswer::Finding);
        assert_eq!(answer_of(Latest::Refused), LocationAnswer::NotAllowed);
        assert_eq!(answer_of(Latest::Off), LocationAnswer::Unavailable);
        let here = position(51.5, -0.12, 30.0, None, None, None, 5);
        assert_eq!(answer_of(Latest::Here(here)), LocationAnswer::Here(here));
    }

    #[test]
    fn switched_off_or_nothing_to_tell_with_is_not_available() {
        for can in [0, 1, 2, 4] {
            assert!(status_can_tell(can), "{can}");
        }
        assert!(!status_can_tell(3), "disabled");
        assert!(!status_can_tell(5), "not available");
    }

    #[test]
    fn a_fix_says_only_what_windows_knows() {
        let p = position(51.5, -0.12, 30.0, Some(12.0), Some(-1.0), Some(f64::NAN), 0);
        assert_eq!(p.altitude, Some(12.0));
        assert_eq!((p.speed, p.course), (None, None));
        let moving = position(51.5, -0.12, 5.0, None, Some(1.5), Some(270.0), 0);
        assert_eq!((moving.speed, moving.course), (Some(1.5), Some(270.0)));
    }

    #[test]
    fn a_fix_s_age_counts_from_its_stamp() {
        // 2026-01-01T00:00:00Z: 1_767_225_600 s after 1970.
        let unix_ms = 1_767_225_600_000u64;
        let ticks = 116_444_736_000_000_000 + (unix_ms as i64) * 10_000;
        assert_eq!(age_ms(ticks, unix_ms + 1500), 1500);
        assert_eq!(age_ms(ticks, unix_ms - 10), 0, "a clock behind the fix");
    }
}
