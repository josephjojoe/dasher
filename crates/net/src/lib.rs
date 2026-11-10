//! Networking for dasher.
//!
//! An HTTP/1.1 and HTTP/2 client (hyper + rustls, no OpenSSL) plus the
//! network policy that decides what a page may load. Only documents, scripts
//! and fetch/XHR traffic ever go over the network; images, stylesheets, fonts
//! and media are refused without a request being made.

mod activity;
mod client;
mod connector;
mod cookies;
mod decode;
mod error;
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
