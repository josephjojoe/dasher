//! MutationObserver: which changes are recorded, what the records say, and
//! when and how they are delivered.

use dasher_dom::{Document, DocumentParser, ParseOptions, ParseStep};
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    // `observe(target, options)` records everything delivered to `log`, one
    // compact line per record.
    run(
        &js,
        r#"
        globalThis.log = [];
        const name = (n) => n === null ? null : n.nodeType === 1 ? n.localName + (n.id ? '#' + n.id : '') : n.nodeName + (n.data !== undefined ? ':' + n.data : '');
        // Targets without their data, which is read after the fact.
        const targetName = (n) => n.nodeType === 1 ? name(n) : n.nodeName;
        globalThis.describe = (r) => {
          let s = r.type + ' ' + targetName(r.target);
          if (r.type === 'childList') {
            s += ' +[' + [...r.addedNodes].map(name) + '] -[' + [...r.removedNodes].map(name) + ']';
            s += ' ' + name(r.previousSibling) + '|' + name(r.nextSibling);
          }
          if (r.attributeName !== null) s += ' ' + (r.attributeNamespace ? r.attributeNamespace + ' ' : '') + r.attributeName;
          if (r.oldValue !== null) s += ' old=' + r.oldValue;
          return s;
        };
        globalThis.observe = (target, options) => {
          const mo = new MutationObserver((records) => { for (const r of records) log.push(describe(r)); });
          mo.observe(target, options);
          return mo;
        };
        globalThis.$ = (id) => document.getElementById(id);
        "#,
    );
    js
}

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
}

/// Runs `source`, then the microtasks it queued, and returns the log.
fn records(js: &JsRuntime, source: &str) -> Vec<String> {
    run(js, "log.length = 0;");
    run(js, source);
    js.perform_microtask_checkpoint();
    let errors = js.take_errors();
    assert!(errors.is_empty(), "{source}: {errors:?}");
    js.take_console();
    run(js, "for (const line of log) console.log(line);");
    js.take_console().into_iter().map(|m| m.text).collect()
}

