//! Recording responses to disk and replaying them, for benchmarks.
//!
//! With `DASHER_RECORD=<dir>` set, every final response a session receives
//! is read whole and written to `<dir>`. With `DASHER_REPLAY=<dir>` set, no
//! request leaves the process: each is answered from `<dir>`, and one that
//! was never recorded fails like a refused connection. Replaying makes the
//! work a real page asks for repeatable, so two builds can be compared on
//! the same input without the network or the site changing between runs.
//!
//! With `DASHER_REPLAY_LOG` also set, every request a replay answers or
//! refuses is printed to stderr as `replay: <hit|miss> <method> <url>`, to
//! count the requests a page makes.
//!
//! Requests are matched by method and URL. Pages build some URLs from the
//! clock or a random number (cache busters), so a request that has no exact
//! match falls back to a recorded one with the same method, host and path.

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bytes::Bytes;
use hyper::header::{HeaderName, HeaderValue};
use hyper::{HeaderMap, Method, StatusCode};
use url::Url;

/// What the process was asked to do, from the environment.
pub(crate) enum Mode {
    Record(PathBuf),
    Replay(Store),
}

impl Mode {
    pub(crate) fn from_env() -> Option<Self> {
        if let Some(dir) = std::env::var_os("DASHER_REPLAY") {
            return Some(Self::Replay(Store::load(Path::new(&dir))));
        }
        let dir = PathBuf::from(std::env::var_os("DASHER_RECORD")?);
        let _ = fs::create_dir_all(&dir);
        Some(Self::Record(dir))
    }
}

/// A recorded response.
pub(crate) struct Recorded {
    pub(crate) status: StatusCode,
    pub(crate) url: Url,
    pub(crate) redirected: bool,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

/// The responses in a recording directory, indexed for lookup.
pub(crate) struct Store {
    exact: HashMap<String, PathBuf>,
    by_path: HashMap<String, PathBuf>,
    /// Requests that found nothing, for the end-of-run report.
    misses: Mutex<Vec<String>>,
}

fn exact_key(method: &Method, url: &Url) -> String {
    format!("{method} {url}")
}

fn path_key(method: &Method, url: &Url) -> String {
    format!(
        "{method} {}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.path()
    )
}

fn file_stem(key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl Store {
    fn load(dir: &Path) -> Self {
        let mut exact = HashMap::new();
        let mut by_path = HashMap::new();
        let mut metas: Vec<PathBuf> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "meta"))
                    .collect()
            })
            .unwrap_or_default();
        metas.sort();
        for meta in metas {
            let Ok(text) = fs::read_to_string(&meta) else {
                continue;
            };
            let mut lines = text.lines();
            let (Some(method), Some(request_url)) = (lines.next(), lines.next()) else {
                continue;
            };
            let (Ok(method), Ok(url)) = (method.parse::<Method>(), Url::parse(request_url)) else {
                continue;
            };
            by_path
                .entry(path_key(&method, &url))
                .or_insert_with(|| meta.clone());
            exact.insert(exact_key(&method, &url), meta);
        }
        Self {
            exact,
            by_path,
            misses: Mutex::new(Vec::new()),
        }
    }

    /// The recorded response for a request, if there is one.
    ///
    /// The fragment is never sent, so it plays no part in the match, and the
    /// response URL keeps the request's fragment as a live response does.
    pub(crate) fn find(&self, method: &Method, url: &Url) -> Option<Recorded> {
        let mut sent = url.clone();
        sent.set_fragment(None);
        let meta = self
            .exact
            .get(&exact_key(method, &sent))
            .or_else(|| self.by_path.get(&path_key(method, &sent)));
        let found = meta.and_then(|meta| read(meta)).map(|mut found| {
            if found.url.fragment().is_none() {
                found.url.set_fragment(url.fragment());
            }
            found
        });
        if std::env::var_os("DASHER_REPLAY_LOG").is_some() {
            let what = if found.is_some() { "hit" } else { "miss" };
            eprintln!("replay: {what} {method} {url}");
        }
        if found.is_none() && std::env::var_os("DASHER_REPLAY_MISSES").is_some() {
            if let Ok(mut misses) = self.misses.lock() {
                misses.push(exact_key(method, url));
                eprintln!("replay miss: {method} {url}");
            }
        }
        found
    }
}

fn read(meta: &Path) -> Option<Recorded> {
    let text = fs::read_to_string(meta).ok()?;
    let mut lines = text.lines();
    let _method = lines.next()?;
    let _request_url = lines.next()?;
    let status = StatusCode::from_u16(lines.next()?.parse().ok()?).ok()?;
    let url = Url::parse(lines.next()?).ok()?;
    let redirected = lines.next()? == "1";
    let mut headers = HeaderMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(": ") else {
            continue;
        };
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.append(name, value);
        }
    }
    let body = fs::read(meta.with_extension("body")).ok()?;
    Some(Recorded {
        status,
        url,
        redirected,
        headers,
        body: Bytes::from(body),
    })
}

/// Writes one response to the recording directory.
pub(crate) fn write(dir: &Path, method: &Method, request_url: &Url, recorded: &Recorded) {
    let mut request_url = request_url.clone();
    request_url.set_fragment(None);
    let request_url = &request_url;
    let stem = file_stem(&exact_key(method, request_url));
    let mut meta = format!(
        "{method}\n{request_url}\n{}\n{}\n{}\n",
        recorded.status.as_u16(),
        recorded.url,
        u8::from(recorded.redirected)
    );
    for (name, value) in &recorded.headers {
        // The stored body is already decoded and complete.
        if matches!(
            name.as_str(),
            "content-encoding" | "content-length" | "transfer-encoding"
        ) {
            continue;
        }
        if let Ok(value) = value.to_str() {
            if !value.contains('\n') {
                meta.push_str(&format!("{name}: {value}\n"));
            }
        }
    }
    let _ = fs::write(dir.join(format!("{stem}.body")), &recorded.body);
    let _ = fs::write(dir.join(format!("{stem}.meta")), meta);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fragment_is_not_matched_and_stays_on_the_response_url() {
        let dir = std::env::temp_dir().join(format!("dasher-replay-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let page = Url::parse("https://example.com/a?x=1").unwrap();
        let recorded = Recorded {
            status: StatusCode::OK,
            url: page.clone(),
            redirected: false,
            headers: HeaderMap::new(),
            body: Bytes::from_static(b"<p>hi"),
        };
        // Recorded while the page's URL had a fragment.
        let with_fragment = Url::parse("https://example.com/a?x=1#old").unwrap();
        write(&dir, &Method::GET, &with_fragment, &recorded);
        let store = Store::load(&dir);

        let reload = Url::parse("https://example.com/a?x=1#sv1rc1").unwrap();
        let found = store
            .find(&Method::GET, &reload)
            .expect("found despite the fragment");
        assert_eq!(found.url.as_str(), "https://example.com/a?x=1#sv1rc1");
        let found = store.find(&Method::GET, &page).unwrap();
        assert_eq!(found.url.as_str(), "https://example.com/a?x=1");
        let _ = fs::remove_dir_all(&dir);
    }
}
