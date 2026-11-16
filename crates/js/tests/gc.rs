//! The cycle collector and variables that closures share with a suspended
//! generator or async function.
//!
//! Such a function's frame lives on the heap, and a closure made in it
//! reads the frame's variables in place. QuickJS-NG 0.11 let the collector
//! free a frame in a cycle, and what its variables held, while a closure
//! that was not in the cycle still read them. A closure now keeps the
//! frame it reads from (a change in `vendor/rquickjs-sys`).

use dasher_js::{JsRuntime, RuntimeOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

/// Runs `setup`, which leaves a suspended function in a cycle nothing
/// reaches and `read()` reaching one of its variables, then collects,
/// fills the heap with other objects and reads the variable.
fn survives(setup: &str) {
    let js = runtime();
    js.run_script(setup, "setup.js").unwrap();
    js.run_gc();
    // Whatever was freed by mistake is handed out again here.
    js.run_script(
        "var filler = []; for (let i = 0; i < 20000; i++) filler.push({ i, s: 'x' + i });",
        "fill.js",
    )
    .unwrap();
    assert_eq!(
        js.eval_json("read()").unwrap().as_deref(),
        Some(r#"{"kept":"value","list":[1,2,3]}"#),
        "{setup}"
    );
    // Letting go of the closure releases the variable for the last time.
    js.run_script("read = null; filler = null;", "drop.js")
        .unwrap();
    js.run_gc();
    js.run_gc();
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn a_closure_outlives_the_generator_it_was_made_in() {
    survives(
        "var read;
         function* work() {
           const held = { kept: 'value', list: [1, 2, 3] };
           read = () => held;
           yield;
         }
         (function () {
           const it = work();
           it.next();
           it.self = it;
         })();",
    );
}

#[test]
fn a_closure_outlives_the_async_function_it_was_made_in() {
    survives(
        "var read;
         async function work(never) {
           const held = { kept: 'value', list: [1, 2, 3] };
           read = () => held;
           await never;
         }
         (function () {
           // A promise nobody settles, reachable only from the function
           // that waits on it.
           work(new Promise(() => {}));
         })();",
    );
}

#[test]
fn a_closure_outlives_the_async_generator_it_was_made_in() {
    survives(
        "var read;
         async function* work() {
           const held = { kept: 'value', list: [1, 2, 3] };
           read = () => held;
           yield 1;
         }
         (function () {
           const it = work();
           it.next();
           it.self = it;
         })();",
    );
}

#[test]
fn a_variable_assigned_after_the_collection_is_the_one_read() {
    // The generator is still reachable here, so its frame is too: the
    // closure and the frame must go on sharing one variable.
    let js = runtime();
    js.run_script(
        "var read, it;
         function* work() {
           let held = { n: 1 };
           read = () => held;
           yield;
           held = { n: 2 };
           yield;
         }
         it = work();
         it.next();",
        "setup.js",
    )
    .unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("read()").unwrap().as_deref(),
        Some(r#"{"n":1}"#)
    );
    js.run_script("it.next();", "next.js").unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("read()").unwrap().as_deref(),
        Some(r#"{"n":2}"#)
    );
    js.run_script("it.next(); it = null;", "end.js").unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("read()").unwrap().as_deref(),
        Some(r#"{"n":2}"#)
    );
}

#[test]
fn mapped_arguments_outlive_the_generator_they_belong_to() {
    survives(
        "var read;
         function* work(held) {
           const args = arguments;
           read = () => args[0];
           yield;
         }
         (function () {
           const it = work({ kept: 'value', list: [1, 2, 3] });
           it.next();
           it.self = it;
         })();",
    );
}

#[test]
fn a_cycle_through_a_closure_and_its_generator_is_collected() {
    // The closure keeps the frame and the frame keeps the closure: a cycle
    // the collector has to see through, or every such function leaks.
    assert_eq!(
        collected(
            "function* work() {
               const held = { big: new Array(1000).fill(0) };
               const read = () => held;
               registry.register(held, 'held');
               yield read;
             }
             (function () {
               for (let i = 0; i < 50; i++) {
                 const it = work();
                 it.next();
                 it.self = it;
               }
             })();"
        ),
        "50"
    );
}

/// Runs `setup`, which registers objects with `registry` and leaves them
/// in cycles nothing reaches, and returns how many were collected.
fn collected(setup: &str) -> String {
    let js = runtime();
    js.run_script(
        "var collected = 0, registry = new FinalizationRegistry(() => { collected++; });",
        "registry.js",
    )
    .unwrap();
    js.run_script(setup, "setup.js").unwrap();
    js.run_gc();
    js.run_gc();
    js.run_script("void 0;", "turn.js").unwrap();
    assert!(js.errors().is_empty(), "{:?}", js.errors());
    js.eval_json("collected").unwrap().unwrap()
}

#[test]
fn a_cycle_from_a_shared_variable_back_to_its_generator_is_collected() {
    // The variable a closure shares with the frame leads back to the
    // generator, and the frame holds the closure.
    assert_eq!(
        collected(
            "function* work() {
               const held = { it: null };
               const read = () => held;
               registry.register(held, 'held');
               held.it = yield read;
               yield;
             }
             (function () {
               for (let i = 0; i < 50; i++) {
                 const it = work();
                 it.next();
                 it.next(it);
               }
             })();"
        ),
        "50"
    );
}

#[test]
fn an_async_function_nothing_will_resume_is_collected_with_its_closures() {
    // Only the function can settle the promise it waits on, through a
    // closure its own frame holds.
    assert_eq!(
        collected(
            "async function work() {
               let settle;
               const held = {};
               const done = () => settle(held);
               registry.register(held, 'held');
               await new Promise((resolve) => { settle = resolve; });
               done();
             }
             (function () {
               for (let i = 0; i < 50; i++) work();
             })();"
        ),
        "50"
    );
}

