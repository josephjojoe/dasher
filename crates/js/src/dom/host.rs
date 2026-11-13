//! Host objects: the `CSSStyleDeclaration`s of `el.style` and
//! `getComputedStyle(el)`, the `DOMTokenList`s of `el.classList` and
//! friends, and `el.dataset`. Each is a raw QuickJS object of a class
//! registered here, whose opaque is a [`Host`]: the element it belongs to
//! and what it keeps parsed. Their interface members are native members
//! (`members` module) with the `Style` or `Tokens` receiver; what plain
//! property access does on them (`style.marginTop`, `classList[0]`,
//! `dataset.userId`) is answered by the classes' exotic methods, so none of
//! it runs JS or goes through a `Proxy`.
//!
//! # What each object keeps
//!
//! An inline style keeps the declarations of the `style` attribute parsed
//! (`style::Declarations`) and a token list its tokens, together with the
//! attribute text they came from. Every read compares the attribute with
//! that text and parses again only if it changed, so the parsed form is
//! never stale whatever changed the attribute (`setAttribute`, `className`,
//! the parser, cloning: a clone is a new element with new objects), and a
//! write serializes once, sets the attribute once (through the same path
//! as `setAttribute`, so `MutationObserver`s see it) and records the new
//! text with the declarations it came from.
//!
//! A computed style keeps nothing but its own copy of the inline
//! declarations: each property is resolved when it is read (the inline
//! value if set, else the default for the element; see
//! `style::COMPUTED`), so the object is live, as in browsers.
//!
//! # Identity and lifetime
//!
//! `el.style`, `el.classList`, `el.dataset` and `getComputedStyle(el)` are
//! made on first use and kept in the element wrapper's extras
//! (`wrapper::Extra`), which the wrapper traces; each object holds a
//! reference to its element's wrapper, which its class's `gc_mark` reports.
//! So `el.style === el.style`, expandos stick, and the pair is collected
//! together. Other token lists (`relList`, `part`...) are made by
//! `N.tokenList` and cached by `dom.js`.
//!
//! # Property access
//!
//! A style answers, as an own property, each index (the property names in
//! order) and any key that looks like a CSS property (`is_property_key`)
//! and is not found on its prototype chain: `style.marginTop` reads
//! `getPropertyValue('margin-top')`, and assigning it calls `setProperty`.
//! Anything else is an ordinary property. A token list answers its indexes;
//! `dataset` answers every string key with its `data-*` attribute.

use std::cell::RefCell;
use std::ffi::{CStr, c_int};

use dasher_dom::{Document, Name, NodeId};
use rquickjs::{Ctx, Object, Result, Value, qjs};

use super::DomState;
use super::members::{Call, JsStr, Members, Outcome, Ret, Thrown, Write, new_string};
use super::natives::{remove_attr, set_attr, valid_attribute_name};
use super::style::{
    self, COMPUTED, Decl, Declarations, dashed, default_display, is_property_key, js_trim,
    lower_name, serialize, style_attribute,
};
use super::wrapper::{Extra, Node, NodeWrapper};

type Res<T> = std::result::Result<T, Thrown>;

/// The kinds of host object, each a class of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostClass {
    Style,
    Tokens,
    StringMap,
}

impl HostClass {
    const ALL: [HostClass; 3] = [HostClass::Style, HostClass::Tokens, HostClass::StringMap];

    /// The interface: its class name, and its key in `interfaces`.
    fn name(self) -> &'static CStr {
        match self {
            HostClass::Style => c"CSSStyleDeclaration",
            HostClass::Tokens => c"DOMTokenList",
            HostClass::StringMap => c"DOMStringMap",
        }
    }

    fn exotic(self) -> &'static qjs::JSClassExoticMethods {
        match self {
            HostClass::Style => &STYLE_EXOTIC,
            HostClass::Tokens => &TOKENS_EXOTIC,
            HostClass::StringMap => &STRING_MAP_EXOTIC,
        }
    }
}

/// The host classes of a runtime, and their prototypes (owned by `DomData`).
pub(crate) struct Hosts {
    ids: [qjs::JSClassID; 3],
    protos: [qjs::JSValue; 3],
}

impl Hosts {
    /// Registers the classes, and finds their prototypes in `interfaces`.
    /// Returns the prototypes too, for the caller to keep alive.
    pub(crate) fn register<'js>(
        ctx: &Ctx<'js>,
        interfaces: &Object<'js>,
    ) -> Result<(Hosts, Vec<Object<'js>>)> {
        // SAFETY: `ctx` is live.
        let rt = unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) };
        let mut ids = [0; 3];
        let mut protos = [qjs::JS_UNDEFINED; 3];
        let mut kept = Vec::with_capacity(3);
        for class in HostClass::ALL {
            // SAFETY: `rt` is live; the class's exotic methods are static.
            ids[class as usize] = unsafe { new_class(rt, class.name(), class.exotic()) }?;
            let key = class.name().to_str().expect("ASCII");
            let proto: Object<'js> = interfaces.get(key)?;
            protos[class as usize] = proto.as_raw();
            kept.push(proto);
        }
        Ok((Hosts { ids, protos }, kept))
    }

    /// The host object `value` is, if it is one of `class`.
    ///
    /// # Safety
    ///
    /// `value` must be alive, and stays borrowed for the result's life.
    pub(crate) unsafe fn get<'v>(&self, value: qjs::JSValue, class: HostClass) -> Option<&'v Host> {
        // SAFETY: `JS_GetOpaque` checks the tag and class; objects of these
        // classes have a `Host` as their opaque.
        unsafe {
            qjs::JS_GetOpaque(value, self.ids[class as usize])
                .cast::<Host>()
                .as_ref()
        }
    }

    /// A new object of `class` for `host`, or `JS_EXCEPTION`.
    fn create(&self, ctx: *mut qjs::JSContext, class: HostClass, host: Host) -> qjs::JSValue {
        let i = class as usize;
        // SAFETY: the prototype is kept alive by `DomData`, and the class
        // is registered in this runtime.
        let obj = unsafe { qjs::JS_NewObjectProtoClass(ctx, self.protos[i], self.ids[i]) };
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(obj) } {
            return obj;
        }
        let ptr = Box::into_raw(Box::new(host));
        // SAFETY: `obj` is a new object of our class; its finalizer frees
        // the box.
        unsafe { qjs::JS_SetOpaque(obj, ptr.cast()) };
        obj
    }
}

