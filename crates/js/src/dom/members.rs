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
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::NonNull;
use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId, NodeKind, ShadowRootMode};
use rquickjs::{Class, Ctx, Exception, Function, Object, Result, Value, qjs};

use super::DomState;
use super::attrs::{self, Reflected};
use super::hooks::Hooks;
use super::host::{self, Host, HostClass, Hosts};
use super::layout;
use super::lists::{self, ListClass};
use super::natives::{stale_node_error, throw_dom};
use super::wrapper::{Node, NodeWrapper, wrap};
use super::{edits, queries};

/// What `this` has to be for a member: a node wrapper of a kind that has
/// the member's interface, or one of the host objects of the `host` module.
/// Anything else throws `TypeError: Illegal invocation`, as browsers do.
#[derive(Clone, Copy)]
pub(super) enum Receiver {
    Node,
    Element,
    /// A text or comment node.
    CharacterData,
    /// A `CSSStyleDeclaration` (an inline or a computed style).
    Style,
    /// A `DOMTokenList`.
    Tokens,
    /// A live list of the `lists` module. The call's `node` is the list
    /// object itself (its root's node is what the call sees as the node).
    List(ListClass),
    /// No `this` (a function such as `getComputedStyle`): the node is the
    /// first argument, which must be an element, or the call throws a
    /// `TypeError` with this message.
    ElementArg(&'static CStr),
}

impl Receiver {
    fn accepts(self, doc: &Document, id: NodeId) -> bool {
        match self {
            Receiver::Node => true,
            Receiver::Element | Receiver::Style | Receiver::Tokens | Receiver::ElementArg(_) => {
                doc.is_element(id)
            }
            Receiver::List(_) => true,
            Receiver::CharacterData => doc.text(id).is_some(),
        }
    }

    fn host_class(self) -> Option<HostClass> {
        match self {
            Receiver::Style => Some(HostClass::Style),
            Receiver::Tokens => Some(HostClass::Tokens),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    /// The `get` of an accessor. Its setter, if it has one, stays as
    /// `dom.js` defines it unless a `Setter` entry replaces it.
    Getter,
    /// The `set` of an accessor.
    Setter,
    /// A method taking `length` arguments (its `length` property).
    Method { length: u8 },
    /// A plain function, put on the `functions` object `dom.js` passes in
    /// `interfaces` rather than on a prototype.
    Function { length: u8 },
}

/// A member's body: given the call and the node behind `this`, and the
/// arguments for a method, what to return or throw. It runs with the
/// document borrowed, so it must not change it or run JS.
pub(super) type Impl = for<'c> fn(&'c Call<'_>, NodeId, &[qjs::JSValue]) -> Outcome<'c>;

/// The body of a member that changes the document or may run JS (converting
/// arguments, delivering mutation records): it runs with nothing borrowed,
/// and borrows the document itself as it needs to.
pub(super) type WriteImpl = for<'w> fn(&Write<'w>) -> std::result::Result<Ret<'static>, Thrown>;

pub(super) type Outcome<'c> = std::result::Result<Ret<'c>, Thrown>;

#[derive(Clone, Copy)]
pub(super) enum Body {
    Read(Impl),
    Write(WriteImpl),
}

/// How an argument of a `Read` member is converted before the document is
/// borrowed, since converting an object can run JS. `Write` members convert
/// their own arguments.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "shared with the members the other DOM tickets add"
)]
pub(super) enum Arg {
    Any,
    /// A string (`ToString`): objects and symbols are converted first.
    Str,
    /// An index (`ToUint32`): anything but an integer is converted first.
    Uint32,
}

/// One native member, and the interfaces whose prototypes get it.
struct Member {
    name: &'static str,
    kind: Kind,
    receiver: Receiver,
    /// Keys of the `interfaces` object `dom.js` registers.
    on: &'static [&'static str],
    /// How `Read` bodies' arguments are converted.
    args: &'static [Arg],
    body: Body,
    /// With fewer arguments than this, the call goes to the JS fallback.
    required: u8,
    /// Whether `dom.js`'s function stays as the member's fallback: the
    /// body can hand the call to it with [`Thrown::Fallback`] (see
    /// [`Member::or_js`]).
    fallback: bool,
}

impl Member {
    /// This member, keeping the function `dom.js` defines in its place as
    /// the fallback for what the body does not handle, and for calls with
    /// fewer than `required` arguments (which it rejects with the glue's
    /// own message).
    const fn or_js(self, required: u8) -> Member {
        Member {
            required,
            fallback: true,
            ..self
        }
    }
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
        args: &[],
        body: Body::Read(f),
        required: 0,
        fallback: false,
    }
}

const fn setter(
    name: &'static str,
    receiver: Receiver,
    on: &'static [&'static str],
    f: WriteImpl,
) -> Member {
    Member {
        name,
        kind: Kind::Setter,
        receiver,
        on,
        args: &[],
        body: Body::Write(f),
        required: 0,
        fallback: false,
    }
}

const fn method(
    name: &'static str,
    length: u8,
    receiver: Receiver,
    on: &'static [&'static str],
    f: Impl,
) -> Member {
    reader(name, length, receiver, on, &[], f)
}

