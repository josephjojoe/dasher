//! `fetch()`, `Headers`, `Request`, `Response`, `Blob` and `FormData`,
//! against the in-process fixture server.

use std::time::Duration;

use dasher_js::{ClockMode, JsRuntime, RunLimits, RunOutcome, RuntimeOptions};
use dasher_net::{Client, RequestPolicy, Session};
use dasher_test_server::{FixtureResponse, FixtureServer};

fn session() -> Session {
    Client::builder().build().unwrap().session()
}

fn page(network: Option<Session>) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Virtual,
        network,
        ..RuntimeOptions::default()
    })
    .unwrap()
}

/// Runs `script` and the event loop until the page is idle.
async fn run(js: &JsRuntime, script: &str) {
    js.run_script(script, "test.js").unwrap();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert_eq!(js.take_errors(), []);
}

fn json(js: &JsRuntime, expr: &str) -> String {
    js.eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn server() -> FixtureServer {
    FixtureServer::builder()
        .route("/api/items", |_| {
            FixtureResponse::json(r#"{"items":["a","b"]}"#)
                .with_header("x-total", "2")
                .with_header("set-cookie", "secret=1")
        })
        .route("/echo", |req| {
            let headers: Vec<String> = req
                .headers
                .iter()
                .map(|(n, v)| format!("{n}: {v}"))
                .collect();
            FixtureResponse::json(format!(
                r#"{{"method":{:?},"query":{:?},"headers":{:?},"body":{:?}}}"#,
                req.method,
                req.query.clone().unwrap_or_default(),
                headers,
                String::from_utf8_lossy(&req.body),
            ))
        })
        .route("/text", |_| FixtureResponse::text("héllo wörld"))
        .route("/missing", |_| FixtureResponse::not_found())
        .route("/no-content", |_| FixtureResponse::new(204))
        .route("/redirect", |_| {
            FixtureResponse::redirect(302, "/text#frag")
        })
        .route("/login", |_| {
            FixtureResponse::text("ok").with_header("set-cookie", "session=abc; Path=/")
        })
        .route("/slow", |_| FixtureResponse::text("slow"))
        .path_latency("/slow", Duration::from_millis(300))
        .start()
        .unwrap()
}

#[tokio::test]
async fn fetches_json_with_status_and_headers() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            fetch({url:?}).then(async (r) => {{
                out.status = r.status;
                out.ok = r.ok;
                out.statusText = r.statusText;
                out.url = r.url;
                out.redirected = r.redirected;
                out.type = r.type;
                out.total = r.headers.get('X-Total');
                out.cookie = r.headers.get('set-cookie');
                out.contentType = r.headers.get('content-type');
                out.bodyUsedBefore = r.bodyUsed;
                out.data = await r.json();
                out.bodyUsedAfter = r.bodyUsed;
                out.again = await r.text().catch((e) => e.constructor.name);
            }});
            "#,
            url = server.url("/api/items")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "out"),
        format!(
            r#"{{"status":200,"ok":true,"statusText":"OK","url":"{}","redirected":false,"type":"cors","total":"2","cookie":null,"contentType":"application/json","bodyUsedBefore":false,"data":{{"items":["a","b"]}},"bodyUsedAfter":true,"again":"TypeError"}}"#,
            server.url("/api/items")
        )
    );
}

