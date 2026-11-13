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

/// The engine notes where an error's frames were and writes the `stack`
/// text out only when it is first read (a patch in `vendor/rquickjs-sys`).
/// The text has to be what the engine would have written at the throw.
#[test]
fn error_stacks_are_written_when_first_read() {
    let js = runtime();
    js.run_script(
        "function inner() { throw new TypeError('bad'); }\n\
         function outer() { [1].map(function each() { inner(); }); }\n\
         var thrown, first, again;\n\
         try { outer(); } catch (e) { thrown = e; }\n\
         try { try { outer(); } catch (e) { first = e.stack; throw e; } }\n\
         catch (e) { again = e.stack; }\n\
         var made = (function make() { return new Error('made'); })();",
        "page.js",
    )
    .unwrap();
    assert_eq!(
        js.eval_json("thrown.stack").unwrap().as_deref(),
        Some(
            r#""    at inner (page.js:1:29)\n    at each (page.js:2:46)\n    at map (native)\n    at outer (page.js:2:28)\n    at <eval> (page.js:4:7)\n""#
        )
    );
    assert_eq!(
        js.eval_json("made.stack").unwrap().as_deref(),
        Some(r#""    at make (page.js:7:42)\n    at <eval> (page.js:7:57)\n""#)
    );
    assert_eq!(
        js.eval_json(
            "[first === again, thrown.stack === thrown.stack, \
              Object.keys(thrown), Object.getOwnPropertyDescriptor(made, 'stack').writable]"
        )
        .unwrap()
        .as_deref(),
        Some("[true,true,[],true]")
    );
    assert!(js.errors().is_empty());
}

/// An unread stack holds on to the functions that were running, so it can
/// still be written after they are otherwise unreachable, and errors that
/// are collected with their stack unread are collected cleanly.
#[test]
fn unread_error_stacks_survive_garbage_collection() {
    let js = runtime();
    js.run_script(
        "var kept = [];\n\
         for (let i = 0; i < 200; i++) {\n\
           const fail = new Function('n', 'throw new RangeError(\"no \" + n)');\n\
           try { fail(i); } catch (e) { if (i % 50 === 0) kept.push(e); e.self = e; }\n\
         }",
        "loop.js",
    )
    .unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("kept.map((e) => e.stack.split('\\n')[0].trim())")
            .unwrap()
            .as_deref(),
        Some(
            r#"["at anonymous (<input>:3:30)","at anonymous (<input>:3:30)","at anonymous (<input>:3:30)","at anonymous (<input>:3:30)"]"#
        )
    );
    js.run_script("kept = null;", "drop.js").unwrap();
    js.run_gc();
    assert!(js.errors().is_empty());
}

/// Functions compiled from one script share a single copy of its text
/// (a patch in `vendor/rquickjs-sys`), which has to outlive the script's
/// own run for as long as any of them does.
#[test]
fn function_source_text_outlives_its_script() {
    let js = runtime();
    js.run_script(
        "var keep = (function () {\n\
           class Shape { area() { return 0; } static of() { return new Shape(); } }\n\
           function outer(a) { return function inner(b) { return (c) => a + b + c; }; }\n\
           return { Shape, outer, inner: outer(1), arrow: outer(1)(2) };\n\
         })();",
        "lib.js",
    )
    .unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json(
            "[String(keep.arrow), String(keep.inner), String(keep.Shape.prototype.area), \
              String(keep.Shape), String(keep.outer)]"
        )
        .unwrap()
        .as_deref(),
        Some(
            r#"["(c) => a + b + c","function inner(b) { return (c) => a + b + c; }","area() { return 0; }","class Shape { area() { return 0; } static of() { return new Shape(); } }","function outer(a) { return function inner(b) { return (c) => a + b + c; }; }"]"#
        )
    );
    // Dropping all but one function keeps the text that function needs.
    js.run_script("keep = keep.arrow;", "drop.js").unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("String(keep)").unwrap().as_deref(),
        Some(r#""(c) => a + b + c""#)
    );
}

/// The parser decides between an arrow function and a bracketed
/// expression, and between a literal and a destructuring pattern, by
/// reading ahead to the matching bracket. A patch in `vendor/rquickjs-sys`
/// remembers what it found for the brackets inside, so each of those
/// decisions has to come out as it does when read afresh. The expected
/// values are what the engine gives without the patch.
#[test]
fn bracket_lookahead_answers_are_the_same_when_remembered() {
    let js = runtime();
    js.run_script(include_str!("fixtures/lookahead.js"), "lookahead.js")
        .unwrap();
    assert_eq!(
        js.eval_json("JSON.parse(lookahead)").unwrap().as_deref(),
        Some(
            r#"{"paren":4,"arrow":50,"newline":"function","literal":3,"object":[1,59,4],"array":[7,8,[9,10]],"params":[55,"set by a default, inside a template that is long enough"],"plain":"inner","keyed":[1,"key"],"forof":["1:59","2:1"],"strict":62,"generator":[5,1],"async":115}"#
        )
    );
}

