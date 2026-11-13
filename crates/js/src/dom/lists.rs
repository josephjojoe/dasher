//! Live collections as native objects: `NodeList` (`childNodes`,
//! `getElementsByName`), `HTMLCollection` (`children`, `getElementsBy*`,
//! `document.forms` and the like, and the form and select collections that
//! extend it) and `NamedNodeMap` (`el.attributes`).
//!
//! Each is a raw QuickJS object of a class registered here, whose opaque is
//! a [`List`]: the root node's wrapper (a reference of its own, reported by
//! the class's `gc_mark`, so the root's tree, and every id the list caches,
//! stays alive as long as the list) and what it holds. A node list or
//! collection keeps the matching node ids (`collection::Matches`),
//! recomputed when the DOM version changes, which is what keeps it live.
//!
//! Indexed access (`list[0]`), named access on an `HTMLCollection`
//! (`form.elements.email`, for a name not on the prototype chain) and
//! enumeration are the classes' exotic methods, so they run no JS and need
//! no `Proxy`; `length`, `item` and `namedItem` are native members with the
//! list receivers (see the `members` module). A `NamedNodeMap`'s items are
//! `Attr` objects, which `dom.js` makes (`Hooks::attr`).
//!
//! The lists `childNodes`, `children` and `attributes` return are made on
//! first use and kept in the wrapper's extras, so `el.children ===
//! el.children`.

use std::ffi::{CStr, c_int};
use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId};
use rquickjs::{Ctx, Result, qjs};

use super::DomState;
use super::collection::{Filter, Matches};
use super::host::{ATOM_TAG_INT, Key, define_plain, delete_nothing, fill, key_of, proto_has};
use super::members::{Call, Members, Outcome, Ret, Thrown, ctx_of};
use super::wrapper::{Extra, NodeWrapper};

/// The kinds of list, each a class of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListClass {
    NodeList,
    HtmlCollection,
    NamedNodeMap,
}

impl ListClass {
    const ALL: [ListClass; 3] = [
        ListClass::NodeList,
        ListClass::HtmlCollection,
        ListClass::NamedNodeMap,
    ];

    fn name(self) -> &'static CStr {
        match self {
            ListClass::NodeList => c"NodeList",
            ListClass::HtmlCollection => c"HTMLCollection",
            ListClass::NamedNodeMap => c"NamedNodeMap",
        }
    }

    fn exotic(self) -> &'static qjs::JSClassExoticMethods {
        match self {
            ListClass::NodeList => &NODE_LIST_EXOTIC,
            ListClass::HtmlCollection => &COLLECTION_EXOTIC,
            ListClass::NamedNodeMap => &ATTRIBUTES_EXOTIC,
        }
    }
}

/// The list classes of a runtime.
pub(crate) struct Lists {
    ids: [qjs::JSClassID; 3],
}

impl Lists {
    /// Registers the classes.
    pub(crate) fn register(ctx: &Ctx<'_>) -> Result<Lists> {
        // SAFETY: `ctx` is live.
        let rt = unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) };
        let mut ids = [0; 3];
        for class in ListClass::ALL {
            let def = qjs::JSClassDef {
                class_name: class.name().as_ptr(),
                finalizer: Some(finalize),
                gc_mark: Some(mark),
                call: None,
                // QuickJS keeps the pointer and only reads through it.
                exotic: std::ptr::from_ref(class.exotic()).cast_mut(),
            };
            let mut id: qjs::JSClassID = 0;
            // SAFETY: `rt` is live; QuickJS copies what it needs of `def`.
            unsafe {
                qjs::JS_NewClassID(rt, &mut id);
                if qjs::JS_NewClass(rt, id, &def) != 0 {
                    return Err(rquickjs::Error::Allocation);
                }
            }
            ids[class as usize] = id;
        }
        Ok(Lists { ids })
    }

    /// The list `value` is, if it is one of `class`.
    ///
    /// # Safety
    ///
    /// `value` must be alive, and stays borrowed for the result's life.
    pub(crate) unsafe fn get<'v>(&self, value: qjs::JSValue, class: ListClass) -> Option<&'v List> {
        // SAFETY: `JS_GetOpaque` checks the tag and class; objects of these
        // classes have a `List` as their opaque.
        unsafe {
            qjs::JS_GetOpaque(value, self.ids[class as usize])
                .cast::<List>()
                .as_ref()
        }
    }

    /// The list `value` is, of any class.
    ///
    /// # Safety
    ///
    /// As for [`Lists::get`].
    pub(crate) unsafe fn any<'v>(&self, value: qjs::JSValue) -> Option<&'v List> {
        // SAFETY: as for the function.
        ListClass::ALL
            .into_iter()
            .find_map(|class| unsafe { self.get(value, class) })
    }

    /// A new list object of `class` with prototype `proto`, or
    /// `JS_EXCEPTION`.
    pub(crate) fn create(
        &self,
        ctx: *mut qjs::JSContext,
        class: ListClass,
        proto: qjs::JSValue,
        list: List,
    ) -> qjs::JSValue {
        // SAFETY: the caller keeps `proto` alive; the class is registered
        // in this runtime.
        let obj = unsafe { qjs::JS_NewObjectProtoClass(ctx, proto, self.ids[class as usize]) };
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(obj) } {
            return obj;
        }
        let ptr = Box::into_raw(Box::new(list));
        // SAFETY: `obj` is a new object of our class; its finalizer frees
        // the box.
        unsafe { qjs::JS_SetOpaque(obj, ptr.cast()) };
        obj
    }
}

