//! Scrolling a page to make it load more: [`Page::auto_scroll`].

use std::time::{Duration, Instant};

use dasher_dom::{Document, NodeKind};

use dasher_js::Budget;

use crate::Page;
use crate::budget::DEFAULT_PAGE_TIME_BUDGET;
use crate::settle::{NetworkIdle, SettleError, SettleOptions, SettleOutcome};

/// Default for [`AutoScrollOptions::max_scrolls`].
pub const DEFAULT_MAX_SCROLLS: usize = 20;

/// Scrolls one viewport down, no further than the bottom of the document.
/// Evaluates to whether there is further to go.
const STEP_DOWN: &str = "(() => {
  const root = document.scrollingElement || document.documentElement;
  const bottom = (root ? root.scrollHeight : 0) - window.innerHeight;
  if (window.scrollY >= bottom) return false;
  window.scrollTo(window.scrollX, Math.min(bottom, window.scrollY + window.innerHeight));
  return window.scrollY < bottom;
})()";

/// The most viewport-sized steps one round takes on its way to the bottom.
const MAX_STEPS: usize = 500;

/// Scrolls to the bottom of the document. If the page is already there (a
/// scroll that brought no new height), it backs up a pixel first, so the
/// position still changes and the page still gets a `scroll` event.
const SCROLL_TO_BOTTOM: &str = "(() => {
  const root = document.scrollingElement || document.documentElement;
  const bottom = root ? root.scrollHeight : 0;
  const y = window.scrollY;
  window.scrollTo(window.scrollX, bottom);
  if (window.scrollY === y) {
    window.scrollBy(0, -1);
    window.scrollTo(window.scrollX, bottom);
  }
})()";

/// How [`Page::auto_scroll`] scrolls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoScrollOptions {
    /// The most times to scroll.
    pub max_scrolls: usize,
    /// Stop once this many scrolls in a row have brought no new content.
    pub idle_scrolls: usize,
    /// What to wait for after each scroll before looking for new content.
    /// Its timeout bounds each wait.
    pub settle: SettleOptions,
    /// The longest the whole routine may take, in real time.
    pub timeout: Option<Duration>,
}

impl Default for AutoScrollOptions {
    /// Up to [`DEFAULT_MAX_SCROLLS`] scrolls, stopping after two that bring
    /// nothing new; after each, waits for 300ms of network idle and DOM
    /// quiet, for at most 5s; 60s in all.
    fn default() -> Self {
        let quiet = Duration::from_millis(300);
        Self {
            max_scrolls: DEFAULT_MAX_SCROLLS,
            idle_scrolls: 2,
            settle: SettleOptions {
                network_idle: Some(NetworkIdle {
                    max_in_flight: 0,
                    quiet,
                }),
                dom_quiet: Some(quiet),
                selector: None,
                predicate: None,
                timeout: Duration::from_secs(5),
                page_time_budget: Some(DEFAULT_PAGE_TIME_BUDGET),
            },
            timeout: Some(Duration::from_secs(60)),
        }
    }
}

/// Why [`Page::auto_scroll`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoScrollStop {
    /// [`AutoScrollOptions::idle_scrolls`] scrolls in a row brought nothing
    /// new: the page has run out of content.
    NoNewContent,
    /// [`AutoScrollOptions::max_scrolls`] was reached.
    ScrollLimit,
    /// [`AutoScrollOptions::timeout`] ran out.
    TimedOut,
    /// The page asked to load another document. Call
    /// [`Page::follow_navigation`] to
    /// follow it.
    NavigationRequested,
    /// An [`InterruptHandle`](dasher_js::InterruptHandle) stopped the page.
    Interrupted,
    /// The page went over a resource limit and can run no more.
    LimitExceeded(dasher_js::ResourceLimit),
    /// The page's [work budget](crate::WorkBudget) ran out:
    /// [`Budget::Cpu`] or [`Budget::Requests`]. (A round whose settle
    /// spends its page-time budget just ends, and scrolling goes on.)
    BudgetExhausted(Budget),
}

