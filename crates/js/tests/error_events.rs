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
