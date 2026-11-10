//! Responses and their bounded bodies.

use std::fmt;
use std::future::Future;

use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE};
use hyper::{HeaderMap, StatusCode, Version};
use tokio::time::Instant;
use url::Url;

use crate::activity::InFlight;
use crate::error::{Error, ErrorKind, Result};

/// A response to a request, after any redirects were followed.
///
/// The headers have arrived; the body is read on demand with
/// [`bytes`](Self::bytes) or [`chunk`](Self::chunk). The request counts as in
/// flight until the body has been read to the end or the response is
/// dropped, and the request's timeout covers reading the body too.
pub struct Response {
    status: StatusCode,
    version: Version,
    headers: HeaderMap,
    url: Url,
    redirected: bool,
    body: Body,
}

impl Response {
    pub(crate) fn new(
        parts: hyper::http::response::Parts,
        url: Url,
        redirected: bool,
        body: Body,
    ) -> Self {
        Self {
            status: parts.status,
            version: parts.version,
            headers: parts.headers,
            url,
            redirected,
            body,
        }
    }

    /// The status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The HTTP version the response arrived over.
    pub fn version(&self) -> Version {
        self.version
    }

    /// The response headers.
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// The URL the response came from: the request URL, or where the last
    /// redirect led.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Whether any redirects were followed to get here.
    pub fn redirected(&self) -> bool {
        self.redirected
    }

    /// The `Content-Type` header, if present and valid UTF-8.
    pub fn content_type(&self) -> Option<&str> {
        self.headers.get(CONTENT_TYPE)?.to_str().ok()
    }

    /// The next chunk of the (decoded) body, or `None` at the end.
    ///
    /// Fails with [`ErrorKind::TooLarge`] once the body exceeds the maximum
    /// response size, and with [`ErrorKind::Timeout`] once the request's
    /// deadline passes.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        self.body.chunk(&self.url).await
    }

    /// Reads the whole (decoded) body.
    pub async fn bytes(mut self) -> Result<Bytes> {
        let Some(first) = self.chunk().await? else {
            return Ok(Bytes::new());
        };
        let Some(second) = self.chunk().await? else {
            // The common single-chunk case needs no copy.
            return Ok(first);
        };
        let mut buf = BytesMut::with_capacity(self.body.capacity_hint(first.len() + second.len()));
        buf.extend_from_slice(&first);
        buf.extend_from_slice(&second);
        drop((first, second));
        while let Some(chunk) = self.chunk().await? {
            buf.extend_from_slice(&chunk);
        }
        Ok(buf.freeze())
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("url", &self.url.as_str())
            .field("redirected", &self.redirected)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

/// A response body that enforces the size limit and the deadline, and keeps
/// the request in flight until it is finished.
pub(crate) struct Body {
    source: Option<Incoming>,
    /// The most body bytes that may be delivered.
    limit: u64,
    /// Body bytes delivered so far.
    received: u64,
    /// The declared length, if the body is not content-encoded.
    content_length: Option<u64>,
    deadline: Option<Instant>,
    in_flight: Option<InFlight>,
}

impl Body {
    pub(crate) fn new(
        incoming: Incoming,
        headers: &HeaderMap,
        limit: u64,
        deadline: Option<Instant>,
        in_flight: InFlight,
    ) -> Self {
        Self {
            source: Some(incoming),
            limit,
            received: 0,
            content_length: content_length(headers),
            deadline,
            in_flight: Some(in_flight),
        }
    }

    /// A good initial buffer size for collecting the body, given that `seen`
    /// bytes have arrived already.
    fn capacity_hint(&self, seen: usize) -> usize {
        let declared = self.content_length.unwrap_or(0).min(self.limit);
        usize::try_from(declared)
            .unwrap_or(usize::MAX)
            .max(seen * 2)
    }

    async fn chunk(&mut self, url: &Url) -> Result<Option<Bytes>> {
        let result = with_deadline(self.deadline, self.next_chunk()).await;
        match result {
            Ok(Some(chunk)) => Ok(Some(chunk)),
            Ok(None) => {
                self.finish();
                Ok(None)
            }
            Err(error) => {
                self.finish();
                Err(error.with_url(url))
            }
        }
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>> {
        let Some(incoming) = &mut self.source else {
            return Ok(None);
        };
        loop {
            let Some(frame) = incoming.frame().await else {
                return Ok(None);
            };
            let frame = frame.map_err(|e| Error::new(ErrorKind::Http).with_source(e))?;
            // Trailers are ignored.
            if let Ok(data) = frame.into_data()
                && !data.is_empty()
            {
                self.received += data.len() as u64;
                if self.received > self.limit {
                    return Err(Error::new(ErrorKind::TooLarge { limit: self.limit }));
                }
                return Ok(Some(data));
            }
        }
    }

    /// Releases the connection and stops counting the request as in flight.
    fn finish(&mut self) {
        self.source = None;
        self.in_flight = None;
    }
}

/// Runs `future`, failing with a timeout error if `deadline` passes first.
pub(crate) async fn with_deadline<T>(
    deadline: Option<Instant>,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, future)
            .await
            .unwrap_or_else(|_| Err(Error::new(ErrorKind::Timeout))),
        None => future.await,
    }
}

/// The `Content-Length` header, if present and valid.
pub(crate) fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}
