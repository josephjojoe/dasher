//! `XMLHttpRequest` against the in-process fixture server.

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
                .with_header("x-multi", "a")
                .with_header("x-multi", "b")
                .with_header("set-cookie", "secret=1")
        })
        .route("/echo", |req| {
            FixtureResponse::text(format!(
                "{} {} {}",
                req.method,
                req.header("x-custom").unwrap_or("-"),
                String::from_utf8_lossy(&req.body)
            ))
        })
        .route("/bad-json", |_| FixtureResponse::json("{nope"))
        .route("/redirect", |_| {
            FixtureResponse::redirect(301, "/api/items")
        })
        .route("/login", |_| {
            FixtureResponse::text("ok").with_header("set-cookie", "session=abc; Path=/")
        })
        .route("/cookie", |req| {
            FixtureResponse::text(req.header("cookie").unwrap_or("none").to_owned())
        })
        .route("/feed", |_| {
            FixtureResponse::ok(
                "application/rss+xml; charset=utf-8",
                "<rss><channel><item><title>One</title></item><item><title>Two</title></item></channel></rss>",
            )
        })
        .route("/broken.xml", |_| {
            FixtureResponse::ok("text/xml", "<rss><channel></rss>")
        })
        .route("/page.html", |_| {
            FixtureResponse::html(
                "<title>Fetched</title><p id=p>hi<script>window.ran = true</script>",
            )
        })
        .route("/slow", |_| FixtureResponse::text("slow"))
        .path_latency("/slow", Duration::from_millis(300))
        .start()
        .unwrap()
}

#[tokio::test]
async fn async_get_goes_through_every_ready_state() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var log = [];
            var out = {{}};
            const xhr = new XMLHttpRequest();
            log.push('state ' + xhr.readyState);
            xhr.onreadystatechange = () => log.push('readystatechange ' + xhr.readyState);
            for (const type of ['loadstart', 'progress', 'load', 'loadend', 'error', 'abort']) {{
                xhr.addEventListener(type, (e) => log.push(type + ' ' + e.loaded + '/' + e.total + ' ' + e.lengthComputable));
            }}
            xhr.open('GET', {url:?});
            xhr.send();
            log.push('sent');
            xhr.onload = function (e) {{
                out = {{
                    status: this.status,
                    statusText: this.statusText,
                    text: this.responseText,
                    response: this.response,
                    url: this.responseURL,
                    total: this.getResponseHeader('X-TOTAL'),
                    multi: this.getResponseHeader('x-multi'),
                    cookie: this.getResponseHeader('set-cookie'),
                    all: this.getAllResponseHeaders(),
                    trusted: e.isTrusted,
                    isProgress: e instanceof ProgressEvent,
                }};
            }};
            "#,
            url = server.url("/redirect")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "log"),
        r#"["state 0","readystatechange 1","loadstart 0/0 false","sent","readystatechange 2","readystatechange 3","progress 19/19 true","readystatechange 4","load 19/19 true","loadend 19/19 true"]"#
    );
    let out = json(&js, "out");
    let expected_head = format!(
        r#"{{"status":200,"statusText":"OK","text":"{{\"items\":[\"a\",\"b\"]}}","response":"{{\"items\":[\"a\",\"b\"]}}","url":"{}","total":"2","multi":"a, b","cookie":null,"all":"content-length: 19\r\ncontent-type: application/json\r\ndate: "#,
        server.url("/api/items")
    );
    assert!(out.starts_with(&expected_head), "{out}");
    assert!(
        out.ends_with(r#"x-multi: a, b\r\nx-total: 2\r\n","trusted":true,"isProgress":true}"#),
        "{out}"
    );
}

#[tokio::test]
async fn response_types() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            function get(path, type, done) {{
                const xhr = new XMLHttpRequest();
                xhr.open('GET', {origin:?} + path);
                xhr.responseType = type;
                xhr.onload = () => done(xhr);
                xhr.send();
            }}
            get('/api/items', 'json', (xhr) => {{
                out.json = xhr.response;
                try {{ xhr.responseText; }} catch (e) {{ out.textError = e.name; }}
            }});
            get('/bad-json', 'json', (xhr) => {{ out.badJson = xhr.response; }});
            get('/api/items', 'arraybuffer', (xhr) => {{
                out.buffer = [xhr.response instanceof ArrayBuffer, xhr.response.byteLength, xhr.response === xhr.response];
            }});
            get('/api/items', 'blob', (xhr) => {{
                out.blob = [xhr.response instanceof Blob, xhr.response.type, xhr.response.size];
            }});
            get('/api/items', 'text', (xhr) => {{ out.text = xhr.response.length; }});
            get('/api/items', 'document', (xhr) => {{ out.document = xhr.response; }});
            const xhr = new XMLHttpRequest();
            xhr.responseType = 'bogus';
            out.ignored = xhr.responseType;
            "#,
            origin = server.origin()
        ),
    )
    .await;
    // The requests finish in any order, so compare with sorted keys.
    assert_eq!(
        json(
            &js,
            "Object.fromEntries(Object.entries(out).sort(([a], [b]) => (a < b ? -1 : 1)))"
        ),
        r#"{"badJson":null,"blob":[true,"application/json",19],"buffer":[true,19,true],"document":null,"ignored":"","json":{"items":["a","b"]},"text":19,"textError":"InvalidStateError"}"#
    );
}

