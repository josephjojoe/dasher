//! The dasher DOM.
//!
//! An arena-allocated, index-based document tree with interned names, built
//! from html5ever's tokenizer and tree builder. Nodes are addressed by id
//! rather than by reference-counted pointers, and the whole tree is freed in
//! one go when its document is dropped.
//!
//! Nothing here knows about JavaScript; the JS bindings wrap a [`Document`]
//! and map [`DomError`]s to `DOMException`s.
//!
//! ```
//! use dasher_dom::{Document, Name};
//!
//! let mut doc = Document::parse("<ul id=list><li class=item>one</ul>");
//! let list = doc.get_element_by_id(doc.root(), "list").unwrap();
//! doc.set_inner_html(list, "<li class=item>two<li class=item>three");
//! let items: Vec<_> = doc.elements_by_class_name(doc.root(), "item").collect();
//! assert_eq!(items.len(), 2);
//! assert!(doc.is_html(items[0], Name::LI));
//! assert_eq!(doc.text_content(list).as_deref(), Some("twothree"));
//! assert_eq!(doc.outer_html(list), r#"<ul id="list"><li class="item">two</li><li class="item">three</li></ul>"#);
//! ```
//!
//! # Shadow trees
//!
//! [`Document::attach_shadow`] gives an element a shadow root: a fragment
//! that is not one of its children but is tied to it, as template contents
//! are to their template. The parser makes them from declarative shadow DOM
//! (`<template shadowrootmode="open">`) too. Plain serialization leaves
//! shadow trees out, as `innerHTML` does; [`Document::outer_html_with`] and
//! [`ShadowRoots`] write them back as declarative shadow DOM.
//!
//! # Incremental parsing
//!
//! [`Document::parse`] builds a whole document at once. A page loader uses
//! [`DocumentParser`] instead: it takes input in chunks as it arrives,
//! pauses after every `</script>` so the script can run against the tree
//! built so far, and accepts `document.write()` output at the insertion
//! point. It borrows the document only while it parses, so scripts can
//! change the tree between steps.
//!
//! # Extra documents
//!
//! Scripts can make documents besides the page's (`DOMParser`,
//! `createHTMLDocument`). They live in the same arena, each as a tree under
//! a document node of its own from [`Document::create_document`], so nodes
//! move between them without copying. [`Document::parse_document_into`]
//! fills one from HTML and [`Document::parse_xml_into`] from XML, with a
//! small non-validating parser of dasher's own.
//!
//! # Lookups by id
//!
//! [`Document::get_element_by_id`] does not walk the tree. The document
//! keeps one index of every element in its arena that has an `id`, kept up
//! to date when elements are created, cloned or released and when `id`
//! attributes change; inserting and removing nodes never touches it. A
//! lookup takes the elements with that id and keeps those under the root
//! asked about, so the document, each shadow root, template contents,
//! detached trees and extra documents each see only their own, and a
//! repeated id gives the first in tree order. dasher-css uses the same
//! index ([`Document::elements_with_id`]) for selectors with an `#id`. It
//! costs about 12 bytes per element with an id; `tests/ids.rs` checks it
//! against a tree walk over random edits.
//!
//! # Memory
//!
//! Footprint matters more than speed here, so the layout is deliberately
//! tight:
//!
//! - Every node is one 56-byte arena slot: five 4-byte links (parent, first
//!   and last child, previous and next sibling), a flags byte and a 32-byte
//!   payload. Empty elements and short text need no other allocation.
//! - Names are 4-byte ids into a per-document interner ([`Names`]); common
//!   HTML names are constants such as [`Name::DIV`].
//! - Text, comments and attribute values up to 24 bytes are stored inline.
//!   An element's attributes are one exact-size boxed slice of 32-byte
//!   records.
//! - Rare state (template contents, shadow roots, element prefixes,
//!   attribute namespaces) lives in side tables rather than in every node.
//!
//! `tests/memory.rs` measures the real heap cost with a counting allocator
//! and fails if it regresses. On an indented 90k-node listing page a parsed
//! tree costs about 70 bytes and 0.33 heap allocations per node.
//!
//! Nodes removed from the tree stay allocated, because script may still
//! hold them, until [`Document::release`] frees them.
//!
//! [`Document::set_node_limit`] caps how many nodes a document should
//! hold. Crossing it is recorded rather than refused, since node creation
//! cannot fail, and bulk operations (fragment parsing, deep clones) stop
//! early once over, so the page driving the document can stop it with a
//! bounded overshoot.

mod document;
mod error;
mod ids;
mod name;
mod node;
mod parse;
mod query;
mod serialize;
mod shadow;
mod stream;
mod xml;

pub use document::{Children, Descendants, Document, QuirksMode};
pub use error::DomError;
pub use name::{Name, Names};
pub use node::{Attr, Doctype, NodeId, NodeKind};
pub use parse::{AdjacentPosition, ParseOptions};
pub use serialize::ShadowRoots;
pub use shadow::{ShadowIncluding, ShadowRootInit, ShadowRootMode, is_valid_custom_element_name};
pub use stream::{DocumentParser, ParseStep, TreeChange};
pub use xml::XmlError;
