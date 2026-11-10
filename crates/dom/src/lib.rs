//! The dasher DOM.
//!
//! An arena-allocated, index-based document tree with interned names, built
//! from html5ever's tokenizer and tree builder. Nodes are addressed by id
//! rather than by reference-counted pointers, and the whole tree is freed in
//! one go when its document is dropped.
//!
//! Nothing here knows about JavaScript; the JS bindings wrap a [`Document`]
//! and map [`DomError`]s to `DOMException`s.

mod document;
mod error;
mod name;
mod node;
mod parse;
mod serialize;

pub use document::{Children, Descendants, Document, QuirksMode};
pub use error::DomError;
pub use name::{Name, Names};
pub use node::{Attr, Doctype, NodeId, NodeKind};
pub use parse::{AdjacentPosition, ParseOptions};
