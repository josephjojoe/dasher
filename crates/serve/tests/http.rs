//! The HTTP server, over real sockets.

mod common;

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dasher_serve::{Config, Server};
use serde_json::{Value, json};
use tokio::sync::oneshot;

/// A server running on a thread of its own.
struct Running {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Running {
    fn start(config: Config) -> Self {
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        let (shutdown, stop) = oneshot::channel::<()>();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let server = Server::bind(([127, 0, 0, 1], 0).into(), config).await?;
                addr_tx.send(server.local_addr()?).unwrap();
                server
                    .run(async {
                        let _ = stop.await;
                    })
                    .await
            })
        });
        Self {
            addr: addr_rx.recv().unwrap(),
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        request(self.addr, method, path, headers, body)
    }

    fn scrape(&self, job: Value) -> Reply {
        self.request("POST", "/scrape", &[], &job.to_string())
    }

    fn health(&self) -> Value {
        self.request("GET", "/health", &[], "").json()
    }

    /// Waits until `predicate` holds for the health report.
    fn wait_for_health(&self, predicate: impl Fn(&Value) -> bool) {
        let started = Instant::now();
        while !predicate(&self.health()) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{}",
                self.health()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Signals shutdown, as SIGTERM would.
    fn begin_shutdown(&mut self) {
        let _ = self.shutdown.take().unwrap().send(());
    }

    /// Waits for `run` to return.
    fn join(&mut self) {
        self.thread.take().unwrap().join().unwrap().unwrap();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn quiet() -> Config {
    Config {
        log: false,
        ..Config::default()
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

/// A minimal HTTP/1.1 client: one request per connection.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();

    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_owned(),
    }
}

#[test]
fn health_reports_the_service() {
    let server = Running::start(quiet());
    for path in ["/health", "/healthz"] {
        let reply = server.request("GET", path, &[], "");
        assert_eq!(reply.status, 200);
        assert_eq!(reply.header("content-type"), Some("application/json"));
        let health = reply.json();
        assert_eq!(health["status"], "ok");
        assert_eq!(health["running"], 0);
        assert_eq!(health["max_concurrency"], 4);
        assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    }
    let reply = server.request("POST", "/health", &[], "");
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("GET, HEAD"));
}

#[test]
fn scrape_runs_a_job() {
    let site = common::site();
    let server = Running::start(quiet());
    let reply = server.scrape(json!({
        "url": site.url("/shop.html"),
        "extract": {"names": {"selector": ".product a", "all": true}},
    }));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let body = reply.json();
    assert_eq!(body["status"], 200);
    assert_eq!(body["extracted"]["names"], json!(["Kettle", "Toaster"]));
    assert!(body["timings"]["total_ms"].is_u64());

    // Failures have a status of their own and an error message.
    let reply = server.scrape(json!({"url": "ftp://x/"}));
    assert_eq!(reply.status, 400);
    assert!(reply.json()["error"].as_str().unwrap().contains("http(s)"));
}

#[test]
fn bad_requests_are_refused() {
    let server = Running::start(quiet());
    let reply = server.request("POST", "/scrape", &[], "{not json");
    assert_eq!(reply.status, 400);
    assert!(
        reply.json()["error"]
            .as_str()
            .unwrap()
            .starts_with("invalid job")
    );

    let reply = server.request("POST", "/scrape", &[], r#"{"url": "x", "bogus": 1}"#);
    assert_eq!(reply.status, 400);
    assert!(reply.json()["error"].as_str().unwrap().contains("bogus"));

    let reply = server.request("GET", "/scrape", &[], "");
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("POST"));

    assert_eq!(server.request("GET", "/", &[], "").status, 404);

    // Refused on its declared length, before the body is sent.
    let mut stream = TcpStream::connect(server.addr).unwrap();
    let head = format!(
        "POST /scrape HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
        dasher_serve::MAX_JOB_SIZE + 1
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 413 "), "{reply}");
}

#[test]
fn a_token_guards_scraping_but_not_health() {
    let site = common::site();
    let server = Running::start(Config {
        auth_token: Some("s3cret".into()),
        ..quiet()
    });
    let job = json!({"url": site.url("/shop.html"), "wait": "load"}).to_string();

    let reply = server.request("POST", "/scrape", &[], &job);
    assert_eq!(reply.status, 401);
    assert_eq!(reply.header("www-authenticate"), Some("Bearer"));
    let wrong = [("Authorization", "Bearer nope")];
    assert_eq!(server.request("POST", "/scrape", &wrong, &job).status, 401);
    assert_eq!(site.request_count(), 0);

    let right = [("Authorization", "Bearer s3cret")];
    assert_eq!(server.request("POST", "/scrape", &right, &job).status, 200);
    assert_eq!(server.request("GET", "/health", &[], "").status, 200);
}

#[test]
fn jobs_past_the_cap_and_queue_are_turned_away() {
    let site = common::site();
    let server = Running::start(Config {
        max_concurrency: 1,
        max_queue: 1,
        ..quiet()
    });
    let slow = json!({"url": site.url("/slow.html"), "wait": "load"});

    thread::scope(|s| {
        let first = s.spawn(|| server.scrape(slow.clone()));
        server.wait_for_health(|h| h["running"] == 1);
        let second = s.spawn(|| server.scrape(slow.clone()));
        server.wait_for_health(|h| h["waiting"] == 1);

        let third = server.scrape(slow.clone());
        assert_eq!(third.status, 503);
        assert_eq!(third.header("retry-after"), Some("1"));

        assert_eq!(first.join().unwrap().status, 200);
        let second = second.join().unwrap();
        assert_eq!(second.status, 200);
        // It waited for the first to finish.
        let queued = second.json()["timings"]["queued_ms"].as_u64().unwrap();
        assert!(queued >= 500, "queued for {queued}ms");
    });
    server.wait_for_health(|h| h["running"] == 0 && h["waiting"] == 0);
}

#[test]
fn shutdown_lets_running_jobs_finish() {
    let site = common::site();
    let mut server = Running::start(quiet());
    let addr = server.addr;
    let slow = json!({"url": site.url("/slow.html"), "wait": "load"});

    let job = thread::spawn(move || request(addr, "POST", "/scrape", &[], &slow.to_string()));
    server.wait_for_health(|h| h["running"] == 1);
    server.begin_shutdown();

    // New connections are refused straight away...
    let started = Instant::now();
    while TcpStream::connect(addr).is_ok() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "still accepting"
        );
        thread::sleep(Duration::from_millis(10));
    }
    // ...but the running job still answers.
    let reply = job.join().unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(reply.json()["title"], "Slow");
    server.join();
}

