//! `dasher serve` and `dasher lambda`, run as the real binary.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};
use serde_json::{Value, json};

fn site() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/data", |_| {
            FixtureResponse::json(r#"{"items": ["alpha", "beta"]}"#)
        })
        .start()
        .unwrap()
}

/// POSTs `body` to `path` and returns the status and JSON body.
fn post(addr: &str, path: &str, body: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

#[cfg(unix)]
#[test]
fn serve_answers_jobs_and_stops_on_sigterm() {
    let site = site();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dasher"))
        .args(["serve", "--host", "127.0.0.1", "--port", "0"])
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The first log line says where it listens.
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let listening: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(listening["message"], "listening", "{line}");
    let addr = listening["address"].as_str().unwrap().to_owned();

    let job = json!({
        "url": site.url("/app.html"),
        "extract": {"items": {"selector": ".item", "all": true}},
    });
    let (status, body) = post(&addr, "/scrape", &job.to_string());
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["extracted"]["items"], json!(["alpha", "beta"]));

    // The job was logged.
    line.clear();
    stderr.read_line(&mut line).unwrap();
    let logged: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(logged["message"], "job finished");
    assert_eq!(logged["severity"], "INFO");

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "still running");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status:?}");
}

#[test]
fn a_binary_named_bootstrap_is_a_lambda_runtime() {
    let site = site();
    let job = json!({"url": site.url("/app.html"), "extract": {"title": "title"}});
    let served = std::sync::atomic::AtomicBool::new(false);
    let api = FixtureServer::builder()
        .route("/2018-06-01/runtime/invocation/next", move |_| {
            if served.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return FixtureResponse::new(500).with_body("no more events");
            }
            FixtureResponse::json(job.to_string())
                .with_header("Lambda-Runtime-Aws-Request-Id", "req-1")
                .with_header("Lambda-Runtime-Deadline-Ms", "99999999999999")
        })
        .route("/2018-06-01/runtime/invocation/req-1/response", |_| {
            FixtureResponse::new(202)
        })
        .start()
        .unwrap();

    let dir = std::env::temp_dir().join(format!("dasher-bootstrap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bootstrap = dir.join("bootstrap");
    std::fs::copy(env!("CARGO_BIN_EXE_dasher"), &bootstrap).unwrap();
    let output = Command::new(&bootstrap)
        .env("AWS_LAMBDA_RUNTIME_API", api.addr().to_string())
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    // It stops when the Runtime API fails, as Lambda expects.
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no more events"), "{stderr}");
    let response = api
        .requests()
        .into_iter()
        .find(|r| r.path.ends_with("/req-1/response"))
        .expect("a response was posted");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["extracted"]["title"], "App", "{body}");
}

#[test]
fn lambda_mode_needs_the_runtime_api() {
    let output = Command::new(env!("CARGO_BIN_EXE_dasher"))
        .arg("lambda")
        .env_remove("AWS_LAMBDA_RUNTIME_API")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("AWS_LAMBDA_RUNTIME_API"));
}

#[test]
fn service_help_describes_both_modes() {
    let output = Command::new(env!("CARGO_BIN_EXE_dasher"))
        .args(["serve", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("dasher serve [options]") && help.contains("dasher lambda"));
}