/// A method that reads, with its arguments converted as `args` says.
const fn reader(
    name: &'static str,
    length: u8,
    receiver: Receiver,
    on: &'static [&'static str],
    args: &'static [Arg],
    f: Impl,
) -> Member {
    Member {
        name,
        kind: Kind::Method { length },
        receiver,
        on,
        args,
        body: Body::Read(f),
        required: 0,
        fallback: false,
    }
}

/// A method that changes the document or runs JS.
const fn writer(
    name: &'static str,
    length: u8,
    receiver: Receiver,
    on: &'static [&'static str],
    f: WriteImpl,
) -> Member {
    Member {
        name,
        kind: Kind::Method { length },
        receiver,
        on,
        args: &[],
        body: Body::Write(f),
        required: 0,
        fallback: false,
    }
}

const NODE: &[&str] = &["Node"];
const ELEMENT: &[&str] = &["Element"];
const CHARACTER_DATA: &[&str] = &["CharacterData"];
/// The `ParentNode` mixin's interfaces.
const PARENT_NODE: &[&str] = &["Element", "Document", "DocumentFragment"];
/// The `NonDocumentTypeChildNode` mixin's interfaces.
const CHILD_NODE: &[&str] = &["Element", "CharacterData"];
const HTML_ELEMENT: &[&str] = &["HTMLElement"];
/// The `ChildNode` mixin's interfaces.
const ANY_CHILD_NODE: &[&str] = &["Element", "CharacterData", "DocumentType"];
const DOCUMENT: &[&str] = &["Document"];
const NODE_LIST: Receiver = Receiver::List(ListClass::NodeList);
const COLLECTION: Receiver = Receiver::List(ListClass::HtmlCollection);
const ATTRIBUTES: Receiver = Receiver::List(ListClass::NamedNodeMap);
/// The interfaces with `getElementById`.
const BY_ID: &[&str] = &["Document", "DocumentFragment"];
/// The `ElementCSSInlineStyle` and `HTMLOrSVGElement` mixins' interfaces.
const STYLED: &[&str] = &["HTMLElement", "SVGElement", "MathMLElement"];
const STYLE: &[&str] = &["CSSStyleDeclaration"];
const TOKENS: &[&str] = &["DOMTokenList"];
const FUNCTIONS: &[&str] = &["functions"];
const STR: &[Arg] = &[Arg::Str];
const INDEX: &[Arg] = &[Arg::Uint32];

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
    // Inline style, classList and dataset (see the `host` module).
    getter("style", Receiver::Element, STYLED, host::style),
    getter("dataset", Receiver::Element, STYLED, host::dataset),
    getter("classList", Receiver::Element, ELEMENT, host::class_list),
    Member {
        name: "getComputedStyle",
        kind: Kind::Function { length: 1 },
        receiver: Receiver::ElementArg(
            c"Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.",
        ),
        on: FUNCTIONS,
        args: &[],
        body: Body::Read(host::computed_style),
        required: 0,
        fallback: false,
    },
    // CSSStyleDeclaration.
    getter("cssText", Receiver::Style, STYLE, host::css_text),
    setter("cssText", Receiver::Style, STYLE, host::set_css_text),
    getter("length", Receiver::Style, STYLE, host::style_length),
    reader("item", 1, Receiver::Style, STYLE, INDEX, host::style_item),
    reader("getPropertyValue", 1, Receiver::Style, STYLE, STR, host::get_property_value),
    reader("getPropertyPriority", 1, Receiver::Style, STYLE, STR, host::get_property_priority),
    writer("setProperty", 2, Receiver::Style, STYLE, host::set_property),
    writer("removeProperty", 1, Receiver::Style, STYLE, host::remove_property),
    getter("cssFloat", Receiver::Style, STYLE, host::css_float),
    setter("cssFloat", Receiver::Style, STYLE, host::set_css_float),
    // DOMTokenList.
    getter("length", Receiver::Tokens, TOKENS, host::tokens_length),
    reader("item", 1, Receiver::Tokens, TOKENS, INDEX, host::tokens_item),
    reader("contains", 1, Receiver::Tokens, TOKENS, STR, host::tokens_contains),
    writer("add", 0, Receiver::Tokens, TOKENS, host::tokens_add),
    writer("remove", 0, Receiver::Tokens, TOKENS, host::tokens_remove),
    writer("toggle", 1, Receiver::Tokens, TOKENS, host::tokens_toggle),
    writer("replace", 2, Receiver::Tokens, TOKENS, host::tokens_replace),
    reader("supports", 1, Receiver::Tokens, TOKENS, STR, host::tokens_supports),
    getter("value", Receiver::Tokens, TOKENS, host::tokens_value),
    setter("value", Receiver::Tokens, TOKENS, host::set_tokens_value),
    method("toString", 0, Receiver::Tokens, TOKENS, host::tokens_value),
    // The synthetic layout (see the `layout` module).
    getter("clientWidth", Receiver::Element, ELEMENT, layout::client_width),
    getter("clientHeight", Receiver::Element, ELEMENT, layout::client_height),
    getter("scrollWidth", Receiver::Element, ELEMENT, layout::box_width),
    getter("scrollHeight", Receiver::Element, ELEMENT, layout::box_height),
    getter("offsetTop", Receiver::Element, HTML_ELEMENT, layout::box_top),
    getter("offsetLeft", Receiver::Element, HTML_ELEMENT, layout::box_left),
    getter("offsetWidth", Receiver::Element, HTML_ELEMENT, layout::box_width),
    getter("offsetHeight", Receiver::Element, HTML_ELEMENT, layout::box_height),
    // Attributes (see the `attrs` module).
    reader("getAttribute", 1, Receiver::Element, ELEMENT, STR, attrs::get_attribute).or_js(1),
    reader("hasAttribute", 1, Receiver::Element, ELEMENT, STR, attrs::has_attribute).or_js(1),
    method("hasAttributes", 0, Receiver::Element, ELEMENT, attrs::has_attributes),
    method("getAttributeNames", 0, Receiver::Element, ELEMENT, attrs::get_attribute_names),
    writer("getAttributeNS", 2, Receiver::Element, ELEMENT, attrs::get_attribute_ns),
    writer("hasAttributeNS", 2, Receiver::Element, ELEMENT, attrs::has_attribute_ns),
    writer("setAttribute", 2, Receiver::Element, ELEMENT, attrs::set_attribute).or_js(2),
    writer("setAttributeNS", 3, Receiver::Element, ELEMENT, attrs::set_attribute_ns).or_js(3),
    writer("removeAttribute", 1, Receiver::Element, ELEMENT, attrs::remove_attribute).or_js(1),
    writer("removeAttributeNS", 2, Receiver::Element, ELEMENT, attrs::remove_attribute_ns).or_js(0),
    writer("toggleAttribute", 1, Receiver::Element, ELEMENT, attrs::toggle_attribute).or_js(1),
    setter("id", Receiver::Element, ELEMENT, attrs::set_id),
    setter("className", Receiver::Element, ELEMENT, attrs::set_class_name),
    // Tree edits (see the `edits` module).
    writer("appendChild", 1, Receiver::Node, NODE, edits::append_child).or_js(1),
    writer("insertBefore", 2, Receiver::Node, NODE, edits::insert_before).or_js(2),
    writer("removeChild", 1, Receiver::Node, NODE, edits::remove_child).or_js(1),
    writer("replaceChild", 2, Receiver::Node, NODE, edits::replace_child_member).or_js(2),
    writer("cloneNode", 0, Receiver::Node, NODE, edits::clone_node).or_js(0),
    setter("textContent", Receiver::Node, NODE, edits::set_text_content_member),
    setter("innerHTML", Receiver::Element, ELEMENT, edits::set_inner_html_member),
    writer("prepend", 0, Receiver::Node, PARENT_NODE, edits::prepend).or_js(0),
    writer("append", 0, Receiver::Node, PARENT_NODE, edits::append).or_js(0),
    writer("replaceChildren", 0, Receiver::Node, PARENT_NODE, edits::replace_children).or_js(0),
    writer("before", 0, Receiver::Node, ANY_CHILD_NODE, edits::before).or_js(0),
    writer("after", 0, Receiver::Node, ANY_CHILD_NODE, edits::after).or_js(0),
    writer("replaceWith", 0, Receiver::Node, ANY_CHILD_NODE, edits::replace_with).or_js(0),
    writer("remove", 0, Receiver::Node, ANY_CHILD_NODE, edits::remove),
    writer("createElement", 1, Receiver::Node, DOCUMENT, edits::create_element).or_js(1),
    writer("createTextNode", 1, Receiver::Node, DOCUMENT, edits::create_text_node).or_js(1),
    writer("createComment", 1, Receiver::Node, DOCUMENT, edits::create_comment).or_js(1),
    writer("createDocumentFragment", 0, Receiver::Node, DOCUMENT, edits::create_document_fragment)
        .or_js(0),
    // Queries (see the `queries` module).
    reader("getElementById", 1, Receiver::Node, BY_ID, STR, queries::get_element_by_id),
    reader("querySelector", 1, Receiver::Node, PARENT_NODE, STR, queries::query_selector).or_js(1),
    reader("querySelectorAll", 1, Receiver::Node, PARENT_NODE, STR, queries::query_selector_all)
        .or_js(1),
    reader("matches", 1, Receiver::Element, ELEMENT, STR, queries::matches).or_js(1),
    reader("webkitMatchesSelector", 1, Receiver::Element, ELEMENT, STR, queries::matches),
    reader("closest", 1, Receiver::Element, ELEMENT, STR, queries::closest).or_js(1),
    // Live lists (see the `lists` module).
    getter("childNodes", Receiver::Node, NODE, lists::child_nodes),
    getter("children", Receiver::Node, PARENT_NODE, lists::children),
    getter("attributes", Receiver::Element, ELEMENT, lists::attributes),
    getter("length", NODE_LIST, &["NodeList"], lists::node_list_length),
    getter("length", COLLECTION, &["HTMLCollection"], lists::collection_length),
    reader("item", 1, COLLECTION, &["HTMLCollection"], INDEX, lists::collection_item),
    reader("namedItem", 1, COLLECTION, &["HTMLCollection"], STR, lists::named_item_member),
    getter("length", ATTRIBUTES, &["NamedNodeMap"], lists::attributes_length),
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
#[allow(
    dead_code,
    reason = "shared with the members the other DOM tickets add"
)]
pub(super) enum Ret<'c> {
    Null,
    Undefined,
    Bool(bool),
    Int(i32),
    Float(f64),
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
    /// A value made for this call, whose reference goes to the caller.
    Value(qjs::JSValue),
}

