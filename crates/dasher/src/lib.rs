//! dasher: a minimal headless browser for scraping.
//!
//! Load a URL, run the page's JavaScript well enough for client-rendered
//! content, infinite scroll and lazy-loading to fill in the DOM, then get back
//! the resulting HTML or values pulled out with selectors or JS.
//!
//! This is the public API; the work is done by the `dasher-*` crates.
