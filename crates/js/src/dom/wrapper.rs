//! Node wrappers: their identity cache, their prototypes and the rings that
//! group the wrappers of one tree. See the module docs of `dom`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId, NodeKind};
use rquickjs::class::{JsClass, Readable, Trace, Tracer};
use rquickjs::function::Constructor;
use rquickjs::{Class, Ctx, Function, JsLifetime, Object, Result, Value, qjs};

use super::attrs;
use super::hooks::Hooks;
use super::host::Hosts;
use super::members::{self, Members};
use super::{DomState, any_in_tree_except, group_root, owner};

/// A node wrapper object.
pub(crate) type Node<'js> = Class<'js, NodeWrapper<'js>>;

/// The Rust side of a node wrapper.
pub(crate) struct NodeWrapper<'js> {
    id: NodeId,
    dom: Rc<DomState>,
    /// Set when the page's document was replaced: `id` means nothing now.
    stale: Cell<bool>,
    /// Neighbours in the ring of wrappers of the same tree. Both `None`
    /// when the wrapper is alone, so a lone wrapper is not a cycle and is
    /// freed by reference counting alone.
    next: RefCell<Option<Node<'js>>>,
    prev: RefCell<Option<Node<'js>>>,
    /// The objects the element hands out and must keep returning
    /// (`el.style`, `el.classList`...), made on first use. Traced, so they
    /// live as long as the wrapper, and they hold the wrapper in turn.
    extras: RefCell<Option<Box<[Option<Value<'js>>; Extra::COUNT]>>>,
}

/// The slots of [`NodeWrapper`]'s extras.
#[derive(Clone, Copy)]
pub(crate) enum Extra {
    Style,
    ComputedStyle,
    ClassList,
    Dataset,
}

impl Extra {
    const COUNT: usize = 4;
}

impl<'js> NodeWrapper<'js> {
    /// The node, or `None` if it belonged to a replaced document.
    pub(crate) fn id(&self) -> Option<NodeId> {
        (!self.stale.get()).then_some(self.id)
    }

    /// The DOM state the node belongs to.
    pub(crate) fn dom(&self) -> &Rc<DomState> {
        &self.dom
    }

    /// The object in `slot`, borrowed: it stays valid while the wrapper
    /// does not change the slot.
    pub(crate) fn extra(&self, slot: Extra) -> Option<qjs::JSValue> {
        let extras = self.extras.borrow();
        extras.as_ref()?[slot as usize].as_ref().map(Value::as_raw)
    }

    /// Puts `value` in `slot`, which must be empty. Allocates nothing that
    /// the GC could run during (beyond the first slot's box), so the
    /// tracer never finds the cell borrowed in one phase of a collection
    /// but not in another.
    pub(crate) fn set_extra(&self, slot: Extra, value: Value<'js>) {
        let mut extras = self.extras.borrow_mut();
        let slots = extras.get_or_insert_with(Box::default);
        debug_assert!(slots[slot as usize].is_none());
        slots[slot as usize] = Some(value);
    }
}

impl Drop for NodeWrapper<'_> {
    fn drop(&mut self) {
        if self.stale.get() {
            return;
        }
        // Runs in the GC finalizer, which never runs while these cells are
        // borrowed: they are only borrowed briefly, without running JS.
        self.dom.wrappers.remove(self.id);
        self.dom.set_observed(self.id, false);
        self.dom.set_custom(self.id, false);
        self.dom.add_candidate(self.id);
    }
}

impl<'js> Trace<'js> for NodeWrapper<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        // The links are only borrowed mutably for a single assignment, never
        // while the GC can run.
        if let Ok(next) = self.next.try_borrow() {
            next.trace(tracer);
        }
        if let Ok(prev) = self.prev.try_borrow() {
            prev.trace(tracer);
        }
        if let Ok(extras) = self.extras.try_borrow() {
            for v in extras.iter().flat_map(|slots| slots.iter().flatten()) {
                v.trace(tracer);
            }
        }
    }
}

// SAFETY: `NodeWrapper` only holds values bound to `'js`, and `Changed` is the
// same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for NodeWrapper<'js> {
    type Changed<'to> = NodeWrapper<'to>;
}

impl<'js> JsClass<'js> for NodeWrapper<'js> {
    const NAME: &'static str = "Node";
    type Mutable = Readable;

    fn prototype(_ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
        // Instances are always made with an explicit prototype.
        Ok(None)
    }

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

