//! The host side of `fetch()` and `XMLHttpRequest`: sending requests through
//! the page's [`Session`] and delivering the results back to the JS glue in
//! `fetch.js`.
//!
//! A request is started from JS with `native.netSend(request)`, which returns
//! an id. The request runs as a Tokio task off the JS thread, registered as a
//! [`PendingOperation`] so the event loop knows a result is coming. It posts
//! up to two tasks back: the response head once headers arrive
//! (`onResponse`), then the whole body (`onBody`), or a failure (`onError`)
//! at either point. Every request goes through the session, so the request
//! policy, cookies and the session's network-idle tracking all apply.
//!
//! `data:` URLs are answered on the JS thread without touching the network.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dasher_net::header::{HeaderName, HeaderValue, ORIGIN};
use dasher_net::url::Origin;
use dasher_net::{
    CookieJar, Credentials, HeaderMap, Method, RedirectMode, Request, ResourceType, Response,
    Session, Url,
};
use rquickjs::{ArrayBuffer, Ctx, Exception, Function, JsLifetime, Object, TypedArray};
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use crate::console::{ConsoleLevel, ConsoleMessage};
use crate::data_url;
use crate::remote::{PendingOperation, TaskSender};
use crate::runtime::Shared;

/// The network side of a runtime: the session requests go through, and the
/// requests in flight.
pub(crate) struct NetState {
    session: Option<Session>,
    /// The cookies `document.cookie` reads and writes: the session's, or a
    /// jar of the page's own when there is no session.
    cookies: Arc<CookieJar>,
    /// Where requests are spawned when they are not started from inside a
    /// Tokio runtime.
    runtime: Option<Handle>,
    last_id: Cell<u32>,
    in_flight: RefCell<HashMap<u32, AbortHandle>>,
}

impl NetState {
    pub(crate) fn new(session: Option<Session>) -> Self {
        let cookies = session
            .as_ref()
            .map_or_else(Arc::default, |s| Arc::clone(s.cookies()));
        Self {
            session,
            cookies,
            runtime: Handle::try_current().ok(),
            last_id: Cell::new(0),
            in_flight: RefCell::default(),
        }
    }

    /// The cookie jar behind `document.cookie`.
    pub(crate) fn cookies(&self) -> &Arc<CookieJar> {
        &self.cookies
    }

    /// The session requests go through, if the page has network access.
    pub(crate) fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// The Tokio runtime to spawn a request on: the current one, or else
    /// the one current when the runtime was created.
    pub(crate) fn spawn_handle(&self) -> Option<Handle> {
        Handle::try_current().ok().or_else(|| self.runtime.clone())
    }

    fn next_id(&self) -> u32 {
        let id = self.last_id.get().wrapping_add(1).max(1);
        self.last_id.set(id);
        id
    }

    /// Cancels the network request `id`, if it is still running.
    fn abort(&self, id: u32) {
        if let Some(handle) = self.in_flight.borrow_mut().remove(&id) {
            handle.abort();
        }
    }

    /// Cancels every request in flight, for when the page goes away.
    pub(crate) fn abort_all(&self) {
        for (_, handle) in self.in_flight.borrow_mut().drain() {
            handle.abort();
        }
    }
}

/// The JS callbacks results are delivered to, registered by `fetch.js`.
#[derive(Clone)]
struct NetHooks<'js> {
    on_response: Function<'js>,
    on_body: Function<'js>,
    on_error: Function<'js>,
    shared: Rc<Shared>,
}

// SAFETY: `NetHooks` only holds values bound to `'js` (and an `Rc` with no
// lifetime), and `Changed` is the same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for NetHooks<'js> {
    type Changed<'to> = NetHooks<'to>;
}

/// A response's status line and headers, as handed to JS.
#[derive(Debug, Clone)]
struct Head {
    status: u16,
    status_text: &'static str,
    url: String,
    redirected: bool,
    /// `"basic"` for same-origin responses, `"cors"` for others.
    response_type: &'static str,
    /// Lowercase names and values, flattened.
    headers: Vec<String>,
}

