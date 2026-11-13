//! Deciding when a page is done: [`Page::settle`].

use std::fmt;
use std::time::{Duration, Instant};

use dasher_css::SelectorList;
use dasher_js::{Budget, ClockMode, ResourceLimit, RunLimits, RunOutcome};

use crate::budget::DEFAULT_PAGE_TIME_BUDGET;
use crate::page::{Page, Stop, Until, Wait};

/// How often, in page time, conditions are looked at again while the
/// page is busy.
const POLL: Duration = Duration::from_millis(50);

/// Default quiet period for [`NetworkIdle`] and [`SettleOptions::dom_quiet`].
pub const DEFAULT_QUIET: Duration = Duration::from_millis(500);

/// Default for [`SettleOptions::timeout`].
pub const DEFAULT_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The network-idle condition: no more than `max_in_flight` requests for
/// `quiet`. `max_in_flight: 0` is Puppeteer's `networkidle0`, `2` is
/// `networkidle2`, for pages that keep a long poll or analytics beacon
/// open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NetworkIdle {
    /// Requests that may stay in flight.
    pub max_in_flight: usize,
    /// How long the network must stay that quiet.
    pub quiet: Duration,
}

impl Default for NetworkIdle {
    /// Nothing in flight for [`DEFAULT_QUIET`].
    fn default() -> Self {
        Self {
            max_in_flight: 0,
            quiet: DEFAULT_QUIET,
        }
    }
}

/// When a page counts as settled (see [`Page::settle`]). Every condition
/// that is set must hold at the same moment; with none set the page is
/// settled at once.
///
/// Quiet periods are measured in page time (see
/// [`ClockMode`](dasher_js::ClockMode)), from when `settle` was called at
/// the earliest. Under the virtual clock a page waiting only on timers
/// skips ahead, so a quiet period costs no real time unless the network is
/// busy. How far it may skip is the
/// [page-time budget](Self::page_time_budget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettleOptions {
    /// The network has gone idle: requests through the page's session
    /// (documents, scripts, `fetch` and XHR) stayed within the limit for
    /// the quiet period.
    pub network_idle: Option<NetworkIdle>,
    /// Scripts have not changed the document for this long (see
    /// [`JsRuntime::dom_activity`](dasher_js::JsRuntime::dom_activity)).
    pub dom_quiet: Option<Duration>,
    /// A CSS selector that matches at least one element.
    pub selector: Option<String>,
    /// A JavaScript expression that evaluates to a truthy value, such as
    /// `document.querySelectorAll('.item').length >= 20`. One that throws
    /// counts as false.
    pub predicate: Option<String>,
    /// The longest to wait, in real time. When it runs out the page is
    /// left as it is.
    pub timeout: Duration,
    /// How much page time settling may use waiting for the page to go
    /// quiet. A page that changes its document or polls on a timer
    /// forever never goes quiet; under the virtual clock, skipping from
    /// one tick to the next would run it flat out until the timeout. This
    /// bounds it, as Chrome's `--virtual-time-budget` does: settling may
    /// skip the clock ahead only so far past where it started (the `load`
    /// event, in [`Page::load`] then settle).
    ///
    /// Skipped time counts, and so does page time that passes for real,
    /// with the page running scripts or waiting on timers, except while
    /// requests are in flight: waiting for the network does not count.
    ///
    /// Once it is spent the clock stops skipping, and the quiet
    /// conditions are given up: with no selector or predicate, settling
    /// ends at once with [`SettleOutcome::BudgetExhausted`]; with one, it
    /// waits on in real time for it to hold, then ends the same way.
    /// `None` waits as long as it takes. Defaults to
    /// [`DEFAULT_PAGE_TIME_BUDGET`].
    pub page_time_budget: Option<Duration>,
}

impl Default for SettleOptions {
    /// Network idle and DOM quiet, each for [`DEFAULT_QUIET`], within
    /// [`DEFAULT_SETTLE_TIMEOUT`] and [`DEFAULT_PAGE_TIME_BUDGET`].
    fn default() -> Self {
        Self {
            network_idle: Some(NetworkIdle::default()),
            dom_quiet: Some(DEFAULT_QUIET),
            selector: None,
            predicate: None,
            timeout: DEFAULT_SETTLE_TIMEOUT,
            page_time_budget: Some(DEFAULT_PAGE_TIME_BUDGET),
        }
    }
}

impl SettleOptions {
    /// No conditions, within [`DEFAULT_SETTLE_TIMEOUT`] and
    /// [`DEFAULT_PAGE_TIME_BUDGET`]. Set the ones wanted.
    pub fn none() -> Self {
        Self {
            network_idle: None,
            dom_quiet: None,
            ..Self::default()
        }
    }
}

