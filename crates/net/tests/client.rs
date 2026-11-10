//! End-to-end tests for the HTTP client against local servers.

mod support;

use std::net::{Ipv4Addr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use dasher_net::header::{self, HeaderMap, HeaderValue};
use dasher_net::{
    BlockReason, Client, CookieJar, Credentials, ErrorKind, Method, RedirectMode, Request,
    RequestPolicy, ResourceType, StatusCode, Url, Version,
};
use dasher_test_server::{FixtureResponse, FixtureServer};

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn client() -> Client {
    Client::new().unwrap()
}

/// A server whose routes echo something about the request.
fn echo_server() -> FixtureServer {
    FixtureServer::builder()
        .route("/", |_| FixtureResponse::html("<p>hello</p>"))
        .route("/header", |req| {
            let name = req.query.clone().unwrap_or_default();
            let values: Vec<&str> = req
                .headers
                .iter()
                .filter(|(n, _)| *n == name)
                .map(|(_, v)| v.as_str())
                .collect();
            FixtureResponse::text(values.join("|"))
        })
        .route("/method", |req| {
            FixtureResponse::text(format!(
                "{} {} {}",
                req.method,
                req.header("content-type").unwrap_or("-"),
                String::from_utf8_lossy(&req.body)
            ))
        })
        .start()
        .unwrap()
}

#[tokio::test]
async fn gets_a_document() {
    let server = echo_server();
    let session = client().session();
    let response = session
        .get(url(&server.url("/")), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(response.url().as_str(), server.url("/"));
    assert!(!response.redirected());
    assert_eq!(response.content_type(), Some("text/html; charset=utf-8"));
    assert_eq!(response.bytes().await.unwrap(), "<p>hello</p>");

    let request = &server.requests()[0];
    assert_eq!(
        request.header("user-agent"),
        Some(dasher_net::DEFAULT_USER_AGENT)
    );
    assert!(request.header("accept").unwrap().starts_with("text/html"));
    assert_eq!(request.header("accept-language"), Some("en-US,en;q=0.9"));
    assert_eq!(request.header("cookie"), None);
    assert_eq!(request.header("referer"), None);
}

#[tokio::test]
async fn header_precedence() {
    let server = echo_server();
    let mut defaults = HeaderMap::new();
    defaults.insert("x-default", HeaderValue::from_static("d"));
    defaults.insert("x-both", HeaderValue::from_static("default"));
    let client = Client::builder()
        .user_agent(HeaderValue::from_static("client-ua"))
        .default_headers(defaults)
        .build()
        .unwrap();

    let read = |session: dasher_net::Session, name: &'static str, request: Option<Request>| {
        let server_url = server.url(&format!("/header?{name}"));
        async move {
            let request = request
                .map(|mut r| {
                    r.url = url(&server_url);
                    r
                })
                .unwrap_or_else(|| Request::get(url(&server_url), ResourceType::Fetch));
            let response = session.send(request).await.unwrap();
            String::from_utf8(response.bytes().await.unwrap().to_vec()).unwrap()
        }
    };

    let session = client.session();
    assert_eq!(read(session.clone(), "user-agent", None).await, "client-ua");
    assert_eq!(read(session.clone(), "x-default", None).await, "d");
    // The default Accept-Language is gone: default_headers replaced it.
    assert_eq!(read(session.clone(), "accept-language", None).await, "");
    assert_eq!(read(session.clone(), "accept", None).await, "*/*");

    let request = Request::get(url("http://unused/"), ResourceType::Fetch)
        .with_header(
            header::HeaderName::from_static("x-both"),
            HeaderValue::from_static("request"),
        )
        .with_header(header::ACCEPT, HeaderValue::from_static("application/json"));
    assert_eq!(
        read(session.clone(), "x-both", Some(request.clone())).await,
        "request"
    );
    assert_eq!(
        read(session.clone(), "accept", Some(request)).await,
        "application/json"
    );

    let session = session.with_user_agent(HeaderValue::from_static("session-ua"));
    assert_eq!(read(session, "user-agent", None).await, "session-ua");
}

#[tokio::test]
async fn refuses_unfetchable_resource_types_without_a_request() {
    let server = FixtureServer::serve(dasher_test_server::fixtures_dir!()).unwrap();
    let session = client().session();
    for ty in [
        ResourceType::Image,
        ResourceType::Stylesheet,
        ResourceType::Font,
        ResourceType::Media,
    ] {
        let error = session
            .get(url(&server.url("/hero.png")), ty)
            .await
            .unwrap_err();
        assert_eq!(
            error.kind(),
            &ErrorKind::Blocked(BlockReason::ResourceType(ty))
        );
    }
    let error = session
        .get(url("file:///etc/passwd"), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Blocked(BlockReason::Scheme));
    assert_eq!(server.request_count(), 0);
    assert_eq!(session.activity().requests_started(), 0);
}

#[tokio::test]
async fn blocks_url_patterns_including_redirect_targets() {
    let server = FixtureServer::builder()
        .route("/ok", |_| FixtureResponse::text("ok"))
        .route("/to-ads", |_| FixtureResponse::redirect(302, "/ads/x.js"))
        .route("/ads/x.js", |_| FixtureResponse::text("ad"))
        .start()
        .unwrap();
    let session = client()
        .session()
        .with_policy(RequestPolicy::new().block_url("*/ads/*"));

    let error = session
        .get(url(&server.url("/ads/x.js")), ResourceType::Script)
        .await
        .unwrap_err();
    assert!(error.is_blocked());

    let error = session
        .get(url(&server.url("/to-ads")), ResourceType::Script)
        .await
        .unwrap_err();
    assert_eq!(
        error.kind(),
        &ErrorKind::Blocked(BlockReason::UrlPattern("*/ads/*".into()))
    );
    assert_eq!(error.url().unwrap().path(), "/ads/x.js");
    assert_eq!(server.request_count_for("/ads/x.js"), 0);
    assert_eq!(server.request_count_for("/to-ads"), 1);
    assert_eq!(session.activity().in_flight(), 0);

    let session = session
        .with_policy(RequestPolicy::new().filter(|req| req.resource_type != ResourceType::Xhr));
    assert!(
        session
            .get(url(&server.url("/ok")), ResourceType::Fetch)
            .await
            .is_ok()
    );
    let error = session
        .get(url(&server.url("/ok")), ResourceType::Xhr)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Blocked(BlockReason::Filter));
}

