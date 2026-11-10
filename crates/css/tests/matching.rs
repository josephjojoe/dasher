//! Selector matching against parsed documents, through the public API.

use dasher_css::{SelectorList, closest, matches, query_selector, query_selector_all};
use dasher_dom::{Document, NodeId};

/// A label for each match: its `id`, or else its tag name.
fn labels(doc: &Document, nodes: impl IntoIterator<Item = NodeId>) -> Vec<String> {
    nodes
        .into_iter()
        .map(|n| match doc.element_id(n) {
            Some(id) => id.to_owned(),
            None => doc
                .local_name(n)
                .map_or("?", |l| doc.name_str(l))
                .to_owned(),
        })
        .collect()
}

/// `document.querySelectorAll(selectors)`, labelled.
fn select(doc: &Document, selectors: &str) -> Vec<String> {
    select_in(doc, doc.root(), selectors)
}

fn select_in(doc: &Document, root: NodeId, selectors: &str) -> Vec<String> {
    match query_selector_all(doc, root, selectors) {
        Ok(nodes) => labels(doc, nodes),
        Err(e) => panic!("{e}"),
    }
}

fn by_id(doc: &Document, id: &str) -> NodeId {
    doc.get_element_by_id(doc.root(), id)
        .unwrap_or_else(|| panic!("no #{id}"))
}

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><title>t</title></head>
<body>
  <div id="main" class="content Box">
    <h1 id="title" data-role="heading">Title</h1>
    <ul id="list">
      <li id="li1" class="item"><a id="a1" href="/one.pdf">one</a></li>
      <li id="li2" class="item special"><a id="a2" href="https://x.test/two">two</a></li>
      <li id="li3" class="item"><span id="s3">three</span></li>
      <li id="li4" class="item"></li>
    </ul>
    <p id="p1" title="hello world">para <em id="em1">one</em></p>
    <p id="p2" lang="fr-CA" dir="rtl"><span id="s2"></span></p>
  </div>
  <svg id="svg" viewBox="0 0 1 1"><foreignObject id="fo"></foreignObject></svg>
</body></html>"#;

#[test]
fn type_id_class_and_universal() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "li"), ["li1", "li2", "li3", "li4"]);
    assert_eq!(select(&doc, "LI"), ["li1", "li2", "li3", "li4"]);
    assert_eq!(select(&doc, "#li2"), ["li2"]);
    assert_eq!(select(&doc, "li#li2.item.special"), ["li2"]);
    assert_eq!(select(&doc, ".special"), ["li2"]);
    assert_eq!(select(&doc, ".box"), Vec::<String>::new());
    assert_eq!(select(&doc, ".Box"), ["main"]);
    assert_eq!(select(&doc, "#LI2"), Vec::<String>::new());
    assert_eq!(select(&doc, "ul > *"), ["li1", "li2", "li3", "li4"]);
    assert_eq!(select(&doc, "*").len(), 20);
    assert_eq!(select(&doc, "*|li").len(), 4);
    assert_eq!(select(&doc, "|li").len(), 0);
}

#[test]
fn foreign_elements_match_names_exactly() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "foreignObject"), ["fo"]);
    assert_eq!(select(&doc, "foreignobject"), Vec::<String>::new());
    assert_eq!(select(&doc, "svg"), ["svg"]);
    assert_eq!(select(&doc, "SVG"), Vec::<String>::new());
    assert_eq!(select(&doc, "[viewBox]"), ["svg"]);
    assert_eq!(select(&doc, "[viewbox]"), Vec::<String>::new());
}

#[test]
fn quirks_mode_ignores_case_in_ids_and_classes() {
    let doc = Document::parse("<div id=Foo class=Bar></div>");
    assert_eq!(select(&doc, "#foo"), ["Foo"]);
    assert_eq!(select(&doc, ".bar"), ["Foo"]);
    let doc = Document::parse("<!DOCTYPE html><div id=Foo class=Bar></div>");
    assert!(select(&doc, "#foo").is_empty());
    assert!(select(&doc, ".bar").is_empty());
}

