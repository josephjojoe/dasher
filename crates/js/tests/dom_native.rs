//! The attribute, tree-edit, query and collection members that are native
//! (see "Native members" in the `dom` module): they must look and behave
//! like the JS members they replaced, including the cases they leave to
//! the JS fallback.

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

/// Evaluates `checks`, a JS array of `[label, actual, expected]`, and
/// returns the labels of those whose JSON differs, with both values.
fn failures(js: &JsRuntime, setup: &str, checks: &str) -> String {
    eval(
        js,
        &format!(
            "{setup}\n;(() => {{ const out$ = [];\n\
             for (const [l$, a$, e$] of {checks}) {{\n\
               const x$ = JSON.stringify(a$), y$ = JSON.stringify(e$);\n\
               if (x$ !== y$) out$.push(l$ + ': ' + x$ + ' != ' + y$);\n\
             }} return out$; }})()"
        ),
    )
}

/// What calling `f` throws, as `Name: message`, or `none`.
const THROWS: &str = "const throws = (f) => { try { f(); return 'none'; } catch (e) { return e.name + ': ' + e.message; } };";

#[test]
fn members_look_native() {
    let js = page("<p>x</p>");
    let out = eval(
        &js,
        r#"
        const bad = [];
        const methods = [
          ['Element', ['getAttribute', 1], ['setAttribute', 2], ['hasAttribute', 1],
            ['removeAttribute', 1], ['toggleAttribute', 1], ['getAttributeNS', 2],
            ['setAttributeNS', 3], ['removeAttributeNS', 2], ['hasAttributeNS', 2],
            ['hasAttributes', 0], ['getAttributeNames', 0], ['matches', 1], ['closest', 1],
            ['webkitMatchesSelector', 1], ['querySelector', 1], ['querySelectorAll', 1],
            ['append', 0], ['prepend', 0], ['replaceChildren', 0], ['before', 0], ['after', 0],
            ['replaceWith', 0], ['remove', 0]],
          ['Node', ['appendChild', 1], ['insertBefore', 2], ['removeChild', 1],
            ['replaceChild', 2], ['cloneNode', 0]],
          ['Document', ['createElement', 1], ['createTextNode', 1], ['createComment', 1],
            ['createDocumentFragment', 0], ['getElementById', 1], ['querySelector', 1]],
          ['DocumentFragment', ['getElementById', 1], ['append', 0]],
          ['CharacterData', ['remove', 0], ['before', 0]],
          ['DocumentType', ['remove', 0], ['after', 0]],
          ['HTMLCollection', ['item', 1], ['namedItem', 1]],
        ];
        for (const [iface, ...members] of methods) {
          for (const [name, length] of members) {
            const d = Object.getOwnPropertyDescriptor(globalThis[iface].prototype, name);
            const where = iface + '.' + name;
            if (!d || typeof d.value !== 'function') { bad.push(where + ' missing'); continue; }
            if (!d.writable || !d.configurable) bad.push(where + ' attributes');
            if (d.value.name !== name || d.value.length !== length) {
              bad.push(where + ' name/length ' + d.value.name + '/' + d.value.length);
            }
            if (Function.prototype.toString.call(d.value) !== 'function ' + name + '() {\n    [native code]\n}') {
              bad.push(where + ' source');
            }
          }
        }
        const accessors = [
          ['Node', 'textContent', true], ['Node', 'childNodes', false], ['Element', 'innerHTML', true],
          ['Element', 'attributes', false], ['Element', 'children', false],
          ['NodeList', 'length', false], ['HTMLCollection', 'length', false],
          ['NamedNodeMap', 'length', false],
          ['HTMLElement', 'title', true], ['HTMLElement', 'hidden', true],
          ['HTMLAnchorElement', 'href', true], ['HTMLInputElement', 'type', true],
          ['HTMLImageElement', 'width', true],
        ];
        for (const [iface, name, setter] of accessors) {
          const d = Object.getOwnPropertyDescriptor(globalThis[iface].prototype, name);
          const where = iface + '.' + name;
          if (!d || typeof d.get !== 'function') { bad.push(where + ' has no getter'); continue; }
          if (!String(d.get).includes('[native code]')) bad.push(where + ' getter source');
          if (setter && !String(d.set).includes('[native code]')) bad.push(where + ' setter source');
          if (setter && d.set.name !== 'set ' + name) bad.push(where + ' setter name ' + d.set.name);
          if (d.get.name !== 'get ' + name) bad.push(where + ' getter name ' + d.get.name);
        }
        // Reflected attributes keep reflect()'s attributes: enumerable.
        if (!Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'title').enumerable) bad.push('title enumerable');
        // draggable and spellcheck are not reflect()'s any more: still JS
        // (whose functions are named after the property they define).
        if (Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'draggable').get.name !== 'get') {
          bad.push('draggable made native');
        }
        // As in browsers, NodeList's iteration is Array.prototype's.
        if (NodeList.prototype.forEach !== Array.prototype.forEach) bad.push('forEach');
        if (NodeList.prototype[Symbol.iterator] !== Array.prototype.values) bad.push('NodeList iterator');
        if (HTMLCollection.prototype[Symbol.iterator] !== Array.prototype.values) bad.push('HTMLCollection iterator');
        bad
        "#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn attributes_behave_as_before() {
    let js = page("<div id=a class='x y' data-k=v></div><svg><rect viewBox='0 0 1 1'/></svg>");
    let out = failures(
        &js,
        &format!(
            "{THROWS}
            const a = document.getElementById('a');
            const rect = document.querySelector('rect');
            a.setAttribute('Title', 'T');
            a.setAttributeNS('urn:x', 'p:q', 'ns');
            const names = a.getAttributeNames();
            const toggled = [a.toggleAttribute('hidden'), a.hasAttribute('HIDDEN'),
              a.toggleAttribute('hidden', true), a.toggleAttribute('hidden'), a.hasAttribute('hidden'),
              a.toggleAttribute('hidden', false)];
            a.removeAttributeNS('urn:x', 'q');
            "
        ),
        r#"[
          ['get', a.getAttribute('CLASS'), 'x y'],
          ['missing', a.getAttribute('nope'), null],
          ['set lowers', a.getAttribute('title'), 'T'],
          ['names', names, ['id', 'class', 'data-k', 'title', 'p:q']],
          ['ns get', a.getAttributeNS('urn:x', 'q'), null],
          ['ns has', a.hasAttributeNS(null, 'id'), true],
          ['toggle', toggled, [true, true, true, false, false, false]],
          ['svg keeps case', rect.getAttribute('viewBox'), '0 0 1 1'],
          ['svg no lower', rect.getAttribute('viewbox'), null],
          ['number value', (a.setAttribute('n', 5), a.getAttribute('n')), '5'],
          ['object value', (a.setAttribute('o', { toString() { return 'obj'; } }), a.getAttribute('o')), 'obj'],
          ['lone surrogate', (a.setAttribute('s', '\uD800x'), a.getAttribute('s')), '�x'],
          ['null value', (a.setAttribute('z', null), a.getAttribute('z')), 'null'],
          ['too few', throws(() => a.getAttribute()),
            "TypeError: Failed to execute 'getAttribute' on 'Element': 1 argument required, but only 0 present."],
          ['too few set', throws(() => a.setAttribute('x')),
            "TypeError: Failed to execute 'setAttribute' on 'Element': 2 arguments required, but only 1 present."],
          ['bad name', throws(() => a.setAttribute('a b', '')),
            "InvalidCharacterError: 'a b' is not a valid attribute name."],
          ['ns error', throws(() => a.setAttributeNS(null, 'p:q', '')),
            "NamespaceError: The namespace URI provided ('') is not valid for the qualified name provided."],
          ['receiver', throws(() => Element.prototype.getAttribute.call({}, 'x')), 'TypeError: Illegal invocation'],
          ['text receiver', throws(() => Element.prototype.setAttribute.call(document.createTextNode(''), 'x', '')),
            'TypeError: Illegal invocation'],
          ['id setter', (a.id = 'b', document.getElementById('b') === a), true],
          ['className setter', (a.className = 'q', a.getAttribute('class')), 'q'],
        ]"#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn named_node_map_is_live_and_native() {
    let js = page("<div id=a class=c title=t></div>");
    let out = failures(
        &js,
        "const a = document.getElementById('a'); const m = a.attributes;",
        r#"[
          ['identity', m === a.attributes, true],
          ['length', m.length, 3],
          ['index', m[1].name + '=' + m[1].value, 'class=c'],
          ['item', m.item(2).value, 't'],
          ['named', m.title.value, 't'],
          ['getNamedItem', m.getNamedItem('CLASS').value, 'c'],
          ['out of range', m[9], undefined],
          ['in', [0 in m, 3 in m, 'title' in m, 'nope' in m], [true, false, true, false]],
          ['keys', Object.keys(m), ['0', '1', '2']],
          ['live', (a.setAttribute('x', '1'), m.length), 4],
          ['iterate', [...m].map((x) => x.name), ['id', 'class', 'title', 'x']],
          ['remove', (m.removeNamedItem('x'), m.length), 3],
          ['proto wins', typeof m.item, 'function'],
          ['expando', (m.foo = 1, m.foo), 1],
          ['tag', Object.prototype.toString.call(m), '[object NamedNodeMap]'],
          ['instanceof', m instanceof NamedNodeMap, true],
          ['owner check', (() => { try { NamedNodeMap.prototype.item.call({}, 0); return 'none'; } catch (e) { return e.name; } })(), 'TypeError'],
        ]"#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn collections_are_live_without_a_proxy() {
    let js = page(
        "<form id=f><input name=email id=e><input name=pw><b id=item></b></form><ul id=u><li>1</li><li>2</li></ul>",
    );
    let out = failures(
        &js,
        "const u = document.getElementById('u'); const kids = u.children; const nodes = u.childNodes;
         const lis = document.getElementsByTagName('li'); const f = document.getElementById('f');
         const els = f.elements; const coll = document.getElementsByTagName('input');",
        r#"[
          ['identity', [kids === u.children, nodes === u.childNodes], [true, true]],
          ['not a proxy', [kids, nodes, lis].some((x) => x.constructor !== x.constructor), false],
          ['length', [kids.length, nodes.length, lis.length], [2, 2, 2]],
          ['index', kids[1].textContent, '2'],
          ['item', [kids.item(0).textContent, kids.item(5), nodes.item(1).textContent], ['1', null, '2']],
          ['live', (u.append(document.createElement('li')), [kids.length, lis.length, nodes.length]), [3, 3, 3]],
          ['named', [coll.email.id, coll.namedItem('pw').name, coll.e.name], ['e', 'pw', 'email']],
          ['proto wins over names', typeof document.getElementsByTagName('b').item, 'function'],
          ['form elements', [els.email.id, els.length], ['e', 2]],
          ['in', [0 in kids, 9 in kids, 'email' in coll, 'nope' in coll], [true, false, true, false]],
          ['keys', Object.keys(coll), ['0', '1']],
          ['own names', Object.getOwnPropertyNames(coll), ['0', '1', 'e', 'email', 'pw']],
          ['descriptor', JSON.stringify(Object.getOwnPropertyDescriptor(kids, 0)) !== undefined &&
            [Object.getOwnPropertyDescriptor(kids, 0).enumerable, Object.getOwnPropertyDescriptor(coll, 'pw').enumerable],
            [true, false]],
          ['index assignment ignored', (kids[0] = 5, kids[0].tagName), 'LI'],
          ['expando', (kids.foo = 2, kids.foo), 2],
          ['spread', [...kids].length, 3],
          ['for of', (() => { let n = 0; for (const x of nodes) n += x.nodeType; return n; })(), 3],
          ['forEach', (() => { const out = []; nodes.forEach((n, i) => out.push(i)); return out; })(), [0, 1, 2]],
          ['entries', [...nodes.entries()].map(([i]) => i), [0, 1, 2]],
          ['Array.from', Array.from(lis).length, 3],
          ['static qsa', [Array.isArray(u.querySelectorAll('li')), u.querySelectorAll('li') instanceof NodeList,
            u.querySelectorAll('li').item(0).tagName], [true, true, 'LI']],
          ['tags', [String(kids), String(nodes)], ['[object HTMLCollection]', '[object NodeList]']],
          ['length receiver', (() => { try { Object.getOwnPropertyDescriptor(HTMLCollection.prototype, 'length').get.call(nodes); return 'none'; } catch (e) { return e.message; } })(), 'Illegal invocation'],
        ]"#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn reflected_attributes_read_and_write_the_content_attributes() {
    let js = page(
        "<base href='http://example.com/dir/'><a id=a href='p?q'>x</a><input id=i type=CHECKBOX maxlength=x><img id=m width=12px><p id=p>",
    );
    let out = failures(
        &js,
        "const a = document.getElementById('a'); const i = document.getElementById('i');
         const m = document.getElementById('m'); const p = document.getElementById('p');",
        r#"[
          ['url', a.href, 'http://example.com/dir/p?q'],
          ['url missing', document.createElement('a').href, ''],
          ['url set', (a.href = '/x', [a.getAttribute('href'), a.href]), ['/x', 'http://example.com/x']],
          ['enum', i.type, 'checkbox'],
          ['enum missing', document.createElement('input').type, 'text'],
          ['long', m.width, 12],
          ['long set', (m.width = '7.9', m.getAttribute('width')), '7'],
          ['bool', [p.hidden, (p.hidden = 1, p.getAttribute('hidden')), (p.hidden = 0, p.hasAttribute('hidden'))], [false, '', false]],
          ['string', [p.title, (p.title = null, p.title)], ['', 'null']],
          ['nullable', [p.popover, (p.popover = 'auto', p.popover), (p.popover = null, p.hasAttribute('popover'))], [null, 'auto', false]],
          ['receiver', (() => { try { Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'title').get.call({}); return 'none'; } catch (e) { return e.message; } })(), 'Illegal invocation'],
        ]"#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn tree_edits_behave_as_before() {
    let js = page("<div id=d><b>1</b><i>2</i><u>3</u></div><div id=e></div>");
    let out = failures(
        &js,
        &format!(
            "{THROWS}
            const d = document.getElementById('d'); const e = document.getElementById('e');
            const [b, i, u] = d.children;
            const html = () => d.innerHTML;
            "
        ),
        r#"[
          ['append strings', (e.append('a', 1, null), e.innerHTML), 'a1null'],
          ['append nodes and strings', (e.replaceChildren(), e.append(b, 'x', i), [e.innerHTML, html()]),
            ['<b>1</b>x<i>2</i>', '<u>3</u>']],
          ['prepend', (d.prepend(i, 'y'), html()), '<i>2</i>y<u>3</u>'],
          ['before among args', (u.before(u, 'z'), html()), '<i>2</i>y<u>3</u>z'],
          ['after', (i.after(b), html()), '<i>2</i><b>1</b>y<u>3</u>z'],
          ['replaceWith', (b.replaceWith('w', b), html()), '<i>2</i>w<b>1</b>y<u>3</u>z'],
          ['remove', (i.remove(), i.parentNode), null],
          ['replaceChildren none', (e.replaceChildren(), e.childNodes.length), 0],
          ['appendChild returns', e.appendChild(i) === i, true],
          ['insertBefore null', (e.insertBefore(b, null), e.lastChild === b), true],
          ['insertBefore undefined', (e.insertBefore(u, undefined), e.lastChild === u), true],
          ['replaceChild returns old', e.replaceChild(d.firstChild, u) === u, true],
          ['removeChild returns', e.removeChild(b) === b, true],
          ['not found', throws(() => e.removeChild(u)),
            'NotFoundError: NotFoundError: the node is not a child of this node'],
          ['not a node', throws(() => e.appendChild({})).split(':')[0], 'TypeError'],
          ['too few', throws(() => e.insertBefore(b)),
            "TypeError: Failed to execute 'insertBefore' on 'Node': 2 arguments required, but only 1 present."],
          ['hierarchy', throws(() => e.appendChild(document)).split(':')[0], 'HierarchyRequestError'],
          ['append object', (e.replaceChildren({ toString() { return 'obj'; } }), e.textContent), 'obj'],
          ['textContent', (e.textContent = null, e.childNodes.length), 0],
          ['innerHTML null', (e.innerHTML = null, e.innerHTML), ''],
          ['innerHTML undefined', (e.innerHTML = undefined, e.innerHTML), 'undefined'],
          ['cloneNode', [d.cloneNode().childNodes.length, d.cloneNode(true).childNodes.length > 0], [0, true]],
          ['clone input keeps value', (() => { const x = document.createElement('input'); x.value = 'v'; return x.cloneNode().value; })(), 'v'],
          ['createElement', [document.createElement('DIV').tagName, document.createElement('svg').namespaceURI],
            ['DIV', 'http://www.w3.org/1999/xhtml']],
          ['createElement bad', throws(() => document.createElement('1x')),
            "InvalidCharacterError: The tag name provided ('1x') is not a valid name."],
          ['createElement too few', throws(() => document.createElement()),
            "TypeError: Failed to execute 'createElement' on 'Document': 1 argument required, but only 0 present."],
          ['text and comment', [document.createTextNode(5).data, document.createComment('c').nodeType], ['5', 8]],
          ['fragment', document.createDocumentFragment().nodeType, 11],
          ['other document', (() => { const o = document.implementation.createHTMLDocument('');
            const x = o.createElement('p'); return [x.ownerDocument === o, o.createTextNode('t').ownerDocument === o]; })(), [true, true]],
        ]"#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn inserted_scripts_run_and_created_scripts_are_async() {
    let js = page("<div id=d></div>");
    let out = eval(
        &js,
        r#"
        globalThis.ran = [];
        const d = document.getElementById('d');
        const s = document.createElement('script');
        s.textContent = 'ran.push(1)';
        d.appendChild(s);
        const t = document.createElement('script');
        t.text = 'ran.push(2)';
        d.append('x', t);
        const f = document.createDocumentFragment();
        const v = document.createElement('script');
        v.text = 'ran.push(3)';
        f.appendChild(v);
        d.replaceChildren(f);
        [ran, s.async, document.createElement('SCRIPT').async]
        "#,
    );
    assert_eq!(out, "[[1,2,3],true,true]");
}

