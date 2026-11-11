//! `innerText` without layout.
//!
//! The real algorithm reads computed styles. This approximates it from the
//! markup: elements that are never rendered or hidden by their attributes
//! or inline style are skipped, whitespace collapses as in normal flow
//! (except inside `<pre>` and `<textarea>`), `<br>` is a line break, block
//! elements start and end lines (paragraphs and headings with a blank
//! line), table cells are separated by tabs and rows by line breaks.

use dasher_dom::{Document, Name, NodeId, NodeKind};

/// `HTMLElement.innerText` for `id`.
pub(crate) fn inner_text(doc: &Document, id: NodeId) -> String {
    if !doc.is_element(id) || hidden(doc, id) {
        return doc.text_content(id).unwrap_or_default();
    }
    let mut out = Writer::default();
    walk(doc, id, false, &mut out);
    out.finish()
}

#[derive(Default)]
struct Writer {
    text: String,
    /// Line breaks owed before the next text.
    breaks: usize,
    /// Whether the last thing written was collapsible whitespace (or the
    /// start of a line), so the next space is dropped.
    space: bool,
}

impl Writer {
    fn line_break(&mut self, n: usize) {
        self.breaks = self.breaks.max(n);
    }

    fn flush_breaks(&mut self) {
        if self.breaks > 0 {
            if !self.text.is_empty() {
                let trimmed = self.text.trim_end_matches(' ').len();
                self.text.truncate(trimmed);
                for _ in 0..self.breaks {
                    self.text.push('\n');
                }
            }
            self.breaks = 0;
            self.space = true;
        }
    }

    fn collapsed(&mut self, s: &str) {
        for c in s.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C') {
                if !self.space && self.breaks == 0 && !self.text.is_empty() {
                    self.text.push(' ');
                    self.space = true;
                }
            } else {
                self.flush_breaks();
                self.text.push(c);
                self.space = false;
            }
        }
    }

    fn preserved(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.flush_breaks();
        self.text.push_str(s);
        self.space = s.ends_with('\n');
    }

    fn finish(mut self) -> String {
        let trimmed = self.text.trim_end_matches(' ').len();
        self.text.truncate(trimmed);
        self.text
    }
}

fn walk(doc: &Document, parent: NodeId, pre: bool, out: &mut Writer) {
    let mut cell_seen = false;
    for child in doc.children(parent) {
        match doc.kind(child) {
            NodeKind::Text => {
                let text = doc.text(child).unwrap_or_default();
                if pre {
                    out.preserved(text);
                } else {
                    out.collapsed(text);
                }
            }
            NodeKind::Element if !hidden(doc, child) => {
                let local = doc.local_name(child).unwrap_or(Name::EMPTY);
                let html = doc.namespace(child) == Some(Name::NS_HTML);
                if html && local == Name::BR {
                    out.preserved("\n");
                    continue;
                }
                if html && matches!(local, Name::TD | Name::TH) {
                    if cell_seen {
                        out.preserved("\t");
                    }
                    cell_seen = true;
                }
                let breaks = if html { block_breaks(local) } else { 0 };
                out.line_break(breaks);
                let pre =
                    pre || (html && matches!(local, Name::PRE | Name::TEXTAREA | Name::LISTING));
                walk(doc, child, pre, out);
                out.line_break(breaks);
            }
            _ => {}
        }
    }
}

/// The line breaks an HTML element puts before and after its content.
fn block_breaks(local: Name) -> usize {
    match local {
        Name::P | Name::H1 | Name::H2 | Name::H3 | Name::H4 | Name::H5 | Name::H6 => 2,
        Name::DIV
        | Name::LI
        | Name::UL
        | Name::OL
        | Name::DL
        | Name::DT
        | Name::DD
        | Name::TR
        | Name::TABLE
        | Name::CAPTION
        | Name::THEAD
        | Name::TBODY
        | Name::TFOOT
        | Name::SECTION
        | Name::ARTICLE
        | Name::ASIDE
        | Name::HEADER
        | Name::FOOTER
        | Name::NAV
        | Name::MAIN
        | Name::ADDRESS
        | Name::BLOCKQUOTE
        | Name::FIGURE
        | Name::FIGCAPTION
        | Name::FORM
        | Name::FIELDSET
        | Name::LEGEND
        | Name::DETAILS
        | Name::SUMMARY
        | Name::DIALOG
        | Name::HR
        | Name::PRE
        | Name::LISTING
        | Name::CENTER
        | Name::HGROUP
        | Name::MENU
        | Name::SEARCH
        | Name::BODY
        | Name::HTML => 1,
        _ => 0,
    }
}

/// Whether an element and its contents are not rendered, as far as the
/// markup tells.
fn hidden(doc: &Document, id: NodeId) -> bool {
    let local = doc.local_name(id).unwrap_or(Name::EMPTY);
    if doc.namespace(id) == Some(Name::NS_HTML) {
        if matches!(
            local,
            Name::SCRIPT
                | Name::STYLE
                | Name::NOSCRIPT
                | Name::TEMPLATE
                | Name::HEAD
                | Name::TITLE
                | Name::META
                | Name::LINK
                | Name::BASE
                | Name::PARAM
                | Name::SOURCE
                | Name::TRACK
                | Name::DATALIST
                | Name::NOEMBED
                | Name::NOFRAMES
        ) {
            return true;
        }
        if doc.attribute_named(id, Name::HIDDEN).is_some() {
            return true;
        }
        if local == Name::INPUT
            && doc
                .attribute_named(id, Name::TYPE)
                .is_some_and(|t| t.eq_ignore_ascii_case("hidden"))
        {
            return true;
        }
    }
    doc.attribute_named(id, Name::STYLE)
        .is_some_and(style_hides)
}

/// Whether an inline style sets `display: none` or `visibility: hidden`.
pub(crate) fn style_hides(style: &str) -> bool {
    style.split(';').any(|decl| {
        let Some((prop, value)) = decl.split_once(':') else {
            return false;
        };
        let value = value.trim().trim_end_matches("!important").trim();
        let prop = prop.trim();
        (prop.eq_ignore_ascii_case("display") && value.eq_ignore_ascii_case("none"))
            || (prop.eq_ignore_ascii_case("visibility")
                && (value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(html: &str) -> String {
        let doc = Document::parse(html);
        inner_text(&doc, doc.body().unwrap())
    }

    #[test]
    fn collapses_whitespace_and_breaks_blocks() {
        assert_eq!(text("<p> a \n b </p><p>c</p>"), "a b\n\nc");
        assert_eq!(
            text("<div>one</div><div>two<br>three</div>"),
            "one\ntwo\nthree"
        );
        assert_eq!(text("<span>a</span> <span>b</span>"), "a b");
        assert_eq!(text("<ul><li>x</li><li>y</li></ul>"), "x\ny");
    }

    #[test]
    fn skips_hidden_content() {
        assert_eq!(
            text("a<script>x()</script><span hidden>b</span><i style='display:none'>c</i>d"),
            "ad"
        );
    }

    #[test]
    fn keeps_preformatted_text_and_separates_cells() {
        assert_eq!(text("<pre>a\n  b</pre>"), "a\n  b");
        assert_eq!(
            text("<table><tr><td>1</td><td>2</td></tr><tr><td>3</td></tr></table>"),
            "1\t2\n3"
        );
    }
}
