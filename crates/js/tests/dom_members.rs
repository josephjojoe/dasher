//! DOM members implemented natively (see "Native members" in the `dom`
//! module): they must look and behave like the JS members they replaced.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
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

/// Runs `actual` and `expected`, two JS array expressions, and returns the
/// entries that differ as `index: actual != expected`.
fn differences(js: &JsRuntime, setup: &str, actual: &str, expected: &str) -> String {
    eval(
        js,
        &format!(
            "{setup}\n;(() => {{ const a$ = {actual}; const e$ = {expected}; const out$ = [];\n\
             for (let i = 0; i < Math.max(a$.length, e$.length); i++) {{\n\
               const x = JSON.stringify(a$[i]), y = JSON.stringify(e$[i]);\n\
               if (x !== y) out$.push(i + ': ' + x + ' != ' + y);\n\
             }} return out$; }})()"
        ),
    )
}

/// Every native member, by the prototypes that have it, with whether it is
/// enumerable there (mixin members are), and whether it has a setter.
const ACCESSORS: &str = r#"[
  ['Node', false, ['nodeType', 'nodeName', 'isConnected', 'ownerDocument', 'parentNode',
    'parentElement', 'firstChild', 'lastChild', 'previousSibling', 'nextSibling']],
  ['Node', false, ['nodeValue'], true],
  ['Element', true, ['firstElementChild', 'lastElementChild', 'childElementCount',
    'previousElementSibling', 'nextElementSibling']],
  ['Document', true, ['firstElementChild', 'lastElementChild', 'childElementCount']],
  ['DocumentFragment', true, ['firstElementChild', 'lastElementChild', 'childElementCount']],
  ['CharacterData', true, ['previousElementSibling', 'nextElementSibling']],
  ['Element', false, ['namespaceURI', 'localName', 'tagName', 'shadowRoot']],
  ['Element', false, ['id', 'className'], true],
  ['CharacterData', false, ['data'], true],
  ['CharacterData', false, ['length']],
]"#;

#[test]
fn accessors_keep_their_descriptors_and_look_native() {
    let js = page("<p>x</p>");
    let out = eval(
        &js,
        &r#"
            const bad = [];
            for (const [iface, enumerable, names, setter] of ACCESSORS) {
              for (const name of names) {
                const d = Object.getOwnPropertyDescriptor(globalThis[iface].prototype, name);
                const where = iface + '.' + name;
                if (!d || typeof d.get !== 'function') { bad.push(where + ' has no getter'); continue; }
                if (d.enumerable !== enumerable) bad.push(where + ' enumerable');
                if (d.configurable !== true) bad.push(where + ' configurable');
                if ((typeof d.set === 'function') !== !!setter) bad.push(where + ' setter');
                if (d.get.name !== 'get ' + name) bad.push(where + ' name ' + d.get.name);
                if (d.get.length !== 0) bad.push(where + ' length');
                if (Function.prototype.toString.call(d.get) !== 'function get ' + name + '() {\n    [native code]\n}') {
                  bad.push(where + ' source ' + Function.prototype.toString.call(d.get));
                }
                if ('prototype' in d.get) bad.push(where + ' has a prototype');
                try { new d.get(); bad.push(where + ' constructs'); } catch (e) {}
              }
            }
            for (const [name, length] of [['hasChildNodes', 0], ['contains', 1]]) {
              const d = Object.getOwnPropertyDescriptor(Node.prototype, name);
              if (d.enumerable || !d.writable || !d.configurable) bad.push(name + ' attributes');
              if (d.value.name !== name || d.value.length !== length) bad.push(name + ' name/length');
              if (!String(d.value).includes('[native code]')) bad.push(name + ' source');
            }
            bad
            "#
        .replace("ACCESSORS", ACCESSORS),
    );
    assert_eq!(out, "[]");
}

