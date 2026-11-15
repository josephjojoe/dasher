//! `Content-Encoding` decoding (gzip, deflate, brotli, zstd) with bounded
//! buffering.

use std::error::Error as StdError;
use std::fmt;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use async_compression::tokio::bufread::{
    BrotliDecoder, DeflateDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder,
};
use bytes::{Buf, Bytes};
use hyper::HeaderMap;
use hyper::body::{Body as _, Incoming};
use hyper::header::CONTENT_ENCODING;
use tokio::io::{AsyncBufRead, AsyncRead, BufReader, ReadBuf};

use crate::error::{Error, ErrorKind};

/// The `Accept-Encoding` sent by default: everything that can be decoded.
pub(crate) const ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";

/// The largest zstd window accepted, as a power of two: 8 MiB, the limit RFC
/// 8878 sets for HTTP and what browsers enforce. Without it a response could
/// make the decoder allocate up to 2 GiB.
const ZSTD_WINDOW_LOG_MAX: u32 = 23;

/// Buffer size between stacked decoders.
const LAYER_BUFFER: usize = 8 * 1024;

/// The most encodings a response may stack.
const MAX_ENCODINGS: usize = 4;

/// A content coding dasher can decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// The encodings in a response's `Content-Encoding`, in the order they were
/// applied. `identity` is skipped.
pub(crate) fn content_encodings(headers: &HeaderMap) -> Result<Vec<Encoding>, Error> {
    let mut encodings = Vec::new();
    for value in headers.get_all(CONTENT_ENCODING) {
        let value = String::from_utf8_lossy(value.as_bytes());
        for token in value.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let encoding =
                if token.eq_ignore_ascii_case("gzip") || token.eq_ignore_ascii_case("x-gzip") {
                    Encoding::Gzip
                } else if token.eq_ignore_ascii_case("deflate") {
                    Encoding::Deflate
                } else if token.eq_ignore_ascii_case("br") {
                    Encoding::Brotli
                } else if token.eq_ignore_ascii_case("zstd") {
                    Encoding::Zstd
                } else if token.eq_ignore_ascii_case("identity") {
                    continue;
                } else {
                    return Err(Error::new(ErrorKind::Decode)
                        .with_source(format!("unsupported Content-Encoding {token:?}")));
                };
            encodings.push(encoding);
        }
    }
    if encodings.len() > MAX_ENCODINGS {
        return Err(Error::new(ErrorKind::Decode).with_source("too many Content-Encodings"));
    }
    Ok(encodings)
}

/// Marks the I/O error raised when the body as received exceeds the limit.
#[derive(Debug)]
struct WireTooLarge;

impl fmt::Display for WireTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("response body too large")
    }
}

impl StdError for WireTooLarge {}

/// Reads a response body as received, failing once more than `limit` bytes
/// have arrived. It holds at most one frame at a time.
pub(crate) struct WireReader {
    body: Incoming,
    chunk: Bytes,
    received: u64,
    limit: u64,
}

impl WireReader {
    pub(crate) fn new(body: Incoming, limit: u64) -> Self {
        Self {
            body,
            chunk: Bytes::new(),
            received: 0,
            limit,
        }
    }

    /// Whether the body is known to be empty without reading it.
    pub(crate) fn is_end_stream(&self) -> bool {
        self.chunk.is_empty() && self.body.is_end_stream()
    }

    /// The next frame's data, or an empty `Bytes` at the end.
    pub(crate) async fn next_chunk(&mut self) -> io::Result<Bytes> {
        poll_fn(|cx| Pin::new(&mut *self).poll_fill_buf(cx).map_ok(|_| ())).await?;
        Ok(std::mem::take(&mut self.chunk))
    }

    /// Waits for data and returns (a cheap copy of) what is buffered: the
    /// start of the body if nothing was consumed yet. Empty at the end.
    pub(crate) async fn fill(&mut self) -> io::Result<Bytes> {
        poll_fn(|cx| Pin::new(&mut *self).poll_fill_buf(cx).map_ok(|_| ())).await?;
        Ok(self.chunk.clone())
    }
}

impl AsyncRead for WireReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let available = ready!(self.as_mut().poll_fill_buf(cx))?;
        let n = available.len().min(buf.remaining());
        buf.put_slice(&available[..n]);
        self.consume(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for WireReader {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        let this = self.get_mut();
        while this.chunk.is_empty() {
            let Some(frame) = ready!(Pin::new(&mut this.body).poll_frame(cx)) else {
                break;
            };
            let frame = frame.map_err(io::Error::other)?;
            // Trailers are ignored.
            if let Ok(data) = frame.into_data() {
                this.received += data.len() as u64;
                if this.received > this.limit {
                    return Poll::Ready(Err(io::Error::other(WireTooLarge)));
                }
                this.chunk = data;
            }
        }
        Poll::Ready(Ok(&this.chunk))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        self.get_mut().chunk.advance(amount);
    }
}

