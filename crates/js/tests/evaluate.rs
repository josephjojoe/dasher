//! Host evaluation that calls functions and waits for promises.

use dasher_js::{ClockMode, ErrorKind, Evaluation, JsRuntime, RunLimits, RuntimeOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Virtual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn ready(js: &JsRuntime, source: &str) -> Option<String> {
    match js.evaluate(source).unwrap() {
        Evaluation::Ready(value) => value,
        Evaluation::Pending(_) => panic!("{source}: still pending"),
    }
}

#[test]
fn plain_values_are_ready_at_once() {
    let js = runtime();
    assert_eq!(ready(&js, "1 + 1").as_deref(), Some("2"));
    assert_eq!(
        ready(&js, "var x = {a: [1, 'b']}; x").as_deref(),
        Some(r#"{"a":[1,"b"]}"#)
    );
    assert_eq!(ready(&js, "undefined"), None);
    assert_eq!(ready(&js, "Symbol()"), None);
}

#[test]
fn functions_are_called() {
    let js = runtime();
    assert_eq!(ready(&js, "() => 6 * 7").as_deref(), Some("42"));
    assert_eq!(
        ready(&js, "function f() { return 'named'; }; f").as_deref(),
        Some(r#""named""#)
    );
}

#[test]
fn promises_settled_by_microtasks_are_ready() {
    let js = runtime();
    assert_eq!(
        ready(&js, "Promise.resolve([1, 2])").as_deref(),
        Some("[1,2]")
    );
    assert_eq!(
        ready(&js, "async () => { await null; return 'done'; }").as_deref(),
        Some(r#""done""#)
    );
}

#[test]
fn errors_are_returned_not_logged() {
    let js = runtime();
    let thrown = js.evaluate("throw new TypeError('bad')").unwrap_err();
    assert_eq!(thrown.kind, ErrorKind::Uncaught);
    assert_eq!(thrown.message, "TypeError: bad");

    let rejected = js
        .evaluate("async () => { throw new RangeError('no'); }")
        .unwrap_err();
    assert_eq!(rejected.message, "RangeError: no");

    let cyclic = js.evaluate("var o = {}; o.o = o; o").unwrap_err();
    assert!(cyclic.message.starts_with("TypeError"), "{cyclic:?}");
    let async_bigint = js
        .evaluate("Promise.resolve(1n)")
        .expect_err("BigInt cannot be serialized");
    assert!(async_bigint.message.starts_with("TypeError"));

    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[tokio::test]
async fn pending_promises_settle_as_the_event_loop_runs() {
    let js = runtime();
    let Evaluation::Pending(pending) = js
        .evaluate("new Promise((resolve) => setTimeout(() => resolve({late: true}), 5000))")
        .unwrap()
    else {
        panic!("a timer cannot have fired yet");
    };
    assert!(pending.poll().is_none());
    js.run(RunLimits::default()).await;
    assert_eq!(
        pending.poll().unwrap().unwrap().as_deref(),
        Some(r#"{"late":true}"#)
    );
    assert!(pending.poll().is_none(), "the outcome is handed over once");

    let Evaluation::Pending(pending) = js
        .evaluate("new Promise((_, reject) => setTimeout(() => reject(new Error('late')), 10))")
        .unwrap()
    else {
        panic!("still pending");
    };
    js.run(RunLimits::default()).await;
    assert_eq!(pending.poll().unwrap().unwrap_err().message, "Error: late");
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn dropping_a_pending_evaluation_is_harmless() {
    let js = runtime();
    let evaluation = js.evaluate("new Promise(() => {})").unwrap();
    assert!(matches!(evaluation, Evaluation::Pending(_)));
    drop(evaluation);
    drop(js);
}