unsafe extern "C" fn finalize(_rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
    // SAFETY: `val` is an object of one of our classes being freed; its
    // opaque is the box `Lists::create` made, or null.
    unsafe {
        let ptr = qjs::JS_GetOpaque(val, qjs::JS_GetClassID(val)).cast::<List>();
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
    }
}

unsafe extern "C" fn mark(rt: *mut qjs::JSRuntime, val: qjs::JSValue, mark: qjs::JS_MarkFunc) {
    // SAFETY: as in `finalize`.
    unsafe {
        let ptr = qjs::JS_GetOpaque(val, qjs::JS_GetClassID(val)).cast::<List>();
        if let Some(list) = ptr.as_ref() {
            qjs::JS_MarkValue(rt, list.owner, mark);
        }
    }
}

/// The Rust side of a list object.
pub(crate) struct List {
    rt: *mut qjs::JSRuntime,
    /// The root node's wrapper object: a reference of our own.
    owner: qjs::JSValue,
    /// The `NodeWrapper` in `owner`, which does not move while it lives.
    wrapper: *const NodeWrapper<'static>,
    /// What a node list or collection holds; `None` for a `NamedNodeMap`.
    matches: Option<Matches>,
}

impl List {
    /// A list over the node `node` (borrowed), whose wrapper is `wrapper`,
    /// holding what `filter` picks (`None` for attributes).
    pub(crate) fn new(
        ctx: *mut qjs::JSContext,
        node: qjs::JSValue,
        wrapper: &NodeWrapper<'_>,
        filter: Option<Filter>,
    ) -> List {
        List {
            // SAFETY: `ctx` is live.
            rt: unsafe { qjs::JS_GetRuntime(ctx) },
            // SAFETY: `node` is alive; the duplicate is ours.
            owner: unsafe { qjs::JS_DupValue(ctx, node) },
            wrapper: std::ptr::from_ref(wrapper).cast(),
            matches: filter.map(Matches::new),
        }
    }

    /// The root node's wrapper.
    pub(crate) fn wrapper(&self) -> &NodeWrapper<'_> {
        // SAFETY: `owner` keeps the wrapper object, and so its cell, alive.
        unsafe { &*self.wrapper.cast::<NodeWrapper<'_>>() }
    }

    /// The root node's wrapper object, borrowed.
    pub(crate) fn owner(&self) -> qjs::JSValue {
        self.owner
    }

    /// Whether this is a `NamedNodeMap`.
    pub(crate) fn is_attributes(&self) -> bool {
        self.matches.is_none()
    }

    /// The nodes the list holds now; empty for a `NamedNodeMap`.
    pub(crate) fn ids(&self, dom: &DomState, doc: &Document, root: NodeId) -> Rc<[NodeId]> {
        match &self.matches {
            Some(m) => m.ids(doc, root, dom.version()),
            None => Rc::from([]),
        }
    }
}

impl Drop for List {
    fn drop(&mut self) {
        // SAFETY: the reference is ours, in this runtime, which outlives
        // its objects.
        unsafe { qjs::JS_FreeValueRT(self.rt, self.owner) };
    }
}