/// Registers a class of objects whose opaque is a [`Host`], with these
/// exotic methods, and returns its id.
///
/// # Safety
///
/// `rt` must be live.
unsafe fn new_class(
    rt: *mut qjs::JSRuntime,
    name: &'static CStr,
    exotic: &'static qjs::JSClassExoticMethods,
) -> Result<qjs::JSClassID> {
    let mut id: qjs::JSClassID = 0;
    let def = qjs::JSClassDef {
        class_name: name.as_ptr(),
        finalizer: Some(finalize),
        gc_mark: Some(mark),
        call: None,
        // QuickJS keeps the pointer and only reads through it.
        exotic: std::ptr::from_ref(exotic).cast_mut(),
    };
    // SAFETY: as for the function; QuickJS copies what it needs of `def`.
    unsafe {
        qjs::JS_NewClassID(rt, &mut id);
        if qjs::JS_NewClass(rt, id, &def) != 0 {
            return Err(rquickjs::Error::Allocation);
        }
    }
    Ok(id)
}

unsafe extern "C" fn finalize(_rt: *mut qjs::JSRuntime, val: qjs::JSValue) {
    // SAFETY: `val` is an object of one of our classes being freed; its
    // opaque is the box `Hosts::create` made, or null.
    unsafe {
        let ptr = qjs::JS_GetOpaque(val, qjs::JS_GetClassID(val)).cast::<Host>();
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
    }
}

unsafe extern "C" fn mark(rt: *mut qjs::JSRuntime, val: qjs::JSValue, mark: qjs::JS_MarkFunc) {
    // SAFETY: as in `finalize`.
    unsafe {
        let ptr = qjs::JS_GetOpaque(val, qjs::JS_GetClassID(val)).cast::<Host>();
        if let Some(host) = ptr.as_ref() {
            qjs::JS_MarkValue(rt, host.owner, mark);
        }
    }
}

/// The Rust side of a host object.
pub(crate) struct Host {
    rt: *mut qjs::JSRuntime,
    /// The element's wrapper object: a reference of our own.
    owner: qjs::JSValue,
    /// The `NodeWrapper` in `owner`, which does not move while it lives.
    wrapper: *const NodeWrapper<'static>,
    kind: HostKind,
}

enum HostKind {
    /// `el.style`.
    Inline(RefCell<Declarations>),
    /// `getComputedStyle(el)`: its own copy of the inline declarations.
    Computed(RefCell<Declarations>),
    Tokens(Tokens),
    /// `el.dataset`.
    Dataset,
}

struct Tokens {
    /// The attribute, lower case (`class`, `rel`...).
    attr: Box<str>,
    /// What `supports()` accepts, lower case; `None` if it throws.
    supported: Option<Box<[Box<str>]>>,
    cache: RefCell<TokenCache>,
}

impl Host {
    /// A host for the element `node` (borrowed) whose wrapper is `wrapper`.
    fn new(
        ctx: *mut qjs::JSContext,
        node: qjs::JSValue,
        wrapper: &NodeWrapper<'_>,
        kind: HostKind,
    ) -> Host {
        Host {
            // SAFETY: `ctx` is live.
            rt: unsafe { qjs::JS_GetRuntime(ctx) },
            // SAFETY: `node` is alive; the duplicate is ours.
            owner: unsafe { qjs::JS_DupValue(ctx, node) },
            wrapper: std::ptr::from_ref(wrapper).cast(),
            kind,
        }
    }

    /// The element's wrapper.
    pub(crate) fn wrapper(&self) -> &NodeWrapper<'_> {
        // SAFETY: `owner` keeps the wrapper object, and so its cell, alive.
        unsafe { &*self.wrapper.cast::<NodeWrapper<'_>>() }
    }

    /// The element's wrapper object, borrowed.
    pub(crate) fn owner(&self) -> qjs::JSValue {
        self.owner
    }

    fn tokens(&self) -> &Tokens {
        match &self.kind {
            HostKind::Tokens(t) => t,
            _ => unreachable!("the receiver check makes this a token list"),
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: the reference is ours, in this runtime, which outlives
        // its objects.
        unsafe { qjs::JS_FreeValueRT(self.rt, self.owner) };
    }
}

/// The value of the attribute `getAttribute(name)` finds, for a lower-case
/// `name`: the first with that local name and no prefix.
fn attribute<'d>(doc: &'d Document, id: NodeId, name: &str) -> Option<&'d str> {
    let local = doc.names().get(name)?;
    doc.attrs(id)
        .iter()
        .find(|a| a.local() == local && doc.attr_prefix(a) == Name::EMPTY)
        .map(|a| a.value())
}

// -------------------------------------------------------------------------
// The element's getters.

/// The object in the wrapper's `slot`, made with `kind` the first time.
fn extra<'c>(
    c: &'c Call<'_>,
    slot: Extra,
    class: HostClass,
    kind: fn() -> HostKind,
) -> Outcome<'c> {
    if let Some(raw) = c.wrapper.extra(slot) {
        // SAFETY: the slot holds a live object; the duplicate is the caller's.
        return Ok(Ret::Value(unsafe { qjs::JS_DupValue(c.ctx, raw) }));
    }
    let host = Host::new(c.ctx, c.node, c.wrapper, kind());
    let obj = c.members.hosts.create(c.ctx, class, host);
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(obj) } {
        return Err(Thrown::Pending);
    }
    // SAFETY: `c.ctx` is the running call's; the `Value` owns a new
    // reference to `obj`.
    let kept = unsafe {
        let ctx = super::members::ctx_of(c.ctx);
        Value::from_raw(ctx, qjs::JS_DupValue(c.ctx, obj))
    };
    c.wrapper.set_extra(slot, kept);
    Ok(Ret::Value(obj))
}

pub(super) fn style<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    extra(c, Extra::Style, HostClass::Style, || {
        HostKind::Inline(RefCell::default())
    })
}

pub(super) fn computed_style<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    extra(c, Extra::ComputedStyle, HostClass::Style, || {
        HostKind::Computed(RefCell::default())
    })
}

