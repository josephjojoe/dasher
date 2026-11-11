//! Memory comes back: a page's allocations are freed when it is dropped,
//! so a long-lived process running job after job does not grow.
//!
//! A counting global allocator tracks the live Rust heap. The JS heap is
//! allocated by QuickJS outside it, and is checked through
//! `Page::memory_usage` and dasher-js's own tests of runtime teardown.
//!
//! Everything runs in one test so that no other test allocates
//! concurrently.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

use dasher::{AutoScrollOptions, Browser, GotoOptions, SettleOptions};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged and only updates a counter on the side.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as isize, Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_add(new_size as isize - layout.size() as isize, Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const JOBS: usize = 100;

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
        .start()
        .unwrap()
}

#[test]
fn sequential_jobs_do_not_grow_the_heap() {
    let server = server();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let browser = Browser::new().unwrap();
    dasher::precompile().unwrap();
    let scroll =
        GotoOptions::settle(SettleOptions::default()).with_auto_scroll(AutoScrollOptions {
            max_scrolls: 10,
            ..AutoScrollOptions::default()
        });

    let mut live = Vec::with_capacity(JOBS);
    runtime.block_on(async {
        for job in 0..JOBS {
            let mut page = browser.new_page();
            // A client-rendered page, an infinite feed and a static one in
            // turn, each read the ways a scraper would.
            match job % 3 {
                0 => {
                    page.goto(&server.url("/spa.html")).await.unwrap();
                    assert_eq!(page.count(".product").unwrap(), 2);
                    let names: Vec<String> = page
                        .evaluate_as("[...document.querySelectorAll('.product a')].map(a => a.textContent.trim())")
                        .await
                        .unwrap();
                    assert_eq!(names, ["Kettle", "Toaster"]);
                }
                1 => {
                    page.goto_with(&server.url("/feed.html"), &scroll)
                        .await
                        .unwrap();
                    assert!(page.count("article").unwrap() > 5);
                }
                _ => {
                    page.goto(&server.url("/hop.html")).await.unwrap();
                }
            }
            assert!(!page.content().is_empty());
            drop(page);
            server.clear_requests();
            live.push(LIVE.load(Relaxed));
        }
    });

    // Connection pools and caches fill up during the first jobs (here, by
    // about the tenth); after that, each job must give back what it took.
    let warm = live[19];
    let last = live[JOBS - 1];
    eprintln!("live Rust heap after job 20: {warm} bytes, after job {JOBS}: {last} bytes");
    assert!(
        last - warm < 4096,
        "the heap grew by {} bytes over {} jobs",
        last - warm,
        JOBS - 20
    );
}