#[test]
fn attribute_selectors() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "[href]"), ["a1", "a2"]);
    assert_eq!(select(&doc, "a[href='/one.pdf']"), ["a1"]);
    assert_eq!(select(&doc, "a[href$=\".pdf\"]"), ["a1"]);
    assert_eq!(select(&doc, "a[href^=https]"), ["a2"]);
    assert_eq!(select(&doc, "a[href*='x.test']"), ["a2"]);
    assert_eq!(select(&doc, "[title~=world]"), ["p1"]);
    assert_eq!(select(&doc, "[title~=wor]"), Vec::<String>::new());
    assert_eq!(select(&doc, "[lang|=fr]"), ["p2"]);
    assert_eq!(select(&doc, "[lang|=en]"), ["html"]);
    assert_eq!(select(&doc, "[data-role=heading]"), ["title"]);
    assert_eq!(select(&doc, "[DATA-ROLE=heading]"), ["title"]);
    assert_eq!(select(&doc, "[data-role=HEADING]"), Vec::<String>::new());
    assert_eq!(select(&doc, "[data-role=HEADING i]"), ["title"]);
    assert_eq!(select(&doc, "[title*='WORLD' i]"), ["p1"]);
    assert_eq!(select(&doc, "[*|data-role]"), ["title"]);
}

#[test]
fn legacy_attributes_match_case_insensitively_on_html_elements() {
    let doc = Document::parse("<input id=i type=TEXT><input id=j data-type=TEXT>");
    assert_eq!(select(&doc, "[type=text]"), ["i"]);
    assert_eq!(select(&doc, "[type=text s]"), Vec::<String>::new());
    assert_eq!(select(&doc, "[data-type=text]"), Vec::<String>::new());
}

#[test]
fn namespaced_attributes_need_a_namespace_wildcard() {
    let doc = Document::parse("<svg><a id=l xlink:href='#x'></a></svg>");
    assert!(select(&doc, "[href]").is_empty());
    assert_eq!(select(&doc, "[*|href]"), ["l"]);
    assert_eq!(select(&doc, "[*|href='#x']"), ["l"]);
}

#[test]
fn combinators() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "ul a"), ["a1", "a2"]);
    assert_eq!(select(&doc, "ul > a"), Vec::<String>::new());
    assert_eq!(select(&doc, "li > a"), ["a1", "a2"]);
    assert_eq!(select(&doc, "#li1 + li"), ["li2"]);
    assert_eq!(select(&doc, "#li2 ~ li"), ["li3", "li4"]);
    assert_eq!(select(&doc, "h1 + ul > li:last-child"), ["li4"]);
    assert_eq!(select(&doc, "div > ul li > span"), ["s3"]);
    assert_eq!(select(&doc, "body > div p em"), ["em1"]);
    assert_eq!(select(&doc, "html  >  body   div#main>p"), ["p1", "p2"]);
    // Backtracking: the nearest `li` ancestor isn't a child of `ul.none`,
    // but the selector must still try further ancestors.
    let doc =
        Document::parse("<div class=a><section><div class=b><p id=x></p></div></section></div>");
    assert_eq!(select(&doc, ".a > section .b p"), ["x"]);
    assert_eq!(select(&doc, "div > div p"), Vec::<String>::new());
    assert_eq!(select(&doc, "div div p"), ["x"]);
    assert_eq!(select(&doc, "section > div > p"), ["x"]);
}

#[test]
fn sibling_combinators_backtrack() {
    let doc = Document::parse(
        "<div><a class=x></a><b></b><a></a><c id=t></c></div><div><a class=x></a><c id=u></c></div>",
    );
    assert_eq!(select(&doc, "a.x ~ a ~ c"), ["t"]);
    assert_eq!(select(&doc, "a.x + c"), ["u"]);
    assert_eq!(select(&doc, "a.x ~ c"), ["t", "u"]);
    assert_eq!(select(&doc, "div:first-child a.x ~ c"), ["t"]);
}

#[test]
fn selector_lists_return_tree_order_without_duplicates() {
    let doc = Document::parse(PAGE);
    assert_eq!(
        select(&doc, "p, h1, #p1, .item:first-child"),
        ["title", "li1", "p1", "p2"]
    );
}

