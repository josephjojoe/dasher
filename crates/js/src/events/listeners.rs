//! The event listener list of one target, kept in Rust.
//!
//! A target's listeners are grouped by event type. Each type has its list
//! of listeners in the order they were added and the slot of its event
//! handler (`onclick` and friends), whose listener sits in the list once
//! the handler is set.
//!
//! # Dispatch and changes during it
//!
//! Dispatch takes a snapshot of the list at each target it reaches, as the
//! spec does: listeners added after that do not run there, and removed ones
//! are skipped through their `removed` flag. A snapshot is a clone of the
//! list's `Rc`, and a change made while one is out copies the list first
//! (`Rc::make_mut`), so dispatch allocates nothing unless listeners change
//! under it.
//!
//! # Garbage collection
//!
//! The JS values a listener holds (its callback, or a handler's function
//! and uncompiled attribute) are reported to the GC by the owner of the
//! list: the node wrapper or event target object it lives in (see
//! [`Listeners::trace`]). Each listener is reported once, through the list
//! it is in; a listener only a snapshot still holds is not reported, which
//! only makes its values roots until the snapshot goes. So callbacks live
//! exactly as long as their target's wrapper group, and a listener that
//! refers back to its target is a cycle the GC can collect. The listeners
//! of the global object are held by the context's events data and live as
//! long as the page.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;

use rquickjs::class::{Trace, Tracer};
use rquickjs::{Ctx, Value, qjs};

/// An event type, as an atom this holds a reference to. It keeps the
/// runtime, not the context: a context reference held by an object would
/// have to be reported to the GC.
pub(crate) struct TypeAtom<'js> {
    atom: qjs::JSAtom,
    rt: *mut qjs::JSRuntime,
    _ctx: PhantomData<Ctx<'js>>,
}

impl<'js> TypeAtom<'js> {
    /// Takes a new reference to `atom`.
    pub(crate) fn dup(ctx: &Ctx<'js>, atom: qjs::JSAtom) -> Self {
        // SAFETY: `ctx` is live and `atom` a valid atom of its runtime.
        unsafe { qjs::JS_DupAtom(ctx.as_raw().as_ptr(), atom) };
        Self::own(ctx, atom)
    }

    /// Takes over a reference to `atom` the caller owns.
    pub(crate) fn own(ctx: &Ctx<'js>, atom: qjs::JSAtom) -> Self {
        Self {
            atom,
            // SAFETY: `ctx` is live.
            rt: unsafe { qjs::JS_GetRuntime(ctx.as_raw().as_ptr()) },
            _ctx: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn raw(&self) -> qjs::JSAtom {
        self.atom
    }

    /// The type as a JS string.
    pub(crate) fn to_value(&self, ctx: &Ctx<'js>) -> Value<'js> {
        // SAFETY: the atom is live; the string is a new reference.
        unsafe {
            let raw = qjs::JS_AtomToString(ctx.as_raw().as_ptr(), self.atom);
            Value::from_raw(ctx.clone(), raw)
        }
    }
}

impl Drop for TypeAtom<'_> {
    fn drop(&mut self) {
        // SAFETY: this holds a reference to the atom, in a runtime that
        // outlives every JS value.
        unsafe { qjs::JS_FreeAtomRT(self.rt, self.atom) };
    }
}

/// What runs when a listener is invoked.
pub(crate) enum Callback<'js> {
    /// A function, or an object with a `handleEvent` method.
    Value(Value<'js>),
    /// The target's event handler for the type: its listener.
    Handler(Handler<'js>),
}

/// An event handler's value: a function, or the body of a content
/// attribute compiled the first time the handler runs or is read.
#[derive(Default)]
pub(crate) struct Handler<'js> {
    pub(crate) value: RefCell<Option<Value<'js>>>,
    /// The element the attribute is on, and its value, until compiled.
    pub(crate) source: RefCell<Option<(Value<'js>, Value<'js>)>>,
}

/// One event listener.
pub(crate) struct Listener<'js> {
    pub(crate) callback: Callback<'js>,
    pub(crate) capture: bool,
    pub(crate) once: bool,
    pub(crate) passive: bool,
    removed: Cell<bool>,
}

impl<'js> Listener<'js> {
    pub(crate) fn new(callback: Callback<'js>, capture: bool, once: bool, passive: bool) -> Self {
        Self {
            callback,
            capture,
            once,
            passive,
            removed: Cell::new(false),
        }
    }

    #[inline]
    pub(crate) fn removed(&self) -> bool {
        self.removed.get()
    }

