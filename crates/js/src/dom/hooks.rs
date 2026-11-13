//! What the native members keep from `dom.js` for the life of the context:
//! the JS fallbacks of members made with `Member::or_js`, the glue's hooks
//! (`connected()` for the scripts an insertion connects, the base URL),
//! `NodeList.prototype` for static node lists, and the table of reflected
//! attributes (see the `attrs` module). Part of `Members`, so reached
//! through the context's opaque pointer like the rest.
//!
//! The JS values here are references of our own, given back on drop (when
//! `DomData` goes, before the runtime is freed).

use std::cell::RefCell;
use std::ffi::c_int;
use std::rc::Rc;

use dasher_dom::NodeId;
use rquickjs::{Array, Ctx, Function, Object, Result, Value, qjs};

use super::DomState;
use super::attrs::Reflected;
use super::lists::{AttrKey, Lists, Proto};
use super::members::new_string;
use super::wrapper::wrap;

pub(crate) struct Hooks {
    rt: *mut qjs::JSRuntime,
    /// By member index: the JS function the member replaced, if it keeps
    /// it as its fallback, else `undefined`.
    fallbacks: Vec<qjs::JSValue>,
    /// The glue's `connected(scripts)`.
    connected: qjs::JSValue,
    /// The glue's `internals.baseURL()`.
    base_url: qjs::JSValue,
    node_list_proto: qjs::JSValue,
    collection_proto: qjs::JSValue,
    attributes_proto: qjs::JSValue,
    /// The glue's `attrAt(el, i)` and `attrNamed(el, name)`, which make
    /// the `Attr` objects of a `NamedNodeMap`.
    attr_at: qjs::JSValue,
    attr_named: qjs::JSValue,
    /// The list classes (see the `lists` module).
    lists: Lists,
    /// `length`, `item` and `namedItem`: keys a list's exotic lookup can
    /// skip, since its prototype has them.
    list_members: [qjs::JSAtom; 3],
    reflected: RefCell<Vec<Rc<Reflected>>>,
    /// The base URL last resolved against, parsed.
    base: RefCell<Option<(String, Option<url::Url>)>>,
}

/// A new reference to `v`.
fn keep(ctx: &Ctx<'_>, v: &Value<'_>) -> qjs::JSValue {
    // SAFETY: `v` is alive; the duplicate is ours.
    unsafe { qjs::JS_DupValue(ctx.as_raw().as_ptr(), v.as_raw()) }
}