#[test]
fn deep_trees_do_not_backtrack_exponentially() {
    let html = "<div>".repeat(2000);
    let doc = Document::parse(&html);
    // Nothing matches, and every `div` ancestor is a candidate for every
    // descendant combinator: naive backtracking would never finish.
    let sel = "div div div div div div div div div div div div span";
    assert!(select(&doc, sel).is_empty());
    let sel = "div ~ div div div div div div div div div div div div";
    assert!(select(&doc, sel).is_empty());
    assert_eq!(select(&doc, "div > div > div").len(), 1998);
}

#[test]
fn query_roots_and_scope() {
    let doc = Document::parse(PAGE);
    let main = by_id(&doc, "main");
    let list = by_id(&doc, "list");
    // Combinators may reach outside the root; matches are inside it.
    assert_eq!(select_in(&doc, list, "body a"), ["a1", "a2"]);
    assert_eq!(select_in(&doc, list, "div"), Vec::<String>::new());
    assert_eq!(select_in(&doc, main, ":scope > p"), ["p1", "p2"]);
    assert_eq!(select_in(&doc, list, ":scope > li > a"), ["a1", "a2"]);
    assert_eq!(select_in(&doc, list, ":scope"), Vec::<String>::new());
    // On the document, :scope is the root element.
    assert_eq!(select(&doc, ":scope > body > svg"), ["svg"]);
    assert_eq!(select(&doc, ":scope").len(), 1);
    assert_eq!(select(&doc, ":root"), select(&doc, "html"));

    assert_eq!(
        query_selector(&doc, doc.root(), "li.item").unwrap(),
        Some(by_id(&doc, "li1"))
    );
    assert_eq!(query_selector(&doc, list, "p").unwrap(), None);
}

#[test]
fn matches_and_closest() {
    let doc = Document::parse(PAGE);
    let a2 = by_id(&doc, "a2");
    assert!(matches(&doc, a2, "li.special > a").unwrap());
    assert!(matches(&doc, a2, "p, a[href]").unwrap());
    assert!(!matches(&doc, a2, "p").unwrap());
    assert!(matches(&doc, a2, ":scope").unwrap());
    assert!(!matches(&doc, doc.root(), "*").unwrap());

    let li2 = by_id(&doc, "li2");
    assert_eq!(closest(&doc, a2, "li").unwrap(), Some(li2));
    assert_eq!(closest(&doc, a2, "a").unwrap(), Some(a2));
    assert_eq!(
        closest(&doc, a2, "div#main").unwrap(),
        Some(by_id(&doc, "main"))
    );
    assert_eq!(closest(&doc, a2, "table").unwrap(), None);
    // :scope is the element closest() was called on.
    assert_eq!(closest(&doc, a2, ":scope").unwrap(), Some(a2));
    assert_eq!(closest(&doc, a2, "li:not(:scope)").unwrap(), Some(li2));

    // A parsed list can be reused.
    let list = SelectorList::parse("li:has(a)").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        labels(&doc, list.query_all(&doc, doc.root())),
        ["li1", "li2"]
    );
    assert_eq!(list.closest(&doc, a2), Some(li2));
    assert_eq!(list.query_first(&doc, doc.root()), Some(by_id(&doc, "li1")));
}

#[test]
fn invalid_selectors_are_syntax_errors() {
    let doc = Document::parse(PAGE);
    for sel in ["", "div[", "a >", "#1", ":nope", "::nope", ":not()", "ns|a"] {
        let err = query_selector_all(&doc, doc.root(), sel).unwrap_err();
        assert_eq!(err.name(), "SyntaxError", "{sel:?}");
        assert_eq!(err.selector(), sel);
        assert!(matches(&doc, by_id(&doc, "p1"), sel).is_err());
        assert!(closest(&doc, by_id(&doc, "p1"), sel).is_err());
    }
}

