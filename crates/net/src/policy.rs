//! The request policy: what dasher is allowed to fetch.
//!
//! Every request, and every redirect hop of every request, is checked against
//! a [`RequestPolicy`] before anything goes over the network. The policy
//! always refuses:
//!
//! - resource types dasher never downloads (images, stylesheets, fonts and
//!   media; see [`ResourceType::is_fetchable`]), and
//! - URLs whose scheme is not `http` or `https`.
//!
//! On top of that, callers can block URLs with wildcard patterns
//! ([`RequestPolicy::block_url`]) and install an arbitrary filter
//! ([`RequestPolicy::filter`]).

use std::fmt;
use std::sync::Arc;

use hyper::Method;
use url::Url;

/// What a request is for, in the sense of the Fetch standard's request
/// destination.
///
/// Only documents, scripts and fetch/XHR traffic are ever fetched; the other
/// variants exist so callers can describe what a page asked for and have the
/// policy refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    /// A top-level navigation (the page's HTML document).
    Document,
    /// A classic or module script.
    Script,
    /// A `fetch()` call.
    Fetch,
    /// An `XMLHttpRequest`.
    Xhr,
    /// An image, including favicons and `<img srcset>` candidates.
    Image,
    /// A CSS stylesheet.
    Stylesheet,
    /// A web font.
    Font,
    /// Audio, video or text tracks.
    Media,
}

impl ResourceType {
    /// Whether dasher ever fetches this type of resource. Images,
    /// stylesheets, fonts and media are never downloaded.
    pub fn is_fetchable(self) -> bool {
        matches!(
            self,
            Self::Document | Self::Script | Self::Fetch | Self::Xhr
        )
    }

    /// A short lowercase name, e.g. `"script"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Script => "script",
            Self::Fetch => "fetch",
            Self::Xhr => "xhr",
            Self::Image => "image",
            Self::Stylesheet => "stylesheet",
            Self::Font => "font",
            Self::Media => "media",
        }
    }

    /// The default `Accept` header a browser sends for this type of request.
    pub fn accept(self) -> &'static str {
        match self {
            Self::Document => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            Self::Image => "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
            Self::Stylesheet => "text/css,*/*;q=0.1",
            Self::Script | Self::Fetch | Self::Xhr | Self::Font | Self::Media => "*/*",
        }
    }
}

impl fmt::Display for ResourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A URL pattern in which `*` matches any run of characters (including none)
/// and every other character matches itself.
///
/// The pattern must match the whole serialized URL, e.g.
/// `*://*.doubleclick.net/*` or `https://example.com/ads/*`. Use `*` at both
/// ends to match a substring: `*analytics*`. URLs are matched in their
/// normalized form, so the scheme and host are lowercase and hosts are
/// punycode.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct UrlPattern {
    pattern: Box<str>,
}

impl UrlPattern {
    /// Creates a pattern.
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    /// The pattern as given.
    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// Whether `url` matches the pattern.
    pub fn matches(&self, url: &str) -> bool {
        wildcard_match(self.pattern.as_bytes(), url.as_bytes())
    }
}

impl fmt::Debug for UrlPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.pattern, f)
    }
}

impl From<&str> for UrlPattern {
    fn from(pattern: &str) -> Self {
        Self::new(pattern)
    }
}

impl From<String> for UrlPattern {
    fn from(pattern: String) -> Self {
        Self {
            pattern: pattern.into_boxed_str(),
        }
    }
}

/// Glob matching with `*` only, in linear space and O(n·m) worst-case time
/// (the classic backtrack-to-last-star algorithm).
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    // Position of the last `*` seen in the pattern, and the text position it
    // is currently assumed to have consumed up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            // Let the last star swallow one more character and retry.
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// A request as seen by the policy.
#[derive(Debug, Clone, Copy)]
pub struct RequestInfo<'a> {
    /// The URL about to be requested.
    pub url: &'a Url,
    /// The request method.
    pub method: &'a Method,
    /// What the request is for.
    pub resource_type: ResourceType,
    /// The number of redirects followed to reach `url`; `0` for the original
    /// request.
    pub redirects: usize,
}

/// Why the policy refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// The resource type is one dasher never fetches.
    ResourceType(ResourceType),
    /// The URL's scheme is not `http` or `https`.
    Scheme,
    /// The URL matched a blocked pattern.
    UrlPattern(UrlPattern),
    /// The caller's filter refused the request.
    Filter,
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceType(ty) => write!(f, "{ty} requests are never made"),
            Self::Scheme => f.write_str("only http and https URLs can be fetched"),
            Self::UrlPattern(p) => write!(f, "URL matches blocked pattern {:?}", p.as_str()),
            Self::Filter => f.write_str("refused by the request filter"),
        }
    }
}

