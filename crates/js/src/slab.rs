//! The memory behind a JS heap: a size-class slab allocator.
//!
//! QuickJS allocates constantly (every object, shape, property table and
//! string is its own block) and asks the allocator for the size of a block
//! on every allocation and free, to keep its books. A general-purpose
//! `malloc` pays for thread safety and for finding a block's size on each of
//! those calls. On real pages that came to a quarter of all CPU time.
//!
//! A JS heap needs much less: it is used from one thread, most blocks are
//! small, and everything dies together when the page is dropped. So:
//!
//! - The process reserves one span of address space (no memory) and hands
//!   it out in 1 MiB *regions*. A heap takes regions as it grows and gives
//!   them all back, memory included, when it is dropped.
//! - A region is cut into 64 KiB *slabs*. A slab holds blocks of one size
//!   class and starts with a small header, so the size of a block is one
//!   load away from its address: mask the address down to the slab. There
//!   are no per-block headers.
//! - Allocating pops the slab's free list or bumps its high-water mark;
//!   freeing pushes onto the free list. A slab whose blocks are all free
//!   goes back to the heap's pool for any size class to reuse.
//! - Blocks over [`MAX_SMALL`] come from the system allocator with a header
//!   in front. An address inside the span is a slab block; any other is one
//!   of these.
//!
//! If the span cannot be reserved (an address-space limit), every block
//! takes the system path, which is correct and merely slower.

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const SLAB_SHIFT: usize = 16;
/// Bytes in a slab.
const SLAB: usize = 1 << SLAB_SHIFT;
/// Slabs in a region.
const REGION_SLABS: usize = 16;
/// Bytes in a region.
const REGION: usize = SLAB * REGION_SLABS;
/// The largest block served from slabs.
pub(crate) const MAX_SMALL: usize = 8192;
/// Size classes: 16 to 128 in steps of 16, then four per doubling.
const CLASSES: usize = 32;
/// Bytes at the start of a slab kept for its header.
const SLAB_HEADER: usize = 64;
/// Bytes in front of a system block kept for its header.
const LARGE_HEADER: usize = 32;

/// Address space to try to reserve, largest first.
const SPAN_SIZES: [usize; 4] = [64 << 30, 16 << 30, 4 << 30, 1 << 30];

const fn class_sizes() -> [u32; CLASSES] {
    let mut sizes = [0u32; CLASSES];
    let mut i = 0;
    while i < 8 {
        sizes[i] = (i as u32 + 1) * 16;
        i += 1;
    }
    let mut base = 128u32;
    while i < CLASSES {
        let step = base / 4;
        let mut k = 1;
        while k <= 4 {
            sizes[i] = base + step * k;
            i += 1;
            k += 1;
        }
        base *= 2;
    }
    sizes
}

const CLASS_SIZE: [u32; CLASSES] = class_sizes();

/// The size class for a request of `size` bytes, `1..=MAX_SMALL`.
#[inline]
fn class_of(size: usize) -> usize {
    if size <= 128 {
        size.div_ceil(16) - 1
    } else {
        let s = size - 1;
        let log = usize::BITS as usize - 1 - s.leading_zeros() as usize;
        8 + (log - 7) * 4 + ((s >> (log - 2)) & 3)
    }
}

/// A free block: the link to the next one in its slab.
#[repr(C)]
struct Free {
    next: *mut Free,
}

/// The header at the start of every slab.
#[repr(C)]
struct Slab {
    /// The size of this slab's blocks.
    block_size: u32,
    class: u32,
    /// Blocks that were allocated and freed again.
    free: *mut Free,
    /// Offset of the first block never handed out.
    bump: u32,
    /// Blocks handed out and not freed.
    used: u32,
    /// Links in the class's list of slabs with room, or (`next` only) in
    /// the pool of empty slabs.
    next: *mut Slab,
    prev: *mut Slab,
    /// Whether the slab is in its class's list.
    listed: bool,
}

const _: () = assert!(size_of::<Slab>() <= SLAB_HEADER);