pub(crate) type Decoder = Pin<Box<dyn AsyncRead + Send>>;

/// Stacks decoders on `wire` to undo `encodings` (in the order they were
/// applied). `first_bytes` is the start of the body, used to tell zlib from
/// raw deflate.
pub(crate) fn decoder(wire: WireReader, encodings: &[Encoding], first_bytes: &[u8]) -> Decoder {
    let mut layers = encodings.iter().rev();
    let Some(&outermost) = layers.next() else {
        return Box::pin(wire);
    };
    let mut reader = layer(outermost, wire, looks_like_zlib(first_bytes));
    for &encoding in layers {
        reader = layer(
            encoding,
            BufReader::with_capacity(LAYER_BUFFER, reader),
            true,
        );
    }
    reader
}

fn layer<R>(encoding: Encoding, reader: R, zlib: bool) -> Decoder
where
    R: AsyncBufRead + Send + 'static,
{
    match encoding {
        Encoding::Gzip => Box::pin(GzipDecoder::new(reader)),
        // "deflate" is meant to be zlib, but some servers send raw deflate;
        // browsers accept both.
        Encoding::Deflate if zlib => Box::pin(ZlibDecoder::new(reader)),
        Encoding::Deflate => Box::pin(DeflateDecoder::new(reader)),
        Encoding::Brotli => Box::pin(BrotliDecoder::new(reader)),
        Encoding::Zstd => {
            let mut decoder = ZstdDecoder::with_params(
                reader,
                &[async_compression::zstd::DParameter::window_log_max(
                    ZSTD_WINDOW_LOG_MAX,
                )],
            );
            // A zstd body may be several frames back to back (RFC 8878,
            // section 3.1), which servers that flush early do send; without
            // this the body would end at the first frame.
            decoder.multiple_members(true);
            Box::pin(decoder)
        }
    }
}

/// Whether `bytes` start with a zlib header (RFC 1950): deflate method and a
/// valid header checksum.
fn looks_like_zlib(bytes: &[u8]) -> bool {
    match bytes {
        [cmf, flg, ..] => cmf & 0x0f == 8 && (u16::from(*cmf) << 8 | u16::from(*flg)) % 31 == 0,
        // Too little to tell; zlib is what the standard says.
        _ => true,
    }
}

/// Turns an I/O error from reading the body into the network error it
/// stands for.
pub(crate) fn body_error(error: io::Error, limit: u64) -> Error {
    match error.get_ref() {
        Some(inner) if inner.is::<WireTooLarge>() => Error::new(ErrorKind::TooLarge { limit }),
        Some(inner) if inner.is::<hyper::Error>() => {
            let source = error.into_inner().expect("checked above");
            Error::new(ErrorKind::Http).with_source(source)
        }
        _ => Error::new(ErrorKind::Decode).with_source(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    fn parse(values: &[&str]) -> Result<Vec<Encoding>, Error> {
        let mut headers = HeaderMap::new();
        for v in values {
            headers.append(CONTENT_ENCODING, HeaderValue::from_str(v).unwrap());
        }
        content_encodings(&headers)
    }

    #[test]
    fn parses_content_encoding() {
        use Encoding::*;
        assert_eq!(parse(&[]).unwrap(), []);
        assert_eq!(parse(&["identity"]).unwrap(), []);
        assert_eq!(parse(&["GZIP"]).unwrap(), [Gzip]);
        assert_eq!(parse(&["x-gzip"]).unwrap(), [Gzip]);
        assert_eq!(parse(&["deflate, br"]).unwrap(), [Deflate, Brotli]);
        assert_eq!(parse(&["gzip", " zstd ,"]).unwrap(), [Gzip, Zstd]);
        assert_eq!(parse(&["compress"]).unwrap_err().kind(), &ErrorKind::Decode);
        assert!(parse(&["gzip, gzip, gzip, gzip, gzip"]).is_err());
    }

    #[test]
    fn zlib_detection() {
        assert!(looks_like_zlib(&[0x78, 0x9c]));
        assert!(looks_like_zlib(&[0x78, 0x01]));
        assert!(looks_like_zlib(&[0x78, 0xda, 0xff]));
        assert!(!looks_like_zlib(&[0xcb, 0x48]));
        assert!(!looks_like_zlib(&[0x78, 0x9d]));
        assert!(looks_like_zlib(&[0x78]));
    }
}