pub(super) fn class_list<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    extra(c, Extra::ClassList, HostClass::Tokens, || {
        HostKind::Tokens(Tokens {
            attr: "class".into(),
            supported: None,
            cache: RefCell::default(),
        })
    })
}

pub(super) fn dataset<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    extra(c, Extra::Dataset, HostClass::StringMap, || {
        HostKind::Dataset
    })
}

/// `N.tokenList(el, attr, supported)`: a new `DOMTokenList` over `attr`,
/// for the token lists `dom.js` caches itself.
pub(super) fn install<'js>(obj: &Object<'js>) -> Result<()> {
    let f = rquickjs::Function::new(
        obj.ctx().clone(),
        |ctx: Ctx<'js>,
         el: Node<'js>,
         attr: String,
         supported: Option<Vec<String>>|
         -> Result<Value<'js>> {
            let raw_ctx = ctx.as_raw().as_ptr();
            // SAFETY: the context's opaque is the live `Members`, if set.
            let members = unsafe { qjs::JS_GetContextOpaque(raw_ctx).cast::<Members>().as_ref() };
            let members = members.ok_or(rquickjs::Error::Unknown)?;
            let wrapper = el.borrow();
            let kind = HostKind::Tokens(Tokens {
                attr: attr.to_ascii_lowercase().into(),
                supported: supported.map(|s| s.into_iter().map(Into::into).collect()),
                cache: RefCell::default(),
            });
            let host = Host::new(raw_ctx, el.as_raw(), &wrapper, kind);
            drop(wrapper);
            let obj = members.hosts.create(raw_ctx, HostClass::Tokens, host);
            // SAFETY: a tag check.
            if unsafe { qjs::JS_IsException(obj) } {
                return Err(rquickjs::Error::Exception);
            }
            // SAFETY: `obj` is a new reference, which the `Value` takes.
            Ok(unsafe { Value::from_raw(ctx.clone(), obj) })
        },
    )?;
    obj.set("tokenList", f)
}

// -------------------------------------------------------------------------
// CSSStyleDeclaration.

/// The declarations of `host`'s inline style (or the inline layer of its
/// computed style), as `f` sees them.
fn with_decls<R>(host: &Host, doc: &Document, id: NodeId, f: impl FnOnce(&[Decl]) -> R) -> R {
    let cache = match &host.kind {
        HostKind::Inline(d) | HostKind::Computed(d) => d,
        _ => unreachable!("the receiver check makes this a style"),
    };
    let mut cache = cache.borrow_mut();
    f(cache.get(style_attribute(doc, id)))
}

fn is_computed(host: &Host) -> bool {
    matches!(host.kind, HostKind::Computed(_))
}

/// The property names a style lists (its indexes), in order.
fn style_names(doc: &Document, host: &Host, id: NodeId) -> Vec<String> {
    with_decls(host, doc, id, |decls| {
        if !is_computed(host) {
            return decls.iter().map(|d| d.name.to_string()).collect();
        }
        let mut names: Vec<String> = COMPUTED.iter().map(|&(n, _)| n.to_owned()).collect();
        for d in decls {
            if !COMPUTED.iter().any(|&(n, _)| n == &*d.name) {
                names.push(d.name.to_string());
            }
        }
        names
    })
}

/// The value of the property `name` (already lower case unless custom):
/// the inline declaration's for an inline style, the resolved value for a
/// computed one; `""` if none.
fn style_value(dom: &DomState, doc: &Document, host: &Host, id: NodeId, name: &str) -> String {
    with_decls(host, doc, id, |decls| {
        let inline = decls.iter().find(|d| &*d.name == name);
        if !is_computed(host) {
            return inline.map_or_else(String::new, |d| d.value.to_string());
        }
        computed_value(dom, doc, id, decls, inline, name)
    })
}

/// A computed style's value: the inline value if set, else the default for
/// the element; and `display: none` for an element hidden by its markup
/// (`:visible` fails), unless it sets a `visibility` of its own.
fn computed_value(
    dom: &DomState,
    doc: &Document,
    id: NodeId,
    decls: &[Decl],
    inline: Option<&Decl>,
    name: &str,
) -> String {
    if name == "display" {
        let visibility = decls.iter().find(|d| &*d.name == "visibility");
        let visible = visibility.is_none_or(|d| &*d.value == "visible");
        if visible && !dom.visible(doc, id) {
            return "none".to_owned();
        }
    }
    if let Some(d) = inline {
        return d.value.to_string();
    }
    match name {
        "display" => default_display(doc, id).to_owned(),
        "width" => format!("{}px", dom.element_box(doc, id)[2]),
        "height" => format!("{}px", dom.element_box(doc, id)[3]),
        _ => COMPUTED
            .iter()
            .find(|&&(n, _)| n == name)
            .and_then(|&(_, v)| v)
            .unwrap_or("")
            .to_owned(),
    }
}

fn read_only() -> Thrown {
    Thrown::Dom(
        "NoModificationAllowedError",
        "These styles are computed, and therefore read-only.".to_owned(),
    )
}

fn host_of(host: Option<&Host>) -> &Host {
    host.expect("the receiver check found a host")
}

pub(super) fn css_text<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let host = host_of(c.host);
    if !is_computed(host) {
        let text = with_decls(host, &c.doc, n, serialize);
        return Ok(Ret::Str(text.into()));
    }
    let mut out = String::new();
    for name in style_names(&c.doc, host, n) {
        if !out.is_empty() {
            out.push(' ');
        }
        let value = style_value(c.dom, &c.doc, host, n, &name);
        out.push_str(&format!("{name}: {value};"));
    }
    Ok(Ret::Str(out.into()))
}

pub(super) fn style_length<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let host = host_of(c.host);
    let len = if is_computed(host) {
        style_names(&c.doc, host, n).len()
    } else {
        with_decls(host, &c.doc, n, <[Decl]>::len)
    };
    Ok(Ret::Int(i32::try_from(len).unwrap_or(i32::MAX)))
}

pub(super) fn style_item<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let i = c.u32_arg(0) as usize;
    let name = style_names(&c.doc, host_of(c.host), n).into_iter().nth(i);
    Ok(Ret::Str(name.unwrap_or_default().into()))
}