#[test]
fn a_finished_generator_is_freed_while_its_closures_live() {
    // Once the function has returned its closures own their variables and
    // no longer keep its frame.
    assert_eq!(
        collected(
            "var reads = [];
             function* work() {
               const held = { kept: 'value' };
               reads.push(() => held);
               yield;
             }
             (function () {
               for (let i = 0; i < 50; i++) {
                 const it = work();
                 registry.register(it, 'generator');
                 it.next();
                 it.next();
               }
             })();"
        ),
        "50"
    );
}

#[test]
fn closures_made_before_a_generator_starts_share_its_variables() {
    // Default values run before the generator object exists.
    survives(
        "var read;
         function* work(held = { kept: 'value', list: [1, 2, 3] }, reader = () => held) {
           read = reader;
           yield;
         }
         (function () {
           const it = work();
           it.next();
           it.self = it;
         })();",
    );
    let js = runtime();
    js.run_script(
        "var read, write;
         function* work(held = 1, reader = () => held, writer = (v) => { held = v; }) {
           read = reader;
           write = writer;
           yield;
           yield;
         }
         var it = work();
         it.next();
         var before = read();
         write({ n: 10 });",
        "setup.js",
    )
    .unwrap();
    js.run_gc();
    js.run_script("it.next(); it.next(); it = null;", "end.js")
        .unwrap();
    js.run_gc();
    assert_eq!(
        js.eval_json("[before, read()]").unwrap().as_deref(),
        Some(r#"[1,{"n":10}]"#)
    );
}

#[test]
fn each_turn_of_a_loop_in_a_generator_keeps_its_own_variable() {
    let js = runtime();
    js.run_script(
        "var reads = [];
         function* work() {
           for (let i = 0; i < 3; i++) {
             const held = { i };
             reads.push(() => [i, held.i]);
             yield;
           }
         }
         (function () {
           const it = work();
           it.next();
           it.next();
           it.self = it;
         })();",
        "setup.js",
    )
    .unwrap();
    js.run_gc();
    js.run_script(
        "var filler = []; for (let i = 0; i < 20000; i++) filler.push({ i });",
        "fill.js",
    )
    .unwrap();
    assert_eq!(
        js.eval_json("reads.map((read) => read())")
            .unwrap()
            .as_deref(),
        Some("[[0,0],[1,1]]")
    );
    js.run_script("reads = null; filler = null;", "drop.js")
        .unwrap();
    js.run_gc();
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn a_runtime_is_freed_with_functions_still_suspended() {
    let js = runtime();
    js.run_script(
        "var read, reads = [];
         function* work(first) {
           const held = { args: arguments };
           read = () => [held, first];
           yield;
         }
         async function wait(never) {
           const held = {};
           reads.push(() => held);
           await never;
         }
         async function* stream() {
           const held = {};
           reads.push(() => held);
           yield 1;
         }
         var it = work({});
         it.next();
         wait(new Promise(() => {}));
         var s = stream();
         s.next();",
        "setup.js",
    )
    .unwrap();
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}