/// The header in front of a block from the system allocator.
#[repr(C)]
struct Large {
    /// The block's usable size: what was asked for, plus whatever the
    /// system allocator rounded it up by. QuickJS grows strings in place
    /// into that slack.
    size: usize,
    prev: *mut Large,
    next: *mut Large,
    _pad: usize,
}

const _: () = assert!(size_of::<Large>() == LARGE_HEADER);

// ---------------------------------------------------------------------------
// The process-wide span of reserved address space.

/// Start of the span; 0 until it is reserved, and for good if it cannot be.
static SPAN_BASE: AtomicUsize = AtomicUsize::new(0);
/// Its length in bytes.
static SPAN_LEN: AtomicUsize = AtomicUsize::new(0);

struct Regions {
    /// Whether reserving has been tried.
    tried: bool,
    /// Regions never handed out start here.
    fresh: usize,
    /// Regions handed back, their memory released.
    returned: Vec<u32>,
}

static REGIONS: Mutex<Regions> = Mutex::new(Regions {
    tried: false,
    fresh: 0,
    returned: Vec::new(),
});

fn reserve_span() {
    for size in SPAN_SIZES {
        // SAFETY: an anonymous mapping at an address of the kernel's
        // choosing, with no access: it claims address space only.
        let raw = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size + SLAB,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON | NORESERVE,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            continue;
        }
        let base = (raw as usize + SLAB - 1) & !(SLAB - 1);
        // Callers hold the `REGIONS` lock, which orders these stores before
        // any thread's loads in `in_span` (see `settle_span`).
        SPAN_BASE.store(base, Ordering::Relaxed);
        SPAN_LEN.store(size, Ordering::Relaxed);
        return;
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const NORESERVE: libc::c_int = libc::MAP_NORESERVE;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const NORESERVE: libc::c_int = 0;

/// Reserves the span if that has not been tried yet. Taking the lock also
/// orders the span's base and length before anything this thread does
/// next, which [`in_span`] relies on.
fn settle_span() {
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    if !regions.tried {
        regions.tried = true;
        reserve_span();
    }
}

/// Takes a region and makes it readable and writable. Its memory reads as
/// zero. `None` when the span is used up or was never reserved.
fn take_region() -> Option<*mut u8> {
    let index = {
        let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
        if !regions.tried {
            regions.tried = true;
            reserve_span();
        }
        let len = SPAN_LEN.load(Ordering::Relaxed);
        if len == 0 {
            return None;
        }
        match regions.returned.pop() {
            Some(index) => index as usize,
            None if (regions.fresh + 1) * REGION <= len => {
                regions.fresh += 1;
                regions.fresh - 1
            }
            None => return None,
        }
    };
    let region = (SPAN_BASE.load(Ordering::Relaxed) + index * REGION) as *mut u8;
    // SAFETY: the region is inside the reserved span and owned by the
    // caller from here on.
    let ok =
        unsafe { libc::mprotect(region.cast(), REGION, libc::PROT_READ | libc::PROT_WRITE) } == 0;
    if ok {
        Some(region)
    } else {
        give_region(region);
        None
    }
}

/// Releases a region's memory and makes it available again.
fn give_region(region: *mut u8) {
    // SAFETY: nothing uses the region any more. Mapping over it drops its
    // pages and leaves it reserved, inaccessible and zero-filled.
    let remapped = unsafe {
        libc::mmap(
            region.cast(),
            REGION,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED | NORESERVE,
            -1,
            0,
        )
    };
    if remapped == libc::MAP_FAILED {
        // The region keeps its old contents and protection; leave it out
        // of circulation, since a fresh region is assumed to be zeroed.
        return;
    }
    let index = (region as usize - SPAN_BASE.load(Ordering::Relaxed)) / REGION;
    let mut regions = REGIONS.lock().unwrap_or_else(|e| e.into_inner());
    regions.returned.push(index as u32);
}

/// How many bytes the system allocator really gave for a block of `asked`
/// bytes: never fewer.
///
/// # Safety
///
/// `block` must be a live block from `malloc` or `realloc`.
#[inline]
unsafe fn system_usable(block: *mut c_void, asked: usize) -> usize {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_vendor = "apple"
    ))]
    {
        unsafe extern "C" {
            #[cfg_attr(target_vendor = "apple", link_name = "malloc_size")]
            #[cfg_attr(not(target_vendor = "apple"), link_name = "malloc_usable_size")]
            fn usable_size(ptr: *const c_void) -> usize;
        }
        // SAFETY: per the contract.
        unsafe { usable_size(block) }.max(asked)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_vendor = "apple"
    )))]
    {
        let _ = block;
        asked
    }
}