#[test]
fn records_are_delivered_in_one_microtask_after_the_script() {
    let js = page("<div id=a></div>");
    run(
        &js,
        r#"
        globalThis.order = [];
        const a = $('a');
        const mo = new MutationObserver(function (records, observer) {
          order.push('callback ' + records.length + ' ' + (this === mo) + ' ' + (observer === mo) + ' ' + Array.isArray(records));
        });
        mo.observe(a, { attributes: true, childList: true });
        a.id = 'b';
        a.append('x');
        a.title = 't';
        Promise.resolve().then(() => order.push('earlier microtask'));
        queueMicrotask(() => order.push('later microtask'));
        setTimeout(() => order.push('task'), 0);
        order.push('sync');
        "#,
    );
    // Scripts end with a microtask checkpoint.
    assert_eq!(
        js.eval_json("order").unwrap().unwrap(),
        r#"["sync","callback 3 true true true","earlier microtask","later microtask"]"#
    );
    js.run_ready().unwrap();
    assert_eq!(js.eval_json("order.at(-1)").unwrap().unwrap(), r#""task""#);
}

#[test]
fn child_list_records() {
    let js = page("<div id=a><p id=p1></p><p id=p2></p></div><div id=b></div>");
    run(&js, "observe($('a'), { childList: true });");
    assert_eq!(
        records(
            &js,
            "const s = document.createElement('span'); $('a').appendChild(s);
             $('a').insertBefore(document.createElement('i'), $('p1'));
             s.remove();",
        ),
        [
            "childList div#a +[span] -[] p#p2|null",
            "childList div#a +[i] -[] null|p#p1",
            "childList div#a +[] -[span] p#p2|null",
        ]
    );
    assert_eq!(
        records(
            &js,
            "$('a').replaceChild(document.createElement('b'), $('p1'));"
        ),
        ["childList div#a +[b] -[p#p1] i|p#p2"]
    );
    // Moving a node out of an observed parent records its removal there.
    assert_eq!(
        records(&js, "$('b').appendChild($('p2'));"),
        ["childList div#a +[] -[p#p2] b|null"]
    );
    // Within the same parent: out, then back in.
    assert_eq!(
        records(&js, "$('a').appendChild($('a').firstChild);"),
        [
            "childList div#a +[] -[i] null|b",
            "childList div#a +[i] -[] b|null",
        ]
    );
    assert_eq!(
        records(
            &js,
            "$('a').replaceChildren('x', document.createElement('u'));"
        ),
        ["childList div#a +[#text:x,u] -[b,i] null|null"]
    );
    assert_eq!(
        records(&js, "$('a').textContent = 'hi'; $('a').textContent = '';"),
        [
            "childList div#a +[#text:hi] -[#text:x,u] null|null",
            "childList div#a +[] -[#text:hi] null|null",
        ]
    );
    // Nothing to remove and nothing added: no record.
    assert!(records(&js, "$('a').textContent = ''; $('a').replaceChildren();").is_empty());
}

#[test]
fn fragments_and_html_parsing() {
    let js = page("<div id=a><p id=p></p></div>");
    run(&js, "observe($('a'), { childList: true });");
    assert_eq!(
        records(
            &js,
            "globalThis.f = document.createDocumentFragment();
             f.append(document.createElement('i'), 'x');
             observe(f, { childList: true });
             $('a').prepend(f);",
        ),
        [
            "childList div#a +[i,#text:x] -[] null|p#p",
            "childList #document-fragment +[] -[i,#text:x] null|null",
        ]
    );
    // An empty fragment inserts nothing, so it is not recorded.
    assert!(records(&js, "$('a').append(document.createDocumentFragment());").is_empty());
    assert_eq!(
        records(&js, "$('a').innerHTML = '<b>1</b><u>2</u>';"),
        ["childList div#a +[b,u] -[i,#text:x,p#p] null|null"]
    );
    assert_eq!(
        records(
            &js,
            "$('a').firstChild.insertAdjacentHTML('afterend', '<em></em><s></s>');
             $('a').insertAdjacentHTML('afterbegin', 'text');
             $('a').insertAdjacentHTML('beforeend', '');",
        ),
        [
            "childList div#a +[em,s] -[] b|u",
            "childList div#a +[#text:text] -[] null|b",
        ]
    );
    assert_eq!(
        records(&js, "$('a').querySelector('em').outerHTML = '<q></q>tail';"),
        ["childList div#a +[q,#text:tail] -[em] b|s"]
    );
}

#[test]
fn template_contents_are_their_own_tree() {
    let js = page("<template id=t><p></p></template>");
    run(
        &js,
        "observe($('t'), { childList: true, subtree: true }); observe($('t').content, { childList: true });",
    );
    assert_eq!(
        records(&js, "$('t').innerHTML = '<i></i>';"),
        ["childList #document-fragment +[i] -[p] null|null"]
    );
}

#[test]
fn attribute_records() {
    let js = page("<div id=a class=x></div>");
    run(
        &js,
        "observe($('a'), { attributes: true, attributeOldValue: true });",
    );
    assert_eq!(
        records(
            &js,
            "const a = $('a');
             a.setAttribute('class', 'y');
             a.setAttribute('class', 'y');
             a.classList.add('z');
             a.dataset.fooBar = '1';
             a.style.color = 'red';
             a.removeAttribute('data-foo-bar');
             a.removeAttribute('missing');
             a.toggleAttribute('hidden');
             a.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#u');
             a.setAttributeNS('http://www.w3.org/1999/xlink', 'xl:href', '#v');
             a.removeAttributeNS('http://www.w3.org/1999/xlink', 'href');",
        ),
        [
            "attributes div#a class old=x",
            "attributes div#a class old=y",
            "attributes div#a class old=y",
            "attributes div#a data-foo-bar",
            "attributes div#a style",
            "attributes div#a data-foo-bar old=1",
            "attributes div#a hidden",
            "attributes div#a http://www.w3.org/1999/xlink href",
            "attributes div#a http://www.w3.org/1999/xlink href old=#u",
            "attributes div#a http://www.w3.org/1999/xlink href old=#v",
        ]
    );
    // Without attributeOldValue there is no old value.
    run(&js, "observe($('a'), { attributes: true });");
    run(
        &js,
        "observe($('a'), { attributeFilter: ['title', 'lang'] });",
    );
    assert_eq!(
        records(&js, "$('a').title = 'T'; $('a').id = 'a';"),
        // Each observer gets its records in turn, in the order the
        // observers were created.
        [
            "attributes div#a title",
            "attributes div#a id old=a",
            "attributes div#a title",
            "attributes div#a id",
            "attributes div#a title",
        ]
    );
    // A filter only matches attributes in no namespace.
    run(&js, "observe($('a'), { attributeFilter: ['href'] });");
    let log = records(
        &js,
        "$('a').setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#w');",
    );
    assert_eq!(log.len(), 2, "{log:?}");
}

#[test]
fn character_data_records() {
    let js = page("<p id=p>one<!--c--></p>");
    run(
        &js,
        "observe($('p'), { characterData: true, subtree: true, characterDataOldValue: true });
         observe($('p').firstChild, { characterData: true });
         observe($('p'), { characterData: true });",
    );
    assert_eq!(
        records(
            &js,
            "const t = $('p').firstChild;
             t.data = 'two';
             t.appendData('!');
             t.nodeValue = 'three';
             t.textContent = 'four';
             $('p').lastChild.data = 'd';",
        ),
        [
            "characterData #text old=one",
            "characterData #text old=two",
            "characterData #text old=two!",
            "characterData #text old=three",
            "characterData #comment old=c",
            "characterData #text",
            "characterData #text",
            "characterData #text",
            "characterData #text",
        ]
    );
}

#[test]
fn normalize_records() {
    let js = page("<p id=p></p>");
    run(
        &js,
        "const p = $('p');
         p.append('a', 'b', '', 'c', document.createElement('br'), '', 'd');
         observe(p, { childList: true, characterData: true, subtree: true, characterDataOldValue: true });",
    );
    assert_eq!(
        records(&js, "$('p').normalize();"),
        [
            "characterData #text old=a",
            "childList p#p +[] -[#text:b] #text:abc|#text:",
            "childList p#p +[] -[#text:] #text:abc|#text:c",
            "childList p#p +[] -[#text:c] #text:abc|br",
            "childList p#p +[] -[#text:] br|#text:d",
        ]
    );
    assert_eq!(
        js.eval_json("$('p').innerHTML").unwrap().unwrap(),
        r#""abc<br>d""#
    );
}

#[test]
fn subtree_and_options() {
    let js = page("<div id=a><p id=p><b id=b></b></p></div>");
    run(
        &js,
        "observe($('a'), { childList: true, attributes: true });",
    );
    // Changes below the target need subtree.
    assert!(records(&js, "$('p').append('x'); $('b').id = 'c'; $('c').id = 'b';").is_empty());
    run(
        &js,
        "observe($('a'), { childList: true, attributes: true, subtree: true });",
    );
    assert_eq!(
        records(&js, "$('b').append('y'); $('b').title = 't';"),
        [
            "childList b#b +[#text:y] -[] null|null",
            "attributes b#b title",
        ]
    );
    // Observing again replaces the options rather than adding a second
    // registration.
    assert_eq!(
        records(
            &js,
            "const mo = observe($('p'), { attributes: true });
             mo.observe($('p'), { childList: true });
             $('p').title = 'x';
             $('p').append('z');",
        ),
        // The first record is the subtree observer's on #a.
        [
            "attributes p#p title",
            "childList p#p +[#text:z] -[] #text:x|null",
            "childList p#p +[#text:z] -[] #text:x|null",
        ]
    );
    // One observer on a node and its ancestor gets one record per change.
    let log = records(
        &js,
        "const both = observe($('a'), { attributes: true, subtree: true });
         both.observe($('b'), { attributes: true });
         $('b').lang = 'en';",
    );
    assert_eq!(
        log.iter().filter(|r| *r == "attributes b#b lang").count(),
        2
    );
}

#[test]
fn observe_validates_its_options() {
    let js = page("<div id=a></div>");
    let out = js
        .eval_json(
            r#"
            const a = $('a');
            const mo = new MutationObserver(() => {});
            const attempt = (...args) => { try { mo.observe(...args); return 'ok'; } catch (e) { return e.constructor.name; } };
            [
              attempt(a), attempt(a, {}), attempt(a, { subtree: true }),
              attempt(a, { attributes: false, attributeOldValue: true }),
              attempt(a, { attributes: false, attributeFilter: [] }),
              attempt(a, { characterData: false, characterDataOldValue: true }),
              attempt(a, 'x'), attempt({}, { childList: true }), attempt(null, { childList: true }),
              attempt(a, { attributeFilter: 'class' }),
              attempt(a, { childList: true }), attempt(a, { attributeOldValue: false }),
              attempt(a, { attributeFilter: new Set(['x']) }), attempt(a, { characterDataOldValue: false }),
              (() => { try { new MutationObserver(); } catch (e) { return e.constructor.name; } })(),
              (() => { try { new MutationRecord(); } catch (e) { return e.constructor.name; } })(),
              String(mo), WebKitMutationObserver === MutationObserver,
              Object.keys(window).includes('MutationObserver'),
            ]
            "#,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        out,
        r#"["TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","ok","ok","ok","ok","TypeError","TypeError","[object MutationObserver]",true,false]"#
    );
    // attributeOldValue, attributeFilter and characterDataOldValue imply
    // the kind of record they are about.
    run(
        &js,
        "observe(a, { attributeOldValue: true }); observe(a, { characterDataOldValue: false, subtree: true });",
    );
    assert_eq!(
        records(
            &js,
            "a.title = 'x'; a.append('t'); a.firstChild.data = 'u';"
        ),
        ["attributes div#a title", "characterData #text"]
    );
}

#[test]
fn take_records_and_disconnect() {
    let js = page("<div id=a></div>");
    let out = js
        .eval_json(
            r#"
            globalThis.calls = 0;
            const mo = new MutationObserver(() => calls++);
            mo.observe($('a'), { attributes: true });
            $('a').title = '1';
            $('a').title = '2';
            const taken = mo.takeRecords();
            mo.disconnect();
            [taken.length, taken[0] instanceof MutationRecord, taken[1].target === $('a'), mo.takeRecords().length]
            "#,
        )
        .unwrap()
        .unwrap();
    assert_eq!(out, "[2,true,true,0]");
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("calls").unwrap().unwrap(), "0");
    run(
        &js,
        "globalThis.mo2 = new MutationObserver(() => calls++);
         mo2.observe($('a'), { attributes: true });
         mo2.observe(document.body, { childList: true, subtree: true });
         $('a').title = '3';
         mo2.disconnect();
         $('a').title = '4';
         $('a').append('x');",
    );
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("calls").unwrap().unwrap(), "0");
    // A disconnected observer can observe again.
    run(
        &js,
        "mo2.observe($('a'), { attributes: true }); $('a').title = '5';",
    );
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("calls").unwrap().unwrap(), "1");
}