pub(super) fn get_property_value<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let name = lower_name(&c.str_arg(0)?.as_str());
    let value = style_value(c.dom, &c.doc, host_of(c.host), n, &name);
    Ok(Ret::Str(value.into()))
}

pub(super) fn get_property_priority<'c>(
    c: &'c Call<'_>,
    n: NodeId,
    _: &[qjs::JSValue],
) -> Outcome<'c> {
    let host = host_of(c.host);
    if is_computed(host) {
        return Ok(Ret::Static(""));
    }
    // As before, custom property names are lower-cased here too.
    let name = c.str_arg(0)?.as_str().to_lowercase();
    let important = with_decls(host, &c.doc, n, |decls| {
        decls.iter().any(|d| *d.name == *name && d.important)
    });
    Ok(Ret::Static(if important { "important" } else { "" }))
}

pub(super) fn css_float<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let value = style_value(c.dom, &c.doc, host_of(c.host), n, "float");
    Ok(Ret::Str(value.into()))
}

/// The inline declarations as they are now, for changing.
fn current_decls(w: &Write<'_>) -> Vec<Decl> {
    let doc = w.dom.doc();
    with_decls(host_of(w.host), &doc, w.id, <[Decl]>::to_vec)
}

/// Writes `decls` back to the `style` attribute: once, through the path
/// `setAttribute` takes, recording the text with the declarations.
fn write_decls(w: &Write<'_>, decls: Vec<Decl>) -> Res<()> {
    let HostKind::Inline(cache) = &host_of(w.host).kind else {
        return Err(read_only());
    };
    let text = serialize(&decls);
    cache.borrow_mut().set(&text, decls);
    set_attr(&w.rctx(), w.dom, w.id, "style", &text)?;
    Ok(())
}

pub(super) fn set_css_text(w: &Write<'_>) -> Res<Ret<'static>> {
    let value = w.arg(0);
    // SAFETY: a tag check.
    let text = if unsafe { qjs::JS_IsNull(value) } {
        String::new()
    } else {
        w.str_arg(0)?.as_str().into_owned()
    };
    write_decls(w, style::parse(&text))?;
    Ok(Ret::Undefined)
}

pub(super) fn set_property(w: &Write<'_>) -> Res<Ret<'static>> {
    let name = lower_name(&w.str_arg(0)?.as_str());
    let priority = w.arg(2);
    set_named(w, &name, w.arg(1), Some(priority))?;
    Ok(Ret::Undefined)
}

pub(super) fn set_css_float(w: &Write<'_>) -> Res<Ret<'static>> {
    set_named(w, "float", w.arg(0), None)?;
    Ok(Ret::Undefined)
}

/// `setProperty(name, value, priority)` for a name already lower case
/// (unless custom): an empty, `null` or `undefined` value removes it.
fn set_named(
    w: &Write<'_>,
    name: &str,
    value: qjs::JSValue,
    priority: Option<qjs::JSValue>,
) -> Res<()> {
    // SAFETY: tag checks.
    if unsafe { qjs::JS_IsNull(value) || qjs::JS_IsUndefined(value) } {
        remove_named(w, name)?;
        return Ok(());
    }
    // SAFETY: `w.ctx` is the running call's and `value` alive.
    let value = unsafe { JsStr::new(w.ctx, value) }?;
    let value = value.as_str();
    let value = js_trim(&value);
    if value.is_empty() {
        remove_named(w, name)?;
        return Ok(());
    }
    let important = match priority {
        // SAFETY: a tag check.
        Some(p) if !unsafe { qjs::JS_IsUndefined(p) } => {
            // SAFETY: as above.
            let p = unsafe { JsStr::new(w.ctx, p) }?;
            p.as_str().to_lowercase() == "important"
        }
        _ => false,
    };
    if is_computed(host_of(w.host)) {
        return Err(read_only());
    }
    let mut decls = current_decls(w);
    let decl = Decl {
        name: name.into(),
        value: value.into(),
        important,
    };
    match decls.iter_mut().find(|d| *d.name == *name) {
        Some(d) => *d = decl,
        None => decls.push(decl),
    }
    write_decls(w, decls)
}

/// `removeProperty(name)`: the old value, or `""` if it was not set.
fn remove_named(w: &Write<'_>, name: &str) -> Res<String> {
    let host = host_of(w.host);
    if is_computed(host) {
        let doc = w.dom.doc();
        let listed = style_names(&doc, host, w.id).iter().any(|n| n == name);
        drop(doc);
        return if listed {
            Err(read_only())
        } else {
            Ok(String::new())
        };
    }
    let mut decls = current_decls(w);
    let Some(i) = decls.iter().position(|d| &*d.name == name) else {
        return Ok(String::new());
    };
    let old = decls.remove(i).value.into_string();
    write_decls(w, decls)?;
    Ok(old)
}

pub(super) fn remove_property(w: &Write<'_>) -> Res<Ret<'static>> {
    let name = lower_name(&w.str_arg(0)?.as_str());
    let old = remove_named(w, &name)?;
    Ok(Ret::Str(old.into()))
}

// -------------------------------------------------------------------------
// DOMTokenList.

/// A token list's tokens, parsed once per attribute value.
#[derive(Default)]
struct TokenCache {
    text: Option<Box<str>>,
    tokens: Vec<Box<str>>,
}

impl TokenCache {
    fn get(&mut self, text: Option<&str>) -> &[Box<str>] {
        if self.text.as_deref() != text {
            self.tokens = text.map(split_tokens).unwrap_or_default();
            self.text = text.map(Into::into);
        }
        &self.tokens
    }
}

fn is_token_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

/// The ordered set of tokens in `text`: split on ASCII whitespace, without
/// repeats.
fn split_tokens(text: &str) -> Vec<Box<str>> {
    let mut out: Vec<Box<str>> = Vec::new();
    for t in text.split(is_token_space).filter(|t| !t.is_empty()) {
        if !out.iter().any(|x| &**x == t) {
            out.push(t.into());
        }
    }
    out
}

fn with_tokens<R>(host: &Host, doc: &Document, id: NodeId, f: impl FnOnce(&[Box<str>]) -> R) -> R {
    let t = host.tokens();
    let mut cache = t.cache.borrow_mut();
    f(cache.get(attribute(doc, id, &t.attr)))
}

pub(super) fn tokens_length<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let len = with_tokens(host_of(c.host), &c.doc, n, <[Box<str>]>::len);
    Ok(Ret::Int(i32::try_from(len).unwrap_or(i32::MAX)))
}

pub(super) fn tokens_item<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let i = c.u32_arg(0) as usize;
    let token = with_tokens(host_of(c.host), &c.doc, n, |t| {
        t.get(i).map(|t| t.to_string())
    });
    Ok(token.map_or(Ret::Null, |t| Ret::Str(t.into())))
}

pub(super) fn tokens_contains<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let token = c.str_arg(0)?;
    let token = token.as_str();
    // Membership needs no parsing: scan the attribute's tokens in place.
    let value = attribute(&c.doc, n, &host_of(c.host).tokens().attr);
    let found = value.is_some_and(|v| v.split(is_token_space).any(|t| t == token));
    Ok(Ret::Bool(found))
}

pub(super) fn tokens_supports<'c>(c: &'c Call<'_>, _: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let Some(supported) = &host_of(c.host).tokens().supported else {
        return Err(Thrown::Type(
            c"Failed to execute 'supports' on 'DOMTokenList': DOMTokenList has no supported tokens.",
        ));
    };
    let token = c.str_arg(0)?.as_str().to_lowercase();
    Ok(Ret::Bool(supported.iter().any(|s| **s == *token)))
}

