//! Real-world compatibility: whole apps built with the frameworks and
//! libraries scrapers actually meet, loaded and settled the way a scrape
//! would, with the content they render checked in the final HTML.
//!
//! The apps are in `fixtures/compat`, built from the sources in `compat/`
//! with production builds of the real libraries (see `compat/README.md`).
//! Each test also checks that the page threw nothing and logged no errors,
//! and drives the interactions a scrape might need (clicks, form input,
//! client-side navigation).

use std::path::Path;
use std::time::Duration;

use dasher_js::{ClockMode, ConsoleLevel, RuntimeOptions};
use dasher_net::{Client, Session};
use dasher_page::{Page, PageOptions, SettleOptions, SettleOutcome};
use dasher_test_server::{FixtureResponse, FixtureServer, fixtures_dir};

const MS: fn(u64) -> Duration = Duration::from_millis;

/// Serves the fixture `file` (under `fixtures/compat`) as HTML, for the
/// clean URLs a static host or an SPA fallback would answer.
fn page_file(file: &str) -> impl Fn(&dasher_test_server::RecordedRequest) -> FixtureResponse {
    let path = Path::new(fixtures_dir!()).join("compat").join(file);
    move |_| FixtureResponse::html(std::fs::read(&path).unwrap())
}

fn server() -> FixtureServer {
    FixtureServer::builder()
        .root(fixtures_dir!())
        // A little latency everywhere, more on the APIs, so the content
        // arrives well after the load event.
        .latency(MS(5))
        .path_latency("/compat/react-spa/api/products.json", MS(150))
        .path_latency("/compat/next/api/reviews.json", MS(150))
        .path_latency("/compat/vue/api/jobs.json", MS(150))
        .path_latency("/compat/vite/api/stories.json", MS(150))
        .path_latency("/compat/jquery/api/listings.json", MS(150))
        .path_latency("/compat/router/api/items/2.json", MS(150))
        .path_latency("/compat/lit/api/products.json", MS(150))
        // Clean URLs, as the static host of a Next.js export serves them.
        .route("/compat/next/about", page_file("next/about.html"))
        .route("/compat/next/legacy", page_file("next/legacy.html"))
        // History-routed paths all get the SPA's one document.
        .route("/compat/router/items/2", page_file("router/index.html"))
        .route("/compat/router/about", page_file("router/index.html"))
        // A POST endpoint, echoing what it was sent.
        .route("/compat/jquery/api/stats.json", |req| {
            FixtureResponse::json(format!(
                r#"{{"views":1234,"method":{:?},"body":{:?}}}"#,
                req.method,
                String::from_utf8_lossy(&req.body),
            ))
        })
        .start()
        .unwrap()
}

fn session() -> Session {
    Client::builder().build().unwrap().session()
}

/// Loads `path` and settles it with the default conditions, as a scrape
/// does, and checks that nothing went wrong on the way.
async fn open(server: &FixtureServer, path: &str) -> Page {
    let options = PageOptions {
        runtime: RuntimeOptions {
            clock: ClockMode::Virtual,
            ..RuntimeOptions::default()
        },
        timeout: Some(Duration::from_secs(20)),
        ..PageOptions::default()
    };
    let mut page = Page::load(&session(), &server.url(path), options)
        .await
        .unwrap();
    settle(&mut page).await;
    page
}

/// Settles the page after an interaction, and checks again for errors.
async fn settle(page: &mut Page) {
    let outcome = page.settle(&SettleOptions::default()).await.unwrap();
    assert_eq!(outcome, SettleOutcome::Settled);
    assert_clean(page);
}

fn assert_clean(page: &Page) {
    assert_eq!(page.js().take_errors(), [], "uncaught errors");
    let errors: Vec<String> = page
        .js()
        .take_console()
        .into_iter()
        .filter(|m| m.level == ConsoleLevel::Error)
        .map(|m| m.text)
        .collect();
    assert_eq!(errors, Vec::<String>::new(), "console errors");
}

