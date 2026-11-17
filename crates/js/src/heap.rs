//! The JS heap allocator: dasher's slab allocator (see [`crate::slab`])
//! in place of the C allocator QuickJS uses by default, with dasher's
//! memory limit enforced here instead of inside QuickJS.
//!
//! QuickJS checks its own limit before calling the allocator and, when it
//! cannot even allocate the error object, throws `null`, which is
//! indistinguishable from a script's `throw null`. Enforcing the limit here
//! lets the runtime record that an allocation was refused and report the
//! failure as out of memory.
//!
//! One kind of allocation is not refused: the engine's while it compiles.
//! QuickJS's compiler uses most of what it allocates without checking it,
//! and a function whose bytecode buffer could not grow was left cut short
//! for the next pass to read past the end of: a page that filled its heap
//! and then called a function for the first time crashed the process
//! (linkedin.com). So an allocation made during a compile goes over the
//! limit, the heap counts as full from then on, and the engine stops the
//! compile with an out-of-memory error at its next token (see `JSOverdraft`
//! in `quickjs.h`). What goes over is what one token allocates.

use std::cell::Cell;
use std::rc::Rc;

use rquickjs::Runtime;

/// The heap and its accounting, shared between the allocator and the
/// runtime.
pub(crate) struct HeapState {
    #[cfg(unix)]
    slab: imp::Slab,
    used: Cell<usize>,
    limit: usize,
    refused: Cell<bool>,
    /// Set for good once an allocation is refused with the heap more than
    /// half full: the page has filled its heap, as opposed to asking for
    /// one absurd block.
    exhausted: Cell<bool>,
    /// The engine's word on whether it is compiling, once [`attach`]ed.
    #[cfg(unix)]
    overdraft: Cell<*mut imp::Overdraft>,
}

impl std::fmt::Debug for HeapState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeapState")
            .field("used", &self.used)
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl HeapState {
    fn new(limit: Option<usize>) -> Self {
        Self {
            #[cfg(unix)]
            slab: imp::Slab::new(),
            used: Cell::new(0),
            limit: limit.filter(|&l| l > 0).unwrap_or(usize::MAX),
            refused: Cell::new(false),
            exhausted: Cell::new(false),
            #[cfg(unix)]
            overdraft: Cell::new(std::ptr::null_mut()),
        }
    }

    /// Bytes allocated and not yet freed, where the heap is dasher's own
    /// (on Unix); the engine counts the same.
    pub(crate) fn used(&self) -> usize {
        self.used.get()
    }

    /// The limit, if there is one.
    pub(crate) fn limit(&self) -> Option<usize> {
        (self.limit != usize::MAX).then_some(self.limit)
    }

    /// Whether the page has filled its heap (see `exhausted`).
    pub(crate) fn exhausted(&self) -> bool {
        self.exhausted.get()
    }

    /// Whether an allocation was refused for exceeding the limit since the
    /// flag was last cleared.
    pub(crate) fn take_refused(&self) -> bool {
        self.refused.replace(false)
    }

    pub(crate) fn clear_refused(&self) {
        self.refused.set(false);
    }

    /// Checks that `size` more bytes fit under the limit, or that the
    /// engine is compiling and gets them anyway.
    #[cfg_attr(
        not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd",
            target_vendor = "apple"
        )),
        allow(dead_code)
    )]
    fn admit(&self, size: usize) -> bool {
        let fits = self
            .used
            .get()
            .checked_add(size)
            .is_some_and(|total| total <= self.limit);
        if !fits {
            self.refused.set(true);
            if self.used.get() > self.limit / 2 {
                self.exhausted.set(true);
            }
            return self.overdraw();
        }
        true
    }

    /// Whether an allocation that does not fit is made all the same: when
    /// the engine is compiling. Tells the engine, which stops the compile.
    #[cfg(unix)]
    fn overdraw(&self) -> bool {
        let overdraft = self.overdraft.get();
        if overdraft.is_null() {
            return false;
        }
        // SAFETY: `attach` set the pointer to a field of the runtime, which
        // is live whenever it allocates, and nothing else holds a reference
        // to the field while the allocator runs.
        unsafe {
            if (*overdraft).allowed == 0 {
                return false;
            }
            (*overdraft).taken = 1;
        }
        true
    }

    #[cfg(not(unix))]
    fn overdraw(&self) -> bool {
        false
    }
}