#[test]
fn removed_nodes_stay_observed_until_delivery() {
    let js = page("<div id=a><p id=p><b id=b></b></p></div>");
    run(
        &js,
        "observe($('a'), { childList: true, attributes: true, subtree: true });",
    );
    assert_eq!(
        records(
            &js,
            "globalThis.p = $('p'); globalThis.b = $('b');
             p.remove();
             b.title = 'still seen';
             b.append('x');",
        ),
        [
            "childList div#a +[] -[p#p] null|null",
            "attributes b#b title",
            "childList b#b +[#text:x] -[] null|null",
        ]
    );
    // After delivery the removed subtree is no longer observed.
    assert!(records(&js, "b.title = 'unseen';").is_empty());
    // Observing a detached node directly works as usual.
    run(&js, "observe(p, { attributes: true, subtree: true });");
    assert_eq!(records(&js, "b.lang = 'fr';"), ["attributes b#b lang"]);
}

#[test]
fn records_queued_by_callbacks_are_delivered_in_the_same_checkpoint() {
    let js = page("<div id=a></div><div id=b></div>");
    run(
        &js,
        r#"
        globalThis.seen = [];
        new MutationObserver((rs) => {
          seen.push('a:' + rs.length);
          if (seen.length < 3) $('b').title = String(seen.length);
        }).observe($('a'), { attributes: true });
        new MutationObserver((rs) => {
          seen.push('b:' + rs.length);
          $('a').title = 'again';
        }).observe($('b'), { attributes: true });
        $('a').title = 'start';
        "#,
    );
    js.perform_microtask_checkpoint();
    assert_eq!(
        js.eval_json("seen").unwrap().unwrap(),
        r#"["a:1","b:1","a:1"]"#
    );
}

