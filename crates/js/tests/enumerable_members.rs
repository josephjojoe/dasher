//! Web IDL makes the members of an interface enumerable: its operations,
//! attributes and constants, on the prototype and (static ones) on the
//! interface object. Scripts count on it: a wrapper of `XMLHttpRequest`
//! built by forwarding what a `for…in` over the prototype finds misses
//! every member that is not. In Chrome none of an interface's own members
//! is hidden but `constructor` and the symbol-keyed ones; the glue's
//! classes get there through `internals.expose` (see `prelude.js`).

use dasher_dom::Document;
use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};

fn page(html: &str) -> JsRuntime {
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        ..RuntimeOptions::default()
    })
    .unwrap();
    js.set_document(Document::parse(html));
    js
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

/// The constructors ECMAScript defines, whose members are not enumerable
/// in a browser either.
const ECMASCRIPT: &str = r#"[
  'Object', 'Function', 'Array', 'Number', 'Boolean', 'String', 'Symbol', 'BigInt', 'Date', 'Promise',
  'RegExp', 'Error', 'AggregateError', 'EvalError', 'RangeError', 'ReferenceError', 'SyntaxError',
  'TypeError', 'URIError', 'InternalError', 'ArrayBuffer', 'SharedArrayBuffer', 'DataView', 'Uint8Array',
  'Int8Array', 'Uint16Array', 'Int16Array', 'Uint32Array', 'Int32Array', 'Float16Array', 'Float32Array',
  'Float64Array', 'Uint8ClampedArray', 'BigUint64Array', 'BigInt64Array', 'Map', 'Set', 'WeakMap',
  'WeakSet', 'WeakRef', 'FinalizationRegistry', 'Proxy', 'Iterator',
]"#;

/// The members left hidden on purpose.
const HIDDEN: &str = r#"[
  // Location's members are [LegacyUnforgeable], which in a browser puts
  // them on the object itself with a `valueOf` that is not enumerable:
  // `for (k in location)` lists `toString` and not `valueOf`. Here they
  // are on the prototype, and `valueOf` keeps that.
  'Location.prototype.valueOf',
]"#;

/// Evaluates `call` with `walk(holder, ecmascript)` defined, which goes
/// through the properties of `holder` that are constructors with a
/// prototype and returns `{ interfaces, members, hidden }`: how many there
/// are, how many own string-keyed members they have (the prototype's but
/// `constructor`, the constructor's but `length`, `name` and `prototype`),
/// and the names of those that are not enumerable. `ecmascript` picks the
/// constructors of [`ECMASCRIPT`] or all the others.
fn walk(js: &JsRuntime, call: &str) -> String {
    let source = r#"
        const walk = (holder, ecmascript) => {
          const builtins = new Set(ECMASCRIPT);
          const describe = Object.getOwnPropertyDescriptor;
          const hidden = [];
          let interfaces = 0, members = 0;
          for (const name of Object.getOwnPropertyNames(holder)) {
            if (holder === globalThis && builtins.has(name) !== ecmascript) continue;
            const C = holder[name];
            if (typeof C !== 'function' || typeof C.prototype !== 'object' || C.prototype === null) continue;
            interfaces++;
            for (const key of Object.getOwnPropertyNames(C.prototype)) {
              if (key === 'constructor') continue;
              members++;
              if (!describe(C.prototype, key).enumerable) hidden.push(name + '.prototype.' + key);
            }
            for (const key of Object.getOwnPropertyNames(C)) {
              if (key === 'length' || key === 'name' || key === 'prototype') continue;
              members++;
              if (!describe(C, key).enumerable) hidden.push(name + '.' + key);
            }
          }
          return { interfaces, members, hidden };
        };
        CALL
    "#;
    eval(
        js,
        &source
            .replace("ECMASCRIPT", ECMASCRIPT)
            .replace("CALL", call),
    )
}

#[test]
fn every_member_of_an_interface_is_enumerable() {
    let js = page("<p>x</p>");
    // The walk finds the interfaces (some 250, with some 2,300 members),
    // and nothing hidden but what is hidden on purpose.
    let out = walk(
        &js,
        &format!(
            "const found = walk(globalThis, false); const allowed = new Set({HIDDEN});
             [found.interfaces > 200, found.members > 2000, found.hidden.filter((m) => !allowed.has(m)),
              [...allowed].filter((m) => !found.hidden.includes(m))]"
        ),
    );
    assert_eq!(out, "[true,true,[],[]]");
}

#[test]
fn interfaces_that_are_not_globals_too() {
    // Objects pages reach whose interface is not a global here (or is made
    // some other way than as a class the glue exposes): every member up
    // their prototype chains.
    let js = page("<p class=a>x</p>");
    let out = eval(
        &js,
        r#"
        const objects = {
          'navigator.userAgentData': navigator.userAgentData,
          'performance': performance,
          'performance.timing': performance.timing,
          'performance.navigation': performance.navigation,
          'navigation entry': performance.getEntriesByType('navigation')[0],
          'mark': performance.mark('m'),
          'console': console,
          'CSS': CSS,
          'crypto.subtle': crypto.subtle,
          'location.ancestorOrigins': location.ancestorOrigins,
        };
        const hidden = [];
        for (const [name, object] of Object.entries(objects)) {
          if (object === null || typeof object !== 'object') hidden.push(name + ' is ' + object);
          for (let o = object; o !== null && o !== Object.prototype; o = Object.getPrototypeOf(o)) {
            for (const key of Object.getOwnPropertyNames(o)) {
              if (key !== 'constructor' && !Object.getOwnPropertyDescriptor(o, key).enumerable) {
                hidden.push(name + ': ' + key);
              }
            }
          }
        }
        hidden
        "#,
    );
    assert_eq!(out, "[]");
}

