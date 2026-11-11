//! Lazy content: IntersectionObserver lazy-loading, infinite scroll driven
//! by scroll events and by an IntersectionObserver sentinel, and the
//! auto-scroll routine, in both layout modes.

use std::time::Duration;

use dasher_js::{ClockMode, LayoutMode, RuntimeOptions, Viewport, WindowOptions};
use dasher_net::Client;
use dasher_page::{
    AutoScrollOptions, AutoScrollStop, Page, PageOptions, SettleOptions, SettleOutcome,
};
use dasher_test_server::{FixtureResponse, FixtureServer, RecordedRequest, fixtures_dir};

/// Pages of the feed before it runs out.
const FEED_PAGES: usize = 5;

fn feed(request: &RecordedRequest, pages: usize) -> FixtureResponse {
    let page: usize = request
        .query
        .as_deref()
        .and_then(|q| q.strip_prefix("page="))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let items: Vec<String> = if page < pages {
        (0..10)
            .map(|i| format!("\"item {}\"", page * 10 + i))
            .collect()
    } else {
        Vec::new()
    };
    FixtureResponse::json(format!("[{}]", items.join(",")))
}

fn server(pages: usize) -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        .latency(Duration::from_millis(20))
        .route("/api/feed", move |r| feed(r, pages))
        .route("/api/section", |r| {
            FixtureResponse::text(format!("Section {}", r.query.as_deref().unwrap_or("")))
        })
        .start()
        .unwrap()
}

/// A short viewport, so the scrolled layout has room to scroll.
fn options(layout: LayoutMode) -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            window: WindowOptions {
                layout,
                viewport: Viewport {
                    height: 100,
                    ..Viewport::default()
                },
                ..WindowOptions::default()
            },
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(10)),
        ..PageOptions::default()
    }
}

async fn load(server: &FixtureServer, path: &str, layout: LayoutMode) -> Page {
    let session = Client::builder().build().unwrap().session();
    let mut page = Page::load(&session, &server.url(path), options(layout))
        .await
        .unwrap();
    assert_eq!(
        page.settle(&SettleOptions::default()).await.unwrap(),
        SettleOutcome::Settled
    );
    page
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn count(page: &Page, selector: &str) -> usize {
    json(
        page,
        &format!("document.querySelectorAll('{selector}').length"),
    )
    .parse()
    .unwrap()
}

const LAYOUTS: [LayoutMode; 2] = [LayoutMode::InView, LayoutMode::Scrolled];

#[tokio::test]
async fn lazy_sections_load_at_once_by_default() {
    let server = server(FEED_PAGES);
    let page = load(&server, "/scroll/lazy.html", LayoutMode::InView).await;
    assert_eq!(count(&page, "section[data-loaded]"), 4);
    assert_eq!(
        json(
            &page,
            "document.querySelector('section:last-of-type .body').textContent"
        ),
        r#""Section n=4""#
    );
    assert_eq!(
        json(
            &page,
            "[...document.images].map((img) => img.getAttribute('src'))"
        ),
        r#"["/img/1.png","/img/2.png"]"#
    );
    // Images are never downloaded.
    assert_eq!(server.request_count_for("/img/1.png"), 0);
}

#[tokio::test]
async fn lazy_sections_wait_for_scrolling_in_the_scrolled_layout() {
    let server = server(FEED_PAGES);
    let mut page = load(&server, "/scroll/lazy.html", LayoutMode::Scrolled).await;
    // 100px of viewport: the first two sections (60px each) touch it.
    assert_eq!(count(&page, "section[data-loaded]"), 2);
    assert_eq!(
        json(&page, "document.images[0].getAttribute('src')"),
        "null"
    );
    let report = page
        .auto_scroll(&AutoScrollOptions::default())
        .await
        .unwrap();
    assert_eq!(report.stopped, AutoScrollStop::NoNewContent);
    assert_eq!(count(&page, "section[data-loaded]"), 4);
    assert_eq!(count(&page, "img[src]"), 2);
}

#[tokio::test]
async fn scroll_event_infinite_scroll_loads_every_page() {
    for layout in LAYOUTS {
        let server = server(FEED_PAGES);
        let mut page = load(&server, "/scroll/scroll-event.html", layout).await;
        // The first page loads on its own.
        assert_eq!(count(&page, ".item"), 10, "{layout:?}");
        let report = page
            .auto_scroll(&AutoScrollOptions::default())
            .await
            .unwrap();
        assert_eq!(report.stopped, AutoScrollStop::NoNewContent, "{layout:?}");
        assert_eq!(count(&page, ".item"), 50, "{layout:?}");
        assert_eq!(report.productive_scrolls, 4, "{layout:?}");
        assert_eq!(
            json(&page, "document.getElementById('end').hidden"),
            "false"
        );
        // One request per page, plus the one that came back empty.
        assert_eq!(server.request_count_for("/api/feed"), 6, "{layout:?}");
    }
}

#[tokio::test]
async fn sentinel_infinite_scroll_loads_every_page() {
    for layout in LAYOUTS {
        let server = server(FEED_PAGES);
        let mut page = load(&server, "/scroll/sentinel.html", layout).await;
        assert_eq!(count(&page, "li"), 10, "{layout:?}");
        let report = page
            .auto_scroll(&AutoScrollOptions::default())
            .await
            .unwrap();
        assert_eq!(report.stopped, AutoScrollStop::NoNewContent, "{layout:?}");
        assert_eq!(count(&page, "li"), 50, "{layout:?}");
        assert_eq!(count(&page, "#sentinel"), 0, "{layout:?}");
        assert_eq!(
            json(&page, "document.querySelector('li:last-child').textContent"),
            r#""item 49""#
        );
    }
}

#[tokio::test]
async fn an_endless_feed_stops_at_the_scroll_limit() {
    let server = server(usize::MAX);
    let mut page = load(&server, "/scroll/scroll-event.html", LayoutMode::InView).await;
    let options = AutoScrollOptions {
        max_scrolls: 3,
        ..AutoScrollOptions::default()
    };
    let report = page.auto_scroll(&options).await.unwrap();
    assert_eq!(report.stopped, AutoScrollStop::ScrollLimit);
    assert_eq!(report.scrolls, 3);
    assert_eq!(count(&page, ".item"), 40);
}

#[tokio::test]
async fn auto_scroll_respects_its_timeout() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/api/feed", |r| feed(r, usize::MAX))
        .latency(Duration::from_millis(400))
        .start()
        .unwrap();
    let session = Client::builder().build().unwrap().session();
    let mut page = Page::load(
        &session,
        &server.url("/scroll/scroll-event.html"),
        options(LayoutMode::InView),
    )
    .await
    .unwrap();
    let options = AutoScrollOptions {
        timeout: Some(Duration::from_millis(1000)),
        ..AutoScrollOptions::default()
    };
    let report = page.auto_scroll(&options).await.unwrap();
    assert_eq!(report.stopped, AutoScrollStop::TimedOut);
    assert!(report.scrolls >= 1 && report.scrolls < 5, "{report:?}");
}
