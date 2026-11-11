//! The document arena: node creation, navigation, mutation and cloning.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use crate::error::DomError;
use crate::name::{Name, Names};
use compact_str::CompactString;

use crate::node::{Attr, AttrNs, Doctype, Element, Node, NodeData, NodeId, NodeKind, flags};

/// The document's quirks mode, set by the parser from the doctype.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum QuirksMode {
    #[default]
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

/// An HTML document: an arena of nodes plus the names they use.
///
/// Every node, attached or not, lives in the document's arena and is
/// addressed by a [`NodeId`]. The tree is held together by parent, child and
/// sibling links between ids, so there is no per-node reference counting and
/// dropping the `Document` frees everything at once.
///
/// Nodes removed from the tree stay allocated (script may still hold them)
/// until they are explicitly [released](Self::release).
pub struct Document {
    nodes: Vec<Node>,
    free_head: Option<NodeId>,
    free_len: usize,
    names: Names,
    quirks_mode: QuirksMode,
    scripting: bool,
    /// `<template>` element -> its contents fragment.
    template_contents: HashMap<NodeId, NodeId>,
    /// Contents fragment -> its `<template>` element.
    template_hosts: HashMap<NodeId, NodeId>,
    /// Element prefixes; only `createElementNS` with a prefix makes these.
    prefixes: HashMap<NodeId, Name>,
    /// The (namespace, prefix) pairs attributes use, indexed by [`AttrNs`].
    /// Entry 0 is "no namespace"; a document rarely has more than a few.
    attr_namespaces: Vec<(Name, Name)>,
    /// See [`set_node_limit`](Self::set_node_limit); `usize::MAX` for none.
    node_limit: usize,
    /// Whether the document has held more than `node_limit` nodes.
    over_node_limit: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("nodes", &self.len())
            .field("quirks_mode", &self.quirks_mode)
            .finish_non_exhaustive()
    }
}

impl Document {
    /// An empty document: just the document node, with no children.
    pub fn new() -> Self {
        Document {
            nodes: vec![Node::new(NodeData::Document)],
            free_head: None,
            free_len: 0,
            names: Names::new(),
            quirks_mode: QuirksMode::NoQuirks,
            scripting: true,
            template_contents: HashMap::new(),
            template_hosts: HashMap::new(),
            prefixes: HashMap::new(),
            attr_namespaces: vec![(Name::EMPTY, Name::EMPTY)],
            node_limit: usize::MAX,
            over_node_limit: false,
        }
    }

    /// The document node.
    pub fn root(&self) -> NodeId {
        NodeId::from_index(0)
    }

    /// The number of live nodes in the arena, attached or not.
    pub fn len(&self) -> usize {
        self.nodes.len() - self.free_len
    }

    /// Whether the document holds only its document node.
    pub fn is_empty(&self) -> bool {
        self.len() == 1
    }

    /// The number of arena slots, including released ones awaiting reuse.
    pub fn capacity(&self) -> usize {
        self.nodes.len()
    }

    /// Caps the number of live nodes (see [`len`](Self::len)) the document
    /// should hold, or lifts the cap with `None`.
    ///
    /// The DOM API cannot fail where nodes are created, so going over the
    /// limit does not stop the node being made. Instead the document
    /// remembers it: [`node_limit_exceeded`](Self::node_limit_exceeded)
    /// stays true from then on, and whoever drives the document (the page)
    /// is expected to stop. Bulk operations stop early once over: fragment
    /// parsing (`innerHTML` and friends) feeds its input in 16 KiB slices
    /// and leaves the rest unparsed, and a deep clone stops copying. So one
    /// operation overshoots by a bounded amount, not by whatever its input
    /// would have built.
    pub fn set_node_limit(&mut self, limit: Option<usize>) {
        self.node_limit = limit.unwrap_or(usize::MAX);
        self.over_node_limit |= self.len() > self.node_limit;
    }

    /// The cap set with [`set_node_limit`](Self::set_node_limit).
    pub fn node_limit(&self) -> Option<usize> {
        (self.node_limit != usize::MAX).then_some(self.node_limit)
    }

    /// Whether the document has held more nodes than its
    /// [limit](Self::set_node_limit) allows, at any point.
    pub fn node_limit_exceeded(&self) -> bool {
        self.over_node_limit
    }

    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    /// Whether scripting is enabled. This decides how `<noscript>` is parsed
    /// and serialized; dasher runs scripts, so it defaults to `true`.
    pub fn scripting_enabled(&self) -> bool {
        self.scripting
    }

    pub fn set_scripting_enabled(&mut self, enabled: bool) {
        self.scripting = enabled;
    }

    /// The document's name table.
    pub fn names(&self) -> &Names {
        &self.names
    }

    /// Interns `s` in the document's name table.
    pub fn intern(&mut self, s: &str) -> Name {
        self.names.intern(s)
    }

    /// The string for `name`.
    pub fn name_str(&self, name: Name) -> &str {
        self.names.resolve(name)
    }

    // ---------------------------------------------------------------------
    // Arena plumbing.

    pub(crate) fn node(&self, id: NodeId) -> &Node {
        let node = &self.nodes[id.index()];
        assert!(
            !matches!(node.data, NodeData::Free),
            "use of a released node"
        );
        node
    }

    pub(crate) fn node_mut(&mut self, id: NodeId) -> &mut Node {
        let node = &mut self.nodes[id.index()];
        assert!(
            !matches!(node.data, NodeData::Free),
            "use of a released node"
        );
        node
    }

    fn alloc(&mut self, data: NodeData) -> NodeId {
        if self.len() >= self.node_limit {
            self.over_node_limit = true;
        }
        match self.free_head {
            Some(id) => {
                let slot = &mut self.nodes[id.index()];
                self.free_head = slot.next_sibling;
                self.free_len -= 1;
                *slot = Node::new(data);
                id
            }
            None => {
                let id = NodeId::from_index(self.nodes.len());
                self.nodes.push(Node::new(data));
                id
            }
        }
    }

