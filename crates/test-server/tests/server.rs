//! End-to-end tests for the fixture server, using a raw HTTP/1.1 client so
//! the server is tested independently of dasher's own network stack.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

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
}

fn send(addr: SocketAddr, method: &str, target: &str, extra: &str, body: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\
         Content-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();

    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap();
    let headers = lines
        .map(|line| {
            let (n, v) = line.split_once(':').unwrap();
            (n.trim().to_owned(), v.trim().to_owned())
        })
        .collect();
    Reply {
        status: status.parse().unwrap(),
        headers,
        body: body.to_owned(),
    }
}

fn get(addr: SocketAddr, target: &str) -> Reply {
    send(addr, "GET", target, "", "")
}

#[test]
fn serves_files_with_content_types() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();

    let reply = get(server.addr(), "/app.js");
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(reply.body, "document.title = \"loaded\";\n");

    let reply = get(server.addr(), "/");
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(reply.body.contains("fixture index"));
}

#[test]
fn directories_serve_their_index() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    assert_eq!(get(server.addr(), "/dir").body, "<p>nested index</p>\n");
    assert_eq!(get(server.addr(), "/dir/").body, "<p>nested index</p>\n");
}

#[test]
fn decodes_percent_encoded_paths() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let reply = get(server.addr(), "/with%20space.txt");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "spaced\n");
}

#[test]
fn missing_files_and_traversal_are_not_found() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    assert_eq!(get(server.addr(), "/nope.html").status, 404);
    assert_eq!(get(server.addr(), "/../Cargo.toml").status, 404);
    assert_eq!(get(server.addr(), "/%2e%2e/Cargo.toml").status, 404);
}

#[test]
fn head_has_headers_but_no_body() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let reply = send(server.addr(), "HEAD", "/app.js", "", "");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-length"), Some("27"));
    assert_eq!(reply.body, "");
}

#[test]
fn other_methods_on_files_are_rejected() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let reply = send(server.addr(), "POST", "/app.js", "", "x");
    assert_eq!(reply.status, 405);
    assert_eq!(reply.header("allow"), Some("GET, HEAD"));
}

#[test]
fn counts_and_records_requests() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    assert_eq!(server.request_count(), 0);

    get(server.addr(), "/app.js?v=1");
    get(server.addr(), "/app.js?v=2");
    get(server.addr(), "/nope.png");
    send(
        server.addr(),
        "POST",
        "/app.js",
        "X-Test: yes\r\n",
        "payload",
    );

    assert_eq!(server.request_count(), 4);
    assert_eq!(server.request_count_for("/app.js"), 3);
    assert_eq!(server.request_count_for("/nope.png"), 1);
    assert_eq!(server.request_count_for("/index.html"), 0);

    let requests = server.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/app.js");
    assert_eq!(requests[0].query.as_deref(), Some("v=1"));
    assert_eq!(requests[2].query, None);
    assert_eq!(requests[3].method, "POST");
    assert_eq!(requests[3].header("x-test"), Some("yes"));
    assert_eq!(requests[3].header("X-TEST"), Some("yes"));
    assert_eq!(&requests[3].body[..], b"payload");

    server.clear_requests();
    assert_eq!(server.request_count(), 0);
    assert!(server.requests().is_empty());
}

#[test]
fn routes_take_precedence_over_files() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/app.js", |_| FixtureResponse::text("from route"))
        .route("/api/echo", |req| {
            FixtureResponse::json(format!(
                r#"{{"method":"{}","query":"{}","body":"{}"}}"#,
                req.method,
                req.query.as_deref().unwrap_or(""),
                String::from_utf8_lossy(&req.body)
            ))
            .with_status(201)
            .with_header("set-cookie", "a=1")
            .with_header("set-cookie", "b=2")
        })
        .route("/old", |_| FixtureResponse::redirect(301, "/app.js"))
        .start()
        .unwrap();

    assert_eq!(get(server.addr(), "/app.js").body, "from route");

    let reply = send(server.addr(), "PUT", "/api/echo?x=1", "", "hi");
    assert_eq!(reply.status, 201);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert_eq!(reply.body, r#"{"method":"PUT","query":"x=1","body":"hi"}"#);
    let cookies: Vec<_> = reply
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(cookies, ["a=1", "b=2"]);

    let reply = get(server.addr(), "/old");
    assert_eq!(reply.status, 301);
    assert_eq!(reply.header("location"), Some("/app.js"));
}

#[test]
fn without_a_root_only_routes_answer() {
    let server = FixtureServer::builder()
        .route("/x", |_| FixtureResponse::text("x"))
        .start()
        .unwrap();
    assert_eq!(get(server.addr(), "/x").body, "x");
    assert_eq!(get(server.addr(), "/index.html").status, 404);
}

#[test]
fn latency_delays_responses() {
    let delay = Duration::from_millis(150);
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .latency(delay)
        .path_latency("/fast.txt", Duration::ZERO)
        .route("/fast.txt", |_| FixtureResponse::text("fast"))
        .start()
        .unwrap();

    let start = Instant::now();
    assert_eq!(get(server.addr(), "/app.js").status, 200);
    assert!(start.elapsed() >= delay);

    let start = Instant::now();
    assert_eq!(get(server.addr(), "/fast.txt").body, "fast");
    assert!(start.elapsed() < delay);
}

#[test]
fn requests_are_counted_on_arrival_before_the_delay() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .path_latency("/app.js", Duration::from_secs(2))
        .start()
        .unwrap();

    let addr = server.addr();
    let client = thread::spawn(move || get(addr, "/app.js"));

    let deadline = Instant::now() + Duration::from_secs(1);
    while server.request_count_for("/app.js") == 0 {
        assert!(Instant::now() < deadline, "request was never recorded");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!client.is_finished(), "response arrived before the delay");
    assert_eq!(client.join().unwrap().status, 200);
}

#[test]
fn delayed_requests_are_served_concurrently() {
    let delay = Duration::from_millis(400);
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .latency(delay)
        .start()
        .unwrap();

    let addr = server.addr();
    let start = Instant::now();
    let clients: Vec<_> = (0..4)
        .map(|_| thread::spawn(move || get(addr, "/app.js").status))
        .collect();
    for client in clients {
        assert_eq!(client.join().unwrap(), 200);
    }
    // Serial handling would take 4x the delay.
    assert!(start.elapsed() < delay * 3);
    assert_eq!(server.request_count(), 4);
}

#[test]
fn dropping_the_server_stops_it() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let addr = server.addr();
    assert_eq!(get(addr, "/app.js").status, 200);
    drop(server);
    assert!(TcpStream::connect(addr).is_err());
}

#[test]
fn drop_does_not_wait_for_delayed_responses() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .latency(Duration::from_secs(30))
        .start()
        .unwrap();
    let addr = server.addr();
    let client = thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /app.js HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        // The connection is closed without a response when the server stops.
        let _ = stream.read_to_end(&mut buf);
        buf
    });
    while server.request_count() == 0 {
        thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    drop(server);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(client.join().unwrap().is_empty());
}

#[tokio::test]
async fn works_from_a_current_thread_runtime() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let addr = server.addr();
    let reply = tokio::task::spawn_blocking(move || get(addr, "/app.js"))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
async fn works_from_a_multi_thread_runtime() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let addr = server.addr();
    let reply = tokio::task::spawn_blocking(move || get(addr, "/app.js"))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
}
