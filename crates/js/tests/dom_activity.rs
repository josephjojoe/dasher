//! DOM mutation activity, which the page layer reads to tell when a page
//! has stopped changing.

use std::time::Duration;

use dasher_dom::Document;
use dasher_js::{ClockMode, DomActivity, JsRuntime, RuntimeOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
}

fn mutations(js: &JsRuntime) -> u64 {
    js.dom_activity().mutations
}

#[test]
fn counts_changes_to_the_document() {
    let js = page("<div id=a>text</div>");
    assert_eq!(js.dom_activity(), DomActivity::default());

    run(
        &js,
        "document.getElementById('a').append(document.createElement('p'));",
    );
    assert_eq!(
        js.dom_activity(),
        DomActivity {
            mutations: 1,
            last_mutation: Some(Duration::ZERO),
        }
    );
    run(
        &js,
        "const a = document.getElementById('a');
         a.className = 'x';
         a.removeAttribute('class');
         a.firstChild.data = 'more';
         a.innerHTML = '<i>1</i><b>2</b>';
         a.replaceChildren();",
    );
    assert_eq!(mutations(&js), 6);

    // A subtree built while detached counts once, when it is inserted.
    run(
        &js,
        "const d = document.createElement('div');
         for (let i = 0; i < 10; i++) { const li = document.createElement('li'); li.id = 'i' + i; li.textContent = i; d.append(li); }
         document.body.append(d);",
    );
    assert_eq!(mutations(&js), 7);

    // Nothing that changes nothing counts, and neither does the host.
    run(
        &js,
        "document.body.append(document.createDocumentFragment()); document.body.removeAttribute('x');",
    );
    js.with_document_mut(|doc| {
        let body = doc.body().unwrap();
        doc.set_attribute(body, "data-host", "1");
    });
    assert_eq!(mutations(&js), 7);
}

#[test]
fn records_when_the_last_change_happened() {
    let js = page("<ul id=list></ul>");
    run(
        &js,
        "let n = 0;
         const timer = setInterval(() => {
           document.getElementById('list').append(document.createElement('li'));
           if (++n === 3) clearInterval(timer);
         }, 250);",
    );
    js.advance(Duration::from_secs(2)).unwrap();
    assert_eq!(
        js.dom_activity(),
        DomActivity {
            mutations: 3,
            last_mutation: Some(Duration::from_millis(750)),
        }
    );
    assert_eq!(js.now(), Duration::from_secs(2));
}
