//! Page lifecycle for dasher.
//!
//! A `Page` owns one document, one JS runtime and its network session. This
//! crate orchestrates loading: fetching the document, parsing it, running
//! scripts in order, driving the event loop until the page settles, and
//! triggering scrolling and lazy-loading. Dropping a page frees everything it
//! owns.