/// The interface prototypes `dom.js` registers, and the values the DOM
/// bindings keep for the life of the context.
pub(crate) struct DomData<'js> {
    pub(crate) exception: Function<'js>,
    pub(crate) document: Node<'js>,
    /// Takes mutation records (see the `mutation` module).
    pub(crate) mutation_hook: RefCell<Option<Function<'js>>>,
    /// Takes custom element reactions (see the `custom` module).
    pub(crate) ce_hook: RefCell<Option<Function<'js>>>,
    /// Wrappers of removed custom elements, kept alive until their
    /// reactions are delivered.
    pub(crate) ce_held: RefCell<Vec<Node<'js>>>,
    /// Told when the document changed (see the `layout` module).
    pub(crate) layout_hook: RefCell<Option<Function<'js>>>,
    /// What the native members need (see the `members` module).
    pub(crate) members: Box<Members>,
    /// The prototypes of the host objects (see the `host` module), which
    /// `members` points at.
    _host_protos: Vec<Object<'js>>,
    element: Object<'js>,
    html_element: Object<'js>,
    html_unknown: Object<'js>,
    svg_element: Object<'js>,
    mathml_element: Object<'js>,
    text: Object<'js>,
    comment: Object<'js>,
    doctype: Object<'js>,
    fragment: Object<'js>,
    shadow_root: Object<'js>,
    html_tags: HashMap<String, Object<'js>>,
    svg_tags: HashMap<String, Object<'js>>,
}

// SAFETY: `DomData` only holds values bound to `'js`, and `Changed` is the
// same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for DomData<'js> {
    type Changed<'to> = DomData<'to>;
}

fn tag_table<'js>(table: Object<'js>) -> Result<HashMap<String, Object<'js>>> {
    table.props::<String, Object<'js>>().collect()
}

/// Stores the prototypes from `protos`, creates the document wrapper and
/// installs the native members on the prototypes in `protos.interfaces`.
pub(crate) fn init<'js>(
    ctx: &Ctx<'js>,
    dom: &Rc<DomState>,
    protos: Object<'js>,
    exception: Function<'js>,
) -> Result<()> {
    let document = create(dom, dom.doc().root(), protos.get("Document")?)?;
    let interfaces: Object<'js> = protos.get("interfaces")?;
    let fallbacks = members::install(ctx, &interfaces)?;
    let hooks = Hooks::new(ctx, fallbacks, &protos.get("hooks")?, &interfaces)?;
    let (hosts, host_protos) = Hosts::register(ctx, &interfaces)?;
    let members = Members::attach(ctx, &document, hosts, hooks);
    attrs::install_reflected(ctx, &members, protos.get("reflected")?)?;
    let data = DomData {
        exception,
        members,
        _host_protos: host_protos,
        document,
        mutation_hook: RefCell::new(None),
        ce_hook: RefCell::new(None),
        ce_held: RefCell::new(Vec::new()),
        layout_hook: RefCell::new(None),
        element: protos.get("Element")?,
        html_element: protos.get("HTMLElement")?,
        html_unknown: protos.get("HTMLUnknownElement")?,
        svg_element: protos.get("SVGElement")?,
        mathml_element: protos.get("MathMLElement")?,
        text: protos.get("Text")?,
        comment: protos.get("Comment")?,
        doctype: protos.get("DocumentType")?,
        fragment: protos.get("DocumentFragment")?,
        shadow_root: protos.get("ShadowRoot")?,
        html_tags: tag_table(protos.get("html")?)?,
        svg_tags: tag_table(protos.get("svg")?)?,
    };
    if ctx.store_userdata(data).is_err() {
        return Err(rquickjs::Error::Unknown);
    }
    Ok(())
}

