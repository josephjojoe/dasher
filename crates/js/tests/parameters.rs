//! Parameters with default values or patterns, and the closures made in
//! them.
//!
//! Such a function's parameters are variables of their own scope, which
//! the closures in the default values read. QuickJS-NG 0.11 gave the body
//! a second copy of each, so neither saw what the other assigned (a change
//! in `vendor/rquickjs-sys`).

use dasher_js::{JsRuntime, RuntimeOptions};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

/// The fixture's cases cover assignment in both directions, names the body
/// declares again with `var` or as a function (a second variable that
/// starts from the parameter's value), patterns, rest parameters,
/// `arguments`, every kind of function, `with`, and `eval` in a default
/// value or in a function inside the body. The expected values are Node
/// 22's; the engine gave 15 of the 53 before the change.
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

/// A function that calls `eval` in its own body is compiled as upstream
/// compiled it: the body and the code it evaluates use the arguments, and
/// the closures in the default values their own variables. Node gives 5
/// and `[2,2]` here.
#[test]
fn a_function_with_eval_in_its_body_keeps_two_variables() {
    let js = runtime();
    assert_eq!(
        js.eval_json(
            "(() => {
               function assigned(a = 1, set = (v) => { a = v; }) { set(5); return eval('a'); }
               function read(a = eval('1'), g = () => a) { a = 2; return [eval('a'), g()]; }
               return [assigned(), read()];
             })()"
        )
        .unwrap()
        .as_deref(),
        Some("[1,[2,1]]")
    );
}
