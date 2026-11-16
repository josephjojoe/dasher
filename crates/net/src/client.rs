//! The HTTP client, browsing sessions and requests.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{
    self, ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, COOKIE, HeaderName, HeaderValue, LOCATION,
    PROXY_AUTHORIZATION, REFERER, SET_COOKIE, USER_AGENT,
};
use hyper::{HeaderMap, Method, StatusCode, Uri};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use tokio::time::Instant;
use url::{Origin, Url};

use crate::activity::NetworkActivity;
use crate::connector::{Connector, Proxy};
use crate::cookies::CookieJar;
use crate::decode::ACCEPT_ENCODING as ACCEPT_ENCODING_VALUE;
use crate::error::{Error, ErrorKind, Result};
use crate::guard::{self, GuardedHttp};
use crate::policy::{BlockReason, RequestInfo, RequestPolicy, ResourceType};
#[cfg(feature = "replay")]
use crate::replay;
use crate::response::{Body, Response, with_deadline};

/// The `User-Agent` sent when none is configured: a current desktop Chrome on
/// Linux, so sites serve the same markup they would to a real browser.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36";
/// The default maximum response body size: 32 MiB.
pub const DEFAULT_MAX_RESPONSE_SIZE: u64 = 32 * 1024 * 1024;
/// The default timeout for a whole request, including reading the body.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// The default timeout for establishing a connection (TCP and TLS).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The default maximum number of redirects followed, as in browsers.
pub const DEFAULT_MAX_REDIRECTS: usize = 20;

/// Bytes of a redirect response's body read (and discarded) so that its
/// connection can go back to the pool. Longer bodies just close it.
const REDIRECT_DRAIN_LIMIT: usize = 64 * 1024;

type Hyper = HyperClient<Connector, Full<Bytes>>;

/// Configures a [`Client`].
#[derive(Clone)]
pub struct ClientBuilder {
    user_agent: HeaderValue,
    default_headers: HeaderMap,
    policy: RequestPolicy,
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    max_redirects: usize,
    max_response_size: u64,
    pool_idle_timeout: Duration,
    pool_max_idle_per_host: usize,
    http2: bool,
    built_in_roots: bool,
    root_certificates: Vec<CertificateDer<'static>>,
    proxy: Option<Proxy>,
    block_private_addresses: bool,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        Self {
            user_agent: HeaderValue::from_static(DEFAULT_USER_AGENT),
            default_headers,
            policy: RequestPolicy::default(),
            timeout: Some(DEFAULT_TIMEOUT),
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
            max_redirects: DEFAULT_MAX_REDIRECTS,
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
            pool_idle_timeout: Duration::from_secs(30),
            pool_max_idle_per_host: 4,
            http2: true,
            built_in_roots: true,
            root_certificates: Vec::new(),
            proxy: None,
            block_private_addresses: false,
        }
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .field("max_response_size", &self.max_response_size)
            .field("proxy", &self.proxy)
            .field("block_private_addresses", &self.block_private_addresses)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// The `User-Agent` header sessions send by default
    /// ([`DEFAULT_USER_AGENT`] unless set).
    pub fn user_agent(mut self, user_agent: HeaderValue) -> Self {
        self.user_agent = user_agent;
        self
    }

    /// Headers sessions add to every request by default, replacing the
    /// built-in defaults of the same name (by default only
    /// `Accept-Language: en-US,en;q=0.9`). A request's own headers win over
    /// these.
    pub fn default_headers(mut self, headers: HeaderMap) -> Self {
        self.default_headers = headers;
        self
    }

    /// The request policy sessions use by default.
    pub fn policy(mut self, policy: RequestPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The default timeout for a whole request: connecting, every redirect,
    /// and reading the body. `None` disables it. Defaults to
    /// [`DEFAULT_TIMEOUT`].
    pub fn timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// The timeout for establishing a TCP connection. `None` disables it.
    /// Defaults to [`DEFAULT_CONNECT_TIMEOUT`].
    pub fn connect_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The most redirects a request follows. Defaults to
    /// [`DEFAULT_MAX_REDIRECTS`].
    pub fn max_redirects(mut self, max: usize) -> Self {
        self.max_redirects = max;
        self
    }

    /// The default maximum size of a response body in bytes, both as
    /// received and after decoding. Defaults to
    /// [`DEFAULT_MAX_RESPONSE_SIZE`].
    pub fn max_response_size(mut self, bytes: u64) -> Self {
        self.max_response_size = bytes;
        self
    }

    /// How long an unused pooled connection is kept. Defaults to 30 seconds.
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }

