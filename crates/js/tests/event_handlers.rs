//! Event handler content attributes: `onclick="..."` on elements and
//! `<body onload="...">` for the window.

use dasher_dom::Document;
use dasher_js::{ClockMode, EventTarget, JsRuntime, RuntimeOptions, WindowOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/page.html".into(),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js.run_script(
        "window.log = []; window.errors = [];
         addEventListener('error', (e) => errors.push(e.message));",
        "setup.js",
    )
    .unwrap();
    js
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

#[test]
fn parsed_attributes_run_with_the_element_and_the_event() {
    let js = page(
        r#"<button id=b onclick="log.push([this.id, event.type, event instanceof MouseEvent, arguments.length])">x</button>
           <svg><circle id=c onclick="log.push(this.id)"/></svg>"#,
    );
    let out = eval(
        &js,
        r#"
        const b = document.getElementById('b');
        b.click();
        document.getElementById('c').dispatchEvent(new Event('click'));
        [log, typeof b.onclick, b.onclick.name, errors]
        "#,
    );
    assert_eq!(
        out,
        r#"[[["b","click",true,1],"c"],"function","onclick",[]]"#
    );
}

#[test]
fn handlers_take_their_place_among_listeners() {
    let js = page(r#"<div id=a onclick="log.push('attr')"></div>"#);
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        // Set by the parser, so before any listener.
        a.addEventListener('click', () => log.push('listener'));
        a.click();
        // Setting the property keeps the handler's place.
        a.onclick = () => log.push('idl');
        a.click();
        // Changing the attribute does too, and replaces the function.
        a.setAttribute('onclick', "log.push('attr2')");
        a.click();
        // Removing it deactivates the handler; setting it again adds it last.
        a.removeAttribute('onclick');
        a.click();
        a.setAttribute('ONCLICK', "log.push('attr3')");
        a.click();
        // A script-made element gets its handler when the attribute is set.
        const d = document.createElement('div');
        d.addEventListener('click', () => log.push('first'));
        d.setAttribute('onclick', "log.push('second')");
        d.click();
        log
        "#,
    );
    assert_eq!(
        out,
        r#"["attr","listener","idl","listener","attr2","listener","listener","listener","attr3","first","second"]"#
    );
}

