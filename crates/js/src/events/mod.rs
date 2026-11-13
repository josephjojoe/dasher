//! DOM events in Rust: event targets and their listeners, `Event` objects
//! and dispatch.
//!
//! `events.js` builds the rest of the events API (`CustomEvent` and the
//! other event subclasses, `AbortSignal`, error reporting) on what this
//! module puts in `native.events`:
//!
//! - `EventTarget` and `Event`, native constructors whose instances keep
//!   their state in Rust ([`TargetData`], [`EventData`]). Subclasses made
//!   in JS (`class X extends EventTarget`, `CustomEvent`) reach them
//!   through `super()`, so every event target and event has that state.
//!   Their members are QuickJS C functions on the prototypes.
//! - Listener storage ([`Listeners`]) per target: in the [`TargetData`] of
//!   targets made by the constructor, in the node wrapper for nodes (see
//!   `dom`), and in [`EventsData`] for the global object, which QuickJS
//!   makes itself. A target is recognised from its object like a node
//!   wrapper is (see [`Layout`]).
//! - Dispatch (the `dispatch` module), which computes the event path (with
//!   shadow DOM retargeting) on node ids and calls the listeners, and the
//!   host's way in: `internals.fire` for the glue and
//!   [`fire`] for Rust check the path for listeners before an event object
//!   is made, so firing an event nobody listens for costs a walk up the
//!   tree and allocates nothing.
//! - Event handlers (`onclick` and friends): `on*` accessors that
//!   `defineEventHandlers` puts on prototypes, and the content attributes
//!   that set them, compiled lazily (see the `dispatch` module).
//!
//! # Garbage collection
//!
//! A listener's callback is reported to the GC by its target's object (see
//! the `listeners` module), so it lives exactly as long as the target's
//! wrapper group. An event reports its target, current target and path.
//! An abort signal reports the targets whose listeners it removes when it
//! aborts, as the abort algorithm that removes them would hold them.

mod dispatch;
mod listeners;
mod natives;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::{Rc, Weak};

use rquickjs::class::{JsClass, Readable, Trace, Tracer};
use rquickjs::function::Constructor;
use rquickjs::{Class, Ctx, Function, JsLifetime, Object, Result, Value, qjs};

pub(crate) use dispatch::{HostTarget, fire, reaches};
pub(crate) use listeners::Listeners;
use listeners::{Listener, TypeAtom};

use crate::dom::NodeWrapper;
use crate::native_data::NativeData;
use crate::runtime::Shared;

// -------------------------------------------------------------------------
// Event targets.

/// The Rust side of an `EventTarget` made by its constructor: everything
/// but nodes and the global object.
#[derive(Default)]
pub(crate) struct TargetData<'js> {
    listeners: Listeners<'js>,
    /// Set for an `AbortSignal`.
    signal: RefCell<Option<Box<Signal<'js>>>>,
}

/// An abort signal's part of the listener machinery.
#[derive(Default)]
struct Signal<'js> {
    aborted: bool,
    /// Listeners added with this signal, removed when it aborts.
    removals: Vec<Removal<'js>>,
    /// The length of `removals` after it was last pruned.
    pruned: usize,
}

struct Removal<'js> {
    target: Value<'js>,
    ty: TypeAtom<'js>,
    /// Weak: once the listener is removed some other way, nothing needs
    /// it.
    listener: Weak<Listener<'js>>,
}

impl<'js> Trace<'js> for TargetData<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.listeners.trace(tracer);
        if let Ok(signal) = self.signal.try_borrow()
            && let Some(signal) = &*signal
        {
            for r in &signal.removals {
                r.target.trace(tracer);
            }
        }
    }
}

// SAFETY: `TargetData` only holds values bound to `'js`, and `Changed` is
// the same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for TargetData<'js> {
    type Changed<'to> = TargetData<'to>;
}

impl<'js> JsClass<'js> for TargetData<'js> {
    const NAME: &'static str = "EventTarget";
    type Mutable = Readable;

    fn prototype(_ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
        // Instances are always made with an explicit prototype.
        Ok(None)
    }

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

