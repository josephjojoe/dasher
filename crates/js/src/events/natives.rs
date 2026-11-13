//! The event natives: QuickJS C functions for the `EventTarget` and
//! `Event` constructors and members, the `on*` accessors, and the functions
//! `events.js` and `dom.js` use (`native.events`).
//!
//! The members are called straight from property lookups, as the DOM's
//! native members are (see "Native members" in the `dom` module): each finds
//! its target or event from `this` through [`EventsData::target`] and
//! [`EventsData::event`], with no rquickjs conversion on the way.

use std::ffi::{CString, c_int};
use std::ptr::NonNull;

use rquickjs::{Array, Class, Ctx, Exception, Function, Object, Result, Value, qjs};

use super::dispatch::{self, get, handler_value, hop_of, pick_up_attribute};
use super::listeners::{HandlerState, TypeAtom};
use super::{EventData, EventsData, HandlerKind, Hooks, TargetData, TargetRef, atom, flag};

type Native = unsafe extern "C" fn(
    *mut qjs::JSContext,
    qjs::JSValue,
    c_int,
    *mut qjs::JSValue,
    c_int,
) -> qjs::JSValue;

/// A `Ctx` for the running call's context.
///
/// # Safety
///
/// `ctx` must be the context of the running call.
pub(super) unsafe fn ctx_of<'js>(ctx: *mut qjs::JSContext) -> Ctx<'js> {
    // SAFETY: as for the function; the runtime is locked while JS runs.
    unsafe { Ctx::from_raw(NonNull::new_unchecked(ctx)) }
}

/// A C function `f`, named `name` with `length`, called with `magic`.
fn cfunc<'js>(
    ctx: &Ctx<'js>,
    f: Native,
    name: &str,
    length: c_int,
    magic: c_int,
    constructor: bool,
) -> Result<Value<'js>> {
    let name = CString::new(name).map_err(|_| rquickjs::Error::Unknown)?;
    type Plain = unsafe extern "C" fn(
        *mut qjs::JSContext,
        qjs::JSValue,
        c_int,
        *mut qjs::JSValue,
    ) -> qjs::JSValue;
    // SAFETY: QuickJS keeps C functions as a union of pointer types and
    // calls this one as the `generic_magic` (or constructor) kind given,
    // with the signature it has.
    let f = unsafe { std::mem::transmute::<Native, Plain>(f) };
    let cproto = if constructor {
        qjs::JSCFunctionEnum_JS_CFUNC_constructor_or_func_magic
    } else {
        qjs::JSCFunctionEnum_JS_CFUNC_generic_magic
    };
    // SAFETY: `ctx` is live and `name` a C string, which QuickJS copies.
    let raw = unsafe {
        qjs::JS_NewCFunction2(
            ctx.as_raw().as_ptr(),
            Some(f),
            name.as_ptr(),
            length,
            cproto,
            magic,
        )
    };
    // SAFETY: `raw` is a new value (or the exception marker), owned here.
    if unsafe { qjs::JS_IsException(raw) } {
        return Err(rquickjs::Error::Exception);
    }
    // SAFETY: as above.
    Ok(unsafe { Value::from_raw(ctx.clone(), raw) })
}

const METHOD_FLAGS: c_int =
    (qjs::JS_PROP_CONFIGURABLE | qjs::JS_PROP_WRITABLE | qjs::JS_PROP_ENUMERABLE) as c_int;
const ACCESSOR_FLAGS: c_int = (qjs::JS_PROP_CONFIGURABLE | qjs::JS_PROP_ENUMERABLE) as c_int;

fn define_method<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    name: &str,
    f: Native,
    length: c_int,
    magic: c_int,
) -> Result<()> {
    let f = cfunc(ctx, f, name, length, magic, false)?;
    let name = atom(ctx, name)?;
    // SAFETY: live values; QuickJS takes the duplicate.
    let r = unsafe {
        qjs::JS_DefinePropertyValue(
            ctx.as_raw().as_ptr(),
            obj.as_raw(),
            name.raw(),
            qjs::JS_DupValue(ctx.as_raw().as_ptr(), f.as_raw()),
            METHOD_FLAGS,
        )
    };
    if r < 0 {
        return Err(rquickjs::Error::Exception);
    }
    Ok(())
}

