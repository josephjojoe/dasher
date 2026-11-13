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

/// Postfix `++` and `--` have an int fast path, and the compiler rewrites
/// `i++;` and `x = e;` on a `let` (see `PATCHES.md`). Every value gives
/// what it gave before.
#[test]
fn increments_give_the_same_values_on_every_path() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/increments.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["number 0 1 number 0 -1","1 -1","0 1","number -1 0 number -1 -2","0 -2","-1 0","number 1 2 number 1 0","2 0","1 2","number 2147483646 2147483647 number 2147483646 2147483645","2147483647 2147483645","2147483646 2147483647","number 2147483647 2147483648 number 2147483647 2147483646","2147483648 2147483646","2147483647 2147483648","number -2147483647 -2147483646 number -2147483647 -2147483648","-2147483646 -2147483648","-2147483647 -2147483646","number -2147483648 -2147483647 number -2147483648 -2147483649","-2147483647 -2147483649","-2147483648 -2147483647","number 1.5 2.5 number 1.5 0.5","2.5 0.5","1.5 2.5","number -0 1 number -0 -1","1 -1","-0 1","number NaN NaN number NaN NaN","NaN NaN","NaN NaN","number Infinity Infinity number Infinity Infinity","Infinity Infinity","Infinity Infinity","number 5 6 number 5 4","6 4","5 6","number NaN NaN number NaN NaN","NaN NaN","NaN NaN","number 0 1 number 0 -1","1 -1","0 1","number 0 1 number 0 -1","1 -1","0 1","number NaN NaN number NaN NaN","NaN NaN","NaN NaN","number 1 2 number 1 0","2 0","1 2","bigint 10n 11n bigint 10n 9n","11n 9n","10n 11n","number 0 1 number 0 -1","1 -1","0 1","number 3 4 number 3 2","4 2","3 4","number NaN NaN number NaN NaN","NaN NaN","NaN NaN","number 41 42 number 41 40","42 40","41 42","21474836445","36507221981","ReferenceError","ReferenceError","ReferenceError","2","0,1,2","valueOf","7 8","2","10"]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// An object literal with fixed keys is made, after its first run, from the
/// shape it ended with, and `define_field` stores into a property already
/// there (see `PATCHES.md`). Scripts see the same properties, order and
/// attributes, and class fields behave as before.
#[test]
fn object_literals_have_the_properties_scripts_expect() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/object_literals.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["a:0:111 b:2:111 c:x:111 d:null:111","a:3:111 b:2:111","0:zero:111 1:one:111 2:two:111 z:0:111 quoted key:1:111 x:sx0:111","a:1:111 proto:null","a:1:111 isArray:true","__proto__:5:111 a:1:111","sx0:1:111 a:2:111","a:3:111 b:2:111 c:4:111","m:fn:111 g:accessor:NaN11 a:1:111","fn:fn:111 arrow:fn:111 cls:fn:111 fn,arrow,cls","nested:obj:111 after:1:111","b:2:111 c:3:111 / a:1:111 b:2:111","a:1:010 b:2:010 / a:1:111 b:2:111 false","k:1:101 / k:1:111","in value","10,b,a {\"b\":1,\"a\":[1,{\"c\":2}]}","21 191","a:A:111 b:B:111","a:changed:111 b:2:111 / a:1:111","a:1:111 b:2:111 c:x:111 d:null:111","a:3:111 b:2:111","0:zero:111 1:one:111 2:two:111 z:0:111 quoted key:1:111 x:sx1:111","a:1:111 proto:null","a:1:111 isArray:true","__proto__:5:111 a:1:111","sx1:1:111 a:2:111","a:3:111 b:2:111 c:4:111","m:fn:111 g:accessor:NaN11 a:1:111","fn:fn:111 arrow:fn:111 cls:fn:111 fn,arrow,cls","nested:obj:111 after:1:111","b:2:111 c:3:111 / a:1:111 b:2:111","a:1:010 b:2:010 / a:1:111 b:2:111 false","k:1:101 / k:1:111","in value","10,b,a {\"b\":1,\"a\":[1,{\"c\":2}]}","21 191","a:A:111 b:B:111","a:changed:111 b:2:111 / a:1:111","a:2:111 b:2:111 c:x:111 d:null:111","a:3:111 b:2:111","0:zero:111 1:one:111 2:two:111 z:0:111 quoted key:1:111 x:sx2:111","a:1:111 proto:null","a:1:111 isArray:true","__proto__:5:111 a:1:111","sx2:1:111 a:2:111","a:3:111 b:2:111 c:4:111","m:fn:111 g:accessor:NaN11 a:1:111","fn:fn:111 arrow:fn:111 cls:fn:111 fn,arrow,cls","nested:obj:111 after:1:111","b:2:111 c:3:111 / a:1:111 b:2:111","a:1:010 b:2:010 / a:1:111 b:2:111 false","k:1:101 / k:1:111","in value","10,b,a {\"b\":1,\"a\":[1,{\"c\":2}]}","21 191","a:A:111 b:B:111","a:changed:111 b:2:111 / a:1:111","trap:0:111 other:1:111 proto","trap:1:111 other:1:111 proto","a:1:111 b:2:111 / a:3:111 b:2:111 c:4:111 / x:2:111","TypeError","TypeError","a:1:111 b:2:111 / a:3:111 b:2:111 c:4:111 / x:2:111","TypeError","TypeError","a:1:111 b:2:111 / a:3:111 b:2:111 c:4:111 / x:2:111","TypeError","TypeError","1,2,3,1,2,3,1,2,3,trap f,trap f,trap f"]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// Spread, object rest and `Object.assign` copy an ordinary source without a
/// list of its keys (see `PATCHES.md`). Getters, setters, symbols, index keys,
/// Proxies and sources changed during the copy give what they gave before.
#[test]
fn property_copies_behave_the_same_on_every_path() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/copying.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["a:1:111 b:2:111 c:3:111 | a:1:111 b:2:111 c:3:111 d:0:111 | z:0:111 a:over:111 b:2:111 c:3:111","a:1:111 b:2:111 c:3:111 | 0:x:111 1:y:111 a:1:111 b:2:111 c:3:111 e:1:111","1 b:2:111 c:3:111","1 x:2:111 Symbol(g):3:111","1:one:111 2:two:111 4294967294:big:111 b:1:111 4294967295:notindex:111 Symbol(s):s:111","v:1:111 ro:3:111 | v:1:111 ro:3:111","g:G:111 plain:1:111 h:H:111 plain2:undefined:111 | g:G:111 plain:1:111 h:H:111 added:1:111","first:1:111 second:2:111 third:changed:111 fourth:undefined:111","a:accessor:NaN11 b:2:111 c:3:111","q:2:111 | trap:1:111 q:2:111","TypeError","ro2:1:111","TypeError","[\"x\",2,null] 3","a:1:111 b:2:111 | a:1:111 b:2:111","a:1:111 b:2:111 c:3:111","a:1:111 b:2:111","own:2:111 | own:2:111","n:1:111 m:2:111 [object Object]","g:G:111 c:undefined:111 d:4:111 | g:G:111 d:4:111 added:new:111 | g:accessor:NaN11 d:4:111 added:new:111","a:1:111 c:3:111 | a:1:111 c:3:111","0:1:111 1:2:111 |  | ","own:1:111","y:2:111 z:3:111","40 149","a:1:111 b:2:111 c:3:111 | a:1:111 b:2:111 c:3:111 d:1:111 | z:0:111 a:over:111 b:2:111 c:3:111","a:1:111 b:2:111 c:3:111 | 0:x:111 1:y:111 a:1:111 b:2:111 c:3:111 e:1:111","1 b:2:111 c:3:111","1 x:2:111 Symbol(g):3:111","1:one:111 2:two:111 4294967294:big:111 b:1:111 4294967295:notindex:111 Symbol(s):s:111","v:1:111 ro:3:111 | v:1:111 ro:3:111","g:G:111 plain:1:111 h:H:111 plain2:undefined:111 | g:G:111 plain:1:111 h:H:111 added:1:111","first:1:111 second:2:111 third:changed:111 fourth:undefined:111","a:accessor:NaN11 b:2:111 c:3:111","q:2:111 | trap:1:111 q:2:111","TypeError","ro2:1:111","TypeError","[\"x\",2,null] 3","a:1:111 b:2:111 | a:1:111 b:2:111","a:1:111 b:2:111 c:3:111","a:1:111 b:2:111","own:2:111 | own:2:111","n:1:111 m:2:111 [object Object]","g:G:111 c:undefined:111 d:4:111 | g:G:111 d:4:111 added:new:111 | g:accessor:NaN11 d:4:111 added:new:111","a:1:111 c:3:111 | a:1:111 c:3:111","0:1:111 1:2:111 |  | ","own:1:111","y:2:111 z:3:111","40 149","a:1:111 b:2:111 c:3:111 | a:1:111 b:2:111 c:3:111 d:2:111 | z:0:111 a:over:111 b:2:111 c:3:111","a:1:111 b:2:111 c:3:111 | 0:x:111 1:y:111 a:1:111 b:2:111 c:3:111 e:1:111","1 b:2:111 c:3:111","1 x:2:111 Symbol(g):3:111","1:one:111 2:two:111 4294967294:big:111 b:1:111 4294967295:notindex:111 Symbol(s):s:111","v:1:111 ro:3:111 | v:1:111 ro:3:111","g:G:111 plain:1:111 h:H:111 plain2:undefined:111 | g:G:111 plain:1:111 h:H:111 added:1:111","first:1:111 second:2:111 third:changed:111 fourth:undefined:111","a:accessor:NaN11 b:2:111 c:3:111","q:2:111 | trap:1:111 q:2:111","TypeError","ro2:1:111","TypeError","[\"x\",2,null] 3","a:1:111 b:2:111 | a:1:111 b:2:111","a:1:111 b:2:111 c:3:111","a:1:111 b:2:111","own:2:111 | own:2:111","n:1:111 m:2:111 [object Object]","g:G:111 c:undefined:111 d:4:111 | g:G:111 d:4:111 added:new:111 | g:accessor:NaN11 d:4:111 added:new:111","a:1:111 c:3:111 | a:1:111 c:3:111","0:1:111 1:2:111 |  | ","own:1:111","y:2:111 z:3:111","40 149","get g,get h,get g,get h,set a 1,proto setter 1,ownKeys,get a,get b,ownKeys,get a,get b,set a,set b,set c,get z,get g,get h,get g,get h,set a 1,proto setter 1,ownKeys,get a,get b,ownKeys,get a,get b,set a,set b,set c,get z,get g,get h,get g,get h,set a 1,proto setter 1,ownKeys,get a,get b,ownKeys,get a,get b,set a,set b,set c,get z"]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// Destructuring, spread, for-of, `apply` and `concat` skip the iterator
/// protocol and species lookups for plain arrays (see `PATCHES.md`). With the
/// iterator, `next`, `constructor` or `Symbol.species` replaced they behave as before.
#[test]
fn array_iteration_behaves_the_same_on_every_path() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/array_iteration.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["plain 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-iterator 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-next 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","iterator-return 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","own  own","1,2 2","custom-constructor 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-species 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","false false 2,4 2,3 2,3 1,2,3","3","proto-index 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3",",from proto, 1002 1002","plain-again 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-iterator 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-next 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","iterator-return 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","own  own","1,2 2","custom-constructor 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom-species 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","false false 2,4 2,3 2,3 1,2,3","3","proto-index 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3",",from proto, 1002 1002","plain-again 1 2,3,4 3 10 def dz  hole  2/3/4/3 1//3/3 1/2//4 5,6 a,b 0,1,2,3,4,9 7,8,1,,3,1,2,10,20,100,1,2","6:3 0:0 780:40 NaN:3 NaN:4 0ab:2 0:0 9:2 9 2:2 function anonymous(\n) {\n1\n}","1,2,3 m","1,2,3,4,5,6,7 7 13 [0,1,null,3,1,2,null,null,\"a\",\"b\",[8,9],\"str\",5] true true","2,4,6 3 [10,null,30] 3 false 70000 69999 1,3 1,2 aa,bb a,c stop","true true 2,4 2,3 2,3 1,2,3","custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,return called,return called,own iterator,own iterator,proxy get Symbol(Symbol.iterator),proxy get Symbol(Symbol.iterator),proxy get length,proxy get 0,proxy get length,proxy get 1,proxy get length,proxy get Symbol(Symbol.isConcatSpreadable),proxy get length,proxy get 0,proxy get 1,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,own constructor,proto index,proto index,proto index,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom iterator,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,custom next,return called,return called,own iterator,own iterator,proxy get Symbol(Symbol.iterator),proxy get Symbol(Symbol.iterator),proxy get length,proxy get 0,proxy get length,proxy get 1,proxy get length,proxy get Symbol(Symbol.isConcatSpreadable),proxy get length,proxy get 0,proxy get 1,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,custom species,own constructor,proto index,proto index,proto index"]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// A capability for the realm's own `Promise` is made without an executor,
/// functions made in C take `length` and `name` from a remembered shape,
/// and naming a new function stores its `name` in place (see `PATCHES.md`).
/// Reactions run in the same order and functions look the same.
#[test]
fn promises_and_functions_behave_the_same_on_every_path() {
    let js = runtime();
    js.run_script(include_str!("fixtures/promises.js"), "promises.js")
        .unwrap();
    assert_eq!(
        js.eval_json("promiseLog").unwrap().as_deref(),
        Some(
            r#"["resolving functions length:1:001 name::001 | length:1:001 name::001 true","builtins length:2:001 name:max:001 | length:1:001 name:resolve:001 | length:1:001 name:map:001 | length:0:001 name:bound :001","names arrow,fexpr,inner,m,n,f,g,C,function,late,destr,fixed","arrow length:0:001 name:arrow:001 | length:0:001 name:fexpr:001 prototype:obj:100","fresh length:0:001 name:fresh:001","fresh length:0:001 name:fresh:001","fresh length:0:001 name:fresh:001","same true","MyPromise constructed","MyPromise constructed","subclass true true","fake ctor","af start 10","af start [object Promise]","outer start","inner start","async promise true Promise","sync end","a 1","b 2","caught r","thenable then called","fake 5","af after await 10","af after await 20","thenable then called","thenable then called","withResolvers ok","bad thenable then getter","finally block","inner after await","arrow value","method WithAsync","p0 then resolved late","a2","finally","t from thenable","af result 11","af2 result 21","thrower async throw","recovered no","allSettled rejected,fulfilled","race","any","throwing thenable then throws","after await in finally","outer got 21","awaitsThenable from thenable","returned promise","all 1,2,from thenable","withFinally try","outer result 42","b2 inner","manyAwaits 6","loop 5","chain 5"]"#
        )
    );
    assert!(js.errors().is_empty());
}

