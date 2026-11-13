//! [`Job`]: what one request asks dasher to do, as JSON.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

/// How long a job may take when it does not say.
pub const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(30);
/// The quiet period of the wait strategies when a job does not say.
pub const DEFAULT_QUIET: Duration = Duration::from_millis(500);

/// A scraping job: the page to load, how to wait for it, and what to send
/// back.
///
/// Only `url` is required:
///
/// ```json
/// {
///   "url": "https://shop.example/products",
///   "wait": "settled",
///   "wait_for": ".product",
///   "timeout_ms": 20000,
///   "scroll": { "max_scrolls": 5 },
///   "extract": {
///     "names": { "selector": ".product .name", "all": true },
///     "next": { "selector": "a.next", "attribute": "href" },
///     "total": { "eval": "window.__STATE__.total" }
///   }
/// }
/// ```
///
/// Unknown fields are refused, so a misspelt option is an error rather
/// than silently ignored.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// The `http(s)` URL to load.
    pub url: String,
    /// When the page counts as loaded. Defaults to [`Wait::Settled`].
    #[serde(default)]
    pub wait: Wait,
    /// The quiet period, in milliseconds, of the wait strategies: how long
    /// the network must be idle or the DOM unchanged. Defaults to 500.
    pub quiet_ms: Option<u64>,
    /// Also waits until an element matches this CSS selector.
    pub wait_for: Option<String>,
    /// Also waits until this JavaScript expression is truthy.
    pub wait_until: Option<String>,
    /// The time limit for the whole job, in milliseconds. Defaults to
    /// 30 000; the service may cap it. When loading runs out of time the
    /// page is used as far as it got.
    pub timeout_ms: Option<u64>,
    /// The most page time, in milliseconds, the virtual clock may skip
    /// ahead while waiting for the page to go quiet. Pages that update
    /// themselves on a timer forever never do; once this is spent the
    /// quiet periods are given up and the page is used as it stands.
    /// Defaults to 5000; 0 for no limit.
    pub page_time_budget_ms: Option<u64>,
    /// The most CPU time, in milliseconds, loading, waiting and scrolling
    /// may use. Defaults to 20 000; 0 for no limit.
    pub cpu_budget_ms: Option<u64>,
    /// The most requests the page may make: the document, scripts,
    /// `fetch()` and XHR. Defaults to 1000; 0 for no limit.
    pub request_budget: Option<u64>,
    /// Scrolls the page after it has loaded to bring in infinite feeds
    /// and lazy content: `true`, or `{"max_scrolls": n}`.
    #[serde(default)]
    pub scroll: Scroll,
    /// Whether to return the page's HTML. Defaults to `true` when nothing
    /// is extracted and `false` otherwise.
    pub html: Option<bool>,
    /// Values to pull out of the page, by name. See [`Extract`].
    #[serde(default)]
    pub extract: BTreeMap<String, Extract>,
    /// Which console messages to return. Defaults to
    /// [`ConsoleFilter::Errors`].
    #[serde(default)]
    pub console: ConsoleFilter,
    /// The user agent: the `User-Agent` header and `navigator.userAgent`.
    pub user_agent: Option<String>,
    /// The window size, in CSS pixels.
    pub viewport: Option<ViewportSize>,
    /// Headers sent with every request.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Cookies set for `url` before it loads, in `Set-Cookie` syntax
    /// (`"session=abc; Path=/"`).
    #[serde(default)]
    pub cookies: Vec<String>,
    /// URL patterns to refuse, with `*` as a wildcard
    /// (`"*://*.doubleclick.net/*"`).
    #[serde(default)]
    pub block: Vec<String>,
    /// An `http://` proxy to send requests through.
    pub proxy: Option<String>,
    /// Runs timers in real time instead of skipping ahead when the page is
    /// only waiting on them.
    #[serde(default)]
    pub realtime: bool,
}

impl Job {
    /// Parses and checks a job from JSON text.
    pub fn from_slice(json: &[u8]) -> Result<Self, InvalidJob> {
        let job: Self = serde_json::from_slice(json).map_err(InvalidJob::json)?;
        job.validate()?;
        Ok(job)
    }

    /// Parses and checks a job from a JSON value.
    pub fn from_value(json: Value) -> Result<Self, InvalidJob> {
        let job: Self = serde_json::from_value(json).map_err(InvalidJob::json)?;
        job.validate()?;
        Ok(job)
    }