fn define_accessor<'js>(
    ctx: &Ctx<'js>,
    obj: &Object<'js>,
    name: &str,
    getter: (Native, c_int),
    setter: Option<(Native, c_int)>,
) -> Result<()> {
    let g = cfunc(ctx, getter.0, &format!("get {name}"), 0, getter.1, false)?;
    let s = match setter {
        Some((f, magic)) => Some(cfunc(ctx, f, &format!("set {name}"), 1, magic, false)?),
        None => None,
    };
    let name = atom(ctx, name)?;
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: live values; QuickJS takes the duplicates.
    let r = unsafe {
        qjs::JS_DefinePropertyGetSet(
            raw,
            obj.as_raw(),
            name.raw(),
            qjs::JS_DupValue(raw, g.as_raw()),
            s.as_ref()
                .map_or(qjs::JS_UNDEFINED, |s| qjs::JS_DupValue(raw, s.as_raw())),
            ACCESSOR_FLAGS,
        )
    };
    if r < 0 {
        return Err(rquickjs::Error::Exception);
    }
    Ok(())
}

// -------------------------------------------------------------------------
// Call plumbing.

/// The arguments of a call.
///
/// # Safety
///
/// QuickJS passes `argc` live values at `argv`.
unsafe fn args<'a>(argc: c_int, argv: *mut qjs::JSValue) -> &'a [qjs::JSValue] {
    match usize::try_from(argc) {
        // SAFETY: as for the function.
        Ok(n) if n > 0 && !argv.is_null() => unsafe { std::slice::from_raw_parts(argv, n) },
        _ => &[],
    }
}

fn arg(args: &[qjs::JSValue], i: usize) -> qjs::JSValue {
    args.get(i).copied().unwrap_or(qjs::JS_UNDEFINED)
}

/// `value` as a new JS value for QuickJS, or the exception marker.
fn finish<'js>(ctx: &Ctx<'js>, result: Result<Value<'js>>) -> qjs::JSValue {
    match result {
        // SAFETY: `v` is alive; the duplicate is the caller's.
        Ok(v) => unsafe { qjs::JS_DupValue(ctx.as_raw().as_ptr(), v.as_raw()) },
        Err(rquickjs::Error::Exception) => qjs::JS_EXCEPTION,
        Err(rquickjs::Error::Allocation) => {
            // SAFETY: `ctx` is live.
            unsafe { qjs::JS_ThrowOutOfMemory(ctx.as_raw().as_ptr()) };
            qjs::JS_EXCEPTION
        }
        Err(other) => {
            let _ = Exception::throw_internal(ctx, &other.to_string());
            qjs::JS_EXCEPTION
        }
    }
}

fn undefined<'js>(ctx: &Ctx<'js>) -> Value<'js> {
    Value::new_undefined(ctx.clone())
}

/// A borrowed argument as a `Value` of its own.
fn value<'js>(ctx: &Ctx<'js>, raw: qjs::JSValue) -> Value<'js> {
    // SAFETY: `raw` is alive; the `Value` owns the new reference.
    unsafe { Value::from_raw(ctx.clone(), qjs::JS_DupValue(ctx.as_raw().as_ptr(), raw)) }
}

fn not_enough(ctx: &Ctx<'_>, what: &str, n: usize, got: usize) -> rquickjs::Error {
    Exception::throw_type(
        ctx,
        &format!(
            "Failed to {what}: {n} argument{} required, but only {got} present.",
            if n == 1 { "" } else { "s" }
        ),
    )
}

fn illegal(ctx: &Ctx<'_>) -> rquickjs::Error {
    Exception::throw_type(ctx, "Illegal invocation")
}

/// An event type from a JS value: `ToString`, then an atom.
fn type_atom<'js>(ctx: &Ctx<'js>, value: qjs::JSValue) -> Result<TypeAtom<'js>> {
    let raw = ctx.as_raw().as_ptr();
    // SAFETY: `value` is alive. A string converts to its atom directly;
    // anything else goes through `ToString` first.
    unsafe {
        if qjs::JS_IsString(value) {
            let a = qjs::JS_ValueToAtom(raw, value);
            if a == qjs::JS_ATOM_NULL {
                return Err(rquickjs::Error::Exception);
            }
            return Ok(TypeAtom::own(ctx, a));
        }
        let s = qjs::JS_ToString(raw, value);
        if qjs::JS_IsException(s) {
            return Err(rquickjs::Error::Exception);
        }
        let a = qjs::JS_ValueToAtom(raw, s);
        qjs::JS_FreeValue(raw, s);
        if a == qjs::JS_ATOM_NULL {
            return Err(rquickjs::Error::Exception);
        }
        Ok(TypeAtom::own(ctx, a))
    }
}

