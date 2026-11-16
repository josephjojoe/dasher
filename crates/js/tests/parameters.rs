//! Parameters with default values or patterns, and the closures made in
//! them.
//!
//! Such a function's parameters are variables of their own scope, which
//! the closures in the default values read. QuickJS-NG 0.11 gave the body
//! a second copy of each, so neither saw what the other assigned (a change
//! in `vendor/rquickjs-sys`).
//!
//! The parameters are outside the body's own variables: a `var` that a
//! direct `eval` declares in the body is one of those, and hides a
//! parameter of its name from the body but not from the default values.

use dasher_js::{JsRuntime, RuntimeOptions, ScriptCache, ScriptCacheOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

/// The fixture's cases cover assignment in both directions, names the body
/// declares again with `var` or as a function (a second variable that
/// starts from the parameter's value), patterns, rest parameters,
/// `arguments`, every kind of function, `with`, and `eval` in a default
/// value, in the body and in a function inside it, reading, assigning and
/// declaring the names. The expected values are Node 22's; the engine gave
/// 39 of the 94 before the change.
#[test]
fn a_body_and_the_closures_in_its_defaults_share_the_parameters() {
    let js = runtime();
    assert_eq!(
        js.eval_json(include_str!("fixtures/parameters.js"))
            .unwrap()
            .as_deref(),
        Some(include_str!("fixtures/parameters.expected.json"))
    );
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn an_async_function_shares_them_across_an_await() {
    let js = runtime();
    js.run_script(
        "var result;
         async function work(held = 1, read = () => held, write = (v) => { held = v; }) {
           held = await 2;
           const first = read();
           write(3);
           await null;
           return [first, held, read()];
         }
         work().then((value) => { result = value; });",
        "setup.js",
    )
    .unwrap();
    js.run_script("void 0;", "turn.js").unwrap();
    assert_eq!(js.eval_json("result").unwrap().as_deref(), Some("[2,3,3]"));
}

/// A compiled function carries which of its variables the body left for
/// the parameters' own, for the code a direct `eval` compiles when the
/// function runs. That has to survive the script cache, which writes the
/// compiled script out and reads it back for the next page.
#[test]
fn a_script_read_back_from_the_cache_gives_the_same_answers() {
    let cache = ScriptCache::new(ScriptCacheOptions::default());
    let source = format!("var answers = {};", include_str!("fixtures/parameters.js"));
    for page in 0..3 {
        let js = JsRuntime::new(RuntimeOptions {
            script_cache: Some(cache.clone()),
            ..RuntimeOptions::default()
        })
        .expect("runtime");
        js.run_script(&source, "parameters.js").unwrap();
        assert_eq!(
            js.eval_json("answers").unwrap().as_deref(),
            Some(include_str!("fixtures/parameters.expected.json")),
            "page {page}"
        );
        assert!(js.errors().is_empty(), "{:?}", js.errors());
    }
    let stats = cache.stats();
    assert_eq!((stats.misses, stats.hits), (1, 2), "{stats:?}");
}
