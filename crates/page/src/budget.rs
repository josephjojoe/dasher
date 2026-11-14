//! Budgets that bound how much work a page load does.

use std::time::Duration;

/// Default for [`SettleOptions::page_time_budget`](crate::SettleOptions::page_time_budget).
pub const DEFAULT_PAGE_TIME_BUDGET: Duration = Duration::from_secs(5);

/// Default for [`WorkBudget::cpu`].
pub const DEFAULT_CPU_BUDGET: Duration = Duration::from_secs(10);

/// Default for [`WorkBudget::requests`].
pub const DEFAULT_REQUEST_BUDGET: u64 = 1000;

/// How much work a page may do, from when the budget is set (at the start
/// of [`Page::load`](crate::Page::load), or by
/// [`Page::set_budget`](crate::Page::set_budget)) on, across the
/// navigations it follows. When either runs out the page is stopped where
/// it is, which is not an error: loading, settling and scrolling return
/// the page as it stands and say which budget ended them.
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
}

impl Default for WorkBudget {
    fn default() -> Self {
        Self {
            cpu: Some(DEFAULT_CPU_BUDGET),
            requests: Some(DEFAULT_REQUEST_BUDGET),
        }
    }
}

impl WorkBudget {
    /// No limits.
    pub fn unlimited() -> Self {
        Self {
            cpu: None,
            requests: None,
        }
    }
}
