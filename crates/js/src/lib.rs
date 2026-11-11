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
//!   and has no upper bound.
//! - `localStorage` and `sessionStorage`, kept in memory per origin in a
//!   [`StorageStore`] that pages of one session can share.
//! - `URL`, `URLSearchParams`, `TextEncoder`, `TextDecoder`, `atob`,
//!   `btoa`, `crypto.getRandomValues`/`randomUUID`, `structuredClone`,
//!   `postMessage`, `MessageChannel` and `requestIdleCallback`.
//!
//! Everything that is only a stand-in (dialogs, `ResizeObserver`,
//! `WebSocket`, `crypto.subtle`, navigator extras and so on) is defined in
//! `src/stubs.js`. Its header lists every stub, plus the APIs left out on
//! purpose (workers, `indexedDB`, `caches`, `Intl`).
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
mod encoding;
mod error;
mod event_loop;
mod glue;
mod heap;
mod log;
mod media;
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
pub use error::{ErrorKind, ScriptError};
pub use event_loop::{DEFAULT_FRAME_INTERVAL, LoopHandle, Stopped, Task};
pub use glue::precompile;
pub use remote::{PendingOperation, SendTask, TaskSender};
pub use run::{RunLimits, RunOutcome};
pub use runtime::{
    DEFAULT_MAX_STACK_SIZE, DEFAULT_MEMORY_LIMIT, InterruptHandle, JsRuntime, RuntimeOptions,
};
pub use storage::{DEFAULT_STORAGE_QUOTA, QuotaExceeded, StorageStore};
pub use window::{DEFAULT_USER_AGENT, NavigationKind, NavigationRequest, Viewport, WindowOptions};

/// The rquickjs version dasher is built against, for host bindings.
pub use rquickjs;
