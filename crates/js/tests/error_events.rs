//! `error`, `unhandledrejection` and `rejectionhandled` events at the
//! global.

use std::time::Duration;

use dasher_js::{ClockMode, ErrorKind, JsRuntime, RuntimeOptions};

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

fn messages(js: &JsRuntime) -> Vec<(ErrorKind, String)> {
    js.take_errors()
        .into_iter()
        .map(|e| (e.kind, e.message))
        .collect()
}

#[test]
fn onerror_gets_the_legacy_arguments_and_can_handle_the_error() {
    let js = manual();
    js.run_script(
        r#"
        var seen = [];
        window.onerror = function (message, filename, lineno, colno, error) {
            seen.push({ message, filename, lineno, colno, isError: error instanceof TypeError,
                        self: this === window, argc: arguments.length });
            return true;
        };
        "#,
        "setup.js",
    )
    .unwrap();
    let err = js
        .run_script(
            "function boom() {\n  null.x;\n}\nboom();",
            "https://a.test/app.js",
        )
        .unwrap_err();
    // The host still learns the script failed, but the page handled it.
    assert_eq!(err.kind, ErrorKind::Uncaught);
    assert!(js.errors().is_empty(), "{:?}", js.errors());
    assert_eq!(
        eval(&js, "seen"),
        r#"[{"message":"Uncaught TypeError: cannot read property 'x' of null","filename":"https://a.test/app.js","lineno":2,"colno":3,"isError":true,"self":true,"argc":5}]"#
    );
}

#[test]
fn listeners_see_a_trusted_cancelable_error_event() {
    let js = manual();
    js.run_script(
        r#"
        var seen = [];
        addEventListener('error', (e) => {
            seen.push({ type: e.type, ctor: e.constructor.name, trusted: e.isTrusted,
                        cancelable: e.cancelable, target: e.target === window,
                        message: e.message, error: e.error });
        });
        setTimeout(() => { throw 'from timer'; });
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "seen"),
        r#"[{"type":"error","ctor":"ErrorEvent","trusted":true,"cancelable":true,"target":true,"message":"Uncaught from timer","error":"from timer"}]"#
    );
    // Nobody canceled it, so it is still an uncaught error.
    assert_eq!(
        messages(&js),
        [(ErrorKind::Uncaught, "from timer".to_owned())]
    );
}

