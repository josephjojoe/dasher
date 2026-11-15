//! Loading pages from the fixture server: fetching, parsing, script order,
//! lifecycle events, encodings, cookies, base URLs and navigation.

use std::time::{Duration, Instant};

use dasher_js::{ClockMode, ConsoleLevel, ReadyState, RunLimits, RunOutcome, RuntimeOptions};
use dasher_net::{Client, Session, StatusCode};
use dasher_page::{LoadError, Page, PageOptions};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

fn session() -> Session {
    Client::builder().build().unwrap().session()
}

fn options() -> PageOptions {
    PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(10)),
        ..PageOptions::default()
    }
}

async fn load(server: &FixtureServer, path: &str) -> Page {
    Page::load(&session(), &server.url(path), options())
        .await
        .unwrap()
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn bytes_page(content_type: &str, body: &[u8]) -> FixtureResponse {
    FixtureResponse::ok(content_type, body.to_vec())
}

#[tokio::test]
async fn scripts_run_in_spec_order_around_the_lifecycle_events() {
    let ms = Duration::from_millis;
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .path_latency("/slow-defer.js", ms(100))
        .path_latency("/slow-ordered.js", ms(300))
        .path_latency("/async.js", ms(500))
        .start()
        .unwrap();
    let page = load(&server, "/order.html").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.ready_state(), ReadyState::Complete);
    assert!(!page.timed_out());
    let log: Vec<String> = serde_list(&json(&page, "log"));
    assert_eq!(
        log,
        [
            "inline:loading",
            // The parser waited for the blocking script.
            "blocking true true:loading",
            "inline2 true:loading",
            "readystatechange:interactive",
            // Deferred scripts run in document order, however they load.
            "slow-defer:interactive",
            "defer:interactive",
            "DOMContentLoaded:interactive",
            // Scripts inserted with async = false run in insertion order.
            "slow-ordered:interactive",
            "onload slow-ordered.js:interactive",
            "ordered:interactive",
            "onload ordered.js:interactive",
            // The load event waits for async scripts.
            "async:interactive",
            "readystatechange:complete",
            "load:complete",
        ]
    );
    // noscript content is raw text, and data blocks, nomodule scripts and
    // template contents never run or load.
    assert_eq!(json(&page, "document.getElementById('fallback')"), "null");
    assert_eq!(server.request_count_for("/nomodule.js"), 0);
    assert_eq!(page.js().take_errors(), []);
}

/// Parses a JSON array of strings (enough for these tests).
fn serde_list(json: &str) -> Vec<String> {
    let inner = json
        .trim()
        .strip_prefix('[')
        .unwrap()
        .strip_suffix(']')
        .unwrap();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split("\",\"")
        .map(|s| s.trim_matches('"').replace("\\\"", "\""))
        .collect()
}

#[tokio::test]
async fn document_write_inserts_at_the_parser_position() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let page = load(&server, "/write.html").await;
    assert_eq!(
        json(
            &page,
            "[...document.querySelectorAll('p')].map((p) => p.id || p.textContent)"
        ),
        r#"["a","w1","w2","w3","last"]"#
    );
    // The written script blocked the parser before the rest of the write.
    assert_eq!(json(&page, "w3NotYetParsed"), "true");
    // Writes from async scripts are ignored.
    assert_eq!(json(&page, "asyncRan"), "true");
    assert_eq!(json(&page, "document.getElementById('bad')"), "null");
    assert_eq!(page.js().take_errors(), []);
}

#[tokio::test]
async fn script_load_failures_fire_error_and_parsing_goes_on() {
    let server = FixtureServer::serve(fixtures_dir!()).unwrap();
    let page = load(&server, "/errors.html").await;
    assert_eq!(
        json(&page, "events"),
        r#"["error missing.js","load throws.js","ok ran","load ok.js","error ","error http://[bad","parsed"]"#
    );
    let errors = page.js().take_errors();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message, "Error: thrown by throws.js");
    let console = page.js().take_console();
    let failures: Vec<&str> = console
        .iter()
        .filter(|m| m.level == ConsoleLevel::Error)
        .map(|m| m.text.as_str())
        .collect();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(failures[0].contains("/missing.js") && failures[0].contains("404"));
}

