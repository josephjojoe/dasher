//! Running one [`Job`] in a page of its own.

use std::time::{Duration, Instant};

use dasher::{
    AutoScrollOptions, Browser, ClockMode, ConsoleLevel, Error, GotoOptions, JsonValue, LoadError,
    NetworkIdle, Page, SettleOptions,
};
use serde_json::{Map, json};

use crate::job::{ConsoleFilter, Extract, Get, Job, Scroll, Wait};
use crate::report;

/// URL patterns every job refuses: the cloud metadata endpoints, which
/// hand out the instance's credentials.
///
/// These match URLs only, not what a host name resolves to. The check
/// that does, [`RunOptions::block_private_addresses`], covers these
/// addresses and more; the patterns stay as a second line, and for when
/// that check is turned off.
pub const METADATA_BLOCKS: &[&str] = &[
    // AWS, GCP and Azure instance metadata, and ECS task metadata.
    "*://169.254.*",
    // AWS instance metadata over IPv6.
    "*://[fd00:ec2::254]*",
    // GCP's names for its metadata server.
    "*://metadata.google.internal*",
    "*://metadata/*",
    "*://metadata./*",
    "*://metadata:*",
];

/// Time kept back from a hard deadline for collecting and sending the
/// result.
const DEADLINE_RESERVE: Duration = Duration::from_millis(1500);
/// The least time a promise from an `eval` extraction gets, even when
/// loading used up the job's time.
const MIN_EVAL_TIME: Duration = Duration::from_secs(1);

/// The limits a job runs under, set by whoever runs it.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The longest any job may take; longer `timeout_ms` are cut to it.
    pub max_timeout: Duration,
    /// When the result must be ready, such as a Lambda invocation's
    /// deadline. Jobs are cut short to finish before it.
    pub deadline: Option<Instant>,
    /// URL patterns refused on top of the job's own `block` list.
    pub blocked: Vec<String>,
    /// Whether pages may not connect to loopback, link-local, private,
    /// CGNAT and other non-public addresses, checked after DNS on every
    /// connection (see
    /// [`BrowserBuilder::block_private_addresses`](dasher::BrowserBuilder::block_private_addresses)).
    /// On by default; turn it off only to scrape servers on a private
    /// network or localhost, such as test fixtures.
    pub block_private_addresses: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(300),
            deadline: None,
            blocked: METADATA_BLOCKS.iter().map(|&s| s.to_owned()).collect(),
            block_private_addresses: true,
        }
    }
}

/// What a job produced: an HTTP status for the result and the JSON body.
///
/// Status 200 means the page loaded (whatever status the site answered
/// with; that is the body's `status`). Otherwise the body has an `error`
/// message: 400 for a job that cannot run as given, 422 when the page
/// went over its JS heap or DOM node limit, 502 when the page could not
/// be fetched, 503 when the service is too busy or shutting down, 504 when
/// no response came in time.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The HTTP status.
    pub status: u16,
    /// The JSON body.
    pub body: JsonValue,
}

impl Outcome {
    /// A failure with `status` and an `error` message.
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            body: json!({ "error": message.into() }),
        }
    }

    fn with(mut self, key: &str, value: JsonValue) -> Self {
        if let JsonValue::Object(map) = &mut self.body {
            map.insert(key.to_owned(), value);
        }
        self
    }
}

/// Runs `job` in a fresh browser and page, which are dropped before this
/// returns, so nothing (cookies, storage, connections) carries over to
/// another job.
///
/// `queued` is how long the job waited for a slot; it is reported in the
/// timings. Must run on a current-thread Tokio runtime or in a
/// `LocalSet`, as pages do.
pub async fn run(job: &Job, options: &RunOptions, queued: Duration) -> Outcome {
    let started = Instant::now();
    let outcome = run_page(job, options, started).await;
    let timings = |extra: Map<String, JsonValue>| {
        let mut timings = extra;
        timings.insert("queued_ms".into(), millis(queued).into());
        timings.insert("total_ms".into(), millis(queued + started.elapsed()).into());
        JsonValue::Object(timings)
    };
    match outcome {
        Ok((outcome, phases)) => outcome.with("timings", timings(phases)),
        Err(outcome) => outcome
            .with("url", job.url.clone().into())
            .with("timings", timings(Map::new())),
    }
}

