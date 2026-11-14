//! An AWS Lambda custom runtime, for the `provided.al2023` runtime or a
//! container image.
//!
//! [`run_lambda`] speaks the [Lambda Runtime API] directly (it is a few
//! plain HTTP calls to a local endpoint), so no Lambda SDK is linked in.
//! It takes two kinds of event:
//!
//! - A [`Job`] itself, from a direct invocation (`aws lambda invoke`, an
//!   SDK, Step Functions). The response is the job's JSON body; a job
//!   that failed has an `error` field.
//! - An HTTP request from a function URL or API Gateway (payload format
//!   1.0 or 2.0). `POST` with a job as the body runs it; `GET` reports
//!   health. The response carries the same status codes as the HTTP
//!   server's.
//!
//! [Lambda Runtime API]: https://docs.aws.amazon.com/lambda/latest/dg/runtimes-api.html

use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::SendRequest;
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;

use crate::job::Job;
use crate::log;
use crate::run::{self, Outcome, RunOptions};

/// The environment variable Lambda puts the Runtime API's address in.
pub const RUNTIME_API_ENV: &str = "AWS_LAMBDA_RUNTIME_API";

const API_PREFIX: &str = "/2018-06-01/runtime";

/// How the Lambda runtime runs jobs.
#[derive(Clone, Debug)]
pub struct LambdaConfig {
    /// The longest a job may take; the invocation's deadline cuts it
    /// shorter still.
    pub max_timeout: Duration,
    /// URL patterns every job refuses; the cloud metadata endpoints by
    /// default. The Runtime API is always refused too.
    pub blocked: Vec<String>,
    /// Whether pages are refused connections to private, loopback and
    /// link-local addresses, the Runtime API's among them; see
    /// [`RunOptions::block_private_addresses`]. On by default.
    pub block_private_addresses: bool,
    /// Whether to log a line to stderr per job.
    pub log: bool,
    /// Compiled scripts kept for later invocations of a warm instance (see
    /// [`Config::script_cache`](crate::Config::script_cache)). Give it a
    /// directory under `/tmp` to keep them across cold starts of an
    /// instance's sandbox too.
    pub script_cache: Option<dasher::ScriptCache>,
}

impl Default for LambdaConfig {
    fn default() -> Self {
        Self {
            // Lambda's own limit.
            max_timeout: Duration::from_secs(15 * 60),
            blocked: RunOptions::default().blocked,
            block_private_addresses: true,
            log: true,
            script_cache: Some(dasher::ScriptCache::new(
                dasher::ScriptCacheOptions::default(),
            )),
        }
    }
}

/// Runs invocations from the Runtime API at `api` (`host:port`, as in
/// [`RUNTIME_API_ENV`]) one at a time, each in a fresh page.
///
/// Returns only if the Runtime API fails or refuses a request for the
/// next invocation; Lambda then starts a new instance. Must run on a
/// current-thread Tokio runtime, as pages do.
pub async fn run_lambda(api: &str, config: LambdaConfig) -> io::Result<()> {
    let mut client = RuntimeApi::new(api);
    let mut options = RunOptions {
        max_timeout: config.max_timeout,
        deadline: None,
        blocked: config.blocked,
        block_private_addresses: config.block_private_addresses,
        script_cache: config.script_cache,
    };
    // Pages must not reach the Runtime API and answer for us.
    options.blocked.push(format!("*://{api}/*"));
    loop {
        let (status, headers, event) = client
            .call(Method::GET, &format!("{API_PREFIX}/invocation/next"), None)
            .await?;
        if status != StatusCode::OK {
            return Err(io::Error::other(format!(
                "the Runtime API answered {status} for the next invocation: {}",
                String::from_utf8_lossy(&event)
            )));
        }
        let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
        let Some(request_id) = header("lambda-runtime-aws-request-id").map(str::to_owned) else {
            return Err(io::Error::other("an invocation came without a request id"));
        };
        options.deadline = header("lambda-runtime-deadline-ms")
            .and_then(|ms| ms.parse().ok())
            .map(instant_from_epoch_ms);

        let response = handle(&event, &options, config.log).await;

        let path = format!("{API_PREFIX}/invocation/{request_id}/response");
        let (status, _, body) = client.call(Method::POST, &path, Some(response)).await?;
        if !status.is_success() {
            // Most likely the response was too large; say so instead.
            let message = format!(
                "the Runtime API refused the response ({status}): {}",
                String::from_utf8_lossy(&body)
            );
            if config.log {
                log::log("ERROR", &message, json!({ "request_id": request_id }));
            }
            let error = json!({ "errorMessage": message, "errorType": "Dasher.ResponseRejected" });
            let path = format!("{API_PREFIX}/invocation/{request_id}/error");
            client
                .call(Method::POST, &path, Some(error.to_string().into()))
                .await?;
        }
    }
}

/// The response payload for an invocation's event.
async fn handle(event: &[u8], options: &RunOptions, log: bool) -> Bytes {
    let event: Value = match serde_json::from_slice(event) {
        Ok(event) => event,
        Err(e) => {
            return json!({ "error": format!("invalid event: {e}") })
                .to_string()
                .into();
        }
    };
    let reply = if is_http_event(&event) {
        http_event(&event, options, log).await
    } else {
        match Job::from_value(event) {
            Ok(job) => run_job(&job, options, log).await.body,
            Err(e) => json!({ "error": e.to_string() }),
        }
    };
    reply.to_string().into()
}

async fn run_job(job: &Job, options: &RunOptions, log: bool) -> Outcome {
    let outcome = run::run(job, options, Duration::ZERO).await;
    if log {
        log::job(job, &outcome);
    }
    outcome
}

