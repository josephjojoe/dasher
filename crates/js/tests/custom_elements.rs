//! Custom elements: definitions, construction, upgrades and lifecycle
//! callbacks.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    // `Logged` is a base class whose callbacks append to `log`.
    run(
        &js,
        r#"
        globalThis.log = [];
        globalThis.errors = [];
        addEventListener('error', (e) => errors.push(e.message));
        globalThis.$ = (id) => document.getElementById(id);
        globalThis.Logged = class extends HTMLElement {
          static observedAttributes = ['state'];
          constructor() { super(); log.push('construct ' + this.id); }
          connectedCallback() { log.push('connected ' + this.id); }
          disconnectedCallback() { log.push('disconnected ' + this.id); }
          attributeChangedCallback(name, old, value, ns) { log.push(`attr ${this.id} ${name} ${old}->${value}${ns ? ' ' + ns : ''}`); }
        };
        globalThis.take = () => { const l = log; log = []; return l; };
        "#,
    );
    js
}

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

#[test]
fn the_registry_is_there() {
    let js = page("");
    assert_eq!(
        eval(
            &js,
            "[typeof customElements, customElements instanceof CustomElementRegistry, \
              String(customElements), customElements.get('x-a'), customElements.getName(Logged)]"
        ),
        r#"["object",true,"[object CustomElementRegistry]",null,null]"#
    );
}

#[test]
fn constructing_defined_elements() {
    let js = page("");
    run(
        &js,
        r#"
        globalThis.MyEl = class extends HTMLElement {
          field = 'f';
          #secret = 's';
          constructor() { super(); this.made = true; }
          get secret() { return this.#secret; }
        };
        customElements.define('my-el', MyEl);
        "#,
    );
    assert_eq!(
        eval(
            &js,
            r#"(() => {
              const a = new MyEl();
              const b = document.createElement('my-el');
              const c = document.createElementNS('http://www.w3.org/1999/xhtml', 'my-el');
              return [a instanceof MyEl, a instanceof HTMLElement, a.localName, a.made, a.field, a.secret,
                b instanceof MyEl, b.made, c instanceof MyEl, a.constructor === MyEl, a.isConnected,
                customElements.get('my-el') === MyEl, customElements.getName(MyEl), a.outerHTML,
                document.body.appendChild(a) === a, document.querySelector('my-el') === a];
            })()"#
        ),
        r#"[true,true,"my-el",true,"f","s",true,true,true,true,false,true,"my-el","<my-el></my-el>",true,true]"#
    );
    // Without a definition there is nothing to construct.
    assert_eq!(
        eval(
            &js,
            r#"(() => {
              const out = [];
              const attempt = (f) => { try { f(); out.push('ok'); } catch (e) { out.push(e.name); } };
              attempt(() => new HTMLElement());
              attempt(() => { class Undefined extends HTMLElement {} new Undefined(); });
              attempt(() => { class Sub extends MyEl {} new Sub(); });
              attempt(() => new HTMLDivElement());
              return out;
            })()"#
        ),
        r#"["TypeError","TypeError","TypeError","TypeError"]"#
    );
}

#[test]
fn define_checks_its_arguments() {
    let js = page("");
    assert_eq!(
        eval(
            &js,
            r#"(() => {
              const out = [];
              const attempt = (f) => { try { f(); out.push('ok'); } catch (e) { out.push(e.name); } };
              class A extends HTMLElement {}
              attempt(() => customElements.define('nodash', class extends HTMLElement {}));
              attempt(() => customElements.define('Up-per', class extends HTMLElement {}));
              attempt(() => customElements.define('font-face', class extends HTMLElement {}));
              attempt(() => customElements.define('x-a', () => {}));
              attempt(() => customElements.define('x-a', A));
              attempt(() => customElements.define('x-a', class extends HTMLElement {}));
              attempt(() => customElements.define('x-b', A));
              attempt(() => customElements.define('x-c', class extends HTMLElement {}, { extends: 'x-a' }));
              attempt(() => customElements.define('x-d', class extends HTMLElement {}, { extends: 'blink' }));
              attempt(() => customElements.define('x-e', class extends HTMLElement { get connectedCallback() { return 1; } }));
              attempt(() => customElements.define('x-f'));
              return out;
            })()"#
        ),
        r#"["SyntaxError","SyntaxError","SyntaxError","TypeError","ok","NotSupportedError","NotSupportedError","NotSupportedError","NotSupportedError","TypeError","TypeError"]"#
    );
}

