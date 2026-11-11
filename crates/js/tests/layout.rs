//! Synthetic layout: element boxes, scrolling, scrollIntoView and
//! ResizeObserver, in both layout modes.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, LayoutMode, RuntimeOptions, Viewport, WindowOptions};

const LIST: &str = "<!doctype html><body><main id=main>\
    <p id=a>a</p><p id=b>b</p><p id=c>c</p><p id=hidden hidden>h</p>\
    <div id=filler></div><p id=last>z</p></main></body>";

fn page(layout: LayoutMode, html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/".into(),
            viewport: Viewport {
                width: 800,
                height: 100,
                ..Viewport::default()
            },
            layout,
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
    js.run_ready().unwrap();
}

/// Fills `#filler` with `n` empty divs, 20px each in the stacked layout.
fn fill(js: &JsRuntime, n: usize) {
    run(
        js,
        &format!(
            "{{ const f = document.getElementById('filler');
               for (let i = 0; i < {n}; i++) f.append(document.createElement('div')); }}"
        ),
    );
}

const RECT: &str = "(id) => { const r = document.getElementById(id).getBoundingClientRect(); \
                    return [r.top, r.height]; }";

#[test]
fn in_view_boxes_stay_at_the_top() {
    let js = page(LayoutMode::InView, LIST);
    fill(&js, 20);
    run(&js, &format!("globalThis.rect = {RECT}; scrollTo(0, 300);"));
    assert_eq!(
        eval(
            &js,
            "[rect('a'), rect('last'), rect('hidden'), document.getElementById('c').offsetHeight,
              scrollY, document.documentElement.scrollHeight]"
        ),
        // Every element body * counts a line: main, 4 p's, filler, 20 divs, last.
        "[[0,20],[0,20],[0,0],20,300,540]"
    );
    // No limit on scrolling.
    run(&js, "scrollTo(0, 99999)");
    assert_eq!(eval(&js, "scrollY"), "99999");
}

#[test]
fn scrolled_layout_stacks_elements_and_moves_them_with_the_scroll() {
    let js = page(LayoutMode::Scrolled, LIST);
    fill(&js, 20);
    run(&js, &format!("globalThis.rect = {RECT};"));
    // body is line 0, main line 1 spanning everything inside it.
    assert_eq!(
        eval(
            &js,
            "[rect('main'), rect('a'), rect('c'), rect('hidden'), rect('filler'), rect('last'),
              document.getElementById('last').offsetTop, document.documentElement.scrollHeight]"
        ),
        "[[20,540],[40,20],[80,20],[0,0],[120,420],[540,20],540,560]"
    );
    run(&js, "scrollTo(0, 100)");
    assert_eq!(eval(&js, "[scrollY, rect('a')[0]]"), "[100,-60]");
    // Scrolling stops at the bottom: 560 tall, 100 in view.
    run(&js, "scrollTo(0, 99999)");
    assert_eq!(eval(&js, "[scrollY, rect('last')[0]]"), "[460,80]");
}

#[test]
fn scroll_into_view_moves_the_window_and_fires_scroll() {
    for layout in [LayoutMode::InView, LayoutMode::Scrolled] {
        let js = page(layout, LIST);
        fill(&js, 20);
        run(
            &js,
            "globalThis.events = 0; addEventListener('scroll', () => events++);
             document.getElementById('last').scrollIntoView();",
        );
        // The scrolled layout stops at the bottom: 560 tall, 100 in view.
        let bottom = if layout == LayoutMode::Scrolled {
            460
        } else {
            540
        };
        assert_eq!(
            eval(&js, "[scrollY, events]"),
            format!("[{bottom},1]"),
            "{layout:?}"
        );
        run(
            &js,
            "document.getElementById('c').scrollIntoView({ block: 'end' });
             globalThis.end = scrollY;
             document.getElementById('c').scrollIntoView({ block: 'center' });
             globalThis.center = scrollY;
             document.getElementById('hidden').scrollIntoView();",
        );
        assert_eq!(eval(&js, "[end, center, scrollY, events]"), "[0,40,40,2]");
    }
}

#[test]
fn scroll_into_view_if_needed() {
    let js = page(LayoutMode::Scrolled, LIST);
    fill(&js, 20);
    run(&js, "document.getElementById('b').scrollIntoViewIfNeeded()");
    assert_eq!(eval(&js, "scrollY"), "0");
    run(&js, "document.getElementById('c').scrollIntoViewIfNeeded()");
    assert_eq!(eval(&js, "scrollY"), "0");
    // Centred, but no further than the bottom.
    run(
        &js,
        "document.getElementById('a').append(...Array.from({ length: 10 }, () => document.createElement('i')))",
    );
    run(&js, "document.getElementById('a').scrollIntoViewIfNeeded()");
    assert_eq!(eval(&js, "scrollY"), "100");
    run(
        &js,
        "document.getElementById('last').scrollIntoViewIfNeeded(false)",
    );
    assert_eq!(eval(&js, "scrollY"), "660");
}

#[test]
fn resize_observer_reports_the_synthetic_size_once() {
    let js = page(LayoutMode::Scrolled, LIST);
    run(
        &js,
        "globalThis.sizes = [];
         const ro = new ResizeObserver((entries) => {
           for (const e of entries) sizes.push([e.target.id, e.contentRect.width, e.contentRect.height]);
         });
         ro.observe(document.getElementById('main'));
         ro.observe(document.getElementById('a'));",
    );
    run(
        &js,
        "document.getElementById('a').append(document.createElement('span'))",
    );
    assert_eq!(eval(&js, "sizes"), r#"[["main",800,140],["a",800,20]]"#);
}