    /// Gives back spare arena capacity, e.g. once parsing is done.
    pub fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
    }

    /// Frees a detached subtree: `id`, its descendants and any template
    /// contents inside it. Their slots are reused by later allocations.
    ///
    /// The caller must know that nothing still refers to these ids (for the
    /// JS bindings: no live wrapper object). Using a released id panics,
    /// unless its slot has been reused, in which case it silently names the
    /// new node.
    ///
    /// # Panics
    ///
    /// If `id` is the document node, still has a parent, or is the contents
    /// of a template.
    pub fn release(&mut self, id: NodeId) {
        assert!(id != self.root(), "cannot release the document node");
        assert!(self.parent(id).is_none(), "cannot release an attached node");
        assert!(
            !self.template_hosts.contains_key(&id),
            "cannot release template contents separately from the template"
        );
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            stack.extend(self.children(n));
            if let Some(contents) = self.template_contents.remove(&n) {
                self.template_hosts.remove(&contents);
                stack.push(contents);
            }
            if self.node(n).flags & flags::HAS_PREFIX != 0 {
                self.prefixes.remove(&n);
            }
            let slot = &mut self.nodes[n.index()];
            *slot = Node::new(NodeData::Free);
            slot.next_sibling = self.free_head;
            self.free_head = Some(n);
            self.free_len += 1;
        }
    }

    // ---------------------------------------------------------------------
    // Creation.

    /// A new element in the HTML namespace, e.g. `create_element("div")`.
    pub fn create_element(&mut self, local_name: &str) -> NodeId {
        let local = self.intern(local_name);
        self.create_element_named(Name::NS_HTML, local)
    }

    /// A new element with a namespace and qualified name, as in
    /// `createElementNS`. `qualified_name` may carry a `prefix:`. Validating
    /// the names is up to the caller.
    pub fn create_element_ns(&mut self, ns: &str, qualified_name: &str) -> NodeId {
        let ns = self.intern(ns);
        let (prefix, local) = match qualified_name.split_once(':') {
            Some((p, l)) => (self.intern(p), self.intern(l)),
            None => (Name::EMPTY, self.intern(qualified_name)),
        };
        let id = self.create_element_named(ns, local);
        if prefix != Name::EMPTY {
            self.prefixes.insert(id, prefix);
            self.node_mut(id).flags |= flags::HAS_PREFIX;
        }
        id
    }

    /// A new element from already-interned names.
    pub fn create_element_named(&mut self, ns: Name, local: Name) -> NodeId {
        self.create_element_with_attrs(ns, local, Box::default())
    }

    pub(crate) fn create_element_with_attrs(
        &mut self,
        ns: Name,
        local: Name,
        attrs: Box<[Attr]>,
    ) -> NodeId {
        let id = self.alloc(NodeData::Element(Element { local, ns, attrs }));
        if ns == Name::NS_HTML && local == Name::TEMPLATE {
            let contents = self.alloc(NodeData::Fragment);
            self.template_contents.insert(id, contents);
            self.template_hosts.insert(contents, id);
        }
        id
    }

    /// A new text node.
    pub fn create_text(&mut self, data: &str) -> NodeId {
        self.alloc(NodeData::Text(CompactString::from(data)))
    }

    /// A new comment node.
    pub fn create_comment(&mut self, data: &str) -> NodeId {
        self.alloc(NodeData::Comment(CompactString::from(data)))
    }

    /// A new, empty document fragment.
    pub fn create_fragment(&mut self) -> NodeId {
        self.alloc(NodeData::Fragment)
    }

    /// A new doctype node.
    pub fn create_doctype(&mut self, name: &str, public_id: &str, system_id: &str) -> NodeId {
        self.alloc(NodeData::Doctype(Box::new(Doctype {
            name: name.into(),
            public_id: public_id.into(),
            system_id: system_id.into(),
        })))
    }

    /// A new, empty document node: the root of a tree of its own in this
    /// arena, apart from [`root`](Self::root). Its nodes are never
    /// [connected](Self::is_connected), and the document-level queries
    /// ([`head`](Self::head), [`body`](Self::body), [`title`](Self::title)
    /// and so on) do not look at it. The JS bindings use this for
    /// `document.implementation.createHTMLDocument()`.
    pub fn create_document(&mut self) -> NodeId {
        self.alloc(NodeData::Document)
    }

    // ---------------------------------------------------------------------
    // Node data.

    pub fn kind(&self, id: NodeId) -> NodeKind {
        self.node(id).kind()
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Element(_))
    }

    pub fn is_text(&self, id: NodeId) -> bool {
        matches!(self.node(id).data, NodeData::Text(_))
    }

    fn element(&self, id: NodeId) -> Option<&Element> {
        match &self.node(id).data {
            NodeData::Element(e) => Some(e),
            _ => None,
        }
    }

    fn element_mut(&mut self, id: NodeId) -> &mut Element {
        match &mut self.node_mut(id).data {
            NodeData::Element(e) => e,
            _ => panic!("not an element"),
        }
    }

    pub(crate) fn has_flag(&self, id: NodeId, flag: u8) -> bool {
        self.node(id).flags & flag != 0
    }

    pub(crate) fn set_flag(&mut self, id: NodeId, flag: u8) {
        self.node_mut(id).flags |= flag;
    }

    /// An element's local name, or `None` for other nodes.
    pub fn local_name(&self, id: NodeId) -> Option<Name> {
        self.element(id).map(|e| e.local)
    }

    /// An element's namespace, or `None` for other nodes.
    pub fn namespace(&self, id: NodeId) -> Option<Name> {
        self.element(id).map(|e| e.ns)
    }

    /// An element's prefix ([`Name::EMPTY`] if it has none), or `None` for
    /// other nodes.
    pub fn prefix(&self, id: NodeId) -> Option<Name> {
        self.element(id)?;
        if self.has_flag(id, flags::HAS_PREFIX) {
            Some(self.prefixes[&id])
        } else {
            Some(Name::EMPTY)
        }
    }

    /// Whether `id` is an element with this namespace and local name.
    pub fn is_element_named(&self, id: NodeId, ns: Name, local: Name) -> bool {
        self.element(id)
            .is_some_and(|e| e.ns == ns && e.local == local)
    }

    /// Whether `id` is an HTML element with this local name.
    pub fn is_html(&self, id: NodeId, local: Name) -> bool {
        self.is_element_named(id, Name::NS_HTML, local)
    }

    /// An element's qualified name (`prefix:local`, or just `local`).
    pub fn qualified_name(&self, id: NodeId) -> Option<Cow<'_, str>> {
        let local = self.name_str(self.local_name(id)?);
        Some(match self.prefix(id) {
            Some(Name::EMPTY) | None => Cow::Borrowed(local),
            Some(p) => Cow::Owned(format!("{}:{local}", self.name_str(p))),
        })
    }

    /// `Element.tagName`: the qualified name, upper-cased for HTML elements.
    pub fn tag_name(&self, id: NodeId) -> Option<Cow<'_, str>> {
        let qname = self.qualified_name(id)?;
        if self.namespace(id) == Some(Name::NS_HTML)
            && qname.bytes().any(|b| b.is_ascii_lowercase())
        {
            Some(Cow::Owned(qname.to_ascii_uppercase()))
        } else {
            Some(qname)
        }
    }

    /// `Node.nodeName`.
    pub fn node_name(&self, id: NodeId) -> Cow<'_, str> {
        match &self.node(id).data {
            NodeData::Element(_) => self.tag_name(id).expect("is an element"),
            NodeData::Text(_) => Cow::Borrowed("#text"),
            NodeData::Comment(_) => Cow::Borrowed("#comment"),
            NodeData::Document => Cow::Borrowed("#document"),
            NodeData::Doctype(d) => Cow::Borrowed(&d.name),
            NodeData::Fragment => Cow::Borrowed("#document-fragment"),
            NodeData::Free => unreachable!(),
        }
    }

    /// A doctype node's name and identifiers.
    pub fn doctype(&self, id: NodeId) -> Option<&Doctype> {
        match &self.node(id).data {
            NodeData::Doctype(d) => Some(d),
            _ => None,
        }
    }

    /// The data of a text or comment node.
    pub fn text(&self, id: NodeId) -> Option<&str> {
        match &self.node(id).data {
            NodeData::Text(s) | NodeData::Comment(s) => Some(s),
            _ => None,
        }
    }

    /// Replaces the data of a text or comment node. Does nothing to other
    /// nodes.
    pub fn set_text(&mut self, id: NodeId, data: &str) {
        if let NodeData::Text(s) | NodeData::Comment(s) = &mut self.node_mut(id).data {
            s.clear();
            s.push_str(data);
            s.shrink_to_fit();
        }
    }

    pub(crate) fn text_mut(&mut self, id: NodeId) -> Option<&mut CompactString> {
        match &mut self.node_mut(id).data {
            NodeData::Text(s) | NodeData::Comment(s) => Some(s),
            _ => None,
        }
    }

    /// The contents fragment of a `<template>` element.
    pub fn template_contents(&self, id: NodeId) -> Option<NodeId> {
        self.template_contents.get(&id).copied()
    }

    /// The `<template>` element whose contents fragment is `id`.
    pub fn template_host(&self, id: NodeId) -> Option<NodeId> {
        self.template_hosts.get(&id).copied()
    }

    /// A script element's "already started" flag. Scripts inserted through
    /// `innerHTML` and similar fragment parsing have it set and never run.
    pub fn script_already_started(&self, id: NodeId) -> bool {
        self.has_flag(id, flags::SCRIPT_ALREADY_STARTED)
    }

    pub fn set_script_already_started(&mut self, id: NodeId) {
        self.set_flag(id, flags::SCRIPT_ALREADY_STARTED);
    }

    // ---------------------------------------------------------------------
    // Attributes.

    /// An element's attributes in order; empty for other nodes.
    pub fn attrs(&self, id: NodeId) -> &[Attr] {
        self.element(id).map_or(&[], |e| &e.attrs)
    }

    /// The namespace of an attribute of one of this document's elements.
    pub fn attr_namespace(&self, attr: &Attr) -> Name {
        self.attr_namespaces[attr.ns.index()].0
    }

    /// The prefix of an attribute of one of this document's elements, or
    /// [`Name::EMPTY`].
    pub fn attr_prefix(&self, attr: &Attr) -> Name {
        self.attr_namespaces[attr.ns.index()].1
    }

    fn attr_ns_id(&mut self, ns: Name, prefix: Name) -> AttrNs {
        let pair = (ns, prefix);
        match self.attr_namespaces.iter().position(|&p| p == pair) {
            Some(i) => AttrNs::from_index(i),
            None => {
                self.attr_namespaces.push(pair);
                AttrNs::from_index(self.attr_namespaces.len() - 1)
            }
        }
    }

    fn attr_index(&self, id: NodeId, qualified_name: &str) -> Option<usize> {
        let attrs = self.attrs(id);
        match qualified_name.split_once(':') {
            None => {
                let local = self.names.get(qualified_name)?;
                attrs
                    .iter()
                    .position(|a| a.local == local && self.attr_prefix(a) == Name::EMPTY)
            }
            Some((prefix, local)) => {
                let prefix = self.names.get(prefix)?;
                let local = self.names.get(local)?;
                attrs
                    .iter()
                    .position(|a| a.local == local && self.attr_prefix(a) == prefix)
            }
        }
    }

    fn attr_index_ns(&self, id: NodeId, ns: Name, local: Name) -> Option<usize> {
        self.attrs(id)
            .iter()
            .position(|a| a.local == local && self.attr_namespace(a) == ns)
    }

    /// The value of the first attribute with this qualified name, as in
    /// `getAttribute`. Lower-casing the name for HTML elements is up to the
    /// caller.
    pub fn attribute(&self, id: NodeId, qualified_name: &str) -> Option<&str> {
        let i = self.attr_index(id, qualified_name)?;
        Some(&self.attrs(id)[i].value)
    }

    /// The value of the attribute with no namespace and this local name.
    pub fn attribute_named(&self, id: NodeId, local: Name) -> Option<&str> {
        self.attrs(id)
            .iter()
            .find(|a| a.local == local && a.ns == AttrNs::NONE)
            .map(|a| &*a.value)
    }

    /// The value of the attribute with this namespace and local name.
    pub fn attribute_ns(&self, id: NodeId, ns: Name, local: Name) -> Option<&str> {
        let i = self.attr_index_ns(id, ns, local)?;
        Some(&self.attrs(id)[i].value)
    }

    pub fn has_attribute(&self, id: NodeId, qualified_name: &str) -> bool {
        self.attr_index(id, qualified_name).is_some()
    }

    /// Sets an attribute by qualified name, as in `setAttribute`: updates
    /// the first attribute with that name, or appends a new attribute with
    /// no namespace.
    ///
    /// # Panics
    ///
    /// If `id` is not an element.
    pub fn set_attribute(&mut self, id: NodeId, qualified_name: &str, value: &str) {
        match self.attr_index(id, qualified_name) {
            Some(i) => self.element_mut(id).attrs[i].value = value.into(),
            None => {
                let local = self.intern(qualified_name);
                self.push_attr(id, Name::EMPTY, Name::EMPTY, local, value);
            }
        }
    }

    /// Sets a namespaced attribute, as in `setAttributeNS`. Validating the
    /// names is up to the caller.
    ///
    /// # Panics
    ///
    /// If `id` is not an element.
    pub fn set_attribute_ns(&mut self, id: NodeId, ns: &str, qualified_name: &str, value: &str) {
        let ns = self.intern(ns);
        let (prefix, local) = match qualified_name.split_once(':') {
            Some((p, l)) => (self.intern(p), self.intern(l)),
            None => (Name::EMPTY, self.intern(qualified_name)),
        };
        self.set_or_push_attr(id, ns, prefix, local, value);
    }

    /// Sets the attribute with no namespace and this local name.
    pub fn set_attribute_named(&mut self, id: NodeId, local: Name, value: &str) {
        self.set_or_push_attr(id, Name::EMPTY, Name::EMPTY, local, value);
    }

    fn set_or_push_attr(&mut self, id: NodeId, ns: Name, prefix: Name, local: Name, value: &str) {
        match self.attr_index_ns(id, ns, local) {
            Some(i) => self.element_mut(id).attrs[i].value = value.into(),
            None => self.push_attr(id, ns, prefix, local, value),
        }
    }

    /// Appends an attribute unless the element already has one with this
    /// namespace and local name, as the parser does for repeated `<html>` and
    /// `<body>` tags.
    pub(crate) fn add_attr_if_missing(
        &mut self,
        id: NodeId,
        ns: Name,
        prefix: Name,
        local: Name,
        value: &str,
    ) {
        if self.attr_index_ns(id, ns, local).is_none() {
            self.push_attr(id, ns, prefix, local, value);
        }
    }

    /// A new attribute record for one of this document's elements.
    pub(crate) fn new_attr(&mut self, ns: Name, prefix: Name, local: Name, value: &str) -> Attr {
        Attr {
            local,
            ns: self.attr_ns_id(ns, prefix),
            value: CompactString::from(value),
        }
    }

    fn push_attr(&mut self, id: NodeId, ns: Name, prefix: Name, local: Name, value: &str) {
        let attr = self.new_attr(ns, prefix, local, value);
        let e = self.element_mut(id);
        let mut attrs = std::mem::take(&mut e.attrs).into_vec();
        attrs.reserve_exact(1);
        attrs.push(attr);
        e.attrs = attrs.into_boxed_slice();
    }

    /// Removes the first attribute with this qualified name. Returns whether
    /// there was one.
    pub fn remove_attribute(&mut self, id: NodeId, qualified_name: &str) -> bool {
        match self.attr_index(id, qualified_name) {
            Some(i) => {
                self.remove_attr_at(id, i);
                true
            }
            None => false,
        }
    }

    /// Removes the attribute with this namespace and local name. Returns
    /// whether there was one.
    pub fn remove_attribute_ns(&mut self, id: NodeId, ns: Name, local: Name) -> bool {
        match self.attr_index_ns(id, ns, local) {
            Some(i) => {
                self.remove_attr_at(id, i);
                true
            }
            None => false,
        }
    }

    fn remove_attr_at(&mut self, id: NodeId, i: usize) {
        let e = self.element_mut(id);
        let mut attrs = std::mem::take(&mut e.attrs).into_vec();
        attrs.remove(i);
        e.attrs = attrs.into_boxed_slice();
    }

    /// The element's `id` attribute.
    pub fn element_id(&self, id: NodeId) -> Option<&str> {
        self.attribute_named(id, Name::ID)
    }

    /// The tokens of the element's `class` attribute.
    pub fn classes(&self, id: NodeId) -> impl Iterator<Item = &str> + '_ {
        self.attribute_named(id, Name::CLASS)
            .unwrap_or("")
            .split(is_ascii_whitespace)
            .filter(|s| !s.is_empty())
    }

    /// Whether the element's `class` attribute contains `class`. Matching is
    /// ASCII case-insensitive in quirks mode, as for selectors.
    pub fn has_class(&self, id: NodeId, class: &str) -> bool {
        if self.quirks_mode == QuirksMode::Quirks {
            self.classes(id).any(|c| c.eq_ignore_ascii_case(class))
        } else {
            self.classes(id).any(|c| c == class)
        }
    }

    // ---------------------------------------------------------------------
    // Navigation.

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).first_child
    }

    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).last_child
    }

    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).prev_sibling
    }

    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).next_sibling
    }

    /// The parent, if it is an element.
    pub fn parent_element(&self, id: NodeId) -> Option<NodeId> {
        self.parent(id).filter(|&p| self.is_element(p))
    }

    /// The children of `id`, in order.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            doc: self,
            next: self.first_child(id),
        }
    }

    /// The element children of `id`, in order.
    pub fn element_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.children(id).filter(|&c| self.is_element(c))
    }

    /// The descendants of `id` in tree order, not including `id` itself.
    /// Template contents are not descendants of their template.
    pub fn descendants(&self, id: NodeId) -> Descendants<'_> {
        Descendants {
            doc: self,
            root: id,
            next: self.first_child(id),
        }
    }

    /// The ancestors of `id`, nearest first, not including `id` itself.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.parent(id), |&n| self.parent(n))
    }

    /// The node following `id` in tree order within the subtree of `root`.
    pub fn next_in_tree(&self, id: NodeId, root: NodeId) -> Option<NodeId> {
        if let Some(c) = self.first_child(id) {
            return Some(c);
        }
        let mut n = id;
        loop {
            if n == root {
                return None;
            }
            if let Some(s) = self.next_sibling(n) {
                return Some(s);
            }
            n = self.parent(n)?;
        }
    }

    /// The root of the tree containing `id`: the topmost ancestor, or `id`.
    pub fn tree_root(&self, id: NodeId) -> NodeId {
        self.ancestors(id).last().unwrap_or(id)
    }

    /// Whether `id` is in the document (its root is the document node).
    pub fn is_connected(&self, id: NodeId) -> bool {
        self.tree_root(id) == self.root()
    }

    /// Whether `other` is `id` or one of its descendants (`Node.contains`).
    pub fn contains(&self, id: NodeId, other: NodeId) -> bool {
        other == id || self.ancestors(other).any(|a| a == id)
    }

    /// Whether `ancestor` is `id` or an ancestor of it, also looking through
    /// template contents to their template ("host-including").
    fn is_host_including_inclusive_ancestor(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut n = id;
        loop {
            if n == ancestor {
                return true;
            }
            match self.parent(n).or_else(|| self.template_host(n)) {
                Some(p) => n = p,
                None => return false,
            }
        }
    }

    /// The document element (`<html>`).
    pub fn document_element(&self) -> Option<NodeId> {
        self.element_children(self.root()).next()
    }

    /// The `<head>` element: the first `head` child of an `<html>` root.
    pub fn head(&self) -> Option<NodeId> {
        let html = self
            .document_element()
            .filter(|&h| self.is_html(h, Name::HTML))?;
        self.children(html).find(|&c| self.is_html(c, Name::HEAD))
    }

    /// The `<body>` element: the first `body` or `frameset` child of an
    /// `<html>` root.
    pub fn body(&self) -> Option<NodeId> {
        let html = self
            .document_element()
            .filter(|&h| self.is_html(h, Name::HTML))?;
        self.children(html)
            .find(|&c| self.is_html(c, Name::BODY) || self.is_html(c, Name::FRAMESET))
    }

    /// `document.title`: the text of the first `<title>` element, with
    /// ASCII whitespace stripped and collapsed.
    pub fn title(&self) -> String {
        let root = self.root();
        let Some(title) = self
            .descendants(root)
            .find(|&n| self.is_html(n, Name::TITLE))
        else {
            return String::new();
        };
        let mut text = String::new();
        for c in self.children(title) {
            if let NodeData::Text(s) = &self.node(c).data {
                text.push_str(s);
            }
        }
        let mut out = String::with_capacity(text.len());
        for word in text.split(is_ascii_whitespace).filter(|w| !w.is_empty()) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
        out
    }

    // ---------------------------------------------------------------------
    // Unchecked tree surgery. These keep the links consistent but do not
    // enforce the DOM's hierarchy rules.

    /// Unlinks `id` from its parent, if it has one.
    pub(crate) fn detach(&mut self, id: NodeId) {
        let node = self.node_mut(id);
        let (parent, prev, next) = (node.parent, node.prev_sibling, node.next_sibling);
        node.parent = None;
        node.prev_sibling = None;
        node.next_sibling = None;
        let Some(parent) = parent else { return };
        match prev {
            Some(p) => self.node_mut(p).next_sibling = next,
            None => self.node_mut(parent).first_child = next,
        }
        match next {
            Some(n) => self.node_mut(n).prev_sibling = prev,
            None => self.node_mut(parent).last_child = prev,
        }
    }

    /// Moves `child` to be the child of `parent` just before `before`, or
    /// the last child if `before` is `None`.
    pub(crate) fn insert_unchecked(
        &mut self,
        parent: NodeId,
        child: NodeId,
        before: Option<NodeId>,
    ) {
        debug_assert_ne!(Some(child), before);
        self.detach(child);
        let prev = match before {
            Some(b) => {
                debug_assert_eq!(self.parent(b), Some(parent));
                self.prev_sibling(b)
            }
            None => self.last_child(parent),
        };
        {
            let node = self.node_mut(child);
            node.parent = Some(parent);
            node.prev_sibling = prev;
            node.next_sibling = before;
        }
        match prev {
            Some(p) => self.node_mut(p).next_sibling = Some(child),
            None => self.node_mut(parent).first_child = Some(child),
        }
        match before {
            Some(b) => self.node_mut(b).prev_sibling = Some(child),
            None => self.node_mut(parent).last_child = Some(child),
        }
    }

    /// Moves all children of `from` to the end of `to`.
    pub(crate) fn reparent_children(&mut self, from: NodeId, to: NodeId) {
        while let Some(c) = self.first_child(from) {
            self.insert_unchecked(to, c, None);
        }
    }

    // ---------------------------------------------------------------------
    // Checked mutation, following the DOM standard's algorithms.

    /// `parent.appendChild(node)`. Returns `node`.
    pub fn append_child(&mut self, parent: NodeId, node: NodeId) -> Result<NodeId, DomError> {
        self.insert_before(parent, node, None)
    }

    /// `parent.insertBefore(node, child)`: inserts `node` before `child`, or
    /// at the end if `child` is `None`. A fragment's children are moved in
    /// its place. Returns `node`.
    pub fn insert_before(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: Option<NodeId>,
    ) -> Result<NodeId, DomError> {
        self.ensure_pre_insertion_validity(node, parent, child)?;
        let before = if child == Some(node) {
            self.next_sibling(node)
        } else {
            child
        };
        self.insert(node, parent, before);
        Ok(node)
    }

    /// Inserts `node` (a fragment's children, if it is one) without checks.
    pub(crate) fn insert(&mut self, node: NodeId, parent: NodeId, before: Option<NodeId>) {
        if self.kind(node) == NodeKind::DocumentFragment {
            while let Some(c) = self.first_child(node) {
                self.insert_unchecked(parent, c, before);
            }
        } else {
            self.insert_unchecked(parent, node, before);
        }
    }

    /// `parent.removeChild(child)`. Returns `child`.
    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) -> Result<NodeId, DomError> {
        if self.parent(child) != Some(parent) {
            return Err(DomError::NotFound);
        }
        self.detach(child);
        Ok(child)
    }

    /// `node.remove()`: detaches `node` from its parent, if any.
    pub fn remove(&mut self, node: NodeId) {
        self.detach(node);
    }

    /// `parent.replaceChild(node, child)`. Returns `child`.
    pub fn replace_child(
        &mut self,
        parent: NodeId,
        node: NodeId,
        child: NodeId,
    ) -> Result<NodeId, DomError> {
        self.ensure_replace_validity(node, parent, child)?;
        let mut before = self.next_sibling(child);
        if before == Some(node) {
            before = self.next_sibling(node);
        }
        self.detach(child);
        self.insert(node, parent, before);
        Ok(child)
    }

    /// Removes all children of `parent`, leaving them detached.
    pub fn remove_children(&mut self, parent: NodeId) {
        while let Some(c) = self.first_child(parent) {
            self.detach(c);
        }
    }

    /// Replaces all children of `parent` with `node` (or with nothing), as
    /// the DOM's "replace all" does for `textContent` and `innerHTML`.
    pub(crate) fn replace_all(&mut self, parent: NodeId, node: Option<NodeId>) {
        self.remove_children(parent);
        if let Some(node) = node {
            self.insert(node, parent, None);
        }
    }

    fn check_parent_and_node(&self, node: NodeId, parent: NodeId) -> Result<(), DomError> {
        use NodeKind::*;
        let parent_kind = self.kind(parent);
        if !matches!(parent_kind, Document | DocumentFragment | Element) {
            return Err(DomError::HierarchyRequest);
        }
        if self.is_host_including_inclusive_ancestor(node, parent) {
            return Err(DomError::HierarchyRequest);
        }
        Ok(())
    }

    fn check_node_kind(&self, node: NodeId, parent: NodeId) -> Result<(), DomError> {
        use NodeKind::*;
        match (self.kind(node), self.kind(parent)) {
            (Document, _) | (Text, Document) => Err(DomError::HierarchyRequest),
            (Doctype, p) if p != Document => Err(DomError::HierarchyRequest),
            _ => Ok(()),
        }
    }

    fn ensure_pre_insertion_validity(
        &self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
    ) -> Result<(), DomError> {
        self.check_parent_and_node(node, parent)?;
        if child.is_some_and(|c| self.parent(c) != Some(parent)) {
            return Err(DomError::NotFound);
        }
        self.check_node_kind(node, parent)?;
        if self.kind(parent) != NodeKind::Document {
            return Ok(());
        }
        let doctype_after_child = child.is_some_and(|c| {
            self.kind(c) == NodeKind::Doctype || self.following_siblings_have(c, NodeKind::Doctype)
        });
        let bad = match self.kind(node) {
            NodeKind::DocumentFragment => {
                let (elements, has_text) = self.fragment_shape(node);
                elements > 1
                    || has_text
                    || (elements == 1
                        && (self.has_child(parent, NodeKind::Element, None) || doctype_after_child))
            }
            NodeKind::Element => {
                self.has_child(parent, NodeKind::Element, None) || doctype_after_child
            }
            NodeKind::Doctype => {
                self.has_child(parent, NodeKind::Doctype, None)
                    || match child {
                        Some(c) => self.preceding_siblings_have(c, NodeKind::Element),
                        None => self.has_child(parent, NodeKind::Element, None),
                    }
            }
            _ => false,
        };
        if bad {
            Err(DomError::HierarchyRequest)
        } else {
            Ok(())
        }
    }

    fn ensure_replace_validity(
        &self,
        node: NodeId,
        parent: NodeId,
        child: NodeId,
    ) -> Result<(), DomError> {
        self.check_parent_and_node(node, parent)?;
        if self.parent(child) != Some(parent) {
            return Err(DomError::NotFound);
        }
        self.check_node_kind(node, parent)?;
        if self.kind(parent) != NodeKind::Document {
            return Ok(());
        }
        let doctype_after_child = self.following_siblings_have(child, NodeKind::Doctype);
        let other_element = self.has_child(parent, NodeKind::Element, Some(child));
        let bad = match self.kind(node) {
            NodeKind::DocumentFragment => {
                let (elements, has_text) = self.fragment_shape(node);
                elements > 1
                    || has_text
                    || (elements == 1 && (other_element || doctype_after_child))
            }
            NodeKind::Element => other_element || doctype_after_child,
            NodeKind::Doctype => {
                self.has_child(parent, NodeKind::Doctype, Some(child))
                    || self.preceding_siblings_have(child, NodeKind::Element)
            }
            _ => false,
        };
        if bad {
            Err(DomError::HierarchyRequest)
        } else {
            Ok(())
        }
    }

    /// (element children, has a text child) of a fragment.
    fn fragment_shape(&self, fragment: NodeId) -> (usize, bool) {
        let mut elements = 0;
        let mut has_text = false;
        for c in self.children(fragment) {
            match self.kind(c) {
                NodeKind::Element => elements += 1,
                NodeKind::Text => has_text = true,
                _ => {}
            }
        }
        (elements, has_text)
    }

    fn has_child(&self, parent: NodeId, kind: NodeKind, except: Option<NodeId>) -> bool {
        self.children(parent)
            .any(|c| Some(c) != except && self.kind(c) == kind)
    }

    fn following_siblings_have(&self, id: NodeId, kind: NodeKind) -> bool {
        std::iter::successors(self.next_sibling(id), |&n| self.next_sibling(n))
            .any(|n| self.kind(n) == kind)
    }

    fn preceding_siblings_have(&self, id: NodeId, kind: NodeKind) -> bool {
        std::iter::successors(self.prev_sibling(id), |&n| self.prev_sibling(n))
            .any(|n| self.kind(n) == kind)
    }

    // ---------------------------------------------------------------------
    // Cloning and text content.

    /// `node.cloneNode(deep)`: a detached copy of `id`, with its descendants
    /// if `deep`. Cloning a template deeply also copies its contents.
    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> NodeId {
        let copy = self.clone_one(id);
        if deep {
            // (source parent, copy parent) pairs still to fill in.
            let mut work = vec![(id, copy)];
            while let Some((src, dst)) = work.pop() {
                if let (Some(sc), Some(dc)) =
                    (self.template_contents(src), self.template_contents(dst))
                {
                    work.push((sc, dc));
                }
                let mut child = self.first_child(src);
                while let Some(c) = child {
                    if self.over_node_limit {
                        // The copy is cut short; whoever enforces the
                        // limit throws the page away anyway.
                        return copy;
                    }
                    let c_copy = self.clone_one(c);
                    self.insert_unchecked(dst, c_copy, None);
                    work.push((c, c_copy));
                    child = self.next_sibling(c);
                }
            }
        }
        copy
    }

    fn clone_one(&mut self, id: NodeId) -> NodeId {
        let node_flags = self.node(id).flags;
        let data = match &self.node(id).data {
            NodeData::Element(e) => {
                let (ns, local, attrs) = (e.ns, e.local, e.attrs.clone());
                let copy = self.create_element_with_attrs(ns, local, attrs);
                self.node_mut(copy).flags = node_flags;
                if node_flags & flags::HAS_PREFIX != 0 {
                    let prefix = self.prefixes[&id];
                    self.prefixes.insert(copy, prefix);
                }
                return copy;
            }
            other => other.clone(),
        };
        let copy = self.alloc(data);
        self.node_mut(copy).flags = node_flags;
        copy
    }

    /// `Node.textContent`: the concatenated text of all descendant text
    /// nodes for elements and fragments, the data of text and comment
    /// nodes, and `None` for documents and doctypes.
    pub fn text_content(&self, id: NodeId) -> Option<String> {
        match &self.node(id).data {
            NodeData::Text(s) | NodeData::Comment(s) => Some(s.to_string()),
            NodeData::Element(_) | NodeData::Fragment => {
                let mut out = String::new();
                for n in self.descendants(id) {
                    if let NodeData::Text(s) = &self.node(n).data {
                        out.push_str(s);
                    }
                }
                Some(out)
            }
            NodeData::Document | NodeData::Doctype(_) => None,
            NodeData::Free => unreachable!(),
        }
    }

    /// Sets `Node.textContent`: replaces an element's or fragment's children
    /// with a single text node (none if `text` is empty), or sets the data of
    /// a text or comment node. Does nothing to documents and doctypes.
    pub fn set_text_content(&mut self, id: NodeId, text: &str) {
        match self.kind(id) {
            NodeKind::Text | NodeKind::Comment => self.set_text(id, text),
            NodeKind::Element | NodeKind::DocumentFragment => {
                let node = (!text.is_empty()).then(|| self.create_text(text));
                self.replace_all(id, node);
            }
            NodeKind::Document | NodeKind::Doctype => {}
        }
    }
}

