//! An in-process HTTP fixture server for dasher's tests.
//!
//! Page-level behaviour is tested against local HTML/JS fixtures, never live
//! websites. [`FixtureServer`] serves files from a fixtures directory over
//! plain HTTP/1.1 on `127.0.0.1`, can delay responses to simulate a slow
//! network, records every request it receives so tests can count them, and
//! can answer chosen paths from a closure (JSON APIs, redirects, odd status
//! codes).
//!
//! The server runs on its own thread with its own Tokio runtime, so it works
//! the same from plain `#[test]` functions and from async tests on any
//! runtime flavour. It shuts down when the [`FixtureServer`] is dropped.
//!
//! ```
//! use std::io::{Read, Write};
//! use std::net::TcpStream;
//! use std::time::Duration;
//!
//! use dasher_test_server::{FixtureResponse, FixtureServer};
//!
//! let server = FixtureServer::builder()
//!     .root(dasher_test_server::fixtures_dir!())
//!     .latency(Duration::from_millis(5))
//!     .route("/api/items", |_req| FixtureResponse::json(r#"["a","b"]"#))
//!     .start()
//!     .unwrap();
//!
//! let mut stream = TcpStream::connect(server.addr()).unwrap();
//! stream
//!     .write_all(b"GET /api/items HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
//!     .unwrap();
//! let mut response = String::new();
//! stream.read_to_string(&mut response).unwrap();
//!
//! assert!(response.ends_with(r#"["a","b"]"#));
//! assert_eq!(server.request_count_for("/api/items"), 1);
//! assert_eq!(server.request_count_for("/missing.png"), 0);
//! ```

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Expands to the calling crate's `tests/fixtures` directory as a `&Path`.
///
/// This is the conventional place for a crate's fixture files.
#[macro_export]
macro_rules! fixtures_dir {
    () => {
        ::std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
    };
}

/// A request as received by the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    /// The request method, e.g. `GET`.
    pub method: String,
    /// The URL path, still percent-encoded, without the query string.
    pub path: String,
    /// The query string without the leading `?`, if there was one.
    pub query: Option<String>,
    /// Header names (lowercase) and values, in the order received.
    pub headers: Vec<(String, String)>,
    /// The full request body.
    pub body: Bytes,
}

impl RecordedRequest {
    /// The value of the first header called `name`, compared
    /// case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A response produced by a route handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Response headers. `Content-Length` is added automatically.
    pub headers: Vec<(String, String)>,
    /// The response body.
    pub body: Bytes,
}

impl FixtureResponse {
    /// An empty response with the given status.
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Bytes::new(),
        }
    }

    /// A `200 OK` response with the given content type and body.
    pub fn ok(content_type: &str, body: impl Into<Bytes>) -> Self {
        Self::new(200)
            .with_header("content-type", content_type)
            .with_body(body)
    }

    /// A `200 OK` plain-text response.
    pub fn text(body: impl Into<Bytes>) -> Self {
        Self::ok("text/plain; charset=utf-8", body)
    }

    /// A `200 OK` HTML response.
    pub fn html(body: impl Into<Bytes>) -> Self {
        Self::ok("text/html; charset=utf-8", body)
    }

    /// A `200 OK` JSON response.
    pub fn json(body: impl Into<Bytes>) -> Self {
        Self::ok("application/json", body)
    }

    /// A redirect with the given status (301, 302, 303, 307 or 308) to
    /// `location`.
    pub fn redirect(status: u16, location: &str) -> Self {
        Self::new(status).with_header("location", location)
    }

    /// A `404 Not Found` response.
    pub fn not_found() -> Self {
        Self::new(404)
            .with_header("content-type", "text/plain; charset=utf-8")
            .with_body("not found")
    }

    /// Replaces the status code.
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Adds a header. Repeat the call to send a header more than once.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Replaces the body.
    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }

    fn into_hyper(self) -> Response<Full<Bytes>> {
        let Ok(status) = StatusCode::from_u16(self.status) else {
            return internal_error(&format!("invalid fixture status {}", self.status));
        };
        let mut response = Response::new(Full::new(self.body));
        *response.status_mut() = status;
        for (name, value) in &self.headers {
            let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                return internal_error(&format!("invalid fixture header {name}: {value}"));
            };
            response.headers_mut().append(name, value);
        }
        response
    }
}

fn internal_error(message: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(message.to_owned())));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response
}

type Handler = Arc<dyn Fn(&RecordedRequest) -> FixtureResponse + Send + Sync>;

