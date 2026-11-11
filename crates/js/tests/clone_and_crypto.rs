//! structuredClone and crypto.

use dasher_js::{JsRuntime, RuntimeOptions};

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

fn runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions::default()).unwrap()
}

#[test]
fn structured_clone_copies_supported_types() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const shared = { n: 1 };
        const buf = new Uint8Array([1, 2, 3, 4]);
        const src = {
          shared, again: shared, date: new Date(5), re: /a+/gi, map: new Map([[shared, 'v']]),
          set: new Set([1, 'two']), arr: [1, , 3], bytes: buf, view: new DataView(buf.buffer, 1, 2),
          big: 10n, boxed: new String('s'), err: new RangeError('bad'), dom: new DOMException('m', 'AbortError'),
          inst: new (class Point { constructor() { this.x = 1; } })(), ['__proto__']: 'own',
        };
        src.self = src;
        const c = structuredClone(src);
        ({ notSame: c !== src && c.shared !== shared, cycle: c.self === c, sharedRef: c.again === c.shared,
           date: c.date instanceof Date && c.date.getTime(), re: [c.re.source, c.re.flags],
           map: c.map.get(c.shared), set: [...c.set], holes: 1 in c.arr, len: c.arr.length,
           bytes: [c.bytes instanceof Uint8Array, [...c.bytes], c.bytes.buffer !== buf.buffer],
           view: [c.view.byteOffset, c.view.getUint8(0), c.view.buffer === c.bytes.buffer],
           big: typeof c.big, boxed: typeof c.boxed === 'object' && String(c.boxed),
           err: [c.err instanceof RangeError, c.err.message], dom: [c.dom.name, c.dom.code],
           inst: [Object.getPrototypeOf(c.inst) === Object.prototype, c.inst.x],
           proto: Object.getPrototypeOf(c) === Object.prototype && c.__proto__ === 'own' })
        "#,
    );
    assert_eq!(
        out,
        r#"{"notSame":true,"cycle":true,"sharedRef":true,"date":5,"re":["a+","gi"],"map":"v","set":[1,"two"],"holes":false,"len":3,"bytes":[true,[1,2,3,4],true],"view":[1,2,true],"big":"bigint","boxed":"s","err":[true,"bad"],"dom":["AbortError",20],"inst":[true,1],"proto":true}"#
    );
}

#[test]
fn structured_clone_rejects_and_transfers() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const errs = [];
        for (const v of [() => 1, Symbol('s'), { f() {} }, new WeakMap(), Promise.resolve(), new EventTarget(), new URL('http://a/')]) {
          try { structuredClone(v); errs.push('cloned'); } catch (e) { errs.push(e.name); }
        }
        const buf = new ArrayBuffer(8);
        new Uint8Array(buf)[0] = 7;
        const moved = structuredClone({ buf }, { transfer: [buf] });
        ({ errs, detached: buf.byteLength, moved: [moved.buf.byteLength, new Uint8Array(moved.buf)[0]],
           prim: [structuredClone(1), structuredClone(null), structuredClone('s')] })
        "#,
    );
    assert_eq!(
        out,
        r#"{"errs":["DataCloneError","DataCloneError","DataCloneError","DataCloneError","DataCloneError","DataCloneError","DataCloneError"],"detached":0,"moved":[8,7],"prim":[1,null,"s"]}"#
    );
}

#[test]
fn crypto_random_values_and_uuids() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const a = new Uint32Array(64);
        const same = crypto.getRandomValues(a) === a;
        const nonzero = a.some((x) => x !== 0);
        const sub = new Uint8Array(16);
        crypto.getRandomValues(sub.subarray(8));
        const errs = [];
        try { crypto.getRandomValues(new Float64Array(2)); } catch (e) { errs.push(e.name); }
        try { crypto.getRandomValues(new Uint8Array(65537)); } catch (e) { errs.push(e.name); }
        try { crypto.getRandomValues([1]); } catch (e) { errs.push(e.name); }
        try { new Crypto(); } catch (e) { errs.push(e.name); }
        const ids = new Set(Array.from({ length: 100 }, () => crypto.randomUUID()));
        const id = crypto.randomUUID();
        ({ same, nonzero, head: sub.subarray(0, 8).every((x) => x === 0), errs, unique: ids.size,
           shape: /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id),
           tag: String(crypto) })
        "#,
    );
    assert_eq!(
        out,
        r#"{"same":true,"nonzero":true,"head":true,"errs":["TypeMismatchError","QuotaExceededError","TypeError","TypeError"],"unique":100,"shape":true,"tag":"[object Crypto]"}"#
    );
}

#[tokio::test]
async fn subtle_digest_hashes_buffer_sources() {
    use dasher_js::{RunLimits, RunOutcome};

    let js = runtime();
    js.run_script(
        r#"
        var out = {};
        const hex = (buffer) => [...new Uint8Array(buffer)].map((b) => b.toString(16).padStart(2, '0')).join('');
        const abc = new TextEncoder().encode('abc');
        (async () => {
            out.sha1 = hex(await crypto.subtle.digest('SHA-1', abc));
            const sha256 = await crypto.subtle.digest({ name: 'sha-256' }, abc.buffer);
            out.sha256 = [Object.prototype.toString.call(sha256), hex(sha256)];
            out.lengths = [
                (await crypto.subtle.digest('SHA-384', new DataView(abc.buffer))).byteLength,
                (await crypto.subtle.digest('SHA-512', new Uint16Array(2))).byteLength,
            ];
            // A view hashes only its own bytes.
            out.view = hex(await crypto.subtle.digest('SHA-256', new Uint8Array([0, 97, 98, 99, 0]).subarray(1, 4)));
            const errors = [];
            for (const args of [['MD5', abc], ['SHA-256', 'abc'], ['SHA-256'], [{}, abc]]) {
                await crypto.subtle.digest(...args).catch((e) => errors.push(e.name));
            }
            out.errors = errors;
        })();
        "#,
        "digest.js",
    )
    .unwrap();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert_eq!(
        eval(&js, "out"),
        r#"{"sha1":"a9993e364706816aba3e25717850c26c9cd0d89d","sha256":["[object ArrayBuffer]","ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"],"lengths":[48,64],"view":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","errors":["NotSupportedError","TypeError","TypeError","TypeError"]}"#
    );
}
