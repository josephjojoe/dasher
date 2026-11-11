//! Declarative refreshes: `<meta http-equiv="refresh">` and the `Refresh`
//! header.

use std::time::{Duration, Instant};

use dasher_js::{ClockMode, RuntimeOptions};
use dasher_net::{Client, Session};
use dasher_page::{Page, PageOptions};
use dasher_test_server::{FixtureResponse, FixtureServer};

fn session() -> Session {
    Client::builder().build().unwrap().session()
}

fn options() -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(10)),
        ..PageOptions::default()
    }
}

async fn load_with(server: &FixtureServer, path: &str, options: PageOptions) -> Page {
    Page::load(&session(), &server.url(path), options)
        .await
        .unwrap()
}

async fn load(server: &FixtureServer, path: &str) -> Page {
    load_with(server, path, options()).await
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn body_text(page: &Page) -> String {
    json(page, "document.body.textContent")
}

fn refresh_page(content: &str) -> FixtureResponse {
    FixtureResponse::html(format!(
        r#"<head><meta http-equiv="refresh" content="{content}"></head><p>interstitial"#
    ))
}

fn console_has(page: &Page, text: &str) -> bool {
    page.js()
        .take_console()
        .iter()
        .any(|m| m.text.contains(text))
}

fn server() -> FixtureServer {
    FixtureServer::builder()
        .route("/now", |_| refresh_page("0; url=/target"))
        .route("/soon", |_| refresh_page("3; URL='/target'"))
        .route("/later", |_| refresh_page("60; url=/target"))
        .route("/header", |_| {
            FixtureResponse::html("<p>interstitial").with_header("refresh", "1; url=/target")
        })
        .route("/header-and-meta", |_| {
            refresh_page("0; url=/other").with_header("refresh", "0; url=/target")
        })
        .route("/noscript", |_| {
            FixtureResponse::html(
                r#"<head><noscript><meta http-equiv="refresh" content="0; url=/nojs"></noscript>
                <base href="/sub/">
                <meta http-equiv="refresh" content="0; url=target"></head>"#,
            )
        })
        .route("/inserted", |_| {
            FixtureResponse::html(
                r#"<script>
                addEventListener('DOMContentLoaded', () => {
                    const meta = document.createElement('meta');
                    meta.httpEquiv = 'refresh';
                    meta.content = '0; url=/target';
                    document.head.append(meta);
                });
                </script>"#,
            )
        })
        .route("/script-first", |_| {
            FixtureResponse::html(
                r#"<meta http-equiv="refresh" content="5; url=/other">
                <script>setTimeout(() => location.href = '/target', 1000)</script>"#,
            )
        })
        .route("/poll", |_| refresh_page("5"))
        .route("/mailto", |_| refresh_page("1; url=mailto:a@example.com"))
        .route("/target", |_| FixtureResponse::html("<p>target"))
        .route("/sub/target", |_| FixtureResponse::html("<p>sub target"))
        .start()
        .unwrap()
}

#[tokio::test]
async fn follows_meta_refreshes_after_load() {
    let server = server();
    let page = load(&server, "/now").await;
    assert_eq!(page.url(), server.url("/target"));
    assert_eq!(body_text(&page), r#""target""#);
    assert!(!page.timed_out());
    let referer = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/target")
        .and_then(|r| r.header("referer").map(str::to_owned));
    assert_eq!(referer, Some(server.url("/now")));

    // The delay is page time: the virtual clock skips it.
    let started = Instant::now();
    let page = load(&server, "/soon").await;
    assert_eq!(page.url(), server.url("/target"));
    assert!(started.elapsed() < Duration::from_secs(2));

    // Relative to the base URL; noscript content is not markup.
    let page = load(&server, "/noscript").await;
    assert_eq!(page.url(), server.url("/sub/target"));
    assert_eq!(server.request_count_for("/nojs"), 0);

    // Scripts may insert the meta before load.
    let page = load(&server, "/inserted").await;
    assert_eq!(page.url(), server.url("/target"));
}

#[tokio::test]
async fn follows_the_refresh_header_before_any_meta() {
    let server = server();
    let page = load(&server, "/header").await;
    assert_eq!(page.url(), server.url("/target"));

    let page = load(&server, "/header-and-meta").await;
    assert_eq!(page.url(), server.url("/target"));
    assert_eq!(server.request_count_for("/other"), 0);
}

#[tokio::test]
async fn a_script_navigation_before_the_refresh_wins() {
    let server = server();
    let page = load(&server, "/script-first").await;
    assert_eq!(page.url(), server.url("/target"));
    assert_eq!(server.request_count_for("/other"), 0);
}

#[tokio::test]
async fn long_delays_and_polling_reloads_are_ignored() {
    let server = server();
    let page = load(&server, "/later").await;
    assert_eq!(page.url(), server.url("/later"));
    assert_eq!(body_text(&page), r#""interstitial""#);
    assert!(console_has(&page, "its delay is too long"));
    assert_eq!(page.js().refresh_due(), None);

    let page = load(&server, "/poll").await;
    assert_eq!(page.url(), server.url("/poll"));
    assert!(console_has(&page, "reloads the page after a delay"));
    assert_eq!(server.request_count_for("/poll"), 1);

    let page = load(&server, "/mailto").await;
    assert_eq!(page.url(), server.url("/mailto"));
    assert!(console_has(&page, "not an http(s) URL"));

    // The limit can be raised, or refreshes turned off.
    let page = load_with(
        &server,
        "/later",
        PageOptions {
            max_refresh_delay: Some(Duration::from_secs(60)),
            ..options()
        },
    )
    .await;
    assert_eq!(page.url(), server.url("/target"));
    let page = load_with(
        &server,
        "/now",
        PageOptions {
            max_refresh_delay: None,
            ..options()
        },
    )
    .await;
    assert_eq!(page.url(), server.url("/now"));
    assert!(console_has(&page, "refreshes are off"));
}

#[tokio::test]
async fn immediate_reloads_are_followed_within_the_navigation_budget() {
    let server = FixtureServer::builder()
        // A cookie check: set a cookie, then reload to show the content.
        .route("/check", |req| {
            if req.header("cookie").is_some_and(|c| c.contains("ok=1")) {
                FixtureResponse::html("<p>content")
            } else {
                refresh_page("0").with_header("set-cookie", "ok=1; Path=/")
            }
        })
        .route("/loop", |req| {
            let n: u32 = req
                .query
                .as_deref()
                .and_then(|q| q.strip_prefix("n="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            refresh_page(&format!("0; url=/loop?n={}", n + 1))
        })
        .start()
        .unwrap();
    let page = load(&server, "/check").await;
    assert_eq!(page.url(), server.url("/check"));
    assert_eq!(body_text(&page), r#""content""#);
    assert_eq!(server.request_count_for("/check"), 2);

    let page = load_with(
        &server,
        "/loop",
        PageOptions {
            max_navigations: 3,
            ..options()
        },
    )
    .await;
    assert_eq!(page.url(), server.url("/loop?n=3"));
    assert_eq!(server.request_count_for("/loop"), 4);
    assert!(console_has(&page, "too many navigations"));
}

#[tokio::test]
async fn running_out_of_time_waiting_for_a_refresh_is_a_timeout() {
    let server = server();
    let started = Instant::now();
    let page = load_with(
        &server,
        "/soon",
        PageOptions {
            runtime: RuntimeOptions {
                clock: ClockMode::Realtime,
                ..RuntimeOptions::default()
            },
            timeout: Some(Duration::from_millis(500)),
            ..options()
        },
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(page.timed_out());
    assert_eq!(page.url(), server.url("/soon"));
    assert_eq!(body_text(&page), r#""interstitial""#);
}
