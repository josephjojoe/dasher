//! Event dispatch: the event path, retargeting, invoking listeners, and
//! event handlers.
//!
//! <https://dom.spec.whatwg.org/#concept-event-dispatch>
//!
//! # The path
//!
//! For a node, the path is worked out on node ids: each node's parent, or
//! the slot it is assigned to when its parent is a shadow host (found by
//! `dom.js`, which keeps manual slot assignments), a shadow root's host for
//! composed events and those that came into the shadow tree through a slot,
//! and the window after the page's document (except for `load`). Only then
//! do the nodes get wrappers, the values listeners see. The targets
//! listeners outside a shadow tree see are worked out the same way
//! (<https://dom.spec.whatwg.org/#retarget>). Any other target's path is
//! the target alone.
//!
//! Before the host or the glue makes an event to fire, [`reaches`] walks
//! the same path on ids, without making wrappers, and looks for a listener
//! or handler of the type, or a content attribute that would set one. With
//! none, nothing more happens: no event object, no wrappers, no JS.
//!
//! # Event handlers
//!
//! A handler is a listener of its own kind in the target's list (see the
//! `listeners` module). The `on*` content attributes an element was made
//! with (by the parser, `innerHTML` or cloning) are picked up the first time
//! the handler is needed, at dispatch or through the IDL attribute, and go
//! first in the list, since they were there before any script could add a
//! listener. Attributes changed later reach the handler through
//! `attributeHandlerChanged`, which `dom.js` calls. A handler's source is
//! compiled (by `dom.js`, which builds its scope chain) the first time it
//! runs or is read.

use std::rc::Rc;

use dasher_dom::{Document, Name, NodeId, NodeKind};
use rquickjs::{Ctx, Exception, Result, Value, qjs};

use super::listeners::{Callback, Handler, HandlerState, Listener, Listeners, TypeAtom};
use super::{EventData, EventsData, TargetRef, atom, flag, phase};
use crate::dom::{DomState, wrap};

/// A step of a node's event path: a node, or the global object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hop {
    Node(NodeId),
    Global,
}

/// Where the host fires an event.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HostTarget {
    Window,
    /// A node of the page's document, or the document itself.
    Node(NodeId),
}

enum Step {
    To(Hop),
    End,
    /// The node's parent (given) is a shadow host: the node's next step is
    /// the slot it is assigned to, if any, else its parent.
    AskSlot(NodeId),
}

/// The step after `n` on an event path ("get the parent").
fn step(doc: &Document, n: NodeId, composed: bool, origin_root: NodeId, is_load: bool) -> Step {
    let parent = doc.parent(n);
    if let Some(p) = parent
        && doc.has_shadow_roots()
        && matches!(doc.kind(n), NodeKind::Element | NodeKind::Text)
        && doc.is_element(p)
        && doc.shadow_root(p).is_some()
    {
        return Step::AskSlot(p);
    }
    if let Some(p) = parent {
        return Step::To(Hop::Node(p));
    }
    if n == doc.root() {
        return if is_load {
            Step::End
        } else {
            Step::To(Hop::Global)
        };
    }
    // Composed events leave a shadow tree for its host, as do events that
    // came into it through a slot.
    if doc.kind(n) == NodeKind::DocumentFragment
        && let Some(host) = doc.shadow_host(n)
        && (composed || origin_root != n)
    {
        return Step::To(Hop::Node(host));
    }
    Step::End
}

/// `a` retargeted against `b` (`None` for the global object).
fn retarget(doc: &Document, mut a: NodeId, b: Option<NodeId>) -> NodeId {
    loop {
        let root = doc.tree_root(a);
        let Some(host) = doc.shadow_host(root) else {
            return a;
        };
        if let Some(b) = b
            && shadow_including_inclusive_ancestor(doc, root, b)
        {
            return a;
        }
        a = host;
    }
}

fn shadow_including_inclusive_ancestor(doc: &Document, ancestor: NodeId, mut node: NodeId) -> bool {
    loop {
        if doc.contains(ancestor, node) {
            return true;
        }
        match doc.shadow_host(doc.tree_root(node)) {
            Some(host) => node = host,
            None => return false,
        }
    }
}

fn is_body_like(doc: &Document, el: NodeId) -> bool {
    doc.is_html(el, Name::BODY) || doc.is_html(el, Name::FRAMESET)
}

