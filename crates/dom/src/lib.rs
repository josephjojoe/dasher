//! The dasher DOM.
//!
//! An arena-allocated, index-based document tree with interned names, built
//! from html5ever's tokenizer and tree builder. Nodes are addressed by id
//! rather than by reference-counted pointers, and the whole tree is freed in
//! one go when its document is dropped.