    /// The most idle connections kept per host. Defaults to 4.
    pub fn pool_max_idle_per_host(mut self, max: usize) -> Self {
        self.pool_max_idle_per_host = max;
        self
    }

    /// Whether to offer HTTP/2 when negotiating TLS. Defaults to `true`.
    /// Plain-text connections always use HTTP/1.1.
    pub fn http2(mut self, enabled: bool) -> Self {
        self.http2 = enabled;
        self
    }

    /// Whether to trust the built-in Mozilla root certificates. Defaults to
    /// `true`. They are compiled in, so dasher needs no CA bundle in its
    /// container.
    pub fn built_in_root_certificates(mut self, enabled: bool) -> Self {
        self.built_in_roots = enabled;
        self
    }

    /// Trusts an additional root certificate, DER-encoded.
    pub fn add_root_certificate(mut self, der: impl Into<Vec<u8>>) -> Self {
        self.root_certificates
            .push(CertificateDer::from(der.into()));
        self
    }

    /// Sends every request through `proxy`.
    pub fn proxy(mut self, proxy: Proxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    /// Refuses to connect to loopback, link-local, private (RFC 1918 and
    /// IPv6 unique local), shared CGNAT, unspecified, multicast and
    /// reserved addresses. Off by default.
    ///
    /// Turn it on when the URLs come from someone you don't trust, so they
    /// can't reach the cloud metadata server (`169.254.169.254`), services
    /// on localhost or the private network. The check is made on the
    /// addresses actually connected to, after DNS, for every connection
    /// including those for redirects: a public name that resolves or
    /// rebinds to a private address is refused too. Of the addresses a
    /// name resolves to, the private ones are skipped. Requests refused
    /// fail with [`ErrorKind::Blocked`] and
    /// [`BlockReason::PrivateAddress`].
    ///
    /// Through a [`proxy`](Self::proxy), the proxy resolves host names and
    /// connects to them, so only literal IP targets (and the proxy's own
    /// address) can be checked; see [`Proxy`].
    pub fn block_private_addresses(mut self, enabled: bool) -> Self {
        self.block_private_addresses = enabled;
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<Client> {
        let tls = self.tls_config()?;
        let http = GuardedHttp::new(self.block_private_addresses, |http| {
            // The HTTPS connector hands it https URLs too.
            http.enforce_http(false);
            http.set_nodelay(true);
            http.set_connect_timeout(self.connect_timeout);
        });
        let connector = Connector::new(http, tls, self.http2, self.proxy.as_ref());

        let hyper = |max_idle_per_host: usize, connector: Connector| {
            HyperClient::builder(TokioExecutor::new())
                .timer(TokioTimer::new())
                .pool_timer(TokioTimer::new())
                .pool_idle_timeout(self.pool_idle_timeout)
                .pool_max_idle_per_host(max_idle_per_host)
                // Bound what one connection can buffer on our behalf.
                .http2_initial_stream_window_size(1024 * 1024)
                .http2_initial_connection_window_size(2 * 1024 * 1024)
                .http2_max_header_list_size(256 * 1024)
                .build(connector)
        };
        // No idle connections kept turns hyper's pool off altogether.
        let unpooled = hyper(0, connector.clone());
        let hyper = hyper(self.pool_max_idle_per_host, connector);

        Ok(Client {
            inner: Arc::new(ClientInner {
                hyper,
                unpooled,
                proxy_authorization: self.proxy.as_ref().and_then(|p| p.authorization().cloned()),
                user_agent: self.user_agent,
                default_headers: Arc::new(self.default_headers),
                policy: Arc::new(self.policy),
                timeout: self.timeout,
                max_redirects: self.max_redirects,
                max_response_size: self.max_response_size,
                #[cfg(feature = "replay")]
                replay: replay::Mode::from_env(),
            }),
        })
    }

    fn tls_config(&self) -> Result<rustls::ClientConfig> {
        let config_error = |e: rustls::Error| Error::new(ErrorKind::Config).with_source(e);
        let mut roots = RootCertStore::empty();
        if self.built_in_roots {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        for certificate in &self.root_certificates {
            roots.add(certificate.clone()).map_err(config_error)?;
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Ok(rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(config_error)?
            .with_root_certificates(roots)
            .with_no_client_auth())
    }
}

/// An HTTP/1.1 and HTTP/2 client with a connection pool, over rustls.
///
/// One client is meant to be shared by everything in the process, so
/// connections are reused across pages; cloning it is cheap. Requests are
/// made through a [`Session`], which adds cookies, the request policy and
/// in-flight tracking.
///
/// The client must be used from within a Tokio runtime, which runs its
/// connections.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    hyper: Hyper,
    /// The same client without a connection pool, for [`Session::unpooled`].
    unpooled: Hyper,
    /// Sent with plain `http` requests, which go to the proxy itself.
    proxy_authorization: Option<HeaderValue>,
    user_agent: HeaderValue,
    default_headers: Arc<HeaderMap>,
    policy: Arc<RequestPolicy>,
    timeout: Option<Duration>,
    max_redirects: usize,
    max_response_size: u64,
    /// Recording or replaying responses, when the environment asks for it.
    #[cfg(feature = "replay")]
    replay: Option<replay::Mode>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("user_agent", &self.inner.user_agent)
            .field("timeout", &self.inner.timeout)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client with the default configuration.
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Starts configuring a client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Starts a browsing session with an empty cookie jar, using the
    /// client's default user agent, headers and policy.
    pub fn session(&self) -> Session {
        Session {
            client: self.clone(),
            pooled: true,
            cookies: Arc::new(CookieJar::new()),
            activity: NetworkActivity::new(),
            policy: Arc::clone(&self.inner.policy),
            user_agent: self.inner.user_agent.clone(),
            default_headers: Arc::clone(&self.inner.default_headers),
        }
    }
}

/// Whether a request carries cookies, and stores the ones it receives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Credentials {
    /// Always (the default; what navigations and scripts do).
    #[default]
    Include,
    /// Only while the URL being requested has this origin; redirects that
    /// leave it stop sending and storing cookies. This is `fetch()`'s
    /// default, with the page's origin.
    SameOrigin(Origin),
    /// Never.
    Omit,
}

impl Credentials {
    fn allowed_for(&self, url: &Url) -> bool {
        match self {
            Self::Include => true,
            Self::SameOrigin(origin) => url.origin() == *origin,
            Self::Omit => false,
        }
    }
}

/// What to do when a response is a redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedirectMode {
    /// Follow it (the default).
    #[default]
    Follow,
    /// Fail with [`ErrorKind::Redirect`].
    Error,
    /// Return the redirect response itself.
    Manual,
}

/// A request to send through a [`Session`].
///
/// ```
/// use dasher_net::{Request, ResourceType};
/// use url::Url;
///
/// let page = Url::parse("https://example.com/").unwrap();
/// let request = Request::get(page.join("/api/items").unwrap(), ResourceType::Fetch)
///     .with_referrer(page);
/// # let _ = request;
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Request {
    /// The method.
    pub method: Method,
    /// The URL. A fragment is kept for the response URL but never sent.
    /// Credentials in it are sent as HTTP basic authentication unless there
    /// is an `Authorization` header.
    pub url: Url,
    /// What the request is for; decides the policy check and the default
    /// `Accept` header.
    pub resource_type: ResourceType,
    /// Headers, overriding the session's defaults of the same name.
    pub headers: HeaderMap,
    /// The body (empty for none).
    pub body: Bytes,
    /// The URL of the page making the request, used for the `Referer`
    /// header under the `strict-origin-when-cross-origin` policy (unless
    /// `headers` has one).
    pub referrer: Option<Url>,
    /// Whether cookies are sent and stored.
    pub credentials: Credentials,
    /// What to do with redirects.
    pub redirect: RedirectMode,
    /// Overrides the client's timeout for this request.
    pub timeout: Option<Duration>,
    /// Overrides the client's maximum response size for this request.
    pub max_response_size: Option<u64>,
}

impl Request {
    /// A request with no body, headers or referrer that follows redirects
    /// and includes cookies.
    pub fn new(method: Method, url: Url, resource_type: ResourceType) -> Self {
        Self {
            method,
            url,
            resource_type,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            referrer: None,
            credentials: Credentials::Include,
            redirect: RedirectMode::Follow,
            timeout: None,
            max_response_size: None,
        }
    }