impl<'js> TargetData<'js> {
    /// Makes this target an abort signal.
    fn make_signal(&self) {
        let mut signal = self.signal.borrow_mut();
        if signal.is_none() {
            *signal = Some(Box::default());
        }
    }

    /// Whether this is an abort signal, and if so whether it aborted.
    fn signal_aborted(&self) -> Option<bool> {
        self.signal.borrow().as_ref().map(|s| s.aborted)
    }

    /// Notes that `listener`, of `target`, goes when this signal aborts.
    fn add_removal(&self, target: Value<'js>, ty: TypeAtom<'js>, listener: &Rc<Listener<'js>>) {
        let mut signal = self.signal.borrow_mut();
        let Some(signal) = signal.as_mut() else {
            return;
        };
        if signal.removals.len() >= 2 * signal.pruned.max(8) {
            let dead: Vec<Removal<'js>>;
            (signal.removals, dead) = std::mem::take(&mut signal.removals)
                .into_iter()
                .partition(|r| r.listener.strong_count() > 0);
            signal.pruned = signal.removals.len();
            // Dropping values can run finalizers, which never touch this.
            drop(dead);
        }
        signal.removals.push(Removal {
            target,
            ty,
            listener: Rc::downgrade(listener),
        });
    }

    /// Marks the signal aborted and takes the listeners to remove.
    fn abort(&self) -> Vec<Removal<'js>> {
        let mut signal = self.signal.borrow_mut();
        let Some(signal) = signal.as_mut() else {
            return Vec::new();
        };
        signal.aborted = true;
        std::mem::take(&mut signal.removals)
    }
}

/// An event target, as found from its object.
#[derive(Clone, Copy)]
pub(crate) enum TargetRef<'a, 'js> {
    Node(&'a NodeWrapper<'js>),
    Object(&'a TargetData<'js>),
    Global,
}

// -------------------------------------------------------------------------
// Events.

/// Bits of [`EventData::flags`].
pub(crate) mod flag {
    pub(crate) const BUBBLES: u16 = 1;
    pub(crate) const CANCELABLE: u16 = 1 << 1;
    pub(crate) const COMPOSED: u16 = 1 << 2;
    pub(crate) const CANCELED: u16 = 1 << 3;
    pub(crate) const STOP: u16 = 1 << 4;
    pub(crate) const STOP_IMMEDIATE: u16 = 1 << 5;
    pub(crate) const IN_PASSIVE: u16 = 1 << 6;
    pub(crate) const DISPATCHING: u16 = 1 << 7;
    pub(crate) const TRUSTED: u16 = 1 << 8;
}

/// `Event.eventPhase` values.
pub(crate) mod phase {
    pub(crate) const NONE: u8 = 0;
    pub(crate) const CAPTURING: u8 = 1;
    pub(crate) const AT_TARGET: u8 = 2;
    pub(crate) const BUBBLING: u8 = 3;
}

/// The Rust side of an `Event`.
pub(crate) struct EventData<'js> {
    ty: RefCell<TypeAtom<'js>>,
    flags: Cell<u16>,
    phase: Cell<u8>,
    time_stamp: f64,
    target: RefCell<Option<Value<'js>>>,
    current_target: RefCell<Option<Value<'js>>>,
    /// The path while the event is dispatched, for `composedPath()`.
    path: RefCell<Vec<Value<'js>>>,
}

impl<'js> EventData<'js> {
    pub(crate) fn new(ty: TypeAtom<'js>, flags: u16, time_stamp: f64) -> Self {
        Self {
            ty: RefCell::new(ty),
            flags: Cell::new(flags),
            phase: Cell::new(phase::NONE),
            time_stamp,
            target: RefCell::new(None),
            current_target: RefCell::new(None),
            path: RefCell::new(Vec::new()),
        }
    }

    #[inline]
    pub(crate) fn ty(&self) -> qjs::JSAtom {
        self.ty.borrow().raw()
    }

    #[inline]
    pub(crate) fn has(&self, bits: u16) -> bool {
        self.flags.get() & bits != 0
    }

    #[inline]
    pub(crate) fn set(&self, bits: u16, on: bool) {
        let f = self.flags.get();
        self.flags.set(if on { f | bits } else { f & !bits });
    }