fn to_bool(ctx: &Ctx<'_>, value: qjs::JSValue) -> bool {
    // SAFETY: `ToBoolean` of a live value never runs JS.
    unsafe { qjs::JS_ToBool(ctx.as_raw().as_ptr(), value) > 0 }
}

/// `object[name]` converted to a boolean.
fn get_bool<'js>(ctx: &Ctx<'js>, object: &Value<'js>, name: &TypeAtom<'js>) -> Result<bool> {
    let v = get(ctx, object, name.raw())?;
    Ok(to_bool(ctx, v.as_raw()))
}

fn is_nullish(value: qjs::JSValue) -> bool {
    // SAFETY: plain tag checks.
    unsafe { qjs::JS_IsNull(value) || qjs::JS_IsUndefined(value) }
}

fn is_object(value: qjs::JSValue) -> bool {
    // SAFETY: a plain tag check.
    unsafe { qjs::JS_IsObject(value) }
}

impl<'js> EventsData<'js> {
    /// `new_target.prototype` if it is an object, else `fallback`.
    fn prototype_of(
        &self,
        ctx: &Ctx<'js>,
        new_target: qjs::JSValue,
        fallback: &Object<'js>,
    ) -> Result<Object<'js>> {
        let proto = get(ctx, &value(ctx, new_target), self.atoms.prototype.raw())?;
        Ok(match proto.into_object() {
            Some(o) => o,
            None => fallback.clone(),
        })
    }

    /// A new event object.
    pub(super) fn new_event(
        &self,
        ty: TypeAtom<'js>,
        flags: u16,
        proto: Option<Object<'js>>,
    ) -> Result<Value<'js>> {
        let now = self.shared.event_loop.clock.now().as_secs_f64() * 1000.0;
        let data = EventData::new(ty, flags, now);
        let proto = proto.unwrap_or_else(|| self.event_proto.clone());
        Ok(Class::instance_proto(data, proto)?.into_value())
    }

    /// The value of a target for listeners and paths: the global object
    /// for `undefined` and `null`.
    fn target_value(&self, ctx: &Ctx<'js>, this: qjs::JSValue) -> Value<'js> {
        if is_nullish(this) {
            ctx.globals().into_value()
        } else {
            value(ctx, this)
        }
    }

    fn throw_dom(&self, ctx: &Ctx<'js>, name: &str, message: &str) -> rquickjs::Error {
        match self.hooks() {
            Some(hooks) => match hooks.dom_exception.call::<_, Value<'js>>((name, message)) {
                Ok(e) => ctx.throw(e),
                Err(e) => e,
            },
            None => Exception::throw_type(ctx, message),
        }
    }
}

/// Runs a native's body with its context and events data.
///
/// # Safety
///
/// `ctx` must be the context of the running call.
unsafe fn with_data(
    ctx: *mut qjs::JSContext,
    f: impl for<'js> FnOnce(&Ctx<'js>, &EventsData<'js>) -> Result<Value<'js>>,
) -> qjs::JSValue {
    // SAFETY: as for the function.
    let ctx = unsafe { ctx_of(ctx) };
    // SAFETY: as above.
    let Some(data) = (unsafe { EventsData::of(ctx.as_raw().as_ptr()) }) else {
        let e = illegal(&ctx);
        return finish(&ctx, Err(e));
    };
    let r = f(&ctx, data);
    finish(&ctx, r)
}

// -------------------------------------------------------------------------
// EventTarget.

unsafe extern "C" fn target_constructor(
    ctx: *mut qjs::JSContext,
    new_target: qjs::JSValue,
    _argc: c_int,
    _argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            if !is_object(new_target) {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to construct 'EventTarget': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
                ));
            }
            let proto = data.prototype_of(ctx, new_target, &data.target_proto)?;
            Ok(Class::instance_proto(TargetData::default(), proto)?.into_value())
        })
    }
}

