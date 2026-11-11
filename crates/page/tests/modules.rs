//! Module scripts in pages: ordering against deferred classic scripts,
//! import maps, modulepreload hints, dynamic imports and inserted module
//! scripts, from the fixtures in `fixtures/modules`.

use std::time::Duration;

use dasher_js::{ClockMode, ReadyState, RuntimeOptions};
use dasher_net::{Client, Session};
use dasher_page::{Page, PageOptions};
use dasher_test_server::{FixtureServer, fixtures_dir};

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

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

fn log(page: &Page) -> Vec<String> {
    let log = json(page, "log.join('\\n')");
    log.trim_matches('"')
        .split("\\n")
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn module_scripts_run_deferred_in_document_order() {
    let ms = Duration::from_millis;
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        // The entry module arrives after the deferred classic script that
        // follows it, and must still run first.
        .path_latency("/modules/assets/entry.js", ms(200))
        .path_latency("/modules/assets/chunk.js", ms(100))
        .path_latency("/modules/assets/async-module.js", ms(600))
        .start()
        .unwrap();
    let page = Page::load(&session(), &server.url("/modules/index.html"), options())
        .await
        .unwrap();
    assert_eq!(page.ready_state(), ReadyState::Complete);
    assert_eq!(page.js().take_errors(), []);
    assert_eq!(
        log(&page),
        [
            // Modules run their imports first; currentScript is null in
            // modules.
            "side null:interactive",
            "entry hi:interactive",
            "inline module rendered:interactive",
            "classic defer:interactive",
            "DOMContentLoaded:interactive",
            // A dynamic import does not hold up the load event, but this
            // one is quicker than the async module, which does.
            "chunk lazy /modules/assets/chunk.js:interactive",
            "async module:interactive",
            "load:complete",
        ]
    );
    assert_eq!(
        json(&page, "document.getElementById('app').textContent"),
        r#""rendered""#
    );
    // Every module was fetched once: the preloaded vendor module and the
    // entry module that the inline one imports again are reused.
    for path in ["entry.js", "vendor.js", "side.js", "chunk.js"] {
        assert_eq!(
            server.request_count_for(&format!("/modules/assets/{path}")),
            1,
            "{path}"
        );
    }
    // Browsers with modules skip nomodule scripts.
    assert_eq!(server.request_count_for("/modules/legacy.js"), 0);
}

#[tokio::test]
async fn module_scripts_inserted_by_scripts() {
    let server = FixtureServer::builder()
        .root(fixtures_dir!())
        .path_latency("/modules/assets/slow-module.js", Duration::from_millis(200))
        .start()
        .unwrap();
    let page = Page::load(&session(), &server.url("/modules/inserted.html"), options())
        .await
        .unwrap();
    let log = log(&page);
    // Those with async = false run in insertion order, firing load.
    let ordered: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|l| {
            l.contains("slow-module") || l.contains("async-module") || l.ends_with("module")
        })
        .collect();
    assert_eq!(
        ordered,
        [
            "slow module",
            "load assets/slow-module.js",
            "async module",
            "load assets/async-module.js",
        ]
    );
    // A module that cannot be fetched, or whose import cannot be, fires
    // error; inline modules fire no load event.
    for expected in ["error assets/missing.js", "error inline", "inline hi"] {
        assert!(
            log.iter().any(|l| l == expected),
            "{expected} not in {log:?}"
        );
    }
    assert!(
        !log.iter().any(|l| l == "never" || l == "load inline"),
        "{log:?}"
    );
    // The missing module was asked for once, though two scripts need it.
    assert_eq!(server.request_count_for("/modules/assets/missing.js"), 1);
}
