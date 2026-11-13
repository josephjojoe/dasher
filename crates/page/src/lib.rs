//! Page lifecycle for dasher.
//!
//! A [`Page`] owns one document, one JS runtime and its network session.
//! This crate orchestrates loading: fetching the document, parsing it,
//! running scripts in order, driving the event loop until the page settles,
//! and triggering scrolling and lazy-loading. Dropping a page frees
//! everything it owns.
//!
//! # Loading
//!
//! [`Page::load`] does what a browser does for a navigation, minus
//! rendering:
//!
//! 1. It fetches the document through the [`Session`](dasher_net::Session),
//!    following redirects; the page's URL is the final one.
//! 2. It works out the encoding: a byte order mark, then the
//!    `Content-Type` charset, then a `<meta charset>` in the first 1024
//!    bytes. Undeclared documents are read as UTF-8 unless they are not
//!    valid UTF-8, then as windows-1252. `text/plain`, JSON and other text
//!    types are shown in a `<pre>`, as browsers display them.
//! 3. It parses the body as it streams in, with scripting enabled (so
//!    `<noscript>` content is inert text), pausing at each `</script>`:
//!    - inline classic scripts run at once, with `document.currentScript`
//!      set. Markup they `document.write()` is parsed next, before the rest
//!      of the page;
//!    - external scripts without `async` or `defer` block the parser until
//!      they have loaded and run;
//!    - `defer` scripts run after parsing, in document order;
//!    - `async` scripts run as soon as they load;
//!    - module scripts (`type="module"`, inline or external) are deferred:
//!      they run after parsing, in document order together with `defer`
//!      scripts, once their whole module graph has loaded. With `async`
//!      they run as soon as it has. Their imports, dynamic `import()` and
//!      `<link rel=modulepreload>` hints load through the page's session
//!      (see `dasher_js`'s module docs);
//!    - inline import maps (`type="importmap"`) apply to the module
//!      specifiers resolved after them.
//!
//!    Data blocks (`type="text/template"` and the like), `nomodule`
//!    classic scripts and scripts in `<template>` never run.
//! 4. Scripts that scripts insert are loaded too: by default they run as
//!    soon as they load, and those with `async = false` in insertion order.
//!    External scripts fire `load` at their element after running, and
//!    `error` if they could not be fetched (or answered with a non-2xx
//!    status), which is also logged to the console. A module script whose
//!    graph could not be fetched fires `error`, inline or not.
//! 5. Then, as in "the end" of the HTML standard: `readyState` goes to
//!    `interactive`, deferred scripts and modules run, `DOMContentLoaded`
//!    fires, the async scripts are waited for, `readyState` goes to
//!    `complete`, and `load` and `pageshow` fire at the window.
//!
//! While loading, the event loop runs whenever the loader waits for the
//! network, so timers, `fetch()` and XHR make progress as they would in a
//! browser. Page time does not skip ahead in [`ClockMode::Virtual`] until
//! loading is over.
//!
//! Relative script URLs resolve against the document's base URL, which
//! honours `<base href>`. `document.cookie` reads and writes the session's
//! cookie jar, the same one requests use. Script requests send cookies
//! unless the element has `crossorigin`.
//!
//! When a script asks to navigate (`location.href = …`, `assign`,
//! `replace`, `reload`), loading stops and the new document is loaded in
//! its place with a fresh JS runtime, up to
//! [`PageOptions::max_navigations`] times. Navigations to anything but
//! `http(s)` URLs are ignored. [`PageOptions::timeout`] bounds the whole
//! load; a page that runs out of time is returned as far as it got.
//!
//! Declarative refreshes, `<meta http-equiv="refresh" content="0;
//! url=/next">` and the `Refresh` response header, are navigations too,
//! counted against the same budget. As in browsers, the first valid one
//! wins (the header before any meta) and its delay starts once the `load`
//! event has fired. `load` then keeps the page running until the refresh
//! comes due and loads its target. The delay is page time, so under
//! [`ClockMode::Virtual`] it costs no real time. Only short delays are
//! followed, those up to [`PageOptions::max_refresh_delay`] (10 seconds by
//! default), and a refresh that reloads the same document only when it is
//! immediate: longer delays and delayed self-reloads are the auto-refresh
//! of pages that poll for updates, and are ignored (with a console
//! message). Meta elements are looked for when parsing ends and again
//! after `load`; ones scripts insert later are not followed.
//!
//! A page that goes over a resource limit while loading (it fills its JS
//! heap, or it or its scripts build more DOM nodes than
//! [`RuntimeOptions::max_dom_nodes`](dasher_js::RuntimeOptions::max_dom_nodes)
//! allows) is dropped, and loading fails with [`LoadError::LimitExceeded`].
//! Later, [`Page::settle`], [`Page::auto_scroll`] and [`Page::run`] report
//! it as their outcome; the page runs no more JS after that.
//!
//! After loading, [`Page::run`] keeps the event loop going (still loading
//! inserted scripts), and [`Page::follow_navigation`] follows navigations
//! requested later.
//!
//! # Settling
//!
//! The `load` event says little about whether a client-rendered page has
//! its content. [`Page::settle`] keeps the page running until the
//! conditions in [`SettleOptions`] hold together, within a timeout:
//!
//! - network idle: no more than N requests in flight for a quiet period
//!   ([`NetworkIdle`]);
//! - DOM quiet: scripts have not changed the document for a while;
//! - a CSS selector matches;
//! - a JavaScript predicate is truthy.
//!
//! The default waits for both kinds of quiet, 500ms each, for up to 10s.
//! Quiet periods are page time, so under the virtual clock a page that is
//! only waiting on timers settles without real waiting.
//!
//! # Scrolling and lazy content
//!
//! Nothing is laid out, but layout queries get synthetic answers chosen so
//! that lazy content loads (see [`LayoutMode`]). By default every element
//! is in view: `IntersectionObserver` reports it intersecting as soon as it
//! is observed, so lazy images and sections load during the settle, and
//! the document is tall enough to scroll. [`Page::auto_scroll`] then works
//! through infinite feeds: it scrolls to the bottom a viewport at a time,
//! settles, and repeats until scrolling brings nothing new or a limit is
//! hit. Feeds that listen for `scroll` events and ones that watch a
//! sentinel element with `IntersectionObserver` both keep loading.
//!
//! [`ClockMode::Virtual`]: dasher_js::ClockMode::Virtual
//! [`LayoutMode`]: dasher_js::LayoutMode
//!
//! ```no_run
//! use dasher_net::Client;
//! use dasher_page::{AutoScrollOptions, Page, PageOptions, SettleOptions};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let session = Client::new()?.session();
//! let mut page = Page::load(&session, "https://example.com/", PageOptions::default()).await?;
//! page.settle(&SettleOptions::default()).await?;
//! page.auto_scroll(&AutoScrollOptions::default()).await?;
//! let title = page.js().eval_json("document.title")?;
//! let html = page.html();
//! # let _ = (title, html);
//! # Ok(())
//! # }
//! ```

mod budget;
mod charset;
mod page;
mod refresh;
mod script;
mod scroll;
mod settle;

pub use budget::{
    DEFAULT_CPU_BUDGET, DEFAULT_PAGE_TIME_BUDGET, DEFAULT_REQUEST_BUDGET, WorkBudget,
};
pub use dasher_js::Budget;
pub use page::{DEFAULT_MAX_NAVIGATIONS, DEFAULT_MAX_REFRESH_DELAY, LoadError, Page, PageOptions};
pub use scroll::{AutoScrollOptions, AutoScrollReport, AutoScrollStop, DEFAULT_MAX_SCROLLS};
pub use settle::{
    DEFAULT_QUIET, DEFAULT_SETTLE_TIMEOUT, NetworkIdle, SettleError, SettleOptions, SettleOutcome,
};