    fn validate(&self) -> Result<(), InvalidJob> {
        if self.url.trim().is_empty() {
            return Err(InvalidJob::new("url is empty"));
        }
        if self.timeout_ms == Some(0) {
            return Err(InvalidJob::new("timeout_ms must be positive"));
        }
        if let Some(v) = self.viewport {
            if v.width == 0 || v.height == 0 {
                return Err(InvalidJob::new(
                    "viewport width and height must be positive",
                ));
            }
        }
        if self.extract.keys().any(String::is_empty) {
            return Err(InvalidJob::new("extract names must not be empty"));
        }
        Ok(())
    }

    /// The job's time limit.
    pub fn timeout(&self) -> Duration {
        self.timeout_ms
            .map_or(DEFAULT_JOB_TIMEOUT, Duration::from_millis)
    }

    /// The quiet period of the wait strategies.
    pub fn quiet(&self) -> Duration {
        self.quiet_ms.map_or(DEFAULT_QUIET, Duration::from_millis)
    }

    /// The page-time budget for settling; `None` for none.
    pub fn page_time_budget(&self) -> Option<Duration> {
        match self.page_time_budget_ms {
            None => Some(dasher::DEFAULT_PAGE_TIME_BUDGET),
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
        }
    }

    /// The CPU and request budget for the whole page load.
    pub fn work_budget(&self) -> dasher::WorkBudget {
        let defaults = dasher::WorkBudget::default();
        dasher::WorkBudget {
            cpu: match self.cpu_budget_ms {
                None => defaults.cpu,
                Some(0) => None,
                Some(ms) => Some(Duration::from_millis(ms)),
            },
            requests: match self.request_budget {
                None => defaults.requests,
                Some(0) => None,
                Some(n) => Some(n),
            },
        }
    }

    /// Whether the page's HTML is returned.
    pub fn wants_html(&self) -> bool {
        self.html.unwrap_or(self.extract.is_empty())
    }

    /// Every CSS selector in the job, so they can be checked before the
    /// page loads.
    pub fn selectors(&self) -> impl Iterator<Item = &str> {
        let extracted = self.extract.values().filter_map(|e| match e {
            Extract::Select { selector, .. } => Some(selector.as_str()),
            Extract::Eval(_) => None,
        });
        self.wait_for.as_deref().into_iter().chain(extracted)
    }
}

/// When a page counts as loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Wait {
    /// The `load` event has fired.
    Load,
    /// No requests in flight for the quiet period.
    #[serde(alias = "networkidle", alias = "networkidle0")]
    NetworkIdle,
    /// At most two requests in flight for the quiet period.
    #[serde(alias = "networkidle2")]
    NetworkIdle2,
    /// No DOM changes for the quiet period.
    DomQuiet,
    /// Both `network-idle` and `dom-quiet` (the default).
    #[default]
    Settled,
}

/// Whether, and how far, to scroll the page after loading it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Value")]
pub enum Scroll {
    /// Don't scroll (the default).
    #[default]
    Off,
    /// Scroll until no new content arrives, at most `max_scrolls` times
    /// (dasher's default if `None`).
    On {
        /// The most scrolls to make.
        max_scrolls: Option<usize>,
    },
}

impl TryFrom<Value> for Scroll {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        const EXPECTED: &str = r#"scroll is true, false or {"max_scrolls": n}"#;
        match value {
            Value::Bool(false) | Value::Null => Ok(Self::Off),
            Value::Bool(true) => Ok(Self::On { max_scrolls: None }),
            Value::Object(map) => {
                let mut max_scrolls = None;
                for (key, value) in map {
                    match key.as_str() {
                        "max_scrolls" => {
                            let n = value.as_u64().ok_or("max_scrolls is a whole number")?;
                            max_scrolls = Some(usize::try_from(n).unwrap_or(usize::MAX));
                        }
                        _ => return Err(format!("unknown scroll option {key:?}")),
                    }
                }
                Ok(Self::On { max_scrolls })
            }
            _ => Err(EXPECTED.to_owned()),
        }
    }
}