type Filter = Arc<dyn Fn(&RequestInfo<'_>) -> bool + Send + Sync>;

/// Decides which requests may go over the network.
///
/// The default policy allows every `http`/`https` request for a document,
/// script, fetch or XHR and refuses everything else.
///
/// ```
/// use dasher_net::{RequestInfo, RequestPolicy, ResourceType};
///
/// let policy = RequestPolicy::new()
///     .block_url("*://*.doubleclick.net/*")
///     .filter(|req: &RequestInfo<'_>| req.url.host_str() != Some("tracker.example"));
/// # let _ = policy;
/// ```
#[derive(Clone, Default)]
pub struct RequestPolicy {
    blocked: Vec<UrlPattern>,
    filter: Option<Filter>,
}

impl RequestPolicy {
    /// The default policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuses requests whose URL matches `pattern` (see [`UrlPattern`]).
    pub fn block_url(mut self, pattern: impl Into<UrlPattern>) -> Self {
        self.blocked.push(pattern.into());
        self
    }

    /// Refuses requests for which `filter` returns `false`. It runs after the
    /// built-in checks and the URL patterns, so it only sees requests that
    /// would otherwise be allowed. Setting a filter replaces any previous one.
    pub fn filter<F>(mut self, filter: F) -> Self
    where
        F: Fn(&RequestInfo<'_>) -> bool + Send + Sync + 'static,
    {
        self.filter = Some(Arc::new(filter));
        self
    }

    /// The blocked URL patterns, in the order they were added.
    pub fn blocked_urls(&self) -> &[UrlPattern] {
        &self.blocked
    }

    /// Checks a request, returning why it is refused if it is.
    pub fn check(&self, request: &RequestInfo<'_>) -> Result<(), BlockReason> {
        if !request.resource_type.is_fetchable() {
            return Err(BlockReason::ResourceType(request.resource_type));
        }
        if !matches!(request.url.scheme(), "http" | "https") {
            return Err(BlockReason::Scheme);
        }
        let url = request.url.as_str();
        if let Some(pattern) = self.blocked.iter().find(|p| p.matches(url)) {
            return Err(BlockReason::UrlPattern(pattern.clone()));
        }
        match &self.filter {
            Some(filter) if !filter(request) => Err(BlockReason::Filter),
            _ => Ok(()),
        }
    }
}

impl fmt::Debug for RequestPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestPolicy")
            .field("blocked", &self.blocked)
            .field("filter", &self.filter.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info<'a>(url: &'a Url, ty: ResourceType) -> RequestInfo<'a> {
        RequestInfo {
            url,
            method: &Method::GET,
            resource_type: ty,
            redirects: 0,
        }
    }

    #[test]
    fn wildcards() {
        let m = |p: &str, t: &str| UrlPattern::new(p).matches(t);
        assert!(m("", ""));
        assert!(!m("", "a"));
        assert!(m("*", ""));
        assert!(m("*", "anything"));
        assert!(m("abc", "abc"));
        assert!(!m("abc", "abcd"));
        assert!(!m("abcd", "abc"));
        assert!(m("a*c", "abbbc"));
        assert!(m("a*c", "ac"));
        assert!(!m("a*c", "abcb"));
        assert!(m("a*b*c", "a-b-b-c"));
        assert!(m("**a**", "xxaxx"));
        assert!(m("*.js", "https://x/app.min.js"));
        assert!(!m("*.js", "https://x/app.json"));
        assert!(m(
            "*://*.doubleclick.net/*",
            "https://ad.doubleclick.net/x?y"
        ));
        assert!(!m("*://*.doubleclick.net/*", "https://doubleclick.net/x"));
        // Pathological input stays fast.
        let text = "a".repeat(10_000);
        assert!(!m("*a*a*a*a*a*b", &text));
    }

    #[test]
    fn refuses_unfetchable_types() {
        let policy = RequestPolicy::new();
        let url = Url::parse("https://example.com/x").unwrap();
        for ty in [
            ResourceType::Document,
            ResourceType::Script,
            ResourceType::Fetch,
            ResourceType::Xhr,
        ] {
            assert_eq!(policy.check(&info(&url, ty)), Ok(()), "{ty}");
        }
        for ty in [
            ResourceType::Image,
            ResourceType::Stylesheet,
            ResourceType::Font,
            ResourceType::Media,
        ] {
            assert_eq!(
                policy.check(&info(&url, ty)),
                Err(BlockReason::ResourceType(ty))
            );
        }
    }

    #[test]
    fn refuses_other_schemes() {
        let policy = RequestPolicy::new();
        for url in [
            "file:///etc/passwd",
            "data:text/plain,hi",
            "ftp://example.com/",
            "javascript:alert(1)",
        ] {
            let url = Url::parse(url).unwrap();
            assert_eq!(
                policy.check(&info(&url, ResourceType::Fetch)),
                Err(BlockReason::Scheme),
                "{url}"
            );
        }
    }

    #[test]
    fn blocks_patterns_and_filters() {
        let policy = RequestPolicy::new()
            .block_url("*://ads.example/*")
            .block_url(String::from("*/track?*"))
            .filter(|r| r.method != Method::DELETE && r.redirects < 3);
        assert_eq!(policy.blocked_urls().len(), 2);

        let ok = Url::parse("https://example.com/").unwrap();
        assert_eq!(policy.check(&info(&ok, ResourceType::Script)), Ok(()));

        let ad = Url::parse("http://ads.example/banner.js").unwrap();
        assert_eq!(
            policy.check(&info(&ad, ResourceType::Script)),
            Err(BlockReason::UrlPattern("*://ads.example/*".into()))
        );
        let track = Url::parse("https://example.com/track?id=1").unwrap();
        assert!(policy.check(&info(&track, ResourceType::Xhr)).is_err());

        let delete = RequestInfo {
            method: &Method::DELETE,
            ..info(&ok, ResourceType::Fetch)
        };
        assert_eq!(policy.check(&delete), Err(BlockReason::Filter));
        let deep = RequestInfo {
            redirects: 3,
            ..info(&ok, ResourceType::Fetch)
        };
        assert_eq!(policy.check(&deep), Err(BlockReason::Filter));
    }

    #[test]
    fn unfetchable_types_are_refused_before_the_filter_runs() {
        let policy = RequestPolicy::new().filter(|_| panic!("filter must not run"));
        let url = Url::parse("https://example.com/a.png").unwrap();
        assert!(policy.check(&info(&url, ResourceType::Image)).is_err());
    }
}
