//! Parsing HTML into the arena and serializing it back.

use dasher_dom::{
    AdjacentPosition, Document, DomError, Name, NodeId, NodeKind, ParseOptions, QuirksMode,
    ShadowRootMode,
};

fn body_html(html: &str) -> String {
    let doc = Document::parse(html);
    doc.inner_html(doc.body().expect("has a body"))
}

fn find(doc: &Document, local: Name) -> NodeId {
    doc.descendants(doc.root())
        .find(|&n| doc.local_name(n) == Some(local))
        .expect("element exists")
}

#[test]
fn parses_a_simple_document() {
    let doc = Document::parse("<!DOCTYPE html><title>T</title><p class=a id=x>Hi &amp; bye");
    assert_eq!(
        doc.html(),
        "<!DOCTYPE html><html><head><title>T</title></head>\
         <body><p class=\"a\" id=\"x\">Hi &amp; bye</p></body></html>"
    );
    assert_eq!(doc.quirks_mode(), QuirksMode::NoQuirks);
    assert_eq!(doc.title(), "T");
    let dt = doc.first_child(doc.root()).unwrap();
    assert_eq!(doc.doctype(dt).unwrap().name.as_ref(), "html");
    let p = find(&doc, Name::P);
    assert_eq!(doc.attribute(p, "class"), Some("a"));
    // Text from several tokenizer chunks ends up in one node.
    assert_eq!(doc.children(p).count(), 1);
    assert_eq!(doc.text(doc.first_child(p).unwrap()), Some("Hi & bye"));
}

#[test]
fn missing_doctype_means_quirks_mode() {
    assert_eq!(Document::parse("<p>x").quirks_mode(), QuirksMode::Quirks);
    let limited = Document::parse(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \
         \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\"><p>x",
    );
    assert_eq!(limited.quirks_mode(), QuirksMode::LimitedQuirks);
}

#[test]
fn empty_input_still_has_html_head_and_body() {
    let doc = Document::parse("");
    assert_eq!(doc.html(), "<html><head></head><body></body></html>");
    assert!(doc.head().is_some());
}

#[test]
fn misnested_formatting_elements_use_the_adoption_agency() {
    assert_eq!(body_html("<b>1<i>2</b>3</i>"), "<b>1<i>2</i></b><i>3</i>");
    assert_eq!(
        body_html("<a href=x>1<p>2</a>3</p>"),
        "<a href=\"x\">1</a><p><a href=\"x\">2</a>3</p>"
    );
}

#[test]
fn stray_table_content_is_foster_parented() {
    assert_eq!(
        body_html("<table>a<tr><td>b</td></tr>c</table>"),
        "ac<table><tbody><tr><td>b</td></tr></tbody></table>"
    );
}

#[test]
fn templates_get_their_own_contents() {
    let doc = Document::parse("<body><template id=t><td>x</td></template>");
    let tpl = find(&doc, Name::TEMPLATE);
    assert_eq!(doc.first_child(tpl), None);
    let contents = doc.template_contents(tpl).unwrap();
    assert_eq!(doc.kind(contents), NodeKind::DocumentFragment);
    assert_eq!(doc.inner_html(tpl), "<td>x</td>");
    assert_eq!(
        doc.inner_html(doc.body().unwrap()),
        "<template id=\"t\"><td>x</td></template>"
    );
}

#[test]
fn declarative_shadow_roots_are_attached() {
    let doc = Document::parse(
        "<div id=host><template shadowrootmode=closed shadowrootserializable>\
         <b>x</b><slot></slot></template>light</div><p>after",
    );
    let host = doc.get_element_by_id(doc.root(), "host").unwrap();
    // The template is gone; its content is the shadow tree.
    assert_eq!(doc.inner_html(host), "light");
    let root = doc.shadow_root(host).expect("has a shadow root");
    assert_eq!(doc.inner_html(root), "<b>x</b><slot></slot>");
    let init = doc.shadow_root_init(root).unwrap();
    assert_eq!(init.mode, ShadowRootMode::Closed);
    assert!(init.serializable && !init.clonable && !init.delegates_focus);
    assert!(doc.is_connected(doc.first_child(root).unwrap()));
    assert_eq!(
        body_html("<div><template shadowrootmode=open></template></div><p>after"),
        "<div></div><p>after</p>"
    );
}