/// A value to pull out of the page.
///
/// In JSON, either a CSS selector string, which gives the text of the
/// first match (or `null`), or an object:
///
/// - `{"selector": "h2"}`: the text of the first match, as a reader sees
///   it, or `null`.
/// - `"all": true` gives an array with one entry per match.
/// - `"attribute": "href"` gives an attribute instead of the text;
///   `"html": "inner"` or `"html": "outer"` the element's HTML; and
///   `"count": true` the number of matches.
/// - `{"eval": "js"}`: the JSON value of a JavaScript expression or
///   function, with promises awaited.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Value")]
pub enum Extract {
    /// Values read from the elements matching a selector.
    Select {
        /// The CSS selector.
        selector: String,
        /// Every match rather than the first.
        all: bool,
        /// What to read from each match.
        get: Get,
    },
    /// The value of JavaScript evaluated in the page.
    Eval(String),
}

/// What [`Extract::Select`] reads from an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Get {
    /// Its visible text.
    Text,
    /// An attribute's value.
    Attribute(String),
    /// Its `innerHTML`.
    InnerHtml,
    /// Its `outerHTML`.
    OuterHtml,
    /// How many elements match.
    Count,
}

impl TryFrom<Value> for Extract {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, String> {
        match value {
            Value::String(selector) => Ok(Self::Select {
                selector,
                all: false,
                get: Get::Text,
            }),
            Value::Object(map) => extract_from_object(map),
            _ => Err("an extract entry is a selector string or an object".to_owned()),
        }
    }
}

fn extract_from_object(mut map: Map<String, Value>) -> Result<Extract, String> {
    let string = |value: Value, what: &str| match value {
        Value::String(s) => Ok(s),
        _ => Err(format!("{what} is a string")),
    };
    let boolean = |value: Value, what: &str| match value {
        Value::Bool(b) => Ok(b),
        _ => Err(format!("{what} is true or false")),
    };

    if let Some(source) = map.remove("eval") {
        if let Some(other) = map.keys().next() {
            return Err(format!("{other:?} cannot be used with eval"));
        }
        return Ok(Extract::Eval(string(source, "eval")?));
    }
    let selector = string(
        map.remove("selector")
            .ok_or("an extract entry needs a selector or eval")?,
        "selector",
    )?;
    let mut all = false;
    let mut gets = Vec::new();
    for (key, value) in map {
        match key.as_str() {
            "all" => all = boolean(value, "all")?,
            "attribute" => gets.push(Get::Attribute(string(value, "attribute")?)),
            "html" => gets.push(match string(value, "html")?.as_str() {
                "inner" => Get::InnerHtml,
                "outer" => Get::OuterHtml,
                _ => return Err(r#"html is "inner" or "outer""#.to_owned()),
            }),
            "count" => {
                if boolean(value, "count")? {
                    gets.push(Get::Count);
                }
            }
            _ => return Err(format!("unknown extract option {key:?}")),
        }
    }
    if gets.len() > 1 {
        return Err("use only one of attribute, html and count".to_owned());
    }
    let get = gets.pop().unwrap_or(Get::Text);
    if all && get == Get::Count {
        return Err("count cannot be used with all".to_owned());
    }
    Ok(Extract::Select { selector, all, get })
}

/// Which console messages a job returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleFilter {
    /// None.
    None,
    /// `console.error` messages only (the default).
    #[default]
    Errors,
    /// Every message.
    All,
}

/// A window size, in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewportSize {
    /// The width.
    pub width: u32,
    /// The height.
    pub height: u32,
}

/// Why a job was refused before it ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidJob(String);

impl InvalidJob {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn json(e: serde_json::Error) -> Self {
        Self(format!("invalid job: {e}"))
    }
}

