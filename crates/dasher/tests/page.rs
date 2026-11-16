//! The public API end to end, against local fixtures.

use std::time::{Duration, Instant};

use dasher::{
    AutoScrollOptions, AutoScrollStop, Browser, Budget, ClockMode, ConsoleLevel, Error,
    GotoOptions, JsonValue, SettleOptions, SettleOutcome, WorkBudget,
};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};
use serde::Deserialize;

fn server() -> FixtureServer {
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
        .route("/api/slow", |_| FixtureResponse::json(r#"{"slow":true}"#))
        .route("/cookie", |_| {
            FixtureResponse::html("<title>Cookie</title>")
                .with_header("Set-Cookie", "session=s3cr3t; Path=/; HttpOnly")
        })
        .route("/missing", |_| {
            FixtureResponse::html("<title>Not here</title>").with_status(404)
        })
        .path_latency("/api/products", Duration::from_millis(100))
        .path_latency("/api/slow", Duration::from_millis(300))
        .start()
        .unwrap()
}

fn browser() -> Browser {
    Browser::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[tokio::test]
async fn goto_waits_for_client_rendered_content() {
    let server = server();
    let mut page = browser().new_page();
    let navigation = page.goto(&server.url("/spa.html")).await.unwrap();

    assert_eq!(navigation.status, 200);
    assert_eq!(navigation.url, server.url("/spa.html"));
    assert_eq!(navigation.settle, Some(SettleOutcome::Settled));
    assert!(!navigation.timed_out);
    assert!(
        navigation.elapsed < Duration::from_secs(5),
        "{navigation:?}"
    );

    let html = page.content();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(html.contains(r#"<a href="/p/2">  Toaster  </a>"#), "{html}");
    assert_eq!(page.title().as_deref(), Some("2 products"));
    assert_eq!(page.status(), Some(200));
    assert_eq!(
        page.response_header("Content-Type").as_deref(),
        Some("text/html; charset=utf-8")
    );
}

#[tokio::test]
async fn goto_can_return_at_the_load_event() {
    let server = server();
    let mut page = browser().new_page();
    let navigation = page
        .goto_with(&server.url("/spa.html"), &GotoOptions::load())
        .await
        .unwrap();
    assert_eq!(navigation.settle, None);
    assert_eq!(page.count(".product").unwrap(), 0);
    assert_eq!(page.title().as_deref(), Some("Loading…"));

    // Settling later picks up where loading left off.
    let outcome = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::Settled);
    assert_eq!(page.count(".product").unwrap(), 2);
}

#[tokio::test]
async fn error_statuses_load_and_bad_urls_fail() {
    let server = server();
    let browser = browser();
    let page = browser.open(&server.url("/missing")).await.unwrap();
    assert_eq!(page.status(), Some(404));
    assert_eq!(page.title().as_deref(), Some("Not here"));

    let mut page = browser.new_page();
    assert!(matches!(
        page.goto("not a url").await,
        Err(Error::Load(dasher::LoadError::InvalidUrl(_)))
    ));
    // Nothing is left over from a failed load.
    assert_eq!(page.url(), None);
}

#[tokio::test]
async fn private_addresses_can_be_refused() {
    let server = server();
    let browser = Browser::builder()
        .block_private_addresses(true)
        .build()
        .unwrap();
    let mut page = browser.new_page();
    let error = page.goto(&server.url("/blank.html")).await.unwrap_err();
    let Error::Load(dasher::LoadError::Network(network)) = &error else {
        panic!("expected a network error, got {error:?}");
    };
    assert!(network.is_blocked(), "{error}");
    assert!(
        error
            .to_string()
            .contains("127.0.0.1 is a loopback address"),
        "{error}"
    );
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn a_blank_page_has_no_document() {
    let mut page = browser().new_page();
    assert_eq!(page.content(), "<html><head></head><body></body></html>");
    assert_eq!(page.url(), None);
    assert_eq!(page.count("body").unwrap(), 1);
    assert_eq!(page.text("p").unwrap(), None);
    assert!(page.cookies().is_empty());
    assert!(matches!(page.evaluate("1").await, Err(Error::NoDocument)));
    assert!(matches!(page.set_cookie("a=1"), Err(Error::NoDocument)));
    assert!(matches!(
        page.wait_for_selector("p").await,
        Err(Error::NoDocument)
    ));
    page.close();
}

#[tokio::test]
async fn query_helpers_read_the_live_dom() {
    let server = server();
    let page = browser().open(&server.url("/spa.html")).await.unwrap();

    assert_eq!(page.count("li.product").unwrap(), 2);
    assert_eq!(page.text("h1").unwrap().as_deref(), Some("Products"));
    assert_eq!(page.texts(".product a").unwrap(), ["Kettle", "Toaster"]);
    assert_eq!(page.texts(".product").unwrap(), ["Kettle 25", "Toaster 40"]);
    assert_eq!(
        page.text("#footer").unwrap().as_deref(),
        Some("Made with care in 2026")
    );
    // Script and style text is not part of what a reader sees.
    let body = page.text("html").unwrap().unwrap();
    assert!(
        !body.contains("setTimeout") && !body.contains("color"),
        "{body}"
    );
    assert_eq!(page.text(".nothing").unwrap(), None);

    assert_eq!(
        page.attribute("a", "href").unwrap().as_deref(),
        Some("/p/1")
    );
    assert_eq!(page.attribute("a", "title").unwrap(), None);
    assert_eq!(page.attributes("a", "href").unwrap(), ["/p/1", "/p/2"]);
    assert_eq!(
        page.inner_html(".product").unwrap().as_deref(),
        Some(r#"<a href="/p/1">  Kettle  </a> <span class="price">25</span>"#)
    );
    assert_eq!(
        page.outer_html(".price").unwrap().as_deref(),
        Some(r#"<span class="price">25</span>"#)
    );
    assert_eq!(
        page.outer_htmls(".price").unwrap(),
        [
            r#"<span class="price">25</span>"#,
            r#"<span class="price">40</span>"#
        ]
    );
    assert_eq!(page.inner_htmls(".price").unwrap(), ["25", "40"]);
    assert!(page.inner_htmls(".nothing").unwrap().is_empty());
    assert!(matches!(
        page.texts("a[").unwrap_err(),
        Error::InvalidSelector(s) if s == "a["
    ));
}

#[tokio::test]
async fn shadow_trees_are_in_the_content_and_reachable_by_selectors() {
    let server = server();
    let page = browser().open(&server.url("/shadow.html")).await.unwrap();

    // Selectors look into shadow trees, open or closed.
    assert_eq!(
        page.texts(".price, .note").unwrap(),
        ["25", "Server-rendered"]
    );
    assert_eq!(page.count("product-card h2").unwrap(), 1);
    // Text reads as the shadow tree renders, with slotted content in place.
    assert_eq!(
        page.text("product-card").unwrap().as_deref(),
        Some("Kettle 25 A kettle.")
    );
    // The HTML carries the shadow trees as declarative shadow DOM.
    let card = r#"<product-card id="kettle"><template shadowrootmode="closed"><h2><slot name="name"></slot></h2><span class="price">25</span><p><slot></slot></p></template><span slot="name">Kettle</span> A kettle.</product-card>"#;
    assert_eq!(page.outer_html("#kettle").unwrap().as_deref(), Some(card));
    let content = page.content();
    assert!(content.contains(card), "{content}");
    assert!(
        content.contains(r#"<x-static><template shadowrootmode="open"><p class="note">Server-rendered</p></template></x-static>"#),
        "{content}"
    );
    // Script's own view is unchanged.
    let mut page = page;
    assert_eq!(
        page.evaluate(
            "[document.querySelector('.price'), document.getElementById('kettle').innerHTML]"
        )
        .await
        .unwrap(),
        serde_json::json!([null, "<span slot=\"name\">Kettle</span> A kettle."])
    );
}

#[tokio::test]
async fn evaluate_returns_json_and_awaits_promises() {
    let server = server();
    let mut page = browser().open(&server.url("/spa.html")).await.unwrap();

    assert_eq!(page.evaluate("document.title").await.unwrap(), "2 products");
    assert_eq!(
        page.evaluate("[...document.querySelectorAll('.price')].map(e => +e.textContent)")
            .await
            .unwrap(),
        serde_json::json!([25, 40])
    );
    assert_eq!(page.evaluate("undefined").await.unwrap(), JsonValue::Null);
    assert_eq!(
        page.evaluate("() => location.pathname").await.unwrap(),
        "/spa.html"
    );

    // Promises are awaited while the page runs: here, a fetch.
    let started = Instant::now();
    let slow = page
        .evaluate("async () => (await fetch('/api/slow')).json()")
        .await
        .unwrap();
    assert_eq!(slow, serde_json::json!({"slow": true}));
    assert!(started.elapsed() >= Duration::from_millis(300));

    #[derive(Deserialize, Debug, PartialEq)]
    struct Product {
        name: String,
        price: u32,
    }
    let products: Vec<Product> = page
        .evaluate_as(
            "[...document.querySelectorAll('.product')].map(li => ({
                name: li.querySelector('a').textContent.trim(),
                price: +li.querySelector('.price').textContent,
            }))",
        )
        .await
        .unwrap();
    assert_eq!(
        products,
        [
            Product {
                name: "Kettle".into(),
                price: 25
            },
            Product {
                name: "Toaster".into(),
                price: 40
            },
        ]
    );
    assert!(matches!(
        page.evaluate_as::<u32>("'not a number'").await,
        Err(Error::Json(_))
    ));

    let Err(Error::Script(thrown)) = page.evaluate("missing.property").await else {
        panic!("a ReferenceError");
    };
    assert_eq!(thrown.message, "ReferenceError: missing is not defined");
    let Err(Error::Script(rejected)) = page.evaluate("Promise.reject(new Error('nope'))").await
    else {
        panic!("a rejection");
    };
    assert_eq!(rejected.message, "Error: nope");

    // A promise nothing will ever settle fails at once rather than hang.
    assert!(matches!(
        page.evaluate("new Promise(() => {})").await,
        Err(Error::Timeout)
    ));
    // Evaluation errors are not the page's.
    assert_eq!(page.errors().len(), 1, "{:?}", page.errors());
}

#[tokio::test]
async fn evaluating_a_slow_promise_times_out() {
    let server = server();
    let browser = Browser::builder()
        .clock(ClockMode::Realtime)
        .build()
        .unwrap();
    let mut page = browser.new_page();
    page.goto_with(&server.url("/blank.html"), &GotoOptions::load())
        .await
        .unwrap();
    page.set_timeout(Duration::from_millis(200));
    let started = Instant::now();
    let result = page
        .evaluate("new Promise((resolve) => setTimeout(resolve, 5000))")
        .await;
    assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(2));
    // The page still works afterwards.
    assert_eq!(page.evaluate("1 + 1").await.unwrap(), 2);
}

#[tokio::test]
async fn waiting_for_selectors_and_functions() {
    let server = server();
    let mut page = browser().new_page();
    page.goto_with(&server.url("/late.html"), &GotoOptions::load())
        .await
        .unwrap();
    assert_eq!(page.count("#late").unwrap(), 0);
    page.wait_for_selector("#late").await.unwrap();
    assert_eq!(page.text("#late").unwrap().as_deref(), Some("here at last"));
    page.wait_for_function("window.ready === true")
        .await
        .unwrap();

    page.set_timeout(Duration::from_millis(100));
    assert!(matches!(
        page.wait_for_selector("#never").await,
        Err(Error::Timeout)
    ));
    assert!(matches!(
        page.wait_for_function("window.never").await,
        Err(Error::Timeout)
    ));
    assert!(matches!(
        page.wait_for_selector("##").await,
        Err(Error::InvalidSelector(_))
    ));
}

#[tokio::test]
async fn navigations_requested_after_load_are_followed() {
    let server = server();
    let mut page = browser().new_page();
    let navigation = page.goto(&server.url("/hop.html")).await.unwrap();
    assert_eq!(navigation.url, server.url("/late.html"));
    assert_eq!(navigation.settle, Some(SettleOutcome::Settled));
    assert_eq!(page.title().as_deref(), Some("Late"));
    assert_eq!(page.count("#root").unwrap(), 1);
}

#[tokio::test]
async fn goto_with_auto_scroll_loads_an_infinite_feed() {
    let server = server();
    let mut page = browser().new_page();
    let options = GotoOptions::default().with_auto_scroll(AutoScrollOptions::default());
    let navigation = page
        .goto_with(&server.url("/feed.html"), &options)
        .await
        .unwrap();
    let scroll = navigation.scroll.unwrap();
    assert_eq!(scroll.stopped, AutoScrollStop::NoNewContent);
    assert_eq!(scroll.productive_scrolls, 3);
    assert_eq!(page.count("article").unwrap(), 16);
    assert_eq!(page.texts("article").unwrap().last().unwrap(), "post 3.5");
}

#[tokio::test]
async fn manual_scrolling_reaches_the_page() {
    let server = server();
    let mut page = browser().new_page();
    page.goto(&server.url("/feed.html")).await.unwrap();
    page.scroll_to(0.0, 400.0).unwrap();
    assert_eq!(page.evaluate("scrollY").await.unwrap(), 400);
    page.scroll_by(0.0, -100.0).unwrap();
    assert_eq!(page.evaluate("scrollY").await.unwrap(), 300);
    // A scroll to the bottom (scrolling stops there) fires the feed's
    // listener; settling lets the fetch it started land.
    page.scroll_to(0.0, 1e9).unwrap();
    page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(page.count("article").unwrap(), 6);
}

#[tokio::test]
async fn cookies_are_shared_through_the_browser() {
    let server = server();
    let browser = browser();
    browser
        .set_cookie(&server.url("/"), "preset=1; Path=/")
        .unwrap();

    let mut page = browser.new_page();
    page.goto(&server.url("/cookie")).await.unwrap();
    let request = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/cookie")
        .unwrap();
    assert_eq!(request.header("cookie"), Some("preset=1"));

    let names = |page: &dasher::Page| -> Vec<(String, String)> {
        page.cookies()
            .iter()
            .map(|c| (c.name().to_owned(), c.value().to_owned()))
            .collect()
    };
    assert_eq!(
        names(&page),
        [
            ("preset".into(), "1".into()),
            ("session".into(), "s3cr3t".into())
        ]
    );
    assert!(page.cookies()[1].http_only());

    page.set_cookie("theme=dark").unwrap();
    assert_eq!(
        page.evaluate("document.cookie").await.unwrap(),
        "preset=1; theme=dark"
    );

    // A second page of the same browser sees the same jar.
    let other = browser.open(&server.url("/blank.html")).await.unwrap();
    assert_eq!(names(&other).len(), 3);
    assert_eq!(browser.cookies().len(), 3);
}

#[tokio::test]
async fn console_messages_and_script_errors_are_kept() {
    let server = server();
    let page = browser().open(&server.url("/spa.html")).await.unwrap();
    let console = page.console();
    let texts: Vec<(ConsoleLevel, &str)> =
        console.iter().map(|m| (m.level, m.text.as_str())).collect();
    assert_eq!(
        texts,
        [
            (ConsoleLevel::Log, "rendering 2 pages"),
            (ConsoleLevel::Warn, "rendered 2"),
        ]
    );
    let errors = page.errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, dasher::ScriptErrorKind::Uncaught);
    assert_eq!(errors[0].message, "Error: widget failed");
}

#[tokio::test]
async fn browser_configuration_reaches_requests_and_scripts() {
    let server = server();
    let browser = Browser::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) TestBot/1.0")
        .viewport(390, 844)
        .header("X-Api-Key", "k")
        .header("Accept-Language", "fr-FR")
        .block("*/blocked/*")
        .build()
        .unwrap();
    let mut page = browser.open(&server.url("/blank.html")).await.unwrap();

    let request = server
        .requests()
        .into_iter()
        .find(|r| r.path == "/blank.html")
        .unwrap();
    assert_eq!(
        request.header("user-agent"),
        Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) TestBot/1.0")
    );
    assert_eq!(request.header("x-api-key"), Some("k"));
    assert_eq!(request.header("accept-language"), Some("fr-FR"));
    assert_eq!(server.request_count_for("/blocked/tracker.js"), 0);
    assert_eq!(page.evaluate("trackerLoaded").await.unwrap(), false);

    let env = page
        .evaluate("[navigator.userAgent, navigator.platform, innerWidth, innerHeight]")
        .await
        .unwrap();
    assert_eq!(
        env,
        serde_json::json!([
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) TestBot/1.0",
            "Win32",
            390,
            844
        ])
    );
}