    /// A `GET` request.
    pub fn get(url: Url, resource_type: ResourceType) -> Self {
        Self::new(Method::GET, url, resource_type)
    }

    /// Adds a header, keeping any earlier values of the same name.
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Sets the body.
    pub fn with_body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets the referrer.
    pub fn with_referrer(mut self, referrer: Url) -> Self {
        self.referrer = Some(referrer);
        self
    }

    /// Sets the credentials mode.
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = credentials;
        self
    }

    /// Sets the redirect mode.
    pub fn with_redirect(mut self, redirect: RedirectMode) -> Self {
        self.redirect = redirect;
        self
    }

    /// Overrides the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Overrides the maximum response size.
    pub fn with_max_response_size(mut self, bytes: u64) -> Self {
        self.max_response_size = Some(bytes);
        self
    }
}

/// A browsing session: a cookie jar, a request policy, default headers and
/// an in-flight tracker on top of a shared [`Client`].
///
/// Every request a page makes goes through its session. Clones share the
/// cookie jar and the tracker.
#[derive(Clone)]
pub struct Session {
    client: Client,
    /// Whether requests share the client's connection pool.
    pooled: bool,
    cookies: Arc<CookieJar>,
    activity: NetworkActivity,
    policy: Arc<RequestPolicy>,
    user_agent: HeaderValue,
    default_headers: Arc<HeaderMap>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("cookies", &self.cookies)
            .field("activity", &self.activity)
            .field("policy", &self.policy)
            .field("user_agent", &self.user_agent)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// This session, making each request on a connection of its own.
    ///
    /// A pooled connection is driven by the runtime that opened it. A
    /// request made from another runtime while that one is blocked (as
    /// synchronous XHR does: the page waits on a thread that runs the
    /// request) would wait forever on a connection nobody is driving, so
    /// such requests must not reuse the pool, nor leave connections of
    /// their short-lived runtime in it.
    pub fn unpooled(&self) -> Self {
        Self {
            pooled: false,
            ..self.clone()
        }
    }