/// The first of `ids` that `name` names, as `namedItem` finds it: by `id`,
/// or by `name` for an HTML element.
pub(crate) fn named_item(doc: &Document, ids: &[NodeId], name: &str) -> Option<NodeId> {
    if name.is_empty() {
        return None;
    }
    ids.iter().copied().find(|&n| {
        doc.element_id(n) == Some(name)
            || (doc.namespace(n) == Some(Name::NS_HTML)
                && doc.attribute_named(n, Name::NAME) == Some(name))
    })
}

/// The names an `HTMLCollection` supports, in order: each element's `id`,
/// and the `name` of an HTML element, without repeats or empty ones.
pub(crate) fn supported_names(doc: &Document, ids: &[NodeId]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for &n in ids {
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
    names
}

// -------------------------------------------------------------------------
// The members.

/// The list behind a `Call` with a list receiver: `call_member` passes the
/// list object itself as the call's `node`.
fn list_of<'c>(c: &'c Call<'_>, class: ListClass) -> std::result::Result<&'c List, Thrown> {
    // SAFETY: `c.node` is `this`, alive for the call.
    unsafe { c.members.lists().get(c.node, class) }.ok_or(Thrown::Illegal)
}

pub(super) fn node_list_length<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = list_of(c, ListClass::NodeList)?;
    Ok(Ret::Int(len(&list.ids(c.dom, &c.doc, n))))
}

pub(super) fn collection_length<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let list = list_of(c, ListClass::HtmlCollection)?;
    Ok(Ret::Int(len(&list.ids(c.dom, &c.doc, n))))
}

fn len(ids: &[NodeId]) -> i32 {
    i32::try_from(ids.len()).unwrap_or(i32::MAX)
}

pub(super) fn collection_item<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let list = list_of(c, ListClass::HtmlCollection)?;
    let i = c.u32_arg(0) as usize;
    Ok(Ret::Node(list.ids(c.dom, &c.doc, n).get(i).copied()))
}

pub(super) fn named_item_member<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let list = list_of(c, ListClass::HtmlCollection)?;
    let name = c.str_arg(0)?;
    let ids = list.ids(c.dom, &c.doc, n);
    Ok(Ret::Node(named_item(&c.doc, &ids, &name.as_str())))
}

pub(super) fn attributes_length<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    list_of(c, ListClass::NamedNodeMap)?;
    Ok(Ret::Int(len_of(c.doc.attrs(n).len())))
}

fn len_of(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// The list in the wrapper's `slot`, made the first time.
fn kept<'c>(
    c: &'c Call<'_>,
    slot: Extra,
    class: ListClass,
    proto: qjs::JSValue,
    filter: Option<Filter>,
) -> Outcome<'c> {
    if let Some(raw) = c.wrapper.extra(slot) {
        // SAFETY: the slot holds a live object; the duplicate is the caller's.
        return Ok(Ret::Value(unsafe { qjs::JS_DupValue(c.ctx, raw) }));
    }
    let list = List::new(c.ctx, c.node, c.wrapper, filter);
    let obj = c.members.lists().create(c.ctx, class, proto, list);
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(obj) } {
        return Err(Thrown::Pending);
    }
    // SAFETY: `c.ctx` is the running call's; the `Value` owns a new
    // reference to `obj`.
    let value = unsafe {
        let ctx = ctx_of(c.ctx);
        rquickjs::Value::from_raw(ctx, qjs::JS_DupValue(c.ctx, obj))
    };
    c.wrapper.set_extra(slot, value);
    Ok(Ret::Value(obj))
}

pub(super) fn child_nodes<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let proto = c.members.proto(Proto::NodeList);
    kept(c, Extra::ChildNodes, ListClass::NodeList, proto, Some(Filter::ChildNodes))
}

pub(super) fn children<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let proto = c.members.proto(Proto::HtmlCollection);
    kept(c, Extra::Children, ListClass::HtmlCollection, proto, Some(Filter::Children))
}

pub(super) fn attributes<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let proto = c.members.proto(Proto::NamedNodeMap);
    kept(c, Extra::Attributes, ListClass::NamedNodeMap, proto, None)
}

/// The prototypes lists are made with.
#[derive(Clone, Copy)]
pub(crate) enum Proto {
    NodeList,
    HtmlCollection,
    NamedNodeMap,
}

// -------------------------------------------------------------------------
// The exotic methods.

