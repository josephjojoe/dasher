//! JavaScript for dasher.
//!
//! The QuickJS runtime (via rquickjs), the event loop (tasks, microtasks and
//! timers) and the bindings that expose the DOM and browser globals to page
//! scripts. Layout-dependent APIs return plausible synthetic values so that
//! content gated on visibility or scrolling still loads.
//!
//! # Runtime and event loop
//!
//! Each page gets its own [`JsRuntime`]: one QuickJS runtime and context,
//! with a memory limit, a stack limit and an interrupt handler that stops
//! runaway scripts at a [deadline](JsRuntime::set_deadline) or on request
//! through an [`InterruptHandle`]. Exceptions, unhandled promise rejections
//! and out-of-memory conditions are captured as [`ScriptError`]s, never
//! propagated into the host. As in browsers, uncaught exceptions are first
//! fired at the global as `error` events and unhandled rejections as
//! `unhandledrejection` events. A page that cancels the event has handled
//! the error, so it is not logged. A rejection that gets a handler after
//! it was reported fires `rejectionhandled`.
//!
//! The event loop runs tasks (due timers and host tasks) one at a time, each
//! followed by a microtask checkpoint, and animation frame callbacks on a
//! virtual frame clock. It can be stepped synchronously with
//! [`run_ready`](JsRuntime::run_ready) and [`advance`](JsRuntime::advance),
//! or driven with [`run`](JsRuntime::run) under Tokio, which waits for
//! timers and for work posted from other threads through a [`TaskSender`].
//! [`ClockMode::Virtual`] lets the loop skip idle gaps so long timers do not
//! hold up a page that is settling.
//!
//! Page scripts get `setTimeout`, `setInterval`, `clearTimeout`,
//! `clearInterval`, `queueMicrotask`, `requestAnimationFrame`,
//! `cancelAnimationFrame`, `reportError`, `performance` and a `console`
//! whose output is captured (see [`JsRuntime::take_console`]). `Date` reads
//! the same page clock as `performance.now()`.
//!
//! DOM events are implemented in JS glue evaluated after the prelude:
//! `EventTarget`, `Event`, `CustomEvent`, `ErrorEvent`,
//! `PromiseRejectionEvent`, `DOMException`, `AbortController` and
//! `AbortSignal`. Each glue file (listed in order in
//! `src/glue.rs`) receives the host functions and a shared `internals`
//! object for the glue that follows it. `internals` provides trusted event
//! dispatch, `on*` attributes, abort hooks, task queueing, URL parsing,
//! structured cloning and the document URL.
//!
//! ```
//! use std::time::Duration;
//!
//! use dasher_js::{ClockMode, JsRuntime, RuntimeOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let js = JsRuntime::new(RuntimeOptions {
//!     clock: ClockMode::Manual,
//!     ..RuntimeOptions::default()
//! })?;
//! js.run_script("setTimeout(() => console.log('later', 42), 5000);", "page.js")?;
//! js.advance(Duration::from_secs(5))?;
//! assert_eq!(js.take_console()[0].text, "later 42");
//! assert_eq!(js.eval_json("performance.now()")?.as_deref(), Some("5000"));
//! # Ok(())
//! # }
//! ```
//!
//! # Window and environment
//!
//! The global object is a `Window` (an `EventTarget`), aliased as `window`,
//! `self`, `frames`, `parent` and `top`. [`RuntimeOptions::window`]
//! configures what pages see:
//!
//! - `location`, which reads [`WindowOptions::url`]. Assigning to it or
//!   calling `assign`, `replace` or `reload` does not navigate. It records
//!   a [`NavigationRequest`] for the page layer, and [`JsRuntime::run`]
//!   then returns [`RunOutcome::NavigationRequested`]. Fragment changes,
//!   `history.pushState`/`replaceState` and history traversal stay in the
//!   document, fire `hashchange`/`popstate`, and are tracked by
//!   [`JsRuntime::url`].
//! - `navigator`, `screen`, `innerWidth` and the rest of the viewport,
//!   `devicePixelRatio` and `matchMedia`. These give static answers from
//!   [`WindowOptions`] and [`Viewport`], and default to desktop Chrome on
//!   Linux at 1920×1080. The scroll position moves when scripts scroll
//!   (`scrollTo`, `scrollBy`, `scrollTop`, `scrollIntoView`), firing
//!   `scroll` events.
//! - `localStorage` and `sessionStorage`, kept in memory per origin in a
//!   [`StorageStore`] that pages of one session can share.
//! - `URL`, `URLSearchParams`, `TextEncoder`, `TextDecoder`, `atob`,
//!   `btoa`, `crypto.getRandomValues`/`randomUUID`/`subtle.digest`,
//!   `structuredClone`, `postMessage`, `MessageChannel` and
//!   `requestIdleCallback`.
//! - Streams: `ReadableStream`, `WritableStream`, `TransformStream`, their
//!   readers, writers and controllers, the queuing strategies,
//!   `TextEncoderStream` and `TextDecoderStream`. Byte streams work as
//!   default streams of `Uint8Array` chunks; BYOB readers are not
//!   supported. `response.body` and `blob.stream()` are readable streams.
//! - `Intl` (`NumberFormat`, `DateTimeFormat`, `PluralRules`, `Collator`,
//!   `RelativeTimeFormat`, `ListFormat`), which QuickJS lacks, and the
//!   `toLocaleString`/`localeCompare` methods built on it. Every locale
//!   formats as en-US, and time zones other than UTC and fixed offsets
//!   use the host's zone. It is evaluated on first use, so pages that
//!   never format anything don't carry it in their heap.
//!
//! Everything that is only a stand-in (dialogs, `ResizeObserver`,
//! `WebSocket`, the rest of `crypto.subtle`, navigator extras and so on) is
//! defined in `src/stubs.js`. Its header lists every stub, plus the APIs
//! left out on purpose (workers, `indexedDB`, `caches`).
//!
//! The JS side is compiled to bytecode once per process. Call
//! [`precompile`] at start-up to keep that cost off the first page.
//!
//! ```
//! use dasher_js::{JsRuntime, NavigationKind, RuntimeOptions, WindowOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let js = JsRuntime::new(RuntimeOptions {
//!     window: WindowOptions {
//!         url: "https://example.com/list?page=1".into(),
//!         ..WindowOptions::default()
//!     },
//!     ..RuntimeOptions::default()
//! })?;
//! js.run_script("history.pushState({}, '', '?page=2'); localStorage.seen = '1';", "app.js")?;
//! assert_eq!(js.url(), "https://example.com/list?page=2");
//! js.run_script("location.href = '/detail/7';", "app.js")?;
//! let request = js.take_navigation_request().unwrap();
//! assert_eq!(request.url, "https://example.com/detail/7");
//! assert_eq!(request.kind, NavigationKind::Push);
//! # Ok(())
//! # }
//! ```
//!
//! # The DOM
//!
//! Each runtime holds the page's [`dasher_dom::Document`], an empty
//! `about:blank` one until the host replaces it with
//! [`set_document`](JsRuntime::set_document). The host reads it with
//! [`with_document`](JsRuntime::with_document), e.g. to serialize the page
//! or run selectors, and may change it with
//! [`with_document_mut`](JsRuntime::with_document_mut).
//!
//! Scripts see the standard tree API: `Node` and its subclasses, with the
//! HTML element interfaces they test with `instanceof` (`HTMLDivElement`,
//! `HTMLInputElement`, `SVGSVGElement`...), `document`, `Text`, `Comment`,
//! `DocumentFragment`, attributes, `classList`, `dataset`, live
//! `childNodes`/`children`/`getElementsBy*` collections,
//! `querySelector*`/`matches`/`closest` (via `dasher-css`),
//! `innerHTML`/`outerHTML`/`innerText`, the tree mutation methods, form
//! control state, `TreeWalker`, and event propagation from nodes through
//! `document` to `window`. Inline `style` is parsed from and written back to
//! the `style` attribute; `getComputedStyle` reports it over per-element
//! defaults, with no cascade.
//!
//! `attachShadow` gives open and closed shadow roots that scripts can fill
//! and query, reached again through `shadowRoot`; composed events cross
//! from the shadow tree to its host. Shadow trees are not part of the
//! document's HTML, slots are never assigned, events are not retargeted,
//! and `<template shadowrootmode>` stays an ordinary template.
//!
//! `document.implementation.createHTMLDocument()` makes a second, inert
//! document in the page's arena for building markup off the page (jQuery
//! parses HTML strings in one). Its scripts never run and it has no URL,
//! window or cookies. Nodes move between it and the page freely; a
//! detached node's `ownerDocument` is always the page's document.
//!
//! Geometry is synthetic, chosen so content gated on visibility or
//! scrolling still loads; [`WindowOptions::layout`] picks how (see
//! [`LayoutMode`]). By default every rendered element is a visible,
//! non-zero box at the top of the viewport, the document is as tall as its
//! content and scrolls without limit, and `IntersectionObserver` reports
//! every observed element as intersecting. [`LayoutMode::Scrolled`] stacks
//! the elements in document order instead, so they intersect only once
//! scrolled into view. `ResizeObserver` reports each element's synthetic
//! size once.
//!
//! `MutationObserver` sees every change scripts make to the tree, with the
//! standard's records (`childList`, `attributes` and `characterData`, with
//! `subtree`, `attributeFilter` and old values) delivered in a microtask.
//! Changes the host makes through
//! [`with_document_mut`](JsRuntime::with_document_mut) are not observed.
//! The same changes, when made to nodes in the document, are counted for
//! the host as [`DomActivity`] (see [`JsRuntime::dom_activity`]), so the
//! page layer can tell when a page has stopped changing.
//!
//! Inline classic script elements that a script inserts into the document
//! run as they are inserted. Other inserted scripts are left alone unless a
//! page loader takes them over (see below). Stylesheet and preload links
//! that scripts insert are never fetched, but get a `load` event as if
//! they had loaded empty, since scripts wait for it.
//!
//! Every node keeps one JS wrapper for as long as script can reach it, so
//! identity and expando properties are stable, but wrappers do not keep
//! detached subtrees alive: once a detached subtree has no reachable
//! wrapper, its nodes are freed from the arena. The `dom` module docs
//! explain how.
//!
//! ```
//! use dasher_dom::Document;
//! use dasher_js::{JsRuntime, RuntimeOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let js = JsRuntime::new(RuntimeOptions::default())?;
//! js.set_document(Document::parse("<ul id=list><li>one</li></ul>"));
//! js.run_script(
//!     "const li = document.createElement('li');
//!      li.textContent = 'two';
//!      document.getElementById('list').append(li);",
//!     "page.js",
//! )?;
//! assert_eq!(js.eval_json("document.querySelectorAll('li').length")?.as_deref(), Some("2"));
//! let html = js.with_document(|doc| doc.outer_html(doc.body().unwrap()));
//! assert_eq!(html, r#"<body><ul id="list"><li>one</li><li>two</li></ul></body>"#);
//! # Ok(())
//! # }
//! ```
//!
//! # Page loader hooks
//!
//! The page loader in `dasher-page` fetches and parses the document and
//! decides when scripts run. It drives the JS side through:
//!
//! - [`set_ready_state`](JsRuntime::set_ready_state) for
//!   `document.readyState` (firing `readystatechange`) and
//!   [`fire_event`](JsRuntime::fire_event) for `DOMContentLoaded`, `load`,
//!   `pageshow` and the `load`/`error` events of script elements;
//! - [`run_script_element`](JsRuntime::run_script_element), which runs a
//!   script's source with `document.currentScript` set;
//! - [`take_over_inserted_scripts`](JsRuntime::take_over_inserted_scripts),
//!   after which external and module scripts that scripts insert are
//!   queued as [`InsertedScript`]s for the loader instead of being ignored.
//!   Each holds a pending operation, so the event loop does not go idle or
//!   skip ahead in virtual time before the loader has dealt with it;
//! - [`set_document_write`](JsRuntime::set_document_write), which makes
//!   `document.write()` capture markup for the parser while a
//!   parser-executed script runs, and ignore it from async scripts;
//! - [`set_document_info`](JsRuntime::set_document_info) for
//!   `document.characterSet`, `contentType` and `referrer`;
//! - [`hold_detached_nodes`](JsRuntime::hold_detached_nodes), which keeps
//!   node ids the parser still uses valid while scripts change the tree;
//! - the module graph functions (see [ES modules](#es-modules)),
//!   [`register_import_map`](JsRuntime::register_import_map) and
//!   [`link_inserted`](JsRuntime::link_inserted) for parser-inserted
//!   modulepreload links.
//!
//! # ES modules
//!
//! Module scripts, static `import`/`export … from`, dynamic `import()`,
//! `import.meta` (`url` and `resolve`), import maps and
//! `<link rel=modulepreload>` all work, with module sources fetched
//! through the runtime's session. The host starts a module script's graph
//! with [`fetch_module_graph`](JsRuntime::fetch_module_graph) (or
//! [`fetch_inline_module_graph`](JsRuntime::fetch_inline_module_graph)),
//! waits for it with [`module_graph_status`](JsRuntime::module_graph_status)
//! and [`module_graph_settled`](JsRuntime::module_graph_settled), and runs
//! it with [`run_module_graph`](JsRuntime::run_module_graph).
//!
//! QuickJS links modules synchronously, so each graph is fetched in full
//! first: every module is scanned for its static imports, which are
//! resolved (relative to the importing module's final URL, or through the
//! import map) and fetched in parallel. A module's source is dropped once
//! QuickJS has compiled it. For the same reason, scripts' `import(` calls
//! are renamed, before they are compiled, to a hidden global that fetches
//! the requested graph asynchronously. `import()` in code that is not
//! scanned (`eval`, `new Function`) still works, but blocks the page while
//! each missing module is fetched.
//!
//! Module scripts are decoded as UTF-8 whatever their `Content-Type` says,
//! and the MIME type is not checked. A failed fetch is logged to the
//! console and fails the graph; an exception a module throws is reported
//! like any uncaught exception.
//!
//! # Network
//!
//! `fetch()` (with `Headers`, `Request`, `Response`, `Blob`, `File`,
//! `FormData` and `AbortSignal` support) and `XMLHttpRequest` send their
//! requests through the [`dasher_net::Session`] given as
//! [`RuntimeOptions::network`]. That way they share the page's cookie jar
//! and request policy, and they count towards the session's network-idle
//! tracking. Requests run as Tokio tasks and are cancelled when the runtime
//! is dropped. While one is in progress the event loop does not report
//! itself idle, and the virtual clock does not skip ahead. Relative URLs
//! resolve against the document's URL when they are sent, and the default
//! `same-origin` credentials mode sends cookies only to the document's own
//! origin. Without a session every request fails as a network error, except
//! `data:` URLs, which are always answered locally. Failed loads are logged
//! to the console with the reason.
//!
//! ```no_run
//! use dasher_js::{JsRuntime, RunLimits, RuntimeOptions};
//! use dasher_net::Client;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let session = Client::new()?.session();
//! let js = JsRuntime::new(RuntimeOptions {
//!     network: Some(session.clone()),
//!     ..RuntimeOptions::default()
//! })?;
//! js.run_script(
//!     "fetch('https://example.com/api').then((r) => r.json()).then((d) => globalThis.data = d);",
//!     "page.js",
//! )?;
//! js.run(RunLimits::default()).await;
//! let data = js.eval_json("data")?;
//! # let _ = data;
//! # Ok(())
//! # }
//! ```

