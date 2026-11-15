//! Inline style, computed style, `DOMTokenList` and `dataset`: native host
//! objects (see the `host` module of `dom`). They must behave as the JS
//! versions they replaced, keep their parsed form in step with the
//! attributes however those change, and live and die with their element.

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

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
}

#[test]
fn inline_style_follows_the_attribute() {
    let js = page("<div id=a style='color: red'></div><div id=box></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const s = a.style;
        const r = [s.color, s.length];
        a.setAttribute('style', 'width: 1px; height: 2px');
        r.push(s.color, s.width, s.length, s[1]);
        a.removeAttribute('style');
        r.push(s.width, s.length, s.cssText);
        s.top = '3px';
        r.push(a.getAttribute('style'));
        a.style.cssText = 'left: 4px';
        r.push(s.top, s.left, a.getAttribute('style'));
        // A clone is a new element with its own style object.
        const c = a.cloneNode(true);
        r.push(c.style === s, c.style.left, c.style.length);
        c.style.left = '5px';
        r.push(s.left, c.getAttribute('style'));
        // innerHTML makes new elements, parsed from the markup.
        const box = document.getElementById('box');
        box.innerHTML = '<p style="margin: 0 !important">x</p>';
        const p = box.firstChild;
        r.push(p.style.margin, p.style.getPropertyPriority('margin'));
        box.innerHTML = '<p style="margin: 1px">x</p>';
        r.push(p.style.margin, box.firstChild.style.margin, box.firstChild.style === p.style);
        // The parser and outerHTML replace elements too.
        p.outerHTML = '<p>y</p>';
        r.push(p.style.margin);
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["red",1,"","1px",2,"height","",0,"","top: 3px;","","4px","left: 4px;",false,"4px",1,"4px","left: 5px;","0","important","0","1px",false,"0"]"#
    );
}

#[test]
fn class_list_follows_the_attribute() {
    let js = page("<div id=a class='x y'></div><div id=box></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const c = a.classList;
        const r = [c.length, c[1], c.contains('x')];
        a.className = 'p q r';
        r.push(c.length, c[2], c.contains('x'), c.contains('q'), [...c]);
        a.setAttribute('class', 'one');
        r.push(c.value, c.length, c[0], c[1]);
        a.removeAttribute('class');
        r.push(c.length, c.value, c.contains('one'));
        c.add('z');
        r.push(a.className);
        const clone = a.cloneNode();
        r.push(clone.classList === c, clone.classList.contains('z'));
        clone.classList.add('w');
        r.push(a.className, clone.className);
        const box = document.getElementById('box');
        box.innerHTML = '<i class="m n"></i>';
        r.push(box.firstChild.classList.contains('n'), box.firstChild.classList.length);
        a.classList.value = 'v  v w';
        r.push(c.length, c.toString(), String(c));
        r
        "#,
    );
    assert_eq!(
        out,
        r#"[2,"y",true,3,"r",false,true,["p","q","r"],"one",1,"one",null,0,"",false,"z",false,true,"z","z w",true,2,2,"v  v w","v  v w"]"#
    );
}