#[test]
fn queries_behave_as_before() {
    let js =
        page("<main><p id=x class=c>a</p><p class=c>b</p><template><p id=t></p></template></main>");
    let out = failures(
        &js,
        &format!(
            "{THROWS} const main = document.querySelector('main'); const frag = document.createDocumentFragment();
             frag.append(document.createElement('span')); frag.firstChild.id = 'in-frag';"
        ),
        r#"[
          ['byId', document.getElementById('x').textContent, 'a'],
          ['byId missing', document.getElementById('t'), null],
          ['byId fragment', frag.getElementById('in-frag').tagName, 'SPAN'],
          ['byId no arg', document.getElementById(), null],
          ['query', main.querySelector('.c:last-child, p + p').textContent, 'b'],
          ['queryAll', [...document.querySelectorAll('p.c')].map((p) => p.textContent), ['a', 'b']],
          ['matches', [main.matches('main'), main.webkitMatchesSelector('div')], [true, false]],
          ['closest', document.getElementById('x').closest('main') === main, true],
          ['bad selector', throws(() => main.querySelector('p[')), "SyntaxError: 'p[' is not a valid selector."],
          ['bad matches', throws(() => main.matches('')), "SyntaxError: '' is not a valid selector."],
          ['too few', throws(() => main.querySelectorAll()),
            "TypeError: Failed to execute 'querySelectorAll': 1 argument required, but only 0 present."],
          ['getElementsByClassName', document.getElementsByClassName('c').length, 2],
        ]"#,
    );
    assert_eq!(out, "[]");
}