static NODE_LIST_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(nodes_get_own),
    get_own_property_names: Some(nodes_names_of),
    delete_property: Some(delete_nothing),
    define_own_property: Some(define_unless_index),
    has_property: None,
    get_property: None,
    set_property: None,
};

static COLLECTION_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(collection_get_own),
    get_own_property_names: Some(collection_names_of),
    delete_property: Some(delete_nothing),
    define_own_property: Some(define_unless_index),
    has_property: None,
    get_property: None,
    set_property: None,
};

static ATTRIBUTES_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(attributes_get_own),
    get_own_property_names: Some(attributes_names_of),
    delete_property: Some(delete_nothing),
    define_own_property: Some(define_unless_index),
    has_property: None,
    get_property: None,
    set_property: None,
};

/// What an exotic method needs: the members, the list behind `obj` and its
/// root. `Err` with an exception thrown for a node of a replaced document;
/// `None` when `obj` is not ours.
///
/// # Safety
///
/// `ctx` must be live and `obj` alive.
unsafe fn list_in<'a>(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    class: ListClass,
) -> Option<std::result::Result<(&'a Members, &'a List, NodeId), ()>> {
    // SAFETY: the context's opaque is null or the live `Members`.
    let members = unsafe { qjs::JS_GetContextOpaque(ctx).cast::<Members>().as_ref() }?;
    // SAFETY: as for the function.
    let list = unsafe { members.lists().get(obj, class) }?;
    Some(match list.wrapper().id() {
        Some(id) => Ok((members, list, id)),
        None => {
            // SAFETY: as for the function.
            unsafe { Thrown::Stale.throw(ctx) };
            Err(())
        }
    })
}

/// Fills `desc` with item `i` of the list's nodes, if there is one.
///
/// # Safety
///
/// As for an exotic `get_own_property`.
unsafe fn item_property(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    list: &List,
    root: NodeId,
    i: usize,
) -> c_int {
    let dom = list.wrapper().dom();
    let item = {
        let doc = dom.doc();
        list.ids(dom, &doc, root).get(i).copied()
    };
    let Some(item) = item else { return 0 };
    if desc.is_null() {
        return 1;
    }
    let value = node_value(ctx, dom, item);
    let flags = qjs::JS_PROP_ENUMERABLE | qjs::JS_PROP_CONFIGURABLE;
    // SAFETY: as for the function.
    unsafe { fill(ctx, desc, value, flags) }
}

/// The wrapper of `id`, as a new reference, or `JS_EXCEPTION`.
fn node_value(ctx: *mut qjs::JSContext, dom: &Rc<DomState>, id: NodeId) -> qjs::JSValue {
    if let Some(raw) = dom.wrappers.get(id) {
        // SAFETY: the cache holds live wrappers; the duplicate is ours.
        return unsafe { qjs::JS_DupValue(ctx, raw) };
    }
    // SAFETY: `ctx` is live.
    let rctx = unsafe { ctx_of(ctx) };
    match super::wrapper::wrap(&rctx, dom, id) {
        Ok(node) => {
            let raw = node.as_raw();
            // SAFETY: `node` is alive; the duplicate is the caller's.
            unsafe { qjs::JS_DupValue(ctx, raw) }
        }
        Err(_) => qjs::JS_EXCEPTION,
    }
}

unsafe extern "C" fn nodes_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    if atom & ATOM_TAG_INT == 0 {
        return 0;
    }
    // SAFETY: QuickJS calls this with a live context, object and atom.
    let Some(found) = (unsafe { list_in(ctx, obj, ListClass::NodeList) }) else {
        return 0;
    };
    let Ok((_, list, root)) = found else { return -1 };
    // SAFETY: as above.
    unsafe { item_property(ctx, desc, list, root, (atom & !ATOM_TAG_INT) as usize) }
}