#[test]
fn logical_pseudo_classes() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "li:not(.special)"), ["li1", "li3", "li4"]);
    assert_eq!(
        select(&doc, "li:not(.special, :first-child)"),
        ["li3", "li4"]
    );
    assert_eq!(select(&doc, "li:not(ul > :nth-child(-n+3))"), ["li4"]);
    assert_eq!(select(&doc, ":is(h1, p) > :is(em, span)"), ["em1", "s2"]);
    assert_eq!(select(&doc, ":where(#li1, #li3)"), ["li1", "li3"]);
    // Forgiving: the invalid item is dropped, the rest still matches.
    assert_eq!(select(&doc, ":is(#li1, ::-nope, !!)"), ["li1"]);
    assert!(select(&doc, ":is()").is_empty());
    assert!(select(&doc, "li:is(:hover)").is_empty());
}

#[test]
fn has() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "li:has(a)"), ["li1", "li2"]);
    assert_eq!(select(&doc, "li:has(a[href$='.pdf'])"), ["li1"]);
    assert_eq!(select(&doc, "li:has(> span)"), ["li3"]);
    assert_eq!(select(&doc, "div:has(> span)"), Vec::<String>::new());
    assert_eq!(select(&doc, "div:has(span)"), ["main"]);
    assert_eq!(select(&doc, "li:has(+ .special)"), ["li1"]);
    assert_eq!(select(&doc, "li:has(~ li > span)"), ["li1", "li2"]);
    assert_eq!(select(&doc, "h1:has(~ ul a, + p)"), ["title"]);
    assert_eq!(select(&doc, "h1:has(~ ul .nope)"), Vec::<String>::new());
    assert_eq!(select(&doc, "li:not(:has(*))"), ["li4"]);
    assert_eq!(select(&doc, "ul:has(> li:nth-child(4))"), ["list"]);
    // The relative selector is anchored to the :has() subject, not to any
    // ancestor that happens to match.
    assert_eq!(select(&doc, "li:has(> a) span"), Vec::<String>::new());
    assert_eq!(select(&doc, ":has(> ul > li > a) > h1"), ["title"]);
    // :scope inside :has() still means the query's scope.
    let main = by_id(&doc, "main");
    assert_eq!(select_in(&doc, main, "ul:has(~ :scope p)").len(), 0);
    assert_eq!(select_in(&doc, main, ":has(> :scope) p"), ["p1", "p2"]);
}

#[test]
fn structural_pseudo_classes() {
    let doc = Document::parse(
        "<!DOCTYPE html><ol>\
         <li id=1></li><li id=2 class=x></li><li id=3></li><li id=4 class=x></li>\
         <li id=5></li><li id=6 class=x></li><li id=7></li></ol>",
    );
    assert_eq!(select(&doc, "li:nth-child(odd)"), ["1", "3", "5", "7"]);
    assert_eq!(select(&doc, "li:nth-child(even)"), ["2", "4", "6"]);
    assert_eq!(select(&doc, "li:nth-child(3n)"), ["3", "6"]);
    assert_eq!(select(&doc, "li:nth-child(3n+1)"), ["1", "4", "7"]);
    assert_eq!(select(&doc, "li:nth-child(-n+3)"), ["1", "2", "3"]);
    assert_eq!(select(&doc, "li:nth-child(n+6)"), ["6", "7"]);
    assert_eq!(select(&doc, "li:nth-child(2)"), ["2"]);
    assert_eq!(select(&doc, "li:nth-child(0)"), Vec::<String>::new());
    assert_eq!(select(&doc, "li:nth-child(-2n+0)"), Vec::<String>::new());
    assert_eq!(select(&doc, "li:nth-last-child(2)"), ["6"]);
    assert_eq!(select(&doc, "li:nth-last-child(-n+2)"), ["6", "7"]);
    assert_eq!(select(&doc, "li:nth-child(2 of .x)"), ["4"]);
    assert_eq!(select(&doc, "li:nth-child(odd of .x)"), ["2", "6"]);
    assert_eq!(select(&doc, "li:nth-last-child(1 of :not(.x))"), ["7"]);
    assert_eq!(select(&doc, "li:first-child, li:last-child"), ["1", "7"]);
    assert_eq!(select(&doc, "li:only-child"), Vec::<String>::new());

    let doc = Document::parse(
        "<div id=d><h2 id=h></h2><p id=p1></p><span id=s></span><p id=p2></p><p id=p3></p></div>\
         <div id=e><p id=only></p></div>",
    );
    assert_eq!(select(&doc, "p:first-of-type"), ["p1", "only"]);
    assert_eq!(select(&doc, "p:last-of-type"), ["p3", "only"]);
    assert_eq!(select(&doc, "p:nth-of-type(2)"), ["p2"]);
    assert_eq!(select(&doc, "p:nth-last-of-type(2)"), ["p2"]);
    assert_eq!(select(&doc, "#d > :only-of-type"), ["h", "s"]);
    assert_eq!(select(&doc, "body :only-child"), ["only"]);
    // Text nodes don't count as siblings.
    let doc = Document::parse("<div>text <b id=b></b> text</div>");
    assert_eq!(select(&doc, "b:only-child"), ["b"]);
}