/// What a member throws.
pub(super) enum Thrown {
    /// `TypeError: Illegal invocation`, for the wrong `this`.
    Illegal,
    /// A `TypeError` with this message.
    Type(&'static CStr),
    /// The `InvalidStateError` for a node of a replaced document.
    Stale,
    /// A `DOMException` with this name and message.
    Dom(&'static str, String),
    /// An rquickjs error (an exception may be pending).
    Error(rquickjs::Error),
    /// An exception is already pending.
    Pending,
    /// Not for the native body: call the JS fallback instead (members made
    /// with [`Member::or_js`] only). A body returns it before it has done
    /// anything scripts could notice.
    Fallback,
}

/// One call of a `Read` member: what it can look at.
pub(super) struct Call<'a> {
    pub(super) ctx: *mut qjs::JSContext,
    pub(super) members: &'a Members,
    pub(super) dom: &'a Rc<DomState>,
    pub(super) doc: Ref<'a, Document>,
    /// The node's wrapper, and its JS value (`this`, the first argument
    /// of a function, or the owner of a host object).
    pub(super) wrapper: &'a NodeWrapper<'a>,
    pub(super) node: qjs::JSValue,
    /// The host object behind `this`, for host receivers.
    pub(super) host: Option<&'a Host>,
    pub(super) args: &'a [qjs::JSValue],
}

/// One call of a `Write` member: the same, without the document borrowed.
#[allow(
    dead_code,
    reason = "shared with the members the other DOM tickets add"
)]
pub(super) struct Write<'a> {
    pub(super) ctx: *mut qjs::JSContext,
    pub(super) members: &'a Members,
    pub(super) dom: &'a Rc<DomState>,
    pub(super) id: NodeId,
    pub(super) wrapper: &'a NodeWrapper<'a>,
    pub(super) node: qjs::JSValue,
    pub(super) host: Option<&'a Host>,
    pub(super) args: &'a [qjs::JSValue],
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

    /// Argument `i` as a string. `Read` members get it from an argument
    /// converted before the call (see [`Arg`]), so this runs no JS. A
    /// missing argument is `"undefined"`, as `String(undefined)`.
    pub(super) fn str_arg(&self, i: usize) -> std::result::Result<JsStr, Thrown> {
        // SAFETY: `self.ctx` is the running call's, the arguments alive.
        unsafe { JsStr::arg(self.ctx, self.args, i) }
    }

    /// Argument `i` as `ToUint32` (converted before the call).
    pub(super) fn u32_arg(&self, i: usize) -> u32 {
        match self.args.get(i) {
            // SAFETY: a tag check and the integer of an int value.
            Some(&v) if unsafe { qjs::JS_VALUE_GET_TAG(v) } == qjs::JS_TAG_INT => {
                // SAFETY: as above.
                (unsafe { qjs::JS_VALUE_GET_INT(v) }) as u32
            }
            _ => 0,
        }
    }

    /// `ret` as a new JS value, or `JS_EXCEPTION` with an exception thrown.
    pub(super) fn value(&self, ret: Ret<'_>) -> qjs::JSValue {
        value_of(self.ctx, self.members, self.dom, &self.doc, ret)
    }
}