#[test]
fn no_stand_in_is_left() {
    // Every `nativeMember()` stand-in in dom.js was replaced: none of the
    // exposed interfaces has a JS member whose body calls it.
    let js = page("");
    let out = eval(
        &js,
        r#"
        const left = [];
        for (const key of Object.getOwnPropertyNames(globalThis)) {
          const C = globalThis[key];
          if (typeof C !== 'function' || !C.prototype) continue;
          for (const name of Object.getOwnPropertyNames(C.prototype)) {
            const d = Object.getOwnPropertyDescriptor(C.prototype, name);
            for (const f of [d.get, d.value]) {
              if (typeof f === 'function' && /\bnativeMember\(/.test(Function.prototype.toString.call(f))) {
                left.push(key + '.' + name);
              }
            }
          }
        }
        left
        "#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn values_match_the_dom() {
    let js = page(
        "<!doctype html><div id=a class='x y'>t<b id=b>bold</b><!--c--><i></i>u</div><svg><foreignObject/></svg>",
    );
    let setup = r#"
        const a = document.getElementById('a');
        const b = document.getElementById('b');
        const t = a.firstChild;
        const c = b.nextSibling;
        const fo = document.querySelector('foreignObject');
        const frag = document.createDocumentFragment();
        frag.append('x', document.createElement('em'), 'y');
        const ns = document.createElementNS('http://www.w3.org/1999/xhtml', 'foo:bar');
        const odd = document.createElement('span');
        odd.setAttributeNS('urn:x', 'id', 'q');
        const emoji = document.createTextNode('😀a');
    "#;
    let actual = r#"[
        a.nodeType, t.nodeType, c.nodeType, document.nodeType, document.doctype.nodeType, frag.nodeType,
        a.nodeName, t.nodeName, c.nodeName, document.nodeName, document.doctype.nodeName, frag.nodeName,
        a.tagName, a.localName, a.namespaceURI, fo.tagName, fo.localName, fo.namespaceURI,
        ns.tagName, ns.localName, ns.prefix,
        a.id, a.className, b.className, odd.id, odd.className,
        a.parentNode === document.body, a.parentElement === document.body,
        document.documentElement.parentElement, document.documentElement.parentNode === document,
        a.firstChild === t, a.lastChild.data, t.previousSibling, t.nextSibling === b,
        a.firstElementChild === b, a.lastElementChild.tagName, a.childElementCount,
        b.previousElementSibling, b.nextElementSibling.tagName, t.nextElementSibling === b,
        c.previousElementSibling === b, frag.firstElementChild.tagName, frag.childElementCount,
        document.firstElementChild === document.documentElement, document.childElementCount,
        a.isConnected, frag.isConnected, frag.firstChild.isConnected,
        a.ownerDocument === document, document.ownerDocument, frag.ownerDocument === document,
        t.nodeValue, c.nodeValue, a.nodeValue, t.data, c.data, t.length, emoji.length,
        a.hasChildNodes(), document.createElement('p').hasChildNodes(),
        a.contains(t), a.contains(a), t.contains(a), a.contains(null), a.contains(undefined),
        document.contains(b),
    ]"#;
    let expected = r#"[
        1, 3, 8, 9, 10, 11,
        'DIV', '#text', '#comment', '#document', 'html', '#document-fragment',
        'DIV', 'div', 'http://www.w3.org/1999/xhtml', 'foreignObject', 'foreignObject',
        'http://www.w3.org/2000/svg',
        'FOO:BAR', 'bar', 'foo',
        'a', 'x y', '', 'q', '',
        true, true, null, true,
        true, 'u', null, true,
        true, 'I', 2,
        null, 'I', true,
        true, 'EM', 1,
        true, 1,
        true, false, false,
        true, null, true,
        't', 'c', null, 't', 'c', 1, 3,
        true, false,
        true, true, false, false, false,
        true,
    ]"#;
    assert_eq!(differences(&js, setup, actual, expected), "[]");
}

#[test]
fn shadow_root_hides_closed_roots() {
    let js = page("<div id=o></div><div id=c></div><p></p>");
    let out = eval(
        &js,
        r#"
        const o = document.getElementById('o'), c = document.getElementById('c');
        const root = o.attachShadow({ mode: 'open' });
        c.attachShadow({ mode: 'closed' });
        [o.shadowRoot === root, c.shadowRoot, document.querySelector('p').shadowRoot,
         root.firstChild, root.ownerDocument === document, root.isConnected]
        "#,
    );
    assert_eq!(out, "[true,null,null,null,true,true]");
}