#[test]
fn templates_that_cannot_be_shadow_roots_stay_templates() {
    // A bad mode, a host that cannot have one, a second root, and
    // fragment parsing.
    for html in [
        "<div><template shadowrootmode=sideways><b>x</b></template></div>",
        "<ul><template shadowrootmode=open><b>x</b></template></ul>",
        "<div><template shadowrootmode=open></template><template shadowrootmode=open><b>x</b></template></div>",
    ] {
        let doc = Document::parse(html);
        let tpl = doc
            .descendants(doc.root())
            .filter(|&n| doc.local_name(n) == Some(Name::TEMPLATE))
            .last()
            .expect("a template stays");
        assert_eq!(doc.inner_html(tpl), "<b>x</b>", "{html}");
    }
    let mut doc = Document::parse("<div></div>");
    let div = find(&doc, Name::DIV);
    doc.set_inner_html(
        div,
        "<p><template shadowrootmode=open><b>x</b></template></p>",
    );
    let tpl = find(&doc, Name::TEMPLATE);
    assert_eq!(doc.inner_html(tpl), "<b>x</b>");
    assert!(!doc.has_shadow_roots());
}

#[test]
fn foreign_content_gets_namespaces_and_adjusted_names() {
    let doc = Document::parse(
        "<svg viewbox='0 0 1 1'><foreignobject><p>x</p></foreignobject>\
         <use xlink:href=#a /></svg><math><mi>y</mi></math>",
    );
    let svg = find(&doc, Name::SVG);
    assert_eq!(doc.namespace(svg), Some(Name::NS_SVG));
    assert_eq!(doc.attribute_named(svg, Name::VIEWBOX), Some("0 0 1 1"));
    let fo = doc.first_child(svg).unwrap();
    assert_eq!(doc.name_str(doc.local_name(fo).unwrap()), "foreignObject");
    let p = doc.first_child(fo).unwrap();
    assert!(doc.is_html(p, Name::P));
    let use_el = doc.next_sibling(fo).unwrap();
    assert_eq!(
        doc.attribute_ns(use_el, Name::NS_XLINK, Name::HREF),
        Some("#a")
    );
    assert_eq!(doc.attribute(use_el, "xlink:href"), Some("#a"));
    let math = find(&doc, Name::MATH);
    assert_eq!(doc.namespace(math), Some(Name::NS_MATHML));
    assert_eq!(
        doc.inner_html(doc.body().unwrap()),
        "<svg viewBox=\"0 0 1 1\"><foreignObject><p>x</p></foreignObject>\
         <use xlink:href=\"#a\"></use></svg><math><mi>y</mi></math>"
    );
}

#[test]
fn noscript_depends_on_scripting() {
    let html = "<body><noscript><p>x</p></noscript>";
    let on = Document::parse(html);
    let ns = find(&on, Name::NOSCRIPT);
    assert!(on.is_text(on.first_child(ns).unwrap()));
    assert_eq!(on.outer_html(ns), "<noscript><p>x</p></noscript>");

    let off = Document::parse_with(
        html,
        ParseOptions {
            scripting_enabled: false,
        },
    );
    let ns = find(&off, Name::NOSCRIPT);
    assert!(off.is_html(off.first_child(ns).unwrap(), Name::P));
    assert_eq!(off.outer_html(ns), "<noscript><p>x</p></noscript>");
}

#[test]
fn script_and_style_contents_are_raw_text() {
    let html = "<script>if (a < b && c) { x = '</div>'; }</script><style>a > b { }</style>";
    let doc = Document::parse(html);
    let script = find(&doc, Name::SCRIPT);
    assert_eq!(
        doc.text_content(script).as_deref(),
        Some("if (a < b && c) { x = '</div>'; }")
    );
    assert!(!doc.script_already_started(script));
    assert_eq!(doc.inner_html(doc.head().unwrap()), html);
}