#[tokio::test]
async fn storage_is_shared_like_a_browser_profile() {
    let server = server();
    let browser = browser();
    let mut first = browser.open(&server.url("/blank.html")).await.unwrap();
    first
        .evaluate("localStorage.setItem('l', '1'); sessionStorage.setItem('s', '1')")
        .await
        .unwrap();

    // sessionStorage lives as long as the tab, across documents.
    first.goto(&server.url("/late.html")).await.unwrap();
    assert_eq!(
        first
            .evaluate("[localStorage.getItem('l'), sessionStorage.getItem('s')]")
            .await
            .unwrap(),
        serde_json::json!(["1", "1"])
    );
    // localStorage is the browser's; sessionStorage the tab's.
    let mut second = browser.open(&server.url("/blank.html")).await.unwrap();
    assert_eq!(
        second
            .evaluate("[localStorage.getItem('l'), sessionStorage.getItem('s')]")
            .await
            .unwrap(),
        serde_json::json!(["1", null])
    );
}

#[tokio::test]
async fn goto_timeout_bounds_the_whole_navigation() {
    let server = server();
    let browser = Browser::builder()
        .clock(ClockMode::Realtime)
        .build()
        .unwrap();
    let mut page = browser.new_page();
    let options = GotoOptions::settle(SettleOptions {
        selector: Some("#late".into()),
        ..SettleOptions::none()
    })
    .with_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let navigation = page
        .goto_with(&server.url("/late.html"), &options)
        .await
        .unwrap();
    assert!(navigation.timed_out);
    assert_eq!(navigation.settle, Some(SettleOutcome::TimedOut));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(page.count("#root").unwrap(), 1);
}

