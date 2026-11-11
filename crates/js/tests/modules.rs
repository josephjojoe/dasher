//! ES modules: module graphs, dynamic `import()`, `import.meta`, import
//! maps and modulepreload links, against the in-process fixture server.

use dasher_dom::Document;
use dasher_js::{
    ClockMode, JsRuntime, ModuleCredentials, ModuleGraph, ModuleGraphStatus, RunLimits, RunOutcome,
    RuntimeOptions, WindowOptions,
};
use dasher_net::{Client, Session};
use dasher_test_server::{FixtureResponse, FixtureServer};

fn session() -> Session {
    Client::builder().build().unwrap().session()
}

fn page(server: &FixtureServer) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Virtual,
        network: Some(session()),
        window: WindowOptions {
            url: server.url("/app/index.html"),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap()
}

fn js_module(source: &str) -> FixtureResponse {
    FixtureResponse::ok("text/javascript", source.to_owned())
}

fn server() -> FixtureServer {
    FixtureServer::builder()
        .route("/app/main.js", |_| {
            js_module(
                r#"
                import { greet } from "./lib/greet.js";
                import * as counter from "./lib/counter.js";
                export * from "./lib/reexport.js";
                globalThis.log.push("main " + greet("world") + " " + counter.count());
                globalThis.meta = import.meta.url;
                globalThis.resolved = import.meta.resolve("./x/y.js");
                "#,
            )
        })
        .route("/app/lib/greet.js", |_| {
            js_module(
                r#"
                import { count } from "./counter.js";
                globalThis.log.push("greet " + count());
                export function greet(name) { return "hello " + name; }
                "#,
            )
        })
        .route("/app/lib/counter.js", |_| {
            js_module(
                r#"
                globalThis.log.push("counter");
                let n = 0;
                export function count() { return ++n; }
                "#,
            )
        })
        .route("/app/lib/reexport.js", |_| js_module("export const answer = 42;"))
        .route("/app/lazy.js", |_| {
            js_module(
                r#"
                import { count } from "./lib/counter.js";
                export default "lazy " + count();
                export const where = import.meta.url;
                "#,
            )
        })
        .route("/app/importer.js", |_| {
            js_module(
                r#"
                globalThis.lazy = import("./lazy.js").then((m) => m.default);
                export async function later(name) { return (await import("./" + name)).where; }
                "#,
            )
        })
        .route("/app/tla.js", |_| {
            js_module(
                r#"
                globalThis.log.push("tla start");
                await new Promise((r) => setTimeout(r, 10));
                globalThis.log.push("tla end");
                "#,
            )
        })
        .route("/app/throws.js", |_| js_module("throw new Error('module boom');"))
        .route("/app/syntax.js", |_| js_module("export const = 1;"))
        .route("/app/broken-dep.js", |_| js_module("import './missing.js';"))
        .route("/app/missing.js", |_| FixtureResponse::not_found())
        .route("/app/moved.js", |_| FixtureResponse::redirect(302, "/elsewhere/real.js"))
        .route("/elsewhere/real.js", |_| {
            js_module("import { v } from './sibling.js'; globalThis.moved = [import.meta.url, v];")
        })
        .route("/elsewhere/sibling.js", |_| js_module("export const v = 'sibling';"))
        .route("/vendor/react.js", |_| js_module("export default 'react';"))
        .route("/app/bare.js", |_| {
            js_module("import react from 'react'; globalThis.react = react;")
        })
        .route("/app/lib/dyn.js", |_| {
            js_module(
                r#"
                export function load() { return import("./counter.js"); }
                setTimeout(() => import("./greet.js").then((m) => globalThis.fromTimer = m.greet("timer")), 0);
                "#,
            )
        })
        .route("/app/preloaded.js", |_| js_module("export const p = 'preloaded';"))
        .start()
        .unwrap()
}

/// Runs the event loop until nothing is left to do.
async fn settle(js: &JsRuntime) {
    assert_eq!(js.run(RunLimits::default()).await, RunOutcome::Idle);
}

/// Fetches the graph, runs it and lets the page settle.
async fn run_module(js: &JsRuntime, graph: ModuleGraph) {
    settle(js).await;
    assert_eq!(js.module_graph_status(graph), ModuleGraphStatus::Ready);
    assert!(js.run_module_graph(graph));
    settle(js).await;
}