    /// The handler, for a handler's listener.
    pub(crate) fn handler(&self) -> Option<&Handler<'js>> {
        match &self.callback {
            Callback::Handler(h) => Some(h),
            Callback::Value(_) => None,
        }
    }

    fn is(&self, callback: qjs::JSValue, capture: bool) -> bool {
        self.capture == capture
            && matches!(&self.callback, Callback::Value(v) if same_value(v.as_raw(), callback))
    }
}

impl<'js> Trace<'js> for Listener<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        match &self.callback {
            Callback::Value(v) => v.trace(tracer),
            Callback::Handler(h) => {
                // Only ever borrowed mutably for an assignment, never while
                // the GC can run; skipping one only makes its values roots.
                if let Ok(value) = h.value.try_borrow() {
                    value.trace(tracer);
                }
                if let Ok(source) = h.source.try_borrow()
                    && let Some((owner, text)) = &*source
                {
                    owner.trace(tracer);
                    text.trace(tracer);
                }
            }
        }
    }
}

/// Whether two values are the same object (or the same primitive bits).
fn same_value(a: qjs::JSValue, b: qjs::JSValue) -> bool {
    // SAFETY: reading the tag and payload of plain values.
    unsafe {
        qjs::JS_VALUE_GET_TAG(a) == qjs::JS_VALUE_GET_TAG(b)
            && qjs::JS_VALUE_GET_PTR(a) == qjs::JS_VALUE_GET_PTR(b)
    }
}

/// A snapshot of one type's listeners, for dispatch.
pub(crate) type Snapshot<'js> = Rc<Vec<Rc<Listener<'js>>>>;

/// Where a type's event handler stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HandlerState {
    /// Never set or looked at: a content attribute the element was made
    /// with may still set it.
    Unseen,
    /// Set to null, or its attribute removed.
    Inactive,
    Active,
}

enum Slot<'js> {
    Unseen,
    Inactive,
    /// The handler's listener, which is in the list.
    Active(Rc<Listener<'js>>),
}

struct TypeEntry<'js> {
    ty: TypeAtom<'js>,
    list: Snapshot<'js>,
    handler: Slot<'js>,
}

/// A target's event listeners. Empty, it is an empty `Vec`: nothing is
/// allocated for a target until it gets a listener or handler.
#[derive(Default)]
pub(crate) struct Listeners<'js> {
    types: RefCell<Vec<TypeEntry<'js>>>,
}

impl<'js> Listeners<'js> {
    /// Runs `f` on the entry for `ty`, made if `ctx` is given and there is
    /// none. The list is borrowed while `f` runs: it must not run JS.
    fn with_entry<R>(
        &self,
        ty: qjs::JSAtom,
        create: Option<&Ctx<'js>>,
        f: impl FnOnce(&mut TypeEntry<'js>) -> R,
    ) -> Option<R> {
        let mut types = self.types.borrow_mut();
        let i = match types.iter().position(|e| e.ty.raw() == ty) {
            Some(i) => i,
            None => {
                let ctx = create?;
                types.push(TypeEntry {
                    ty: TypeAtom::dup(ctx, ty),
                    list: Rc::default(),
                    handler: Slot::Unseen,
                });
                types.len() - 1
            }
        };
        Some(f(&mut types[i]))
    }

    /// Adds a listener for `callback`, unless one with the same callback
    /// and capture is there. Returns the listener added.
    pub(crate) fn add(
        &self,
        ctx: &Ctx<'js>,
        ty: qjs::JSAtom,
        callback: qjs::JSValue,
        capture: bool,
        once: bool,
        passive: bool,
    ) -> Option<Rc<Listener<'js>>> {
        self.with_entry(ty, Some(ctx), |entry| {
            if entry.list.iter().any(|l| l.is(callback, capture)) {
                return None;
            }
            // SAFETY: `callback` is an argument of the running call; the
            // `Value` owns the new reference.
            let value = unsafe {
                Value::from_raw(
                    ctx.clone(),
                    qjs::JS_DupValue(ctx.as_raw().as_ptr(), callback),
                )
            };
            let listener = Rc::new(Listener::new(
                Callback::Value(value),
                capture,
                once,
                passive,
            ));
            Rc::make_mut(&mut entry.list).push(Rc::clone(&listener));
            Some(listener)
        })
        .flatten()
    }