#[tokio::test]
async fn memory_usage_covers_the_current_document() {
    let server = server();
    let mut page = browser().new_page();
    assert_eq!(page.memory_usage(), dasher::MemoryUsage::default());

    page.goto(&server.url("/spa.html")).await.unwrap();
    let usage = page.memory_usage();
    assert!(usage.js_heap > 100_000, "{usage:?}");
    // The document, doctype, html, head, body, the products and their text.
    assert!(usage.dom_nodes > 30, "{usage:?}");

    page.evaluate("globalThis.big = 'x'.repeat(1 << 20); 0")
        .await
        .unwrap();
    // The megabyte of string shows up, less whatever garbage from the page
    // load the collector takes the chance to free (a few KiB).
    let after = page.memory_usage();
    assert!(
        after.js_heap >= usage.js_heap + 1_000_000,
        "{usage:?} then {after:?}"
    );
}

#[tokio::test]
async fn a_page_that_never_goes_quiet_ends_on_its_page_time_budget() {
    let server = server();
    let mut page = browser().new_page();
    // By default only the network has to go idle, which it does.
    let navigation = page.goto(&server.url("/ticker.html")).await.unwrap();
    assert_eq!(navigation.settle, Some(SettleOutcome::Settled));
    assert_eq!(navigation.budget_exhausted, None);

    // Waiting for DOM quiet too, the page never gets there.
    let options = GotoOptions::settle(SettleOptions {
        dom_quiet: Some(dasher::DEFAULT_QUIET),
        ..SettleOptions::default()
    });
    let started = Instant::now();
    let navigation = page
        .goto_with(&server.url("/ticker.html"), &options)
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!navigation.timed_out);
    assert_eq!(navigation.budget_exhausted, Some(Budget::PageTime));
    assert_eq!(
        navigation.settle,
        Some(SettleOutcome::BudgetExhausted(Budget::PageTime))
    );
    assert_eq!(page.text("#news").unwrap().as_deref(), Some("Headline"));
    // Five seconds of page time at four ticks a second.
    let ticks: u32 = page
        .evaluate_as("+document.getElementById('clock').textContent")
        .await
        .unwrap();
    assert!((18..=24).contains(&ticks), "{ticks}");
}

