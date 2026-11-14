//! The hooks the page loader drives the JS side with.

use dasher_dom::{Document, Name};
use dasher_js::{
    ClockMode, DocumentInfo, DocumentWrite, EventTarget, JsRuntime, ReadyState, RuntimeOptions,
    WindowOptions,
};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/page".into(),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn json(js: &JsRuntime, expr: &str) -> String {
    js.eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn script_ids(js: &JsRuntime) -> Vec<dasher_dom::NodeId> {
    js.with_document(|doc| {
        doc.descendants(doc.root())
            .filter(|&n| doc.is_html(n, Name::SCRIPT))
            .collect()
    })
}

#[test]
fn ready_state_changes_fire_readystatechange() {
    let js = page("");
    assert_eq!(js.ready_state(), ReadyState::Complete);
    js.run_script(
        "globalThis.log = [];
         document.addEventListener('readystatechange', (e) =>
           log.push(document.readyState + ':' + e.isTrusted));",
        "test.js",
    )
    .unwrap();
    js.set_ready_state(ReadyState::Loading);
    js.set_ready_state(ReadyState::Loading);
    js.set_ready_state(ReadyState::Interactive);
    js.set_ready_state(ReadyState::Complete);
    assert_eq!(js.ready_state(), ReadyState::Complete);
    assert_eq!(
        json(&js, "log"),
        r#"["loading:true","interactive:true","complete:true"]"#
    );
}

/// `performance.timing` (Navigation Timing Level 1) reports when the
/// document reached each stage, as epoch milliseconds, and 0 for a stage
/// not reached yet.
#[test]
fn performance_timing_follows_the_lifecycle() {
    use std::time::Duration;
    let js = page("");
    let stages = "['domInteractive', 'domContentLoadedEventStart', 'domContentLoadedEventEnd',
                   'domComplete', 'loadEventStart', 'loadEventEnd']
                    .map(k => performance.timing[k] && performance.timing[k] - performance.timing.navigationStart)";
    assert_eq!(
        json(
            &js,
            "performance.timing.navigationStart === Math.floor(performance.timeOrigin)"
        ),
        "true"
    );
    js.set_ready_state(ReadyState::Loading);
    assert_eq!(json(&js, stages), "[0,0,0,0,0,0]");
    assert_eq!(
        json(
            &js,
            "['fetchStart', 'domainLookupStart', 'domainLookupEnd', 'connectStart', 'connectEnd',
              'requestStart', 'responseStart', 'responseEnd', 'domLoading']
               .every(k => performance.timing[k] === performance.timing.navigationStart)"
        ),
        "true"
    );
    assert_eq!(
        json(
            &js,
            "['unloadEventStart', 'unloadEventEnd', 'redirectStart', 'redirectEnd']
               .map(k => performance.timing[k])"
        ),
        "[0,0,0,0]"
    );

    js.advance(Duration::from_millis(30)).unwrap();
    js.set_ready_state(ReadyState::Interactive);
    js.advance(Duration::from_millis(10)).unwrap();
    // The times are taken whether or not anything listens.
    js.fire_event(EventTarget::Document, "DOMContentLoaded", true, false);
    assert_eq!(json(&js, stages), "[30,40,40,0,0,0]");
    js.advance(Duration::from_millis(60)).unwrap();
    js.set_ready_state(ReadyState::Complete);
    // What a page's load handler sees: the event has started, not ended.
    js.run_script(
        "addEventListener('load', () => {
           globalThis.during = [performance.timing.loadEventStart - performance.timing.navigationStart,
                                performance.timing.loadEventEnd];
         });",
        "test.js",
    )
    .unwrap();
    js.fire_event(EventTarget::Window, "load", false, false);
    assert_eq!(json(&js, "during"), "[100,0]");
    assert_eq!(json(&js, stages), "[30,40,40,100,100,100]");
    // A load event fired at an element is not the document's.
    js.advance(Duration::from_millis(5)).unwrap();
    js.fire_event(EventTarget::Document, "load", false, false);
    assert_eq!(json(&js, stages), "[30,40,40,100,100,100]");

    // The shape scripts check.
    assert_eq!(
        json(
            &js,
            "[performance.timing === performance.timing, performance.timing instanceof PerformanceTiming,
              String(performance.timing), Object.keys(performance.timing.toJSON()).length,
              performance.toJSON().timing.domComplete === performance.timing.domComplete,
              'navigationStart' in PerformanceTiming.prototype,
              (() => { try { new PerformanceTiming(); } catch (e) { return e.message; } })()]"
        ),
        r#"[true,true,"[object PerformanceTiming]",21,true,true,"Illegal constructor"]"#
    );
    assert_eq!(
        json(
            &js,
            "[performance.navigation.type, performance.navigation.redirectCount,
              performance.navigation instanceof PerformanceNavigation, String(performance.navigation),
              PerformanceNavigation.TYPE_NAVIGATE, PerformanceNavigation.TYPE_RELOAD,
              PerformanceNavigation.TYPE_BACK_FORWARD, PerformanceNavigation.TYPE_RESERVED,
              performance.navigation.TYPE_RELOAD, JSON.stringify(performance.navigation)]"
        ),
        r#"[0,0,true,"[object PerformanceNavigation]",0,1,2,255,1,"{\"type\":0,\"redirectCount\":0}"]"#
    );
}

