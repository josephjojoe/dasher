//! An in-memory cookie jar following RFC 6265 (and the parts of RFC 6265bis
//! that browsers ship: nameless cookies, cookie prefixes and "leave secure
//! cookies alone").
//!
//! One jar belongs to one browsing session. It is shared between the HTTP
//! client, which stores `Set-Cookie` headers and sends `Cookie` headers, and
//! the page's `document.cookie`, which sees and sets only non-`HttpOnly`
//! cookies.
//!
//! Deliberate simplifications, acceptable for a scraper:
//!
//! - There is no public suffix list. A `Domain` attribute without a dot
//!   (`Domain=com`) is refused unless it names the host itself, but cookies
//!   for registrable suffixes such as `co.uk` are accepted.
//! - `SameSite` is parsed and kept but not enforced: requests always carry the
//!   cookies that match their URL.
//!
//! The jar is bounded: at most [`MAX_COOKIES_PER_DOMAIN`] cookies per domain
//! and [`MAX_COOKIES`] overall, evicting the least recently set first, and a
//! cookie's name and value together may not exceed [`MAX_COOKIE_SIZE`] bytes.

use std::fmt;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use url::{Host, Url};

/// The most cookies kept for one domain (Chrome's limit).
pub const MAX_COOKIES_PER_DOMAIN: usize = 180;
/// The most cookies kept in one jar (Chrome's limit).
pub const MAX_COOKIES: usize = 3000;
/// The longest name plus value accepted, in bytes (RFC 6265bis' limit).
pub const MAX_COOKIE_SIZE: usize = 4096;
/// Attribute values longer than this are ignored (RFC 6265bis' limit).
const MAX_ATTRIBUTE_VALUE: usize = 1024;
/// The furthest in the future a cookie may expire: 400 days (RFC 6265bis).
const MAX_AGE_SECONDS: i64 = 400 * 24 * 60 * 60;

/// The `SameSite` attribute of a cookie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SameSite {
    /// No (or an unrecognised) `SameSite` attribute.
    #[default]
    Unspecified,
    /// `SameSite=Strict`.
    Strict,
    /// `SameSite=Lax`.
    Lax,
    /// `SameSite=None`.
    None,
}

/// A stored cookie.
#[derive(Clone, PartialEq, Eq)]
pub struct Cookie {
    name: Box<str>,
    value: Box<str>,
    /// Lowercase, without a leading dot.
    domain: Box<str>,
    path: Box<str>,
    /// Unix time in seconds; `None` for a session cookie.
    expires: Option<i64>,
    host_only: bool,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
    /// Order of creation within the jar, for sorting and eviction.
    created: u64,
}

impl Cookie {
    /// The cookie's name. Empty for a nameless cookie (`Set-Cookie: value`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The cookie's value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The domain the cookie belongs to, lowercase and without a leading dot.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Whether the cookie is sent only to exactly [`domain`](Self::domain)
    /// (it had no `Domain` attribute) rather than to its subdomains too.
    pub fn host_only(&self) -> bool {
        self.host_only
    }

    /// The cookie's path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// When the cookie expires, or `None` for a session cookie.
    pub fn expires(&self) -> Option<SystemTime> {
        self.expires.map(|secs| {
            if secs >= 0 {
                UNIX_EPOCH + std::time::Duration::from_secs(secs.unsigned_abs())
            } else {
                UNIX_EPOCH - std::time::Duration::from_secs(secs.unsigned_abs())
            }
        })
    }

    /// Whether the cookie is only sent over secure connections.
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Whether the cookie is hidden from `document.cookie`.
    pub fn http_only(&self) -> bool {
        self.http_only
    }

    /// The cookie's `SameSite` attribute.
    pub fn same_site(&self) -> SameSite {
        self.same_site
    }

    fn is_expired(&self, now: i64) -> bool {
        self.expires.is_some_and(|e| e <= now)
    }

    fn same_key(&self, other: &Self) -> bool {
        self.name == other.name && self.domain == other.domain && self.path == other.path
    }

    fn write_pair(&self, out: &mut String) {
        if !self.name.is_empty() {
            out.push_str(&self.name);
            out.push('=');
        }
        out.push_str(&self.value);
    }
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("value", &self.value)
            .field("domain", &self.domain)
            .field("host_only", &self.host_only)
            .field("path", &self.path)
            .field("expires", &self.expires)
            .field("secure", &self.secure)
            .field("http_only", &self.http_only)
            .field("same_site", &self.same_site)
            .finish()
    }
}