fn run(page: &Page, script: &str) {
    page.js().run_script(script, "test.js").unwrap();
}

fn json(page: &Page, expr: &str) -> String {
    page.js()
        .eval_json(expr)
        .unwrap()
        .unwrap_or_else(|| "undefined".into())
}

#[track_caller]
fn assert_contains(html: &str, needles: &[&str]) {
    for needle in needles {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
}

#[track_caller]
fn assert_lacks(html: &str, needles: &[&str]) {
    for needle in needles {
        assert!(!html.contains(needle), "unexpected {needle:?} in:\n{html}");
    }
}

#[tokio::test]
async fn react_spa_renders_client_side() {
    let server = server();
    let mut page = open(&server, "/compat/react-spa/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            // React 19 hoists the <title> it renders into the head.
            "<title>Shop (3)</title>",
            r#"<li class="product" data-sku="A-100""#,
            "<h2",
            "Espresso Machine</h2>",
            r#"<span class="price">$249.99</span>"#,
            "<li>kitchen</li><li>coffee</li>",
            "Milk Jug",
            r#"<output id="matches">3 matching</output>"#,
            // The lazy component resolved inside its Suspense boundary.
            "4.8 out of 5 stars",
            // useLayoutEffect measured the header.
            r#"<header data-height="#,
        ],
    );
    assert_lacks(
        &html,
        &[r#"class="spinner""#, "Loading reviews…", "Pour-over Kettle"],
    );
    assert_eq!(json(&page, "document.title"), r#""Shop (3)""#);

    // React's delegated click handler loads the next page of results.
    run(&page, "document.querySelector('button.more').click();");
    settle(&mut page).await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            "Pour-over Kettle",
            "Digital Scale",
            r#"<div id="toast">5 products loaded</div>"#,
        ],
    );
    assert_lacks(&html, &["Load more"]);

    // A controlled input, set the way testing libraries do.
    run(
        &page,
        "const input = document.getElementById('search');
         Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, 'kettle');
         input.dispatchEvent(new Event('input', { bubbles: true }));",
    );
    settle(&mut page).await;
    assert_contains(
        &page.html(),
        &[r#"<output id="matches">1 matching</output>"#],
    );
    assert_eq!(
        server.request_count_for("/compat/react-spa/api/products.json"),
        1
    );
    assert_eq!(
        server.request_count_for("/compat/react-spa/assets/app.css"),
        0
    );
}

#[tokio::test]
async fn next_app_router_hydrates_server_html_and_navigates() {
    let server = server();
    let mut page = open(&server, "/compat/next/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            // Rendered on the server...
            r#"<h1 id="headline">Hydration in a headless browser</h1>"#,
            "Server-rendered markup arrives with the document.",
            // ...hydrated...
            r#"data-hydrated="true""#,
            // ...and filled in by a client component after hydration.
            r#"<ol id="reviews" data-pathname="/">"#,
            "<strong>Grace</strong>: Clear and useful.",
            "Worked on the first try.",
        ],
    );
    assert_lacks(&html, &["Loading reviews…"]);

    // Hydration attached React's handlers to the server's button.
    run(&page, "document.getElementById('like').click();");
    settle(&mut page).await;
    assert_eq!(
        json(&page, "document.getElementById('like').textContent"),
        r#""42 likes""#
    );

    // A <Link> navigates on the client: Next fetches the route's RSC
    // payload and swaps the page in without loading a new document.
    run(&page, "document.getElementById('about-link').click();");
    settle(&mut page).await;
    assert!(page.url().ends_with("/compat/next/about"), "{}", page.url());
    assert_contains(
        &page.html(),
        &["<title>About</title>", "Reached by client-side navigation."],
    );
    assert_eq!(json(&page, "document.title"), r#""About""#);
    let documents = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.path == "/compat/next/about")
        .count();
    assert_eq!(documents, 0, "Link fell back to a full page load");
    assert!(server.request_count_for("/compat/next/about/__next.about.__PAGE__.txt") > 0);

    // Back restores the first route, client-side too.
    run(&page, "history.back();");
    settle(&mut page).await;
    assert!(page.url().ends_with("/compat/next/"), "{}", page.url());
    assert_contains(
        &page.html(),
        &[
            r#"<h1 id="headline">"#,
            "<strong>Alan</strong>: Worked on the first try.",
        ],
    );

    // The nomodule polyfills are never fetched.
    let polyfills = json(
        &page,
        "document.querySelector('script[nomodule]').getAttribute('src')",
    );
    assert_eq!(server.request_count_for(polyfills.trim_matches('"')), 0);
}

