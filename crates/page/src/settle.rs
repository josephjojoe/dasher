//! Deciding when a page is done: [`Page::settle`].

use std::fmt;
use std::time::{Duration, Instant};

use dasher_css::SelectorList;
use dasher_js::{ClockMode, RunLimits, RunOutcome};

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
/// busy.
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
}

impl Default for SettleOptions {
    /// Network idle and DOM quiet, each for [`DEFAULT_QUIET`], within
    /// [`DEFAULT_SETTLE_TIMEOUT`].
    fn default() -> Self {
        Self {
            network_idle: Some(NetworkIdle::default()),
            dom_quiet: Some(DEFAULT_QUIET),
            selector: None,
            predicate: None,
            timeout: DEFAULT_SETTLE_TIMEOUT,
        }
    }
}

impl SettleOptions {
    /// No conditions, within [`DEFAULT_SETTLE_TIMEOUT`]. Set the ones
    /// wanted.
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
    /// interrupted.
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

        loop {
            if self.check_navigation().is_err() {
                return SettleOutcome::NavigationRequested;
            }
            let now = self.js.now();
            // When a quiet period ends, if that is all that is missing.
            let mut wake: Option<Duration> = None;
            let mut met = true;

            if let Some(idle) = options.network_idle {
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
            if let Some(quiet) = options.dom_quiet {
                let last = self.js.dom_activity().last_mutation;
                let since = last.map_or(start, |t| t.max(start));
                if now < since + quiet {
                    met = false;
                    wake = Some(wake.map_or(since + quiet, |w| w.min(since + quiet)));
                }
            }
            if let (true, Some(selector)) = (met, selector) {
                met = self
                    .js
                    .with_document(|doc| selector.query_first(doc, doc.root()).is_some());
            }
            if let (true, Some(predicate)) = (met, &options.predicate) {
                met = self.js.eval_truthy(predicate).unwrap_or(false);
            }
            if met {
                return SettleOutcome::Settled;
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
                Ok(_) => {}
            }
            match wait.outcome {
                Some(RunOutcome::DeadlineExceeded) => return SettleOutcome::TimedOut,
                Some(RunOutcome::Interrupted) => return SettleOutcome::Interrupted,
                Some(RunOutcome::Idle) => {
                    // Nothing left to run. Let page time pass until the next
                    // look: skip ahead unless page time follows the wall
                    // clock, or wait for real, waking for the network.
                    let now = self.js.now();
                    if self.js.clock_mode() != ClockMode::Realtime && wake.is_some() {
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
