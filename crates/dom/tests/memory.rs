//! Measures and bounds the heap cost of DOM nodes.
//!
//! A counting global allocator records live heap bytes and allocations, so
//! the numbers are what the tree really costs, not just `size_of`. Run with
//! `cargo test -p dasher-dom --test memory -- --nocapture` to see the report.
//!
//! Byte counts are requested sizes; the system allocator adds its own
//! per-allocation overhead (typically 8-16 bytes) on top, which is why live
//! allocations are reported too.
//!
//! Everything runs in one test so that no other test allocates concurrently.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

use dasher_dom::Document;

struct Counting;

static BYTES: AtomicIsize = AtomicIsize::new(0);
static ALLOCS: AtomicIsize = AtomicIsize::new(0);

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged and only updates counters on the side.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            BYTES.fetch_add(layout.size() as isize, Relaxed);
            ALLOCS.fetch_add(1, Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            BYTES.fetch_add(layout.size() as isize, Relaxed);
            ALLOCS.fetch_add(1, Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        BYTES.fetch_sub(layout.size() as isize, Relaxed);
        ALLOCS.fetch_sub(1, Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            BYTES.fetch_add(new_size as isize - layout.size() as isize, Relaxed);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Live heap (bytes, allocations) used by whatever `f` returns.
fn measure<T>(f: impl FnOnce() -> T) -> (T, isize, isize) {
    let (b0, a0) = (BYTES.load(Relaxed), ALLOCS.load(Relaxed));
    let value = f();
    let (b1, a1) = (BYTES.load(Relaxed), ALLOCS.load(Relaxed));
    (value, b1 - b0, a1 - a0)
}

#[test]
fn per_node_memory_cost() {
    const N: usize = 100_000;
    println!("dasher-dom per-node memory");

    // Bare elements: the arena slot is the whole cost.
    let (doc, bytes, allocs) = measure(|| {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        for _ in 0..N {
            let e = doc.create_element("span");
            doc.append_child(root, e).unwrap();
        }
        doc.shrink_to_fit();
        doc
    });
    let per_element = bytes as f64 / N as f64;
    println!(
        "  empty element:            {per_element:6.1} B/node, {allocs} live allocations total"
    );
    assert!(per_element <= 57.0, "empty element costs {per_element} B");
    assert!(
        allocs < 10,
        "empty elements allocate individually ({allocs})"
    );
    drop(doc);

    // Text nodes: arena slot plus the string.
    let (doc, bytes, allocs) = measure(|| {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        for _ in 0..N {
            let t = doc.create_text("0123456789");
            doc.append_child(root, t).unwrap();
        }
        doc.shrink_to_fit();
        doc
    });
    let per_text = bytes as f64 / N as f64;
    println!(
        "  10-byte text node:        {per_text:6.1} B/node, {:.2} allocations/node",
        allocs as f64 / N as f64
    );
    assert!(per_text <= 67.0, "text node costs {per_text} B");
    drop(doc);

    // Elements with two short attributes.
    let (doc, bytes, allocs) = measure(|| {
        let mut doc = Document::new();
        let root = doc.create_element("div");
        for _ in 0..N {
            let e = doc.create_element("a");
            doc.set_attribute(e, "href", "/x");
            doc.set_attribute(e, "class", "y");
            doc.append_child(root, e).unwrap();
        }
        doc.shrink_to_fit();
        doc
    });
    let per_attr_el = bytes as f64 / N as f64;
    println!(
        "  element with 2 attrs:     {per_attr_el:6.1} B/node, {:.2} allocations/node",
        allocs as f64 / N as f64
    );
    assert!(
        per_attr_el <= 56.0 + 2.0 * 32.0 + 3.0 + 2.0,
        "attributed element costs {per_attr_el} B"
    );
    drop(doc);

    // A parsed, listing-style page: what a scrape actually builds.
    let mut html =
        String::from("<!DOCTYPE html><html><head><title>List</title></head><body><main>");
    for i in 0..N / 5 {
        html.push_str(&format!(
            "<div class=\"item\" id=\"i{i}\"><a href=\"/p/{i}\">Item {i}</a>\
             <span class=\"price\">${i}.99</span></div>"
        ));
    }
    html.push_str("</main></body></html>");
    let (doc, bytes, allocs) = measure(|| Document::parse(&html));
    let nodes = doc.len();
    let text_bytes: usize = doc
        .descendants(doc.root())
        .filter_map(|n| doc.text(n))
        .map(str::len)
        .sum();
    let attr_bytes: usize = doc
        .descendants(doc.root())
        .flat_map(|n| doc.attrs(n))
        .map(|a| a.value().len())
        .sum();
    let per_node = bytes as f64 / nodes as f64;
    let overhead = (bytes as usize - text_bytes - attr_bytes) as f64 / nodes as f64;
    println!(
        "  parsed listing page:      {per_node:6.1} B/node over {nodes} nodes \
         ({overhead:.1} B/node excluding text and attribute values), \
         {:.2} allocations/node; source HTML {} B, DOM {} B",
        allocs as f64 / nodes as f64,
        html.len(),
        bytes
    );
    assert!(
        overhead <= 90.0,
        "parsed page costs {overhead} B/node of overhead"
    );
    drop(doc);

    // Dropping a document frees everything it allocated.
    let ((), bytes, allocs) = measure(|| drop(Document::parse(&html)));
    assert_eq!((bytes, allocs), (0, 0), "dropping a Document leaks");
}