#[tokio::test]
async fn follows_redirects() {
    let server = FixtureServer::builder()
        .route("/a", |_| FixtureResponse::redirect(302, "/b"))
        .route("/b", |_| FixtureResponse::redirect(301, "c?x=1"))
        .route("/c", |req| {
            FixtureResponse::text(format!("c {}", req.query.as_deref().unwrap_or("")))
        })
        .route("/frag", |_| FixtureResponse::redirect(307, "/c#own"))
        .start()
        .unwrap();
    let session = client().session();

    let response = session
        .get(url(&server.url("/a#top")), ResourceType::Document)
        .await
        .unwrap();
    assert!(response.redirected());
    // The original fragment carries over to a Location without one.
    assert_eq!(response.url().as_str(), server.url("/c?x=1#top"));
    assert_eq!(response.bytes().await.unwrap(), "c x=1");

    let response = session
        .get(url(&server.url("/frag#top")), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.url().fragment(), Some("own"));
    assert_eq!(server.request_count(), 5);
}

#[tokio::test]
async fn redirects_rewrite_methods_like_browsers() {
    let server = echo_server();
    let redirector = FixtureServer::builder()
        .route("/303", {
            let target = server.url("/method");
            move |_| FixtureResponse::redirect(303, &target)
        })
        .route("/307", {
            let target = server.url("/method");
            move |_| FixtureResponse::redirect(307, &target)
        })
        .route("/302", {
            let target = server.url("/method");
            move |_| FixtureResponse::redirect(302, &target)
        })
        .start()
        .unwrap();
    let session = client().session();
    let post = |path: &str, method: Method| {
        Request::new(method, url(&redirector.url(path)), ResourceType::Fetch)
            .with_header(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))
            .with_body("payload")
    };
    let text = |r: dasher_net::Response| async move { r.bytes().await.unwrap() };

    let r = session.send(post("/303", Method::POST)).await.unwrap();
    assert_eq!(text(r).await, "GET - ");
    let r = session.send(post("/302", Method::POST)).await.unwrap();
    assert_eq!(text(r).await, "GET - ");
    let r = session.send(post("/302", Method::PUT)).await.unwrap();
    assert_eq!(text(r).await, "PUT text/plain payload");
    let r = session.send(post("/307", Method::POST)).await.unwrap();
    assert_eq!(text(r).await, "POST text/plain payload");
}