impl Head {
    fn from_response(response: &Response, origin: &Origin) -> Self {
        Self::new(
            response.status().as_u16(),
            response.url().clone(),
            response.redirected(),
            response.headers(),
            origin,
        )
    }

    fn new(status: u16, url: Url, redirected: bool, headers: &HeaderMap, origin: &Origin) -> Self {
        let mut flat = Vec::with_capacity(headers.len() * 2);
        for (name, value) in headers {
            // Forbidden response header names: never exposed to scripts.
            if name == "set-cookie" || name == "set-cookie2" {
                continue;
            }
            flat.push(name.as_str().to_owned());
            // Header values are byte strings; scripts see them as Latin-1.
            flat.push(value.as_bytes().iter().map(|&b| char::from(b)).collect());
        }
        let status_text = dasher_net::StatusCode::from_u16(status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("");
        let response_type = if url.origin() == *origin {
            "basic"
        } else {
            "cors"
        };
        Self {
            status,
            status_text,
            url: url.into(),
            redirected,
            response_type,
            headers: flat,
        }
    }

    fn to_object<'js>(&self, ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
        let head = Object::new(ctx.clone())?;
        head.set("status", self.status)?;
        head.set("statusText", self.status_text)?;
        head.set("url", self.url.as_str())?;
        head.set("redirected", self.redirected)?;
        head.set("type", self.response_type)?;
        head.set("headers", self.headers.clone())?;
        Ok(head)
    }
}

/// Why a request failed, as handed to JS.
#[derive(Debug, Clone)]
struct Failure {
    /// `"timeout"` when the request's timeout ran out, else `"network"`.
    kind: &'static str,
    message: String,
}

impl Failure {
    fn network(message: impl fmt::Display) -> Self {
        Self {
            kind: "network",
            message: message.to_string(),
        }
    }

    fn from_net(error: &dasher_net::Error) -> Self {
        Self {
            kind: if error.is_timeout() {
                "timeout"
            } else {
                "network"
            },
            message: error.to_string(),
        }
    }
}

/// A request read from JS, ready to send.
struct Outgoing {
    request: Request,
    /// The origin of the document making the request.
    origin: Origin,
}

/// Reads the request object `fetch.js` passes to `netSend`.
///
/// Fields: `method`, `url` (absolute), `headers` (flattened name/value
/// pairs), `body` (a `Uint8Array` or absent), `credentials` (`"include"`,
/// `"same-origin"` or `"omit"`), `redirect` (`"follow"`, `"error"` or
/// `"manual"`), `kind` (`"fetch"` or `"xhr"`), `timeout` (milliseconds, 0
/// for none), `referrer` (a URL, or empty for none) and `document` (the
/// document's URL).
fn read_request<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> rquickjs::Result<Outgoing> {
    let type_error = |message: String| Exception::throw_type(ctx, &message);

    let method: String = object.get("method")?;
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|_| type_error(format!("'{method}' is not a valid HTTP method.")))?;
    let url: String = object.get("url")?;
    let url = Url::parse(&url).map_err(|_| type_error(format!("Invalid URL '{url}'")))?;
    let kind = match object.get::<_, String>("kind")?.as_str() {
        "xhr" => ResourceType::Xhr,
        _ => ResourceType::Fetch,
    };
    let document: String = object.get("document")?;
    let document = Url::parse(&document).ok();
    let origin = document.as_ref().map_or_else(
        || Url::parse("about:blank").expect("valid URL").origin(),
        Url::origin,
    );

    let mut headers = HeaderMap::new();
    let flat: Vec<String> = object.get("headers")?;
    for pair in flat.chunks_exact(2) {
        let name = HeaderName::from_bytes(pair[0].as_bytes())
            .map_err(|_| type_error(format!("'{}' is not a valid HTTP header name.", pair[0])))?;
        let value = HeaderValue::from_bytes(&latin1_bytes(&pair[1]))
            .map_err(|_| type_error(format!("'{}' is not a valid HTTP header value.", pair[1])))?;
        headers.append(name, value);
    }
    // Browsers send the requesting origin on cross-origin requests and on
    // any request that may have side effects; servers check it against
    // CSRF.
    let unsafe_method = method != Method::GET && method != Method::HEAD;
    if (unsafe_method || url.origin() != origin)
        && !headers.contains_key(ORIGIN)
        && let Ok(value) = HeaderValue::try_from(origin.ascii_serialization())
    {
        headers.insert(ORIGIN, value);
    }

    let mut request = Request::new(method, url, kind);
    request.headers = headers;
    if let Some(body) = object.get::<_, Option<TypedArray<'js, u8>>>("body")? {
        request.body = Bytes::copy_from_slice(body.as_bytes().unwrap_or_default());
    }
    request.credentials = match object.get::<_, String>("credentials")?.as_str() {
        "include" => Credentials::Include,
        "omit" => Credentials::Omit,
        _ => Credentials::SameOrigin(origin.clone()),
    };
    request.redirect = match object.get::<_, String>("redirect")?.as_str() {
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        _ => RedirectMode::Follow,
    };
    let timeout: f64 = object.get::<_, Option<f64>>("timeout")?.unwrap_or(0.0);
    if timeout.is_finite() && timeout > 0.0 {
        request.timeout = Some(Duration::from_secs_f64(timeout / 1000.0));
    }
    let referrer: Option<String> = object.get("referrer")?;
    request.referrer = referrer.as_deref().and_then(|r| Url::parse(r).ok());
    Ok(Outgoing { request, origin })
}