impl<'js> EventsData<'js> {
    /// The content attribute that sets the handler for `ty` on `hop`, as
    /// the element it is on and its value: for the window, the body's
    /// window-reflecting ones; for an HTML, SVG or MathML element, its
    /// own.
    fn handler_attribute(
        &self,
        doc: &Document,
        hop: Hop,
        ty: qjs::JSAtom,
    ) -> Option<(NodeId, String)> {
        let kinds = self.handler_kinds.borrow();
        let kind = kinds.get(&ty)?;
        let el = match hop {
            Hop::Global => {
                if !kind.window {
                    return None;
                }
                doc.body()?
            }
            Hop::Node(el) => {
                if !kind.element || !doc.is_element(el) {
                    return None;
                }
                let ns = doc.namespace(el);
                if ns != Some(Name::NS_HTML)
                    && ns != Some(Name::NS_SVG)
                    && ns != Some(Name::NS_MATHML)
                {
                    return None;
                }
                // The body's window-reflecting attributes are the window's.
                if kind.window && is_body_like(doc, el) {
                    return None;
                }
                el
            }
        };
        let name = doc.names().get(&kind.attribute)?;
        let attr = doc
            .attrs(el)
            .iter()
            .find(|a| a.local() == name && doc.attr_prefix(a) == Name::EMPTY)?;
        Some((el, attr.value().to_owned()))
    }

    /// Whether `listeners` (of `hop`, if a node or the window) has a
    /// listener for `ty`, or would get a handler from a content attribute.
    fn listening(
        &self,
        doc: &Document,
        listeners: &Listeners<'js>,
        hop: Option<Hop>,
        ty: qjs::JSAtom,
    ) -> bool {
        listeners.has(ty)
            || (listeners.handler_state(ty) == HandlerState::Unseen
                && hop.is_some_and(|hop| self.handler_attribute(doc, hop, ty).is_some()))
    }

    fn hop_listening(
        &self,
        ctx: *mut qjs::JSContext,
        doc: &Document,
        hop: Hop,
        ty: qjs::JSAtom,
    ) -> bool {
        match hop {
            Hop::Global => self.listening(doc, &self.global_listeners, Some(hop), ty),
            Hop::Node(n) => match self.shared.dom.wrapper_raw(n) {
                // SAFETY: a live wrapper from the cache, and nothing runs
                // while it is looked at.
                Some(raw) => match unsafe { self.target(ctx, raw) } {
                    Some(TargetRef::Node(w)) => self.listening(doc, w.listeners(), Some(hop), ty),
                    _ => false,
                },
                None => self.handler_attribute(doc, hop, ty).is_some(),
            },
        }
    }

    /// Whether an event of type `ty` fired at node `start` could reach any
    /// listener (see the module docs). Errs on the side of yes.
    fn reaches_node(&self, ctx: *mut qjs::JSContext, start: NodeId, ty: qjs::JSAtom) -> bool {
        let doc = self.shared.dom.doc();
        let is_load = ty == self.atoms.load.raw();
        let mut hop = Hop::Node(start);
        loop {
            if self.hop_listening(ctx, &doc, hop, ty) {
                return true;
            }
            let Hop::Node(n) = hop else {
                return false;
            };
            match step(&doc, n, true, n, is_load) {
                Step::End => return false,
                Step::To(next) => hop = next,
                Step::AskSlot(_) => return true,
            }
        }
    }

    /// Whether an event of type `ty` fired at `target` could reach any
    /// listener.
    pub(super) fn reaches_target(
        &self,
        ctx: *mut qjs::JSContext,
        target: TargetRef<'_, 'js>,
        ty: qjs::JSAtom,
    ) -> bool {
        match target {
            TargetRef::Node(w) => match w.id() {
                Some(id) => self.reaches_node(ctx, id, ty),
                None => w.listeners().has(ty),
            },
            TargetRef::Object(t) => t.listeners.has(ty),
            TargetRef::Global => {
                let doc = self.shared.dom.doc();
                self.hop_listening(ctx, &doc, Hop::Global, ty)
            }
        }
    }
}