    /// `preventDefault()`.
    pub(crate) fn prevent_default(&self) {
        if self.has(flag::CANCELABLE) && !self.has(flag::IN_PASSIVE) {
            self.set(flag::CANCELED, true);
        }
    }

    pub(crate) fn set_target(&self, target: Option<Value<'js>>) {
        // The old value goes after the borrow: freeing can run finalizers.
        drop(self.target.replace(target));
    }

    pub(crate) fn set_current_target(&self, target: Option<Value<'js>>) {
        drop(self.current_target.replace(target));
    }
}

impl<'js> Trace<'js> for EventData<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        // Only borrowed mutably for an assignment, never while the GC can
        // run; skipping one only makes its values roots.
        if let Ok(t) = self.target.try_borrow() {
            t.trace(tracer);
        }
        if let Ok(t) = self.current_target.try_borrow() {
            t.trace(tracer);
        }
        if let Ok(path) = self.path.try_borrow() {
            for v in path.iter() {
                v.trace(tracer);
            }
        }
    }
}

// SAFETY: `EventData` only holds values bound to `'js`, and `Changed` is
// the same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for EventData<'js> {
    type Changed<'to> = EventData<'to>;
}

impl<'js> JsClass<'js> for EventData<'js> {
    const NAME: &'static str = "Event";
    type Mutable = Readable;

    fn prototype(_ctx: &Ctx<'js>) -> Result<Option<Object<'js>>> {
        Ok(None)
    }

    fn constructor(_ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        Ok(None)
    }
}

// -------------------------------------------------------------------------
// Recognising objects.

/// Where a Rust class's value sits in its objects, for finding it from a
/// raw object without rquickjs: rquickjs keeps the value in a `ClassCell`,
/// a `#[repr(C)]` struct of a vtable pointer (one per Rust type) and the
/// value, which the object's opaque points at (as `Members` does for node
/// wrappers).
pub(crate) struct Layout {
    vtable: *const c_void,
    offset: usize,
}

impl Layout {
    fn learn<'js, T: JsClass<'js, Mutable = Readable>>(
        class_id: qjs::JSClassID,
        object: &Class<'js, T>,
    ) -> Self {
        // SAFETY: `object` is a live class object, so its opaque is its
        // cell, which starts with the vtable pointer.
        let cell = unsafe { qjs::JS_GetOpaque(object.as_raw(), class_id) };
        assert!(!cell.is_null(), "a class object has a cell");
        // SAFETY: as above.
        let vtable = unsafe { *cell.cast::<*const c_void>() };
        let value: *const T = &*object.borrow();
        let offset = (value as usize)
            .checked_sub(cell as usize)
            .expect("the value is inside its cell");
        Self { vtable, offset }
    }

    /// The `T` behind `value`, if it is an object of `T`'s class with the
    /// vtable learned. Cheap: a class check and a pointer compare.
    ///
    /// # Safety
    ///
    /// `value` must be alive and stay so for the result's life, and this
    /// must be `T`'s layout.
    #[inline]
    unsafe fn fast<'v, 'js, T: JsClass<'js, Mutable = Readable> + 'v>(
        &self,
        class_id: qjs::JSClassID,
        value: qjs::JSValue,
    ) -> Option<&'v T> {
        // SAFETY: `JS_GetOpaque` checks the tag and class; an object of
        // rquickjs's class has a class cell as its opaque.
        unsafe {
            let cell = qjs::JS_GetOpaque(value, class_id);
            if cell.is_null() || *cell.cast::<*const c_void>() != self.vtable {
                return None;
            }
            Some(&*cell.cast::<u8>().add(self.offset).cast::<T>())
        }
    }

    /// The `T` behind `value`, if it is an object of `T`'s class.
    ///
    /// # Safety
    ///
    /// As for [`fast`](Self::fast).
    #[inline]
    unsafe fn get<'v, 'js, T: JsClass<'js, Mutable = Readable> + 'v>(
        &self,
        ctx: *mut qjs::JSContext,
        class_id: qjs::JSClassID,
        value: qjs::JSValue,
    ) -> Option<&'v T> {
        // SAFETY: as for the function.
        if let Some(v) = unsafe { self.fast(class_id, value) } {
            return Some(v);
        }
        // SAFETY: as above.
        unsafe { Self::slow(ctx, class_id, value) }
    }

    /// [`get`](Self::get) for anything but the vtable learned: another Rust
    /// class, or `T` with a second vtable (generic code can be
    /// instantiated twice). Asks rquickjs.
    ///
    /// # Safety
    ///
    /// As for [`fast`](Self::fast), and `ctx` must be live.
    unsafe fn slow<'v, 'js, T: JsClass<'js, Mutable = Readable> + 'v>(
        ctx: *mut qjs::JSContext,
        class_id: qjs::JSClassID,
        value: qjs::JSValue,
    ) -> Option<&'v T> {
        // SAFETY: as for the function; the `Value` owns its new reference,
        // and the object outlives it as the caller promises.
        unsafe {
            if qjs::JS_GetOpaque(value, class_id).is_null() {
                return None;
            }
            let ctx = natives::ctx_of(ctx);
            let value =
                Value::from_raw(ctx.clone(), qjs::JS_DupValue(ctx.as_raw().as_ptr(), value));
            let class = Class::<T>::from_value(&value).ok()?;
            let ptr: *const T = &*class.borrow();
            Some(&*ptr)
        }
    }
}

