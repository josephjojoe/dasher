//! DOM bindings: nodes, the tree, attributes, collections, selectors,
//! style, events and wrapper lifetimes.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions, WindowOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/dir/page.html".into(),
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
}

fn json(value: &str) -> String {
    format!("{value:?}")
}

#[test]
fn node_types_and_interfaces() {
    let js = page(
        "<!doctype html><title>T</title><div id=a><p>hi<!--c--></p><svg><circle/></svg><x-foo></x-foo><blink></blink></div>",
    );
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const p = a.firstChild;
        [
          document instanceof Document, document instanceof HTMLDocument, document instanceof Node,
          document instanceof EventTarget, document.nodeType, document.nodeName,
          document.doctype.name, document.doctype instanceof DocumentType,
          a instanceof HTMLDivElement, a instanceof HTMLElement, a instanceof Element,
          a.tagName, a.localName, a.namespaceURI, a.nodeName,
          p instanceof HTMLParagraphElement, p.firstChild instanceof Text, p.firstChild.nodeType,
          p.lastChild instanceof Comment, p.lastChild.data, p.lastChild.nodeName,
          a.querySelector('svg') instanceof SVGSVGElement, a.querySelector('circle') instanceof SVGElement,
          a.querySelector('circle').namespaceURI, a.querySelector('circle').tagName,
          a.querySelector('x-foo').constructor === HTMLElement,
          a.querySelector('blink') instanceof HTMLUnknownElement,
          Object.prototype.toString.call(a), String(document.createDocumentFragment()),
          document.documentElement.tagName, document.head.tagName, document.body.tagName,
          document.ownerDocument, a.ownerDocument === document, Node.ELEMENT_NODE, a.TEXT_NODE,
        ]
        "#,
    );
    assert_eq!(
        out,
        r##"[true,true,true,true,9,"#document","html",true,true,true,true,"DIV","div","http://www.w3.org/1999/xhtml","DIV",true,true,3,true,"c","#comment",true,true,"http://www.w3.org/2000/svg","circle",true,true,"[object HTMLDivElement]","[object DocumentFragment]","HTML","HEAD","BODY",null,true,1,3]"##
    );
}

#[test]
fn wrappers_keep_their_identity_and_expandos() {
    let js = page("<ul><li>1</li><li>2</li></ul>");
    run(
        &js,
        r#"
        let ul = document.querySelector('ul');
        ul.firstChild.expando = { n: 1 };
        document.body.lastChild.marker = 'body-child';
        "#,
    );
    js.run_gc();
    let out = eval(
        &js,
        r#"
        ul = document.querySelector('ul');
        [
          ul === document.body.firstChild, ul.firstChild === ul.childNodes[0],
          ul.firstChild.expando.n, document.body.lastChild.marker,
          ul.children[1] === ul.lastElementChild, document.querySelectorAll('li')[1] === ul.lastChild,
          document === window.document, ul.parentNode.parentNode === document.documentElement,
        ]
        "#,
    );
    assert_eq!(out, "[true,true,1,\"body-child\",true,true,true,true]");
}

#[test]
fn tree_mutation() {
    let js = page("<div id=a><span>1</span><span>2</span></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const [s1, s2] = a.children;
        const b = document.createElement('B');
        a.insertBefore(b, s2);
        const order1 = a.innerHTML;
        a.appendChild(s1);
        const order2 = a.innerHTML;
        a.replaceChild(document.createTextNode('t'), b);
        const order3 = a.innerHTML;
        a.removeChild(s2);
        s2.remove();
        a.prepend('x', s2);
        a.append(document.createComment('end'));
        const order4 = a.innerHTML;
        s1.before('<', document.createElement('i'));
        s1.after('>');
        s1.replaceWith('S1');
        const order5 = a.innerHTML;
        a.replaceChildren(s1, 'k');
        const order6 = a.outerHTML;
        const frag = document.createDocumentFragment();
        frag.append(document.createElement('em'), 'f');
        a.appendChild(frag);
        [order1, order2, order3, order4, order5, order6, a.innerHTML, frag.childNodes.length,
         s2.parentNode, b.parentNode, a.childElementCount, a.contains(s1), a.contains(a), a.contains(null)]
        "#,
    );
    assert_eq!(
        out,
        r#"["<span>1</span><b></b><span>2</span>","<b></b><span>2</span><span>1</span>","t<span>2</span><span>1</span>","x<span>2</span>t<span>1</span><!--end-->","x<span>2</span>t&lt;<i></i>S1&gt;<!--end-->","<div id=\"a\"><span>1</span>k</div>","<span>1</span>k<em></em>f",0,null,null,2,true,true,false]"#
    );
}

#[test]
fn mutation_errors_are_dom_exceptions() {
    let js = page("<div id=a><span></span></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const errors = [];
        const attempt = (f) => { try { f(); errors.push('none'); } catch (e) { errors.push(e instanceof DOMException ? e.name + ':' + e.code : e.constructor.name); } };
        attempt(() => a.firstChild.appendChild(a));
        attempt(() => a.removeChild(document.body));
        attempt(() => a.insertBefore(document.createElement('i'), document.body));
        attempt(() => document.appendChild(document.createElement('html')));
        attempt(() => document.createElement('1bad'));
        attempt(() => a.setAttribute('a b', 'x'));
        attempt(() => a.appendChild('text'));
        attempt(() => document.querySelector('div['));
        attempt(() => a.querySelector('##'));
        attempt(() => document.documentElement.outerHTML = '<p>');
        attempt(() => new Document().appendChild(document.createTextNode('x')));
        attempt(() => new Node());
        attempt(() => a.insertAdjacentHTML('nowhere', 'x'));
        errors
        "#,
    );
    assert_eq!(
        out,
        r#"["HierarchyRequestError:3","NotFoundError:8","NotFoundError:8","HierarchyRequestError:3","InvalidCharacterError:5","InvalidCharacterError:5","TypeError","SyntaxError:12","SyntaxError:12","NoModificationAllowedError:7","HierarchyRequestError:3","TypeError","SyntaxError:12"]"#
    );
}

