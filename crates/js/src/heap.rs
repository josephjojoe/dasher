//! The JS heap allocator: dasher's slab allocator (see [`crate::slab`])
//! in place of the C allocator QuickJS uses by default, with dasher's
//! memory limit enforced here instead of inside QuickJS.
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

#[cfg(unix)]
mod imp {
    use std::ptr;
    use std::rc::Rc;

    use rquickjs::Runtime;
    use rquickjs::allocator::Allocator;

    use super::HeapState;

    /// The JS heap on dasher's slab allocator, with the limit in front.
    struct SlabAllocator {
        heap: crate::slab::SlabHeap,
        state: Rc<HeapState>,
    }

    impl SlabAllocator {
        #[inline]
        fn track(&self, ptr: *mut u8) -> *mut u8 {
            if !ptr.is_null() {
                // SAFETY: `ptr` was just returned by the heap.
                let size = unsafe { crate::slab::usable_size(ptr) };
                self.state.used.set(self.state.used.get() + size);
            }
            ptr
        }
    }

    // SAFETY: every pointer handed out comes from `heap`, and is freed,
    // resized and measured only through it.
    unsafe impl Allocator for SlabAllocator {
        fn alloc(&mut self, size: usize) -> *mut u8 {
            if !self.state.admit(size) {
                return ptr::null_mut();
            }
            let ptr = self.heap.alloc(size);
            self.track(ptr)
        }

        fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
            let Some(total) = count.checked_mul(size) else {
                return ptr::null_mut();
            };
            if !self.state.admit(total) {
                return ptr::null_mut();
            }
            let ptr = self.heap.alloc(total);
            if !ptr.is_null() {
                // SAFETY: a fresh block of at least `total` bytes.
                unsafe { ptr::write_bytes(ptr, 0, total) };
            }
            self.track(ptr)
        }

        unsafe fn dealloc(&mut self, ptr: *mut u8) {
            if ptr.is_null() {
                return;
            }
            // SAFETY: the caller passes a live pointer from this allocator,
            // which is not used again after this call.
            unsafe {
                let size = crate::slab::usable_size(ptr);
                self.state
                    .used
                    .set(self.state.used.get().saturating_sub(size));
                self.heap.free(ptr);
            }
        }

        unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
            if ptr.is_null() {
                return self.alloc(new_size);
            }
            // SAFETY: the caller passes a live pointer from this allocator.
            let old_size = unsafe { crate::slab::usable_size(ptr) };
            if new_size > old_size && !self.state.admit(new_size - old_size) {
                return ptr::null_mut();
            }
            // SAFETY: as above; on success the old pointer is no longer used.
            let new = unsafe { self.heap.realloc(ptr, new_size.max(1)) };
            if new.is_null() {
                return ptr::null_mut();
            }
            self.state
                .used
                .set(self.state.used.get().saturating_sub(old_size));
            self.track(new)
        }

        unsafe fn usable_size(ptr: *mut u8) -> usize {
            // SAFETY: the caller passes a live pointer from this allocator.
            unsafe { crate::slab::usable_size(ptr) }
        }
    }

    pub(super) fn new_runtime(state: &Rc<HeapState>) -> rquickjs::Result<Runtime> {
        Runtime::new_with_alloc(SlabAllocator {
            heap: crate::slab::SlabHeap::new(),
            state: Rc::clone(state),
        })
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
