//! The fixture suite and the server that serves it.
//!
//! The files are compiled into the binary, so the benchmark runs anywhere,
//! including in a container without the source tree. Data the pages fetch
//! is generated, deterministically.

use std::fmt::Write as _;
use std::io;
use std::time::Duration;

use dasher::{AutoScrollOptions, GotoOptions, SettleOptions};
use dasher_test_server::{FixtureResponse, FixtureServer, RecordedRequest};

/// One page the benchmark loads.
#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub name: &'static str,
    pub description: &'static str,
    pub path: &'static str,
    /// A selector that matches `expected` elements once the page is done:
    /// a run that does not get there fails, rather than measuring a page
    /// that broke.
    pub selector: &'static str,
    pub expected: usize,
    pub scroll: bool,
}

impl Scenario {
    pub fn goto_options(&self) -> GotoOptions {
        let options = GotoOptions::settle(SettleOptions::default());
        if self.scroll {
            options.with_auto_scroll(AutoScrollOptions {
                max_scrolls: 100,
                ..AutoScrollOptions::default()
            })
        } else {
            options
        }
    }
}

/// Rows in the large static page.
const LARGE_ROWS: usize = 5000;
/// Products the SPA's API returns.
const PRODUCTS: usize = 100;
/// Pages of the infinite feed, and posts per page.
const FEED_PAGES: usize = 20;
const POSTS_PER_PAGE: usize = 20;

pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "static",
        description: "11 KB article, no scripts",
        path: "/static.html",
        selector: "article p",
        expected: 25,
        scroll: false,
    },
    Scenario {
        name: "large",
        description: "1.6 MB listing, 5000-row table, no scripts",
        path: "/large.html",
        selector: "tr.row",
        expected: LARGE_ROWS,
        scroll: false,
    },
    Scenario {
        name: "spa",
        description: "React 18 shop: 100 products and 10 review lists from a JSON API",
        path: "/spa/",
        selector: ".product .review",
        expected: 10 * 3,
        scroll: false,
    },
    Scenario {
        name: "feed",
        description: "infinite scroll: 20 pages of 20 posts behind an IntersectionObserver",
        path: "/feed/",
        selector: "article.post",
        expected: FEED_PAGES * POSTS_PER_PAGE,
        scroll: true,
    },
];

pub fn scenario(name: &str) -> Option<&'static Scenario> {
    SCENARIOS.iter().find(|s| s.name == name)
}

/// Serves every scenario's files and APIs.
pub fn server() -> io::Result<FixtureServer> {
    let file = |content_type: &'static str, body: &'static [u8]| {
        move |_: &RecordedRequest| FixtureResponse::ok(content_type, body)
    };
    const HTML: &str = "text/html; charset=utf-8";
    const JS: &str = "text/javascript; charset=utf-8";
    let large = large_page();
    FixtureServer::builder()
        .route(
            "/static.html",
            file(HTML, include_bytes!("../fixtures/static.html")),
        )
        .route("/large.html", move |_| FixtureResponse::html(large.clone()))
        .route("/spa/", file(HTML, include_bytes!("../fixtures/spa/index.html")))
        .route(
            "/spa/react.production.min.js",
            file(JS, include_bytes!("../fixtures/spa/react.production.min.js")),
        )
        .route(
            "/spa/react-dom.production.min.js",
            file(
                JS,
                include_bytes!("../fixtures/spa/react-dom.production.min.js"),
            ),
        )
        .route("/spa/app.js", file(JS, include_bytes!("../fixtures/spa/app.js")))
        .route("/api/products", |_| FixtureResponse::json(products()))
        .route("/api/reviews", |req| {
            FixtureResponse::json(reviews(query_number(req, "product")))
        })
        .route(
            "/feed/",
            file(HTML, include_bytes!("../fixtures/feed/index.html")),
        )
        .route("/api/feed", |req| {
            FixtureResponse::json(feed_page(query_number(req, "page")))
        })
        // A little latency, so requests overlap as they do over a network.
        .latency(Duration::from_millis(2))
        .start()
}

