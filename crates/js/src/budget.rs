//! Work budgets: how much CPU time and how many requests a page may use
//! before it is stopped, and which budget ran out.
//!
//! The limits are thresholds, as a deadline is: the page stops once the
//! thread's CPU time reaches one, or once its session has started more
//! requests than the other. The host works them out from a budget when an
//! operation starts (see `dasher_page`), so they carry over unchanged to
//! the runtime of a document the page navigates to.

use std::cell::Cell;
use std::fmt;
use std::time::{Duration, Instant};

/// A budget that ran out, ending what the page was doing.
///
/// Unlike a [`ResourceLimit`](crate::ResourceLimit), running out of a
/// budget is not a failure: the page is left as it is, and lifting the
/// limits lets it run again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Budget {
    /// The virtual clock skipped as far ahead as it may (see
    /// [`JsRuntime::set_skip_limit`](crate::JsRuntime::set_skip_limit)).
    PageTime,
    /// The page used up its CPU time ([`WorkLimits::cpu_time`]).
    Cpu,
    /// The page made too many requests ([`WorkLimits::requests`]).
    Requests,
}

impl Budget {
    /// The budget's snake_case name: `page_time`, `cpu` or `requests`.
    pub fn name(self) -> &'static str {
        match self {
            Self::PageTime => "page_time",
            Self::Cpu => "cpu",
            Self::Requests => "requests",
        }
    }
}

impl fmt::Display for Budget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PageTime => "the page-time budget ran out",
            Self::Cpu => "the CPU budget ran out",
            Self::Requests => "the request budget ran out",
        })
    }
}

/// When a page has done enough work and is stopped, as
/// [`JsRuntime::set_work_limits`](crate::JsRuntime::set_work_limits)
/// takes them. `None` leaves that side unlimited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WorkLimits {
    /// The [`thread_cpu_time`] at which the page stops.
    pub cpu_time: Option<Duration>,
    /// The page stops once its session's
    /// [`requests_started`](dasher_net::NetworkActivity::requests_started)
    /// count goes past this.
    pub requests: Option<u64>,
}

impl WorkLimits {
    /// Limits that let the page use `cpu` more CPU time and make
    /// `requests` more requests from now, given how many its session has
    /// started so far.
    pub fn from_now(cpu: Option<Duration>, requests: Option<u64>, started: u64) -> Self {
        Self {
            cpu_time: cpu.map(|cpu| thread_cpu_time() + cpu),
            requests: requests.map(|n| started.saturating_add(n)),
        }
    }
}

/// CPU time the calling thread has used, user and system.
///
/// A page runs on one thread, so this is its CPU time, plus that of
/// anything else the thread runs. Where the platform cannot say, it falls
/// back to wall time since the first call, which is never less.
pub fn thread_cpu_time() -> Duration {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid, writable timespec.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } == 0 {
            let secs = u64::try_from(ts.tv_sec).unwrap_or(0);
            let nanos = u32::try_from(ts.tv_nsec).unwrap_or(0);
            return Duration::new(secs, nanos);
        }
    }
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// A runtime's work limits and whether one has been reached.
#[derive(Debug)]
pub(crate) struct BudgetState {
    limits: Cell<WorkLimits>,
    exhausted: Cell<Option<Budget>>,
    /// The thread's CPU time when last read, and when that was. CPU time
    /// grows no faster than wall time, so until the wall clock has moved
    /// far enough for the limit to be possible it need not be read again.
    cpu_seen: Cell<Option<(Duration, Instant)>>,
}

impl Default for BudgetState {
    fn default() -> Self {
        Self {
            limits: Cell::new(WorkLimits::default()),
            exhausted: Cell::new(None),
            cpu_seen: Cell::new(None),
        }
    }
}

impl BudgetState {
    pub(crate) fn set(&self, limits: WorkLimits) {
        self.limits.set(limits);
        self.exhausted.set(None);
        self.cpu_seen.set(None);
    }

    pub(crate) fn limits(&self) -> WorkLimits {
        self.limits.get()
    }

    /// The budget that ran out, if one has; `requests_started` gives the
    /// session's request count. Once one has run out it stays out until
    /// the limits are set again.
    pub(crate) fn check(&self, requests_started: impl FnOnce() -> u64) -> Option<Budget> {
        if let Some(budget) = self.exhausted.get() {
            return Some(budget);
        }
        let limits = self.limits.get();
        let out = if limits.requests.is_some_and(|max| requests_started() > max) {
            Some(Budget::Requests)
        } else if limits.cpu_time.is_some_and(|at| self.cpu_reached(at)) {
            Some(Budget::Cpu)
        } else {
            None
        };
        self.exhausted.set(out);
        out
    }

    fn cpu_reached(&self, at: Duration) -> bool {
        let now = Instant::now();
        if let Some((cpu, when)) = self.cpu_seen.get()
            && cpu + now.saturating_duration_since(when) < at
        {
            return false;
        }
        let cpu = thread_cpu_time();
        self.cpu_seen.set(Some((cpu, now)));
        cpu >= at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_cpu_time_counts_this_threads_work_only() {
        // Work on this thread shows, and never faster than wall time.
        let start = thread_cpu_time();
        let wall = Instant::now();
        let mut x = 0u64;
        while thread_cpu_time() - start < Duration::from_millis(20) {
            assert!(wall.elapsed() < Duration::from_secs(10));
            x = std::hint::black_box(x.wrapping_add(1));
        }
        assert!(thread_cpu_time() - start <= wall.elapsed() + Duration::from_millis(2));

        // Another thread's work does not.
        let before = thread_cpu_time();
        std::thread::spawn(|| {
            let spin = Instant::now();
            let mut y = 0u64;
            while spin.elapsed() < Duration::from_millis(50) {
                y = std::hint::black_box(y.wrapping_add(1));
            }
        })
        .join()
        .unwrap();
        assert!(thread_cpu_time() - before < Duration::from_millis(20));
    }

    #[test]
    fn requests_run_out_past_the_limit_and_stay_out() {
        let state = BudgetState::default();
        state.set(WorkLimits::from_now(None, Some(3), 10));
        assert_eq!(state.check(|| 13), None);
        assert_eq!(state.check(|| 14), Some(Budget::Requests));
        assert_eq!(state.check(|| 0), Some(Budget::Requests));
        state.set(WorkLimits::default());
        assert_eq!(state.check(|| 100), None);
    }

    #[test]
    fn cpu_runs_out_once_used() {
        let state = BudgetState::default();
        state.set(WorkLimits::from_now(
            Some(Duration::from_millis(20)),
            None,
            0,
        ));
        assert_eq!(state.check(|| 0), None);
        let spin = Instant::now();
        while state.check(|| 0).is_none() {
            assert!(spin.elapsed() < Duration::from_secs(5));
        }
        assert_eq!(state.check(|| 0), Some(Budget::Cpu));
        assert!(spin.elapsed() >= Duration::from_millis(15));
    }
}
