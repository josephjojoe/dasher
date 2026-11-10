//! The window: aliases, location, history, navigator, screen, scrolling and
//! messaging.

use std::time::Duration;

use dasher_js::{
    ClockMode, JsRuntime, NavigationKind, NavigationRequest, RunOutcome, RuntimeOptions, Viewport,
    WindowOptions,
};

fn page(url: &str) -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        clock: ClockMode::Manual,
        window: WindowOptions {
            url: url.into(),
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

fn run(js: &JsRuntime, source: &str) {
    js.run_script(source, "test.js")
        .unwrap_or_else(|e| panic!("{source}: {e}"));
}

#[test]
fn global_aliases_and_window_prototype() {
    let js = page("https://example.com/");
    let out = eval(
        &js,
        r#"
        var topLevel = 1;
        ({ aliases: [window === globalThis, self === window, frames === window, parent === window, top === window, window.window === window],
           proto: window instanceof Window && window instanceof EventTarget,
           tag: Object.prototype.toString.call(window), topLevel: window.topLevel,
           illegal: (() => { try { new Window(); } catch (e) { return e.name; } })(),
           replaced: (() => { self = 5; return self; })() })
        "#,
    );
    assert_eq!(
        out,
        r#"{"aliases":[true,true,true,true,true,true],"proto":true,"tag":"[object Window]","topLevel":1,"illegal":"TypeError","replaced":5}"#
    );
}

#[test]
fn window_is_an_event_target() {
    let js = page("https://example.com/");
    let out = eval(
        &js,
        r#"
        const log = [];
        addEventListener('custom', (e) => log.push('bare:' + (e.currentTarget === window)));
        window.addEventListener('custom', () => log.push('window'));
        window.oncustom = () => log.push('not a handler attribute');
        window.onload = (e) => log.push('onload:' + e.type);
        dispatchEvent(new Event('custom'));
        window.dispatchEvent(new Event('load'));
        log
        "#,
    );
    assert_eq!(out, r#"["bare:true","window","onload:load"]"#);
}

#[test]
fn location_reads_the_page_url() {
    let js = page("https://user@example.com:8443/a/b?q=1#top");
    let out = eval(
        &js,
        r#"
        ({ href: location.href, origin: location.origin, protocol: location.protocol, host: location.host,
           hostname: location.hostname, port: location.port, pathname: location.pathname,
           search: location.search, hash: location.hash, str: String(location), same: window.location === location,
           winOrigin: origin, secure: isSecureContext, tag: Object.prototype.toString.call(location) })
        "#,
    );
    assert_eq!(
        out,
        r##"{"href":"https://user@example.com:8443/a/b?q=1#top","origin":"https://example.com:8443","protocol":"https:","host":"example.com:8443","hostname":"example.com","port":"8443","pathname":"/a/b","search":"?q=1","hash":"#top","str":"https://user@example.com:8443/a/b?q=1#top","same":true,"winOrigin":"https://example.com:8443","secure":true,"tag":"[object Location]"}"##
    );
    assert_eq!(js.url(), "https://user@example.com:8443/a/b?q=1#top");
}

#[test]
fn assigning_location_requests_navigation() {
    let js = page("https://example.com/dir/page");
    run(&js, "location.href = 'other?x=1';");
    assert_eq!(
        js.take_navigation_request(),
        Some(NavigationRequest {
            url: "https://example.com/dir/other?x=1".into(),
            kind: NavigationKind::Push,
        })
    );
    // The document itself does not change until the host navigates.
    assert_eq!(js.url(), "https://example.com/dir/page");

    for (script, url, kind) in [
        (
            "window.location = '/root'",
            "https://example.com/root",
            NavigationKind::Push,
        ),
        (
            "location.assign('https://other.test/')",
            "https://other.test/",
            NavigationKind::Push,
        ),
        (
            "location.replace('/r')",
            "https://example.com/r",
            NavigationKind::Replace,
        ),
        (
            "location.reload()",
            "https://example.com/dir/page",
            NavigationKind::Reload,
        ),
        (
            "history.go()",
            "https://example.com/dir/page",
            NavigationKind::Reload,
        ),
        (
            "location.search = 'a=b'",
            "https://example.com/dir/page?a=b",
            NavigationKind::Push,
        ),
        (
            "location.pathname = '/p'",
            "https://example.com/p",
            NavigationKind::Push,
        ),
        (
            "location.hostname = 'example.org'",
            "https://example.org/dir/page",
            NavigationKind::Push,
        ),
    ] {
        run(&js, script);
        assert_eq!(
            js.take_navigation_request(),
            Some(NavigationRequest {
                url: url.into(),
                kind
            }),
            "{script}"
        );
    }

    let err = js.run_script("location.assign('http://[bad')", "a.js");
    assert!(err.unwrap_err().message.starts_with("SyntaxError"));
    run(&js, "location.href = 'javascript:void 0'");
    assert_eq!(js.take_navigation_request(), None);
}

#[test]
fn fragment_changes_stay_in_the_document() {
    let js = page("https://example.com/page");
    run(
        &js,
        r#"
        var log = [];
        addEventListener('hashchange', (e) => log.push([e.oldURL, e.newURL, location.hash]));
        location.hash = 'one';
        location.href = '#two';
        location.hash = '#two';
        "#,
    );
    assert_eq!(js.navigation_request(), None);
    assert_eq!(js.url(), "https://example.com/page#two");
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "[log, history.length]"),
        r##"[[["https://example.com/page","https://example.com/page#one","#two"],["https://example.com/page#one","https://example.com/page#two","#two"]],3]"##
    );
}