#[test]
fn empty_and_root() {
    let doc = Document::parse(
        "<div id=a></div><div id=b><!-- c --></div><div id=c> </div><div id=d><i></i></div>",
    );
    assert_eq!(select(&doc, "div:empty"), ["a", "b"]);
    assert_eq!(select(&doc, ":root"), ["html"]);
}

#[test]
fn pseudo_elements_and_dynamic_state_never_match() {
    let doc = Document::parse(PAGE);
    for sel in [
        "p::before",
        "p:after",
        "a:hover",
        "a:focus",
        "a:visited",
        "input:focus-within",
        "p::first-line:hover",
        ":target",
        "*::-webkit-scrollbar",
        "li:has(a:active)",
    ] {
        assert!(select(&doc, sel).is_empty(), "{sel:?}");
    }
    assert_eq!(select(&doc, "a:hover, h1"), ["title"]);
    assert_eq!(select(&doc, "a:not(:hover)"), ["a1", "a2"]);
}

#[test]
fn template_contents_are_not_searched() {
    let doc = Document::parse("<template id=t><p id=inner></p></template><p id=outer></p>");
    assert_eq!(select(&doc, "p"), ["outer"]);
    let contents = doc.template_contents(by_id(&doc, "t")).unwrap();
    assert_eq!(select_in(&doc, contents, "p"), ["inner"]);
    // A fragment is not an element, so :scope matches nothing in it.
    assert!(select_in(&doc, contents, ":scope > p").is_empty());
    assert!(select_in(&doc, contents, ":root").is_empty());
}

#[test]
fn detached_elements() {
    let mut doc = Document::new();
    let div = doc.create_element("div");
    let span = doc.create_element("span");
    doc.append_child(div, span).unwrap();
    assert!(matches(&doc, div, "div:first-child:only-child").unwrap());
    assert!(!matches(&doc, div, ":root").unwrap());
    assert!(matches(&doc, span, "div > span").unwrap());
    assert_eq!(query_selector(&doc, div, "span").unwrap(), Some(span));
}

#[test]
fn links() {
    let doc = Document::parse(
        "<a id=a href=x></a><a id=b></a><area id=c href=y><link id=d href=z>\
         <svg><a id=e href=q></a></svg>",
    );
    assert_eq!(select(&doc, ":any-link"), ["a", "c", "e"]);
    assert_eq!(select(&doc, "a:link"), ["a", "e"]);
}