fn query_number(req: &RecordedRequest, name: &str) -> usize {
    req.query
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// A deterministic stand-in for text: `n` words picked by a simple
/// generator from `seed`.
fn words(seed: usize, n: usize) -> String {
    const WORDS: &[&str] = &[
        "steel", "kettle", "compact", "quiet", "travel", "classic", "smart", "bamboo", "linen",
        "ceramic", "walnut", "copper", "matte", "everyday", "pocket", "studio", "outdoor", "soft",
    ];
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345) % (1 << 31);
            WORDS[x % WORDS.len()]
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The large static page: a long product table with navigation around it.
fn large_page() -> String {
    let mut html = String::with_capacity(1 << 21);
    html.push_str(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>Catalogue</title>\n<link rel=\"stylesheet\" href=\"/large.css\">\n</head>\n\
         <body>\n<header><h1>Catalogue</h1><nav>",
    );
    for i in 0..50 {
        let _ = write!(html, "<a href=\"/c/{i}\">Category {i}</a> ");
    }
    html.push_str(
        "</nav></header>\n<table id=\"catalogue\">\n<thead><tr><th>SKU</th><th>Name</th>\
         <th>Description</th><th>Price</th><th>Stock</th><th></th></tr></thead>\n<tbody>\n",
    );
    for i in 0..LARGE_ROWS {
        let _ = writeln!(
            html,
            "<tr class=\"row\" data-sku=\"SKU-{i:05}\"><td class=\"sku\">SKU-{i:05}</td>\
             <td class=\"name\"><a href=\"/p/{i}\">{}</a></td><td class=\"desc\">{}</td>\
             <td class=\"price\">${}.{:02}</td><td class=\"stock\">{}</td>\
             <td><img src=\"/thumb/{i}.jpg\" alt=\"\" width=\"48\" height=\"48\"></td></tr>",
            words(i, 3),
            words(i + 7, 10),
            10 + i % 490,
            i % 100,
            i % 37,
        );
    }
    html.push_str(
        "</tbody>\n</table>\n<footer><p>End of catalogue.</p></footer>\n</body>\n</html>\n",
    );
    html
}

fn products() -> String {
    let tags = ["kitchen", "travel", "home", "office", "outdoor", "gift"];
    let items: Vec<String> = (0..PRODUCTS)
        .map(|i| {
            format!(
                r#"{{"id":{i},"name":"{name}","description":"{desc}","price":{price}.{cents:02},"rating":{rating},"image":"/img/p{i}.jpg","tags":["{t1}","{t2}"]}}"#,
                name = words(i, 2),
                desc = words(i + 1000, 24),
                price = 5 + i * 3 % 200,
                cents = i * 7 % 100,
                rating = 1 + i % 5,
                t1 = tags[i % tags.len()],
                t2 = tags[(i / 3) % tags.len()],
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn reviews(product: usize) -> String {
    let items: Vec<String> = (0..3)
        .map(|i| {
            format!(
                r#"{{"id":"{product}-{i}","author":"{author}","rating":{rating},"text":"{text}"}}"#,
                author = words(product * 3 + i, 1),
                rating = 1 + (product + i) % 5,
                text = words(product * 31 + i, 30),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn feed_page(page: usize) -> String {
    if page >= FEED_PAGES {
        return r#"{"posts":[],"nextPage":null}"#.to_owned();
    }
    let posts: Vec<String> = (0..POSTS_PER_PAGE)
        .map(|i| {
            let id = page * POSTS_PER_PAGE + i;
            format!(
                r#"{{"id":{id},"author":"user{author}","avatar":"/avatars/{author}.png","time":"2026-11-{day:02}T12:00:00Z","likes":{likes},"body":"{body}"}}"#,
                author = id % 97,
                day = 1 + id % 28,
                likes = id * 13 % 1000,
                body = words(id, 40),
            )
        })
        .collect();
    let next = if page + 1 < FEED_PAGES {
        (page + 1).to_string()
    } else {
        "null".to_owned()
    };
    format!(r#"{{"posts":[{}],"nextPage":{next}}}"#, posts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_data_is_well_formed_json() {
        for json in [products(), reviews(3), feed_page(0), feed_page(FEED_PAGES)] {
            serde_json::from_str::<serde_json::Value>(&json).unwrap();
        }
    }

    #[test]
    fn the_large_page_is_large() {
        let page = large_page();
        assert!(page.len() > 1_000_000, "{}", page.len());
        assert_eq!(page.matches("<tr class=\"row\"").count(), LARGE_ROWS);
    }
}