/// The options of `addEventListener`: capture, once, passive and the
/// signal's target data and value.
struct AddOptions<'a, 'js> {
    capture: bool,
    once: bool,
    passive: bool,
    signal: Option<(&'a TargetData<'js>, Value<'js>)>,
}

fn add_options<'a, 'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    options: qjs::JSValue,
) -> Result<AddOptions<'a, 'js>> {
    let mut o = AddOptions {
        capture: false,
        once: false,
        passive: false,
        signal: None,
    };
    if !is_object(options) {
        o.capture = to_bool(ctx, options);
        return Ok(o);
    }
    let options = value(ctx, options);
    let a = &data.atoms;
    o.capture = get_bool(ctx, &options, &a.capture)?;
    o.once = get_bool(ctx, &options, &a.once)?;
    o.passive = get_bool(ctx, &options, &a.passive)?;
    let signal = get(ctx, &options, a.signal.raw())?;
    if !signal.is_undefined() {
        // SAFETY: `signal` is kept with the result, so its object lives.
        let target = unsafe { data.target(ctx.as_raw().as_ptr(), signal.as_raw()) };
        let Some(TargetRef::Object(t)) = target.filter(|t| match t {
            TargetRef::Object(t) => t.signal_aborted().is_some(),
            _ => false,
        }) else {
            return Err(Exception::throw_type(
                ctx,
                "Failed to execute 'addEventListener' on 'EventTarget': member signal is not of type 'AbortSignal'.",
            ));
        };
        // SAFETY: as above; the reference is only used while `signal` is.
        let t: &'a TargetData<'js> = unsafe { &*std::ptr::from_ref(t) };
        o.signal = Some((t, signal));
    }
    Ok(o)
}

unsafe extern "C" fn add_event_listener(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            if args.len() < 2 {
                let what = "execute 'addEventListener' on 'EventTarget'";
                return Err(not_enough(ctx, what, 2, args.len()));
            }
            // SAFETY: `this` is alive for the call.
            let target = data
                .target(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let ty = type_atom(ctx, args[0])?;
            let callback = args[1];
            if is_nullish(callback) {
                return Ok(undefined(ctx));
            }
            if !is_object(callback) {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to execute 'addEventListener' on 'EventTarget': parameter 2 is not of type 'Object'.",
                ));
            }
            let o = add_options(ctx, data, arg(args, 2))?;
            if let Some((signal, _)) = &o.signal
                && signal.signal_aborted() == Some(true)
            {
                return Ok(undefined(ctx));
            }
            let listeners = data.listeners(target);
            let added = listeners.add(ctx, ty.raw(), callback, o.capture, o.once, o.passive);
            if let (Some(listener), Some((signal, _))) = (added, &o.signal) {
                let target = data.target_value(ctx, this);
                signal.add_removal(target, ty, &listener);
            }
            Ok(undefined(ctx))
        })
    }
}

unsafe extern "C" fn remove_event_listener(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            if args.len() < 2 {
                let what = "execute 'removeEventListener' on 'EventTarget'";
                return Err(not_enough(ctx, what, 2, args.len()));
            }
            // SAFETY: `this` is alive for the call.
            let target = data
                .target(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let ty = type_atom(ctx, args[0])?;
            let callback = args[1];
            let options = arg(args, 2);
            let capture = if is_object(options) {
                get_bool(ctx, &value(ctx, options), &data.atoms.capture)?
            } else {
                to_bool(ctx, options)
            };
            if is_object(callback) {
                data.listeners(target)
                    .remove_matching(ty.raw(), callback, capture);
            }
            Ok(undefined(ctx))
        })
    }
}

unsafe extern "C" fn dispatch_event(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            let what = "execute 'dispatchEvent' on 'EventTarget'";
            if args.is_empty() {
                return Err(not_enough(ctx, what, 1, 0));
            }
            let raw = ctx.as_raw().as_ptr();
            // SAFETY: `this` and the argument are alive for the call.
            if data.target(raw, this).is_none() {
                return Err(illegal(ctx));
            }
            let Some(ev) = data.event(raw, args[0]) else {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.",
                ));
            };
            if ev.has(flag::DISPATCHING) {
                return Err(data.throw_dom(
                    ctx,
                    "InvalidStateError",
                    "Failed to execute 'dispatchEvent' on 'EventTarget': The event is already being dispatched.",
                ));
            }
            let target = data.target_value(ctx, this);
            let event = value(ctx, args[0]);
            let r = dispatch::dispatch(ctx, data, &target, &event, ev, false)?;
            Ok(Value::new_bool(ctx.clone(), r))
        })
    }
}

// -------------------------------------------------------------------------
// Event.

unsafe extern "C" fn event_constructor(
    ctx: *mut qjs::JSContext,
    new_target: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            if !is_object(new_target) {
                return Err(Exception::throw_type(
                    ctx,
                    "Failed to construct 'Event': Please use the 'new' operator, this DOM object constructor cannot be called as a function.",
                ));
            }
            if args.is_empty() {
                return Err(not_enough(ctx, "construct 'Event'", 1, 0));
            }
            let ty = type_atom(ctx, args[0])?;
            let flags = init_flags(ctx, data, arg(args, 1))?;
            let proto = data.prototype_of(ctx, new_target, &data.event_proto)?;
            data.new_event(ty, flags, Some(proto))
        })
    }
}

