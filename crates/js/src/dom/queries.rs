//! The native members that query the tree: `getElementById`,
//! `querySelector` and `querySelectorAll`, `matches` and `closest`.
//!
//! They run with the document borrowed and reach dasher-dom's id index and
//! dasher-css's query plans directly, with the selector parsed once and
//! cached by text (`DomState::selector`). `querySelectorAll` returns what
//! the glue did: an array of the matches with `NodeList.prototype`.

use std::rc::Rc;

use dasher_css::SelectorList;
use dasher_dom::NodeId;
use rquickjs::qjs;

type Result<T, E = Thrown> = std::result::Result<T, E>;

use super::members::{Call, Outcome, Ret, Thrown};

/// The selector list for argument 0, or the `SyntaxError` the glue threw.
fn selector(c: &Call<'_>) -> Result<Rc<SelectorList>, Thrown> {
    let text = c.str_arg(0)?;
    let text = text.as_str();
    c.dom.selector(&text).map_err(|_| {
        Thrown::Dom(
            "SyntaxError",
            format!("'{text}' is not a valid selector."),
        )
    })
}

pub(super) fn get_element_by_id<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let id = c.str_arg(0)?;
    Ok(Ret::Node(c.doc.get_element_by_id(n, &id.as_str())))
}

pub(super) fn query_selector<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Node(list.query_first(&c.doc, n)))
}

pub(super) fn query_selector_all<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let list = selector(c)?;
    let found: Vec<NodeId> = list.query_all(&c.doc, n).collect();
    let mut values = Vec::with_capacity(found.len());
    for id in found {
        let v = c.value(Ret::Node(Some(id)));
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(v) } {
            for v in values {
                // SAFETY: the values made so far are ours.
                unsafe { qjs::JS_FreeValue(c.ctx, v) };
            }
            return Err(Thrown::Pending);
        }
        values.push(v);
    }
    let count = i32::try_from(values.len()).map_err(|_| Thrown::Type(c"Too many results."))?;
    // SAFETY: the array takes the values' references.
    let array = unsafe { qjs::JS_NewArrayFrom(c.ctx, count, values.as_ptr()) };
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(array) } {
        return Err(Thrown::Pending);
    }
    if !c.members.static_node_list(c.ctx, array) {
        // SAFETY: `array` is ours.
        unsafe { qjs::JS_FreeValue(c.ctx, array) };
        return Err(Thrown::Pending);
    }
    Ok(Ret::Value(array))
}

pub(super) fn matches<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Bool(list.matches(&c.doc, n)))
}

pub(super) fn closest<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Node(list.closest(&c.doc, n)))
}
