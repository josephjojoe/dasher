//! Parsing XML documents into a second document node.

use dasher_dom::{Document, Name, NodeId, NodeKind, XmlError};

fn parse(xml: &str) -> (Document, NodeId) {
    let mut doc = Document::new();
    let d = doc.create_document();
    doc.parse_xml_into(d, xml)
        .unwrap_or_else(|e| panic!("{xml}: {e}"));
    (doc, d)
}

fn error(xml: &str) -> XmlError {
    let mut doc = Document::new();
    let d = doc.create_document();
    let len = doc.len();
    let e = doc.parse_xml_into(d, xml).expect_err(xml);
    assert!(
        doc.first_child(d).is_none(),
        "{xml}: partial tree left behind"
    );
    assert_eq!(doc.len(), len, "{xml}: partial tree not freed");
    e
}

fn root(doc: &Document, d: NodeId) -> NodeId {
    doc.element_children(d).next().expect("has a root element")
}

#[test]
fn elements_attributes_and_text() {
    let (doc, d) = parse(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version='2.0'><channel><title>Feed &amp; more</title>\
         <link>https://example.com/</link><item/><Item a=\"1 &lt; 2\"/></channel></rss>\n",
    );
    let rss = root(&doc, d);
    assert_eq!(doc.namespace(rss), Some(Name::EMPTY));
    assert_eq!(doc.attribute(rss, "version"), Some("2.0"));
    // Case is kept, void HTML names mean nothing, and empty-element tags
    // have no children.
    assert_eq!(
        doc.inner_html(rss),
        "<channel><title>Feed &amp; more</title><link>https://example.com/</link>\
         <item></item><Item a=\"1 &lt; 2\"></Item></channel>"
    );
    assert_eq!(doc.children(d).count(), 1, "the declaration makes no node");
}

#[test]
fn namespaces_resolve_by_scope() {
    let (doc, d) = parse(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="#a"/><g xmlns="urn:x"><p:q xmlns:p="urn:p" p:r="1" s="2"/></g><circle/></svg>"##,
    );
    let svg = root(&doc, d);
    assert_eq!(doc.namespace(svg), Some(Name::NS_SVG));
    let kids: Vec<NodeId> = doc.element_children(svg).collect();
    let use_ = kids[0];
    let attr = &doc.attrs(use_)[0];
    assert_eq!(doc.attr_namespace(attr), Name::NS_XLINK);
    assert_eq!(doc.attr_prefix(attr), Name::XLINK);
    let g = kids[1];
    assert_eq!(doc.name_str(doc.namespace(g).unwrap()), "urn:x");
    let q = doc.first_child(g).unwrap();
    assert_eq!(doc.name_str(doc.namespace(q).unwrap()), "urn:p");
    assert_eq!(doc.qualified_name(q).as_deref(), Some("p:q"));
    let attrs = doc.attrs(q);
    assert_eq!(doc.name_str(doc.attr_namespace(&attrs[1])), "urn:p");
    assert_eq!(doc.attr_namespace(&attrs[2]), Name::EMPTY);
    // The inner default namespace ended with <g>.
    assert_eq!(doc.namespace(kids[2]), Some(Name::NS_SVG));
    // xmlns attributes are in the xmlns namespace.
    assert_eq!(doc.attr_namespace(&doc.attrs(svg)[0]), Name::NS_XMLNS);
}

#[test]
fn cdata_comments_references_and_line_breaks() {
    let (doc, d) = parse(
        "<!-- before --><a>x<![CDATA[<b>&amp;</b>]]>y&#65;&#x42;\r\nz<!--in--><b t=\"1\n2\"/></a>",
    );
    assert_eq!(doc.kind(doc.first_child(d).unwrap()), NodeKind::Comment);
    let a = root(&doc, d);
    let text = doc.first_child(a).unwrap();
    assert_eq!(doc.text(text), Some("x<b>&amp;</b>yAB\nz"));
    let b = doc.last_child(a).unwrap();
    assert_eq!(doc.attribute(b, "t"), Some("1 2"));
}

#[test]
fn doctypes_are_kept_and_internal_subsets_skipped() {
    let (doc, d) = parse(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"x.dtd\" [ <!ENTITY e \"]\"> ]>\
         <html xmlns=\"http://www.w3.org/1999/xhtml\"><body/></html>",
    );
    let dt = doc.first_child(d).unwrap();
    let doctype = doc.doctype(dt).unwrap();
    assert_eq!(doctype.name.as_ref(), "html");
    assert_eq!(
        doctype.public_id.as_ref(),
        "-//W3C//DTD XHTML 1.0 Strict//EN"
    );
    assert_eq!(doctype.system_id.as_ref(), "x.dtd");
    let html = root(&doc, d);
    assert!(doc.is_html(html, Name::HTML));
}

#[test]
fn malformed_input_is_an_error_with_a_position() {
    let e = error("<a>\n  <b></a>");
    assert_eq!((e.line, e.column), (2, 10));
    assert!(e.message.contains("</a>"), "{e}");
    for bad in [
        "",
        "just text",
        "<a>",
        "<a></a><b></b>",
        "<a>&nbsp;</a>",
        "<a x=1></a>",
        "<a x='1' x='2'></a>",
        "<p:a></p:a>",
        "<a>]]></a>",
        "<a b='<'></a>",
        "<a></a>trailing",
        "<a><!-- open </a>",
    ] {
        error(bad);
    }
}

#[test]
fn parsing_stops_past_the_node_limit() {
    let mut doc = Document::new();
    doc.set_node_limit(Some(100));
    let d = doc.create_document();
    let xml = format!("<r>{}</r>", "<i>x</i>".repeat(10_000));
    // Cut short without an error: the page is stopped anyway.
    doc.parse_xml_into(d, &xml).unwrap();
    assert!(doc.node_limit_exceeded());
    assert!(doc.len() < 200, "{} nodes", doc.len());
}