#[test]
fn html_and_text_content() {
    let js = page("<div id=a>old <b>bold</b></div><template id=t><p>in template</p></template>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const results = [a.textContent, a.innerHTML];
        a.innerHTML = '<ul><li>1<li>2</ul><script>window.ran = true</script>';
        results.push(a.querySelectorAll('li').length, a.firstChild.outerHTML, typeof window.ran);
        a.textContent = 'plain <text>';
        results.push(a.innerHTML, a.childNodes.length);
        a.firstChild.outerHTML;
        a.insertAdjacentHTML('beforeend', '<i>i</i>');
        a.insertAdjacentHTML('afterbegin', '<u>u</u>');
        a.insertAdjacentHTML('beforebegin', '<hr>');
        results.push(a.innerHTML, a.previousSibling.tagName);
        a.querySelector('i').outerHTML = '<s>s</s><s>t</s>';
        results.push(a.innerHTML);
        const t = document.getElementById('t');
        results.push(t.content instanceof DocumentFragment, t.content.firstChild.textContent, t.innerHTML, t.childNodes.length);
        t.innerHTML = '<b>new</b>';
        results.push(t.content.firstChild.tagName);
        a.textContent = '';
        results.push(a.hasChildNodes(), document.createTextNode('x').textContent, document.textContent);
        results
        "#,
    );
    assert_eq!(
        out,
        r#"["old bold","old <b>bold</b>",2,"<ul><li>1</li><li>2</li></ul>","undefined","plain &lt;text&gt;",1,"<u>u</u>plain &lt;text&gt;<i>i</i>","HR","<u>u</u>plain &lt;text&gt;<s>s</s><s>t</s>",true,"in template","<p>in template</p>",0,"B",false,"x",null]"#
    );
}

#[test]
fn inner_text_follows_rendering() {
    let js = page(
        "<div id=a><p>one   two</p><p>three<br>four</p><script>x</script><span hidden>no</span></div>",
    );
    assert_eq!(
        eval(&js, "document.getElementById('a').innerText"),
        json("one two\n\nthree\nfour")
    );
    let out = eval(
        &js,
        "const a = document.getElementById('a'); a.innerText = 'x\\ny'; [a.innerHTML, a.innerText]",
    );
    assert_eq!(out, r#"["x<br>y","x\ny"]"#);
}

#[test]
fn attributes() {
    let js = page(
        "<div id=a class='x  y' data-user-id=7 data-x=1 title=t></div><svg viewBox='0 0 1 1'></svg>",
    );
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const r = [a.getAttribute('ID'), a.getAttribute('missing'), a.hasAttribute('TITLE'), a.getAttributeNames()];
        a.setAttribute('Data-New', 'v');
        r.push(a.getAttribute('data-new'), a.attributes.length, a.attributes[0].name, a.attributes.title.value,
          [...a.attributes].map((x) => x.name + '=' + x.value).join(' '));
        a.removeAttribute('title');
        r.push(a.toggleAttribute('hidden'), a.hidden, a.toggleAttribute('hidden'), a.hidden);
        a.id = 'b';
        r.push(document.getElementById('b') === a, a.id);
        const attr = a.getAttributeNode('class');
        attr.value = 'z';
        r.push(a.className, attr.ownerElement === a, attr instanceof Attr);
        const svg = document.querySelector('svg');
        r.push(svg.getAttribute('viewBox'), svg.getAttribute('viewbox'));
        a.setAttributeNS('http://www.w3.org/1999/xlink', 'xlink:href', '#x');
        r.push(a.getAttributeNS('http://www.w3.org/1999/xlink', 'href'), a.hasAttributeNS(null, 'id'));
        r
        "#,
    );
    assert_eq!(
        out,
        r##"["a",null,true,["id","class","data-user-id","data-x","title"],"v",6,"id","t","id=a class=x  y data-user-id=7 data-x=1 title=t data-new=v",true,true,false,false,true,"b","z",true,true,"0 0 1 1",null,"#x",true]"##
    );
}

#[test]
fn class_list_and_dataset() {
    let js = page("<div id=a class='x  y' data-user-id=7></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const c = a.classList;
        const r = [c.length, c[0], c.contains('y'), c.value, c === a.classList];
        c.add('z', 'x');
        c.remove('y');
        r.push(a.className, c.toggle('q'), c.toggle('q'), c.toggle('z', true), c.replace('x', 'w'), a.className, [...c]);
        try { c.add('a b'); } catch (e) { r.push(e.name); }
        const d = a.dataset;
        r.push(d.userId, d.missing, 'userId' in d, Object.keys(d));
        d.fooBar = 'baz';
        delete d.userId;
        r.push(a.getAttribute('data-foo-bar'), a.hasAttribute('data-user-id'), JSON.stringify(d));
        r
        "#,
    );
    assert_eq!(
        out,
        r#"[2,"x",true,"x  y",true,"x z",true,false,true,true,"w z",["w","z"],"InvalidCharacterError","7",null,true,["userId"],"baz",false,"{\"fooBar\":\"baz\"}"]"#
    );
}

#[test]
fn inline_style() {
    let js = page("<div id=a style='color: red; margin-top:4px !important'></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const s = a.style;
        const r = [s.color, s.marginTop, s.getPropertyPriority('margin-top'), s.length, s[0], s.display, 'display' in s];
        s.display = 'none';
        s.backgroundColor = 'blue';
        s.setProperty('--custom', ' 1px ');
        s.color = '';
        s.WebkitTransform = 'none';
        r.push(a.getAttribute('style'), s.cssText === a.getAttribute('style'), s.getPropertyValue('--custom'));
        a.setAttribute('style', 'width: 10px');
        r.push(s.width, s.display, s.length);
        s.cssText = 'float: left';
        r.push(s.cssFloat, a.getAttribute('style'), s.removeProperty('float'), a.getAttribute('style'));
        a.style = 'top: 1px';
        r.push(s.top, s === a.style);
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["red","4px","important",2,"color","",true,"margin-top: 4px !important; display: none; background-color: blue; --custom: 1px; -webkit-transform: none;",true,"1px","10px","",1,"left","float: left;","left","","1px",true]"#
    );
}

#[test]
fn selectors_and_collections() {
    let js = page(
        "<ul id=l><li class=a>1</li><li class='a b'>2</li><li name=n>3</li></ul><form name=f id=f><input name=q></form>",
    );
    let out = eval(
        &js,
        r#"
        const l = document.getElementById('l');
        const lis = l.getElementsByTagName('li');
        const as = document.getElementsByClassName('a');
        const kids = l.childNodes;
        const qsa = document.querySelectorAll('li');
        const r = [lis.length, as.length, kids.length, qsa.length, qsa instanceof NodeList, lis instanceof HTMLCollection,
          lis[2].textContent, lis.item(0).textContent, lis[5], l.children === l.children, kids === l.childNodes];
        const li = document.createElement('li');
        li.className = 'a';
        l.appendChild(li);
        r.push(lis.length, as.length, kids.length, qsa.length);
        l.firstChild.remove();
        r.push(lis.length, as.length, [...lis].map((x) => x.textContent).join(','), Array.from(kids).length);
        r.push(l.querySelector(':scope > .b').textContent, l.querySelector('li:nth-child(2)').textContent,
          l.lastChild.matches('ul > li.a'), l.lastChild.closest('ul') === l, l.lastChild.closest('form'));
        r.push(document.getElementsByName('n').length, document.forms.f.id, document.forms[0].elements.q.name,
          document.forms.length, 'f' in document.forms, document.all.length > 5);
        qsa.forEach((x, i) => { if (i === 0) r.push(x.textContent); });
        r.push(Array.prototype.slice.call(lis).length, Object.keys(lis).join());
        r
        "#,
    );
    assert_eq!(
        out,
        r#"[3,2,3,3,true,true,"3","1",null,true,true,4,3,4,3,3,2,"2,3,",3,"2","3",true,true,null,1,"f","q",1,true,true,"1",3,"0,1,2"]"#
    );
}