impl<'a> Write<'a> {
    /// Argument `i` as a string (`ToString`, which may run JS). A missing
    /// argument is `"undefined"`.
    pub(super) fn str_arg(&self, i: usize) -> std::result::Result<JsStr, Thrown> {
        // SAFETY: `self.ctx` is the running call's, the arguments alive.
        unsafe { JsStr::arg(self.ctx, self.args, i) }
    }

    /// Argument `i`, or `undefined`.
    pub(super) fn arg(&self, i: usize) -> qjs::JSValue {
        self.args.get(i).copied().unwrap_or(qjs::JS_UNDEFINED)
    }

    /// An rquickjs context, for calling the natives in `natives`.
    pub(super) fn rctx(&self) -> Ctx<'a> {
        // SAFETY: `self.ctx` is the context of the running call.
        unsafe { ctx_of(self.ctx) }
    }
}

/// `ret` as a new JS value, or `JS_EXCEPTION` with an exception thrown.
fn value_of(
    ctx: *mut qjs::JSContext,
    members: &Members,
    dom: &Rc<DomState>,
    doc: &Document,
    ret: Ret<'_>,
) -> qjs::JSValue {
    match ret {
        Ret::Null | Ret::Node(None) => qjs::JS_NULL,
        Ret::Undefined => qjs::JS_UNDEFINED,
        Ret::Bool(true) => qjs::JS_TRUE,
        Ret::Bool(false) => qjs::JS_FALSE,
        Ret::Int(i) => qjs::JS_MKVAL(qjs::JS_TAG_INT, i),
        Ret::Float(f) => qjs::JS_NewFloat64(f),
        Ret::Node(Some(id)) => wrapper_value(ctx, dom, id),
        Ret::Name(name) => members.name(ctx, doc, name, false),
        Ret::UpperName(name) => members.name(ctx, doc, name, true),
        Ret::Static(s) => members.fixed(ctx, s),
        Ret::Str(s) => new_string(ctx, &s),
        Ret::Value(v) => v,
    }
}