#[test]
fn push_state_and_traversal() {
    let js = page("https://example.com/start");
    run(
        &js,
        r#"
        var log = [];
        addEventListener('popstate', (e) => log.push(['pop', e.state, location.pathname]));
        history.pushState({ page: 1 }, '', '/one');
        history.pushState({ page: 2 }, '', 'two');
        history.replaceState({ page: 22 }, '');
        "#,
    );
    assert_eq!(js.url(), "https://example.com/two");
    assert_eq!(
        eval(&js, "[history.length, history.state, location.href]"),
        r#"[3,{"page":22},"https://example.com/two"]"#
    );

    run(&js, "history.back(); history.back();");
    js.run_ready().unwrap();
    assert_eq!(js.url(), "https://example.com/start");
    run(&js, "history.forward(); history.go(5); history.go(-9);");
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "[log, history.state]"),
        r#"[[["pop",{"page":1},"/one"],["pop",null,"/start"],["pop",{"page":1},"/one"]],{"page":1}]"#
    );

    // Pushing from the middle drops the forward entries.
    run(&js, "history.pushState(null, '', '/three');");
    assert_eq!(eval(&js, "history.length"), "3");
    assert_eq!(js.navigation_request(), None);
}

#[test]
fn push_state_clones_and_checks_the_url() {
    let js = page("https://example.com/");
    let out = eval(
        &js,
        r#"
        const state = { n: 1 };
        history.pushState(state, '');
        const cloned = history.state !== state && history.state.n === 1;
        const errs = [];
        try { history.pushState(null, '', 'https://evil.test/'); } catch (e) { errs.push(e.name); }
        try { history.pushState(() => 1, ''); } catch (e) { errs.push(e.name); }
        try { history.pushState(null); } catch (e) { errs.push(e.name); }
        ({ cloned, errs, length: history.length })
        "#,
    );
    assert_eq!(
        out,
        r#"{"cloned":true,"errs":["SecurityError","DataCloneError","TypeError"],"length":2}"#
    );
}

#[test]
fn history_traversal_fires_hashchange_for_fragment_entries() {
    let js = page("https://example.com/");
    run(
        &js,
        "var log = []; onhashchange = (e) => log.push(e.newURL); onpopstate = () => log.push('pop'); location.hash = 'a';",
    );
    js.run_ready().unwrap();
    run(&js, "history.back()");
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "log"),
        r##"["https://example.com/#a","pop","https://example.com/"]"##
    );
}

