//! Property test of the query plans (the `plan` module): on random trees
//! after random edits, `query_all` and `query_first` must find exactly what
//! testing every descendant with `matches_in_scope` finds.

use dasher_css::SelectorList;
use dasher_dom::{Document, NodeId, QuirksMode, ShadowRootInit};

/// A small, fixed-seed generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

const IDS: &[&str] = &["a", "b", "A", "c"];
const TAGS: &[&str] = &["div", "p", "section", "x-item", "svg"];
const CLASSES: &[&str] = &["x", "y", "X"];
/// `viewBox` keeps its capital on an `svg` element and loses it on an HTML
/// one.
const ATTRS: &[&str] = &["data-k", "viewBox", "lang"];

/// Selectors that exercise each plan: ids in the subject, ids further
/// left (through each combinator), lists, single compounds, and tag and
/// attribute names that no node has, that some nodes have, and that nodes
/// only come to have through the edits (`later-tag`, `data-later`).
const SELECTORS: &[&str] = &[
    "#a",
    "#A",
    "#b",
    "#missing",
    // No element has `C`: these only match `c`, in quirks mode.
    "#C",
    "#C p",
    "#C > *, #a",
    "p#a",
    "#a.x",
    "#a, #b",
    "#b, #a, #a",
    "#a, p",
    "#a p",
    "#a > p",
    "#a *",
    "#a #b",
    "div #b",
    "#a ~ p",
    "#a + p",
    "#a + p #b",
    "#a ~ div p",
    "#missing p",
    "#a p, #b",
    ":not(#a)",
    "#a:not(p)",
    "[id=a]",
    ".x",
    ".X",
    "p",
    "P",
    "x-item",
    "X-ITEM",
    "svg",
    "*",
    "div p",
    "div > .x",
    "p.x.y",
    ":scope > *",
    ":scope #a",
    "#a :scope",
    "never-used",
    "NEVER-USED",
    "*|never-used",
    "[data-never]",
    "[data-never=v] p",
    "never-used p",
    "p never-used",
    "div > [data-never] ~ p",
    "never-used, [data-never]",
    "never-used, p",
    "[data-never], #a",
    "x-item[data-never], .x",
    ":not(never-used)",
    "p:not([data-never])",
    ":is(never-used, p)",
    ":is(never-used, [data-never])",
    "div:has(never-used)",
    "div:not(:has([data-never]))",
    "[data-k]",
    "[DATA-K]",
    "[data-k=v]",
    "[data-k] p",
    "[viewBox]",
    "[viewbox]",
    "svg[viewBox], p[viewbox]",
    "[lang]",
    "later-tag",
    "LATER-TAG",
    "div later-tag",
    "[data-later]",
    "[data-later=on], never-used",
    "later-tag, [data-later]",
];

fn random_html(rng: &mut Rng, depth: usize, out: &mut String) {
    for _ in 0..rng.below(4) {
        let tag = rng.pick(TAGS);
        out.push('<');
        out.push_str(tag);
        if rng.below(2) == 0 {
            out.push_str(&format!(" id={}", rng.pick(IDS)));
        }
        if rng.below(2) == 0 {
            out.push_str(&format!(
                " class='{} {}'",
                rng.pick(CLASSES),
                rng.pick(CLASSES)
            ));
        }
        if rng.below(3) == 0 {
            out.push_str(&format!(" {}=v", rng.pick(ATTRS)));
        }
        out.push('>');
        if depth > 0 {
            random_html(rng, depth - 1, out);
        }
        out.push_str(&format!("</{tag}>"));
    }
}

fn elements(doc: &Document, root: NodeId) -> Vec<NodeId> {
    let mut out: Vec<_> = doc
        .descendants(root)
        .filter(|&n| doc.is_element(n))
        .collect();
    if doc.is_element(root) {
        out.push(root);
    }
    out
}