fn prototype_for<'js>(ctx: &Ctx<'js>, doc: &Document, id: NodeId) -> Result<Object<'js>> {
    let data = ctx
        .userdata::<DomData<'js>>()
        .ok_or(rquickjs::Error::Unknown)?;
    let proto = match doc.kind(id) {
        NodeKind::Element => {
            let ns = doc.namespace(id).unwrap_or(Name::EMPTY);
            let local = doc.name_str(doc.local_name(id).unwrap_or(Name::EMPTY));
            if ns == Name::NS_HTML {
                match data.html_tags.get(local) {
                    Some(proto) => proto,
                    None if local.contains('-') => &data.html_element,
                    None => &data.html_unknown,
                }
            } else if ns == Name::NS_SVG {
                data.svg_tags.get(local).unwrap_or(&data.svg_element)
            } else if ns == Name::NS_MATHML {
                &data.mathml_element
            } else {
                &data.element
            }
        }
        NodeKind::Text => &data.text,
        NodeKind::Comment => &data.comment,
        NodeKind::Doctype => &data.doctype,
        NodeKind::DocumentFragment if doc.shadow_host(id).is_some() => &data.shadow_root,
        NodeKind::DocumentFragment => &data.fragment,
        NodeKind::Document => {
            return data
                .document
                .get_prototype()
                .ok_or(rquickjs::Error::Unknown);
        }
    };
    Ok(proto.clone())
}

/// A new wrapper for `id`, entered in the cache but not in any ring.
fn create<'js>(dom: &Rc<DomState>, id: NodeId, proto: Object<'js>) -> Result<Node<'js>> {
    let wrapper = NodeWrapper {
        id,
        dom: Rc::clone(dom),
        stale: Cell::new(false),
        next: RefCell::new(None),
        prev: RefCell::new(None),
        extras: RefCell::new(None),
    };
    let node = Class::instance_proto(wrapper, proto)?;
    dom.wrappers.insert(id, node.as_raw());
    Ok(node)
}

/// The cache from node to live wrapper: the wrapper's object pointer in a
/// slot indexed by the node's arena index, null where there is none.
///
/// Weak: a slot is cleared by its wrapper's finalizer, so it never keeps
/// the wrapper alive. The slots are only borrowed briefly, never across
/// anything that can run JS or the GC (finalizers write to them).
#[derive(Default)]
pub(crate) struct Wrappers {
    slots: RefCell<Vec<*mut c_void>>,
}

impl Wrappers {
    /// The wrapper object of `id`, borrowed from the cache: it stays valid
    /// only until JS or the GC next runs.
    #[inline]
    pub(crate) fn get(&self, id: NodeId) -> Option<qjs::JSValue> {
        let ptr = *self.slots.borrow().get(index(id))?;
        (!ptr.is_null()).then(|| qjs::JS_MKPTR(qjs::JS_TAG_OBJECT, ptr))
    }

    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    fn insert(&self, id: NodeId, wrapper: qjs::JSValue) {
        let mut slots = self.slots.borrow_mut();
        let i = index(id);
        if slots.len() <= i {
            slots.resize(i + 1, std::ptr::null_mut());
        }
        // SAFETY: `wrapper` is an object value, so it holds a pointer.
        slots[i] = unsafe { qjs::JS_VALUE_GET_PTR(wrapper) };
    }

    fn remove(&self, id: NodeId) {
        if let Some(slot) = self.slots.borrow_mut().get_mut(index(id)) {
            *slot = std::ptr::null_mut();
        }
    }

    /// The nodes that have a live wrapper, in arena order.
    pub(crate) fn ids(&self) -> Vec<NodeId> {
        let slots = self.slots.borrow();
        let wrapped = slots.iter().enumerate().filter(|(_, p)| !p.is_null());
        wrapped.map(|(i, _)| node_at(i)).collect()
    }

    /// Empties every slot but `keep`'s.
    fn clear_except(&self, keep: NodeId) {
        let mut slots = self.slots.borrow_mut();
        for (i, slot) in slots.iter_mut().enumerate() {
            if i != index(keep) {
                *slot = std::ptr::null_mut();
            }
        }
    }
}

fn index(id: NodeId) -> usize {
    id.to_raw() as usize - 1
}

fn node_at(index: usize) -> NodeId {
    let raw = u32::try_from(index + 1).expect("arena indexes fit in u32");
    NodeId::from_raw(raw).expect("index + 1 is non-zero")
}

impl DomState {
    /// The live wrapper of `id`, if there is one.
    pub(crate) fn cached<'js>(&self, ctx: &Ctx<'js>, id: NodeId) -> Option<Node<'js>> {
        let raw = self.wrappers.get(id)?;
        Some(from_raw(ctx, raw))
    }

    /// The wrappers of `ids` that are alive.
    pub(crate) fn cached_all<'js>(&self, ctx: &Ctx<'js>, ids: &[NodeId]) -> Vec<Node<'js>> {
        let raws: Vec<qjs::JSValue> = ids.iter().filter_map(|&id| self.wrappers.get(id)).collect();
        raws.into_iter().map(|raw| from_raw(ctx, raw)).collect()
    }
}

