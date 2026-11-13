//! The native event machinery (src/events): what scripts see of event
//! targets and events, listener changes during dispatch, receivers, abort
//! signals and garbage collection of listeners.

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn manual() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn page(html: &str) -> JsRuntime {
    let js = manual();
    js.set_document(Document::parse(html));
    js
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

#[test]
fn listeners_added_or_removed_during_dispatch() {
    let js = page("<div id=a><span id=b></span></div>");
    let out = eval(
        &js,
        r#"
        const a = document.getElementById('a'), b = document.getElementById('b'), log = [];
        const late = () => log.push('late');
        const gone = () => log.push('gone');
        b.addEventListener('x', () => {
          log.push('b1');
          // Added to the target being invoked: not run this time.
          b.addEventListener('x', late);
          // Removed before its turn: skipped.
          b.removeEventListener('x', gone);
          // Added further along the path: run.
          a.addEventListener('x', () => log.push('a-added'));
        });
        b.addEventListener('x', gone);
        b.addEventListener('x', () => log.push('b3'));
        b.dispatchEvent(new Event('x', { bubbles: true }));
        log.push('|');
        b.dispatchEvent(new Event('x', { bubbles: true }));
        log
        "#,
    );
    assert_eq!(
        out,
        r#"["b1","b3","a-added","|","b1","b3","late","a-added","a-added"]"#
    );
}

#[test]
fn phases_targets_and_stopping() {
    let js = page("<div id=a><p id=b><span id=c></span></p></div>");
    let out = eval(
        &js,
        r#"
        const [a, b, c] = ['a', 'b', 'c'].map((id) => document.getElementById(id)), log = [];
        const note = (name) => (e) => log.push([name, e.eventPhase, e.currentTarget.id || 'w', e.target.id]);
        addEventListener('x', note('window-capture'), true);
        a.addEventListener('x', note('a-capture'), { capture: true });
        c.addEventListener('x', note('c'));
        c.addEventListener('x', note('c-capture'), true);
        b.addEventListener('x', note('b'));
        a.addEventListener('x', (e) => { note('a')(e); e.stopPropagation(); });
        document.addEventListener('x', note('document'));
        const e = new Event('x', { bubbles: true });
        c.dispatchEvent(e);
        const path = [];
        c.addEventListener('y', (e) => path.push(...e.composedPath().map((n) => n.nodeName || 'window')));
        c.dispatchEvent(new Event('y'));
        ({ log, after: [e.eventPhase, e.currentTarget, e.target === c, e.composedPath().length], path })
        "#,
    );
    assert_eq!(
        out,
        concat!(
            // At the target, capture listeners go first, as the spec has it.
            r#"{"log":[["window-capture",1,"w","c"],["a-capture",1,"a","c"],["c-capture",2,"c","c"],"#,
            r#"["c",2,"c","c"],["b",3,"b","c"],["a",3,"a","c"]],"#,
            r##""after":[0,null,true,0],"path":["SPAN","P","DIV","BODY","HTML","#document","window"]}"##
        )
    );
}

#[test]
fn event_objects() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const errs = [];
        try { Event('x'); } catch (e) { errs.push(e.constructor.name); }
        try { EventTarget(); } catch (e) { errs.push(e.constructor.name); }
        try { new Event('x', 5); } catch (e) { errs.push(e.constructor.name); }
        class Ping extends Event {
          constructor() { super('ping', { cancelable: true }); this.extra = 1; }
        }
        const p = new Ping();
        const d = Object.getOwnPropertyDescriptor(Event.prototype, 'type');
        const m = Object.getOwnPropertyDescriptor(EventTarget.prototype, 'addEventListener');
        const e = new Event('a');
        e.initEvent('b', true, true);
        e.returnValue = false;
        e.cancelBubble = true;
        const t = new EventTarget();
        let inside;
        t.addEventListener('b', (ev) => { inside = [ev.cancelBubble, ev.defaultPrevented]; });
        const r = t.dispatchEvent(e);
        ({
          errs,
          ping: [p instanceof Ping, p instanceof Event, p.type, p.cancelable, p.extra, p.isTrusted,
                 Object.prototype.toString.call(p), p.eventPhase === Event.NONE],
          desc: [typeof d.get, d.set, d.enumerable, d.configurable, m.enumerable, m.writable,
                 EventTarget.prototype.addEventListener.length, Event.length,
                 String(Event).includes('[native code]')],
          init: [e.type, e.bubbles, e.cancelable, e.defaultPrevented, inside, r, e.cancelBubble],
          time: typeof e.timeStamp,
        })
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"{"errs":["TypeError","TypeError","TypeError"],"#,
            r#""ping":[true,true,"ping",true,1,false,"[object Event]",true],"#,
            r#""desc":["function",null,true,true,true,true,2,1,true],"#,
            // Stopped before dispatch, so no listener ran; the flag is
            // cleared after.
            r#""init":["b",true,true,true,null,false,false],"time":"number"}"#
        )
    );
}