unsafe extern "C" fn collection_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    // SAFETY: QuickJS calls this with a live context, object and atom.
    let Some(found) = (unsafe { list_in(ctx, obj, ListClass::HtmlCollection) }) else {
        return 0;
    };
    let Ok((members, list, root)) = found else { return -1 };
    if atom & ATOM_TAG_INT != 0 {
        // SAFETY: as above.
        return unsafe { item_property(ctx, desc, list, root, (atom & !ATOM_TAG_INT) as usize) };
    }
    if members.is_list_member(atom) {
        return 0;
    }
    // SAFETY: as above.
    let key = match unsafe { key_of(ctx, atom) } {
        Err(()) => return -1,
        Ok(Key::Str(key)) => key,
        Ok(_) => return 0,
    };
    // Named properties only show where the prototype chain has nothing.
    // SAFETY: as above; nothing is borrowed.
    match unsafe { proto_has(ctx, obj, atom) } {
        0 => {}
        1 => return 0,
        _ => return -1,
    }
    let dom = list.wrapper().dom();
    let item = {
        let doc = dom.doc();
        let ids = list.ids(dom, &doc, root);
        named_item(&doc, &ids, &key.as_str())
    };
    let Some(item) = item else { return 0 };
    if desc.is_null() {
        return 1;
    }
    let value = node_value(ctx, dom, item);
    // SAFETY: as above.
    unsafe { fill(ctx, desc, value, qjs::JS_PROP_CONFIGURABLE) }
}

unsafe extern "C" fn attributes_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    // SAFETY: QuickJS calls this with a live context, object and atom.
    let Some(found) = (unsafe { list_in(ctx, obj, ListClass::NamedNodeMap) }) else {
        return 0;
    };
    let Ok((members, list, root)) = found else { return -1 };
    let key = if atom & ATOM_TAG_INT != 0 {
        let i = (atom & !ATOM_TAG_INT) as usize;
        if i >= list.wrapper().dom().doc().attrs(root).len() {
            return 0;
        }
        AttrKey::Index(i)
    } else {
        if members.is_list_member(atom) {
            return 0;
        }
        // SAFETY: as above.
        match unsafe { key_of(ctx, atom) } {
            Err(()) => return -1,
            Ok(Key::Str(_)) => {}
            Ok(_) => return 0,
        }
        // SAFETY: as above; nothing is borrowed.
        match unsafe { proto_has(ctx, obj, atom) } {
            0 => {}
            1 => return 0,
            _ => return -1,
        }
        AttrKey::Name(atom)
    };
    // The `Attr` objects are the glue's.
    // SAFETY: as above.
    let value = unsafe { members.attr(ctx, list.owner(), key) };
    // SAFETY: tag checks.
    if unsafe { qjs::JS_IsException(value) } {
        return -1;
    }
    // SAFETY: as above.
    if unsafe { qjs::JS_IsNull(value) } {
        return 0;
    }
    let flags = match key {
        AttrKey::Index(_) => qjs::JS_PROP_ENUMERABLE | qjs::JS_PROP_CONFIGURABLE,
        AttrKey::Name(_) => qjs::JS_PROP_CONFIGURABLE,
    };
    // SAFETY: as above.
    unsafe { fill(ctx, desc, value, flags) }
}

/// Which `Attr` an exotic `NamedNodeMap` lookup wants.
#[derive(Clone, Copy)]
pub(crate) enum AttrKey {
    Index(usize),
    /// By qualified name: the property key.
    Name(qjs::JSAtom),
}

/// Gives QuickJS the property list: `indexes` indexes (enumerable), then
/// `names` (not enumerable).
///
/// # Safety
///
/// As for an exotic `get_own_property_names`.
unsafe fn property_list(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    indexes: usize,
    names: &[String],
) -> c_int {
    let n = indexes + names.len();
    // SAFETY: as for the function.
    unsafe {
        *ptab = std::ptr::null_mut();
        *plen = 0;
        if n == 0 {
            return 0;
        }
        let size = n * size_of::<qjs::JSPropertyEnum>();
        let tab = qjs::js_malloc(ctx, size as qjs::size_t).cast::<qjs::JSPropertyEnum>();
        if tab.is_null() {
            return -1;
        }
        for i in 0..indexes {
            tab.add(i).write(qjs::JSPropertyEnum {
                is_enumerable: true,
                atom: qjs::JS_NewAtomUInt32(ctx, i as u32),
            });
        }
        for (j, name) in names.iter().enumerate() {
            tab.add(indexes + j).write(qjs::JSPropertyEnum {
                is_enumerable: false,
                atom: qjs::JS_NewAtomLen(ctx, name.as_ptr().cast(), name.len() as qjs::size_t),
            });
        }
        *ptab = tab;
        *plen = n as u32;
    }
    0
}

