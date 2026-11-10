//! TextEncoder, TextDecoder, atob and btoa.

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
fn text_encoder() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const enc = new TextEncoder();
        const dest = new Uint8Array(5);
        ({ bytes: [...enc.encode('aé€😀')], lone: [...enc.encode('\ud800')], empty: enc.encode().length,
           into: enc.encodeInto('a€😀', dest), dest: [...dest], encoding: enc.encoding,
           type: enc.encode('x') instanceof Uint8Array })
        "#,
    );
    assert_eq!(
        out,
        r#"{"bytes":[97,195,169,226,130,172,240,159,152,128],"lone":[239,191,189],"empty":0,"into":{"read":2,"written":4},"dest":[97,226,130,172,0],"encoding":"utf-8","type":true}"#
    );
}

#[test]
fn text_decoder() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const bytes = new TextEncoder().encode('﻿hé€');
        const d = new TextDecoder();
        const view = new DataView(bytes.buffer, 4);
        let fatal;
        try { new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xff])); } catch (e) { fatal = e.name; }
        let bad;
        try { new TextDecoder('klingon'); } catch (e) { bad = e.name; }
        ({ plain: d.decode(bytes), keep: new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes).length,
           view: d.decode(view), buffer: d.decode(bytes.buffer), lossy: d.decode(new Uint8Array([0x61, 0xff])),
           fatal, bad, latin1: [new TextDecoder('latin1').encoding, new TextDecoder('latin1').decode(new Uint8Array([0xe9, 0x80]))],
           utf16: new TextDecoder('utf-16le').decode(new Uint8Array([0xff, 0xfe, 104, 0, 105, 0])),
           empty: d.decode() })
        "#,
    );
    assert_eq!(
        out,
        r#"{"plain":"hé€","keep":4,"view":"é€","buffer":"hé€","lossy":"a�","fatal":"TypeError","bad":"RangeError","latin1":["windows-1252","é€"],"utf16":"hi","empty":""}"#
    );
}

#[test]
fn text_decoder_streams_across_chunks() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const bytes = new TextEncoder().encode('﻿a€😀b');
        const d = new TextDecoder();
        let out = '';
        for (const b of bytes) out += d.decode(new Uint8Array([b]), { stream: true });
        out += d.decode();
        const again = d.decode(bytes);
        const cut = new TextDecoder().decode(bytes.subarray(0, 6));
        ({ out, again, cut })
        "#,
    );
    assert_eq!(out, r#"{"out":"a€😀b","again":"a€😀b","cut":"a�"}"#);
}

#[test]
fn atob_and_btoa() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const errs = [];
        try { btoa('€'); } catch (e) { errs.push(e.name); }
        try { atob('a'); } catch (e) { errs.push(e.name); }
        ({ b: btoa('helloÿ'), a: atob(' aGVs bG//\n') === 'hello\u00ff', pad: atob('Zm8='), round: atob(btoa('\u0000\u0080x')) === '\u0000\u0080x', errs })
        "#,
    );
    assert_eq!(
        out,
        r#"{"b":"aGVsbG//","a":true,"pad":"fo","round":true,"errs":["InvalidCharacterError","InvalidCharacterError"]}"#
    );
}
