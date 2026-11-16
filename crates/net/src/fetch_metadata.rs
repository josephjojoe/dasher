//! Fetch metadata request headers: `Sec-Fetch-Dest`, `Sec-Fetch-Mode`,
//! `Sec-Fetch-Site` and `Sec-Fetch-User`, which every current browser sends
//! and servers read to tell a navigation from a script's request, and their
//! own pages' requests from another site's.
//! <https://w3c.github.io/webappsec-fetch-metadata/>
//!
//! A server may answer differently without them: instagram.com serves its
//! error page to a navigation that carries none.
//!
//! Deliberate simplifications:
//!
//! - There is no public suffix list (as for cookies), so whether two hosts
//!   are the same site is decided from their last two labels, or their last
//!   three under a two-letter top-level domain whose second level is a
//!   generic one (`co.uk`, `com.au`). Hosts under a suffix like `github.io`
//!   count as one site.
//! - A request with no initiator counts as the user's own, as a typed URL
//!   is: `Sec-Fetch-Site: none`.

use std::net::IpAddr;

use hyper::HeaderMap;
use hyper::header::{HeaderName, HeaderValue};
use url::{Host, Origin, Url};

use crate::policy::ResourceType;

const SEC_FETCH_DEST: HeaderName = HeaderName::from_static("sec-fetch-dest");
const SEC_FETCH_MODE: HeaderName = HeaderName::from_static("sec-fetch-mode");
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");
const SEC_FETCH_USER: HeaderName = HeaderName::from_static("sec-fetch-user");

/// A request's mode in the Fetch standard's sense, which `Sec-Fetch-Mode`
/// reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestMode {
    /// A navigation: loading a document.
    Navigate,
    /// A classic script without `crossorigin`, or `fetch()` with
    /// `mode: 'no-cors'`.
    NoCors,
    /// `fetch()` and `XMLHttpRequest` by default, module scripts, and
    /// classic scripts with `crossorigin`.
    Cors,
    /// `fetch()` with `mode: 'same-origin'`.
    SameOrigin,
}

impl RequestMode {
    /// The mode a request for `resource_type` has when none is given.
    pub fn for_type(resource_type: ResourceType) -> Self {
        match resource_type {
            ResourceType::Document => Self::Navigate,
            ResourceType::Fetch | ResourceType::Xhr | ResourceType::Font => Self::Cors,
            ResourceType::Script
            | ResourceType::Image
            | ResourceType::Stylesheet
            | ResourceType::Media => Self::NoCors,
        }
    }

    /// The header value: `navigate`, `no-cors`, `cors` or `same-origin`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::NoCors => "no-cors",
            Self::Cors => "cors",
            Self::SameOrigin => "same-origin",
        }
    }
}

/// How the origin making a request relates to the URLs it has been sent
/// to, redirects included: the value of `Sec-Fetch-Site`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Relation {
    /// No origin made it: the user did.
    None,
    SameOrigin,
    SameSite,
    CrossSite,
}

impl Relation {
    /// The relation of `initiator` to `url`.
    pub(crate) fn of(initiator: Option<&Origin>, url: &Url) -> Self {
        let Some(initiator) = initiator else {
            return Self::None;
        };
        let target = url.origin();
        if *initiator == target {
            Self::SameOrigin
        } else if same_site(initiator, &target) {
            Self::SameSite
        } else {
            Self::CrossSite
        }
    }

    /// The relation after a redirect to `next`: the furthest the request
    /// has been from its initiator.
    pub(crate) fn after_redirect(self, initiator: Option<&Origin>, next: &Url) -> Self {
        self.max(Self::of(initiator, next))
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::SameOrigin => "same-origin",
            Self::SameSite => "same-site",
            Self::CrossSite => "cross-site",
        }
    }
}

/// Adds the headers for a request of `resource_type` and `mode` to `url`.
/// Like browsers, only to a URL that is potentially trustworthy: `https`,
/// or `http` to this machine.
pub(crate) fn insert(
    headers: &mut HeaderMap,
    url: &Url,
    resource_type: ResourceType,
    mode: RequestMode,
    site: Relation,
) {
    if !potentially_trustworthy(url) {
        return;
    }
    headers.insert(
        SEC_FETCH_DEST,
        HeaderValue::from_static(resource_type.destination()),
    );
    headers.insert(SEC_FETCH_MODE, HeaderValue::from_static(mode.as_str()));
    headers.insert(SEC_FETCH_SITE, HeaderValue::from_static(site.as_str()));
    // A navigation nothing on a page asked for is the user's.
    if mode == RequestMode::Navigate && site == Relation::None {
        headers.insert(SEC_FETCH_USER, HeaderValue::from_static("?1"));
    }
}

