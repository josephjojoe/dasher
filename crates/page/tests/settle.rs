//! Waiting for pages to settle: network idle, DOM quiet, selectors,
//! predicates and the timeout.

use std::time::{Duration, Instant};

use dasher_js::{ClockMode, RuntimeOptions};
use dasher_net::{Client, Session};
use dasher_page::{NetworkIdle, Page, PageOptions, SettleError, SettleOptions, SettleOutcome};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

const MS: fn(u64) -> Duration = Duration::from_millis;

fn server() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/items", |_| {
            FixtureResponse::json(r#"["one","two","three"]"#)
        })
        .route("/api/detail", |_| {
            FixtureResponse::json(r#"{"status":"done"}"#)
        })
        .path_latency("/api/items", MS(300))
        .path_latency("/api/detail", MS(200))
        .start()
        .unwrap()
}

fn options(clock: ClockMode) -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock,
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(10)),
        ..PageOptions::default()
    }
}

async fn load(server: &FixtureServer, path: &str, clock: ClockMode) -> Page {
    let session: Session = Client::builder().build().unwrap().session();
    Page::load(&session, &server.url(path), options(clock))
        .await
        .unwrap()
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

const RENDERED: &str = "[[...document.querySelectorAll('li')].map((li) => li.textContent), \
                        document.getElementById('status').textContent]";

#[tokio::test]
async fn content_fetched_after_load_is_waited_for() {
    for clock in [ClockMode::Virtual, ClockMode::Realtime] {
        let server = server();
        let mut page = load(&server, "/settle/after-load.html", clock).await;
        // Loading ends before the content arrives.
        assert_eq!(json(&page, RENDERED), r#"[[],"loading"]"#, "{clock:?}");
        let outcome = page.settle(&SettleOptions::default()).await.unwrap();
        assert_eq!(outcome, SettleOutcome::Settled, "{clock:?}");
        assert_eq!(
            json(&page, RENDERED),
            r#"[["one","two","three"],"done"]"#,
            "{clock:?}"
        );
    }
}

#[tokio::test]
async fn network_idle_alone_waits_for_requests_started_by_timers() {
    let server = server();
    let mut page = load(&server, "/settle/after-load.html", ClockMode::Virtual).await;
    let options = SettleOptions {
        network_idle: Some(NetworkIdle::default()),
        ..SettleOptions::none()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
    assert_eq!(json(&page, "window.ready"), "true");
}

#[tokio::test]
async fn a_selector_or_predicate_ends_the_wait_as_soon_as_it_holds() {
    let server = server();
    let mut page = load(&server, "/settle/after-load.html", ClockMode::Virtual).await;
    let options = SettleOptions {
        selector: Some("#items li:nth-child(3)".into()),
        ..SettleOptions::none()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
    // The list is there; the details are still on their way.
    assert_eq!(
        json(&page, RENDERED),
        r#"[["one","two","three"],"loading"]"#
    );

    let options = SettleOptions {
        predicate: Some("window.ready === true && document.title.length > 0".into()),
        ..SettleOptions::none()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
    assert_eq!(json(&page, RENDERED), r#"[["one","two","three"],"done"]"#);
}

#[tokio::test]
async fn conditions_combine() {
    let server = server();
    let mut page = load(&server, "/settle/after-load.html", ClockMode::Virtual).await;
    // The selector matches at once, but the network has to go idle too.
    let options = SettleOptions {
        selector: Some("#status".into()),
        // Throws until `ready` is defined: counts as false.
        predicate: Some("window.ready.valueOf()".into()),
        ..SettleOptions::default()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
    assert_eq!(json(&page, RENDERED), r#"[["one","two","three"],"done"]"#);
    assert!(page.js().errors().is_empty());
}

#[tokio::test]
async fn a_page_that_never_goes_quiet_times_out() {
    let server = server();
    let mut page = load(&server, "/settle/ticker.html", ClockMode::Realtime).await;
    let options = SettleOptions {
        timeout: MS(700),
        ..SettleOptions::default()
    };
    let start = Instant::now();
    assert_eq!(
        page.settle(&options).await.unwrap(),
        SettleOutcome::TimedOut
    );
    let took = start.elapsed();
    assert!(took >= MS(700) && took < MS(1500), "{took:?}");
    // It kept running meanwhile, and is usable afterwards.
    assert_eq!(
        json(&page, "document.getElementById('loaded').textContent"),
        r#""one,two,three""#
    );

    // Without the DOM condition it settles.
    let options = SettleOptions {
        dom_quiet: None,
        ..SettleOptions::default()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
}

#[tokio::test]
async fn a_navigation_ends_the_wait() {
    let server = server();
    let mut page = load(&server, "/settle/navigate.html", ClockMode::Virtual).await;
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::NavigationRequested
    );
    assert!(page.follow_navigation().await.unwrap());
    assert!(page.url().ends_with("/settle/after-load.html"));
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    assert_eq!(json(&page, "window.ready"), "true");
}

#[tokio::test]
async fn no_conditions_settle_at_once_and_bad_selectors_are_refused() {
    let server = server();
    let mut page = load(&server, "/settle/ticker.html", ClockMode::Virtual).await;
    assert_eq!(
        page.settle(&SettleOptions::none()).await.unwrap(),
        SettleOutcome::Settled
    );
    let options = SettleOptions {
        selector: Some("li[".into()),
        ..SettleOptions::none()
    };
    assert_eq!(
        page.settle(&options).await,
        Err(SettleError::InvalidSelector("li[".into()))
    );
}
