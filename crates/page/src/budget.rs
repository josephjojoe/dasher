//! Budgets that bound how much work a page load does.

use std::time::Duration;

/// Default for [`SettleOptions::page_time_budget`](crate::SettleOptions::page_time_budget).
pub const DEFAULT_PAGE_TIME_BUDGET: Duration = Duration::from_secs(5);

/// Default for [`WorkBudget::cpu`].
pub const DEFAULT_CPU_BUDGET: Duration = Duration::from_secs(10);

/// Default for [`WorkBudget::requests`].
pub const DEFAULT_REQUEST_BUDGET: u64 = 1000;

/// Default for [`WorkBudget::wait`].
pub const DEFAULT_WAIT_BUDGET: Duration = Duration::from_secs(5);

/// How much work a page may do and how long it is waited for, from when
/// the budget is set (at the start of [`Page::load`](crate::Page::load),
/// or by [`Page::set_budget`](crate::Page::set_budget)) on, across the
/// navigations it follows. When one runs out the page is left where it
/// is, which is not an error: loading, settling and scrolling return the
/// page as it stands and say which budget ended them.
///
/// Running out of CPU time or requests stops the page: the script that is
/// running is interrupted and no more JS runs. Running out of the wait
/// interrupts nothing; the page is only waited for no longer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkBudget {
    /// CPU time, user and system, of the thread the page runs on.
    /// Defaults to [`DEFAULT_CPU_BUDGET`]. Pages sharing a thread are
    /// charged for each other's work while they run at the same time.
    pub cpu: Option<Duration>,
    /// Requests made through the page's session: the document, scripts,
    /// `fetch()` and XHR, redirects counting once. Defaults to
    /// [`DEFAULT_REQUEST_BUDGET`]. The page stops when it starts one more
    /// than this.
    pub requests: Option<u64>,
    /// How long, in real time, the page is waited for. Defaults to
    /// [`DEFAULT_WAIT_BUDGET`]. Once it has passed nothing new is
    /// started: no task of the page's event loop (timers, network
    /// callbacks, events), no deferred or async script, no
    /// `DOMContentLoaded` or `load` event. Nothing that is not there yet
    /// is waited for either: a response for the document, more of its
    /// body, a script, a request, a quiet network.
    ///
    /// What is under way finishes. A script that is running is not
    /// interrupted, the parser goes on through the part of the document
    /// that has arrived, and it still waits for and runs the scripts it
    /// meets there, since what follows a script may depend on it. Those
    /// are bounded by the CPU budget and the time limit
    /// ([`PageOptions::timeout`](crate::PageOptions::timeout)) alone.
    pub wait: Option<Duration>,
}

impl Default for WorkBudget {
    fn default() -> Self {
        Self {
            cpu: Some(DEFAULT_CPU_BUDGET),
            requests: Some(DEFAULT_REQUEST_BUDGET),
            wait: Some(DEFAULT_WAIT_BUDGET),
        }
    }
}

impl WorkBudget {
    /// No limits.
    pub fn unlimited() -> Self {
        Self {
            cpu: None,
            requests: None,
            wait: None,
        }
    }
}
