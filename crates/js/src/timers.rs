//! Bookkeeping for `setTimeout` / `setInterval`.
//!
//! Only ids and due times live here; the callbacks stay on the JS side, keyed
//! by the same ids.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

pub(crate) type TimerId = u32;

/// Timers nested deeper than this get their timeout raised to
/// [`MIN_NESTED_TIMEOUT`] (HTML timer initialization steps).
const MAX_UNCLAMPED_NESTING: u32 = 5;
const MIN_NESTED_TIMEOUT: Duration = Duration::from_millis(4);

#[derive(Debug)]
struct Timer {
    /// The key in `Timers::queue`, or `None` while the timer is queued to run
    /// or running.
    key: Option<(Duration, u64)>,
    /// The repeat interval for `setInterval`.
    interval: Option<Duration>,
    /// The nesting level of the task that runs this timer.
    nesting: u32,
}

/// What a timer needs to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fire {
    pub(crate) repeat: bool,
    pub(crate) nesting: u32,
}

#[derive(Debug, Default)]
pub(crate) struct Timers {
    last_id: TimerId,
    seq: u64,
    /// Scheduled timers ordered by due time, then by scheduling order.
    queue: BTreeSet<(Duration, u64, TimerId)>,
    timers: HashMap<TimerId, Timer>,
}

impl Timers {
    /// Schedules a timer `timeout` after `now`. `nesting` is the timer
    /// nesting level of the task that is creating it (0 outside timers).
    pub(crate) fn insert(
        &mut self,
        now: Duration,
        timeout: Duration,
        repeat: bool,
        nesting: u32,
    ) -> TimerId {
        let id = self.next_id();
        let timer = Timer {
            key: None,
            interval: repeat.then_some(timeout),
            nesting,
        };
        self.timers.insert(id, timer);
        self.schedule(id, now, timeout);
        id
    }

    fn next_id(&mut self) -> TimerId {
        loop {
            self.last_id = self.last_id.wrapping_add(1);
            if self.last_id != 0 && !self.timers.contains_key(&self.last_id) {
                return self.last_id;
            }
        }
    }

    fn schedule(&mut self, id: TimerId, now: Duration, timeout: Duration) {
        let Some(timer) = self.timers.get_mut(&id) else {
            return;
        };
        let timeout = if timer.nesting > MAX_UNCLAMPED_NESTING {
            timeout.max(MIN_NESTED_TIMEOUT)
        } else {
            timeout
        };
        timer.nesting += 1;
        self.seq += 1;
        let key = (now + timeout, self.seq);
        timer.key = Some(key);
        self.queue.insert((key.0, key.1, id));
    }

    /// Cancels a timer. Unknown ids are ignored.
    pub(crate) fn remove(&mut self, id: TimerId) {
        if let Some(Timer { key: Some(key), .. }) = self.timers.remove(&id) {
            self.queue.remove(&(key.0, key.1, id));
        }
    }

    /// When the earliest scheduled timer is due.
    pub(crate) fn next_due(&self) -> Option<Duration> {
        self.queue.first().map(|&(due, _, _)| due)
    }

    /// Takes the earliest timer due at or before `now` off the schedule,
    /// leaving it to be queued as a task.
    pub(crate) fn pop_due(&mut self, now: Duration) -> Option<TimerId> {
        let &(due, _, id) = self.queue.first()?;
        if due > now {
            return None;
        }
        self.queue.pop_first();
        if let Some(timer) = self.timers.get_mut(&id) {
            timer.key = None;
        }
        Some(id)
    }

    /// Called when a popped timer's task runs. Returns `None` if the timer
    /// was cleared in the meantime. One-shot timers are forgotten here.
    pub(crate) fn start(&mut self, id: TimerId) -> Option<Fire> {
        let timer = self.timers.get(&id)?;
        if timer.key.is_some() {
            // Not popped: this is a stale task for a rescheduled id.
            return None;
        }
        let fire = Fire {
            repeat: timer.interval.is_some(),
            nesting: timer.nesting,
        };
        if !fire.repeat {
            self.timers.remove(&id);
        }
        Some(fire)
    }

