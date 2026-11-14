//! Per-page resource limits: a page that fills its JS heap or builds too
//! many DOM nodes is stopped for good, with an error that says which.

use dasher_dom::Document;
use dasher_js::{
    ClockMode, ErrorKind, JsRuntime, ResourceLimit, RunLimits, RunOutcome, RuntimeOptions,
};

const MIB: usize = 1024 * 1024;

fn page(options: RuntimeOptions) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..options
    })
    .unwrap();
    js.set_document(Document::parse("<!doctype html><body><div id=list></div>"));
    js
}

fn with_heap(limit: usize) -> JsRuntime {
    page(RuntimeOptions {
        memory_limit: Some(limit),
        ..RuntimeOptions::default()
    })
}

fn with_nodes(limit: usize) -> JsRuntime {
    page(RuntimeOptions {
        max_dom_nodes: Some(limit),
        ..RuntimeOptions::default()
    })
}

fn nodes(js: &JsRuntime) -> usize {
    js.with_document(Document::len)
}

#[test]
fn filling_the_heap_stops_the_page_even_if_it_catches_the_error() {
    let js = with_heap(16 * MIB);
    // The page swallows the out-of-memory error and carries on.
    let err = js
        .run_script(
            "const keep = [];
             try { for (;;) keep.push('x'.repeat(4096) + keep.length); } catch (e) {}
             for (;;) {}",
            "hog.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
    assert_eq!(err.message, "the page ran out of JS heap (limit 16 MiB)");
    let limit = ResourceLimit::JsHeap { limit: 16 * MIB };
    assert_eq!(js.limit_exceeded(), Some(limit));

    // Nothing more runs, and the error is in the page's log.
    let refused = js.eval_json("1").unwrap_err();
    assert_eq!(refused.kind, ErrorKind::OutOfMemory);
    assert!(js.run_script("1", "later.js").is_err());
    assert!(js.errors().iter().any(|e| e.kind == ErrorKind::OutOfMemory));
}

#[tokio::test]
async fn the_event_loop_reports_the_limit() {
    let js = with_heap(16 * MIB);
    js.run_script(
        "const keep = [];
         setInterval(() => { for (let i = 0; i < 1000; i++) keep.push('y'.repeat(1024) + i); }, 10);",
        "leak.js",
    )
    .unwrap();
    let outcome = js.run(RunLimits::default()).await;
    assert_eq!(
        outcome,
        RunOutcome::LimitExceeded(ResourceLimit::JsHeap { limit: 16 * MIB })
    );
    // It stays stopped.
    assert_eq!(js.run(RunLimits::default()).await, outcome);
}

#[test]
fn one_absurd_allocation_only_throws() {
    let js = with_heap(64 * MIB);
    js.run_script(
        "try { new ArrayBuffer(2 ** 31); } catch (e) { globalThis.failed = e.name; }",
        "probe.js",
    )
    .unwrap();
    assert_eq!(js.limit_exceeded(), None);
    assert!(js.eval_json("failed").unwrap().is_some());
}

#[test]
fn too_many_nodes_stop_the_page() {
    let js = with_nodes(5000);
    let err = js
        .run_script(
            "const list = document.getElementById('list');
             for (;;) list.appendChild(document.createElement('li'));",
            "grow.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
    assert_eq!(
        err.message,
        "the page went over its limit of 5000 DOM nodes"
    );
    assert_eq!(
        js.limit_exceeded(),
        Some(ResourceLimit::DomNodes { limit: 5000 })
    );
    // Cut off at the next interrupt check: a few thousand nodes over at
    // most.
    assert!(nodes(&js) < 20_000, "{} nodes", nodes(&js));
    assert!(js.eval_json("1").is_err());
}

#[test]
fn one_huge_inner_html_is_cut_short() {
    let js = with_nodes(10_000);
    // Two million nodes' worth of markup in one assignment.
    let err = js
        .run_script(
            "document.getElementById('list').innerHTML = '<p>x</p>'.repeat(1e6);",
            "flood.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
    assert!(nodes(&js) < 20_000, "{} nodes", nodes(&js));
}

#[test]
fn nodes_held_outside_the_tree_count() {
    let js = with_nodes(5000);
    let err = js
        .run_script(
            "const held = [];
             for (let i = 0; i < 10000; i++) held.push(document.createElement('span'));",
            "hold.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
}

#[test]
fn pages_within_their_limits_are_unaffected() {
    let js = with_nodes(5000);
    js.run_script(
        "const list = document.getElementById('list');
         for (let i = 0; i < 1000; i++) list.appendChild(document.createElement('li'));
         list.innerHTML = '';",
        "fine.js",
    )
    .unwrap();
    js.run_gc();
    // Freed nodes make room again.
    js.run_script(
        "for (let i = 0; i < 1000; i++) list.appendChild(document.createElement('li'));",
        "again.js",
    )
    .unwrap();
    assert_eq!(js.limit_exceeded(), None);
}

#[test]
fn every_document_of_the_page_gets_the_limit() {
    let js = with_nodes(100);
    js.set_document(Document::parse(&"<p>x</p>".repeat(200)));
    assert_eq!(
        js.limit_exceeded(),
        Some(ResourceLimit::DomNodes { limit: 100 })
    );
    assert!(js.eval_json("1").is_err());
}

#[test]
fn a_catastrophic_regexp_is_stopped_by_the_deadline() {
    // Each takes far longer than a second to fail; the matcher's own loops
    // and dasher's (see vendor/rquickjs-sys/PATCHES.md) must still let the
    // deadline through.
    let patterns = [
        // nested repeats, through the one-character loop and the back-off
        "/(a+)+$/.test(s)",
        "/(?:a|aa)+$/.test(s)",
        "/^(\\w+\\s?)*$/.test(s + ' !')",
        // a required character that is there, and runs of a leading repeat
        "/[a-z]+@(?:a|aa)+$/.test('x@' + s)",
        "(s + 'b!').replace(/(a*)*b$/g, '')",
        "s.replace(/a/g, 'x').split(/(x+x+)+y/)",
        // a two-character start, with the `i` flag
        "/(?:aa|ab)(?:a|aa)+$/i.test(s)",
    ];
    for pattern in patterns {
        let js = page(RuntimeOptions::default());
        js.run_script("var s = 'a'.repeat(40) + '!';", "a.js")
            .unwrap();
        js.set_deadline(Some(
            std::time::Instant::now() + std::time::Duration::from_millis(50),
        ));
        let started = std::time::Instant::now();
        let error = js.run_script(pattern, "b.js").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Interrupted, "{pattern}: {error:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{pattern} ran {:?} past its deadline",
            started.elapsed()
        );
    }
}