/// Whether `ptr` is a slab block, as opposed to a system block.
///
/// Called only on threads that have made a [`SlabHeap`], whose `new`
/// settles the span under the `REGIONS` lock first. That lock orders the
/// span's base and length before these loads, so they need no ordering of
/// their own: this runs on every allocation, free and size query.
#[inline]
fn in_span(ptr: *const u8) -> bool {
    // Zero if the span could not be reserved, which makes every block a
    // system block, as it then is.
    let len = SPAN_LEN.load(Ordering::Relaxed);
    (ptr as usize).wrapping_sub(SPAN_BASE.load(Ordering::Relaxed)) < len
}

#[inline]
fn slab_of(ptr: *const u8) -> *mut Slab {
    (ptr as usize & !(SLAB - 1)) as *mut Slab
}

/// The usable size of a block from any [`SlabHeap`].
///
/// # Safety
///
/// `ptr` must be a live block returned by a `SlabHeap`.
#[inline]
pub(crate) unsafe fn usable_size(ptr: *const u8) -> usize {
    if in_span(ptr) {
        // SAFETY: a block in the span lies in a slab with a live header.
        unsafe { (*slab_of(ptr)).block_size as usize }
    } else {
        // SAFETY: any other block has a `Large` header in front of it.
        unsafe { (*ptr.cast::<Large>().sub(1)).size }
    }
}

// ---------------------------------------------------------------------------
// A heap.

/// One JS heap's memory. Not thread-safe: a heap belongs to the thread
/// running its runtime.
pub(crate) struct SlabHeap {
    /// Per class, the slabs that have room.
    avail: [*mut Slab; CLASSES],
    /// Slabs with no live blocks, for any class to take.
    pool: *mut Slab,
    /// The rest of the newest region, not yet cut into slabs.
    fresh: *mut u8,
    fresh_end: *mut u8,
    regions: Vec<*mut u8>,
    /// Blocks from the system allocator, so that dropping the heap frees
    /// the ones still live.
    large: *mut Large,
}

impl SlabHeap {
    pub(crate) fn new() -> Self {
        settle_span();
        Self {
            avail: [ptr::null_mut(); CLASSES],
            pool: ptr::null_mut(),
            fresh: ptr::null_mut(),
            fresh_end: ptr::null_mut(),
            regions: Vec::new(),
            large: ptr::null_mut(),
        }
    }

    /// Allocates `size` bytes, aligned to 16. Null on failure.
    #[inline(always)]
    pub(crate) fn alloc(&mut self, size: usize) -> *mut u8 {
        if size > MAX_SMALL {
            return self.alloc_large(size);
        }
        let class = class_of(size.max(1));
        let slab = self.avail[class];
        if slab.is_null() {
            return self.alloc_slow(class, size);
        }
        // SAFETY: slabs in `avail` are live, belong to this heap and have
        // room: a free block, or space past the high-water mark.
        unsafe {
            let block = (*slab).free;
            let block = if block.is_null() {
                let block = slab.cast::<u8>().add((*slab).bump as usize);
                (*slab).bump += (*slab).block_size;
                block
            } else {
                (*slab).free = (*block).next;
                block.cast::<u8>()
            };
            (*slab).used += 1;
            if (*slab).free.is_null() && (*slab).bump + (*slab).block_size > SLAB as u32 {
                self.unlist(slab);
            }
            block
        }
    }

