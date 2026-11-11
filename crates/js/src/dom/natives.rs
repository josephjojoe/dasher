//! The native functions `dom.js` builds the DOM API on, installed as
//! `native.dom`.
//!
//! They take node wrappers and plain values, leave argument conversion and
//! most API surface to the glue, and throw `DOMException`s for DOM errors.
//! Every change to the tree goes through the mutation functions here, which
//! keep the wrapper groups in step (see the `dom` module docs).

use std::rc::Rc;

use dasher_dom::{AdjacentPosition, Document, DomError, Name, NodeId, NodeKind, QuirksMode};
use rquickjs::function::IntoJsFunc;
use rquickjs::{Class, Ctx, Function, IntoJs, Object, Result, Value, qjs};

use super::DomState;
use super::collection::{Collection, CollectionObject, Filter};
use super::inner_text::inner_text;
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

fn dom_error(ctx: &Ctx<'_>, error: DomError) -> rquickjs::Error {
    throw_dom(ctx, error.name(), &error.to_string())
}

/// A value that is `null` in JS when absent (rquickjs makes `None`
/// `undefined`).
struct Nullable<T>(Option<T>);

impl<'js, T: IntoJs<'js>> IntoJs<'js> for Nullable<T> {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        match self.0 {
            Some(v) => v.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

/// The node behind a wrapper.
fn id(ctx: &Ctx<'_>, node: &Node<'_>) -> Result<NodeId> {
    node.borrow().id().ok_or_else(|| {
        throw_dom(
            ctx,
            "InvalidStateError",
            "The node belongs to a document that has been replaced.",
        )
    })
}

/// The element behind a wrapper; a `TypeError` for other nodes, which page
/// code can pass by calling an `Element` method on them.
fn element(ctx: &Ctx<'_>, dom: &DomState, node: &Node<'_>) -> Result<NodeId> {
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

fn wrap_opt<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>, id: Option<NodeId>) -> Result<Value<'js>> {
    let node = id.map(|id| wrap(ctx, dom, id)).transpose()?;
    Ok(or_null(ctx, node))
}

fn wrap_all<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>, ids: &[NodeId]) -> Result<Vec<Node<'js>>> {
    ids.iter().map(|&id| wrap(ctx, dom, id)).collect()
}

/// Whether an element name is acceptable to `createElement`: the XML
/// `Name` rules, loosely (non-ASCII characters are all allowed).
fn valid_element_name(name: &str) -> bool {
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
fn valid_attribute_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|c| {
            matches!(
                c,
                '\0' | '\t' | '\n' | '\x0C' | '\r' | ' ' | '/' | '>' | '=' | '"' | '\''
            )
        })
}