/// How [`Page::settle`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettleOutcome {
    /// Every condition held.
    Settled,
    /// The timeout ran out first.
    TimedOut,
    /// The page asked to load another document. Call
    /// [`Page::follow_navigation`] to follow it.
    NavigationRequested,
    /// An [`InterruptHandle`](dasher_js::InterruptHandle) stopped the page.
    Interrupted,
    /// The page went over a resource limit and can run no more.
    LimitExceeded(ResourceLimit),
    /// A budget ran out and the page was left as it is. With
    /// [`Budget::PageTime`], the
    /// [page-time budget](SettleOptions::page_time_budget) was spent
    /// before the page went quiet; the selector and predicate, if any,
    /// hold. With [`Budget::Cpu`] or [`Budget::Requests`], the page's
    /// [work budget](crate::WorkBudget) ran out, and it runs no more JS
    /// until [`Page::set_budget`] gives it another.
    BudgetExhausted(Budget),
}

/// Why [`Page::settle`] could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettleError {
    /// [`SettleOptions::selector`] is not a valid selector.
    InvalidSelector(String),
}

impl fmt::Display for SettleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSelector(s) => write!(f, "{s:?} is not a valid selector"),
        }
    }
}

impl std::error::Error for SettleError {}

impl Page {
    /// Runs the page until every condition in `options` holds at once, the
    /// timeout runs out, or the page asks to navigate.
    ///
    /// Scripts the page inserts keep loading meanwhile. Conditions are
    /// checked whenever the event loop has run, at least every 50ms of
    /// page time. Scripts still running when the timeout passes are
    /// interrupted. So are they when the page's
    /// [work budget](crate::WorkBudget) runs out.
    pub async fn settle(&mut self, options: &SettleOptions) -> Result<SettleOutcome, SettleError> {
        let selector = options
            .selector
            .as_deref()
            .map(|s| SelectorList::parse(s).map_err(|_| SettleError::InvalidSelector(s.into())))
            .transpose()?;
        let deadline = Instant::now() + options.timeout;
        self.deadline = Some(deadline);
        self.js.set_deadline(Some(deadline));
        let outcome = self
            .wait_settled(options, selector.as_ref(), deadline)
            .await;
        self.js.set_skip_limit(None);
        self.clear_deadline();
        Ok(outcome)
    }

