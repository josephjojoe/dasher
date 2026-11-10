//! `getElementById`, `getElementsByTagName` and `getElementsByClassName`.
//!
//! These search the descendants of a root in tree order. The `By...Name`
//! queries return lazy iterators: the JS bindings can re-run them to back a
//! live `HTMLCollection`, or collect them for a snapshot.

use crate::document::{Document, is_ascii_whitespace};
use crate::name::Name;
use crate::node::NodeId;

impl Document {
    /// The first element among the descendants of `root`, in tree order,
    /// whose `id` attribute is `id`. An empty `id` matches nothing.
    pub fn get_element_by_id(&self, root: NodeId, id: &str) -> Option<NodeId> {
        if id.is_empty() {
            return None;
        }
        self.descendants(root)
            .find(|&n| self.element_id(n) == Some(id))
    }

    /// Descendant elements of `root` with this qualified name, as in
    /// `getElementsByTagName`: `"*"` matches every element; HTML elements
    /// match the ASCII-lowercased name, other elements the name exactly.
    pub fn elements_by_tag_name<'a>(
        &'a self,
        root: NodeId,
        qualified_name: &str,
    ) -> impl Iterator<Item = NodeId> + 'a {
        let matcher = if qualified_name == "*" {
            None
        } else {
            Some((
                self.split_qualified_name(&qualified_name.to_ascii_lowercase()),
                self.split_qualified_name(qualified_name),
            ))
        };
        self.descendants(root).filter(move |&n| {
            let (Some(ns), Some(local)) = (self.namespace(n), self.local_name(n)) else {
                return false;
            };
            let Some((html, other)) = matcher else {
                return true;
            };
            let wanted = if ns == Name::NS_HTML { html } else { other };
            wanted.is_some_and(|(p, l)| l == local && Some(p) == self.prefix(n))
        })
    }

    /// The interned (prefix, local) of a qualified name, or `None` if either
    /// part was never interned (so no element can have it).
    fn split_qualified_name(&self, qualified_name: &str) -> Option<(Name, Name)> {
        let names = self.names();
        match qualified_name.split_once(':') {
            Some((p, l)) => Some((names.get(p)?, names.get(l)?)),
            None => Some((Name::EMPTY, names.get(qualified_name)?)),
        }
    }

    /// Descendant elements of `root` that have every class in the
    /// whitespace-separated `class_names`, as in `getElementsByClassName`.
    /// No classes match nothing. Matching ignores ASCII case in quirks mode.
    pub fn elements_by_class_name<'a>(
        &'a self,
        root: NodeId,
        class_names: &str,
    ) -> impl Iterator<Item = NodeId> + 'a {
        let wanted: Vec<String> = class_names
            .split(is_ascii_whitespace)
            .filter(|c| !c.is_empty())
            .map(str::to_owned)
            .collect();
        self.descendants(root).filter(move |&n| {
            !wanted.is_empty() && self.is_element(n) && wanted.iter().all(|c| self.has_class(n, c))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::parse(
            "<!DOCTYPE html><div id=a class='x y'><p id=b class=y>1</p>\
             <svg><foreignObject/></svg><p id=a class='Y x'>2</p></div><p class=x>3</p>",
        )
    }

    fn ids(doc: &Document, it: impl Iterator<Item = NodeId>) -> Vec<String> {
        it.map(|n| doc.element_id(n).unwrap_or("-").to_owned())
            .collect()
    }

    #[test]
    fn get_element_by_id_returns_the_first_match() {
        let doc = doc();
        let root = doc.root();
        let a = doc.get_element_by_id(root, "a").unwrap();
        assert!(doc.is_html(a, Name::DIV));
        let b = doc.get_element_by_id(root, "b").unwrap();
        assert_eq!(doc.get_element_by_id(a, "b"), Some(b));
        // Descendants only, never the root itself.
        assert_eq!(doc.get_element_by_id(b, "b"), None);
        assert_eq!(doc.get_element_by_id(root, ""), None);
        assert_eq!(doc.get_element_by_id(root, "missing"), None);
    }

    #[test]
    fn elements_by_tag_name() {
        let doc = doc();
        let root = doc.root();
        assert_eq!(
            ids(&doc, doc.elements_by_tag_name(root, "p")),
            ["b", "a", "-"]
        );
        // HTML elements match case-insensitively...
        assert_eq!(doc.elements_by_tag_name(root, "P").count(), 3);
        // ...foreign elements match exactly.
        assert_eq!(doc.elements_by_tag_name(root, "foreignObject").count(), 1);
        assert_eq!(doc.elements_by_tag_name(root, "foreignobject").count(), 0);
        assert_eq!(doc.elements_by_tag_name(root, "nope").count(), 0);
        // html, head, body, div, p, svg, foreignObject, p, p.
        assert_eq!(doc.elements_by_tag_name(root, "*").count(), 9);
        let div = doc.get_element_by_id(root, "a").unwrap();
        assert_eq!(doc.elements_by_tag_name(div, "p").count(), 2);
    }

    #[test]
    fn elements_by_tag_name_with_prefix() {
        let mut doc = Document::new();
        let root = doc.root();
        let el = doc.create_element_ns("urn:x", "x:thing");
        doc.append_child(root, el).unwrap();
        assert_eq!(doc.elements_by_tag_name(root, "x:thing").count(), 1);
        assert_eq!(doc.elements_by_tag_name(root, "thing").count(), 0);
    }

    #[test]
    fn elements_by_class_name() {
        let doc = doc();
        let root = doc.root();
        assert_eq!(ids(&doc, doc.elements_by_class_name(root, "y")), ["a", "b"]);
        assert_eq!(ids(&doc, doc.elements_by_class_name(root, " x\ty ")), ["a"]);
        assert_eq!(
            ids(&doc, doc.elements_by_class_name(root, "x")),
            ["a", "a", "-"]
        );
        assert_eq!(doc.elements_by_class_name(root, "").count(), 0);
        assert_eq!(doc.elements_by_class_name(root, "  ").count(), 0);
    }

    #[test]
    fn class_names_ignore_case_in_quirks_mode() {
        let doc = Document::parse("<p class=Foo></p>");
        assert_eq!(doc.elements_by_class_name(doc.root(), "foo").count(), 1);
    }
}