/// Creates a QuickJS runtime whose heap is capped at `limit` bytes.
pub(crate) fn new_runtime(limit: Option<usize>) -> rquickjs::Result<(Runtime, Rc<HeapState>)> {
    let state = Rc::new(HeapState::new(limit));
    let runtime = imp::new_runtime(&state)?;
    Ok((runtime, state))
}

/// Has the runtime of `ctx` call its heap directly, past `rquickjs`'s
/// allocator wrappers: one call per allocation and free instead of three.
/// `state` must be the one [`new_runtime`] returned with that runtime.
pub(crate) fn attach(ctx: &rquickjs::Ctx<'_>, state: &Rc<HeapState>) {
    imp::attach(ctx, state);
}

#[cfg(unix)]
mod imp {
    use std::cell::UnsafeCell;
    use std::ffi::{c_int, c_void};
    use std::ptr;
    use std::rc::Rc;

    use rquickjs::allocator::Allocator;
    use rquickjs::{Ctx, Runtime, qjs};

    use super::HeapState;
    use crate::slab::{self, SlabHeap};

    /// QuickJS's `JSOverdraft`.
    #[repr(C)]
    pub(crate) struct Overdraft {
        /// Set by the engine while it compiles.
        pub(super) allowed: c_int,
        /// Set here when an allocation went over the limit.
        pub(super) taken: c_int,
    }

    /// The JS heap on dasher's slab allocator, with the limit in front.
    pub(crate) struct Slab(UnsafeCell<SlabHeap>);

    impl Slab {
        pub(super) fn new() -> Self {
            Self(UnsafeCell::new(SlabHeap::new()))
        }
    }

    impl HeapState {
        /// The heap. Only these methods reach it, and none calls back into
        /// QuickJS or another of them while using it, so a `&mut` made from
        /// this pointer is never aliased. The state belongs to the runtime's
        /// thread.
        #[inline]
        fn heap(&self) -> *mut SlabHeap {
            self.slab.0.get()
        }

        #[inline]
        fn track(&self, ptr: *mut u8) -> *mut u8 {
            if !ptr.is_null() {
                // SAFETY: `ptr` was just returned by the heap.
                let size = unsafe { slab::usable_size(ptr) };
                self.used.set(self.used.get() + size);
            }
            ptr
        }

        #[inline]
        fn alloc(&self, size: usize) -> *mut u8 {
            if !self.admit(size) {
                return ptr::null_mut();
            }
            // SAFETY: see `heap`.
            let ptr = unsafe { (*self.heap()).alloc(size) };
            self.track(ptr)
        }

        #[inline]
        fn calloc(&self, count: usize, size: usize) -> *mut u8 {
            let Some(total) = count.checked_mul(size) else {
                return ptr::null_mut();
            };
            let ptr = self.alloc(total);
            if !ptr.is_null() {
                // SAFETY: a fresh block of at least `total` bytes.
                unsafe { ptr::write_bytes(ptr, 0, total) };
            }
            ptr
        }

        /// # Safety
        ///
        /// `ptr` is null or a live block of this heap, not used again.
        #[inline]
        unsafe fn dealloc(&self, ptr: *mut u8) {
            if ptr.is_null() {
                return;
            }
            // SAFETY: per the contract.
            unsafe {
                let size = slab::usable_size(ptr);
                self.used.set(self.used.get().saturating_sub(size));
                (*self.heap()).free(ptr);
            }
        }