/// Where a cookie is being set from or read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// HTTP headers: `HttpOnly` cookies are visible.
    Http,
    /// `document.cookie`: `HttpOnly` cookies are invisible and untouchable.
    Script,
}

#[derive(Default)]
struct Store {
    cookies: Vec<Cookie>,
    next_created: u64,
}

/// An in-memory cookie jar for one browsing session. Thread-safe; share it
/// with an `Arc`.
#[derive(Default)]
pub struct CookieJar {
    store: Mutex<Store>,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieJar")
            .field("len", &self.lock().cookies.len())
            .finish()
    }
}

impl CookieJar {
    /// An empty jar.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Store> {
        // Nothing in here panics while holding the lock, but don't let a
        // poisoned lock take the session down either way.
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores the cookie from a `Set-Cookie` response header received from
    /// `url`. Returns whether a cookie was stored, replaced or deleted
    /// (`false` if the header was ignored).
    pub fn set_from_header(&self, url: &Url, set_cookie: &str) -> bool {
        self.set(url, set_cookie, Api::Http, unix_now())
    }

    /// Stores a cookie the way assigning to `document.cookie` on a page at
    /// `url` does: `HttpOnly` cookies can be neither set nor replaced.
    pub fn set_from_script(&self, url: &Url, cookie: &str) -> bool {
        self.set(url, cookie, Api::Script, unix_now())
    }

    /// The `Cookie` request header value for a request to `url`, or `None` if
    /// no cookies apply.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        let value = self.serialize(url, Api::Http, unix_now());
        (!value.is_empty()).then_some(value)
    }

    /// What `document.cookie` reads on a page at `url`: the matching
    /// non-`HttpOnly` cookies, or an empty string.
    pub fn script_cookies_for(&self, url: &Url) -> String {
        self.serialize(url, Api::Script, unix_now())
    }

    /// Every unexpired cookie in the jar, in creation order.
    pub fn cookies(&self) -> Vec<Cookie> {
        let now = unix_now();
        let mut store = self.lock();
        store.cookies.retain(|c| !c.is_expired(now));
        store.cookies.clone()
    }

    /// The number of cookies in the jar, including any that expired but
    /// have not been purged yet.
    pub fn len(&self) -> usize {
        self.lock().cookies.len()
    }

    /// Whether the jar holds no cookies.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes every cookie.
    pub fn clear(&self) {
        self.lock().cookies.clear();
    }

    fn set(&self, url: &Url, header: &str, api: Api, now: i64) -> bool {
        let Some(host) = canonical_host(url) else {
            return false;
        };
        let secure_url = is_secure(url);
        let Some(parsed) = parse_set_cookie(header, now) else {
            return false;
        };

        // RFC 6265bis 5.7: storage model.
        let (domain, host_only) = match parsed.domain {
            Some(domain) if domain != host => {
                // A host can't set cookies for an IP address other than
                // itself, for a domain it isn't in, or for a bare TLD.
                if is_ip(&host) || !domain_match(&host, &domain) || !domain.contains('.') {
                    return false;
                }
                (domain, false)
            }
            Some(domain) => (domain, false),
            None => (host, true),
        };
        if api == Api::Script && parsed.http_only {
            return false;
        }
        if parsed.secure && !secure_url {
            return false;
        }
        let path = parsed.path.unwrap_or_else(|| default_path(url).to_owned());

        let name_lower = parsed.name.to_ascii_lowercase();
        if name_lower.starts_with("__secure-") && !parsed.secure {
            return false;
        }
        if name_lower.starts_with("__host-") && !(parsed.secure && host_only && path == "/") {
            return false;
        }
        // A nameless cookie must not look like a prefixed one.
        if parsed.name.is_empty() {
            let value_lower = parsed.value.to_ascii_lowercase();
            if value_lower.starts_with("__secure-") || value_lower.starts_with("__host-") {
                return false;
            }
        }

        let mut store = self.lock();
        store.cookies.retain(|c| !c.is_expired(now));

        // Leave secure cookies alone: an insecure origin can't shadow them.
        if !parsed.secure
            && !secure_url
            && store.cookies.iter().any(|c| {
                c.secure
                    && c.name.as_ref() == parsed.name
                    && (domain_match(&c.domain, &domain) || domain_match(&domain, &c.domain))
                    && path_match(&path, &c.path)
            })
        {
            return false;
        }

        let mut cookie = Cookie {
            name: parsed.name.into(),
            value: parsed.value.into(),
            domain: domain.into(),
            path: path.into(),
            expires: parsed.expires,
            host_only,
            secure: parsed.secure,
            http_only: parsed.http_only,
            same_site: parsed.same_site,
            created: 0,
        };

        if let Some(index) = store.cookies.iter().position(|c| c.same_key(&cookie)) {
            let old = &store.cookies[index];
            if api == Api::Script && old.http_only {
                return false;
            }
            cookie.created = old.created;
            store.cookies.remove(index);
        } else {
            cookie.created = store.next_created;
            store.next_created += 1;
        }

        if cookie.is_expired(now) {
            // Setting an expired cookie is how sites delete one.
            return true;
        }
        let domain = cookie.domain.clone();
        store.cookies.push(cookie);
        store.evict(&domain);
        true
    }

    fn serialize(&self, url: &Url, api: Api, now: i64) -> String {
        let Some(host) = canonical_host(url) else {
            return String::new();
        };
        let secure_url = is_secure(url);
        let path = match url.path() {
            "" => "/",
            p => p,
        };

        let mut store = self.lock();
        store.cookies.retain(|c| !c.is_expired(now));
        let mut matching: Vec<&Cookie> = store
            .cookies
            .iter()
            .filter(|c| {
                let domain_ok = if c.host_only {
                    *c.domain == *host
                } else {
                    domain_match(&host, &c.domain)
                };
                domain_ok
                    && path_match(path, &c.path)
                    && (!c.secure || secure_url)
                    && (api == Api::Http || !c.http_only)
            })
            .collect();
        // Longer paths first, then older cookies first.
        matching.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then(a.created.cmp(&b.created))
        });

        let mut out = String::new();
        for (i, cookie) in matching.iter().enumerate() {
            if i > 0 {
                out.push_str("; ");
            }
            cookie.write_pair(&mut out);
        }
        out
    }
}

