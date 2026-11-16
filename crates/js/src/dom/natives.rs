//! The native functions `dom.js` builds the DOM API on, installed as
//! `native.dom`.
//!
//! They take node wrappers and plain values, leave argument conversion and
//! most API surface to the glue, and throw `DOMException`s for DOM errors.
//! Every change to the tree goes through the mutation functions here, which
//! keep the wrapper groups in step (see the `dom` module docs).

use std::borrow::Cow;
use std::rc::Rc;

use dasher_dom::{AdjacentPosition, Document, DomError, Name, NodeId, NodeKind, QuirksMode};
use rquickjs::function::IntoJsFunc;
use rquickjs::{Array, Class, Ctx, Function, IntoJs, Object, Result, Value, qjs};

use super::DomState;
use super::collection::Filter;
use super::inner_text::inner_text;
use super::layout;
use super::lists;
use super::mutation::deliver;
use super::wrapper::{self, DomData, Move, Node, after_insert, after_remove, plan_insert, wrap};

/// Throws a `DOMException` with this name and message.
pub(crate) fn throw_dom(ctx: &Ctx<'_>, name: &str, message: &str) -> rquickjs::Error {
    let make = ctx.userdata::<DomData<'_>>().map(|d| d.exception.clone());
    let Some(make) = make else {
        return rquickjs::Exception::throw_message(ctx, &format!("{name}: {message}"));
    };
    match make.call::<_, Value<'_>>((name, message)) {
        Ok(exception) => ctx.throw(exception),
        Err(e) => e,
    }
}

pub(super) fn dom_error(ctx: &Ctx<'_>, error: DomError) -> rquickjs::Error {
    throw_dom(ctx, error.name(), &error.to_string())
}

/// A value that is `null` in JS when absent (rquickjs makes `None`
/// `undefined`).
pub(super) struct Nullable<T>(pub(super) Option<T>);

impl<'js, T: IntoJs<'js>> IntoJs<'js> for Nullable<T> {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        match self.0 {
            Some(v) => v.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

/// The node behind a wrapper.
pub(super) fn id(ctx: &Ctx<'_>, node: &Node<'_>) -> Result<NodeId> {
    node.borrow().id().ok_or_else(|| stale_node_error(ctx))
}

/// Throws the error for using a node of a replaced document.
pub(super) fn stale_node_error(ctx: &Ctx<'_>) -> rquickjs::Error {
    throw_dom(
        ctx,
        "InvalidStateError",
        "The node belongs to a document that has been replaced.",
    )
}

/// The element behind a wrapper; a `TypeError` for other nodes, which page
/// code can pass by calling an `Element` method on them.
pub(super) fn element(ctx: &Ctx<'_>, dom: &DomState, node: &Node<'_>) -> Result<NodeId> {
    let n = id(ctx, node)?;
    if dom.doc().is_element(n) {
        Ok(n)
    } else {
        Err(rquickjs::Exception::throw_type(ctx, "Illegal invocation"))
    }
}

fn or_null<'js>(ctx: &Ctx<'js>, node: Option<Node<'js>>) -> Value<'js> {
    node.map_or_else(|| Value::new_null(ctx.clone()), Class::into_value)
}

pub(super) fn wrap_opt<'js>(
    ctx: &Ctx<'js>,
    dom: &Rc<DomState>,
    id: Option<NodeId>,
) -> Result<Value<'js>> {
    let node = id.map(|id| wrap(ctx, dom, id)).transpose()?;
    Ok(or_null(ctx, node))
}

fn wrap_all<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>, ids: &[NodeId]) -> Result<Vec<Node<'js>>> {
    ids.iter().map(|&id| wrap(ctx, dom, id)).collect()
}

/// Whether an element name is acceptable to `createElement`: the XML
/// `Name` rules, loosely (non-ASCII characters are all allowed).
pub(super) fn valid_element_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == ':' || !first.is_ascii())
        && chars.all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':') || !c.is_ascii()
        })
}

/// Whether a name is acceptable to `setAttribute`.
pub(super) fn valid_attribute_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|c| {
            matches!(
                c,
                '\0' | '\t' | '\n' | '\x0C' | '\r' | ' ' | '/' | '>' | '=' | '"' | '\''
            )
        })
}

/// Lower-cases `name` if `id` is an HTML element, as attribute and tag name
/// lookups do in HTML documents. Only copies a name that has upper case.
pub(super) fn html_lower<'a>(doc: &Document, id: NodeId, name: &'a str) -> Cow<'a, str> {
    if doc.namespace(id) == Some(Name::NS_HTML) && name.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(name.to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

fn adjacent_position(ctx: &Ctx<'_>, position: &str) -> Result<AdjacentPosition> {
    match position.to_ascii_lowercase().as_str() {
        "beforebegin" => Ok(AdjacentPosition::BeforeBegin),
        "afterbegin" => Ok(AdjacentPosition::AfterBegin),
        "beforeend" => Ok(AdjacentPosition::BeforeEnd),
        "afterend" => Ok(AdjacentPosition::AfterEnd),
        _ => Err(throw_dom(
            ctx,
            "SyntaxError",
            &format!(
                "The value provided ('{position}') is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'."
            ),
        )),
    }
}

// -------------------------------------------------------------------------
// Mutations.
//
// Each records the changes it makes for mutation observers while the
// document is borrowed, then delivers them once the borrow is released.

/// `document.compatMode` for a quirks mode.
fn compat_mode(quirks: QuirksMode) -> &'static str {
    match quirks {
        QuirksMode::Quirks => "BackCompat",
        _ => "CSS1Compat",
    }
}

/// The script elements under `moved` that just became connected and have
/// not run, for the page loader to prepare, and the links that load
/// something, which get a load event.
fn connected_scripts(doc: &Document, moved: &[NodeId]) -> Vec<NodeId> {
    let mut scripts = Vec::new();
    for &root in moved {
        if !doc.is_connected(root) {
            continue;
        }
        let subtree = std::iter::once(root).chain(doc.descendants(root));
        scripts.extend(subtree.filter(|&n| {
            (doc.is_html(n, Name::SCRIPT) && !doc.script_already_started(n))
                || is_loading_link(doc, n)
        }));
    }
    scripts
}