mod clock;
mod console;
mod data_url;
mod dom;
mod encoding;
mod error;
mod event_loop;
mod glue;
mod heap;
mod loader;
mod log;
mod media;
mod modules;
mod net;
mod random;
mod remote;
mod run;
mod runtime;
mod storage;
mod timers;
mod urls;
mod window;

pub use clock::ClockMode;
pub use console::{ConsoleLevel, ConsoleMessage};
pub use dom::DomActivity;
pub use error::{ErrorKind, ScriptError};
pub use event_loop::{DEFAULT_FRAME_INTERVAL, LoopHandle, Stopped, Task};
pub use glue::precompile;
pub use loader::{DocumentInfo, DocumentWrite, EventTarget, InsertedScript, ReadyState};
pub use modules::{ModuleCredentials, ModuleGraph, ModuleGraphStatus};
pub use remote::{PendingOperation, SendTask, TaskSender};
pub use run::{RunLimits, RunOutcome};
pub use runtime::{
    DEFAULT_MAX_STACK_SIZE, DEFAULT_MEMORY_LIMIT, InterruptHandle, JsRuntime, RuntimeOptions,
};
pub use storage::{DEFAULT_STORAGE_QUOTA, QuotaExceeded, StorageStore};
pub use window::{
    DEFAULT_USER_AGENT, LayoutMode, NavigationKind, NavigationRequest, Viewport, WindowOptions,
};

/// The rquickjs version dasher is built against, for host bindings.
pub use rquickjs;
