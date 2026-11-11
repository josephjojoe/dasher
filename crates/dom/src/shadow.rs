//! Shadow roots: document fragments attached to a host element, holding a
//! tree of their own.
//!
//! As with template contents, a shadow root is not a child of its host: the
//! document keeps a map from each host to its root and back. The shadow
//! tree's nodes live in the same arena. A node in a shadow tree is
//! [connected](Document::is_connected) when its host is.

use crate::document::Document;
use crate::error::DomError;
use crate::name::Name;
use crate::node::{NodeData, NodeId};

/// Whether script outside the shadow tree can reach it through the host
/// (`element.shadowRoot`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadowRootMode {
    Open,
    Closed,
}

impl ShadowRootMode {
    /// The mode's name: `"open"` or `"closed"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ShadowRootMode::Open => "open",
            ShadowRootMode::Closed => "closed",
        }
    }
}

/// A shadow root's settings, as in `attachShadow`'s `ShadowRootInit`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ShadowRootInit {
    pub mode: ShadowRootMode,
    pub delegates_focus: bool,
    /// Whether cloning the host clones the shadow root too.
    pub clonable: bool,
    /// Whether `getHTML({ serializableShadowRoots: true })` includes it.
    pub serializable: bool,
}

impl ShadowRootInit {
    /// An open root with every flag unset.
    pub fn open() -> Self {
        Self {
            mode: ShadowRootMode::Open,
            delegates_focus: false,
            clonable: false,
            serializable: false,
        }
    }
}

/// What the document keeps for each shadow root.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShadowRootData {
    pub(crate) root: NodeId,
    pub(crate) init: ShadowRootInit,
    /// Made by the parser from a `<template shadowrootmode>`. Such a root
    /// may be taken over by a later `attachShadow` of the same mode.
    pub(crate) declarative: bool,
}

/// HTML elements other than custom elements that may host a shadow root.
const SHADOW_HOSTS: &[Name] = &[
    Name::ARTICLE,
    Name::ASIDE,
    Name::BLOCKQUOTE,
    Name::BODY,
    Name::DIV,
    Name::FOOTER,
    Name::H1,
    Name::H2,
    Name::H3,
    Name::H4,
    Name::H5,
    Name::H6,
    Name::HEADER,
    Name::MAIN,
    Name::NAV,
    Name::P,
    Name::SECTION,
    Name::SPAN,
];

/// Names that look like custom element names but are taken by SVG and
/// MathML.
const RESERVED_CUSTOM_NAMES: &[&str] = &[
    "annotation-xml",
    "color-profile",
    "font-face",
    "font-face-src",
    "font-face-uri",
    "font-face-format",
    "font-face-name",
    "missing-glyph",
];

/// Whether `name` is a valid custom element name: a lowercase ASCII letter,
/// then name characters including a hyphen, and not one of the reserved
/// names. <https://html.spec.whatwg.org/#valid-custom-element-name>
pub fn is_valid_custom_element_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && name.contains('-')
        && chars.all(|c| {
            matches!(c, '-' | '.' | '_' | '0'..='9' | 'a'..='z' | '\u{B7}')
                || matches!(c as u32,
                    0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x37D | 0x37F..=0x1FFF
                    | 0x200C..=0x200D | 0x203F..=0x2040 | 0x2070..=0x218F
                    | 0x2C00..=0x2FEF | 0x3001..=0xD7FF | 0xF900..=0xFDCF
                    | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
        })
        && !RESERVED_CUSTOM_NAMES.contains(&name)
}

impl Document {
    /// Whether `id` is an element that may host a shadow root: an HTML
    /// element that is a valid custom element name or one of the few
    /// standard elements the DOM standard allows.
    pub fn can_host_shadow_root(&self, id: NodeId) -> bool {
        match &self.node(id).data {
            NodeData::Element(e) if e.ns == Name::NS_HTML => {
                SHADOW_HOSTS.contains(&e.local)
                    || is_valid_custom_element_name(self.name_str(e.local))
            }
            _ => false,
        }
    }

    /// `element.attachShadow(init)`: gives `host` a new, empty shadow root
    /// and returns it.
    ///
    /// Fails with [`DomError::NotSupported`] if `host` cannot host one or
    /// already does. As the HTML standard says, a root the parser made
    /// from declarative markup is the exception: attaching one of the same
    /// mode empties that root and returns it.
    pub fn attach_shadow(
        &mut self,
        host: NodeId,
        init: ShadowRootInit,
    ) -> Result<NodeId, DomError> {
        if !self.can_host_shadow_root(host) {
            return Err(DomError::NotSupported);
        }
        if let Some(existing) = self.shadow_roots.get_mut(&host) {
            if !existing.declarative || existing.init.mode != init.mode {
                return Err(DomError::NotSupported);
            }
            existing.declarative = false;
            let root = existing.root;
            self.remove_children(root);
            return Ok(root);
        }
        Ok(self.attach_shadow_unchecked(host, init, false))
    }