#[tokio::test]
async fn posts_with_headers_and_upload_events() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var log = [];
            const xhr = new XMLHttpRequest();
            xhr.open('POST', {url:?});
            xhr.setRequestHeader('X-Custom', 'one');
            xhr.setRequestHeader('x-custom', 'two');
            xhr.setRequestHeader('Cookie', 'forged=1');
            for (const type of ['loadstart', 'progress', 'load', 'loadend']) {{
                xhr.upload.addEventListener(type, (e) => log.push('upload ' + type + ' ' + e.loaded));
            }}
            xhr.onload = () => log.push(xhr.responseText);
            xhr.send('q=dasher');
            "#,
            url = server.url("/echo")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "log"),
        r#"["upload loadstart 0","upload progress 8","upload load 8","upload loadend 8","POST one, two q=dasher"]"#
    );
    let request = &server.requests()[0];
    assert_eq!(
        request.header("content-type"),
        Some("text/plain;charset=UTF-8")
    );
    assert_eq!(request.header("cookie"), None);
    assert_eq!(request.header("origin"), Some("null"));
}

#[tokio::test]
async fn abort_cancels_the_request() {
    let server = server();
    let session = session();
    let js = page(Some(session.clone()));
    run(
        &js,
        &format!(
            r#"
            var log = [];
            const xhr = new XMLHttpRequest();
            xhr.open('GET', {url:?});
            for (const type of ['readystatechange', 'load', 'error', 'abort', 'loadend']) {{
                xhr.addEventListener(type, () => log.push(type + ' ' + xhr.readyState));
            }}
            xhr.send();
            setTimeout(() => {{
                xhr.abort();
                log.push('after ' + xhr.readyState + ' ' + xhr.status);
            }}, 50);
            "#,
            url = server.url("/slow")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "log"),
        r#"["readystatechange 4","abort 4","loadend 4","after 0 0"]"#
    );
    assert_eq!(session.activity().in_flight(), 0);
}

#[tokio::test]
async fn errors_and_timeouts() {
    let server = server();
    let session = session().with_policy(RequestPolicy::new().block_url("*://*/api/*"));
    let js = page(Some(session));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            function get(name, url, timeout) {{
                const xhr = new XMLHttpRequest();
                xhr.open('GET', url);
                if (timeout) xhr.timeout = timeout;
                const events = [];
                for (const type of ['load', 'error', 'timeout', 'loadend']) {{
                    xhr.addEventListener(type, () => events.push(type));
                }}
                xhr.onloadend = () => {{ out[name] = [events, xhr.status, xhr.readyState, xhr.responseText]; }};
                xhr.send();
            }}
            get('blocked', {origin:?} + '/api/items');
            get('refused', 'http://127.0.0.1:1/');
            get('timeout', {origin:?} + '/slow', 50);
            "#,
            origin = server.origin()
        ),
    )
    .await;
    assert_eq!(
        json(&js, "[out.blocked, out.refused, out.timeout]"),
        r#"[[["error","loadend"],0,4,""],[["error","loadend"],0,4,""],[["timeout","loadend"],0,4,""]]"#
    );
}

#[tokio::test]
async fn synchronous_requests_block_until_done() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var log = [];
            const xhr = new XMLHttpRequest();
            xhr.onreadystatechange = () => log.push('readystatechange ' + xhr.readyState);
            xhr.onload = () => log.push('load');
            xhr.open('GET', {url:?}, false);
            xhr.send();
            log.push([xhr.status, xhr.responseText]);
            try {{ xhr.responseType = 'json'; }} catch (e) {{ log.push(e.name); }}
            const failing = new XMLHttpRequest();
            failing.open('GET', 'http://127.0.0.1:1/', false);
            try {{ failing.send(); }} catch (e) {{ log.push([e.name, failing.readyState, failing.status]); }}
            const sync = new XMLHttpRequest();
            sync.open('GET', 'data:,inline', false);
            sync.send();
            log.push(sync.responseText);
            "#,
            url = server.url("/slow")
        ),
    )
    .await;
    assert_eq!(
        json(&js, "log"),
        r#"["readystatechange 1","readystatechange 4","load",[200,"slow"],"InvalidStateError",["NetworkError",4,0],"inline"]"#
    );
}