    /// Uses `jar` instead of the session's own cookie jar, e.g. to share
    /// cookies between sessions.
    pub fn with_cookie_jar(mut self, jar: Arc<CookieJar>) -> Self {
        self.cookies = jar;
        self
    }

    /// Uses `policy` instead of the client's default.
    pub fn with_policy(mut self, policy: RequestPolicy) -> Self {
        self.policy = Arc::new(policy);
        self
    }

    /// Uses `user_agent` instead of the client's default.
    pub fn with_user_agent(mut self, user_agent: HeaderValue) -> Self {
        self.user_agent = user_agent;
        self
    }

    /// Uses `headers` instead of the client's default headers.
    pub fn with_default_headers(mut self, headers: HeaderMap) -> Self {
        self.default_headers = Arc::new(headers);
        self
    }

    /// The client the session sends requests through.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The session's cookie jar.
    pub fn cookies(&self) -> &Arc<CookieJar> {
        &self.cookies
    }

    /// The session's in-flight request tracker.
    pub fn activity(&self) -> &NetworkActivity {
        &self.activity
    }

    /// The session's request policy.
    pub fn policy(&self) -> &RequestPolicy {
        &self.policy
    }

    /// The session's `User-Agent`.
    pub fn user_agent(&self) -> &HeaderValue {
        &self.user_agent
    }

