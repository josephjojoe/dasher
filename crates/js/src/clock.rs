//! Page time: the clock behind timers, animation frames, `performance.now()`
//! and `Date`.

use std::cell::Cell;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How page time relates to the wall clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ClockMode {
    /// Page time follows the wall clock and timers fire when they are due in
    /// real time, as in a browser.
    #[default]
    Realtime,
    /// Page time follows the wall clock, but whenever the page is only
    /// waiting on timers or animation frames (no tasks queued and no host
    /// work such as network requests outstanding), the event loop jumps
    /// ahead to the next one instead of sleeping. Long timers fire at once
    /// while scripts still see time pass consistently.
    Virtual,
    /// Page time only moves when the event loop jumps ahead. Deterministic,
    /// for tests.
    Manual,
}

/// Page time since the time origin. Ahead of the wall clock by however much
/// the event loop has skipped.
#[derive(Debug)]
pub(crate) struct Clock {
    mode: ClockMode,
    start: Instant,
    /// The time origin as milliseconds since the Unix epoch.
    epoch_ms: f64,
    skipped: Cell<Duration>,
    /// The most `skipped` may reach, if skipping is limited.
    skip_limit: Cell<Option<Duration>>,
}

impl Clock {
    pub(crate) fn new(mode: ClockMode) -> Self {
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        Self {
            mode,
            start: Instant::now(),
            epoch_ms,
            skipped: Cell::new(Duration::ZERO),
            skip_limit: Cell::new(None),
        }
    }

    pub(crate) fn mode(&self) -> ClockMode {
        self.mode
    }

    /// Page time elapsed since the time origin.
    pub(crate) fn now(&self) -> Duration {
        match self.mode {
            ClockMode::Manual => self.skipped.get(),
            ClockMode::Realtime | ClockMode::Virtual => self.start.elapsed() + self.skipped.get(),
        }
    }

    /// Moves page time forward to `time`, or as far towards it as the
    /// [skip limit](Self::set_skip_limit) allows. Never moves it back.
    /// Returns whether page time reached `time`.
    pub(crate) fn advance_to(&self, time: Duration) -> bool {
        let now = self.now();
        if time <= now {
            return true;
        }
        let wanted = time - now;
        let skip = self.skip_left().map_or(wanted, |left| left.min(wanted));
        self.skipped.set(self.skipped.get() + skip);
        skip == wanted
    }

    /// How much page time has been skipped in all: how far page time is
    /// ahead of the wall clock (in `Manual` mode, all of page time).
    pub(crate) fn skipped(&self) -> Duration {
        self.skipped.get()
    }

    /// Caps [`skipped`](Self::skipped) at `limit`: page time no longer
    /// jumps ahead once it has skipped that much in all. `None` lifts the
    /// cap.
    pub(crate) fn set_skip_limit(&self, limit: Option<Duration>) {
        self.skip_limit.set(limit);
    }

    /// How much more page time may be skipped, if skipping is limited.
    pub(crate) fn skip_left(&self) -> Option<Duration> {
        self.skip_limit
            .get()
            .map(|limit| limit.saturating_sub(self.skipped.get()))
    }

    /// The time origin as milliseconds since the Unix epoch, which is what
    /// `performance.timeOrigin` reports and `Date.now()` counts from.
    pub(crate) fn epoch_ms(&self) -> f64 {
        self.epoch_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_only_moves_when_advanced() {
        let clock = Clock::new(ClockMode::Manual);
        assert_eq!(clock.now(), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(clock.now(), Duration::ZERO);
        clock.advance_to(Duration::from_secs(10));
        assert_eq!(clock.now(), Duration::from_secs(10));
        clock.advance_to(Duration::from_secs(1));
        assert_eq!(clock.now(), Duration::from_secs(10));
    }

    #[test]
    fn skipping_stops_at_the_limit() {
        let clock = Clock::new(ClockMode::Manual);
        let s = Duration::from_secs;
        clock.set_skip_limit(Some(s(5)));
        assert!(clock.advance_to(s(3)));
        assert_eq!(clock.skip_left(), Some(s(2)));
        assert!(!clock.advance_to(s(10)));
        assert_eq!(clock.now(), s(5));
        assert_eq!(clock.skip_left(), Some(Duration::ZERO));
        assert!(!clock.advance_to(s(6)));
        assert_eq!(clock.now(), s(5));
        clock.set_skip_limit(None);
        assert!(clock.advance_to(s(6)));
        assert_eq!(clock.skipped(), s(6));
    }

    #[test]
    fn virtual_clock_runs_in_real_time_plus_skips() {
        let clock = Clock::new(ClockMode::Virtual);
        clock.advance_to(Duration::from_secs(60));
        let a = clock.now();
        assert!(a >= Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
        assert!(clock.now() >= a + Duration::from_millis(5));
    }

    #[test]
    fn epoch_is_close_to_now() {
        let clock = Clock::new(ClockMode::Realtime);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            * 1000.0;
        assert!((now_ms - clock.epoch_ms()).abs() < 1000.0);
    }
}