    #[cold]
    fn alloc_slow(&mut self, class: usize, size: usize) -> *mut u8 {
        let slab = if self.pool.is_null() {
            if self.fresh == self.fresh_end {
                let Some(region) = take_region() else {
                    // No span, or it is full: use the system allocator.
                    return self.alloc_large(size);
                };
                self.regions.push(region);
                self.fresh = region;
                // SAFETY: one past the end of the region.
                self.fresh_end = unsafe { region.add(REGION) };
            }
            let slab = self.fresh.cast::<Slab>();
            // SAFETY: stays within the region, or lands on its end.
            self.fresh = unsafe { self.fresh.add(SLAB) };
            slab
        } else {
            let slab = self.pool;
            // SAFETY: pooled slabs are live and linked through `next`.
            self.pool = unsafe { (*slab).next };
            slab
        };
        // SAFETY: `slab` is 64 KiB of writable memory owned by this heap
        // with no live blocks.
        unsafe {
            slab.write(Slab {
                block_size: CLASS_SIZE[class],
                class: class as u32,
                free: ptr::null_mut(),
                bump: SLAB_HEADER as u32,
                used: 0,
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
                listed: false,
            });
            self.list(slab);
        }
        self.alloc(size)
    }

    /// Puts `slab` at the head of its class's list.
    ///
    /// # Safety
    ///
    /// `slab` is a live slab of this heap that is not in the list.
    #[inline]
    unsafe fn list(&mut self, slab: *mut Slab) {
        // SAFETY: per the contract; list neighbours are live slabs.
        unsafe {
            let head = &mut self.avail[(*slab).class as usize];
            (*slab).prev = ptr::null_mut();
            (*slab).next = *head;
            if !head.is_null() {
                (**head).prev = slab;
            }
            *head = slab;
            (*slab).listed = true;
        }
    }