    /// Sends a `GET` request for `url`.
    pub async fn get(&self, url: Url, resource_type: ResourceType) -> Result<Response> {
        self.send(Request::get(url, resource_type)).await
    }

    /// Sends a request, following redirects according to its redirect mode,
    /// and returns once the final response's headers have arrived.
    pub async fn send(&self, request: Request) -> Result<Response> {
        let client = &self.client.inner;
        self.check_policy(&request.method, &request.url, request.resource_type, 0)?;

        let in_flight = self.activity.start();
        let timeout = request.timeout.or(client.timeout);
        let deadline = timeout.map(|t| Instant::now() + t);
        let limit = request
            .max_response_size
            .unwrap_or(client.max_response_size);

        #[cfg(feature = "replay")]
        let mut filling = None;
        #[cfg(feature = "replay")]
        if let Some(replay::Mode::Replay(store)) = &client.replay {
            match store.find(&request.method, &request.url) {
                Some(recorded) => {
                    if request.credentials.allowed_for(&request.url) {
                        for value in recorded.headers.get_all(SET_COOKIE) {
                            if let Ok(value) = value.to_str() {
                                self.cookies.set_from_header(&recorded.url, value);
                            }
                        }
                    }
                    return Ok(Response::recorded(recorded, limit, in_flight));
                }
                // Fill mode: fetch what the recording lacks, and add it.
                None if store.fills() => filling = Some(store.dir()),
                None => {
                    return Err(Error::new(ErrorKind::Connect)
                        .with_url(&request.url)
                        .with_source("not in the recording"));
                }
            }
        }
        #[cfg(feature = "replay")]
        let recording = match (&client.replay, filling) {
            (_, Some(dir)) => Some((dir, request.method.clone(), request.url.clone())),
            (Some(replay::Mode::Record(dir)), None) => {
                Some((dir.as_path(), request.method.clone(), request.url.clone()))
            }
            _ => None,
        };

        let Request {
            mut method,
            mut url,
            resource_type,
            mut headers,
            mut body,
            referrer,
            credentials,
            redirect,
            ..
        } = request;
        let mut redirects = 0;
        loop {
            let use_cookies = credentials.allowed_for(&url);
            let outgoing = self.build_request(
                &method,
                &url,
                resource_type,
                &headers,
                &body,
                referrer.as_ref(),
                use_cookies,
            )?;
            let response = with_deadline(deadline, async {
                let hyper = if self.pooled {
                    &client.hyper
                } else {
                    &client.unpooled
                };
                hyper.request(outgoing).await.map_err(|e| {
                    if let Some(ip) = guard::blocked_address(&e) {
                        return Error::new(ErrorKind::Blocked(BlockReason::PrivateAddress(ip)));
                    }
                    let kind = if e.is_connect() {
                        ErrorKind::Connect
                    } else {
                        ErrorKind::Http
                    };
                    Error::new(kind).with_source(e)
                })
            })
            .await
            .map_err(|e| e.with_url(&url))?;
            let (parts, incoming) = response.into_parts();

            if use_cookies {
                for value in parts.headers.get_all(SET_COOKIE) {
                    if let Ok(value) = value.to_str() {
                        self.cookies.set_from_header(&url, value);
                    }
                }
            }

            if let Some(location) = redirect_location(parts.status, &parts.headers) {
                match redirect {
                    RedirectMode::Follow => {}
                    RedirectMode::Manual => {
                        let body = Body::new(incoming, &parts.headers, limit, deadline, in_flight)
                            .map_err(|e| e.with_url(&url))?;
                        return Ok(Response::new(parts, url, redirects > 0, body));
                    }
                    RedirectMode::Error => {
                        return Err(Error::new(ErrorKind::Redirect)
                            .with_url(&url)
                            .with_source("redirects are not allowed for this request"));
                    }
                }
                if redirects >= client.max_redirects {
                    return Err(Error::new(ErrorKind::Redirect)
                        .with_url(&url)
                        .with_source(format!("more than {} redirects", client.max_redirects)));
                }
                let mut next = url.join(&location).map_err(|e| {
                    Error::new(ErrorKind::Redirect)
                        .with_url(&url)
                        .with_source(format!("invalid Location {location:?}: {e}"))
                })?;
                if next.fragment().is_none() {
                    next.set_fragment(url.fragment());
                }
                redirects += 1;
                self.check_policy(&method, &next, resource_type, redirects)?;

                // Let the connection go back to the pool if that's cheap.
                let _ = with_deadline(deadline, drain(incoming)).await;

                if rewrites_to_get(parts.status, &method) {
                    method = Method::GET;
                    body = Bytes::new();
                    for name in REQUEST_BODY_HEADERS {
                        headers.remove(name);
                    }
                }
                if next.origin() != url.origin() {
                    headers.remove(AUTHORIZATION);
                    headers.remove(COOKIE);
                }
                url = next;
                continue;
            }

            let body = Body::new(incoming, &parts.headers, limit, deadline, in_flight)
                .map_err(|e| e.with_url(&url))?;
            // Fail early on a declared length over the limit.
            body.check_declared_length().map_err(|e| e.with_url(&url))?;
            let response = Response::new(parts, url, redirects > 0, body);
            #[cfg(feature = "replay")]
            if let Some((dir, method, request_url)) = recording {
                let recorded = replay::Recorded {
                    status: response.status(),
                    url: response.url().clone(),
                    redirected: response.redirected(),
                    headers: response.headers().clone(),
                    body: response.bytes().await?,
                };
                replay::write(dir, &method, &request_url, &recorded);
                return Ok(Response::recorded(recorded, limit, self.activity.start()));
            }
            return Ok(response);
        }
    }

