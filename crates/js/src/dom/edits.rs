//! The native members that change the tree: `appendChild` and the other
//! `Node` edits, the `ParentNode` and `ChildNode` mixins (`append`,
//! `before`, `remove`...), `cloneNode`, the `textContent` and `innerHTML`
//! setters and the document's `create*` methods.
//!
//! Each runs the same mutation native in `natives` that the JS glue called
//! (`insert`, `detach`, `replace_child`...), so mutation records, custom
//! element reactions, wrapper groups and DOM activity come out exactly as
//! before. Scripts and loading links that an insertion connects go to the
//! glue's `connected()` (see `Members::connected`), as they did.
//!
//! Anything unusual (an argument that is not a node where one is needed, an
//! object to convert to a string among nodes, a custom element or script
//! to create, an extra document) falls back to the JS implementation, which
//! `dom.js` keeps as the member's body; a body asks for it with
//! `Thrown::Fallback` before it has changed anything.

use dasher_dom::{DomError, Name, NodeId, NodeKind};
use rquickjs::qjs;

type Result<T, E = Thrown> = std::result::Result<T, E>;

use super::members::{Ret, Thrown, Write};
use super::natives::{
    self, detach, insert, replace_all, replace_child, set_inner_html, set_text_content,
    valid_element_name,
};

type Done = Result<Ret<'static>, Thrown>;

impl From<DomError> for Thrown {
    fn from(e: DomError) -> Self {
        Thrown::Dom(e.name(), e.to_string())
    }
}

/// The node argument `i` is, `None` if it is not a node wrapper.
fn node_arg(w: &Write<'_>, i: usize) -> Result<Option<NodeId>, Thrown> {
    // SAFETY: the argument is alive for the call.
    let Some(wrapper) = (unsafe { w.members.wrapper(w.ctx, w.arg(i)) }) else {
        return Ok(None);
    };
    wrapper.id().map(Some).ok_or(Thrown::Stale)
}

/// A node argument that must be there, or the JS implementation throws
/// what it throws.
fn required_node(w: &Write<'_>, i: usize) -> Result<NodeId, Thrown> {
    node_arg(w, i)?.ok_or(Thrown::Fallback)
}

/// Argument `i` as a new reference, to return it.
fn returned(w: &Write<'_>, i: usize) -> Ret<'static> {
    // SAFETY: the argument is alive; the duplicate goes to the caller.
    Ret::Value(unsafe { qjs::JS_DupValue(w.ctx, w.arg(i)) })
}

/// Hands the scripts an insertion connected to the glue.
fn connected(w: &Write<'_>, scripts: &[NodeId]) -> Result<(), Thrown> {
    if scripts.is_empty() {
        return Ok(());
    }
    w.members.connected(&w.rctx(), w.dom, scripts)?;
    Ok(())
}

pub(super) fn append_child(w: &Write<'_>) -> Done {
    let node = required_node(w, 0)?;
    let scripts = insert(&w.rctx(), w.dom, w.id, node, None)?;
    connected(w, &scripts)?;
    Ok(returned(w, 0))
}

pub(super) fn insert_before(w: &Write<'_>) -> Done {
    let node = required_node(w, 0)?;
    let child = w.arg(1);
    // SAFETY: tag checks of a live value.
    let before = if unsafe { qjs::JS_IsNull(child) || qjs::JS_IsUndefined(child) } {
        None
    } else {
        Some(required_node(w, 1)?)
    };
    let scripts = insert(&w.rctx(), w.dom, w.id, node, before)?;
    connected(w, &scripts)?;
    Ok(returned(w, 0))
}

pub(super) fn remove_child(w: &Write<'_>) -> Done {
    let child = required_node(w, 0)?;
    if w.dom.doc().parent(child) != Some(w.id) {
        return Err(DomError::NotFound.into());
    }
    detach(&w.rctx(), w.dom, child)?;
    Ok(returned(w, 0))
}

pub(super) fn replace_child_member(w: &Write<'_>) -> Done {
    let node = required_node(w, 0)?;
    let child = required_node(w, 1)?;
    let scripts = replace_child(&w.rctx(), w.dom, w.id, node, child)?;
    connected(w, &scripts)?;
    Ok(returned(w, 1))
}

pub(super) fn remove(w: &Write<'_>) -> Done {
    detach(&w.rctx(), w.dom, w.id)?;
    Ok(Ret::Undefined)
}

// -------------------------------------------------------------------------
// The variadic mixin members.

/// The arguments of `append()` and friends, checked: nodes and primitives
/// (made text nodes). An object that is not a node would be converted to a
/// string by running JS, and a symbol converts differently in the glue, so
/// either falls back.
struct Nodes<'w> {
    dom: &'w super::DomState,
    /// While the call holds ids of the unwrapped nodes it made, no tree
    /// may be freed: a tree is garbage once nothing wraps any of its nodes.
    hold: Option<super::HoldGarbage<'w>>,
    /// Each argument: a node, or `None` for a primitive.
    args: Vec<Option<NodeId>>,
    /// The nodes made for the call, queued to be freed (if they end up
    /// unused) once it is over.
    made: std::cell::RefCell<Vec<NodeId>>,
}

