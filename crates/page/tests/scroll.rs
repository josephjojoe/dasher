//! Lazy content: IntersectionObserver lazy-loading, infinite scroll driven
//! by scroll events and by an IntersectionObserver sentinel, loaders that
//! trigger themselves, and the auto-scroll routine, under each visibility
//! policy.

use std::time::Duration;

use dasher_js::{ClockMode, RuntimeOptions, Viewport, Visibility, WindowOptions};
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
fn options(layout: Visibility) -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            window: WindowOptions {
                visibility: layout,
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

async fn load(server: &FixtureServer, path: &str, layout: Visibility) -> Page {
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

const LAYOUTS: [Visibility; 3] = [Visibility::Loaded, Visibility::All, Visibility::Viewport];

#[tokio::test]
async fn lazy_sections_load_at_once_by_default() {
    for layout in [Visibility::Loaded, Visibility::All] {
        lazy_sections_load_at_once(layout).await;
    }
}

async fn lazy_sections_load_at_once(layout: Visibility) {
    let server = server(FEED_PAGES);
    let page = load(&server, "/scroll/lazy.html", layout).await;
    assert_eq!(count(&page, "section[data-loaded]"), 4, "{layout:?}");
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
    let mut page = load(&server, "/scroll/lazy.html", Visibility::Viewport).await;
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
    let mut page = load(&server, "/scroll/scroll-event.html", Visibility::All).await;
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
        options(Visibility::All),
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

/// Settles a page loaded without settling, with a short limit.
async fn settle_briefly(page: &mut Page) -> SettleOutcome {
    let options = SettleOptions {
        timeout: Duration::from_secs(2),
        ..SettleOptions::default()
    };
    page.settle(&options).await.unwrap()
}

#[tokio::test]
async fn a_self_triggering_loader_stops_by_default() {
    // The loader checks its "loading more" element after every batch. Once
    // a batch has pushed it below the fold, the chain ends.
    let server = server(usize::MAX);
    let session = Client::builder().build().unwrap().session();
    let url = server.url("/scroll/retrigger.html");
    let mut page = Page::load(&session, &url, options(Visibility::Loaded))
        .await
        .unwrap();
    assert_eq!(settle_briefly(&mut page).await, SettleOutcome::Settled);
    let first = server.request_count_for("/api/feed");
    assert!((1..=3).contains(&first), "{first} requests");
    assert_eq!(count(&page, "li"), 10 * first);

    // Each scroll reveals what came in and lets it load a little more,
    // never without end.
    let options = AutoScrollOptions {
        max_scrolls: 3,
        ..AutoScrollOptions::default()
    };
    let report = page.auto_scroll(&options).await.unwrap();
    assert_eq!(report.stopped, AutoScrollStop::ScrollLimit);
    assert_eq!(report.productive_scrolls, 3);
    let after = server.request_count_for("/api/feed");
    assert!(after > first && after <= first + 3 * 3, "{after} requests");
    assert_eq!(count(&page, "li"), 10 * after);
}

#[tokio::test]
async fn a_self_triggering_loader_runs_on_when_everything_is_in_view() {
    // What the default guards against: under `All` the element is always
    // at the top of the viewport, so the loader never stops on its own.
    let server = server(usize::MAX);
    let session = Client::builder().build().unwrap().session();
    let url = server.url("/scroll/retrigger.html");
    let mut page = Page::load(&session, &url, options(Visibility::All))
        .await
        .unwrap();
    assert_eq!(settle_briefly(&mut page).await, SettleOutcome::TimedOut);
    let requests = server.request_count_for("/api/feed");
    assert!(requests > 20, "{requests} requests");
}

#[tokio::test]
async fn content_present_at_load_loads_once_by_default() {
    for layout in LAYOUTS {
        let server = server(FEED_PAGES);
        let page = load(&server, "/scroll/once-lazy.html", layout).await;
        let loaded = count(&page, "section[data-loaded]");
        let reviews = json(&page, "document.getElementById('reviews').textContent");
        let hero = json(&page, "document.images[0].getAttribute('src')");
        if layout == Visibility::Viewport {
            // 40 paragraphs push the sections and reviews out of a 100px
            // viewport; the image near the top still loads.
            assert_eq!(loaded, 0);
            assert_eq!(reviews, r#""""#);
        } else {
            assert_eq!(loaded, 2, "{layout:?}");
            assert_eq!(
                json(&page, "document.querySelectorAll('#comments li').length"),
                "3",
                "{layout:?}"
            );
            assert_eq!(reviews, r#""Section n=reviews""#, "{layout:?}");
        }
        assert_eq!(hero, r#""/img/hero.png""#, "{layout:?}");
        // Each part was fetched once, and no more.
        for part in ["related", "comments", "reviews"] {
            let fetched = server
                .requests()
                .iter()
                .filter(|r| r.query.as_deref() == Some(&format!("n={part}")))
                .count();
            let expected = usize::from(layout != Visibility::Viewport);
            assert_eq!(fetched, expected, "{part} under {layout:?}");
        }
    }
}