/// Whether `node` is a `<link>` that loads something: a stylesheet or a
/// preload (of a module or anything else).
fn is_loading_link(doc: &Document, node: NodeId) -> bool {
    doc.is_html(node, Name::LINK)
        && doc.attribute(node, "rel").is_some_and(|rel| {
            rel.split_ascii_whitespace().any(|t| {
                ["stylesheet", "preload", "modulepreload"]
                    .iter()
                    .any(|r| t.eq_ignore_ascii_case(r))
            })
        })
}

/// The nodes `node` stands for when inserted: a fragment's children, or
/// `node` itself.
fn nodes_to_insert(doc: &Document, node: NodeId) -> Moved {
    if doc.kind(node) == NodeKind::DocumentFragment {
        doc.children(node).collect()
    } else {
        smallvec::smallvec![node]
    }
}

/// The nodes an insertion moves: one, unless a fragment's children.
type Moved = smallvec::SmallVec<[NodeId; 1]>;

/// Where a node is now: its parent and siblings, if it has a parent.
type Place = (NodeId, Option<NodeId>, Option<NodeId>);

fn place(doc: &Document, node: NodeId) -> Option<Place> {
    let parent = doc.parent(node)?;
    Some((parent, doc.prev_sibling(node), doc.next_sibling(node)))
}

/// The children of `parent` strictly between `prev` and `next`, where
/// `None` is the start or the end.
fn children_between(
    doc: &Document,
    parent: NodeId,
    prev: Option<NodeId>,
    next: Option<NodeId>,
) -> Vec<NodeId> {
    let first = match prev {
        Some(p) => doc.next_sibling(p),
        None => doc.first_child(parent),
    };
    std::iter::successors(first, |&n| doc.next_sibling(n))
        .take_while(|&n| Some(n) != next)
        .collect()
}

/// Records what inserting `node` (standing for `moved`) did besides adding
/// `moved` to its new parent: taking `node` out of its old parent (`old`),
/// or emptying the fragment it is.
fn record_moved_out(
    dom: &DomState,
    doc: &Document,
    node: NodeId,
    moved: &[NodeId],
    old: Option<Place>,
) {
    if let Some((parent, prev, next)) = old {
        dom.child_list(doc, parent, &[], &[node], prev, next);
    } else if doc.kind(node) == NodeKind::DocumentFragment {
        dom.child_list(doc, node, &[], moved, None, None);
    }
}

/// `parent.insertBefore(node, before)` on a borrowed document. Returns the
/// nodes inserted. With `record` unset, the insertion into `parent` itself
/// is left for the caller to record, as "replace all" does.
fn insert_into(
    ctx: &Ctx<'_>,
    dom: &DomState,
    doc: &mut Document,
    parent: NodeId,
    node: NodeId,
    before: Option<NodeId>,
    record: bool,
) -> std::result::Result<Moved, DomError> {
    let plan = plan_insert(doc, parent, node);
    let moved = nodes_to_insert(doc, node);
    let old = place(doc, node);
    doc.insert_before(parent, node, before)?;
    after_insert(ctx, dom, doc, parent, node, &moved, plan);
    dom.bump();
    record_moved_out(dom, doc, node, &moved, old);
    if let Some((old_parent, _, _)) = old {
        dom.ce_removed(ctx, doc, old_parent, &[node]);
    }
    dom.ce_inserted(doc, &moved);
    if record && let (Some(&first), Some(&last)) = (moved.first(), moved.last()) {
        let (prev, next) = (doc.prev_sibling(first), doc.next_sibling(last));
        dom.child_list(doc, parent, &moved, &[], prev, next);
    }
    Ok(moved)
}

/// `parent.insertBefore(node, before)`. Returns the scripts it connected.
pub(super) fn insert(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    parent: NodeId,
    node: NodeId,
    before: Option<NodeId>,
) -> Result<Vec<NodeId>> {
    let result = {
        let mut doc = dom.doc_mut();
        insert_into(ctx, dom, &mut doc, parent, node, before, true)
            .map(|moved| connected_scripts(&doc, &moved))
    };
    deliver(ctx, dom)?;
    result.map_err(|e| dom_error(ctx, e))
}

/// Detaches `node` from its parent, if it has one.
pub(super) fn detach(ctx: &Ctx<'_>, dom: &Rc<DomState>, node: NodeId) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        let Some((parent, prev, next)) = place(&doc, node) else {
            return Ok(());
        };
        doc.remove(node);
        dom.ce_removed(ctx, &doc, parent, &[node]);
        after_remove(ctx, dom, &doc, node, parent);
        dom.bump();
        dom.child_list(&doc, parent, &[], &[node], prev, next);
    }
    deliver(ctx, dom)
}

/// Detaches all children of `parent`, returning them. Leaves recording it
/// to the caller.
fn remove_children(
    ctx: &Ctx<'_>,
    dom: &DomState,
    doc: &mut Document,
    parent: NodeId,
) -> Vec<NodeId> {
    let old: Vec<NodeId> = doc.children(parent).collect();
    doc.remove_children(parent);
    dom.ce_removed(ctx, doc, parent, &old);
    for &child in &old {
        after_remove(ctx, dom, doc, child, parent);
    }
    old
}

pub(super) fn replace_child(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    parent: NodeId,
    node: NodeId,
    child: NodeId,
) -> Result<Vec<NodeId>> {
    let result = {
        let mut doc = dom.doc_mut();
        let plan = if node == child {
            Move::SameTree
        } else {
            plan_insert(&doc, parent, node)
        };
        let moved = nodes_to_insert(&doc, node);
        let old = place(&doc, node);
        // Where the new nodes go, as the DOM standard's "replace" works it
        // out before anything moves.
        let mut next = doc.next_sibling(child);
        if next == Some(node) {
            next = doc.next_sibling(node);
        }
        let mut prev = doc.prev_sibling(child);
        if prev == Some(node) {
            prev = doc.prev_sibling(node);
        }
        let result = doc.replace_child(parent, node, child);
        result.map(|_| {
            if node != child {
                dom.ce_removed(ctx, &doc, parent, &[child]);
                after_insert(ctx, dom, &doc, parent, node, &moved, plan);
                after_remove(ctx, dom, &doc, child, parent);
                dom.bump();
            }
            record_moved_out(dom, &doc, node, &moved, old);
            // Replacing a child with itself only moves it out and back in.
            let removed: &[NodeId] = if node == child { &[] } else { &[child] };
            dom.child_list(&doc, parent, &moved, removed, prev, next);
            if node != child {
                if let Some((old_parent, _, _)) = old {
                    dom.ce_removed(ctx, &doc, old_parent, &[node]);
                }
                dom.ce_inserted(&doc, &moved);
            }
            if node == child {
                Vec::new()
            } else {
                connected_scripts(&doc, &moved)
            }
        })
    };
    deliver(ctx, dom)?;
    result.map_err(|e| dom_error(ctx, e))
}

