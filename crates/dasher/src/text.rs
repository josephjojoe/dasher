//! Text extraction for the query helpers.
//!
//! Text follows the tree as it would render: a shadow host shows its shadow
//! tree, in which each `<slot>` shows the host's children assigned to it
//! (or its own fallback content).

use dasher_dom::{Document, Name, NodeId, NodeKind};

/// The text of `node` as a reader would see it, roughly `innerText`
/// without layout: text in scripts, styles and other unrendered elements
/// is left out, block elements and `<br>` separate their text from their
/// neighbours', and runs of whitespace collapse to one space, trimmed.
pub(crate) fn visible_text(doc: &Document, node: NodeId) -> String {
    let mut raw = String::new();
    collect(doc, node, &mut raw);
    let mut out = String::with_capacity(raw.len());
    for word in raw.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn collect(doc: &Document, node: NodeId, out: &mut String) {
    match doc.kind(node) {
        NodeKind::Text => out.push_str(doc.text(node).unwrap_or_default()),
        NodeKind::Element => {
            if is_unrendered(doc, node) {
                return;
            }
            let block = is_block(doc, node);
            if block {
                out.push(' ');
            }
            let assigned = slotted(doc, node);
            if let Some(nodes) = assigned.filter(|a| !a.is_empty()) {
                for n in nodes {
                    collect(doc, n, out);
                }
            } else {
                let parent = doc.shadow_root(node).unwrap_or(node);
                for child in doc.children(parent) {
                    collect(doc, child, out);
                }
            }
            if block {
                out.push(' ');
            }
        }
        NodeKind::Document | NodeKind::DocumentFragment => {
            for child in doc.children(node) {
                collect(doc, child, out);
            }
        }
        _ => {}
    }
}

/// The nodes a `<slot>` in a shadow tree shows instead of its own children,
/// if it is one.
fn slotted(doc: &Document, node: NodeId) -> Option<Vec<NodeId>> {
    if !doc.has_shadow_roots() || !doc.is_html(node, Name::SLOT) {
        return None;
    }
    doc.shadow_host(doc.tree_root(node))?;
    Some(doc.assigned_nodes(node))
}

fn is_unrendered(doc: &Document, node: NodeId) -> bool {
    [
        Name::SCRIPT,
        Name::STYLE,
        Name::NOSCRIPT,
        Name::TEMPLATE,
        Name::HEAD,
    ]
    .into_iter()
    .any(|name| doc.is_html(node, name))
}

fn is_block(doc: &Document, node: NodeId) -> bool {
    const BLOCKS: &[Name] = &[
        Name::HTML,
        Name::BODY,
        Name::DIV,
        Name::P,
        Name::MAIN,
        Name::HEADER,
        Name::FOOTER,
        Name::NAV,
        Name::SECTION,
        Name::ARTICLE,
        Name::ASIDE,
        Name::ADDRESS,
        Name::H1,
        Name::H2,
        Name::H3,
        Name::H4,
        Name::H5,
        Name::H6,
        Name::HGROUP,
        Name::HR,
        Name::PRE,
        Name::BLOCKQUOTE,
        Name::OL,
        Name::UL,
        Name::LI,
        Name::MENU,
        Name::DL,
        Name::DT,
        Name::DD,
        Name::FIGURE,
        Name::FIGCAPTION,
        Name::DETAILS,
        Name::SUMMARY,
        Name::DIALOG,
        Name::CENTER,
        Name::BR,
        Name::TABLE,
        Name::CAPTION,
        Name::THEAD,
        Name::TBODY,
        Name::TFOOT,
        Name::TR,
        Name::TD,
        Name::TH,
    ];
    BLOCKS.iter().any(|&name| doc.is_html(node, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(html: &str, id: &str) -> String {
        let doc = Document::parse(html);
        let node = doc
            .descendants(doc.root())
            .find(|&n| doc.element_id(n) == Some(id))
            .unwrap();
        visible_text(&doc, node)
    }

    #[test]
    fn whitespace_collapses_and_trims() {
        assert_eq!(text_of("<p id=x>  a \n\t b  </p>", "x"), "a b");
        assert_eq!(text_of("<p id=x>a<b>b</b>c</p>", "x"), "abc");
    }

    #[test]
    fn blocks_and_line_breaks_separate_text() {
        assert_eq!(
            text_of("<ul id=x><li>one</li><li>two</li></ul>", "x"),
            "one two"
        );
        assert_eq!(text_of("<p id=x>a<br>b</p>", "x"), "a b");
        assert_eq!(
            text_of("<table id=x><tr><td>1</td><td>2</td></tr></table>", "x"),
            "1 2"
        );
        assert_eq!(text_of("<p id=x><span>a</span><em>b</em></p>", "x"), "ab");
    }

    #[test]
    fn unrendered_content_is_skipped() {
        let html = "<div id=x>a<script>var s</script><style>p{}</style>\
                    <template>t</template><noscript>n</noscript>b</div>";
        assert_eq!(text_of(html, "x"), "ab");
    }

    #[test]
    fn shadow_trees_show_in_place_of_their_hosts_children() {
        let html = "<div id=x><my-card><template shadowrootmode=closed><h2><slot name=title>\
                    Untitled</slot></h2><p><slot>empty</slot></p><slot name=foot>foot</slot>\
                    </template><span slot=title>Hello</span> body <b>text</b><i slot=nope>gone</i>\
                    </my-card></div>";
        assert_eq!(text_of(html, "x"), "Hello body text foot");
        // Hosts without slots show only their shadow tree.
        let html = "<p id=x><x-a><template shadowrootmode=open>shadow</template>light</x-a></p>";
        assert_eq!(text_of(html, "x"), "shadow");
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(text_of("<p id=x>a<!-- c -->b</p>", "x"), "ab");
    }
}
