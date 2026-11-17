//! The native members that query the tree: `getElementById`,
//! `querySelector` and `querySelectorAll`, `matches` and `closest`.
//!
//! They run with the document borrowed and reach dasher-dom's id index and
//! dasher-css's query plans directly, with the selector parsed once and
//! cached by text (`DomState::selector`), and what `querySelector` and
//! `querySelectorAll` found kept until the DOM next changes
//! (`DomState::query_first`, `DomState::query_all`). `querySelectorAll`
//! returns a static `NodeList` of the matches (see the `lists` module).

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
    c.dom
        .selector(&text)
        .map_err(|_| Thrown::Dom("SyntaxError", format!("'{text}' is not a valid selector.")))
}

pub(super) fn get_element_by_id<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let id = c.str_arg(0)?;
    Ok(Ret::Node(c.doc.get_element_by_id(n, &id.as_str())))
}

pub(super) fn query_selector<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Node(c.dom.query_first(&c.doc, n, &list)))
}

pub(super) fn query_selector_all<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let list = selector(c)?;
    let found = c.dom.query_all(&c.doc, n, &list);
    let mut values = Vec::with_capacity(found.len());
    for &id in found.iter() {
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
    let proto = c.members.proto(super::lists::Proto::NodeList);
    let list = c.members.lists().create_static(c.ctx, proto, values);
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(list) } {
        return Err(Thrown::Pending);
    }
    Ok(Ret::Value(list))
}

pub(super) fn matches<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Bool(list.matches(&c.doc, n)))
}

pub(super) fn closest<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = selector(c)?;
    Ok(Ret::Node(list.closest(&c.doc, n)))
}