    fn check_policy(
        &self,
        method: &Method,
        url: &Url,
        resource_type: ResourceType,
        redirects: usize,
    ) -> Result<()> {
        self.policy
            .check(&RequestInfo {
                url,
                method,
                resource_type,
                redirects,
            })
            .map_err(|reason| Error::new(ErrorKind::Blocked(reason)).with_url(url))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_request(
        &self,
        method: &Method,
        url: &Url,
        resource_type: ResourceType,
        headers: &HeaderMap,
        body: &Bytes,
        referrer: Option<&Url>,
        use_cookies: bool,
    ) -> Result<hyper::Request<Full<Bytes>>> {
        let invalid = |e: &dyn fmt::Display| {
            Error::new(ErrorKind::InvalidRequest)
                .with_url(url)
                .with_source(e.to_string())
        };

        let mut all = HeaderMap::with_capacity(8 + self.default_headers.len() + headers.len());
        all.insert(USER_AGENT, self.user_agent.clone());
        all.insert(ACCEPT, HeaderValue::from_static(resource_type.accept()));
        all.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static(ACCEPT_ENCODING_VALUE),
        );
        override_headers(&mut all, &self.default_headers);
        override_headers(&mut all, headers);

        if use_cookies
            && !all.contains_key(COOKIE)
            && let Some(cookies) = self.cookies.header_for(url)
        {
            all.insert(
                COOKIE,
                HeaderValue::try_from(cookies).map_err(|e| invalid(&e))?,
            );
        }
        if !all.contains_key(REFERER)
            && let Some(referer) = referrer.and_then(|r| referrer_for(r, url))
        {
            all.insert(
                REFERER,
                HeaderValue::try_from(referer).map_err(|e| invalid(&e))?,
            );
        }
        if !all.contains_key(AUTHORIZATION)
            && let Some(auth) = basic_auth_from_url(url)
        {
            all.insert(AUTHORIZATION, auth);
        }
        if url.scheme() == "http"
            && let Some(authorization) = &self.client.inner.proxy_authorization
        {
            all.insert(PROXY_AUTHORIZATION, authorization.clone());
        }

        let uri = request_uri(url).map_err(|e| invalid(&e))?;
        let mut request = hyper::Request::new(Full::new(body.clone()));
        *request.method_mut() = method.clone();
        *request.uri_mut() = uri;
        *request.headers_mut() = all;
        Ok(request)
    }
}

