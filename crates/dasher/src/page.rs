//! [`Page`]: one tab, loading documents and pulling data out of them.

use std::fmt;
use std::time::{Duration, Instant};

use dasher_css::SelectorList;
use dasher_dom::{Document, NodeId, ShadowRoots};
use dasher_js::{
    ConsoleLevel, ConsoleMessage, Evaluation, RunLimits, RunOutcome, RuntimeOptions,
    ScriptCacheWork, ScriptError, StorageStore, WindowOptions,
};
use dasher_net::{Cookie, Session, Url};
use dasher_page::{
    AutoScrollOptions, AutoScrollReport, AutoScrollStop, Budget, PageOptions, SettleOptions,
    SettleOutcome, WorkBudget,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::browser::Browser;
use crate::error::{Error, Result};
use crate::text;

/// How often, in page time, a pending promise is checked while the page
/// runs.
const POLL: Duration = Duration::from_millis(50);

/// What the document of a page without one looks like.
const BLANK: &str = "<html><head></head><body></body></html>";

/// How [`Page::goto_with`] loads a page and decides it is ready.
///
/// The default waits for the `load` event, then settles with
/// [`SettleOptions::default`] (network idle for 500ms, within 5 seconds of
/// page time), all within the page's
/// [timeout](Page::set_timeout) and the default [`WorkBudget`], which
/// waits for the page for 5 seconds of real time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GotoOptions {
    /// What to wait for after the `load` event; `None` returns as soon as
    /// it has fired. Its own timeout bounds the wait, within
    /// [`timeout`](Self::timeout).
    pub settle: Option<SettleOptions>,
    /// Then scrolls the page to load more, the way a reader of an
    /// infinite feed would, if set.
    pub auto_scroll: Option<AutoScrollOptions>,
    /// The longest the whole navigation may take: loading, settling and
    /// scrolling. `None` uses the page's [timeout](Page::timeout). When it
    /// runs out, the page is left as far as it got.
    pub timeout: Option<Duration>,
    /// How much CPU time and how many requests the whole navigation may
    /// use, and how long the page is waited for: loading, settling and
    /// scrolling. When one runs out the page is left as it is and
    /// [`Navigation::budget_exhausted`] says which. The budget is lifted
    /// when `goto` returns, so the page can still be evaluated. Defaults
    /// to [`WorkBudget::default`]: 10 CPU seconds, 1000 requests and 5
    /// seconds of waiting. A page that takes longer than that to bring in
    /// what is wanted (a slow API behind [`SettleOptions::selector`], a
    /// long [auto-scroll](Self::auto_scroll)) needs a longer
    /// [`WorkBudget::wait`].
    pub budget: WorkBudget,
}

impl Default for GotoOptions {
    fn default() -> Self {
        Self {
            settle: Some(SettleOptions::default()),
            auto_scroll: None,
            timeout: None,
            budget: WorkBudget::default(),
        }
    }
}

impl GotoOptions {
    /// Returns once the `load` event has fired, without settling.
    pub fn load() -> Self {
        Self {
            settle: None,
            ..Self::default()
        }
    }

    /// Settles with `settle` after the `load` event.
    pub fn settle(settle: SettleOptions) -> Self {
        Self {
            settle: Some(settle),
            ..Self::default()
        }
    }

    /// Also scrolls with `options` once the page has settled.
    pub fn with_auto_scroll(mut self, options: AutoScrollOptions) -> Self {
        self.auto_scroll = Some(options);
        self
    }

    /// Bounds the whole navigation by `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Bounds the work the whole navigation does by `budget`.
    pub fn with_budget(mut self, budget: WorkBudget) -> Self {
        self.budget = budget;
        self
    }
}