/// Configures and starts a [`FixtureServer`].
#[derive(Default)]
pub struct FixtureServerBuilder {
    root: Option<PathBuf>,
    latency: Duration,
    path_latency: HashMap<String, Duration>,
    routes: HashMap<String, Handler>,
}

impl fmt::Debug for FixtureServerBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FixtureServerBuilder")
            .field("root", &self.root)
            .field("latency", &self.latency)
            .field("path_latency", &self.path_latency)
            .field("routes", &self.routes.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl FixtureServerBuilder {
    /// Serves files from `dir` for `GET` and `HEAD` requests. A request for a
    /// directory serves its `index.html`. Without a root, only routes answer.
    pub fn root(mut self, dir: impl Into<PathBuf>) -> Self {
        self.root = Some(dir.into());
        self
    }

    /// Delays every response by `delay`. The request is recorded as soon as
    /// it arrives, before the delay.
    pub fn latency(mut self, delay: Duration) -> Self {
        self.latency = delay;
        self
    }

    /// Delays responses for exactly `path` by `delay`, overriding
    /// [`latency`](Self::latency) for that path.
    pub fn path_latency(mut self, path: &str, delay: Duration) -> Self {
        self.path_latency.insert(path.to_owned(), delay);
        self
    }

    /// Answers requests for exactly `path` (any method, any query string)
    /// with `handler` instead of the file system.
    pub fn route<F>(mut self, path: &str, handler: F) -> Self
    where
        F: Fn(&RecordedRequest) -> FixtureResponse + Send + Sync + 'static,
    {
        self.routes.insert(path.to_owned(), Arc::new(handler));
        self
    }

    /// Binds to an ephemeral port on `127.0.0.1` and starts serving.
    pub fn start(self) -> io::Result<FixtureServer> {
        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;

        let shared = Arc::new(Shared {
            root: self.root,
            latency: self.latency,
            path_latency: self.path_latency,
            routes: self.routes,
            log: Mutex::new(Vec::new()),
        });
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = mpsc::channel();

        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name(format!("fixture-server-{}", addr.port()))
            .spawn(move || {
                // The runtime is created, used and dropped on this thread
                // only, so the server never interacts with the caller's
                // runtime (if any).
                let setup = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .and_then(|runtime| {
                        let listener = {
                            let _guard = runtime.enter();
                            TcpListener::from_std(listener)?
                        };
                        Ok((runtime, listener))
                    });
                let (runtime, listener) = match setup {
                    Ok(ok) => {
                        let _ = ready_tx.send(Ok(()));
                        ok
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                runtime.block_on(serve(listener, thread_shared, shutdown_rx));
            })?;

        ready_rx
            .recv()
            .map_err(|_| io::Error::other("fixture server thread exited during startup"))??;

        Ok(FixtureServer {
            addr,
            shared,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }
}

/// A running in-process HTTP server for tests. Stops when dropped.
pub struct FixtureServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for FixtureServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FixtureServer")
            .field("addr", &self.addr)
            .field("root", &self.shared.root)
            .finish_non_exhaustive()
    }
}

impl FixtureServer {
    /// Starts configuring a server.
    pub fn builder() -> FixtureServerBuilder {
        FixtureServerBuilder::default()
    }

    /// Starts a server that serves files from `dir` with no added latency.
    pub fn serve(dir: impl Into<PathBuf>) -> io::Result<Self> {
        Self::builder().root(dir).start()
    }

    /// The socket address the server listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The server's origin, e.g. `http://127.0.0.1:41234`.
    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// An absolute URL for `path` on this server. A missing leading `/` is
    /// added.
    pub fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("http://{}{path}", self.addr)
        } else {
            format!("http://{}/{path}", self.addr)
        }
    }

    /// The number of requests received so far.
    pub fn request_count(&self) -> usize {
        self.shared.log().len()
    }

    /// The number of requests received so far for exactly `path` (the query
    /// string is ignored).
    pub fn request_count_for(&self, path: &str) -> usize {
        self.shared.log().iter().filter(|r| r.path == path).count()
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.log().clone()
    }

    /// Forgets all recorded requests, resetting the counts to zero.
    pub fn clear_requests(&self) {
        self.shared.log().clear();
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            // A panic on the server thread has already been reported; don't
            // turn it into a double panic here.
            let _ = thread.join();
        }
    }
}

struct Shared {
    root: Option<PathBuf>,
    latency: Duration,
    path_latency: HashMap<String, Duration>,
    routes: HashMap<String, Handler>,
    log: Mutex<Vec<RecordedRequest>>,
}