pub(super) fn tokens_value<'c>(c: &'c Call<'_>, n: NodeId, _: &[qjs::JSValue]) -> Outcome<'c> {
    let value = attribute(&c.doc, n, &host_of(c.host).tokens().attr);
    Ok(Ret::Str(value.unwrap_or_default().into()))
}

pub(super) fn set_tokens_value(w: &Write<'_>) -> Res<Ret<'static>> {
    let value = w.str_arg(0)?;
    let attr = &host_of(w.host).tokens().attr;
    set_attr(&w.rctx(), w.dom, w.id, attr, &value.as_str())?;
    Ok(Ret::Undefined)
}

/// Argument `i` as a token, checked as `what` (`add`, `toggle`...) does.
fn token_arg(w: &Write<'_>, i: usize, what: &str) -> Res<String> {
    let token = w.str_arg(i)?.as_str().into_owned();
    if token.is_empty() {
        return Err(Thrown::Dom(
            "SyntaxError",
            format!(
                "Failed to execute '{what}' on 'DOMTokenList': The token provided must not be empty."
            ),
        ));
    }
    if token.contains(is_token_space) {
        return Err(Thrown::Dom(
            "InvalidCharacterError",
            format!(
                "Failed to execute '{what}' on 'DOMTokenList': The token provided ('{token}') contains HTML space characters, which are not valid in tokens."
            ),
        ));
    }
    Ok(token)
}

/// The tokens as they are now, for changing; `None` if there is no
/// attribute.
fn current_tokens(w: &Write<'_>) -> (bool, Vec<String>) {
    let doc = w.dom.doc();
    let host = host_of(w.host);
    let present = attribute(&doc, w.id, &host.tokens().attr).is_some();
    let tokens = with_tokens(host, &doc, w.id, |t| {
        t.iter().map(|t| t.to_string()).collect()
    });
    (present, tokens)
}

/// Writes the tokens back to the attribute, once.
fn write_tokens(w: &Write<'_>, tokens: &[String]) -> Res<()> {
    let text = tokens.join(" ");
    let t = host_of(w.host).tokens();
    {
        let mut cache = t.cache.borrow_mut();
        cache.tokens = tokens.iter().map(|t| t.as_str().into()).collect();
        cache.text = Some(text.as_str().into());
    }
    set_attr(&w.rctx(), w.dom, w.id, &t.attr, &text)?;
    Ok(())
}

pub(super) fn tokens_add(w: &Write<'_>) -> Res<Ret<'static>> {
    let checked = (0..w.args.len())
        .map(|i| token_arg(w, i, "add"))
        .collect::<Res<Vec<_>>>()?;
    let (_, mut list) = current_tokens(w);
    for t in checked {
        if !list.contains(&t) {
            list.push(t);
        }
    }
    write_tokens(w, &list)?;
    Ok(Ret::Undefined)
}

pub(super) fn tokens_remove(w: &Write<'_>) -> Res<Ret<'static>> {
    let checked = (0..w.args.len())
        .map(|i| token_arg(w, i, "remove"))
        .collect::<Res<Vec<_>>>()?;
    let (present, mut list) = current_tokens(w);
    if !present {
        return Ok(Ret::Undefined);
    }
    list.retain(|t| !checked.contains(t));
    write_tokens(w, &list)?;
    Ok(Ret::Undefined)
}

pub(super) fn tokens_toggle(w: &Write<'_>) -> Res<Ret<'static>> {
    let token = token_arg(w, 0, "toggle")?;
    let force = w.arg(1);
    // SAFETY: tag checks.
    let (force_true, force_false) = unsafe {
        let is_bool = qjs::JS_IsBool(force);
        (
            is_bool && qjs::JS_VALUE_GET_BOOL(force),
            is_bool && !qjs::JS_VALUE_GET_BOOL(force),
        )
    };
    let (_, mut list) = current_tokens(w);
    if list.contains(&token) {
        if force_true {
            return Ok(Ret::Bool(true));
        }
        list.retain(|t| *t != token);
        write_tokens(w, &list)?;
        return Ok(Ret::Bool(false));
    }
    if force_false {
        return Ok(Ret::Bool(false));
    }
    list.push(token);
    write_tokens(w, &list)?;
    Ok(Ret::Bool(true))
}

pub(super) fn tokens_replace(w: &Write<'_>) -> Res<Ret<'static>> {
    let token = token_arg(w, 0, "replace")?;
    let new_token = token_arg(w, 1, "replace")?;
    let (_, list) = current_tokens(w);
    if !list.contains(&token) {
        return Ok(Ret::Bool(false));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for t in list {
        let v = if t == token { new_token.clone() } else { t };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    write_tokens(w, &out)?;
    Ok(Ret::Bool(true))
}

// -------------------------------------------------------------------------
// dataset.

/// The attribute behind the `dataset` key `key`: `data-` and the key with
/// each ASCII capital turned into a dash and its lower case.
fn data_attribute(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 8);
    out.push_str("data-");
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The `dataset` key of a `data-*` attribute name, if it has one.
fn data_key(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    if attr.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&next) if c == '-' && next.is_ascii_lowercase() => {
                out.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => out.push(c),
        }
    }
    Some(out)
}

// -------------------------------------------------------------------------
// The exotic methods.

static STYLE_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(style_get_own),
    get_own_property_names: Some(style_names_of),
    delete_property: Some(delete_nothing),
    define_own_property: Some(style_define),
    has_property: None,
    get_property: None,
    set_property: None,
};

static TOKENS_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(tokens_get_own),
    get_own_property_names: Some(tokens_names_of),
    delete_property: Some(delete_nothing),
    define_own_property: Some(tokens_define),
    has_property: None,
    get_property: None,
    set_property: None,
};

static STRING_MAP_EXOTIC: qjs::JSClassExoticMethods = qjs::JSClassExoticMethods {
    get_own_property: Some(data_get_own),
    get_own_property_names: Some(data_names_of),
    delete_property: Some(data_delete),
    define_own_property: Some(data_define),
    has_property: None,
    get_property: None,
    set_property: None,
};

/// Array index atoms are tagged integers (QuickJS's `JS_ATOM_TAG_INT`).
pub(super) const ATOM_TAG_INT: qjs::JSAtom = 1 << 31;

/// A property key, as the exotic methods see it.
pub(super) enum Key {
    Index(u32),
    Str(JsStr),
    Symbol,
}

/// The key `atom` is.
///
/// # Safety
///
/// `ctx` must be live and `atom` one of its atoms.
pub(super) unsafe fn key_of(ctx: *mut qjs::JSContext, atom: qjs::JSAtom) -> std::result::Result<Key, ()> {
    if atom & ATOM_TAG_INT != 0 {
        return Ok(Key::Index(atom & !ATOM_TAG_INT));
    }
    // SAFETY: as for the function; converting a string runs no JS.
    unsafe {
        let v = qjs::JS_AtomToValue(ctx, atom);
        if qjs::JS_IsException(v) {
            return Err(());
        }
        if qjs::JS_IsSymbol(v) {
            qjs::JS_FreeValue(ctx, v);
            return Ok(Key::Symbol);
        }
        let s = JsStr::new(ctx, v);
        qjs::JS_FreeValue(ctx, v);
        s.map(Key::Str).map_err(|_| ())
    }
}

/// What an exotic method needs: the members, the host behind `obj` and the
/// element's node. `Err` with an exception thrown for a node of a replaced
/// document; `None` when `obj` is not ours.
///
/// # Safety
///
/// `ctx` must be live and `obj` alive.
unsafe fn host_in<'a>(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    class: HostClass,
) -> Option<std::result::Result<(&'a Members, &'a Host, NodeId), ()>> {
    // SAFETY: the context's opaque is null or the live `Members`.
    let members = unsafe { qjs::JS_GetContextOpaque(ctx).cast::<Members>().as_ref() }?;
    // SAFETY: as for the function.
    let host = unsafe { members.hosts.get(obj, class) }?;
    Some(match host.wrapper().id() {
        Some(id) => Ok((members, host, id)),
        None => {
            // SAFETY: as for the function.
            unsafe { Thrown::Stale.throw(ctx) };
            Err(())
        }
    })
}

/// Fills `desc` (if asked for) with a data property, taking `value`.
///
/// # Safety
///
/// `desc` must be null or writable; `value` is a reference of our own.
pub(super) unsafe fn fill(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    value: qjs::JSValue,
    flags: u32,
) -> c_int {
    // SAFETY: a tag check.
    if unsafe { qjs::JS_IsException(value) } {
        return -1;
    }
    // SAFETY: as for the function.
    match unsafe { desc.as_mut() } {
        Some(desc) => {
            desc.flags = flags as c_int;
            desc.value = value;
            desc.getter = qjs::JS_UNDEFINED;
            desc.setter = qjs::JS_UNDEFINED;
        }
        // SAFETY: the reference is ours.
        None => unsafe { qjs::JS_FreeValue(ctx, value) },
    }
    1
}

/// Whether `atom` is found on `obj`'s prototype chain (`key in target`).
///
/// # Safety
///
/// `ctx` must be live and `obj` alive. May run JS (a `Proxy` prototype).
pub(super) unsafe fn proto_has(ctx: *mut qjs::JSContext, obj: qjs::JSValue, atom: qjs::JSAtom) -> c_int {
    // SAFETY: as for the function.
    unsafe {
        let proto = qjs::JS_GetPrototype(ctx, obj);
        if qjs::JS_IsException(proto) {
            return -1;
        }
        let has = qjs::JS_HasProperty(ctx, proto, atom);
        qjs::JS_FreeValue(ctx, proto);
        has
    }
}

/// Defines the property as on an ordinary object.
///
/// # Safety
///
/// As for an exotic `define_own_property`.
pub(super) unsafe fn define_plain(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
    val: qjs::JSValue,
    getter: qjs::JSValue,
    setter: qjs::JSValue,
    flags: c_int,
) -> c_int {
    // SAFETY: as for the function.
    unsafe { qjs::JS_DefineProperty(ctx, obj, atom, val, getter, setter, flags | NO_EXOTIC) }
}

const NO_EXOTIC: c_int = 1 << 17;
const DEFINE_PROPERTY: c_int = 1 << 18;
const HAS_VALUE: c_int = 1 << 13;
const THROW: c_int = 1 << 14;

/// Refuses a definition: `false`, or a `TypeError` when the caller throws.
///
/// # Safety
///
/// `ctx` must be live.
unsafe fn refuse(ctx: *mut qjs::JSContext, flags: c_int) -> c_int {
    if flags & THROW != 0 {
        // SAFETY: as for the function.
        unsafe { super::members::throw_type(ctx, c"Cannot define this property") };
        return -1;
    }
    0
}

/// Gives `names` to QuickJS as a property list.
///
/// # Safety
///
/// As for an exotic `get_own_property_names`.
unsafe fn property_list(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    names: impl ExactSizeIterator<Item = std::result::Result<u32, String>>,
) -> c_int {
    let n = names.len();
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
        for (i, name) in names.enumerate() {
            let atom = match name {
                Ok(index) => qjs::JS_NewAtomUInt32(ctx, index),
                Err(s) => qjs::JS_NewAtomLen(ctx, s.as_ptr().cast(), s.len() as qjs::size_t),
            };
            tab.add(i).write(qjs::JSPropertyEnum {
                is_enumerable: true,
                atom,
            });
        }
        *ptab = tab;
        *plen = n as u32;
    }
    0
}

pub(super) unsafe extern "C" fn delete_nothing(
    _ctx: *mut qjs::JSContext,
    _obj: qjs::JSValue,
    _atom: qjs::JSAtom,
) -> c_int {
    1
}

unsafe extern "C" fn style_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    // SAFETY: QuickJS calls this with a live context, object and atom.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::Style) }) else {
        return 0;
    };
    let Ok((_, host, id)) = found else { return -1 };
    let dom = host.wrapper().dom();
    // SAFETY: as above.
    match unsafe { key_of(ctx, atom) } {
        Err(()) => -1,
        Ok(Key::Symbol) => 0,
        Ok(Key::Index(i)) => {
            let name = style_names(&dom.doc(), host, id)
                .into_iter()
                .nth(i as usize);
            let Some(name) = name else { return 0 };
            if desc.is_null() {
                return 1;
            }
            let flags = qjs::JS_PROP_ENUMERABLE | qjs::JS_PROP_CONFIGURABLE;
            // SAFETY: as above.
            unsafe { fill(ctx, desc, new_string(ctx, &name), flags) }
        }
        Ok(Key::Str(key)) => {
            let key = key.as_str();
            if !is_property_key(&key) {
                return 0;
            }
            // SAFETY: as above; nothing is borrowed.
            match unsafe { proto_has(ctx, obj, atom) } {
                0 => {}
                1 => return 0,
                _ => return -1,
            }
            if desc.is_null() {
                return 1;
            }
            let value = style_value(dom, &dom.doc(), host, id, &dashed(&key));
            let flags = qjs::JS_PROP_WRITABLE | qjs::JS_PROP_CONFIGURABLE;
            // SAFETY: as above.
            unsafe { fill(ctx, desc, new_string(ctx, &value), flags) }
        }
    }
}