    /// Schedules the next run of an interval after its task ran, unless it
    /// was cleared during the run.
    pub(crate) fn reschedule(&mut self, id: TimerId, now: Duration) {
        if let Some(interval) = self.timers.get(&id).and_then(|t| t.interval) {
            self.schedule(id, now, interval);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.timers.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.timers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn drain(timers: &mut Timers, now: Duration) -> Vec<TimerId> {
        std::iter::from_fn(|| timers.pop_due(now)).collect()
    }

    #[test]
    fn fires_in_due_order_then_insertion_order() {
        let mut t = Timers::default();
        let a = t.insert(ms(0), ms(20), false, 0);
        let b = t.insert(ms(0), ms(10), false, 0);
        let c = t.insert(ms(0), ms(10), false, 0);
        assert_eq!(t.next_due(), Some(ms(10)));
        assert_eq!(drain(&mut t, ms(9)), []);
        assert_eq!(drain(&mut t, ms(10)), [b, c]);
        assert_eq!(drain(&mut t, ms(100)), [a]);
    }

    #[test]
    fn ids_are_positive_and_unique() {
        let mut t = Timers::default();
        let ids: Vec<_> = (0..3).map(|_| t.insert(ms(0), ms(0), false, 0)).collect();
        assert_eq!(ids, [1, 2, 3]);
        t.last_id = u32::MAX;
        assert_eq!(t.insert(ms(0), ms(0), false, 0), 4, "skips 0 and live ids");
    }

    #[test]
    fn one_shot_timers_are_forgotten_when_started() {
        let mut t = Timers::default();
        let id = t.insert(ms(0), ms(5), false, 0);
        assert_eq!(t.pop_due(ms(5)), Some(id));
        assert_eq!(
            t.start(id),
            Some(Fire {
                repeat: false,
                nesting: 1
            })
        );
        assert!(t.is_empty());
        assert_eq!(t.start(id), None);
    }

    #[test]
    fn cleared_timers_never_start() {
        let mut t = Timers::default();
        let a = t.insert(ms(0), ms(5), false, 0);
        let b = t.insert(ms(0), ms(5), false, 0);
        t.remove(a);
        assert_eq!(drain(&mut t, ms(5)), [b]);
        // Cleared after being queued but before running.
        t.remove(b);
        assert_eq!(t.start(b), None);
        assert!(t.is_empty());
    }

    #[test]
    fn intervals_reschedule_until_cleared() {
        let mut t = Timers::default();
        let id = t.insert(ms(0), ms(10), true, 0);
        for round in 1..=3u64 {
            assert_eq!(t.pop_due(ms(10 * round)), Some(id));
            assert!(t.start(id).unwrap().repeat);
            t.reschedule(id, ms(10 * round));
        }
        assert_eq!(t.next_due(), Some(ms(40)));
        assert_eq!(t.pop_due(ms(40)), Some(id));
        t.start(id);
        t.remove(id); // clearInterval from inside the callback
        t.reschedule(id, ms(40));
        assert!(t.is_empty());
        assert_eq!(t.next_due(), None);
    }

    #[test]
    fn deeply_nested_timers_are_clamped_to_4ms() {
        let mut t = Timers::default();
        let shallow = t.insert(ms(0), ms(0), false, 5);
        assert_eq!(t.pop_due(ms(0)), Some(shallow));
        let deep = t.insert(ms(0), ms(0), false, 6);
        assert_eq!(t.next_due(), Some(ms(4)));
        assert_eq!(t.pop_due(ms(4)), Some(deep));
        assert_eq!(t.start(deep).unwrap().nesting, 7);
        // Long timeouts are unaffected.
        t.insert(ms(0), ms(50), false, 9);
        assert_eq!(t.next_due(), Some(ms(50)));
    }

    #[test]
    fn zero_delay_intervals_get_clamped_after_a_few_runs() {
        let mut t = Timers::default();
        let id = t.insert(ms(0), ms(0), true, 0);
        let mut dues = Vec::new();
        let mut now = ms(0);
        for _ in 0..8 {
            now = t.next_due().unwrap();
            dues.push(now.as_millis());
            t.pop_due(now);
            t.start(id);
            t.reschedule(id, now);
        }
        assert_eq!(dues, [0, 0, 0, 0, 0, 0, 4, 8]);
        assert_eq!(now, ms(8));
    }
}