#[test]
fn style_declaration_api() {
    let js = page("<div id=a style='color: red; --Custom: 1; margin-top: 4px !important'></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const s = a.style;
        const r = [s === a.style, s instanceof CSSStyleDeclaration, Object.prototype.toString.call(s)];
        r.push(s.length, s[0], s[2], s[3], s.item(1), s.item(9), s.item('2'));
        r.push(s.marginTop, s['margin-top'], s.getPropertyValue('MARGIN-TOP'), s.getPropertyValue('--Custom'),
          s.getPropertyValue('--custom'), s.getPropertyPriority('margin-top'), s.getPropertyPriority('color'));
        r.push('display' in s, 'margin-top' in s, 'foo$' in s, s.display, s.fooBar, s['foo$']);
        r.push(Object.keys(s), [...s], JSON.stringify(Object.getOwnPropertyDescriptor(s, 0)));
        r.push(Object.getOwnPropertyDescriptor(s, 'zIndex') !== undefined);
        // Keys that are not CSS-like are ordinary properties; CSS-like ones
        // only if defined as such.
        s.$expando = { x: 1 };
        Object.defineProperty(s, 'color', { value: 'own', configurable: true });
        r.push(a.style.$expando.x, s.color, s.getPropertyValue('color'));
        delete s.color;
        r.push(s.color);
        s.setProperty('Padding-Left', ' 2px ', 'IMPORTANT');
        s.setProperty('margin-top', '5px');
        s.setProperty('--x', 'y');
        s.cssFloat = 'left';
        s.webkitTransform = 'none';
        s.MozAppearance = 'none';
        s['border-width'] = 1;
        s[0] = 'ignored';
        r.push(a.getAttribute('style'));
        r.push(s.removeProperty('padding-left'), s.removeProperty('nothing'));
        s.setProperty('color', null);
        s.setProperty('--x', '');
        s.cssFloat = undefined;
        r.push(s.cssText);
        s.cssText = null;
        r.push(a.getAttribute('style'), s.length);
        try { CSSStyleDeclaration.prototype.item.call({}, 0); } catch (e) { r.push(e.message); }
        try { new CSSStyleDeclaration(); } catch (e) { r.push(e.message); }
        r.push(s.parentRule, s.setProperty.length, Object.getOwnPropertyDescriptor(CSSStyleDeclaration.prototype, 'cssText').set.name);
        r
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[true,true,"[object CSSStyleDeclaration]",3,"color","margin-top",null,"--Custom","","margin-top","#,
            r#""4px","4px","4px","1","","important","",true,true,false,"","",null,"#,
            r#"["0","1","2"],["color","--Custom","margin-top"],"{\"value\":\"color\",\"writable\":false,\"enumerable\":true,\"configurable\":true}","#,
            r#"true,1,"own","red","red","#,
            r#""color: red; --Custom: 1; margin-top: 5px; padding-left: 2px !important; --x: y; float: left; -webkit-transform: none; -moz-appearance: none; border-width: 1;","#,
            r#""2px","","--Custom: 1; margin-top: 5px; -webkit-transform: none; -moz-appearance: none; border-width: 1;","","#,
            r#"0,"Illegal invocation","Illegal constructor",null,2,"set cssText"]"#
        )
    );
}

#[test]
fn computed_style_resolves_each_read_and_stays_live() {
    let js = page(
        "<div id=a style='color: blue; --v: 3'></div><p id=h hidden></p>\
         <span id=s style='visibility: hidden'></span><li id=l></li>",
    );
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const cs = getComputedStyle(a);
        const r = [cs === getComputedStyle(a), cs === a.style, cs instanceof CSSStyleDeclaration];
        r.push(cs.display, cs.color, cs.opacity, cs.width, cs.height, cs.getPropertyValue('--v'),
          cs.marginLeft, cs['border-top-width'], cs.fontFamily, cs.unknownThing, cs.getPropertyPriority('color'));
        r.push(cs.length, cs[0], cs[52], cs.item(8), cs.cssText.slice(0, 40));
        a.style.display = 'flex';
        a.style.color = '';
        r.push(cs.display, cs.color);
        a.hidden = true;
        r.push(cs.display, cs.width);
        a.hidden = false;
        a.setAttribute('style', 'visibility: hidden');
        r.push(cs.display, cs.visibility);
        r.push(getComputedStyle(document.getElementById('h')).display,
          getComputedStyle(document.getElementById('s')).display,
          getComputedStyle(document.getElementById('l')).display,
          getComputedStyle(document.createElement('span')).display,
          getComputedStyle(document.createElementNS('http://www.w3.org/2000/svg', 'svg')).display);
        for (const f of [() => cs.setProperty('color', 'red'), () => { cs.color = 'red'; }, () => { cs.cssText = ''; },
          () => cs.removeProperty('display')]) {
          try { f(); r.push('no error'); } catch (e) { r.push(e.name); }
        }
        r.push(cs.removeProperty('not-listed'));
        for (const v of [document, null, {}, document.createTextNode('x')]) {
          try { getComputedStyle(v); } catch (e) { r.push(e.constructor.name); }
        }
        r.push(getComputedStyle.length, getComputedStyle.name, getComputedStyle(a, '::before') === cs);
        r
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[true,false,true,"block","blue","1","1920px","20px","3","0px","0px","Arial, sans-serif","","",53,"display","--v","width","display: block; visibility: visible; opa","#,
            r#""flex","rgb(0, 0, 0)","none","0px","block","hidden","none","inline","list-item","inline","inline","#,
            r#""NoModificationAllowedError","NoModificationAllowedError","NoModificationAllowedError","NoModificationAllowedError","","#,
            r#""TypeError","TypeError","TypeError","TypeError",1,"getComputedStyle",true]"#
        )
    );
}