#[test]
fn defining_upgrades_the_document_in_tree_order() {
    let js = page(
        "<x-log id=a state=on><x-log id=b></x-log></x-log>\
         <div id=host></div><template><x-log id=t></x-log></template><x-log id=c></x-log>",
    );
    run(
        &js,
        r#"
        $('host').attachShadow({ mode: 'closed' }).innerHTML = '<x-log id=s></x-log>';
        globalThis.parsed = $('a');
        customElements.define('x-log', class extends Logged {});
        "#,
    );
    // Each one is constructed, told about its observed attributes, then
    // connected; the shadow tree comes in tree order and template contents
    // are left alone.
    assert_eq!(
        eval(&js, "take()"),
        r#"["construct a","attr a state null->on","connected a","construct b","connected b","construct s","connected s","construct c","connected c"]"#
    );
    assert_eq!(
        eval(
            &js,
            "[$('a') === parsed, parsed instanceof customElements.get('x-log'), \
              document.querySelector('template').content.firstChild instanceof customElements.get('x-log')]"
        ),
        "[true,true,false]"
    );
}

#[test]
fn lifecycle_callbacks_follow_the_tree() {
    let js = page("<div id=box></div><div id=other></div>");
    run(
        &js,
        "customElements.define('x-log', class extends Logged {}); \
         globalThis.el = document.createElement('x-log'); el.id = 'e';",
    );
    assert_eq!(eval(&js, "take()"), r#"["construct "]"#);
    run(
        &js,
        r#"
        $('box').appendChild(el);
        el.setAttribute('state', 'one');
        el.setAttribute('other', 'ignored');
        el.setAttributeNS('urn:x', 'x:state', 'ns');
        el.removeAttribute('state');
        $('other').append(el);
        el.remove();
        el.toggleAttribute('state');
        "#,
    );
    assert_eq!(
        eval(&js, "take()"),
        r#"["connected e","attr e state null->one","attr e state null->ns urn:x","attr e state one->null","disconnected e","connected e","disconnected e","attr e state null->"]"#
    );
    // Whole subtrees, fragments and replacement.
    run(
        &js,
        r#"
        const wrap = document.createElement('section');
        wrap.innerHTML = '<x-log id=i1></x-log><p><x-log id=i2></x-log></p>';
        $('box').append(wrap);
        $('box').replaceChildren();
        const f = document.createDocumentFragment();
        f.append(document.createElement('x-log'));
        f.firstChild.id = 'f';
        take();
        $('box').append(f);
        $('box').innerHTML = '';
        "#,
    );
    assert_eq!(eval(&js, "take()"), r#"["connected f","disconnected f"]"#);
}

#[test]
fn markup_inserted_by_script_is_upgraded_once_connected() {
    let js = page("<div id=box></div>");
    run(
        &js,
        r#"
        customElements.define('x-log', class extends Logged {});
        globalThis.detached = document.createElement('div');
        detached.innerHTML = '<x-log id=d state=s></x-log>';
        log.push('built');
        $('box').innerHTML = '<x-log id=i></x-log>';
        $('box').insertAdjacentHTML('beforeend', '<x-log id=j></x-log>');
        $('box').append(detached);
        "#,
    );
    assert_eq!(
        eval(&js, "take()"),
        r#"["built","construct i","connected i","construct j","connected j","construct d","attr d state null->s","connected d"]"#
    );
}

#[test]
fn when_defined_and_upgrade() {
    let js = page("");
    run(
        &js,
        r#"
        globalThis.results = [];
        customElements.whenDefined('later-el').then((c) => results.push(c.name));
        customElements.whenDefined('Bad').catch((e) => results.push(e.name));
        globalThis.box = document.createElement('div');
        box.innerHTML = '<later-el></later-el>';
        customElements.define('later-el', class LaterEl extends HTMLElement {});
        results.push(box.firstChild instanceof customElements.get('later-el'));
        customElements.upgrade(box);
        results.push(box.firstChild instanceof customElements.get('later-el'));
        customElements.whenDefined('later-el').then((c) => results.push('again ' + c.name));
        "#,
    );
    assert_eq!(
        eval(&js, "results"),
        r#"[false,true,"SyntaxError","LaterEl","again LaterEl"]"#
    );
}

#[test]
fn clones_are_upgraded() {
    let js = page("");
    run(
        &js,
        "customElements.define('x-log', class extends Logged {}); \
         globalThis.tree = document.createElement('div'); \
         tree.innerHTML = '<x-log id=c></x-log>'; \
         globalThis.copy = tree.cloneNode(true);",
    );
    assert_eq!(
        eval(
            &js,
            "[take(), copy.firstChild instanceof customElements.get('x-log'), \
              tree.firstChild instanceof customElements.get('x-log')]"
        ),
        r#"[["construct c"],true,false]"#
    );
}

#[test]
fn failed_upgrades_are_reported_and_left_alone() {
    let js = page("<bad-el id=b></bad-el>");
    run(
        &js,
        r#"
        customElements.define('bad-el', class extends HTMLElement {
          constructor() { super(); throw new Error('boom'); }
          connectedCallback() { log.push('never'); }
        });
        customElements.define('wrong-el', class extends HTMLElement {
          constructor() { return document.createElement('div'); }
        });
        document.body.insertAdjacentHTML('beforeend', '<wrong-el></wrong-el>');
        // Callbacks that throw are reported, and the others still run.
        customElements.define('throwing-el', class extends HTMLElement {
          connectedCallback() { throw new Error('connected'); }
        });
        document.body.append(document.createElement('throwing-el'), document.createElement('throwing-el'));
        // A failed element is not tried again.
        const b = $('b');
        b.remove();
        document.body.append(b);
        "#,
    );
    assert_eq!(
        eval(
            &js,
            "[errors, take(), $('b') instanceof HTMLElement, \
              (() => { try { document.createElement('bad-el'); } catch (e) { return e.message; } })()]"
        ),
        r#"[["Uncaught Error: boom","Uncaught TypeError: Failed to construct 'wrong-el': The result must be the element being upgraded.","Uncaught Error: connected","Uncaught Error: connected"],[],true,"boom"]"#
    );
}

#[test]
fn customized_built_in_elements() {
    let js = page("<button is=fancy-button id=p>parsed</button>");
    run(
        &js,
        r#"
        globalThis.Fancy = class extends HTMLButtonElement {
          constructor() { super(); this.fancy = true; }
        };
        customElements.define('fancy-button', Fancy, { extends: 'button' });
        "#,
    );
    assert_eq!(
        eval(
            &js,
            r#"(() => {
              const made = document.createElement('button', { is: 'fancy-button' });
              const direct = new Fancy();
              return [$('p') instanceof Fancy, $('p').fancy, made instanceof Fancy, made.outerHTML,
                direct.localName, direct instanceof HTMLButtonElement,
                document.createElement('fancy-button') instanceof Fancy];
            })()"#
        ),
        r#"[true,true,true,"<button is=\"fancy-button\"></button>","button",true,false]"#
    );
}