#[test]
fn repeated_html_and_body_tags_merge_attributes() {
    let doc = Document::parse("<html a=1><body c=1><html b=2 a=3><body c=2 d=2>");
    let html = doc.document_element().unwrap();
    assert_eq!(doc.attribute(html, "a"), Some("1"));
    assert_eq!(doc.attribute(html, "b"), Some("2"));
    let body = doc.body().unwrap();
    assert_eq!(doc.attribute(body, "c"), Some("1"));
    assert_eq!(doc.attribute(body, "d"), Some("2"));
}

#[test]
fn line_endings_and_entities_are_normalized() {
    let doc = Document::parse("<pre>a\r\nb\rc&nbsp;&lt;&#x41;&copy</pre>");
    let pre = find(&doc, Name::PRE);
    assert_eq!(doc.text_content(pre).as_deref(), Some("a\nb\nc\u{a0}<A©"));
    assert_eq!(doc.inner_html(pre), "a\nb\nc&nbsp;&lt;A©");
}

#[test]
fn serialization_round_trips() {
    let html = "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
                <title>x</title><script>a<b</script></head><body>\
                <!-- c --><div id=\"a\" class=\"b c\"><img src=\"x.png\" alt=\"&quot;\">\
                <ul><li>1</li><li>2</li></ul><table><tbody><tr><td>t</td></tr></tbody></table>\
                <svg><path d=\"M0 0\"></path></svg><template><p>t</p></template></div></body></html>";
    let doc = Document::parse(html);
    let once = doc.html();
    assert_eq!(once, html);
    assert_eq!(Document::parse(&once).html(), once);
}

#[test]
fn custom_elements_and_attributes_are_interned_per_document() {
    let doc = Document::parse("<my-widget data-id=7 data-id2=8></my-widget>");
    let w = doc.first_child(doc.body().unwrap()).unwrap();
    assert_eq!(doc.name_str(doc.local_name(w).unwrap()), "my-widget");
    assert_eq!(doc.attribute(w, "data-id"), Some("7"));
    assert_eq!(doc.names().dynamic_len(), 3);
}

#[test]
fn fragment_parsing_uses_the_context_element() {
    let mut doc =
        Document::parse("<table><tr id=row></tr></table><ul id=list></ul><textarea></textarea>");
    let tr = find(&doc, Name::TR);
    doc.set_inner_html(tr, "<td>1<td>2");
    assert_eq!(doc.inner_html(tr), "<td>1</td><td>2</td>");

    let ul = find(&doc, Name::UL);
    doc.set_inner_html(ul, "<li>a<li>b");
    assert_eq!(doc.inner_html(ul), "<li>a</li><li>b</li>");

    let textarea = find(&doc, Name::TEXTAREA);
    doc.set_inner_html(textarea, "<b>not markup</b>");
    assert_eq!(
        doc.text_content(textarea).as_deref(),
        Some("<b>not markup</b>")
    );
}

#[test]
fn contextual_fragment_scripts_can_still_run() {
    let mut doc = Document::parse("<table><tbody><tr><td>1</td></tr></tbody></table>");
    let scripts = |doc: &Document, fragment: NodeId| -> Vec<NodeId> {
        doc.descendants(fragment)
            .filter(|&n| doc.is_html(n, Name::SCRIPT))
            .collect()
    };
    // No context, and the root element as context: parsed as in <body>.
    let html = doc.document_element().unwrap();
    for context in [None, Some(html)] {
        let fragment =
            doc.parse_contextual_fragment(context, "<tr><td>x</td></tr><script>a()</script>");
        assert_eq!(doc.kind(fragment), NodeKind::DocumentFragment);
        assert_eq!(doc.inner_html(fragment), "x<script>a()</script>");
        let scripts = scripts(&doc, fragment);
        assert_eq!(scripts.len(), 1);
        assert!(!doc.script_already_started(scripts[0]));
    }
    // An element as context: its content model applies.
    let row = find(&doc, Name::TR);
    let fragment =
        doc.parse_contextual_fragment(Some(row), "<td><div><script>b()</script></div></td>");
    assert_eq!(
        doc.inner_html(fragment),
        "<td><div><script>b()</script></div></td>"
    );
    assert!(!doc.script_already_started(scripts(&doc, fragment)[0]));
    // The plain fragment parser still marks them.
    let fragment = doc.parse_fragment(row, "<td><script>c()</script></td>");
    assert!(doc.script_already_started(scripts(&doc, fragment)[0]));
    // The context is not touched.
    assert_eq!(doc.inner_html(row), "<td>1</td>");
}

