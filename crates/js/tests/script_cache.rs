//! The script cache: compiled scripts kept across pages.

use dasher_js::{JsRuntime, RuntimeOptions, ScriptCache, ScriptCacheOptions, ScriptCacheStats};

const LAZY: &str = include_str!("fixtures/lazy.js");
/// What `lazy.js` gives with every function compiled with its script.
const LAZY_ANSWER: &str = r#"{"closures":"top,a,b,c,later","shadowing":"outer:block","loops":[0,1,2],"tdz":["ReferenceError","set"],"constant":"TypeError","inBlock":"seen","named":[120,"undefined"],"withAround":"from with","withInside":["local!","o?"],"eval":["made","qq","r","snumber","top"],"args":[2,null,"changed","x","outerouter:inner",3],"classes":[42,"Derived>Base2","Base2",["private method","private getter",true,false],10,"Base1",[2,3]],"newTarget":[true,false],"objects":["obj>proto","proto","set"],"strict":[true,"object",true],"classStrict":[true,"TypeError","undefined"],"generators":[1,2,3],"properties":[2,"lengths","AsyncFunction","GeneratorFunction","","function lengths(a, b, c = 1, d) {}","function  spaced ( x,y ) { /* c */ return x; }"],"control":["done","try0"],"iife":["z!",false],"constructed":[3,"function anonymous(a,b\n) {\nreturn a + b\n}"],"stack":"    at each (lazy.js:157:48)\n    at map (native)\n    at deeper (lazy.js:157:20)\n    at <eval> (lazy.js:160:7)\n","async":"async"}"#;

fn page(cache: &ScriptCache) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        script_cache: Some(cache.clone()),
        ..RuntimeOptions::default()
    })
    .expect("runtime")
}

fn cache() -> ScriptCache {
    ScriptCache::new(ScriptCacheOptions::default())
}

fn counts(stats: ScriptCacheStats) -> (u64, u64, u64) {
    (stats.misses, stats.hits, stats.builds)
}

/// Runs `lazy.js` on a page and checks its answers.
fn run_lazy(cache: &ScriptCache) {
    let js = page(cache);
    js.run_script(LAZY, "lazy.js").unwrap();
    assert_eq!(js.eval_json("lazy").unwrap().as_deref(), Some(LAZY_ANSWER));
    assert!(js.errors().is_empty(), "{:?}", js.errors());
}

#[test]
fn a_script_compiled_once_runs_the_same_from_the_cache() {
    let cache = cache();
    run_lazy(&cache);
    // compiled from its text, and made into an entry when the page went
    assert_eq!(counts(cache.stats()), (1, 0, 1));
    run_lazy(&cache);
    // read back, with the functions the first page ran compiled; the
    // second page ran the same ones, so the entry stays
    assert_eq!(counts(cache.stats()), (1, 1, 1));
    run_lazy(&cache);
    assert_eq!(counts(cache.stats()), (1, 2, 1));
    let stats = cache.stats();
    assert_eq!(stats.entries, 1);
    assert!(stats.bytes > LAZY.len(), "{stats:?}");
}

/// The answers of a few probes of a script's functions, as JSON.
const PROBE: &str = r#"
var probe = (function () {
  var seen = [];
  function named(a, b) { return a + b; }
  var arrow = (x) => x * 2;
  class Point { constructor(x) { this.x = x; } get double() { return this.x * 2; } }
  var stackOf = function () { try { null.f(); } catch (e) { return e.stack; } };
  return {
    named: named, arrow: arrow, Point: Point, stackOf: stackOf,
    run: function () { return [named(1, 2), arrow(4), new Point(3).double]; },
  };
})();
"#;

fn probe_source() -> String {
    // long enough to be cached
    format!("{PROBE}\n/*{}*/\n", "x".repeat(2000))
}

fn probe(js: &JsRuntime) -> String {
    js.eval_json(
        "[probe.run(), String(probe.named), String(probe.arrow), String(probe.Point), \
         probe.stackOf(), probe.named.length, probe.Point.name]",
    )
    .unwrap()
    .unwrap()
}

#[test]
fn functions_read_back_have_their_text_and_positions() {
    let source = probe_source();
    let plain = JsRuntime::new(RuntimeOptions::default()).unwrap();
    plain.run_script(&source, "https://example.com/probe.js").unwrap();
    let expected = probe(&plain);
    assert!(expected.contains("probe.js:7:"), "{expected}");

    let cache = cache();
    for round in 0..3 {
        let js = page(&cache);
        js.run_script(&source, "https://example.com/probe.js").unwrap();
        assert_eq!(probe(&js), expected, "round {round}");
    }
    assert_eq!(counts(cache.stats()), (1, 2, 1));

    // Another URL with the same text finds it too, and its functions say
    // they come from there.
    let js = page(&cache);
    js.run_script(&source, "https://cdn.example.net/copy.js").unwrap();
    let elsewhere = probe(&js);
    assert_eq!(elsewhere, expected.replace("example.com/probe.js", "cdn.example.net/copy.js"));
    assert_eq!(cache.stats().hits, 3);
}

