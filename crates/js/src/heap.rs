//! The JS heap allocator: the C allocator QuickJS uses by default, with
//! dasher's memory limit enforced here instead of inside QuickJS.
//!
//! QuickJS checks its own limit before calling the allocator and, when it
//! cannot even allocate the error object, throws `null`, which is
//! indistinguishable from a script's `throw null`. Enforcing the limit here
//! lets the runtime record that an allocation was refused and report the
//! failure as out of memory.

use std::cell::Cell;
use std::rc::Rc;

use rquickjs::Runtime;

/// Heap accounting shared between the allocator and the runtime.
#[derive(Debug)]
pub(crate) struct HeapState {
    used: Cell<usize>,
    limit: usize,
    refused: Cell<bool>,
    /// Set for good once an allocation is refused with the heap more than
    /// half full: the page has filled its heap, as opposed to asking for
    /// one absurd block.
    exhausted: Cell<bool>,
}

impl HeapState {
    fn new(limit: Option<usize>) -> Self {
        Self {
            used: Cell::new(0),
            limit: limit.filter(|&l| l > 0).unwrap_or(usize::MAX),
            refused: Cell::new(false),
            exhausted: Cell::new(false),
        }
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

    /// Checks that `size` more bytes fit under the limit.
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
        }
        fits
    }
}

/// Creates a QuickJS runtime whose heap is capped at `limit` bytes.
pub(crate) fn new_runtime(limit: Option<usize>) -> rquickjs::Result<(Runtime, Rc<HeapState>)> {
    let state = Rc::new(HeapState::new(limit));
    let runtime = imp::new_runtime(&state)?;
    Ok((runtime, state))
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_vendor = "apple"
))]
mod imp {
    use std::ffi::c_void;
    use std::ptr;
    use std::rc::Rc;

    use rquickjs::Runtime;
    use rquickjs::allocator::Allocator;

    use super::HeapState;

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut c_void;
        fn calloc(count: usize, size: usize) -> *mut c_void;
        fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
        fn free(ptr: *mut c_void);
        #[cfg_attr(target_vendor = "apple", link_name = "malloc_size")]
        #[cfg_attr(not(target_vendor = "apple"), link_name = "malloc_usable_size")]
        fn usable_size(ptr: *const c_void) -> usize;
    }

    struct LimitedAllocator(Rc<HeapState>);

    impl LimitedAllocator {
        fn track(&self, ptr: *mut c_void) -> *mut u8 {
            if !ptr.is_null() {
                // SAFETY: `ptr` was just returned by the C allocator.
                let size = unsafe { usable_size(ptr) };
                self.0.used.set(self.0.used.get() + size);
            }
            ptr.cast()
        }
    }

    // SAFETY: every pointer handed out comes from the C allocator, and is
    // freed, resized and measured only through it.
    unsafe impl Allocator for LimitedAllocator {
        fn alloc(&mut self, size: usize) -> *mut u8 {
            if !self.0.admit(size) {
                return ptr::null_mut();
            }
            // SAFETY: `malloc` has no preconditions; a null result is passed
            // through as an allocation failure.
            self.track(unsafe { malloc(size) })
        }

        fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
            let Some(total) = count.checked_mul(size) else {
                return ptr::null_mut();
            };
            if !self.0.admit(total) {
                return ptr::null_mut();
            }
            // SAFETY: `calloc` has no preconditions; a null result is passed
            // through as an allocation failure.
            self.track(unsafe { calloc(count, size) })
        }

        unsafe fn dealloc(&mut self, ptr: *mut u8) {
            if ptr.is_null() {
                return;
            }
            // SAFETY: the caller passes a live pointer from this allocator.
            let size = unsafe { usable_size(ptr.cast()) };
            self.0.used.set(self.0.used.get().saturating_sub(size));
            // SAFETY: as above; it is not used again after this call.
            unsafe { free(ptr.cast()) };
        }

        unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
            // SAFETY: the caller passes a live pointer from this allocator.
            let old_size = unsafe { usable_size(ptr.cast()) };
            if new_size > old_size && !self.0.admit(new_size - old_size) {
                return ptr::null_mut();
            }
            // SAFETY: as above; on success the old pointer is no longer used.
            let new = unsafe { realloc(ptr.cast(), new_size) };
            if new.is_null() {
                return ptr::null_mut();
            }
            self.0.used.set(self.0.used.get().saturating_sub(old_size));
            self.track(new)
        }

        unsafe fn usable_size(ptr: *mut u8) -> usize {
            // SAFETY: the caller passes a live pointer from this allocator.
            unsafe { usable_size(ptr.cast()) }
        }
    }

    pub(super) fn new_runtime(state: &Rc<HeapState>) -> rquickjs::Result<Runtime> {
        Runtime::new_with_alloc(LimitedAllocator(Rc::clone(state)))
    }
}

/// Platforms without `malloc_usable_size`: QuickJS enforces the limit itself
/// and allocation failures that throw `null` are reported as such.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_vendor = "apple"
)))]
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
}