// -------------------------------------------------------------------------
// Per-context data.

/// Atoms the natives look up properties with.
struct Atoms<'js> {
    capture: TypeAtom<'js>,
    once: TypeAtom<'js>,
    passive: TypeAtom<'js>,
    signal: TypeAtom<'js>,
    handle_event: TypeAtom<'js>,
    bubbles: TypeAtom<'js>,
    cancelable: TypeAtom<'js>,
    composed: TypeAtom<'js>,
    prototype: TypeAtom<'js>,
    load: TypeAtom<'js>,
    error: TypeAtom<'js>,
    message: TypeAtom<'js>,
    filename: TypeAtom<'js>,
    lineno: TypeAtom<'js>,
    colno: TypeAtom<'js>,
}

/// JS functions the natives call, which `events.js` registers.
#[derive(Clone)]
struct Hooks<'js> {
    /// `(element, type, source, forWindow)`: compiles a content
    /// attribute's handler body.
    compile_handler: Function<'js>,
    /// `(node)`: the slot a node is assigned to, or null.
    slot_for: Function<'js>,
    /// `(name, message)`: a new `DOMException`.
    dom_exception: Function<'js>,
    /// The `ErrorEvent` constructor.
    error_event: Value<'js>,
}

/// Which handler attributes an event type has.
struct HandlerKind<'js> {
    /// Keeps the type's atom, the map's key, alive.
    _ty: TypeAtom<'js>,
    /// The body's attribute sets the window's handler.
    window: bool,
    /// HTML, SVG and MathML elements' attributes set their own.
    element: bool,
    /// The attribute name, `on` and the type.
    attribute: Box<str>,
}

/// What the event natives need: how to recognise targets and events, the
/// global object's listeners, cached atoms and the JS hooks. Kept as
/// runtime userdata and reached through the context's opaque pointer
/// (see `native_data`).
pub(crate) struct EventsData<'js> {
    shared: Rc<Shared>,
    class_id: qjs::JSClassID,
    target_layout: Layout,
    event_layout: Layout,
    /// The global object, not counted: it lives as long as the context.
    global: *mut c_void,
    global_listeners: Listeners<'js>,
    target_proto: Object<'js>,
    event_proto: Object<'js>,
    atoms: Atoms<'js>,
    hooks: RefCell<Option<Hooks<'js>>>,
    /// The types of the `on*` accessors, by their C functions' magic.
    handler_types: RefCell<Vec<TypeAtom<'js>>>,
    handler_kinds: RefCell<HashMap<qjs::JSAtom, HandlerKind<'js>>>,
}

// SAFETY: `EventsData` only holds values bound to `'js`, and `Changed` is
// the same struct with the lifetime substituted.
unsafe impl<'js> JsLifetime<'js> for EventsData<'js> {
    type Changed<'to> = EventsData<'to>;
}

