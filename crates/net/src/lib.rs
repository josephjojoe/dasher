//! Networking for dasher.
//!
//! An HTTP/1.1 and HTTP/2 client (hyper + rustls, no OpenSSL) plus the
//! network policy that decides what a page may load. Only documents, scripts
//! and fetch/XHR traffic ever go over the network; images, stylesheets, fonts
//! and media are refused without a request being made.
//!
//! - A [`Client`] owns the connection pool and TLS configuration. Build one
//!   per process and share it, so pages reuse connections.
//! - A [`Session`] is one browsing session on top of it: a [`CookieJar`], a
//!   [`RequestPolicy`], the user agent and default headers, and a
//!   [`NetworkActivity`] tracker that tells the page layer when the network
//!   is idle. Every request a page makes goes through its session.
//! - [`Session::send`] checks the policy on every hop, follows redirects,
//!   stores and sends cookies, and returns a [`Response`] whose body is
//!   decoded (gzip, deflate, brotli, zstd) and bounded by the maximum
//!   response size and the request's timeout.
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use dasher_net::{Client, Request, RequestPolicy, ResourceType, Url};
//!
//! # async fn example() -> dasher_net::Result<()> {
//! let client = Client::builder()
//!     .timeout(Some(Duration::from_secs(20)))
//!     .policy(RequestPolicy::new().block_url("*://*.google-analytics.com/*"))
//!     .build()?;
//! let session = client.session();
//!
//! let page = Url::parse("https://example.com/").unwrap();
//! let response = session.get(page.clone(), ResourceType::Document).await?;
//! let html = response.bytes().await?;
//!
//! let api = Request::get(page.join("/api/items").unwrap(), ResourceType::Fetch)
//!     .with_referrer(page);
//! let json = session.send(api).await?.bytes().await?;
//!
//! session.activity().wait_for_idle(0, Duration::from_millis(500)).await;
//! # let _ = (html, json);
//! # Ok(())
//! # }
//! ```

mod activity;
mod client;
mod connector;
mod cookies;
mod decode;
mod error;
mod guard;
mod policy;
mod response;

pub use activity::{InFlight, NetworkActivity};
pub use client::{
    Client, ClientBuilder, Credentials, DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_REDIRECTS,
    DEFAULT_MAX_RESPONSE_SIZE, DEFAULT_TIMEOUT, DEFAULT_USER_AGENT, RedirectMode, Request, Session,
};
pub use connector::Proxy;
pub use cookies::{
    Cookie, CookieJar, MAX_COOKIE_SIZE, MAX_COOKIES, MAX_COOKIES_PER_DOMAIN, SameSite,
};
pub use error::{Error, ErrorKind, Result};
pub use policy::{BlockReason, RequestInfo, RequestPolicy, ResourceType, UrlPattern};
pub use response::Response;

// Types that appear in this crate's API.
pub use hyper::{HeaderMap, Method, StatusCode, Version, header};
pub use url::{self, Url};