#[tokio::test]
async fn inline_handler_attributes_run() {
    let server = FixtureServer::builder()
        // The body's attribute replaces the handler a head script set.
        .route("/body-onload", |_| {
            bytes_page(
                "text/html",
                b"<script>window.log = []; window.onload = () => log.push('head script');</script>\
                  <body onload=\"log.push('attr', document.readyState)\">\
                  <button onclick=\"log.push('click', this.tagName)\"></button>",
            )
        })
        // A script after the body replaces the attribute's handler.
        .route("/script-after-body", |_| {
            bytes_page(
                "text/html",
                b"<script>window.log = [];</script><body onload=\"log.push('attr')\">\
                  <script>window.onload = () => log.push('body script');</script>",
            )
        })
        .start()
        .unwrap();

    let page = load(&server, "/body-onload").await;
    page.js()
        .run_script("document.querySelector('button').click()", "test.js")
        .unwrap();
    assert_eq!(
        json(&page, "log"),
        r#"["attr","complete","click","BUTTON"]"#
    );

    let page = load(&server, "/script-after-body").await;
    assert_eq!(json(&page, "log"), r#"["body script"]"#);
}

#[tokio::test]
async fn honours_the_declared_or_sniffed_charset() {
    let server = FixtureServer::builder()
        .route("/latin1-header", |_| {
            bytes_page(
                "text/html; charset=iso-8859-1",
                b"<p>caf\xe9</p><script src=/latin1-script></script>",
            )
        })
        .route("/latin1-script", |_| {
            bytes_page("text/javascript", b"window.s = 'caf\xe9 \x80';")
        })
        .route("/meta-sjis", |_| {
            bytes_page(
                "text/html",
                b"<!doctype html><meta charset=\"shift_jis\"><title>\x93\xfa\x96\x7b</title>",
            )
        })
        .route("/undeclared-latin1", |_| {
            bytes_page("text/html", b"<p>caf\xe9")
        })
        .route("/undeclared-utf8", |_| {
            bytes_page("text/html", "<p>café ☺".as_bytes())
        })
        .route("/bom", |_| {
            bytes_page(
                "text/html; charset=windows-1252",
                "\u{feff}<p>café".as_bytes(),
            )
        })
        .start()
        .unwrap();
    let text = |page: &Page| json(page, "[document.characterSet, document.body.textContent]");

    let page = load(&server, "/latin1-header").await;
    assert_eq!(page.encoding(), "windows-1252");
    assert_eq!(text(&page), r#"["windows-1252","café"]"#);
    // External scripts without a charset use the document's encoding.
    assert_eq!(json(&page, "s"), r#""café €""#);

    let page = load(&server, "/meta-sjis").await;
    assert_eq!(page.encoding(), "Shift_JIS");
    assert_eq!(json(&page, "document.title"), r#""日本""#);

    let page = load(&server, "/undeclared-latin1").await;
    assert_eq!(text(&page), r#"["windows-1252","café"]"#);
    let page = load(&server, "/undeclared-utf8").await;
    assert_eq!(text(&page), r#"["UTF-8","café ☺"]"#);
    let page = load(&server, "/bom").await;
    assert_eq!(text(&page), r#"["UTF-8","café"]"#);
}

#[tokio::test]
async fn document_cookie_shares_the_session_jar() {
    let server = FixtureServer::builder()
        .route("/cookies", |_| {
            FixtureResponse::html(
                "<script>document.cookie = 'fromjs=1; path=/'; window.seen = document.cookie;</script>\
                 <script src=/cookie-echo.js></script>",
            )
            .with_header("set-cookie", "session=abc; Path=/")
            .with_header("set-cookie", "secret=1; Path=/; HttpOnly")
        })
        .route("/cookie-echo.js", |req| {
            let cookie = req.header("cookie").unwrap_or_default().to_owned();
            FixtureResponse::ok("text/javascript", format!("window.sent = {cookie:?};"))
        })
        .start()
        .unwrap();
    let session = session();
    let page = Page::load(&session, &server.url("/cookies"), options())
        .await
        .unwrap();
    // HttpOnly cookies are sent but scripts cannot see them.
    assert_eq!(json(&page, "seen"), r#""session=abc; fromjs=1""#);
    assert_eq!(json(&page, "sent"), r#""session=abc; secret=1; fromjs=1""#);
    assert_eq!(session.cookies().len(), 3);
}

#[tokio::test]
async fn relative_urls_resolve_against_the_base_url() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/moved", |_| {
            FixtureResponse::redirect(302, "/base/page.html")
        })
        .start()
        .unwrap();
    let page = load(&server, "/moved").await;
    assert_eq!(page.url(), server.url("/base/page.html"));
    assert_eq!(
        json(&page, "lib"),
        format!("{:?}", server.url("/assets/lib.js"))
    );
    assert_eq!(
        json(&page, "href"),
        format!("{:?}", server.url("/assets/x.html"))
    );
    assert_eq!(
        json(&page, "baseURI"),
        format!("{:?}", server.url("/assets/"))
    );
}

#[tokio::test]
async fn follows_script_navigations() {
    let server = FixtureServer::builder()
        .route("/start", |_| {
            FixtureResponse::html(
                "<script>location.href = '/middle';</script><script src=/never.js></script>",
            )
        })
        .route("/middle", |_| {
            FixtureResponse::html(
                "<script>addEventListener('load', () => location.replace('/end?x=1'));</script>",
            )
        })
        .route("/end", |req| {
            let referer = req.header("referer").unwrap_or_default().to_owned();
            FixtureResponse::html(format!(
                "<script>window.ref = document.referrer; window.header = {referer:?};</script>"
            ))
        })
        .start()
        .unwrap();
    let page = load(&server, "/start").await;
    assert_eq!(page.url(), server.url("/end?x=1"));
    assert_eq!(page.ready_state(), ReadyState::Complete);
    assert_eq!(json(&page, "ref"), format!("{:?}", server.url("/middle")));
    assert_eq!(
        json(&page, "header"),
        format!("{:?}", server.url("/middle"))
    );
    // Loading /start stopped as soon as it asked to navigate.
    assert_eq!(server.request_count_for("/never.js"), 0);
}

#[tokio::test]
async fn navigations_stop_at_the_limit() {
    let server = FixtureServer::builder()
        .route("/loop", |req| {
            let n: u32 = req
                .query
                .as_deref()
                .and_then(|q| q.strip_prefix("n="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            FixtureResponse::html(format!(
                "<script>window.n = {n}; location.href = '/loop?n={}';</script>",
                n + 1
            ))
        })
        .route("/js-url", |_| {
            FixtureResponse::html(
                "<script>location.href = 'javascript:void 0'; window.stayed = true;</script>",
            )
        })
        .start()
        .unwrap();
    let page = Page::load(
        &session(),
        &server.url("/loop"),
        PageOptions {
            max_navigations: 3,
            ..options()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.url(), server.url("/loop?n=3"));
    assert_eq!(json(&page, "n"), "3");
    assert_eq!(server.request_count_for("/loop"), 4);
    // The last document finished loading, without its navigation.
    assert_eq!(page.ready_state(), ReadyState::Complete);
    assert!(page.js().navigation_request().is_none());
    assert!(
        page.js()
            .take_console()
            .iter()
            .any(|m| m.text.contains("too many navigations"))
    );

    let page = load(&server, "/js-url").await;
    assert_eq!(page.url(), server.url("/js-url"));
}

#[tokio::test]
async fn the_page_keeps_running_and_loading_scripts_after_load() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .route("/go-later", |_| {
            FixtureResponse::html(
                "<script>setTimeout(() => location.href = '/landed', 1000);</script>",
            )
        })
        .route("/landed", |_| FixtureResponse::html("<p>landed"))
        .start()
        .unwrap();
    let mut page = load(&server, "/later.html").await;
    assert_eq!(json(&page, "window.late"), "undefined");
    assert_eq!(page.run(RunLimits::default()).await, RunOutcome::Idle);
    // The timer fired at 5 s of virtual time and its script loaded.
    let late: f64 = json(&page, "late").parse().unwrap();
    assert!(late >= 5000.0, "{late}");

    let mut page = load(&server, "/go-later").await;
    assert_eq!(
        page.run(RunLimits::default()).await,
        RunOutcome::NavigationRequested
    );
    assert!(page.follow_navigation().await.unwrap());
    assert_eq!(page.url(), server.url("/landed"));
    assert_eq!(json(&page, "document.body.textContent"), r#""landed""#);
    assert!(!page.follow_navigation().await.unwrap());
}

#[tokio::test]
async fn non_html_responses() {
    let server = FixtureServer::builder()
        .route("/data", |_| FixtureResponse::json(r#"{"a": "<b>1</b>"}"#))
        .route("/gone", |_| {
            FixtureResponse::html("<h1>Not here</h1>").with_status(404)
        })
        .start()
        .unwrap();
    let page = load(&server, "/data").await;
    assert_eq!(
        page.html(),
        r#"<html><head></head><body><pre>{"a": "&lt;b&gt;1&lt;/b&gt;"}</pre></body></html>"#
    );
    assert_eq!(json(&page, "document.contentType"), r#""application/json""#);

    let page = load(&server, "/gone").await;
    assert_eq!(page.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json(&page, "document.querySelector('h1').textContent"),
        r#""Not here""#
    );
}

#[tokio::test]
async fn a_timeout_returns_the_page_as_far_as_it_got() {
    let server = FixtureServer::builder()
        .route("/stuck", |_| {
            FixtureResponse::html("<p id=before>before</p><script src=/slow.js></script><p>after")
        })
        .route("/slow.js", |_| {
            FixtureResponse::ok("text/javascript", "window.slow = 1;")
        })
        .path_latency("/slow.js", Duration::from_secs(5))
        .start()
        .unwrap();
    let started = Instant::now();
    let page = Page::load(
        &session(),
        &server.url("/stuck"),
        PageOptions {
            timeout: Some(Duration::from_millis(300)),
            ..options()
        },
    )
    .await
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(page.timed_out());
    assert_eq!(page.ready_state(), ReadyState::Loading);
    // The time limit no longer applies, so the host can still read the page.
    assert_eq!(json(&page, "document.querySelectorAll('p').length"), "1");
}

#[tokio::test]
async fn load_errors() {
    assert!(matches!(
        Page::load(&session(), "file:///etc/passwd", options()).await,
        Err(LoadError::InvalidUrl(_))
    ));
    assert!(matches!(
        Page::load(&session(), "not a url", options()).await,
        Err(LoadError::InvalidUrl(_))
    ));
    // Nothing listens on port 9 of localhost.
    assert!(matches!(
        Page::load(&session(), "http://127.0.0.1:9/", options()).await,
        Err(LoadError::Network(_))
    ));
}

#[tokio::test]
async fn parsed_custom_elements_are_upgraded_before_the_next_script() {
    let server = FixtureServer::builder()
        .route("/ce.html", |_| {
            FixtureResponse::html(
                r#"<!DOCTYPE html><title>CE</title>
<script>
  window.log = [];
  customElements.define('x-early', class extends HTMLElement {
    constructor() { super(); log.push('construct ' + this.id); }
    connectedCallback() { log.push('connected ' + this.id); }
  });
</script>
<x-early id=a></x-early>
<div><x-early id=b><x-late id=c></x-late></x-early></div>
<script>log.push('script sees ' + document.querySelectorAll('x-early').length + ' ' + (document.getElementById('b').constructor !== HTMLElement));</script>
<x-early id=d></x-early>
<script defer src="/x-late.js"></script>
"#,
            )
        })
        .route("/x-late.js", |_| {
            FixtureResponse::ok(
                "text/javascript",
                "customElements.define('x-late', class extends HTMLElement { \
                   connectedCallback() { log.push('late ' + this.id); } })",
            )
        })
        .start()
        .unwrap();
    let page = load(&server, "/ce.html").await;
    assert_eq!(
        json(&page, "log"),
        r#"["construct a","connected a","construct b","connected b","script sees 2 true","construct d","connected d","late c"]"#
    );
}

#[tokio::test]
async fn mutation_observers_see_what_the_parser_inserts() {
    // An observer registered in the head processes items as they arrive,
    // as ad, anti-flicker and "upgrade elements as they load" scripts do.
    let page_html = r#"<!DOCTYPE html><html><head><script>
        globalThis.log = [];
        new MutationObserver((records) => {
          for (const r of records) {
            for (const n of r.addedNodes) {
              if (n.nodeType === 1 && n.classList.contains('item')) {
                n.dataset.seen = 'yes';
                log.push('saw ' + n.id);
              }
            }
          }
        }).observe(document.documentElement, { childList: true, subtree: true });
      </script></head><body>
      <div class=item id=a></div><div class=item id=b></div>
      <script src=/blocking.js></script>
      <div class=item id=c></div>
    </body></html>"#;
    let server = FixtureServer::builder()
        .route("/observe", move |_| FixtureResponse::html(page_html))
        .route("/blocking.js", |_| {
            FixtureResponse::ok(
                "text/javascript",
                "log.push('script, a seen: ' + document.getElementById('a').dataset.seen);",
            )
        })
        .path_latency("/blocking.js", Duration::from_millis(50))
        .start()
        .unwrap();
    let page = load(&server, "/observe").await;
    assert_eq!(
        serde_list(&json(&page, "log")),
        ["saw a", "saw b", "script, a seen: yes", "saw c"]
    );
    assert_eq!(
        json(&page, "document.querySelectorAll('[data-seen=yes]').length"),
        "3"
    );
    assert_eq!(page.js().take_errors(), []);
}

#[tokio::test]
async fn classic_scripts_load_from_data_urls() {
    // Instagram loads its bootstrap scripts this way. `aGkoJ2InKQ==` is
    // `hi('b')`.
    let server = FixtureServer::builder()
        .route("/data", |_| {
            FixtureResponse::html(
                "<script>var log = []; function hi(x) { log.push(x + ':' + document.currentScript.src.slice(0, 5)); }</script>\
                 <script src=\"data:text/javascript,hi('a')\"></script>\
                 <script src=\"data:application/x-javascript; charset=utf-8;base64,aGkoJ2InKQ==\" async></script>\
                 <script src=\"data:text/javascript,hi(%27c%27)\" onload=\"log.push('load')\"></script>",
            )
        })
        .start()
        .unwrap();
    let page = load(&server, "/data").await;
    assert_eq!(
        serde_list(&json(&page, "log.slice().sort()")),
        ["a:data:", "b:data:", "c:data:", "load"]
    );
}