/// Closures are made from a cached shape with `length`, `name` and
/// `prototype` stored in place (see `PATCHES.md`). Scripts see the same
/// properties, in the same order, as when each was defined in turn, and a
/// function that later gains or loses a property does not affect the next.
#[test]
fn closures_have_the_properties_scripts_expect() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/closures.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"[["length:2:001","name:decl:001","prototype:object:100"],["length:1:001","name:arrow:001"],["length:3:001","name:m:001"],["length:0:001","name:af:001"],["length:0:001","name:gen:001","prototype:object:100"],[["length:1:001","name::001","prototype:object:100","extra:1:111"],["length:1:001","prototype:object:100"],["length:1:001","name::001","prototype:null:100"]],[0,1,2],true,true,true,true]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// A sloppy function's `arguments` is kept as an array when no argument
/// has a named parameter to stay in step with, and mapped otherwise (see
/// `PATCHES.md`). Either way scripts see the same object.
#[test]
fn arguments_objects_behave_the_same_however_they_are_stored() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/arguments.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"[["0:1:111","1:x:111","length:2:101","callee:fn:101","Symbol(Symbol.iterator):fn:101"],true,["0:A:111","1:B:111","2:3:111","length:3:101","callee:fn:101","Symbol(Symbol.iterator):fn:101"],"A","B",["A","B",3],["0:1:111","length:1:101","callee:accessor:NaN00","Symbol(Symbol.iterator):fn:101"],true,["x"],[1,"x"],"[object Arguments]","{\"0\":1,\"1\":\"x\"}",["length:0:101","callee:fn:101","Symbol(Symbol.iterator):fn:101"],[1,null,null,"g",null],["0","3"],["got",2],9]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// The hash a `Map` or `Set` files a key under is mixed before use (see
/// `PATCHES.md`). Order and lookups do not depend on it.
#[test]
fn map_and_set_keys_keep_their_order_and_identity() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/collections.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["0,7,14,21,28,35,2,9,16,23,30,37,4,11,18,25,32,39,6,13,20,27,34,1,8,15,22,29,36,10,17,24,31,38,5,12,19,26,33,3,NaN,3,2147483648,1.5","zero","nan","s","again","big","f",44,42,"1,2,4,5,7,8,10,11,13,14,16,17,19,20,22,23,25,26,28,29,31,32,34,35,37,38,40,41,43,44,46,47,49,50,52,53,55,56,58,59,61,62",true,true,false,false]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// Most functions are compiled on their first call rather than with their
/// script (see `PATCHES.md`). Closures, `with`, direct eval, `arguments`,
/// default parameters, classes and private names, `super`, `new.target`,
/// strict mode, generators, async functions, function properties and text
/// and stack positions must all come out as when every function is
/// compiled with its script. The expected values are what the engine gives
/// that way.
#[test]
fn functions_compiled_on_first_call_behave_as_compiled_with_their_script() {
    let js = runtime();
    js.run_script(include_str!("fixtures/lazy.js"), "lazy.js")
        .unwrap();
    assert_eq!(
        js.eval_json("lazy").unwrap().as_deref(),
        Some(
            r#"{"closures":"top,a,b,c,later","shadowing":"outer:block","loops":[0,1,2],"tdz":["ReferenceError","set"],"constant":"TypeError","inBlock":"seen","named":[120,"undefined"],"withAround":"from with","withInside":["local!","o?"],"eval":["made","qq","r","snumber","top"],"args":[2,null,"changed","x","outerouter:inner",3],"classes":[42,"Derived>Base2","Base2",["private method","private getter",true,false],10,"Base1",[2,3]],"newTarget":[true,false],"objects":["obj>proto","proto","set"],"strict":[true,"object",true],"classStrict":[true,"TypeError","undefined"],"generators":[1,2,3],"properties":[2,"lengths","AsyncFunction","GeneratorFunction","","function lengths(a, b, c = 1, d) {}","function  spaced ( x,y ) { /* c */ return x; }"],"control":["done","try0"],"iife":["z!",false],"constructed":[3,"function anonymous(a,b\n) {\nreturn a + b\n}"],"stack":"    at each (lazy.js:157:48)\n    at map (native)\n    at deeper (lazy.js:157:20)\n    at <eval> (lazy.js:160:7)\n","async":"async"}"#
        )
    );
    assert!(js.errors().is_empty());
}

/// A syntax error anywhere in a script stops all of it, including one in a
/// function that is never called and so never compiled in full.
#[test]
fn syntax_errors_in_functions_not_yet_compiled_stop_the_script() {
    let js = runtime();
    for source in [
        "ran = true; function f() { return function () { return 1 +; }; }",
        "ran = true; var f = () => { let x; let x; };",
        "ran = true; var o = { m() { 'use strict'; with (o) {} } };",
        "ran = true; var f = function (a, a) { 'use strict'; };",
        "ran = true; var f = async function () { function g() { await 1; } };",
        "ran = true; var f = function () { class A { m() { return this.#missing; } } };",
        "ran = true; var f = function () { return () => { break; }; };",
    ] {
        js.run_script("var ran = false;", "init.js").unwrap();
        let err = js.run_script(source, "bad.js").unwrap_err();
        assert!(err.message.starts_with("SyntaxError"), "{source}: {}", err.message);
        assert_eq!(js.eval_json("ran").unwrap().as_deref(), Some("false"), "{source}");
    }
    js.take_errors();
}

/// A function compiled on its first call sees the variables around it as
/// they are when it is called, however deeply it is nested and whichever
/// of the functions around it were compiled late too.
#[test]
fn late_compiled_functions_share_variables_with_the_functions_around_them() {
    let js = runtime();
    js.run_script(
        "var counter = (function () {\n\
           let n = 0;\n\
           function bump() { return ++n; }\n\
           return { bump, read: () => n, reset: function () { n = 0; }, \
                    nested() { return function () { return () => bump() + n; }; } };\n\
         })();\n\
         counter.bump(); counter.bump();",
        "counter.js",
    )
    .unwrap();
    assert_eq!(
        js.eval_json("[counter.read(), counter.nested()()(), counter.read(), (counter.reset(), counter.read())]")
            .unwrap()
            .as_deref(),
        Some("[2,6,3,0]")
    );
}