#[test]
fn urls_resolve_against_the_document_and_base() {
    let js = page("<a id=a href='../x?q=1#h'>link</a><img id=i src='img.png'>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const r = [a.href, a.pathname, a.search, a.hash, a.host, a.protocol, document.getElementById('i').src,
          String(a), document.URL, document.baseURI];
        const base = document.createElement('base');
        base.href = 'https://cdn.example.org/assets/';
        document.head.appendChild(base);
        r.push(a.href, document.baseURI, base.href);
        a.search = '?z=2';
        r.push(a.getAttribute('href'));
        r
        "#,
    );
    assert_eq!(
        out,
        r##"["https://example.com/x?q=1#h","/x","?q=1","#h","example.com","https:","https://example.com/dir/img.png","https://example.com/x?q=1#h","https://example.com/dir/page.html","https://example.com/dir/page.html","https://cdn.example.org/x?q=1#h","https://cdn.example.org/assets/","https://cdn.example.org/assets/","https://cdn.example.org/x?z=2#h"]"##
    );
}

#[test]
fn events_capture_bubble_and_cancel() {
    let js = page("<div id=outer><button id=b>go</button></div>");
    let out = eval(
        &js,
        r#"
        const log = [];
        const outer = document.getElementById('outer');
        const b = document.getElementById('b');
        window.addEventListener('ping', (e) => log.push('window-capture:' + e.eventPhase), true);
        document.addEventListener('ping', (e) => log.push('document-bubble:' + (e.currentTarget === document)));
        outer.addEventListener('ping', (e) => log.push('outer-capture'), { capture: true });
        outer.addEventListener('ping', (e) => log.push('outer-bubble'));
        b.addEventListener('ping', (e) => { log.push('target:' + e.eventPhase + ':' + (e.target === b)); e.preventDefault(); });
        window.addEventListener('ping', (e) => log.push('window-bubble:' + e.composedPath().length));
        const notCanceled = b.dispatchEvent(new CustomEvent('ping', { bubbles: true, cancelable: true, detail: 42 }));
        log.push('result:' + notCanceled);
        outer.addEventListener('stop', (e) => { log.push('stopper'); e.stopPropagation(); });
        document.addEventListener('stop', () => log.push('never'));
        b.dispatchEvent(new Event('stop', { bubbles: true }));
        b.dispatchEvent(new Event('flat'));
        let clicked = 0;
        b.onclick = (e) => { clicked++; return e instanceof MouseEvent; };
        b.click();
        log.push('clicked:' + clicked);
        const ev = document.createEvent('Event');
        ev.initEvent('legacy', true, true);
        document.body.addEventListener('legacy', () => log.push('legacy'));
        b.dispatchEvent(ev);
        log
        "#,
    );
    assert_eq!(
        out,
        r#"["window-capture:1","outer-capture","target:2:true","outer-bubble","document-bubble:true","window-bubble:6","result:false","stopper","clicked:1","legacy"]"#
    );
}

#[test]
fn form_controls_keep_their_state() {
    let js = page(
        "<form id=f action=/submit><input id=t name=t value=init><input id=c type=checkbox checked>\
         <input type=radio name=r id=r1 checked><input type=radio name=r id=r2>\
         <select id=s><option>a<option value=B selected>b<option>c</select>\
         <textarea id=ta>hello</textarea><button id=go>go</button></form>",
    );
    let out = eval(
        &js,
        r#"
        const $ = (id) => document.getElementById(id);
        const r = [$('t').value, $('t').defaultValue, $('t').type, $('c').checked, $('s').value, $('s').selectedIndex,
          $('s').options.length, $('ta').value, $('f').action, $('f').method, $('go').type, $('t').form === $('f')];
        $('t').value = 'typed';
        r.push($('t').value, $('t').getAttribute('value'));
        $('c').click();
        r.push($('c').checked, $('c').hasAttribute('checked'));
        $('r2').checked = true;
        r.push($('r1').checked, $('r2').checked);
        $('s').value = 'c';
        r.push($('s').selectedIndex, $('s').options[2].selected, $('s').selectedOptions[0].text);
        $('s').selectedIndex = 0;
        r.push($('s').value);
        $('ta').value = 'edited';
        r.push($('ta').value, $('ta').textContent);
        let submitted = 0;
        $('f').addEventListener('submit', (e) => { submitted++; e.preventDefault(); });
        $('go').click();
        r.push(submitted, $('t').cloneNode().value);
        const opt = new Option('Label', 'v', false, true);
        $('s').add(opt);
        r.push($('s').value, opt instanceof HTMLOptionElement, new Image(3, 4).width);
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["init","init","text",true,"B",1,3,"hello","https://example.com/submit","get","submit",true,"typed","init",false,true,false,true,2,true,"c","a","edited","hello",1,"typed","v",true,3]"#
    );
}

#[test]
fn geometry_is_synthetic_but_visible() {
    let js = page("<div id=a>x</div><div id=h hidden>y</div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a');
        const r = a.getBoundingClientRect();
        const h = document.getElementById('h').getBoundingClientRect();
        [r.width > 0, r.height > 0, r.top >= 0, r.bottom <= innerHeight, h.width, a.offsetParent === document.body,
         document.documentElement.clientWidth === innerWidth, document.documentElement.clientHeight === innerHeight,
         getComputedStyle(a).display, getComputedStyle(document.getElementById('h')).display,
         getComputedStyle(document.createElement('span')).display, document.scrollingElement === document.documentElement]
        "#,
    );
    assert_eq!(
        out,
        r#"[true,true,true,true,0,true,true,true,"block","none","inline",true]"#
    );
}

#[test]
fn document_properties() {
    let js = page(
        "<!doctype html><html lang=en><head><title> My  Page </title></head><body><script id=s></script></body></html>",
    );
    let out = eval(
        &js,
        r#"
        const r = [document.title, document.compatMode, document.characterSet, document.readyState,
          document.contentType, document.visibilityState, document.hidden, document.defaultView === window,
          document.activeElement === document.body, document.scripts.length, document.location === location,
          document.domain, typeof document.fonts.ready.then];
        document.title = 'New';
        r.push(document.querySelector('title').textContent);
        document.cookie = 'a=1; path=/';
        document.cookie = 'b=2';
        r.push(document.cookie);
        const input = document.createElement('input');
        document.body.appendChild(input);
        input.focus();
        r.push(document.activeElement === input);
        const style = document.createElement('style');
        document.head.appendChild(style);
        style.sheet.insertRule('.a { color: red }', 0);
        r.push(document.styleSheets.length, style.sheet.cssRules[0].cssText, document.styleSheets[0].ownerNode === style);
        const tw = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);
        const seen = [];
        while (tw.nextNode()) seen.push(tw.currentNode.tagName);
        r.push(seen.join());
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["My Page","CSS1Compat","UTF-8","complete","text/html","visible",false,true,true,1,true,"example.com","function","New","b=2; a=1",true,1,".a { color: red }",true,"SCRIPT,INPUT"]"#
    );
}