#[test]
fn token_list_api() {
    let js = page("<div id=a class='x y'></div><link id=l rel='stylesheet icon'>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const c = a.classList;
        const r = [c === a.classList, Object.prototype.toString.call(c), c[0], c[2], c['1'], 'length' in c, 1 in c, 2 in c];
        r.push(Object.keys(c), JSON.stringify(c), [...c.keys()], [...c.entries()], [...c.values()]);
        const seen = [];
        c.forEach(function (t, i, list) { seen.push(t, i, list === c, this.k); }, { k: 'this' });
        r.push(seen);
        c[0] = 'nope';
        r.push(c[0]);
        c.add('z', 'x', 'z');
        c.remove('y', 'missing');
        r.push(a.className, c.toggle('q'), c.toggle('q'), c.toggle('z', true), c.toggle('w', false), c.replace('x', 'z'), c.replace('m', 'n'), a.className);
        for (const f of [() => c.add(''), () => c.add('a b'), () => c.toggle(' '), () => c.replace('z', ''), () => c.supports('x')]) {
          try { f(); r.push('no error'); } catch (e) { r.push(e.name + ': ' + e.message); }
        }
        const rel = document.getElementById('l').relList;
        r.push(rel.contains('icon'), rel.supports('StyleSheet'), rel.supports('nope'), rel === document.getElementById('l').relList);
        c.add({ toString() { return 'obj'; } });
        r.push(c.contains({ toString() { return 'obj'; } }), c.item({ valueOf() { return 1; } }), c.item(-1), c.contains());
        try { DOMTokenList.prototype.contains.call(a.style, 'x'); } catch (e) { r.push(e.message); }
        // Methods do not depend on what page script does to the prototype.
        const length = Object.getOwnPropertyDescriptor(DOMTokenList.prototype, 'length');
        Object.defineProperty(DOMTokenList.prototype, 'length', { get() { return 0; }, configurable: true });
        r.push([...c].length);
        Object.defineProperty(DOMTokenList.prototype, 'length', length);
        r
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[true,"[object DOMTokenList]","x",null,"y",true,true,false,["0","1"],"{\"0\":\"x\",\"1\":\"y\"}",[0,1],[[0,"x"],[1,"y"]],["x","y"],"#,
            r#"["x",0,true,"this","y",1,true,"this"],"x","x z",true,false,true,false,true,false,"z","#,
            r#""SyntaxError: Failed to execute 'add' on 'DOMTokenList': The token provided must not be empty.","#,
            r#""InvalidCharacterError: Failed to execute 'add' on 'DOMTokenList': The token provided ('a b') contains HTML space characters, which are not valid in tokens.","#,
            r#""InvalidCharacterError: Failed to execute 'toggle' on 'DOMTokenList': The token provided (' ') contains HTML space characters, which are not valid in tokens.","#,
            r#""SyntaxError: Failed to execute 'replace' on 'DOMTokenList': The token provided must not be empty.","#,
            r#""TypeError: Failed to execute 'supports' on 'DOMTokenList': DOMTokenList has no supported tokens.","#,
            r#"true,true,false,true,true,"obj",null,false,"Illegal invocation",2]"#
        )
    );
}

