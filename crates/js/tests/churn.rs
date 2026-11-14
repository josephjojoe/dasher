//! DOM churn: scripts that replace the same few nodes over and over, as a
//! ticker rewriting its text or an app re-rendering a list does, keep the
//! document's node count flat. Removed nodes go back to the arena as soon
//! as script can no longer reach them, whatever else is going on: a
//! mutation observer watching, custom elements reacting, or the parser
//! still feeding the document.
//!
//! Each runtime gets a node limit far below the number of nodes the churn
//! creates, so any per-iteration leak stops the page.

use dasher_dom::{Document, DocumentParser, ParseOptions, ParseStep};
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

/// Each runtime's node limit: a hundredth or less of what the churn
/// creates.
const LIMIT: usize = 10_000;

/// How many times the churn `name` runs: a million, or in debug builds,
/// where a million of each takes minutes, a million `textContent`
/// assignments and 50,000 of the rest (still 15 times the limit in nodes).
fn times(name: &str) -> u32 {
    if cfg!(debug_assertions) && name != "textContent" {
        50_000
    } else {
        1_000_000
    }
}
/// How many arena slots the churn may use at most. Garbage with wrappers
/// waits for the cycle collector, which the DOM runs once a few thousand
/// nodes have piled up (see `next_gc` in `src/dom.rs`); garbage without
/// goes at the next change.
const PEAK: usize = 6_000;
/// How many nodes besides the document's tree may be left over once the
/// collector has run (the last iteration's garbage, say).
const SLACK: usize = 32;

/// Ways of replacing an element's contents, as statements using `el`, `i`
/// and `T`, the tag name of the elements made.
const CHURNS: &[(&str, &str)] = &[
    ("textContent", "el.textContent = 'tick ' + i;"),
    (
        "innerHTML",
        "el.innerHTML = '<' + T + '>' + i + '</' + T + '><' + T + '><b>x</b></' + T + '>';",
    ),
    (
        "replaceChildren",
        "el.replaceChildren(document.createElement(T), 'n' + i);",
    ),
    (
        "append then remove",
        "const d = document.createElement('div');
         d.innerHTML = '<p><' + T + '>' + i + '</' + T + '></p><p></p>';
         el.append(d);
         d.remove();",
    ),
    (
        "moved out of a template",
        "const t = document.createElement('template');
         t.innerHTML = '<' + T + '><b>' + i + '</b></' + T + '>';
         el.replaceChildren(t.content.firstChild);",
    ),
];