#[test]
fn callback_errors_are_reported_and_delivery_continues() {
    let js = page("<div id=a></div>");
    run(
        &js,
        r#"
        globalThis.events = [];
        addEventListener('error', (e) => { events.push(e.message); e.preventDefault(); });
        new MutationObserver(() => { throw new Error('boom'); }).observe($('a'), { attributes: true });
        new MutationObserver(() => events.push('second')).observe($('a'), { attributes: true });
        $('a').title = 'x';
        "#,
    );
    js.perform_microtask_checkpoint();
    assert_eq!(
        js.eval_json("events").unwrap().unwrap(),
        r#"["Uncaught Error: boom","second"]"#
    );
    assert!(js.take_errors().is_empty());
}

#[test]
fn unobserved_changes_and_collected_observers() {
    let js = page("<div id=a></div>");
    // Changes nobody observes cost nothing and deliver nothing.
    assert!(
        records(
            &js,
            "for (let i = 0; i < 100; i++) $('a').append(document.createElement('i'));"
        )
        .is_empty()
    );
    // Observed detached nodes that become garbage take their registration
    // with them, and the node slots they free do not inherit it.
    run(
        &js,
        r#"
        globalThis.calls = 0;
        globalThis.keep = new MutationObserver(() => calls++);
        for (let i = 0; i < 50; i++) keep.observe(document.createElement('div'), { attributes: true, childList: true });
        "#,
    );
    js.run_gc();
    run(
        &js,
        "for (let i = 0; i < 200; i++) { const d = document.createElement('div'); d.title = 'x'; d.append('y'); $('a').append(d); }",
    );
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("calls").unwrap().unwrap(), "0");
}