unsafe extern "C" fn style_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as in `style_get_own`.
    let n = match unsafe { host_in(ctx, obj, HostClass::Style) } {
        None => 0,
        Some(Err(())) => return -1,
        Some(Ok((_, host, id))) => {
            let dom = host.wrapper().dom();
            style_names(&dom.doc(), host, id).len() as u32
        }
    };
    // SAFETY: as for an exotic method.
    unsafe { property_list(ctx, ptab, plen, (0..n).map(Ok)) }
}

unsafe extern "C" fn style_define(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
    val: qjs::JSValue,
    getter: qjs::JSValue,
    setter: qjs::JSValue,
    flags: c_int,
) -> c_int {
    // SAFETY: QuickJS calls this with live values.
    let plain = || unsafe { define_plain(ctx, obj, atom, val, getter, setter, flags) };
    if flags & DEFINE_PROPERTY != 0 || flags & HAS_VALUE == 0 {
        return plain();
    }
    // SAFETY: as above.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::Style) }) else {
        return plain();
    };
    let Ok((members, host, id)) = found else {
        return -1;
    };
    // SAFETY: as above.
    let name = match unsafe { key_of(ctx, atom) } {
        Err(()) => return -1,
        // Indexes are read-only; assigning one does nothing.
        Ok(Key::Index(_)) => return 1,
        Ok(Key::Symbol) => return plain(),
        Ok(Key::Str(key)) => {
            let key = key.as_str();
            if !is_property_key(&key) {
                return plain();
            }
            // SAFETY: as above.
            match unsafe { proto_has(ctx, obj, atom) } {
                0 => {}
                1 => return plain(),
                _ => return -1,
            }
            dashed(&key)
        }
    };
    let w = Write {
        ctx,
        members,
        dom: host.wrapper().dom(),
        id,
        wrapper: host.wrapper(),
        node: host.owner(),
        host: Some(host),
        args: &[],
    };
    match set_named(&w, &name, val, None) {
        Ok(()) => 1,
        Err(thrown) => {
            // SAFETY: as above.
            unsafe { thrown.throw(ctx) };
            -1
        }
    }
}

