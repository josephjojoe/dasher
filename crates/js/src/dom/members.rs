//! DOM interface members implemented in Rust and installed straight on the
//! prototypes as QuickJS C functions. The pattern, and how to add a member,
//! is described in the `dom` module docs ("Native members").
//!
//! Every member goes through one C function, [`call_member`], whose magic
//! number is the member's index in [`MEMBERS`]. It finds the node wrapper
//! behind `this` without going through rquickjs (see [`Members::wrapper`]),
//! checks it is the kind of node the member's interface needs, borrows the
//! document and runs the member's [`Impl`], which says what to return as a
//! [`Ret`]. Nothing is allocated on the way unless the result needs it.

use std::borrow::Cow;
use std::cell::{Ref, RefCell};
use std::ffi::{CStr, CString, c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId, NodeKind, ShadowRootMode};
use rquickjs::{Class, Ctx, Exception, Function, Object, Result, Value, qjs};

use super::DomState;
use crate::native_data::NativeData;
use super::natives::stale_node_error;
use super::wrapper::{Node, NodeWrapper, wrap};

/// What `this` has to be for a member: a node wrapper of a kind that has
/// the member's interface. Anything else throws `TypeError: Illegal
/// invocation`, as browsers do.
#[derive(Clone, Copy)]
enum Receiver {
    Node,
    Element,
    /// A text or comment node.
    CharacterData,
}

impl Receiver {
    fn accepts(self, doc: &Document, id: NodeId) -> bool {
        match self {
            Receiver::Node => true,
            Receiver::Element => doc.is_element(id),
            Receiver::CharacterData => doc.text(id).is_some(),
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    /// The `get` of an accessor. Its setter, if it has one, stays as
    /// `dom.js` defines it.
    Getter,
    /// A method taking `length` arguments (its `length` property).
    Method { length: u8 },
}

/// A member's body: given the call and the node behind `this`, and the
/// arguments for a method, what to return or throw.
type Impl = for<'c> fn(&'c Call<'_>, NodeId, &[qjs::JSValue]) -> Outcome<'c>;

type Outcome<'c> = std::result::Result<Ret<'c>, Thrown>;

/// One native member, and the interfaces whose prototypes get it.
struct Member {
    name: &'static str,
    kind: Kind,
    receiver: Receiver,
    /// Keys of the `interfaces` object `dom.js` registers.
    on: &'static [&'static str],
    f: Impl,
}

const fn getter(
    name: &'static str,
    receiver: Receiver,
    on: &'static [&'static str],
    f: Impl,
) -> Member {
    Member {
        name,
        kind: Kind::Getter,
        receiver,
        on,
        f,
    }
}

const fn method(
    name: &'static str,
    length: u8,
    receiver: Receiver,
    on: &'static [&'static str],
    f: Impl,
) -> Member {
    Member {
        name,
        kind: Kind::Method { length },
        receiver,
        on,
        f,
    }
}

const NODE: &[&str] = &["Node"];
const ELEMENT: &[&str] = &["Element"];
const CHARACTER_DATA: &[&str] = &["CharacterData"];
/// The `ParentNode` mixin's interfaces.
const PARENT_NODE: &[&str] = &["Element", "Document", "DocumentFragment"];
/// The `NonDocumentTypeChildNode` mixin's interfaces.
const CHILD_NODE: &[&str] = &["Element", "CharacterData"];