/// `replaceChildren`: replaces all of `parent`'s children with `node`.
pub(super) fn replace_all(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    parent: NodeId,
    node: Option<NodeId>,
) -> Result<Vec<NodeId>> {
    let result = {
        let mut doc = dom.doc_mut();
        if let Some(node) = node
            && doc.is_host_including_inclusive_ancestor(node, parent)
        {
            return Err(dom_error(ctx, DomError::HierarchyRequest));
        }
        let removed = remove_children(ctx, dom, &mut doc, parent);
        dom.bump();
        let added = match node {
            Some(node) => insert_into(ctx, dom, &mut doc, parent, node, None, false),
            None => Ok(Moved::new()),
        };
        let inserted: &[NodeId] = added.as_deref().unwrap_or(&[]);
        dom.child_list(&doc, parent, inserted, &removed, None, None);
        added.map(|moved| connected_scripts(&doc, &moved))
    };
    deliver(ctx, dom)?;
    result.map_err(|e| dom_error(ctx, e))
}

pub(super) fn set_text_content(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    node: NodeId,
    text: &str,
) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        match doc.kind(node) {
            NodeKind::Element | NodeKind::DocumentFragment => {
                let removed = remove_children(ctx, dom, &mut doc, node);
                let mut added = Vec::new();
                if !text.is_empty() {
                    let t = doc.create_text(text);
                    doc.append_child(node, t)
                        .expect("a text node fits anywhere");
                    added.push(t);
                }
                dom.bump();
                dom.child_list(&doc, node, &added, &removed, None, None);
            }
            NodeKind::Text | NodeKind::Comment => {
                dom.character_data(&doc, node);
                doc.set_text(node, text);
            }
            NodeKind::Document | NodeKind::Doctype => {}
        }
    }
    deliver(ctx, dom)
}

fn set_data(ctx: &Ctx<'_>, dom: &Rc<DomState>, node: NodeId, data: &str) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        dom.character_data(&doc, node);
        doc.set_text(node, data);
    }
    deliver(ctx, dom)
}

pub(super) fn set_inner_html(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    node: NodeId,
    html: &str,
) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        let target = doc.template_contents(node).unwrap_or(node);
        let removed = remove_children(ctx, dom, &mut doc, target);
        doc.set_inner_html(node, html);
        dom.bump();
        let added: Vec<NodeId> = doc.children(target).collect();
        dom.child_list(&doc, target, &added, &removed, None, None);
        dom.ce_inserted(&doc, &added);
    }
    deliver(ctx, dom)
}

fn set_outer_html(ctx: &Ctx<'_>, dom: &Rc<DomState>, node: NodeId, html: &str) -> Result<()> {
    let result = {
        let mut doc = dom.doc_mut();
        let old = place(&doc, node);
        let result = doc.set_outer_html(node, html);
        if let (Ok(()), Some((parent, prev, next))) = (&result, old) {
            dom.ce_removed(ctx, &doc, parent, &[node]);
            after_remove(ctx, dom, &doc, node, parent);
            dom.bump();
            let added = children_between(&doc, parent, prev, next);
            dom.child_list(&doc, parent, &added, &[node], prev, next);
            dom.ce_inserted(&doc, &added);
        }
        result
    };
    deliver(ctx, dom)?;
    result.map_err(|e| dom_error(ctx, e))
}

fn insert_adjacent_html(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    node: NodeId,
    position: AdjacentPosition,
    html: &str,
) -> Result<()> {
    let result = {
        let mut doc = dom.doc_mut();
        // Where the new nodes will go, to find them afterwards.
        let (parent, next) = match position {
            AdjacentPosition::BeforeBegin => (doc.parent(node), Some(node)),
            AdjacentPosition::AfterEnd => (doc.parent(node), doc.next_sibling(node)),
            AdjacentPosition::AfterBegin => (Some(node), doc.first_child(node)),
            AdjacentPosition::BeforeEnd => (Some(node), None),
        };
        let prev = match next {
            Some(n) => doc.prev_sibling(n),
            None => parent.and_then(|p| doc.last_child(p)),
        };
        let result = doc.insert_adjacent_html(node, position, html);
        dom.bump();
        if let (Ok(()), Some(parent)) = (&result, parent) {
            let added = children_between(&doc, parent, prev, next);
            dom.child_list(&doc, parent, &added, &[], prev, next);
            dom.ce_inserted(&doc, &added);
        }
        result
    };
    deliver(ctx, dom)?;
    result.map_err(|e| dom_error(ctx, e))
}

/// A copy of `n` (with its descendants if `deep`), which belongs to `n`'s
/// node document.
pub(super) fn clone_into_owner(
    dom: &DomState,
    doc: &mut Document,
    n: NodeId,
    deep: bool,
) -> NodeId {
    let copy = doc.clone_node(n, deep);
    dom.set_owner(doc, copy, dom.owner_document(doc, n));
    copy
}

/// `Node.normalize()`: drops empty text nodes and merges runs of adjacent
/// ones into the first, following the DOM standard's steps.
fn normalize(ctx: &Ctx<'_>, dom: &Rc<DomState>, root: NodeId) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        let texts: Vec<NodeId> = doc.descendants(root).filter(|&n| doc.is_text(n)).collect();
        let mut removed = Vec::new();
        let mut take_out = |doc: &mut Document, n: NodeId| {
            if let Some((parent, prev, next)) = place(doc, n) {
                doc.remove(n);
                dom.child_list(doc, parent, &[], &[n], prev, next);
                removed.push((n, parent));
            }
        };
        for t in texts {
            if doc.parent(t).is_none() {
                continue; // Already merged into a previous node.
            }
            if doc.text(t).is_some_and(str::is_empty) {
                take_out(&mut doc, t);
                continue;
            }
            let run: Vec<NodeId> =
                std::iter::successors(doc.next_sibling(t), |&n| doc.next_sibling(n))
                    .take_while(|&n| doc.is_text(n))
                    .collect();
            if run.is_empty() {
                continue;
            }
            let mut data = doc.text(t).unwrap_or_default().to_owned();
            for &n in &run {
                data.push_str(doc.text(n).unwrap_or_default());
            }
            dom.character_data(&doc, t);
            doc.set_text(t, &data);
            for n in run {
                take_out(&mut doc, n);
            }
        }
        if !removed.is_empty() {
            for (n, parent) in removed {
                after_remove(ctx, dom, &doc, n, parent);
            }
            dom.bump();
        }
    }
    deliver(ctx, dom)
}