#[tokio::test]
async fn next_pages_router_hydrates_next_data() {
    let server = server();
    let page = open(&server, "/compat/next/legacy").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            "<title data-next-head=\"\">Legacy shop</title>",
            r#"<script id="__NEXT_DATA__" type="application/json">"#,
            // The router state after hydration...
            r#"<h1 id="legacy-title">Shop (<!-- -->/legacy<!-- -->)</h1>"#,
            // ...and the data an effect fetched.
            r#"Teapot<span class="stock"> (3 in stock)</span>"#,
            r#"Kettle<span class="stock"> (0 in stock)</span>"#,
        ],
    );
    assert_eq!(server.request_count_for("/compat/next/api/stock.json"), 1);
}

#[tokio::test]
async fn vue_app_compiles_its_in_dom_template() {
    let server = server();
    let mut page = open(&server, "/compat/vue/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            "<title>Careers (4)</title>",
            r#"<div id="app" data-v-app="">"#,
            "<h1>Careers</h1>",
            r#"<p class="summary">3 open of 4 jobs</p>"#,
            r#"<li class="job" data-id="1"><h2>Rust Engineer</h2><span class="team">Platform</span><button type="button">Apply</button></li>"#,
            r#"<li class="job closed" data-id="2">"#,
            r#"<span class="closed-label">Closed</span>"#,
            r#"<option value="Design">Design</option>"#,
            r#"<small class="credit">Posted by <em>Example Corp</em></small>"#,
        ],
    );
    // Nothing of the template is left uncompiled.
    assert_lacks(&html, &["{{", "v-cloak=", "v-for", "Loading jobs…"]);
    // axios sent its query through XHR.
    assert_eq!(
        server
            .requests()
            .iter()
            .find(|r| r.path == "/compat/vue/api/jobs.json")
            .and_then(|r| r.query.clone())
            .as_deref(),
        Some("page=1")
    );

    // An @click handler and a v-model select.
    run(
        &page,
        "document.querySelectorAll('.job button')[1].click();
         const select = document.querySelector('select');
         select.value = 'Platform';
         select.dispatchEvent(new Event('change'));",
    );
    settle(&mut page).await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            r#"<p id="applied" style="">Applied to: Product Designer</p>"#,
            "Site Reliability Engineer",
        ],
    );
    assert_lacks(&html, &["Data Analyst"]);
}

#[tokio::test]
async fn vite_bundle_loads_modules_and_split_chunks() {
    let server = server();
    let page = open(&server, "/compat/vite/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            "<h1>Top stories</h1>",
            r#"<a href="https://example.com/1">A browser without pixels</a>"#,
            "312 points · Nov 1, 2026",
            "QuickJS in production",
            // The lazily imported route chunk rendered, with its data.
            r#"<section id="comments"><h2>Comments</h2><p class="comment">Great write-up.</p><p class="comment">Does it handle modules?</p></section>"#,
            // Vite's preload helper inserted the chunk's CSS and modulepreload.
            r#"<link rel="modulepreload" as="script" crossorigin="" href="/compat/vite/assets/Comments-"#,
        ],
    );
    assert_lacks(&html, &["Loading comments…"]);
    let requests = server.requests();
    let scripts: Vec<&str> = requests
        .iter()
        .map(|r| r.path.as_str())
        .filter(|p| p.ends_with(".js"))
        .collect();
    // Entry, vendor chunk and the lazy chunk, each fetched once; no CSS.
    assert_eq!(scripts.len(), 3, "{scripts:?}");
    assert!(requests.iter().all(|r| !r.path.ends_with(".css")));
}

