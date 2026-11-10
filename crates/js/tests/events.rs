//! DOM events: Event, EventTarget, DOMException and abort signals.

use std::time::Duration;

use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn manual() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

#[test]
fn listeners_run_in_order_with_options() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const t = new EventTarget(), log = [];
        const a = (e) => log.push('a:' + e.type + ':' + (e.currentTarget === t) + ':' + e.eventPhase);
        t.addEventListener('x', a);
        t.addEventListener('x', a); // duplicate, ignored
        t.addEventListener('x', () => log.push('once'), { once: true });
        t.addEventListener('x', { handleEvent(e) { log.push('obj:' + (this !== t)); } });
        t.addEventListener('x', (e) => { e.preventDefault(); log.push('passive:' + e.defaultPrevented); }, { passive: true });
        const r1 = t.dispatchEvent(new Event('x', { cancelable: true }));
        const r2 = t.dispatchEvent(new Event('x', { cancelable: true }));
        t.removeEventListener('x', a);
        t.dispatchEvent(new Event('x'));
        ({ log, r1, r2 })
        "#,
    );
    assert_eq!(
        out,
        r#"{"log":["a:x:true:2","once","obj:true","passive:false","a:x:true:2","obj:true","passive:false","obj:true","passive:false"],"r1":true,"r2":true}"#
    );
}

#[test]
fn prevent_default_and_stop_immediate_propagation() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const t = new EventTarget(), log = [];
        t.addEventListener('x', (e) => { e.preventDefault(); e.stopImmediatePropagation(); log.push(1); });
        t.addEventListener('x', () => log.push(2));
        const e = new Event('x', { cancelable: true });
        const result = t.dispatchEvent(e);
        ({ log, result, prevented: e.defaultPrevented, target: e.target === t, phase: e.eventPhase,
           trusted: e.isTrusted, current: e.currentTarget })
        "#,
    );
    assert_eq!(
        out,
        r#"{"log":[1],"result":false,"prevented":true,"target":true,"phase":0,"trusted":false,"current":null}"#
    );
}

#[test]
fn listener_exceptions_are_reported_and_dispatch_continues() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const t = new EventTarget(); let ran = false;
        t.addEventListener('x', () => { throw new Error('listener failed'); });
        t.addEventListener('x', () => { ran = true; });
        t.dispatchEvent(new Event('x'));
        ran
        "#,
    );
    assert_eq!(out, "true");
    let errors = js.take_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Error: listener failed");
}

#[test]
fn subclasses_and_custom_events() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        class Thing extends EventTarget {}
        const t = new Thing(); let detail;
        t.addEventListener('ping', (e) => { detail = e.detail; });
        t.dispatchEvent(new CustomEvent('ping', { detail: { n: 1 } }));
        const err = new ErrorEvent('error', { message: 'm', lineno: 3 });
        ({ detail, tag: Object.prototype.toString.call(t), isEvent: err instanceof Event,
           msg: err.message, line: err.lineno, col: err.colno,
           ctorName: CustomEvent.name, enumerable: Object.keys(globalThis).includes('EventTarget') })
        "#,
    );
    assert_eq!(
        out,
        r#"{"detail":{"n":1},"tag":"[object EventTarget]","isEvent":true,"msg":"m","line":3,"col":0,"ctorName":"CustomEvent","enumerable":false}"#
    );
}

#[test]
fn event_handler_attributes() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const c = new AbortController(), log = [];
        c.signal.addEventListener('abort', () => log.push('first'));
        c.signal.onabort = () => log.push('old');
        c.signal.onabort = (e) => log.push('handler:' + e.type);
        c.signal.addEventListener('abort', () => log.push('last'));
        c.abort();
        ({ log, handler: typeof c.signal.onabort })
        "#,
    );
    assert_eq!(
        out,
        r#"{"log":["first","handler:abort","last"],"handler":"function"}"#
    );
}

#[test]
fn dom_exception() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const e = new DOMException('nope', 'AbortError');
        ({ name: e.name, message: e.message, code: e.code, isError: e instanceof Error,
           str: String(e), constant: DOMException.ABORT_ERR, plain: new DOMException().name,
           stack: typeof e.stack })
        "#,
    );
    assert_eq!(
        out,
        r#"{"name":"AbortError","message":"nope","code":20,"isError":true,"str":"AbortError: nope","constant":20,"plain":"Error","stack":"string"}"#
    );
    let err = js.run_script("throw new DOMException('bad', 'SyntaxError')", "a.js");
    assert_eq!(err.unwrap_err().message, "SyntaxError: bad");
}

#[test]
fn abort_controller_and_signals() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const c = new AbortController(), t = new EventTarget(), log = [];
        t.addEventListener('x', () => log.push('x'), { signal: c.signal });
        const any = AbortSignal.any([c.signal, new AbortController().signal]);
        any.addEventListener('abort', () => log.push('any'));
        t.dispatchEvent(new Event('x'));
        c.abort();
        c.abort('again');
        t.dispatchEvent(new Event('x'));
        let thrown;
        try { c.signal.throwIfAborted(); } catch (e) { thrown = e.name; }
        const pre = AbortSignal.abort('why');
        let illegal;
        try { new AbortSignal(); } catch (e) { illegal = e instanceof TypeError; }
        ({ log, aborted: c.signal.aborted, reason: c.signal.reason.name, thrown,
           anyReason: any.reason === c.signal.reason, pre: [pre.aborted, pre.reason], illegal })
        "#,
    );
    assert_eq!(
        out,
        r#"{"log":["x","any"],"aborted":true,"reason":"AbortError","thrown":"AbortError","anyReason":true,"pre":[true,"why"],"illegal":true}"#
    );
}

#[test]
fn abort_signal_timeout_uses_the_page_clock() {
    let js = manual();
    js.run_script(
        "var s = AbortSignal.timeout(500); s.onabort = () => console.log(s.reason.name);",
        "a.js",
    )
    .unwrap();
    js.advance(Duration::from_millis(499)).unwrap();
    assert!(js.take_console().is_empty());
    js.advance(Duration::from_millis(1)).unwrap();
    assert_eq!(js.take_console()[0].text, "TimeoutError");
}

#[test]
fn dispatch_rejects_bad_input() {
    let js = manual();
    let out = eval(
        &js,
        r#"
        const t = new EventTarget(), errs = [];
        try { t.dispatchEvent({ type: 'x' }); } catch (e) { errs.push(e.name); }
        try { new Event(); } catch (e) { errs.push(e.name); }
        try { t.addEventListener('x', 5); } catch (e) { errs.push(e.name); }
        t.addEventListener('x', (e) => { try { t.dispatchEvent(e); } catch (err) { errs.push(err.name); } });
        t.dispatchEvent(new Event('x'));
        errs
        "#,
    );
    assert_eq!(
        out,
        r#"["TypeError","TypeError","TypeError","InvalidStateError"]"#
    );
}