/// A new reference to the wrapper object `raw` from the cache.
fn from_raw<'js>(ctx: &Ctx<'js>, raw: qjs::JSValue) -> Node<'js> {
    // SAFETY: cache entries are removed by the wrapper's finalizer before
    // its object is freed, and no JS (so no GC) has run since `raw` was read
    // from the cache, so `raw` is a live wrapper object. Duplicating it gives
    // us a reference of our own, which `Value` then owns.
    let value =
        unsafe { Value::from_raw(ctx.clone(), qjs::JS_DupValue(ctx.as_raw().as_ptr(), raw)) };
    // SAFETY: the cache only holds node wrappers, and `Class` is a
    // transparent wrapper of `Object`, itself one of `Value`: this is what
    // `Class::from_value` returns once it has checked the class.
    unsafe { std::mem::transmute::<Value<'js>, Node<'js>>(value) }
}

/// The wrapper of `id`, creating it if needed.
pub(crate) fn wrap<'js>(ctx: &Ctx<'js>, dom: &Rc<DomState>, id: NodeId) -> Result<Node<'js>> {
    if let Some(node) = dom.cached(ctx, id) {
        return Ok(node);
    }
    let (proto, anchor) = {
        let doc = dom.doc();
        let proto = prototype_for(ctx, &doc, id)?;
        (proto, find_anchor(ctx, dom, &doc, id, &[]))
    };
    let node = create(dom, id, proto)?;
    if let Some(anchor) = anchor {
        splice(&anchor, &node);
    }
    Ok(node)
}

/// A wrapper in the same tree as `id`, outside the subtrees rooted at
/// `skip`, to link a new member of the tree to.
fn find_anchor<'js>(
    ctx: &Ctx<'js>,
    dom: &DomState,
    doc: &Document,
    id: NodeId,
    skip: &[NodeId],
) -> Option<Node<'js>> {
    let raw = {
        let wrappers = &dom.wrappers;
        // Usually an ancestor is wrapped: the document always is.
        let mut found = None;
        let mut n = id;
        while let Some(up) = doc.parent(n).or_else(|| owner(doc, n)) {
            if let Some(raw) = wrappers.get(up) {
                found = Some(raw);
                break;
            }
            n = up;
        }
        if found.is_none() {
            // `id` itself is not wrapped yet, or is the parent of an
            // insertion and was checked by the caller.
            any_in_tree_except(doc, n, skip, |m| {
                found = wrappers.get(m);
                found.is_some()
            });
        }
        found
    }?;
    Some(from_raw(ctx, raw))
}

// -------------------------------------------------------------------------
// Rings.

// The links are `Option<Node>`s, `None` in both directions for a wrapper
// alone. Every clone or drop of a `Node` is two reference counts (the
// object's and the context's), so the operations below move links where
// they can rather than copy them. A link is only taken or replaced while
// its cell is borrowed, and the old values are dropped with no cell
// borrowed, since dropping the last reference to a wrapper runs its
// finalizer.

fn take_next<'js>(node: &Node<'js>) -> Option<Node<'js>> {
    node.borrow().next.borrow_mut().take()
}

fn take_prev<'js>(node: &Node<'js>) -> Option<Node<'js>> {
    node.borrow().prev.borrow_mut().take()
}

#[must_use = "the old link must be dropped once no cell is borrowed"]
fn replace_next<'js>(node: &Node<'js>, next: Option<Node<'js>>) -> Option<Node<'js>> {
    node.borrow().next.replace(next)
}

#[must_use = "the old link must be dropped once no cell is borrowed"]
fn replace_prev<'js>(node: &Node<'js>, prev: Option<Node<'js>>) -> Option<Node<'js>> {
    node.borrow().prev.replace(prev)
}

#[cfg(test)]
fn next<'js>(node: &Node<'js>) -> Node<'js> {
    node.borrow()
        .next
        .borrow()
        .clone()
        .unwrap_or_else(|| node.clone())
}

#[cfg(test)]
fn prev<'js>(node: &Node<'js>) -> Node<'js> {
    node.borrow()
        .prev
        .borrow()
        .clone()
        .unwrap_or_else(|| node.clone())
}