/// ASCII whitespace as the HTML standard defines it.
pub(crate) fn is_ascii_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// Iterator over a node's children. See [`Document::children`].
pub struct Children<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let n = self.next?;
        self.next = self.doc.next_sibling(n);
        Some(n)
    }
}

/// Pre-order iterator over a node's descendants. See
/// [`Document::descendants`].
pub struct Descendants<'a> {
    doc: &'a Document,
    root: NodeId,
    next: Option<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let n = self.next?;
        self.next = self.doc.next_in_tree(n, self.root);
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<html><head></head><body></body></html>` built by hand.
    fn skeleton() -> (Document, NodeId, NodeId, NodeId) {
        let mut doc = Document::new();
        let html = doc.create_element("html");
        let head = doc.create_element("head");
        let body = doc.create_element("body");
        let root = doc.root();
        doc.append_child(root, html).unwrap();
        doc.append_child(html, head).unwrap();
        doc.append_child(html, body).unwrap();
        (doc, html, head, body)
    }

    fn names(doc: &Document, ids: impl IntoIterator<Item = NodeId>) -> Vec<String> {
        ids.into_iter()
            .map(|n| match doc.kind(n) {
                NodeKind::Text => format!("#{}", doc.text(n).unwrap()),
                _ => doc.node_name(n).into_owned(),
            })
            .collect()
    }

    /// Checks every link in the tree under `id` is mirrored correctly.
    fn assert_consistent(doc: &Document, id: NodeId) {
        let mut prev = None;
        for c in doc.children(id) {
            assert_eq!(doc.parent(c), Some(id));
            assert_eq!(doc.prev_sibling(c), prev);
            if let Some(p) = prev {
                assert_eq!(doc.next_sibling(p), Some(c));
            }
            assert_consistent(doc, c);
            prev = Some(c);
        }
        assert_eq!(doc.last_child(id), prev);
        if prev.is_none() {
            assert_eq!(doc.first_child(id), None);
        }
    }

    #[test]
    fn new_document_has_only_the_document_node() {
        let doc = Document::new();
        assert_eq!(doc.kind(doc.root()), NodeKind::Document);
        assert_eq!(doc.len(), 1);
        assert!(doc.is_empty());
        assert_eq!(doc.document_element(), None);
        assert_eq!(doc.text_content(doc.root()), None);
    }

    #[test]
    fn append_insert_and_navigate() {
        let (mut doc, html, head, body) = skeleton();
        assert_eq!(doc.document_element(), Some(html));
        assert_eq!(doc.head(), Some(head));
        assert_eq!(doc.body(), Some(body));

        let a = doc.create_element("a");
        let b = doc.create_element("b");
        let c = doc.create_element("c");
        doc.append_child(body, a).unwrap();
        doc.append_child(body, c).unwrap();
        doc.insert_before(body, b, Some(c)).unwrap();
        assert_eq!(names(&doc, doc.children(body)), ["A", "B", "C"]);
        assert_eq!(doc.prev_sibling(b), Some(a));
        assert_eq!(doc.next_sibling(b), Some(c));
        assert_eq!(doc.parent_element(b), Some(body));
        assert_eq!(
            doc.ancestors(b).collect::<Vec<_>>(),
            [body, html, doc.root()]
        );
        assert_eq!(
            names(&doc, doc.descendants(html)),
            ["HEAD", "BODY", "A", "B", "C"]
        );
        assert!(doc.contains(html, c));
        assert!(!doc.contains(c, html));
        assert!(doc.is_connected(c));
        assert_consistent(&doc, doc.root());
    }

    #[test]
    fn inserting_moves_a_node_from_its_old_parent() {
        let (mut doc, _, head, body) = skeleton();
        let x = doc.create_element("x");
        doc.append_child(head, x).unwrap();
        doc.append_child(body, x).unwrap();
        assert_eq!(doc.first_child(head), None);
        assert_eq!(doc.first_child(body), Some(x));
        // Inserting a node before itself leaves it where it is.
        let y = doc.create_element("y");
        doc.append_child(body, y).unwrap();
        doc.insert_before(body, x, Some(x)).unwrap();
        assert_eq!(names(&doc, doc.children(body)), ["X", "Y"]);
        assert_consistent(&doc, doc.root());
    }

    #[test]
    fn remove_and_replace() {
        let (mut doc, _, head, body) = skeleton();
        let a = doc.create_element("a");
        let b = doc.create_element("b");
        let c = doc.create_element("c");
        for n in [a, b, c] {
            doc.append_child(body, n).unwrap();
        }
        assert_eq!(doc.remove_child(head, b), Err(DomError::NotFound));
        assert_eq!(doc.remove_child(body, b), Ok(b));
        assert_eq!(doc.parent(b), None);
        assert!(!doc.is_connected(b));
        assert_eq!(names(&doc, doc.children(body)), ["A", "C"]);

        let d = doc.create_element("d");
        assert_eq!(doc.replace_child(body, d, a), Ok(a));
        assert_eq!(names(&doc, doc.children(body)), ["D", "C"]);
        // Replacing a node with its own next sibling.
        assert_eq!(doc.replace_child(body, c, d), Ok(d));
        assert_eq!(names(&doc, doc.children(body)), ["C"]);

        doc.remove(c);
        doc.remove(c);
        assert_eq!(doc.first_child(body), None);
        assert_consistent(&doc, doc.root());
    }

    #[test]
    fn fragments_insert_their_children() {
        let (mut doc, _, _, body) = skeleton();
        let end = doc.create_element("end");
        doc.append_child(body, end).unwrap();
        let frag = doc.create_fragment();
        for name in ["a", "b"] {
            let e = doc.create_element(name);
            doc.append_child(frag, e).unwrap();
        }
        doc.insert_before(body, frag, Some(end)).unwrap();
        assert_eq!(names(&doc, doc.children(body)), ["A", "B", "END"]);
        assert_eq!(doc.first_child(frag), None);

        let frag2 = doc.create_fragment();
        let t = doc.create_text("t");
        doc.append_child(frag2, t).unwrap();
        doc.replace_child(body, frag2, end).unwrap();
        assert_eq!(names(&doc, doc.children(body)), ["A", "B", "#t"]);
        assert_consistent(&doc, doc.root());
    }

    #[test]
    fn hierarchy_errors() {
        let (mut doc, html, head, body) = skeleton();
        let root = doc.root();
        let text = doc.create_text("x");

        // Into a descendant, or into itself.
        assert_eq!(
            doc.append_child(body, html),
            Err(DomError::HierarchyRequest)
        );
        assert_eq!(
            doc.append_child(body, body),
            Err(DomError::HierarchyRequest)
        );
        // Into a text node.
        let e = doc.create_element("e");
        assert_eq!(doc.append_child(text, e), Err(DomError::HierarchyRequest));
        // Text directly under the document; a second document element.
        assert_eq!(
            doc.append_child(root, text),
            Err(DomError::HierarchyRequest)
        );
        assert_eq!(doc.append_child(root, e), Err(DomError::HierarchyRequest));
        // A document node anywhere.
        let other_doc = doc.create_document();
        assert_eq!(
            doc.append_child(body, other_doc),
            Err(DomError::HierarchyRequest)
        );
        // A doctype outside the document, or after the document element.
        let dt = doc.create_doctype("html", "", "");
        assert_eq!(doc.append_child(body, dt), Err(DomError::HierarchyRequest));
        assert_eq!(doc.append_child(root, dt), Err(DomError::HierarchyRequest));
        assert_eq!(doc.insert_before(root, dt, Some(html)), Ok(dt));
        // A second doctype.
        let dt2 = doc.create_doctype("html", "", "");
        assert_eq!(
            doc.insert_before(root, dt2, Some(html)),
            Err(DomError::HierarchyRequest)
        );
        // An element before the doctype.
        doc.remove(html);
        assert_eq!(
            doc.insert_before(root, html, Some(dt)),
            Err(DomError::HierarchyRequest)
        );
        assert_eq!(doc.append_child(root, html), Ok(html));
        // Reference child under a different parent.
        assert_eq!(
            doc.insert_before(body, e, Some(head)),
            Err(DomError::NotFound)
        );
        // Replacing the document element with another element is fine.
        assert_eq!(doc.replace_child(root, e, html), Ok(html));
        assert_eq!(doc.document_element(), Some(e));
        // But not with a fragment holding two elements, or text.
        let frag = doc.create_fragment();
        let f1 = doc.create_element("f1");
        let f2 = doc.create_element("f2");
        doc.append_child(frag, f1).unwrap();
        doc.append_child(frag, f2).unwrap();
        assert_eq!(
            doc.replace_child(root, frag, e),
            Err(DomError::HierarchyRequest)
        );
        assert_consistent(&doc, root);
    }

    #[test]
    fn template_contents_count_as_ancestors_for_insertion() {
        let (mut doc, _, _, body) = skeleton();
        let tpl = doc.create_element("template");
        doc.append_child(body, tpl).unwrap();
        let contents = doc.template_contents(tpl).unwrap();
        assert_eq!(doc.kind(contents), NodeKind::DocumentFragment);
        assert_eq!(doc.template_host(contents), Some(tpl));
        assert_eq!(
            doc.append_child(contents, body),
            Err(DomError::HierarchyRequest)
        );
        let p = doc.create_element("p");
        assert_eq!(doc.append_child(contents, p), Ok(p));
        assert_eq!(doc.first_child(tpl), None);
    }

    #[test]
    fn attributes() {
        let (mut doc, _, _, body) = skeleton();
        assert_eq!(doc.attribute(body, "class"), None);
        doc.set_attribute(body, "class", "a  b\tc");
        doc.set_attribute(body, "data-x", "1");
        doc.set_attribute(body, "class", "a b c");
        assert_eq!(doc.attribute(body, "class"), Some("a b c"));
        assert_eq!(doc.attribute(body, "data-x"), Some("1"));
        assert_eq!(doc.attrs(body).len(), 2);
        assert!(doc.has_class(body, "b"));
        assert!(!doc.has_class(body, "B"));
        assert_eq!(doc.classes(body).collect::<Vec<_>>(), ["a", "b", "c"]);

        doc.set_attribute_ns(body, "http://www.w3.org/1999/xlink", "xlink:href", "#x");
        assert_eq!(doc.attribute(body, "xlink:href"), Some("#x"));
        assert_eq!(doc.attribute(body, "href"), None);
        assert_eq!(
            doc.attribute_ns(body, Name::NS_XLINK, Name::HREF),
            Some("#x")
        );

        assert!(doc.remove_attribute(body, "data-x"));
        assert!(!doc.remove_attribute(body, "data-x"));
        assert!(doc.remove_attribute_ns(body, Name::NS_XLINK, Name::HREF));
        assert_eq!(doc.attrs(body).len(), 1);

        doc.set_attribute_named(body, Name::ID, "main");
        assert_eq!(doc.element_id(body), Some("main"));
        let text = doc.create_text("t");
        assert_eq!(doc.attrs(text), &[]);
    }

    #[test]
    fn class_matching_ignores_case_in_quirks_mode() {
        let (mut doc, _, _, body) = skeleton();
        doc.set_attribute(body, "class", "Foo");
        assert!(!doc.has_class(body, "foo"));
        doc.set_quirks_mode(QuirksMode::Quirks);
        assert!(doc.has_class(body, "foo"));
    }

    #[test]
    fn names_of_elements() {
        let mut doc = Document::new();
        let div = doc.create_element("div");
        assert_eq!(doc.local_name(div), Some(Name::DIV));
        assert_eq!(doc.namespace(div), Some(Name::NS_HTML));
        assert_eq!(doc.tag_name(div).unwrap(), "DIV");
        assert!(doc.is_html(div, Name::DIV));

        let svg = doc.create_element_ns("http://www.w3.org/2000/svg", "foreignObject");
        assert_eq!(doc.tag_name(svg).unwrap(), "foreignObject");
        assert_eq!(doc.prefix(svg), Some(Name::EMPTY));

        let prefixed = doc.create_element_ns("urn:x", "x:Thing");
        assert_eq!(doc.qualified_name(prefixed).unwrap(), "x:Thing");
        assert_eq!(doc.name_str(doc.prefix(prefixed).unwrap()), "x");
        assert_eq!(doc.name_str(doc.namespace(prefixed).unwrap()), "urn:x");

        let text = doc.create_text("hi");
        let comment = doc.create_comment("c");
        let frag = doc.create_fragment();
        let dt = doc.create_doctype("html", "", "");
        assert_eq!(doc.node_name(text), "#text");
        assert_eq!(doc.node_name(comment), "#comment");
        assert_eq!(doc.node_name(frag), "#document-fragment");
        assert_eq!(doc.node_name(dt), "html");
        assert_eq!(doc.node_name(doc.root()), "#document");
        assert_eq!(doc.kind(dt).node_type(), 10);
        assert_eq!(doc.local_name(text), None);
    }

    #[test]
    fn text_content() {
        let (mut doc, _, _, body) = skeleton();
        let p = doc.create_element("p");
        let t1 = doc.create_text("Hello, ");
        let b = doc.create_element("b");
        let t2 = doc.create_text("world");
        let c = doc.create_comment("not text");
        doc.append_child(body, p).unwrap();
        doc.append_child(p, t1).unwrap();
        doc.append_child(p, b).unwrap();
        doc.append_child(b, t2).unwrap();
        doc.append_child(p, c).unwrap();
        assert_eq!(doc.text_content(body).as_deref(), Some("Hello, world"));
        assert_eq!(doc.text_content(c).as_deref(), Some("not text"));

        doc.set_text_content(t2, "there");
        assert_eq!(doc.text_content(p).as_deref(), Some("Hello, there"));

        doc.set_text_content(p, "replaced");
        assert_eq!(doc.children(p).count(), 1);
        assert_eq!(doc.text_content(p).as_deref(), Some("replaced"));
        assert_eq!(doc.parent(b), None);

        doc.set_text_content(p, "");
        assert_eq!(doc.first_child(p), None);
        assert_consistent(&doc, doc.root());
    }

    #[test]
    fn title_is_stripped_and_collapsed() {
        let (mut doc, _, head, _) = skeleton();
        assert_eq!(doc.title(), "");
        let title = doc.create_element("title");
        let t = doc.create_text("  Hello \n\t world  ");
        doc.append_child(head, title).unwrap();
        doc.append_child(title, t).unwrap();
        assert_eq!(doc.title(), "Hello world");
    }

    #[test]
    fn clone_shallow_and_deep() {
        let (mut doc, _, _, body) = skeleton();
        let ul = doc.create_element("ul");
        doc.set_attribute(ul, "class", "list");
        doc.append_child(body, ul).unwrap();
        for i in 0..3 {
            let li = doc.create_element("li");
            let t = doc.create_text(&i.to_string());
            doc.append_child(li, t).unwrap();
            doc.append_child(ul, li).unwrap();
        }

        let shallow = doc.clone_node(ul, false);
        assert_eq!(doc.parent(shallow), None);
        assert_eq!(doc.first_child(shallow), None);
        assert_eq!(doc.attribute(shallow, "class"), Some("list"));

        let deep = doc.clone_node(ul, true);
        assert_eq!(doc.text_content(deep).as_deref(), Some("012"));
        assert_eq!(names(&doc, doc.children(deep)), ["LI", "LI", "LI"]);
        // The copy is independent of the original.
        doc.set_attribute(deep, "class", "copy");
        let first = doc.first_child(deep).unwrap();
        doc.set_text_content(first, "x");
        assert_eq!(doc.attribute(ul, "class"), Some("list"));
        assert_eq!(doc.text_content(ul).as_deref(), Some("012"));
        assert_consistent(&doc, deep);
    }

    #[test]
    fn clone_copies_template_contents_and_prefixes() {
        let mut doc = Document::new();
        let tpl = doc.create_element("template");
        let contents = doc.template_contents(tpl).unwrap();
        let span = doc.create_element("span");
        doc.append_child(contents, span).unwrap();
        let copy = doc.clone_node(tpl, true);
        let copy_contents = doc.template_contents(copy).unwrap();
        assert_ne!(copy_contents, contents);
        assert_eq!(names(&doc, doc.children(copy_contents)), ["SPAN"]);
        let shallow = doc.clone_node(tpl, false);
        assert_eq!(
            doc.first_child(doc.template_contents(shallow).unwrap()),
            None
        );

        let prefixed = doc.create_element_ns("urn:x", "x:y");
        let copy = doc.clone_node(prefixed, false);
        assert_eq!(doc.qualified_name(copy).unwrap(), "x:y");
    }

    #[test]
    fn script_flags_survive_cloning() {
        let mut doc = Document::new();
        let script = doc.create_element("script");
        assert!(!doc.script_already_started(script));
        doc.set_script_already_started(script);
        let copy = doc.clone_node(script, false);
        assert!(doc.script_already_started(copy));
    }

    #[test]
    fn released_slots_are_reused() {
        let (mut doc, _, _, body) = skeleton();
        let div = doc.create_element("div");
        let tpl = doc.create_element("template");
        let inner = doc.create_text("inner");
        doc.append_child(div, tpl).unwrap();
        doc.append_child(doc.template_contents(tpl).unwrap(), inner)
            .unwrap();
        doc.append_child(body, div).unwrap();
        let before = doc.len();
        let slots = doc.capacity();

        doc.remove(div);
        doc.release(div);
        // div, template, its contents fragment and the text node.
        assert_eq!(doc.len(), before - 4);
        assert_eq!(doc.capacity(), slots);

        for _ in 0..4 {
            doc.create_element("p");
        }
        assert_eq!(doc.capacity(), slots);
        doc.create_element("p");
        assert_eq!(doc.capacity(), slots + 1);
    }

    #[test]
    #[should_panic(expected = "released")]
    fn using_a_released_node_panics() {
        let mut doc = Document::new();
        let div = doc.create_element("div");
        doc.release(div);
        doc.kind(div);
    }

    #[test]
    #[should_panic(expected = "attached")]
    fn releasing_an_attached_node_panics() {
        let (mut doc, _, _, body) = skeleton();
        doc.release(body);
    }

    #[test]
    fn deep_trees_do_not_recurse() {
        let (mut doc, _, _, body) = skeleton();
        let mut parent = body;
        for _ in 0..10_000 {
            let div = doc.create_element("div");
            doc.append_child(parent, div).unwrap();
            parent = div;
        }
        let t = doc.create_text("deep");
        doc.append_child(parent, t).unwrap();
        assert_eq!(doc.text_content(body).as_deref(), Some("deep"));
        let copy = doc.clone_node(body, true);
        assert_eq!(doc.descendants(copy).count(), 10_001);
    }
}