#[test]
fn query_plans_agree_with_matching_every_element() {
    let lists: Vec<(&str, SelectorList)> = SELECTORS
        .iter()
        .map(|&s| (s, SelectorList::parse(s).unwrap()))
        .collect();
    let mut checked = 0;
    for seed in 1..=150u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut html = String::new();
        if rng.below(3) != 0 {
            html.push_str("<!DOCTYPE html>");
        }
        random_html(&mut rng, 4, &mut html);
        let mut doc = Document::parse(&html);
        if rng.below(4) == 0 {
            doc.set_quirks_mode(QuirksMode::Quirks);
        }
        let all = elements(&doc, doc.root());
        if all.is_empty() {
            continue;
        }

        // A detached tree, and a shadow tree, with ids of their own.
        let detached = doc.create_element("div");
        doc.set_inner_html(detached, "<p id=a class=x><p id=b>");
        let host = rng.pick(&all);
        let shadow = doc.attach_shadow(host, ShadowRootInit::open()).ok();
        if let Some(shadow) = shadow {
            let p = doc.create_element("p");
            doc.set_attribute(p, "id", "a");
            doc.append_child(shadow, p).unwrap();
        }
        // Random moves and id changes, so the index has been maintained
        // rather than just built, and now and then a tag or an attribute
        // that the parsed document did not have.
        for _ in 0..rng.below(6) {
            let (x, y) = (rng.pick(&all), rng.pick(&all));
            match rng.below(5) {
                0 => {
                    let _ = doc.append_child(x, y);
                }
                1 => doc.set_attribute(x, "id", rng.pick(IDS)),
                2 => {
                    doc.remove_attribute(x, "id");
                }
                3 => doc.set_attribute(x, "data-later", rng.pick(&["on", "off"])),
                _ => {
                    let later = doc.create_element("later-tag");
                    let _ = doc.append_child(x, later);
                }
            }
        }

        let mut roots = vec![doc.root(), detached];
        roots.extend(shadow);
        for _ in 0..3 {
            roots.push(rng.pick(&elements(&doc, doc.root())));
        }
        for &root in &roots {
            for (text, list) in &lists {
                let expected: Vec<NodeId> = doc
                    .descendants(root)
                    .filter(|&n| list.matches_in_scope(&doc, n, Some(root)))
                    .collect();
                let found: Vec<NodeId> = list.query_all(&doc, root).collect();
                assert_eq!(
                    found, expected,
                    "seed {seed}: {text:?} under {root:?} in {html}"
                );
                assert_eq!(
                    list.query_first(&doc, root),
                    expected.first().copied(),
                    "seed {seed}: first {text:?} under {root:?} in {html}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "{checked}");
}

/// A query for a tag or attribute name the document has never used finds
/// nothing, and finds the node as soon as one has the name: whether it was
/// parsed, created, set as an attribute, or is in a detached tree, a
/// template's contents or a shadow tree.
#[test]
fn a_name_matches_as_soon_as_a_node_has_it() {
    let all = |doc: &Document, root: NodeId, selector: &str| -> Vec<NodeId> {
        SelectorList::parse(selector)
            .unwrap()
            .query_all(doc, root)
            .collect()
    };
    let mut doc = Document::parse(
        "<!DOCTYPE html><body><div id=a><p>one</p></div><template><p>two</p></template>",
    );
    let root = doc.root();
    let a = doc.get_element_by_id(root, "a").unwrap();
    for selector in [
        "my-widget",
        "[data-probe]",
        "[data-probe=x]",
        "div [data-probe]",
        "my-widget p",
        "my-widget, [data-probe]",
        "[DATA-PROBE]",
    ] {
        assert!(all(&doc, root, selector).is_empty(), "{selector}");
    }
    // The names an unrelated selector or lookup mentions change nothing.
    assert!(!doc.has_attribute(a, "data-probe"));
    assert!(all(&doc, root, "[data-probe]").is_empty());

    doc.set_attribute(a, "data-probe", "x");
    assert_eq!(all(&doc, root, "[data-probe]"), [a]);
    assert_eq!(all(&doc, root, "[DATA-PROBE=x]"), [a]);
    assert_eq!(all(&doc, root, "my-widget, [data-probe]"), [a]);
    assert!(all(&doc, root, "my-widget p").is_empty());

    // Created but not yet in the document: found under its own tree only.
    let widget = doc.create_element("my-widget");
    let inner = doc.create_element("p");
    doc.append_child(widget, inner).unwrap();
    assert!(all(&doc, root, "my-widget").is_empty());
    assert_eq!(all(&doc, widget, "my-widget p, p"), [inner]);
    doc.append_child(a, widget).unwrap();
    assert_eq!(all(&doc, root, "my-widget"), [widget]);
    assert_eq!(all(&doc, root, "my-widget p"), [inner]);
    assert_eq!(all(&doc, root, "MY-WIDGET"), [widget]);

    // Removed again: the name stays known and the walk finds nothing.
    doc.remove_attribute(a, "data-probe");
    assert!(all(&doc, root, "[data-probe]").is_empty());

    // Markup parsed later, into the document and into a shadow tree.
    doc.set_inner_html(inner, "<late-tag data-late=1></late-tag>");
    assert_eq!(all(&doc, root, "late-tag[data-late]").len(), 1);
    let shadow = doc.attach_shadow(a, ShadowRootInit::open()).unwrap();
    let hidden = doc.create_element("shadow-only");
    doc.append_child(shadow, hidden).unwrap();
    assert!(all(&doc, root, "shadow-only").is_empty());
    assert_eq!(all(&doc, shadow, "shadow-only"), [hidden]);
    let deep: Vec<NodeId> = SelectorList::parse("shadow-only")
        .unwrap()
        .query_all_deep(&doc, root)
        .collect();
    assert_eq!(deep, [hidden]);
}