#[test]
fn rel_lists_and_sandbox_answer_supports() {
    // Chrome answers `supports` on the relList of a, area and form (with
    // the rel keywords that change what it does) and on iframe.sandbox
    // instead of throwing; reCAPTCHA asks the sandbox on many pages.
    let js = page("<a id=a rel='noopener nofollow'></a><form id=f></form>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a').relList;
        const f = document.getElementById('f').relList;
        const area = document.createElement('area').relList;
        [a.supports('noopener'), a.supports('NoReferrer'), a.supports('nofollow'), a.contains('nofollow'),
         f.supports('opener'), f.supports('stylesheet'), area.supports('noreferrer'),
         document.createElement('link').relList.supports('preload'),
         document.createElement('iframe').sandbox.supports('allow-scripts'),
         document.createElement('iframe').sandbox.supports('allow-everything')]
        "#,
    );
    assert_eq!(
        out,
        "[true,true,false,true,true,false,true,true,true,false]"
    );
}

#[test]
fn dataset_api() {
    // The parser lower-cases `data-Upper`; `d['user-id']` reads
    // `data-user-id` as the JS version did.
    let js = page("<div id=a data-user-id=7 data-x='' data-Upper=1></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const d = a.dataset;
        const r = [d === a.dataset, Object.prototype.toString.call(d), d.userId, d.x, d.missing, d['user-id'], d.upper];
        r.push('userId' in d, 'x' in d, 'nope' in d, d.hasOwnProperty('userId'), Object.keys(d), JSON.stringify(d));
        r.push(JSON.stringify(Object.getOwnPropertyDescriptor(d, 'userId')));
        d.fooBar = 12;
        d.x = null;
        d[3] = 'three';
        Object.defineProperty(d, 'viaDefine', { value: 'v' });
        r.push(a.getAttribute('data-foo-bar'), a.getAttribute('data-x'), a.getAttribute('data-3'), d.viaDefine);
        delete d.userId;
        delete d.never;
        r.push(a.hasAttribute('data-user-id'), Object.keys(d));
        try { d['a-b'] = 1; } catch (e) { r.push(e.name); }
        try { d['a b'] = 1; } catch (e) { r.push(e.name); }
        const sym = Symbol('s');
        r.push(Reflect.set(d, sym, 1), d[sym], Reflect.defineProperty(d, 'acc', { get() { return 1; } }), d.acc);
        r.push(typeof d.toString, d.constructor === DOMStringMap);
        a.setAttribute('data-to-string', 'shadowed');
        r.push(d.toString);
        r
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[true,"[object DOMStringMap]","7","",null,"7","1",true,true,false,true,["userId","x","upper"],"{\"userId\":\"7\",\"x\":\"\",\"upper\":\"1\"}","#,
            r#""{\"value\":\"7\",\"writable\":true,\"enumerable\":true,\"configurable\":true}","12","null","three","v","#,
            r#"false,["x","upper","fooBar","3","viaDefine"],"SyntaxError","InvalidCharacterError",false,null,false,null,"function",true,"shadowed"]"#
        )
    );
}

#[test]
fn writes_are_observed_once_each() {
    let js = page("<div id=a class=x style='color: red'></div>");
    run(
        &js,
        r#"
        window.records = [];
        const a = document.getElementById('a');
        new MutationObserver((list) => {
          for (const m of list) records.push([m.attributeName, m.oldValue]);
        }).observe(a, { attributes: true, attributeOldValue: true });
        a.style.setProperty('width', '1px');
        a.style.color = 'blue';
        a.style.cssText = 'top: 0';
        a.style.removeProperty('top');
        a.style.removeProperty('top');
        a.classList.add('y', 'z');
        a.classList.toggle('x');
        a.classList.remove('none');
        a.dataset.k = 'v';
        delete a.dataset.k;
        "#,
    );
    assert_eq!(
        eval(&js, "records"),
        concat!(
            r#"[["style","color: red"],["style","color: red; width: 1px;"],["style","color: blue; width: 1px;"],"#,
            r#"["style","top: 0;"],["class","x"],["class","x y z"],["class","y z"],["data-k",null],["data-k","v"]]"#
        )
    );
}

