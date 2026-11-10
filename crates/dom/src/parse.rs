//! HTML parsing with html5ever's tokenizer and tree builder, straight into
//! the document arena.
//!
//! html5ever drives a [`TreeSink`]; ours turns each tree-building step into
//! arena operations, interning names as elements are created. Fragment
//! parsing (`innerHTML`, `outerHTML`, `insertAdjacentHTML`) builds into the
//! same arena, so the parsed nodes can be moved into place without copying.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{
    ElemName, ElementFlags, NodeOrText, QuirksMode as H5QuirksMode, TreeBuilderOpts, TreeSink,
};
use html5ever::{Attribute, LocalName, Namespace, ParseOpts, QualName, ns};

use crate::document::{Document, QuirksMode};
use crate::error::DomError;
use crate::name::Name;
use crate::node::{Attr, NodeId, NodeKind, flags};

/// Options for parsing a whole document.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    /// Parse as if scripting is enabled, which makes `<noscript>` content
    /// raw text. dasher runs scripts, so this defaults to `true`, matching
    /// what a browser builds.
    pub scripting_enabled: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            scripting_enabled: true,
        }
    }
}

/// Where [`Document::insert_adjacent_html`] puts the parsed nodes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdjacentPosition {
    /// Before the element itself (`"beforebegin"`).
    BeforeBegin,
    /// Before the element's first child (`"afterbegin"`).
    AfterBegin,
    /// After the element's last child (`"beforeend"`).
    BeforeEnd,
    /// After the element itself (`"afterend"`).
    AfterEnd,
}

impl Document {
    /// Parses a complete HTML document.
    pub fn parse(html: &str) -> Document {
        Self::parse_with(html, ParseOptions::default())
    }

    /// Parses a complete HTML document with options.
    pub fn parse_with(html: &str, options: ParseOptions) -> Document {
        let mut doc = Document::new();
        doc.set_scripting_enabled(options.scripting_enabled);
        let root = doc.root();
        let opts = ParseOpts {
            tree_builder: TreeBuilderOpts {
                scripting_enabled: options.scripting_enabled,
                ..TreeBuilderOpts::default()
            },
            ..ParseOpts::default()
        };
        html5ever::parse_document(Sink::new(&mut doc, root, false), opts).one(html);
        doc.shrink_to_fit();
        doc
    }

    /// Parses `html` as a fragment in the context of the element `context`,
    /// as the HTML standard's fragment parsing algorithm does for
    /// `innerHTML`. Returns a new document fragment holding the result.
    ///
    /// Scripts in the fragment are marked "already started", so they never
    /// run.
    ///
    /// # Panics
    ///
    /// If `context` is not an element.
    pub fn parse_fragment(&mut self, context: NodeId, html: &str) -> NodeId {
        assert!(
            self.is_element(context),
            "fragment context must be an element"
        );
        let context_name = QualName::new(
            None,
            Namespace::from(self.name_str(self.namespace(context).expect("is an element"))),
            LocalName::from(self.name_str(self.local_name(context).expect("is an element"))),
        );
        // Only an `annotation-xml` context's `encoding` attribute matters to
        // the tree builder.
        let context_attrs = self
            .attrs(context)
            .iter()
            .filter(|a| self.attr_namespace(a) == Name::EMPTY && a.local == Name::ENCODING)
            .map(|a| Attribute {
                name: QualName::new(None, ns!(), LocalName::from("encoding")),
                value: StrTendril::from(a.value()),
            })
            .collect();
        let scripting = self.scripting_enabled();
        let opts = ParseOpts {
            tree_builder: TreeBuilderOpts {
                scripting_enabled: scripting,
                quirks_mode: match self.quirks_mode() {
                    QuirksMode::NoQuirks => H5QuirksMode::NoQuirks,
                    QuirksMode::LimitedQuirks => H5QuirksMode::LimitedQuirks,
                    QuirksMode::Quirks => H5QuirksMode::Quirks,
                },
                ..TreeBuilderOpts::default()
            },
            ..ParseOpts::default()
        };

        // html5ever builds the fragment under an <html> element appended to
        // a scratch document node, and makes its own copy of the context
        // element. Both are thrown away once the result is moved out.
        let scratch = self.create_document_node();
        let context_copy = html5ever::parse_fragment(
            Sink::new(self, scratch, true),
            opts,
            context_name,
            context_attrs,
            scripting,
        )
        .one(html);

        let fragment = self.create_fragment();
        if let Some(html) = self.first_child(scratch) {
            self.reparent_children(html, fragment);
        }
        self.release(scratch);
        if let Some(c) = context_copy.filter(|&c| self.parent(c).is_none()) {
            self.release(c);
        }
        fragment
    }