/// A new JS string.
pub(super) fn new_string(ctx: *mut qjs::JSContext, s: &str) -> qjs::JSValue {
    // SAFETY: the pointer and length are those of a live `str`.
    unsafe { qjs::JS_NewStringLen(ctx, s.as_ptr().cast(), s.len() as qjs::size_t) }
}

/// The wrapper of `id`, made if there is none.
fn wrapper_value(ctx: *mut qjs::JSContext, dom: &Rc<DomState>, id: NodeId) -> qjs::JSValue {
    if let Some(raw) = dom.wrappers.get(id) {
        // SAFETY: the cache only holds live wrappers, and nothing has
        // run since it was read. The duplicate is the caller's.
        return unsafe { qjs::JS_DupValue(ctx, raw) };
    }
    // SAFETY: `ctx` is the context of the running call.
    let ctx = unsafe { ctx_of(ctx) };
    // A new wrapper's prototype comes from the document, which `wrap`
    // borrows again (shared, as here); finalizers the GC runs while it
    // allocates never touch the document.
    match wrap(&ctx, dom, id) {
        Ok(node) => into_raw(&ctx, node.into_value()),
        Err(e) => throw_error(&ctx, e),
    }
}

/// A string argument, as UTF-8 from QuickJS (`JS_ToCStringLen2`), freed on
/// drop.
pub(super) enum JsStr {
    Js {
        ctx: *mut qjs::JSContext,
        ptr: *const c_char,
        len: usize,
    },
    Static(&'static str),
}

impl JsStr {
    /// Argument `i` converted to a string, which runs JS for an object.
    ///
    /// # Safety
    ///
    /// `ctx` must be the running call's, and `args` alive.
    unsafe fn arg(
        ctx: *mut qjs::JSContext,
        args: &[qjs::JSValue],
        i: usize,
    ) -> std::result::Result<JsStr, Thrown> {
        match args.get(i) {
            // SAFETY: as for the function.
            Some(&v) => unsafe { JsStr::new(ctx, v) },
            None => Ok(JsStr::Static("undefined")),
        }
    }

    /// `value` converted to a string, which runs JS for an object.
    ///
    /// # Safety
    ///
    /// `ctx` must be the running call's, and `value` alive.
    pub(super) unsafe fn new(
        ctx: *mut qjs::JSContext,
        value: qjs::JSValue,
    ) -> std::result::Result<JsStr, Thrown> {
        let mut len: qjs::size_t = 0;
        // SAFETY: as for the function.
        let ptr = unsafe { qjs::JS_ToCStringLen2(ctx, &mut len, value, false) };
        if ptr.is_null() {
            return Err(Thrown::Pending);
        }
        Ok(JsStr::Js {
            ctx,
            ptr,
            len: len as usize,
        })
    }

    /// The text, with lone surrogates (which QuickJS writes as WTF-8)
    /// replaced by U+FFFD, as `internals.usv` does.
    pub(super) fn as_str(&self) -> Cow<'_, str> {
        let bytes = match *self {
            JsStr::Static(s) => return Cow::Borrowed(s),
            // SAFETY: QuickJS gave `len` bytes at `ptr`, alive until freed.
            JsStr::Js { ptr, len, .. } => unsafe {
                std::slice::from_raw_parts(ptr.cast::<u8>(), len)
            },
        };
        match std::str::from_utf8(bytes) {
            Ok(s) => Cow::Borrowed(s),
            Err(_) => Cow::Owned(well_formed(bytes)),
        }
    }
}

impl Drop for JsStr {
    fn drop(&mut self) {
        if let JsStr::Js { ctx, ptr, .. } = *self {
            // SAFETY: `ptr` came from `JS_ToCStringLen2` in `ctx`.
            unsafe { qjs::JS_FreeCString(ctx, ptr) };
        }
    }
}

/// WTF-8 (UTF-8 that may encode lone surrogates) as UTF-8, each surrogate
/// replaced by U+FFFD.
fn well_formed(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                // SAFETY: `from_utf8` checked the first `good` bytes.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&rest[..good]) });
                out.push('\u{FFFD}');
                // A surrogate is three bytes (ED A0..BF xx); anything else
                // QuickJS never writes, so skip what `from_utf8` rejects.
                let skip = if rest[good] == 0xED && rest.len() >= good + 3 {
                    3
                } else {
                    e.error_len().unwrap_or(rest.len() - good)
                };
                rest = &rest[good + skip..];
            }
        }
    }
    out
}

/// The arguments of a `Read` member after converting what [`Arg`] asks
/// for; the converted values are owned here and freed on drop.
struct Converted {
    ctx: *mut qjs::JSContext,
    values: Vec<qjs::JSValue>,
    owned: Vec<usize>,
}