#[test]
fn constructors_and_character_data() {
    let js = page("<p id=p></p>");
    let out = eval(
        &js,
        r#"
        const t = new Text('hello world');
        const c = new Comment('note');
        const f = new DocumentFragment();
        const p = document.getElementById('p');
        p.append(t, c);
        const rest = t.splitText(5);
        const r = [t.data, rest.data, p.childNodes.length, t.wholeText, t.length, t.substringData(1, 3)];
        t.appendData('!');
        t.insertData(0, '>');
        t.deleteData(1, 1);
        t.replaceData(0, 1, '<');
        r.push(t.data, f instanceof DocumentFragment, c instanceof Comment, c.nodeValue);
        p.normalize();
        r.push(p.childNodes.length, p.firstChild.data);
        const deep = p.cloneNode(true);
        r.push(deep.outerHTML, deep.isEqualNode(p), deep === p, p.cloneNode().childNodes.length);
        r.push(p.compareDocumentPosition(document.body), document.body.compareDocumentPosition(p),
          p.firstChild.compareDocumentPosition(p.lastChild), p.compareDocumentPosition(deep) & 1);
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["hello"," world",3,"hello world",5,"ell","<ello!",true,true,"note",2,"<ello! world","<p id=\"p\">&lt;ello! world<!--note--></p>",true,false,0,10,20,4,1]"#
    );
}

fn nodes(js: &JsRuntime) -> usize {
    js.with_document(Document::len)
}