fn runtime(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        max_dom_nodes: Some(LIMIT),
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn run(js: &JsRuntime, source: &str, what: &str) {
    js.run_script(source, "churn.js")
        .unwrap_or_else(|e| panic!("{what}: {e}"));
    let errors = js.take_errors();
    assert!(errors.is_empty(), "{what}: {errors:?}");
}

/// A loop running the churn `name` [`times`] times on `el`.
fn churn_loop(name: &str, churn: &str) -> String {
    let times = times(name);
    format!("for (let i = 0; i < {times}; i++) {{ {churn} }}")
}

/// Checks that the arena never grew past [`PEAK`] slots, and that once
/// garbage wrappers are collected the document holds little more than its
/// tree.
fn assert_flat(js: &JsRuntime, what: &str) {
    assert_eq!(js.limit_exceeded(), None, "{what}");
    let capacity = js.with_document(Document::capacity);
    assert!(
        capacity <= PEAK,
        "{what}: the arena grew to {capacity} slots"
    );
    js.run_gc();
    js.with_document(|doc| {
        let tree = doc.descendants(doc.root()).count() + 1;
        assert!(
            doc.len() <= tree + SLACK,
            "{what}: {} nodes for a tree of {tree}",
            doc.len()
        );
    });
}

#[test]
fn churn_stays_flat() {
    for &(name, churn) in CHURNS {
        let js = runtime("<!doctype html><body><ul id=list></ul>");
        run(
            &js,
            &format!(
                "const T = 'li'; const el = document.getElementById('list'); {}",
                churn_loop(name, churn)
            ),
            name,
        );
        assert_flat(&js, name);
    }
}

#[test]
fn churn_stays_flat_while_observed() {
    for &(name, churn) in CHURNS {
        let js = runtime("<!doctype html><body><ul id=list></ul>");
        // The records name every node removed, so those live until the
        // observer has had them: let it, every 250 changes, as a page that
        // churns over many tasks does.
        let times = times(name);
        run(
            &js,
            &format!(
                "const T = 'li';
                 const el = document.getElementById('list');
                 globalThis.seen = 0;
                 new MutationObserver((records) => {{ seen += records.length; }})
                   .observe(document, {{ childList: true, subtree: true, characterData: true }});
                 globalThis.done = (async () => {{
                   for (let i = 0; i < {times}; i++) {{
                     {{ {churn} }}
                     if (i % 250 === 249) await null;
                   }}
                   return true;
                 }})();
                 done.then((d) => {{ globalThis.done = d; }});"
            ),
            name,
        );
        js.perform_microtask_checkpoint();
        assert_eq!(
            js.eval_json("done").unwrap().as_deref(),
            Some("true"),
            "{name}"
        );
        let seen: u64 = js.eval_json("seen").unwrap().unwrap().parse().unwrap();
        assert!(seen >= u64::from(times), "{name}: {seen} records");
        assert_flat(&js, name);
    }
}

#[test]
fn churn_stays_flat_in_custom_elements() {
    for &(name, churn) in CHURNS {
        let js = runtime("<!doctype html><body><x-list id=list></x-list>");
        // The list is a custom element, and so is everything put in it.
        run(
            &js,
            &format!(
                "globalThis.connected = 0; globalThis.disconnected = 0;
                 customElements.define('x-list', class extends HTMLElement {{}});
                 customElements.define('x-item', class extends HTMLElement {{
                   connectedCallback() {{ connected++; }}
                   disconnectedCallback() {{ disconnected++; }}
                 }});
                 const T = 'x-item';
                 const el = document.getElementById('list');
                 {}",
                churn_loop(name, churn)
            ),
            name,
        );
        if name != "textContent" {
            let connected: u64 = js.eval_json("connected").unwrap().unwrap().parse().unwrap();
            assert!(
                connected >= u64::from(times(name)),
                "{name}: {connected} connected"
            );
        }
        assert_flat(&js, name);
    }
}

#[test]
fn churn_stays_flat_while_parsing() {
    for &(name, churn) in CHURNS {
        let js = runtime("");
        js.set_document(Document::new());
        let mut parser =
            js.with_document_mut(|doc| DocumentParser::new(doc, ParseOptions::default()));
        // The script runs with the body and #outer still open.
        parser.push(&format!(
            "<!doctype html><body><div id=outer><ul id=list></ul><script>
               const T = 'li'; const el = document.getElementById('list'); {}
             </script><p>after</p>",
            churn_loop(name, churn)
        ));
        let step = js.with_parser(&mut parser, |p, doc| p.resume(doc));
        let ParseStep::Script(script) = step else {
            panic!("{name}: expected the script, got {step:?}");
        };
        let source = js.with_document(|doc| doc.text_content(script).unwrap());
        js.run_script_element(script, &source, "https://example.com/")
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_flat(&js, name);

        parser.push("</div><p>end");
        assert_eq!(
            js.with_parser(&mut parser, |p, doc| p.resume(doc)),
            ParseStep::NeedInput
        );
        js.with_parser(&mut parser, |p, doc| p.finish(doc));
        assert_eq!(
            js.eval_json("[...document.body.children].map(e => e.id || e.textContent).join()")
                .unwrap()
                .as_deref(),
            Some("\"outer,end\""),
            "{name}"
        );
        assert_eq!(
            js.eval_json("document.querySelector('#outer > p').textContent")
                .unwrap()
                .as_deref(),
            Some("\"after\""),
            "{name}"
        );
        assert_flat(&js, name);
    }
}

/// What the parser still builds into stays, even when script removes it
/// and drops it; it goes once the parser is done with it.
#[test]
fn nodes_the_parser_builds_into_survive_removal() {
    let js = runtime("");
    js.set_document(Document::new());
    let mut parser = js.with_document_mut(|doc| DocumentParser::new(doc, ParseOptions::default()));
    parser.push(
        "<!doctype html><body><div id=outer><b><i>open</i></b><section><script>
           document.getElementById('outer').remove();
           // Churn, so garbage is swept.
           {
             const t = document.body.appendChild(document.createElement('span'));
             for (let i = 0; i < 1000; i++) t.textContent = i;
             t.remove();
           }
         </script>",
    );
    let ParseStep::Script(script) = js.with_parser(&mut parser, |p, doc| p.resume(doc)) else {
        panic!("expected the script");
    };
    let source = js.with_document(|doc| doc.text_content(script).unwrap());
    js.run_script_element(script, &source, "https://example.com/")
        .unwrap();
    js.run_gc();
    let tree = js.with_document(|doc| doc.descendants(doc.root()).count() + 1);
    // #outer's tree, out of the document: div, b, i, text, section, script,
    // the script's text.
    assert_eq!(js.with_document(Document::len), tree + 7);

    // The parser goes on building into the removed #outer: the misnested
    // </b> reopens it from the list of active formatting elements.
    parser.push("<p>in section</p></b>still<em>in outer</em></div><p id=end>end");
    assert_eq!(
        js.with_parser(&mut parser, |p, doc| p.resume(doc)),
        ParseStep::NeedInput
    );
    js.with_parser(&mut parser, |p, doc| p.finish(doc));
    assert_eq!(
        js.eval_json("document.body.innerHTML").unwrap().as_deref(),
        Some("\"<p id=\\\"end\\\">end</p>\"")
    );
    // Once the parser is done with it, nothing refers to #outer's tree.
    js.run_script("document.body.append('x')", "touch.js")
        .unwrap();
    let tree = js.with_document(|doc| doc.descendants(doc.root()).count() + 1);
    assert_eq!(js.with_document(Document::len), tree);
}

/// A `<template shadowrootmode>` becomes a shadow root and is never
/// inserted: it is the parser's own, to free when parsing ends. One the
/// input pauses inside, as a page arriving in chunks does, is not garbage
/// in between.
#[test]
fn declarative_shadow_templates_survive_a_pause() {
    let js = runtime("");
    js.set_document(Document::new());
    let mut parser = js.with_document_mut(|doc| DocumentParser::new(doc, ParseOptions::default()));
    // The input runs out with the template open.
    parser.push("<!doctype html><body><my-card><template shadowrootmode=open><p>in the");
    assert_eq!(
        js.with_parser(&mut parser, |p, doc| p.resume(doc)),
        ParseStep::NeedInput
    );
    parser.push(
        " shadow</p></template>light</my-card><script>
           // Churn, so garbage is swept.
           {
             const t = document.body.appendChild(document.createElement('span'));
             for (let i = 0; i < 1000; i++) t.textContent = i;
             t.remove();
           }
         </script>",
    );
    let ParseStep::Script(script) = js.with_parser(&mut parser, |p, doc| p.resume(doc)) else {
        panic!("expected the script");
    };
    let source = js.with_document(|doc| doc.text_content(script).unwrap());
    js.run_script_element(script, &source, "https://example.com/")
        .unwrap();
    parser.push("<p>end");
    assert_eq!(
        js.with_parser(&mut parser, |p, doc| p.resume(doc)),
        ParseStep::NeedInput
    );
    js.with_parser(&mut parser, |p, doc| p.finish(doc));
    assert_eq!(
        js.eval_json("document.querySelector('my-card').shadowRoot.innerHTML")
            .unwrap()
            .as_deref(),
        Some("\"<p>in the shadow</p>\"")
    );
    // The template is gone: what is left is the document's tree and the
    // shadow tree (its root, the p and its text).
    js.run_script("document.body.append('x')", "touch.js")
        .unwrap();
    let tree = js.with_document(|doc| doc.descendants(doc.root()).count() + 1);
    assert_eq!(js.with_document(Document::len), tree + 3);
}

/// A node moved or removed out of a detached tree may have held its last
/// wrapper; the tree it leaves goes too.
#[test]
fn trees_left_behind_are_freed() {
    let js = runtime("<!doctype html><body><ul id=list></ul>");
    let empty = js.with_document(Document::len);
    // Rows made in templates, keeping only the row: the templates' own
    // wrappers are collected before the rows move.
    run(
        &js,
        "globalThis.row = (i) => {
           const t = document.createElement('template');
           t.innerHTML = '<li>' + i + '</li><li>spare</li>';
           return t.content.firstChild;
         };
         globalThis.rows = [];
         for (let i = 0; i < 100; i++) rows.push(row(i));",
        "make rows",
    );
    js.run_gc();
    run(
        &js,
        "document.getElementById('list').append(...rows);",
        "move rows",
    );
    // And the same for removal: keep only an inner node of a detached tree.
    run(
        &js,
        "globalThis.inner = [];
         for (let i = 0; i < 100; i++) {
           const d = document.createElement('div');
           d.innerHTML = '<p><i>' + i + '</i></p>';
           inner.push(d.firstChild.firstChild);
         }",
        "make trees",
    );
    js.run_gc();
    run(
        &js,
        "for (const i of inner) i.remove();
         rows = []; inner = [];
         document.getElementById('list').textContent = '';",
        "take out",
    );
    js.run_gc();
    run(&js, "document.body.append('x');", "touch");
    assert_eq!(js.with_document(Document::len), empty + 1);
}

/// Detached nodes script still refers to survive churn: kept wrappers and
/// records not yet delivered.
#[test]
fn referenced_nodes_survive_churn() {
    let js = runtime("<!doctype html><body><ul id=list><li id=first>one</li></ul>");
    run(
        &js,
        "const el = document.getElementById('list');
         globalThis.kept = el.firstChild;
         kept.title = 'kept';
         el.textContent = 'ranged';
         globalThis.ranged = el.firstChild;
         globalThis.records = [];
         const mo = new MutationObserver(() => {});
         mo.observe(el, { childList: true });
         el.textContent = 'recorded';
         for (let i = 0; i < 20000; i++) document.body.textContent = i;
         records = mo.takeRecords();",
        "churn",
    );
    js.run_gc();
    run(&js, "document.body.append('x');", "touch");
    assert_eq!(
        js.eval_json(
            "[kept.id, kept.title, kept.textContent, ranged.data, kept.isConnected,
              records.length, records[0].removedNodes[0] === ranged,
              records[0].addedNodes[0].data]"
        )
        .unwrap()
        .as_deref(),
        Some(r#"["first","kept","one","ranged",false,1,true,"recorded"]"#)
    );
    assert_flat(&js, "after churn");
}
