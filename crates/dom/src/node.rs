//! Node storage: ids, kinds and the per-node records kept in the arena.

use std::fmt;
use std::num::NonZeroU32;

use crate::name::Name;

/// A node in a [`Document`](crate::Document), addressed by its arena slot.
///
/// Ids are plain `Copy` values and only mean something to the document that
/// created them. An id stays valid for the life of the document unless the
/// node is explicitly [released](crate::Document::release).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(NonZeroU32);

impl NodeId {
    pub(crate) fn from_index(index: usize) -> Self {
        let raw = u32::try_from(index + 1).expect("more than u32::MAX - 1 nodes");
        Self(NonZeroU32::new(raw).expect("index + 1 is non-zero"))
    }

    pub(crate) fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }

    /// The id as a non-zero integer, e.g. to stash in a JS object.
    pub fn to_raw(self) -> u32 {
        self.0.get()
    }

    /// Rebuilds an id from [`to_raw`](Self::to_raw). Returns `None` for zero.
    pub fn from_raw(raw: u32) -> Option<Self> {
        NonZeroU32::new(raw).map(Self)
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.0)
    }
}

/// The type of a node, as in the DOM's `Node.nodeType`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NodeKind {
    Element,
    Text,
    Comment,
    Document,
    Doctype,
    DocumentFragment,
}

impl NodeKind {
    /// The numeric `Node.nodeType` value.
    pub fn node_type(self) -> u16 {
        match self {
            NodeKind::Element => 1,
            NodeKind::Text => 3,
            NodeKind::Comment => 8,
            NodeKind::Document => 9,
            NodeKind::Doctype => 10,
            NodeKind::DocumentFragment => 11,
        }
    }
}

/// An attribute: a namespaced name and a value.
///
/// Attributes set from HTML or with `setAttribute` have no namespace and no
/// prefix ([`Name::EMPTY`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Attr {
    pub(crate) local: Name,
    pub(crate) ns: Name,
    pub(crate) prefix: Name,
    pub(crate) value: Box<str>,
}

impl Attr {
    /// The local name.
    pub fn local(&self) -> Name {
        self.local
    }

    /// The namespace, or [`Name::EMPTY`].
    pub fn ns(&self) -> Name {
        self.ns
    }

    /// The prefix, or [`Name::EMPTY`].
    pub fn prefix(&self) -> Name {
        self.prefix
    }

    /// The value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A doctype's name and identifiers.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Doctype {
    pub name: Box<str>,
    pub public_id: Box<str>,
    pub system_id: Box<str>,
}

/// Per-node boolean state, packed into what would otherwise be padding in
/// [`Node`].
pub(crate) mod flags {
    /// The script element's "already started" flag: it must not run.
    pub const SCRIPT_ALREADY_STARTED: u8 = 1 << 0;
    /// The element has a prefix, stored out of line.
    pub const HAS_PREFIX: u8 = 1 << 2;
}

#[derive(Clone, Debug)]
pub(crate) struct Element {
    pub(crate) local: Name,
    pub(crate) ns: Name,
    pub(crate) attrs: Box<[Attr]>,
}

#[derive(Clone, Debug)]
pub(crate) enum NodeData {
    Document,
    Doctype(Box<Doctype>),
    Element(Element),
    Text(String),
    Comment(String),
    Fragment,
    /// A released slot, waiting on the free list.
    Free,
}

/// One arena slot: tree links plus the node's payload.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) parent: Option<NodeId>,
    pub(crate) first_child: Option<NodeId>,
    pub(crate) last_child: Option<NodeId>,
    pub(crate) prev_sibling: Option<NodeId>,
    pub(crate) next_sibling: Option<NodeId>,
    /// See [`flags`].
    pub(crate) flags: u8,
    pub(crate) data: NodeData,
}

impl Node {
    pub(crate) fn new(data: NodeData) -> Self {
        Node {
            parent: None,
            first_child: None,
            last_child: None,
            prev_sibling: None,
            next_sibling: None,
            flags: 0,
            data,
        }
    }

    pub(crate) fn kind(&self) -> NodeKind {
        match self.data {
            NodeData::Document => NodeKind::Document,
            NodeData::Doctype(_) => NodeKind::Doctype,
            NodeData::Element(_) => NodeKind::Element,
            NodeData::Text(_) => NodeKind::Text,
            NodeData::Comment(_) => NodeKind::Comment,
            NodeData::Fragment => NodeKind::DocumentFragment,
            NodeData::Free => panic!("use of a released node"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip_through_raw() {
        let id = NodeId::from_index(41);
        assert_eq!(id.index(), 41);
        assert_eq!(id.to_raw(), 42);
        assert_eq!(NodeId::from_raw(42), Some(id));
        assert_eq!(NodeId::from_raw(0), None);
    }

    #[test]
    fn records_stay_compact() {
        // The per-node budget. See also tests/memory.rs, which measures the
        // heap cost of real trees.
        assert_eq!(size_of::<Option<NodeId>>(), 4);
        assert!(size_of::<NodeData>() <= 32, "{}", size_of::<NodeData>());
        assert!(size_of::<Node>() <= 56, "{}", size_of::<Node>());
        assert!(size_of::<Attr>() <= 32, "{}", size_of::<Attr>());
    }
}
