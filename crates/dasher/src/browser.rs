//! [`Browser`]: configuration and state shared by pages.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use dasher_js::{
    ClockMode, RuntimeOptions, ScriptCache, ScriptCacheOptions, StorageStore, Viewport, Visibility,
    WindowOptions,
};
use dasher_net::header::{HeaderName, HeaderValue};
use dasher_net::{Client, Cookie, CookieJar, HeaderMap, Proxy, RequestPolicy, Url, header};

use crate::error::{Error, Result};
use crate::page::Page;

/// Default for [`BrowserBuilder::timeout`].
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A headless browser: the configuration, connection pool, cookie jar and
/// `localStorage` its pages share.
///
/// A browser is cheap to clone (clones share everything) and can be sent
/// between threads; build one per process and open pages from it. The
/// [`Page`]s themselves stay on the thread that made them.
///
/// ```no_run
/// # async fn example() -> dasher::Result<()> {
/// let browser = dasher::Browser::builder()
///     .user_agent("my-scraper/1.0")
///     .block("*://*.doubleclick.net/*")
///     .build()?;
/// let mut page = browser.new_page();
/// page.goto("https://example.com/").await?;
/// println!("{}", page.content());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Browser {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    cookies: Arc<CookieJar>,
    /// Settings every page's JS runtime starts from. Its local storage is
    /// the browser's, shared by all pages.
    runtime: RuntimeOptions,
    max_navigations: usize,
    timeout: Duration,
}

impl fmt::Debug for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Browser")
            .field("user_agent", &self.inner.runtime.window.user_agent)
            .field("viewport", &self.inner.runtime.window.viewport)
            .field("cookies", &self.inner.cookies.len())
            .finish_non_exhaustive()
    }
}

impl Browser {
    /// A browser with the default configuration.
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Starts configuring a browser.
    pub fn builder() -> BrowserBuilder {
        BrowserBuilder::default()
    }

    /// Opens a blank page. Load a document into it with
    /// [`Page::goto`].
    ///
    /// Each page has its own JS runtime, document and `sessionStorage`,
    /// and shares the browser's connections, cookies and `localStorage`.
    pub fn new_page(&self) -> Page {
        let session = self
            .inner
            .client
            .session()
            .with_cookie_jar(Arc::clone(&self.inner.cookies));
        Page::new(self.clone(), session)
    }

    /// Opens a page and loads `url` into it with the default
    /// [`GotoOptions`](crate::GotoOptions): a shorthand for
    /// [`new_page`](Self::new_page) then [`Page::goto`].
    pub async fn open(&self, url: &str) -> Result<Page> {
        let mut page = self.new_page();
        page.goto(url).await?;
        Ok(page)
    }

    /// Every cookie in the browser's jar, for all sites.
    pub fn cookies(&self) -> Vec<Cookie> {
        self.inner.cookies.cookies()
    }

    /// Stores a cookie as if `url` had answered with
    /// `Set-Cookie: <cookie>`, e.g.
    /// `set_cookie("https://example.com/", "session=abc; Path=/")`.
    ///
    /// Fails if `url` is not an `http(s)` URL or the cookie is refused
    /// (a malformed cookie, or a `Domain` that does not match `url`).
    pub fn set_cookie(&self, url: &str, cookie: &str) -> Result<()> {
        let parsed = Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .ok_or_else(|| Error::InvalidCookie(format!("{url:?} is not an http(s) URL")))?;
        if self.inner.cookies.set_from_header(&parsed, cookie) {
            Ok(())
        } else {
            Err(Error::InvalidCookie(format!(
                "{cookie:?} was refused for {url}"
            )))
        }
    }

    /// Removes every cookie.
    pub fn clear_cookies(&self) {
        self.inner.cookies.clear();
    }

    /// The cache of compiled scripts its pages share, if it has one (see
    /// [`BrowserBuilder::script_cache`]).
    pub fn script_cache(&self) -> Option<&ScriptCache> {
        self.inner.runtime.script_cache.as_ref()
    }

    pub(crate) fn runtime_options(&self) -> &RuntimeOptions {
        &self.inner.runtime
    }

    pub(crate) fn max_navigations(&self) -> usize {
        self.inner.max_navigations
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.inner.timeout
    }
}

