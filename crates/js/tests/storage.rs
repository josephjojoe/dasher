//! localStorage and sessionStorage.

use dasher_js::{JsRuntime, RuntimeOptions, StorageStore, WindowOptions};

fn page(url: &str, local: &StorageStore) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        window: WindowOptions {
            url: url.into(),
            local_storage: local.clone(),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn eval(js: &JsRuntime, source: &str) -> String {
    js.eval_json(source)
        .unwrap_or_else(|e| panic!("{source}: {e}"))
        .unwrap_or_default()
}

#[test]
fn storage_api() {
    let js = page("https://example.com/", &StorageStore::new());
    let out = eval(
        &js,
        r#"
        localStorage.setItem('b', 2);
        localStorage.setItem('a', { toString() { return 'obj'; } });
        const before = [localStorage.length, localStorage.key(0), localStorage.key(1), localStorage.key(2)];
        const got = [localStorage.getItem('a'), localStorage.getItem('missing')];
        localStorage.removeItem('b');
        const after = localStorage.length;
        localStorage.clear();
        ({ before, got, after, cleared: localStorage.length, tag: String(localStorage),
           isStorage: localStorage instanceof Storage, separate: sessionStorage.length,
           illegal: (() => { try { new Storage(); } catch (e) { return e.name; } })() })
        "#,
    );
    assert_eq!(
        out,
        r#"{"before":[2,"a","b",null],"got":["obj",null],"after":1,"cleared":0,"tag":"[object Storage]","isStorage":true,"separate":0,"illegal":"TypeError"}"#
    );
}

#[test]
fn items_are_named_properties() {
    let js = page("https://example.com/", &StorageStore::new());
    let out = eval(
        &js,
        r#"
        localStorage.token = 'abc';
        localStorage['length'] = 'shadowed';
        localStorage.getItem = 'also stored';
        const read = [localStorage.token, localStorage.length, typeof localStorage.getItem,
                      'token' in localStorage, localStorage.nothing];
        const keys = Object.keys(localStorage);
        const json = JSON.stringify(localStorage);
        delete localStorage.token;
        ({ read, keys, json, deleted: localStorage.getItem('token'), lengthItem: localStorage.getItem('length') })
        "#,
    );
    assert_eq!(
        out,
        r#"{"read":["abc",3,"function",true,null],"keys":["getItem","length","token"],"json":"{\"length\":3,\"token\":\"abc\"}","deleted":null,"lengthItem":"shadowed"}"#
    );
}

#[test]
fn local_storage_is_shared_per_origin_through_the_store() {
    let store = StorageStore::new();
    store
        .set("https://example.com", "seeded", "from host")
        .unwrap();
    let first = page("https://example.com/a", &store);
    let other_origin = page("https://other.test/", &store);
    assert_eq!(eval(&first, "localStorage.seeded"), r#""from host""#);
    eval(
        &first,
        "localStorage.setItem('k', 'v'); sessionStorage.setItem('s', '1'); 0",
    );
    let second = page("https://example.com/b", &store);
    assert_eq!(
        eval(
            &second,
            "[localStorage.getItem('k'), sessionStorage.getItem('s')]"
        ),
        r#"["v",null]"#
    );
    assert_eq!(eval(&other_origin, "localStorage.length"), "0");
    assert_eq!(store.get("https://example.com", "k").as_deref(), Some("v"));
}

#[test]
fn quota_and_opaque_origins() {
    let js = page("https://example.com/", &StorageStore::with_quota(100));
    let out = eval(
        &js,
        r#"
        let error;
        try { localStorage.setItem('big', 'x'.repeat(100)); } catch (e) { error = [e.name, e instanceof DOMException]; }
        ({ error, stored: localStorage.getItem('big') })
        "#,
    );
    assert_eq!(
        out,
        r#"{"error":["QuotaExceededError",true],"stored":null}"#
    );

    let store = StorageStore::new();
    let blank = page("about:blank", &store);
    assert_eq!(
        eval(
            &blank,
            "localStorage.setItem('x', '1'); localStorage.getItem('x')"
        ),
        r#""1""#
    );
    assert!(store.origins().is_empty());
}
