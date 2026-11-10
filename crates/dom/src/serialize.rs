//! HTML serialization: `outerHTML`, `innerHTML` and whole documents.
//!
//! Follows the HTML standard's fragment serializing algorithm, including the
//! current rule of escaping `<` and `>` in attribute values.

use crate::document::Document;
use crate::name::Name;
use crate::node::{NodeData, NodeId};

impl Document {
    /// The whole document as HTML: doctype, comments and the document
    /// element.
    pub fn html(&self) -> String {
        self.inner_html(self.root())
    }

    /// `Element.outerHTML`: the node itself and its descendants. For a
    /// document or fragment this is the same as [`inner_html`](Self::inner_html).
    pub fn outer_html(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.write_outer_html(id, &mut out);
        out
    }

    /// `innerHTML`: the node's children (a template's contents).
    pub fn inner_html(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.write_inner_html(id, &mut out);
        out
    }

    /// Appends [`outer_html`](Self::outer_html) to `out`.
    pub fn write_outer_html(&self, id: NodeId, out: &mut String) {
        match self.node(id).data {
            NodeData::Document | NodeData::Fragment => self.write_inner_html(id, out),
            _ => self.serialize(Some(id), true, out),
        }
    }

    /// Appends [`inner_html`](Self::inner_html) to `out`.
    pub fn write_inner_html(&self, id: NodeId, out: &mut String) {
        if self.is_element(id) && self.serializes_as_void(id) {
            return;
        }
        self.serialize(self.first_child(self.content_parent(id)), false, out);
    }

    /// Serializes `first` and its following siblings (just `first` if
    /// `only_first`) with their descendants. Iterative, so deep trees cannot
    /// overflow the stack.
    fn serialize(&self, first: Option<NodeId>, only_first: bool, out: &mut String) {
        // Elements whose end tags are still to be written.
        let mut open: Vec<NodeId> = Vec::new();
        let mut next = first;
        loop {
            match next {
                Some(n) => {
                    if self.write_start(n, out) {
                        open.push(n);
                        next = self.first_child(self.content_parent(n));
                    } else if open.is_empty() && only_first {
                        return;
                    } else {
                        next = self.next_sibling(n);
                    }
                }
                None => {
                    let Some(el) = open.pop() else { return };
                    out.push_str("</");
                    self.write_tag_name(el, out);
                    out.push('>');
                    if open.is_empty() && only_first {
                        return;
                    }
                    next = self.next_sibling(el);
                }
            }
        }
    }

    /// The node whose children are serialized as `id`'s content.
    fn content_parent(&self, id: NodeId) -> NodeId {
        self.template_contents(id).unwrap_or(id)
    }

    /// Writes a node, or an element's start tag. Returns true if the node is
    /// an element whose content and end tag must follow.
    fn write_start(&self, id: NodeId, out: &mut String) -> bool {
        match &self.node(id).data {
            NodeData::Element(e) => {
                out.push('<');
                self.write_tag_name(id, out);
                for a in &e.attrs {
                    out.push(' ');
                    match a.ns {
                        Name::EMPTY => {}
                        Name::NS_XML => out.push_str("xml:"),
                        Name::NS_XMLNS if a.local == Name::XMLNS => {}
                        Name::NS_XMLNS => out.push_str("xmlns:"),
                        Name::NS_XLINK => out.push_str("xlink:"),
                        _ if a.prefix != Name::EMPTY => {
                            out.push_str(self.name_str(a.prefix));
                            out.push(':');
                        }
                        _ => {}
                    }
                    out.push_str(self.name_str(a.local));
                    out.push_str("=\"");
                    escape(&a.value, true, out);
                    out.push('"');
                }
                out.push('>');
                !self.serializes_as_void(id)
            }
            NodeData::Text(s) => {
                if self.parent(id).is_some_and(|p| self.is_raw_text_parent(p)) {
                    out.push_str(s);
                } else {
                    escape(s, false, out);
                }
                false
            }
            NodeData::Comment(s) => {
                out.push_str("<!--");
                out.push_str(s);
                out.push_str("-->");
                false
            }
            NodeData::Doctype(d) => {
                out.push_str("<!DOCTYPE ");
                out.push_str(&d.name);
                out.push('>');
                false
            }
            // Only reachable through outer_html, which handles these.
            NodeData::Document | NodeData::Fragment => false,
            NodeData::Free => unreachable!(),
        }
    }