fn is_alone(node: &Node<'_>) -> bool {
    node.borrow().next.borrow().is_none()
}

/// Takes `node` out of its ring.
pub(crate) fn unlink(node: &Node<'_>) {
    let (Some(p), Some(n)) = (take_prev(node), take_next(node)) else {
        return;
    };
    if p == n {
        // A ring of two: the other one is left alone. Its links are both
        // `node`.
        let old = (take_next(&p), take_prev(&p));
        drop((p, n, old));
        return;
    }
    // `p.next` and `n.prev` were `node`.
    let old_prev = replace_prev(&n, Some(p.clone()));
    let old_next = replace_next(&p, Some(n));
    drop((p, old_prev, old_next));
}

/// Merges the rings of `a` and `b`, which must be different rings.
pub(crate) fn splice<'js>(a: &Node<'js>, b: &Node<'js>) {
    let (a_alone, b_alone) = (is_alone(a), is_alone(b));
    // The ring a -> an ... and the ring b -> bn ... become
    // a -> bn ... b -> an ..., where a lone wrapper is its own next.
    let an = take_next(a).unwrap_or_else(|| a.clone());
    let bn = take_next(b).unwrap_or_else(|| b.clone());
    let old = if a_alone || b_alone {
        (
            replace_prev(&bn, Some(a.clone())),
            replace_prev(&an, Some(b.clone())),
        )
    } else {
        // `bn.prev` is `b` and `an.prev` is `a`: swap them.
        let bp = take_prev(&bn);
        let ap = replace_prev(&an, bp);
        (replace_prev(&bn, ap), None)
    };
    let old_next = (replace_next(a, Some(bn)), replace_next(b, Some(an)));
    drop((old, old_next));
}

// -------------------------------------------------------------------------
// Keeping rings in step with trees.

/// The wrappers of nodes in the subtrees rooted at `roots`.
fn wrapped_under<'js>(
    ctx: &Ctx<'js>,
    dom: &DomState,
    doc: &Document,
    roots: &[NodeId],
) -> Vec<Node<'js>> {
    if let [root] = *roots {
        let leaf = doc.first_child(root).is_none()
            && doc.template_contents(root).is_none()
            && doc.shadow_root(root).is_none();
        if leaf {
            return dom.cached(ctx, root).into_iter().collect();
        }
    }
    let mut ids = Vec::new();
    for &root in roots {
        any_in_tree_except(doc, root, &[], |n| {
            if dom.wrappers.contains(n) {
                ids.push(n);
            }
            false
        });
    }
    dom.cached_all(ctx, &ids)
}

/// How a pending insertion moves wrappers between trees.
pub(crate) enum Move {
    /// Within one tree: nothing to do.
    SameTree,
    /// The node was the root of its tree, so its whole ring moves.
    WholeTree,
    /// A fragment that is the root of its own tree moves its children.
    Fragment,
    /// Anything else: find the wrappers under the moved nodes.
    Walk,
}

/// Works out, before `node` is inserted under `parent`, how the insertion
/// moves wrappers.
pub(crate) fn plan_insert(doc: &Document, parent: NodeId, node: NodeId) -> Move {
    let old = group_root(doc, node);
    if old == group_root(doc, parent) {
        Move::SameTree
    } else if old != node {
        Move::Walk
    } else if doc.kind(node) == NodeKind::DocumentFragment {
        Move::Fragment
    } else {
        Move::WholeTree
    }
}

/// Regroups wrappers after `node` (or, for a fragment, its former children
/// `moved`) was inserted under `parent`.
pub(crate) fn after_insert<'js>(
    ctx: &Ctx<'js>,
    dom: &DomState,
    doc: &Document,
    parent: NodeId,
    node: NodeId,
    moved: &[NodeId],
    plan: Move,
) {
    dom.inserted(doc, node);
    let whole = match plan {
        Move::SameTree => return,
        Move::WholeTree | Move::Fragment => dom.cached(ctx, node),
        Move::Walk => None,
    };
    let anchor_at = |dom: &DomState| {
        dom.cached(ctx, parent)
            .or_else(|| find_anchor(ctx, dom, doc, parent, moved))
    };
    if let Some(whole) = whole {
        if let Some(anchor) = anchor_at(dom) {
            splice(&anchor, &whole);
        }
        if matches!(plan, Move::Fragment) {
            unlink(&whole);
        }
        return;
    }
    let wrappers = wrapped_under(ctx, dom, doc, moved);
    let Some(first) = wrappers.first() else {
        return;
    };
    for w in &wrappers {
        unlink(w);
    }
    let anchor = anchor_at(dom);
    let (anchor, rest) = match &anchor {
        Some(a) => (a, &wrappers[..]),
        None => (first, &wrappers[1..]),
    };
    for w in rest {
        splice(anchor, w);
    }
}