/// Every native member. A member's index here is its C function's magic
/// number.
static MEMBERS: &[Member] = &[
    // Node.
    getter("nodeType", Receiver::Node, NODE, node_type),
    getter("nodeName", Receiver::Node, NODE, node_name),
    getter("isConnected", Receiver::Node, NODE, is_connected),
    getter("ownerDocument", Receiver::Node, NODE, owner_document),
    getter("parentNode", Receiver::Node, NODE, parent_node),
    getter("parentElement", Receiver::Node, NODE, parent_element),
    getter("firstChild", Receiver::Node, NODE, first_child),
    getter("lastChild", Receiver::Node, NODE, last_child),
    getter("previousSibling", Receiver::Node, NODE, previous_sibling),
    getter("nextSibling", Receiver::Node, NODE, next_sibling),
    getter("nodeValue", Receiver::Node, NODE, node_value),
    method("hasChildNodes", 0, Receiver::Node, NODE, has_child_nodes),
    method("contains", 1, Receiver::Node, NODE, contains),
    // ParentNode and NonDocumentTypeChildNode.
    getter(
        "firstElementChild",
        Receiver::Node,
        PARENT_NODE,
        first_element_child,
    ),
    getter(
        "lastElementChild",
        Receiver::Node,
        PARENT_NODE,
        last_element_child,
    ),
    getter(
        "childElementCount",
        Receiver::Node,
        PARENT_NODE,
        child_element_count,
    ),
    getter(
        "previousElementSibling",
        Receiver::Node,
        CHILD_NODE,
        previous_element_sibling,
    ),
    getter(
        "nextElementSibling",
        Receiver::Node,
        CHILD_NODE,
        next_element_sibling,
    ),
    // Element.
    getter("namespaceURI", Receiver::Element, ELEMENT, namespace_uri),
    getter("localName", Receiver::Element, ELEMENT, local_name),
    getter("tagName", Receiver::Element, ELEMENT, tag_name),
    getter("id", Receiver::Element, ELEMENT, id),
    getter("className", Receiver::Element, ELEMENT, class_name),
    getter("shadowRoot", Receiver::Element, ELEMENT, shadow_root),
    // CharacterData.
    getter("data", Receiver::CharacterData, CHARACTER_DATA, data),
    getter("length", Receiver::CharacterData, CHARACTER_DATA, length),
];

// -------------------------------------------------------------------------
// The members.

fn node_type<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Int(c.doc.kind(n).node_type().into()))
}

fn node_name<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(match c.doc.kind(n) {
        NodeKind::Element => element_tag_name(&c.doc, n),
        NodeKind::Text => Ret::Static("#text"),
        NodeKind::Comment => Ret::Static("#comment"),
        NodeKind::Document => Ret::Static("#document"),
        NodeKind::DocumentFragment => Ret::Static("#document-fragment"),
        NodeKind::Doctype => Ret::Str(c.doc.node_name(n)),
    })
}

fn is_connected<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Bool(c.doc.is_connected(n)))
}

fn owner_document<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    if c.doc.kind(n) == NodeKind::Document {
        return Ok(Ret::Null);
    }
    Ok(Ret::Node(Some(c.dom.owner_document(&c.doc, n))))
}

fn parent_node<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.parent(n)))
}

fn parent_element<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.parent_element(n)))
}

fn first_child<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.first_child(n)))
}

fn last_child<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.last_child(n)))
}

fn previous_sibling<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.prev_sibling(n)))
}

fn next_sibling<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Node(c.doc.next_sibling(n)))
}

fn node_value<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(c.doc
        .text(n)
        .map_or(Ret::Null, |t| Ret::Str(Cow::Borrowed(t))))
}

fn has_child_nodes<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Bool(c.doc.first_child(n).is_some()))
}

fn contains<'c>(c: &'c Call<'_>, n: NodeId, args: &[qjs::JSValue]) -> Outcome<'c> {
    let Some(&other) = args.first() else {
        return Ok(Ret::Bool(false));
    };
    // SAFETY: plain tag checks of a value the caller owns.
    if unsafe { qjs::JS_IsNull(other) || qjs::JS_IsUndefined(other) } {
        return Ok(Ret::Bool(false));
    }
    let other = c.node_arg(
        other,
        c"Failed to execute 'contains' on 'Node': parameter 1 is not of type 'Node'.",
    )?;
    Ok(Ret::Bool(c.doc.contains(n, other)))
}