#[tokio::test]
async fn jquery_page_loads_content_over_xhr() {
    let server = server();
    let mut page = open(&server, "/compat/jquery/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            // $.getJSON with a serialized form as its query.
            r#"<tr class="listing" data-id="101"><td class="title"><a href="/item/101">Road bike, 56cm</a></td><td class="price">$450.00</td><td class="posted">today</td></tr>"#,
            // Text escaped through $('<div>').text().html().
            "Kids' bike &lt;with stabilisers&gt;",
            r#"<tr class="listing alt" data-id="102">"#,
            // .load() of a fragment of another page.
            r#"<div id="sidebar" data-status="success"><ul id="links">"#,
            r#"<a href="/bikes">Bikes</a>"#,
            // JSONP through an inserted script.
            r#"<span id="weather">Leeds: 11°</span>"#,
            // A POST with $.ajax, then .show().
            r#"<div id="stats" style="">1234 views</div>"#,
            r##"<a href="#" class="more" data-page="2">More</a>"##,
        ],
    );
    assert_lacks(&html, &["Loading listings…", "Not this part", "Loading…"]);
    // The loaded page's scripts did not run.
    assert_eq!(json(&page, "window.sidebarScriptRan"), "undefined");
    let requests = server.requests();
    let listings = requests
        .iter()
        .find(|r| r.path == "/compat/jquery/api/listings.json")
        .unwrap();
    assert_eq!(listings.query.as_deref(), Some("q=bike&sort=new&photos=1"));
    assert_eq!(listings.header("x-requested-with"), Some("XMLHttpRequest"));
    let stats = requests
        .iter()
        .find(|r| r.path == "/compat/jquery/api/stats.json")
        .unwrap();
    assert_eq!(stats.method, "POST");
    assert_eq!(&stats.body[..], b"view=listings");

    // A delegated click handler pages through the results.
    run(&page, "document.querySelector('#pager a.more').click();");
    settle(&mut page).await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            "Mountain bike",
            r#"<div id="pager"><span class="end">No more listings</span></div>"#,
        ],
    );
    assert_eq!(json(&page, "$('tr.listing').length"), "4");
}

