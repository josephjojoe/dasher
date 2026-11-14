//! Driving the event loop asynchronously until the page goes quiet.

use std::time::{Duration, Instant};

use crate::budget::Budget;
use crate::clock::ClockMode;
use crate::event_loop::Stopped;
use crate::limits::ResourceLimit;
use crate::runtime::JsRuntime;

/// Tasks run between yields to the async executor, so other futures on the
/// same thread (such as the network requests a page is waiting on) progress.
const TASKS_PER_YIELD: usize = 64;

/// The longest tasks run without a yield, however few they are. A page whose
/// tasks take a millisecond each, as a framework's render slices do, would
/// otherwise never reach [`TASKS_PER_YIELD`] before the caller's `until`,
/// and a caller that comes straight back for more (as settling does) would
/// never let the script that page is waiting for be fetched. So the time is
/// kept across calls.
const YIELD_INTERVAL: Duration = Duration::from_millis(1);

/// Bounds for [`JsRuntime::run`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunLimits {
    /// Page time at which to stop. Work due later is left scheduled.
    pub until: Option<Duration>,
    /// Maximum number of tasks and animation frames to run.
    pub max_tasks: Option<usize>,
}

impl RunLimits {
    /// Stop once page time reaches `time`.
    pub fn until(time: Duration) -> Self {
        Self {
            until: Some(time),
            max_tasks: None,
        }
    }
}

/// Why [`JsRuntime::run`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunOutcome {
    /// Nothing is left: no tasks, timers, animation frames or host
    /// operations in progress.
    Idle,
    /// Page time reached [`RunLimits::until`]; later work is still scheduled.
    ReachedTime,
    /// [`RunLimits::max_tasks`] tasks ran.
    TaskLimit,
    /// The runtime's deadline passed.
    DeadlineExceeded,
    /// An [`InterruptHandle`](crate::InterruptHandle) asked it to stop.
    Interrupted,
    /// The page went over a resource limit and is stopped for good (see
    /// [`JsRuntime::limit_exceeded`]).
    LimitExceeded(ResourceLimit),
    /// The page used up its CPU time or request budget (see
    /// [`JsRuntime::set_work_limits`]); it runs no more JS until the
    /// limits are set again. Or, with [`Budget::PageTime`], the
    /// [skip limit](JsRuntime::set_skip_limit) was reached under
    /// [`ClockMode::Manual`], so page time can move no further.
    BudgetExhausted(Budget),
    /// The page asked to load another document (see
    /// [`JsRuntime::navigation_request`]). The loop stops after the task
    /// that asked, since the current document is about to be replaced, and
    /// returns this straight away while the request is pending.
    NavigationRequested,
}

impl From<Stopped> for RunOutcome {
    fn from(stopped: Stopped) -> Self {
        match stopped {
            Stopped::DeadlineExceeded => Self::DeadlineExceeded,
            Stopped::Interrupted => Self::Interrupted,
            Stopped::LimitExceeded(limit) => Self::LimitExceeded(limit),
            Stopped::BudgetExhausted(budget) => Self::BudgetExhausted(budget),
        }
    }
}

impl JsRuntime {
    /// Runs the event loop until it is idle or a limit is reached.
    ///
    /// When nothing is runnable it waits for the next timer or animation
    /// frame, for tasks posted through a [`TaskSender`], or for the
    /// deadline. In [`ClockMode::Virtual`] and [`ClockMode::Manual`] it
    /// skips ahead to the next timer or frame instead of waiting, unless
    /// host operations are in progress; then time runs at wall-clock speed
    /// (frozen in `Manual`) until they finish.
    ///
    /// Skipping stops at the [skip limit](Self::set_skip_limit): past it,
    /// `Virtual` waits for timers in real time, and `Manual`, where page
    /// time cannot otherwise move, returns
    /// [`RunOutcome::BudgetExhausted`] with [`Budget::PageTime`].
    ///
    /// Callbacks waiting in `requestIdleCallback` run as they come due, but
    /// do not keep the loop going: with nothing else left it is
    /// [`Idle`](RunOutcome::Idle).
    ///
    /// Must be polled inside a Tokio runtime with the time driver enabled,
    /// on the thread that owns this runtime (for example in a `LocalSet`).
    ///
    /// [`TaskSender`]: crate::TaskSender
    pub async fn run(&self, limits: RunLimits) -> RunOutcome {
        let state = self.event_loop();
        let clock = &state.clock;
        let remote = &self.shared().remote;
        let mut ran = 0;
        loop {
            loop {
                if limits.max_tasks.is_some_and(|max| ran >= max) {
                    return RunOutcome::TaskLimit;
                }
                if self.shared().window.has_navigation() {
                    return RunOutcome::NavigationRequested;
                }
                match self.step() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(stopped) => return stopped.into(),
                }
                ran += 1;
                // A page that always has work due must still give the
                // caller its turn once page time is past `until`.
                if limits.until.is_some_and(|until| clock.now() > until) {
                    return RunOutcome::ReachedTime;
                }
                if ran % TASKS_PER_YIELD == 0 || state.yielded_at.get().elapsed() >= YIELD_INTERVAL
                {
                    tokio::task::yield_now().await;
                    state.yielded_at.set(Instant::now());
                }
            }

            let now = clock.now();
            if limits.until.is_some_and(|until| now >= until) {
                return RunOutcome::ReachedTime;
            }
            // Pending first: an operation queues its task before it ends.
            let pending = remote.has_pending();
            if remote.has_queued() {
                continue;
            }
            // Idle callbacks run when they come due, but alone they do not
            // keep the loop going: with nothing else left, it is idle.
            let next = state.next_holding_wakeup().and_then(|_| self.next_wakeup());
            if !pending && clock.mode() != ClockMode::Realtime {
                // Nothing but timers and frames: skip the idle gap, as far
                // as the skip limit allows. Past it, wait in real time.
                match (next, limits.until) {
                    (None, _) => return RunOutcome::Idle,
                    (Some(next), Some(until)) if next > until => {
                        if clock.advance_to(until) {
                            return RunOutcome::ReachedTime;
                        }
                    }
                    (Some(next), _) => {
                        if clock.advance_to(next) {
                            continue;
                        }
                    }
                }
                if clock.mode() == ClockMode::Manual {
                    // Page time cannot move, so nothing will come due.
                    return RunOutcome::BudgetExhausted(Budget::PageTime);
                }
            }
            if !pending && next.is_none() {
                return RunOutcome::Idle;
            }

            // Wait for the next timer or frame (page time runs in real time
            // except in Manual mode), a posted task, or the deadline.
            let now = clock.now();
            let wake_at = match clock.mode() {
                ClockMode::Manual => None,
                ClockMode::Realtime | ClockMode::Virtual => match (next, limits.until) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                },
            };
            let deadline = self.deadline();
            tokio::select! {
                () = remote.notify.notified() => {}
                () = sleep_for(wake_at.map(|at| at.saturating_sub(now))) => {}
                () = sleep_until(deadline) => {}
            }
            state.yielded_at.set(Instant::now());
        }
    }
}

async fn sleep_for(duration: Option<Duration>) {
    match duration {
        Some(duration) => tokio::time::sleep(duration).await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
        None => std::future::pending().await,
    }
}