#[test]
fn lifecycle_events_reach_the_right_targets() {
    let js = page("<p id=p>");
    js.run_script(
        "globalThis.log = [];
         const note = (where) => (e) => log.push(where + ':' + e.type + ':' + e.bubbles);
         document.addEventListener('DOMContentLoaded', note('document'));
         addEventListener('DOMContentLoaded', note('window'));
         document.addEventListener('load', note('document'));
         addEventListener('load', note('window'));
         addEventListener('pageshow', (e) => log.push('pageshow:' + e.persisted + ':' + (e instanceof PageTransitionEvent)));
         document.getElementById('p').addEventListener('error', (e) => { log.push('p:' + e.type); e.preventDefault(); });",
        "test.js",
    )
    .unwrap();
    assert!(js.fire_event(EventTarget::Document, "DOMContentLoaded", true, false));
    assert!(js.fire_event(EventTarget::Window, "load", false, false));
    assert!(js.fire_event(EventTarget::Window, "pageshow", false, false));
    let p = js.with_document(|doc| doc.get_element_by_id(doc.root(), "p").unwrap());
    assert!(!js.fire_event(EventTarget::Node(p), "error", false, true));
    assert_eq!(
        json(&js, "log"),
        r#"["document:DOMContentLoaded:true","window:DOMContentLoaded:true","window:load:false","pageshow:false:true","p:error"]"#
    );
}

#[test]
fn script_elements_run_with_current_script_set() {
    let js = page("<script id=a></script><script id=b></script>");
    let [a, b] = script_ids(&js)[..] else {
        panic!("two scripts")
    };
    js.run_script_element(
        a,
        "globalThis.seen = [document.currentScript.id];
         Promise.resolve().then(() => seen.push('micro:' + document.currentScript));",
        "https://example.com/page",
    )
    .unwrap();
    // The microtask checkpoint ran after the script, with no current script.
    assert_eq!(json(&js, "seen"), r#"["a","micro:null"]"#);
    assert_eq!(json(&js, "document.currentScript"), "null");

    let error = js
        .run_script_element(
            b,
            "seen.push(document.currentScript.id); throw new TypeError('boom');",
            "https://cdn.example.com/b.js",
        )
        .unwrap_err();
    assert_eq!(error.message, "TypeError: boom");
    assert!(
        error
            .stack
            .as_deref()
            .unwrap_or_default()
            .contains("https://cdn.example.com/b.js")
    );
    assert_eq!(js.take_errors().len(), 1);
    assert_eq!(json(&js, "seen"), r#"["a","micro:null","b"]"#);
    assert_eq!(json(&js, "document.currentScript"), "null");
}

#[test]
fn inserted_scripts_are_left_alone_without_a_loader() {
    let js = page("");
    js.run_script(
        "const s = document.createElement('script');
         s.src = '/x.js';
         document.head.append(s);
         const inline = document.createElement('script');
         inline.text = 'globalThis.ran = true';
         document.head.append(inline);",
        "test.js",
    )
    .unwrap();
    assert!(js.take_inserted_scripts().is_empty());
    assert_eq!(json(&js, "ran"), "true");
    assert!(js.is_idle());
}

#[test]
fn the_loader_takes_over_inserted_external_scripts() {
    let js = page("");
    js.take_over_inserted_scripts();
    js.run_script(
        "const add = (src, async) => {
           const s = document.createElement('script');
           s.src = src;
           if (async !== undefined) s.async = async;
           document.head.append(s);
         };
         add('/async.js');
         add('/ordered.js', false);
         const parsed = document.createElement('div');
         parsed.innerHTML = '<script src=/never.js></script>';
         document.body.append(parsed);
         const inline = document.createElement('script');
         inline.text = 'globalThis.inlineRan = document.currentScript === inline';
         document.head.append(inline);
         const module = document.createElement('script');
         module.type = 'module';
         module.text = 'x';
         document.head.append(module);",
        "test.js",
    )
    .unwrap();
    // Inline classic scripts still run as they are inserted.
    assert_eq!(json(&js, "inlineRan"), "true");
    // Scripts parsed by innerHTML never run, so they are not handed over.
    let taken = js.take_inserted_scripts();
    let summary: Vec<(String, bool)> = js.with_document(|doc| {
        taken
            .iter()
            .map(|s| {
                let src = doc.attribute(s.element, "src").unwrap_or("inline module");
                (src.to_owned(), s.non_blocking)
            })
            .collect()
    });
    assert_eq!(
        summary,
        [
            ("/async.js".to_owned(), true),
            ("/ordered.js".to_owned(), false),
            ("inline module".to_owned(), true),
        ]
    );
    // Until the loader is done with them, the loop has work in progress.
    assert!(!js.is_idle());
    drop(taken);
    assert!(js.is_idle());
    assert!(js.take_inserted_scripts().is_empty());
}

#[tokio::test]
async fn script_inserted_wakes_when_a_script_is_inserted() {
    let js = page("");
    js.take_over_inserted_scripts();
    js.run_script(
        "setTimeout(() => {
           const s = document.createElement('script');
           s.src = '/late.js';
           document.body.append(s);
         }, 10);",
        "test.js",
    )
    .unwrap();
    tokio::select! {
        _ = js.run(dasher_js::RunLimits::default()) => panic!("the loop waits for the loader"),
        () = js.script_inserted() => {}
    }
    assert_eq!(js.take_inserted_scripts().len(), 1);
}

