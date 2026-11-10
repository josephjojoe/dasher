//! Console capture.

use std::time::Duration;

use dasher_js::{ClockMode, ConsoleLevel, JsRuntime, RuntimeOptions};

fn manual() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn texts(js: &JsRuntime, script: &str) -> Vec<String> {
    js.run_script(script, "console.js").unwrap();
    js.take_console().into_iter().map(|m| m.text).collect()
}

#[test]
fn captures_levels_and_page_time() {
    let js = manual();
    js.run_script(
        "console.log('a'); console.info('b'); console.warn('c'); console.error('d'); console.debug('e'); setTimeout(() => console.log('late'), 1500);",
        "a.js",
    )
    .unwrap();
    js.advance(Duration::from_secs(2)).unwrap();
    let log: Vec<_> = js
        .take_console()
        .into_iter()
        .map(|m| (m.level, m.text, m.time))
        .collect();
    let at0 = Duration::ZERO;
    assert_eq!(
        log,
        [
            (ConsoleLevel::Log, "a".into(), at0),
            (ConsoleLevel::Info, "b".into(), at0),
            (ConsoleLevel::Warn, "c".into(), at0),
            (ConsoleLevel::Error, "d".into(), at0),
            (ConsoleLevel::Debug, "e".into(), at0),
            (
                ConsoleLevel::Log,
                "late".into(),
                Duration::from_millis(1500)
            ),
        ]
    );
    assert!(js.console().is_empty());
}

#[test]
fn renders_values_like_a_browser_console() {
    let js = manual();
    let out = texts(
        &js,
        r#"
        console.log('str', 42, -0, 10n, true, null, undefined, Symbol('s'));
        console.log({a: 1, 'b-c': 'x', nested: {deep: {deeper: {deepest: 1}}}});
        console.log([1, 'two', [3]], new Map([['k', 1]]), new Set([1]));
        console.log(function named() {}, () => {}, class Foo {});
        const cyc = {}; cyc.self = cyc; console.log(cyc);
        class Point { constructor() { this.x = 1; } } console.log(new Point());
        console.log(new Date(0), /re/g, Promise.resolve());
        console.log(Object.create(null));
        console.log(Array.from({length: 102}, (_, i) => i).length, Array(150).fill(0));
        "#,
    );
    assert_eq!(out[0], "str 42 -0 10n true null undefined Symbol(s)");
    assert_eq!(
        out[1],
        r#"{a: 1, "b-c": "x", nested: {deep: {deeper: [Object]}}}"#
    );
    assert_eq!(out[2], r#"[1, "two", [3]] Map(1) {"k" => 1} Set(1) {1}"#);
    assert_eq!(
        out[3],
        "[Function: named] [Function: (anonymous)] [class Foo]"
    );
    assert_eq!(out[4], "{self: [Circular]}");
    assert_eq!(out[5], "Point {x: 1}");
    assert_eq!(out[6], "1970-01-01T00:00:00.000Z /re/g Promise {}");
    assert_eq!(out[7], "[Object: null prototype] {}");
    assert!(out[8].starts_with("102 [0, 0,"), "{}", out[8]);
    assert!(out[8].ends_with(", ... 50 more items]"), "{}", out[8]);
}

#[test]
fn renders_errors_with_their_stack() {
    let js = manual();
    let out = texts(
        &js,
        "function f() { console.error(new TypeError('bad')); } f();",
    );
    assert!(out[0].starts_with("TypeError: bad\n"), "{}", out[0]);
    assert!(out[0].contains("at f"), "{}", out[0]);
}

#[test]
fn applies_format_specifiers() {
    let js = manual();
    let out = texts(
        &js,
        r#"
        console.log('%s is %d years and %i days, %f%%', 'Bob', 42.9, '7', 1.5);
        console.log('%o and %O and %j', 'q', {a: 1}, [1]);
        console.log('%cstyled', 'color: red', 'extra');
        console.log('%s missing %s', 'one');
        console.log('100%', 'sure');
        console.log('%d', Symbol('x'));
        "#,
    );
    assert_eq!(
        out,
        [
            "Bob is 42 years and 7 days, 1.5%",
            r#""q" and {a: 1} and [1]"#,
            "styled extra",
            "one missing %s",
            "100% sure",
            "NaN",
        ]
    );
}

#[test]
fn assert_count_time_group_and_trace() {
    let js = manual();
    js.run_script(
        r#"
        console.assert(true, 'hidden');
        console.assert(false, 'shown %s', 'x');
        console.assert(0);
        console.count(); console.count(); console.count('x'); console.countReset(); console.count();
        console.time('t');
        setTimeout(() => { console.timeLog('t', 'mid'); console.timeEnd('t'); console.timeEnd('t'); }, 250);
        console.group('outer');
        console.log('in\nside');
        console.groupCollapsed();
        console.log('deeper');
        console.groupEnd(); console.groupEnd(); console.groupEnd();
        console.log('out');
        (function traced() { console.trace('here'); })();
        console.dir('quoted');
        "#,
        "a.js",
    )
    .unwrap();
    js.advance(Duration::from_secs(1)).unwrap();
    let log = js.take_console();
    let texts: Vec<_> = log.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(log[0].level, ConsoleLevel::Error);
    assert_eq!(
        texts[..13],
        [
            "Assertion failed: shown x",
            "Assertion failed",
            "default: 1",
            "default: 2",
            "x: 1",
            "default: 1",
            "outer",
            "  in\n  side",
            "    deeper",
            "out",
            texts[10], // the trace, checked below
            r#""quoted""#,
            "t: 250 ms mid",
        ]
    );
    assert!(texts[10].starts_with("Trace: here\n"), "{}", texts[10]);
    assert!(texts[10].contains("traced"), "{}", texts[10]);
    assert!(!texts[10].contains("dasher:prelude"), "{}", texts[10]);
    assert_eq!(texts[13], "t: 250 ms");
    assert_eq!(
        (log[14].level, texts[14]),
        (ConsoleLevel::Warn, "Timer 't' does not exist")
    );
}

#[test]
fn console_never_throws() {
    let js = manual();
    let out = texts(
        &js,
        r#"
        const evil = new Proxy({}, { ownKeys() { throw new Error('no'); } });
        console.log(evil);
        console.log('%d', { valueOf() { throw new Error('no'); } });
        console.log({ get boom() { throw new Error('no'); } });
        "#,
    );
    assert_eq!(out.len(), 3);
    assert_eq!(out[2], r#"{boom: "[Getter threw]"}"#);
    assert!(js.errors().is_empty());
}

#[test]
fn console_log_is_bounded_and_messages_are_truncated() {
    let js = JsRuntime::new(RuntimeOptions {
        console_capacity: 2,
        max_console_message_len: 5,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.run_script(
        "for (let i = 0; i < 5; i++) console.log('message ' + i);",
        "a.js",
    )
    .unwrap();
    let texts: Vec<_> = js.console().into_iter().map(|m| m.text).collect();
    assert_eq!(texts, ["messa…", "messa…"]);
    assert_eq!(js.dropped_console_messages(), 3);
}