    /// Removes the listener for `callback` and `capture`, if there is one.
    pub(crate) fn remove_matching(&self, ty: qjs::JSAtom, callback: qjs::JSValue, capture: bool) {
        let removed = self
            .with_entry(ty, None, |entry| {
                let i = entry.list.iter().position(|l| l.is(callback, capture))?;
                Some(Rc::make_mut(&mut entry.list).remove(i))
            })
            .flatten();
        if let Some(listener) = &removed {
            listener.removed.set(true);
        }
        // The listener, and with it its callback, goes with the borrow let
        // go: freeing a value can run finalizers.
        drop(removed);
    }

    /// Removes `listener` from the list for `ty`, if it is there, and
    /// marks it removed.
    pub(crate) fn remove(&self, ty: qjs::JSAtom, listener: &Rc<Listener<'js>>) {
        listener.removed.set(true);
        let removed = self
            .with_entry(ty, None, |entry| {
                let i = entry.list.iter().position(|l| Rc::ptr_eq(l, listener))?;
                Some(Rc::make_mut(&mut entry.list).remove(i))
            })
            .flatten();
        drop(removed);
    }

    /// The listeners for `ty` now, or `None` if there are none.
    #[inline]
    pub(crate) fn snapshot(&self, ty: qjs::JSAtom) -> Option<Snapshot<'js>> {
        let types = self.types.borrow();
        let entry = types.iter().find(|e| e.ty.raw() == ty)?;
        (!entry.list.is_empty()).then(|| Rc::clone(&entry.list))
    }

    /// Whether there is any listener for `ty`.
    #[inline]
    pub(crate) fn has(&self, ty: qjs::JSAtom) -> bool {
        let types = self.types.borrow();
        types.iter().any(|e| e.ty.raw() == ty && !e.list.is_empty())
    }

    pub(crate) fn handler_state(&self, ty: qjs::JSAtom) -> HandlerState {
        let types = self.types.borrow();
        match types.iter().find(|e| e.ty.raw() == ty).map(|e| &e.handler) {
            None | Some(Slot::Unseen) => HandlerState::Unseen,
            Some(Slot::Inactive) => HandlerState::Inactive,
            Some(Slot::Active(_)) => HandlerState::Active,
        }
    }

    /// The listener of the handler for `ty`, if it is set.
    pub(crate) fn handler(&self, ty: qjs::JSAtom) -> Option<Rc<Listener<'js>>> {
        let types = self.types.borrow();
        match &types.iter().find(|e| e.ty.raw() == ty)?.handler {
            Slot::Active(l) => Some(Rc::clone(l)),
            _ => None,
        }
    }

    /// The handler for `ty`'s listener, made and added (first if `first`,
    /// else last) if the handler is not set. The caller sets its value.
    pub(crate) fn activate_handler(
        &self,
        ctx: &Ctx<'js>,
        ty: qjs::JSAtom,
        first: bool,
    ) -> Rc<Listener<'js>> {
        self.with_entry(ty, Some(ctx), |entry| {
            if let Slot::Active(l) = &entry.handler {
                return Rc::clone(l);
            }
            let listener = Rc::new(Listener::new(
                Callback::Handler(Handler::default()),
                false,
                false,
                false,
            ));
            let list = Rc::make_mut(&mut entry.list);
            if first {
                list.insert(0, Rc::clone(&listener));
            } else {
                list.push(Rc::clone(&listener));
            }
            entry.handler = Slot::Active(Rc::clone(&listener));
            listener
        })
        .expect("made with a context")
    }

    /// Unsets the handler for `ty`, taking its listener out of the list.
    pub(crate) fn deactivate_handler(&self, ctx: &Ctx<'js>, ty: qjs::JSAtom) {
        let old = self.with_entry(ty, Some(ctx), |entry| {
            match std::mem::replace(&mut entry.handler, Slot::Inactive) {
                Slot::Active(l) => {
                    let list = Rc::make_mut(&mut entry.list);
                    list.retain(|x| !Rc::ptr_eq(x, &l));
                    Some(l)
                }
                _ => None,
            }
        });
        if let Some(Some(listener)) = old {
            listener.removed.set(true);
            if let Some(h) = listener.handler() {
                let value = h.value.take();
                let source = h.source.take();
                drop((value, source));
            }
        }
    }

    /// Reports every listener's values to the GC. See the module docs.
    pub(crate) fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        // Only borrowed mutably while nothing allocates on the JS heap.
        let Ok(types) = self.types.try_borrow() else {
            return;
        };
        for entry in types.iter() {
            for listener in entry.list.iter() {
                listener.trace(tracer);
            }
        }
    }
}