/// Whether an event named `ty` fired at `target` could reach a listener.
pub(crate) fn reaches(ctx: &Ctx<'_>, target: HostTarget, ty: &str) -> bool {
    let Some(data) = EventsData::get(ctx) else {
        return true;
    };
    let Ok(ty) = atom(ctx, ty) else {
        return true;
    };
    let raw = ctx.as_raw().as_ptr();
    match target {
        HostTarget::Window => data.reaches_target(raw, TargetRef::Global, ty.raw()),
        HostTarget::Node(id) => data.reaches_node(raw, id, ty.raw()),
    }
}

/// Fires a trusted plain `Event` named `ty` at `target`, if any listener
/// could hear it. Returns whether no listener canceled it.
pub(crate) fn fire<'js>(
    ctx: &Ctx<'js>,
    target: HostTarget,
    ty: &str,
    bubbles: bool,
    cancelable: bool,
) -> Result<bool> {
    let Some(data) = EventsData::get(ctx) else {
        return Ok(true);
    };
    let ty = atom(ctx, ty)?;
    let raw = ctx.as_raw().as_ptr();
    let target = match target {
        HostTarget::Window => {
            if !data.reaches_target(raw, TargetRef::Global, ty.raw()) {
                return Ok(true);
            }
            ctx.globals().into_value()
        }
        HostTarget::Node(id) => {
            if !data.reaches_node(raw, id, ty.raw()) {
                return Ok(true);
            }
            wrap(ctx, &data.shared.dom, id)?.into_value()
        }
    };
    let mut flags = 0;
    if bubbles {
        flags |= flag::BUBBLES;
    }
    if cancelable {
        flags |= flag::CANCELABLE;
    }
    let event = data.new_event(ty, flags, None)?;
    // SAFETY: `event` is a live event object.
    let ev = unsafe { data.event(raw, event.as_raw()) }.ok_or(rquickjs::Error::Unknown)?;
    dispatch(ctx, data, &target, &event, ev, true)
}

/// One target on an event's path.
struct Entry<'js> {
    value: Value<'js>,
    /// Its listeners, inside `value`'s object (or the events data).
    listeners: *const Listeners<'js>,
    /// The node or global it is, for handler attributes.
    hop: Option<Hop>,
    /// The target its listeners see, when the event is retargeted.
    target: Option<Value<'js>>,
    /// Whether it is the target, or a host the target is retargeted to.
    at_target: bool,
}

impl<'js> EventsData<'js> {
    fn entry(&self, ctx: &Ctx<'js>, value: Value<'js>, hop: Option<Hop>) -> Entry<'js> {
        // SAFETY: `value` is alive, and kept with the entry.
        let listeners = match unsafe { self.target(ctx.as_raw().as_ptr(), value.as_raw()) } {
            Some(t) => std::ptr::from_ref(self.listeners(t)),
            None => std::ptr::null(),
        };
        Entry {
            value,
            listeners,
            hop,
            target: None,
            at_target: false,
        }
    }

    fn hop_value(&self, ctx: &Ctx<'js>, hop: Hop) -> Result<Value<'js>> {
        Ok(match hop {
            Hop::Node(id) => wrap(ctx, &self.shared.dom, id)?.into_value(),
            Hop::Global => ctx.globals().into_value(),
        })
    }

    /// The slot `node`'s event path goes through, from `dom.js`.
    fn slot_for(&self, ctx: &Ctx<'js>, dom: &Rc<DomState>, node: NodeId) -> Result<Option<NodeId>> {
        let Some(hooks) = self.hooks() else {
            return Ok(None);
        };
        let node = wrap(ctx, dom, node)?;
        let slot: Value<'js> = hooks.slot_for.call((node,))?;
        // SAFETY: `slot` is alive while looked at.
        Ok(
            match unsafe { self.target(ctx.as_raw().as_ptr(), slot.as_raw()) } {
                Some(TargetRef::Node(w)) => w.id(),
                _ => None,
            },
        )
    }

    /// The event path of `target`, with retargeting worked out.
    fn path(
        &self,
        ctx: &Ctx<'js>,
        target: &Value<'js>,
        ev: &EventData<'js>,
    ) -> Result<Vec<Entry<'js>>> {
        let raw = ctx.as_raw().as_ptr();
        // SAFETY: `target` is alive while looked at.
        let start = match unsafe { self.target(raw, target.as_raw()) } {
            Some(TargetRef::Node(w)) => w.id(),
            Some(TargetRef::Global) => {
                let mut e = self.entry(ctx, target.clone(), Some(Hop::Global));
                e.at_target = true;
                return Ok(vec![e]);
            }
            _ => None,
        };
        let Some(start) = start else {
            let mut e = self.entry(ctx, target.clone(), None);
            e.at_target = true;
            return Ok(vec![e]);
        };
        let dom = &self.shared.dom;
        let composed = ev.has(flag::COMPOSED);
        let is_load = ev.ty() == self.atoms.load.raw();
        let origin_root = dom.doc().tree_root(start);
        let mut hops = vec![Hop::Node(start)];
        let mut n = start;
        loop {
            let s = step(&dom.doc(), n, composed, origin_root, is_load);
            let next = match s {
                Step::End => break,
                Step::To(hop) => hop,
                Step::AskSlot(parent) => Hop::Node(self.slot_for(ctx, dom, n)?.unwrap_or(parent)),
            };
            hops.push(next);
            match next {
                Hop::Node(m) => n = m,
                Hop::Global => break,
            }
        }
        // Listeners outside a shadow tree see its host as the target.
        let retargets: Option<Vec<NodeId>> = {
            let doc = dom.doc();
            doc.shadow_host(origin_root).is_some().then(|| {
                hops.iter()
                    .map(|&h| {
                        let b = match h {
                            Hop::Node(b) => Some(b),
                            Hop::Global => None,
                        };
                        retarget(&doc, start, b)
                    })
                    .collect()
            })
        };
        let mut entries = Vec::with_capacity(hops.len());
        for (i, &hop) in hops.iter().enumerate() {
            let value = if i == 0 {
                target.clone()
            } else {
                self.hop_value(ctx, hop)?
            };
            let mut e = self.entry(ctx, value, Some(hop));
            e.at_target = i == 0;
            if let Some(retargets) = &retargets {
                let t = retargets[i];
                e.at_target |= hop == Hop::Node(t);
                e.target = Some(if t == start {
                    target.clone()
                } else {
                    wrap(ctx, dom, t)?.into_value()
                });
            }
            entries.push(e);
        }
        Ok(entries)
    }
}