#[test]
fn components_render_into_shadow_roots() {
    let js = page(
        "<user-card name=Ada><span slot=bio>Mathematician</span></user-card>\
         <user-card name=Alan></user-card>",
    );
    run(
        &js,
        r#"
        // The usual shape of a component: a shadow root made in the
        // constructor, rendered when connected and when attributes change,
        // and a nested component inside it.
        customElements.define('user-card', class extends HTMLElement {
          static observedAttributes = ['name'];
          #root = this.attachShadow({ mode: 'closed' });
          connectedCallback() { this.render(); }
          attributeChangedCallback() { if (this.isConnected) this.render(); }
          render() {
            this.#root.innerHTML = `<h2>${this.getAttribute('name')}</h2><user-badge></user-badge><slot name=bio>No bio</slot>`;
          }
        });
        customElements.define('user-badge', class extends HTMLElement {
          connectedCallback() {
            this.attachShadow({ mode: 'open' }).textContent = 'badge:' + this.getRootNode().host.getAttribute('name');
          }
        });
        document.querySelector('user-card').setAttribute('name', 'Ada L.');
        "#,
    );
    js.with_document(|doc| {
        let html = doc.outer_html_with(doc.body().unwrap(), dasher_dom::ShadowRoots::All);
        assert_eq!(
            html,
            "<body><user-card name=\"Ada L.\"><template shadowrootmode=\"closed\"><h2>Ada L.</h2>\
             <user-badge><template shadowrootmode=\"open\">badge:Ada L.</template></user-badge>\
             <slot name=\"bio\">No bio</slot></template><span slot=\"bio\">Mathematician</span>\
             </user-card><user-card name=\"Alan\"><template shadowrootmode=\"closed\"><h2>Alan</h2>\
             <user-badge><template shadowrootmode=\"open\">badge:Alan</template></user-badge>\
             <slot name=\"bio\">No bio</slot></template></user-card></body>"
        );
    });
}

