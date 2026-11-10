//! Servers for the client tests that the fixture server can't stand in for:
//! raw TCP for malformed or stalled responses, and TLS with HTTP/2.

#![allow(dead_code)]

use std::convert::Infallible;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::sync::oneshot;

/// Starts a server that answers each of the next `replies.len()` connections
/// with the corresponding raw bytes after reading the request head, then
/// waits `hold` before closing. Returns its address.
pub fn raw_server(replies: Vec<Vec<u8>>, hold: Duration) -> SocketAddr {
    let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => head.push(byte[0]),
                        _ => return,
                    }
                }
                let _ = stream.write_all(&reply);
                let _ = stream.flush();
                thread::sleep(hold);
            });
        }
    });
    addr
}

/// A TLS server for `localhost` speaking HTTP/1.1 and HTTP/2 (by ALPN) with a
/// self-signed certificate. Every response body is `"<version> <path>"`.
pub struct TlsServer {
    pub addr: SocketAddr,
    /// The DER certificate to trust.
    pub certificate: Vec<u8>,
    connections: Arc<AtomicUsize>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TlsServer {
    pub fn start() -> Self {
        let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = key.cert.der().to_vec();
        let private_key =
            rustls::pki_types::PrivateKeyDer::try_from(key.signing_key.serialize_der()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone().into()], private_key)
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let (shutdown, mut shutdown_rx) = oneshot::channel();
        let counter = Arc::clone(&connections);
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { continue };
                            counter.fetch_add(1, Ordering::SeqCst);
                            let acceptor = acceptor.clone();
                            tokio::spawn(async move {
                                let Ok(tls) = acceptor.accept(stream).await else { return };
                                let service = service_fn(|req: Request<hyper::body::Incoming>| async move {
                                    let body = format!("{:?} {}", req.version(), req.uri().path());
                                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
                                });
                                let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                                    .serve_connection(TokioIo::new(tls), service)
                                    .await;
                            });
                        }
                    }
                }
            });
        });
        Self {
            addr,
            certificate,
            connections,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("https://localhost:{}{path}", self.addr.port())
    }

    /// The number of TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