/// Whether `event` is an HTTP request from a function URL or API Gateway
/// rather than a job.
fn is_http_event(event: &Value) -> bool {
    event.get("requestContext").is_some_and(Value::is_object)
        && (event.get("httpMethod").is_some() || event.pointer("/requestContext/http").is_some())
}

/// Answers an HTTP event in the API Gateway proxy response format.
async fn http_event(event: &Value, options: &RunOptions, log: bool) -> Value {
    let method = event
        .pointer("/requestContext/http/method")
        .or_else(|| event.get("httpMethod"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let outcome = match method {
        "GET" | "HEAD" => Outcome {
            status: 200,
            body: json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }),
        },
        "POST" => match http_body(event).map(|body| Job::from_slice(&body)) {
            Ok(Ok(job)) => run_job(&job, options, log).await,
            Ok(Err(e)) => Outcome::error(400, e.to_string()),
            Err(e) => Outcome::error(400, e),
        },
        _ => Outcome::error(405, "method not allowed"),
    };
    json!({
        "statusCode": outcome.status,
        "headers": { "content-type": "application/json" },
        "body": outcome.body.to_string(),
        "isBase64Encoded": false,
    })
}

/// The body of an HTTP event, decoded from base64 if need be.
fn http_body(event: &Value) -> Result<Vec<u8>, String> {
    let body = event
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event.get("isBase64Encoded").and_then(Value::as_bool) == Some(true) {
        decode_base64(body).ok_or_else(|| "the request body is not valid base64".to_owned())
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

/// Decodes standard base64, padded or not.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    }
    let text = text.trim_end_matches('=').as_bytes();
    if text.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for chunk in text.chunks(4) {
        let mut bits = 0;
        for (i, &c) in chunk.iter().enumerate() {
            bits |= value(c)? << (18 - 6 * i);
        }
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

/// The `Instant` of a Unix time in milliseconds.
fn instant_from_epoch_ms(ms: u64) -> Instant {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let left = Duration::from_millis(ms).saturating_sub(now);
    Instant::now() + left
}

/// A connection to the Runtime API.
struct RuntimeApi {
    host: String,
    sender: Option<SendRequest<Full<Bytes>>>,
}

impl RuntimeApi {
    fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
            sender: None,
        }
    }

    /// Makes a request, reconnecting once if a kept-alive connection has
    /// gone stale.
    async fn call(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Bytes>,
    ) -> io::Result<(StatusCode, HeaderMap, Bytes)> {
        let reused = self.sender.as_ref().is_some_and(|s| !s.is_closed());
        match self.try_call(&method, path, body.clone()).await {
            Err(_) if reused => {
                self.sender = None;
                self.try_call(&method, path, body).await
            }
            result => result,
        }
    }

    async fn try_call(
        &mut self,
        method: &Method,
        path: &str,
        body: Option<Bytes>,
    ) -> io::Result<(StatusCode, HeaderMap, Bytes)> {
        let mut sender = match self.sender.take() {
            Some(sender) if !sender.is_closed() => sender,
            _ => self.connect().await?,
        };
        sender.ready().await.map_err(io::Error::other)?;
        let mut request = Request::new(Full::new(body.unwrap_or_default()));
        *request.method_mut() = method.clone();
        *request.uri_mut() = path.parse().map_err(io::Error::other)?;
        let host = HeaderValue::from_str(&self.host).map_err(io::Error::other)?;
        request.headers_mut().insert(header::HOST, host);
        if method == Method::POST {
            request.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        let response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        let (parts, body) = response.into_parts();
        let body = body.collect().await.map_err(io::Error::other)?.to_bytes();
        self.sender = Some(sender);
        Ok((parts.status, parts.headers, body))
    }

    async fn connect(&self) -> io::Result<SendRequest<Full<Bytes>>> {
        let stream = TcpStream::connect(&self.host).await?;
        let _ = stream.set_nodelay(true);
        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(io::Error::other)?;
        tokio::spawn(async move {
            // A failed connection shows up as an error on the next call.
            let _ = connection.await;
        });
        Ok(sender)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_with_and_without_padding() {
        let decode = |s| decode_base64(s).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(decode("").as_deref(), Some(""));
        assert_eq!(
            decode("eyJ1cmwiOiJ4In0=").as_deref(),
            Some(r#"{"url":"x"}"#)
        );
        assert_eq!(decode("eyJ1cmwiOiJ4In0").as_deref(), Some(r#"{"url":"x"}"#));
        assert_eq!(decode("YQ==").as_deref(), Some("a"));
        assert_eq!(decode("YWI=").as_deref(), Some("ab"));
        assert_eq!(decode("YWJj").as_deref(), Some("abc"));
        assert_eq!(decode_base64("+/+/"), Some(vec![0xfb, 0xff, 0xbf]));
        assert_eq!(decode_base64("Y"), None);
        assert_eq!(decode_base64("Y!=="), None);
    }

    #[test]
    fn http_events_are_told_apart_from_jobs() {
        assert!(is_http_event(
            &json!({"requestContext": {"http": {"method": "POST"}}, "body": "{}"})
        ));
        assert!(is_http_event(
            &json!({"requestContext": {}, "httpMethod": "POST"})
        ));
        assert!(!is_http_event(&json!({"url": "https://example.com/"})));
        assert!(!is_http_event(&json!({"url": "u", "requestContext": 1})));
    }

    #[test]
    fn deadlines_in_the_past_are_now() {
        let before = Instant::now();
        assert!(instant_from_epoch_ms(0) >= before);
        let soon = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + Duration::from_secs(5);
        let instant = instant_from_epoch_ms(soon.as_millis() as u64);
        assert!(instant > before + Duration::from_secs(4));
    }
}