/// Configures a [`Browser`].
///
/// The defaults suit scraping: a desktop Chrome user agent and a 1920×1080
/// viewport, a 30 second timeout, and the virtual clock, so pages waiting
/// on long timers do not hold things up.
#[derive(Clone, Debug)]
pub struct BrowserBuilder {
    user_agent: Option<String>,
    headers: Vec<(String, String)>,
    proxy: Option<String>,
    blocked: Vec<String>,
    block_private_addresses: bool,
    request_timeout: Option<Duration>,
    max_response_size: Option<u64>,
    root_certificates: Vec<Vec<u8>>,
    viewport: Viewport,
    visibility: Visibility,
    clock: ClockMode,
    memory_limit: Option<usize>,
    max_dom_nodes: Option<usize>,
    max_navigations: usize,
    timeout: Duration,
    /// `None` until set: a cache of the browser's own.
    script_cache: Option<Option<ScriptCache>>,
}

impl Default for BrowserBuilder {
    fn default() -> Self {
        Self {
            user_agent: None,
            headers: Vec::new(),
            proxy: None,
            blocked: Vec::new(),
            block_private_addresses: false,
            request_timeout: None,
            max_response_size: None,
            root_certificates: Vec::new(),
            viewport: Viewport::default(),
            visibility: Visibility::default(),
            clock: ClockMode::Virtual,
            memory_limit: Some(dasher_js::DEFAULT_MEMORY_LIMIT),
            max_dom_nodes: Some(dasher_js::DEFAULT_MAX_DOM_NODES),
            max_navigations: dasher_page::DEFAULT_MAX_NAVIGATIONS,
            timeout: DEFAULT_TIMEOUT,
            script_cache: None,
        }
    }
}

