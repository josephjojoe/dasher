//! Live collections: `childNodes`, `children`, `getElementsBy*` and the
//! document's `forms`, `images`, `links` and the like.
//!
//! The list objects are in the `lists` module; what they hold is here. A
//! list holds its root's wrapper and caches the matching node ids,
//! recomputed when the DOM version changes. Holding the root keeps the
//! whole tree, and so every cached id, from being released.

use std::cell::RefCell;
use std::rc::Rc;

use dasher_css::SelectorList;
use dasher_dom::{Document, Name, NodeId};

/// Which nodes a collection holds.
pub(crate) enum Filter {
    ChildNodes,
    Children,
    /// `getElementsByTagName`.
    Tag(String),
    /// `getElementsByTagNameNS`: namespace (`None` for no namespace) and
    /// local name, either of which may be `*`.
    TagNs(Option<String>, String),
    /// `getElementsByClassName`.
    Class(String),
    /// `getElementsByName`.
    Name(String),
    /// Elements under the root matching a selector, which may use `:scope`.
    Selector(Rc<SelectorList>),
}

impl Filter {
    fn collect(&self, doc: &Document, root: NodeId) -> Vec<NodeId> {
        match self {
            Filter::ChildNodes => doc.children(root).collect(),
            Filter::Children => doc.element_children(root).collect(),
            Filter::Tag(name) => doc.elements_by_tag_name(root, name).collect(),
            Filter::TagNs(ns, local) => doc
                .descendants(root)
                .filter(|&n| {
                    let (Some(n_ns), Some(n_local)) = (doc.namespace(n), doc.local_name(n)) else {
                        return false;
                    };
                    let ns_ok = match ns.as_deref() {
                        Some("*") => true,
                        Some(ns) => doc.name_str(n_ns) == ns,
                        None => n_ns == Name::EMPTY,
                    };
                    ns_ok && (local == "*" || doc.name_str(n_local) == local)
                })
                .collect(),
            Filter::Class(names) => doc.elements_by_class_name(root, names).collect(),
            Filter::Name(name) => doc
                .descendants(root)
                .filter(|&n| {
                    doc.namespace(n) == Some(Name::NS_HTML)
                        && doc.attribute_named(n, Name::NAME) == Some(name.as_str())
                })
                .collect(),
            Filter::Selector(list) => list.query_all(doc, root).collect(),
        }
    }
}

/// What a live list holds: the nodes its filter picks under its root,
/// cached and recomputed when the DOM version changes.
pub(crate) struct Matches {
    filter: Filter,
    /// The DOM version the ids were computed at, and the ids.
    cache: RefCell<Option<(u32, Rc<[NodeId]>)>>,
}

impl Matches {
    pub(crate) fn new(filter: Filter) -> Self {
        Self {
            filter,
            cache: RefCell::new(None),
        }
    }

    /// The matching nodes under `root` as of `version`.
    pub(crate) fn ids(&self, doc: &Document, root: NodeId, version: u32) -> Rc<[NodeId]> {
        if let Some((v, ids)) = &*self.cache.borrow()
            && *v == version
        {
            return Rc::clone(ids);
        }
        let ids: Rc<[NodeId]> = self.filter.collect(doc, root).into();
        *self.cache.borrow_mut() = Some((version, Rc::clone(&ids)));
        ids
    }
}