#[tokio::test]
async fn text_is_utf8_and_errors_statuses_resolve() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = [];
            fetch({text:?}).then((r) => r.text()).then((t) => out.push(t));
            fetch({missing:?}).then((r) => out.push([r.status, r.ok]));
            fetch({empty:?}).then((r) => out.push([r.status, r.body]));
            "#,
            text = server.url("/text"),
            missing = server.url("/missing"),
            empty = server.url("/no-content"),
        ),
    )
    .await;
    let out = json(&js, "out");
    assert!(out.contains(r#""héllo wörld""#), "{out}");
    assert!(out.contains("[404,false]"), "{out}");
    assert!(out.contains("[204,null]"), "{out}");
}

#[tokio::test]
async fn posts_bodies_and_headers() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = [];
            const url = {url:?};
            fetch(url, {{
                method: 'post',
                headers: {{ 'Content-Type': 'application/json', 'X-Requested-With': 'XMLHttpRequest', Cookie: 'forged=1', Host: 'evil' }},
                body: JSON.stringify({{ q: 'dasher' }}),
            }}).then((r) => r.json()).then((d) => out.push(d));
            fetch(new Request(url + '?a=1', {{ method: 'PUT', body: new Uint8Array([104, 105]) }}))
                .then((r) => r.json()).then((d) => out.push(d));
            const form = new FormData();
            form.append('name', 'dasher');
            form.append('file', new Blob(['contents'], {{ type: 'text/plain' }}), 'a.txt');
            fetch(url, {{ method: 'POST', body: form }}).then((r) => r.json()).then((d) => out.push(d));
            "#,
            url = server.url("/echo")
        ),
    )
    .await;

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    let by_method = |m: &str| requests.iter().find(|r| r.method == m).unwrap();

    let post_json = requests
        .iter()
        .find(|r| r.header("content-type") == Some("application/json"))
        .unwrap();
    assert_eq!(post_json.method, "POST");
    assert_eq!(&post_json.body[..], br#"{"q":"dasher"}"#);
    assert_eq!(post_json.header("x-requested-with"), Some("XMLHttpRequest"));
    // Forbidden headers are dropped; the session supplies its own.
    assert_eq!(post_json.header("cookie"), None);
    assert_ne!(post_json.header("host"), Some("evil"));
    // POSTs carry the requesting origin (an opaque one here: no document).
    assert_eq!(post_json.header("origin"), Some("null"));

    let put = by_method("PUT");
    assert_eq!(put.query.as_deref(), Some("a=1"));
    assert_eq!(&put.body[..], b"hi");
    assert_eq!(put.header("content-type"), None);

    let multipart = requests
        .iter()
        .find(|r| {
            r.header("content-type")
                .is_some_and(|t| t.starts_with("multipart/form-data; boundary="))
        })
        .unwrap();
    let boundary = multipart.header("content-type").unwrap()[30..].to_owned();
    let body = String::from_utf8_lossy(&multipart.body).into_owned();
    assert_eq!(
        body,
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\ndasher\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\
             Content-Type: text/plain\r\n\r\ncontents\r\n--{boundary}--\r\n"
        )
    );
}

#[tokio::test]
async fn follows_redirects_or_not() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            const url = {url:?};
            fetch(url).then(async (r) => {{
                out.follow = [r.status, r.redirected, r.url, await r.text()];
            }});
            fetch(url, {{ redirect: 'manual' }}).then((r) => {{
                out.manual = [r.type, r.status, r.url, r.body];
            }});
            fetch(url, {{ redirect: 'error' }}).catch((e) => {{
                out.error = [e.name, e.message];
            }});
            "#,
            url = server.url("/redirect")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "out.follow"),
        format!(r#"[200,true,"{}","héllo wörld"]"#, server.url("/text"))
    );
    assert_eq!(json(&js, "out.manual"), r#"["opaqueredirect",0,"",null]"#);
    assert_eq!(json(&js, "out.error"), r#"["TypeError","Failed to fetch"]"#);
}

#[tokio::test]
async fn cookies_follow_the_credentials_mode() {
    let server = server();
    let session = session();
    let js = page(Some(session.clone()));
    run(
        &js,
        &format!(
            r#"
            const origin = {origin:?};
            (async () => {{
                await fetch(origin + '/login', {{ credentials: 'include' }});
                await fetch(origin + '/echo?mode=include', {{ credentials: 'include' }});
                await fetch(origin + '/echo?mode=omit', {{ credentials: 'omit' }});
                // Same-origin is the default, and this page has no origin.
                await fetch(origin + '/echo?mode=same-origin');
            }})();
            "#,
            origin = server.origin()
        ),
    )
    .await;
    let cookie_for = |mode: &str| {
        server
            .requests()
            .into_iter()
            .find(|r| r.query.as_deref() == Some(&format!("mode={mode}")))
            .unwrap()
            .header("cookie")
            .map(String::from)
    };
    assert_eq!(cookie_for("include").as_deref(), Some("session=abc"));
    assert_eq!(cookie_for("omit"), None);
    assert_eq!(cookie_for("same-origin"), None);
    // The cookie landed in the session's jar.
    let url = dasher_net::Url::parse(&server.url("/")).unwrap();
    assert_eq!(
        session.cookies().header_for(&url).as_deref(),
        Some("session=abc")
    );
}

#[tokio::test]
async fn abort_signal_cancels_the_request() {
    let server = server();
    let session = session();
    let js = page(Some(session.clone()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            const controller = new AbortController();
            fetch({url:?}, {{ signal: controller.signal }}).then(
                () => {{ out.result = 'resolved'; }},
                (e) => {{ out.result = [e.name, e instanceof DOMException]; }});
            setTimeout(() => controller.abort(), 50);
            fetch({url:?}, {{ signal: AbortSignal.abort('why') }}).catch((e) => {{ out.already = e; }});
            fetch({url:?}, {{ signal: AbortSignal.timeout(10) }}).catch((e) => {{ out.timeout = e.name; }});
            "#,
            url = server.url("/slow")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "out"),
        r#"{"already":"why","timeout":"TimeoutError","result":["AbortError",true]}"#
    );
    // Aborted requests stop counting as in flight.
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn aborting_after_the_headers_fails_the_body() {
    let server = FixtureServer::builder()
        .route("/big", |_| FixtureResponse::text("x".repeat(1 << 20)))
        .start()
        .unwrap();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            const controller = new AbortController();
            fetch({url:?}, {{ signal: controller.signal }}).then((r) => {{
                out.status = r.status;
                controller.abort();
                return r.text();
            }}).catch((e) => {{ out.error = e.name; }});
            "#,
            url = server.url("/big")
        ),
    )
    .await;
    // Either the abort beat the body, or the body had already arrived.
    let out = json(&js, "out");
    assert!(
        out == r#"{"status":200,"error":"AbortError"}"# || out == r#"{"status":200}"#,
        "{out}"
    );
}