#[tokio::test]
async fn with_credentials_sends_cookies() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = [];
            function get(path, credentials, next) {{
                const xhr = new XMLHttpRequest();
                xhr.open('GET', {origin:?} + path);
                xhr.withCredentials = credentials;
                xhr.onload = () => {{ out.push(xhr.responseText); if (next) next(); }};
                xhr.send();
            }}
            get('/login', true, () => get('/cookie', true, () => get('/cookie', false)));
            "#,
            origin = server.origin()
        ),
    )
    .await;
    // Without credentials only same-origin requests carry cookies, and this
    // page has no origin.
    assert_eq!(json(&js, "out"), r#"["ok","session=abc","none"]"#);
}

#[tokio::test]
async fn misuse_throws_dom_exceptions() {
    let js = page(None);
    run(
        &js,
        r#"
        var out = [];
        function attempt(f) {
            try { f(); out.push('no error'); } catch (e) { out.push(e.name); }
        }
        const xhr = new XMLHttpRequest();
        attempt(() => xhr.send());
        attempt(() => xhr.setRequestHeader('a', 'b'));
        attempt(() => xhr.open('BAD METHOD', '/x'));
        attempt(() => xhr.open('TRACE', 'https://example.com/'));
        attempt(() => xhr.open('GET', 'http://[bad'));
        xhr.open('GET', 'https://example.com/');
        attempt(() => xhr.setRequestHeader('bad name', 'v'));
        attempt(() => xhr.setRequestHeader('x', 'a\nb'));
        xhr.send();
        attempt(() => xhr.send());
        attempt(() => xhr.setRequestHeader('a', 'b'));
        attempt(() => { xhr.withCredentials = true; });
        attempt(() => new XMLHttpRequestUpload());
        out.push(XMLHttpRequest.DONE, xhr.UNSENT, String(xhr));
        "#,
    )
    .await;
    assert_eq!(
        json(&js, "out"),
        r#"["InvalidStateError","InvalidStateError","SyntaxError","SecurityError","SyntaxError","SyntaxError","SyntaxError","InvalidStateError","InvalidStateError","InvalidStateError","TypeError",4,0,"[object XMLHttpRequest]"]"#
    );
}

#[tokio::test]
async fn same_origin_requests_send_cookies_without_credentials() {
    let server = server();
    let js = JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Virtual,
        network: Some(session()),
        window: dasher_js::WindowOptions {
            url: server.url("/app/"),
            ..dasher_js::WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    run(
        &js,
        r#"
        var out = [];
        function get(path, next) {
            const xhr = new XMLHttpRequest();
            xhr.open('GET', path);
            xhr.onload = () => { out.push(xhr.responseText); if (next) next(); };
            xhr.send();
        }
        get('/login', () => get('../cookie'));
        "#,
    )
    .await;
    assert_eq!(json(&js, "out"), r#"["ok","session=abc"]"#);
}

#[tokio::test]
async fn documents_parsed_from_responses() {
    let server = server();
    let js = page(Some(session()));
    run(
        &js,
        &format!(
            r#"
            var out = {{}};
            function get(path, type, done) {{
                const xhr = new XMLHttpRequest();
                xhr.open('GET', {origin:?} + path);
                xhr.responseType = type;
                xhr.onload = () => done(xhr);
                xhr.send();
            }}
            get('/feed', '', (xhr) => {{
                const doc = xhr.responseXML;
                out.feed = [doc instanceof XMLDocument, doc === xhr.responseXML, doc.contentType,
                    doc.URL === {feed:?}, doc.documentElement.tagName,
                    [...doc.getElementsByTagName('title')].map((t) => t.textContent)];
            }});
            get('/feed', 'document', (xhr) => {{ out.feedDocument = xhr.response.querySelectorAll('item').length; }});
            get('/broken.xml', '', (xhr) => {{ out.broken = xhr.responseXML; }});
            // HTML is only parsed for responseType 'document'.
            get('/page.html', '', (xhr) => {{ out.htmlXml = xhr.responseXML; }});
            get('/page.html', 'document', (xhr) => {{
                const doc = xhr.response;
                out.html = [doc instanceof HTMLDocument, doc.title, doc.getElementById('p').textContent,
                    doc.querySelector('script') !== null, window.ran === undefined, doc !== document];
            }});
            get('/api/items', '', (xhr) => {{ out.json = xhr.responseXML; }});
            "#,
            origin = server.origin(),
            feed = server.url("/feed"),
        ),
    )
    .await;
    assert_eq!(
        json(
            &js,
            "Object.fromEntries(Object.entries(out).sort(([a], [b]) => (a < b ? -1 : 1)))"
        ),
        r#"{"broken":null,"feed":[true,true,"application/rss+xml",true,"rss",["One","Two"]],"feedDocument":2,"html":[true,"Fetched","hiwindow.ran = true",true,true,true],"htmlXml":null,"json":null}"#
    );
}