fn json(js: &JsRuntime, expr: &str) -> String {
    js.eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

#[tokio::test]
async fn runs_a_module_graph_in_dependency_order() {
    let server = server();
    let js = page(&server);
    js.run_script("globalThis.log = [];", "setup.js").unwrap();
    let graph = js.fetch_module_graph(&server.url("/app/main.js"), ModuleCredentials::SameOrigin);
    assert_eq!(js.module_graph_status(graph), ModuleGraphStatus::Loading);
    run_module(&js, graph).await;
    assert_eq!(js.take_errors(), []);
    assert_eq!(
        json(&js, "log"),
        r#"["counter","greet 1","main hello world 2"]"#
    );
    assert_eq!(
        json(&js, "meta"),
        format!("{:?}", server.url("/app/main.js"))
    );
    assert_eq!(
        json(&js, "resolved"),
        format!("{:?}", server.url("/app/x/y.js"))
    );
    // Each module was fetched once, though two import the counter.
    assert_eq!(server.request_count_for("/app/lib/counter.js"), 1);
    // Running the same module again does not evaluate it again.
    let again = js.fetch_module_graph(&server.url("/app/main.js"), ModuleCredentials::SameOrigin);
    run_module(&js, again).await;
    assert_eq!(json(&js, "log.length"), "3");
    assert_eq!(server.request_count_for("/app/main.js"), 1);
}

#[tokio::test]
async fn dynamic_import_from_modules_and_classic_scripts() {
    let server = server();
    let js = page(&server);
    js.run_script("globalThis.log = [];", "setup.js").unwrap();
    let graph = js.fetch_module_graph(
        &server.url("/app/importer.js"),
        ModuleCredentials::SameOrigin,
    );
    run_module(&js, graph).await;
    js.run_script("lazy.then((v) => globalThis.lazyValue = v);", "check.js")
        .unwrap();
    settle(&js).await;
    assert_eq!(json(&js, "lazyValue"), r#""lazy 1""#);

    // From a classic script, relative to the document; already-loaded
    // modules resolve without another fetch.
    js.run_script(
        r#"import("./lazy.js").then((m) => { globalThis.again = m.default; });
           import(`./lib/${"counter"}.js`).then((m) => { globalThis.c = m.count(); });"#,
        "inline.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(json(&js, "again"), r#""lazy 1""#);
    assert_eq!(json(&js, "c"), "2");
    assert_eq!(server.request_count_for("/app/lazy.js"), 1);
    assert_eq!(js.take_errors(), []);
}

#[tokio::test]
async fn dynamic_import_resolves_against_the_calling_module() {
    let server = server();
    let js = page(&server);
    js.run_script("globalThis.log = [];", "setup.js").unwrap();
    js.run_script(
        "import('./lib/dyn.js').then((m) => m.load()).then((c) => globalThis.count = typeof c.count);",
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(js.take_errors(), []);
    assert_eq!(json(&js, "count"), r#""function""#);
    assert_eq!(server.request_count_for("/app/counter.js"), 0);
    assert_eq!(json(&js, "fromTimer"), r#""hello timer""#);
}

#[tokio::test]
async fn dynamic_import_failures_reject() {
    let server = server();
    let js = page(&server);
    js.run_script(
        r#"
        globalThis.errors = {};
        const record = (key) => (e) => { errors[key] = e.name + ": " + e.message; };
        import("./missing.js").catch(record("missing"));
        import("react").catch(record("bare"));
        import("./syntax.js").catch(record("syntax"));
        import("./throws.js").catch(record("throws"));
        import("./broken-dep.js").catch(record("dep"));
        import({ toString() { throw new RangeError("bad specifier"); } }).catch(record("toString"));
        "#,
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    let errors = json(&js, "errors");
    let missing = server.url("/app/missing.js");
    for expected in [
        format!(r#""missing":"TypeError: Failed to fetch dynamically imported module: {missing}""#),
        String::from(r#""bare":"TypeError: Failed to resolve module specifier \"react\""#),
        String::from(r#""syntax":"SyntaxError:"#),
        String::from(r#""throws":"Error: module boom""#),
        format!(
            r#""dep":"TypeError: Failed to fetch dynamically imported module: {}""#,
            server.url("/app/broken-dep.js")
        ),
        String::from(r#""toString":"RangeError: bad specifier""#),
    ] {
        assert!(errors.contains(&expected), "{expected} not in {errors}");
    }
    assert_eq!(js.take_errors(), []);
    // The failed fetch was logged.
    let console = js.take_console();
    assert!(
        console
            .iter()
            .any(|m| m.text.contains("missing.js") && m.text.contains("404")),
        "{console:?}"
    );
}

#[tokio::test]
async fn failed_graphs_do_not_run() {
    let server = server();
    let js = page(&server);
    let graph = js.fetch_module_graph(
        &server.url("/app/broken-dep.js"),
        ModuleCredentials::SameOrigin,
    );
    settle(&js).await;
    assert_eq!(js.module_graph_status(graph), ModuleGraphStatus::Failed);
    assert!(!js.run_module_graph(graph));
}

#[tokio::test]
async fn module_errors_are_reported_as_uncaught() {
    let server = server();
    let js = page(&server);
    js.run_script(
        "globalThis.seen = []; addEventListener('error', (e) => seen.push(e.message));",
        "setup.js",
    )
    .unwrap();
    let graph = js.fetch_module_graph(&server.url("/app/throws.js"), ModuleCredentials::SameOrigin);
    run_module(&js, graph).await;
    let graph = js.fetch_module_graph(&server.url("/app/syntax.js"), ModuleCredentials::SameOrigin);
    run_module(&js, graph).await;
    let errors = js.take_errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(errors[0].message, "Error: module boom");
    assert!(errors[1].message.starts_with("SyntaxError"), "{errors:?}");
    assert_eq!(json(&js, "seen.length"), "2");
}

#[tokio::test]
async fn top_level_await() {
    let server = server();
    let js = page(&server);
    js.run_script("globalThis.log = [];", "setup.js").unwrap();
    let graph = js.fetch_module_graph(&server.url("/app/tla.js"), ModuleCredentials::SameOrigin);
    settle(&js).await;
    assert!(js.run_module_graph(graph));
    assert_eq!(json(&js, "log"), r#"["tla start"]"#);
    settle(&js).await;
    assert_eq!(json(&js, "log"), r#"["tla start","tla end"]"#);
}

#[tokio::test]
async fn redirected_modules_resolve_against_their_final_url() {
    let server = server();
    let js = page(&server);
    let graph = js.fetch_module_graph(&server.url("/app/moved.js"), ModuleCredentials::SameOrigin);
    run_module(&js, graph).await;
    assert_eq!(js.take_errors(), []);
    assert_eq!(
        json(&js, "moved"),
        format!(r#"[{:?},"sibling"]"#, server.url("/elsewhere/real.js"))
    );
}

#[tokio::test]
async fn inline_modules_and_import_maps() {
    let server = server();
    let js = page(&server);
    js.register_import_map(
        r#"{ "imports": { "react": "/vendor/react.js", "app/": "./" }, "scopes": { "bad": 1 } }"#,
        &server.url("/app/index.html"),
    );
    // Scopes must be objects.
    assert_eq!(js.take_errors().len(), 1);
    js.register_import_map(
        r#"{ "imports": { "react": "/vendor/react.js", "app/": "./", "nope": 5 } }"#,
        &server.url("/app/index.html"),
    );
    assert!(js.take_console().iter().any(|m| m.text.contains("nope")));

    let graph = js.fetch_inline_module_graph(
        r#"
        import react from "react";
        import { answer } from "app/lib/reexport.js";
        globalThis.inline = [react, answer, import.meta.url];
        "#,
        &server.url("/app/index.html"),
        ModuleCredentials::SameOrigin,
    );
    run_module(&js, graph).await;
    assert_eq!(js.take_errors(), []);
    assert_eq!(
        json(&js, "inline"),
        format!(r#"["react",42,{:?}]"#, server.url("/app/index.html"))
    );
    // Modules see the map too.
    let graph = js.fetch_module_graph(&server.url("/app/bare.js"), ModuleCredentials::SameOrigin);
    run_module(&js, graph).await;
    assert_eq!(json(&js, "react"), r#""react""#);

    js.register_import_map("not json", &server.url("/app/index.html"));
    assert_eq!(js.take_errors().len(), 1);
}

#[tokio::test]
async fn module_preload_links() {
    let server = server();
    let js = page(&server);
    js.set_document(Document::parse("<head></head><body></body>"));
    js.run_script(
        r#"
        globalThis.events = [];
        for (const href of ["./preloaded.js", "./missing.js"]) {
          const link = document.createElement("link");
          link.rel = "modulepreload";
          link.href = href;
          link.onload = () => events.push("load " + href);
          link.onerror = () => events.push("error " + href);
          document.head.append(link);
        }
        "#,
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(
        json(&js, "events.sort()"),
        r#"["error ./missing.js","load ./preloaded.js"]"#
    );
    js.run_script(
        "import('./preloaded.js').then((m) => globalThis.p = m.p);",
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(json(&js, "p"), r#""preloaded""#);
    assert_eq!(server.request_count_for("/app/preloaded.js"), 1);
    assert!(
        json(
            &js,
            "document.createElement('link').relList.supports('modulepreload')"
        ) == "true"
    );
}

#[tokio::test]
async fn imports_the_scanner_cannot_see_still_load() {
    let server = server();
    let js = page(&server);
    // `import` inside eval code is QuickJS's own; the loader fetches the
    // module there and then.
    js.run_script(
        r#"eval('imp' + 'ort("./lib/reexport.js")').then((m) => globalThis.answer = m.answer);"#,
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(json(&js, "answer"), "42");
    assert_eq!(js.take_errors(), []);
}

#[tokio::test]
async fn data_url_modules() {
    let server = server();
    let js = page(&server);
    js.run_script(
        r#"import("data:text/javascript,export default 6 * 7").then((m) => globalThis.v = m.default);"#,
        "test.js",
    )
    .unwrap();
    settle(&js).await;
    assert_eq!(json(&js, "v"), "42");
}

#[tokio::test]
async fn the_import_function_stays_out_of_sight() {
    let server = server();
    let js = page(&server);
    assert_eq!(
        json(
            &js,
            "Object.keys(globalThis).some((k) => k.includes('imp'))"
        ),
        "false"
    );
}