#[tokio::test]
async fn blocked_and_failed_requests_reject_with_type_error() {
    let server = server();
    let session = session().with_policy(RequestPolicy::new().block_url("*://*/api/*"));
    let js = page(Some(session.clone()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            fetch({blocked:?}).catch((e) => out.blocked = e.name + ': ' + e.message);
            fetch('http://127.0.0.1:1/').catch((e) => out.refused = e.message);
            fetch('ftp://example.com/').catch((e) => out.scheme = e.message);
            fetch('http://[bad').catch((e) => out.invalid = e.name);
            fetch({url:?}, {{ method: 'GET', body: 'x' }}).catch((e) => out.getBody = e.name);
            "#,
            blocked = server.url("/api/items"),
            url = server.url("/echo"),
        ),
    )
    .await;
    assert_eq!(
        json(
            &js,
            "[out.blocked, out.refused, out.scheme, out.invalid, out.getBody]"
        ),
        r#"["TypeError: Failed to fetch","Failed to fetch","Failed to fetch","TypeError","TypeError"]"#
    );
    assert_eq!(server.request_count(), 0);
    // The reason only goes to the console.
    let console = js.take_console();
    assert_eq!(console.len(), 3, "{console:?}");
    assert!(
        console.iter().any(|m| m.text.contains("blocked")),
        "{console:?}"
    );
    // Blocked requests never counted as network activity.
    assert_eq!(session.activity().requests_started(), 1);
}

#[tokio::test]
async fn requests_count_as_network_activity() {
    let server = server();
    let session = session();
    let js = page(Some(session.clone()));
    let mut rx = session.activity().subscribe();
    let watcher = tokio::spawn(async move {
        let mut seen = *rx.borrow();
        while rx.changed().await.is_ok() {
            seen = seen.max(*rx.borrow());
            if *rx.borrow() == 0 {
                break;
            }
        }
        seen
    });
    js.run_script(
        &format!("fetch({:?}).then((r) => r.text());", server.url("/slow")),
        "a.js",
    )
    .unwrap();
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
    assert_eq!(watcher.await.unwrap(), 1);
    assert_eq!(session.activity().in_flight(), 0);
    assert_eq!(session.activity().requests_started(), 1);
}

#[tokio::test]
async fn data_urls_and_no_network() {
    let js = page(None);
    run(
        &js,
        r#"
        var out = {};
        fetch('data:application/json;base64,eyJhIjoxfQ==').then(async (r) => {
            out.json = [r.status, r.headers.get('content-type'), await r.json()];
        });
        fetch('data:,a%20b').then((r) => r.blob()).then(async (b) => out.blob = [b.type, await b.text()]);
        fetch('https://example.com/').catch((e) => out.offline = e.message);
        "#,
    )
    .await;
    assert_eq!(
        json(&js, "[out.json, out.blob, out.offline]"),
        r#"[[200,"application/json",{"a":1}],["text/plain;charset=us-ascii","a b"],"Failed to fetch"]"#
    );
}

