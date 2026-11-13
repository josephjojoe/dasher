//! dasher: a minimal headless browser for scraping.
//!
//! Load a URL, run the page's JavaScript well enough for client-rendered
//! content, infinite scroll and lazy-loading to fill in the DOM, then get
//! back the resulting HTML, or values pulled out with selectors or JS.
//! Nothing is rendered: images, stylesheets, fonts and media are never
//! downloaded, and layout queries get synthetic answers chosen so that
//! content gated behind visibility or scrolling still loads.
//!
//! - A [`Browser`] holds what pages share: configuration (user agent,
//!   viewport, headers, proxy, blocked URLs), the connection pool, the
//!   cookie jar and `localStorage`. Build one per process; it is cheap to
//!   clone and can be shared between threads.
//! - A [`Page`] is a tab. [`Page::goto`] loads a document, runs its
//!   scripts and waits for it to settle; then [`content`](Page::content)
//!   gives the HTML, [`evaluate`](Page::evaluate) runs JavaScript and
//!   returns JSON, and [`text`](Page::text),
//!   [`attributes`](Page::attributes) and friends pull values out with
//!   CSS selectors. Each page owns its JS runtime and DOM, and dropping
//!   it frees them.
//!
//! Pages are not `Send`: drive them from a current-thread Tokio runtime,
//! or a `LocalSet`, on the thread that opened them.
//!
//! # Getting the rendered HTML
//!
//! ```no_run
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> dasher::Result<()> {
//!     let browser = dasher::Browser::new()?;
//!     let mut page = browser.new_page();
//!     let navigation = page.goto("https://example.com/").await?;
//!     println!("{} {}", navigation.status, navigation.url);
//!     println!("{}", page.content());
//!     Ok(())
//! }
//! ```
//!
//! # Pulling out values
//!
//! ```no_run
//! # async fn example() -> dasher::Result<()> {
//! # let browser = dasher::Browser::new()?;
//! let mut page = browser.open("https://shop.example/products").await?;
//!
//! // With selectors, straight from the DOM.
//! let names: Vec<String> = page.texts(".product .name")?;
//! let links: Vec<String> = page.attributes(".product a", "href")?;
//!
//! // With JavaScript: functions are called and promises awaited.
//! let prices: Vec<f64> = page
//!     .evaluate_as("[...document.querySelectorAll('.price')].map(e => parseFloat(e.textContent))")
//!     .await?;
//! let stock = page
//!     .evaluate("async () => (await fetch('/api/stock')).json()")
//!     .await?;
//! # let _ = (names, links, prices, stock);
//! # Ok(())
//! # }
//! ```
//!
//! The selector helpers, [`Page::wait_for_selector`] and [`Page::content`]
//! all see into shadow trees (open or closed), so content rendered by web
//! components is found like any other: `my-card .price` matches a `.price`
//! in `my-card`'s shadow tree, text reads as the components render, and
//! the HTML holds each shadow tree as declarative shadow DOM
//! (`<template shadowrootmode>`). Scripts in [`Page::evaluate`] keep the
//! browser's view, where `querySelector` stops at shadow roots.
//!
//! # Waiting for content
//!
//! By default [`goto`](Page::goto) waits for the `load` event, then until
//! the network has been idle and the DOM unchanged for 500ms (see
//! [`SettleOptions`]). [`GotoOptions`] chooses other conditions, and
//! [`Page::wait_for_selector`] and [`Page::wait_for_function`] wait for
//! something specific. For infinite feeds, auto-scroll after settling:
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use dasher::{AutoScrollOptions, GotoOptions, SettleOptions};
//!
//! # async fn example(page: &mut dasher::Page) -> dasher::Result<()> {
//! let options = GotoOptions::settle(SettleOptions {
//!     selector: Some(".feed-item".into()),
//!     ..SettleOptions::default()
//! })
//! .with_auto_scroll(AutoScrollOptions {
//!     max_scrolls: 10,
//!     ..AutoScrollOptions::default()
//! })
//! .with_timeout(Duration::from_secs(60));
//! let navigation = page.goto_with("https://social.example/feed", &options).await?;
//! println!("scrolled {} times", navigation.scroll.map_or(0, |s| s.scrolls));
//! # Ok(())
//! # }
//! ```
//!
//! # Memory
//!
//! Each page has two limits: its JS heap
//! ([`BrowserBuilder::memory_limit`], 256 MiB by default) and the number of
//! nodes its DOM may hold ([`BrowserBuilder::max_dom_nodes`], 500,000).
//! A page that goes over one is stopped at once, its document is freed,
//! and the operation that was running fails with
//! [`Error::LimitExceeded`], so one runaway page cannot take the process
//! down. [`Page::memory_usage`] shows how close a page is. A page's memory
//! is all returned when it is dropped or navigates elsewhere.
//!
//! # Time
//!
//! Pages run on page time. By default it is [virtual](ClockMode::Virtual):
//! whenever a page is only waiting on timers, time skips ahead to the next
//! one, so `setTimeout(render, 2000)` costs no real time and quiet periods
//! pass at once unless the network is busy. Choose
//! [`ClockMode::Realtime`] with [`BrowserBuilder::clock`] for pages that
//! misbehave under it.

mod browser;
mod error;
mod page;
mod text;

pub use browser::{Browser, BrowserBuilder, DEFAULT_TIMEOUT};
pub use error::{Error, Result};
pub use page::{GotoOptions, MemoryUsage, Navigation, Page};

pub use dasher_js::{
    ClockMode, ConsoleLevel, ConsoleMessage, DEFAULT_MAX_DOM_NODES, DEFAULT_MEMORY_LIMIT,
    ErrorKind as ScriptErrorKind, ResourceLimit, ScriptError, Viewport, Visibility, precompile,
};
pub use dasher_net::{Cookie, SameSite};
pub use dasher_page::{
    AutoScrollOptions, AutoScrollReport, AutoScrollStop, Budget, DEFAULT_CPU_BUDGET,
    DEFAULT_MAX_SCROLLS, DEFAULT_PAGE_TIME_BUDGET, DEFAULT_QUIET, DEFAULT_REQUEST_BUDGET,
    DEFAULT_SETTLE_TIMEOUT, LoadError, NetworkIdle, SettleOptions, SettleOutcome, WorkBudget,
};
/// The JSON value [`Page::evaluate`] returns, from `serde_json`.
pub use serde_json::Value as JsonValue;
