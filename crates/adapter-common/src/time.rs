//! Shared time helpers for host adapters.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A monotonic clock that does not read the host at all: it reports whole
/// frames of a fixed length, and only moves when something tells it a frame
/// ended.
///
/// This exists for screenshots. An app that animates reads the monotonic
/// clock, works out how long the last frame took, and advances its motion by
/// that much. On a real run that is exactly right. On a screenshot it makes
/// the captured frame a function of how fast the machine happened to be, so
/// two shots of the same app from the same binary disagree -- which is K-721:
/// the `cubes` golden failed on roughly half of all macOS pushes with no code
/// change behind it, because the cubes had spun a different distance by the
/// time the shot was taken. With a stepped clock the app sees frame 0, frame
/// 1, frame 2 at a fixed 16 ms apart however long the host really took, so
/// the shot after N frames is always the same picture.
///
/// It is never on for a person's run. The real clock is what makes an app
/// animate; a stepped clock would make it animate in host-speed-independent
/// time, which is wrong everywhere except a comparison.
#[derive(Debug, Clone)]
pub struct SteppedClock {
    step_nanos: u64,
    frames: Rc<Cell<u64>>,
}

impl SteppedClock {
    /// A clock whose every frame lasts `step_nanos`.
    pub fn new(step_nanos: u64) -> Self {
        Self {
            step_nanos,
            frames: Rc::new(Cell::new(0)),
        }
    }

    /// End the current frame. The host calls this once per presented frame, so
    /// every clock read inside one frame sees the same instant -- an app that
    /// reads the clock twice in a frame must not see time move between the two
    /// reads, or its own timing arithmetic goes out by a step.
    pub fn end_frame(&self) {
        self.frames.set(self.frames.get().saturating_add(1));
    }

    /// Frames ended so far. The screenshot delay counts these instead of real
    /// milliseconds, so "shoot after 400 ms" means the same frame every run.
    pub fn frames(&self) -> u64 {
        self.frames.get()
    }

    pub fn step_nanos(&self) -> u64 {
        self.step_nanos
    }

    fn monotonic_nanos(&self) -> u64 {
        self.frames.get().saturating_mul(self.step_nanos)
    }
}

/// Clock behavior shared by local Phase 2 host adapters.
#[derive(Debug)]
pub struct HostClock {
    started: Instant,
    fixed_wall_millis: Option<u64>,
    stepped: Option<SteppedClock>,
}

impl HostClock {
    pub fn new(fixed_wall_millis: Option<u64>) -> Self {
        Self {
            started: Instant::now(),
            fixed_wall_millis,
            stepped: None,
        }
    }

    /// Serve the monotonic clock from a stepped clock rather than the host.
    /// Only the screenshot path sets this -- see [`SteppedClock`].
    pub fn with_stepped(mut self, stepped: Option<SteppedClock>) -> Self {
        self.stepped = stepped;
        self
    }

    pub fn now_millis(&self) -> Result<u64, TimeError> {
        if let Some(millis) = self.fixed_wall_millis {
            return Ok(millis);
        }

        system_time_millis(SystemTime::now())
    }

    pub fn monotonic_nanos(&self) -> u64 {
        if let Some(stepped) = self.stepped.as_ref() {
            return stepped.monotonic_nanos();
        }

        duration_nanos_saturated(self.started.elapsed())
    }

    pub fn sleep_millis(millis: u32) {
        std::thread::sleep(Duration::from_millis(millis.into()));
    }
}

pub fn system_time_millis(time: SystemTime) -> Result<u64, TimeError> {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TimeError::BeforeUnixEpoch)?
        .as_millis();

    millis_to_u64(millis)
}

pub fn duration_nanos_saturated(duration: Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

pub fn millis_to_u64(millis: u128) -> Result<u64, TimeError> {
    millis.try_into().map_err(|_| TimeError::OutOfRange)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("system time is before Unix epoch")]
    BeforeUnixEpoch,
    #[error("system time is outside the supported millisecond range")]
    OutOfRange,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_wall_clock_returns_configured_value() {
        let clock = HostClock::new(Some(1_777));

        assert_eq!(clock.now_millis().expect("fixed clock"), 1_777);
    }

    #[test]
    fn system_time_millis_rejects_pre_epoch_times() {
        let err = system_time_millis(UNIX_EPOCH - Duration::from_millis(1))
            .expect_err("pre-epoch time should fail");

        assert_eq!(err, TimeError::BeforeUnixEpoch);
    }

    #[test]
    fn stepped_monotonic_clock_moves_only_when_a_frame_ends() {
        let stepped = SteppedClock::new(16_000_000);
        let clock = HostClock::new(None).with_stepped(Some(stepped.clone()));

        // Two reads inside one frame must agree. An app that reads the clock
        // twice per frame would otherwise see time move mid-frame.
        assert_eq!(clock.monotonic_nanos(), 0);
        assert_eq!(clock.monotonic_nanos(), 0);

        stepped.end_frame();
        assert_eq!(clock.monotonic_nanos(), 16_000_000);
        stepped.end_frame();
        assert_eq!(clock.monotonic_nanos(), 32_000_000);
        assert_eq!(stepped.frames(), 2);
    }

    #[test]
    fn stepped_clock_ignores_how_long_the_host_really_took() {
        // This is the K-721 property: the same number of frames is the same
        // reported time however slow the machine was in between.
        let stepped = SteppedClock::new(16_000_000);
        let clock = HostClock::new(None).with_stepped(Some(stepped.clone()));

        stepped.end_frame();
        std::thread::sleep(Duration::from_millis(50));
        stepped.end_frame();

        assert_eq!(clock.monotonic_nanos(), 32_000_000);
    }

    #[test]
    fn a_clock_without_a_step_still_reads_the_host() {
        let clock = HostClock::new(None).with_stepped(None);
        let first = clock.monotonic_nanos();
        std::thread::sleep(Duration::from_millis(5));

        assert!(
            clock.monotonic_nanos() > first,
            "an ordinary run must still see real time pass, or apps stop animating"
        );
    }

    #[test]
    fn monotonic_clock_does_not_move_backwards() {
        let clock = HostClock::new(None);
        let first = clock.monotonic_nanos();
        let second = clock.monotonic_nanos();

        assert!(second >= first);
    }

    #[test]
    fn system_time_millis_rejects_out_of_range_values() {
        let err = millis_to_u64(u64::MAX as u128 + 1).expect_err("large millis should fail");

        assert_eq!(err, TimeError::OutOfRange);
    }

    #[test]
    fn duration_nanos_saturates_instead_of_wrapping() {
        let nanos = duration_nanos_saturated(Duration::new(u64::MAX, 999_999_999));

        assert_eq!(nanos, u64::MAX);
    }
}