/// Lower-cases `name` if `id` is an HTML element, as attribute and tag name
/// lookups do in HTML documents.
fn html_lower(doc: &Document, id: NodeId, name: &str) -> String {
    if doc.namespace(id) == Some(Name::NS_HTML) {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
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

/// The script elements under `moved` that just became connected and have
/// not run, for the page loader to prepare.
fn connected_scripts(doc: &Document, moved: &[NodeId]) -> Vec<NodeId> {
    let mut scripts = Vec::new();
    for &root in moved {
        if !doc.is_connected(root) {
            continue;
        }
        let subtree = std::iter::once(root).chain(doc.descendants(root));
        scripts.extend(
            subtree.filter(|&n| doc.is_html(n, Name::SCRIPT) && !doc.script_already_started(n)),
        );
    }
    scripts
}

/// `parent.insertBefore(node, before)`. Returns the scripts it connected.
fn insert(
    ctx: &Ctx<'_>,
    dom: &DomState,
    parent: NodeId,
    node: NodeId,
    before: Option<NodeId>,
) -> Result<Vec<NodeId>> {
    let result = {
        let mut doc = dom.doc_mut();
        let plan = plan_insert(&doc, parent, node);
        let moved: Vec<NodeId> = if doc.kind(node) == NodeKind::DocumentFragment {
            doc.children(node).collect()
        } else {
            vec![node]
        };
        let result = doc.insert_before(parent, node, before);
        result.map(|_| {
            after_insert(ctx, dom, &doc, parent, node, &moved, plan);
            dom.bump();
            connected_scripts(&doc, &moved)
        })
    };
    result.map_err(|e| dom_error(ctx, e))
}

/// Detaches `node` from its parent, if it has one.
fn detach(ctx: &Ctx<'_>, dom: &DomState, node: NodeId) {
    let mut doc = dom.doc_mut();
    if doc.parent(node).is_none() {
        return;
    }
    doc.remove(node);
    after_remove(ctx, dom, &doc, node);
    dom.bump();
}

/// Detaches all children of `parent`.
fn remove_children(ctx: &Ctx<'_>, dom: &DomState, doc: &mut Document, parent: NodeId) {
    let old: Vec<NodeId> = doc.children(parent).collect();
    doc.remove_children(parent);
    for child in old {
        after_remove(ctx, dom, doc, child);
    }
}

fn replace_child(
    ctx: &Ctx<'_>,
    dom: &DomState,
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
        let moved: Vec<NodeId> = if doc.kind(node) == NodeKind::DocumentFragment {
            doc.children(node).collect()
        } else {
            vec![node]
        };
        let result = doc.replace_child(parent, node, child);
        result.map(|_| {
            if node == child {
                return Vec::new();
            }
            after_insert(ctx, dom, &doc, parent, node, &moved, plan);
            after_remove(ctx, dom, &doc, child);
            dom.bump();
            connected_scripts(&doc, &moved)
        })
    };
    result.map_err(|e| dom_error(ctx, e))
}

/// `replaceChildren`: replaces all of `parent`'s children with `node`.
fn replace_all(
    ctx: &Ctx<'_>,
    dom: &DomState,
    parent: NodeId,
    node: Option<NodeId>,
) -> Result<Vec<NodeId>> {
    {
        let mut doc = dom.doc_mut();
        if let Some(node) = node {
            if doc.contains(node, parent)
                || doc
                    .template_host(node)
                    .is_some_and(|h| doc.contains(h, parent))
            {
                return Err(dom_error(ctx, DomError::HierarchyRequest));
            }
        }
        remove_children(ctx, dom, &mut doc, parent);
        dom.bump();
    }
    match node {
        Some(node) => insert(ctx, dom, parent, node, None),
        None => Ok(Vec::new()),
    }
}

fn set_text_content(ctx: &Ctx<'_>, dom: &DomState, node: NodeId, text: &str) {
    let mut doc = dom.doc_mut();
    match doc.kind(node) {
        NodeKind::Element | NodeKind::DocumentFragment => {
            remove_children(ctx, dom, &mut doc, node);
            if !text.is_empty() {
                let t = doc.create_text(text);
                doc.append_child(node, t)
                    .expect("a text node fits anywhere");
            }
            dom.bump();
        }
        NodeKind::Text | NodeKind::Comment => doc.set_text(node, text),
        NodeKind::Document | NodeKind::Doctype => {}
    }
}

fn set_inner_html(ctx: &Ctx<'_>, dom: &DomState, node: NodeId, html: &str) {
    let mut doc = dom.doc_mut();
    let target = doc.template_contents(node).unwrap_or(node);
    remove_children(ctx, dom, &mut doc, target);
    doc.set_inner_html(node, html);
    dom.bump();
}

fn set_outer_html(ctx: &Ctx<'_>, dom: &DomState, node: NodeId, html: &str) -> Result<()> {
    let result = {
        let mut doc = dom.doc_mut();
        let had_parent = doc.parent(node).is_some();
        let result = doc.set_outer_html(node, html);
        if result.is_ok() && had_parent {
            after_remove(ctx, dom, &doc, node);
            dom.bump();
        }
        result
    };
    result.map_err(|e| dom_error(ctx, e))
}

/// `Node.normalize()`: merges adjacent text nodes and drops empty ones.
fn normalize(ctx: &Ctx<'_>, dom: &DomState, root: NodeId) {
    let mut doc = dom.doc_mut();
    let texts: Vec<NodeId> = doc.descendants(root).filter(|&n| doc.is_text(n)).collect();
    let mut removed = Vec::new();
    for t in texts {
        if doc.parent(t).is_none() {
            continue; // Already merged into a previous node.
        }
        let mut data = doc.text(t).unwrap_or_default().to_owned();
        while let Some(next) = doc.next_sibling(t).filter(|&n| doc.is_text(n)) {
            data.push_str(doc.text(next).unwrap_or_default());
            doc.remove(next);
            removed.push(next);
        }
        if data.is_empty() {
            doc.remove(t);
            removed.push(t);
        } else {
            doc.set_text(t, &data);
        }
    }
    if !removed.is_empty() {
        for n in removed {
            after_remove(ctx, dom, &doc, n);
        }
        dom.bump();
    }
}

// -------------------------------------------------------------------------
// Queries.

/// `compareDocumentPosition` bits.
const DISCONNECTED: u32 = 0x01;
const PRECEDING: u32 = 0x02;
const FOLLOWING: u32 = 0x04;
const CONTAINS: u32 = 0x08;
const CONTAINED_BY: u32 = 0x10;
const IMPLEMENTATION_SPECIFIC: u32 = 0x20;

fn compare_position(doc: &Document, a: NodeId, b: NodeId) -> u32 {
    if a == b {
        return 0;
    }
    let chain = |n: NodeId| {
        let mut v: Vec<NodeId> = std::iter::once(n).chain(doc.ancestors(n)).collect();
        v.reverse();
        v
    };
    let (ca, cb) = (chain(a), chain(b));
    if ca[0] != cb[0] {
        let order = if a < b { FOLLOWING } else { PRECEDING };
        return DISCONNECTED | IMPLEMENTATION_SPECIFIC | order;
    }
    let common = ca.iter().zip(&cb).take_while(|(x, y)| x == y).count();
    if common == ca.len() {
        return CONTAINED_BY | FOLLOWING;
    }
    if common == cb.len() {
        return CONTAINS | PRECEDING;
    }
    // Siblings under the deepest common ancestor: which comes first?
    let (sa, sb) = (ca[common], cb[common]);
    let b_after_a =
        std::iter::successors(doc.next_sibling(sa), |&n| doc.next_sibling(n)).any(|n| n == sb);
    if b_after_a { FOLLOWING } else { PRECEDING }
}

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

fn collection<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<CollectionObject<'js>> {
    Class::from_value(&unproxy(ctx, value))
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

fn def<'js, F, P>(obj: &Object<'js>, name: &str, f: F) -> Result<()>
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
    def(obj, "unproxy", |ctx: Ctx<'js>, v: Value<'js>| {
        unproxy(&ctx, v)
    })?;
    let d = Rc::clone(dom);
    def(obj, "nodeType", move |ctx: Ctx<'js>, n: Node<'js>| {
        Ok::<_, rquickjs::Error>(d.doc().kind(id(&ctx, &n)?).node_type())
    })?;
    let d = Rc::clone(dom);
    def(obj, "nodeName", move |ctx: Ctx<'js>, n: Node<'js>| {
        Ok::<_, rquickjs::Error>(d.doc().node_name(id(&ctx, &n)?).into_owned())
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
            Ok(compare_position(&d.doc(), id(&ctx, &a)?, id(&ctx, &b)?))
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
        "childCount",
        move |ctx: Ctx<'js>, n: Node<'js>, elements: bool| -> Result<usize> {
            let n = id(&ctx, &n)?;
            let doc = d.doc();
            Ok(if elements {
                doc.element_children(n).count()
            } else {
                doc.children(n).count()
            })
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
            detach(&ctx, &d, c);
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "remove",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<()> {
            detach(&ctx, &d, id(&ctx, &n)?);
            Ok(())
        },
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
            let copy = d.doc_mut().clone_node(n, deep);
            wrap(&ctx, &d, copy)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "normalize",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<()> {
            normalize(&ctx, &d, id(&ctx, &n)?);
            Ok(())
        },
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
            d.doc_mut().set_text(id(&ctx, &n)?, &data);
            Ok(())
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
            set_text_content(&ctx, &d, id(&ctx, &n)?, &text);
            Ok(())
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
            set_inner_html(&ctx, &d, element(&ctx, &d, &n)?, &html);
            Ok(())
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
            let n = element(&ctx, &d, &n)?;
            let result = d.doc_mut().insert_adjacent_html(n, position, &html);
            d.bump();
            result.map_err(|e| dom_error(&ctx, e))
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
    def(obj, "compatMode", move || match d.doc().quirks_mode() {
        QuirksMode::Quirks => "BackCompat",
        _ => "CSS1Compat",
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
        "tagName",
        move |ctx: Ctx<'js>, n: Node<'js>| -> Result<String> {
            Ok(d.doc()
                .tag_name(id(&ctx, &n)?)
                .map(|s| s.into_owned())
                .unwrap_or_default())
        },
    )?;
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
            let n = element(&ctx, &d, &n)?;
            let mut doc = d.doc_mut();
            let name = html_lower(&doc, n, &name);
            doc.set_attribute(n, &name, &value);
            d.bump();
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "removeAttr",
        move |ctx: Ctx<'js>, n: Node<'js>, name: String| -> Result<bool> {
            let n = id(&ctx, &n)?;
            let mut doc = d.doc_mut();
            let name = html_lower(&doc, n, &name);
            let removed = doc.remove_attribute(n, &name);
            if removed {
                d.bump();
            }
            Ok(removed)
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
            let n = element(&ctx, &d, &n)?;
            d.doc_mut().set_attribute_ns(n, &ns, &name, &value);
            d.bump();
            Ok(())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "removeAttrNS",
        move |ctx: Ctx<'js>, n: Node<'js>, ns: Option<String>, local: String| -> Result<bool> {
            let n = id(&ctx, &n)?;
            let mut doc = d.doc_mut();
            let names = doc.names();
            let (Some(ns), Some(local)) =
                (names.get(ns.as_deref().unwrap_or("")), names.get(&local))
            else {
                return Ok(false);
            };
            let removed = doc.remove_attribute_ns(n, ns, local);
            if removed {
                d.bump();
            }
            Ok(removed)
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
              proto: Object<'js>|
              -> Result<CollectionObject<'js>> {
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
            Class::instance_proto(Collection::new(root, filter), proto)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collLength",
        move |ctx: Ctx<'js>, c: Value<'js>| -> Result<usize> {
            let c = collection(&ctx, c)?;
            let c = c.borrow();
            let root = id(&ctx, &c.root)?;
            Ok(c.ids(&d.doc(), root, d.version()).len())
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collItem",
        move |ctx: Ctx<'js>, c: Value<'js>, i: f64| -> Result<Value<'js>> {
            let c = collection(&ctx, c)?;
            let item = {
                let c = c.borrow();
                let root = id(&ctx, &c.root)?;
                let ids = c.ids(&d.doc(), root, d.version());
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
            let c = collection(&ctx, c)?;
            let item = {
                let c = c.borrow();
                let root = id(&ctx, &c.root)?;
                let doc = d.doc();
                let ids = c.ids(&doc, root, d.version());
                ids.iter().copied().find(|&n| {
                    !name.is_empty()
                        && (doc.element_id(n) == Some(name.as_str())
                            || (doc.namespace(n) == Some(Name::NS_HTML)
                                && doc.attribute_named(n, Name::NAME) == Some(name.as_str())))
                })
            };
            wrap_opt(&ctx, &d, item)
        },
    )?;
    let d = Rc::clone(dom);
    def(
        obj,
        "collNames",
        move |ctx: Ctx<'js>, c: Value<'js>| -> Result<Vec<String>> {
            let c = collection(&ctx, c)?;
            let c = c.borrow();
            let root = id(&ctx, &c.root)?;
            let doc = d.doc();
            let mut names: Vec<String> = Vec::new();
            for &n in c.ids(&doc, root, d.version()).iter() {
                let candidates = [
                    doc.element_id(n),
                    (doc.namespace(n) == Some(Name::NS_HTML))
                        .then(|| doc.attribute_named(n, Name::NAME))
                        .flatten(),
                ];
                for name in candidates.into_iter().flatten() {
                    if !name.is_empty() && !names.iter().any(|x| x == name) {
                        names.push(name.to_owned());
                    }
                }
            }
            Ok(names)
        },
    )?;
    Ok(())
}
