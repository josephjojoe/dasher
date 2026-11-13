//! URL and URLSearchParams.

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
fn parses_and_exposes_components() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const u = new URL('../a b?x=1&y=%C3%A9#frag', 'https://user:pw@EXAMPLE.com:8443/dir/sub/');
        ({ href: u.href, origin: u.origin, protocol: u.protocol, username: u.username, password: u.password,
           host: u.host, hostname: u.hostname, port: u.port, pathname: u.pathname, search: u.search,
           hash: u.hash, str: String(u), json: JSON.stringify({ u }), y: u.searchParams.get('y') })
        "#,
    );
    assert_eq!(
        out,
        r##"{"href":"https://user:pw@example.com:8443/dir/a%20b?x=1&y=%C3%A9#frag","origin":"https://example.com:8443","protocol":"https:","username":"user","password":"pw","host":"example.com:8443","hostname":"example.com","port":"8443","pathname":"/dir/a%20b","search":"?x=1&y=%C3%A9","hash":"#frag","str":"https://user:pw@example.com:8443/dir/a%20b?x=1&y=%C3%A9#frag","json":"{\"u\":\"https://user:pw@example.com:8443/dir/a%20b?x=1&y=%C3%A9#frag\"}","y":"é"}"##
    );
}

#[test]
fn invalid_urls_throw_and_static_helpers() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const errs = [];
        try { new URL('/relative'); } catch (e) { errs.push(e.name); }
        try { new URL('http://a/').href = 'nope'; } catch (e) { errs.push(e.name); }
        ({ errs, can: [URL.canParse('/x', 'http://a/'), URL.canParse('/x')],
           parse: [URL.parse('/x', 'http://a/').href, URL.parse('::')],
           tag: Object.prototype.toString.call(new URL('http://a/')) })
        "#,
    );
    assert_eq!(
        out,
        r#"{"errs":["TypeError","TypeError"],"can":[true,false],"parse":["http://a/x",null],"tag":"[object URL]"}"#
    );
}

#[test]
fn setters_update_the_url() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const u = new URL('http://example.com/path?q=1#h');
        u.pathname = '/new path'; u.port = '8080'; u.hash = ''; u.protocol = 'https';
        u.hostname = 'other.test'; u.port = 'bogus'; u.username = 'me';
        const a = u.href;
        u.search = '?a=b';
        ({ a, b: u.href, param: u.searchParams.get('a') })
        "#,
    );
    assert_eq!(
        out,
        r#"{"a":"https://me@other.test:8080/new%20path?q=1","b":"https://me@other.test:8080/new%20path?a=b","param":"b"}"#
    );
}

#[test]
fn search_params_api() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const p = new URLSearchParams('?b=2&a=1&b=3&c=x+y%21&d');
        const all = p.getAll('b'); p.append('e', 'é &='); p.delete('a'); p.set('b', '9');
        const s1 = p.toString(); p.append('a', '0'); p.sort();
        const fromObj = new URLSearchParams({ x: 1, y: 'two' }).toString();
        const fromPairs = new URLSearchParams([['k', 'v'], ['k', 'w']]).getAll('k');
        const copy = new URLSearchParams(p); copy.append('z', '1');
        const seen = []; p.forEach((v, k) => seen.push(k + '=' + v));
        ({ all, s1, sorted: p.toString(), c: p.get('c'), d: p.get('d'), missing: p.get('nope'),
           has: [p.has('e'), p.has('b', '1'), p.has('b', '9')], size: p.size, fromObj, fromPairs,
           copySize: [p.size, copy.size], entries: [...p].length, keys: [...p.keys()], seen })
        "#,
    );
    assert_eq!(
        out,
        r#"{"all":["2","3"],"s1":"b=9&c=x+y%21&d=&e=%C3%A9+%26%3D","sorted":"a=0&b=9&c=x+y%21&d=&e=%C3%A9+%26%3D","c":"x y!","d":"","missing":null,"has":[true,false,true],"size":5,"fromObj":"x=1&y=two","fromPairs":["v","w"],"copySize":[5,6],"entries":5,"keys":["a","b","c","d","e"],"seen":["a=0","b=9","c=x y!","d=","e=é &="]}"#
    );
}

#[test]
fn search_params_are_live() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const u = new URL('http://a/?x=1');
        const p = u.searchParams;
        p.append('y', 'a b');
        const one = u.href;
        u.search = '?z=3';
        const two = [p.get('z'), p.has('x')];
        p.delete('z');
        ({ one, two, three: u.href, same: u.searchParams === p })
        "#,
    );
    assert_eq!(
        out,
        r#"{"one":"http://a/?x=1&y=a+b","two":["3",false],"three":"http://a/","same":true}"#
    );
}

/// Strings reach Rust as USVStrings: lone surrogates become U+FFFD, and
/// other values are stringified first.
#[test]
fn inputs_are_converted_to_usv_strings() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const u = new URL('http://a/\ud800?q=\udc00#😀');
        u.hash = 'x\ud800';
        ({ href: u.href, q: u.searchParams.get('q'),
           obj: new URL({ toString() { return '/p'; } }, 'http://b/').href,
           num: new URL(42, 'http://c/d/').href,
           base: new URL('x', 'http://d/\ud800/').href })
        "#,
    );
    assert_eq!(
        out,
        r#"{"href":"http://a/%EF%BF%BD?q=%EF%BF%BD#x%EF%BF%BD","q":"�","obj":"http://b/p","num":"http://c/d/42","base":"http://d/%EF%BF%BD/x"}"#
    );
}

/// The last base is kept for the next parse: changing it, or giving a bad
/// one, must not reuse the old one.
#[test]
fn bases_are_parsed_afresh_when_they_change() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const r = [];
        for (const base of ['http://a/x/', 'http://a/x/', 'http://b/', 'nope', 'http://b/', '']) {
          try { r.push(new URL('y?z', base).href); } catch (e) { r.push(e.message); }
        }
        r
        "#,
    );
    assert_eq!(
        out,
        r#"["http://a/x/y?z","http://a/x/y?z","http://b/y?z","Failed to construct 'URL': Invalid URL","http://b/y?z","Failed to construct 'URL': Invalid URL"]"#
    );
}

/// Setters change the URL in place: a failed `href` setter leaves it as it
/// was, and the `href` and `search` setters refresh its searchParams.
#[test]
fn setters_change_the_url_in_place() {
    let js = runtime();
    let out = eval(
        &js,
        r#"
        const u = new URL('https://h.example/p?a=1');
        const p = u.searchParams;
        let err;
        try { u.href = 'not a url'; } catch (e) { err = e.message; }
        const kept = u.href;
        u.href = 'http://other.example:81/q?b=2#f';
        const afterHref = [p.get('a'), p.get('b'), u.port, u.origin];
        u.protocol = 'nope nope';
        u.host = '';
        p.set('c', '3');
        const parsed = URL.parse('/r', u);
        ({ err, kept, afterHref, href: u.href, same: u.searchParams === p,
           parsed: [parsed.href, parsed === URL.parse('/r', u), URL.parse('::', u)] })
        "#,
    );
    assert_eq!(
        out,
        r#"{"err":"Failed to set the 'href' property on 'URL': Invalid URL","kept":"https://h.example/p?a=1","afterHref":[null,"2","81","http://other.example:81"],"href":"http://other.example:81/q?b=2&c=3#f","same":true,"parsed":["http://other.example:81/r",false,"http://other.example:81/::"]}"#
    );
}