#[test]
fn element_internals() {
    let js = page("");
    run(
        &js,
        r#"
        customElements.define('form-el', class extends HTMLElement {
          static formAssociated = true;
          constructor() {
            super();
            this.internals = this.attachInternals();
            this.attachShadow({ mode: 'closed' });
          }
        });
        customElements.define('no-internals', class extends HTMLElement {
          static disabledFeatures = ['internals', 'shadow'];
        });
        "#,
    );
    assert_eq!(
        eval(
            &js,
            r#"(() => {
              const out = [];
              const attempt = (f) => { try { f(); out.push('ok'); } catch (e) { out.push(e.name); } };
              const el = document.createElement('form');
              const f = el.appendChild(document.createElement('form-el'));
              const i = f.internals;
              out.push(i instanceof ElementInternals, i.shadowRoot instanceof ShadowRoot, i.form === el,
                i.checkValidity(), i.validity.valid, i.states.add('x').has('x'));
              i.setFormValue('v');
              attempt(() => f.attachInternals());
              attempt(() => document.createElement('div').attachInternals());
              attempt(() => document.createElement('no-internals').attachInternals());
              attempt(() => document.createElement('no-internals').attachShadow({ mode: 'open' }));
              return out;
            })()"#
        ),
        r#"[true,true,true,true,true,true,"NotSupportedError","NotSupportedError","NotSupportedError","NotSupportedError"]"#
    );
}

#[test]
fn custom_elements_are_freed_when_unreachable() {
    let js = page("");
    let nodes = |js: &JsRuntime| js.with_document(Document::len);
    run(
        &js,
        "customElements.define('x-log', class extends Logged {});",
    );
    let base = nodes(&js);
    run(
        &js,
        "globalThis.keep = document.createElement('div'); \
         for (let i = 0; i < 100; i++) keep.append(document.createElement('x-log')); \
         document.body.append(keep); keep.remove();",
    );
    js.run_gc();
    assert!(nodes(&js) >= base + 100);
    run(&js, "delete globalThis.keep; take();");
    js.run_gc();
    run(&js, "document.body.append(document.createElement('p'))");
    assert!(
        nodes(&js) <= base + 2,
        "{} nodes, {base} at start",
        nodes(&js)
    );
}