#[tokio::test]
async fn history_routing_renders_deep_links_and_follows_navigation() {
    let server = server();
    // A deep link: the server sends the SPA's document, and the router
    // renders the route for location.pathname after its loader's fetch.
    let mut page = open(&server, "/compat/router/items/2").await;
    assert_contains(
        &page.html(),
        &[
            r#"<section id="item" data-id="2"><h1>Cafetière</h1><p class="description">Borosilicate glass, eight cups.</p><p class="price">£18</p>"#,
            r#"<div id="layout" data-navigation="idle">"#,
        ],
    );

    // A <Link> click pushes a history entry and renders the new route.
    run(&page, "document.querySelector('#item a').click();");
    settle(&mut page).await;
    assert!(page.url().ends_with("/compat/router"), "{}", page.url());
    assert_eq!(
        json(
            &page,
            "[...document.querySelectorAll('.item-link')].map((a) => a.textContent)"
        ),
        r#"["Cafetière","Kettle","Teapot"]"#
    );
    assert_contains(
        &page.html(),
        &[r#"<a class="item-link" href="/compat/router/items/3" data-discover="true">Kettle</a>"#],
    );
    run(
        &page,
        "document.querySelectorAll('a.item-link')[2].click();",
    );
    settle(&mut page).await;
    assert!(
        page.url().ends_with("/compat/router/items/1"),
        "{}",
        page.url()
    );
    assert_contains(
        &page.html(),
        &["<h1>Teapot</h1>", "Glazed stoneware, six cups."],
    );
    assert_eq!(json(&page, "history.length"), "3");

    // Back is a popstate the router follows.
    run(&page, "history.back();");
    settle(&mut page).await;
    assert!(page.url().ends_with("/compat/router"), "{}", page.url());
    assert_contains(
        &page.html(),
        &[r#"<section id="catalogue" data-sort="name">"#],
    );

    // Search parameters, then a route that does not exist.
    run(&page, "__router.navigate('/?sort=price');");
    settle(&mut page).await;
    assert!(
        page.url().ends_with("/compat/router?sort=price"),
        "{}",
        page.url()
    );
    assert_eq!(
        json(
            &page,
            "[...document.querySelectorAll('.item-link')].map((a) => a.textContent)"
        ),
        r#"["Cafetière","Teapot","Kettle"]"#
    );
    run(&page, "__router.navigate('/nope');");
    settle(&mut page).await;
    assert_contains(&page.html(), &[r#"<h1 id="not-found">Page not found</h1>"#]);
    // Only the first document was loaded from the server.
    assert_eq!(server.request_count_for("/compat/router/items/2"), 1);
    assert_eq!(server.request_count_for("/compat/router/assets/app.js"), 1);
}

#[tokio::test]
async fn lit_components_render_into_shadow_roots() {
    let server = server();
    let mut page = open(&server, "/compat/lit/").await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            // Lit renders the app into its shadow root, which the page's
            // HTML carries as declarative shadow DOM, around the light DOM
            // it slots in.
            r#"<shop-app src="/compat/lit/api/products.json"><template shadowrootmode="open">"#,
            r#"<slot name="title"></slot>"#,
            r#"<cart-badge count="0"><template shadowrootmode="open">"#,
            // (Lit marks its bindings with comments, as it does in browsers.)
            r#"<p class="count"><!--?lit$"#,
            "3 products</p>",
            "<h1 slot=\"title\">Lit Shop</h1>",
            // Each card is a component of its own, with its own shadow tree.
            r#"<h2 part="name">"#,
            "Kettle",
            "$25.00",
            "Desk Lamp",
            "$32.50",
            "breakfast</li>",
            // The plain custom element rewrote its text.
            r#"<relative-time datetime="2026-03-14T12:00:00Z" title="2026-03-14T12:00:00.000Z">Mar 14, 2026</relative-time>"#,
            // The server-rendered banner was hydrated, not replaced.
            r#"<promo-banner><template shadowrootmode="open"><strong data-hydrated="">Spring sale</strong> <slot></slot></template>Ends Friday</promo-banner>"#,
        ],
    );
    assert_lacks(&html, &["Loading…"]);
    // Scripts see what they would in a browser: the shadow trees are not
    // in the light DOM.
    assert_eq!(
        json(
            &page,
            "[document.querySelectorAll('product-card').length, \
              document.querySelector('shop-app').shadowRoot.querySelectorAll('product-card').length, \
              customElements.get('shop-app').name.length > 0]"
        ),
        "[0,3,true]"
    );

    // Clicking a button inside a card's shadow root sends a composed event
    // that the app hears, re-rendering the badge and reflecting the card's
    // state into an attribute.
    run(
        &page,
        "const app = document.querySelector('shop-app').shadowRoot; \
         app.querySelectorAll('product-card')[1].shadowRoot.querySelector('button').click();",
    );
    settle(&mut page).await;
    let html = page.html();
    assert_contains(
        &html,
        &[
            r#"<cart-badge count="1">"#,
            "Cart: <b>",
            r#"<p class="cart">In cart: "#,
            "T-2</p>",
            r#"<product-card added=""><template shadowrootmode="open">"#,
            "Added</button>",
        ],
    );

    // Typing in the filter, inside the shadow root, narrows the list.
    run(
        &page,
        "const input = document.querySelector('shop-app').shadowRoot.querySelector('input'); \
         input.value = 'toa'; input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));",
    );
    settle(&mut page).await;
    assert_eq!(
        json(
            &page,
            "[...document.querySelector('shop-app').shadowRoot.querySelectorAll('product-card')] \
               .map((c) => c.shadowRoot.querySelector('h2').textContent)"
        ),
        r#"["Toaster"]"#
    );
}