/// The first element among `start` and the nodes after it in `step`'s
/// direction.
fn element_from(
    doc: &Document,
    start: Option<NodeId>,
    step: fn(&Document, NodeId) -> Option<NodeId>,
) -> Option<NodeId> {
    let mut it = start;
    while let Some(x) = it {
        if doc.is_element(x) {
            return Some(x);
        }
        it = step(doc, x);
    }
    None
}

fn first_element_child<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let first = c.doc.first_child(n);
    Ok(Ret::Node(element_from(
        &c.doc,
        first,
        Document::next_sibling,
    )))
}

fn last_element_child<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let last = c.doc.last_child(n);
    Ok(Ret::Node(element_from(
        &c.doc,
        last,
        Document::prev_sibling,
    )))
}

fn child_element_count<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let count = c.doc.element_children(n).count();
    Ok(Ret::Int(i32::try_from(count).unwrap_or(i32::MAX)))
}

fn previous_element_sibling<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let prev = c.doc.prev_sibling(n);
    Ok(Ret::Node(element_from(
        &c.doc,
        prev,
        Document::prev_sibling,
    )))
}

fn next_element_sibling<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let next = c.doc.next_sibling(n);
    Ok(Ret::Node(element_from(
        &c.doc,
        next,
        Document::next_sibling,
    )))
}

fn namespace_uri<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(match c.doc.namespace(n) {
        Some(ns) if ns != Name::EMPTY => Ret::Name(ns),
        _ => Ret::Null,
    })
}

fn local_name<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(c.doc.local_name(n).map_or(Ret::Null, Ret::Name))
}

fn tag_name<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(element_tag_name(&c.doc, n))
}

/// An element's `tagName`, from the name cache unless it has a prefix.
fn element_tag_name(doc: &Document, n: NodeId) -> Ret<'_> {
    let Some(local) = doc.local_name(n) else {
        return Ret::Static("");
    };
    if doc.prefix(n) != Some(Name::EMPTY) {
        return Ret::Str(doc.tag_name(n).unwrap_or_default());
    }
    if doc.namespace(n) == Some(Name::NS_HTML) {
        Ret::UpperName(local)
    } else {
        Ret::Name(local)
    }
}

/// The value of the attribute `getAttribute(local)` finds, for a name that
/// is already lower case: the first with that local name and no prefix.
fn unprefixed_attribute(doc: &Document, n: NodeId, local: Name) -> Option<&str> {
    doc.attrs(n)
        .iter()
        .find(|a| a.local() == local && doc.attr_prefix(a) == Name::EMPTY)
        .map(|a| a.value())
}

fn id<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let value = unprefixed_attribute(&c.doc, n, Name::ID);
    Ok(value.map_or(Ret::Static(""), |v| Ret::Str(Cow::Borrowed(v))))
}

fn class_name<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let value = unprefixed_attribute(&c.doc, n, Name::CLASS);
    Ok(value.map_or(Ret::Static(""), |v| Ret::Str(Cow::Borrowed(v))))
}

fn shadow_root<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let root = c.doc.shadow_root(n).filter(|&root| {
        c.doc
            .shadow_root_init(root)
            .is_some_and(|init| init.mode == ShadowRootMode::Open)
    });
    Ok(Ret::Node(root))
}

fn data<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    Ok(Ret::Str(Cow::Borrowed(c.doc.text(n).unwrap_or_default())))
}

fn length<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let len = utf16_len(c.doc.text(n).unwrap_or_default());
    Ok(Ret::Int(i32::try_from(len).unwrap_or(i32::MAX)))
}

/// The length of `s` in UTF-16 code units, as JS counts it.
fn utf16_len(s: &str) -> usize {
    // Every character but a continuation byte starts a code unit, and a
    // four-byte character (lead byte 0xF0 and up) is a surrogate pair.
    s.bytes()
        .map(|b| usize::from(b & 0xC0 != 0x80) + usize::from(b >= 0xF0))
        .sum()
}