#[tokio::test]
async fn redirect_limits_and_modes() {
    let server = FixtureServer::builder()
        .route("/loop", |_| FixtureResponse::redirect(302, "/loop"))
        .route("/bad", |_| FixtureResponse::redirect(302, "http://[::1"))
        .route("/no-location", |_| FixtureResponse::new(302))
        .start()
        .unwrap();
    let client = Client::builder().max_redirects(3).build().unwrap();
    let session = client.session();

    let error = session
        .get(url(&server.url("/loop")), ResourceType::Document)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Redirect);
    assert_eq!(server.request_count_for("/loop"), 4);

    let error = session
        .get(url(&server.url("/bad")), ResourceType::Document)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Redirect);

    // A redirect status without a Location is just a response.
    let response = session
        .get(url(&server.url("/no-location")), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    drop(response);

    let manual = Request::get(url(&server.url("/loop")), ResourceType::Fetch)
        .with_redirect(RedirectMode::Manual);
    let response = session.send(manual).await.unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/loop");
    assert!(!response.redirected());
    drop(response);

    let error = Request::get(url(&server.url("/loop")), ResourceType::Fetch)
        .with_redirect(RedirectMode::Error);
    let error = session.send(error).await.unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Redirect);
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn stores_and_sends_cookies() {
    let server = FixtureServer::builder()
        .route("/login", |_| {
            FixtureResponse::redirect(302, "/home")
                .with_header("set-cookie", "session=abc; Path=/; HttpOnly")
                .with_header("set-cookie", "theme=dark")
        })
        .route("/home", |req| {
            FixtureResponse::text(req.header("cookie").unwrap_or("").to_owned())
        })
        .start()
        .unwrap();
    let session = client().session();

    // The cookies set by the redirect are sent to its target.
    let response = session
        .get(url(&server.url("/login")), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap(), "session=abc; theme=dark");
    let home = url(&server.url("/home"));
    assert_eq!(session.cookies().script_cookies_for(&home), "theme=dark");

    let omit = Request::get(home.clone(), ResourceType::Fetch).with_credentials(Credentials::Omit);
    let response = session.send(omit).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "");

    // Cookies are per session, unless a jar is shared.
    let other = client().session();
    let response = other.get(home.clone(), ResourceType::Fetch).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "");
    let shared = client()
        .session()
        .with_cookie_jar(Arc::clone(session.cookies()));
    let response = shared.get(home, ResourceType::Fetch).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "session=abc; theme=dark");
}

#[tokio::test]
async fn omitted_credentials_ignore_set_cookie() {
    let server = FixtureServer::builder()
        .route("/set", |_| {
            FixtureResponse::text("").with_header("set-cookie", "a=1")
        })
        .start()
        .unwrap();
    let session = client().session();
    let request = Request::get(url(&server.url("/set")), ResourceType::Fetch)
        .with_credentials(Credentials::Omit);
    session.send(request).await.unwrap();
    assert!(session.cookies().is_empty());
}

#[tokio::test]
async fn same_origin_credentials() {
    let echo_cookie = |req: &dasher_test_server::RecordedRequest| {
        FixtureResponse::text(req.header("cookie").unwrap_or("").to_owned())
    };
    let page = FixtureServer::builder()
        .route("/api", echo_cookie)
        .start()
        .unwrap();
    // Another port on the same host is another origin, but the same cookie
    // host.
    let other = FixtureServer::builder()
        .route("/api", echo_cookie)
        .start()
        .unwrap();
    let jar = Arc::new(CookieJar::new());
    jar.set_from_header(&url(&page.url("/")), "a=1");
    let session = client().session().with_cookie_jar(jar);
    let origin = url(&page.origin()).origin();

    let same = Request::get(url(&page.url("/api")), ResourceType::Fetch)
        .with_credentials(Credentials::SameOrigin(origin.clone()));
    let response = session.send(same).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "a=1");

    let cross = Request::get(url(&other.url("/api")), ResourceType::Fetch)
        .with_credentials(Credentials::SameOrigin(origin));
    let response = session.send(cross).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "");
}

