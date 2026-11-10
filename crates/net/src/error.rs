//! The network layer's error type.

use std::error::Error as StdError;
use std::fmt;

use url::Url;

use crate::policy::BlockReason;

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;

/// What went wrong with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request policy refused the request (or a redirect it led to)
    /// before anything was sent.
    Blocked(BlockReason),
    /// The client configuration is invalid (e.g. a bad root certificate).
    Config,
    /// The request itself is malformed: a bad header, or a URL hyper can't
    /// send.
    InvalidRequest,
    /// The request, including reading the body, took longer than its
    /// timeout.
    Timeout,
    /// Connecting failed: DNS, TCP, TLS or the proxy.
    Connect,
    /// The connection failed after it was established, or the server spoke
    /// broken HTTP.
    Http,
    /// A redirect could not be followed: the `Location` was invalid, the
    /// request's redirect mode is [`RedirectMode::Error`], or there were too
    /// many redirects.
    ///
    /// [`RedirectMode::Error`]: crate::RedirectMode::Error
    Redirect,
    /// The response body is bigger than the maximum response size.
    TooLarge {
        /// The limit that was exceeded, in bytes.
        limit: u64,
    },
    /// The response body could not be decoded (bad or unsupported
    /// `Content-Encoding`).
    Decode,
}

/// An error from the network layer.
pub struct Error {
    kind: ErrorKind,
    url: Option<Box<Url>>,
    source: Option<BoxError>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            url: None,
            source: None,
        }
    }

    pub(crate) fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub(crate) fn with_url(mut self, url: &Url) -> Self {
        self.url = Some(Box::new(url.clone()));
        self
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The URL being requested when the error happened. For redirects this
    /// is the hop that failed, not the original URL.
    pub fn url(&self) -> Option<&Url> {
        self.url.as_deref()
    }

    /// Whether the policy refused the request.
    pub fn is_blocked(&self) -> bool {
        matches!(self.kind, ErrorKind::Blocked(_))
    }

    /// Whether the request timed out.
    pub fn is_timeout(&self) -> bool {
        self.kind == ErrorKind::Timeout
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Error");
        d.field("kind", &self.kind);
        if let Some(url) = &self.url {
            d.field("url", &url.as_str());
        }
        if let Some(source) = &self.source {
            d.field("source", source);
        }
        d.finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Blocked(reason) => write!(f, "request blocked: {reason}")?,
            ErrorKind::Config => f.write_str("invalid client configuration")?,
            ErrorKind::InvalidRequest => f.write_str("invalid request")?,
            ErrorKind::Timeout => f.write_str("request timed out")?,
            ErrorKind::Connect => f.write_str("connection failed")?,
            ErrorKind::Http => f.write_str("HTTP error")?,
            ErrorKind::Redirect => f.write_str("redirect failed")?,
            ErrorKind::TooLarge { limit } => {
                write!(f, "response body exceeds the {limit} byte limit")?;
            }
            ErrorKind::Decode => f.write_str("could not decode the response body")?,
        }
        if let Some(url) = &self.url {
            write!(f, " ({url})")?;
        }
        if let Some(source) = &self.source {
            write!(f, ": {source}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn StdError + 'static))
    }
}

/// A `Result` with the network layer's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let url = Url::parse("https://example.com/a").unwrap();
        let e = Error::new(ErrorKind::TooLarge { limit: 10 })
            .with_url(&url)
            .with_source("boom");
        assert_eq!(
            e.to_string(),
            "response body exceeds the 10 byte limit (https://example.com/a): boom"
        );
        assert_eq!(e.url(), Some(&url));
        assert!(e.source().is_some());
        assert!(!e.is_timeout() && !e.is_blocked());

        let e = Error::new(ErrorKind::Blocked(BlockReason::Scheme));
        assert!(e.is_blocked());
        assert_eq!(
            e.to_string(),
            "request blocked: only http and https URLs can be fetched"
        );
    }
}