/// The namespace, local name and value of the attribute `setAttribute` and
/// `removeAttribute` would find for `qualified`.
fn attr_by_qualified_name<'d>(
    doc: &'d Document,
    id: NodeId,
    qualified: &str,
) -> Option<(Name, &'d str, &'d str)> {
    doc.attrs(id).iter().find_map(|a| {
        let local = doc.name_str(a.local());
        let prefix = doc.attr_prefix(a);
        let matches = if prefix == Name::EMPTY {
            local == qualified
        } else {
            qualified.split_once(':') == Some((doc.name_str(prefix), local))
        };
        matches.then(|| (doc.attr_namespace(a), local, a.value()))
    })
}

pub(super) fn set_attr(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    n: NodeId,
    name: &str,
    value: &str,
) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        let name = html_lower(&doc, n, name);
        // What a custom element's `attributeChangedCallback` gets, copied
        // only for custom elements; a mutation record copies what it needs.
        let reaction = {
            let found = attr_by_qualified_name(&doc, n, &name);
            let (ns, local, old) = match found {
                Some((ns, local, old)) => (ns, local, Some(old)),
                None => (Name::EMPTY, &*name, None),
            };
            dom.attribute(&doc, n, ns, local, old);
            dom.is_custom(n)
                .then(|| (ns, local.to_owned(), old.map(str::to_owned)))
        };
        doc.set_attribute(n, &name, value);
        dom.bump();
        if let Some((ns, local, old)) = reaction {
            dom.ce_attribute(&doc, n, ns, &local, old.as_deref(), Some(value));
        }
    }
    deliver(ctx, dom)
}

pub(super) fn remove_attr(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    n: NodeId,
    name: &str,
) -> Result<bool> {
    let removed = {
        let mut doc = dom.doc_mut();
        let name = html_lower(&doc, n, name);
        let reaction = match attr_by_qualified_name(&doc, n, &name) {
            None => return Ok(false),
            Some((ns, local, old)) => {
                dom.attribute(&doc, n, ns, local, Some(old));
                dom.is_custom(n)
                    .then(|| (ns, local.to_owned(), old.to_owned()))
            }
        };
        let removed = doc.remove_attribute(n, &name);
        if removed {
            dom.bump();
        }
        if let Some((ns, local, old)) = &reaction {
            dom.ce_attribute(&doc, n, *ns, local, Some(old), None);
        }
        removed
    };
    deliver(ctx, dom)?;
    Ok(removed)
}

pub(super) fn set_attr_ns(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    n: NodeId,
    ns: &str,
    name: &str,
    value: &str,
) -> Result<()> {
    {
        let mut doc = dom.doc_mut();
        let local = name.split_once(':').map_or(name, |(_, l)| l);
        // `setAttributeNS` interns both names anyway.
        let (ns_name, local_name) = (doc.intern(ns), doc.intern(local));
        let old = doc.attribute_ns(n, ns_name, local_name).map(str::to_owned);
        dom.attribute(&doc, n, ns_name, local, old.as_deref());
        doc.set_attribute_ns(n, ns, name, value);
        dom.bump();
        dom.ce_attribute(&doc, n, ns_name, local, old.as_deref(), Some(value));
    }
    deliver(ctx, dom)
}

pub(super) fn remove_attr_ns(
    ctx: &Ctx<'_>,
    dom: &Rc<DomState>,
    n: NodeId,
    ns: Option<&str>,
    local: &str,
) -> Result<bool> {
    let removed = {
        let mut doc = dom.doc_mut();
        let names = doc.names();
        let (Some(ns), Some(local)) = (names.get(ns.unwrap_or("")), names.get(local)) else {
            return Ok(false);
        };
        let old = doc.attribute_ns(n, ns, local).map(str::to_owned);
        if let Some(old) = &old {
            dom.attribute(&doc, n, ns, doc.name_str(local), Some(old));
        }
        let removed = doc.remove_attribute_ns(n, ns, local);
        if removed {
            dom.bump();
        }
        if let Some(old) = &old {
            let local = doc.name_str(local).to_owned();
            dom.ce_attribute(&doc, n, ns, &local, Some(old), None);
        }
        removed
    };
    deliver(ctx, dom)?;
    Ok(removed)
}

// -------------------------------------------------------------------------
// Queries.

fn is_equal_node(doc: &Document, a: NodeId, b: NodeId) -> bool {
    if doc.kind(a) != doc.kind(b) {
        return false;
    }
    let same = match doc.kind(a) {
        NodeKind::Element => {
            let names = |n| (doc.namespace(n), doc.prefix(n), doc.local_name(n));
            let attr_key = |n: NodeId, attr: &dasher_dom::Attr| {
                (
                    doc.attr_namespace(attr),
                    attr.local(),
                    attr.value().to_owned(),
                    n,
                )
            };
            names(a) == names(b) && {
                let (aa, ab) = (doc.attrs(a), doc.attrs(b));
                aa.len() == ab.len()
                    && aa.iter().all(|x| {
                        let (ns, local, value, _) = attr_key(a, x);
                        doc.attribute_ns(b, ns, local) == Some(value.as_str())
                    })
            }
        }
        NodeKind::Text | NodeKind::Comment => doc.text(a) == doc.text(b),
        NodeKind::Doctype => doc.doctype(a) == doc.doctype(b),
        NodeKind::Document | NodeKind::DocumentFragment => true,
    };
    same && doc.children(a).count() == doc.children(b).count()
        && doc
            .children(a)
            .zip(doc.children(b))
            .all(|(x, y)| is_equal_node(doc, x, y))
}

/// The value behind a proxy, or the value itself.
fn unproxy<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Value<'js> {
    let raw = value.as_raw();
    // SAFETY: `raw` belongs to `value`, which is alive. `JS_GetProxyTarget`
    // returns a new reference to the target, which `Value` then owns.
    unsafe {
        if qjs::JS_IsProxy(raw) {
            Value::from_raw(
                ctx.clone(),
                qjs::JS_GetProxyTarget(ctx.as_raw().as_ptr(), raw),
            )
        } else {
            value
        }
    }
}