    /// Sets `Element.innerHTML`: replaces the element's children (a
    /// template's contents) with the parsed fragment. The old children are
    /// detached, not released.
    ///
    /// # Panics
    ///
    /// If `id` is not an element.
    pub fn set_inner_html(&mut self, id: NodeId, html: &str) {
        let fragment = self.parse_fragment(id, html);
        let target = self.template_contents(id).unwrap_or(id);
        self.replace_all(target, Some(fragment));
        self.release(fragment);
    }

    /// Sets `Element.outerHTML`: replaces the element with the parsed
    /// fragment, parsed in the context of its parent. Does nothing if the
    /// element has no parent; fails if the parent is the document.
    pub fn set_outer_html(&mut self, id: NodeId, html: &str) -> Result<(), DomError> {
        let Some(parent) = self.parent(id) else {
            return Ok(());
        };
        if self.kind(parent) == NodeKind::Document {
            return Err(DomError::NoModificationAllowed);
        }
        let fragment = self.parse_fragment_in(parent, html, false);
        let before = self.next_sibling(id);
        self.detach(id);
        self.insert(fragment, parent, before);
        self.release(fragment);
        Ok(())
    }

    /// `Element.insertAdjacentHTML`.
    pub fn insert_adjacent_html(
        &mut self,
        id: NodeId,
        position: AdjacentPosition,
        html: &str,
    ) -> Result<(), DomError> {
        let (context, parent, before) = match position {
            AdjacentPosition::BeforeBegin | AdjacentPosition::AfterEnd => {
                let parent = self.parent(id);
                let Some(parent) = parent.filter(|&p| self.kind(p) != NodeKind::Document) else {
                    return Err(DomError::NoModificationAllowed);
                };
                let before = if position == AdjacentPosition::BeforeBegin {
                    Some(id)
                } else {
                    self.next_sibling(id)
                };
                (parent, parent, before)
            }
            AdjacentPosition::AfterBegin => (id, id, self.first_child(id)),
            AdjacentPosition::BeforeEnd => (id, id, None),
        };
        let fragment = self.parse_fragment_in(context, html, true);
        self.insert(fragment, parent, before);
        self.release(fragment);
        Ok(())
    }

    /// Fragment parsing in the context of `context`, substituting a `<body>`
    /// context when `context` is not an element, or (if `html_means_body`)
    /// is the `<html>` element. `outerHTML` substitutes only for fragment
    /// parents; `insertAdjacentHTML` for `<html>` too.
    fn parse_fragment_in(&mut self, context: NodeId, html: &str, html_means_body: bool) -> NodeId {
        if self.is_element(context) && !(html_means_body && self.is_html(context, Name::HTML)) {
            return self.parse_fragment(context, html);
        }
        let body = self.create_element_named(Name::NS_HTML, Name::BODY);
        let fragment = self.parse_fragment(body, html);
        self.release(body);
        fragment
    }
}

/// The html5ever tree sink that builds into a [`Document`].
struct Sink<'a> {
    doc: RefCell<&'a mut Document>,
    /// The document node html5ever builds under: the real one for document
    /// parsing, a scratch one for fragments.
    document: NodeId,
    fragment: bool,
    /// html5ever's atom for each local name seen, indexed by [`Name`] id, so
    /// `elem_name` does not have to re-create atoms from strings.
    local_atoms: RefCell<Vec<Option<LocalName>>>,
    /// The first element created: html5ever's copy of the fragment context.
    first_element: Cell<Option<NodeId>>,
    /// Text nodes appended to after creation, to shrink when parsing ends.
    grown_text: RefCell<Vec<NodeId>>,
}

impl<'a> Sink<'a> {
    fn new(doc: &'a mut Document, document: NodeId, fragment: bool) -> Self {
        Sink {
            doc: RefCell::new(doc),
            document,
            fragment,
            local_atoms: RefCell::new(Vec::new()),
            first_element: Cell::new(None),
            grown_text: RefCell::new(Vec::new()),
        }
    }

    fn intern_qual_name(doc: &mut Document, name: &QualName) -> (Name, Name, Name) {
        let prefix = name.prefix.as_ref().map_or(Name::EMPTY, |p| doc.intern(p));
        (doc.intern(&name.ns), prefix, doc.intern(&name.local))
    }

    /// Appends text to `prev` if it is a text node, else returns false.
    fn append_to_text(&self, doc: &mut Document, prev: Option<NodeId>, text: &str) -> bool {
        match prev {
            Some(p) if doc.is_text(p) => {
                doc.text_mut(p).expect("is text").push_str(text);
                self.grown_text.borrow_mut().push(p);
                true
            }
            _ => false,
        }
    }
}

/// An element's name as html5ever atoms.
#[derive(Debug)]
struct AtomName {
    ns: Namespace,
    local: LocalName,
}

impl ElemName for AtomName {
    fn ns(&self) -> &Namespace {
        &self.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.local
    }
}

