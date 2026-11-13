//! What the native functions installed straight on prototypes (DOM members,
//! event targets and events) find through the context's opaque pointer.
//!
//! Those functions are plain QuickJS C functions, called with nothing but
//! the context, so the per-context data they need is reached from the
//! context's opaque pointer: one load, where rquickjs's userdata would be a
//! hash lookup. Nothing else in dasher or rquickjs uses that pointer.
//!
//! The pointers here are set once the data they point at exists (the DOM's
//! when `dom.js` registers its prototypes, the events' when the host
//! functions are installed) and are owned elsewhere, as runtime userdata
//! that lives as long as JS can run.

use std::cell::Cell;
use std::ffi::c_void;

use rquickjs::{Ctx, JsLifetime, qjs};

use crate::dom::Members;
use crate::events::EventsData;

pub(crate) struct NativeData {
    members: Cell<*const Members>,
    events: Cell<*const c_void>,
}

// SAFETY: `NativeData` holds no values bound to a lifetime.
unsafe impl<'js> JsLifetime<'js> for NativeData {
    type Changed<'to> = NativeData;
}

impl NativeData {
    /// Stores an empty `NativeData` as `ctx`'s userdata and points the
    /// context at it.
    pub(crate) fn install(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
        let data = NativeData {
            members: Cell::new(std::ptr::null()),
            events: Cell::new(std::ptr::null()),
        };
        if ctx.store_userdata(data).is_err() {
            return Err(rquickjs::Error::Unknown);
        }
        let data = ctx
            .userdata::<NativeData>()
            .ok_or(rquickjs::Error::Unknown)?;
        let ptr: *const NativeData = &*data;
        // SAFETY: rquickjs keeps userdata boxed, so it does not move, until
        // the runtime is dropped, after which no JS runs.
        unsafe { qjs::JS_SetContextOpaque(ctx.as_raw().as_ptr(), ptr.cast_mut().cast()) };
        Ok(())
    }

    /// The data of the running call's context, if it was installed.
    ///
    /// # Safety
    ///
    /// `ctx` must be a live context.
    #[inline]
    pub(crate) unsafe fn of<'a>(ctx: *mut qjs::JSContext) -> Option<&'a NativeData> {
        // SAFETY: the opaque is null or the `NativeData` installed above.
        unsafe { qjs::JS_GetContextOpaque(ctx).cast::<NativeData>().as_ref() }
    }

    pub(crate) fn of_ctx<'a>(ctx: &Ctx<'_>) -> Option<&'a NativeData> {
        // SAFETY: `ctx` is live.
        unsafe { Self::of(ctx.as_raw().as_ptr()) }
    }

    /// What the native DOM members need, once `dom.js` has registered.
    #[inline]
    pub(crate) fn members<'a>(&self) -> Option<&'a Members> {
        // SAFETY: null, or the `Members` that `DomData` owns for as long
        // as JS runs.
        unsafe { self.members.get().as_ref() }
    }

    pub(crate) fn set_members(&self, members: *const Members) {
        self.members.set(members);
    }

    /// What the native event functions need.
    #[inline]
    pub(crate) fn events<'a, 'js>(&self) -> Option<&'a EventsData<'js>> {
        // SAFETY: null, or the `EventsData` kept as userdata for as long as
        // JS runs. Its lifetime is that of the context, which the caller's
        // `'js` stands for.
        unsafe { self.events.get().cast::<EventsData<'js>>().as_ref() }
    }

    pub(crate) fn set_events(&self, events: *const EventsData<'_>) {
        self.events.set(events.cast());
    }
}