#[test]
fn detached_subtrees_without_wrappers_are_freed() {
    let js = page("<div id=list></div>");
    let base = nodes(&js);
    run(
        &js,
        "document.getElementById('list').innerHTML = '<p>x</p>'.repeat(200);",
    );
    assert!(nodes(&js) >= base + 400);
    for _ in 0..3 {
        run(
            &js,
            "document.getElementById('list').innerHTML = '<p>y</p>'.repeat(200);",
        );
    }
    run(&js, "document.getElementById('list').textContent = '';");
    js.run_gc();
    assert!(
        nodes(&js) <= base + 2,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}

#[test]
fn detached_trees_are_freed_once_unreachable() {
    let js = page("");
    let base = nodes(&js);
    run(
        &js,
        r#"
        window.d = document.createElement('div');
        d.innerHTML = '<p><b>x</b></p>'.repeat(100);
        d.firstChild.expando = 1;
        window.p = d.lastChild.firstChild;
        d.addEventListener('click', () => d.id);
        "#,
    );
    js.run_gc();
    assert!(nodes(&js) >= base + 300);
    run(&js, "delete window.d;");
    js.run_gc();
    // The <b> still holds its tree, and so its siblings' expandos.
    assert!(nodes(&js) >= base + 300);
    assert_eq!(eval(&js, "p.parentNode.parentNode.firstChild.expando"), "1");
    run(&js, "delete window.p;");
    js.run_gc();
    assert!(
        nodes(&js) <= base + 1,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}

#[test]
fn removed_nodes_with_listeners_do_not_leak() {
    let js = page("<ul id=l></ul>");
    run(
        &js,
        r#"
        const l = document.getElementById('l');
        for (let i = 0; i < 100; i++) {
          const li = document.createElement('li');
          li.innerHTML = '<a href=#>item</a>';
          li.addEventListener('click', () => li.classList.toggle('on'));
          li.onmouseover = () => li.id;
          l.appendChild(li);
        }
        "#,
    );
    let full = nodes(&js);
    run(&js, "document.getElementById('l').replaceChildren();");
    js.run_gc();
    assert!(
        nodes(&js) + 300 <= full,
        "{} nodes, {full} before",
        nodes(&js)
    );
}

#[test]
fn trees_built_while_detached_keep_their_wrappers() {
    let js = page("<div id=root></div>");
    run(
        &js,
        r#"
        (() => {
          // Bottom-up, like a virtual DOM renderer: children are appended to
          // detached parents, which are then attached.
          const list = document.createElement('ul');
          for (let i = 0; i < 20; i++) {
            const li = document.createElement('li');
            const span = document.createElement('span');
            span.__fiber = { index: i };
            li.appendChild(span);
            list.appendChild(li);
          }
          const wrapper = document.createElement('section');
          wrapper.appendChild(list);
          const frag = document.createDocumentFragment();
          frag.appendChild(wrapper);
          document.getElementById('root').appendChild(frag);
        })();
        "#,
    );
    js.run_gc();
    assert_eq!(
        eval(
            &js,
            "Array.from(document.querySelectorAll('span'), (s) => s.__fiber.index).join()"
        ),
        json("0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19")
    );
    // Moving a subtree out of the document and back keeps them too.
    run(
        &js,
        r#"
        window.moved = document.querySelector('ul');
        moved.remove();
        "#,
    );
    js.run_gc();
    run(
        &js,
        "document.body.appendChild(moved); delete window.moved;",
    );
    js.run_gc();
    assert_eq!(
        eval(
            &js,
            "document.querySelector('li:last-child span').__fiber.index"
        ),
        "19"
    );
}

#[test]
fn replacing_the_document_invalidates_old_nodes() {
    let js = page("<p id=old>old</p>");
    run(
        &js,
        "window.old = document.getElementById('old'); window.doc = document;",
    );
    js.set_document(Document::parse("<p id=new>new</p>"));
    let out = eval(
        &js,
        r#"
        let error;
        try { old.textContent; } catch (e) { error = e.name; }
        [doc === document, document.getElementById('new').textContent, document.getElementById('old'), error]
        "#,
    );
    assert_eq!(out, r#"[true,"new",null,"InvalidStateError"]"#);
}

#[test]
fn the_host_can_read_and_change_the_document() {
    let js = page("<div id=a></div>");
    run(
        &js,
        "window.a = document.getElementById('a'); a.marker = 1;",
    );
    js.with_document_mut(|doc| {
        let a = doc.get_element_by_id(doc.root(), "a").unwrap();
        let p = doc.create_element("p");
        doc.append_child(a, p).unwrap();
        doc.remove(a);
        let body = doc.body().unwrap();
        doc.append_child(body, a).unwrap();
    });
    js.run_gc();
    assert_eq!(
        eval(
            &js,
            "[a.firstChild.tagName, document.body.lastChild === a, a.marker]"
        ),
        r#"["P",true,1]"#
    );
    let html = js.with_document(|doc| doc.outer_html(doc.body().unwrap()));
    assert_eq!(html, r#"<body><div id="a"><p></p></div></body>"#);
}

#[test]
fn inserted_inline_scripts_run() {
    let js = page("");
    let out = eval(
        &js,
        r#"
        const s = document.createElement('script');
        s.textContent = 'window.ran = (window.ran || 0) + 1; window.current = document.currentScript;';
        document.head.appendChild(s);
        const r = [window.ran, window.current === s, document.currentScript, s.async];
        document.body.appendChild(s);
        const data = document.createElement('script');
        data.type = 'application/json';
        data.textContent = 'window.ran = 100';
        document.body.appendChild(data);
        document.body.innerHTML += '<script>window.ran = 200</script>';
        const frag = document.createDocumentFragment();
        const inner = document.createElement('script');
        inner.text = 'window.fromFragment = true';
        frag.appendChild(inner);
        const detached = document.createElement('div');
        detached.appendChild(frag);
        r.push(window.fromFragment);
        document.body.appendChild(detached);
        r.push(window.ran, window.fromFragment);
        r
        "#,
    );
    assert_eq!(out, "[1,true,null,true,null,1,true]");
}

#[test]
fn element_methods_on_other_nodes_throw() {
    let js = page("<p>text</p>");
    let out = eval(
        &js,
        r#"
        const text = document.querySelector('p').firstChild;
        const proto = Element.prototype;
        const desc = (name) => Object.getOwnPropertyDescriptor(proto, name);
        const attempts = [
          () => proto.setAttribute.call(text, 'a', 'b'),
          () => proto.setAttributeNS.call(text, null, 'a', 'b'),
          () => desc('innerHTML').set.call(text, '<b>'),
          () => desc('outerHTML').set.call(document, '<b>'),
          () => proto.insertAdjacentHTML.call(text, 'afterbegin', '<b>'),
          () => proto.appendChild.call({}, text),
          () => Node.prototype.appendChild.call(text, document.createElement('i')),
        ];
        attempts.map((f) => { try { f(); return 'none'; } catch (e) { return e.name; } })
        "#,
    );
    assert_eq!(
        out,
        r#"["TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","HierarchyRequestError"]"#
    );
    assert_eq!(eval(&js, "document.body.innerHTML"), json("<p>text</p>"));
}

#[test]
#[ignore = "timing check, run by hand"]
fn bulk_operations_scale() {
    let js = page("<ul id=l></ul>");
    let start = std::time::Instant::now();
    run(
        &js,
        r#"
        const l = document.getElementById('l');
        for (let i = 0; i < 20000; i++) {
          const li = document.createElement('li');
          li.className = 'item';
          li.textContent = 'item ' + i;
          l.appendChild(li);
        }
        let n = 0;
        for (let i = 0; i < l.childNodes.length; i++) n += l.childNodes[i].nodeType;
        window.count = document.querySelectorAll('li.item').length + n;
        for (const li of Array.from(l.children)) if (li.textContent.endsWith('7')) li.remove();
        l.innerHTML = '<li>x</li>'.repeat(20000);
        l.textContent = '';
        "#,
    );
    js.run_gc();
    eprintln!(
        "{:?}, {} nodes left",
        start.elapsed(),
        js.with_document(Document::len)
    );
}

#[test]
fn shadow_roots_hold_content_for_their_hosts() {
    let js = page("<div id=host></div><p id=plain></p>");
    run(
        &js,
        r#"
        const host = document.getElementById('host');
        window.root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<style>p{}</style><p id=inner>shadow <b>text</b></p><slot></slot>';
        window.inner = root.getElementById('inner');
        window.closedRoot = document.createElement('x-widget').attachShadow({ mode: 'closed' });
        "#,
    );
    assert_eq!(
        eval(
            &js,
            "[Object.prototype.toString.call(root), root instanceof ShadowRoot, \
             root instanceof DocumentFragment, root.mode, root.host.id, \
             document.getElementById('host').shadowRoot === root, root.nodeType]"
        ),
        r#"["[object ShadowRoot]",true,true,"open","host",true,11]"#
    );
    // The shadow tree is reachable from script but not part of the
    // document's tree or its HTML.
    assert_eq!(
        eval(
            &js,
            "[root.querySelector('b').textContent, inner.getRootNode() === root, \
             inner.getRootNode({ composed: true }) === document, inner.isConnected, \
             document.getElementById('inner'), document.body.innerHTML, root.innerHTML]"
        ),
        r#"["text",true,true,true,null,"<div id=\"host\"></div><p id=\"plain\"></p>","<style>p{}</style><p id=\"inner\">shadow <b>text</b></p><slot></slot>"]"#
    );
    // Closed roots are not exposed through their host.
    assert_eq!(
        eval(
            &js,
            "[closedRoot.mode, closedRoot.host.shadowRoot, closedRoot.host.isConnected, \
             closedRoot.appendChild(document.createElement('i')).isConnected]"
        ),
        r#"["closed",null,false,false]"#
    );
}

#[test]
fn attach_shadow_checks_its_host_and_mode() {
    let js = page("<div id=host></div>");
    assert_eq!(
        eval(
            &js,
            r#"(() => {
                const errors = [];
                const attempt = (f) => { try { f(); errors.push('ok'); } catch (e) { errors.push(e.name); } };
                const host = document.getElementById('host');
                attempt(() => host.attachShadow({ mode: 'sideways' }));
                attempt(() => host.attachShadow({ mode: 'open' }));
                attempt(() => host.attachShadow({ mode: 'open' }));
                attempt(() => document.createElement('img').attachShadow({ mode: 'open' }));
                attempt(() => document.createElement('font-face').attachShadow({ mode: 'open' }));
                attempt(() => document.createElement('my-element').attachShadow({ mode: 'open' }));
                attempt(() => new ShadowRoot());
                return errors;
            })()"#
        ),
        r#"["TypeError","ok","NotSupportedError","NotSupportedError","NotSupportedError","ok","TypeError"]"#
    );
}

#[test]
fn composed_events_leave_shadow_trees() {
    let js = page("<div id=host></div>");
    assert_eq!(
        eval(
            &js,
            r#"(() => {
                const seen = [];
                const host = document.getElementById('host');
                const root = host.attachShadow({ mode: 'open' });
                const button = root.appendChild(document.createElement('button'));
                root.addEventListener('ping', (e) => seen.push('root:' + e.composed));
                host.addEventListener('ping', (e) => seen.push('host:' + e.composed));
                document.addEventListener('ping', (e) => seen.push('document:' + e.composed));
                button.dispatchEvent(new Event('ping', { bubbles: true }));
                button.dispatchEvent(new Event('ping', { bubbles: true, composed: true }));
                return seen;
            })()"#
        ),
        r#"["root:false","root:true","host:true","document:true"]"#
    );
}