#[test]
fn prevent_default_handles_errors_from_every_kind_of_task() {
    let js = manual();
    js.run_script(
        r#"
        var caught = [];
        addEventListener('error', (e) => { caught.push(e.error.message); e.preventDefault(); });
        setTimeout(() => { throw new Error('timer'); });
        requestAnimationFrame(() => { throw new Error('frame'); });
        queueMicrotask(() => { throw new Error('microtask'); });
        const t = new EventTarget();
        t.addEventListener('ping', () => { throw new Error('listener'); });
        t.dispatchEvent(new Event('ping'));
        reportError(new Error('reported'));
        throw new Error('script');
        "#,
        "a.js",
    )
    .unwrap_err();
    js.queue_task(|ctx| ctx.eval::<(), _>("throw new Error('host task')"));
    js.advance(Duration::from_millis(100)).unwrap();
    assert_eq!(
        eval(&js, "caught"),
        r#"["listener","reported","script","microtask","host task","timer","frame"]"#
    );
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn a_throwing_error_handler_does_not_recurse() {
    let js = manual();
    js.run_script(
        r#"
        var calls = 0;
        onerror = () => { calls++; throw new Error('handler failed'); };
        addEventListener('error', (e) => { calls++; reportError(new Error('reported inside')); });
        "#,
        "a.js",
    )
    .unwrap();
    js.run_script("throw new Error('original')", "b.js")
        .unwrap_err();
    assert_eq!(eval(&js, "calls"), "2");
    assert_eq!(
        messages(&js),
        [
            (ErrorKind::Uncaught, "Error: handler failed".to_owned()),
            (ErrorKind::Uncaught, "Error: reported inside".to_owned()),
            (ErrorKind::Uncaught, "Error: original".to_owned()),
        ]
    );
    // The next error is dispatched as usual.
    js.run_script("throw new Error('again')", "c.js")
        .unwrap_err();
    assert_eq!(eval(&js, "calls"), "4");
}

#[test]
fn errors_without_a_stack_have_no_location() {
    let js = manual();
    js.run_script(
        r#"
        var seen;
        onerror = (message, filename, lineno, colno, error) => {
            seen = { message, filename, lineno, colno, error };
            return true;
        };
        "#,
        "a.js",
    )
    .unwrap();
    js.run_script("throw 42", "b.js").unwrap_err();
    assert_eq!(
        eval(&js, "seen"),
        r#"{"message":"Uncaught 42","filename":"","lineno":0,"colno":0,"error":42}"#
    );
    assert!(js.errors().is_empty());
}

#[test]
fn eval_json_does_not_fire_error_events() {
    let js = manual();
    js.run_script("var fired = 0; onerror = () => { fired++; };", "a.js")
        .unwrap();
    js.eval_json("nope()").unwrap_err();
    assert_eq!(eval(&js, "fired"), "0");
}

#[test]
fn unhandled_rejections_fire_a_cancelable_event_first() {
    let js = manual();
    js.run_script(
        r#"
        var seen = [];
        addEventListener('unhandledrejection', (e) => {
            seen.push({ ctor: e.constructor.name, trusted: e.isTrusted, cancelable: e.cancelable,
                        isPromise: e.promise instanceof Promise, reason: e.reason.message });
            if (e.reason.message === 'handled') e.preventDefault();
        });
        Promise.reject(new Error('handled'));
        (async () => { throw new Error('lost'); })();
        const caught = Promise.reject(new Error('caught'));
        caught.catch(() => {});
        "#,
        "a.js",
    )
    .unwrap();
    assert_eq!(
        eval(&js, "seen"),
        r#"[{"ctor":"PromiseRejectionEvent","trusted":true,"cancelable":true,"isPromise":true,"reason":"handled"},{"ctor":"PromiseRejectionEvent","trusted":true,"cancelable":true,"isPromise":true,"reason":"lost"}]"#
    );
    assert_eq!(
        messages(&js),
        [(ErrorKind::UnhandledRejection, "Error: lost".to_owned())]
    );
}

#[test]
fn onunhandledrejection_returning_false_handles_the_rejection() {
    let js = manual();
    js.run_script(
        r#"
        var reasons = [];
        onunhandledrejection = (e) => { reasons.push(e.reason); return false; };
        setTimeout(() => Promise.reject('in a timer'));
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "reasons"), r#"["in a timer"]"#);
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn listener_microtasks_run_and_their_rejections_are_logged_without_looping() {
    let js = manual();
    js.run_script(
        r#"
        var events = 0, after = false;
        // An async listener that fails rejects a new promise each time.
        addEventListener('unhandledrejection', async (e) => {
            events++;
            await null;
            after = true;
            throw new Error('listener failed');
        });
        Promise.reject(new Error('first'));
        "#,
        "a.js",
    )
    .unwrap();
    assert!(!js.has_pending_microtasks());
    assert_eq!(eval(&js, "[events, after]"), "[1,true]");
    assert_eq!(
        messages(&js),
        [
            (ErrorKind::UnhandledRejection, "Error: first".to_owned()),
            (
                ErrorKind::UnhandledRejection,
                "Error: listener failed".to_owned()
            ),
        ]
    );
}

#[test]
fn a_throwing_rejection_listener_fires_an_error_event() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        onunhandledrejection = (e) => { log.push('rejection'); throw new Error('oops'); };
        onerror = (message) => { log.push(message); return true; };
        Promise.reject(1);
        "#,
        "a.js",
    )
    .unwrap();
    assert_eq!(eval(&js, "log"), r#"["rejection","Uncaught Error: oops"]"#);
    assert_eq!(
        messages(&js),
        [(ErrorKind::UnhandledRejection, "1".to_owned())]
    );
}

#[test]
fn late_handlers_fire_rejectionhandled_in_a_task() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        addEventListener('unhandledrejection', (e) => log.push('unhandled:' + e.reason));
        onrejectionhandled = (e) => {
            log.push('handled:' + e.reason + ':' + (e.promise === p) + ':' + e.cancelable +
                     ':' + e.constructor.name);
        };
        var p = Promise.reject('r');
        setTimeout(() => { p.catch(() => {}); log.push('attached'); });
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "log"),
        r#"["unhandled:r","attached","handled:r:true:false:PromiseRejectionEvent"]"#
    );
    // The rejection was logged when it went unhandled; that stays.
    assert_eq!(
        messages(&js),
        [(ErrorKind::UnhandledRejection, "r".to_owned())]
    );
    // A second handler is not news.
    js.run_script("p.catch(() => {});", "b.js").unwrap();
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "log.length"), "3");
}

#[test]
fn handling_a_rejection_during_its_event_fires_no_rejectionhandled() {
    let js = manual();
    js.run_script(
        r#"
        var log = [];
        addEventListener('unhandledrejection', (e) => {
            e.promise.catch(() => {});
            e.preventDefault();
            log.push('unhandled');
        });
        addEventListener('rejectionhandled', () => log.push('handled'));
        Promise.reject('r');
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "log"), r#"["unhandled"]"#);
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn a_hanging_error_handler_is_cut_off_by_the_deadline() {
    let js = manual();
    js.run_script(
        "onerror = () => { for (;;) {} }; onunhandledrejection = () => { for (;;) {} };",
        "a.js",
    )
    .unwrap();
    js.set_deadline(Some(std::time::Instant::now() + Duration::from_millis(50)));
    js.run_script(
        "Promise.reject(new Error('r')); throw new Error('t');",
        "b.js",
    )
    .unwrap_err();
    // Neither handler finished, so neither error counts as handled.
    let kinds: Vec<_> = js.take_errors().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&ErrorKind::Uncaught), "{kinds:?}");
}