#[test]
fn custom_elements_see_the_writes() {
    let js = page("");
    let out = eval(
        &js,
        r#"
        const seen = [];
        customElements.define('x-s', class extends HTMLElement {
          static observedAttributes = ['style', 'class'];
          attributeChangedCallback(name, old, value) {
            // Reading back from inside the reaction sees the new value.
            seen.push(name, old, value, this.style.length, this.classList.length);
          }
        });
        const el = document.createElement('x-s');
        el.style.color = 'red';
        el.classList.add('a');
        seen
        "#,
    );
    assert_eq!(
        out,
        r#"["style",null,"color: red;",1,0,"class",null,"a",1,1]"#
    );
}

#[test]
fn host_objects_live_and_die_with_their_element() {
    let js = page("");
    let nodes = |js: &JsRuntime| js.with_document(Document::len);
    let base = nodes(&js);
    run(
        &js,
        r#"
        window.d = document.createElement('div');
        d.innerHTML = '<p class=a style="color: red" data-x=1>x</p>'.repeat(50);
        for (const p of d.children) {
          p.style.$expando = p.classList.length;
          p.dataset.y = '2';
          getComputedStyle(p).color;
        }
        window.style = d.firstChild.style;
        "#,
    );
    js.run_gc();
    assert!(nodes(&js) >= base + 100);
    run(&js, "delete window.d;");
    js.run_gc();
    // The style holds its element, and so the whole tree.
    assert!(nodes(&js) >= base + 100);
    assert_eq!(
        eval(&js, "[style.$expando, style.color, style.length]"),
        "[1,\"red\",1]"
    );
    run(&js, "delete window.style;");
    js.run_gc();
    assert!(
        nodes(&js) <= base + 1,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}

#[test]
fn host_objects_of_a_replaced_document_throw() {
    let js = page("<div id=a style='color: red' class=x data-k=v></div>");
    run(
        &js,
        "window.old = document.getElementById('a'); window.s = old.style; window.c = old.classList; \
         window.d = old.dataset; window.cs = getComputedStyle(old);",
    );
    js.set_document(Document::parse("<p></p>"));
    let out = eval(
        &js,
        r#"
        const r = [];
        for (const f of [() => s.color, () => s.length, () => c.contains('x'), () => c[0], () => d.k,
          () => cs.display, () => s.setProperty('a', 'b')]) {
          try { f(); r.push('no error'); } catch (e) { r.push(e.name); }
        }
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["InvalidStateError","InvalidStateError","InvalidStateError","InvalidStateError","InvalidStateError","InvalidStateError","InvalidStateError"]"#
    );
}

#[test]
fn layout_getters_agree_with_the_boxes() {
    let js = page("<div id=a>x</div><p id=h hidden>y</p><span id=s style='display: none'></span>");
    let out = eval(
        &js,
        r#"
        const r = [];
        for (const id of ['a', 'h', 's']) {
          const el = document.getElementById(id);
          const b = el.getBoundingClientRect();
          r.push([el.offsetLeft, el.offsetTop, el.offsetWidth, el.offsetHeight, el.clientWidth, el.clientHeight,
            el.scrollWidth, el.scrollHeight].join(), [b.x, b.y, b.width, b.height].join());
        }
        const html = document.documentElement;
        r.push(html.clientWidth === innerWidth, html.clientHeight === innerHeight, document.body.offsetWidth === innerWidth,
          document.body.offsetHeight >= innerHeight);
        const a = document.getElementById('a');
        a.hidden = true;
        r.push(a.offsetWidth, a.clientHeight);
        a.hidden = false;
        r.push(a.offsetWidth > 0);
        const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
        r.push('offsetWidth' in svg, svg.clientWidth);
        document.body.append(svg);
        r.push(svg.clientWidth > 0);
        try { Object.getOwnPropertyDescriptor(Element.prototype, 'clientWidth').get.call(document); } catch (e) { r.push(e.message); }
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["0,0,1920,20,1920,20,1920,20","0,0,1920,20","0,0,0,0,0,0,0,0","0,0,0,0","0,0,0,0,0,0,0,0","0,0,0,0",true,true,true,true,0,0,true,false,0,true,"Illegal invocation"]"#
    );
}