#[test]
fn ecmascript_and_intl_members_stay_hidden() {
    let js = page("");
    let out = walk(
        &js,
        "const es = walk(globalThis, true), intl = walk(Intl, false);
         [es.interfaces > 35, es.members > 350, es.hidden.length === es.members,
          intl.interfaces >= 7, intl.members > 30, intl.hidden.length === intl.members,
          Object.keys(Intl), Object.keys(Array.prototype), Object.keys(Promise), Object.keys(Date.prototype)]",
    );
    assert_eq!(out, "[true,true,true,true,true,true,[],[],[],[]]");
}

#[test]
fn for_in_finds_what_it_does_in_chrome() {
    let js = page("<p>x</p>");
    let out = eval(
        &js,
        r#"
        const keysIn = (o) => { const keys = []; for (const k in o) keys.push(k); return keys; };
        // The `names` a `for…in` over `o` misses.
        const missed = (o, ...names) => { const keys = new Set(keysIn(o)); return names.filter((n) => !keys.has(n)); };
        [
          Object.keys(URL.prototype).sort(),
          Object.keys(Storage.prototype).sort(),
          Object.keys(Response).sort(), Object.keys(AbortSignal).sort(),
          ['canParse', 'parse'].filter((k) => !Object.keys(URL).includes(k)),
          missed(document.querySelector('p'), 'tagName', 'getAttribute', 'innerHTML', 'click', 'style',
            'appendChild', 'nodeType', 'addEventListener', 'ELEMENT_NODE'),
          missed(document, 'createElement', 'body', 'querySelector', 'readyState'),
          missed(new XMLHttpRequest(), 'open', 'send', 'setRequestHeader', 'readyState', 'onload', 'DONE'),
          missed(new Headers(), 'get', 'set', 'forEach', 'entries'),
          missed(localStorage, 'getItem', 'length'),
          // Never the hidden ones.
          keysIn(document.body).filter((k) => k === 'constructor'),
          keysIn(location).filter((k) => k === 'valueOf'),
        ]"#,
    );
    assert_eq!(
        out,
        concat!(
            r#"[["hash","host","hostname","href","origin","password","pathname","port","protocol","#,
            r#""search","searchParams","toJSON","toString","username"],"#,
            r#"["clear","getItem","key","length","removeItem","setItem"],"#,
            r#"["error","json","redirect"],["abort","any","timeout"],"#,
            r#"[],[],[],[],[],[],[],[]]"#
        )
    );
}

#[test]
fn streams_loaded_through_a_body() {
    // The streams glue is loaded on first use, which may be a body rather
    // than a global (see lazy.js).
    let js = page("");
    let out = eval(
        &js,
        r#"
        const proto = Object.getPrototypeOf(new Response('x').body);
        [Object.prototype.toString.call(proto), Object.keys(proto).sort()]
        "#,
    );
    assert_eq!(
        out,
        r#"["[object ReadableStream]",["cancel","getReader","locked","pipeThrough","pipeTo","tee","values"]]"#
    );
}

#[test]
fn the_window_s_own_members_are_enumerable_on_the_global() {
    // In Chrome the global's own properties are enumerable when they are
    // the Window's operations and attributes (`fetch`, `document`), and
    // not when they are interface objects, namespaces (`console`, `CSS`,
    // `Intl`) or ECMAScript's own: which here is every name that starts in
    // lower case but the ones listed.
    let js = page("");
    let out = eval(
        &js,
        r#"
        const hidden = new Set(['console', 'globalThis', 'undefined', 'eval', 'parseInt', 'parseFloat', 'isNaN',
          'isFinite', 'decodeURI', 'decodeURIComponent', 'encodeURI', 'encodeURIComponent', 'escape',
          'unescape']);
        const wrong = [];
        let enumerable = 0;
        for (const name of Object.getOwnPropertyNames(globalThis)) {
          const expected = /^[a-z]/.test(name) && !hidden.has(name);
          const actual = Object.getOwnPropertyDescriptor(globalThis, name).enumerable;
          if (actual) enumerable++;
          if (actual !== expected) wrong.push(name);
        }
        const keys = Object.keys(globalThis);
        [wrong, enumerable > 80,
         ['fetch', 'alert', 'atob', 'btoa', 'getComputedStyle', 'getSelection', 'matchMedia', 'postMessage',
          'structuredClone', 'queueMicrotask', 'setTimeout', 'requestAnimationFrame', 'scrollTo',
          'customElements', 'document', 'navigator', 'location', 'performance'].filter((k) => !keys.includes(k)),
         ['URL', 'Node', 'XMLHttpRequest', 'ReadableStream', 'Intl', 'CSS', 'console', 'Promise']
           .filter((k) => keys.includes(k)),
         Object.keys(console).includes('log')]
        "#,
    );
    assert_eq!(out, "[[],true,[],[],true]");
}