fn selector_error(ctx: &Ctx<'_>, selector: &str) -> rquickjs::Error {
    throw_dom(
        ctx,
        "SyntaxError",
        &format!("'{selector}' is not a valid selector."),
    )
}

// -------------------------------------------------------------------------
// Installation.

pub(super) fn def<'js, F, P>(obj: &Object<'js>, name: &str, f: F) -> Result<()>
where
    F: IntoJsFunc<'js, P> + 'js,
{
    obj.set(name, Function::new(obj.ctx().clone(), f)?.with_name(name)?)
}

pub(super) fn install<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    install_core(obj, dom)?;
    install_tree(obj, dom)?;
    install_content(obj, dom)?;
    install_attributes(obj, dom)?;
    install_queries(obj, dom)?;
    super::shadow::install(obj, dom)?;
    super::custom::install(obj, dom)?;
    super::host::install(obj)?;
    layout::install(obj, dom)?;
    Ok(())
}

fn install_core<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "init",
        move |ctx: Ctx<'js>, protos: Object<'js>, exception: Function<'js>| {
            wrapper::init(&ctx, &d, protos, exception)
        },
    )?;
    def(obj, "document", |ctx: Ctx<'js>| -> Result<Node<'js>> {
        ctx.userdata::<DomData<'js>>()
            .map(|d| d.document.clone())
            .ok_or(rquickjs::Error::Unknown)
    })?;
    let d = Rc::clone(dom);
    def(obj, "version", move || d.version())?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setLayoutHook",
        move |ctx: Ctx<'js>, hook: Option<Function<'js>>| -> Result<()> {
            let data = ctx
                .userdata::<DomData<'js>>()
                .ok_or(rquickjs::Error::Unknown)?;
            d.watch_layout(hook.is_some());
            data.layout_hook.replace(hook);
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "layoutLines",
        move |ctx: Ctx<'js>, nodes: Array<'js>| -> Result<Vec<i32>> {
            let ids = nodes
                .iter::<Node<'js>>()
                .map(|n| n.and_then(|n| id(&ctx, &n)))
                .collect::<Result<Vec<_>>>()?;
            Ok(layout::lines(&d.doc(), &ids))
        },
    )?;
    let d = Rc::clone(dom);
    def(obj, "insertionEpoch", move || d.doc().insertion_epoch())?;
    let d = Rc::clone(dom);
    def(
        obj,
        "loadedLines",
        move |ctx: Ctx<'js>, nodes: Array<'js>| -> Result<Vec<i32>> {
            let ids = nodes
                .iter::<Node<'js>>()
                .map(|n| n.and_then(|n| id(&ctx, &n)))
                .collect::<Result<Vec<_>>>()?;
            Ok(layout::loaded_lines(&d.doc(), &ids))
        },
    )?;
    def(
        obj,
        "setMutationHook",
        |ctx: Ctx<'js>, hook: Function<'js>| -> Result<()> {
            let data = ctx
                .userdata::<DomData<'js>>()
                .ok_or(rquickjs::Error::Unknown)?;
            data.mutation_hook.replace(Some(hook));
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setObserved",
        move |ctx: Ctx<'js>, n: Node<'js>, observed: bool| -> Result<()> {
            d.set_observed(id(&ctx, &n)?, observed);
            Ok(())
        },
    )?;
    def(obj, "unproxy", |ctx: Ctx<'js>, v: Value<'js>| {
        unproxy(&ctx, v)
    })?;
    let d = Rc::clone(dom);
    def(obj, "nodeType", move |ctx: Ctx<'js>, n: Node<'js>| {
        Ok::<_, rquickjs::Error>(d.doc().kind(id(&ctx, &n)?).node_type())
    })?;
    Ok(())
}

