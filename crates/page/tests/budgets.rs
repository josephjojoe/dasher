//! Budgets: pages that would run forever under the virtual clock are
//! stopped by the page-time, CPU or request budget, and returned as they
//! stand.

use std::time::{Duration, Instant};

use dasher_js::{Budget, ClockMode, RunLimits, RunOutcome, RuntimeOptions, thread_cpu_time};
use dasher_net::{Client, Session};
use dasher_page::{
    AutoScrollOptions, AutoScrollStop, DEFAULT_QUIET, Page, PageOptions, SettleOptions,
    SettleOutcome, WorkBudget,
};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

const MS: fn(u64) -> Duration = Duration::from_millis;

fn server() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/items", |_| {
            FixtureResponse::json(r#"["one","two","three"]"#)
        })
        .route("/api/more", |_| FixtureResponse::json(r#"["a","b"]"#))
        .route("/api/slow", |_| {
            FixtureResponse::json(r#"["slow","but","sure"]"#)
        })
        .path_latency("/api/items", MS(100))
        .path_latency("/api/slow", MS(800))
        .start()
        .unwrap()
}

fn options(budget: WorkBudget, clock: ClockMode) -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock,
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(20)),
        budget,
        ..PageOptions::default()
    }
}

async fn load_on(server: &FixtureServer, path: &str, budget: WorkBudget, clock: ClockMode) -> Page {
    let session: Session = Client::builder().build().unwrap().session();
    Page::load(&session, &server.url(path), options(budget, clock))
        .await
        .unwrap()
}

async fn load_with(server: &FixtureServer, path: &str, budget: WorkBudget) -> Page {
    load_on(server, path, budget, ClockMode::Virtual).await
}

async fn load(server: &FixtureServer, path: &str) -> Page {
    load_with(server, path, WorkBudget::default()).await
}

/// Network idle and DOM quiet: what a page ticking forever never reaches.
fn settled() -> SettleOptions {
    SettleOptions {
        dom_quiet: Some(DEFAULT_QUIET),
        ..SettleOptions::default()
    }
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn number(page: &Page, expr: &str) -> u64 {
    json(page, expr).parse().unwrap()
}

#[tokio::test]
async fn a_page_ticking_forever_ends_on_the_page_time_budget() {
    let server = server();
    let mut page = load(&server, "/budget/interval.html").await;
    let cpu = thread_cpu_time();
    let started = Instant::now();
    let outcome = page.settle(&settled()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::PageTime));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(thread_cpu_time() - cpu < Duration::from_secs(1));
    // The content is there, and the clock ticked for about five seconds
    // of page time: a bounded amount of work.
    assert_eq!(
        json(&page, "document.getElementById('loaded').textContent"),
        r#""one,two,three""#
    );
    let ticks = number(&page, "ticks");
    assert!((45..=60).contains(&ticks), "{ticks}");
    assert_eq!(json(&page, "!!document.getElementById('late')"), "true");
}

#[tokio::test]
async fn by_default_a_page_ticking_forever_settles_once_the_network_is_idle() {
    let server = server();
    let mut page = load(&server, "/budget/interval.html").await;
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    assert_eq!(
        json(&page, "document.getElementById('loaded').textContent"),
        r#""one,two,three""#
    );
}

#[tokio::test]
async fn a_page_polling_forever_ends_on_the_page_time_budget() {
    let server = server();
    let mut page = load(&server, "/budget/poll.html").await;
    let cpu = thread_cpu_time();
    let outcome = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::PageTime));
    assert!(thread_cpu_time() - cpu < Duration::from_secs(1));
    // Five seconds of idle page time, plus the 100 ms each poll spends in
    // flight, which does not count: a poll per 300 ms of it.
    let polls = number(&page, "+document.getElementById('count').textContent");
    assert!((15..=30).contains(&polls), "{polls}");
}

#[tokio::test]
async fn without_a_page_time_budget_the_same_page_spins_to_the_timeout() {
    let server = server();
    let mut page = load(&server, "/budget/interval.html").await;
    let options = SettleOptions {
        page_time_budget: None,
        timeout: MS(500),
        ..settled()
    };
    assert_eq!(
        page.settle(&options).await.unwrap(),
        SettleOutcome::TimedOut
    );
    // Half a second of real time ran the clock far ahead.
    assert!(number(&page, "ticks") > 200);
}

#[tokio::test]
async fn past_the_budget_an_explicit_wait_goes_on_in_real_time() {
    let server = server();
    let mut page = load(&server, "/budget/interval.html").await;
    let options = SettleOptions {
        selector: Some("#late".into()),
        page_time_budget: Some(MS(300)),
        ..settled()
    };
    let started = Instant::now();
    let cpu = thread_cpu_time();
    let outcome = page.settle(&options).await.unwrap();
    // Quiet was given up; the selector holds.
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::PageTime));
    assert_eq!(json(&page, "!!document.getElementById('late')"), "true");
    // The rest of the 1.5 s was waited for in real time, at a tick per
    // 100 ms, not run flat out.
    let took = started.elapsed();
    assert!(took >= MS(700) && took < Duration::from_secs(4), "{took:?}");
    assert!(thread_cpu_time() - cpu < MS(500));
    let ticks = number(&page, "ticks");
    assert!((13..=18).contains(&ticks), "{ticks}");
}