/// The job's work, giving the result and the timings of its phases.
async fn run_page(
    job: &Job,
    options: &RunOptions,
    started: Instant,
) -> Result<(Outcome, Map<String, JsonValue>), Outcome> {
    let mut timeout = job.timeout().min(options.max_timeout);
    if let Some(deadline) = options.deadline {
        let left = deadline
            .saturating_duration_since(started)
            .saturating_sub(DEADLINE_RESERVE);
        if left.is_zero() {
            return Err(Outcome::error(504, "no time left to run the job"));
        }
        timeout = timeout.min(left);
    }
    let deadline = started + timeout;

    let browser = browser(job, options).map_err(|e| failure(&e))?;
    for cookie in &job.cookies {
        browser
            .set_cookie(&job.url, cookie)
            .map_err(|e| failure(&e))?;
    }
    let mut page = browser.new_page();
    // Refuse bad selectors before spending time on the page.
    for selector in job.selectors() {
        page.count(selector).map_err(|e| failure(&e))?;
    }
    page.set_timeout(timeout);
    let navigation = page
        .goto_with(&job.url, &goto_options(job, timeout))
        .await
        .map_err(|e| failure(&e))?;

    let extract_started = Instant::now();
    let mut extracted = Map::new();
    let mut extract_errors = Map::new();
    for (name, spec) in &job.extract {
        let value = match extract(&mut page, spec, deadline).await {
            Ok(value) => value,
            Err(e) => {
                extract_errors.insert(name.clone(), report::error_message(&e).into());
                JsonValue::Null
            }
        };
        extracted.insert(name.clone(), value);
    }
    let extract_time = extract_started.elapsed();

    let mut body = Map::new();
    body.insert("url".into(), navigation.url.clone().into());
    body.insert("status".into(), navigation.status.into());
    body.insert("title".into(), page.title().into());
    body.insert("timed_out".into(), navigation.timed_out.into());
    body.insert(
        "settle".into(),
        navigation.settle.map(report::settle_name).into(),
    );
    body.insert(
        "scroll".into(),
        navigation.scroll.as_ref().map(report::scroll_json).into(),
    );
    body.insert(
        "budget_exhausted".into(),
        navigation.budget_exhausted.map(|b| b.name()).into(),
    );
    body.insert("requests".into(), navigation.requests.into());
    if job.wants_html() {
        body.insert("html".into(), page.content().into());
    }
    if !job.extract.is_empty() {
        body.insert("extracted".into(), extracted.into());
    }
    if !extract_errors.is_empty() {
        body.insert("extract_errors".into(), extract_errors.into());
    }
    let console: Vec<JsonValue> = page
        .console()
        .iter()
        .filter(|m| match job.console {
            ConsoleFilter::None => false,
            ConsoleFilter::Errors => m.level == ConsoleLevel::Error,
            ConsoleFilter::All => true,
        })
        .map(report::console_json)
        .collect();
    body.insert("console".into(), console.into());
    let errors: Vec<JsonValue> = page.errors().iter().map(report::error_json).collect();
    body.insert("errors".into(), errors.into());
    // The page, its runtime and its document go here.
    drop(page);

    let mut timings = Map::new();
    timings.insert("navigation_ms".into(), millis(navigation.elapsed).into());
    timings.insert("extract_ms".into(), millis(extract_time).into());
    Ok((
        Outcome {
            status: 200,
            body: body.into(),
        },
        timings,
    ))
}

fn browser(job: &Job, options: &RunOptions) -> dasher::Result<Browser> {
    let mut builder = Browser::builder().block_private_addresses(options.block_private_addresses);
    if let Some(user_agent) = &job.user_agent {
        builder = builder.user_agent(user_agent);
    }
    if let Some(viewport) = job.viewport {
        builder = builder.viewport(viewport.width, viewport.height);
    }
    for (name, value) in &job.headers {
        builder = builder.header(name, value);
    }
    if let Some(proxy) = &job.proxy {
        builder = builder.proxy(proxy);
    }
    for pattern in options.blocked.iter().chain(&job.block) {
        builder = builder.block(pattern);
    }
    if job.realtime {
        builder = builder.clock(ClockMode::Realtime);
    }
    builder.build()
}