impl Hooks {
    /// Takes the fallbacks `members::install` kept, and the hooks and
    /// prototypes from what `dom.js` passed to `N.init`.
    pub(crate) fn new<'js>(
        ctx: &Ctx<'js>,
        fallbacks: Vec<Option<Value<'js>>>,
        hooks: &Object<'js>,
        interfaces: &Object<'js>,
    ) -> Result<Hooks> {
        let connected: Function<'js> = hooks.get("connected")?;
        let base_url: Function<'js> = hooks.get("baseURL")?;
        let node_list: Object<'js> = interfaces.get("NodeList")?;
        let collection: Object<'js> = interfaces.get("HTMLCollection")?;
        let attributes: Object<'js> = interfaces.get("NamedNodeMap")?;
        let attr_at: Function<'js> = hooks.get("attrAt")?;
        let attr_named: Function<'js> = hooks.get("attrNamed")?;
        let raw = ctx.as_raw().as_ptr();
        let atom = |name: &str| {
            // SAFETY: `ctx` is live; the atom's reference is ours.
            unsafe { qjs::JS_NewAtomLen(raw, name.as_ptr().cast(), name.len() as qjs::size_t) }
        };
        Ok(Hooks {
            // SAFETY: `ctx` is live.
            rt: unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) },
            fallbacks: fallbacks
                .iter()
                .map(|f| f.as_ref().map_or(qjs::JS_UNDEFINED, |f| keep(ctx, f)))
                .collect(),
            connected: keep(ctx, &connected.into_value()),
            base_url: keep(ctx, &base_url.into_value()),
            node_list_proto: keep(ctx, &node_list.into_value()),
            collection_proto: keep(ctx, &collection.into_value()),
            attributes_proto: keep(ctx, &attributes.into_value()),
            attr_at: keep(ctx, &attr_at.into_value()),
            attr_named: keep(ctx, &attr_named.into_value()),
            lists: Lists::register(ctx)?,
            list_members: [atom("length"), atom("item"), atom("namedItem")],
            reflected: RefCell::default(),
            base: RefCell::default(),
        })
    }

    /// Calls member `magic`'s JS fallback as the member was called.
    ///
    /// # Safety
    ///
    /// The arguments must be those QuickJS passed to the member's C
    /// function.
    pub(crate) unsafe fn call_fallback(
        &self,
        ctx: *mut qjs::JSContext,
        magic: c_int,
        this: qjs::JSValue,
        argc: c_int,
        argv: *mut qjs::JSValue,
    ) -> qjs::JSValue {
        let f = self
            .fallbacks
            .get(magic as usize)
            .copied()
            .unwrap_or(qjs::JS_UNDEFINED);
        // SAFETY: as for the function; `JS_Call` throws if `f` is not a
        // function. The fallbacks live as long as `self`, and the call
        // takes references of its own.
        unsafe { qjs::JS_Call(ctx, f, this, argc, argv) }
    }

    /// Hands `scripts`, which an insertion connected, to the glue's
    /// `connected()`, which runs or loads them.
    pub(crate) fn connected(
        &self,
        ctx: &Ctx<'_>,
        dom: &Rc<DomState>,
        scripts: &[NodeId],
    ) -> Result<()> {
        let array = Array::new(ctx.clone())?;
        for (i, &n) in scripts.iter().enumerate() {
            array.set(i, wrap(ctx, dom, n)?)?;
        }
        // SAFETY: `self.connected` is a live function of ours; the `Value`
        // takes a new reference.
        let hook = unsafe {
            Value::from_raw(ctx.clone(), qjs::JS_DupValue(ctx.as_raw().as_ptr(), self.connected))
        };
        let hook = Function::from_value(hook)?;
        hook.call::<_, ()>((array,))
    }

    /// Gives the array `array` `NodeList.prototype`, as the glue's
    /// `staticNodeList`. Returns false with an exception thrown on failure.
    pub(crate) fn static_node_list(&self, ctx: *mut qjs::JSContext, array: qjs::JSValue) -> bool {
        // SAFETY: `array` is a live object of the caller's, the prototype
        // ours.
        unsafe { qjs::JS_SetPrototype(ctx, array, self.node_list_proto) >= 0 }
    }

    pub(crate) fn lists(&self) -> &Lists {
        &self.lists
    }

    pub(crate) fn proto(&self, proto: Proto) -> qjs::JSValue {
        match proto {
            Proto::NodeList => self.node_list_proto,
            Proto::HtmlCollection => self.collection_proto,
            Proto::NamedNodeMap => self.attributes_proto,
        }
    }

    /// Whether `atom` is a member every list's prototype has.
    pub(crate) fn is_list_member(&self, atom: qjs::JSAtom) -> bool {
        self.list_members.contains(&atom)
    }

    /// The `Attr` the glue makes for `key` of `owner`'s attributes, or
    /// `null`, as a new reference (or `JS_EXCEPTION`). Runs JS.
    ///
    /// # Safety
    ///
    /// `ctx` must be live and `owner` alive.
    pub(crate) unsafe fn attr(
        &self,
        ctx: *mut qjs::JSContext,
        owner: qjs::JSValue,
        key: AttrKey,
    ) -> qjs::JSValue {
        let (f, arg) = match key {
            AttrKey::Index(i) => (
                self.attr_at,
                qjs::JS_MKVAL(qjs::JS_TAG_INT, i32::try_from(i).unwrap_or(i32::MAX)),
            ),
            // SAFETY: as for the function.
            AttrKey::Name(atom) => (self.attr_named, unsafe { qjs::JS_AtomToString(ctx, atom) }),
        };
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(arg) } {
            return arg;
        }
        let mut args = [owner, arg];
        // SAFETY: the hooks are live functions of ours; the call takes
        // references of its own.
        let v = unsafe { qjs::JS_Call(ctx, f, qjs::JS_UNDEFINED, 2, args.as_mut_ptr()) };
        // SAFETY: `arg` is ours (an int needs no freeing).
        unsafe { qjs::JS_FreeValue(ctx, args[1]) };
        v
    }

    /// Adds a reflected attribute, returning its index.
    pub(crate) fn add_reflected(&self, reflected: Reflected) -> usize {
        let mut table = self.reflected.borrow_mut();
        table.push(Rc::new(reflected));
        table.len() - 1
    }

    pub(crate) fn reflected(&self, index: usize) -> Option<Rc<Reflected>> {
        self.reflected.borrow().get(index).cloned()
    }

    /// `value` resolved against the document's base URL, as a new JS
    /// string (or `value` itself if it does not resolve), or
    /// `JS_EXCEPTION`. Runs JS: the glue's `baseURL()`.
    pub(crate) fn resolve_url(&self, ctx: *mut qjs::JSContext, value: &str) -> qjs::JSValue {
        // SAFETY: the hook is a live function of ours.
        let base = unsafe { qjs::JS_Call(ctx, self.base_url, qjs::JS_UNDEFINED, 0, std::ptr::null_mut()) };
        // SAFETY: a tag check.
        if unsafe { qjs::JS_IsException(base) } {
            return base;
        }
        // SAFETY: `base` is ours; converting a string runs no JS.
        let text = unsafe { super::members::JsStr::new(ctx, base) };
        // SAFETY: as above.
        unsafe { qjs::JS_FreeValue(ctx, base) };
        let Ok(text) = text else {
            return qjs::JS_EXCEPTION;
        };
        let text = text.as_str();
        let mut cache = self.base.borrow_mut();
        if cache.as_ref().is_none_or(|(s, _)| *s != *text) {
            *cache = Some((text.to_string(), url::Url::parse(&text).ok()));
        }
        let resolved = cache
            .as_ref()
            .and_then(|(_, base)| base.as_ref())
            .and_then(|base| base.join(value).ok());
        match resolved {
            Some(url) => new_string(ctx, url.as_str()),
            None => new_string(ctx, value),
        }
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        let values = self.fallbacks.drain(..).chain([
            self.connected,
            self.base_url,
            self.node_list_proto,
            self.collection_proto,
            self.attributes_proto,
            self.attr_at,
            self.attr_named,
        ]);
        for v in values {
            // SAFETY: each is a reference of ours (or `undefined`), in this
            // runtime, which outlives `DomData`.
            unsafe { qjs::JS_FreeValueRT(self.rt, v) };
        }
        for atom in self.list_members {
            // SAFETY: as above.
            unsafe { qjs::JS_FreeAtomRT(self.rt, atom) };
        }
    }
}
