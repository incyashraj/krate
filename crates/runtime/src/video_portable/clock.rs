//! The clock a portable video keeps time by (K-1348).
//!
//! While there is sound to follow, the sound is the clock: the position is
//! how many sound frames the speakers have actually played, so the picture
//! waits when the sound stalls and never runs ahead of what is heard. With
//! no sound (a silent clip, no speakers, or the sound has run out before the
//! picture) the wall clock takes over from wherever the sound left off.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Follow {
    /// Frames the speakers have played: `at_base` of them when the clock
    /// was last set, at `rate` a second.
    Sound { at_base: u64, rate: u32 },
    /// The wall: running `since` the clock was last set, when it runs.
    Wall { since: Option<Instant> },
}

#[derive(Debug, Clone)]
pub(super) struct Clock {
    /// Milliseconds from the start when the clock was last set.
    base: f64,
    running: bool,
    follow: Follow,
}

impl Clock {
    /// A clock that follows the wall, stopped at the start.
    pub(super) fn wall() -> Self {
        Self {
            base: 0.0,
            running: false,
            follow: Follow::Wall { since: None },
        }
    }

    /// A clock that follows the speakers, which play `rate` frames a second
    /// and have played `played` so far.
    pub(super) fn sound(rate: u32, played: u64) -> Self {
        Self {
            base: 0.0,
            running: false,
            follow: Follow::Sound {
                at_base: played,
                rate: rate.max(1),
            },
        }
    }

    pub(super) fn follows_sound(&self) -> bool {
        matches!(self.follow, Follow::Sound { .. })
    }

    pub(super) fn running(&self) -> bool {
        self.running
    }

    /// Where playback is, in milliseconds, with the speakers having played
    /// `played` frames by `now`.
    pub(super) fn millis(&self, played: u64, now: Instant) -> f64 {
        match self.follow {
            Follow::Sound { at_base, rate } => {
                self.base + played.saturating_sub(at_base) as f64 * 1000.0 / f64::from(rate)
            }
            Follow::Wall { since } => {
                self.base
                    + since.map_or(0.0, |since| {
                        now.saturating_duration_since(since).as_secs_f64() * 1000.0
                    })
            }
        }
    }

    /// Put the clock at `millis`, keeping whether it runs.
    pub(super) fn set(&mut self, millis: f64, played: u64, now: Instant) {
        self.base = millis.max(0.0);
        self.follow = match self.follow {
            Follow::Sound { rate, .. } => Follow::Sound {
                at_base: played,
                rate,
            },
            Follow::Wall { .. } => Follow::Wall {
                since: self.running.then_some(now),
            },
        };
    }

    pub(super) fn start(&mut self, played: u64, now: Instant) {
        if !self.running {
            let at = self.millis(played, now);
            self.running = true;
            self.set(at, played, now);
        }
    }

    pub(super) fn pause(&mut self, played: u64, now: Instant) {
        if self.running {
            let at = self.millis(played, now);
            self.running = false;
            self.set(at, played, now);
        }
    }

    /// The sound ran out (or there are no speakers any more): carry on by
    /// the wall from where the sound left off.
    pub(super) fn follow_wall(&mut self, played: u64, now: Instant) {
        let at = self.millis(played, now);
        self.follow = Follow::Wall { since: None };
        self.set(at, played, now);
    }

    /// Sound is back (after a seek): follow the speakers again from here.
    pub(super) fn follow_sound(&mut self, rate: u32, played: u64, now: Instant) {
        let at = self.millis(played, now);
        self.follow = Follow::Sound {
            at_base: played,
            rate: rate.max(1),
        };
        self.set(at, played, now);
    }
}

#[cfg(test)]
mod tests {
    use super::Clock;
    use std::time::{Duration, Instant};

    #[test]
    fn the_sound_clock_moves_only_as_the_speakers_play() {
        let t0 = Instant::now();
        let mut clock = Clock::sound(48_000, 1_000);
        clock.start(1_000, t0);
        // A second of wall time with nothing played: the picture must wait.
        assert_eq!(clock.millis(1_000, t0 + Duration::from_secs(1)), 0.0);
        // Half a second played is half a second, whatever the wall says.
        assert_eq!(clock.millis(25_000, t0 + Duration::from_secs(5)), 500.0);
        clock.set(2_000.0, 25_000, t0);
        assert_eq!(clock.millis(25_000 + 4_800, t0), 2_100.0);
    }

    #[test]
    fn the_wall_clock_runs_only_while_playing() {
        let t0 = Instant::now();
        let mut clock = Clock::wall();
        assert_eq!(clock.millis(0, t0 + Duration::from_secs(3)), 0.0);
        clock.start(0, t0);
        let at = clock.millis(0, t0 + Duration::from_millis(250));
        assert!((at - 250.0).abs() < 0.001, "{at}");
        clock.pause(0, t0 + Duration::from_millis(250));
        let held = clock.millis(0, t0 + Duration::from_secs(9));
        assert!((held - 250.0).abs() < 0.001, "paused stays put: {held}");
        clock.set(1_000.0, 0, t0);
        assert_eq!(clock.millis(0, t0 + Duration::from_secs(9)), 1_000.0);
    }

    #[test]
    fn when_the_sound_runs_out_the_wall_carries_on_from_there() {
        let t0 = Instant::now();
        let mut clock = Clock::sound(1_000, 0);
        clock.start(0, t0);
        assert_eq!(clock.millis(700, t0), 700.0);
        clock.follow_wall(700, t0);
        assert!(!clock.follows_sound());
        let later = clock.millis(700, t0 + Duration::from_millis(300));
        assert!((later - 1_000.0).abs() < 0.001, "{later}");
        clock.follow_sound(1_000, 700, t0 + Duration::from_millis(300));
        assert!(clock.follows_sound());
        assert!((clock.millis(800, t0) - 1_100.0).abs() < 0.001);
    }
}