/// What [`Page::auto_scroll`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AutoScrollReport {
    /// How many times it scrolled.
    pub scrolls: usize,
    /// How many of those brought new content.
    pub productive_scrolls: usize,
    /// Why it stopped.
    pub stopped: AutoScrollStop,
}

impl Page {
    /// Scrolls to the bottom of the page again and again, the way a reader
    /// of an infinite feed would, until scrolling stops bringing new
    /// content or a limit is hit.
    ///
    /// Each round scrolls the window down to the bottom of the document a
    /// viewport at a time, letting the page handle each step (`scroll`
    /// events, `IntersectionObserver` callbacks; see
    /// [`Visibility`](dasher_js::Visibility)), then
    /// [settles](Self::settle) with [`AutoScrollOptions::settle`]. A round
    /// brought new content if the elements or text in the document changed.
    ///
    /// Fails only if the settle options' selector is invalid.
    pub async fn auto_scroll(
        &mut self,
        options: &AutoScrollOptions,
    ) -> Result<AutoScrollReport, SettleError> {
        let deadline = options.timeout.map(|t| Instant::now() + t);
        let mut report = AutoScrollReport {
            scrolls: 0,
            productive_scrolls: 0,
            stopped: AutoScrollStop::ScrollLimit,
        };
        let mut idle = 0;
        let mut before = self.js.with_document(content_signature);
        while report.scrolls < options.max_scrolls {
            let mut settle = options.settle.clone();
            if let Some(deadline) = deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    report.stopped = AutoScrollStop::TimedOut;
                    return Ok(report);
                }
                settle.timeout = settle.timeout.min(left);
            }
            self.scroll_down();
            report.scrolls += 1;
            match self.settle(&settle).await? {
                SettleOutcome::Settled
                | SettleOutcome::TimedOut
                | SettleOutcome::BudgetExhausted(Budget::PageTime) => {}
                SettleOutcome::BudgetExhausted(budget) => {
                    report.stopped = AutoScrollStop::BudgetExhausted(budget);
                    return Ok(report);
                }
                SettleOutcome::NavigationRequested => {
                    report.stopped = AutoScrollStop::NavigationRequested;
                    return Ok(report);
                }
                SettleOutcome::Interrupted => {
                    report.stopped = AutoScrollStop::Interrupted;
                    return Ok(report);
                }
                SettleOutcome::LimitExceeded(limit) => {
                    report.stopped = AutoScrollStop::LimitExceeded(limit);
                    return Ok(report);
                }
            }
            let after = self.js.with_document(content_signature);
            if after == before {
                idle += 1;
                if idle >= options.idle_scrolls.max(1) {
                    report.stopped = AutoScrollStop::NoNewContent;
                    return Ok(report);
                }
            } else {
                idle = 0;
                report.productive_scrolls += 1;
                before = after;
            }
        }
        Ok(report)
    }
}

impl Page {
    /// Scrolls to the bottom a viewport at a time, letting the page react
    /// to each step, so whatever watches for elements coming into view
    /// sees every one pass, then scrolls to the very bottom.
    fn scroll_down(&self) {
        // Scrolling is what brings appended content into view under the
        // default visibility.
        self.js.reveal();
        for _ in 0..MAX_STEPS {
            let more = self.js.eval_truthy(STEP_DOWN).unwrap_or(false);
            // Scroll listeners and IntersectionObserver callbacks; errors are
            // the page's own and already reported to it.
            let _ = self.js.run_ready();
            if !more {
                break;
            }
        }
        let _ = self.js.eval_json(SCROLL_TO_BOTTOM);
    }
}

/// A cheap fingerprint of the document's content: how many elements it
/// has and how much text. New items change it; a clock ticking in place
/// changes neither count, as long as its width stays the same.
fn content_signature(doc: &Document) -> (usize, usize) {
    let mut elements = 0;
    let mut text = 0;
    for node in doc.descendants(doc.root()) {
        match doc.kind(node) {
            NodeKind::Element => elements += 1,
            NodeKind::Text => text += doc.text(node).map_or(0, str::len),
            _ => {}
        }
    }
    (elements, text)
}