impl BrowserBuilder {
    /// The user agent: both the `User-Agent` header and
    /// `navigator.userAgent`, with `navigator.platform` to match. Defaults
    /// to a current desktop Chrome on Linux.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// Adds a header to every request, replacing the default of the same
    /// name (only `Accept-Language: en-US,en;q=0.9` by default).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sends every request through an HTTP proxy, such as
    /// `http://user:password@proxy.internal:3128`. HTTPS is tunnelled with
    /// `CONNECT`.
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = Some(url.into());
        self
    }

    /// Refuses requests whose whole URL matches `pattern`, where `*`
    /// matches any run of characters: `*://*.google-analytics.com/*`,
    /// `*/ads/*`, `*analytics*`. Blocked scripts and `fetch()` calls fail as network
    /// errors do. Images, stylesheets, fonts and media are never loaded in
    /// any case.
    pub fn block(mut self, pattern: impl Into<String>) -> Self {
        self.blocked.push(pattern.into());
        self
    }

    /// Refuses to connect to loopback, link-local, private (RFC 1918 and
    /// IPv6 unique local), CGNAT, unspecified, multicast and reserved
    /// addresses. Off by default; turn it on when the URLs, or the pages
    /// behind them, come from someone you don't trust, so they can't reach
    /// the cloud metadata server or anything else on the private network.
    ///
    /// The check is on the address connected to, after DNS, so it catches
    /// host names that resolve or rebind to a private address and
    /// redirects to them, not only literal IPs. A page refused this way
    /// fails to load with a blocked [`LoadError::Network`]; refused
    /// scripts and `fetch()` calls fail as network errors do. Through a
    /// [`proxy`](Self::proxy) only literal IP targets and the proxy's own
    /// address can be checked, since the proxy resolves names itself.
    /// See [`dasher_net::ClientBuilder::block_private_addresses`].
    ///
    /// [`LoadError::Network`]: crate::LoadError::Network
    pub fn block_private_addresses(mut self, enabled: bool) -> Self {
        self.block_private_addresses = enabled;
        self
    }

    /// How long one request may take, from sending it to the end of its
    /// body. Defaults to dasher-net's
    /// [`DEFAULT_TIMEOUT`](dasher_net::DEFAULT_TIMEOUT).
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// The largest response body accepted, in bytes.
    pub fn max_response_size(mut self, bytes: u64) -> Self {
        self.max_response_size = Some(bytes);
        self
    }

    /// Trusts an extra root certificate (DER-encoded), e.g. for an
    /// internal CA.
    pub fn add_root_certificate(mut self, der: impl Into<Vec<u8>>) -> Self {
        self.root_certificates.push(der.into());
        self
    }

    /// The window size pages see, in CSS pixels; the screen is made the
    /// same size. Nothing is rendered: this feeds `innerWidth`, `screen`,
    /// `matchMedia` and the synthetic layout.
    pub fn viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport.width = width;
        self.viewport.height = height;
        self.viewport.screen_width = width;
        self.viewport.screen_height = height;
        self
    }

    /// The full window and screen description, for device pixel ratio and
    /// touch emulation.
    pub fn viewport_details(mut self, viewport: Viewport) -> Self {
        self.viewport = viewport;
        self
    }

    /// What counts as in view, which decides how much lazily loaded
    /// content pages fetch, and so trades CPU time and requests for
    /// content:
    ///
    /// - [`Visibility::Loaded`] (the default): what the document holds
    ///   once the load event has run is in view, so lazy sections and
    ///   images present at load arrive; what the page appends after that
    ///   is below the fold until auto-scroll reveals it, so no loader can
    ///   keep itself going.
    /// - [`Visibility::All`]: everything is in view, whenever it was
    ///   inserted. The most content, but a page whose loader checks for
    ///   visibility after each batch loads until the timeout.
    /// - [`Visibility::Viewport`]: a real viewport; only what is scrolled
    ///   to is in view. The least work, and the least lazy content.
    pub fn visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// How page time relates to the wall clock. Defaults to
    /// [`ClockMode::Virtual`]: when a page is only waiting on timers, time
    /// skips ahead to the next one.
    pub fn clock(mut self, clock: ClockMode) -> Self {
        self.clock = clock;
        self
    }

    /// Caps each page's JS heap, in bytes (`None` for no cap). Defaults to
    /// [`DEFAULT_MEMORY_LIMIT`](crate::DEFAULT_MEMORY_LIMIT), 256 MiB.
    ///
    /// A script that asks for more than fits gets an out-of-memory error.
    /// A page that fills its heap (an allocation is refused with more than
    /// half of it in use) is stopped, and the page operation running fails
    /// with [`Error::LimitExceeded`].
    pub fn memory_limit(mut self, bytes: Option<usize>) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// Caps the number of nodes each page's DOM may hold, scripts' detached
    /// nodes included (`None` for no cap). Defaults to
    /// [`DEFAULT_MAX_DOM_NODES`](crate::DEFAULT_MAX_DOM_NODES), 500,000.
    ///
    /// A page that goes over it is stopped, and the page operation running
    /// fails with [`Error::LimitExceeded`].
    pub fn max_dom_nodes(mut self, nodes: Option<usize>) -> Self {
        self.max_dom_nodes = nodes;
        self
    }

    /// How many script-triggered navigations (`location.href = …`) a page
    /// follows per [`goto`](Page::goto).
    pub fn max_navigations(mut self, max: usize) -> Self {
        self.max_navigations = max;
        self
    }

    /// Where pages keep compiled scripts for later pages: a
    /// [`ScriptCache`], or `None` to compile every script from its text.
    ///
    /// By default each browser has a cache of its own, in memory, of
    /// [`DEFAULT_SCRIPT_CACHE_BYTES`](crate::DEFAULT_SCRIPT_CACHE_BYTES)
    /// (32 MiB), shared by its pages and its clones. A script compiled
    /// once is read back by every later page that loads the same text,
    /// whatever its URL, so a page's second visit (and a site that shares
    /// a bundle with one visited before) skips most of its compiling. The
    /// cache fills up to its bound and stays that size, which counts
    /// against the process's memory; pass `None` for a browser that opens
    /// a single page. Pass a cache made with
    /// [`ScriptCache::new`](crate::ScriptCache::new) to bound it
    /// differently, to keep it on disk too, or to share one between
    /// browsers.
    pub fn script_cache(mut self, cache: Option<ScriptCache>) -> Self {
        self.script_cache = Some(cache);
        self
    }

    /// The default time limit for page operations: a whole
    /// [`goto`](Page::goto), waiting for a selector, awaiting an
    /// evaluated promise. Defaults to [`DEFAULT_TIMEOUT`]. Pages can
    /// change their own with [`Page::set_timeout`].
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builds the browser. Fails if a header, the user agent or the proxy
    /// URL is invalid.
    pub fn build(self) -> Result<Browser> {
        let mut client = Client::builder();

        let mut window = WindowOptions {
            viewport: self.viewport,
            visibility: self.visibility,
            local_storage: StorageStore::new(),
            ..WindowOptions::default()
        };
        if let Some(user_agent) = self.user_agent {
            let value = HeaderValue::from_str(&user_agent)
                .map_err(|_| Error::Config(format!("invalid user agent {user_agent:?}")))?;
            client = client.user_agent(value);
            if let Some(platform) = platform_for(&user_agent) {
                window.platform = platform.to_owned();
            }
            window.user_agent = user_agent;
        } else {
            window.user_agent = dasher_net::DEFAULT_USER_AGENT.to_owned();
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        for (name, value) in &self.headers {
            let name = HeaderName::from_bytes(name.trim().as_bytes())
                .map_err(|_| Error::Config(format!("invalid header name {name:?}")))?;
            let value = HeaderValue::from_str(value.trim())
                .map_err(|_| Error::Config(format!("invalid value for header {name}")))?;
            if name == header::USER_AGENT {
                return Err(Error::Config(
                    "set the user agent with BrowserBuilder::user_agent".into(),
                ));
            }
            headers.insert(name, value);
        }
        client = client.default_headers(headers);

        let mut policy = RequestPolicy::new();
        for pattern in self.blocked {
            policy = policy.block_url(pattern);
        }
        client = client.policy(policy);

        if let Some(proxy) = &self.proxy {
            let proxy = Proxy::http(proxy).map_err(|e| Error::Config(e.to_string()))?;
            client = client.proxy(proxy);
        }
        client = client.block_private_addresses(self.block_private_addresses);
        if let Some(timeout) = self.request_timeout {
            client = client.timeout(Some(timeout));
        }
        if let Some(bytes) = self.max_response_size {
            client = client.max_response_size(bytes);
        }
        for der in self.root_certificates {
            client = client.add_root_certificate(der);
        }
        let client = client.build().map_err(|e| Error::Config(e.to_string()))?;

        let runtime = RuntimeOptions {
            memory_limit: self.memory_limit,
            max_dom_nodes: self.max_dom_nodes,
            clock: self.clock,
            window,
            script_cache: self
                .script_cache
                .unwrap_or_else(|| Some(ScriptCache::new(ScriptCacheOptions::default()))),
            ..RuntimeOptions::default()
        };
        Ok(Browser {
            inner: Arc::new(Inner {
                client,
                cookies: Arc::new(CookieJar::new()),
                runtime,
                max_navigations: self.max_navigations,
                timeout: self.timeout,
            }),
        })
    }
}