unsafe extern "C" fn tokens_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    if atom & ATOM_TAG_INT == 0 {
        return 0;
    }
    let i = (atom & !ATOM_TAG_INT) as usize;
    // SAFETY: as in `style_get_own`.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::Tokens) }) else {
        return 0;
    };
    let Ok((_, host, id)) = found else { return -1 };
    let dom = host.wrapper().dom();
    let token = with_tokens(host, &dom.doc(), id, |t| t.get(i).map(|t| t.to_string()));
    let Some(token) = token else { return 0 };
    if desc.is_null() {
        return 1;
    }
    let flags = qjs::JS_PROP_ENUMERABLE | qjs::JS_PROP_CONFIGURABLE;
    // SAFETY: as above.
    unsafe { fill(ctx, desc, new_string(ctx, &token), flags) }
}

unsafe extern "C" fn tokens_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as in `style_get_own`.
    let n = match unsafe { host_in(ctx, obj, HostClass::Tokens) } {
        None => 0,
        Some(Err(())) => return -1,
        Some(Ok((_, host, id))) => {
            let dom = host.wrapper().dom();
            with_tokens(host, &dom.doc(), id, <[Box<str>]>::len) as u32
        }
    };
    // SAFETY: as for an exotic method.
    unsafe { property_list(ctx, ptab, plen, (0..n).map(Ok)) }
}

unsafe extern "C" fn tokens_define(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
    val: qjs::JSValue,
    getter: qjs::JSValue,
    setter: qjs::JSValue,
    flags: c_int,
) -> c_int {
    if atom & ATOM_TAG_INT != 0 {
        // The indexes are read-only; assigning one does nothing.
        return 1;
    }
    // SAFETY: QuickJS calls this with live values.
    unsafe { define_plain(ctx, obj, atom, val, getter, setter, flags) }
}