#[test]
fn inner_html_replaces_children_and_marks_scripts() {
    let mut doc = Document::parse("<div id=d><span>old</span></div>");
    let div = find(&doc, Name::DIV);
    let old = doc.first_child(div).unwrap();
    doc.set_inner_html(div, "<p>new</p><script>run()</script>");
    assert_eq!(doc.inner_html(div), "<p>new</p><script>run()</script>");
    // The old child is detached but still usable.
    assert_eq!(doc.parent(old), None);
    assert_eq!(doc.outer_html(old), "<span>old</span>");
    let script = find(&doc, Name::SCRIPT);
    assert!(doc.script_already_started(script));
    assert!(doc.is_connected(script));

    doc.set_inner_html(div, "");
    assert_eq!(doc.first_child(div), None);
}

#[test]
fn inner_html_on_a_template_fills_its_contents() {
    let mut doc = Document::parse("<template></template>");
    let tpl = find(&doc, Name::TEMPLATE);
    doc.set_inner_html(tpl, "<tr><td>x</td></tr>");
    assert_eq!(doc.first_child(tpl), None);
    assert_eq!(doc.inner_html(tpl), "<tr><td>x</td></tr>");
}

#[test]
fn fragment_parsing_does_not_leak_scratch_nodes() {
    let mut doc = Document::parse("<div></div>");
    let div = find(&doc, Name::DIV);
    let before = doc.len();
    doc.set_inner_html(div, "<b>x</b>");
    // Just the <b> and its text; html5ever's scratch nodes are released.
    assert_eq!(doc.len(), before + 2);
    let slots = doc.capacity();
    for _ in 0..10 {
        let b = doc.first_child(div).unwrap();
        doc.set_inner_html(div, "<b>x</b>");
        doc.release(b);
    }
    // Each round reuses the slots freed by the previous one.
    assert_eq!(doc.len(), before + 2);
    assert!(
        doc.capacity() <= slots + 8,
        "{} > {}",
        doc.capacity(),
        slots + 8
    );
}

#[test]
fn fragments_inherit_quirks_mode() {
    // In quirks mode a <p> does not close on <table>.
    let mut quirky = Document::parse("<div></div>");
    let div = find(&quirky, Name::DIV);
    quirky.set_inner_html(div, "<p><table></table>");
    assert_eq!(quirky.inner_html(div), "<p><table></table></p>");

    let mut standard = Document::parse("<!DOCTYPE html><div></div>");
    let div = find(&standard, Name::DIV);
    standard.set_inner_html(div, "<p><table></table>");
    assert_eq!(standard.inner_html(div), "<p></p><table></table>");
}

#[test]
fn outer_html_replaces_the_element() {
    let mut doc = Document::parse("<div><span>a</span><i>b</i></div>");
    let span = find(&doc, Name::SPAN);
    doc.set_outer_html(span, "<b>1</b><b>2</b>").unwrap();
    let div = find(&doc, Name::DIV);
    assert_eq!(doc.inner_html(div), "<b>1</b><b>2</b><i>b</i>");
    assert_eq!(doc.parent(span), None);

    // No parent: nothing happens.
    assert_eq!(doc.set_outer_html(span, "<p>"), Ok(()));
    // The document element cannot be replaced this way.
    let html = doc.document_element().unwrap();
    assert_eq!(
        doc.set_outer_html(html, "<p>"),
        Err(DomError::NoModificationAllowed)
    );

    // A child of <html> is parsed in the <html> context, where body
    // content gets an implied <head> and <body>.
    let mut doc2 = Document::parse("<head></head><body></body>");
    let body = doc2.body().unwrap();
    doc2.set_outer_html(body, "<p>x</p>").unwrap();
    let html2 = doc2.document_element().unwrap();
    assert_eq!(
        doc2.inner_html(html2),
        "<head></head><head></head><body><p>x</p></body>"
    );

    // Inside a fragment, a <body> context is used.
    let frag = doc.create_fragment();
    let x = doc.create_element("x");
    doc.append_child(frag, x).unwrap();
    doc.set_outer_html(x, "<td>cell</td>text").unwrap();
    assert_eq!(doc.inner_html(frag), "celltext");
}