#[tokio::test]
async fn sends_referer_and_basic_auth() {
    let server = echo_server();
    let session = client().session();
    let get = |request: Request| {
        let session = session.clone();
        async move { session.send(request).await.unwrap().bytes().await.unwrap() }
    };

    let page = url(&format!("{}/page?q=1#frag", server.origin()));
    let same = Request::get(url(&server.url("/header?referer")), ResourceType::Fetch)
        .with_referrer(page.clone());
    assert_eq!(get(same).await, format!("{}/page?q=1", server.origin()));

    let cross = Request::get(url(&server.url("/header?referer")), ResourceType::Fetch)
        .with_referrer(url("http://other.example/page"));
    assert_eq!(get(cross).await, "http://other.example/");

    let mut with_auth = url(&server.url("/header?authorization"));
    with_auth.set_username("user").unwrap();
    with_auth.set_password(Some("pa ss")).unwrap();
    let auth = Request::get(with_auth, ResourceType::Fetch);
    assert_eq!(get(auth).await, "Basic dXNlcjpwYSBzcw==");
}

#[tokio::test]
async fn enforces_the_maximum_response_size() {
    let server = FixtureServer::builder()
        .route("/big", |_| FixtureResponse::text(vec![b'x'; 1000]))
        .start()
        .unwrap();
    let client = Client::builder().max_response_size(999).build().unwrap();
    let session = client.session();

    // The declared length is checked before the body is read.
    let error = session
        .get(url(&server.url("/big")), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::TooLarge { limit: 999 });
    assert_eq!(session.activity().in_flight(), 0);

    let request =
        Request::get(url(&server.url("/big")), ResourceType::Fetch).with_max_response_size(1000);
    let body = session.send(request).await.unwrap().bytes().await.unwrap();
    assert_eq!(body.len(), 1000);

    // A HEAD response declares the length without a body.
    let head = Request::new(Method::HEAD, url(&server.url("/big")), ResourceType::Fetch);
    let response = session.send(head).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "");
}

#[tokio::test]
async fn enforces_the_maximum_size_of_undeclared_bodies() {
    let chunk = format!("{:x}\r\n{}\r\n", 600, "y".repeat(600));
    let reply =
        format!("HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{chunk}{chunk}0\r\n\r\n");
    let addr = support::raw_server(vec![reply.clone().into(), reply.into()], Duration::ZERO);
    let client = Client::builder().max_response_size(1000).build().unwrap();
    let session = client.session();
    let target = url(&format!("http://{addr}/"));

    let mut response = session
        .get(target.clone(), ResourceType::Fetch)
        .await
        .unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap().len(), 600);
    assert_eq!(session.activity().in_flight(), 1);
    let error = response.chunk().await.unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::TooLarge { limit: 1000 });
    assert_eq!(error.url(), Some(&target));
    assert_eq!(session.activity().in_flight(), 0);
    // After an error the body is finished.
    assert!(response.chunk().await.unwrap().is_none());

    let response = session.get(target, ResourceType::Fetch).await.unwrap();
    assert_eq!(
        response.bytes().await.unwrap_err().kind(),
        &ErrorKind::TooLarge { limit: 1000 }
    );
}