/// Dispatches `event` (whose data is `ev`) at `target` through the capture,
/// target and bubble phases. Returns whether no listener canceled it. An
/// error is an exception that must not be caught, such as an interrupt;
/// anything listeners throw is reported and dispatch goes on.
pub(crate) fn dispatch<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    target: &Value<'js>,
    event: &Value<'js>,
    ev: &EventData<'js>,
    trusted: bool,
) -> Result<bool> {
    ev.set(flag::TRUSTED, trusted);
    ev.set(flag::DISPATCHING, true);
    ev.set_target(Some(target.clone()));
    let result = data.path(ctx, target, ev).and_then(|entries| {
        let path: Vec<Value<'js>> = entries.iter().map(|e| e.value.clone()).collect();
        drop(ev.path.replace(path));
        let retargeted = entries.first().is_some_and(|e| e.target.is_some());
        let result = run_phases(ctx, data, &entries, event, ev);
        if retargeted {
            // An event from inside a shadow tree does not give it away
            // afterwards.
            ev.set_target(None);
        }
        result
    });
    ev.phase.set(phase::NONE);
    ev.set_current_target(None);
    ev.set(flag::DISPATCHING | flag::STOP | flag::STOP_IMMEDIATE, false);
    drop(ev.path.take());
    result.map(|()| !ev.has(flag::CANCELED))
}

fn run_phases<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    entries: &[Entry<'js>],
    event: &Value<'js>,
    ev: &EventData<'js>,
) -> Result<()> {
    for e in entries.iter().rev() {
        if ev.has(flag::STOP) {
            return Ok(());
        }
        if let Some(t) = &e.target {
            ev.set_target(Some(t.clone()));
        }
        let p = if e.at_target {
            phase::AT_TARGET
        } else {
            phase::CAPTURING
        };
        invoke(ctx, data, e, event, ev, p, true)?;
    }
    let bubbles = ev.has(flag::BUBBLES);
    for e in entries {
        if ev.has(flag::STOP) {
            return Ok(());
        }
        if !e.at_target && !bubbles {
            continue;
        }
        if let Some(t) = &e.target {
            ev.set_target(Some(t.clone()));
        }
        let p = if e.at_target {
            phase::AT_TARGET
        } else {
            phase::BUBBLING
        };
        invoke(ctx, data, e, event, ev, p, false)?;
    }
    Ok(())
}