/// The number of nodes in the list behind `obj`, and its supported names
/// if `named`.
///
/// # Safety
///
/// `ctx` must be live and `obj` alive.
unsafe fn list_keys(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    class: ListClass,
) -> std::result::Result<(usize, Vec<String>), ()> {
    // SAFETY: as for the function.
    match unsafe { list_in(ctx, obj, class) } {
        None => Ok((0, Vec::new())),
        Some(Err(())) => Err(()),
        Some(Ok((_, list, root))) => {
            let dom = list.wrapper().dom();
            let doc = dom.doc();
            Ok(match class {
                ListClass::NamedNodeMap => (doc.attrs(root).len(), Vec::new()),
                ListClass::NodeList => (list.ids(dom, &doc, root).len(), Vec::new()),
                ListClass::HtmlCollection => {
                    let ids = list.ids(dom, &doc, root);
                    (ids.len(), supported_names(&doc, &ids))
                }
            })
        }
    }
}

unsafe fn names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
    class: ListClass,
) -> c_int {
    // SAFETY: QuickJS calls the exotic methods with live values.
    let Ok((indexes, names)) = (unsafe { list_keys(ctx, obj, class) }) else {
        return -1;
    };
    // SAFETY: as above.
    unsafe { property_list(ctx, ptab, plen, indexes, &names) }
}

unsafe extern "C" fn nodes_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as for an exotic method.
    unsafe { names_of(ctx, ptab, plen, obj, ListClass::NodeList) }
}

unsafe extern "C" fn collection_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as for an exotic method.
    unsafe { names_of(ctx, ptab, plen, obj, ListClass::HtmlCollection) }
}

unsafe extern "C" fn attributes_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as for an exotic method.
    unsafe { names_of(ctx, ptab, plen, obj, ListClass::NamedNodeMap) }
}

unsafe extern "C" fn define_unless_index(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
    val: qjs::JSValue,
    getter: qjs::JSValue,
    setter: qjs::JSValue,
    flags: c_int,
) -> c_int {
    if atom & ATOM_TAG_INT != 0 {
        // The indexes are read-only; assigning one does nothing, as the
        // `Proxy` before.
        return 1;
    }
    // SAFETY: QuickJS calls this with live values.
    unsafe { define_plain(ctx, obj, atom, val, getter, setter, flags) }
}

/// `N.collection(root, kind, a, b, proto, named)`: a new live list over
/// `root`, an `HTMLCollection` if `named`, else a `NodeList`.
pub(super) fn new_collection<'js>(
    ctx: &Ctx<'js>,
    root: &super::wrapper::Node<'js>,
    filter: Filter,
    proto: &rquickjs::Object<'js>,
    named: bool,
) -> Result<rquickjs::Value<'js>> {
    let raw_ctx = ctx.as_raw().as_ptr();
    // SAFETY: the context's opaque is the live `Members`, if set.
    let members = unsafe { qjs::JS_GetContextOpaque(raw_ctx).cast::<Members>().as_ref() };
    let members = members.ok_or(rquickjs::Error::Unknown)?;
    let list = {
        let wrapper = root.borrow();
        List::new(raw_ctx, root.as_raw(), &wrapper, Some(filter))
    };
    let class = if named {
        ListClass::HtmlCollection
    } else {
        ListClass::NodeList
    };
    let obj = members.lists().create(raw_ctx, class, proto.as_raw(), list);
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(obj) } {
        return Err(rquickjs::Error::Exception);
    }
    // SAFETY: `obj` is a new reference, which the `Value` takes.
    Ok(unsafe { rquickjs::Value::from_raw(ctx.clone(), obj) })
}

/// The list behind `value`, of any class, with its root, for the natives
/// `dom.js` calls (`N.collLength` and friends, `N.mapOwner`).
pub(super) fn list_value<'a>(
    ctx: &Ctx<'_>,
    value: &rquickjs::Value<'_>,
) -> Result<(&'a List, NodeId)> {
    let raw_ctx = ctx.as_raw().as_ptr();
    // SAFETY: the context's opaque is the live `Members`, if set.
    let members = unsafe { qjs::JS_GetContextOpaque(raw_ctx).cast::<Members>().as_ref() };
    // SAFETY: `value` is alive while the caller holds it, which covers the
    // caller's use of the list.
    let list = members.and_then(|m| unsafe { m.lists().any(value.as_raw()) });
    let list = list.ok_or_else(|| rquickjs::Exception::throw_type(ctx, "Illegal invocation"))?;
    let root = list
        .wrapper()
        .id()
        .ok_or_else(|| super::natives::stale_node_error(ctx))?;
    Ok((list, root))
}