// -------------------------------------------------------------------------
// Calls.

/// What a member returns, turned into a JS value by [`Call::value`].
enum Ret<'c> {
    Null,
    Bool(bool),
    Int(i32),
    /// A node's wrapper, or `null`.
    Node(Option<NodeId>),
    /// A name from the document's name table, as a cached string.
    Name(Name),
    /// A name upper-cased, as a cached string (HTML `tagName`).
    UpperName(Name),
    /// A fixed string, cached.
    Static(&'static str),
    /// Any other string, made for this call.
    Str(Cow<'c, str>),
}

/// What a member throws.
enum Thrown {
    /// `TypeError: Illegal invocation`, for the wrong `this`.
    Illegal,
    /// A `TypeError` with this message.
    Type(&'static CStr),
    /// The `InvalidStateError` for a node of a replaced document.
    Stale,
}

/// One call of a member: what it can look at.
struct Call<'a> {
    ctx: *mut qjs::JSContext,
    members: &'a Members,
    dom: &'a Rc<DomState>,
    doc: Ref<'a, Document>,
}

impl Call<'_> {
    /// The node behind a wrapper passed as an argument.
    fn node_arg(
        &self,
        value: qjs::JSValue,
        not_a_node: &'static CStr,
    ) -> std::result::Result<NodeId, Thrown> {
        // SAFETY: `value` is an argument of the running call, so alive.
        let wrapper = unsafe { self.members.wrapper(self.ctx, value) };
        let wrapper = wrapper.ok_or(Thrown::Type(not_a_node))?;
        wrapper.id().ok_or(Thrown::Stale)
    }

    /// `ret` as a new JS value, or `JS_EXCEPTION` with an exception thrown.
    fn value(&self, ret: Ret<'_>) -> qjs::JSValue {
        match ret {
            Ret::Null | Ret::Node(None) => qjs::JS_NULL,
            Ret::Bool(true) => qjs::JS_TRUE,
            Ret::Bool(false) => qjs::JS_FALSE,
            Ret::Int(i) => qjs::JS_MKVAL(qjs::JS_TAG_INT, i),
            Ret::Node(Some(id)) => self.node(id),
            Ret::Name(name) => self.members.name(self.ctx, &self.doc, name, false),
            Ret::UpperName(name) => self.members.name(self.ctx, &self.doc, name, true),
            Ret::Static(s) => self.members.fixed(self.ctx, s),
            // SAFETY: the pointer and length are those of a live `str`.
            Ret::Str(s) => unsafe {
                qjs::JS_NewStringLen(self.ctx, s.as_ptr().cast(), s.len() as qjs::size_t)
            },
        }
    }

    /// The wrapper of `id`, made if there is none.
    fn node(&self, id: NodeId) -> qjs::JSValue {
        if let Some(raw) = self.dom.wrappers.get(id) {
            // SAFETY: the cache only holds live wrappers, and nothing has
            // run since it was read. The duplicate is the caller's.
            return unsafe { qjs::JS_DupValue(self.ctx, raw) };
        }
        // SAFETY: `self.ctx` is the context of the running call.
        let ctx = unsafe { ctx_of(self.ctx) };
        // A new wrapper's prototype comes from the document, which `wrap`
        // borrows again (shared, as here); finalizers the GC runs while it
        // allocates never touch the document.
        match wrap(&ctx, self.dom, id) {
            Ok(node) => into_raw(&ctx, node.into_value()),
            Err(e) => throw_error(&ctx, e),
        }
    }
}