#[test]
fn document_write_follows_the_loader_mode() {
    let js = page("<body><script id=s></script>");
    let [s] = script_ids(&js)[..] else {
        panic!("one script")
    };
    let write = "document.write('<p>a</p>', '<p>b</p>'); document.writeln('<i>c</i>');";

    js.set_document_write(DocumentWrite::Capture);
    js.run_script_element(s, write, "page").unwrap();
    assert_eq!(js.take_document_write(), "<p>a</p><p>b</p><i>c</i>\n");
    assert_eq!(js.take_document_write(), "");
    assert_eq!(json(&js, "document.querySelectorAll('p').length"), "0");

    js.set_document_write(DocumentWrite::Ignore);
    js.run_script_element(s, write, "page").unwrap();
    assert_eq!(js.take_document_write(), "");
    assert_eq!(json(&js, "document.querySelectorAll('p').length"), "0");

    // Without a parser, the markup goes in after the running script.
    js.set_document_write(DocumentWrite::Insert);
    js.run_script_element(s, "document.write('<p>a</p>', '<p>b</p>');", "page")
        .unwrap();
    assert_eq!(
        json(&js, "document.body.innerHTML"),
        r#""<script id=\"s\"></script><p>a</p><p>b</p>""#
    );
}

#[test]
fn document_info_comes_from_the_host() {
    let js = page("");
    assert_eq!(
        json(
            &js,
            "[document.characterSet, document.contentType, document.referrer]"
        ),
        r#"["UTF-8","text/html",""]"#
    );
    js.set_document_info(&DocumentInfo {
        character_set: "windows-1252".into(),
        content_type: "text/plain".into(),
        referrer: "https://example.com/from".into(),
    });
    assert_eq!(
        json(
            &js,
            "[document.characterSet, document.charset, document.inputEncoding, document.contentType, document.referrer]"
        ),
        r#"["windows-1252","windows-1252","windows-1252","text/plain","https://example.com/from"]"#
    );
}

#[test]
fn empty_inline_scripts_run_once_they_get_text() {
    let js = page("");
    js.run_script(
        "const s = document.createElement('script');
         document.head.append(s);
         s.text = 'globalThis.count = (globalThis.count || 0) + 1';
         document.body.append(s);
         document.head.append(s);",
        "test.js",
    )
    .unwrap();
    assert_eq!(json(&js, "count"), "1");
}

#[test]
fn held_nodes_survive_garbage_collection() {
    let js = page("<div id=box><p>text</p></div><div id=other><p>gone</p></div>");
    let boxed = js.with_document(|doc| doc.get_element_by_id(doc.root(), "box").unwrap());
    let before = js.with_document(Document::len);

    // Holding the `<p>` keeps its whole tree, the removed `#box`.
    let p = js.with_document(|doc| doc.first_child(boxed).unwrap());
    let hold = js.hold_node(p);
    let again = js.hold_node(p);
    js.run_script(
        "document.getElementById('box').remove(); document.getElementById('other').remove();",
        "test.js",
    )
    .unwrap();
    js.run_gc();
    js.with_document(|doc| {
        assert!(doc.is_html(boxed, Name::DIV));
        assert_eq!(doc.text_content(boxed).as_deref(), Some("text"));
        // What is not held goes as usual.
        assert_eq!(doc.len(), before - 3);
    });

    // Until the last hold goes.
    drop(hold);
    js.run_gc();
    assert_eq!(js.with_document(Document::len), before - 3);
    drop(again);
    js.run_gc();
    assert_eq!(js.with_document(Document::len), before - 6);
}