/// Runs `entry`'s listeners for the phase (capture listeners, or the
/// others).
fn invoke<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    entry: &Entry<'js>,
    event: &Value<'js>,
    ev: &EventData<'js>,
    phase: u8,
    capture: bool,
) -> Result<()> {
    // SAFETY: the listeners live in `entry.value`'s object, or in `data`.
    let Some(listeners) = (unsafe { entry.listeners.as_ref() }) else {
        return Ok(());
    };
    let ty = ev.ty();
    // Handlers are bubble-phase listeners; one set by a content attribute
    // nothing has read yet joins the list here.
    if !capture {
        pick_up_attribute(ctx, data, listeners, entry.hop, ty)?;
    }
    let Some(list) = listeners.snapshot(ty) else {
        return Ok(());
    };
    ev.set_current_target(Some(entry.value.clone()));
    ev.phase.set(phase);
    for listener in list.iter() {
        if listener.removed() || listener.capture != capture {
            continue;
        }
        if listener.once {
            listeners.remove(ty, listener);
        }
        ev.set(flag::IN_PASSIVE, listener.passive);
        let result = call_listener(ctx, data, listener, entry, event, ev, ty);
        ev.set(flag::IN_PASSIVE, false);
        if let Err(e) = result {
            report(ctx, data, e)?;
        }
        if ev.has(flag::STOP_IMMEDIATE) {
            break;
        }
    }
    Ok(())
}

/// Reports an exception a listener threw. Exceptions that must not be
/// caught (an interrupt) come back, to end the dispatch.
pub(super) fn report<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    error: rquickjs::Error,
) -> Result<()> {
    if !matches!(error, rquickjs::Error::Exception) {
        return Err(error);
    }
    let thrown = ctx.catch();
    // SAFETY: a plain check of a live value.
    if unsafe { qjs::JS_IsUncatchableError(thrown.as_raw()) } {
        return Err(ctx.throw(thrown));
    }
    data.shared.report_exception(ctx, thrown);
    Ok(())
}

/// Calls `f` with `this` and `args`.
pub(super) fn call<'js>(
    ctx: &Ctx<'js>,
    f: &Value<'js>,
    this: &Value<'js>,
    args: &[qjs::JSValue],
) -> Result<Value<'js>> {
    let argc = i32::try_from(args.len()).unwrap_or(i32::MAX);
    // SAFETY: the values are alive; QuickJS does not keep `argv`.
    let raw = unsafe {
        qjs::JS_Call(
            ctx.as_raw().as_ptr(),
            f.as_raw(),
            this.as_raw(),
            argc,
            args.as_ptr().cast_mut(),
        )
    };
    // SAFETY: `raw` is a new value, owned by the `Value`.
    if unsafe { qjs::JS_IsException(raw) } {
        return Err(rquickjs::Error::Exception);
    }
    // SAFETY: as above.
    Ok(unsafe { Value::from_raw(ctx.clone(), raw) })
}

/// `object[atom]`.
pub(super) fn get<'js>(
    ctx: &Ctx<'js>,
    object: &Value<'js>,
    atom: qjs::JSAtom,
) -> Result<Value<'js>> {
    // SAFETY: `object` is alive; the result is a new value, owned here.
    unsafe {
        let raw = qjs::JS_GetProperty(ctx.as_raw().as_ptr(), object.as_raw(), atom);
        if qjs::JS_IsException(raw) {
            return Err(rquickjs::Error::Exception);
        }
        Ok(Value::from_raw(ctx.clone(), raw))
    }
}

fn call_listener<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    listener: &Listener<'js>,
    entry: &Entry<'js>,
    event: &Value<'js>,
    ev: &EventData<'js>,
    ty: qjs::JSAtom,
) -> Result<()> {
    match &listener.callback {
        Callback::Value(callback) => {
            if callback.is_function() {
                call(ctx, callback, &entry.value, &[event.as_raw()])?;
                return Ok(());
            }
            let handle = get(ctx, callback, data.atoms.handle_event.raw())?;
            if !handle.is_function() {
                return Err(Exception::throw_type(
                    ctx,
                    "The provided callback is not a function and has no 'handleEvent' method.",
                ));
            }
            call(ctx, &handle, callback, &[event.as_raw()])?;
            Ok(())
        }
        Callback::Handler(handler) => run_handler(ctx, data, handler, entry, event, ev, ty),
    }
}