#[test]
fn pages_do_not_see_what_other_pages_did_to_a_cached_script() {
    let source = format!(
        "{}\n{}",
        r#"
var tag = function (strings) { return strings; };
var shared = (function () {
  var count = 0;
  function bump() { count += 1; return count; }
  return { bump: bump, template: function () { return tag`a${1}b`; } };
})();
"#,
        format!("/*{}*/", "y".repeat(2000))
    );
    let cache = cache();
    for round in 0..3 {
        let js = page(&cache);
        js.run_script(&source, "shared.js").unwrap();
        eprintln!("round {round}");
        // Each page starts from the script's own state, whatever the
        // pages before it did to its functions and objects.
        let seen = js
            .eval_json(
                "var t = shared.template();
                 var before = [shared.bump(), shared.bump(), Object.isFrozen(t), t.length,
                               shared.bump.marked === undefined, t.raw.join('|')];
                 shared.bump.marked = true;
                 try { t.push('poison'); } catch (e) {}
                 Object.getPrototypeOf(shared.bump).poisoned = 1;
                 before",
            )
            .unwrap()
            .unwrap();
        assert_eq!(seen, r#"[1,2,true,2,true,"a|b"]"#);
        assert_eq!(js.eval_json("typeof Function.prototype.poisoned").unwrap().as_deref(), Some(r#""number""#));
    }
    assert_eq!(counts(cache.stats()), (1, 2, 1));
}

#[test]
fn short_scripts_and_scripts_that_do_not_compile_are_not_cached() {
    let cache = cache();
    let js = page(&cache);
    js.run_script("var small = 1;", "small.js").unwrap();
    let bad = format!("var x = ;{}", " ".repeat(2000));
    let err = js.run_script(&bad, "bad.js").unwrap_err();
    assert!(err.message.starts_with("SyntaxError"), "{err}");
    drop(js);
    let stats = cache.stats();
    assert_eq!((stats.misses, stats.builds, stats.entries), (1, 0, 0));
}

#[test]
fn the_memory_bound_holds() {
    let cache = ScriptCache::new(ScriptCacheOptions {
        memory_bytes: 12 * 1024,
        ..ScriptCacheOptions::default()
    });
    for i in 0..10 {
        let js = page(&cache);
        js.run_script(&format!("var v{i} = function () {{ return {i}; }};/*{}*/", "z".repeat(3000)), "s.js")
            .unwrap();
    }
    let stats = cache.stats();
    assert!(stats.bytes <= 12 * 1024, "{stats:?}");
    assert!(stats.entries >= 2 && stats.evictions >= 5, "{stats:?}");

    // With no memory at all nothing is kept.
    let none = ScriptCache::new(ScriptCacheOptions {
        memory_bytes: 0,
        ..ScriptCacheOptions::default()
    });
    for _ in 0..2 {
        page(&none).run_script(&probe_source(), "p.js").unwrap();
    }
    assert_eq!((none.stats().entries, none.stats().hits), (0, 0));
}

#[test]
fn a_cache_directory_starts_a_new_process_warm() {
    let dir = std::env::temp_dir().join(format!("dasher-script-cache-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let options = ScriptCacheOptions {
        dir: Some(dir.clone()),
        ..ScriptCacheOptions::default()
    };
    let source = probe_source();
    let first = ScriptCache::new(options.clone());
    let js = page(&first);
    js.run_script(&source, "p.js").unwrap();
    let expected = probe(&js);
    drop(js);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    // a new cache, as a new process would make
    let second = ScriptCache::new(options.clone());
    let js = page(&second);
    js.run_script(&source, "p.js").unwrap();
    assert_eq!(probe(&js), expected);
    let stats = second.stats();
    assert_eq!((stats.hits, stats.disk_hits, stats.misses), (1, 1, 0));

    // A damaged file is ignored, and replaced.
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let mut bytes = std::fs::read(&file).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x55;
    std::fs::write(&file, &bytes).unwrap();
    let third = ScriptCache::new(options);
    let js = page(&third);
    js.run_script(&source, "p.js").unwrap();
    assert_eq!(probe(&js), expected);
    drop(js);
    let stats = third.stats();
    assert_eq!((stats.hits, stats.misses, stats.builds), (0, 1, 1));
    assert_ne!(std::fs::read(&file).unwrap(), bytes);
    let _ = std::fs::remove_dir_all(&dir);
}