    pub(crate) fn attach_shadow_unchecked(
        &mut self,
        host: NodeId,
        init: ShadowRootInit,
        declarative: bool,
    ) -> NodeId {
        let root = self.create_fragment();
        self.shadow_roots.insert(
            host,
            ShadowRootData {
                root,
                init,
                declarative,
            },
        );
        self.shadow_hosts.insert(root, host);
        root
    }

    /// The shadow root `host` hosts, open or closed.
    pub fn shadow_root(&self, host: NodeId) -> Option<NodeId> {
        self.shadow_roots.get(&host).map(|s| s.root)
    }

    /// The host of the shadow root `root`; `None` if `root` is not a
    /// shadow root.
    pub fn shadow_host(&self, root: NodeId) -> Option<NodeId> {
        self.shadow_hosts.get(&root).copied()
    }

    /// The settings of the shadow root `root`.
    pub fn shadow_root_init(&self, root: NodeId) -> Option<ShadowRootInit> {
        let host = self.shadow_host(root)?;
        Some(self.shadow_roots[&host].init)
    }

    /// Whether the document has any shadow roots.
    pub fn has_shadow_roots(&self) -> bool {
        !self.shadow_roots.is_empty()
    }

    /// The root of `id`'s tree, then of its host's tree, and so on, until a
    /// root that is not a shadow root (the DOM's "shadow-including root").
    pub fn shadow_including_root(&self, id: NodeId) -> NodeId {
        let mut root = self.tree_root(id);
        while let Some(host) = self.shadow_host(root) {
            root = self.tree_root(host);
        }
        root
    }

    /// The slot `node` is assigned to by name: the first `<slot>` in its
    /// parent's shadow tree whose `name` matches `node`'s `slot` attribute
    /// (the default slot, for text and for elements without one). `None`
    /// if `node`'s parent hosts no shadow root, or no slot matches.
    pub fn assigned_slot(&self, node: NodeId) -> Option<NodeId> {
        let wanted = match &self.node(node).data {
            NodeData::Element(_) => self.attribute_named(node, Name::SLOT).unwrap_or(""),
            NodeData::Text(_) => "",
            _ => return None,
        };
        let root = self.shadow_root(self.parent(node)?)?;
        self.descendants(root).find(|&n| {
            self.is_html(n, Name::SLOT)
                && self.attribute_named(n, Name::NAME).unwrap_or("") == wanted
        })
    }

    /// The nodes assigned to `slot` by name: the children of its shadow
    /// tree's host whose [assigned slot](Self::assigned_slot) it is. Empty
    /// if `slot` is not in a shadow tree.
    pub fn assigned_nodes(&self, slot: NodeId) -> Vec<NodeId> {
        let Some(host) = self.shadow_host(self.tree_root(slot)) else {
            return Vec::new();
        };
        self.children(host)
            .filter(|&c| self.assigned_slot(c) == Some(slot))
            .collect()
    }

    /// `id` and everything under it, also going into shadow trees, in the
    /// DOM's shadow-including tree order: a host is followed by its shadow
    /// root and the shadow tree, then by its children. Template contents are
    /// not included.
    pub fn shadow_including_inclusive_descendants(&self, id: NodeId) -> ShadowIncluding<'_> {
        ShadowIncluding {
            doc: self,
            stack: vec![id],
        }
    }
}

/// See [`Document::shadow_including_inclusive_descendants`].
pub struct ShadowIncluding<'a> {
    doc: &'a Document,
    /// Nodes still to visit, the next one last.
    stack: Vec<NodeId>,
}