    /// Local name for HTML, SVG and MathML elements; qualified name for
    /// anything else.
    fn write_tag_name(&self, id: NodeId, out: &mut String) {
        let ns = self.namespace(id).expect("is an element");
        if !matches!(ns, Name::NS_HTML | Name::NS_SVG | Name::NS_MATHML) {
            let prefix = self.prefix(id).expect("is an element");
            if prefix != Name::EMPTY {
                out.push_str(self.name_str(prefix));
                out.push(':');
            }
        }
        out.push_str(self.name_str(self.local_name(id).expect("is an element")));
    }

    fn serializes_as_void(&self, id: NodeId) -> bool {
        self.namespace(id) == Some(Name::NS_HTML)
            && matches!(
                self.local_name(id),
                Some(
                    Name::AREA
                        | Name::BASE
                        | Name::BASEFONT
                        | Name::BGSOUND
                        | Name::BR
                        | Name::COL
                        | Name::EMBED
                        | Name::FRAME
                        | Name::HR
                        | Name::IMG
                        | Name::INPUT
                        | Name::KEYGEN
                        | Name::LINK
                        | Name::META
                        | Name::PARAM
                        | Name::SOURCE
                        | Name::TRACK
                        | Name::WBR
                )
            )
    }

    /// Whether text children of `id` are written without escaping.
    fn is_raw_text_parent(&self, id: NodeId) -> bool {
        if self.namespace(id) != Some(Name::NS_HTML) {
            return false;
        }
        match self.local_name(id) {
            Some(
                Name::STYLE
                | Name::SCRIPT
                | Name::XMP
                | Name::IFRAME
                | Name::NOEMBED
                | Name::NOFRAMES
                | Name::PLAINTEXT,
            ) => true,
            Some(Name::NOSCRIPT) => self.scripting_enabled(),
            _ => false,
        }
    }
}