impl Store {
    /// Enforces the size limits after a cookie for `domain` was added.
    fn evict(&mut self, domain: &str) {
        let per_domain = self.cookies.iter().filter(|c| *c.domain == *domain).count();
        if per_domain > MAX_COOKIES_PER_DOMAIN {
            self.evict_oldest(|c| *c.domain == *domain);
        }
        if self.cookies.len() > MAX_COOKIES {
            self.evict_oldest(|_| true);
        }
    }

    fn evict_oldest(&mut self, mut filter: impl FnMut(&Cookie) -> bool) {
        let oldest = self
            .cookies
            .iter()
            .enumerate()
            .filter(|(_, c)| filter(c))
            .min_by_key(|(_, c)| c.created)
            .map(|(i, _)| i);
        if let Some(index) = oldest {
            self.cookies.remove(index);
        }
    }
}

/// A `Set-Cookie` header after parsing (RFC 6265bis 5.6), before the storage
/// rules are applied.
#[derive(Debug, PartialEq, Eq)]
struct ParsedCookie {
    name: String,
    value: String,
    expires: Option<i64>,
    /// Lowercase, without a leading dot.
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
}

fn parse_set_cookie(header: &str, now: i64) -> Option<ParsedCookie> {
    // Control characters other than tab make the whole cookie invalid.
    if header
        .bytes()
        .any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
    {
        return None;
    }
    let (pair, attributes) = match header.split_once(';') {
        Some((pair, rest)) => (pair, rest),
        None => (header, ""),
    };
    let (name, value) = match pair.split_once('=') {
        Some((name, value)) => (trim(name), trim(value)),
        None => ("", trim(pair)),
    };
    if name.is_empty() && value.is_empty() {
        return None;
    }
    if name.len() + value.len() > MAX_COOKIE_SIZE {
        return None;
    }

    let mut cookie = ParsedCookie {
        name: name.to_owned(),
        value: value.to_owned(),
        expires: None,
        domain: None,
        path: None,
        secure: false,
        http_only: false,
        same_site: SameSite::Unspecified,
    };
    let mut max_age = None;
    let mut expires = None;
    for attribute in attributes.split(';') {
        let (key, value) = match attribute.split_once('=') {
            Some((key, value)) => (trim(key), trim(value)),
            None => (trim(attribute), ""),
        };
        if value.len() > MAX_ATTRIBUTE_VALUE {
            continue;
        }
        if key.eq_ignore_ascii_case("expires") {
            if let Some(time) = parse_cookie_date(value) {
                expires = Some(time);
            }
        } else if key.eq_ignore_ascii_case("max-age") {
            if let Some(delta) = parse_max_age(value) {
                max_age = Some(delta);
            }
        } else if key.eq_ignore_ascii_case("domain") {
            let domain = value.strip_prefix('.').unwrap_or(value);
            if !domain.is_empty() {
                cookie.domain = Some(canonicalize_domain(domain)?);
            }
        } else if key.eq_ignore_ascii_case("path") {
            cookie.path = value.starts_with('/').then(|| value.to_owned());
        } else if key.eq_ignore_ascii_case("secure") {
            cookie.secure = true;
        } else if key.eq_ignore_ascii_case("httponly") {
            cookie.http_only = true;
        } else if key.eq_ignore_ascii_case("samesite") {
            cookie.same_site = if value.eq_ignore_ascii_case("strict") {
                SameSite::Strict
            } else if value.eq_ignore_ascii_case("lax") {
                SameSite::Lax
            } else if value.eq_ignore_ascii_case("none") {
                SameSite::None
            } else {
                SameSite::Unspecified
            };
        }
    }

    // Max-Age wins over Expires; both are capped at 400 days from now.
    cookie.expires = match (max_age, expires) {
        (Some(delta), _) if delta <= 0 => Some(i64::MIN),
        (Some(delta), _) => Some(now.saturating_add(delta.min(MAX_AGE_SECONDS))),
        (None, Some(time)) => Some(time.min(now.saturating_add(MAX_AGE_SECONDS))),
        (None, None) => None,
    };
    Some(cookie)
}

