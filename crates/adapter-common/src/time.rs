//! Shared time helpers for host adapters.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Clock behavior shared by local Phase 2 host adapters.
#[derive(Debug)]
pub struct HostClock {
    started: Instant,
    fixed_wall_millis: Option<u64>,
}

impl HostClock {
    pub fn new(fixed_wall_millis: Option<u64>) -> Self {
        Self {
            started: Instant::now(),
            fixed_wall_millis,
        }
    }

    pub fn now_millis(&self) -> Result<u64, TimeError> {
        if let Some(millis) = self.fixed_wall_millis {
            return Ok(millis);
        }

        system_time_millis(SystemTime::now())
    }

    pub fn monotonic_nanos(&self) -> u64 {
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