/// Headers describing a request body, dropped when a redirect turns the
/// request into a `GET`.
const REQUEST_BODY_HEADERS: [HeaderName; 4] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_ENCODING,
    header::CONTENT_LANGUAGE,
];

/// Replaces the headers in `base` that `overrides` has, keeping every value
/// `overrides` gives.
fn override_headers(base: &mut HeaderMap, overrides: &HeaderMap) {
    for name in overrides.keys() {
        base.remove(name);
    }
    for (name, value) in overrides {
        base.append(name, value.clone());
    }
}

/// The `Location` of a redirect response, if it is one that can be followed.
fn redirect_location(status: StatusCode, headers: &HeaderMap) -> Option<String> {
    if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = headers.get(LOCATION)?;
    // Servers sometimes send raw UTF-8; URL parsing will percent-encode it.
    Some(String::from_utf8_lossy(location.as_bytes()).into_owned())
}

/// Whether following a redirect with `status` turns a `method` request into
/// a `GET` without a body (Fetch standard, HTTP-redirect fetch step 12).
fn rewrites_to_get(status: StatusCode, method: &Method) -> bool {
    match status.as_u16() {
        301 | 302 => *method == Method::POST,
        303 => *method != Method::GET && *method != Method::HEAD,
        _ => false,
    }
}

/// Reads a little of a body so its connection can be reused.
async fn drain(mut body: Incoming) -> Result<()> {
    let mut read = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| Error::new(ErrorKind::Http).with_source(e))?;
        if let Some(data) = frame.data_ref() {
            read += data.len();
            if read > REDIRECT_DRAIN_LIMIT {
                break;
            }
        }
    }
    Ok(())
}

/// The URI to send for `url`: without the fragment and credentials.
fn request_uri(url: &Url) -> Result<Uri, hyper::http::uri::InvalidUri> {
    if url.fragment().is_none() && url.username().is_empty() && url.password().is_none() {
        return url.as_str().parse();
    }
    let mut url = url.clone();
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.as_str().parse()
}

/// The `Referer` to send from a page at `referrer` to `target` under the
/// default `strict-origin-when-cross-origin` referrer policy.
pub(crate) fn referrer_for(referrer: &Url, target: &Url) -> Option<String> {
    if !matches!(referrer.scheme(), "http" | "https") {
        return None;
    }
    // No referrer on a downgrade from HTTPS to plain HTTP.
    if referrer.scheme() == "https" && target.scheme() == "http" {
        return None;
    }
    if referrer.origin() == target.origin() {
        let mut stripped = referrer.clone();
        stripped.set_fragment(None);
        let _ = stripped.set_username("");
        let _ = stripped.set_password(None);
        Some(stripped.into())
    } else {
        Some(format!("{}/", referrer.origin().ascii_serialization()))
    }
}

/// An `Authorization: Basic` header from the credentials in `url`, if any.
fn basic_auth_from_url(url: &Url) -> Option<HeaderValue> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let decode = |s: &str| percent_encoding::percent_decode_str(s).collect::<Vec<u8>>();
    Some(basic_auth(
        &decode(url.username()),
        &decode(url.password().unwrap_or("")),
    ))
}

/// An `Authorization`/`Proxy-Authorization` value for HTTP basic
/// authentication.
pub(crate) fn basic_auth(user: &[u8], password: &[u8]) -> HeaderValue {
    let mut credentials = Vec::with_capacity(user.len() + 1 + password.len());
    credentials.extend_from_slice(user);
    credentials.push(b':');
    credentials.extend_from_slice(password);
    let mut value = String::from("Basic ");
    base64_encode(&credentials, &mut value);
    let mut value = HeaderValue::try_from(value).expect("base64 is a valid header value");
    value.set_sensitive(true);
    value
}