#[test]
fn wrong_receivers_throw_illegal_invocation() {
    let js = page("<p>t</p>");
    let setup = r#"
        const get = (C, name) => Object.getOwnPropertyDescriptor(C.prototype, name).get;
        const p = document.querySelector('p');
        const t = p.firstChild;
        const attempt = (f) => {
          try { return 'returned ' + f(); } catch (e) { return e.constructor.name + ': ' + e.message; }
        };
        const illegal = 'TypeError: Illegal invocation';
        const notNode = "TypeError: Failed to execute 'contains' on 'Node': parameter 1 is not of type 'Node'.";
    "#;
    let actual = r#"[
        attempt(() => get(Node, 'nodeType').call({})),
        attempt(() => get(Node, 'nodeType').call(undefined)),
        attempt(() => get(Node, 'nodeType').call(1)),
        attempt(() => Node.prototype.nodeType),
        attempt(() => get(Node, 'firstChild').call(new Proxy(p, {}))),
        attempt(() => get(Element, 'tagName').call(t)),
        attempt(() => get(Element, 'id').call(document)),
        attempt(() => get(CharacterData, 'data').call(p)),
        attempt(() => get(Node, 'parentNode').call(document.createElement('p').attributes)),
        attempt(() => Node.prototype.hasChildNodes.call({})),
        attempt(() => p.contains({})),
        attempt(() => p.contains(1)),
        attempt(() => get(Node, 'nodeType').call(t)),
    ]"#;
    let expected = r#"[
        illegal, illegal, illegal, illegal, illegal, illegal, illegal, illegal, illegal, illegal,
        notNode, notNode, 'returned 3',
    ]"#;
    assert_eq!(differences(&js, setup, actual, expected), "[]");
}

#[test]
fn subclasses_inherit_and_override_native_members() {
    let js = page("");
    let out = eval(
        &js,
        r#"
        class XA extends HTMLElement {
          get id() { return 'own:' + super.id; }
          get tagName() { return super.tagName.toLowerCase(); }
        }
        customElements.define('x-a', XA);
        const x = document.createElement('x-a');
        x.setAttribute('id', 'i');
        document.body.append(x);
        const y = new XA();
        [x instanceof XA, x.id, x.tagName, x.nodeName, x.localName, x.parentNode === document.body,
         x.isConnected, y.tagName, y.isConnected, Object.getPrototypeOf(y) === XA.prototype]
        "#,
    );
    assert_eq!(
        out,
        r#"[true,"own:i","x-a","X-A","x-a",true,true,"x-a",false,true]"#
    );
}

#[test]
fn names_follow_a_replaced_document() {
    // The tag name cache is by name id, and ids of names that are not
    // well known are the document's own.
    let js = page("<x-old></x-old>");
    assert_eq!(
        eval(&js, "document.querySelector('x-old').tagName"),
        "\"X-OLD\""
    );
    js.set_document(Document::parse("<x-new></x-new><y-new></y-new>"));
    let out = eval(
        &js,
        "[...document.body.children].map((e) => [e.tagName, e.localName, e.nodeName])",
    );
    assert_eq!(
        out,
        r#"[["X-NEW","x-new","X-NEW"],["Y-NEW","y-new","Y-NEW"]]"#
    );
}

#[test]
fn nodes_of_a_replaced_document_throw() {
    let js = page("<p>t</p>");
    js.run_script("globalThis.old = document.querySelector('p');", "a.js")
        .unwrap();
    js.set_document(Document::parse("<div></div>"));
    let out = eval(
        &js,
        r#"
        const attempt = (f) => { try { return 'returned ' + f(); } catch (e) { return e.name; } };
        [attempt(() => old.tagName), attempt(() => old.firstChild), attempt(() => old.hasChildNodes()),
         attempt(() => document.contains(old)), attempt(() => document.body.contains(null))]
        "#,
    );
    assert_eq!(
        out,
        r#"["InvalidStateError","InvalidStateError","InvalidStateError","InvalidStateError","returned false"]"#
    );
}

#[test]
fn wrappers_made_by_getters_keep_their_identity() {
    // Walking with the native getters makes wrappers for unwrapped nodes,
    // which then stay the same objects, through collections too.
    let js = page("<ul><li>1</li><li>2</li><li>3</li></ul>");
    let out = eval(
        &js,
        r#"
        const ul = document.querySelector('ul');
        const seen = [];
        for (let n = ul.firstChild; n !== null; n = n.nextSibling) { n.mark = seen.length; seen.push(n); }
        const again = [];
        for (let n = ul.lastChild; n !== null; n = n.previousSibling) again.unshift(n);
        [seen.length, again.every((n, i) => n === seen[i] && n.mark === i),
         ul.children[1] === seen[1], seen[2].parentNode === ul, seen[0].firstChild.parentNode === seen[0]]
        "#,
    );
    assert_eq!(out, "[3,true,true,true,true]");
    js.run_gc();
    assert_eq!(eval(&js, "document.querySelector('li').mark"), "0");
}