fn potentially_trustworthy(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(Host::Domain(host)) => {
                let host = host.trim_end_matches('.');
                host.eq_ignore_ascii_case("localhost")
                    || host.to_ascii_lowercase().ends_with(".localhost")
            }
            Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
            Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Whether two origins are the same site: the same scheme and registrable
/// domain, whatever their ports.
fn same_site(a: &Origin, b: &Origin) -> bool {
    let (Origin::Tuple(a_scheme, a_host, _), Origin::Tuple(b_scheme, b_host, _)) = (a, b) else {
        return false;
    };
    if a_scheme != b_scheme {
        return false;
    }
    match (a_host, b_host) {
        (Host::Domain(a), Host::Domain(b)) => site_domain(a) == site_domain(b),
        (a, b) => a == b,
    }
}

/// Second-level labels that are registries under a country's domain.
const GENERIC_SECOND_LEVEL: [&str; 11] = [
    "ac", "co", "com", "edu", "go", "gov", "mil", "ne", "net", "or", "org",
];

/// The registrable domain of `host`, as near as can be told without a
/// public suffix list (see the module docs).
fn site_domain(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    let last = |labels: usize| match host.rmatch_indices('.').nth(labels - 1) {
        Some((dot, _)) => &host[dot + 1..],
        None => host,
    };
    let tld = last(1);
    let two = last(2);
    let second = two
        .strip_suffix(tld)
        .and_then(|s| s.strip_suffix('.'))
        .unwrap_or_default();
    if tld.len() == 2 && GENERIC_SECOND_LEVEL.contains(&second) {
        last(3)
    } else {
        two
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn site(from: &str, to: &str) -> &'static str {
        Relation::of(Some(&url(from).origin()), &url(to)).as_str()
    }

    #[test]
    fn site_domains() {
        assert_eq!(site_domain("example.com"), "example.com");
        assert_eq!(site_domain("www.example.com"), "example.com");
        assert_eq!(site_domain("a.b.example.com."), "example.com");
        assert_eq!(site_domain("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_domain("bbc.co.uk"), "bbc.co.uk");
        assert_eq!(site_domain("shop.example.com.au"), "example.com.au");
        assert_eq!(site_domain("www.flashscore.ro"), "flashscore.ro");
        assert_eq!(site_domain("co.uk"), "co.uk");
        assert_eq!(site_domain("localhost"), "localhost");
    }

    #[test]
    fn sites() {
        assert_eq!(Relation::of(None, &url("https://a.com/")).as_str(), "none");
        assert_eq!(site("https://a.com/x", "https://a.com/y"), "same-origin");
        assert_eq!(
            site("https://www.a.com/", "https://api.a.com/"),
            "same-site"
        );
        assert_eq!(site("https://a.com/", "https://a.com:8443/"), "same-site");
        assert_eq!(site("https://a.com/", "https://b.com/"), "cross-site");
        // The scheme is part of the site.
        assert_eq!(site("http://a.com/", "https://a.com/"), "cross-site");
        assert_eq!(site("https://a.co.uk/", "https://b.co.uk/"), "cross-site");
        assert_eq!(
            site("https://x.a.co.uk/", "https://y.a.co.uk/"),
            "same-site"
        );
        assert_eq!(
            site("http://127.0.0.1:1/", "http://127.0.0.1:2/"),
            "same-site"
        );
        assert_eq!(site("http://127.0.0.1/", "http://127.0.0.2/"), "cross-site");
        // An opaque origin is no site's.
        assert_eq!(site("data:text/html,x", "https://a.com/"), "cross-site");
    }

    #[test]
    fn redirects_keep_the_furthest_relation() {
        let from = url("https://a.com/").origin();
        let start = Relation::of(Some(&from), &url("https://a.com/go"));
        assert_eq!(start, Relation::SameOrigin);
        let away = start.after_redirect(Some(&from), &url("https://b.com/"));
        assert_eq!(away, Relation::CrossSite);
        let back = away.after_redirect(Some(&from), &url("https://a.com/"));
        assert_eq!(back, Relation::CrossSite);
        let typed = Relation::of(None, &url("https://a.com/"));
        assert_eq!(
            typed.after_redirect(None, &url("https://b.com/")),
            Relation::None
        );
    }

    #[test]
    fn only_to_trustworthy_urls() {
        let sent = |to: &str| {
            let mut headers = HeaderMap::new();
            insert(
                &mut headers,
                &url(to),
                ResourceType::Document,
                RequestMode::Navigate,
                Relation::None,
            );
            headers.len()
        };
        assert_eq!(sent("https://example.com/"), 4);
        assert_eq!(sent("http://localhost:8080/"), 4);
        assert_eq!(sent("http://app.localhost/"), 4);
        assert_eq!(sent("http://127.0.0.1:3000/"), 4);
        assert_eq!(sent("http://[::1]/"), 4);
        assert_eq!(sent("http://example.com/"), 0);
        assert_eq!(sent("http://192.168.1.2/"), 0);
    }

    #[test]
    fn user_only_on_the_users_navigations() {
        let user = |resource_type, mode, site| {
            let mut headers = HeaderMap::new();
            insert(
                &mut headers,
                &url("https://a.com/"),
                resource_type,
                mode,
                site,
            );
            headers.contains_key(SEC_FETCH_USER)
        };
        assert!(user(
            ResourceType::Document,
            RequestMode::Navigate,
            Relation::None
        ));
        assert!(!user(
            ResourceType::Document,
            RequestMode::Navigate,
            Relation::SameOrigin
        ));
        assert!(!user(
            ResourceType::Fetch,
            RequestMode::Cors,
            Relation::None
        ));
    }
}