/// The bytes of a JS string holding a byte string (every char below 256).
/// Characters above that, which header validation in JS rules out, are
/// sent as UTF-8.
fn latin1_bytes(s: &str) -> Vec<u8> {
    if s.chars().all(|c| (c as u32) < 256) {
        s.chars().map(|c| c as u8).collect()
    } else {
        s.as_bytes().to_vec()
    }
}

/// Answers a `data:` URL request.
fn data_response(outgoing: &Outgoing) -> Result<(Head, Bytes), Failure> {
    let request = &outgoing.request;
    if request.method != Method::GET {
        return Err(Failure::network("data: URLs only support GET"));
    }
    let data = data_url::parse(request.url.as_str())
        .ok_or_else(|| Failure::network(format!("invalid data: URL {}", request.url)))?;
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::try_from(data.mime_type) {
        headers.insert(dasher_net::header::CONTENT_TYPE, value);
    }
    let mut url = request.url.clone();
    url.set_fragment(None);
    // Data URLs are same-origin with every document for this purpose.
    let head = Head::new(200, url, false, &headers, &outgoing.origin);
    let head = Head {
        response_type: "basic",
        ..head
    };
    Ok((head, Bytes::from(data.body)))
}

fn hooks<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<NetHooks<'js>> {
    ctx.userdata::<NetHooks<'js>>()
        .map(|hooks| hooks.clone())
        .ok_or_else(|| Exception::throw_internal(ctx, "network hooks are not installed"))
}

fn deliver_head(ctx: &Ctx<'_>, id: u32, head: &Head) -> rquickjs::Result<()> {
    hooks(ctx)?.on_response.call((id, head.to_object(ctx)?))
}

fn deliver_body(ctx: &Ctx<'_>, id: u32, body: Bytes) -> rquickjs::Result<()> {
    let hooks = hooks(ctx)?;
    hooks.shared.net.in_flight.borrow_mut().remove(&id);
    let buffer = ArrayBuffer::new(ctx.clone(), Vec::from(body))?;
    hooks.on_body.call((id, buffer))
}

fn deliver_error(
    ctx: &Ctx<'_>,
    id: u32,
    method: &Method,
    url: &str,
    failure: &Failure,
) -> rquickjs::Result<()> {
    let hooks = hooks(ctx)?;
    let shared = &hooks.shared;
    shared.net.in_flight.borrow_mut().remove(&id);
    // Browsers log failed loads to the console; so does dasher, with the
    // reason, which scripts never get to see.
    shared.console.borrow_mut().push(ConsoleMessage {
        level: ConsoleLevel::Error,
        text: format!("{method} {url} failed: {}", failure.message),
        time: shared.event_loop.clock.now(),
    });
    hooks
        .on_error
        .call((id, failure.kind, failure.message.as_str()))
}

