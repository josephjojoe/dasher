//! The wait budget: past it nothing new is started and nothing that has
//! not arrived is waited for, but what is under way finishes.

use std::time::{Duration, Instant};

use dasher_js::{Budget, ReadyState};
use dasher_net::{Client, Session};
use dasher_page::{
    AutoScrollOptions, AutoScrollStop, LoadError, Page, PageOptions, SettleOptions, SettleOutcome,
    WorkBudget,
};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

const MS: fn(u64) -> Duration = Duration::from_millis;

/// How long the late resources take, well past every wait budget here.
const LATE: Duration = Duration::from_millis(1500);

fn server() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/wait/late.js", |_| {
            FixtureResponse::ok("text/javascript", "window.late = true;")
        })
        .route("/wait/quick.js", |_| {
            FixtureResponse::ok("text/javascript", "window.quick = true;")
        })
        .route("/wait/api", |_| FixtureResponse::json("[]"))
        .route("/wait/late.html", |_| {
            FixtureResponse::html("<!doctype html><p>late</p>")
        })
        .path_latency("/wait/late.js", LATE)
        .path_latency("/wait/late.html", LATE)
        .path_latency("/wait/api", MS(50))
        .start()
        .unwrap()
}

fn waiting(wait: Duration) -> PageOptions {
    PageOptions {
        timeout: Some(Duration::from_secs(20)),
        budget: WorkBudget {
            wait: Some(wait),
            ..WorkBudget::default()
        },
        ..PageOptions::default()
    }
}

async fn load(server: &FixtureServer, path: &str, wait: Duration) -> Page {
    let session: Session = Client::builder().build().unwrap().session();
    Page::load(&session, &server.url(path), waiting(wait))
        .await
        .unwrap()
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

#[tokio::test]
async fn a_late_async_script_is_not_waited_for() {
    let server = server();
    let started = Instant::now();
    let page = load(&server, "/wait/late-async.html", MS(300)).await;
    let took = started.elapsed();
    assert!(took >= MS(300) && took < LATE, "{took:?}");
    assert_eq!(page.budget_exhausted(), Some(Budget::Wait));
    assert!(!page.timed_out());
    // The document was parsed to its end; the script and the load event
    // never came.
    assert!(page.html().contains("id=\"after\""));
    assert_eq!(json(&page, "window.late"), "undefined");
    assert_eq!(json(&page, "window.loaded"), "undefined");
    assert_ne!(page.ready_state(), ReadyState::Complete);
}

#[tokio::test]
async fn a_late_deferred_script_is_not_waited_for() {
    let server = server();
    let started = Instant::now();
    let page = load(&server, "/wait/late-defer.html", MS(300)).await;
    assert!(started.elapsed() < LATE);
    assert_eq!(page.budget_exhausted(), Some(Budget::Wait));
    assert!(page.html().contains("id=\"after\""));
    assert_eq!(json(&page, "window.late"), "undefined");
    assert_eq!(json(&page, "window.ready"), "undefined");
}

#[tokio::test]
async fn a_running_script_finishes_and_nothing_new_starts() {
    let server = server();
    let started = Instant::now();
    let page = load(&server, "/wait/busy.html", MS(200)).await;
    // The script ran its 600ms out.
    assert!(started.elapsed() >= MS(600));
    assert_eq!(page.budget_exhausted(), Some(Budget::Wait));
    assert!(!page.timed_out());
    assert_eq!(json(&page, "window.finished"), "true");
    assert_eq!(json(&page, "window.microtask"), "true");
    // The parser went on through the rest of the document, scripts
    // included.
    assert_eq!(json(&page, "window.next"), "true");
    assert!(page.html().contains("id=\"after\""));
    // No task, deferred script or event after it.
    assert_eq!(json(&page, "window.timer"), "undefined");
    assert_eq!(json(&page, "window.quick"), "undefined");
    assert_eq!(json(&page, "window.ready"), "undefined");
    assert_eq!(page.ready_state(), ReadyState::Loading);
}

#[tokio::test]
async fn the_parser_still_waits_for_its_script() {
    let server = server();
    let started = Instant::now();
    let page = load(&server, "/wait/blocking.html", MS(200)).await;
    assert!(started.elapsed() >= LATE);
    assert_eq!(page.budget_exhausted(), Some(Budget::Wait));
    // The script ran and the scripts after it saw what it did.
    assert_eq!(json(&page, "window.late"), "true");
    assert_eq!(json(&page, "window.sawLate"), "true");
    assert!(page.html().contains("id=\"after\""));
    // Nothing else ran while the parser waited past the budget, nor after.
    assert_eq!(json(&page, "window.timer"), "undefined");
    assert_eq!(json(&page, "window.loaded"), "undefined");
}

#[tokio::test]
async fn a_late_document_is_not_waited_for() {
    let server = server();
    let session: Session = Client::builder().build().unwrap().session();
    let started = Instant::now();
    let result = Page::load(&session, &server.url("/wait/late.html"), waiting(MS(300))).await;
    assert!(matches!(result, Err(LoadError::Timeout)), "{result:?}");
    assert!(started.elapsed() < LATE);
}

#[tokio::test]
async fn settling_ends_when_the_wait_does() {
    let server = server();
    let started = Instant::now();
    let mut page = load(&server, "/wait/polling.html", MS(500)).await;
    assert_eq!(page.budget_exhausted(), None);
    assert_eq!(page.ready_state(), ReadyState::Complete);
    let options = SettleOptions {
        page_time_budget: None,
        ..SettleOptions::default()
    };
    let outcome = page.settle(&options).await.unwrap();
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::Wait));
    let took = started.elapsed();
    assert!(took >= MS(500) && took < MS(1500), "{took:?}");

    // Nothing more runs, scrolling included, until the budget is replaced.
    let polls = json(&page, "window.polls");
    let report = page
        .auto_scroll(&AutoScrollOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.stopped,
        AutoScrollStop::BudgetExhausted(Budget::Wait)
    );
    assert_eq!(report.scrolls, 0);
    assert_eq!(json(&page, "window.polls"), polls);

    page.set_budget(WorkBudget {
        wait: Some(MS(300)),
        ..WorkBudget::unlimited()
    });
    let outcome = page.settle(&options).await.unwrap();
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::Wait));
    assert_ne!(json(&page, "window.polls"), polls);
}

#[tokio::test]
async fn a_page_that_is_done_in_time_is_not_cut_short() {
    let server = server();
    let mut page = load(&server, "/wait/late-async.html", LATE * 3).await;
    assert_eq!(page.budget_exhausted(), None);
    assert_eq!(json(&page, "window.late"), "true");
    assert_eq!(json(&page, "window.loaded"), "true");
    let outcome = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::Settled);
}