impl Drop for Nodes<'_> {
    fn drop(&mut self) {
        self.hold = None;
        for &n in self.made.get_mut().iter() {
            self.dom.add_candidate(n);
        }
    }
}

impl<'w> Nodes<'w> {
    fn of(w: &Write<'w>) -> Result<Nodes<'w>, Thrown> {
        let mut args = Vec::with_capacity(w.args.len());
        for i in 0..w.args.len() {
            let node = node_arg(w, i)?;
            if node.is_none() {
                // SAFETY: a tag check of a live value.
                let tag = unsafe { qjs::JS_VALUE_GET_TAG(w.arg(i)) };
                if tag == qjs::JS_TAG_OBJECT || tag == qjs::JS_TAG_SYMBOL {
                    return Err(Thrown::Fallback);
                }
            }
            args.push(node);
        }
        Ok(Nodes {
            dom: w.dom,
            hold: Some(w.dom.hold_garbage()),
            args,
            made: std::cell::RefCell::default(),
        })
    }

    fn contains(&self, n: NodeId) -> bool {
        self.args.contains(&Some(n))
    }

    /// The one node the arguments stand for, as the glue's
    /// `convertNodes`: a lone node, a new text node, or a new fragment of
    /// them all.
    fn convert(&self, w: &Write<'_>) -> Result<NodeId, Thrown> {
        let text = |i: usize| -> Result<NodeId, Thrown> {
            // A primitive: converting it runs no JS.
            let s = w.str_arg(i)?;
            let t = w.dom.doc_mut().create_text(&s.as_str());
            self.made.borrow_mut().push(t);
            Ok(t)
        };
        if let [one] = self.args[..] {
            return match one {
                Some(n) => Ok(n),
                None => text(0),
            };
        }
        let fragment = w.dom.doc_mut().create_fragment();
        self.made.borrow_mut().push(fragment);
        for (i, &arg) in self.args.iter().enumerate() {
            let n = match arg {
                Some(n) => n,
                None => text(i)?,
            };
            insert(&w.rctx(), w.dom, fragment, n, None)?;
        }
        Ok(fragment)
    }
}

/// The glue's `viableSibling`: the nearest sibling of `n` in the direction
/// of `step` that is not among the arguments.
fn viable_sibling(
    w: &Write<'_>,
    nodes: &Nodes,
    step: fn(&dasher_dom::Document, NodeId) -> Option<NodeId>,
) -> Option<NodeId> {
    let doc = w.dom.doc();
    let mut s = step(&doc, w.id);
    while let Some(x) = s.filter(|&x| nodes.contains(x)) {
        s = step(&doc, x);
    }
    s
}

pub(super) fn append(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let node = nodes.convert(w)?;
    let scripts = insert(&w.rctx(), w.dom, w.id, node, None)?;
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

pub(super) fn prepend(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let node = nodes.convert(w)?;
    let first = w.dom.doc().first_child(w.id);
    let scripts = insert(&w.rctx(), w.dom, w.id, node, first)?;
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

pub(super) fn replace_children(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let node = if nodes.args.is_empty() {
        None
    } else {
        Some(nodes.convert(w)?)
    };
    let scripts = replace_all(&w.rctx(), w.dom, w.id, node)?;
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

pub(super) fn before(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let Some(parent) = w.dom.doc().parent(w.id) else {
        return Ok(Ret::Undefined);
    };
    let prev = viable_sibling(w, &nodes, dasher_dom::Document::prev_sibling);
    let node = nodes.convert(w)?;
    let before = {
        let doc = w.dom.doc();
        match prev {
            None => doc.first_child(parent),
            Some(p) => doc.next_sibling(p),
        }
    };
    let scripts = insert(&w.rctx(), w.dom, parent, node, before)?;
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

pub(super) fn after(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let Some(parent) = w.dom.doc().parent(w.id) else {
        return Ok(Ret::Undefined);
    };
    let next = viable_sibling(w, &nodes, dasher_dom::Document::next_sibling);
    let node = nodes.convert(w)?;
    let scripts = insert(&w.rctx(), w.dom, parent, node, next)?;
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

pub(super) fn replace_with(w: &Write<'_>) -> Done {
    let nodes = Nodes::of(w)?;
    let Some(parent) = w.dom.doc().parent(w.id) else {
        return Ok(Ret::Undefined);
    };
    let next = viable_sibling(w, &nodes, dasher_dom::Document::next_sibling);
    let node = nodes.convert(w)?;
    let scripts = if w.dom.doc().parent(w.id) == Some(parent) {
        replace_child(&w.rctx(), w.dom, parent, node, w.id)?
    } else {
        insert(&w.rctx(), w.dom, parent, node, next)?
    };
    connected(w, &scripts)?;
    Ok(Ret::Undefined)
}

// -------------------------------------------------------------------------
// Content setters and cloning.

/// A setter's value as a string, with `null` (and, if `undefined_empty`,
/// `undefined`) as the empty string.
fn text_value(
    w: &Write<'_>,
    undefined_empty: bool,
) -> Result<super::members::JsStr, Thrown> {
    let v = w.arg(0);
    // SAFETY: tag checks of a live value.
    let empty = unsafe { qjs::JS_IsNull(v) || (undefined_empty && qjs::JS_IsUndefined(v)) };
    if empty {
        Ok(super::members::JsStr::Static(""))
    } else {
        w.str_arg(0)
    }
}

pub(super) fn set_text_content_member(w: &Write<'_>) -> Done {
    let text = text_value(w, true)?;
    set_text_content(&w.rctx(), w.dom, w.id, &text.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn set_inner_html_member(w: &Write<'_>) -> Done {
    let html = text_value(w, false)?;
    set_inner_html(&w.rctx(), w.dom, w.id, &html.as_str())?;
    Ok(Ret::Undefined)
}

pub(super) fn clone_node(w: &Write<'_>) -> Done {
    // `!!deep` runs no JS.
    // SAFETY: the argument is alive.
    let deep = unsafe { qjs::JS_ToBool(w.ctx, w.arg(0)) } > 0;
    let copy = {
        let mut doc = w.dom.doc_mut();
        // The glue's cloning steps copy form control state, and copies of
        // custom elements are upgraded: leave those to it.
        let stateful = doc.is_html(w.id, Name::INPUT) || doc.is_html(w.id, Name::TEXTAREA);
        if stateful || w.dom.custom_elements_defined() {
            return Err(Thrown::Fallback);
        }
        natives::clone_into_owner(w.dom, &mut doc, w.id, deep)
    };
    Ok(Ret::Node(Some(copy)))
}

// -------------------------------------------------------------------------
// The document's factories.

/// Whether `this` is the page's document, the one the fast paths handle:
/// extra documents set node documents and keep XML names.
fn page_document(w: &Write<'_>) -> bool {
    let doc = w.dom.doc();
    w.id == doc.root() && doc.kind(w.id) == NodeKind::Document
}

pub(super) fn create_element(w: &Write<'_>) -> Done {
    let options = w.arg(1);
    // SAFETY: tag checks of live values.
    let plain = unsafe {
        (qjs::JS_IsUndefined(options) || qjs::JS_IsNull(options))
            && qjs::JS_VALUE_GET_TAG(w.arg(0)) != qjs::JS_TAG_OBJECT
            && qjs::JS_VALUE_GET_TAG(w.arg(0)) != qjs::JS_TAG_SYMBOL
    };
    if !plain || !page_document(w) {
        return Err(Thrown::Fallback);
    }
    let name = w.str_arg(0)?;
    let name = name.as_str();
    if !valid_element_name(&name) {
        return Err(Thrown::Dom(
            "InvalidCharacterError",
            format!("The tag name provided ('{name}') is not a valid name."),
        ));
    }
    let lower = name.to_ascii_lowercase();
    // Scripts made by script are marked async, and custom elements are
    // made by the registry: both in the glue.
    if lower == "script" || w.dom.has_definition(&lower) {
        return Err(Thrown::Fallback);
    }
    let n = w.dom.doc_mut().create_element(&lower);
    Ok(Ret::Node(Some(n)))
}

/// `createTextNode` and `createComment` on the page's document.
fn create_character_data(w: &Write<'_>, comment: bool) -> Done {
    if !page_document(w) {
        return Err(Thrown::Fallback);
    }
    let data = w.str_arg(0)?;
    let data = data.as_str();
    let mut doc = w.dom.doc_mut();
    let n = if comment {
        doc.create_comment(&data)
    } else {
        doc.create_text(&data)
    };
    Ok(Ret::Node(Some(n)))
}

pub(super) fn create_text_node(w: &Write<'_>) -> Done {
    create_character_data(w, false)
}

pub(super) fn create_comment(w: &Write<'_>) -> Done {
    create_character_data(w, true)
}

pub(super) fn create_document_fragment(w: &Write<'_>) -> Done {
    if !page_document(w) {
        return Err(Thrown::Fallback);
    }
    let n = w.dom.doc_mut().create_fragment();
    Ok(Ret::Node(Some(n)))
}