#[test]
fn declarative_shadow_roots_reach_script() {
    let js = page(
        "<my-card id=c><template shadowrootmode=open shadowrootserializable><h2><slot name=title></slot></h2>\
         <slot></slot></template><span slot=title>Title</span>body</my-card>",
    );
    assert_eq!(
        eval(
            &js,
            "(() => { const c = document.getElementById('c'); const r = c.shadowRoot; \
             return [r instanceof ShadowRoot, r.host === c, r.mode, r.serializable, r.clonable, \
               r.innerHTML, c.innerHTML, document.querySelector('template')]; })()"
        ),
        r#"[true,true,"open",true,false,"<h2><slot name=\"title\"></slot></h2><slot></slot>","<span slot=\"title\">Title</span>body",null]"#
    );
    // A component's own attachShadow takes the declarative root over.
    assert_eq!(
        eval(
            &js,
            "(() => { const c = document.getElementById('c'); const old = c.shadowRoot; \
             const r = c.attachShadow({ mode: 'open' }); \
             return [r === old, r.innerHTML, (() => { try { c.attachShadow({ mode: 'open' }); } catch (e) { return e.name; } })()]; })()"
        ),
        r#"[true,"","NotSupportedError"]"#
    );
}

#[test]
fn get_html_includes_the_shadow_roots_asked_for() {
    let js = page("<div id=host><b>light</b></div>");
    assert_eq!(
        eval(
            &js,
            r#"(() => {
                const host = document.getElementById('host');
                const root = host.attachShadow({ mode: 'closed', serializable: true });
                root.innerHTML = '<i>shadow</i>';
                const other = document.createElement('x-y');
                other.attachShadow({ mode: 'open' }).innerHTML = '<u>x</u>';
                root.appendChild(other);
                return [host.innerHTML, host.getHTML(), host.getHTML({ serializableShadowRoots: true }),
                  host.getHTML({ shadowRoots: [other.shadowRoot] }), root.getHTML({ serializableShadowRoots: true })];
            })()"#
        ),
        r#"["<b>light</b>","<b>light</b>","<template shadowrootmode=\"closed\" shadowrootserializable=\"\"><i>shadow</i><x-y></x-y></template><b>light</b>","<b>light</b>","<i>shadow</i><x-y></x-y>"]"#
    );
}

#[test]
fn slots_are_assigned_by_name() {
    let js = page(
        "<my-el id=host><span slot=title>T</span>text<b>default</b><i slot=missing>m</i></my-el>",
    );
    run(
        &js,
        r#"
        window.host = document.getElementById('host');
        window.root = host.attachShadow({ mode: 'open' });
        root.innerHTML = '<slot name=title>fallback</slot><slot></slot><slot name=empty><em>fb</em></slot>';
        window.slots = root.querySelectorAll('slot');
        "#,
    );
    assert_eq!(
        eval(
            &js,
            "[slots[0].assignedNodes().map(n => n.textContent),               slots[1].assignedNodes().map(n => n.textContent),               slots[1].assignedElements().map(n => n.tagName),               slots[2].assignedNodes(), slots[2].assignedNodes({ flatten: true }).map(n => n.tagName),               host.querySelector('span').assignedSlot === slots[0], host.lastChild.assignedSlot,               host.childNodes[1].assignedSlot === slots[1]]"
        ),
        r#"[["T"],["text","default"],["B"],[],["EM"],true,null,true]"#
    );
}

#[test]
fn events_from_shadow_trees_are_retargeted() {
    let js = page("<my-el id=host><span id=light>x</span></my-el>");
    assert_eq!(
        eval(
            &js,
            r#"(() => {
                const seen = [];
                const host = document.getElementById('host');
                const root = host.attachShadow({ mode: 'open' });
                root.innerHTML = '<div><button>b</button><slot></slot></div>';
                const button = root.querySelector('button');
                const log = (where) => (e) => seen.push(where + ':' + (e.target.id || e.target.localName) + ':' + e.eventPhase);
                root.querySelector('div').addEventListener('ping', log('div'));
                host.addEventListener('ping', log('host'));
                document.body.addEventListener('ping', log('body'));
                button.dispatchEvent(new Event('ping', { bubbles: true, composed: true }));
                // Not bubbling, but the host is its retargeted target.
                button.dispatchEvent(new Event('ping', { composed: true }));
                // Slotted content goes through its slot.
                document.getElementById('light').dispatchEvent(new Event('ping', { bubbles: true }));
                const e = new Event('ping', { composed: true });
                button.dispatchEvent(e);
                seen.push(String(e.target));
                return seen;
            })()"#
        ),
        r#"["div:button:3","host:host:2","body:host:3","host:host:2","div:light:3","host:light:3","body:light:3","host:host:2","null"]"#
    );
}

