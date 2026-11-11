//! ES modules.
//!
//! The pieces the module loader is built from: finding a script's imports
//! without parsing it ([`lexer`]) and resolving module specifiers with
//! import maps ([`resolve`]).

// The loader that uses these comes in the next change.
#![allow(dead_code)]

mod lexer;
mod resolve;