#[test]
fn replacing_the_document_keeps_only_document_observers() {
    let js = page("<div id=a></div>");
    run(
        &js,
        "observe(document, { attributes: true, subtree: true }); observe($('a'), { attributes: true });",
    );
    js.set_document(Document::parse("<div id=b></div><div id=a></div>"));
    assert_eq!(
        records(&js, "$('b').title = 'x'; $('a').title = 'y';"),
        ["attributes div#b title", "attributes div#a title"]
    );
}

/// Random mutations under an observed body: replaying the records on a
/// mirror of the tree must reproduce the tree after every step, and every
/// attribute record's old value must be the value the mirror had.
#[test]
fn records_replay_to_the_tree() {
    let js = page("<body><div id=a><p>t</p></div><ul><li>1</li><li>2</li></ul></body>");
    run(
        &js,
        r#"
        let seed = 11;
        const rand = (n) => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed % n; };
        const body = document.body;
        const mo = new MutationObserver(() => {});
        mo.observe(body, { childList: true, attributes: true, attributeOldValue: true, subtree: true });
        // node -> [children], for the nodes the mirror knows to be in body.
        const kids = new Map();
        const attrs = new Map();
        const snapshot = (n) => {
          kids.set(n, [...n.childNodes]);
          if (n.nodeType === 1) attrs.set(n, new Map([...n.attributes].map((a) => [a.name, a.value])));
          for (const c of n.childNodes) snapshot(c);
        };
        snapshot(body);
        const apply = (r) => {
          const list = kids.get(r.target);
          if (list === undefined) return; // A removed node, still observed.
          if (r.type === 'attributes') {
            const m = attrs.get(r.target);
            const had = m.has(r.attributeName) ? m.get(r.attributeName) : null;
            if (had !== r.oldValue) throw new Error('old value ' + r.oldValue + ' but mirror had ' + had);
            const now = r.target.getAttribute(r.attributeName);
            if (now === null) m.delete(r.attributeName); else m.set(r.attributeName, now);
            return;
          }
          for (const n of r.removedNodes) {
            const i = list.indexOf(n);
            if (i < 0) throw new Error('removed node not a child');
            list.splice(i, 1);
          }
          let at = r.previousSibling === null ? 0 : list.indexOf(r.previousSibling) + 1;
          if (at === 0 && r.previousSibling !== null) throw new Error('previousSibling not a child');
          const expectedNext = list[at] === undefined ? null : list[at];
          if (r.addedNodes.length && expectedNext !== r.nextSibling) throw new Error('nextSibling mismatch');
          list.splice(at, 0, ...r.addedNodes);
          for (const n of r.addedNodes) snapshot(n);
        };
        const check = (n) => {
          const list = kids.get(n);
          const real = [...n.childNodes];
          if (list.length !== real.length || list.some((c, i) => c !== real[i])) throw new Error('children differ under ' + n.nodeName);
          if (n.nodeType === 1) {
            const m = attrs.get(n);
            if (m.size !== n.attributes.length || [...n.attributes].some((a) => m.get(a.name) !== a.value)) throw new Error('attributes differ on ' + n.nodeName);
          }
          real.forEach(check);
        };
        const pool = [body, ...body.querySelectorAll('*')];
        for (let i = 0; i < 6; i++) pool.push(document.createElement(i % 2 ? 'div' : 'span'));
        pool.push(document.createDocumentFragment());
        // Mostly nodes in the body, where the records come from.
        const pick = () => {
          if (rand(3) === 0) return pool[rand(pool.length)];
          const inBody = [body, ...body.querySelectorAll('*')];
          return inBody[rand(inBody.length)];
        };
        // Anything but the body, which stays where it is.
        const pickMovable = () => pool[1 + rand(pool.length - 1)];
        globalThis.step = () => {
          const b = pickMovable();
          let a = pick();
          const op = rand(14);
          if (op === 4 || op === 10 || op === 11) a = pickMovable();
          try {
            switch (op) {
              case 0: a.appendChild(b); break;
              case 1: a.insertBefore(b, a.firstChild); break;
              case 2: if (rand(3) === 0) b.remove(); break;
              case 3: a.innerHTML = '<i>x</i><b></b>y'; break;
              case 4: if (b.parentNode) b.parentNode.replaceChild(a, b); break;
              case 5: { const c = a.lastChild; if (c && c.nodeType === 1) pool.push(c); break; }
              case 6: a.textContent = rand(2) ? 'text' : ''; break;
              case 7: a.replaceChildren(b, 'z', b.cloneNode(true)); break;
              case 8: a.setAttribute('k' + rand(3), String(rand(5))); break;
              case 9: a.removeAttribute('k' + rand(3)); break;
              case 10: a.insertAdjacentHTML(['beforebegin', 'afterbegin', 'beforeend', 'afterend'][rand(4)], '<u></u>w'); break;
              case 11: a.outerHTML = '<s>o</s>'; break;
              case 12: a.append('m', 'n'); a.normalize(); break;
              case 13: a.classList.toggle('c'); break;
            }
          } catch (e) {
            if (!(e instanceof DOMException) && !(e instanceof TypeError)) throw e;
          }
          for (const r of mo.takeRecords()) apply(r);
          check(body);
          return op;
        };
        "#,
    );
    for i in 0..1500 {
        js.eval_json("step()")
            .unwrap_or_else(|e| panic!("step {i}: {e}"));
        if i % 100 == 0 {
            js.run_gc();
        }
    }
    assert!(js.take_errors().is_empty());
}

