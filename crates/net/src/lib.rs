//! Networking for dasher.
//!
//! An HTTP/1.1 and HTTP/2 client (hyper + rustls, no OpenSSL) plus the
//! network policy that decides what a page may load. Only documents, scripts
//! and fetch/XHR traffic ever go over the network; images, stylesheets, fonts
//! and media are refused without a request being made.

mod policy;

pub use policy::{BlockReason, RequestInfo, RequestPolicy, ResourceType, UrlPattern};
