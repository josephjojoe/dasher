//! `var` in code that a direct `eval` runs inside a function.
//!
//! The variable belongs to the function, and declaring it again leaves it
//! as it is. QuickJS-NG 0.11 set it to `undefined` on every declaration, so
//! `eval('var n; n = (n || 0) + 1')` in a loop counted to 1 (a change in
//! `vendor/rquickjs-sys`).

use dasher_js::{JsRuntime, RuntimeOptions};

/// The expected values are Node 22's; the engine gave 13 of the 20 before
/// the change.
#[test]
fn declaring_a_variable_again_keeps_its_value() {
    let js = JsRuntime::new(RuntimeOptions::default()).expect("runtime");
    assert_eq!(
        js.eval_json(include_str!("fixtures/eval_var.js"))
            .unwrap()
            .as_deref(),
        Some(include_str!("fixtures/eval_var.expected.json"))
    );
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}
