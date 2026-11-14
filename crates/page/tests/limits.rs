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
        .route("/churns-while-loading", |_| {
            // Ten times the node limit made and dropped while the parser is
            // still going, then a ticker rewriting a list.
            FixtureResponse::html(
                "<!doctype html><div id=outer><p id=clock></p><script>
                   const clock = document.getElementById('clock');
                   for (let i = 0; i < 50000; i++) clock.textContent = 'tick ' + i;
                 </script><ul id=list></ul><script>
                   const list = document.getElementById('list');
                   let n = 0;
                   setInterval(() => {
                     for (let i = 0; i < 1000; i++, n++) {
                       list.innerHTML = '<li>' + n + '</li><li><b>' + n + '</b></li>';
                     }
                   }, 10);
                 </script><p id=after>parsed</p></div>",
            )
        })
        .route("/removed-scripts", |_| {
            // Scripts removed, with nothing left referring to them, before
            // they run: they still run, as in browsers, while churn frees
            // whatever is garbage.
            FixtureResponse::html(
                "<!doctype html><head><script defer src=/ran.js?defer></script>
                 <script async src=/ran.js?async></script></head><body><p id=clock></p><script>
                   for (const s of document.querySelectorAll('script[src]')) s.remove();
                   {
                     const s = document.createElement('script');
                     s.src = '/ran.js?inserted';
                     document.head.append(s);
                     s.remove();
                   }
                   // Wrapped garbage, so the cycle collector runs and takes
                   // the removed scripts' wrappers too.
                   const clock = document.getElementById('clock');
                   for (let i = 0; i < 20000; i++) clock.replaceChildren(document.createElement('b'));
                 </script>",
            )
        })
        .route("/ran.js", |req| {
            let which = req.query.clone().unwrap_or_default();
            FixtureResponse::ok(
                "text/javascript",
                format!(
                    "(globalThis.ran ||= []).push('{which}');
                     for (let i = 0; i < 20000; i++) clock.replaceChildren(document.createElement('i'));"
                )
                .into_bytes(),
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
    load_with(server, path, options()).await
}

async fn load_with(
    server: &FixtureServer,
    path: &str,
    options: PageOptions,
) -> Result<Page, LoadError> {
    let session = Client::builder().build().unwrap().session();
    Page::load(&session, &server.url(path), options).await
}

/// For pages that churn for a while, which takes seconds in debug builds.
fn patient() -> PageOptions {
    PageOptions {
        timeout: Some(Duration::from_secs(60)),
        ..options()
    }
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

#[tokio::test]
async fn churn_while_loading_and_after_stays_within_the_limit() {
    let server = server();
    let mut page = load_with(&server, "/churns-while-loading", patient())
        .await
        .unwrap();
    assert_eq!(page.js().limit_exceeded(), None);
    assert_eq!(
        page.js()
            .eval_json("document.getElementById('after').textContent")
            .unwrap(),
        Some("\"parsed\"".into())
    );
    // Ticks until 20,000 lists have come and gone: twelve times the node
    // limit in elements and text.
    let outcome = page
        .settle(&SettleOptions {
            network_idle: None,
            dom_quiet: None,
            selector: None,
            predicate: Some("n >= 20000".into()),
            timeout: Duration::from_secs(120),
            // Let the clock skip through every tick.
            page_time_budget: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome, SettleOutcome::Settled);
    assert_eq!(page.js().limit_exceeded(), None);
}

#[tokio::test]
async fn removed_scripts_still_run_while_the_page_churns() {
    let server = server();
    let page = load_with(&server, "/removed-scripts", patient())
        .await
        .unwrap();
    assert_eq!(page.js().limit_exceeded(), None);
    assert_eq!(
        page.js().eval_json("ran.sort().join()").unwrap(),
        Some("\"async,defer,inserted\"".into())
    );
}