/// The `dataset` key `atom` names, or `None` for a symbol.
///
/// # Safety
///
/// As for [`key_of`].
unsafe fn data_key_of(
    ctx: *mut qjs::JSContext,
    atom: qjs::JSAtom,
) -> std::result::Result<Option<String>, ()> {
    // SAFETY: as for the function.
    Ok(match unsafe { key_of(ctx, atom) }? {
        Key::Index(i) => Some(i.to_string()),
        Key::Str(s) => Some(s.as_str().into_owned()),
        Key::Symbol => None,
    })
}

unsafe extern "C" fn data_get_own(
    ctx: *mut qjs::JSContext,
    desc: *mut qjs::JSPropertyDescriptor,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    // SAFETY: as in `style_get_own`.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::StringMap) }) else {
        return 0;
    };
    let Ok((_, host, id)) = found else { return -1 };
    // SAFETY: as above.
    let key = match unsafe { data_key_of(ctx, atom) } {
        Err(()) => return -1,
        Ok(None) => return 0,
        Ok(Some(key)) => key,
    };
    let dom = host.wrapper().dom();
    let doc = dom.doc();
    let Some(value) = attribute(&doc, id, &data_attribute(&key)) else {
        return 0;
    };
    if desc.is_null() {
        return 1;
    }
    let value = new_string(ctx, value);
    drop(doc);
    // SAFETY: as above.
    unsafe { fill(ctx, desc, value, qjs::JS_PROP_C_W_E) }
}

unsafe extern "C" fn data_names_of(
    ctx: *mut qjs::JSContext,
    ptab: *mut *mut qjs::JSPropertyEnum,
    plen: *mut u32,
    obj: qjs::JSValue,
) -> c_int {
    // SAFETY: as in `style_get_own`.
    let keys = match unsafe { host_in(ctx, obj, HostClass::StringMap) } {
        None => Vec::new(),
        Some(Err(())) => return -1,
        Some(Ok((_, host, id))) => {
            let doc = host.wrapper().dom().doc();
            doc.attrs(id)
                .iter()
                .filter(|a| doc.attr_namespace(a) == Name::EMPTY)
                .filter_map(|a| data_key(doc.name_str(a.local())))
                .collect()
        }
    };
    // SAFETY: as for an exotic method.
    unsafe { property_list(ctx, ptab, plen, keys.into_iter().map(Err)) }
}

unsafe extern "C" fn data_define(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
    val: qjs::JSValue,
    _getter: qjs::JSValue,
    _setter: qjs::JSValue,
    flags: c_int,
) -> c_int {
    // SAFETY: QuickJS calls this with live values.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::StringMap) }) else {
        return 0;
    };
    let Ok((members, host, id)) = found else {
        return -1;
    };
    // SAFETY: as above.
    let key = match unsafe { data_key_of(ctx, atom) } {
        Err(()) => return -1,
        Ok(Some(key)) if flags & HAS_VALUE != 0 => key,
        // A symbol or an accessor: refused, as by the `Proxy` before.
        // SAFETY: as above.
        Ok(_) => return unsafe { refuse(ctx, flags) },
    };
    let w = Write {
        ctx,
        members,
        dom: host.wrapper().dom(),
        id,
        wrapper: host.wrapper(),
        node: host.owner(),
        host: Some(host),
        args: &[],
    };
    match set_data(&w, &key, val) {
        Ok(()) => 1,
        Err(thrown) => {
            // SAFETY: as above.
            unsafe { thrown.throw(ctx) };
            -1
        }
    }
}

/// `dataset[key] = value`.
fn set_data(w: &Write<'_>, key: &str, value: qjs::JSValue) -> Res<()> {
    let has_dash_lower = key
        .as_bytes()
        .windows(2)
        .any(|p| p[0] == b'-' && p[1].is_ascii_lowercase());
    if has_dash_lower {
        return Err(Thrown::Dom(
            "SyntaxError",
            format!("'{key}' is not a valid property name."),
        ));
    }
    let name = data_attribute(key);
    // SAFETY: `w.ctx` is the running call's and `value` alive.
    let value = unsafe { JsStr::new(w.ctx, value) }?;
    if !valid_attribute_name(&name) {
        return Err(Thrown::Dom(
            "InvalidCharacterError",
            format!("'{name}' is not a valid attribute name."),
        ));
    }
    set_attr(&w.rctx(), w.dom, w.id, &name, &value.as_str())?;
    Ok(())
}

unsafe extern "C" fn data_delete(
    ctx: *mut qjs::JSContext,
    obj: qjs::JSValue,
    atom: qjs::JSAtom,
) -> c_int {
    // SAFETY: as in `style_get_own`.
    let Some(found) = (unsafe { host_in(ctx, obj, HostClass::StringMap) }) else {
        return 1;
    };
    let Ok((_, host, id)) = found else { return -1 };
    // SAFETY: as above.
    let key = match unsafe { data_key_of(ctx, atom) } {
        Err(()) => return -1,
        Ok(None) => return 1,
        Ok(Some(key)) => key,
    };
    // SAFETY: as above.
    let rctx = unsafe { super::members::ctx_of(ctx) };
    match remove_attr(&rctx, host.wrapper().dom(), id, &data_attribute(&key)) {
        Ok(_) => 1,
        Err(e) => {
            // SAFETY: as above.
            unsafe { Thrown::Error(e).throw(ctx) };
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_names() {
        assert_eq!(data_attribute("userId"), "data-user-id");
        assert_eq!(data_attribute("x"), "data-x");
        assert_eq!(data_key("data-user-id").as_deref(), Some("userId"));
        assert_eq!(data_key("data-x-1").as_deref(), Some("x-1"));
        assert_eq!(data_key("data-X"), None);
        assert_eq!(data_key("datax"), None);
        assert_eq!(data_key("data-").as_deref(), Some(""));
    }

    #[test]
    fn tokens() {
        assert_eq!(
            split_tokens(" a\tb  a\nc "),
            ["a", "b", "c"].map(Box::<str>::from)
        );
        assert!(split_tokens("").is_empty());
    }

    #[test]
    fn atom_tag() {
        assert_eq!(ATOM_TAG_INT, 0x8000_0000);
    }
}