/// Trims the whitespace RFC 6265 allows around names, values and attributes.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

fn parse_max_age(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Overlong values saturate rather than being ignored.
    let magnitude = digits.parse::<i64>().unwrap_or(i64::MAX);
    Some(if value.starts_with('-') {
        -magnitude
    } else {
        magnitude
    })
}

/// Lowercases a `Domain` attribute and converts it to ASCII (punycode), the
/// way URL hosts are stored.
fn canonicalize_domain(domain: &str) -> Option<String> {
    match Host::parse(domain).ok()? {
        Host::Domain(d) => Some(d),
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(format!("[{ip}]")),
    }
}

fn canonical_host(url: &Url) -> Option<String> {
    url.host_str().map(str::to_ascii_lowercase)
}

fn is_ip(host: &str) -> bool {
    host.starts_with('[') || host.parse::<IpAddr>().is_ok()
}

/// Whether cookies marked `Secure` may be set from and sent to `url`:
/// `https`, or a loopback host (which browsers treat as potentially
/// trustworthy).
fn is_secure(url: &Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    match url.host() {
        Some(Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// RFC 6265 5.1.3: whether `host` domain-matches `domain`.
fn domain_match(host: &str, domain: &str) -> bool {
    if host == domain {
        return true;
    }
    host.len() > domain.len()
        && host.ends_with(domain)
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
        && !is_ip(host)
}

/// RFC 6265 5.1.4: the default cookie path for a request URL.
fn default_path(url: &Url) -> &str {
    let path = url.path();
    if !path.starts_with('/') {
        return "/";
    }
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

/// RFC 6265 5.1.4: whether `request_path` path-matches `cookie_path`.
fn path_match(request_path: &str, cookie_path: &str) -> bool {
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// RFC 6265 5.1.1: parses a cookie date, as Unix time in seconds. The
/// algorithm is deliberately lenient: it picks the time, day, month and year
/// out of whatever tokens are present.
fn parse_cookie_date(input: &str) -> Option<i64> {
    let is_delimiter =
        |b: u8| matches!(b, 0x09 | 0x20..=0x2f | 0x3b..=0x40 | 0x5b..=0x60 | 0x7b..=0x7e);
    let (mut time, mut day, mut month, mut year) = (None, None, None, None);
    for token in input
        .as_bytes()
        .split(|&b| is_delimiter(b))
        .filter(|t| !t.is_empty())
    {
        // Each token fills the first still-missing field it parses as.
        if time.is_none() && parse_time(token).is_some() {
            time = parse_time(token);
        } else if day.is_none() && leading_digits(token, 1, 2).is_some() {
            day = leading_digits(token, 1, 2);
        } else if month.is_none() && parse_month(token).is_some() {
            month = parse_month(token);
        } else if year.is_none() && leading_digits(token, 2, 4).is_some() {
            year = leading_digits(token, 2, 4);
        }
    }
    let ((hour, minute, second), day, month, mut year) = (time?, day?, month?, year?);
    if (70..=99).contains(&year) {
        year += 1900;
    } else if year <= 69 {
        year += 2000;
    }
    if year < 1601 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    Some(days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

/// `min..=max` digits at the start of `token`, followed by nothing or by a
/// non-digit.
fn leading_digits(token: &[u8], min: usize, max: usize) -> Option<u32> {
    let n = token.iter().take_while(|b| b.is_ascii_digit()).count();
    if n < min || n > max {
        return None;
    }
    token[..n]
        .iter()
        .try_fold(0u32, |acc, &b| Some(acc * 10 + u32::from(b - b'0')))
}

/// `hh:mm:ss`, each one or two digits, optionally followed by non-digits.
fn parse_time(token: &[u8]) -> Option<(u32, u32, u32)> {
    let mut parts = token.splitn(3, |&b| b == b':');
    let hour = parts.next()?;
    let minute = parts.next()?;
    let second = parts.next()?;
    let exact = |p: &[u8]| (1..=2).contains(&p.len()) && p.iter().all(u8::is_ascii_digit);
    if !exact(hour) || !exact(minute) {
        return None;
    }
    Some((
        leading_digits(hour, 1, 2)?,
        leading_digits(minute, 1, 2)?,
        leading_digits(second, 1, 2)?,
    ))
}

fn parse_month(token: &[u8]) -> Option<u32> {
    const MONTHS: [&[u8; 3]; 12] = [
        b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov",
        b"dec",
    ];
    let prefix = token.get(..3)?;
    MONTHS
        .iter()
        .position(|m| prefix.eq_ignore_ascii_case(&m[..]))
        .map(|i| i as u32 + 1)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000; // 2023-11-14

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn set(jar: &CookieJar, at: &str, header: &str) -> bool {
        jar.set(&url(at), header, Api::Http, NOW)
    }

    fn get(jar: &CookieJar, at: &str) -> String {
        jar.serialize(&url(at), Api::Http, NOW)
    }

    fn script_get(jar: &CookieJar, at: &str) -> String {
        jar.serialize(&url(at), Api::Script, NOW)
    }

    #[test]
    fn cookie_dates() {
        let d = |s| parse_cookie_date(s);
        // 2015-10-21 07:28:00 UTC
        let t = 1_445_412_480;
        assert_eq!(d("Wed, 21 Oct 2015 07:28:00 GMT"), Some(t));
        assert_eq!(d("Wednesday, 21-Oct-15 07:28:00 GMT"), Some(t));
        assert_eq!(d("Wed Oct 21 07:28:00 2015"), Some(t));
        assert_eq!(d("21 oct 2015 7:28:0"), Some(t));
        assert_eq!(d("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(d("Thu, 01-Jan-1970 00:00:01 GMT"), Some(1));
        assert_eq!(d("Sat, 01 Jan 2000 00:00:00 GMT"), Some(946_684_800));
        assert_eq!(d("Tue, 29 Feb 2000 00:00:00 GMT"), Some(951_782_400));
        assert_eq!(d("Fri, 31 Dec 9999 23:59:59 GMT"), Some(253_402_300_799));
        assert_eq!(d("Mon, 01 Jan 1601 00:00:00 GMT"), Some(-11_644_473_600));
        assert_eq!(d("Wed, 21 Oct 69 07:28:00 GMT").map(|x| x > t), Some(true));
        assert_eq!(d("Thu, 29 Feb 2001 00:00:00 GMT"), None);
        assert_eq!(d("Wed, 32 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(d("Wed, 21 Oct 2015 24:00:00 GMT"), None);
        assert_eq!(d("Wed, 21 Foo 2015 07:28:00 GMT"), None);
        assert_eq!(d("Wed, 21 Oct 1600 07:28:00 GMT"), None);
        assert_eq!(d("Wed, 21 Oct 2015"), None);
        assert_eq!(d(""), None);
        assert_eq!(d("garbage"), None);
    }

    #[test]
    fn parses_set_cookie() {
        let c = parse_set_cookie(
            " sid = abc 123 ; Path=/app; Domain=.Example.COM; Secure; HttpOnly; \
             SameSite=lax; Max-Age=60; unknown=1",
            NOW,
        )
        .unwrap();
        assert_eq!(c.name, "sid");
        assert_eq!(c.value, "abc 123");
        assert_eq!(c.path.as_deref(), Some("/app"));
        assert_eq!(c.domain.as_deref(), Some("example.com"));
        assert!(c.secure && c.http_only);
        assert_eq!(c.same_site, SameSite::Lax);
        assert_eq!(c.expires, Some(NOW + 60));

        let nameless = parse_set_cookie("justavalue", NOW).unwrap();
        assert_eq!(
            (nameless.name.as_str(), nameless.value.as_str()),
            ("", "justavalue")
        );
        let empty_value = parse_set_cookie("a=", NOW).unwrap();
        assert_eq!(
            (empty_value.name.as_str(), empty_value.value.as_str()),
            ("a", "")
        );

        assert_eq!(parse_set_cookie("", NOW), None);
        assert_eq!(parse_set_cookie("=", NOW), None);
        assert_eq!(parse_set_cookie(" ; Path=/", NOW), None);
        assert_eq!(parse_set_cookie("a=b\x01c", NOW), None);
        assert!(parse_set_cookie("a=b\tc", NOW).is_some());
        let huge = format!("a={}", "x".repeat(MAX_COOKIE_SIZE));
        assert_eq!(parse_set_cookie(&huge, NOW), None);
    }

    #[test]
    fn expiry_attributes() {
        let exp = |h| parse_set_cookie(h, NOW).unwrap().expires;
        assert_eq!(exp("a=b"), None);
        assert_eq!(exp("a=b; Max-Age=0"), Some(i64::MIN));
        assert_eq!(exp("a=b; Max-Age=-5"), Some(i64::MIN));
        assert_eq!(exp("a=b; Max-Age=abc"), None);
        assert_eq!(exp("a=b; Max-Age=1e3"), None);
        assert_eq!(exp("a=b; Expires=Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        // Max-Age beats Expires, whichever comes first.
        assert_eq!(
            exp("a=b; Max-Age=10; Expires=Thu, 01 Jan 1970 00:00:00 GMT"),
            Some(NOW + 10)
        );
        assert_eq!(
            exp("a=b; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=10"),
            Some(NOW + 10)
        );
        // Capped at 400 days, and huge values don't overflow.
        assert_eq!(
            exp("a=b; Max-Age=99999999999999999999999"),
            Some(NOW + MAX_AGE_SECONDS)
        );
        assert_eq!(
            exp("a=b; Expires=Fri, 31 Dec 9999 23:59:59 GMT"),
            Some(NOW + MAX_AGE_SECONDS)
        );
        // An unparseable Expires is ignored, leaving a session cookie.
        assert_eq!(exp("a=b; Expires=soon"), None);
    }

    #[test]
    fn round_trip_and_replace() {
        let jar = CookieJar::new();
        assert!(set(&jar, "http://example.com/", "a=1"));
        assert!(set(&jar, "http://example.com/", "b=2"));
        assert_eq!(get(&jar, "http://example.com/"), "a=1; b=2");
        assert!(set(&jar, "http://example.com/", "a=3"));
        // Replacing keeps the original creation order.
        assert_eq!(get(&jar, "http://example.com/x"), "a=3; b=2");
        assert_eq!(jar.len(), 2);
        assert_eq!(get(&jar, "http://other.com/"), "");
        assert_eq!(jar.header_for(&url("http://other.com/")), None);
        jar.clear();
        assert!(jar.is_empty());
    }

    #[test]
    fn deletion_by_expiry() {
        let jar = CookieJar::new();
        set(&jar, "http://example.com/", "a=1");
        set(&jar, "http://example.com/", "b=2; Max-Age=100");
        assert!(set(&jar, "http://example.com/", "a=; Max-Age=0"));
        assert!(set(
            &jar,
            "http://example.com/",
            "b=; Expires=Thu, 01 Jan 1970 00:00:00 GMT"
        ));
        assert_eq!(get(&jar, "http://example.com/"), "");
        assert!(jar.is_empty());
    }

    #[test]
    fn expired_cookies_are_not_sent() {
        let jar = CookieJar::new();
        set(&jar, "http://example.com/", "a=1; Max-Age=10");
        assert_eq!(get(&jar, "http://example.com/"), "a=1");
        assert_eq!(
            jar.serialize(&url("http://example.com/"), Api::Http, NOW + 10),
            ""
        );
        assert!(jar.is_empty());
    }

    #[test]
    fn host_only_and_domain_cookies() {
        let jar = CookieJar::new();
        set(&jar, "http://www.example.com/", "host=1");
        set(&jar, "http://www.example.com/", "dom=2; Domain=example.com");
        assert_eq!(get(&jar, "http://www.example.com/"), "host=1; dom=2");
        assert_eq!(get(&jar, "http://example.com/"), "dom=2");
        assert_eq!(get(&jar, "http://a.b.example.com/"), "dom=2");
        assert_eq!(get(&jar, "http://sub.www.example.com/"), "dom=2");
        assert_eq!(get(&jar, "http://badexample.com/"), "");
        assert_eq!(get(&jar, "http://EXAMPLE.com/"), "dom=2");

        let cookies = jar.cookies();
        assert!(cookies[0].host_only() && cookies[0].domain() == "www.example.com");
        assert!(!cookies[1].host_only() && cookies[1].domain() == "example.com");
    }

    #[test]
    fn refuses_foreign_domains() {
        let jar = CookieJar::new();
        assert!(!set(&jar, "http://example.com/", "a=1; Domain=other.com"));
        assert!(!set(
            &jar,
            "http://example.com/",
            "a=1; Domain=www.example.com"
        ));
        assert!(!set(&jar, "http://example.com/", "a=1; Domain=com"));
        assert!(!set(&jar, "http://example.com/", "a=1; Domain=.com"));
        assert!(!set(&jar, "http://1.2.3.4/", "a=1; Domain=2.3.4"));
        assert!(!set(&jar, "http://example.com/", "a=1; Domain=xample.com"));
        assert!(jar.is_empty());
        // A domain equal to the host is fine, even without a dot or for an IP.
        assert!(set(&jar, "http://localhost/", "a=1; Domain=localhost"));
        assert!(set(&jar, "http://1.2.3.4/", "b=1; Domain=1.2.3.4"));
        assert_eq!(get(&jar, "http://localhost/"), "a=1");
        assert_eq!(get(&jar, "http://1.2.3.4/"), "b=1");
    }

    #[test]
    fn idn_domains() {
        let jar = CookieJar::new();
        assert!(set(
            &jar,
            "http://www.bücher.example/",
            "a=1; Domain=BÜCHER.example"
        ));
        assert_eq!(jar.cookies()[0].domain(), "xn--bcher-kva.example");
        assert_eq!(get(&jar, "http://xn--bcher-kva.example/"), "a=1");
    }

    #[test]
    fn paths() {
        let jar = CookieJar::new();
        set(&jar, "http://example.com/", "root=1");
        set(&jar, "http://example.com/", "app=1; Path=/app");
        set(&jar, "http://example.com/", "slash=1; Path=/app/");
        set(&jar, "http://example.com/docs/page.html", "default=1");
        set(&jar, "http://example.com/", "bad=1; Path=relative");
        assert_eq!(get(&jar, "http://example.com/"), "root=1; bad=1");
        assert_eq!(get(&jar, "http://example.com/app"), "app=1; root=1; bad=1");
        assert_eq!(
            get(&jar, "http://example.com/app/x"),
            "slash=1; app=1; root=1; bad=1"
        );
        assert_eq!(get(&jar, "http://example.com/application"), "root=1; bad=1");
        assert_eq!(
            get(&jar, "http://example.com/docs/x"),
            "default=1; root=1; bad=1"
        );
        assert_eq!(
            get(&jar, "http://example.com/docs"),
            "default=1; root=1; bad=1"
        );
        assert_eq!(get(&jar, "http://example.com/docsx"), "root=1; bad=1");
    }

    #[test]
    fn default_paths() {
        assert_eq!(default_path(&url("http://e.com")), "/");
        assert_eq!(default_path(&url("http://e.com/")), "/");
        assert_eq!(default_path(&url("http://e.com/a")), "/");
        assert_eq!(default_path(&url("http://e.com/a/")), "/a");
        assert_eq!(default_path(&url("http://e.com/a/b?q=/x")), "/a");
    }

    #[test]
    fn same_name_different_paths_are_distinct() {
        let jar = CookieJar::new();
        set(&jar, "http://example.com/", "a=root");
        set(&jar, "http://example.com/", "a=deep; Path=/x");
        assert_eq!(get(&jar, "http://example.com/x/y"), "a=deep; a=root");
        assert_eq!(jar.len(), 2);
    }

    #[test]
    fn secure_cookies() {
        let jar = CookieJar::new();
        assert!(!set(&jar, "http://example.com/", "a=1; Secure"));
        assert!(set(&jar, "https://example.com/", "a=1; Secure"));
        assert_eq!(get(&jar, "http://example.com/"), "");
        assert_eq!(get(&jar, "https://example.com/"), "a=1");
        // An insecure origin can't shadow or overwrite a secure cookie...
        assert!(!set(&jar, "http://example.com/", "a=2"));
        assert!(!set(
            &jar,
            "http://www.example.com/",
            "a=2; Domain=example.com"
        ));
        // ...but a secure one can.
        assert!(set(&jar, "https://example.com/", "a=3"));
        assert_eq!(get(&jar, "http://example.com/"), "a=3");

        // Loopback counts as secure, like in browsers.
        assert!(set(&jar, "http://127.0.0.1:8080/", "s=1; Secure"));
        assert_eq!(get(&jar, "http://127.0.0.1:9090/"), "s=1");
        assert!(set(&jar, "http://localhost/", "s=1; Secure"));
        assert!(set(&jar, "http://[::1]/", "s=1; Secure"));
    }

    #[test]
    fn prefixes() {
        let jar = CookieJar::new();
        let https = "https://example.com/";
        assert!(!set(&jar, https, "__Secure-a=1"));
        assert!(set(&jar, https, "__Secure-a=1; Secure"));
        assert!(!set(&jar, https, "__secure-b=1"));
        assert!(!set(&jar, https, "__Host-a=1; Secure; Path=/x"));
        assert!(!set(
            &jar,
            https,
            "__Host-a=1; Secure; Path=/; Domain=example.com"
        ));
        assert!(!set(&jar, https, "__Host-a=1; Path=/"));
        assert!(set(&jar, https, "__Host-a=1; Secure; Path=/"));
        assert!(!set(&jar, https, "__Host-x"));
        assert_eq!(get(&jar, https), "__Secure-a=1; __Host-a=1");
    }

    #[test]
    fn http_only_is_hidden_from_scripts() {
        let jar = CookieJar::new();
        let page = url("http://example.com/");
        set(&jar, "http://example.com/", "session=s; HttpOnly");
        set(&jar, "http://example.com/", "theme=dark");
        assert_eq!(script_get(&jar, "http://example.com/"), "theme=dark");
        assert_eq!(get(&jar, "http://example.com/"), "session=s; theme=dark");

        // Scripts can neither create nor overwrite HttpOnly cookies.
        assert!(!jar.set(&page, "x=1; HttpOnly", Api::Script, NOW));
        assert!(!jar.set(&page, "session=hacked", Api::Script, NOW));
        assert!(jar.set(&page, "theme=light", Api::Script, NOW));
        assert_eq!(get(&jar, "http://example.com/"), "session=s; theme=light");
    }

    #[test]
    fn public_api_uses_the_clock() {
        let jar = CookieJar::new();
        let page = url("https://example.com/a/b");
        assert!(jar.set_from_header(&page, "h=1; HttpOnly; Max-Age=3600"));
        assert!(jar.set_from_script(&page, "s=2; Path=/"));
        assert_eq!(jar.header_for(&page).as_deref(), Some("h=1; s=2"));
        assert_eq!(jar.script_cookies_for(&page), "s=2");
        let cookies = jar.cookies();
        assert!(cookies[0].expires().unwrap() > SystemTime::now());
        assert_eq!(cookies[0].path(), "/a");
        assert_eq!(cookies[1].expires(), None);
    }

    #[test]
    fn non_http_urls_are_ignored() {
        let jar = CookieJar::new();
        assert!(!jar.set(&url("data:text/plain,x"), "a=1", Api::Http, NOW));
        assert_eq!(jar.serialize(&url("data:text/plain,x"), Api::Http, NOW), "");
    }

    #[test]
    fn bounded_size() {
        let jar = CookieJar::new();
        for i in 0..MAX_COOKIES_PER_DOMAIN + 10 {
            set(&jar, "http://example.com/", &format!("c{i}=1"));
        }
        assert_eq!(jar.len(), MAX_COOKIES_PER_DOMAIN);
        // The oldest were evicted.
        let header = get(&jar, "http://example.com/");
        assert!(!header.starts_with("c0=1"));
        assert!(header.starts_with("c10=1"));

        let jar = CookieJar::new();
        for d in 0..MAX_COOKIES / 100 + 1 {
            for i in 0..100 {
                set(&jar, &format!("http://d{d}.example/"), &format!("c{i}=1"));
            }
        }
        assert_eq!(jar.len(), MAX_COOKIES);
        assert_eq!(get(&jar, "http://d0.example/"), "");
    }

    #[test]
    fn domain_matching() {
        assert!(domain_match("example.com", "example.com"));
        assert!(domain_match("a.example.com", "example.com"));
        assert!(!domain_match("aexample.com", "example.com"));
        assert!(!domain_match("example.com", "a.example.com"));
        assert!(!domain_match("1.2.3.4", "2.3.4"));
    }
}