#[tokio::test]
async fn run_stops_when_the_page_navigates() {
    let js = page("https://example.com/");
    run(
        &js,
        "setTimeout(() => { location.href = '/next'; }, 10); setTimeout(() => console.log('never'), 20);",
    );
    let outcome = js.run(Default::default()).await;
    assert_eq!(outcome, RunOutcome::NavigationRequested);
    assert_eq!(
        js.navigation_request().map(|n| n.url),
        Some("https://example.com/next".into())
    );
    assert!(js.take_console().is_empty());
    assert_eq!(
        js.run(Default::default()).await,
        RunOutcome::NavigationRequested
    );
    js.take_navigation_request();
    assert_eq!(js.run(Default::default()).await, RunOutcome::Idle);
}

#[test]
fn navigator_defaults_look_like_desktop_chrome() {
    let js = page("https://example.com/");
    let out = eval(
        &js,
        r#"
        ({ ua: navigator.userAgent.includes('Chrome/'), vendor: navigator.vendor, platform: navigator.platform,
           language: navigator.language, languages: navigator.languages, frozen: Object.isFrozen(navigator.languages),
           webdriver: navigator.webdriver, cookies: navigator.cookieEnabled, online: navigator.onLine,
           cores: navigator.hardwareConcurrency, memory: navigator.deviceMemory, touch: navigator.maxTouchPoints,
           brands: navigator.userAgentData.brands.map((b) => b.brand), mobile: navigator.userAgentData.mobile,
           uadPlatform: navigator.userAgentData.platform, appVersion: navigator.appVersion.startsWith('5.0 ('),
           same: clientInformation === navigator, tag: String(navigator) })
        "#,
    );
    assert_eq!(
        out,
        r#"{"ua":true,"vendor":"Google Inc.","platform":"Linux x86_64","language":"en-US","languages":["en-US","en"],"frozen":true,"webdriver":false,"cookies":true,"online":true,"cores":8,"memory":8,"touch":0,"brands":["Not)A;Brand","Chromium","Google Chrome"],"mobile":false,"uadPlatform":"Linux","appVersion":true,"same":true,"tag":"[object Navigator]"}"#
    );
    assert_eq!(
        eval(
            &js,
            "navigator.userAgentData.getHighEntropyValues(['architecture', 'bitness']).then((v) => globalThis.hints = v); 0"
        ),
        "0"
    );
    assert_eq!(
        eval(&js, "[hints.architecture, hints.bitness, hints.platform]"),
        r#"["x86","64","Linux"]"#
    );
}