impl fmt::Display for InvalidJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidJob {}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn job(value: Value) -> Result<Job, String> {
        Job::from_value(value).map_err(|e| e.to_string())
    }

    #[test]
    fn a_url_alone_uses_the_defaults() {
        let job = job(json!({"url": "https://example.com/"})).unwrap();
        assert_eq!(job.wait, Wait::Settled);
        assert_eq!(job.timeout(), DEFAULT_JOB_TIMEOUT);
        assert_eq!(job.quiet(), DEFAULT_QUIET);
        assert_eq!(job.scroll, Scroll::Off);
        assert_eq!(job.console, ConsoleFilter::Errors);
        assert!(job.wants_html());
        assert!(job.extract.is_empty() && job.headers.is_empty());
        assert!(!job.realtime);
    }

    #[test]
    fn every_field_is_read() {
        let job = job(json!({
            "url": "https://example.com/",
            "wait": "networkidle2",
            "quiet_ms": 250,
            "wait_for": ".item",
            "wait_until": "window.done",
            "timeout_ms": 1500,
            "scroll": {"max_scrolls": 7},
            "html": true,
            "extract": {"title": "h1"},
            "console": "all",
            "user_agent": "bot/1",
            "viewport": {"width": 390, "height": 844},
            "headers": {"X-A": "1"},
            "cookies": ["a=1"],
            "block": ["*ads*"],
            "proxy": "http://proxy:3128",
            "realtime": true,
        }))
        .unwrap();
        assert_eq!(job.wait, Wait::NetworkIdle2);
        assert_eq!(job.quiet(), Duration::from_millis(250));
        assert_eq!(job.timeout(), Duration::from_millis(1500));
        assert_eq!(
            job.scroll,
            Scroll::On {
                max_scrolls: Some(7)
            }
        );
        assert!(job.wants_html());
        assert_eq!(job.console, ConsoleFilter::All);
        assert_eq!(
            job.viewport,
            Some(ViewportSize {
                width: 390,
                height: 844
            })
        );
        assert_eq!(job.selectors().collect::<Vec<_>>(), [".item", "h1"]);
        assert!(job.realtime);
    }

    #[test]
    fn scroll_takes_a_flag_or_options() {
        let scroll = |v| job(json!({"url": "u", "scroll": v})).map(|j| j.scroll);
        assert_eq!(scroll(json!(false)), Ok(Scroll::Off));
        assert_eq!(scroll(json!(true)), Ok(Scroll::On { max_scrolls: None }));
        assert_eq!(scroll(json!({})), Ok(Scroll::On { max_scrolls: None }));
        assert!(scroll(json!("yes")).unwrap_err().contains("scroll is true"));
        assert!(scroll(json!({"max": 1})).unwrap_err().contains("\"max\""));
    }

    #[test]
    fn extract_entries_take_several_shapes() {
        let extract = |v| Extract::try_from(v);
        let select = |selector: &str, all, get| Extract::Select {
            selector: selector.into(),
            all,
            get,
        };
        assert_eq!(extract(json!("h1")), Ok(select("h1", false, Get::Text)));
        assert_eq!(
            extract(json!({"selector": "a", "all": true, "attribute": "href"})),
            Ok(select("a", true, Get::Attribute("href".into())))
        );
        assert_eq!(
            extract(json!({"selector": "a", "html": "outer"})),
            Ok(select("a", false, Get::OuterHtml))
        );
        assert_eq!(
            extract(json!({"selector": "a", "all": true, "html": "inner"})),
            Ok(select("a", true, Get::InnerHtml))
        );
        assert_eq!(
            extract(json!({"selector": "li", "count": true})),
            Ok(select("li", false, Get::Count))
        );
        assert_eq!(
            extract(json!({"eval": "1 + 1"})),
            Ok(Extract::Eval("1 + 1".into()))
        );
    }

    #[test]
    fn mistakes_are_explained() {
        let err = |v: Value| job(v).unwrap_err();
        assert!(err(json!({})).contains("missing field `url`"));
        assert!(err(json!({"url": " "})).contains("url is empty"));
        assert!(err(json!({"url": "u", "colour": 1})).contains("unknown field `colour`"));
        assert!(err(json!({"url": "u", "wait": "soon"})).contains("unknown variant `soon`"));
        assert!(err(json!({"url": "u", "timeout_ms": 0})).contains("positive"));
        assert!(
            err(json!({"url": "u", "viewport": {"width": 0, "height": 1}})).contains("positive")
        );
        assert!(err(json!({"url": "u", "extract": {"": "h1"}})).contains("empty"));

        let extract = |v: Value| Extract::try_from(v).unwrap_err();
        assert!(extract(json!(1)).contains("selector string or an object"));
        assert!(extract(json!({})).contains("needs a selector or eval"));
        assert!(extract(json!({"eval": "x", "all": true})).contains("cannot be used with eval"));
        assert!(extract(json!({"selector": 1})).contains("selector is a string"));
        assert!(extract(json!({"selector": "a", "html": "both"})).contains("inner"));
        assert!(
            extract(json!({"selector": "a", "html": "inner", "attribute": "x"}))
                .contains("only one of")
        );
        assert!(extract(json!({"selector": "a", "all": true, "count": true})).contains("all"));
        assert!(extract(json!({"selector": "a", "get": "x"})).contains("unknown extract option"));
    }
}