#[test]
fn one_observer_can_watch_many_nodes() {
    let js = page("<div id=a></div>");
    run(
        &js,
        r#"
        globalThis.count = 0;
        const mo = new MutationObserver((rs) => { count += rs.length; });
        const nodes = [];
        for (let i = 0; i < 5000; i++) {
          const d = document.createElement('div');
          nodes.push(d);
          mo.observe(d, { attributes: true });
        }
        for (const d of nodes) d.title = 'x';
        "#,
    );
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("count").unwrap().unwrap(), "5000");
    run(
        &js,
        "mo.disconnect(); for (const d of nodes) d.title = 'y';",
    );
    js.perform_microtask_checkpoint();
    assert_eq!(js.eval_json("count").unwrap().unwrap(), "5000");
}

/// The log so far, emptying it.
fn take_log(js: &JsRuntime) -> Vec<String> {
    js.take_console();
    run(
        js,
        "for (const line of log) console.log(line); log.length = 0;",
    );
    js.take_console().into_iter().map(|m| m.text).collect()
}

#[test]
fn parser_changes_are_recorded() {
    let js = page("");
    js.set_document(Document::new());
    let mut parser = js.with_document_mut(|doc| DocumentParser::new(doc, ParseOptions::default()));
    let step = |parser: &mut DocumentParser, chunk: &str| {
        parser.push(chunk);
        js.with_parser(parser, |p, doc| p.resume(doc))
    };
    let run_paused = |step: ParseStep| {
        let ParseStep::Script(script) = step else {
            panic!("expected a script, got {step:?}");
        };
        let source = js.with_document(|doc| doc.text_content(script).unwrap());
        js.run_script_element(script, &source, "https://example.com/")
            .unwrap();
    };

    // Before anything is observed, the parser logs nothing.
    let paused = step(
        &mut parser,
        "<!DOCTYPE html><html><head><script>
           observe(document, { childList: true, subtree: true, attributes: true,
                               characterData: true, characterDataOldValue: true });
         </script>",
    );
    assert!(parser.take_changes().is_empty());
    run_paused(paused);
    assert_eq!(take_log(&js), Vec::<String>::new());

    // Each step's records are delivered before it returns, one per node.
    assert_eq!(
        step(&mut parser, "</head><body class=a><p id=one>Hel"),
        ParseStep::NeedInput
    );
    assert_eq!(
        take_log(&js),
        [
            "childList html +[body] -[] head|null",
            "childList body +[p#one] -[] null|null",
            "childList p#one +[#text:Hel] -[] null|null",
        ]
    );

    // Text added to a text node from an earlier step is one characterData
    // record; repeated <body> attributes are attribute records.
    step(
        &mut parser,
        "lo &amp; bye</p><body id=b class=c><table>x<tr>",
    );
    assert_eq!(
        take_log(&js),
        [
            "characterData #text old=Hel",
            "attributes body#b id",
            "childList body#b +[table] -[] p#one|null",
            // Foster-parented text goes before the table.
            "childList body#b +[#text:x] -[] p#one|table",
            "childList table +[tbody] -[] null|null",
            "childList tbody +[tr] -[] null|null",
        ]
    );

    // A script sees the records for everything parsed before it, itself
    // included, before it runs.
    let paused = step(
        &mut parser,
        "</table><script>log.push('ran after ' + log.length)</script><template><b>",
    );
    run_paused(paused);
    assert_eq!(
        take_log(&js),
        [
            "childList body#b +[script] -[] table|null",
            "childList script +[#text:log.push('ran after ' + log.length)] -[] null|null",
            "ran after 2",
        ]
    );

    // Template contents are a tree of their own, not observed here.
    js.with_parser(&mut parser, |p, doc| p.finish(doc));
    assert_eq!(
        take_log(&js),
        ["childList body#b +[template] -[] script|null"]
    );
    assert!(js.take_errors().is_empty());
}