fn install_tree<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "nav",
        move |ctx: Ctx<'js>, n: Node<'js>, which: u32| {
            let n = id(&ctx, &n)?;
            let target = {
                let doc = d.doc();
                let elem = |mut it: Option<NodeId>, step: &dyn Fn(NodeId) -> Option<NodeId>| {
                    while let Some(x) = it {
                        if doc.is_element(x) {
                            return Some(x);
                        }
                        it = step(x);
                    }
                    None
                };
                match which {
                    0 => doc.parent(n),
                    1 => doc.first_child(n),
                    2 => doc.last_child(n),
                    3 => doc.prev_sibling(n),
                    4 => doc.next_sibling(n),
                    5 => doc.parent_element(n),
                    6 => elem(doc.first_child(n), &|x| doc.next_sibling(x)),
                    7 => elem(doc.last_child(n), &|x| doc.prev_sibling(x)),
                    8 => elem(doc.prev_sibling(n), &|x| doc.prev_sibling(x)),
                    9 => elem(doc.next_sibling(n), &|x| doc.next_sibling(x)),
                    _ => None,
                }
            };
            wrap_opt(&ctx, &d, target)
        },
    )?;
    let d = Rc::clone(dom);
    def(obj, "isConnected", move |ctx: Ctx<'js>, n: Node<'js>| {
        Ok::<_, rquickjs::Error>(d.doc().is_connected(id(&ctx, &n)?))
    })?;
    let d = Rc::clone(dom);
    def(obj, "rootNode", move |ctx: Ctx<'js>, n: Node<'js>| {
        let root = d.doc().tree_root(id(&ctx, &n)?);
        wrap(&ctx, &d, root)
    })?;
    let d = Rc::clone(dom);
    def(
        obj,
        "ownerDocument",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Node<'js>> {
            let owner = d.owner_document(&d.doc(), id(&ctx, &n)?);
            wrap(&ctx, &d, owner)
        },
    )?;
    // Makes `owner` the node document of the detached tree rooted at `n`,
    // for nodes a document creates or adopts.
    let d = Rc::clone(dom);
    def(
        obj,
        "setOwner",
        move |ctx: Ctx<'js>, n: Node<'js>, owner: Node<'js>| -> Result<()> {
            let (n, owner) = (id(&ctx, &n)?, id(&ctx, &owner)?);
            let doc = d.doc();
            if doc.kind(owner) == NodeKind::Document {
                d.set_owner(&doc, n, owner);
            }
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "contains",
        move |ctx: Ctx<'js>, a: Node<'js>, b: Node<'js>| -> Result<bool> {
            Ok(d.doc().contains(id(&ctx, &a)?, id(&ctx, &b)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "compare",
        move |ctx: Ctx<'js>, a: Node<'js>, b: Node<'js>| -> Result<u32> {
            Ok(d.compare_position(&d.doc(), id(&ctx, &a)?, id(&ctx, &b)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "isEqual",
        move |ctx: Ctx<'js>, a: Node<'js>, b: Node<'js>| -> Result<bool> {
            Ok(is_equal_node(&d.doc(), id(&ctx, &a)?, id(&ctx, &b)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "insert",
        move |ctx: Ctx<'js>, parent: Node<'js>, node: Node<'js>, before: Option<Node<'js>>| {
            let before = before.map(|b| id(&ctx, &b)).transpose()?;
            let scripts = insert(&ctx, &d, id(&ctx, &parent)?, id(&ctx, &node)?, before)?;
            wrap_all(&ctx, &d, &scripts)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "removeChild",
        move |ctx: Ctx<'js>, parent: Node<'js>, child: Node<'js>| -> Result<()> {
            let (p, c) = (id(&ctx, &parent)?, id(&ctx, &child)?);
            if d.doc().parent(c) != Some(p) {
                return Err(dom_error(&ctx, DomError::NotFound));
            }
            detach(&ctx, &d, c)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "remove",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<()> { detach(&ctx, &d, id(&ctx, &n)?) },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "replaceChild",
        move |ctx: Ctx<'js>, parent: Node<'js>, node: Node<'js>, child: Node<'js>| {
            let scripts = replace_child(
                &ctx,
                &d,
                id(&ctx, &parent)?,
                id(&ctx, &node)?,
                id(&ctx, &child)?,
            )?;
            wrap_all(&ctx, &d, &scripts)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "replaceAll",
        move |ctx: Ctx<'js>, parent: Node<'js>, node: Option<Node<'js>>| {
            let node = node.map(|n| id(&ctx, &n)).transpose()?;
            let scripts = replace_all(&ctx, &d, id(&ctx, &parent)?, node)?;
            wrap_all(&ctx, &d, &scripts)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "clone",
        move |ctx: Ctx<'js>, n: Node<'js>, deep: bool| -> Result<Node<'js>> {
            let n = id(&ctx, &n)?;
            let copy = clone_into_owner(&d, &mut d.doc_mut(), n, deep);
            wrap(&ctx, &d, copy)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "normalize",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<()> { normalize(&ctx, &d, id(&ctx, &n)?) },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createElement",
        move |ctx: Ctx<'js>, name: String| -> Result<Node<'js>> {
            if !valid_element_name(&name) {
                return Err(throw_dom(
                    &ctx,
                    "InvalidCharacterError",
                    &format!("The tag name provided ('{name}') is not a valid name."),
                ));
            }
            let n = d.doc_mut().create_element(&name.to_ascii_lowercase());
            wrap(&ctx, &d, n)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createElementNS",
        move |ctx: Ctx<'js>, ns: Option<String>, name: String| -> Result<Node<'js>> {
            if !valid_element_name(&name) || name.starts_with(':') || name.ends_with(':') {
                return Err(throw_dom(
                    &ctx,
                    "InvalidCharacterError",
                    &format!(
                        "The qualified name provided ('{name}') contains the invalid name-start character."
                    ),
                ));
            }
            let ns = ns.unwrap_or_default();
            if ns.is_empty() && name.contains(':') {
                return Err(throw_dom(
                    &ctx,
                    "NamespaceError",
                    "The namespace URI provided ('') is not valid for the qualified name provided.",
                ));
            }
            let n = d.doc_mut().create_element_ns(&ns, &name);
            wrap(&ctx, &d, n)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createText",
        move |ctx: Ctx<'js>, data: String| -> Result<Node<'js>> {
            let n = d.doc_mut().create_text(&data);
            wrap(&ctx, &d, n)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createComment",
        move |ctx: Ctx<'js>, data: String| -> Result<Node<'js>> {
            let n = d.doc_mut().create_comment(&data);
            wrap(&ctx, &d, n)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createFragment",
        move |ctx: Ctx<'js>| -> Result<Node<'js>> {
            let n = d.doc_mut().create_fragment();
            wrap(&ctx, &d, n)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createDocument",
        move |ctx: Ctx<'js>| -> Result<Node<'js>> {
            let n = d.doc_mut().create_document();
            d.note_extra_document();
            wrap(&ctx, &d, n)
        },
    )?;
    // Replaces the children of `n`, a document made by script, with a
    // document parsed from `text`, as HTML or XML. Returns the compat mode
    // for HTML; for XML, null or why the text is not well-formed.
    let d = Rc::clone(dom);
    def(
        obj,
        "parseInto",
        move |ctx: Ctx<'js>, n: Node<'js>, text: String, xml: bool| -> Result<Value<'js>> {
            let target = id(&ctx, &n)?;
            let outcome = {
                let mut doc = d.doc_mut();
                if doc.kind(target) != NodeKind::Document || target == doc.root() {
                    return Err(throw_dom(
                        &ctx,
                        "InvalidStateError",
                        "Only documents made by script can be parsed into.",
                    ));
                }
                let removed = remove_children(&ctx, &d, &mut doc, target);
                let outcome = if xml {
                    doc.parse_xml_into(target, &text)
                        .err()
                        .map(|e| e.to_string())
                } else {
                    let quirks = doc.parse_document_into(target, &text);
                    Some(compat_mode(quirks).to_owned())
                };
                d.bump();
                let added: Vec<NodeId> = doc.children(target).collect();
                d.child_list(&doc, target, &added, &removed, None, None);
                outcome
            };
            deliver(&ctx, &d)?;
            match outcome {
                Some(s) => rquickjs::String::from_str(ctx, &s).map(|s| s.into_value()),
                None => Ok(Value::new_null(ctx)),
            }
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "createDoctype",
        move |ctx: Ctx<'js>,
              name: String,
              public_id: String,
              system_id: String|
              -> Result<Node<'js>> {
            let n = d.doc_mut().create_doctype(&name, &public_id, &system_id);
            wrap(&ctx, &d, n)
        },
    )?;
    Ok(())
}

fn install_content<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "data",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Nullable<String>> {
            Ok(Nullable(d.doc().text(id(&ctx, &n)?).map(str::to_owned)))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setData",
        move |ctx: Ctx<'js>, n: Node<'js>, data: String| -> Result<()> {
            set_data(&ctx, &d, id(&ctx, &n)?, &data)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "textContent",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Nullable<String>> {
            Ok(Nullable(d.doc().text_content(id(&ctx, &n)?)))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setTextContent",
        move |ctx: Ctx<'js>, n: Node<'js>, text: String| -> Result<()> {
            set_text_content(&ctx, &d, id(&ctx, &n)?, &text)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "innerText",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<String> {
            Ok(inner_text(&d.doc(), id(&ctx, &n)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "innerHTML",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<String> {
            Ok(d.doc().inner_html(id(&ctx, &n)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "outerHTML",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<String> {
            Ok(d.doc().outer_html(id(&ctx, &n)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setInnerHTML",
        move |ctx: Ctx<'js>, n: Node<'js>, html: String| -> Result<()> {
            set_inner_html(&ctx, &d, element(&ctx, &d, &n)?, &html)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setOuterHTML",
        move |ctx: Ctx<'js>, n: Node<'js>, html: String| -> Result<()> {
            set_outer_html(&ctx, &d, element(&ctx, &d, &n)?, &html)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "insertAdjacentHTML",
        move |ctx: Ctx<'js>, n: Node<'js>, position: String, html: String| -> Result<()> {
            let position = adjacent_position(&ctx, &position)?;
            insert_adjacent_html(&ctx, &d, element(&ctx, &d, &n)?, position, &html)
        },
    )?;
    // `Range.createContextualFragment`: a new fragment parsed in the
    // context of an element (none: a `<body>`), whose scripts can still run.
    let d = Rc::clone(dom);
    def(
        obj,
        "contextualFragment",
        move |ctx: Ctx<'js>, context: Option<Node<'js>>, html: String| -> Result<Node<'js>> {
            let context = context.map(|c| element(&ctx, &d, &c)).transpose()?;
            let fragment = d.doc_mut().parse_contextual_fragment(context, &html);
            wrap(&ctx, &d, fragment)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "markScriptStarted",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<()> {
            let n = element(&ctx, &d, &n)?;
            d.doc_mut().set_script_already_started(n);
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "templateContent",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Value<'js>> {
            let contents = d.doc().template_contents(id(&ctx, &n)?);
            wrap_opt(&ctx, &d, contents)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "doctype",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Vec<String>> {
            let doc = d.doc();
            Ok(doc
                .doctype(id(&ctx, &n)?)
                .map(|t| {
                    vec![
                        t.name.to_string(),
                        t.public_id.to_string(),
                        t.system_id.to_string(),
                    ]
                })
                .unwrap_or_default())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "docPart",
        move |ctx: Ctx<'js>, which: u32| -> Result<Value<'js>> {
            let part = {
                let doc = d.doc();
                match which {
                    0 => doc.document_element(),
                    1 => doc.head(),
                    2 => doc.body(),
                    3 => doc
                        .children(doc.root())
                        .find(|&n| doc.kind(n) == NodeKind::Doctype),
                    _ => None,
                }
            };
            wrap_opt(&ctx, &d, part)
        },
    )?;
    let d = Rc::clone(dom);
    def(obj, "title", move || d.doc().title())?;
    let d = Rc::clone(dom);
    def(obj, "compatMode", move || {
        compat_mode(d.doc().quirks_mode())
    })?;
    let d = Rc::clone(dom);
    def(obj, "baseHref", move || -> Nullable<String> {
        let doc = d.doc();
        Nullable(
            doc.descendants(doc.root())
                .find(|&n| {
                    doc.is_html(n, Name::BASE) && doc.attribute_named(n, Name::HREF).is_some()
                })
                .and_then(|n| doc.attribute_named(n, Name::HREF))
                .map(str::to_owned),
        )
    })?;
    Ok(())
}

fn install_attributes<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "elementName",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<Vec<Nullable<String>>> {
            // [localName, namespaceURI, prefix]
            let doc = d.doc();
            let n = id(&ctx, &n)?;
            let name = |x: Option<Name>| {
                Nullable(
                    x.filter(|&x| x != Name::EMPTY)
                        .map(|x| doc.name_str(x).to_owned()),
                )
            };
            Ok(vec![
                name(doc.local_name(n)),
                name(doc.namespace(n)),
                name(doc.prefix(n)),
            ])
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "getAttr",
        move |ctx: Ctx<'js>, n: Node<'js>, name: String| -> Result<Nullable<String>> {
            let doc = d.doc();
            let n = id(&ctx, &n)?;
            Ok(Nullable(
                doc.attribute(n, &html_lower(&doc, n, &name))
                    .map(str::to_owned),
            ))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "hasAttr",
        move |ctx: Ctx<'js>, n: Node<'js>, name: String| -> Result<bool> {
            let doc = d.doc();
            let n = id(&ctx, &n)?;
            Ok(doc.has_attribute(n, &html_lower(&doc, n, &name)))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setAttr",
        move |ctx: Ctx<'js>, n: Node<'js>, name: String, value: String| -> Result<()> {
            if !valid_attribute_name(&name) {
                return Err(throw_dom(
                    &ctx,
                    "InvalidCharacterError",
                    &format!("'{name}' is not a valid attribute name."),
                ));
            }
            set_attr(&ctx, &d, element(&ctx, &d, &n)?, &name, &value)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "removeAttr",
        move |ctx: Ctx<'js>, n: Node<'js>, name: String| -> Result<bool> {
            remove_attr(&ctx, &d, id(&ctx, &n)?, &name)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "getAttrNS",
        move |ctx: Ctx<'js>,
              n: Node<'js>,
              ns: Option<String>,
              local: String|
              -> Result<Nullable<String>> {
            let n = id(&ctx, &n)?;
            let doc = d.doc();
            let names = doc.names();
            let (Some(ns), Some(local)) =
                (names.get(ns.as_deref().unwrap_or("")), names.get(&local))
            else {
                return Ok(Nullable(None));
            };
            Ok(Nullable(doc.attribute_ns(n, ns, local).map(str::to_owned)))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "setAttrNS",
        move |ctx: Ctx<'js>,
              n: Node<'js>,
              ns: Option<String>,
              name: String,
              value: String|
              -> Result<()> {
            if !valid_attribute_name(&name) {
                return Err(throw_dom(
                    &ctx,
                    "InvalidCharacterError",
                    &format!("'{name}' is not a valid attribute name."),
                ));
            }
            let ns = ns.unwrap_or_default();
            if ns.is_empty() && name.contains(':') {
                return Err(throw_dom(
                    &ctx,
                    "NamespaceError",
                    "The namespace URI provided ('') is not valid for the qualified name provided.",
                ));
            }
            set_attr_ns(&ctx, &d, element(&ctx, &d, &n)?, &ns, &name, &value)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "removeAttrNS",
        move |ctx: Ctx<'js>, n: Node<'js>, ns: Option<String>, local: String| -> Result<bool> {
            remove_attr_ns(&ctx, &d, id(&ctx, &n)?, ns.as_deref(), &local)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "attrCount",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<usize> {
            Ok(d.doc().attrs(id(&ctx, &n)?).len())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "attr",
        move |ctx: Ctx<'js>, n: Node<'js>, i: usize| -> Result<Nullable<Vec<Nullable<String>>>> {
            // [qualifiedName, namespaceURI, prefix, localName, value]
            let doc = d.doc();
            let Some(attr) = doc.attrs(id(&ctx, &n)?).get(i) else {
                return Ok(Nullable(None));
            };
            let local = doc.name_str(attr.local()).to_owned();
            let prefix = doc.attr_prefix(attr);
            let ns = doc.attr_namespace(attr);
            let opt = |x: Name| (x != Name::EMPTY).then(|| doc.name_str(x).to_owned());
            let qualified = match opt(prefix) {
                Some(p) => format!("{p}:{local}"),
                None => local.clone(),
            };
            Ok(Nullable(Some(vec![
                Nullable(Some(qualified)),
                Nullable(opt(ns)),
                Nullable(opt(prefix)),
                Nullable(Some(local)),
                Nullable(Some(attr.value().to_owned())),
            ])))
        },
    )?;
    Ok(())
}

fn install_queries<'js>(obj: &Object<'js>, dom: &Rc<DomState>) -> Result<()> {
    let d = Rc::clone(dom);
    def(
        obj,
        "query",
        move |ctx: Ctx<'js>, n: Node<'js>, selector: String| -> Result<Value<'js>> {
            let list = d
                .selector(&selector)
                .map_err(|_| selector_error(&ctx, &selector))?;
            let found = list.query_first(&d.doc(), id(&ctx, &n)?);
            wrap_opt(&ctx, &d, found)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "queryAll",
        move |ctx: Ctx<'js>, n: Node<'js>, selector: String| -> Result<Vec<Node<'js>>> {
            let list = d
                .selector(&selector)
                .map_err(|_| selector_error(&ctx, &selector))?;
            let found: Vec<NodeId> = list.query_all(&d.doc(), id(&ctx, &n)?).collect();
            wrap_all(&ctx, &d, &found)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "matches",
        move |ctx: Ctx<'js>, n: Node<'js>, selector: String| -> Result<bool> {
            let list = d
                .selector(&selector)
                .map_err(|_| selector_error(&ctx, &selector))?;
            Ok(list.matches(&d.doc(), id(&ctx, &n)?))
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "closest",
        move |ctx: Ctx<'js>, n: Node<'js>, selector: String| -> Result<Value<'js>> {
            let list = d
                .selector(&selector)
                .map_err(|_| selector_error(&ctx, &selector))?;
            let found = list.closest(&d.doc(), id(&ctx, &n)?);
            wrap_opt(&ctx, &d, found)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "count",
        move |ctx: Ctx<'js>, n: Node<'js>, selector: String| -> Result<usize> {
            let list = d
                .selector(&selector)
                .map_err(|_| selector_error(&ctx, &selector))?;
            Ok(list.query_all(&d.doc(), id(&ctx, &n)?).count())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "byId",
        move |ctx: Ctx<'js>, n: Node<'js>, element_id: String| -> Result<Value<'js>> {
            let found = d.doc().get_element_by_id(id(&ctx, &n)?, &element_id);
            wrap_opt(&ctx, &d, found)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collection",
        move |ctx: Ctx<'js>,
              root: Node<'js>,
              kind: u32,
              a: Option<String>,
              b: Option<String>,
              proto: Object<'js>,
              named: bool|
              -> Result<Value<'js>> {
            let filter = match kind {
                0 => Filter::ChildNodes,
                1 => Filter::Children,
                2 => Filter::Tag(a.unwrap_or_default()),
                3 => Filter::TagNs(a.filter(|s| !s.is_empty()), b.unwrap_or_default()),
                4 => Filter::Class(a.unwrap_or_default()),
                5 => Filter::Name(a.unwrap_or_default()),
                _ => {
                    let selector = a.unwrap_or_default();
                    let list = d
                        .selector(&selector)
                        .map_err(|_| selector_error(&ctx, &selector))?;
                    Filter::Selector(list)
                }
            };
            lists::new_collection(&ctx, &root, filter, &proto, named)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collLength",
        move |ctx: Ctx<'js>, c: Value<'js>| -> Result<usize> {
            let (list, root) = lists::list_value(&ctx, &c)?;
            Ok(list.ids(&d, &d.doc(), root).len())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collItem",
        move |ctx: Ctx<'js>, c: Value<'js>, i: f64| -> Result<Value<'js>> {
            let item = {
                let (list, root) = lists::list_value(&ctx, &c)?;
                let ids = list.ids(&d, &d.doc(), root);
                (i >= 0.0 && i.fract() == 0.0)
                    .then(|| ids.get(i as usize).copied())
                    .flatten()
            };
            wrap_opt(&ctx, &d, item)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collNamed",
        move |ctx: Ctx<'js>, c: Value<'js>, name: String| -> Result<Value<'js>> {
            let item = {
                let (list, root) = lists::list_value(&ctx, &c)?;
                let doc = d.doc();
                let ids = list.ids(&d, &doc, root);
                lists::named_item(&doc, &ids, &name)
            };
            wrap_opt(&ctx, &d, item)
        },
    )?;
    // The element whose attributes a `NamedNodeMap` is.
    def(
        obj,
        "mapOwner",
        |ctx: Ctx<'js>, map: Value<'js>| -> Result<Value<'js>> {
            let (list, _) = lists::list_value(&ctx, &map)?;
            if !list.is_attributes() {
                return Err(rquickjs::Exception::throw_type(&ctx, "Illegal invocation"));
            }
            // SAFETY: `list` lives in `map`, which is alive; the `Value` takes a
            // new reference to its owner.
            Ok(unsafe {
                Value::from_raw(
                    ctx.clone(),
                    qjs::JS_DupValue(ctx.as_raw().as_ptr(), list.owner()),
                )
            })
        },
    )?;
    Ok(())
}