#[tokio::test]
async fn times_out_waiting_for_headers() {
    let server = FixtureServer::builder()
        .route("/slow", |_| FixtureResponse::text("late"))
        .path_latency("/slow", Duration::from_secs(5))
        .start()
        .unwrap();
    let client = Client::builder()
        .timeout(Some(Duration::from_millis(100)))
        .build()
        .unwrap();
    let session = client.session();
    let error = session
        .get(url(&server.url("/slow")), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert!(error.is_timeout(), "{error}");
    assert_eq!(session.activity().in_flight(), 0);

    // A per-request timeout overrides the client's.
    let fast = FixtureServer::builder()
        .route("/slow", |_| FixtureResponse::text("late"))
        .path_latency("/slow", Duration::from_millis(200))
        .start()
        .unwrap();
    let request = Request::get(url(&fast.url("/slow")), ResourceType::Fetch)
        .with_timeout(Duration::from_secs(10));
    let response = session.send(request).await.unwrap();
    assert_eq!(response.bytes().await.unwrap(), "late");
}

#[tokio::test]
async fn times_out_reading_the_body() {
    // Headers promise ten bytes; only two arrive before the server stalls.
    let addr = support::raw_server(
        vec![b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nhi".to_vec()],
        Duration::from_secs(5),
    );
    let client = Client::builder()
        .timeout(Some(Duration::from_millis(300)))
        .build()
        .unwrap();
    let session = client.session();
    let mut response = session
        .get(url(&format!("http://{addr}/")), ResourceType::Fetch)
        .await
        .unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap(), "hi");
    assert!(response.chunk().await.unwrap_err().is_timeout());
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn connection_failures() {
    // Bind and drop to find a port nothing listens on.
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let session = client().session();
    let error = session
        .get(
            url(&format!("http://127.0.0.1:{port}/")),
            ResourceType::Fetch,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Connect);
    assert_eq!(session.activity().in_flight(), 0);

    let addr = support::raw_server(vec![b"garbage\r\n\r\n".to_vec()], Duration::ZERO);
    let error = session
        .get(url(&format!("http://{addr}/")), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Http);
}

#[tokio::test]
async fn tracks_requests_in_flight_until_the_body_is_done() {
    let server = FixtureServer::builder()
        .route("/slow", |_| FixtureResponse::text("done"))
        .path_latency("/slow", Duration::from_millis(200))
        .start()
        .unwrap();
    let session = client().session();
    let activity = session.activity().clone();
    let target = url(&server.url("/slow"));

    let pending = tokio::spawn({
        let session = session.clone();
        let target = target.clone();
        async move { session.get(target, ResourceType::Fetch).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(activity.in_flight(), 1);
    let response = pending.await.unwrap().unwrap();
    // Headers are in, but the body hasn't been read.
    assert_eq!(activity.in_flight(), 1);
    assert_eq!(response.bytes().await.unwrap(), "done");
    assert_eq!(activity.in_flight(), 0);

    // Dropping an unread response also ends it.
    let response = session.get(target, ResourceType::Fetch).await.unwrap();
    assert_eq!(activity.in_flight(), 1);
    drop(response);
    assert_eq!(activity.in_flight(), 0);
    assert_eq!(activity.requests_started(), 2);

    activity.wait_for_idle(0, Duration::from_millis(10)).await;
}

#[tokio::test]
async fn cancelling_a_request_ends_it() {
    let server = FixtureServer::builder()
        .route("/slow", |_| FixtureResponse::text("late"))
        .path_latency("/slow", Duration::from_secs(5))
        .start()
        .unwrap();
    let session = client().session();
    let pending = tokio::spawn({
        let session = session.clone();
        let target = url(&server.url("/slow"));
        async move { session.get(target, ResourceType::Fetch).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(session.activity().in_flight(), 1);
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn http2_over_tls_with_connection_reuse() {
    let server = support::TlsServer::start();
    let client = Client::builder()
        .built_in_root_certificates(false)
        .add_root_certificate(server.certificate.clone())
        .build()
        .unwrap();
    let session = client.session();

    let response = session
        .get(url(&server.url("/first")), ResourceType::Document)
        .await
        .unwrap();
    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(response.bytes().await.unwrap(), "HTTP/2.0 /first");

    // Concurrent requests share the one HTTP/2 connection.
    let mut requests = tokio::task::JoinSet::new();
    for i in 0..8 {
        let session = session.clone();
        let target = url(&server.url(&format!("/{i}")));
        requests.spawn(async move {
            let response = session.get(target, ResourceType::Fetch).await.unwrap();
            response.bytes().await.unwrap()
        });
    }
    let mut bodies = requests.join_all().await;
    bodies.sort();
    assert_eq!(bodies[7], "HTTP/2.0 /7");
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn http1_over_tls_when_http2_is_disabled() {
    let server = support::TlsServer::start();
    let client = Client::builder()
        .built_in_root_certificates(false)
        .add_root_certificate(server.certificate.clone())
        .http2(false)
        .build()
        .unwrap();
    let session = client.session();
    for path in ["/a", "/b"] {
        let response = session
            .get(url(&server.url(path)), ResourceType::Fetch)
            .await
            .unwrap();
        assert_eq!(response.version(), Version::HTTP_11);
        assert_eq!(response.bytes().await.unwrap(), format!("HTTP/1.1 {path}"));
    }
    // Sequential HTTP/1.1 requests reuse the pooled connection.
    assert_eq!(server.connections(), 1);
}

#[tokio::test]
async fn rejects_untrusted_certificates() {
    let server = support::TlsServer::start();
    let session = client().session();
    let error = session
        .get(url(&server.url("/")), ResourceType::Fetch)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Connect);
}

#[test]
fn invalid_root_certificates_are_a_config_error() {
    let error = Client::builder()
        .add_root_certificate(b"not a certificate".to_vec())
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), &ErrorKind::Config);
}
