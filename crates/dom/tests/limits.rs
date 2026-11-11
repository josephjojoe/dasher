//! The node limit: noticed when crossed, and bulk operations stop early.

use dasher_dom::Document;

#[test]
fn crossing_the_limit_is_remembered() {
    let mut doc = Document::parse("<p>one</p>");
    assert_eq!(doc.node_limit(), None);
    let limit = doc.len() + 2;
    doc.set_node_limit(Some(limit));
    assert_eq!(doc.node_limit(), Some(limit));

    let a = doc.create_element("div");
    let _b = doc.create_text("b");
    assert_eq!(doc.len(), limit);
    assert!(!doc.node_limit_exceeded(), "reaching the limit is allowed");

    let c = doc.create_comment("c");
    assert!(doc.node_limit_exceeded());
    // Freeing nodes does not undo it.
    doc.release(c);
    doc.release(a);
    assert!(doc.node_limit_exceeded());
}

#[test]
fn a_limit_below_the_current_size_is_already_exceeded() {
    let mut doc = Document::parse("<ul><li>1<li>2<li>3</ul>");
    doc.set_node_limit(Some(3));
    assert!(doc.node_limit_exceeded());
}

#[test]
fn inner_html_stops_parsing_past_the_limit() {
    let mut doc = Document::parse("<div id=x></div>");
    doc.set_node_limit(Some(1000));
    let x = doc.get_element_by_id(doc.root(), "x").unwrap();
    // 800 KB that would make 200,000 nodes.
    let html = "<p>x</p>".repeat(100_000);
    doc.set_inner_html(x, &html);
    assert!(doc.node_limit_exceeded());
    // One 16 KiB slice of input past the limit at most: about 4,000 nodes.
    assert!(doc.len() < 10_000, "{} nodes", doc.len());
}

#[test]
fn small_fragments_parse_as_before() {
    let mut doc = Document::parse("<div id=x></div>");
    doc.set_node_limit(Some(1000));
    let x = doc.get_element_by_id(doc.root(), "x").unwrap();
    // Longer than one slice, with a multi-byte character across the
    // boundary.
    let text = format!("{}é{}", "a".repeat(16 * 1024 - 1), "b".repeat(100));
    doc.set_inner_html(x, &format!("<p>{text}</p><p>end</p>"));
    assert!(!doc.node_limit_exceeded());
    assert_eq!(doc.text_content(x).unwrap(), format!("{text}end"));
}

#[test]
fn deep_clones_stop_past_the_limit() {
    let mut doc = Document::parse(&format!(
        "<ul id=list>{}</ul>",
        "<li>item</li>".repeat(2000)
    ));
    let list = doc.get_element_by_id(doc.root(), "list").unwrap();
    doc.set_node_limit(Some(doc.len() + 100));
    let before = doc.len();
    doc.clone_node(list, true);
    assert!(doc.node_limit_exceeded());
    assert!(doc.len() < before + 200, "{} nodes", doc.len() - before);
}

#[test]
fn parsing_a_second_document_stops_past_the_limit() {
    let mut doc = Document::parse("<p>page</p>");
    doc.set_node_limit(Some(1000));
    let other = doc.create_document();
    doc.parse_document_into(other, &"<p>x</p>".repeat(100_000));
    assert!(doc.node_limit_exceeded());
    assert!(doc.len() < 10_000, "{} nodes", doc.len());
}