impl<'js> EventsData<'js> {
    /// The data of `ctx`, once installed.
    pub(crate) fn get<'a>(ctx: &Ctx<'js>) -> Option<&'a EventsData<'js>> {
        NativeData::of_ctx(ctx)?.events()
    }

    /// The data of the running call's context.
    ///
    /// # Safety
    ///
    /// `ctx` must be live, and `'js` its lifetime.
    #[inline]
    pub(crate) unsafe fn of<'a>(ctx: *mut qjs::JSContext) -> Option<&'a EventsData<'js>> {
        // SAFETY: as for the function.
        unsafe { NativeData::of(ctx) }?.events()
    }

    /// The target `value` is: `undefined` and `null` stand for the global
    /// object, as for the global's own operations.
    ///
    /// # Safety
    ///
    /// `value` must be alive and stay so for the result's life.
    #[inline]
    pub(crate) unsafe fn target<'a>(
        &self,
        ctx: *mut qjs::JSContext,
        value: qjs::JSValue,
    ) -> Option<TargetRef<'a, 'js>> {
        // SAFETY: plain tag reads.
        let tag = unsafe { qjs::JS_VALUE_GET_TAG(value) };
        if tag == qjs::JS_TAG_UNDEFINED || tag == qjs::JS_TAG_NULL {
            return Some(TargetRef::Global);
        }
        if tag != qjs::JS_TAG_OBJECT {
            return None;
        }
        // SAFETY: as above.
        if unsafe { qjs::JS_VALUE_GET_PTR(value) } == self.global {
            return Some(TargetRef::Global);
        }
        // SAFETY: as for the function. The cheap checks go first; a slow
        // one only runs for objects that are neither (or have a second
        // vtable).
        unsafe {
            if let Some(data) = self.target_layout.fast(self.class_id, value) {
                return Some(TargetRef::Object(data));
            }
            if let Some(members) = NativeData::of(ctx).and_then(NativeData::members)
                && let Some(wrapper) = members.wrapper(ctx, value)
            {
                let wrapper: *const NodeWrapper<'_> = wrapper;
                return Some(TargetRef::Node(&*wrapper.cast()));
            }
            Layout::slow::<TargetData<'js>>(ctx, self.class_id, value).map(TargetRef::Object)
        }
    }

    /// The event `value` is.
    ///
    /// # Safety
    ///
    /// `value` must be alive and stay so for the result's life.
    #[inline]
    pub(crate) unsafe fn event<'a>(
        &self,
        ctx: *mut qjs::JSContext,
        value: qjs::JSValue,
    ) -> Option<&'a EventData<'js>> {
        // SAFETY: as for the function.
        unsafe { self.event_layout.get(ctx, self.class_id, value) }
    }

    /// The listeners of a target.
    pub(crate) fn listeners<'a>(&'a self, target: TargetRef<'a, 'js>) -> &'a Listeners<'js> {
        match target {
            TargetRef::Node(w) => w.listeners(),
            TargetRef::Object(t) => &t.listeners,
            TargetRef::Global => &self.global_listeners,
        }
    }

    fn hooks(&self) -> Option<Hooks<'js>> {
        self.hooks.borrow().clone()
    }

    fn is_global(&self, value: &Value<'js>) -> bool {
        value.is_object()
            // SAFETY: plain payload read of an object value.
            && unsafe { qjs::JS_VALUE_GET_PTR(value.as_raw()) } == self.global
    }
}

/// An atom for `s`, holding a reference.
fn atom<'js>(ctx: &Ctx<'js>, s: &str) -> Result<TypeAtom<'js>> {
    // SAFETY: `ctx` is live, and the pointer and length are `s`'s.
    let raw = unsafe {
        qjs::JS_NewAtomLen(
            ctx.as_raw().as_ptr(),
            s.as_ptr().cast(),
            s.len() as qjs::size_t,
        )
    };
    if raw == qjs::JS_ATOM_NULL {
        return Err(rquickjs::Error::Exception);
    }
    Ok(TypeAtom::own(ctx, raw))
}