/// The flags an `EventInit` dictionary sets.
fn init_flags<'js>(ctx: &Ctx<'js>, data: &EventsData<'js>, init: qjs::JSValue) -> Result<u16> {
    if is_nullish(init) {
        return Ok(0);
    }
    if !is_object(init) {
        return Err(Exception::throw_type(
            ctx,
            "Failed to construct 'Event': The provided value is not of type 'EventInit'.",
        ));
    }
    let init = value(ctx, init);
    let a = &data.atoms;
    let mut flags = 0;
    if get_bool(ctx, &init, &a.bubbles)? {
        flags |= flag::BUBBLES;
    }
    if get_bool(ctx, &init, &a.cancelable)? {
        flags |= flag::CANCELABLE;
    }
    if get_bool(ctx, &init, &a.composed)? {
        flags |= flag::COMPOSED;
    }
    Ok(flags)
}

/// The getters and methods of `Event.prototype`, by magic number.
const EVENT_MEMBERS: &[(&str, Option<c_int>)] = &[
    ("type", None),
    ("target", None),
    ("srcElement", None),
    ("currentTarget", None),
    ("eventPhase", None),
    ("bubbles", None),
    ("cancelable", None),
    ("composed", None),
    ("defaultPrevented", None),
    ("isTrusted", None),
    ("timeStamp", None),
    ("returnValue", None),
    ("cancelBubble", None),
    ("composedPath", Some(0)),
    ("stopPropagation", Some(0)),
    ("stopImmediatePropagation", Some(0)),
    ("preventDefault", Some(0)),
    ("initEvent", Some(1)),
];

unsafe extern "C" fn event_member(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // Plain reads go without making a `Ctx`.
    // SAFETY: QuickJS calls with the live context, and `this` is alive.
    if let Some(data) = unsafe { EventsData::of(ctx) }
        // SAFETY: as above.
        && let Some(ev) = unsafe { data.event(ctx, this) }
    {
        let b = |bits| {
            if ev.has(bits) {
                qjs::JS_TRUE
            } else {
                qjs::JS_FALSE
            }
        };
        match magic {
            4 => return qjs::JS_MKVAL(qjs::JS_TAG_INT, i32::from(ev.phase.get())),
            5 => return b(flag::BUBBLES),
            6 => return b(flag::CANCELABLE),
            7 => return b(flag::COMPOSED),
            8 => return b(flag::CANCELED),
            9 => return b(flag::TRUSTED),
            11 => {
                return if ev.has(flag::CANCELED) {
                    qjs::JS_FALSE
                } else {
                    qjs::JS_TRUE
                };
            }
            12 => return b(flag::STOP),
            14 => {
                ev.set(flag::STOP, true);
                return qjs::JS_UNDEFINED;
            }
            15 => {
                ev.set(flag::STOP | flag::STOP_IMMEDIATE, true);
                return qjs::JS_UNDEFINED;
            }
            16 => {
                ev.prevent_default();
                return qjs::JS_UNDEFINED;
            }
            _ => {}
        }
    }
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            // SAFETY: `this` is alive for the call.
            let ev = data
                .event(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let null = || Value::new_null(ctx.clone());
            Ok(match magic {
                0 => TypeAtom::dup(ctx, ev.ty()).to_value(ctx),
                1 | 2 => ev.target.borrow().clone().unwrap_or_else(null),
                3 => ev.current_target.borrow().clone().unwrap_or_else(null),
                10 => Value::new_float(ctx.clone(), ev.time_stamp),
                13 => {
                    let array = Array::new(ctx.clone())?;
                    if ev.has(flag::DISPATCHING) {
                        let path = ev.path.borrow().clone();
                        for (i, v) in path.into_iter().enumerate() {
                            array.set(i, v)?;
                        }
                    }
                    array.into_value()
                }
                17 => {
                    if args.is_empty() {
                        return Err(not_enough(ctx, "execute 'initEvent' on 'Event'", 1, 0));
                    }
                    let ty = type_atom(ctx, args[0])?;
                    if ev.has(flag::DISPATCHING) {
                        return Ok(undefined(ctx));
                    }
                    drop(ev.ty.replace(ty));
                    ev.set(
                        flag::BUBBLES
                            | flag::CANCELABLE
                            | flag::CANCELED
                            | flag::STOP
                            | flag::STOP_IMMEDIATE
                            | flag::TRUSTED,
                        false,
                    );
                    ev.set(flag::BUBBLES, to_bool(ctx, arg(args, 1)));
                    ev.set(flag::CANCELABLE, to_bool(ctx, arg(args, 2)));
                    ev.set_target(None);
                    undefined(ctx)
                }
                _ => return Err(illegal(ctx)),
            })
        })
    }
}

