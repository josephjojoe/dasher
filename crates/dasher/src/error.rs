//! The error type of the public API.

use std::fmt;

use dasher_js::{ResourceLimit, ScriptError};
use dasher_page::LoadError;

/// A specialized `Result` for dasher operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a dasher operation failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The [`Browser`](crate::Browser) configuration is invalid: a bad
    /// header, user agent or proxy URL, for example.
    Config(String),
    /// The page could not be loaded at all: an invalid URL, a network
    /// failure, or no response before the timeout. Pages that load with
    /// an error status (404, 500) are not errors; see
    /// [`Navigation::status`](crate::Navigation::status).
    Load(LoadError),
    /// A cookie could not be stored.
    InvalidCookie(String),
    /// A CSS selector could not be parsed.
    InvalidSelector(String),
    /// JavaScript evaluated for the host threw, rejected, or produced a
    /// value that cannot be represented as JSON.
    Script(ScriptError),
    /// A value from the page could not be converted into the requested
    /// Rust type.
    Json(serde_json::Error),
    /// The page has no document yet: call [`Page::goto`](crate::Page::goto)
    /// first.
    NoDocument,
    /// The condition being waited for did not come about in time.
    Timeout,
    /// The page navigated to another document while a result from the old
    /// one was being waited for.
    NavigatedAway,
    /// The page went over one of its limits: it filled its JS heap
    /// ([`BrowserBuilder::memory_limit`](crate::BrowserBuilder::memory_limit))
    /// or built too many DOM nodes
    /// ([`BrowserBuilder::max_dom_nodes`](crate::BrowserBuilder::max_dom_nodes)).
    /// Its document was freed, so the page is blank.
    LimitExceeded(ResourceLimit),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(f, "invalid configuration: {message}"),
            Self::Load(e) => e.fmt(f),
            Self::InvalidCookie(message) => write!(f, "invalid cookie: {message}"),
            Self::InvalidSelector(s) => write!(f, "{s:?} is not a valid selector"),
            Self::Script(e) => e.fmt(f),
            Self::Json(e) => write!(f, "unexpected value from the page: {e}"),
            Self::NoDocument => f.write_str("the page has no document; call goto first"),
            Self::Timeout => f.write_str("timed out"),
            Self::NavigatedAway => {
                f.write_str("the page navigated away before the result was ready")
            }
            Self::LimitExceeded(limit) => limit.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(e) => Some(e),
            Self::Script(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LoadError> for Error {
    fn from(e: LoadError) -> Self {
        match e {
            LoadError::LimitExceeded(limit) => Self::LimitExceeded(limit),
            e => Self::Load(e),
        }
    }
}

impl From<ScriptError> for Error {
    fn from(e: ScriptError) -> Self {
        Self::Script(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<dasher_page::SettleError> for Error {
    fn from(e: dasher_page::SettleError) -> Self {
        match e {
            dasher_page::SettleError::InvalidSelector(s) => Self::InvalidSelector(s),
            // SettleError is non-exhaustive.
            other => Self::Config(other.to_string()),
        }
    }
}