#[test]
fn form_state() {
    let doc = Document::parse(
        r#"<!DOCTYPE html><form>
        <input id=cb type=checkbox checked>
        <input id=cb2 type=CHECKBOX>
        <input id=r1 type=radio name=g><input id=r2 type=radio name=g>
        <input id=r3 type=radio name=h checked><input id=r4 type=radio name=h>
        <input id=txt required placeholder=Name>
        <input id=filled placeholder=Name value=Bob readonly>
        <input id=hid type=hidden required>
        <textarea id=ta placeholder=Notes></textarea>
        <select id=sel><option id=o1>a</option><option id=o2>b</option></select>
        <select id=sel2><option id=o3>a</option><optgroup><option id=o4 selected>b</option></optgroup></select>
        <select id=multi multiple><option id=o5>a</option><option id=o6 selected>b</option></select>
        <fieldset id=fs disabled>
          <legend id=lg><input id=in-legend></legend>
          <input id=in-fs>
        </fieldset>
        <optgroup id=og disabled><option id=o7></option></optgroup>
        <button id=btn type=button disabled>x</button>
        <button id=submit1>go</button><input id=submit2 type=submit>
        <progress id=prog></progress><progress id=prog2 value=1></progress>
        <div id=ce contenteditable><span id=ce-child></span><b id=ce-off contenteditable=false></b></div>
        </form>"#,
    );
    assert_eq!(select(&doc, ":checked"), ["cb", "r3", "o1", "o4", "o6"]);
    assert_eq!(select(&doc, "option:checked"), ["o1", "o4", "o6"]);
    assert_eq!(
        select(&doc, ":disabled"),
        ["fs", "in-fs", "og", "o7", "btn"]
    );
    assert!(select(&doc, ":enabled").contains(&"in-legend".to_owned()));
    assert!(!select(&doc, ":enabled").contains(&"in-fs".to_owned()));
    assert!(select(&doc, "div:enabled").is_empty());
    assert_eq!(select(&doc, ":required"), ["txt"]);
    assert!(select(&doc, ":optional").contains(&"filled".to_owned()));
    assert!(!select(&doc, ":optional").contains(&"hid".to_owned()));
    assert_eq!(
        select(&doc, ":read-write"),
        ["txt", "ta", "in-legend", "ce", "ce-child"]
    );
    assert!(select(&doc, ":read-only").contains(&"filled".to_owned()));
    assert!(select(&doc, ":read-only").contains(&"in-fs".to_owned()));
    assert_eq!(select(&doc, ":placeholder-shown"), ["txt", "ta"]);
    assert_eq!(
        select(&doc, ":default"),
        ["cb", "r3", "o4", "o6", "submit1"]
    );
    assert_eq!(select(&doc, ":indeterminate"), ["r1", "r2", "prog"]);
}

#[test]
fn lang_and_dir() {
    let doc = Document::parse(PAGE);
    assert_eq!(select(&doc, "p:lang(en)"), ["p1"]);
    assert_eq!(select(&doc, "p:lang(fr)"), ["p2"]);
    assert_eq!(select(&doc, "span:lang('fr-ca')"), ["s2"]);
    assert_eq!(select(&doc, "p:lang(de, fr)"), ["p2"]);
    assert_eq!(select(&doc, "p:lang(f)"), Vec::<String>::new());
    assert_eq!(select(&doc, "span:dir(rtl)"), ["s2"]);
    assert_eq!(select(&doc, "span:dir(ltr)"), ["s3"]);
    assert!(select(&doc, ":dir(sideways)").is_empty());
}

#[test]
fn open_and_defined() {
    let doc = Document::parse(
        "<details id=d open></details><details id=e></details><dialog id=f open></dialog>\
         <my-widget id=w></my-widget>",
    );
    assert_eq!(select(&doc, ":open"), ["d", "f"]);
    assert_eq!(select(&doc, "my-widget:defined"), ["w"]);
}

#[test]
fn visible_is_decided_from_the_markup() {
    let doc = Document::parse(
        r#"<!DOCTYPE html><body>
        <p id=shown>a</p>
        <p id=hidden-attr hidden><span id=in-hidden></span></p>
        <div style="display: none"><span id=in-display-none></span></div>
        <div style="visibility:hidden"><span id=in-invisible></span>
          <span id=visible-again style="visibility: visible"></span></div>
        <input id=hidden-input type=hidden><input id=text-input>
        <details><summary id=summary></summary><p id=in-details></p></details>
        <dialog><p id=in-dialog></p></dialog>
        <noscript><p id=in-noscript></p></noscript>
        </body>"#,
    );
    let visible = select(&doc, "body :visible");
    assert_eq!(
        visible,
        ["shown", "visible-again", "text-input", "details", "summary"]
    );
    assert!(select(&doc, "script:visible, head:visible, title:visible").is_empty());
    assert_eq!(select(&doc, "#in-hidden:not(:visible)"), ["in-hidden"]);
}