/// The setters of `returnValue` (magic 0) and `cancelBubble` (1).
unsafe extern "C" fn event_setter(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            // SAFETY: `this` is alive for the call.
            let ev = data
                .event(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let on = to_bool(ctx, arg(args, 0));
            match magic {
                0 if !on => ev.prevent_default(),
                1 if on => ev.set(flag::STOP, true),
                _ => {}
            }
            Ok(undefined(ctx))
        })
    }
}

// -------------------------------------------------------------------------
// Event handlers.

fn handler_type<'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    magic: c_int,
) -> Result<TypeAtom<'js>> {
    let types = data.handler_types.borrow();
    let ty = usize::try_from(magic).ok().and_then(|i| types.get(i));
    ty.map(|t| TypeAtom::dup(ctx, t.raw()))
        .ok_or(rquickjs::Error::Unknown)
}

unsafe extern "C" fn handler_getter(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    _argc: c_int,
    _argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            // SAFETY: `this` is alive for the call.
            let target = data
                .target(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let ty = handler_type(ctx, data, magic)?;
            let listeners = data.listeners(target);
            pick_up_attribute(ctx, data, listeners, hop_of(target), ty.raw())?;
            let Some(listener) = listeners.handler(ty.raw()) else {
                return Ok(Value::new_null(ctx.clone()));
            };
            let handler = listener.handler().ok_or(rquickjs::Error::Unknown)?;
            let for_window = matches!(target, TargetRef::Global);
            let v = handler_value(ctx, data, handler, for_window, ty.raw())?;
            Ok(v.unwrap_or_else(|| Value::new_null(ctx.clone())))
        })
    }
}

unsafe extern "C" fn handler_setter(
    ctx: *mut qjs::JSContext,
    this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            // SAFETY: `this` is alive for the call.
            let target = data
                .target(ctx.as_raw().as_ptr(), this)
                .ok_or_else(|| illegal(ctx))?;
            let ty = handler_type(ctx, data, magic)?;
            let listeners = data.listeners(target);
            // An attribute the element was made with keeps its place.
            pick_up_attribute(ctx, data, listeners, hop_of(target), ty.raw())?;
            let v = value(ctx, arg(args, 0));
            if v.is_function() {
                let listener = listeners.activate_handler(ctx, ty.raw(), false);
                if let Some(h) = listener.handler() {
                    drop(h.source.replace(None));
                    drop(h.value.replace(Some(v)));
                }
            } else {
                listeners.deactivate_handler(ctx, ty.raw());
            }
            Ok(undefined(ctx))
        })
    }
}

// -------------------------------------------------------------------------
// The glue's natives.

unsafe extern "C" fn listening(
    ctx: *mut qjs::JSContext,
    _this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            let ty = type_atom(ctx, arg(args, 1))?;
            let raw = ctx.as_raw().as_ptr();
            // SAFETY: the argument is alive for the call.
            let r = match data.target(raw, arg(args, 0)) {
                Some(t) => data.reaches_target(raw, t, ty.raw()),
                None => false,
            };
            Ok(Value::new_bool(ctx.clone(), r))
        })
    }
}

/// `fire(target, type, init, Ctor)`: makes a trusted event with `new
/// Ctor(type, init)` (a plain `Event` without `Ctor`) and dispatches it at
/// `target`, if any listener could hear it. Returns whether no listener
/// canceled it.
unsafe extern "C" fn fire(
    ctx: *mut qjs::JSContext,
    _this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            let raw = ctx.as_raw().as_ptr();
            let target_raw = arg(args, 0);
            let ty = type_atom(ctx, arg(args, 1))?;
            // SAFETY: the argument is alive for the call.
            let reached = match data.target(raw, target_raw) {
                Some(t) => data.reaches_target(raw, t, ty.raw()),
                None => true,
            };
            if !reached {
                return Ok(Value::new_bool(ctx.clone(), true));
            }
            let ctor = arg(args, 3);
            let event = if is_nullish(ctor) {
                let flags = init_flags(ctx, data, arg(args, 2))?;
                data.new_event(ty, flags, None)?
            } else {
                let mut argv = [ty.to_value(ctx), value(ctx, arg(args, 2))];
                let mut raws = [argv[0].as_raw(), argv[1].as_raw()];
                // SAFETY: live values; the result is owned below.
                let e = qjs::JS_CallConstructor(raw, ctor, 2, raws.as_mut_ptr());
                if qjs::JS_IsException(e) {
                    return Err(rquickjs::Error::Exception);
                }
                argv[1] = Value::from_raw(ctx.clone(), e);
                std::mem::replace(&mut argv[1], undefined(ctx))
            };
            // SAFETY: `event` is alive while `ev` is used.
            let ev = data.event(raw, event.as_raw()).ok_or_else(|| {
                Exception::throw_type(ctx, "The event constructor did not make an Event.")
            })?;
            let target = data.target_value(ctx, target_raw);
            let r = dispatch::dispatch(ctx, data, &target, &event, ev, true)?;
            Ok(Value::new_bool(ctx.clone(), r))
        })
    }
}

