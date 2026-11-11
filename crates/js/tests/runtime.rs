//! Runtime limits, interrupts and error capture.

use std::time::{Duration, Instant};

use dasher_js::{ErrorKind, JsRuntime, RuntimeOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

#[test]
fn runs_scripts_and_returns_json() {
    let js = runtime();
    js.run_script("var x = 40;", "a.js").unwrap();
    js.run_script("x += 2;", "b.js").unwrap();
    assert_eq!(
        js.eval_json("({x, s: 'hi'})").unwrap().as_deref(),
        Some(r#"{"x":42,"s":"hi"}"#)
    );
    assert_eq!(js.eval_json("undefined").unwrap(), None);
    assert!(js.errors().is_empty());
}

#[test]
fn evaluates_conditions_as_truthy_or_not() {
    let js = runtime();
    for (source, truthy) in [
        ("1 + 1 === 2", true),
        ("[]", true),
        ("'x'", true),
        ("0", false),
        ("''", false),
        ("undefined", false),
        ("NaN", false),
        ("let n = 3; n > 2", true),
    ] {
        assert_eq!(js.eval_truthy(source).unwrap(), truthy, "{source}");
    }
    let error = js.eval_truthy("missing.value").unwrap_err();
    assert!(error.message.contains("missing"), "{error}");
    assert!(js.errors().is_empty());
}

#[test]
fn uncaught_exceptions_are_captured_with_stack() {
    let js = runtime();
    let err = js
        .run_script(
            "function boom() { throw new TypeError('bad'); }\nboom();",
            "page.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Uncaught);
    assert_eq!(err.message, "TypeError: bad");
    let stack = err.stack.as_deref().unwrap();
    assert!(
        stack.contains("boom") && stack.contains("page.js"),
        "{stack}"
    );
    assert_eq!(js.take_errors(), [err]);
    assert!(js.errors().is_empty());

    // The runtime is still usable.
    assert_eq!(js.eval_json("1 + 1").unwrap().as_deref(), Some("2"));
}

#[test]
fn thrown_non_errors_are_rendered() {
    let js = runtime();
    let err = js
        .run_script("throw {code: 7, why: 'x'}", "a.js")
        .unwrap_err();
    assert_eq!(err.message, r#"{code: 7, why: "x"}"#);
    assert_eq!(err.stack, None);
    let err = js.run_script("throw 'plain'", "a.js").unwrap_err();
    assert_eq!(err.message, "plain");
    let err = js.run_script("throw null", "a.js").unwrap_err();
    assert_eq!(
        (err.kind, err.message.as_str()),
        (ErrorKind::Uncaught, "null")
    );
}

#[test]
fn syntax_errors_are_captured() {
    let js = runtime();
    let err = js.run_script("let = ;", "bad.js").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Uncaught);
    assert!(err.message.starts_with("SyntaxError"), "{}", err.message);
}

#[test]
fn eval_errors_are_returned_but_not_logged() {
    let js = runtime();
    let err = js.eval_json("nope()").unwrap_err();
    assert_eq!(err.message, "ReferenceError: nope is not defined");
    assert!(js.errors().is_empty());
}

#[test]
fn microtasks_run_after_scripts_and_their_errors_are_captured() {
    let js = runtime();
    js.run_script(
        "var order = []; Promise.resolve().then(() => { order.push('micro'); throw new Error('in job'); }); order.push('sync');",
        "a.js",
    )
    .unwrap();
    assert_eq!(
        js.eval_json("order").unwrap().as_deref(),
        Some(r#"["sync","micro"]"#)
    );
    let errors = js.take_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, ErrorKind::UnhandledRejection);
    assert_eq!(errors[0].message, "Error: in job");
}

#[test]
fn unhandled_rejections_are_reported_once_handled_ones_are_not() {
    let js = runtime();
    js.run_script(
        r#"
        Promise.reject(new Error('lost'));
        const p = Promise.reject(new Error('caught later'));
        Promise.resolve().then(() => p.catch(() => {}));
        (async () => { throw new RangeError('async'); })();
        "#,
        "a.js",
    )
    .unwrap();
    let messages: Vec<_> = js
        .take_errors()
        .into_iter()
        .map(|e| (e.kind, e.message))
        .collect();
    assert_eq!(
        messages,
        [
            (ErrorKind::UnhandledRejection, "Error: lost".to_owned()),
            (
                ErrorKind::UnhandledRejection,
                "RangeError: async".to_owned()
            ),
        ]
    );
}

#[test]
fn report_error_records_without_throwing() {
    let js = runtime();
    js.run_script(
        "reportError(new Error('reported')); var after = true;",
        "a.js",
    )
    .unwrap();
    assert_eq!(js.eval_json("after").unwrap().as_deref(), Some("true"));
    let errors = js.take_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Error: reported");
}

#[test]
fn deadline_interrupts_runaway_scripts() {
    let js = runtime();
    js.set_deadline(Some(Instant::now() + Duration::from_millis(50)));
    let started = Instant::now();
    let err = js.run_script("for (;;) {}", "spin.js").unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(err.kind, ErrorKind::Interrupted);
    assert_eq!(js.take_errors().len(), 1);

    // Past the deadline nothing new starts.
    let err = js.run_script("var ran = true", "late.js").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Interrupted);

    // try/catch cannot swallow the interrupt.
    js.set_deadline(Some(Instant::now() + Duration::from_millis(50)));
    let err = js
        .run_script(
            "try { for (;;) {} } catch (e) { var swallowed = true }",
            "catch.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Interrupted);

    js.set_deadline(None);
    assert_eq!(
        js.eval_json("typeof swallowed").unwrap().as_deref(),
        Some(r#""undefined""#)
    );
}

#[test]
fn interrupt_handle_stops_scripts_from_another_thread() {
    let js = runtime();
    let handle = js.interrupt_handle();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        handle.interrupt();
    });
    let err = js.run_script("while (true) {}", "spin.js").unwrap_err();
    assert_eq!(err.kind, ErrorKind::Interrupted);
    killer.join().unwrap();

    assert!(js.eval_json("1").is_err());
    js.clear_interrupt();
    assert_eq!(js.eval_json("1").unwrap().as_deref(), Some("1"));
}

#[test]
fn memory_limit_raises_out_of_memory_instead_of_crashing() {
    let js = JsRuntime::new(RuntimeOptions {
        memory_limit: Some(8 * 1024 * 1024),
        ..RuntimeOptions::default()
    })
    .unwrap();
    let err = js
        .run_script(
            "(function () { const a = []; for (;;) a.push('x'.repeat(1024) + a.length); })();",
            "hog.js",
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
    assert!(js.memory_allocated() <= 8 * 1024 * 1024);

    // The page filled its heap, so it is stopped for good (see
    // tests/limits.rs).
    let err = js.eval_json("[1, 2].length").unwrap_err();
    assert_eq!(err.kind, ErrorKind::OutOfMemory, "{err:?}");
}

#[test]
fn stack_limit_turns_deep_recursion_into_an_error() {
    let js = JsRuntime::new(RuntimeOptions {
        max_stack_size: 256 * 1024,
        ..RuntimeOptions::default()
    })
    .unwrap();
    let err = js
        .run_script("function f(n) { return f(n + 1) + 1; } f(0);", "deep.js")
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Uncaught);
    assert!(err.message.contains("call stack"), "{}", err.message);
}

#[test]
fn error_log_is_bounded() {
    let js = JsRuntime::new(RuntimeOptions {
        error_capacity: 3,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.run_script(
        "for (let i = 0; i < 10; i++) reportError(new Error('e' + i));",
        "a.js",
    )
    .unwrap();
    let messages: Vec<_> = js.errors().into_iter().map(|e| e.message).collect();
    assert_eq!(messages, ["Error: e7", "Error: e8", "Error: e9"]);
    assert_eq!(js.dropped_errors(), 7);
}

#[test]
fn dropping_a_runtime_with_live_state_is_clean() {
    for _ in 0..20 {
        let js = runtime();
        js.run_script(
            "globalThis.keep = {self: null}; keep.self = keep; Promise.reject(new Error('x')); new Promise(() => {});",
            "a.js",
        )
        .unwrap();
        // Leave a rejection pending in the tracker when the runtime drops.
        js.with(|ctx| {
            let _ = ctx.eval::<(), _>("Promise.reject(1)");
        });
    }
}