#[tokio::test]
async fn page_time_passing_for_real_counts_too() {
    // Under the real-time clock nothing is skipped, but a page that never
    // goes quiet still ends on the budget rather than the timeout.
    let server = server();
    let mut page = load_on(
        &server,
        "/budget/interval.html",
        WorkBudget::default(),
        ClockMode::Realtime,
    )
    .await;
    let options = SettleOptions {
        page_time_budget: Some(MS(600)),
        ..settled()
    };
    let started = Instant::now();
    assert_eq!(
        page.settle(&options).await.unwrap(),
        SettleOutcome::BudgetExhausted(Budget::PageTime)
    );
    let took = started.elapsed();
    assert!(took >= MS(500) && took < Duration::from_secs(3), "{took:?}");
}

#[tokio::test]
async fn waiting_for_the_network_does_not_count() {
    let server = server();
    let mut page = load(&server, "/budget/slow-api.html").await;
    // Less than the response takes and the quiet periods after it, which
    // are skipped and do count.
    let options = SettleOptions {
        page_time_budget: Some(MS(700)),
        ..SettleOptions::default()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
    assert_eq!(
        json(&page, "document.getElementById('status').textContent"),
        r#""slow,but,sure""#
    );
}

#[tokio::test]
async fn a_page_that_settles_is_not_held_up_by_the_budget() {
    let server = server();
    let mut page = load(&server, "/settle/after-load.html").await;
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    // Selector waits with no quiet conditions settle as they did.
    let mut page = load(&server, "/budget/interval.html").await;
    let options = SettleOptions {
        selector: Some("#late".into()),
        ..SettleOptions::none()
    };
    assert_eq!(page.settle(&options).await.unwrap(), SettleOutcome::Settled);
}

#[tokio::test]
async fn a_page_fetching_in_a_loop_ends_on_the_request_budget() {
    let server = server();
    let budget = WorkBudget {
        requests: Some(50),
        ..WorkBudget::default()
    };
    let mut page = load_with(&server, "/budget/fetch-loop.html", budget).await;
    let started = Instant::now();
    let outcome = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::BudgetExhausted(Budget::Requests));
    assert!(started.elapsed() < Duration::from_secs(5));
    // The document and 50 batches, give or take the one in flight.
    let made = server.request_count_for("/api/more");
    assert!((48..=50).contains(&made), "{made}");

    // The page stands as it was, and runs again with a new budget.
    assert!(page.js().eval_json("batches").is_err());
    page.set_budget(WorkBudget::unlimited());
    let batches = number(&page, "batches");
    assert!((45..=50).contains(&batches), "{batches}");
    assert_eq!(
        number(&page, "document.querySelectorAll('li').length"),
        batches
    );
}

#[tokio::test]
async fn scrolling_stops_on_the_request_budget() {
    let server = server();
    let budget = WorkBudget {
        requests: Some(20),
        ..WorkBudget::default()
    };
    let mut page = load_with(&server, "/budget/fetch-loop.html", budget).await;
    let report = page
        .auto_scroll(&AutoScrollOptions::default())
        .await
        .unwrap();
    assert_eq!(
        report.stopped,
        AutoScrollStop::BudgetExhausted(Budget::Requests)
    );
}

#[tokio::test]
async fn a_script_that_never_ends_is_stopped_by_the_cpu_budget() {
    let server = server();
    let budget = WorkBudget {
        cpu: Some(MS(200)),
        ..WorkBudget::default()
    };
    let started = Instant::now();
    let mut page = load_with(&server, "/budget/busy.html", budget).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(page.budget_exhausted(), Some(Budget::Cpu));
    assert!(!page.timed_out());
    // Loading stopped where it was.
    assert!(page.html().contains("id=\"before\""));
    assert!(!page.html().contains("id=\"after\""));
    assert_eq!(
        page.run(RunLimits::default()).await,
        RunOutcome::BudgetExhausted(Budget::Cpu)
    );
    page.set_budget(WorkBudget::unlimited());
    assert_eq!(json(&page, "document.title"), r#""Busy""#);
}

#[tokio::test]
async fn idle_callbacks_do_not_hold_the_page_open() {
    let server = server();
    let mut page = load(&server, "/budget/idle-loop.html").await;
    let started = Instant::now();
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        json(&page, "document.getElementById('status').textContent"),
        r#""done""#
    );
    assert!(number(&page, "idle") > 0);
    assert_eq!(page.run(RunLimits::default()).await, RunOutcome::Idle);
}

#[tokio::test]
async fn a_page_busy_running_tasks_is_not_quiet() {
    let server = server();
    let mut page = load(&server, "/budget/busy-then-render.html").await;
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    assert_eq!(
        json(&page, "document.getElementById('status').textContent"),
        r#""rendered""#
    );
}

#[tokio::test]
async fn a_page_busy_waiting_for_a_script_gets_it() {
    let server = server();
    // Without the script the page would compute until this ran out.
    let budget = WorkBudget {
        cpu: Some(Duration::from_secs(2)),
        ..WorkBudget::default()
    };
    let mut page = load_with(&server, "/budget/waits-for-script.html", budget).await;
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    assert_eq!(
        json(&page, "document.getElementById('status').textContent"),
        r#""loaded""#
    );
}
