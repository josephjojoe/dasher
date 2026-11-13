//! Synthetic layout: element boxes, scrolling, scrollIntoView,
//! IntersectionObserver and ResizeObserver, under each visibility policy.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions, Viewport, Visibility, WindowOptions};

const LIST: &str = "<!doctype html><body><main id=main>\
    <p id=a>a</p><p id=b>b</p><p id=c>c</p><p id=hidden hidden>h</p>\
    <div id=filler></div><p id=last>z</p></main></body>";

fn page(layout: Visibility, html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/".into(),
            viewport: Viewport {
                width: 800,
                height: 100,
                ..Viewport::default()
            },
            visibility: layout,
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
    let js = page(Visibility::All, LIST);
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
fn loaded_keeps_what_was_there_in_view_and_appended_content_below() {
    let js = page(Visibility::Loaded, LIST);
    // Before the first reveal it is `All`.
    fill(&js, 20);
    run(&js, &format!("globalThis.rect = {RECT};"));
    assert_eq!(
        eval(
            &js,
            "[rect('a'), rect('last'), document.documentElement.scrollHeight]"
        ),
        "[[0,20],[0,20],200]"
    );
    // After it, twenty more divs in #filler take a line each and push
    // what follows them down; what came before stays in view.
    js.reveal();
    fill(&js, 20);
    assert_eq!(
        eval(
            &js,
            "[rect('a'), rect('filler'), rect('last'), document.getElementById('last').offsetTop,
              rect('hidden'), document.documentElement.scrollHeight]"
        ),
        // Two viewports of 100px, plus the 400px of new divs.
        "[[0,20],[0,400],[400,20],400,[0,0],600]"
    );
    // Boxes stay put when the page scrolls itself, which stops at the
    // bottom.
    run(&js, "scrollTo(0, 99999)");
    assert_eq!(eval(&js, "[scrollY, rect('last')[0]]"), "[500,400]");
    // The next reveal brings it all back to the top.
    js.reveal();
    assert_eq!(
        eval(&js, "[rect('last'), document.documentElement.scrollHeight]"),
        "[[0,20],200]"
    );
}