/// Installs the event natives as `native.events`.
pub(crate) fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    shared: &Rc<Shared>,
) -> Result<()> {
    let probe = Object::new(ctx.clone())?;
    let target = Class::instance_proto(TargetData::default(), probe.clone())?;
    // SAFETY: a live object.
    let class_id = unsafe { qjs::JS_GetClassID(target.as_raw()) };
    let target_layout = Layout::learn(class_id, &target);
    let ty = atom(ctx, "")?;
    let event = Class::instance_proto(EventData::new(ty, 0, 0.0), probe)?;
    let event_layout = Layout::learn(class_id, &event);
    drop((target, event));

    let globals = ctx.globals();
    let atoms = Atoms {
        capture: atom(ctx, "capture")?,
        once: atom(ctx, "once")?,
        passive: atom(ctx, "passive")?,
        signal: atom(ctx, "signal")?,
        handle_event: atom(ctx, "handleEvent")?,
        bubbles: atom(ctx, "bubbles")?,
        cancelable: atom(ctx, "cancelable")?,
        composed: atom(ctx, "composed")?,
        prototype: atom(ctx, "prototype")?,
        load: atom(ctx, "load")?,
        error: atom(ctx, "error")?,
        message: atom(ctx, "message")?,
        filename: atom(ctx, "filename")?,
        lineno: atom(ctx, "lineno")?,
        colno: atom(ctx, "colno")?,
    };
    let data = EventsData {
        shared: Rc::clone(shared),
        class_id,
        target_layout,
        event_layout,
        // SAFETY: plain payload read of an object value.
        global: unsafe { qjs::JS_VALUE_GET_PTR(globals.as_raw()) },
        global_listeners: Listeners::default(),
        target_proto: Object::new(ctx.clone())?,
        event_proto: Object::new(ctx.clone())?,
        atoms,
        hooks: RefCell::new(None),
        handler_types: RefCell::default(),
        handler_kinds: RefCell::default(),
    };
    if ctx.store_userdata(data).is_err() {
        return Err(rquickjs::Error::Unknown);
    }
    let data = ctx
        .userdata::<EventsData<'js>>()
        .ok_or(rquickjs::Error::Unknown)?;
    let ptr: *const EventsData<'js> = &*data;
    drop(data);
    NativeData::of_ctx(ctx)
        .ok_or(rquickjs::Error::Unknown)?
        .set_events(ptr);
    // SAFETY: the userdata stays put until the runtime is dropped.
    let data = unsafe { &*ptr };
    let events = natives::install(ctx, data)?;
    native.set("events", events)
}

#[cfg(test)]
mod tests {
    use dasher_dom::Document;

    use crate::{ClockMode, EventTarget, JsRuntime, RuntimeOptions};

    /// The host firing an event nobody listens for makes no wrapper and no
    /// event object, and runs no JS.
    #[test]
    fn firing_into_silence_costs_nothing() {
        let js = JsRuntime::new(RuntimeOptions {
            clock: ClockMode::Manual,
            ..RuntimeOptions::default()
        })
        .unwrap();
        js.set_document(Document::parse("<div><p><span id=s>x</span></p></div>"));
        js.run_script("addEventListener('other', () => {});", "a.js")
            .unwrap();
        let span = js.with_document(|doc| doc.get_element_by_id(doc.root(), "s").unwrap());
        let dom = std::rc::Rc::clone(&js.shared().dom);
        let before = js.memory_allocated();
        for ty in ["load", "error", "ping"] {
            assert!(js.fire_event(EventTarget::Node(span), ty, true, false));
        }
        assert!(js.fire_event(EventTarget::Window, "load", false, false));
        assert!(js.fire_event(EventTarget::Document, "DOMContentLoaded", true, false));
        assert_eq!(js.memory_allocated(), before);
        assert!(dom.wrapper_raw(span).is_none());

        // A listener at the window hears a bubbling event from the span,
        // which then gets its wrapper.
        js.run_script("var heard = []; addEventListener('ping', (e) => heard.push(e.target.id, e.isTrusted));", "b.js")
            .unwrap();
        assert!(js.fire_event(EventTarget::Node(span), "ping", true, false));
        assert!(dom.wrapper_raw(span).is_some());
        assert_eq!(
            js.eval_json("heard").unwrap().as_deref(),
            Some(r#"["s",true]"#)
        );
    }
}