/// for-in takes an ordinary object's shape for its key list, and
/// `Object.keys` and `Object.values` read keys and values from the shape (see
/// `PATCHES.md`). Keys, order and values are the same however objects change.
#[test]
fn enumeration_gives_the_same_keys_on_every_path() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/enumeration.js"))
            .unwrap()
            .as_deref(),
        Some(
            r#"["a,b,c,d,e | a,b,c,d,e | [1,2,3,4,5] | [[\"a\",1],[\"b\",2],[\"c\",3],[\"d\",4],[\"e\",5]] | a,b,c,d,e","a,c | a,c | [1,3] | [[\"a\",1],[\"c\",3]] | a,c","x,y | x,y | [1,2] | [[\"x\",1],[\"y\",2]] | x,y,h","1,2,10,z,4294967295 | 1,2,10,z,4294967295 | [\"a\",\"b\",\"c\",1,\"n\"] | [[\"1\",\"a\"],[\"2\",\"b\"],[\"10\",\"c\"],[\"z\",1],[\"4294967295\",\"n\"]] | 1,2,10,z,4294967295","s,t | s,t | [1,3] | [[\"s\",1],[\"t\",3]] | s,t","g,v | g,v | [\"G\",1] | [[\"g\",\"G\"],[\"v\",1]] | g,v","n,m | n,m | [1,2] | [[\"n\",1],[\"m\",2]] | n,m","own,inherited | own | [3] | [[\"own\",3]] | own,shadowed","0,1,2 | 0,1,2 | [1,2,3] | [[\"0\",1],[\"1\",2],[\"2\",3]] | 0,1,2,length # 0,1,2 | 0,1,2 | [\"s\",\"t\",\"r\"] | [[\"0\",\"s\"],[\"1\",\"t\"],[\"2\",\"r\"]] | 0,1,2,length #  |  | [] | [] | length,name,prototype","i,j | i,j | [1,2] | [[\"i\",1],[\"j\",2]] | i,j","f,g | f,g | [1,2] | [[\"f\",1],[\"g\",2]] | f,g","px,py | px,py | [1,2] | [[\"px\",1],[\"py\",2]] | px,py","mutated during for-in: a,b,d","deleted and re-added: a,b","made non-enumerable: a,b,c","own shape changed during for-in: a,c / a,c,na,nc","xx,xz,zx,zz","30 109 435","a,b,c,d,e | a,b,c,d,e | [1,2,3,4,5] | [[\"a\",1],[\"b\",2],[\"c\",3],[\"d\",4],[\"e\",5]] | a,b,c,d,e","a,c | a,c | [1,3] | [[\"a\",1],[\"c\",3]] | a,c","x,y | x,y | [1,2] | [[\"x\",1],[\"y\",2]] | x,y,h","1,2,10,z,4294967295 | 1,2,10,z,4294967295 | [\"a\",\"b\",\"c\",1,\"n\"] | [[\"1\",\"a\"],[\"2\",\"b\"],[\"10\",\"c\"],[\"z\",1],[\"4294967295\",\"n\"]] | 1,2,10,z,4294967295","s,t | s,t | [1,3] | [[\"s\",1],[\"t\",3]] | s,t","g,v | g,v | [\"G\",1] | [[\"g\",\"G\"],[\"v\",1]] | g,v","n,m | n,m | [1,2] | [[\"n\",1],[\"m\",2]] | n,m","own,inherited | own | [3] | [[\"own\",3]] | own,shadowed","0,1,2 | 0,1,2 | [1,2,3] | [[\"0\",1],[\"1\",2],[\"2\",3]] | 0,1,2,length # 0,1,2 | 0,1,2 | [\"s\",\"t\",\"r\"] | [[\"0\",\"s\"],[\"1\",\"t\"],[\"2\",\"r\"]] | 0,1,2,length #  |  | [] | [] | length,name,prototype","i,j | i,j | [1,2] | [[\"i\",1],[\"j\",2]] | i,j","f,g | f,g | [1,2] | [[\"f\",1],[\"g\",2]] | f,g","px,py | px,py | [1,2] | [[\"px\",1],[\"py\",2]] | px,py","mutated during for-in: a,b,d","deleted and re-added: a,b","made non-enumerable: a,b,c","own shape changed during for-in: a,c / a,c,na,nc","xx,xz,zx,zz","30 109 435","with enumerable on Object.prototype: q,enumerableOnProto | q | [1] | [[\"q\",1]] | q # 0,enumerableOnProto","with hidden on Object.prototype: q,hiddenOnProto | q,hiddenOnProto | [1,2] | [[\"q\",1],[\"hiddenOnProto\",2]] | q,hiddenOnProto","a,b,c,d,e | a,b,c,d,e | [1,2,3,4,5] | [[\"a\",1],[\"b\",2],[\"c\",3],[\"d\",4],[\"e\",5]] | a,b,c,d,e","a,c | a,c | [1,3] | [[\"a\",1],[\"c\",3]] | a,c","x,y | x,y | [1,2] | [[\"x\",1],[\"y\",2]] | x,y,h","1,2,10,z,4294967295 | 1,2,10,z,4294967295 | [\"a\",\"b\",\"c\",1,\"n\"] | [[\"1\",\"a\"],[\"2\",\"b\"],[\"10\",\"c\"],[\"z\",1],[\"4294967295\",\"n\"]] | 1,2,10,z,4294967295","s,t | s,t | [1,3] | [[\"s\",1],[\"t\",3]] | s,t","g,v | g,v | [\"G\",1] | [[\"g\",\"G\"],[\"v\",1]] | g,v","n,m | n,m | [1,2] | [[\"n\",1],[\"m\",2]] | n,m","own,inherited | own | [3] | [[\"own\",3]] | own,shadowed","0,1,2 | 0,1,2 | [1,2,3] | [[\"0\",1],[\"1\",2],[\"2\",3]] | 0,1,2,length # 0,1,2 | 0,1,2 | [\"s\",\"t\",\"r\"] | [[\"0\",\"s\"],[\"1\",\"t\"],[\"2\",\"r\"]] | 0,1,2,length #  |  | [] | [] | length,name,prototype","i,j | i,j | [1,2] | [[\"i\",1],[\"j\",2]] | i,j","f,g | f,g | [1,2] | [[\"f\",1],[\"g\",2]] | f,g","px,py | px,py | [1,2] | [[\"px\",1],[\"py\",2]] | px,py","mutated during for-in: a,b,d","deleted and re-added: a,b","made non-enumerable: a,b,c","own shape changed during for-in: a,c / a,c,na,nc","xx,xz,zx,zz","30 109 435","getter,getter,ownKeys,ownKeys,ownKeys,ownKeys,ownKeys,getter,getter,ownKeys,ownKeys,ownKeys,ownKeys,ownKeys,getter,getter,ownKeys,ownKeys,ownKeys,ownKeys,ownKeys"]"#
        )
    );
    assert!(js.errors().is_empty());
}