        /// # Safety
        ///
        /// `ptr` is null or a live block of this heap; on success it is not
        /// used again.
        #[inline]
        unsafe fn realloc(&self, ptr: *mut u8, new_size: usize) -> *mut u8 {
            if ptr.is_null() {
                return self.alloc(new_size);
            }
            // SAFETY: the caller passes a live pointer from this heap.
            let old_size = unsafe { slab::usable_size(ptr) };
            if new_size > old_size && !self.admit(new_size - old_size) {
                return ptr::null_mut();
            }
            // SAFETY: as above.
            let new = unsafe { (*self.heap()).realloc(ptr, new_size.max(1)) };
            if new.is_null() {
                return ptr::null_mut();
            }
            self.used.set(self.used.get().saturating_sub(old_size));
            self.track(new)
        }
    }

    /// The allocator the runtime is created with. It serves the runtime's
    /// first blocks; [`attach`] then points QuickJS straight at the same
    /// heap, past `rquickjs`'s boxed `dyn Allocator` and its wrappers.
    struct SlabAllocator(Rc<HeapState>);

    // SAFETY: every pointer handed out comes from the state's heap, and is
    // freed, resized and measured only through it.
    unsafe impl Allocator for SlabAllocator {
        fn alloc(&mut self, size: usize) -> *mut u8 {
            self.0.alloc(size)
        }

        fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
            self.0.calloc(count, size)
        }

        unsafe fn dealloc(&mut self, ptr: *mut u8) {
            // SAFETY: per the trait's contract.
            unsafe { self.0.dealloc(ptr) }
        }

        unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
            // SAFETY: per the trait's contract.
            unsafe { self.0.realloc(ptr, new_size) }
        }

        unsafe fn usable_size(ptr: *mut u8) -> usize {
            // SAFETY: the caller passes a live pointer from this allocator.
            unsafe { slab::usable_size(ptr) }
        }
    }

    pub(super) fn new_runtime(state: &Rc<HeapState>) -> rquickjs::Result<Runtime> {
        Runtime::new_with_alloc(SlabAllocator(Rc::clone(state)))
    }

    // The functions QuickJS calls directly once attached. `opaque` is the
    // runtime's `HeapState`.

    unsafe extern "C" fn js_calloc(
        opaque: *mut c_void,
        count: qjs::size_t,
        size: qjs::size_t,
    ) -> *mut c_void {
        // SAFETY: `attach` set `opaque` to a state that outlives the runtime.
        let state = unsafe { &*opaque.cast::<HeapState>() };
        state.calloc(count as usize, size as usize).cast()
    }

    unsafe extern "C" fn js_malloc(opaque: *mut c_void, size: qjs::size_t) -> *mut c_void {
        // SAFETY: as in `js_calloc`.
        let state = unsafe { &*opaque.cast::<HeapState>() };
        state.alloc(size as usize).cast()
    }

    unsafe extern "C" fn js_free(opaque: *mut c_void, ptr: *mut c_void) {
        // SAFETY: as in `js_calloc`; QuickJS frees only its live blocks.
        unsafe { (*opaque.cast::<HeapState>()).dealloc(ptr.cast()) }
    }

    unsafe extern "C" fn js_realloc(
        opaque: *mut c_void,
        ptr: *mut c_void,
        size: qjs::size_t,
    ) -> *mut c_void {
        // SAFETY: as in `js_free`.
        unsafe { (*opaque.cast::<HeapState>()).realloc(ptr.cast(), size as usize) }.cast()
    }

    unsafe extern "C" fn js_malloc_usable_size(ptr: *const c_void) -> qjs::size_t {
        if ptr.is_null() {
            return 0;
        }
        // SAFETY: QuickJS measures only its live blocks.
        unsafe { slab::usable_size(ptr.cast()) as qjs::size_t }
    }

    static FUNCTIONS: qjs::JSMallocFunctions = qjs::JSMallocFunctions {
        js_calloc: Some(js_calloc),
        js_malloc: Some(js_malloc),
        js_free: Some(js_free),
        js_realloc: Some(js_realloc),
        js_malloc_usable_size: Some(js_malloc_usable_size),
    };

    unsafe extern "C" {
        // dasher's additions to QuickJS.
        fn JS_SetMallocFunctions(
            rt: *mut qjs::JSRuntime,
            mf: *const qjs::JSMallocFunctions,
            opaque: *mut c_void,
        );
        fn JS_GetOverdraft(rt: *mut qjs::JSRuntime) -> *mut Overdraft;
    }

    /// Has QuickJS call the heap directly from here on. Every allocation
    /// already goes to `state`'s heap, so blocks made before keep working.
    /// From here on, too, the limit lets the engine's compiler go over it.
    pub(super) fn attach(ctx: &Ctx<'_>, state: &Rc<HeapState>) {
        // SAFETY: `ctx` is live, so its runtime is. The functions manage the
        // same heap as the allocator the runtime was made with, and `state`
        // outlives the runtime: that allocator holds a reference to it until
        // the runtime is freed. The overdraft record is part of the
        // runtime, which makes its last allocator call when it frees itself.
        unsafe {
            let rt = qjs::JS_GetRuntime(ctx.as_raw().as_ptr());
            JS_SetMallocFunctions(rt, &FUNCTIONS, Rc::as_ptr(state).cast_mut().cast());
            state.overdraft.set(JS_GetOverdraft(rt));
        }
    }
}