/// Standard base64 with padding.
fn base64_encode(input: &[u8], out: &mut String) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn base64() {
        let enc = |s: &[u8]| {
            let mut out = String::new();
            base64_encode(s, &mut out);
            out
        };
        assert_eq!(enc(b""), "");
        assert_eq!(enc(b"f"), "Zg==");
        assert_eq!(enc(b"fo"), "Zm8=");
        assert_eq!(enc(b"foo"), "Zm9v");
        assert_eq!(enc(b"foob"), "Zm9vYg==");
        assert_eq!(enc(b"fooba"), "Zm9vYmE=");
        assert_eq!(enc(b"foobar"), "Zm9vYmFy");
        assert_eq!(enc(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn basic_auth_header() {
        assert_eq!(basic_auth_from_url(&url("http://a.com/")), None);
        let auth = basic_auth_from_url(&url("http://Aladdin:open%20sesame@a.com/")).unwrap();
        assert_eq!(auth, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert!(auth.is_sensitive());
        let auth = basic_auth_from_url(&url("http://user@a.com/")).unwrap();
        assert_eq!(auth, "Basic dXNlcjo=");
    }

    #[test]
    fn request_uris() {
        let uri = |s: &str| request_uri(&url(s)).unwrap().to_string();
        assert_eq!(uri("http://a.com/p?q=1"), "http://a.com/p?q=1");
        assert_eq!(uri("http://u:p@a.com/p?q=1#frag"), "http://a.com/p?q=1");
        assert_eq!(uri("https://a.com/x#y"), "https://a.com/x");
        assert_eq!(uri("http://[::1]:8080/x"), "http://[::1]:8080/x");
    }

    #[test]
    fn referrers() {
        let r = |from: &str, to: &str| referrer_for(&url(from), &url(to));
        assert_eq!(
            r("https://a.com/page?q=1#frag", "https://a.com/api"),
            Some("https://a.com/page?q=1".into())
        );
        assert_eq!(
            r("https://u:p@a.com/page", "https://a.com/api"),
            Some("https://a.com/page".into())
        );
        assert_eq!(
            r("https://a.com/page?q=1", "https://b.com/x"),
            Some("https://a.com/".into())
        );
        assert_eq!(
            r("https://a.com:8443/page", "https://a.com/x"),
            Some("https://a.com:8443/".into())
        );
        assert_eq!(r("https://a.com/page", "http://a.com/x"), None);
        assert_eq!(
            r("http://a.com/page", "https://a.com/x"),
            Some("http://a.com/".into())
        );
        assert_eq!(r("about:blank", "https://a.com/x"), None);
    }

    #[test]
    fn redirect_method_rewriting() {
        let s = |n| StatusCode::from_u16(n).unwrap();
        assert!(rewrites_to_get(s(301), &Method::POST));
        assert!(rewrites_to_get(s(302), &Method::POST));
        assert!(!rewrites_to_get(s(302), &Method::PUT));
        assert!(rewrites_to_get(s(303), &Method::PUT));
        assert!(rewrites_to_get(s(303), &Method::POST));
        assert!(!rewrites_to_get(s(303), &Method::HEAD));
        assert!(!rewrites_to_get(s(307), &Method::POST));
        assert!(!rewrites_to_get(s(308), &Method::POST));
    }

    #[test]
    fn header_overrides() {
        let mut base = HeaderMap::new();
        base.insert(ACCEPT, HeaderValue::from_static("a"));
        base.insert(USER_AGENT, HeaderValue::from_static("ua"));
        let mut overrides = HeaderMap::new();
        overrides.append(ACCEPT, HeaderValue::from_static("b"));
        overrides.append(ACCEPT, HeaderValue::from_static("c"));
        override_headers(&mut base, &overrides);
        let accept: Vec<_> = base.get_all(ACCEPT).iter().collect();
        assert_eq!(accept, ["b", "c"]);
        assert_eq!(base[USER_AGENT], "ua");
    }

    #[test]
    fn credentials_modes() {
        let a = url("https://a.com/x");
        let b = url("https://b.com/x");
        assert!(Credentials::Include.allowed_for(&b));
        assert!(!Credentials::Omit.allowed_for(&a));
        let same = Credentials::SameOrigin(a.origin());
        assert!(same.allowed_for(&url("https://a.com/other")));
        assert!(!same.allowed_for(&b));
        assert!(!same.allowed_for(&url("http://a.com/x")));
    }
}
