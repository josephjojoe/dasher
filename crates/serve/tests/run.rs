//! Running jobs against local fixtures.

mod common;

use std::time::{Duration, Instant};

use dasher_serve::{Job, Outcome, RunOptions, run};
use serde_json::{Value, json};

/// The service's options, but allowing the fixture servers on localhost.
fn local() -> RunOptions {
    RunOptions {
        block_private_addresses: false,
        ..RunOptions::default()
    }
}

async fn run_json(job: Value) -> Outcome {
    run_with(job, &local()).await
}

async fn run_with(job: Value, options: &RunOptions) -> Outcome {
    let job = Job::from_value(job).unwrap();
    run(&job, options, Duration::ZERO).await
}

#[tokio::test]
async fn returns_the_rendered_html_and_page_facts() {
    let site = common::site();
    let outcome = run_json(json!({ "url": site.url("/shop.html") })).await;
    assert_eq!(outcome.status, 200, "{}", outcome.body);
    let body = &outcome.body;
    assert_eq!(body["url"], site.url("/shop.html"));
    assert_eq!(body["status"], 200);
    assert_eq!(body["title"], "2 products");
    assert_eq!(body["timed_out"], false);
    assert_eq!(body["settle"], "settled");
    assert_eq!(body["scroll"], Value::Null);
    let html = body["html"].as_str().unwrap();
    assert!(html.contains(r#"<a href="/p/2">Toaster</a>"#), "{html}");
    assert!(body.get("extracted").is_none());

    // Only console errors by default, and the uncaught exception.
    assert_eq!(
        body["console"],
        json!([{"level": "error", "text": "price feed is stale"}])
    );
    assert_eq!(body["errors"][0]["kind"], "uncaught");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("widget failed")
    );

    let timings = &body["timings"];
    for key in ["queued_ms", "navigation_ms", "extract_ms", "total_ms"] {
        assert!(timings[key].is_u64(), "{key}: {timings}");
    }
}

#[tokio::test]
async fn extracts_values_with_selectors_and_javascript() {
    let site = common::site();
    let outcome = run_json(json!({
        "url": site.url("/shop.html"),
        "console": "all",
        "extract": {
            "heading": "h1",
            "names": {"selector": ".product a", "all": true},
            "links": {"selector": ".product a", "all": true, "attribute": "href"},
            "next": {"selector": "a.next", "attribute": "href"},
            "first": {"selector": ".product", "html": "inner"},
            "prices": {"selector": ".price", "all": true, "html": "outer"},
            "count": {"selector": ".product", "count": true},
            "missing": ".nothing",
            "total": {"eval": "window.__STATE__.total"},
            "api": {"eval": "async () => (await fetch('/api/products')).json()"},
            "broken": {"eval": "null.boom"},
        },
    }))
    .await;
    assert_eq!(outcome.status, 200, "{}", outcome.body);
    let body = &outcome.body;
    assert!(body.get("html").is_none(), "no html unless asked for");
    let extracted = &body["extracted"];
    assert_eq!(extracted["heading"], "Products");
    assert_eq!(extracted["names"], json!(["Kettle", "Toaster"]));
    assert_eq!(extracted["links"], json!(["/p/1", "/p/2"]));
    assert_eq!(extracted["next"], "/shop.html?page=2");
    assert_eq!(
        extracted["first"],
        r#"<a href="/p/1">Kettle</a> <span class="price">25</span>"#
    );
    assert_eq!(
        extracted["prices"],
        json!([
            r#"<span class="price">25</span>"#,
            r#"<span class="price">40</span>"#
        ])
    );
    assert_eq!(extracted["count"], 2);
    assert_eq!(extracted["missing"], Value::Null);
    assert_eq!(extracted["total"], 2);
    assert_eq!(extracted["api"][1]["name"], "Toaster");
    // A failing extraction does not sink the others.
    assert_eq!(extracted["broken"], Value::Null);
    assert!(
        body["extract_errors"]["broken"]
            .as_str()
            .unwrap()
            .contains("TypeError"),
        "{body}"
    );
    assert_eq!(body["extract_errors"].as_object().unwrap().len(), 1);
    assert_eq!(
        body["console"][0],
        json!({"level": "log", "text": "rendering"})
    );
}

#[tokio::test]
async fn html_can_be_asked_for_alongside_extraction() {
    let site = common::site();
    let outcome = run_json(json!({
        "url": site.url("/shop.html"),
        "html": true,
        "console": "none",
        "extract": {"title": "title"},
    }))
    .await;
    assert!(outcome.body["html"].as_str().unwrap().contains("Kettle"));
    assert_eq!(outcome.body["extracted"]["title"], "2 products");
    assert_eq!(outcome.body["console"], json!([]));
}

#[tokio::test]
async fn wait_options_and_scrolling_are_followed() {
    let site = common::site();
    let at_load = run_json(json!({
        "url": site.url("/shop.html"),
        "wait": "load",
        "extract": {"count": {"selector": ".product", "count": true}},
    }))
    .await;
    assert_eq!(at_load.body["settle"], Value::Null);
    assert_eq!(at_load.body["extracted"]["count"], 0);

    let scrolled = run_json(json!({
        "url": site.url("/feed.html"),
        "scroll": {"max_scrolls": 2},
        "extract": {"posts": {"selector": "article", "count": true}},
    }))
    .await;
    assert_eq!(scrolled.body["scroll"]["scrolls"], 2, "{}", scrolled.body);
    assert_eq!(scrolled.body["scroll"]["stopped"], "scroll_limit");
    assert_eq!(scrolled.body["extracted"]["posts"], 11);
}

#[tokio::test]
async fn browser_settings_reach_the_site() {
    let site = common::site();
    let outcome = run_json(json!({
        "url": site.url("/echo"),
        "user_agent": "dasher-test/1",
        "headers": {"X-Token": "t0k"},
        "cookies": ["session=abc; Path=/"],
        "extract": {"ua": "#ua", "cookie": "#cookie", "token": "#token"},
    }))
    .await;
    assert_eq!(
        outcome.body["extracted"],
        json!({"ua": "dasher-test/1", "cookie": "session=abc", "token": "t0k"})
    );
    // Nothing carries over: the next job has no cookies.
    let next = run_json(json!({"url": site.url("/echo"), "extract": {"cookie": "#cookie"}})).await;
    assert_eq!(next.body["extracted"]["cookie"], "");
}

#[tokio::test]
async fn error_pages_load_like_any_other() {
    let site = common::site();
    let outcome = run_json(json!({ "url": site.url("/missing") })).await;
    assert_eq!(outcome.status, 200);
    assert_eq!(outcome.body["status"], 404);
    assert_eq!(outcome.body["title"], "Not here");
}

#[tokio::test]
async fn jobs_that_cannot_run_fail_with_a_status() {
    let site = common::site();
    let error = |outcome: &Outcome| outcome.body["error"].as_str().unwrap().to_owned();

    let bad_url = run_json(json!({ "url": "ftp://example.com/" })).await;
    assert_eq!(bad_url.status, 400);
    assert!(error(&bad_url).contains("not an http(s) URL"));
    assert_eq!(bad_url.body["url"], "ftp://example.com/");
    assert!(bad_url.body["timings"]["total_ms"].is_u64());

    let bad_selector = run_json(json!({
        "url": site.url("/shop.html"),
        "extract": {"x": "a["},
    }))
    .await;
    assert_eq!(bad_selector.status, 400);
    assert!(error(&bad_selector).contains("not a valid selector"));
    assert_eq!(site.request_count(), 0, "nothing is fetched for a bad job");

    let bad_header = run_json(json!({
        "url": site.url("/shop.html"),
        "headers": {"bad name": "x"},
    }))
    .await;
    assert_eq!(bad_header.status, 400);

    // Nothing listens on port 9 of localhost.
    let unreachable = run_json(json!({ "url": "http://127.0.0.1:9/" })).await;
    assert_eq!(unreachable.status, 502, "{}", unreachable.body);
}

#[tokio::test]
async fn metadata_endpoints_are_refused() {
    for url in [
        "http://169.254.169.254/latest/meta-data/",
        "http://metadata.google.internal/computeMetadata/v1/",
        "http://metadata/computeMetadata/v1/",
    ] {
        let outcome = run_json(json!({ "url": url })).await;
        assert_eq!(outcome.status, 502, "{url}");
        assert!(
            outcome.body["error"]
                .as_str()
                .unwrap()
                .contains("blocked pattern"),
            "{url}: {}",
            outcome.body
        );
    }
}

#[tokio::test]
async fn private_addresses_are_refused_by_default() {
    let site = common::site();
    let by_ip = site.url("/shop.html");
    let by_name = by_ip.replace("127.0.0.1", "localhost");
    for url in [&by_ip, &by_name] {
        let outcome = run_with(json!({ "url": url }), &RunOptions::default()).await;
        assert_eq!(outcome.status, 502, "{url}: {}", outcome.body);
        let error = outcome.body["error"].as_str().unwrap();
        assert!(
            error.contains("loopback address, and private addresses are refused"),
            "{url}: {error}"
        );
    }
    assert_eq!(site.request_count(), 0);
}

#[tokio::test]
async fn the_time_limit_is_capped_by_the_service_and_the_deadline() {
    let site = common::site();
    let job = json!({ "url": site.url("/slow.html"), "timeout_ms": 60_000 });

    let capped = RunOptions {
        max_timeout: Duration::from_millis(200),
        ..local()
    };
    let started = Instant::now();
    let outcome = run_with(job.clone(), &capped).await;
    assert_eq!(outcome.status, 504, "{}", outcome.body);
    assert!(started.elapsed() < Duration::from_millis(900));

    let deadline = RunOptions {
        deadline: Some(Instant::now() + Duration::from_millis(1700)),
        ..local()
    };
    let started = Instant::now();
    let outcome = run_with(job.clone(), &deadline).await;
    assert_eq!(outcome.status, 504, "{}", outcome.body);
    assert!(started.elapsed() < Duration::from_millis(900));

    let past = RunOptions {
        deadline: Some(Instant::now()),
        ..local()
    };
    let outcome = run_with(job, &past).await;
    assert_eq!(outcome.status, 504);
    assert!(
        outcome.body["error"]
            .as_str()
            .unwrap()
            .contains("no time left")
    );
}