/// The `navigator.platform` that goes with a user agent string, when it
/// names a well-known platform.
fn platform_for(user_agent: &str) -> Option<&'static str> {
    let has = |s| user_agent.contains(s);
    if has("iPhone") {
        Some("iPhone")
    } else if has("iPad") {
        Some("iPad")
    } else if has("Android") {
        Some("Linux armv8l")
    } else if has("Windows") {
        Some("Win32")
    } else if has("Macintosh") || has("Mac OS X") {
        Some("MacIntel")
    } else if has("Linux") || has("X11") {
        Some("Linux x86_64")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Browser>();
    }

    #[test]
    fn platform_follows_the_user_agent() {
        assert_eq!(
            platform_for("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
            Some("Win32")
        );
        assert_eq!(
            platform_for("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"),
            Some("MacIntel")
        );
        assert_eq!(
            platform_for("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)"),
            Some("iPhone")
        );
        assert_eq!(
            platform_for("Mozilla/5.0 (Linux; Android 14; Pixel 8)"),
            Some("Linux armv8l")
        );
        assert_eq!(platform_for("curl/8.0"), None);
    }

    #[test]
    fn invalid_configuration_is_reported() {
        let bad = |b: BrowserBuilder| matches!(b.build(), Err(Error::Config(_)));
        assert!(bad(Browser::builder().header("bad name", "x")));
        assert!(bad(Browser::builder().header("X-Ok", "line\nbreak")));
        assert!(bad(Browser::builder().header("User-Agent", "x")));
        assert!(bad(Browser::builder().user_agent("a\u{7f}b")));
        assert!(bad(Browser::builder().proxy("socks5://localhost:1080")));
        assert!(Browser::builder().header("X-Ok", "fine").build().is_ok());
    }

    #[test]
    fn cookies_are_set_for_http_urls_only() {
        let browser = Browser::new().unwrap();
        browser
            .set_cookie("https://example.com/", "a=1; Path=/")
            .unwrap();
        assert!(matches!(
            browser.set_cookie("ftp://example.com/", "a=1"),
            Err(Error::InvalidCookie(_))
        ));
        assert!(matches!(
            browser.set_cookie("https://example.com/", "a=1; Domain=other.test"),
            Err(Error::InvalidCookie(_))
        ));
        let cookies = browser.cookies();
        assert_eq!(cookies.len(), 1);
        assert_eq!((cookies[0].name(), cookies[0].value()), ("a", "1"));
        browser.clear_cookies();
        assert!(browser.cookies().is_empty());
    }
}
