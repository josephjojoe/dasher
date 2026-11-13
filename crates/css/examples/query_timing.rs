//! Times DOM queries in Rust, without the JS bindings, on the same
//! 9,000-node page as `dasher-bench micro`'s DOM suite:
//!
//! ```sh
//! cargo run --release -p dasher-css --example query_timing
//! ```
//!
//! What a call costs from JS is this plus the bindings' own overhead (see
//! the `nodeType` micro-benchmark for the floor).

use std::fmt::Write;
use std::hint::black_box;
use std::time::Instant;

use dasher_css::SelectorList;
use dasher_dom::Document;

fn page() -> String {
    let mut html = String::from("<!DOCTYPE html><main>");
    for i in 0..500 {
        if i == 250 {
            html.push_str(
                "<div id=\"target\" class=\"a b c\" data-x=\"1\" style=\"color: red\" \
                 title=\"t\"><span>s</span><p id=\"para\">Hello world</p></div>",
            );
        }
        let _ = write!(
            html,
            "<section class=\"card\" id=\"c{i}\" data-x=\"{i}\"><h2>Card {i}</h2>\
             <p class=\"text\">Text of card {i}</p><ul>"
        );
        for j in 0..4 {
            let _ = write!(html, "<li><a href=\"/c/{i}/{j}\">Link {j}</a></li>");
        }
        html.push_str("</ul></section>");
    }
    html.push_str("<div id=\"last\"></div></main>");
    html
}

/// Prints ns per call of `f`: the best of 25 rounds of about 4 ms each,
/// since the best round is the one least disturbed by other work.
fn time(name: &str, mut f: impl FnMut()) {
    let mut n = 1u64;
    while {
        let start = Instant::now();
        for _ in 0..n {
            f();
        }
        start.elapsed().as_millis() < 4
    } {
        n *= 2;
    }
    let best = (0..25)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..n {
                f();
            }
            start.elapsed().as_nanos() as f64 / n as f64
        })
        .fold(f64::INFINITY, f64::min);
    println!("{name:<44} {best:>12.1} ns");
}

fn main() {
    let doc = Document::parse(&page());
    let root = doc.root();
    println!("{} nodes", doc.len());
    let target = doc.get_element_by_id(root, "target").unwrap();
    let sel = |s: &str| SelectorList::parse(s).unwrap();

    time("getElementById(last)", || {
        black_box(doc.get_element_by_id(root, black_box("last")));
    });
    time("getElementById(missing)", || {
        black_box(doc.get_element_by_id(root, black_box("missing")));
    });
    for s in [
        "#last",
        "#c250",
        ".text",
        "li",
        "main #c250 a",
        "#target span",
        "div.a > p",
        "section.card h2",
        "[data-x='3']",
    ] {
        let list = sel(s);
        time(&format!("querySelector({s})"), || {
            black_box(list.query_first(&doc, root));
        });
        time(&format!("querySelectorAll({s}).count()"), || {
            black_box(list.query_all(&doc, root).count());
        });
    }
    let div = sel("div");
    time("matches(div)", || {
        black_box(div.matches(&doc, target));
    });
    let span = sel("span");
    time("target.querySelector(span)", || {
        black_box(span.query_first(&doc, target));
    });
    time("parse(#last)", || {
        black_box(SelectorList::parse(black_box("#last")).unwrap());
    });
    time("parse(section.card h2)", || {
        black_box(SelectorList::parse(black_box("section.card h2")).unwrap());
    });
}
