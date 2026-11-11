//! Per-page limits through the public API: a page over its JS heap or DOM
//! node limit fails the operation with `Error::LimitExceeded` and is left
//! blank, ready for another `goto`.

use std::time::Duration;

use dasher::{
    AutoScrollOptions, Browser, Error, GotoOptions, MemoryUsage, ResourceLimit, SettleOptions,
};
use dasher_test_server::{FixtureResponse, FixtureServer};

const MIB: usize = 1024 * 1024;

fn server() -> FixtureServer {
    FixtureServer::builder()
        .route("/ok", |_| {
            FixtureResponse::html("<!doctype html><title>ok</title><ul id=list></ul>")
        })
        .route("/builder", |_| {
            FixtureResponse::html(
                "<!doctype html><ul id=list></ul><script>
                   const list = document.getElementById('list');
                   for (let i = 0; i < 50000; i++) list.appendChild(document.createElement('li'));
                 </script>",
            )
        })
        .route("/feed", |_| {
            FixtureResponse::html(
                "<!doctype html><div id=feed></div><script>
                   addEventListener('scroll', () => {
                     const feed = document.getElementById('feed');
                     for (let i = 0; i < 500; i++) feed.appendChild(document.createElement('article'));
                   });
                 </script>",
            )
        })
        .start()
        .unwrap()
}

fn browser() -> Browser {
    Browser::builder()
        .memory_limit(Some(32 * MIB))
        .max_dom_nodes(Some(5000))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn assert_blank(page: &dasher::Page) {
    assert_eq!(page.url(), None);
    assert_eq!(page.memory_usage(), MemoryUsage::default());
}

#[tokio::test]
async fn goto_fails_when_the_page_builds_too_many_nodes() {
    let server = server();
    let mut page = browser().new_page();
    let err = page.goto(&server.url("/builder")).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::LimitExceeded(ResourceLimit::DomNodes { limit: 5000 })
        ),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "the page went over its limit of 5000 DOM nodes"
    );
    assert_blank(&page);

    // The page can load something else.
    page.goto(&server.url("/ok")).await.unwrap();
    assert_eq!(page.title().as_deref(), Some("ok"));
}

#[tokio::test]
async fn evaluate_fails_when_the_page_fills_its_heap() {
    let server = server();
    let mut page = browser().new_page();
    page.goto(&server.url("/ok")).await.unwrap();
    let err = page
        .evaluate(
            "const keep = []; try { for (;;) keep.push('x'.repeat(4096) + keep.length); } catch {} 1",
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::LimitExceeded(ResourceLimit::JsHeap { limit }) if limit == 32 * MIB
        ),
        "{err:?}"
    );
    assert_blank(&page);
    assert!(matches!(page.evaluate("1").await, Err(Error::NoDocument)));
}

#[tokio::test]
async fn auto_scroll_fails_when_the_feed_outgrows_the_limit() {
    let server = server();
    let mut page = browser().new_page();
    let options =
        GotoOptions::settle(SettleOptions::default()).with_auto_scroll(AutoScrollOptions {
            max_scrolls: 50,
            ..AutoScrollOptions::default()
        });
    let err = page
        .goto_with(&server.url("/feed"), &options)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::LimitExceeded(_)), "{err:?}");
    assert_blank(&page);
}

#[tokio::test]
async fn limits_can_be_lifted() {
    let server = server();
    let browser = Browser::builder()
        .max_dom_nodes(None)
        .memory_limit(None)
        .build()
        .unwrap();
    let mut page = browser.new_page();
    page.goto(&server.url("/builder")).await.unwrap();
    assert_eq!(page.count("li").unwrap(), 50_000);
    assert!(page.memory_usage().dom_nodes > 50_000);
}