/// Platforms without `mmap`: QuickJS uses the C allocator and enforces the
/// limit itself, and allocation failures that throw `null` are reported as
/// such.
#[cfg(not(unix))]
mod imp {
    use std::rc::Rc;

    use rquickjs::Runtime;

    use super::HeapState;

    pub(super) fn new_runtime(state: &Rc<HeapState>) -> rquickjs::Result<Runtime> {
        let runtime = Runtime::new()?;
        if state.limit != usize::MAX {
            runtime.set_memory_limit(state.limit);
        }
        Ok(runtime)
    }

    pub(super) fn attach(_ctx: &rquickjs::Ctx<'_>, _state: &Rc<HeapState>) {}
}

/// Compiling and running scripts with almost no heap left.
///
/// A page that fills its heap goes on running until its next interrupt
/// check, and what it compiles then is compiled with whatever room is left:
/// a few bytes or a few kilobytes. Which of the compiler's allocations is
/// the first not to fit depends on that room, so these tests sweep it, from
/// none to enough. Nothing but a JS error may come of it, every block must
/// be freed with the runtime, and a function that could not be compiled
/// must be compiled, and right, once there is room.
#[cfg(all(
    test,
    any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_vendor = "apple"
    )
))]
mod starved {
    use rquickjs::{Context, Ctx, Function, Value};

    use super::*;

    const LIMIT: usize = 16 * 1024 * 1024;

    const FIXTURES: &[(&str, &str)] = &[
        ("lazy.js", include_str!("../tests/fixtures/lazy.js")),
        ("parser.js", include_str!("../tests/fixtures/parser.js")),
        (
            "parameters.js",
            include_str!("../tests/fixtures/parameters.js"),
        ),
        (
            "object_literals.js",
            include_str!("../tests/fixtures/object_literals.js"),
        ),
        ("closures.js", include_str!("../tests/fixtures/closures.js")),
        (
            "lookahead.js",
            include_str!("../tests/fixtures/lookahead.js"),
        ),
        ("eval_var.js", include_str!("../tests/fixtures/eval_var.js")),
    ];