#[test]
fn shadow_trees_are_freed_with_their_host() {
    let js = page("");
    let base = nodes(&js);
    run(
        &js,
        r#"
        window.host = document.createElement('div');
        host.attachShadow({ mode: 'open' }).innerHTML = '<p>x</p>'.repeat(100);
        host.shadowRoot.firstChild.expando = 1;
        "#,
    );
    js.run_gc();
    assert!(nodes(&js) >= base + 200);
    // The host keeps its shadow root, and its expandos, alive.
    assert_eq!(eval(&js, "host.shadowRoot.firstChild.expando"), "1");
    run(&js, "delete window.host;");
    js.run_gc();
    assert!(
        nodes(&js) <= base + 1,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}

#[test]
fn create_html_document_builds_an_inert_document() {
    let js = page("<title>Page</title><div id=target></div>");
    run(
        &js,
        r#"
        window.doc = document.implementation.createHTMLDocument('Scratch');
        doc.body.innerHTML = '<form></form><form></form><p id=p>parsed <img src=x.png onerror="window.ran = 1"></p><script>window.ran = 2</script>';
        "#,
    );
    assert_eq!(
        eval(
            &js,
            "[doc instanceof Document, doc === document, doc.nodeType, doc.title, \
             doc.documentElement.outerHTML.slice(0, 41), doc.doctype.name, doc.body.childNodes.length, \
             doc.head.firstChild.tagName, doc.URL, doc.defaultView, doc.location, doc.cookie]"
        ),
        r#"[true,false,9,"Scratch","<html><head><title>Scratch</title></head>","html",4,"TITLE","about:blank",null,null,""]"#
    );
    // Lookups stay inside each document.
    assert_eq!(
        eval(
            &js,
            "[doc.getElementById('p').id, document.getElementById('p'), doc.getElementById('target'), \
             doc.querySelectorAll('form').length, doc.getElementsByTagName('p').length, \
             document.title, doc.body.isConnected, doc.getElementById('p').ownerDocument === doc]"
        ),
        r#"["p",null,null,2,1,"Page",false,true]"#
    );
    // Nodes move into the page without adopting; nothing ran in the inert
    // document.
    run(
        &js,
        "document.getElementById('target').append(doc.getElementById('p'));",
    );
    assert_eq!(
        eval(
            &js,
            "[document.getElementById('p').ownerDocument === document, \
             document.querySelector('#target p').textContent, doc.body.childNodes.length, window.ran]"
        ),
        r#"[true,"parsed ",3,null]"#
    );
    // Without a title argument there is no title element.
    assert_eq!(
        eval(
            &js,
            "(() => { const d = document.implementation.createHTMLDocument(); \
             return [d.head.childNodes.length, d.title, d.body.tagName]; })()"
        ),
        r#"[0,"","BODY"]"#
    );
    assert_eq!(
        eval(&js, "doc.title = 'Renamed'; doc.title"),
        json("Renamed")
    );
    assert_eq!(eval(&js, "document.title"), json("Page"));
}

#[test]
fn nodes_keep_the_document_they_were_made_in() {
    let js = page("<div id=target></div>");
    run(
        &js,
        r#"
        window.doc = document.implementation.createHTMLDocument('');
        window.made = doc.createElement('p');
        made.innerHTML = '<b>x</b><i>y</i>';
        "#,
    );
    // Detached nodes made by a document belong to it, all the way down,
    // and so do nodes taken out of one of its trees.
    assert_eq!(
        eval(
            &js,
            "[made.ownerDocument === doc, made.firstChild.ownerDocument === doc,              doc.createTextNode('t').ownerDocument === doc,              doc.createDocumentFragment().ownerDocument === doc,              doc.createComment('c').ownerDocument === doc,              made.cloneNode(true).lastChild.ownerDocument === doc,              (() => { const b = made.firstChild; b.remove(); return b.ownerDocument === doc; })(),              (() => { doc.body.append(made); made.remove(); return made.ownerDocument === doc; })(),              document.createElement('a').ownerDocument === document, doc.ownerDocument]"
        ),
        "[true,true,true,true,true,true,true,true,true,null]"
    );
    // Inserting into the page's tree, importing or adopting moves nodes to
    // the page's document, and back.
    assert_eq!(
        eval(
            &js,
            "(() => {                const target = document.getElementById('target');                const a = doc.createElement('a'); target.append(a); a.remove();                const imported = document.importNode(made, true);                const adopted = document.adoptNode(doc.createElement('em'));                const back = doc.adoptNode(document.createElement('u'));                const html = doc.importNode(document.documentElement, false);                return [a.ownerDocument === document, imported.ownerDocument === document,                  imported.firstChild.ownerDocument === document, made.ownerDocument === doc,                  adopted.ownerDocument === document, back.ownerDocument === doc,                  html.ownerDocument === doc];              })()"
        ),
        "[true,true,true,true,true,true,true]"
    );
    // A node made by a document that is gone falls back to the page's.
    run(
        &js,
        "window.orphan = document.implementation.createHTMLDocument('').createElement('div');          delete window.doc;",
    );
    js.run_gc();
    run(
        &js,
        "document.body.appendChild(document.createElement('i'));",
    );
    assert_eq!(
        eval(
            &js,
            "[orphan.ownerDocument === document, made.ownerDocument === document,              orphan.ownerDocument.createElement('x').ownerDocument === document]"
        ),
        "[true,true,true]"
    );
}

#[test]
fn dom_parser_parses_inert_html_documents() {
    let js = page("<title>Page</title><div id=target></div>");
    run(
        &js,
        r#"
        window.parsed = new DOMParser().parseFromString(
            '<title>Parsed</title><div id=a class=x>one<img src=x onerror="window.ran = 1"></div>' +
            '<script>window.ran = 2</script><noscript><b>markup</b></noscript>', 'text/html');
        "#,
    );
    assert_eq!(
        eval(
            &js,
            "[parsed instanceof Document, parsed instanceof HTMLDocument, parsed === document, \
             parsed.title, parsed.compatMode, parsed.contentType, parsed.URL, parsed.readyState, \
             parsed.body.children.length, parsed.querySelector('.x').id, parsed.getElementById('a').ownerDocument === parsed, \
             parsed.querySelector('noscript b').textContent, parsed.defaultView, document.title, \
             new DOMParser().parseFromString('<!DOCTYPE html><p>', 'text/html').compatMode, \
             String(new DOMParser())]"
        ),
        r#"[true,true,false,"Parsed","BackCompat","text/html","https://example.com/dir/page.html","complete",3,"a",true,"markup",null,"Page","CSS1Compat","[object DOMParser]"]"#
    );
    // Moving the nodes into the page adopts them, and its scripts still
    // never run.
    run(
        &js,
        "const t = document.getElementById('target'); \
         t.append(document.adoptNode(parsed.getElementById('a'))); \
         t.append(parsed.querySelector('script')); \
         t.append(document.importNode(parsed.body, true));",
    );
    assert_eq!(
        eval(
            &js,
            "[window.ran, document.querySelectorAll('#target #a').length, \
             document.getElementById('a').ownerDocument === document, parsed.body.children.length, \
             document.querySelector('#target body') !== null]"
        ),
        "[null,1,true,1,true]"
    );
    assert_eq!(
        eval(
            &js,
            "(() => { try { new DOMParser().parseFromString('x', 'text/plain'); } catch (e) { return e.name; } })()"
        ),
        json("TypeError")
    );
}

#[test]
fn xml_documents() {
    let js = page("");
    run(
        &js,
        r##"
        window.xml = new DOMParser().parseFromString(
            '<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><entry><Title>A &amp; B</Title></entry>' +
            '<svg xmlns="http://www.w3.org/2000/svg"><circle r="1"/></svg></feed>', 'application/xml');
        "##,
    );
    assert_eq!(
        eval(
            &js,
            "[xml instanceof XMLDocument, xml.contentType, xml.documentElement.tagName, \
             xml.documentElement.namespaceURI, xml.getElementsByTagName('Title')[0].textContent, \
             xml.getElementsByTagName('title').length, xml.querySelector('circle') instanceof SVGElement, \
             xml.querySelector('entry') instanceof HTMLElement, xml.createElement('Mixed').tagName, \
             xml.createElement('x').namespaceURI]"
        ),
        r#"[true,"application/xml","feed","http://www.w3.org/2005/Atom","A & B",0,true,false,"Mixed",null]"#
    );
    // Malformed XML gives a document with a parsererror element.
    assert_eq!(
        eval(
            &js,
            "(() => { const d = new DOMParser().parseFromString('<a><b></a>', 'text/xml'); \
             const e = d.getElementsByTagName('parsererror'); \
             return [e.length, d.documentElement.tagName, /XML Parsing Error: .*<\\/a>.* line 1/.test(e[0].textContent)]; })()"
        ),
        r#"[1,"parsererror",true]"#
    );
    // new Document() and createDocument() make empty XML documents.
    assert_eq!(
        eval(
            &js,
            "(() => { \
               const d = new Document(); \
               const x = document.implementation.createDocument('http://www.w3.org/2000/svg', 'svg', null); \
               const e = document.implementation.createDocument(null, '', null); \
               d.appendChild(d.createElement('root')).appendChild(d.createTextNode('t')); \
               return [d instanceof Document, d instanceof XMLDocument, d.contentType, d.documentElement.outerHTML, \
                 x instanceof XMLDocument, x.contentType, x.documentElement.namespaceURI, e.childNodes.length, \
                 d.documentElement.ownerDocument === d]; \
             })()"
        ),
        r#"[true,false,"application/xml","<root>t</root>",true,"image/svg+xml","http://www.w3.org/2000/svg",0,true]"#
    );
}

#[test]
fn writing_to_a_document_made_by_script() {
    let js = page("<p>page</p>");
    assert_eq!(
        eval(
            &js,
            "(() => { \
               const d = document.implementation.createHTMLDocument(''); \
               d.open(); d.write('<!DOCTYPE html><title>W</title><p id=a>one'); d.writeln('</p><p>two</p>'); d.close(); \
               return [d.title, d.body.innerHTML, d.compatMode, document.body.innerHTML]; \
             })()"
        ),
        r#"["W","<p id=\"a\">one</p><p>two</p>\n","CSS1Compat","<p>page</p>"]"#
    );
}

#[test]
fn documents_from_create_html_document_are_freed_once_unreachable() {
    let js = page("");
    let base = nodes(&js);
    run(
        &js,
        r#"
        for (let i = 0; i < 20; i++) {
            const d = document.implementation.createHTMLDocument('');
            d.body.innerHTML = '<p>x</p>'.repeat(20);
        }
        window.kept = document.implementation.createHTMLDocument('');
        kept.body.innerHTML = '<p>y</p>';
        "#,
    );
    js.run_gc();
    run(
        &js,
        "document.body.appendChild(document.createElement('i'));",
    );
    let held = nodes(&js);
    assert!(held <= base + 12, "{held} nodes, {base} at start");
    run(&js, "delete window.kept;");
    js.run_gc();
    run(
        &js,
        "document.body.appendChild(document.createElement('i'));",
    );
    assert!(
        nodes(&js) <= base + 2,
        "{} nodes, {base} at start",
        nodes(&js)
    );
    // The same for parsed documents and nodes they made.
    let base = nodes(&js);
    run(
        &js,
        r#"
        for (let i = 0; i < 20; i++) {
            const d = new DOMParser().parseFromString('<p>x</p>'.repeat(20), 'text/html');
            new DOMParser().parseFromString('<r>' + '<i>x</i>'.repeat(20) + '</r>', 'text/xml');
            window.made = d.createElement('div');
            made.innerHTML = '<b>y</b>';
        }
        "#,
    );
    js.run_gc();
    run(
        &js,
        "delete window.made; document.body.appendChild(document.createElement('i'));",
    );
    js.run_gc();
    run(
        &js,
        "document.body.appendChild(document.createElement('i'));",
    );
    assert!(
        nodes(&js) <= base + 2,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}

#[test]
fn mixin_members_marked_unscopable_are_hidden_from_with() {
    let js = page("<p id=p></p>");
    let out = eval(
        &js,
        r#"
        const u = Element.prototype[Symbol.unscopables];
        const p = document.getElementById('p');
        // `with` needs sloppy code.
        const lookup = new Function('el', 'with (el) return [typeof remove, typeof append, typeof querySelector]');
        [Object.keys(u).sort(), Object.getPrototypeOf(u), Object.keys(Document.prototype[Symbol.unscopables]).sort(),
         Object.keys(CharacterData.prototype[Symbol.unscopables]).sort(), lookup(p)]
        "#,
    );
    assert_eq!(
        out,
        r#"[["after","append","before","prepend","remove","replaceChildren","replaceWith"],null,["append","prepend","replaceChildren"],["after","before","remove","replaceWith"],["undefined","undefined","function"]]"#
    );
}

#[test]
fn file_inputs_have_an_empty_file_list() {
    let js = page("<!doctype html><input id=f type=file><input id=t>");
    let out = eval(
        &js,
        r#"
        const f = document.getElementById('f');
        const made = document.createElement('input');
        made.type = 'file';
        let illegal;
        try { new FileList(); } catch (e) { illegal = e.name; }
        [typeof FileList, f.files instanceof FileList, f.files.length, f.files.item(0), f.files[0] === undefined,
         f.files === f.files, made.files instanceof FileList, made.files === f.files, [...f.files].length,
         Object.prototype.toString.call(f.files), document.getElementById('t').files,
         ({}) instanceof FileList, illegal]
        "#,
    );
    assert_eq!(
        out,
        r#"["function",true,0,null,true,true,true,false,0,"[object FileList]",null,false,"TypeError"]"#
    );
}

#[test]
fn xml_serializer_declares_the_namespace() {
    let js =
        page("<div id=d class=a>x &amp; <b>y</b><!--c--></div><svg id=s><path d=M0></path></svg>");
    assert_eq!(
        eval(
            &js,
            "const x = new XMLSerializer();
             JSON.stringify([x.serializeToString(document.getElementById('d')),
              x.serializeToString(document.getElementById('s')),
              x.serializeToString(document.createTextNode('a<b')),
              x.serializeToString(new DOMParser().parseFromString('<r><i/></r>', 'text/xml')),
              String(x), (() => { try { x.serializeToString({}); } catch (e) { return e.name; } })()])"
        ),
        json(concat!(
            r#"["<div xmlns=\"http://www.w3.org/1999/xhtml\" id=\"d\" class=\"a\">x &amp; <b>y</b><!--c--></div>","#,
            r#""<svg xmlns=\"http://www.w3.org/2000/svg\" id=\"s\"><path d=\"M0\"></path></svg>","a&lt;b","#,
            r#""<r><i></i></r>","[object XMLSerializer]","TypeError"]"#
        ))
    );
}

#[test]
fn media_track_lists_are_empty_event_targets() {
    let js = page("<video id=v></video>");
    assert_eq!(
        eval(
            &js,
            "const v = document.getElementById('v');
             let changed = 0;
             v.textTracks.addEventListener('change', () => changed++);
             v.textTracks.dispatchEvent(new Event('change'));
             JSON.stringify([v.textTracks === v.textTracks, v.textTracks.length, [...v.textTracks].length,
               String(v.textTracks), v.audioTracks.length, v.videoTracks.getTrackById('x'),
               v.textTracks instanceof EventTarget, changed, Array.from(v.textTracks).length])"
        ),
        json(r#"[true,0,0,"[object TextTrackList]",0,null,true,1,0]"#)
    );
}