/// `dispatch(target, event)`: dispatches a trusted event.
unsafe extern "C" fn dispatch_trusted(
    ctx: *mut qjs::JSContext,
    _this: qjs::JSValue,
    argc: c_int,
    argv: *mut qjs::JSValue,
    _magic: c_int,
) -> qjs::JSValue {
    // SAFETY: QuickJS passes `argc` live values at `argv`.
    let args = unsafe { args(argc, argv) };
    // SAFETY: QuickJS calls with the live context.
    unsafe {
        with_data(ctx, |ctx, data| {
            let raw = ctx.as_raw().as_ptr();
            let ev = data
                .event(raw, arg(args, 1))
                .ok_or_else(|| Exception::throw_type(ctx, "Not an Event."))?;
            let target = data.target_value(ctx, arg(args, 0));
            let event = value(ctx, arg(args, 1));
            let r = dispatch::dispatch(ctx, data, &target, &event, ev, true)?;
            Ok(Value::new_bool(ctx.clone(), r))
        })
    }
}

/// The target `value` is, or a `TypeError`.
fn target_arg<'a, 'js>(
    ctx: &Ctx<'js>,
    data: &EventsData<'js>,
    value: &'a Value<'js>,
) -> Result<TargetRef<'a, 'js>> {
    // SAFETY: `value` is alive for as long as the result is used.
    unsafe { data.target(ctx.as_raw().as_ptr(), value.as_raw()) }.ok_or_else(|| illegal(ctx))
}