#[test]
fn insert_adjacent_html_positions() {
    let mut doc = Document::parse("<div><p>x</p></div>");
    let p = find(&doc, Name::P);
    doc.insert_adjacent_html(p, AdjacentPosition::BeforeBegin, "<a>1</a>")
        .unwrap();
    doc.insert_adjacent_html(p, AdjacentPosition::AfterBegin, "<b>2</b>")
        .unwrap();
    doc.insert_adjacent_html(p, AdjacentPosition::BeforeEnd, "<i>3</i>")
        .unwrap();
    doc.insert_adjacent_html(p, AdjacentPosition::AfterEnd, "<u>4</u>")
        .unwrap();
    let div = find(&doc, Name::DIV);
    assert_eq!(
        doc.inner_html(div),
        "<a>1</a><p><b>2</b>x<i>3</i></p><u>4</u>"
    );

    let html = doc.document_element().unwrap();
    assert_eq!(
        doc.insert_adjacent_html(html, AdjacentPosition::AfterEnd, "<p>"),
        Err(DomError::NoModificationAllowed)
    );
    // The <html> element as context parses as if in <body>.
    doc.insert_adjacent_html(html, AdjacentPosition::BeforeEnd, "<p>end</p>")
        .unwrap();
    assert_eq!(doc.outer_html(doc.last_child(html).unwrap()), "<p>end</p>");
}

#[test]
fn parse_fragment_returns_a_detached_fragment() {
    let mut doc = Document::parse("<body>");
    let body = doc.body().unwrap();
    let frag = doc.parse_fragment(body, "a<b>c</b>");
    assert_eq!(doc.kind(frag), NodeKind::DocumentFragment);
    assert_eq!(doc.parent(frag), None);
    assert_eq!(doc.inner_html(frag), "a<b>c</b>");
    assert_eq!(doc.first_child(body), None);
}

#[test]
fn documents_parse_into_a_second_document_node() {
    let mut doc = Document::parse("<!DOCTYPE html><title>Page</title>");
    let other = doc.create_document();
    let quirks = doc.parse_document_into(
        other,
        "<title>Other</title><body><noscript><p>x</p></noscript><script>run()</script>",
    );
    // No doctype: the new document is in quirks mode, the page is not.
    assert_eq!(quirks, QuirksMode::Quirks);
    assert_eq!(doc.quirks_mode(), QuirksMode::NoQuirks);
    assert_eq!(
        doc.inner_html(other),
        "<html><head><title>Other</title></head><body>\
         <noscript><p>x</p></noscript><script>run()</script></body></html>"
    );
    // Its nodes are not the page's: not connected, and not seen by the
    // document-level queries.
    let script = doc
        .descendants(other)
        .find(|&n| doc.is_html(n, Name::SCRIPT))
        .unwrap();
    assert!(!doc.is_connected(script));
    assert!(doc.script_already_started(script), "scripts never run");
    assert_eq!(doc.title(), "Page");
    assert_eq!(doc.tree_root(script), other);
    // Scripting was off: noscript content is markup.
    let noscript = doc
        .descendants(other)
        .find(|&n| doc.is_html(n, Name::NOSCRIPT))
        .unwrap();
    assert!(doc.is_html(doc.first_child(noscript).unwrap(), Name::P));
}

#[test]
fn parsing_into_a_document_keeps_its_doctype() {
    let mut doc = Document::parse("<p>quirky page");
    let other = doc.create_document();
    let quirks = doc.parse_document_into(other, "<!DOCTYPE html><p>hi");
    assert_eq!(quirks, QuirksMode::NoQuirks);
    assert_eq!(doc.quirks_mode(), QuirksMode::Quirks);
    let dt = doc.first_child(other).unwrap();
    assert_eq!(doc.doctype(dt).unwrap().name.as_ref(), "html");
}