#[tokio::test]
async fn response_body_can_be_read_as_a_stream() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            fetch({url:?}).then(async (r) => {{
                const reader = r.body.getReader();
                out.locked = r.body.locked;
                const chunks = [];
                for (;;) {{
                    const {{ value, done }} = await reader.read();
                    if (done) break;
                    chunks.push(value.length, value instanceof Uint8Array);
                }}
                out.chunks = chunks;
                out.used = r.bodyUsed;
                out.again = await r.text().catch((e) => e.name);
                const r2 = await fetch({url:?});
                let n = 0;
                for await (const chunk of r2.body) n += chunk.length;
                out.iterated = n;
            }});
            "#,
            url = server.url("/text")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "out"),
        r#"{"locked":true,"chunks":[13,true],"used":true,"again":"TypeError","iterated":13}"#
    );
}

#[tokio::test]
async fn headers_class() {
    let js = page(None);
    run(
        &js,
        r#"
        const h = new Headers({ 'X-B': ' 2 ', 'x-a': '1' });
        h.append('X-A', '3');
        h.append('Set-Cookie', 'a=1');
        h.append('set-cookie', 'b=2');
        var out = {
            a: h.get('x-a'),
            has: [h.has('X-B'), h.has('x-c')],
            entries: [...h],
            cookies: h.getSetCookie(),
        };
        h.set('x-a', '4');
        h.delete('x-b');
        out.after = [...h.keys()];
        out.fromPairs = [...new Headers([['k', 'v']]).entries()];
        out.errors = [];
        for (const f of [() => h.append('bad name', 'v'), () => h.set('x', 'a\nb'),
                         () => new Headers([['only-one']]), () => Response.error().headers.set('a', 'b')]) {
            try { f(); out.errors.push('no error'); } catch (e) { out.errors.push(e.name); }
        }
        const r = new Request('data:,x', { headers: { Cookie: 'c=1', 'Sec-Fetch-Mode': 'x', Accept: 'a' } });
        out.request = [...r.headers];
        "#,
    )
    .await;
    assert_eq!(
        json(&js, "out"),
        r#"{"a":"1, 3","has":[true,false],"entries":[["set-cookie","a=1"],["set-cookie","b=2"],["x-a","1, 3"],["x-b","2"]],"cookies":["a=1","b=2"],"after":["set-cookie","set-cookie","x-a"],"fromPairs":[["k","v"]],"errors":["TypeError","TypeError","TypeError","TypeError"],"request":[["accept","a"]]}"#
    );
}