    /// Takes `slab` out of its class's list.
    ///
    /// # Safety
    ///
    /// `slab` is a live slab of this heap that is in the list.
    #[inline]
    unsafe fn unlist(&mut self, slab: *mut Slab) {
        // SAFETY: per the contract; list neighbours are live slabs.
        unsafe {
            let (prev, next) = ((*slab).prev, (*slab).next);
            if prev.is_null() {
                self.avail[(*slab).class as usize] = next;
            } else {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
            (*slab).listed = false;
        }
    }

    /// Frees a block.
    ///
    /// # Safety
    ///
    /// `ptr` must be a live block from this heap.
    #[inline(always)]
    pub(crate) unsafe fn free(&mut self, ptr: *mut u8) {
        if !in_span(ptr) {
            // SAFETY: a block outside the span is one of this heap's
            // system blocks.
            unsafe { self.free_large(ptr) };
            return;
        }
        let slab = slab_of(ptr);
        // SAFETY: the block lies in a live slab of this heap.
        unsafe {
            let block = ptr.cast::<Free>();
            (*block).next = (*slab).free;
            (*slab).free = block;
            (*slab).used -= 1;
            if (*slab).used == 0 {
                // Keep one slab per class at hand, so a class that
                // alternates between one block and none does not churn.
                let only = (*slab).listed && (*slab).prev.is_null() && (*slab).next.is_null();
                if !only {
                    if (*slab).listed {
                        self.unlist(slab);
                    }
                    (*slab).next = self.pool;
                    self.pool = slab;
                }
            } else if !(*slab).listed {
                self.list(slab);
            }
        }
    }

    /// Resizes a block, moving it if it must. Null on failure, with the
    /// block left as it was.
    ///
    /// # Safety
    ///
    /// `ptr` must be a live block from this heap, and `size` not zero.
    pub(crate) unsafe fn realloc(&mut self, ptr: *mut u8, size: usize) -> *mut u8 {
        if in_span(ptr) {
            // SAFETY: the block lies in a live slab.
            let old = unsafe { (*slab_of(ptr)).block_size as usize };
            // Stay put unless the block must grow or would fit a smaller
            // class, which gives the difference back.
            if size <= old && CLASS_SIZE[class_of(size.max(1))] as usize == old {
                return ptr;
            }
            let new = self.alloc(size);
            if !new.is_null() {
                // SAFETY: both blocks are live and at least this long.
                unsafe {
                    ptr::copy_nonoverlapping(ptr, new, old.min(size));
                    self.free(ptr);
                }
            }
            return new;
        }
        if size <= MAX_SMALL {
            // A small size: move it into a slab.
            let new = self.alloc(size);
            if !new.is_null() {
                // SAFETY: `ptr` is a system block with its header in front,
                // and both blocks are at least this long.
                unsafe {
                    let old = (*ptr.cast::<Large>().sub(1)).size;
                    ptr::copy_nonoverlapping(ptr, new, old.min(size));
                    self.free_large(ptr);
                }
            }
            return new;
        }
        // SAFETY: `ptr` is a system block: its header is in front of it,
        // and both came from `malloc`/`realloc`.
        unsafe {
            let header = ptr.cast::<Large>().sub(1);
            let Some(total) = size.checked_add(LARGE_HEADER) else {
                return ptr::null_mut();
            };
            let moved = libc::realloc(header.cast(), total).cast::<Large>();
            if moved.is_null() {
                return ptr::null_mut();
            }
            (*moved).size = system_usable(moved.cast(), total) - LARGE_HEADER;
            // The neighbours still point at the old address.
            if (*moved).prev.is_null() {
                self.large = moved;
            } else {
                (*(*moved).prev).next = moved;
            }
            if !(*moved).next.is_null() {
                (*(*moved).next).prev = moved;
            }
            moved.add(1).cast()
        }
    }

    #[cold]
    fn alloc_large(&mut self, size: usize) -> *mut u8 {
        let Some(total) = size.checked_add(LARGE_HEADER) else {
            return ptr::null_mut();
        };
        // SAFETY: `malloc` has no preconditions.
        let header = unsafe { libc::malloc(total) }.cast::<Large>();
        if header.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: `header` is a fresh block with room for the header, and
        // the list holds live system blocks.
        unsafe {
            header.write(Large {
                size: system_usable(header.cast(), total) - LARGE_HEADER,
                prev: ptr::null_mut(),
                next: self.large,
                _pad: 0,
            });
            if !self.large.is_null() {
                (*self.large).prev = header;
            }
            self.large = header;
            header.add(1).cast()
        }
    }

    /// # Safety
    ///
    /// `ptr` must be a live system block from this heap.
    #[cold]
    unsafe fn free_large(&mut self, ptr: *mut u8) {
        // SAFETY: the header is in front of the block and the list holds
        // live system blocks.
        unsafe {
            let header = ptr.cast::<Large>().sub(1);
            let (prev, next) = ((*header).prev, (*header).next);
            if prev.is_null() {
                self.large = next;
            } else {
                (*prev).next = next;
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
            libc::free(header.cast::<c_void>());
        }
    }
}

impl Drop for SlabHeap {
    fn drop(&mut self) {
        let mut large = self.large;
        while !large.is_null() {
            // SAFETY: the list holds live system blocks; nothing uses the
            // heap's memory after it is dropped.
            unsafe {
                let next = (*large).next;
                libc::free(large.cast::<c_void>());
                large = next;
            }
        }
        for &region in &self.regions {
            give_region(region);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_cover_every_small_size_tightly() {
        assert_eq!(CLASS_SIZE[0], 16);
        assert_eq!(CLASS_SIZE[7], 128);
        assert_eq!(CLASS_SIZE[8], 160);
        assert_eq!(CLASS_SIZE[CLASSES - 1] as usize, MAX_SMALL);
        for size in 1..=MAX_SMALL {
            let class = class_of(size);
            let block = CLASS_SIZE[class] as usize;
            assert!(block >= size, "{size} fits its class");
            if class > 0 {
                assert!(
                    (CLASS_SIZE[class - 1] as usize) < size,
                    "{size} is in the smallest class that fits"
                );
            }
            assert!(
                block <= size + size / 4 + 16,
                "at most a quarter wasted for {size}"
            );
        }
    }

    #[test]
    fn blocks_are_aligned_distinct_and_sized() {
        let mut heap = SlabHeap::new();
        let mut seen = std::collections::HashSet::new();
        for size in [
            1, 16, 17, 48, 100, 128, 129, 1000, 4096, 8192, 8193, 100_000,
        ] {
            for _ in 0..50 {
                let p = heap.alloc(size);
                assert!(!p.is_null());
                assert_eq!(p as usize % 16, 0);
                assert!(seen.insert(p as usize));
                // SAFETY: `p` is a live block of at least `size` bytes.
                unsafe {
                    assert!(usable_size(p) >= size);
                    ptr::write_bytes(p, 0xAB, usable_size(p));
                }
            }
        }
    }

    /// A deterministic pseudo-random stream.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as usize
        }
    }

    /// Allocates, resizes and frees at random, checking that every block
    /// keeps the bytes written to it: blocks never overlap and resizing
    /// preserves contents.
    #[test]
    fn random_churn_keeps_block_contents() {
        let mut heap = SlabHeap::new();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
        let check = |p: *mut u8, len: usize, tag: u8| {
            // SAFETY: `p` is a live block of at least `len` bytes.
            let bytes = unsafe { std::slice::from_raw_parts(p, len) };
            assert!(bytes.iter().all(|&b| b == tag), "block contents changed");
        };
        let pick_size = |rng: &mut Rng| match rng.next() % 20 {
            0 => 1 + rng.next() % 200_000,
            1..=4 => 1 + rng.next() % 10_000,
            _ => 1 + rng.next() % 300,
        };
        for step in 0..200_000 {
            let roll = rng.next() % 100;
            if live.is_empty() || roll < 45 {
                let size = pick_size(&mut rng);
                let tag = (step % 251) as u8;
                let p = heap.alloc(size);
                assert!(!p.is_null());
                // SAFETY: a fresh block of `size` bytes.
                unsafe { ptr::write_bytes(p, tag, size) };
                live.push((p, size, tag));
            } else if roll < 65 {
                let i = rng.next() % live.len();
                let (p, len, tag) = live[i];
                check(p, len, tag);
                let size = pick_size(&mut rng);
                // SAFETY: `p` is live and `size` is not zero.
                let q = unsafe { heap.realloc(p, size) };
                assert!(!q.is_null());
                check(q, len.min(size), tag);
                // SAFETY: `q` is a live block of at least `size` bytes.
                unsafe {
                    assert!(usable_size(q) >= size);
                    ptr::write_bytes(q, tag, size);
                }
                live[i] = (q, size, tag);
            } else {
                let i = rng.next() % live.len();
                let (p, len, tag) = live.swap_remove(i);
                check(p, len, tag);
                // SAFETY: `p` is live and not used again.
                unsafe { heap.free(p) };
            }
        }
        for (p, len, tag) in live {
            check(p, len, tag);
        }
    }

    #[test]
    fn freed_memory_is_reused_and_regions_come_back() {
        let mut heap = SlabHeap::new();
        let blocks: Vec<_> = (0..10_000).map(|_| heap.alloc(64)).collect();
        let regions = heap.regions.len();
        for &p in &blocks {
            // SAFETY: each block is live and freed once.
            unsafe { heap.free(p) };
        }
        // The same memory serves another size class.
        for _ in 0..5_000 {
            assert!(!heap.alloc(128).is_null());
        }
        assert_eq!(heap.regions.len(), regions, "empty slabs were reused");
        let first = heap.regions.first().copied();
        drop(heap);
        if let Some(first) = first {
            // A new heap gets a returned region back, reading as zero.
            let mut heap = SlabHeap::new();
            let p = heap.alloc(64);
            assert!(heap.regions.contains(&first) || !in_span(p) || heap.regions.len() == 1);
        }
    }

    #[test]
    fn heaps_do_not_share_memory() {
        let mut a = SlabHeap::new();
        let mut b = SlabHeap::new();
        let pa = a.alloc(32);
        let pb = b.alloc(32);
        assert_ne!(pa, pb);
        if in_span(pa) && in_span(pb) {
            let base = SPAN_BASE.load(Ordering::Relaxed);
            assert_ne!((pa as usize - base) / REGION, (pb as usize - base) / REGION);
        }
    }
}