impl Drop for Converted {
    fn drop(&mut self) {
        for &i in &self.owned {
            // SAFETY: the value at `i` was made for this call.
            unsafe { qjs::JS_FreeValue(self.ctx, self.values[i]) };
        }
    }
}

/// Converts the arguments `kinds` asks for, if any needs it. Can run JS.
///
/// # Safety
///
/// `ctx` must be the running call's, and `args` alive.
unsafe fn convert_args(
    ctx: *mut qjs::JSContext,
    kinds: &[Arg],
    args: &[qjs::JSValue],
) -> std::result::Result<Option<Converted>, ()> {
    let mut out: Option<Converted> = None;
    for (i, (&kind, &v)) in kinds.iter().zip(args).enumerate() {
        // SAFETY: a tag check.
        let tag = unsafe { qjs::JS_VALUE_GET_TAG(v) };
        let new = match kind {
            Arg::Any => continue,
            Arg::Str if tag == qjs::JS_TAG_OBJECT || tag == qjs::JS_TAG_SYMBOL => {
                // SAFETY: as for the function.
                unsafe { qjs::JS_ToString(ctx, v) }
            }
            Arg::Uint32 if tag != qjs::JS_TAG_INT => {
                let mut n = 0i32;
                // SAFETY: as for the function.
                if unsafe { qjs::JS_ToInt32(ctx, &mut n, v) } < 0 {
                    qjs::JS_EXCEPTION
                } else {
                    qjs::JS_MKVAL(qjs::JS_TAG_INT, n)
                }
            }
            _ => continue,
        };
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(new) } {
            return Err(());
        }
        let c = out.get_or_insert_with(|| Converted {
            ctx,
            values: args.to_vec(),
            owned: Vec::new(),
        });
        c.values[i] = new;
        c.owned.push(i);
    }
    Ok(out)
}

impl Thrown {
    /// Throws this in `ctx`, returning `JS_EXCEPTION`.
    ///
    /// # Safety
    ///
    /// `ctx` must be the context of the running call.
    pub(super) unsafe fn throw(self, ctx: *mut qjs::JSContext) -> qjs::JSValue {
        match self {
            // SAFETY: as for the function.
            Thrown::Illegal => unsafe { throw_type(ctx, c"Illegal invocation") },
            // SAFETY: as for the function.
            Thrown::Type(message) => unsafe { throw_type(ctx, message) },
            Thrown::Pending => qjs::JS_EXCEPTION,
            // `call_member` handles it for members that have a fallback.
            // SAFETY: as for the function.
            Thrown::Fallback => unsafe { throw_type(ctx, c"Illegal invocation") },
            Thrown::Error(e) => {
                // SAFETY: as for the function.
                let ctx = unsafe { ctx_of(ctx) };
                throw_error(&ctx, e)
            }
            Thrown::Stale => {
                // SAFETY: as for the function.
                let ctx = unsafe { ctx_of(ctx) };
                throw_error(&ctx, stale_node_error(&ctx))
            }
            Thrown::Dom(name, message) => {
                // SAFETY: as for the function.
                let ctx = unsafe { ctx_of(ctx) };
                throw_error(&ctx, throw_dom(&ctx, name, &message))
            }
        }
    }
}

impl From<rquickjs::Error> for Thrown {
    fn from(e: rquickjs::Error) -> Self {
        Thrown::Error(e)
    }
}