#[test]
fn loaded_intersection_follows_what_was_appended() {
    let js = page(Visibility::Loaded, LIST);
    run(
        &js,
        "globalThis.seen = [];
         new IntersectionObserver((entries) => {
           for (const e of entries) seen.push(e.target.id + ':' + e.isIntersecting);
         }).observe(document.getElementById('last'));",
    );
    assert_eq!(eval(&js, "seen"), r#"["last:true"]"#);
    js.reveal();
    // Four lines of new content still leave it in the 100px viewport; six
    // push it out.
    fill(&js, 4);
    assert_eq!(eval(&js, "seen"), r#"["last:true"]"#);
    fill(&js, 2);
    assert_eq!(eval(&js, "seen"), r#"["last:true","last:false"]"#);
    // An element appended after the reveal is out of view too.
    run(
        &js,
        "{ const late = document.createElement('p'); late.id = 'late';
           document.body.append(late);
           new IntersectionObserver((entries) => seen.push('late:' + entries[0].isIntersecting))
             .observe(late); }",
    );
    assert_eq!(eval(&js, "seen.at(-1)"), r#""late:false""#);
    // Revealing counts all of it as seen.
    js.reveal();
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "seen.slice(-2).sort()"),
        r#"["last:true","late:true"]"#
    );
}

#[test]
fn scrolled_layout_stacks_elements_and_moves_them_with_the_scroll() {
    let js = page(Visibility::Viewport, LIST);
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
    for layout in [Visibility::All, Visibility::Viewport] {
        let js = page(layout, LIST);
        fill(&js, 20);
        run(
            &js,
            "globalThis.events = 0; addEventListener('scroll', () => events++);
             document.getElementById('last').scrollIntoView();",
        );
        // The scrolled layout stops at the bottom: 560 tall, 100 in view.
        let bottom = if layout == Visibility::Viewport {
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
    let js = page(Visibility::Viewport, LIST);
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

/// Observes the ids with one observer and logs `id:isIntersecting:ratio`.
const OBSERVE: &str = "globalThis.seen = [];
    globalThis.observe = (ids, options) => {
      const io = new IntersectionObserver((entries, observer) => {
        if (observer !== io) throw new Error('wrong observer');
        for (const e of entries) {
          seen.push(e.target.id + ':' + e.isIntersecting + ':' + e.intersectionRatio);
        }
      }, options);
      for (const id of ids) io.observe(document.getElementById(id));
      return io;
    };
    globalThis.take = () => seen.splice(0).join(' ');";

#[test]
fn in_view_targets_intersect_at_once() {
    let js = page(Visibility::All, LIST);
    fill(&js, 50);
    run(
        &js,
        &format!("{OBSERVE} globalThis.io = observe(['a', 'last', 'hidden']);"),
    );
    assert_eq!(
        eval(&js, "take()"),
        r#""a:true:1 last:true:1 hidden:false:0""#
    );
    // Nothing changes on scrolling.
    run(&js, "scrollTo(0, 500)");
    assert_eq!(eval(&js, "take()"), r#""""#);
    // Showing or removing a target does.
    run(
        &js,
        "document.getElementById('hidden').hidden = false; document.getElementById('a').remove();",
    );
    assert_eq!(eval(&js, "take()"), r#""a:false:0 hidden:true:1""#);
    // Observing twice does nothing; unobserve and disconnect stop reports.
    run(
        &js,
        "io.observe(document.getElementById('last'));
         io.unobserve(document.getElementById('hidden'));
         document.getElementById('hidden').hidden = true;",
    );
    assert_eq!(eval(&js, "take()"), r#""""#);
    run(
        &js,
        "io.disconnect(); document.getElementById('last').remove();",
    );
    assert_eq!(eval(&js, "take()"), r#""""#);
}

#[test]
fn in_view_target_pushed_down_leaves_until_the_next_scroll() {
    let js = page(Visibility::All, LIST);
    run(
        &js,
        &format!("{OBSERVE} observe(['last']); observe(['a']);"),
    );
    assert_eq!(eval(&js, "take()"), r#""last:true:1 a:true:1""#);
    // New items before the sentinel push it down; 'a' stays put.
    fill(&js, 10);
    assert_eq!(eval(&js, "take()"), r#""last:false:0""#);
    run(&js, "scrollTo(0, document.documentElement.scrollHeight)");
    assert_eq!(eval(&js, "take()"), r#""last:true:1""#);
    // Moving up does not count as leaving.
    run(&js, "document.getElementById('filler').replaceChildren()");
    assert_eq!(eval(&js, "take()"), r#""""#);
}

#[test]
fn scrolled_targets_intersect_once_scrolled_to() {
    let js = page(Visibility::Viewport, LIST);
    fill(&js, 20);
    // The viewport is 100px: a (40..60) is in view, last (540..560) is not.
    run(
        &js,
        &format!("{OBSERVE} globalThis.io = observe(['a', 'last'], {{ threshold: [0, 0.5, 1] }});"),
    );
    assert_eq!(eval(&js, "take()"), r#""a:true:1 last:false:0""#);
    // Half of last in view: crossing 0.5.
    run(&js, "scrollTo(0, 450)");
    assert_eq!(eval(&js, "take()"), r#""a:false:0 last:true:0.5""#);
    run(&js, "scrollTo(0, 460)");
    assert_eq!(eval(&js, "take()"), r#""last:true:1""#);
    // Content inserted before it pushes it out of view.
    fill(&js, 10);
    assert_eq!(eval(&js, "take()"), r#""last:false:0""#);
}

#[test]
fn root_margin_grows_the_viewport() {
    let js = page(Visibility::Viewport, LIST);
    fill(&js, 20);
    run(
        &js,
        &format!(
            "{OBSERVE} globalThis.io = observe(['last'], {{ rootMargin: '0px 0px 450px 0px' }});"
        ),
    );
    assert_eq!(eval(&js, "take()"), r#""last:true:0.5""#);
    assert_eq!(
        eval(&js, "[io.rootMargin, io.thresholds, io.root]"),
        r#"["0px 0px 450px 0px",[0],null]"#
    );
}

#[test]
fn entries_describe_the_geometry() {
    let js = page(Visibility::Viewport, LIST);
    run(
        &js,
        "new IntersectionObserver(([e]) => {
           globalThis.entry = {
             bounds: e.boundingClientRect.toJSON(), hit: e.intersectionRect.height,
             root: [e.rootBounds.width, e.rootBounds.height], time: typeof e.time,
             visible: e.isVisible, tag: Object.prototype.toString.call(e),
           };
         }).observe(document.getElementById('b'));",
    );
    assert_eq!(
        eval(&js, "entry"),
        r#"{"bounds":{"x":0,"y":60,"width":800,"height":20,"top":60,"right":800,"bottom":80,"left":0},"hit":20,"root":[800,100],"time":"number","visible":false,"tag":"[object IntersectionObserverEntry]"}"#
    );
}

#[test]
fn element_roots_contain_their_descendants() {
    let js = page(Visibility::Viewport, LIST);
    fill(&js, 20);
    run(
        &js,
        &format!(
            "{OBSERVE} observe(['last', 'main'], {{ root: document.getElementById('filler') }});
             observe(['last'], {{ root: document.getElementById('main') }});"
        ),
    );
    assert_eq!(
        eval(&js, "take()"),
        r#""last:false:0 main:false:0 last:true:1""#
    );
}

#[test]
fn bad_options_throw() {
    let js = page(Visibility::All, LIST);
    assert_eq!(
        eval(
            &js,
            "const tries = [
               () => new IntersectionObserver(),
               () => new IntersectionObserver(() => {}, { threshold: 2 }),
               () => new IntersectionObserver(() => {}, { rootMargin: '10em' }),
               () => new IntersectionObserver(() => {}, { root: {} }),
               () => new IntersectionObserver(() => {}).observe(document),
             ];
             tries.map((f) => { try { f(); return 'ok'; } catch (e) { return e.name; } })"
        ),
        r#"["TypeError","RangeError","SyntaxError","TypeError","TypeError"]"#
    );
    assert_eq!(
        eval(
            &js,
            "const io = new IntersectionObserver(() => {}, { rootMargin: '5px 10%', threshold: [1, 0.25] });
             [io.rootMargin, io.thresholds, io.takeRecords().length]"
        ),
        r#"["5px 10% 5px 10%",[0.25,1],0]"#
    );
}

#[test]
fn observing_keeps_the_loop_busy_only_while_something_changes() {
    let js = page(Visibility::All, LIST);
    run(
        &js,
        "globalThis.calls = 0;
         new IntersectionObserver(() => calls++).observe(document.getElementById('a'));",
    );
    assert!(js.is_idle());
    run(&js, "document.getElementById('b').remove()");
    assert!(js.is_idle());
    assert_eq!(eval(&js, "calls"), "1");
}

#[test]
fn resize_observer_reports_the_synthetic_size_once() {
    let js = page(Visibility::Viewport, LIST);
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