/// Builds `native.events`, and the prototypes of `EventTarget` and
/// `Event`.
pub(super) fn install<'js>(ctx: &Ctx<'js>, data: &EventsData<'js>) -> Result<Object<'js>> {
    let raw = ctx.as_raw().as_ptr();
    let e = Object::new(ctx.clone())?;

    let proto = &data.target_proto;
    define_method(ctx, proto, "addEventListener", add_event_listener, 2, 0)?;
    define_method(
        ctx,
        proto,
        "removeEventListener",
        remove_event_listener,
        2,
        0,
    )?;
    define_method(ctx, proto, "dispatchEvent", dispatch_event, 1, 0)?;
    let ctor = cfunc(ctx, target_constructor, "EventTarget", 0, 0, true)?;
    // SAFETY: live values.
    unsafe { qjs::JS_SetConstructor(raw, ctor.as_raw(), proto.as_raw()) };
    e.set("EventTarget", ctor)?;

    let proto = &data.event_proto;
    for (i, &(name, kind)) in EVENT_MEMBERS.iter().enumerate() {
        let magic = c_int::try_from(i).expect("few members");
        match kind {
            Some(length) => define_method(ctx, proto, name, event_member, length, magic)?,
            None => {
                let setter = match name {
                    "returnValue" => Some((event_setter as Native, 0)),
                    "cancelBubble" => Some((event_setter as Native, 1)),
                    _ => None,
                };
                define_accessor(ctx, proto, name, (event_member, magic), setter)?;
            }
        }
    }
    let ctor = cfunc(ctx, event_constructor, "Event", 1, 0, true)?;
    // SAFETY: live values.
    unsafe { qjs::JS_SetConstructor(raw, ctor.as_raw(), proto.as_raw()) };
    e.set("Event", ctor)?;

    e.set("fire", cfunc(ctx, fire, "fire", 4, 0, false)?)?;
    e.set(
        "listening",
        cfunc(ctx, listening, "listening", 2, 0, false)?,
    )?;
    e.set(
        "dispatch",
        cfunc(ctx, dispatch_trusted, "dispatch", 2, 0, false)?,
    )?;

    e.set(
        "isEvent",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, v: Value<'js>| {
            EventsData::get(&ctx)
                // SAFETY: `v` is alive while looked at.
                .is_some_and(|d| unsafe { d.event(ctx.as_raw().as_ptr(), v.as_raw()) }.is_some())
        })?,
    )?;
    e.set(
        "createTarget",
        Function::new(ctx.clone(), |proto: Object<'js>| -> Result<Value<'js>> {
            Ok(Class::instance_proto(TargetData::default(), proto)?.into_value())
        })?,
    )?;
    e.set(
        "makeSignal",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, signal: Value<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                match target_arg(&ctx, data, &signal)? {
                    TargetRef::Object(t) => t.make_signal(),
                    _ => return Err(illegal(&ctx)),
                }
                Ok(())
            },
        )?,
    )?;
    e.set(
        "abortSignal",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, signal: Value<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                let TargetRef::Object(t) = target_arg(&ctx, data, &signal)? else {
                    return Err(illegal(&ctx));
                };
                let removals = t.abort();
                for r in &removals {
                    let Some(listener) = r.listener.upgrade() else {
                        continue;
                    };
                    if let Ok(target) = target_arg(&ctx, data, &r.target) {
                        data.listeners(target).remove(r.ty.raw(), &listener);
                    }
                }
                drop(removals);
                Ok(())
            },
        )?,
    )?;
    e.set(
        "existingHandler",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, target: Value<'js>, ty: Value<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                let t = target_arg(&ctx, data, &target)?;
                let ty = type_atom(&ctx, ty.as_raw())?;
                pick_up_attribute(&ctx, data, data.listeners(t), hop_of(t), ty.raw())
            },
        )?,
    )?;
    e.set(
        "attributeHandlerChanged",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             target: Value<'js>,
             ty: Value<'js>,
             owner: Value<'js>,
             source: Value<'js>|
             -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                let t = target_arg(&ctx, data, &target)?;
                let ty = type_atom(&ctx, ty.as_raw())?;
                let listeners = data.listeners(t);
                if source.is_null() {
                    if listeners.handler_state(ty.raw()) != HandlerState::Unseen {
                        listeners.deactivate_handler(&ctx, ty.raw());
                    }
                    return Ok(());
                }
                let listener = listeners.activate_handler(&ctx, ty.raw(), false);
                if let Some(h) = listener.handler() {
                    drop(h.value.replace(None));
                    drop(h.source.replace(Some((owner, source))));
                }
                Ok(())
            },
        )?,
    )?;
    e.set(
        "defineHandlers",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, proto: Object<'js>, names: Array<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                for name in names.iter::<String>() {
                    let name = name?;
                    let ty = atom(&ctx, name.get(2..).unwrap_or_default())?;
                    let magic = {
                        let mut types = data.handler_types.borrow_mut();
                        match types.iter().position(|t| t.raw() == ty.raw()) {
                            Some(i) => i,
                            None => {
                                types.push(ty);
                                types.len() - 1
                            }
                        }
                    };
                    let magic = c_int::try_from(magic).map_err(|_| rquickjs::Error::Unknown)?;
                    define_accessor(
                        &ctx,
                        &proto,
                        &name,
                        (handler_getter, magic),
                        Some((handler_setter, magic)),
                    )?;
                }
                Ok(())
            },
        )?,
    )?;
    e.set(
        "handlerTypes",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, window: Array<'js>, element: Array<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                let mut kinds = data.handler_kinds.borrow_mut();
                for (list, is_window) in [(window, true), (element, false)] {
                    for ty in list.iter::<String>() {
                        let ty = ty?;
                        let a = atom(&ctx, &ty)?;
                        let kind = kinds.entry(a.raw()).or_insert_with(|| HandlerKind {
                            _ty: TypeAtom::dup(&ctx, a.raw()),
                            window: false,
                            element: false,
                            attribute: format!("on{ty}").into_boxed_str(),
                        });
                        if is_window {
                            kind.window = true;
                        } else {
                            kind.element = true;
                        }
                    }
                }
                Ok(())
            },
        )?,
    )?;
    e.set(
        "setHooks",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, hooks: Object<'js>| -> Result<()> {
                let data = EventsData::get(&ctx).ok_or(rquickjs::Error::Unknown)?;
                let hooks = Hooks {
                    compile_handler: hooks.get("compileHandler")?,
                    slot_for: hooks.get("slotFor")?,
                    dom_exception: hooks.get("domException")?,
                    error_event: hooks.get("ErrorEvent")?,
                };
                drop(data.hooks.replace(Some(hooks)));
                Ok(())
            },
        )?,
    )?;
    Ok(e)
}