/// Escapes text (`&`, U+00A0, `<`, `>`) or an attribute value (the same plus
/// `"`) into `out`.
fn escape(s: &str, attr: bool, out: &mut String) {
    let bytes = s.as_bytes();
    let mut last = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let rep = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' if attr => "&quot;",
            // U+00A0 is C2 A0 in UTF-8; C2 is only ever a lead byte.
            0xC2 if bytes.get(i + 1) == Some(&0xA0) => "&nbsp;",
            _ => continue,
        };
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = if b == 0xC2 { i + 2 } else { i + 1 };
    }
    out.push_str(&s[last..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(doc: &mut Document, parent: NodeId, name: &str) -> NodeId {
        let e = doc.create_element(name);
        doc.append_child(parent, e).unwrap();
        e
    }

    fn text(doc: &mut Document, parent: NodeId, data: &str) {
        let t = doc.create_text(data);
        doc.append_child(parent, t).unwrap();
    }

    #[test]
    fn escapes_text_and_attributes() {
        let mut out = String::new();
        escape("a & b < c > d \"q\" \u{a0}é", false, &mut out);
        assert_eq!(out, "a &amp; b &lt; c &gt; d \"q\" &nbsp;é");
        out.clear();
        escape("a & b < c > d \"q\" \u{a0}", true, &mut out);
        assert_eq!(out, "a &amp; b &lt; c &gt; d &quot;q&quot; &nbsp;");
    }

    #[test]
    fn serializes_a_document() {
        let mut doc = Document::new();
        let root = doc.root();
        let dt = doc.create_doctype("html", "", "");
        doc.append_child(root, dt).unwrap();
        let html = el(&mut doc, root, "html");
        let head = el(&mut doc, html, "head");
        let title = el(&mut doc, head, "title");
        text(&mut doc, title, "A < B");
        let body = el(&mut doc, html, "body");
        let c = doc.create_comment(" hi ");
        doc.append_child(body, c).unwrap();
        let p = el(&mut doc, body, "p");
        doc.set_attribute(p, "class", "x\"y");
        doc.set_attribute(p, "data-empty", "");
        text(&mut doc, p, "one");
        el(&mut doc, p, "br");
        text(&mut doc, p, "two");
        assert_eq!(
            doc.html(),
            "<!DOCTYPE html><html><head><title>A &lt; B</title></head><body>\
             <!-- hi --><p class=\"x&quot;y\" data-empty=\"\">one<br>two</p></body></html>"
        );
        assert_eq!(
            doc.outer_html(p),
            "<p class=\"x&quot;y\" data-empty=\"\">one<br>two</p>"
        );
        assert_eq!(doc.inner_html(p), "one<br>two");
        assert_eq!(doc.outer_html(c), "<!-- hi -->");
        let t = doc.first_child(p).unwrap();
        assert_eq!(doc.outer_html(t), "one");
        // Serializing one node leaves out its following siblings.
        assert_eq!(doc.outer_html(head), "<head><title>A &lt; B</title></head>");
    }

    #[test]
    fn void_elements_have_no_end_tag_or_content() {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        let img = el(&mut doc, root, "img");
        // Children added through the DOM are not serialized.
        text(&mut doc, img, "ignored");
        el(&mut doc, root, "input");
        assert_eq!(doc.outer_html(root), "<div><img><input></div>");
        assert_eq!(doc.inner_html(img), "");
        // Only HTML elements are void.
        let svg_img = doc.create_element_ns("http://www.w3.org/2000/svg", "img");
        assert_eq!(doc.outer_html(svg_img), "<img></img>");
    }

    #[test]
    fn raw_text_elements_are_not_escaped() {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        for name in [
            "script", "style", "noscript", "xmp", "iframe", "textarea", "title",
        ] {
            let e = el(&mut doc, root, name);
            text(&mut doc, e, "a<b&c");
        }
        assert_eq!(
            doc.inner_html(root),
            "<script>a<b&c</script><style>a<b&c</style><noscript>a<b&c</noscript>\
             <xmp>a<b&c</xmp><iframe>a<b&c</iframe>\
             <textarea>a&lt;b&amp;c</textarea><title>a&lt;b&amp;c</title>"
        );
        doc.set_scripting_enabled(false);
        let ns = doc.children(root).nth(2).unwrap();
        assert_eq!(doc.outer_html(ns), "<noscript>a&lt;b&amp;c</noscript>");
    }

    #[test]
    fn templates_serialize_their_contents() {
        let mut doc = Document::new();
        let tpl = doc.create_element("template");
        let contents = doc.template_contents(tpl).unwrap();
        let b = el(&mut doc, contents, "b");
        text(&mut doc, b, "x");
        assert_eq!(doc.outer_html(tpl), "<template><b>x</b></template>");
        assert_eq!(doc.inner_html(tpl), "<b>x</b>");
        assert_eq!(doc.outer_html(contents), "<b>x</b>");
    }

    #[test]
    fn foreign_and_namespaced_names() {
        let mut doc = Document::new();
        let svg = doc.create_element_ns("http://www.w3.org/2000/svg", "svg");
        doc.set_attribute(svg, "viewBox", "0 0 1 1");
        doc.set_attribute_ns(svg, "http://www.w3.org/1999/xlink", "xlink:href", "#a");
        doc.set_attribute_ns(
            svg,
            "http://www.w3.org/XML/1998/namespace",
            "xml:lang",
            "en",
        );
        doc.set_attribute_ns(svg, "http://www.w3.org/2000/xmlns/", "xmlns", "urn:a");
        doc.set_attribute_ns(svg, "http://www.w3.org/2000/xmlns/", "xmlns:x", "urn:x");
        doc.set_attribute_ns(svg, "urn:other", "o:attr", "v");
        let custom = doc.create_element_ns("urn:x", "x:thing");
        doc.append_child(svg, custom).unwrap();
        let fo = doc.create_element_ns("http://www.w3.org/2000/svg", "foreignObject");
        doc.append_child(svg, fo).unwrap();
        assert_eq!(
            doc.outer_html(svg),
            "<svg viewBox=\"0 0 1 1\" xlink:href=\"#a\" xml:lang=\"en\" xmlns=\"urn:a\" \
             xmlns:x=\"urn:x\" o:attr=\"v\"><x:thing></x:thing><foreignObject></foreignObject></svg>"
        );
    }

    #[test]
    fn deep_trees_serialize_without_recursion() {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        let mut parent = root;
        for _ in 0..10_000 {
            parent = el(&mut doc, parent, "i");
        }
        let html = doc.outer_html(root);
        assert_eq!(html.len(), "<div></div>".len() + 10_000 * "<i></i>".len());
    }
}