#[tokio::test]
async fn the_work_budget_bounds_a_navigation_and_is_lifted_after() {
    let server = server();
    let mut page = browser().new_page();
    let options = GotoOptions::default().with_budget(WorkBudget {
        requests: Some(30),
        ..WorkBudget::default()
    });
    let navigation = page
        .goto_with(&server.url("/more.html"), &options)
        .await
        .unwrap();
    assert_eq!(navigation.budget_exhausted, Some(Budget::Requests));
    assert_eq!(
        navigation.settle,
        Some(SettleOutcome::BudgetExhausted(Budget::Requests))
    );
    assert!(
        (30..=32).contains(&navigation.requests),
        "{}",
        navigation.requests
    );
    // The page can still be read and evaluated.
    let items = page.count("li").unwrap();
    assert!((25..=31).contains(&items), "{items}");
    let title: String = page.evaluate_as("document.title").await.unwrap();
    assert_eq!(title, "More");

    // Without a budget, the same page runs to the timeout.
    let options = GotoOptions::default()
        .with_budget(WorkBudget::unlimited())
        .with_timeout(Duration::from_millis(500));
    let navigation = page
        .goto_with(&server.url("/more.html"), &options)
        .await
        .unwrap();
    assert!(navigation.timed_out);
    assert_eq!(navigation.budget_exhausted, None);
}

#[tokio::test]
async fn evaluate_resolves_imports_against_the_document() {
    let server = server();
    let mut page = browser().new_page();
    page.goto(&server.url("/blank.html")).await.unwrap();

    for specifier in ["/lib/answer.js", "./lib/answer.js"] {
        let script = format!("import({specifier:?}).then((m) => m.default)");
        assert_eq!(page.evaluate(&script).await.unwrap(), 42, "{specifier}");
    }
}

// Loading the page leaves a connection to the server in the pool, driven by
// this thread's runtime. A request that blocks the page (synchronous XHR, a
// module fetched on demand) runs on another thread and must not wait on it.
#[tokio::test]
async fn blocking_requests_do_not_wait_on_pooled_connections() {
    let server = server();
    let mut page = browser().new_page();
    page.goto(&server.url("/spa.html")).await.unwrap();

    let started = Instant::now();
    let status = page
        .evaluate(
            "(() => { const x = new XMLHttpRequest(); x.open('GET', '/api/products', false); \
             x.send(); return x.status; })()",
        )
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        page.evaluate("import('/lib/answer.js').then((m) => m.default)")
            .await
            .unwrap(),
        42
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}