fn goto_options(job: &Job, timeout: Duration) -> GotoOptions {
    let quiet = job.quiet();
    let idle = |max_in_flight| {
        Some(NetworkIdle {
            max_in_flight,
            quiet,
        })
    };
    let (network_idle, dom_quiet) = match job.wait {
        Wait::Load => (None, None),
        Wait::NetworkIdle => (idle(0), None),
        Wait::NetworkIdle2 => (idle(2), None),
        Wait::DomQuiet => (None, Some(quiet)),
        Wait::Settled => (idle(0), Some(quiet)),
    };
    let settle = SettleOptions {
        network_idle,
        dom_quiet,
        selector: job.wait_for.clone(),
        predicate: job.wait_until.clone(),
        timeout,
        page_time_budget: job.page_time_budget(),
    };
    let waits = settle.network_idle.is_some()
        || settle.dom_quiet.is_some()
        || settle.selector.is_some()
        || settle.predicate.is_some();
    let mut goto = GotoOptions {
        settle: waits.then_some(settle),
        auto_scroll: None,
        timeout: Some(timeout),
        budget: job.work_budget(),
    };
    if let Scroll::On { max_scrolls } = job.scroll {
        let mut scroll = AutoScrollOptions::default();
        if let Some(max) = max_scrolls {
            scroll.max_scrolls = max;
        }
        if job.page_time_budget_ms.is_some() {
            scroll.settle.page_time_budget = job.page_time_budget();
        }
        goto = goto.with_auto_scroll(scroll);
    }
    goto
}

async fn extract(page: &mut Page, spec: &Extract, deadline: Instant) -> dasher::Result<JsonValue> {
    let (selector, all, get) = match spec {
        Extract::Eval(source) => {
            let left = deadline.saturating_duration_since(Instant::now());
            page.set_timeout(left.max(MIN_EVAL_TIME));
            return page.evaluate(source).await;
        }
        Extract::Select { selector, all, get } => (selector.as_str(), *all, get),
    };
    Ok(match (get, all) {
        (Get::Count, _) => page.count(selector)?.into(),
        (Get::Text, false) => page.text(selector)?.into(),
        (Get::Text, true) => page.texts(selector)?.into(),
        (Get::Attribute(name), false) => page.attribute(selector, name)?.into(),
        (Get::Attribute(name), true) => page.attributes(selector, name)?.into(),
        (Get::InnerHtml, false) => page.inner_html(selector)?.into(),
        (Get::InnerHtml, true) => page.inner_htmls(selector)?.into(),
        (Get::OuterHtml, false) => page.outer_html(selector)?.into(),
        (Get::OuterHtml, true) => page.outer_htmls(selector)?.into(),
    })
}

/// The outcome for a job that could not produce a page.
fn failure(error: &Error) -> Outcome {
    let status = match error {
        Error::Config(_)
        | Error::InvalidCookie(_)
        | Error::InvalidSelector(_)
        | Error::Load(LoadError::InvalidUrl(_)) => 400,
        Error::Load(LoadError::Network(_)) => 502,
        Error::Load(LoadError::Timeout) | Error::Timeout => 504,
        // The page's fault, not the service's.
        Error::LimitExceeded(_) => 422,
        _ => 500,
    };
    Outcome::error(status, report::error_message(error))
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use dasher::ResourceLimit;

    use super::*;

    #[test]
    fn pages_over_their_limits_are_unprocessable() {
        let outcome = failure(&Error::LimitExceeded(ResourceLimit::DomNodes { limit: 10 }));
        assert_eq!(outcome.status, 422);
        assert_eq!(
            outcome.body["error"],
            "the page went over its limit of 10 DOM nodes"
        );
    }
}