/// What [`Page::goto`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Navigation {
    /// The document's URL after redirects and script navigations.
    pub url: String,
    /// The HTTP status of the document's response. Error pages load like
    /// any other, as in a browser.
    pub status: u16,
    /// Whether the time ran out before the page got as far as asked:
    /// before the `load` event, or while settling. The page holds what
    /// had loaded by then.
    pub timed_out: bool,
    /// How settling ended, if the page was settled.
    pub settle: Option<SettleOutcome>,
    /// What scrolling did, if the page was scrolled.
    pub scroll: Option<AutoScrollReport>,
    /// The budget that ended the navigation early, if one did. The page is
    /// as it stood then, and this is not a failure:
    ///
    /// - [`Budget::PageTime`]: settling skipped as much page time as
    ///   [`SettleOptions::page_time_budget`] allows without the page going
    ///   quiet (pages that update themselves on a timer forever).
    /// - [`Budget::Cpu`] or [`Budget::Requests`]: the navigation used up
    ///   [`GotoOptions::budget`], while loading, settling or scrolling.
    /// - [`Budget::Wait`]: the page had been waited for as long as
    ///   [`WorkBudget::wait`] allows before it loaded or settled. Nothing
    ///   was interrupted; what had not arrived or run by then is missing.
    ///
    /// When more than one ran out, this is the last.
    pub budget_exhausted: Option<Budget>,
    /// How many requests the navigation made: the document, scripts,
    /// `fetch()` and XHR.
    pub requests: u64,
    /// How long the navigation took, in real time.
    pub elapsed: Duration,
}

/// What [`Page::memory_usage`] reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MemoryUsage {
    /// Bytes allocated by the page's JS heap, which
    /// [`BrowserBuilder::memory_limit`](crate::BrowserBuilder::memory_limit)
    /// caps.
    pub js_heap: usize,
    /// Nodes in the page's DOM, the document node and nodes scripts hold
    /// outside the tree included.
    pub dom_nodes: usize,
}

/// A browser tab: it loads one document at a time, runs its scripts, and
/// hands back its HTML or values pulled out of it.
///
/// Pages come from [`Browser::new_page`]. Each owns its JS runtime and
/// DOM, freed when the page is dropped or [closed](Self::close) or
/// another document replaces them, or when the page goes over one of its
/// [limits](Error::LimitExceeded). Page methods must run inside a Tokio
/// runtime, on the thread that created the page: a current-thread
/// runtime, or a `LocalSet`.
///
/// While no method is running, the page is paused: its timers and
/// requests only make progress during [`goto`](Self::goto),
/// [`settle`](Self::settle), the `wait_for_*` methods,
/// [`evaluate`](Self::evaluate) of a promise and
/// [`auto_scroll`](Self::auto_scroll). Navigations the page asks for
/// (`location.href = …`) are followed by those methods too.
pub struct Page {
    // Field order is drop order: the document and runtime go first.
    inner: Option<dasher_page::Page>,
    session: Session,
    session_storage: StorageStore,
    browser: Browser,
    timeout: Duration,
}

