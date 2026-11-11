//! The HTTP server, for Cloud Run and anything else that speaks HTTP.
//!
//! - `POST /scrape` takes a [`Job`] as JSON and answers with its
//!   [`Outcome`].
//! - `GET /health` reports `{"status": "ok", …}`, or 503 with
//!   `"draining"` once shutdown has begun. (`/healthz` works too, but
//!   Cloud Run reserves paths ending in `z`.)

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::job::Job;
use crate::log;
use crate::run::Outcome;
use crate::service::{Config, MAX_JOB_SIZE, Service};

/// The HTTP server: a listening socket and the [`Service`] behind it.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    service: Arc<Service>,
}

impl Server {
    /// Binds to `addr` (port 0 picks a free one).
    pub async fn bind(addr: SocketAddr, config: Config) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            service: Arc::new(Service::new(config)),
        })
    }

    /// The address the server listens on.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The service behind the server.
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// Serves until `shutdown` completes, then shuts down gracefully: it
    /// stops accepting connections, turns new jobs away, and waits up to
    /// [`Config::shutdown_timeout`] for running jobs to answer before
    /// returning. Pass [`shutdown_signal`] to stop on `SIGTERM` or
    /// Ctrl-C.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> io::Result<()> {
        let Self { listener, service } = self;
        let config = service.config();
        if config.log {
            log::log(
                "INFO",
                "listening",
                json!({
                    "address": listener.local_addr()?.to_string(),
                    "max_concurrency": config.max_concurrency,
                    "version": env!("CARGO_PKG_VERSION"),
                }),
            );
        }
        let mut http = hyper::server::conn::http1::Builder::new();
        http.timer(TokioTimer::new());
        let graceful = GracefulShutdown::new();
        let mut shutdown = std::pin::pin!(shutdown);

        loop {
            let stream = tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        // Out of file descriptors, most likely; let some close.
                        log::log("ERROR", "accept failed", json!({ "error": e.to_string() }));
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                },
                () = &mut shutdown => break,
            };
            let _ = stream.set_nodelay(true);
            let service = Arc::clone(&service);
            let connection = http.serve_connection(
                TokioIo::new(stream),
                service_fn(move |request| handle(Arc::clone(&service), request)),
            );
            let connection = graceful.watch(connection);
            tokio::spawn(async move {
                // Errors are the client's doing (resets, bad requests).
                let _ = connection.await;
            });
        }

        drop(listener);
        service.start_draining();
        let config = service.config();
        if config.log {
            log::log(
                "INFO",
                "shutting down",
                json!({ "running": service.running(), "waiting": service.waiting() }),
            );
        }
        if tokio::time::timeout(config.shutdown_timeout, graceful.shutdown())
            .await
            .is_err()
            && config.log
        {
            log::log(
                "WARNING",
                "shutdown timeout reached; abandoning running jobs",
                json!({ "running": service.running() }),
            );
        }
        Ok(())
    }
}

/// Completes on `SIGTERM` (how Cloud Run and container runtimes stop an
/// instance) or Ctrl-C.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
                return;
            }
            Err(e) => log::log(
                "WARNING",
                "cannot listen for SIGTERM",
                json!({ "error": e.to_string() }),
            ),
        }
    }
    if tokio::signal::ctrl_c().await.is_err() {
        // No way to be told to stop: run until killed.
        std::future::pending::<()>().await;
    }
}

type Body = Full<Bytes>;

async fn handle(
    service: Arc<Service>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path();
    Ok(match path {
        "/health" | "/healthz" => match *request.method() {
            Method::GET | Method::HEAD => {
                let status = if service.is_draining() {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                };
                json_response(status, &service.health())
            }
            _ => method_not_allowed("GET, HEAD"),
        },
        "/scrape" => match *request.method() {
            Method::POST => scrape(&service, request).await,
            _ => method_not_allowed("POST"),
        },
        _ => error_response(StatusCode::NOT_FOUND, "not found; POST jobs to /scrape"),
    })
}

async fn scrape(service: &Service, request: Request<Incoming>) -> Response<Body> {
    let allowed = match &service.config().auth_token {
        Some(token) => authorized(request.headers().get(header::AUTHORIZATION), token),
        None => true,
    };
    let too_large = || {
        error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("jobs are limited to {MAX_JOB_SIZE} bytes"),
        )
    };
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_JOB_SIZE as u64) {
        return too_large();
    }
    let body = match Limited::new(request.into_body(), MAX_JOB_SIZE)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(e) if e.is::<LengthLimitError>() => return too_large(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("cannot read the job: {e}"),
            );
        }
    };
    // Checked after reading the body: answering before it has all
    // arrived would close the connection under the client's feet.
    if !allowed {
        let mut response = error_response(StatusCode::UNAUTHORIZED, "unauthorized");
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return response;
    }
    let job = match Job::from_slice(&body) {
        Ok(job) => job,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    };
    outcome_response(service.submit(job).await)
}

/// Whether `header` is `Bearer <token>`, compared in constant time.
fn authorized(header: Option<&HeaderValue>, token: &str) -> bool {
    let Some(given) = header
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (given, token) = (given.trim().as_bytes(), token.as_bytes());
    given.len() == token.len() && given.iter().zip(token).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn outcome_response(outcome: Outcome) -> Response<Body> {
    let status = StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = json_response(status, &outcome.body);
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}

fn method_not_allowed(allow: &'static str) -> Response<Body> {
    let mut response = error_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static(allow));
    response
}

fn error_response(status: StatusCode, message: &str) -> Response<Body> {
    json_response(status, &json!({ "error": message }))
}

fn json_response(status: StatusCode, body: &Value) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_tokens_must_match_exactly() {
        let check = |header: Option<&str>| {
            let header = header.map(|h| HeaderValue::from_str(h).unwrap());
            authorized(header.as_ref(), "s3cret")
        };
        assert!(check(Some("Bearer s3cret")));
        assert!(!check(None));
        assert!(!check(Some("s3cret")));
        assert!(!check(Some("Bearer s3cre")));
        assert!(!check(Some("Bearer s3cretx")));
        assert!(!check(Some("Basic s3cret")));
    }
}