    async fn wait_settled(
        &mut self,
        options: &SettleOptions,
        selector: Option<&SelectorList>,
        deadline: Instant,
    ) -> SettleOutcome {
        let activity = self.session().activity().clone();
        let mut in_flight = activity.subscribe();
        let mut started = activity.requests_started();
        let start = self.js.now();
        // Page time since when the network has been within its limit.
        let mut network_quiet_since = Some(start);

        let explicit = selector.is_some() || options.predicate.is_some();
        let mut budget = options
            .page_time_budget
            .map(|b| PageTimeBudget::new(b, start, self.js.skipped()));

        loop {
            match self.check_navigation() {
                Ok(()) => {}
                Err(Stop::Limit(limit)) => return SettleOutcome::LimitExceeded(limit),
                Err(Stop::Budget(budget)) => return SettleOutcome::BudgetExhausted(budget),
                Err(_) => return SettleOutcome::NavigationRequested,
            }
            let now = self.js.now();
            // When a quiet period ends, if that is all that is missing.
            let mut wake: Option<Duration> = None;
            let mut met = true;
            // Out of page time: quiet is given up on, and only what was
            // asked for explicitly is still waited for, in real time.
            let spent = match &mut budget {
                Some(budget) => {
                    let left = budget.left(now, self.js.skipped(), activity.in_flight());
                    // The clock may skip no further than the budget goes.
                    self.js.set_skip_limit(Some(self.js.skipped() + left));
                    left.is_zero()
                }
                None => false,
            };
            let quiet = !spent;
            if spent && !explicit {
                return SettleOutcome::BudgetExhausted(Budget::PageTime);
            }

            if let (true, Some(idle)) = (quiet, options.network_idle) {
                let count = *in_flight.borrow_and_update();
                let now_started = activity.requests_started();
                // With no requests allowed, one that came and went since
                // the last look still breaks the quiet.
                let interrupted = idle.max_in_flight == 0 && now_started != started;
                started = now_started;
                if count > idle.max_in_flight {
                    network_quiet_since = None;
                } else if interrupted || network_quiet_since.is_none() {
                    network_quiet_since = Some(now);
                }
                match network_quiet_since {
                    Some(since) if now >= since + idle.quiet => {}
                    Some(since) => {
                        met = false;
                        wake = Some(since + idle.quiet);
                    }
                    None => met = false,
                }
            }
            if let (true, Some(quiet)) = (quiet, options.dom_quiet) {
                let last = self.js.dom_activity().last_mutation;
                let since = last.map_or(start, |t| t.max(start));
                if now < since + quiet {
                    met = false;
                    wake = Some(wake.map_or(since + quiet, |w| w.min(since + quiet)));
                }
            }
            // Quiet means nothing is running either: a page working through
            // a run of tasks has not settled, even if its document has not
            // changed for a while.
            let waits_for_quiet = options.network_idle.is_some() || options.dom_quiet.is_some();
            if met && quiet && waits_for_quiet && self.js.has_work_due() {
                met = false;
            }
            if let (true, Some(selector)) = (met, selector) {
                met = self
                    .js
                    .with_document(|doc| selector.query_first_deep(doc, doc.root()).is_some());
            }
            if let (true, Some(predicate)) = (met, &options.predicate) {
                met = self.js.eval_truthy(predicate).unwrap_or(false);
            }
            if met {
                let gave_up = options.network_idle.is_some() || options.dom_quiet.is_some();
                return if spent && gave_up {
                    SettleOutcome::BudgetExhausted(Budget::PageTime)
                } else {
                    SettleOutcome::Settled
                };
            }
            if Instant::now() >= deadline {
                return SettleOutcome::TimedOut;
            }

            let until = wake.map_or(now + POLL, |w| w.min(now + POLL));
            let mut wait = Wait::new(None);
            match self
                .spin(&mut wait, Until::Run(RunLimits::until(until)))
                .await
            {
                Err(Stop::Navigate) => return SettleOutcome::NavigationRequested,
                Err(Stop::Halted) => return SettleOutcome::TimedOut,
                Err(Stop::Limit(limit)) => return SettleOutcome::LimitExceeded(limit),
                Err(Stop::Budget(budget)) => return SettleOutcome::BudgetExhausted(budget),
                Ok(_) => {}
            }
            match wait.outcome {
                Some(RunOutcome::DeadlineExceeded) => return SettleOutcome::TimedOut,
                Some(RunOutcome::Interrupted) => return SettleOutcome::Interrupted,
                Some(RunOutcome::LimitExceeded(limit)) => {
                    return SettleOutcome::LimitExceeded(limit);
                }
                Some(RunOutcome::BudgetExhausted(budget)) if budget != Budget::PageTime => {
                    return SettleOutcome::BudgetExhausted(budget);
                }
                Some(RunOutcome::Idle | RunOutcome::BudgetExhausted(Budget::PageTime)) => {
                    // Nothing left to run, or nothing that can come due
                    // without skipping. Let page time pass until the next
                    // look: skip ahead unless page time follows the wall
                    // clock or may skip no further, or wait for real,
                    // waking for the network.
                    let now = self.js.now();
                    let can_skip = self.js.skip_left() != Some(Duration::ZERO);
                    if self.js.clock_mode() != ClockMode::Realtime && wake.is_some() && can_skip {
                        let _ = self.js.advance(until.saturating_sub(now));
                    } else {
                        let pause = until.saturating_sub(now).max(Duration::from_millis(1));
                        tokio::select! {
                            () = tokio::time::sleep(pause) => {}
                            _ = in_flight.changed() => {}
                            () = tokio::time::sleep_until(deadline.into()) => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Page time used up from a [`SettleOptions::page_time_budget`].
#[derive(Debug)]
struct PageTimeBudget {
    budget: Duration,
    used: Duration,
    /// Page time and time skipped at the last look.
    now: Duration,
    skipped: Duration,
}

impl PageTimeBudget {
    fn new(budget: Duration, now: Duration, skipped: Duration) -> Self {
        Self {
            budget,
            used: Duration::ZERO,
            now,
            skipped,
        }
    }

    /// Charges the page time since the last look, and returns what is
    /// left. Skipped time always counts; time that passed for real counts
    /// unless requests are in flight, which is waiting for the network.
    fn left(&mut self, now: Duration, skipped: Duration, in_flight: usize) -> Duration {
        let passed = now.saturating_sub(self.now);
        let skip = skipped.saturating_sub(self.skipped);
        let real = passed.saturating_sub(skip);
        self.used += skip;
        if in_flight == 0 {
            self.used += real;
        }
        self.now = now;
        self.skipped = skipped;
        self.budget.saturating_sub(self.used)
    }
}