impl Thrown {
    /// Throws this in `ctx`, returning `JS_EXCEPTION`.
    ///
    /// # Safety
    ///
    /// `ctx` must be the context of the running call.
    unsafe fn throw(self, ctx: *mut qjs::JSContext) -> qjs::JSValue {
        match self {
            // SAFETY: as for the function.
            Thrown::Illegal => unsafe { throw_type(ctx, c"Illegal invocation") },
            // SAFETY: as for the function.
            Thrown::Type(message) => unsafe { throw_type(ctx, message) },
            Thrown::Stale => {
                // SAFETY: as for the function.
                let ctx = unsafe { ctx_of(ctx) };
                throw_error(&ctx, stale_node_error(&ctx))
            }
        }
    }
}

/// Throws a `TypeError`, returning `JS_EXCEPTION`.
///
/// # Safety
///
/// `ctx` must be a live context.
unsafe fn throw_type(ctx: *mut qjs::JSContext, message: &CStr) -> qjs::JSValue {
    // SAFETY: `%s` formats the one C string passed.
    unsafe { qjs::JS_ThrowTypeError(ctx, c"%s".as_ptr(), message.as_ptr()) }
}

/// Makes sure `error` is thrown in `ctx`, returning `JS_EXCEPTION`.
fn throw_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> qjs::JSValue {
    match error {
        rquickjs::Error::Exception => {}
        rquickjs::Error::Allocation => {
            // SAFETY: `ctx` is live.
            unsafe { qjs::JS_ThrowOutOfMemory(ctx.as_raw().as_ptr()) };
        }
        other => {
            let _ = Exception::throw_internal(ctx, &other.to_string());
        }
    }
    qjs::JS_EXCEPTION
}

/// A `Ctx` for the running call's context, for the slow paths that need
/// rquickjs.
///
/// # Safety
///
/// `ctx` must be the context of the running call.
unsafe fn ctx_of<'js>(ctx: *mut qjs::JSContext) -> Ctx<'js> {
    // SAFETY: as for the function; the runtime is locked while JS runs.
    unsafe { Ctx::from_raw(NonNull::new_unchecked(ctx)) }
}

/// Hands `value`'s reference to the caller as a raw value.
fn into_raw(ctx: &Ctx<'_>, value: Value<'_>) -> qjs::JSValue {
    let raw = value.as_raw();
    // SAFETY: `value` is alive; the duplicate outlives it.
    unsafe { qjs::JS_DupValue(ctx.as_raw().as_ptr(), raw) }
}

/// The C function of every member; `magic` is its index in [`MEMBERS`].
unsafe extern "C" fn call_member(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    let member = &MEMBERS[magic as usize];
    // SAFETY: QuickJS passes the live context of the call.
    let members = unsafe { NativeData::of(ctx) }.and_then(NativeData::members);
    let Some(members) = members else {
        // SAFETY: QuickJS passes the live context of the call.
        return unsafe { throw_type(ctx, c"Illegal invocation") };
    };
    // SAFETY: `this` is alive for the call.
    let Some(wrapper) = (unsafe { members.wrapper(ctx, this) }) else {
        // SAFETY: as above.
        return unsafe { Thrown::Illegal.throw(ctx) };
    };
    let Some(id) = wrapper.id() else {
        // SAFETY: as above.
        return unsafe { Thrown::Stale.throw(ctx) };
    };
    let dom = wrapper.dom();
    let call = Call {
        ctx,
        members,
        dom,
        doc: dom.doc(),
    };
    if !member.receiver.accepts(&call.doc, id) {
        drop(call);
        // SAFETY: as above.
        return unsafe { Thrown::Illegal.throw(ctx) };
    }
    let args = match usize::try_from(argc) {
        // SAFETY: QuickJS passes `argc` live values at `argv`.
        Ok(n) if n > 0 && !argv.is_null() => unsafe { std::slice::from_raw_parts(argv, n) },
        _ => &[],
    };
    let thrown = match (member.f)(&call, id, args) {
        Ok(ret) => return call.value(ret),
        Err(thrown) => thrown,
    };
    // Throwing a `DOMException` runs JS: let go of the document first.
    drop(call);
    // SAFETY: as above.
    unsafe { thrown.throw(ctx) }
}