    /// When the script is compiled.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Compile {
        /// With no room: the whole script, as a page's next script is.
        Starved,
        /// With room, as a function not compiled until it is called; then
        /// called with no room.
        OnFirstCall,
    }

    /// Fills the heap until at most `room` bytes are left, and returns how
    /// many are.
    fn starve(ctx: &Ctx<'_>, heap: &HeapState, room: usize) -> usize {
        let pad: Function<'_> = ctx
            .eval(
                "globalThis.padding = [];
                 globalThis.slots = new Array(256).fill(null);
                 (function (n, slot) {
                     if (slot < 0) padding.push(new ArrayBuffer(n));
                     else slots[slot] = 'x'.repeat(n);
                 })",
            )
            .unwrap();
        // Compile it, and leave room in `padding`.
        pad.call::<_, ()>((1, 0)).unwrap();
        for _ in 0..64 {
            pad.call::<_, ()>((1, -1)).unwrap();
        }
        let mut slot = 1;
        loop {
            let free = LIMIT.saturating_sub(heap.used());
            if free <= room {
                return free;
            }
            let spare = free - room;
            // A large block is rounded up to whole pages and a small one to
            // its size class, so close in by halves, then by short strings
            // (one block: a header and the text).
            let done = if spare > 128 * 1024 {
                pad.call::<_, ()>(((spare - 64 * 1024) & !0xffff, -1))
            } else if spare > 512 {
                pad.call::<_, ()>((spare / 2, -1))
            } else if slot < 256 {
                slot += 1;
                pad.call::<_, ()>((spare.saturating_sub(40).clamp(1, 200), slot - 1))
            } else {
                return free;
            };
            if done.is_err() {
                let _ = ctx.catch();
                return LIMIT.saturating_sub(heap.used());
            }
        }
    }

    /// Evaluates `source` as a page's script is: not in strict mode.
    fn sloppy<'js>(ctx: &Ctx<'js>, name: &str, source: &str) -> rquickjs::Result<Value<'js>> {
        let mut options = rquickjs::context::EvalOptions::default();
        options.strict = false;
        options.filename = Some(name.into());
        ctx.eval_with_options(source, options)
    }

    /// What one run found.
    struct Run {
        /// The room there was, in bytes.
        room: usize,
        /// Whether an allocation went over the limit with that room.
        short: bool,
        /// What the script gives once it has room again.
        after: String,
    }

    /// Compiles and runs `source` with about `room` bytes of heap left, then
    /// again with the heap freed.
    fn run(name: &str, source: &str, room: usize, compile: Compile) -> Run {
        let (runtime, heap) = new_runtime(Some(LIMIT)).unwrap();
        let context = Context::full(&runtime).unwrap();
        // Everything used once the heap is full is compiled before it is.
        let found = context.with(|ctx| {
            attach(&ctx, &heap);
            ctx.eval::<(), _>(
                "var padding = 1, slots = 1;
                 function script() {}
                 function quiet() { try { script(); } catch (e) {} }
                 function release() { padding = null; slots = null; }
                 function show(run, value) {
                     try { return 'gives ' + JSON.stringify(run ? script() : value); }
                     catch (e) { return 'throws ' + e; }
                 }
                 quiet(); release(); show(true); show(false, 1);",
            )
            .unwrap();
            let quiet: Function<'_> = ctx.globals().get("quiet").unwrap();
            if compile == Compile::OnFirstCall {
                sloppy(
                    &ctx,
                    name,
                    &format!("script = function () {{\n{source}\n}}"),
                )
                .unwrap();
            }
            let room = starve(&ctx, &heap, room);
            heap.clear_refused();
            match compile {
                Compile::Starved => {
                    if sloppy(&ctx, name, source).is_err() {
                        let _ = ctx.catch();
                    }
                }
                Compile::OnFirstCall => {
                    // Twice: a function that could not be compiled is
                    // compiled again when it is next called.
                    for _ in 0..2 {
                        quiet.call::<_, ()>(()).unwrap();
                    }
                }
            }
            let short = heap.take_refused();

            // With room again, what could not be compiled is.
            let release: Function<'_> = ctx.globals().get("release").unwrap();
            let show: Function<'_> = ctx.globals().get("show").unwrap();
            release.call::<_, ()>(()).unwrap();
            ctx.run_gc();
            let after = match compile {
                Compile::Starved => match sloppy(&ctx, name, source) {
                    Ok(value) => show.call((false, value)).unwrap(),
                    Err(_) => format!("throws {:?}", ctx.catch()),
                },
                Compile::OnFirstCall => show.call((true,)).unwrap(),
            };
            Run { room, short, after }
        });
        drop(context);
        drop(runtime);
        assert_eq!(
            heap.used(),
            0,
            "{name}, {compile:?}, {} bytes: a block was not freed",
            found.room
        );
        found
    }

    fn sweep(compile: Compile) {
        for (name, source) in FIXTURES {
            // What the script gives when it never ran short.
            let expected = run(name, source, usize::MAX, compile);
            assert!(!expected.short, "{name}");
            // From no room to enough for the whole script, or as much as
            // any of these needs to compile.
            for room in (0..128 * 1024).step_by(1021) {
                let found = run(name, source, room, compile);
                if compile == Compile::OnFirstCall {
                    // The functions that could not be compiled then were
                    // compiled since, and do what they should.
                    assert_eq!(
                        found.after, expected.after,
                        "{name}, after {} bytes",
                        found.room
                    );
                }
                if !found.short {
                    break;
                }
            }
        }
    }

    #[test]
    fn a_script_compiled_with_no_room_fails_cleanly() {
        sweep(Compile::Starved);
    }

    #[test]
    fn a_function_first_called_with_no_room_fails_cleanly() {
        sweep(Compile::OnFirstCall);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_refuses_past_the_limit_and_remembers() {
        let heap = HeapState::new(Some(100));
        heap.used.set(90);
        assert!(heap.admit(10));
        assert!(!heap.take_refused());
        assert!(!heap.admit(11));
        assert!(heap.take_refused());
        assert!(!heap.take_refused());
        assert!(!heap.admit(usize::MAX));
        assert!(heap.exhausted(), "refused with the heap 90% full");
    }

    #[test]
    fn one_huge_request_does_not_exhaust_a_roomy_heap() {
        let heap = HeapState::new(Some(100));
        heap.used.set(40);
        assert!(!heap.admit(1000));
        assert!(heap.take_refused());
        assert!(!heap.exhausted());
        heap.used.set(51);
        assert!(!heap.admit(50));
        assert!(heap.exhausted());
    }

    #[test]
    fn no_limit_admits_everything_that_fits_in_usize() {
        let heap = HeapState::new(None);
        assert!(heap.admit(usize::MAX));
        assert_eq!(HeapState::new(Some(0)).limit, usize::MAX);
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_vendor = "apple"
    ))]
    #[test]
    fn tracks_usage_across_a_runtime_lifetime() {
        let (runtime, heap) = new_runtime(Some(64 * 1024 * 1024)).unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let before = heap.used.get();
        assert!(before > 0);
        context.with(|ctx| {
            ctx.eval::<(), _>("globalThis.big = 'x'.repeat(1 << 20)")
                .unwrap()
        });
        assert!(heap.used.get() >= before + (1 << 20));
        context.with(|ctx| ctx.eval::<(), _>("delete globalThis.big").unwrap());
        assert!(heap.used.get() < before + (1 << 20));
        drop(context);
        drop(runtime);
        assert_eq!(heap.used.get(), 0, "everything allocated was freed");
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_vendor = "apple"
    ))]
    #[test]
    fn attached_runtime_keeps_the_books_and_the_limit() {
        let (runtime, heap) = new_runtime(Some(16 * 1024 * 1024)).unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        // Blocks made before attaching are freed through the direct path.
        context.with(|ctx| {
            ctx.eval::<(), _>("globalThis.early = 'e'.repeat(1 << 16)")
                .unwrap();
            attach(&ctx, &heap);
        });
        let before = heap.used.get();
        context.with(|ctx| {
            ctx.eval::<(), _>(
                "globalThis.big = 'x'.repeat(1 << 20); \
                 globalThis.list = Array.from({ length: 10000 }, (_, i) => ({ i }))",
            )
            .unwrap()
        });
        assert!(heap.used.get() >= before + (1 << 20));
        context.with(|ctx| {
            ctx.eval::<(), _>(
                "delete globalThis.big; delete globalThis.list; delete globalThis.early",
            )
            .unwrap()
        });
        assert!(heap.used.get() < before + (1 << 20));
        // The limit still holds on the direct path.
        let refused =
            context.with(|ctx| ctx.eval::<(), _>("'y'.repeat(32 * 1024 * 1024)").is_err());
        assert!(refused);
        assert!(heap.take_refused());
        drop(context);
        drop(runtime);
        assert_eq!(heap.used.get(), 0, "everything allocated was freed");
    }
}