/// Queues delivery of a result that is already known, on the JS thread.
fn deliver_later(
    shared: &Rc<Shared>,
    id: u32,
    method: Method,
    url: String,
    result: Result<(Head, Bytes), Failure>,
) {
    match result {
        Ok((head, body)) => {
            shared
                .event_loop
                .queue(Box::new(move |ctx| deliver_head(&ctx, id, &head)));
            shared
                .event_loop
                .queue(Box::new(move |ctx| deliver_body(&ctx, id, body)));
        }
        Err(failure) => shared.event_loop.queue(Box::new(move |ctx| {
            deliver_error(&ctx, id, &method, &url, &failure)
        })),
    }
}

/// Starts a request and returns its id. The result arrives through the
/// hooks in later tasks.
fn send(shared: &Rc<Shared>, outgoing: Outgoing) -> u32 {
    let net = &shared.net;
    let id = net.next_id();
    let method = outgoing.request.method.clone();
    let url = outgoing.request.url.to_string();

    match outgoing.request.url.scheme() {
        "http" | "https" => {}
        "data" => {
            deliver_later(shared, id, method, url, data_response(&outgoing));
            return id;
        }
        scheme => {
            let failure = Failure::network(format!("unsupported URL scheme '{scheme}:'"));
            deliver_later(shared, id, method, url, Err(failure));
            return id;
        }
    }
    let Some(session) = net.session.clone() else {
        let failure = Failure::network("this page has no network access");
        deliver_later(shared, id, method, url, Err(failure));
        return id;
    };
    let Some(runtime) = net.spawn_handle() else {
        let failure = Failure::network("no Tokio runtime to run the request on");
        deliver_later(shared, id, method, url, Err(failure));
        return id;
    };

    let sender = TaskSender::new(Arc::clone(&shared.remote));
    let operation = sender.begin_operation();
    let task = runtime.spawn(fetch(session, outgoing, sender, operation, id, method, url));
    net.in_flight.borrow_mut().insert(id, task.abort_handle());
    id
}

/// Runs one request off the JS thread, posting its results back.
async fn fetch(
    session: Session,
    outgoing: Outgoing,
    sender: TaskSender,
    operation: PendingOperation,
    id: u32,
    method: Method,
    url: String,
) {
    let Outgoing { request, origin } = outgoing;
    let fail = move |operation: PendingOperation, failure: Failure| {
        operation.complete(move |ctx| deliver_error(&ctx, id, &method, &url, &failure));
    };
    let response = match session.send(request).await {
        Ok(response) => response,
        Err(error) => return fail(operation, Failure::from_net(&error)),
    };
    let head = Head::from_response(&response, &origin);
    sender.send(move |ctx| deliver_head(&ctx, id, &head));
    match response.bytes().await {
        Ok(body) => operation.complete(move |ctx| deliver_body(&ctx, id, body)),
        Err(error) => fail(operation, Failure::from_net(&error)),
    }
}

/// Sends a request and waits for the whole response, blocking the JS
/// thread: synchronous `XMLHttpRequest`. The request runs on a thread of its
/// own, so this works whatever runtime (if any) drives the page.
fn send_sync(shared: &Rc<Shared>, outgoing: Outgoing) -> Result<(Head, Bytes), Failure> {
    match outgoing.request.url.scheme() {
        "http" | "https" => {}
        "data" => return data_response(&outgoing),
        scheme => {
            return Err(Failure::network(format!(
                "unsupported URL scheme '{scheme}:'"
            )));
        }
    }
    // Not a pooled connection: the runtime that drives those is the one
    // this call blocks.
    let session = shared
        .net
        .session
        .as_ref()
        .map(Session::unpooled)
        .ok_or_else(|| Failure::network("this page has no network access"))?;
    let thread = std::thread::Builder::new()
        .name("dasher-sync-xhr".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Failure::network)?;
            runtime.block_on(async {
                let Outgoing { request, origin } = outgoing;
                let response = session
                    .send(request)
                    .await
                    .map_err(|e| Failure::from_net(&e))?;
                let head = Head::from_response(&response, &origin);
                let body = response.bytes().await.map_err(|e| Failure::from_net(&e))?;
                Ok((head, body))
            })
        })
        .map_err(Failure::network)?;
    thread
        .join()
        .unwrap_or_else(|_| Err(Failure::network("the request thread panicked")))
}

