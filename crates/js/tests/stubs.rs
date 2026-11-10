//! Stubs for APIs pages feature-detect but scraping does not need.

use dasher_js::{ClockMode, ConsoleLevel, JsRuntime, RuntimeOptions, WindowOptions};

fn page() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: "https://example.com/".into(),
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
fn dialogs_and_window_functions() {
    let js = page();
    let out = eval(
        &js,
        r#"
        alert('hello');
        ({ confirm: confirm('sure?'), prompt: prompt('name?'), open: open('/x'), find: find('x'),
           selection: [getSelection().rangeCount, String(getSelection())],
           noops: [print(), close(), focus(), blur(), scrollTo(0, 0)].every((v) => v === undefined) })
        "#,
    );
    assert_eq!(
        out,
        r#"{"confirm":false,"prompt":null,"open":null,"find":false,"selection":[0,""],"noops":true}"#
    );
    let console = js.take_console();
    assert_eq!(console[0].level, ConsoleLevel::Info);
    assert_eq!(console[0].text, "alert: hello");
    assert_eq!(js.navigation_request(), None);
}

#[test]
fn navigator_extras() {
    let js = page();
    js.run_script(
        r#"
        var log = [];
        navigator.permissions.query({ name: 'notifications' }).then((s) => log.push(['perm', s.name, s.state]));
        navigator.geolocation.getCurrentPosition(() => log.push('position'), (e) => log.push(['geo', e.code]));
        navigator.mediaDevices.getUserMedia({ video: true }).catch((e) => log.push(['media', e.name]));
        navigator.clipboard.readText().then((t) => log.push(['clip', t]));
        navigator.storage.estimate().then((e) => log.push(['quota', e.usage]));
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    let out = eval(
        &js,
        r#"
        ({ log, beacon: navigator.sendBeacon('/b', 'x'), plugins: navigator.plugins.length,
           pdf: navigator.mimeTypes.namedItem('application/pdf').enabledPlugin.name,
           pluginNames: [...navigator.plugins].map((p) => p.name)[1], connection: navigator.connection.effectiveType,
           pluginsTag: String(navigator.plugins), sw: 'serviceWorker' in navigator })
        "#,
    );
    assert_eq!(
        out,
        r#"{"log":[["perm","notifications","prompt"],["media","NotAllowedError"],["clip",""],["quota",0],["geo",1]],"beacon":true,"plugins":5,"pdf":"PDF Viewer","pluginNames":"Chrome PDF Viewer","connection":"4g","pluginsTag":"[object PluginArray]","sw":false}"#
    );
}

#[test]
fn observers_and_channels() {
    let js = page();
    js.run_script(
        r#"
        var log = [];
        const sized = { getBoundingClientRect: () => ({ width: 300, height: 150 }) };
        const ro = new ResizeObserver((entries, observer) => {
          log.push(['resize', entries.map((e) => [e.contentRect.width, e.borderBoxSize[0].blockSize]), observer === ro]);
        });
        ro.observe(sized); ro.observe({}); ro.observe(sized);
        new PerformanceObserver(() => log.push('perf')).observe({ entryTypes: ['longtask'] });
        new BroadcastChannel('c').postMessage('nobody listens');
        const ws = new WebSocket('/socket');
        log.push(['ws', ws.url, ws.readyState]);
        ws.onerror = () => log.push('ws error');
        ws.onclose = (e) => log.push(['ws close', e.code, ws.readyState]);
        const es = new EventSource('/events');
        es.onerror = () => log.push(['es error', es.readyState]);
        const n = new Notification('hi');
        n.onerror = () => log.push('notification error');
        "#,
        "a.js",
    )
    .unwrap();
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "log"),
        r#"[["ws","wss://example.com/socket",0],["resize",[[300,150],[1920,1080]],true],"ws error",["ws close",1006,3],["es error",2],"notification error"]"#
    );
}

#[test]
fn css_crypto_subtle_and_blob_urls() {
    let js = page();
    js.run_script(
        "var rejected; crypto.subtle.digest('SHA-256', new Uint8Array(1)).catch((e) => { rejected = e.name; });",
        "a.js",
    )
    .unwrap();
    let out = eval(
        &js,
        r#"
        ({ escape: [CSS.escape('a b'), CSS.escape('1x'), CSS.escape('-'), CSS.escape('-2'), CSS.escape('é#')],
           supports: CSS.supports('display', 'grid'), rejected,
           blob: /^blob:https:\/\/example\.com\/[0-9a-f-]{36}$/.test(URL.createObjectURL({})),
           absent: ['Worker', 'SharedWorker', 'indexedDB', 'caches', 'Intl'].filter((n) => n in globalThis) })
        "#,
    );
    assert_eq!(
        out,
        r#"{"escape":["a\\ b","\\31 x","\\-","-\\32 ","é\\#"],"supports":true,"rejected":"NotSupportedError","blob":true,"absent":[]}"#
    );
}
