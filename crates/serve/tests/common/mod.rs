//! The fixture site the service tests scrape.

#![allow(dead_code)]

use std::time::Duration;

use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

/// Serves `tests/fixtures`, plus:
///
/// - `/api/products` and `/api/feed?page=n`, the APIs the fixtures call;
/// - `/echo`, a page showing the request's user agent, cookies and
///   `X-Token` header;
/// - `/slow.html`, which takes a second to answer;
/// - `/missing`, a 404 page.
pub fn site() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/products", |_| {
            FixtureResponse::json(
                r#"[{"id":1,"name":"Kettle","price":25},{"id":2,"name":"Toaster","price":40}]"#,
            )
        })
        .route("/api/feed", |req| {
            let page: usize = req
                .query
                .as_deref()
                .and_then(|q| q.strip_prefix("page="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if page > 3 {
                return FixtureResponse::json("[]");
            }
            let posts: Vec<String> = (1..=5).map(|i| format!("\"post {page}.{i}\"")).collect();
            FixtureResponse::json(format!("[{}]", posts.join(",")))
        })
        .route("/echo", |req| {
            let header = |name| req.header(name).unwrap_or_default().to_owned();
            FixtureResponse::html(format!(
                "<title>Echo</title><p id=ua>{}</p><p id=cookie>{}</p><p id=token>{}</p>",
                header("user-agent"),
                header("cookie"),
                header("x-token"),
            ))
        })
        .route("/slow.html", |_| {
            FixtureResponse::html("<title>Slow</title>")
        })
        .route("/missing", |_| {
            FixtureResponse::html("<title>Not here</title>").with_status(404)
        })
        .path_latency("/api/products", Duration::from_millis(50))
        .path_latency("/slow.html", Duration::from_secs(1))
        .start()
        .unwrap()
}