/// Throws a `TypeError`, returning `JS_EXCEPTION`.
///
/// # Safety
///
/// `ctx` must be a live context.
pub(super) unsafe fn throw_type(ctx: *mut qjs::JSContext, message: &CStr) -> qjs::JSValue {
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
pub(super) unsafe fn ctx_of<'js>(ctx: *mut qjs::JSContext) -> Ctx<'js> {
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
    // SAFETY: the context's opaque is null or the `Members` that `DomData`
    // owns, which lives as long as JS can run.
    let members = unsafe { qjs::JS_GetContextOpaque(ctx).cast::<Members>().as_ref() };
    let Some(members) = members else {
        // SAFETY: QuickJS passes the live context of the call.
        return unsafe { throw_type(ctx, c"Illegal invocation") };
    };
    let args = match usize::try_from(argc) {
        // SAFETY: QuickJS passes `argc` live values at `argv`.
        Ok(n) if n > 0 && !argv.is_null() => unsafe { std::slice::from_raw_parts(argv, n) },
        _ => &[],
    };
    // Find the node: `this`, the owner of the host object `this` is, or
    // the first argument.
    let mut host = None;
    let (wrapper, node) = match member.receiver {
        Receiver::ElementArg(message) => {
            let arg = args.first().copied().unwrap_or(qjs::JS_UNDEFINED);
            // SAFETY: the argument is alive for the call.
            match unsafe { members.wrapper(ctx, arg) } {
                Some(w) => (w, arg),
                // SAFETY: as above.
                None => return unsafe { throw_type(ctx, message) },
            }
        }
        Receiver::List(class) => {
            // SAFETY: `this` is alive for the call.
            let Some(list) = (unsafe { members.lists().get(this, class) }) else {
                // SAFETY: as above.
                return unsafe { Thrown::Illegal.throw(ctx) };
            };
            (list.wrapper(), this)
        }
        receiver => match receiver.host_class() {
            Some(class) => {
                // SAFETY: `this` is alive for the call.
                let Some(h) = (unsafe { members.hosts.get(this, class) }) else {
                    // SAFETY: as above.
                    return unsafe { Thrown::Illegal.throw(ctx) };
                };
                host = Some(h);
                (h.wrapper(), h.owner())
            }
            // SAFETY: `this` is alive for the call.
            None => match unsafe { members.wrapper(ctx, this) } {
                Some(w) => (w, this),
                // SAFETY: as above.
                None => return unsafe { Thrown::Illegal.throw(ctx) },
            },
        },
    };
    let Some(id) = wrapper.id() else {
        // SAFETY: as above.
        return unsafe { Thrown::Stale.throw(ctx) };
    };
    let dom = wrapper.dom();
    if member.fallback && args.len() < usize::from(member.required) {
        // SAFETY: as above.
        return unsafe { members.call_fallback(ctx, magic, this, argc, argv) };
    }
    let rejected = |ctx| match member.receiver {
        // SAFETY: as above.
        Receiver::ElementArg(message) => unsafe { throw_type(ctx, message) },
        // SAFETY: as above.
        _ => unsafe { Thrown::Illegal.throw(ctx) },
    };
    let thrown = match member.body {
        Body::Read(f) => {
            // SAFETY: as above; nothing is borrowed yet.
            let converted = match unsafe { convert_args(ctx, member.args, args) } {
                Ok(c) => c,
                Err(()) => return qjs::JS_EXCEPTION,
            };
            let args = converted.as_ref().map_or(args, |c| &c.values[..]);
            let call = Call {
                ctx,
                members,
                dom,
                doc: dom.doc(),
                wrapper,
                node,
                host,
                args,
            };
            if !member.receiver.accepts(&call.doc, id) {
                drop(call);
                return rejected(ctx);
            }
            match f(&call, id, args) {
                Ok(ret) => return call.value(ret),
                Err(thrown) => thrown,
            }
            // `call`, then `converted`, are dropped here, before throwing.
        }
        Body::Write(f) => {
            if !member.receiver.accepts(&dom.doc(), id) {
                return rejected(ctx);
            }
            let write = Write {
                ctx,
                members,
                dom,
                id,
                wrapper,
                node,
                host,
                args,
            };
            match f(&write) {
                // Only names need the document.
                Ok(Ret::Value(v)) => return v,
                Ok(Ret::Undefined) => return qjs::JS_UNDEFINED,
                Ok(ret) => return value_of(ctx, members, dom, &dom.doc(), ret),
                Err(thrown) => thrown,
            }
        }
    };
    if let (Thrown::Fallback, true) = (&thrown, member.fallback) {
        // SAFETY: as above; nothing is borrowed.
        return unsafe { members.call_fallback(ctx, magic, this, argc, argv) };
    }
    // Throwing a `DOMException` runs JS: the document is let go by now.
    // SAFETY: as above.
    unsafe { thrown.throw(ctx) }
}

// -------------------------------------------------------------------------
// The per-context data.

/// What [`call_member`] needs besides the wrapper: how to recognise a node
/// wrapper, the host object classes, and the cached strings. Owned by
/// `DomData`, and found through the context's opaque pointer, which nothing
/// else in dasher or rquickjs uses.
pub(crate) struct Members {
    /// The class id rquickjs gives all its (non-callable) class objects.
    class_id: qjs::JSClassID,
    /// The vtable pointer at the start of a node wrapper's class cell.
    vtable: *const c_void,
    /// Where the `NodeWrapper` is in its class cell.
    offset: usize,
    rt: *mut qjs::JSRuntime,
    atoms: RefCell<Atoms>,
    /// The classes of style, token list and dataset objects.
    pub(super) hosts: Hosts,
    /// The fallbacks, hooks and tables kept from `dom.js`.
    hooks: Hooks,
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
    pub(crate) fn attach(
        ctx: &Ctx<'_>,
        document: &Node<'_>,
        hosts: Hosts,
        hooks: Hooks,
    ) -> Box<Members> {
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
            hosts,
            hooks,
        });
        let ptr: *const Members = &*members;
        // SAFETY: the box is kept in `DomData` for as long as the context
        // runs JS, and does not move.
        unsafe { qjs::JS_SetContextOpaque(ctx.as_raw().as_ptr(), ptr.cast_mut().cast()) };
        members
    }

    /// Calls member `magic`'s JS fallback (see [`Member::or_js`]).
    ///
    /// # Safety
    ///
    /// The arguments must be those QuickJS passed to the member.
    unsafe fn call_fallback(
        &self,
        ctx: *mut qjs::JSContext,
        magic: c_int,
        this: qjs::JSValue,
        argc: c_int,
        argv: *mut qjs::JSValue,
    ) -> qjs::JSValue {
        // SAFETY: as for the function.
        unsafe { self.hooks.call_fallback(ctx, magic, this, argc, argv) }
    }

    /// Hands the scripts an insertion connected to the glue.
    pub(super) fn connected(
        &self,
        ctx: &Ctx<'_>,
        dom: &Rc<DomState>,
        scripts: &[NodeId],
    ) -> Result<()> {
        self.hooks.connected(ctx, dom, scripts)
    }

    /// Makes `array` a static `NodeList`; false with an exception thrown
    /// if that fails.
    pub(super) fn static_node_list(&self, ctx: *mut qjs::JSContext, array: qjs::JSValue) -> bool {
        self.hooks.static_node_list(ctx, array)
    }

    pub(super) fn add_reflected(&self, reflected: Reflected) -> usize {
        self.hooks.add_reflected(reflected)
    }

    pub(super) fn reflected(&self, index: usize) -> Option<Rc<Reflected>> {
        self.hooks.reflected(index)
    }

    /// `value` resolved against the base URL, as a new JS string. Runs JS.
    pub(super) fn resolve_url(&self, ctx: *mut qjs::JSContext, value: &str) -> qjs::JSValue {
        self.hooks.resolve_url(ctx, value)
    }

    pub(super) fn lists(&self) -> &lists::Lists {
        self.hooks.lists()
    }

    pub(super) fn proto(&self, proto: lists::Proto) -> qjs::JSValue {
        self.hooks.proto(proto)
    }

    pub(super) fn is_list_member(&self, atom: qjs::JSAtom) -> bool {
        self.hooks.is_list_member(atom)
    }

    /// The glue's `Attr` for `key` of `owner`'s attributes, or `null`.
    ///
    /// # Safety
    ///
    /// `ctx` must be live and `owner` alive.
    pub(super) unsafe fn attr(
        &self,
        ctx: *mut qjs::JSContext,
        owner: qjs::JSValue,
        key: lists::AttrKey,
    ) -> qjs::JSValue {
        // SAFETY: as for the function.
        unsafe { self.hooks.attr(ctx, owner, key) }
    }

    /// The node wrapper `value` is, if it is one.
    ///
    /// # Safety
    ///
    /// `value` must be alive, and stays borrowed for the result's life.
    pub(super) unsafe fn wrapper<'v>(
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
    pub(super) fn name(
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
    pub(super) fn fixed(&self, ctx: *mut qjs::JSContext, s: &'static str) -> qjs::JSValue {
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
/// or setter of an accessor (the other stays), or a method's value; a
/// `Function` is put on the `functions` object instead. The property
/// keeps its place and attributes. Fails if a stand-in is missing, so
/// `dom.js` and [`MEMBERS`] cannot drift apart.
///
/// Returns, by member, the function a member made with [`Member::or_js`]
/// replaced (the one on its first interface), to keep as its fallback.
pub(super) fn install<'js>(
    ctx: &Ctx<'js>,
    interfaces: &Object<'js>,
) -> Result<Vec<Option<Value<'js>>>> {
    let object: Object<'js> = ctx.globals().get("Object")?;
    let describe: Function<'js> = object.get("getOwnPropertyDescriptor")?;
    let define: Function<'js> = object.get("defineProperty")?;
    let mut fallbacks = Vec::with_capacity(MEMBERS.len());
    for (magic, member) in MEMBERS.iter().enumerate() {
        fallbacks.push(None);
        let f = new_function(ctx, member, magic)?;
        let key = match member.kind {
            Kind::Getter => "get",
            Kind::Setter => "set",
            Kind::Method { .. } => "value",
            Kind::Function { .. } => {
                for &interface in member.on {
                    let target: Object<'js> = interfaces.get(interface)?;
                    target.set(member.name, f.clone())?;
                }
                continue;
            }
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
            if member.fallback && fallbacks[magic].is_none() {
                fallbacks[magic] = Some(desc.get::<_, Value<'js>>(key)?);
            }
            desc.set(key, f.clone())?;
            define.call::<_, Value<'js>>((proto, member.name, desc))?;
        }
    }
    Ok(fallbacks)
}

/// The C function for `member`: named and sized as `dom.js`'s stand-in is
/// (`get x` with length 0 for a getter).
fn new_function<'js>(ctx: &Ctx<'js>, member: &Member, magic: usize) -> Result<Value<'js>> {
    let (name, length) = match member.kind {
        Kind::Getter => (format!("get {}", member.name), 0),
        Kind::Setter => (format!("set {}", member.name), 1),
        Kind::Method { length } | Kind::Function { length } => {
            (member.name.to_owned(), c_int::from(length))
        }
    };
    let name = CString::new(name).expect("member names have no NUL");
    let magic = c_int::try_from(magic).expect("few members");
    new_magic_function(ctx, call_member, &name, length, magic)
}

/// The C function signature of members, with its magic number.
pub(super) type Magic = unsafe extern "C" fn(
    *mut qjs::JSContext,
    qjs::JSValue,
    c_int,
    *mut qjs::JSValue,
    c_int,
) -> qjs::JSValue;

/// A native function `name` of `length` that QuickJS calls as `f` with
/// `magic`.
pub(super) fn new_magic_function<'js>(
    ctx: &Ctx<'js>,
    f: Magic,
    name: &CStr,
    length: c_int,
    magic: c_int,
) -> Result<Value<'js>> {
    type MagicFn = unsafe extern "C" fn(
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
    let f = unsafe { std::mem::transmute::<MagicFn, Plain>(f) };
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