impl fmt::Debug for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("url", &self.url())
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Page {
    pub(crate) fn new(browser: Browser, session: Session) -> Self {
        Self {
            inner: None,
            session,
            session_storage: StorageStore::new(),
            timeout: browser.timeout(),
            browser,
        }
    }

    // -----------------------------------------------------------------
    // Navigation.

    /// Loads `url` and waits for it to settle, with the default
    /// [`GotoOptions`].
    ///
    /// Fails only if no document could be had at all (a bad URL, a network
    /// error, or no response in time), or with [`Error::LimitExceeded`] if
    /// the page went over its JS heap or DOM node limit. The previous
    /// document is dropped before the new one loads, so after an error the
    /// page is blank.
    pub async fn goto(&mut self, url: &str) -> Result<Navigation> {
        self.goto_with(url, &GotoOptions::default()).await
    }

    /// Loads `url`, follows the navigations its scripts ask for, then
    /// settles and scrolls the page as `options` say.
    ///
    /// Fails only if no document could be had at all, or the page went
    /// over a [limit](Error::LimitExceeded). Running out of time is not an
    /// error: the page is left as far as it got, and
    /// [`Navigation::timed_out`] says so.
    pub async fn goto_with(&mut self, url: &str, options: &GotoOptions) -> Result<Navigation> {
        let started = Instant::now();
        let timeout = options.timeout.unwrap_or(self.timeout);
        let deadline = started + timeout;
        let requests_before = self.session.activity().requests_started();
        // Free the old document before building the new one.
        self.inner = None;

        let page_options = PageOptions {
            runtime: self.runtime_options(),
            max_navigations: self.browser.max_navigations(),
            timeout: Some(timeout),
            budget: options.budget,
            ..PageOptions::default()
        };
        let page = dasher_page::Page::load(&self.session, url, page_options).await?;
        let mut timed_out = page.timed_out();
        let mut budget_exhausted = page.budget_exhausted();
        self.inner = Some(page);

        let result = async {
            let mut settle = None;
            if let (Some(options), false, None) = (&options.settle, timed_out, budget_exhausted) {
                let outcome = self.settle_by(options, deadline).await?;
                timed_out = matches!(outcome, SettleOutcome::TimedOut);
                if let SettleOutcome::BudgetExhausted(budget) = outcome {
                    budget_exhausted = Some(budget);
                }
                settle = Some(outcome);
            }
            // Out of page time to skip, the page may still have more to
            // load on scrolling; out of CPU or requests, it may not.
            let can_scroll = matches!(budget_exhausted, None | Some(Budget::PageTime));
            let mut scroll = None;
            if let (Some(options), false, true) = (&options.auto_scroll, timed_out, can_scroll) {
                let report = self.scroll_by_deadline(options, deadline).await?;
                if let AutoScrollStop::BudgetExhausted(budget) = report.stopped {
                    budget_exhausted = Some(budget);
                }
                scroll = Some(report);
            }
            Ok::<_, Error>((settle, scroll))
        }
        .await;
        // Lift the budget, so the page can be evaluated and extracted from.
        if let Some(page) = self.inner.as_mut() {
            page.set_budget(WorkBudget::unlimited());
        }
        let (settle, scroll) = result?;
        Ok(Navigation {
            url: self.url().unwrap_or_default(),
            status: self.status().unwrap_or_default(),
            timed_out,
            settle,
            scroll,
            budget_exhausted,
            requests: self.session.activity().requests_started() - requests_before,
            elapsed: started.elapsed(),
        })
    }

    /// Runs the page until the conditions in `options` hold, within
    /// `options.timeout`, following the navigations it asks for on the
    /// way. See [`SettleOptions`].
    pub async fn settle(&mut self, options: &SettleOptions) -> Result<SettleOutcome> {
        let deadline = Instant::now() + options.timeout;
        self.settle_by(options, deadline).await
    }

    /// Runs the page until an element matches `selector`, within the
    /// page's [timeout](Self::timeout). Fails with [`Error::Timeout`] if
    /// none does in time.
    pub async fn wait_for_selector(&mut self, selector: &str) -> Result<()> {
        let options = SettleOptions {
            selector: Some(selector.to_owned()),
            timeout: self.timeout,
            ..SettleOptions::none()
        };
        self.wait_for(&options).await
    }

    /// Runs the page until the JavaScript expression `predicate` is
    /// truthy, such as `document.querySelectorAll('.item').length >= 20`,
    /// within the page's [timeout](Self::timeout). One that throws counts
    /// as false. Fails with [`Error::Timeout`] if it does not become true
    /// in time.
    pub async fn wait_for_function(&mut self, predicate: &str) -> Result<()> {
        let options = SettleOptions {
            predicate: Some(predicate.to_owned()),
            timeout: self.timeout,
            ..SettleOptions::none()
        };
        self.wait_for(&options).await
    }

    async fn wait_for(&mut self, options: &SettleOptions) -> Result<()> {
        match self.settle(options).await? {
            SettleOutcome::Settled => Ok(()),
            _ => Err(Error::Timeout),
        }
    }

    /// Settles until `deadline` at the latest, following navigations.
    async fn settle_by(
        &mut self,
        options: &SettleOptions,
        deadline: Instant,
    ) -> Result<SettleOutcome> {
        let outcome = self.settle_following(options, deadline).await;
        self.check_limits()?;
        outcome
    }

    async fn settle_following(
        &mut self,
        options: &SettleOptions,
        deadline: Instant,
    ) -> Result<SettleOutcome> {
        let page = self.inner.as_mut().ok_or(Error::NoDocument)?;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let options = SettleOptions {
                timeout: options.timeout.min(left),
                ..options.clone()
            };
            let outcome = page.settle(&options).await?;
            if outcome != SettleOutcome::NavigationRequested {
                return Ok(outcome);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, page.follow_navigation()).await {
                Err(_) => return Ok(SettleOutcome::TimedOut),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    // The old document stays; tell whoever reads its console.
                    page.js()
                        .log(ConsoleLevel::Warn, format!("Navigation failed: {e}"));
                }
            }
        }
    }

    /// Scrolls the page to the bottom again and again until that stops
    /// bringing new content or a limit in `options` is hit. See
    /// [`AutoScrollOptions`].
    pub async fn auto_scroll(&mut self, options: &AutoScrollOptions) -> Result<AutoScrollReport> {
        let page = self.inner.as_mut().ok_or(Error::NoDocument)?;
        let report = page.auto_scroll(options).await;
        self.check_limits()?;
        Ok(report?)
    }

    async fn scroll_by_deadline(
        &mut self,
        options: &AutoScrollOptions,
        deadline: Instant,
    ) -> Result<AutoScrollReport> {
        let left = deadline.saturating_duration_since(Instant::now());
        let options = AutoScrollOptions {
            timeout: Some(options.timeout.map_or(left, |t| t.min(left))),
            ..options.clone()
        };
        self.auto_scroll(&options).await
    }

    /// Scrolls the window to `(x, y)`, in CSS pixels, and lets the page
    /// handle the `scroll` event and any `IntersectionObserver` callbacks.
    ///
    /// Like each round of auto-scroll, this first reveals what the page
    /// has added since it loaded (or since the last scroll), under the
    /// default [`Visibility::Loaded`](crate::Visibility::Loaded).
    pub fn scroll_to(&mut self, x: f64, y: f64) -> Result<()> {
        self.scroll(&format!(
            "window.scrollTo({}, {})",
            js_number(x),
            js_number(y)
        ))
    }

    /// Scrolls the window by `(dx, dy)`, as [`scroll_to`](Self::scroll_to)
    /// does.
    pub fn scroll_by(&mut self, dx: f64, dy: f64) -> Result<()> {
        self.scroll(&format!(
            "window.scrollBy({}, {})",
            js_number(dx),
            js_number(dy)
        ))
    }

    fn scroll(&mut self, script: &str) -> Result<()> {
        let page = self.inner.as_ref().ok_or(Error::NoDocument)?;
        page.js().reveal();
        let result = page.js().eval_json(script);
        // Errors in the page's handlers are the page's own, and logged.
        let _ = page.js().run_ready();
        self.check_limits()?;
        result?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // JavaScript.

    /// Evaluates JavaScript in the page and returns its value as JSON.
    ///
    /// `source` is run as a classic script and its completion value
    /// taken: an expression (`document.title`), or statements ending in
    /// one. A function (`() => …`, `async () => …`) is called, and a
    /// promise is awaited, the page running meanwhile, for up to the
    /// page's [timeout](Self::timeout). Values that JSON cannot hold
    /// (`undefined`, functions) come back as `null`.
    ///
    /// Fails with [`Error::Script`] if the code throws, the promise
    /// rejects, or the value cannot be serialized (cycles, `BigInt`);
    /// with [`Error::Timeout`] if a promise does not settle in time, or
    /// cannot settle because the page has nothing left to do; with
    /// [`Error::NavigatedAway`] if the page navigates before it settles;
    /// and with [`Error::LimitExceeded`] if the page goes over its JS heap
    /// or DOM node limit meanwhile.
    pub async fn evaluate(&mut self, source: &str) -> Result<Value> {
        let deadline = Instant::now() + self.timeout;
        let page = self.inner.as_mut().ok_or(Error::NoDocument)?;
        page.js().set_deadline(Some(deadline));
        let result = evaluate_on(page, source, deadline).await;
        page.js().set_deadline(None);
        self.check_limits()?;
        Ok(match result? {
            Some(json) => serde_json::from_str(&json)?,
            None => Value::Null,
        })
    }

    /// [`evaluate`](Self::evaluate)s `source` and converts the value into
    /// `T`, such as `Vec<String>` or a `#[derive(Deserialize)]` struct.
    pub async fn evaluate_as<T: DeserializeOwned>(&mut self, source: &str) -> Result<T> {
        Ok(serde_json::from_value(self.evaluate(source).await?)?)
    }

    // -----------------------------------------------------------------
    // The document.

    /// The document serialized as HTML, doctype included, as it stands
    /// now with every change scripts made. A page with no document gives
    /// an empty one.
    ///
    /// Shadow trees, open or closed, are included as declarative shadow
    /// DOM: a `<template shadowrootmode="open">` first in each host, holding
    /// the shadow tree. A browser loading this HTML builds the same shadow
    /// trees; an HTML parser without shadow DOM sees ordinary templates.
    pub fn content(&self) -> String {
        self.inner
            .as_ref()
            .map_or_else(|| BLANK.to_owned(), dasher_page::Page::html)
    }

    /// `document.title`, or `None` if the page has no document.
    pub fn title(&self) -> Option<String> {
        self.with_document(Document::title)
    }

    /// The document's current URL, after redirects, script navigations
    /// and `history.pushState`; `None` if the page has no document.
    pub fn url(&self) -> Option<String> {
        self.inner.as_ref().map(dasher_page::Page::url)
    }

    /// The HTTP status of the document's response; `None` if the page has
    /// no document.
    pub fn status(&self) -> Option<u16> {
        self.inner.as_ref().map(|p| p.status().as_u16())
    }

    /// A response header of the document, such as `content-type`; `None`
    /// if there is no such header (or no document).
    pub fn response_header(&self, name: &str) -> Option<String> {
        let page = self.inner.as_ref()?;
        let value = page.headers().get(name)?;
        Some(String::from_utf8_lossy(value.as_bytes()).into_owned())
    }

    /// How many elements match `selector`.
    pub fn count(&self, selector: &str) -> Result<usize> {
        self.query(selector, |_, nodes| nodes.len())
    }

    /// The text of the first element matching `selector`, as a reader
    /// sees it: script and style contents left out, whitespace collapsed
    /// and trimmed, and block elements and `<br>` kept apart by a space.
    /// `None` if nothing matches.
    pub fn text(&self, selector: &str) -> Result<Option<String>> {
        self.query_first(selector, text::visible_text)
    }

    /// The [text](Self::text) of every element matching `selector`, in
    /// document order.
    pub fn texts(&self, selector: &str) -> Result<Vec<String>> {
        self.query_each(selector, text::visible_text)
    }

    /// The value of attribute `name` on the first element matching
    /// `selector`. `None` if nothing matches or it has no such attribute.
    pub fn attribute(&self, selector: &str, name: &str) -> Result<Option<String>> {
        Ok(self
            .query_first(selector, |doc, node| {
                doc.attribute(node, name).map(str::to_owned)
            })?
            .flatten())
    }

    /// The values of attribute `name` on the elements matching
    /// `selector` that have it, in document order: say, every link's
    /// `href` with `attributes("a", "href")`. Values are as written in the
    /// markup; resolve relative URLs against [`url`](Self::url).
    pub fn attributes(&self, selector: &str, name: &str) -> Result<Vec<String>> {
        Ok(self
            .query_each(selector, |doc, node| {
                doc.attribute(node, name).map(str::to_owned)
            })?
            .into_iter()
            .flatten()
            .collect())
    }

    /// The `innerHTML` of the first element matching `selector`, with
    /// shadow trees included as in [`content`](Self::content).
    pub fn inner_html(&self, selector: &str) -> Result<Option<String>> {
        self.query_first(selector, inner_html)
    }

    /// The `outerHTML` of the first element matching `selector`, with
    /// shadow trees included as in [`content`](Self::content).
    pub fn outer_html(&self, selector: &str) -> Result<Option<String>> {
        self.query_first(selector, outer_html)
    }

    /// The [`inner_html`](Self::inner_html) of every element matching
    /// `selector`, in document order.
    pub fn inner_htmls(&self, selector: &str) -> Result<Vec<String>> {
        self.query_each(selector, inner_html)
    }

    /// The [`outer_html`](Self::outer_html) of every element matching
    /// `selector`, in document order.
    pub fn outer_htmls(&self, selector: &str) -> Result<Vec<String>> {
        self.query_each(selector, outer_html)
    }

    fn query_first<R>(
        &self,
        selector: &str,
        f: impl Fn(&Document, NodeId) -> R,
    ) -> Result<Option<R>> {
        self.query(selector, |doc, nodes| nodes.first().map(|&n| f(doc, n)))
    }

    fn query_each<R>(&self, selector: &str, f: impl Fn(&Document, NodeId) -> R) -> Result<Vec<R>> {
        self.query(selector, |doc, nodes| {
            nodes.iter().map(|&n| f(doc, n)).collect()
        })
    }

    /// Runs `f` over the elements matching `selector`, in document order,
    /// looking into shadow trees too (see [`SelectorList::query_all_deep`]).
    /// A page without a document has none.
    fn query<R>(&self, selector: &str, f: impl FnOnce(&Document, &[NodeId]) -> R) -> Result<R> {
        let list = SelectorList::parse(selector)
            .map_err(|_| Error::InvalidSelector(selector.to_owned()))?;
        let query = |doc: &Document| {
            let nodes: Vec<NodeId> = list.query_all_deep(doc, doc.root()).collect();
            f(doc, &nodes)
        };
        Ok(match &self.inner {
            Some(page) => page.js().with_document(query),
            None => query(&Document::parse(BLANK)),
        })
    }

    fn with_document<R>(&self, f: impl FnOnce(&Document) -> R) -> Option<R> {
        self.inner.as_ref().map(|p| p.js().with_document(f))
    }

    // -----------------------------------------------------------------
    // Cookies.

    /// The cookies the page's next request to its own URL would send,
    /// `HttpOnly` ones included. Empty if the page has no document.
    pub fn cookies(&self) -> Vec<Cookie> {
        self.url()
            .and_then(|url| Url::parse(&url).ok())
            .map(|url| self.session.cookies().cookies_for(&url))
            .unwrap_or_default()
    }

    /// Stores a cookie for the page's URL, written as a `Set-Cookie`
    /// header value (`"theme=dark; Path=/"`). Cookies are shared by every
    /// page of the [`Browser`]; to set one before loading a page, use
    /// [`Browser::set_cookie`].
    pub fn set_cookie(&self, cookie: &str) -> Result<()> {
        let url = self.url().ok_or(Error::NoDocument)?;
        self.browser.set_cookie(&url, cookie)
    }

    // -----------------------------------------------------------------
    // Diagnostics.

    /// What the current document's scripts wrote to the `console`,
    /// oldest first. Each document starts a new log; the oldest messages
    /// are dropped past 1000.
    pub fn console(&self) -> Vec<ConsoleMessage> {
        self.inner
            .as_ref()
            .map(|p| p.js().console())
            .unwrap_or_default()
    }

    /// The uncaught exceptions and unhandled promise rejections of the
    /// current document's scripts, oldest first. Errors in code run by
    /// [`evaluate`](Self::evaluate) are returned from it instead.
    pub fn errors(&self) -> Vec<ScriptError> {
        self.inner
            .as_ref()
            .map(|p| p.js().errors())
            .unwrap_or_default()
    }

    /// How much memory the current document holds: its JS heap and its
    /// DOM. A page with no document uses none.
    pub fn memory_usage(&self) -> MemoryUsage {
        self.inner.as_ref().map_or_else(MemoryUsage::default, |p| {
            let js = p.js();
            MemoryUsage {
                js_heap: js.memory_allocated(),
                dom_nodes: js.with_document(Document::len),
            }
        })
    }

    /// Settles the page's account with the browser's
    /// [`ScriptCache`](crate::ScriptCache) now rather than when it is
    /// dropped, and returns the cache entries left to make again.
    ///
    /// A page notes which functions of its cached scripts ran, and when it
    /// is dropped, compiles again the scripts whose entries lacked them. A
    /// process about to exit, which leaves the page for the OS to free
    /// rather than freeing it, calls this first and runs what it returns:
    /// usually nothing, once the cache has seen the page's scripts twice.
    pub fn take_script_cache_work(&self) -> ScriptCacheWork {
        self.inner
            .as_ref()
            .map(|p| p.js().take_script_cache_work())
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------
    // Settings and lifetime.

    /// The time limit for [`goto`](Self::goto), the `wait_for_*` methods
    /// and awaiting [`evaluate`](Self::evaluate)d promises.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Changes the page's [timeout](Self::timeout).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// The browser the page belongs to.
    pub fn browser(&self) -> &Browser {
        &self.browser
    }

    /// Closes the page, freeing its document and JS runtime and canceling
    /// its requests. Dropping the page does the same.
    pub fn close(self) {}

    /// Fails with [`Error::LimitExceeded`] if the page went over one of its
    /// limits, freeing the document so a runaway page lets go of its memory
    /// at once.
    fn check_limits(&mut self) -> Result<()> {
        match self.inner.as_ref().and_then(|p| p.js().limit_exceeded()) {
            Some(limit) => {
                self.inner = None;
                Err(Error::LimitExceeded(limit))
            }
            None => Ok(()),
        }
    }

    fn runtime_options(&self) -> RuntimeOptions {
        let base = self.browser.runtime_options();
        RuntimeOptions {
            window: WindowOptions {
                session_storage: self.session_storage.clone(),
                ..base.window.clone()
            },
            ..base.clone()
        }
    }
}

/// `n` as a JavaScript number literal.
fn js_number(n: f64) -> String {
    if n.is_finite() {
        n.to_string()
    } else if n.is_nan() {
        "NaN".to_owned()
    } else if n > 0.0 {
        "Infinity".to_owned()
    } else {
        "-Infinity".to_owned()
    }
}

/// Evaluates `source` on `page`, running the page until a promise it
/// gives settles.
async fn evaluate_on(
    page: &mut dasher_page::Page,
    source: &str,
    deadline: Instant,
) -> Result<Option<String>> {
    let pending = match page.js().evaluate(source)? {
        Evaluation::Ready(value) => return Ok(value),
        Evaluation::Pending(pending) => pending,
    };
    loop {
        if let Some(outcome) = pending.poll() {
            return Ok(outcome?);
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let until = page.js().now() + POLL;
        match page.run(RunLimits::until(until)).await {
            RunOutcome::ReachedTime | RunOutcome::TaskLimit => {}
            RunOutcome::NavigationRequested => {
                return pending.poll().map_or(Err(Error::NavigatedAway), |o| Ok(o?));
            }
            // Nothing left that could settle it, or out of time.
            RunOutcome::Idle
            | RunOutcome::DeadlineExceeded
            | RunOutcome::Interrupted
            | RunOutcome::LimitExceeded(_)
            | RunOutcome::BudgetExhausted(_) => {
                return pending.poll().map_or(Err(Error::Timeout), |o| Ok(o?));
            }
        }
    }
}

fn inner_html(doc: &Document, node: NodeId) -> String {
    doc.inner_html_with(node, ShadowRoots::All)
}

fn outer_html(doc: &Document, node: NodeId) -> String {
    doc.outer_html_with(node, ShadowRoots::All)
}
