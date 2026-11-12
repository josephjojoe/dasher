//! dasher runs the loop of a regular expression search itself and leaves
//! the matching to the engine (`src/regexp.rs`). These check that scripts
//! cannot tell.
//!
//! The hook is one per process, and creating a runtime installs it. So
//! this file has a single test: a second one running beside it could put
//! the hook back while the first compares the two loops.

use dasher_js::{JsRuntime, RuntimeOptions, regexp};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

#[test]
fn searches_find_what_the_engines_own_loop_finds() {
    let script = include_str!("fixtures/regexp.js");

    let js = runtime();
    let before = regexp::searches();
    js.run_script(script, "regexp.js").unwrap();
    let ours = js.eval_json("out").unwrap().expect("out");
    assert!(js.errors().is_empty(), "{:?}", js.errors());
    assert!(
        regexp::searches() - before > 5000,
        "the search hook was not called: has the engine changed how it compiles a search?"
    );

    // A few answers written out, in case both loops were wrong together.
    assert_eq!(
        js.eval_json(
            "['banana'.search(/an/), 'a\\nb\\nc'.match(/^./gm), 'xAbCx'.replace(/[a-c]+/i, '-'), \
              /(?<=a)b/.exec('cbab').index, 'caf\\u00e9 CAF\\u00c9'.match(/\\u00e9/gi).length, \
              'a\\ud83d\\ude00b'.split(/(?:)/u).length, /x*$/.exec('aaxx').index, \
              /\\b(?:ipad|iphone)\\b/i.test('Mozilla (iPhone; CPU)'), /^$/m.exec('a\\n').index]"
        )
        .unwrap()
        .as_deref(),
        Some(r#"[1,["a","b","c"],"x-x",3,2,3,2,true,2]"#)
    );

    let js = runtime();
    regexp::uninstall();
    let before = regexp::searches();
    js.run_script(script, "regexp.js").unwrap();
    let theirs = js.eval_json("out").unwrap().expect("out");
    assert_eq!(regexp::searches(), before, "the hook was still installed");

    assert!(
        ours.len() > 50_000,
        "the script produced {} bytes",
        ours.len()
    );
    assert!(
        ours == theirs,
        "dasher's search loop and the engine's disagree"
    );
}