#[test]
fn shutdown_gives_up_on_jobs_after_its_timeout() {
    let site = common::site();
    let mut server = Running::start(Config {
        shutdown_timeout: Duration::from_millis(100),
        ..quiet()
    });
    let addr = server.addr;
    let slow = json!({"url": site.url("/slow.html"), "wait": "load"});
    let _job = thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        let body = slow.to_string();
        let head = format!(
            "POST /scrape HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        let _ = stream.read(&mut [0; 64]);
    });
    server.wait_for_health(|h| h["running"] == 1);

    let started = Instant::now();
    server.begin_shutdown();
    server.join();
    assert!(started.elapsed() < Duration::from_millis(800));
}

#[test]
fn a_client_going_away_stops_its_job() {
    let site = common::site();
    let server = Running::start(quiet());
    let slow = json!({"url": site.url("/slow.html"), "wait": "load"}).to_string();
    let mut stream = TcpStream::connect(server.addr).unwrap();
    let head = format!(
        "POST /scrape HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
        slow.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(slow.as_bytes()).unwrap();
    server.wait_for_health(|h| h["running"] == 1);

    let started = Instant::now();
    drop(stream);
    server.wait_for_health(|h| h["running"] == 0);
    assert!(started.elapsed() < Duration::from_millis(800));
}
