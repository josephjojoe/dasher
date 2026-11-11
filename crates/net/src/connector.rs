//! Connecting to servers: directly, or through an HTTP proxy.

use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use hyper::Uri;
use hyper::header::HeaderValue;
use hyper::http::uri::Scheme;
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_rustls::{HttpsConnector, MaybeHttpsStream};
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tower_service::Service;
use url::Url;

use crate::client::basic_auth;
use crate::error::{BoxError, Error, ErrorKind, Result};
use crate::guard::{self, GuardedHttp};

/// An HTTP proxy that every request goes through.
///
/// Plain `http` requests are sent to the proxy in absolute form; `https`
/// requests are tunnelled with `CONNECT`, so TLS still runs end to end. Only
/// `http://` proxies are supported. Credentials in the proxy URL are sent as
/// `Proxy-Authorization: Basic`.
///
/// With [`ClientBuilder::block_private_addresses`] on, the proxy decides
/// where requests through it go, so only part of the check can be made:
/// the proxy's own address is checked like any other, and so are target
/// URLs whose host is a literal IP, but a host name is resolved by the
/// proxy, out of dasher's sight. Refusing private addresses for those is
/// up to the proxy.
///
/// [`ClientBuilder::block_private_addresses`]: crate::ClientBuilder::block_private_addresses
#[derive(Clone)]
pub struct Proxy {
    uri: Uri,
    authorization: Option<HeaderValue>,
}

impl Proxy {
    /// A proxy at `url`, e.g. `http://user:password@proxy.internal:3128`.
    pub fn http(url: &str) -> Result<Self> {
        let invalid = |reason: &str| {
            Error::new(ErrorKind::Config)
                .with_source(format!("invalid proxy URL {url:?}: {reason}"))
        };
        let parsed = Url::parse(url).map_err(|e| invalid(&e.to_string()))?;
        if parsed.scheme() != "http" {
            return Err(invalid("only http:// proxies are supported"));
        }
        let host = parsed.host_str().ok_or_else(|| invalid("no host"))?;
        let port = parsed.port_or_known_default().unwrap_or(80);
        let uri = format!("http://{host}:{port}")
            .parse()
            .map_err(|e: hyper::http::uri::InvalidUri| invalid(&e.to_string()))?;
        let authorization =
            (!parsed.username().is_empty() || parsed.password().is_some()).then(|| {
                let decode = |s: &str| percent_encoding::percent_decode_str(s).collect::<Vec<u8>>();
                basic_auth(
                    &decode(parsed.username()),
                    &decode(parsed.password().unwrap_or("")),
                )
            });
        Ok(Self { uri, authorization })
    }

    /// The `Proxy-Authorization` value to send, if the proxy has credentials.
    pub(crate) fn authorization(&self) -> Option<&HeaderValue> {
        self.authorization.as_ref()
    }
}

impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the credentials.
        f.debug_struct("Proxy")
            .field("uri", &self.uri)
            .field("authorization", &self.authorization.is_some())
            .finish()
    }
}

type Stream = MaybeHttpsStream<TokioIo<TcpStream>>;

/// Makes connections for the pool.
#[derive(Clone)]
pub(crate) enum Connector {
    Direct(HttpsConnector<GuardedHttp>),
    Proxied(Box<Proxied>),
}

#[derive(Clone)]
pub(crate) struct Proxied {
    /// For `https`: TLS inside a `CONNECT` tunnel.
    tunnel: HttpsConnector<Tunnel<GuardedHttp>>,
    /// For `http`: a plain connection to the proxy itself.
    http: GuardedHttp,
    proxy: Uri,
}

impl Connector {
    pub(crate) fn new(
        http: GuardedHttp,
        tls: rustls::ClientConfig,
        http2: bool,
        proxy: Option<&Proxy>,
    ) -> Self {
        let Some(proxy) = proxy else {
            return Self::Direct(with_tls(http, tls, http2));
        };
        let mut tunnel = Tunnel::new(proxy.uri.clone(), http.clone());
        if let Some(authorization) = &proxy.authorization {
            tunnel = tunnel.with_auth(authorization.clone());
        }
        Self::Proxied(Box::new(Proxied {
            tunnel: with_tls(tunnel, tls, http2),
            http,
            proxy: proxy.uri.clone(),
        }))
    }
}

/// Adds TLS for `https` URLs on top of `connector`, offering HTTP/2 by ALPN
/// if `http2`.
fn with_tls<C>(connector: C, tls: rustls::ClientConfig, http2: bool) -> HttpsConnector<C> {
    let builder = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http();
    if http2 {
        builder.enable_all_versions().wrap_connector(connector)
    } else {
        builder.enable_http1().wrap_connector(connector)
    }
}

/// A connection, remembering whether it goes to a proxy that expects
/// absolute-form requests.
pub(crate) struct Conn {
    stream: Stream,
    proxied: bool,
}

impl Connection for Conn {
    fn connected(&self) -> Connected {
        self.stream.connected().proxy(self.proxied)
    }
}

impl Read for Conn {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl Write for Conn {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }
}

type ConnectFuture = Pin<Box<dyn Future<Output = Result<Conn, BoxError>> + Send>>;

impl Service<Uri> for Connector {
    type Response = Conn;
    type Error = BoxError;
    type Future = ConnectFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        match self {
            Self::Direct(https) => https.poll_ready(cx),
            Self::Proxied(proxied) => {
                ready!(proxied.tunnel.poll_ready(cx))?;
                proxied.http.poll_ready(cx).map_err(Into::into)
            }
        }
    }

    fn call(&mut self, dst: Uri) -> ConnectFuture {
        match self {
            Self::Direct(https) => {
                let connecting = https.call(dst);
                Box::pin(async move {
                    Ok(Conn {
                        stream: connecting.await?,
                        proxied: false,
                    })
                })
            }
            Self::Proxied(proxied) => {
                if let Err(e) = proxied.check_target(&dst) {
                    return Box::pin(async move { Err(e) });
                }
                if dst.scheme() == Some(&Scheme::HTTPS) {
                    proxied.tunnel_to(dst)
                } else {
                    proxied.http_via()
                }
            }
        }
    }
}

impl Proxied {
    /// The proxy connects to the target itself, so only a literal IP in
    /// its URL can be checked here.
    fn check_target(&self, dst: &Uri) -> Result<(), BoxError> {
        match guard::literal_host(dst) {
            Some(ip) => Ok(guard::check(self.http.guard(), ip)?),
            None => Ok(()),
        }
    }

    fn tunnel_to(&mut self, dst: Uri) -> ConnectFuture {
        let connecting = self.tunnel.call(dst);
        Box::pin(async move {
            Ok(Conn {
                stream: connecting.await?,
                proxied: false,
            })
        })
    }

    fn http_via(&mut self) -> ConnectFuture {
        let connecting = self.http.call(self.proxy.clone());
        Box::pin(async move {
            Ok(Conn {
                stream: MaybeHttpsStream::Http(connecting.await?),
                proxied: true,
            })
        })
    }
}
