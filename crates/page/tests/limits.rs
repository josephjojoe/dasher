//! Pages that go over their resource limits while loading, settling or
//! scrolling stop with an error that says which limit.

use std::time::Duration;

use dasher_js::{ClockMode, ResourceLimit, RuntimeOptions};
use dasher_net::Client;
use dasher_page::{
    AutoScrollOptions, AutoScrollStop, LoadError, Page, PageOptions, SettleOptions, SettleOutcome,
};
use dasher_test_server::{FixtureResponse, FixtureServer};

const MIB: usize = 1024 * 1024;
const NODES: usize = 5000;

fn server() -> FixtureServer {
    FixtureServer::builder()
        .route("/small", |_| {
            FixtureResponse::html("<!doctype html><title>ok</title><p>fine</p>")
        })
        .route("/huge", |_| {
            // 50,000 rows of plain markup, far past the node limit.
            FixtureResponse::html(format!(
                "<!doctype html><table>{}</table>",
                "<tr><td>cell</td></tr>".repeat(50_000)
            ))
        })
        .route("/builds-while-loading", |_| {
            FixtureResponse::html(
                "<!doctype html><ul id=list></ul><script>
                   const list = document.getElementById('list');
                   for (let i = 0; i < 100000; i++) list.appendChild(document.createElement('li'));
                 </script><p id=after>never parsed</p>",
            )
        })
        .route("/leaks-later", |_| {
            FixtureResponse::html(
                "<!doctype html><script>
                   const keep = [];
                   setInterval(() => {
                     for (let i = 0; i < 2000; i++) keep.push('z'.repeat(1024) + i);
                   }, 50);
                 </script>",
            )
        })
        .route("/endless-feed", |_| {
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

fn options() -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            memory_limit: Some(32 * MIB),
            max_dom_nodes: Some(NODES),
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(10)),
        ..PageOptions::default()
    }
}

async fn load(server: &FixtureServer, path: &str) -> Result<Page, LoadError> {
    let session = Client::builder().build().unwrap().session();
    Page::load(&session, &server.url(path), options()).await
}

#[tokio::test]
async fn pages_within_their_limits_load() {
    let server = server();
    let page = load(&server, "/small").await.unwrap();
    assert_eq!(page.js().limit_exceeded(), None);
}

#[tokio::test]
async fn a_document_too_big_to_parse_fails_to_load() {
    let server = server();
    let err = load(&server, "/huge").await.unwrap_err();
    let LoadError::LimitExceeded(limit) = err else {
        panic!("{err:?}");
    };
    assert_eq!(limit, ResourceLimit::DomNodes { limit: NODES });
    assert_eq!(
        err.to_string(),
        "loading stopped: the page went over its limit of 5000 DOM nodes"
    );
}

#[tokio::test]
async fn a_script_building_too_much_stops_the_load() {
    let server = server();
    let err = load(&server, "/builds-while-loading").await.unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::LimitExceeded(ResourceLimit::DomNodes { limit: NODES })
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn filling_the_heap_ends_settling() {
    let server = server();
    let mut page = load(&server, "/leaks-later").await.unwrap();
    let outcome = page
        .settle(&SettleOptions {
            // Never there: the page runs until something stops it.
            selector: Some("#never".into()),
            timeout: Duration::from_secs(10),
            ..SettleOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        SettleOutcome::LimitExceeded(ResourceLimit::JsHeap { limit: 32 * MIB })
    );
    // Settling again ends at once.
    let again = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(again, outcome);
}

#[tokio::test]
async fn scrolling_past_the_node_limit_ends_auto_scroll() {
    let server = server();
    let mut page = load(&server, "/endless-feed").await.unwrap();
    let report = page
        .auto_scroll(&AutoScrollOptions {
            max_scrolls: 100,
            ..AutoScrollOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(
        report.stopped,
        AutoScrollStop::LimitExceeded(ResourceLimit::DomNodes { limit: NODES })
    );
    assert!(report.scrolls < 20, "{report:?}");
}