impl Iterator for ShadowIncluding<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let n = self.stack.pop()?;
        let doc = self.doc;
        let mut child = doc.last_child(n);
        while let Some(c) = child {
            self.stack.push(c);
            child = doc.prev_sibling(c);
        }
        if !doc.shadow_roots.is_empty() {
            if let Some(root) = doc.shadow_root(n) {
                self.stack.push(root);
            }
        }
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_element_names() {
        for ok in ["my-el", "x-1", "a-", "math-α", "foo.bar-baz"] {
            assert!(is_valid_custom_element_name(ok), "{ok}");
        }
        for bad in [
            "div",
            "My-el",
            "-x",
            "1-x",
            "font-face",
            "my el",
            "my-El",
            "",
        ] {
            assert!(!is_valid_custom_element_name(bad), "{bad}");
        }
    }

    #[test]
    fn attach_and_find_shadow_roots() {
        let mut doc = Document::parse("<div id=a></div><my-el></my-el><img>");
        let body = doc.body().unwrap();
        let div = doc.first_child(body).unwrap();
        let el = doc.next_sibling(div).unwrap();
        let img = doc.next_sibling(el).unwrap();
        let root = doc.attach_shadow(div, ShadowRootInit::open()).unwrap();
        assert_eq!(doc.shadow_root(div), Some(root));
        assert_eq!(doc.shadow_host(root), Some(div));
        assert_eq!(
            doc.attach_shadow(div, ShadowRootInit::open()),
            Err(DomError::NotSupported)
        );
        assert_eq!(
            doc.attach_shadow(img, ShadowRootInit::open()),
            Err(DomError::NotSupported)
        );
        assert!(doc.attach_shadow(el, ShadowRootInit::open()).is_ok());

        let span = doc.create_element("span");
        doc.append_child(root, span).unwrap();
        assert!(doc.is_connected(span));
        assert_eq!(doc.shadow_including_root(span), doc.root());
        doc.remove(div);
        assert!(!doc.is_connected(span));
        assert_eq!(doc.shadow_including_root(span), div);
    }

    #[test]
    fn shadow_including_order_visits_the_shadow_tree_before_children() {
        let mut doc = Document::parse("<div><b></b></div>");
        let div = doc.first_child(doc.body().unwrap()).unwrap();
        let root = doc.attach_shadow(div, ShadowRootInit::open()).unwrap();
        let i = doc.create_element("i");
        doc.append_child(root, i).unwrap();
        let names: Vec<String> = doc
            .shadow_including_inclusive_descendants(div)
            .map(|n| doc.node_name(n).into_owned())
            .collect();
        assert_eq!(names, ["DIV", "#document-fragment", "I", "B"]);
    }

    #[test]
    fn slots_are_assigned_by_name() {
        let mut doc = Document::parse(
            "<my-el><template shadowrootmode=open><slot name=a></slot><slot></slot>\
             <slot name=a></slot></template><b slot=a>1</b>t<i>2</i><u slot=x>3</u><!--c--></my-el>",
        );
        let host = doc.first_child(doc.body().unwrap()).unwrap();
        let root = doc.shadow_root(host).unwrap();
        let slots: Vec<NodeId> = doc.children(root).collect();
        let kids: Vec<NodeId> = doc.children(host).collect();
        assert_eq!(doc.assigned_nodes(slots[0]), [kids[0]]);
        assert_eq!(doc.assigned_nodes(slots[1]), [kids[1], kids[2]]);
        assert_eq!(doc.assigned_nodes(slots[2]), []);
        assert_eq!(doc.assigned_slot(kids[3]), None);
        assert_eq!(doc.assigned_slot(kids[4]), None);
        let loose = doc.create_element("slot");
        assert_eq!(doc.assigned_nodes(loose), []);
    }

    #[test]
    fn a_host_cannot_go_into_its_own_shadow_tree() {
        let mut doc = Document::parse("<div></div>");
        let div = doc.first_child(doc.body().unwrap()).unwrap();
        let root = doc.attach_shadow(div, ShadowRootInit::open()).unwrap();
        assert_eq!(doc.append_child(root, div), Err(DomError::HierarchyRequest));
    }

    #[test]
    fn releasing_a_host_releases_its_shadow_tree() {
        let mut doc = Document::parse("");
        let before = doc.len();
        let div = doc.create_element("div");
        let root = doc.attach_shadow(div, ShadowRootInit::open()).unwrap();
        let t = doc.create_text("x");
        doc.append_child(root, t).unwrap();
        doc.release(div);
        assert_eq!(doc.len(), before);
        assert!(!doc.has_shadow_roots());
    }

    #[test]
    fn clonable_shadow_roots_are_cloned() {
        let mut doc = Document::parse("");
        let a = doc.create_element("div");
        let b = doc.create_element("div");
        let init = ShadowRootInit {
            clonable: true,
            ..ShadowRootInit::open()
        };
        let ra = doc.attach_shadow(a, init).unwrap();
        doc.attach_shadow(b, ShadowRootInit::open()).unwrap();
        let t = doc.create_text("x");
        doc.append_child(ra, t).unwrap();

        let copy = doc.clone_node(a, false);
        let rc = doc.shadow_root(copy).expect("clonable root is cloned");
        assert_ne!(rc, ra);
        assert_eq!(doc.shadow_root_init(rc), Some(init));
        assert_eq!(doc.text_content(rc).as_deref(), Some("x"));

        let copy = doc.clone_node(b, true);
        assert_eq!(doc.shadow_root(copy), None);
    }

    #[test]
    fn declarative_roots_can_be_taken_over_once() {
        let mut doc = Document::parse(
            "<my-el><template shadowrootmode=open><b>hi</b></template>light</my-el>",
        );
        let el = doc.first_child(doc.body().unwrap()).unwrap();
        let root = doc.shadow_root(el).unwrap();
        assert_eq!(doc.inner_html(root), "<b>hi</b>");
        assert_eq!(
            doc.attach_shadow(
                el,
                ShadowRootInit {
                    mode: ShadowRootMode::Closed,
                    ..ShadowRootInit::open()
                }
            ),
            Err(DomError::NotSupported)
        );
        assert_eq!(doc.attach_shadow(el, ShadowRootInit::open()), Ok(root));
        assert_eq!(doc.first_child(root), None);
        assert!(doc.attach_shadow(el, ShadowRootInit::open()).is_err());
    }
}