impl Shared {
    fn log(&self) -> MutexGuard<'_, Vec<RecordedRequest>> {
        // A panicking route handler never holds this lock, but don't let a
        // poisoned lock cascade into unrelated assertions either way.
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn respond(&self, request: &RecordedRequest) -> FixtureResponse {
        if let Some(handler) = self.routes.get(&request.path) {
            return handler(request);
        }
        let Some(root) = &self.root else {
            return FixtureResponse::not_found();
        };
        if request.method != Method::GET.as_str() && request.method != Method::HEAD.as_str() {
            return FixtureResponse::new(405).with_header("allow", "GET, HEAD");
        }
        let Some(file) = resolve(root, &request.path) else {
            return FixtureResponse::not_found();
        };
        match tokio::fs::read(&file).await {
            Ok(contents) => FixtureResponse::ok(content_type(&file), contents),
            Err(_) => FixtureResponse::not_found(),
        }
    }
}

async fn serve(listener: TcpListener, shared: Arc<Shared>, mut shutdown: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    let service = service_fn(move |req| handle(Arc::clone(&shared), req));
                    // Connection errors (client hung up, malformed request)
                    // are the client's problem; nothing to report.
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        }
    }
    // Returning drops the runtime, which cancels in-flight connections.
}

async fn handle(
    shared: Arc<Shared>,
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => Bytes::new(),
    };
    let request = RecordedRequest {
        method: parts.method.as_str().to_owned(),
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().map(str::to_owned),
        headers: parts
            .headers
            .iter()
            .map(|(name, value)| {
                let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                (name.as_str().to_owned(), value)
            })
            .collect(),
        body,
    };
    shared.log().push(request.clone());

    let delay = shared
        .path_latency
        .get(&request.path)
        .copied()
        .unwrap_or(shared.latency);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    Ok(shared.respond(&request).await.into_hyper())
}

/// Maps a URL path onto a file under `root`, refusing anything that could
/// escape it.
fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(url_path)?;
    let mut path = root.to_path_buf();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => return None,
            s if s.contains(['\\', ':', '\0']) => return None,
            s => path.push(s),
        }
    }
    if path.is_dir() {
        path.push("index.html");
    }
    Some(path)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs" | "cjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("css") => "text/css; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("/a%20b").as_deref(), Some("/a b"));
        assert_eq!(percent_decode("/%E2%9C%93").as_deref(), Some("/\u{2713}"));
        assert_eq!(percent_decode("/plain").as_deref(), Some("/plain"));
        assert_eq!(percent_decode("/bad%2"), None);
        assert_eq!(percent_decode("/bad%zz"), None);
        assert_eq!(percent_decode("/%ff"), None);
    }

    #[test]
    fn resolve_stays_inside_root() {
        let root = Path::new("/srv/fixtures");
        assert_eq!(
            resolve(root, "/a/b.html"),
            Some(PathBuf::from("/srv/fixtures/a/b.html"))
        );
        assert_eq!(
            resolve(root, "//a/./b.html"),
            Some(PathBuf::from("/srv/fixtures/a/b.html"))
        );
        assert_eq!(resolve(root, "/../etc/passwd"), None);
        assert_eq!(resolve(root, "/a/%2e%2e/%2e%2e/etc/passwd"), None);
        assert_eq!(resolve(root, "/a%5c..%5csecret"), None);
        assert_eq!(resolve(root, "/c:/windows"), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("x.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("x.mjs")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x.png")), "image/png");
        assert_eq!(content_type(Path::new("x")), "application/octet-stream");
    }

    #[test]
    fn url_building() {
        let server = FixtureServer::builder().start().unwrap();
        let origin = server.origin();
        assert!(origin.starts_with("http://127.0.0.1:"));
        assert_eq!(server.url("/a.html"), format!("{origin}/a.html"));
        assert_eq!(server.url("a.html"), format!("{origin}/a.html"));
    }

    #[test]
    fn response_builders() {
        let r = FixtureResponse::redirect(302, "/next").with_header("x-a", "1");
        assert_eq!(r.status, 302);
        assert_eq!(
            r.headers,
            vec![
                ("location".to_owned(), "/next".to_owned()),
                ("x-a".to_owned(), "1".to_owned())
            ]
        );
        let r = FixtureResponse::json("{}").with_status(201);
        assert_eq!(r.status, 201);
        assert_eq!(r.body, Bytes::from_static(b"{}"));
    }

    #[test]
    fn invalid_response_parts_become_500() {
        let r = FixtureResponse::new(42).into_hyper();
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let r = FixtureResponse::new(200)
            .with_header("bad header", "x")
            .into_hyper();
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