#[test]
fn receivers() {
    let js = page("<div id=a title=t></div>");
    let out = eval(
        &js,
        r#"
        const errs = [];
        const add = EventTarget.prototype.addEventListener;
        try { add.call({}, 'x', () => {}); } catch (e) { errs.push(e.message); }
        try { Object.getOwnPropertyDescriptor(Event.prototype, 'type').get.call({}); } catch (e) { errs.push(e.message); }
        try { EventTarget.prototype.dispatchEvent.call(new EventTarget(), {}); } catch (e) { errs.push(e.constructor.name); }
        // A bare call targets the window, as in browsers.
        let heard = 0;
        add('bare', () => heard++);
        dispatchEvent(new Event('bare'));
        // Attr objects are event targets too.
        const attr = document.getElementById('a').getAttributeNode('title');
        attr.addEventListener('x', () => heard++);
        attr.dispatchEvent(new Event('x'));
        ({ errs, heard, attr: attr instanceof EventTarget })
        "#,
    );
    assert_eq!(
        out,
        r#"{"errs":["Illegal invocation","Illegal invocation","TypeError"],"heard":2,"attr":true}"#
    );
}

#[test]
fn once_passive_handle_event_and_signals() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const t = new EventTarget(), log = [];
        const c = new AbortController();
        t.addEventListener('x', () => log.push('once'), { once: true });
        t.addEventListener('x', (e) => { e.preventDefault(); log.push('passive:' + e.defaultPrevented); }, { passive: true });
        t.addEventListener('x', { handleEvent(e) { log.push('object:' + (this !== t)); } });
        t.addEventListener('x', {});
        t.addEventListener('x', () => log.push('signal'), { signal: c.signal });
        t.addEventListener('x', () => log.push('never'), { signal: AbortSignal.abort() });
        // The signal's own abort listener goes before abort fires.
        c.signal.addEventListener('abort', () => log.push('abort-self'), { signal: c.signal });
        c.signal.addEventListener('abort', () => log.push('abort'));
        t.dispatchEvent(new Event('x', { cancelable: true }));
        c.abort();
        t.dispatchEvent(new Event('x'));
        let bad;
        try { t.addEventListener('x', () => {}, { signal: {} }); } catch (e) { bad = e.constructor.name; }
        ({ log, bad })
        "#,
    );
    assert_eq!(
        out,
        concat!(
            r#"{"log":["once","passive:false","object:true","signal","abort","#,
            r#""passive:false","object:true"],"bad":"TypeError"}"#
        )
    );
    // The `{}` listener has no handleEvent: reported, and dispatch went on.
    let errors = js.take_errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].message.contains("handleEvent"), "{errors:?}");
}

#[test]
fn listeners_live_as_long_as_their_wrapper_group() {
    let js = page("<div id=a></div>");
    js.run_script(
        r#"
        var log = [];
        var weak;
        (() => {
          // A detached node whose listener refers back to it: a cycle.
          const d = document.createElement('div');
          d.addEventListener('x', () => d.id);
          weak = new WeakRef(d);
          // A connected node whose wrapper nothing else holds.
          const span = document.createElement('span');
          span.addEventListener('x', () => log.push('kept'));
          document.getElementById('a').append(span);
        })();
        "#,
        "a.js",
    )
    .unwrap();
    js.run_gc();
    js.run_gc();
    let out = eval(
        &js,
        r#"
        document.querySelector('span').dispatchEvent(new Event('x'));
        [weak.deref() === undefined, log]
        "#,
    );
    assert_eq!(out, r#"[true,["kept"]]"#);
}

#[test]
fn handler_attributes_on_nodes_scripts_never_touched() {
    // The host fires at nodes with no wrapper: their handler attributes
    // still run.
    let js =
        page(r#"<div onclick="window.log = (window.log || []).concat('div')"><p id=p></p></div>"#);
    let out = eval(
        &js,
        r#"
        document.getElementById('p').dispatchEvent(new MouseEvent('click', { bubbles: true }));
        log
        "#,
    );
    assert_eq!(out, r#"["div"]"#);
}