// -------------------------------------------------------------------------
// The per-context data.

/// What [`call_member`] needs besides the wrapper: how to recognise a node
/// wrapper, and the cached strings. Owned by `DomData`, and found through
/// the context's opaque pointer (see the `native_data` module).
pub(crate) struct Members {
    /// The class id rquickjs gives all its (non-callable) class objects.
    class_id: qjs::JSClassID,
    /// The vtable pointer at the start of a node wrapper's class cell.
    vtable: *const c_void,
    /// Where the `NodeWrapper` is in its class cell.
    offset: usize,
    rt: *mut qjs::JSRuntime,
    atoms: RefCell<Atoms>,
}

/// Strings the members return over and over, as atoms (interned strings
/// that turn into a JS string by taking a reference). Each holds a
/// reference to its atom, given back on drop or when the document, whose
/// name table the indexes refer to, is replaced.
#[derive(Default)]
struct Atoms {
    /// By name id: the name.
    names: Vec<qjs::JSAtom>,
    /// By name id: the name upper-cased.
    upper: Vec<qjs::JSAtom>,
    fixed: Vec<(&'static str, qjs::JSAtom)>,
}

impl Members {
    /// Learns how to recognise node wrappers from `document`'s, and points
    /// the context at the result.
    ///
    /// rquickjs keeps a class object's Rust value in a `ClassCell`, a
    /// `#[repr(C)]` struct of a vtable pointer (one per Rust type) and the
    /// value; the object's opaque pointer points at it. So a node wrapper is
    /// an object of rquickjs's class id whose cell starts with the vtable
    /// pointer `document`'s does.
    pub(crate) fn attach(ctx: &Ctx<'_>, document: &Node<'_>) -> Box<Members> {
        let raw = document.as_raw();
        // SAFETY: `raw` is a live class object, so its opaque is its cell,
        // which starts with the vtable pointer.
        let (class_id, cell) = unsafe {
            let class_id = qjs::JS_GetClassID(raw);
            (class_id, qjs::JS_GetOpaque(raw, class_id))
        };
        assert!(!cell.is_null(), "the document wrapper is a class object");
        // SAFETY: as above.
        let vtable = unsafe { *cell.cast::<*const c_void>() };
        let value: *const NodeWrapper<'_> = &*document.borrow();
        let offset = (value as usize)
            .checked_sub(cell as usize)
            .expect("the wrapper is inside its cell");
        let members = Box::new(Members {
            class_id,
            vtable,
            offset,
            // SAFETY: `ctx` is live.
            rt: unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) },
            atoms: RefCell::default(),
        });
        // The box is kept in `DomData` for as long as the context runs JS,
        // and does not move.
        if let Some(data) = NativeData::of_ctx(ctx) {
            data.set_members(&*members);
        }
        members
    }

    /// The node wrapper `value` is, if it is one.
    ///
    /// # Safety
    ///
    /// `value` must be alive, and stays borrowed for the result's life.
    pub(crate) unsafe fn wrapper<'v>(
        &self,
        ctx: *mut qjs::JSContext,
        value: qjs::JSValue,
    ) -> Option<&'v NodeWrapper<'v>> {
        // SAFETY: `JS_GetOpaque` checks the tag and class; an object of
        // rquickjs's class has a class cell as its opaque.
        unsafe {
            let cell = qjs::JS_GetOpaque(value, self.class_id);
            if cell.is_null() {
                return None;
            }
            if *cell.cast::<*const c_void>() == self.vtable {
                return Some(&*cell.cast::<u8>().add(self.offset).cast::<NodeWrapper<'v>>());
            }
        }
        // Another Rust class, or a node wrapper whose type got a second
        // vtable (generic code can be instantiated twice): ask rquickjs.
        // SAFETY: `ctx` is the running call's, and `value` is alive.
        let ctx = unsafe { ctx_of(ctx) };
        // SAFETY: `value` is alive; the `Value` owns the new reference.
        let value =
            unsafe { Value::from_raw(ctx.clone(), qjs::JS_DupValue(ctx.as_raw().as_ptr(), value)) };
        let node = Class::<NodeWrapper<'_>>::from_value(&value).ok()?;
        let wrapper: *const NodeWrapper<'_> = &*node.borrow();
        // SAFETY: the wrapper lives in its object, which the caller keeps
        // alive.
        Some(unsafe { &*wrapper.cast::<NodeWrapper<'v>>() })
    }

    /// The string for `name`, upper-cased if `upper`, from the cache.
    fn name(
        &self,
        ctx: *mut qjs::JSContext,
        doc: &Document,
        name: Name,
        upper: bool,
    ) -> qjs::JSValue {
        let i = name.to_raw() as usize;
        let mut atoms = self.atoms.borrow_mut();
        let table = if upper {
            &mut atoms.upper
        } else {
            &mut atoms.names
        };
        let atom = match table.get(i) {
            Some(&atom) if atom != qjs::JS_ATOM_NULL => atom,
            _ => {
                let s = doc.name_str(name);
                let s = if upper {
                    Cow::Owned(s.to_ascii_uppercase())
                } else {
                    Cow::Borrowed(s)
                };
                // SAFETY: `ctx` is live, and the pointer and length are a
                // live `str`'s.
                let atom =
                    unsafe { qjs::JS_NewAtomLen(ctx, s.as_ptr().cast(), s.len() as qjs::size_t) };
                if atom == qjs::JS_ATOM_NULL {
                    return qjs::JS_EXCEPTION;
                }
                if table.len() <= i {
                    table.resize(i + 1, qjs::JS_ATOM_NULL);
                }
                table[i] = atom;
                atom
            }
        };
        // SAFETY: the cache holds a reference to `atom`.
        unsafe { qjs::JS_AtomToString(ctx, atom) }
    }

    /// The string `s`, from the cache.
    fn fixed(&self, ctx: *mut qjs::JSContext, s: &'static str) -> qjs::JSValue {
        let mut atoms = self.atoms.borrow_mut();
        let atom = match atoms.fixed.iter().find(|(f, _)| *f == s) {
            Some(&(_, atom)) => atom,
            None => {
                // SAFETY: `ctx` is live, and the pointer and length are `s`'s.
                let atom =
                    unsafe { qjs::JS_NewAtomLen(ctx, s.as_ptr().cast(), s.len() as qjs::size_t) };
                if atom == qjs::JS_ATOM_NULL {
                    return qjs::JS_EXCEPTION;
                }
                atoms.fixed.push((s, atom));
                atom
            }
        };
        // SAFETY: the cache holds a reference to `atom`.
        unsafe { qjs::JS_AtomToString(ctx, atom) }
    }

    /// Forgets the cached names, whose ids only mean something in the
    /// document they came from, when the page's document is replaced.
    pub(crate) fn forget_names(&self) {
        let mut atoms = self.atoms.borrow_mut();
        let names = std::mem::take(&mut atoms.names);
        let upper = std::mem::take(&mut atoms.upper);
        drop(atoms);
        self.free(names.into_iter().chain(upper));
    }

    fn free(&self, atoms: impl Iterator<Item = qjs::JSAtom>) {
        for atom in atoms.filter(|&a| a != qjs::JS_ATOM_NULL) {
            // SAFETY: the cache held a reference to `atom`, in this runtime,
            // which outlives `DomData`.
            unsafe { qjs::JS_FreeAtomRT(self.rt, atom) };
        }
    }
}

impl Drop for Members {
    fn drop(&mut self) {
        let atoms = std::mem::take(self.atoms.get_mut());
        let fixed = atoms.fixed.into_iter().map(|(_, a)| a);
        self.free(atoms.names.into_iter().chain(atoms.upper).chain(fixed));
    }
}

// -------------------------------------------------------------------------
// Installation.

/// Puts every member on the prototypes in `interfaces` (interface name ->
/// prototype), in place of the stand-in `dom.js` defines there: the getter
/// of an accessor (its setter stays), or a method's value. The property
/// keeps its place and attributes. Fails if a stand-in is missing, so
/// `dom.js` and [`MEMBERS`] cannot drift apart.
pub(super) fn install<'js>(ctx: &Ctx<'js>, interfaces: &Object<'js>) -> Result<()> {
    let object: Object<'js> = ctx.globals().get("Object")?;
    let describe: Function<'js> = object.get("getOwnPropertyDescriptor")?;
    let define: Function<'js> = object.get("defineProperty")?;
    for (magic, member) in MEMBERS.iter().enumerate() {
        let f = new_function(ctx, member, magic)?;
        let key = match member.kind {
            Kind::Getter => "get",
            Kind::Method { .. } => "value",
        };
        for &interface in member.on {
            let proto: Object<'js> = interfaces.get(interface)?;
            let desc: Option<Object<'js>> = describe.call((proto.clone(), member.name))?;
            let desc = desc.filter(|d| d.get::<_, Value<'js>>(key).is_ok_and(|v| v.is_function()));
            let Some(desc) = desc else {
                return Err(Exception::throw_internal(
                    ctx,
                    &format!(
                        "{interface}.prototype.{} has no {key} to make native",
                        member.name
                    ),
                ));
            };
            desc.set(key, f.clone())?;
            define.call::<_, Value<'js>>((proto, member.name, desc))?;
        }
    }
    Ok(())
}

/// The C function for `member`: named and sized as `dom.js`'s stand-in is
/// (`get x` with length 0 for a getter).
fn new_function<'js>(ctx: &Ctx<'js>, member: &Member, magic: usize) -> Result<Value<'js>> {
    let (name, length) = match member.kind {
        Kind::Getter => (format!("get {}", member.name), 0),
        Kind::Method { length } => (member.name.to_owned(), c_int::from(length)),
    };
    let name = CString::new(name).expect("member names have no NUL");
    let magic = c_int::try_from(magic).expect("few members");
    type Magic = unsafe extern "C" fn(
        *mut qjs::JSContext,
        qjs::JSValue,
        c_int,
        *mut qjs::JSValue,
        c_int,
    ) -> qjs::JSValue;
    type Plain = unsafe extern "C" fn(
        *mut qjs::JSContext,
        qjs::JSValue,
        c_int,
        *mut qjs::JSValue,
    ) -> qjs::JSValue;
    // SAFETY: QuickJS keeps C functions as a union of pointer types and
    // calls this one as `generic_magic`, with the signature it has.
    let f = unsafe { std::mem::transmute::<Magic, Plain>(call_member) };
    // SAFETY: `ctx` is live and `name` a C string, which QuickJS copies.
    let raw = unsafe {
        qjs::JS_NewCFunction2(
            ctx.as_raw().as_ptr(),
            Some(f),
            name.as_ptr(),
            length,
            qjs::JSCFunctionEnum_JS_CFUNC_generic_magic,
            magic,
        )
    };
    // SAFETY: `raw` is a new value (or the exception marker), owned here.
    if unsafe { qjs::JS_IsException(raw) } {
        return Err(rquickjs::Error::Exception);
    }
    // SAFETY: as above; the `Value` takes the reference.
    Ok(unsafe { Value::from_raw(ctx.clone(), raw) })
}

#[cfg(test)]
mod tests {
    use super::utf16_len;

    #[test]
    fn utf16_lengths() {
        for s in ["", "abc", "é", "日本", "a😀b", "\u{10FFFF}", "\u{FFFF}"] {
            assert_eq!(utf16_len(s), s.encode_utf16().count(), "{s:?}");
        }
    }
}