#[test]
fn navigator_follows_the_configuration() {
    let js = JsRuntime::new(RuntimeOptions {
        window: WindowOptions {
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:140.0) Gecko/20100101 Firefox/140.0".into(),
            platform: "MacIntel".into(),
            languages: vec!["de-DE".into()],
            viewport: Viewport {
                touch: true,
                ..Viewport::default()
            },
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    assert_eq!(
        eval(
            &js,
            "[navigator.vendor, navigator.platform, navigator.language, 'userAgentData' in navigator, navigator.maxTouchPoints]"
        ),
        r#"["","MacIntel","de-DE",false,5]"#
    );
}

#[test]
fn screen_and_viewport() {
    let js = JsRuntime::new(RuntimeOptions {
        window: WindowOptions {
            viewport: Viewport {
                width: 390,
                height: 844,
                device_pixel_ratio: 3.0,
                screen_width: 390,
                screen_height: 844,
                touch: true,
            },
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    })
    .unwrap();
    let out = eval(
        &js,
        r#"
        ({ inner: [innerWidth, innerHeight], outer: [outerWidth, outerHeight], dpr: devicePixelRatio,
           screen: [screen.width, screen.height, screen.availWidth, screen.availHeight, screen.colorDepth],
           orientation: screen.orientation.type, vv: [visualViewport.width, visualViewport.height, visualViewport.scale],
           bars: locationbar.visible })
        "#,
    );
    assert_eq!(
        out,
        r#"{"inner":[390,844],"outer":[390,844],"dpr":3,"screen":[390,844,390,844,24],"orientation":"portrait-primary","vv":[390,844,1],"bars":true}"#
    );
    let desktop = page("about:blank");
    assert_eq!(
        eval(
            &desktop,
            "[innerWidth, innerHeight, screen.orientation.type, devicePixelRatio]"
        ),
        r#"[1920,1080,"landscape-primary",1]"#
    );
}

#[test]
fn scrolling_moves_and_fires_scroll_once_per_task() {
    let js = page("https://example.com/");
    run(
        &js,
        r#"
        var events = 0;
        addEventListener('scroll', () => events++);
        scrollTo(0, 500); scrollBy(10, 100); scroll({ top: 50000 }); scrollBy({ top: -100 });
        "#,
    );
    js.run_ready().unwrap();
    assert_eq!(
        eval(
            &js,
            "[scrollX, scrollY, pageYOffset, visualViewport.pageTop, events]"
        ),
        "[10,49900,49900,49900,1]"
    );
    run(&js, "scrollTo(-5, -5)");
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "[scrollX, scrollY, events]"), "[0,0,2]");
}

#[test]
fn post_message_delivers_cloned_data_as_a_task() {
    let js = page("https://example.com/");
    run(
        &js,
        r#"
        var log = [];
        addEventListener('message', (e) => log.push([e.data, e.origin, e.source === window, e instanceof MessageEvent]));
        var payload = { n: 1 };
        postMessage(payload, '*');
        postMessage('same-origin', '/');
        postMessage('explicit', 'https://example.com');
        postMessage('dropped', 'https://other.test');
        payload.n = 2;
        log.push('sync');
        "#,
    );
    js.run_ready().unwrap();
    assert_eq!(
        eval(&js, "log"),
        r#"["sync",[{"n":1},"https://example.com",true,true],["same-origin","https://example.com",true,true],["explicit","https://example.com",true,true]]"#
    );
    let err = js.run_script("postMessage(1, 'not an origin')", "a.js");
    assert!(err.unwrap_err().message.starts_with("SyntaxError"));
}

#[test]
fn message_channels() {
    let js = page("https://example.com/");
    run(
        &js,
        r#"
        var log = [];
        var { port1, port2 } = new MessageChannel();
        port2.addEventListener('message', (e) => log.push('listener:' + e.data));
        port1.postMessage('queued before start');
        port1.onmessage = (e) => log.push('port1:' + e.data);
        port2.postMessage('to port1');
        setTimeout(() => port2.start(), 5);
        "#,
    );
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "log"), r#"["port1:to port1"]"#);
    js.advance(Duration::from_millis(5)).unwrap();
    assert_eq!(
        eval(&js, "log"),
        r#"["port1:to port1","listener:queued before start"]"#
    );
    run(&js, "port1.close(); port2.postMessage('after close');");
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "log.length"), "2");
}

#[test]
fn request_idle_callback() {
    let js = page("https://example.com/");
    run(
        &js,
        r#"
        var log = [];
        requestIdleCallback((d) => log.push([d.didTimeout, d.timeRemaining() > 0]));
        var cancelled = requestIdleCallback(() => log.push('cancelled'));
        cancelIdleCallback(cancelled);
        "#,
    );
    js.run_ready().unwrap();
    assert_eq!(eval(&js, "log"), "[[false,true]]");
}

#[test]
fn invalid_page_urls_are_rejected() {
    let err = JsRuntime::new(RuntimeOptions {
        window: WindowOptions {
            url: "not a url".into(),
            ..WindowOptions::default()
        },
        ..RuntimeOptions::default()
    });
    assert!(err.is_err());
}