/// Regroups wrappers after the subtree at `root` was detached from `from`.
/// If nothing in it has a wrapper, it is queued to be freed.
pub(crate) fn after_remove(
    ctx: &Ctx<'_>,
    dom: &DomState,
    doc: &Document,
    root: NodeId,
    from: NodeId,
) {
    dom.removed_from(doc, root, from);
    let wrappers = wrapped_under(ctx, dom, doc, &[root]);
    let Some((first, rest)) = wrappers.split_first() else {
        dom.add_candidate(root);
        return;
    };
    for w in &wrappers {
        unlink(w);
    }
    for w in rest {
        splice(first, w);
    }
}

/// Rebuilds every ring from scratch, after the host changed the tree.
pub(crate) fn regroup_all(ctx: &Ctx<'_>, dom: &DomState) {
    let ids = dom.wrappers.ids();
    let nodes = dom.cached_all(ctx, &ids);
    for n in &nodes {
        unlink(n);
    }
    let doc = dom.doc();
    let mut groups: HashMap<NodeId, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        let Some(id) = n.borrow().id() else { continue };
        let root = group_root(&doc, id);
        match groups.get(&root) {
            Some(&first) => splice(&nodes[first], n),
            None => {
                groups.insert(root, i);
            }
        }
    }
}

/// Cuts every wrapper but the document's off from the document, before it is
/// replaced.
pub(crate) fn detach_all(ctx: &Ctx<'_>, dom: &DomState) {
    let root = dom.doc().root();
    let ids = dom.wrappers.ids();
    let nodes = dom.cached_all(ctx, &ids);
    dom.wrappers.clear_except(root);
    for n in &nodes {
        if n.borrow().id != root {
            n.borrow().stale.set(true);
        }
    }
    for n in &nodes {
        unlink(n);
    }
}

/// Checks that the rings match the trees: each wrapper's ring holds exactly
/// the live wrappers of its tree, and the links are mirrored. Also checks
/// the node documents of detached trees.
#[cfg(test)]
pub(crate) fn check_groups(ctx: &Ctx<'_>, dom: &DomState) -> std::result::Result<(), String> {
    let ids = dom.wrappers.ids();
    let doc = dom.doc();
    let mut by_root: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for &id in &ids {
        by_root.entry(group_root(&doc, id)).or_default().push(id);
    }
    for list in by_root.values_mut() {
        list.sort_unstable();
    }
    // Each ring is walked once, from the first of its wrappers met.
    let mut seen = std::collections::HashSet::new();
    for &id in &ids {
        if seen.contains(&id) {
            continue;
        }
        let node = dom.cached(ctx, id).ok_or("cache entry vanished")?;
        let expected = &by_root[&group_root(&doc, id)];
        let mut ring = vec![id];
        let mut n = next(&node);
        while n != node {
            if prev(&next(&n)) != n {
                return Err(format!("unmirrored link at {:?}", n.borrow().id));
            }
            ring.push(n.borrow().id);
            if ring.len() > ids.len() {
                return Err(format!("ring of {id:?} does not close"));
            }
            n = next(&n);
        }
        seen.extend(ring.iter().copied());
        ring.sort_unstable();
        if &ring != expected {
            return Err(format!(
                "ring of {id:?} is {ring:?}, but its tree has wrappers {expected:?}"
            ));
        }
    }
    // Node documents name live detached roots and live document nodes
    // (looking at a released node panics).
    for (&tree, &owner) in dom.owners.borrow().iter() {
        if doc.kind(owner) != NodeKind::Document {
            return Err(format!("{tree:?} is owned by {owner:?}, not a document"));
        }
        if doc.kind(tree) == NodeKind::Document || doc.parent(tree).is_some() {
            return Err(format!("{tree:?} has an owner but is not a detached root"));
        }
    }
    Ok(())
}