#[tokio::test]
async fn request_and_response_classes() {
    let js = page(None);
    run(
        &js,
        r#"
        var out = {};
        (async () => {
            const req = new Request('https://example.com/a?b#c', { method: 'patch', body: 'hi' });
            out.request = [req.method, req.url, req.headers.get('content-type'), req.credentials,
                           req.mode, req.redirect, req.signal.aborted, req.referrer];
            const copy = new Request(req, { headers: { 'x-a': '1' } });
            out.transferred = [req.bodyUsed, await copy.text(), [...copy.headers]];
            out.errors = [];
            for (const f of [() => new Request('https://u:p@example.com/'),
                             () => new Request('https://example.com/', { method: 'CONNECT' }),
                             () => new Request('https://example.com/', { mode: 'navigate' }),
                             () => new Response('x', { status: 204 }),
                             () => new Response('', { status: 700 }),
                             () => req.clone(),
                             () => new Request('http://[bad')]) {
                try { f(); out.errors.push('no error'); } catch (e) { out.errors.push(e.name); }
            }
            const res = new Response('{"a":[1]}', { status: 201, statusText: 'Made', headers: { 'X-A': 'b' } });
            const clone = res.clone();
            out.response = [res.status, res.ok, res.statusText, res.type, res.headers.get('x-a'),
                            await res.json(), await clone.text()];
            const json = Response.json({ x: 1 }, { status: 202 });
            out.json = [json.status, json.headers.get('content-type'), await json.text()];
            const redirect = Response.redirect('https://example.com/next', 301);
            out.redirect = [redirect.status, redirect.headers.get('location')];
            const error = Response.error();
            out.error = [error.type, error.status, error.body];
            const bytes = await new Response(new Uint8Array([1, 2, 3])).arrayBuffer();
            out.bytes = [bytes instanceof ArrayBuffer, bytes.byteLength];
            const form = await new Response('a=1&b=x+y%21&a=2', {
                headers: { 'content-type': 'application/x-www-form-urlencoded' },
            }).formData();
            out.form = [...form];
            out.nullBody = [new Response().body, await new Response().text()];
        })();
        "#,
    )
    .await;
    assert_eq!(
        json(&js, "out.request"),
        r#"["patch","https://example.com/a?b#c","text/plain;charset=UTF-8","same-origin","cors","follow",false,"about:client"]"#
    );
    assert_eq!(json(&js, "out.transferred"), r#"[true,"hi",[["x-a","1"]]]"#);
    assert_eq!(
        json(&js, "out.errors"),
        r#"["TypeError","TypeError","TypeError","TypeError","RangeError","TypeError","TypeError"]"#
    );
    assert_eq!(
        json(&js, "out.response"),
        r#"[201,true,"Made","default","b",{"a":[1]},"{\"a\":[1]}"]"#
    );
    assert_eq!(
        json(&js, "out.json"),
        r#"[202,"application/json","{\"x\":1}"]"#
    );
    assert_eq!(
        json(&js, "out.redirect"),
        r#"[301,"https://example.com/next"]"#
    );
    assert_eq!(json(&js, "out.error"), r#"["error",0,null]"#);
    assert_eq!(json(&js, "out.bytes"), "[true,3]");
    assert_eq!(
        json(&js, "out.form"),
        r#"[["a","1"],["b","x y!"],["a","2"]]"#
    );
    assert_eq!(json(&js, "out.nullBody"), r#"[null,""]"#);
}

#[tokio::test]
async fn blob_file_and_form_data() {
    let js = page(None);
    run(
        &js,
        r#"
        var out = {};
        (async () => {
            const blob = new Blob(['héllo', new Uint8Array([33]), new Blob([' x'])], { type: 'Text/Plain' });
            out.blob = [blob.size, blob.type, await blob.text(), await blob.slice(1, -2).text(),
                        await blob.slice(-1).text(), (await blob.arrayBuffer()).byteLength];
            const file = new File(['abc'], 'a.txt', { type: 'text/plain', lastModified: 42 });
            out.file = [file.name, file.lastModified, file.size, file instanceof Blob, String(file)];
            const form = new FormData();
            form.append('a', '1');
            form.append('a', '2');
            form.set('b', blob);
            form.append('c', file, 'renamed.txt');
            out.form = [form.get('a'), form.getAll('a'), form.has('b'), form.get('b').name,
                        form.get('b') instanceof File, form.get('c').name, [...form.keys()]];
            form.delete('a');
            out.after = [...form.keys()];
        })();
        "#,
    )
    .await;
    assert_eq!(
        json(&js, "out.blob"),
        r#"[9,"text/plain","héllo! x","éllo!","x",9]"#
    );
    assert_eq!(
        json(&js, "out.file"),
        r#"["a.txt",42,3,true,"[object File]"]"#
    );
    assert_eq!(
        json(&js, "out.form"),
        r#"["1",["1","2"],true,"blob",true,"renamed.txt",["a","a","b","c"]]"#
    );
    assert_eq!(json(&js, "out.after"), r#"["b","c"]"#);
}

#[tokio::test]
async fn dropping_the_page_cancels_requests() {
    let server = server();
    let session = session();
    let js = page(Some(session.clone()));
    js.run_script(&format!("fetch({:?});", server.url("/slow")), "a.js")
        .unwrap();
    // Let the request get going, then drop the page.
    js.run(RunLimits::until(js.now() + Duration::from_millis(50)))
        .await;
    drop(js);
    tokio::task::yield_now().await;
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn url_search_params_bodies_are_form_encoded() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            const params = new URLSearchParams({{ q: 'a b', n: '1' }});
            fetch({url:?}, {{ method: 'POST', body: params }});
            fetch(new URL('?x=1', {url:?}));
            "#,
            url = server.url("/echo")
        ),
    )
    .await;
    let requests = server.requests();
    let post = requests.iter().find(|r| r.method == "POST").unwrap();
    assert_eq!(
        post.header("content-type"),
        Some("application/x-www-form-urlencoded;charset=UTF-8")
    );
    assert_eq!(&post.body[..], b"q=a+b&n=1");
    let get = requests.iter().find(|r| r.method == "GET").unwrap();
    assert_eq!(get.query.as_deref(), Some("x=1"));
}