#[test]
fn the_property_overrides_the_attribute() {
    let js = page(r#"<div id=a onclick="log.push('attr')"></div>"#);
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        a.onclick = null;
        a.click();
        const nulled = a.onclick;
        a.onclick = function () { log.push(this === a); };
        a.click();
        [log, nulled, a.getAttribute('onclick')]
        "#,
    );
    assert_eq!(out, r#"[[true],null,"log.push('attr')"]"#);
}

#[test]
fn every_way_of_changing_attributes_updates_the_handler() {
    let js = page("<div id=a></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        a.toggleAttribute('onclick');
        a.click();
        a.attributes.onclick.value = "log.push('attr-node')";
        a.click();
        a.setAttributeNS(null, 'onclick', "log.push('ns')");
        a.click();
        a.removeAttributeNS(null, 'onclick');
        a.click();
        const attr = document.createAttribute('onclick');
        attr.value = "log.push('set-node')";
        a.setAttributeNode(attr);
        a.click();
        a.removeAttributeNode(attr);
        a.click();
        // Not handler attributes.
        a.setAttributeNS(null, 'onClick', "log.push('wrong case')");
        a.setAttribute('onnothing', "log.push('no such event')");
        a.click();
        a.dispatchEvent(new Event('nothing'));
        [log, errors]
        "#,
    );
    assert_eq!(out, r#"[["attr-node","ns","set-node"],[]]"#);
}

#[test]
fn the_scope_has_the_element_form_and_document() {
    let js = page(
        r#"<title>T</title>
           <form id=f method=post><input id=i value=v onclick="log.push([tagName, value, method, title, body.tagName, typeof elements, remove()])"></form>
           <p id=p onclick="log.push([tagName, typeof method, typeof remove])"></p>"#,
    );
    let out = eval(
        &js,
        r#"
        // The element comes first (`title` is the input's), then its form,
        // then the document; `remove` is unscopable.
        window.remove = () => 'global';
        document.getElementById('i').click();
        document.getElementById('p').click();
        [log, errors]
        "#,
    );
    assert_eq!(
        out,
        r#"[[["INPUT","v","post","","BODY","object","global"],["P","undefined","function"]],[]]"#
    );
}

#[test]
fn return_false_cancels() {
    let js =
        page(r#"<a id=a href=x onclick="return false"></a><a id=b onclick="return true"></a>"#);
    let out = eval(
        &js,
        r#"
        const ev = (id) => document.getElementById(id).dispatchEvent(new MouseEvent('click', { cancelable: true }));
        [ev('a'), ev('b')]
        "#,
    );
    assert_eq!(out, "[false,true]");
}

#[test]
fn errors_are_reported_not_thrown() {
    let js = page(
        r#"<div id=bad onclick="(" ></div>
           <div id=throws onclick="throw new Error('boom')"></div>"#,
    );
    let out = eval(
        &js,
        r#"
        const bad = document.getElementById('bad');
        bad.addEventListener('click', () => log.push('after bad'));
        bad.click();
        const value = bad.onclick;
        const t = document.getElementById('throws');
        t.addEventListener('click', () => log.push('after throw'));
        t.click();
        [log, value, errors.length, errors[0].startsWith('Uncaught SyntaxError'), errors[1]]
        "#,
    );
    assert_eq!(
        out,
        r#"[["after bad","after throw"],null,2,true,"Uncaught Error: boom"]"#
    );
}

#[test]
fn copies_compile_their_own_handler() {
    let js = page(r#"<div id=a onclick="log.push(this.id)"></div>"#);
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        a.onclick = () => log.push('idl');
        const copy = a.cloneNode();
        copy.id = 'copy';
        copy.click();
        const holder = document.createElement('div');
        holder.innerHTML = '<span id=inner onclick="log.push(this.id)"></span>';
        holder.firstChild.click();
        log
        "#,
    );
    assert_eq!(out, r#"["copy","inner"]"#);
}

#[test]
fn other_documents_do_not_run_handlers() {
    let js = page("");
    let out = eval(
        &js,
        r#"
        const doc = document.implementation.createHTMLDocument('');
        doc.body.innerHTML = '<div onclick="log.push(1)"></div>';
        doc.body.firstChild.click();
        [log, doc.body.firstChild.onclick]
        "#,
    );
    assert_eq!(out, "[[],null]");
}

#[test]
fn body_attributes_set_window_handlers() {
    let js = page(
        r#"<body onload="log.push(['load', this === window, event.type])"
                 onerror="log.push([event, source, lineno, colno, error && error.message]); return true"
                 onclick="log.push(['click', this.tagName])"
                 onpopstate="log.push('popstate')">"#,
    );
    js.fire_event(EventTarget::Window, "load", false, false);
    let out = eval(
        &js,
        r#"
        document.body.click();
        dispatchEvent(new ErrorEvent('error', { message: 'm', filename: 'f.js', lineno: 1, colno: 2, error: new Error('e') }));
        const sameHandler = document.body.onload === window.onload && typeof onload === 'function';
        // The window-reflecting ones belong to the window, not the body.
        document.body.dispatchEvent(new Event('popstate'));
        dispatchEvent(new Event('popstate'));
        document.body.setAttribute('onload', "log.push('changed')");
        dispatchEvent(new Event('load'));
        document.body.removeAttribute('onload');
        dispatchEvent(new Event('load'));
        [log, sameHandler, window.onload]
        "#,
    );
    assert_eq!(
        out,
        r#"[[["load",true,"load"],["click","BODY"],["m","f.js",1,2,"e"],"popstate","changed"],true,null]"#
    );
}

#[test]
fn the_parsed_body_replaces_handlers_set_before_it() {
    let js = page(r#"<body onload="log.push('attr')">"#);
    // As if a script in the head set it before the parser reached the body.
    js.run_script("window.onload = () => log.push('script');", "head.js")
        .unwrap();
    let body = js.with_document(|doc| doc.body()).unwrap();
    js.body_inserted(body);
    js.fire_event(EventTarget::Window, "load", false, false);
    // A script after the body overrides it in turn.
    js.run_script("window.onload = () => log.push('later script');", "body.js")
        .unwrap();
    js.fire_event(EventTarget::Window, "load", false, false);
    assert_eq!(eval(&js, "log"), r#"["attr","later script"]"#);
}