/// The event handler processing algorithm.
/// <https://html.spec.whatwg.org/#the-event-handler-processing-algorithm>
fn run_handler<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    handler: &Handler<'js>,
    entry: &Entry<'js>,
    event: &Value<'js>,
    ev: &EventData<'js>,
    ty: qjs::JSAtom,
) -> Result<()> {
    let for_window = data.is_global(&entry.value);
    let Some(callback) = handler_value(ctx, data, handler, for_window, ty)? else {
        return Ok(());
    };
    if for_window && ty == data.atoms.error.raw() && is_error_event(ctx, data, event) {
        let a = &data.atoms;
        let args = [
            get(ctx, event, a.message.raw())?,
            get(ctx, event, a.filename.raw())?,
            get(ctx, event, a.lineno.raw())?,
            get(ctx, event, a.colno.raw())?,
            get(ctx, event, a.error.raw())?,
        ];
        let raws = args.each_ref().map(Value::as_raw);
        let result = call(ctx, &callback, &entry.value, &raws)?;
        if result.as_bool() == Some(true) {
            ev.prevent_default();
        }
        return Ok(());
    }
    let result = call(ctx, &callback, &entry.value, &[event.as_raw()])?;
    if result.as_bool() == Some(false) {
        ev.prevent_default();
    }
    Ok(())
}

fn is_error_event<'js>(ctx: &Ctx<'js>, data: &EventsData<'js>, event: &Value<'js>) -> bool {
    let Some(hooks) = data.hooks() else {
        return false;
    };
    // SAFETY: live values; an exception from `instanceof` means no.
    let r = unsafe {
        qjs::JS_IsInstanceOf(
            ctx.as_raw().as_ptr(),
            event.as_raw(),
            hooks.error_event.as_raw(),
        )
    };
    if r < 0 {
        drop(ctx.catch());
    }
    r > 0
}

/// The handler's function, compiling an uncompiled one. A body that does
/// not compile is reported and leaves the handler null.
/// <https://html.spec.whatwg.org/#getting-the-current-value-of-the-event-handler>
pub(super) fn handler_value<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    handler: &Handler<'js>,
    for_window: bool,
    ty: qjs::JSAtom,
) -> Result<Option<Value<'js>>> {
    let source = handler.source.borrow_mut().take();
    if let Some((owner, text)) = source {
        let compiled = match data.hooks() {
            Some(hooks) => {
                let ty = TypeAtom::dup(ctx, ty).to_value(ctx);
                hooks
                    .compile_handler
                    .call::<_, Value<'js>>((owner, ty, text, for_window))
            }
            None => Ok(Value::new_null(ctx.clone())),
        };
        let value = match compiled {
            Ok(v) => (!v.is_null() && !v.is_undefined()).then_some(v),
            Err(e) => {
                report(ctx, data, e)?;
                None
            }
        };
        drop(handler.value.replace(value));
    }
    Ok(handler.value.borrow().clone())
}

/// Picks up the content attribute that sets the handler for `ty`, if the
/// handler was never looked at (see the module docs).
pub(super) fn pick_up_attribute<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    listeners: &Listeners<'js>,
    hop: Option<Hop>,
    ty: qjs::JSAtom,
) -> Result<()> {
    let Some(hop) = hop else {
        return Ok(());
    };
    if listeners.handler_state(ty) != HandlerState::Unseen {
        return Ok(());
    }
    let found = data.handler_attribute(&data.shared.dom.doc(), hop, ty);
    let Some((owner, source)) = found else {
        return Ok(());
    };
    let owner = wrap(ctx, &data.shared.dom, owner)?.into_value();
    let source = rquickjs::String::from_str(ctx.clone(), &source)?.into_value();
    let listener = listeners.activate_handler(ctx, ty, true);
    if let Some(h) = listener.handler() {
        drop(h.value.replace(None));
        drop(h.source.replace(Some((owner, source))));
    }
    Ok(())
}

/// The node or global a target is, for handler attributes.
pub(super) fn hop_of(target: TargetRef<'_, '_>) -> Option<Hop> {
    match target {
        TargetRef::Node(w) => w.id().map(Hop::Node),
        TargetRef::Global => Some(Hop::Global),
        TargetRef::Object(_) => None,
    }
}