impl TreeSink for Sink<'_> {
    type Handle = NodeId;
    type Output = Option<NodeId>;
    type ElemName<'b>
        = AtomName
    where
        Self: 'b;

    fn finish(self) -> Option<NodeId> {
        let doc = self.doc.into_inner();
        for id in self.grown_text.into_inner() {
            if let Some(s) = doc.text_mut(id) {
                s.shrink_to_fit();
            }
        }
        self.first_element.get()
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> NodeId {
        self.document
    }

    fn elem_name<'b>(&'b self, target: &'b NodeId) -> AtomName {
        let doc = self.doc.borrow();
        let ns = doc.namespace(*target).expect("elem_name of a non-element");
        let local = doc.local_name(*target).expect("is an element");
        let ns = match ns {
            Name::NS_HTML => ns!(html),
            Name::NS_SVG => ns!(svg),
            Name::NS_MATHML => ns!(mathml),
            other => Namespace::from(doc.name_str(other)),
        };
        let local = match self.local_atoms.borrow().get(local.to_raw() as usize) {
            Some(Some(atom)) => atom.clone(),
            _ => LocalName::from(doc.name_str(local)),
        };
        AtomName { ns, local }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        el_flags: ElementFlags,
    ) -> NodeId {
        let mut doc = self.doc.borrow_mut();
        let (ns, _prefix, local) = Self::intern_qual_name(&mut doc, &name);
        let attrs: Box<[Attr]> = attrs
            .iter()
            .map(|a| {
                let (ns, prefix, local) = Self::intern_qual_name(&mut doc, &a.name);
                doc.new_attr(ns, prefix, local, &a.value)
            })
            .collect();
        let id = doc.create_element_with_attrs(ns, local, attrs);
        if el_flags.mathml_annotation_xml_integration_point {
            doc.set_flag(id, flags::MATHML_HTML_INTEGRATION_POINT);
        }

        let mut atoms = self.local_atoms.borrow_mut();
        let i = local.to_raw() as usize;
        if atoms.len() <= i {
            atoms.resize(i + 1, None);
        }
        atoms[i].get_or_insert(name.local);
        if self.first_element.get().is_none() {
            self.first_element.set(Some(id));
        }
        id
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.doc.borrow_mut().create_comment(&text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        // Only reachable when parsing XML; the HTML parser turns processing
        // instructions into comments itself.
        self.doc
            .borrow_mut()
            .create_comment(&format!("?{target} {data}"))
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let mut doc = self.doc.borrow_mut();
        match child {
            NodeOrText::AppendNode(n) => doc.insert_unchecked(*parent, n, None),
            NodeOrText::AppendText(text) => {
                let last = doc.last_child(*parent);
                if !self.append_to_text(&mut doc, last, &text) {
                    let t = doc.create_text(&text);
                    doc.insert_unchecked(*parent, t, None);
                }
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        let has_parent = self.doc.borrow().parent(*element).is_some();
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut doc = self.doc.borrow_mut();
        let dt = doc.create_doctype(&name, &public_id, &system_id);
        doc.insert_unchecked(self.document, dt, None);
    }

    fn mark_script_already_started(&self, node: &NodeId) {
        self.doc.borrow_mut().set_script_already_started(*node);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.doc
            .borrow()
            .template_contents(*target)
            .expect("template element has contents")
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: H5QuirksMode) {
        if !self.fragment {
            self.doc.borrow_mut().set_quirks_mode(match mode {
                H5QuirksMode::NoQuirks => QuirksMode::NoQuirks,
                H5QuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
                H5QuirksMode::Quirks => QuirksMode::Quirks,
            });
        }
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let mut doc = self.doc.borrow_mut();
        let Some(parent) = doc.parent(*sibling) else {
            return;
        };
        match new_node {
            NodeOrText::AppendNode(n) => doc.insert_unchecked(parent, n, Some(*sibling)),
            NodeOrText::AppendText(text) => {
                let prev = doc.prev_sibling(*sibling);
                if !self.append_to_text(&mut doc, prev, &text) {
                    let t = doc.create_text(&text);
                    doc.insert_unchecked(parent, t, Some(*sibling));
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<Attribute>) {
        let mut doc = self.doc.borrow_mut();
        for a in &attrs {
            let (ns, prefix, local) = Self::intern_qual_name(&mut doc, &a.name);
            doc.add_attr_if_missing(*target, ns, prefix, local, &a.value);
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.doc.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        self.doc.borrow_mut().reparent_children(*node, *new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.doc
            .borrow()
            .has_flag(*handle, flags::MATHML_HTML_INTEGRATION_POINT)
    }

    fn allow_declarative_shadow_roots(&self, _intended_parent: &NodeId) -> bool {
        // No shadow DOM: a `<template shadowrootmode>` stays a template.
        false
    }
}
