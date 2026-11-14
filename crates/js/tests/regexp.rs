//! dasher runs the loop of a regular expression search itself and leaves
//! the matching to the engine (`src/regexp.rs`), and the engine patch runs
//! the RegExp methods and some patterns its own faster way while a script
//! has not changed what they would look at (`vendor/rquickjs-sys/PATCHES.md`).
//! These check that scripts cannot tell: every answer, and every call a
//! script sees, is the same as with all of that turned off.
//!
//! The switches are one per process, and creating a runtime turns them on.
//! So this file has a single test: a second one running beside it could
//! turn them back on while the first compares the two ways.

use dasher_js::{JsRuntime, RuntimeOptions, regexp};

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).expect("runtime")
}

/// Runs a fixture and returns its `out`.
fn run(js: &JsRuntime, script: &str, name: &str) -> String {
    js.run_script(script, name).unwrap();
    assert!(js.errors().is_empty(), "{name}: {:?}", js.errors());
    js.eval_json("out").unwrap().expect("out")
}

#[test]
fn dashers_regexp_paths_give_the_engines_answers() {
    let fixtures = [
        ("regexp.js", include_str!("fixtures/regexp.js")),
        ("regexp-methods.js", include_str!("fixtures/regexp-methods.js")),
    ];

    let js = runtime();
    let before = regexp::searches();
    let ours: Vec<String> = fixtures
        .iter()
        .map(|(name, script)| run(&js, script, name))
        .collect();
    assert!(
        regexp::searches() - before > 5000,
        "the search hook was not called: has the engine changed how it compiles a search?"
    );

    // A few answers written out, in case both ways were wrong together.
    assert_eq!(
        js.eval_json(
            "['banana'.search(/an/), 'a\\nb\\nc'.match(/^./gm), 'xAbCx'.replace(/[a-c]+/i, '-'), \
              /(?<=a)b/.exec('cbab').index, 'caf\\u00e9 CAF\\u00c9'.match(/\\u00e9/gi).length, \
              'a\\ud83d\\ude00b'.split(/(?:)/u).length, /x*$/.exec('aaxx').index, \
              /\\b(?:ipad|iphone)\\b/i.test('Mozilla (iPhone; CPU)'), /^$/m.exec('a\\n').index, \
              'mail jo@ex.org now'.match(/[a-z.]+@[a-z]+\\.[a-z]{2,}/g), 'a-b-c'.split(/-/, 2), \
              'foo boo'.replace(/o/g, '0'), 'x1y22'.replace(/(\\d)(\\d)?/g, '[$2$1]'), \
              /oo/.exec('a boot').index, /K/iu.test('\\u212a'), /K/i.test('\\u212a')]"
        )
        .unwrap()
        .as_deref(),
        Some(
            r#"[1,["a","b","c"],"x-x",3,2,3,2,true,2,["jo@ex.org"],["a","b"],"f00 b00","x[1]y[22]",3,true,false]"#
        )
    );

    let js = runtime();
    regexp::uninstall();
    let before = regexp::searches();
    let theirs: Vec<String> = fixtures
        .iter()
        .map(|(name, script)| run(&js, script, name))
        .collect();
    assert_eq!(regexp::searches(), before, "the hook was still installed");

    for (((name, _), ours), theirs) in fixtures.iter().zip(&ours).zip(&theirs) {
        assert!(ours.len() > 50_000, "{name} produced {} bytes", ours.len());
        if ours != theirs {
            let at = ours
                .bytes()
                .zip(theirs.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(ours.len().min(theirs.len()));
            let from = at.saturating_sub(300);
            panic!(
                "{name}: dasher's regexp paths and the engine's disagree at byte {at}:\n\
                 dasher: {}\nengine: {}",
                &ours[from..(at + 300).min(ours.len())],
                &theirs[from..(at + 300).min(theirs.len())],
            );
        }
    }
}