/// Strips a UTF-8 byte order mark and decodes the rest, replacing invalid
/// sequences.
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Adds the network host functions to the prelude's `native` object.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> rquickjs::Result<()> {
    let s = Rc::clone(shared);
    native.set(
        "netSetHooks",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, hooks: Object<'js>| {
            let hooks = NetHooks {
                on_response: hooks.get("onResponse")?,
                on_body: hooks.get("onBody")?,
                on_error: hooks.get("onError")?,
                shared: Rc::clone(&s),
            };
            match ctx.store_userdata(hooks) {
                Ok(_) => Ok(()),
                Err(_) => Err(Exception::throw_internal(
                    &ctx,
                    "cannot install network hooks",
                )),
            }
        })?,
    )?;

    let s = Rc::clone(shared);
    native.set(
        "netSend",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, request: Object<'js>| {
            let outgoing = read_request(&ctx, &request)?;
            Ok::<_, rquickjs::Error>(send(&s, outgoing))
        })?,
    )?;

    let s = Rc::clone(shared);
    native.set(
        "netAbort",
        Function::new(ctx.clone(), move |id: u32| s.net.abort(id))?,
    )?;

    let s = Rc::clone(shared);
    native.set(
        "netSendSync",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, request: Object<'js>| {
            let outgoing = read_request(&ctx, &request)?;
            let result = Object::new(ctx.clone())?;
            match send_sync(&s, outgoing) {
                Ok((head, body)) => {
                    result.set("head", head.to_object(&ctx)?)?;
                    result.set("body", ArrayBuffer::new(ctx.clone(), Vec::from(body))?)?;
                }
                Err(failure) => {
                    result.set("error", failure.kind)?;
                    result.set("message", failure.message)?;
                }
            }
            Ok::<_, rquickjs::Error>(result)
        })?,
    )?;

    native.set(
        "resolveUrl",
        Function::new(ctx.clone(), |input: String, base: String| {
            let base = Url::parse(&base).ok();
            Url::options()
                .base_url(base.as_ref())
                .parse(&input)
                .ok()
                .map(String::from)
        })?,
    )?;

    native.set(
        "decodeUtf8",
        Function::new(ctx.clone(), |bytes: TypedArray<'js, u8>| {
            decode_utf8(bytes.as_bytes().unwrap_or_default())
        })?,
    )?;

    native.set(
        "encodeUtf8",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, text: String| {
            ArrayBuffer::new(ctx, text.into_bytes())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_decoding() {
        assert_eq!(decode_utf8(b"\xEF\xBB\xBFhi"), "hi");
        assert_eq!(decode_utf8(b"a\xFFb"), "a\u{FFFD}b");
        assert_eq!(decode_utf8("é".as_bytes()), "é");
    }

    #[test]
    fn latin1() {
        assert_eq!(latin1_bytes("abc"), b"abc");
        assert_eq!(latin1_bytes("\u{e9}"), [0xe9]);
        assert_eq!(latin1_bytes("\u{2603}"), "\u{2603}".as_bytes());
    }

    #[test]
    fn response_heads_hide_cookies_and_classify_origin() {
        let origin = Url::parse("https://a.com/page").unwrap().origin();
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("text/plain"));
        headers.insert("set-cookie", HeaderValue::from_static("a=1"));
        headers.insert("x-latin", HeaderValue::from_bytes(b"caf\xe9").unwrap());
        let url = Url::parse("https://a.com/api").unwrap();
        let head = Head::new(404, url, true, &headers, &origin);
        assert_eq!(head.status_text, "Not Found");
        assert_eq!(head.response_type, "basic");
        assert_eq!(
            head.headers,
            ["content-type", "text/plain", "x-latin", "caf\u{e9}"]
        );
        let other = Url::parse("https://b.com/api").unwrap();
        let head = Head::new(299, other, false, &headers, &origin);
        assert_eq!(head.response_type, "cors");
        assert_eq!(head.status_text, "");
    }
}
